use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::TokenStream;
use syn::{
    parse::Parser,
    punctuated::Punctuated,
    spanned::Spanned,
    visit::{self, Visit},
    Expr, FnArg, GenericArgument, Ident, ItemFn, Lit, LitInt, LitStr, Local, Meta, Pat,
    PathArguments, ReturnType, Stmt, Token, Type,
};

#[derive(Clone)]
pub(crate) struct Binding {
    pub ident: Ident,
    pub ty: Type,
    pub mutable: bool,
}

pub(crate) struct Wait {
    pub operation: Type,
    pub request: Expr,
    pub checkpoint: Vec<Binding>,
    pub local: Local,
    pub propagate: bool,
}

pub(crate) struct ParsedWorkflow {
    pub item: ItemFn,
    pub kind: LitStr,
    pub version: LitInt,
    pub context: Ident,
    pub inputs: Vec<Binding>,
    pub output: Type,
    pub error: Type,
    pub waits: Vec<Wait>,
    pub segments: Vec<Vec<Stmt>>,
}

#[derive(Clone)]
struct LocalInfo {
    ident: Ident,
    ty: Option<Type>,
    mutable: bool,
    simple: bool,
}

pub(crate) fn parse(attributes: TokenStream, mut item: ItemFn) -> syn::Result<ParsedWorkflow> {
    let (kind, version) = attributes_options(attributes)?;
    anchor_source_paths(&mut item)?;
    if item.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            item.sig.fn_token.span,
            "durable workflows must be async functions",
        ));
    }
    if !item.sig.generics.params.is_empty() || item.sig.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &item.sig.generics,
            "generic workflow functions are not supported",
        ));
    }
    if item.sig.constness.is_some()
        || item.sig.unsafety.is_some()
        || item.sig.abi.is_some()
        || item.sig.variadic.is_some()
    {
        return Err(syn::Error::new_spanned(
            &item.sig,
            "durable workflows must use ordinary safe Rust function signatures",
        ));
    }
    let (output, error) = result_types(&item.sig.output)?;
    let mut args = item.sig.inputs.iter();
    let context_argument = args.next().ok_or_else(|| {
        syn::Error::new_spanned(&item.sig, "first argument must be ctx: WorkflowCtx")
    })?;
    let FnArg::Typed(context_argument) = context_argument else {
        return Err(syn::Error::new_spanned(
            context_argument,
            "methods cannot be durable workflows",
        ));
    };
    reject_conditional_attributes(&context_argument.attrs)?;
    let context = simple_pattern(&context_argument.pat)?.0.clone();
    reserved_binding(&context)?;
    if !matches!(context_argument.ty.as_ref(), Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "WorkflowCtx"))
    {
        return Err(syn::Error::new_spanned(
            &context_argument.ty,
            "first argument must have type WorkflowCtx",
        ));
    }
    let mut inputs = Vec::new();
    let mut locals = BTreeMap::new();
    for argument in args {
        let FnArg::Typed(argument) = argument else {
            return Err(syn::Error::new_spanned(
                argument,
                "methods cannot be durable workflows",
            ));
        };
        reject_conditional_attributes(&argument.attrs)?;
        let (ident, mutable) = simple_pattern(&argument.pat)?;
        reserved_binding(ident)?;
        if ident == &context || locals.contains_key(&ident.to_string()) {
            return Err(syn::Error::new_spanned(
                ident,
                "workflow argument names must be unique",
            ));
        }
        owned_type(&argument.ty)?;
        let binding = Binding {
            ident: ident.clone(),
            ty: (*argument.ty).clone(),
            mutable,
        };
        locals.insert(
            ident.to_string(),
            LocalInfo {
                ident: ident.clone(),
                ty: Some(binding.ty.clone()),
                mutable,
                simple: true,
            },
        );
        inputs.push(binding);
    }
    let mut waits = Vec::new();
    let mut segments = vec![Vec::new()];
    let mut omitted = BTreeSet::new();
    for statement in &item.block.stmts {
        if let Stmt::Local(local) = statement {
            reject_conditional_attributes(&local.attrs)?;
            for name in pattern_names(&local.pat) {
                reserved_binding(&name)?;
            }
            if pattern_names(&local.pat)
                .iter()
                .any(|name| name == &context)
            {
                return Err(syn::Error::new_spanned(
                    &local.pat,
                    "the reserved workflow context cannot be shadowed",
                ));
            }
            if local
                .init
                .as_ref()
                .is_some_and(|init| init.diverge.is_some())
            {
                return Err(syn::Error::new_spanned(
                    local,
                    "let-else declarations are not supported in durable workflows",
                ));
            }
            if let Some((awaited, propagate)) = direct_await(local) {
                simple_pattern(&local.pat)?;
                let (operation, request, names) = wait_expression(&awaited.base, &context)?;
                restricted_expression(&request)?;
                check_omitted_expression(&request, &omitted)?;
                let mut checkpoint = Vec::new();
                let mut unique = BTreeSet::new();
                for name in names {
                    if name == context {
                        return Err(syn::Error::new_spanned(
                            name,
                            "workflow context cannot be checkpointed",
                        ));
                    }
                    if !unique.insert(name.to_string()) {
                        return Err(syn::Error::new_spanned(
                            name,
                            "checkpoint identifiers must be unique",
                        ));
                    }
                    let info = locals.get(&name.to_string()).ok_or_else(|| {
                        syn::Error::new_spanned(
                            &name,
                            "checkpoint must name a local in the current segment",
                        )
                    })?;
                    if !info.simple {
                        return Err(syn::Error::new_spanned(
                            name,
                            "checkpoint requires a simple local declaration",
                        ));
                    }
                    let ty = info.ty.clone().ok_or_else(|| {
                        syn::Error::new_spanned(
                            &name,
                            "checkpointed locals require an explicit type annotation",
                        )
                    })?;
                    owned_type(&ty)?;
                    checkpoint.push(Binding {
                        ident: info.ident.clone(),
                        ty,
                        mutable: info.mutable,
                    });
                }
                let retained: BTreeSet<_> = checkpoint
                    .iter()
                    .map(|binding| binding.ident.to_string())
                    .collect();
                omitted.extend(
                    locals
                        .keys()
                        .filter(|name| !retained.contains(*name))
                        .cloned(),
                );
                for name in pattern_names(&local.pat) {
                    omitted.remove(&name.to_string());
                }
                locals = checkpoint
                    .iter()
                    .map(|binding| {
                        (
                            binding.ident.to_string(),
                            LocalInfo {
                                ident: binding.ident.clone(),
                                ty: Some(binding.ty.clone()),
                                mutable: binding.mutable,
                                simple: true,
                            },
                        )
                    })
                    .collect();
                record_local(local, &mut locals);
                waits.push(Wait {
                    operation,
                    request,
                    checkpoint,
                    local: local.clone(),
                    propagate,
                });
                segments.push(Vec::new());
                continue;
            }
        }
        let mut omitted_names = OmittedNames {
            omitted: omitted.clone(),
            error: None,
        };
        omitted_names.visit_stmt(statement);
        omitted_names.finish()?;
        let mut restriction = Restrictions::default();
        restriction.visit_stmt(statement);
        restriction.finish()?;
        if let Stmt::Local(local) = statement {
            record_local(local, &mut locals);
            for name in pattern_names(&local.pat) {
                omitted.remove(&name.to_string());
            }
        }
        segments.last_mut().unwrap().push(statement.clone());
    }
    Ok(ParsedWorkflow {
        item,
        kind,
        version,
        context,
        inputs,
        output,
        error,
        waits,
        segments,
    })
}

fn attributes_options(attributes: TokenStream) -> syn::Result<(LitStr, LitInt)> {
    let options = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(attributes)?;
    let mut kind = None;
    let mut version = None;
    for option in options {
        let Meta::NameValue(option) = option else {
            return Err(syn::Error::new_spanned(
                option,
                "expected kind = \"name\", version = 1",
            ));
        };
        if option.path.is_ident("kind") && kind.is_none() {
            if let Expr::Lit(value) = &option.value {
                if let Lit::Str(value) = &value.lit {
                    if !value.value().is_empty() {
                        kind = Some(value.clone());
                        continue;
                    }
                }
            }
            return Err(syn::Error::new_spanned(
                option.value,
                "workflow kind must be a nonempty string literal",
            ));
        }
        if option.path.is_ident("version") && version.is_none() {
            if let Expr::Lit(value) = &option.value {
                if let Lit::Int(value) = &value.lit {
                    if value.base10_parse::<u32>().is_ok_and(|value| value > 0) {
                        version = Some(value.clone());
                        continue;
                    }
                }
            }
            return Err(syn::Error::new_spanned(
                option.value,
                "workflow version must be a positive u32 integer literal",
            ));
        }
        return Err(syn::Error::new_spanned(
            option,
            "unknown or duplicate workflow option",
        ));
    }
    Ok((
        kind.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                "explicit workflow kind is required",
            )
        })?,
        version.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                "explicit workflow version is required",
            )
        })?,
    ))
}

fn result_types(output: &ReturnType) -> syn::Result<(Type, Type)> {
    if let ReturnType::Type(_, ty) = output {
        if let Type::Path(path) = ty.as_ref() {
            if let Some(segment) = path.path.segments.last() {
                if segment.ident == "Result" {
                    if let PathArguments::AngleBracketed(arguments) = &segment.arguments {
                        if let [GenericArgument::Type(output), GenericArgument::Type(error)] =
                            arguments.args.iter().collect::<Vec<_>>().as_slice()
                        {
                            return Ok(((*output).clone(), (*error).clone()));
                        }
                    }
                }
            }
        }
    }
    Err(syn::Error::new_spanned(
        output,
        "durable workflows must return Result<Output, Error>",
    ))
}

fn simple_pattern(pattern: &Pat) -> syn::Result<(&Ident, bool)> {
    let pattern = if let Pat::Type(pattern) = pattern {
        pattern.pat.as_ref()
    } else {
        pattern
    };
    if let Pat::Ident(pattern) = pattern {
        if pattern.by_ref.is_none() && pattern.subpat.is_none() {
            return Ok((&pattern.ident, pattern.mutability.is_some()));
        }
    }
    Err(syn::Error::new_spanned(
        pattern,
        "durable bindings require a simple local identifier",
    ))
}

fn record_local(local: &Local, locals: &mut BTreeMap<String, LocalInfo>) {
    let ty = if let Pat::Type(pattern) = &local.pat {
        Some((*pattern.ty).clone())
    } else {
        None
    };
    if let Ok((ident, mutable)) = simple_pattern(&local.pat) {
        locals.insert(
            ident.to_string(),
            LocalInfo {
                ident: ident.clone(),
                ty,
                mutable,
                simple: true,
            },
        );
    } else {
        for ident in pattern_names(&local.pat) {
            locals.insert(
                ident.to_string(),
                LocalInfo {
                    ident,
                    ty: None,
                    mutable: false,
                    simple: false,
                },
            );
        }
    }
}

fn pattern_names(pattern: &Pat) -> Vec<Ident> {
    struct Names(Vec<Ident>);
    impl<'a> Visit<'a> for Names {
        fn visit_pat_ident(&mut self, pattern: &'a syn::PatIdent) {
            self.0.push(pattern.ident.clone());
            visit::visit_pat_ident(self, pattern);
        }
    }
    let mut names = Names(Vec::new());
    names.visit_pat(pattern);
    names.0
}

fn reserved_binding(ident: &Ident) -> syn::Result<()> {
    if matches!(ident.to_string().as_str(), "Input" | "Workflow") {
        return Err(syn::Error::new_spanned(
            ident,
            "Input and Workflow are reserved export names; use another local identifier",
        ));
    }
    Ok(())
}

fn anchor_source_paths(item: &mut ItemFn) -> syn::Result<()> {
    struct Anchor {
        state_name: String,
        error: Option<syn::Error>,
    }
    impl syn::visit_mut::VisitMut for Anchor {
        fn visit_pat_ident_mut(&mut self, pattern: &mut syn::PatIdent) {
            if let Err(error) = reserved_binding(&pattern.ident) {
                if self.error.is_none() {
                    self.error = Some(error);
                }
            }
            syn::visit_mut::visit_pat_ident_mut(self, pattern);
        }
        fn visit_path_mut(&mut self, path: &mut syn::Path) {
            if path.leading_colon.is_none() {
                if let Some(first) = path.segments.first() {
                    if first.ident == "self" || first.ident == "super" {
                        if self.error.is_none() {
                            self.error = Some(syn::Error::new_spanned(path, "self:: and super:: paths are not supported inside a durable workflow; use crate:: or an imported name"));
                        }
                        return;
                    }
                    if first.ident == "Input"
                        || first.ident == "Workflow"
                        || first.ident == self.state_name
                    {
                        path.segments.insert(0, syn::parse_quote!(super));
                    }
                }
            }
            syn::visit_mut::visit_path_mut(self, path);
        }
    }
    let mut anchor = Anchor {
        state_name: crate::expand::state_ident(&item.sig.ident).to_string(),
        error: None,
    };
    syn::visit_mut::VisitMut::visit_item_fn_mut(&mut anchor, item);
    anchor.error.map_or(Ok(()), Err)
}

fn reject_conditional_attributes(attributes: &[syn::Attribute]) -> syn::Result<()> {
    for attribute in attributes {
        if attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr") {
            return Err(syn::Error::new_spanned(attribute, "conditional compilation on workflow arguments or statements is not supported; conditionally compile the whole workflow"));
        }
    }
    Ok(())
}

fn check_omitted_expression(expression: &Expr, omitted: &BTreeSet<String>) -> syn::Result<()> {
    let mut names = OmittedNames {
        omitted: omitted.clone(),
        error: None,
    };
    names.visit_expr(expression);
    names.finish()
}

struct OmittedNames {
    omitted: BTreeSet<String>,
    error: Option<syn::Error>,
}
impl OmittedNames {
    fn shadow(&mut self, pattern: &Pat) {
        for ident in pattern_names(pattern) {
            self.omitted.remove(&ident.to_string());
        }
    }
    fn finish(self) -> syn::Result<()> {
        self.error.map_or(Ok(()), Err)
    }
}
impl<'a> Visit<'a> for OmittedNames {
    fn visit_expr_path(&mut self, expression: &'a syn::ExprPath) {
        if expression.qself.is_none() {
            if let Some(ident) = expression.path.get_ident() {
                if self.omitted.contains(&ident.to_string()) && self.error.is_none() {
                    self.error = Some(syn::Error::new_spanned(ident, "local was omitted from the checkpoint; checkpoint it or declare it again before use"));
                }
            }
        }
        visit::visit_expr_path(self, expression);
    }
    fn visit_local(&mut self, local: &'a Local) {
        if let Some(init) = &local.init {
            self.visit_expr(&init.expr);
            if let Some((_, diverge)) = &init.diverge {
                self.visit_expr(diverge);
            }
        }
        self.shadow(&local.pat);
    }
    fn visit_block(&mut self, block: &'a syn::Block) {
        let original = self.omitted.clone();
        for statement in &block.stmts {
            self.visit_stmt(statement);
        }
        self.omitted = original;
    }
    fn visit_expr_closure(&mut self, expression: &'a syn::ExprClosure) {
        let original = self.omitted.clone();
        for argument in &expression.inputs {
            self.shadow(argument);
        }
        self.visit_expr(&expression.body);
        self.omitted = original;
    }
    fn visit_arm(&mut self, arm: &'a syn::Arm) {
        let original = self.omitted.clone();
        self.shadow(&arm.pat);
        if let Some((_, guard)) = &arm.guard {
            self.visit_expr(guard);
        }
        self.visit_expr(&arm.body);
        self.omitted = original;
    }
    fn visit_expr_if(&mut self, expression: &'a syn::ExprIf) {
        if let Expr::Let(condition) = expression.cond.as_ref() {
            self.visit_expr(&condition.expr);
            let original = self.omitted.clone();
            self.shadow(&condition.pat);
            self.visit_block(&expression.then_branch);
            self.omitted = original;
            if let Some((_, otherwise)) = &expression.else_branch {
                self.visit_expr(otherwise);
            }
        } else {
            visit::visit_expr_if(self, expression);
        }
    }
}

fn direct_await(local: &Local) -> Option<(&syn::ExprAwait, bool)> {
    match local.init.as_ref()?.expr.as_ref() {
        Expr::Await(awaited) => Some((awaited, false)),
        Expr::Try(tried) => {
            if let Expr::Await(awaited) = tried.expr.as_ref() {
                Some((awaited, true))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn wait_expression(expression: &Expr, context: &Ident) -> syn::Result<(Type, Expr, Vec<Ident>)> {
    let invalid = || {
        syn::Error::new_spanned(
            expression,
            "only ctx.wait::<Operation>(request).checkpoint(locals...).await is supported",
        )
    };
    let Expr::MethodCall(checkpoint) = expression else {
        return Err(invalid());
    };
    if checkpoint.method != "checkpoint" || checkpoint.turbofish.is_some() {
        return Err(invalid());
    }
    let Expr::MethodCall(wait) = checkpoint.receiver.as_ref() else {
        return Err(invalid());
    };
    if wait.method != "wait" || wait.args.len() != 1 {
        return Err(invalid());
    }
    if !matches!(wait.receiver.as_ref(), Expr::Path(path) if path.path.is_ident(context)) {
        return Err(invalid());
    }
    let Some(arguments) = &wait.turbofish else {
        return Err(invalid());
    };
    if arguments.args.len() != 1 {
        return Err(invalid());
    }
    let Some(GenericArgument::Type(operation @ Type::Path(_))) = arguments.args.first() else {
        return Err(invalid());
    };
    let mut names = Vec::new();
    for argument in &checkpoint.args {
        if let Expr::Path(path) = argument {
            if let Some(ident) = path.path.get_ident() {
                names.push(ident.clone());
                continue;
            }
        }
        return Err(syn::Error::new_spanned(
            argument,
            "checkpoint accepts only local identifiers",
        ));
    }
    Ok((operation.clone(), wait.args.first().unwrap().clone(), names))
}

fn owned_type(ty: &Type) -> syn::Result<()> {
    #[derive(Default)]
    struct OwnedType(Option<syn::Error>);
    impl<'a> Visit<'a> for OwnedType {
        fn visit_type_reference(&mut self, ty: &'a syn::TypeReference) {
            self.0 = Some(syn::Error::new_spanned(
                ty,
                "checkpoint and input types must be owned; references are not supported",
            ));
        }
        fn visit_type_ptr(&mut self, ty: &'a syn::TypePtr) {
            self.0 = Some(syn::Error::new_spanned(
                ty,
                "raw pointers cannot be persisted",
            ));
        }
        fn visit_lifetime(&mut self, lifetime: &'a syn::Lifetime) {
            self.0 = Some(syn::Error::new_spanned(
                lifetime,
                "borrow-based checkpoint and input types are not supported",
            ));
        }
        fn visit_type_path(&mut self, ty: &'a syn::TypePath) {
            if ty.path.segments.iter().any(|segment| {
                matches!(
                    segment.ident.to_string().as_str(),
                    "WorkflowCtx"
                        | "Deps"
                        | "DepsMut"
                        | "QuerierWrapper"
                        | "Storage"
                        | "Api"
                        | "Querier"
                )
            }) {
                self.0 = Some(syn::Error::new_spanned(
                    ty,
                    "invocation capabilities cannot be checkpointed",
                ));
            }
            visit::visit_type_path(self, ty);
        }
    }
    let mut owned = OwnedType::default();
    owned.visit_type(ty);
    owned.0.map_or(Ok(()), Err)
}

fn restricted_expression(expression: &Expr) -> syn::Result<()> {
    let mut restriction = Restrictions::default();
    restriction.visit_expr(expression);
    restriction.finish()
}

#[derive(Default)]
struct Restrictions {
    error: Option<syn::Error>,
}
impl Restrictions {
    fn reject(&mut self, span: proc_macro2::Span, message: &str) {
        if self.error.is_none() {
            self.error = Some(syn::Error::new(span, message));
        }
    }
    fn finish(self) -> syn::Result<()> {
        self.error.map_or(Ok(()), Err)
    }
}
impl<'a> Visit<'a> for Restrictions {
    fn visit_attribute(&mut self, attribute: &'a syn::Attribute) {
        if let Err(error) = reject_conditional_attributes(std::slice::from_ref(attribute)) {
            if self.error.is_none() {
                self.error = Some(error);
            }
        }
    }
    fn visit_expr_await(&mut self, expression: &'a syn::ExprAwait) {
        self.reject(expression.await_token.span, "await must be the entire right-hand side of a simple local binding; nested awaits are not supported");
    }
    fn visit_expr_for_loop(&mut self, expression: &'a syn::ExprForLoop) {
        self.reject(
            expression.for_token.span,
            "loops are not supported in durable workflows; use a synchronous helper",
        );
    }
    fn visit_expr_while(&mut self, expression: &'a syn::ExprWhile) {
        self.reject(
            expression.while_token.span,
            "loops are not supported in durable workflows; use a synchronous helper",
        );
    }
    fn visit_expr_loop(&mut self, expression: &'a syn::ExprLoop) {
        self.reject(
            expression.loop_token.span,
            "loops are not supported in durable workflows; use a synchronous helper",
        );
    }
    fn visit_expr_async(&mut self, expression: &'a syn::ExprAsync) {
        self.reject(
            expression.async_token.span,
            "async blocks are not supported in durable workflows",
        );
    }
    fn visit_item(&mut self, item: &'a syn::Item) {
        self.reject(item.span(), "nested items are not supported in durable workflows; define synchronous helpers outside the workflow");
    }
    fn visit_macro(&mut self, expression: &'a syn::Macro) {
        fn contains_await(tokens: TokenStream) -> bool {
            tokens.into_iter().any(|token| match token {
                proc_macro2::TokenTree::Ident(ident) => ident == "await",
                proc_macro2::TokenTree::Group(group) => contains_await(group.stream()),
                _ => false,
            })
        }
        if contains_await(expression.tokens.clone()) {
            self.reject(expression.span(), "awaits inside macros are not supported");
        } else {
            self.reject(expression.span(), "macros are not supported inside durable workflows; call a synchronous helper instead");
        }
    }
}
