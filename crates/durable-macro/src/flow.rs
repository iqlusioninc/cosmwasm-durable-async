//! Typed, in-memory control-flow graph. Only wait states cross invocations.
//! Edges own their values; dispatch is iterative, including loop backedges.
use crate::parse::{self, Binding, Wait};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use std::collections::{BTreeMap, BTreeSet};
use syn::{
    visit::{self, Visit},
    Expr, Ident, Pat, Stmt,
};

pub(crate) struct Graph {
    nodes: Vec<Node>,
    pub resumes: Vec<usize>,
}
struct Node {
    bindings: Vec<Binding>,
    statements: Vec<Stmt>,
    end: End,
}
enum End {
    Pending,
    Jump(usize),
    Branch(Expr, usize, usize),
    Wait(usize),
    Complete,
}
#[derive(Clone, Default)]
struct Scope {
    bindings: BTreeMap<String, Binding>,
    omitted: BTreeSet<String>,
}
impl Scope {
    fn values(&self) -> Vec<Binding> {
        self.bindings.values().cloned().collect()
    }
    fn check(&self, expr: &Expr) -> syn::Result<()> {
        parse::check_omitted_expression(expr, &self.omitted)?;
        parse::restricted_expression(expr)?;
        struct Transfers(Option<syn::Error>);
        impl<'a> Visit<'a> for Transfers {
            fn visit_expr_return(&mut self, e: &'a syn::ExprReturn) {
                self.0 = Some(syn::Error::new_spanned(e, "early return is not supported in durable control flow; use a final Result expression"));
            }
            fn visit_expr_break(&mut self, e: &'a syn::ExprBreak) {
                self.0 = Some(syn::Error::new_spanned(
                    e,
                    "break must be a standalone statement in a durable loop",
                ));
            }
            fn visit_expr_continue(&mut self, e: &'a syn::ExprContinue) {
                self.0 = Some(syn::Error::new_spanned(
                    e,
                    "continue must be a standalone statement in a durable loop",
                ));
            }
            fn visit_expr_closure(&mut self, _: &'a syn::ExprClosure) {}
        }
        let mut transfers = Transfers(None);
        transfers.visit_expr(expr);
        transfers.0.map_or(Ok(()), Err)
    }
    fn declare(&mut self, local: &syn::Local, context: &Ident) -> syn::Result<()> {
        parse::reject_conditional_attributes(&local.attrs)?;
        let (ident, mutable) = parse::simple_pattern(&local.pat)?;
        if ident == context {
            return Err(syn::Error::new_spanned(
                ident,
                "the reserved workflow context cannot be shadowed",
            ));
        }
        if self.bindings.contains_key(&ident.to_string())
            || self.omitted.contains(&ident.to_string())
        {
            return Err(syn::Error::new_spanned(
                ident,
                "shadowing is not supported in durable control flow; use a distinct local name",
            ));
        }
        let Pat::Type(pattern) = &local.pat else {
            return Err(syn::Error::new_spanned(
                &local.pat,
                "durable control-flow locals require an explicit type annotation",
            ));
        };
        parse::owned_type(&pattern.ty)?;
        if local
            .init
            .as_ref()
            .is_some_and(|init| init.diverge.is_some())
        {
            return Err(syn::Error::new_spanned(
                local,
                "let-else is not supported in durable control flow",
            ));
        }
        self.bindings.insert(
            ident.to_string(),
            Binding {
                ident: ident.clone(),
                ty: (*pattern.ty).clone(),
                mutable,
            },
        );
        Ok(())
    }
}

pub(crate) fn needed(block: &syn::Block) -> bool {
    struct Detect {
        depth: usize,
        found: bool,
    }
    impl<'a> Visit<'a> for Detect {
        fn visit_expr_if(&mut self, e: &'a syn::ExprIf) {
            self.depth += 1;
            visit::visit_expr_if(self, e);
            self.depth -= 1;
        }
        fn visit_expr_loop(&mut self, _: &'a syn::ExprLoop) {
            self.found = true;
        }
        fn visit_expr_while(&mut self, _: &'a syn::ExprWhile) {
            self.found = true;
        }
        fn visit_expr_await(&mut self, _: &'a syn::ExprAwait) {
            self.found |= self.depth > 0;
        }
    }
    let mut detect = Detect {
        depth: 0,
        found: false,
    };
    detect.visit_block(block);
    detect.found
}

pub(crate) fn lower(
    block: &syn::Block,
    context: &Ident,
    inputs: &[Binding],
) -> syn::Result<(Graph, Vec<Wait>)> {
    let scope = Scope {
        bindings: inputs
            .iter()
            .cloned()
            .map(|b| (b.ident.to_string(), b))
            .collect(),
        omitted: BTreeSet::new(),
    };
    let mut builder = Builder {
        graph: Graph {
            nodes: Vec::new(),
            resumes: Vec::new(),
        },
        waits: Vec::new(),
        context,
    };
    let start = builder.node(&scope);
    builder.sequence(start, &block.stmts, scope, None, None)?;
    Ok((builder.graph, builder.waits))
}
struct Builder<'a> {
    graph: Graph,
    waits: Vec<Wait>,
    context: &'a Ident,
}
impl Builder<'_> {
    fn node(&mut self, scope: &Scope) -> usize {
        let id = self.graph.nodes.len();
        self.graph.nodes.push(Node {
            bindings: scope.values(),
            statements: Vec::new(),
            end: End::Pending,
        });
        id
    }
    fn jump(&mut self, source: usize, target: usize, scope: &Scope) -> syn::Result<()> {
        for binding in &self.graph.nodes[target].bindings {
            if !scope.bindings.contains_key(&binding.ident.to_string()) {
                return Err(syn::Error::new_spanned(&binding.ident, "control-flow edge requires this outer local; include it in every checkpoint on this path"));
            }
        }
        self.graph.nodes[source].end = End::Jump(target);
        Ok(())
    }
    fn sequence(
        &mut self,
        mut current: usize,
        statements: &[Stmt],
        mut scope: Scope,
        next: Option<usize>,
        loop_targets: Option<(usize, usize)>,
    ) -> syn::Result<()> {
        for (position, statement) in statements.iter().enumerate() {
            let rest = &statements[position + 1..];
            if let Stmt::Local(local) = statement {
                if let Some((awaited, propagate)) = parse::direct_await(local) {
                    let (operation, request, names) =
                        parse::wait_expression(&awaited.base, self.context)?;
                    scope.check(&request)?;
                    let mut unique = BTreeSet::new();
                    let mut checkpoint = Vec::new();
                    for name in names {
                        if name == *self.context {
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
                        checkpoint.push(
                            scope
                                .bindings
                                .get(&name.to_string())
                                .cloned()
                                .ok_or_else(|| {
                                    syn::Error::new_spanned(
                                        name,
                                        "checkpoint must name a local in the current segment",
                                    )
                                })?,
                        );
                    }
                    scope.omitted.extend(
                        scope
                            .bindings
                            .keys()
                            .filter(|n| !unique.contains(*n))
                            .cloned(),
                    );
                    scope.bindings.retain(|name, _| unique.contains(name));
                    scope.declare(local, self.context)?;
                    let after = self.node(&scope);
                    let index = self.waits.len();
                    self.waits.push(Wait {
                        operation,
                        request,
                        checkpoint,
                        local: local.clone(),
                        propagate,
                    });
                    self.graph.resumes.push(after);
                    self.graph.nodes[current].end = End::Wait(index);
                    current = after;
                    continue;
                }
                if let Some(init) = &local.init {
                    scope.check(&init.expr)?;
                }
                scope.declare(local, self.context)?;
                self.graph.nodes[current].statements.push(statement.clone());
                continue;
            }
            if let Stmt::Expr(expr, _) = statement {
                match expr {
                    Expr::If(branch) => {
                        if matches!(branch.cond.as_ref(), Expr::Let(_)) {
                            return Err(syn::Error::new_spanned(
                                &branch.cond,
                                "if-let is not supported in durable control flow",
                            ));
                        }
                        parse::reject_conditional_attributes(&branch.attrs)?;
                        scope.check(&branch.cond)?;
                        let join = self.node(&scope);
                        let yes = self.node(&scope);
                        let no = self.node(&scope);
                        self.graph.nodes[current].end =
                            End::Branch((*branch.cond).clone(), yes, no);
                        self.sequence(
                            yes,
                            &branch.then_branch.stmts,
                            scope.clone(),
                            Some(join),
                            loop_targets,
                        )?;
                        match &branch.else_branch {
                            Some((_, otherwise)) => {
                                let alternative = match otherwise.as_ref() {
                                    Expr::Block(block) => block.block.stmts.clone(),
                                    other => vec![Stmt::Expr(other.clone(), None)],
                                };
                                self.sequence(
                                    no,
                                    &alternative,
                                    scope.clone(),
                                    Some(join),
                                    loop_targets,
                                )?;
                            }
                            None => self.jump(no, join, &scope)?,
                        }
                        return self.sequence(join, rest, scope, next, loop_targets);
                    }
                    Expr::While(_) | Expr::Loop(_) => {
                        let (condition, body, label, attrs) = match expr {
                            Expr::While(e) => (Some(e.cond.as_ref()), &e.body, &e.label, &e.attrs),
                            Expr::Loop(e) => (None, &e.body, &e.label, &e.attrs),
                            _ => unreachable!(),
                        };
                        parse::reject_conditional_attributes(attrs)?;
                        if label.is_some() {
                            return Err(syn::Error::new_spanned(
                                expr,
                                "labeled durable loops are not supported",
                            ));
                        }
                        if let Some(condition) = condition {
                            scope.check(condition)?;
                            if matches!(condition, Expr::Let(_)) {
                                return Err(syn::Error::new_spanned(
                                    condition,
                                    "while-let is not supported in durable control flow",
                                ));
                            }
                        }
                        let header = self.node(&scope);
                        let body_node = self.node(&scope);
                        let after = self.node(&scope);
                        self.jump(current, header, &scope)?;
                        self.graph.nodes[header].end = if let Some(condition) = condition {
                            End::Branch(condition.clone(), body_node, after)
                        } else {
                            End::Jump(body_node)
                        };
                        self.sequence(
                            body_node,
                            &body.stmts,
                            scope.clone(),
                            Some(header),
                            Some((header, after)),
                        )?;
                        return self.sequence(after, rest, scope, next, loop_targets);
                    }
                    Expr::Break(e) => {
                        parse::reject_conditional_attributes(&e.attrs)?;
                        if !rest.is_empty() {
                            return Err(syn::Error::new_spanned(&rest[0], "statements after unconditional break are not supported in durable control flow"));
                        }
                        if e.label.is_some() || e.expr.is_some() {
                            return Err(syn::Error::new_spanned(e,"labeled or value-carrying break is not supported in durable control flow"));
                        }
                        let (_, target) = loop_targets.ok_or_else(|| {
                            syn::Error::new_spanned(e, "break requires a durable loop")
                        })?;
                        return self.jump(current, target, &scope);
                    }
                    Expr::Continue(e) => {
                        parse::reject_conditional_attributes(&e.attrs)?;
                        if !rest.is_empty() {
                            return Err(syn::Error::new_spanned(&rest[0], "statements after unconditional continue are not supported in durable control flow"));
                        }
                        if e.label.is_some() {
                            return Err(syn::Error::new_spanned(
                                e,
                                "labeled continue is not supported in durable control flow",
                            ));
                        }
                        let (target, _) = loop_targets.ok_or_else(|| {
                            syn::Error::new_spanned(e, "continue requires a durable loop")
                        })?;
                        return self.jump(current, target, &scope);
                    }
                    _ => scope.check(expr)?,
                }
            } else {
                return Err(syn::Error::new_spanned(
                    statement,
                    "items and macros are not supported in durable control flow",
                ));
            }
            self.graph.nodes[current].statements.push(statement.clone());
        }
        if let Some(next) = next {
            self.jump(current, next, &scope)
        } else {
            self.graph.nodes[current].end = End::Complete;
            Ok(())
        }
    }
}

fn node_ident(index: usize) -> Ident {
    format_ident!("Block{}", index)
}
fn hidden(name: &str) -> Ident {
    Ident::new(name, Span::mixed_site())
}
impl Graph {
    fn edge(&self, index: usize) -> TokenStream {
        let flow = hidden("__durable_flow_type");
        let variant = node_ident(index);
        let names = self.nodes[index].bindings.iter().map(|b| &b.ident);
        quote!(#flow::#variant { #(#names),* })
    }
    pub(crate) fn execute(
        &self,
        entry: usize,
        prelude: TokenStream,
        context: &Ident,
        waits: &[Wait],
        variants: &[Ident],
        types: &crate::expand::SegmentTypes<'_>,
    ) -> TokenStream {
        let crate::expand::SegmentTypes {
            output,
            error,
            state: state_type,
        } = types;
        let flow = hidden("__durable_flow_type");
        let cursor = hidden("__durable_cursor");
        let action = hidden("__durable_action_type");
        let result = hidden("__durable_result");
        let value = hidden("__durable_value");
        let request = hidden("__durable_request");
        let state = hidden("__durable_state");
        let prepared = hidden("__durable_prepared");
        let flow_variants = self.nodes.iter().enumerate().map(|(i, node)| {
            let variant = node_ident(i);
            let fields = node.bindings.iter().map(|b| {
                let name = &b.ident;
                let ty = &b.ty;
                quote!(#name:#ty)
            });
            quote!(#variant { #(#fields),* })
        });
        let action_variants = waits.iter().zip(variants).map(|(wait, variant)| {
            let operation = &wait.operation;
            quote!(#variant(#state_type,<#operation as ::durable_runtime::Operation>::Request))
        });
        let arms = self.nodes.iter().enumerate().map(|(i, node)| {
            let variant = node_ident(i);
            let bindings = node.bindings.iter().map(|binding| {
                let name = &binding.ident;
                let mutable = binding.mutable.then(|| quote!(mut));
                quote!(#mutable #name)
            });
            let statements = &node.statements;
            let end = match &node.end {
                End::Pending => unreachable!(),
                End::Jump(target) => {
                    let edge = self.edge(*target);
                    quote!(#cursor = #edge;)
                }
                End::Branch(condition, yes, no) => {
                    let yes = self.edge(*yes);
                    let no = self.edge(*no);
                    quote!(#cursor = if #condition { #yes } else { #no };)
                }
                End::Wait(index) => {
                    let wait = &waits[*index];
                    let operation = &wait.operation;
                    let expr = &wait.request;
                    let variant = &variants[*index];
                    let names = wait.checkpoint.iter().map(|binding| &binding.ident);
                    quote! {
                        let #request: <#operation as ::durable_runtime::Operation>::Request = #expr;
                        return ::core::result::Result::Ok(#action::#variant(
                            #state_type::#variant { #(#names),* },
                            #request,
                        ));
                    }
                }
                End::Complete => quote!(),
            };
            let body = if matches!(node.end, End::Complete) {
                quote! {
                    let #value: ::core::result::Result<#output, #error> = { #(#statements)* };
                    return ::core::result::Result::Ok(#action::Completed(#value?));
                }
            } else {
                quote!(#(#statements)* #end)
            };
            quote!(#flow::#variant { #(#bindings),* } => { #body })
        });
        let waiting = waits.iter().zip(variants).map(|(wait, variant)| {
            let operation = &wait.operation;
            quote! {
                ::core::result::Result::Ok(#action::#variant(#state, #request)) => {
                    let #prepared = #context.prepare::<#operation>(#request)?;
                    ::core::result::Result::Ok(::durable_runtime::Transition::Wait {
                        state: ::durable_runtime::cosmwasm_std::to_json_binary(&#state)?,
                        wait: #prepared,
                    })
                }
            }
        });
        let initial = self.edge(entry);
        quote! {
            #[allow(non_camel_case_types)]
            enum #flow { #(#flow_variants),* }
            #[allow(non_camel_case_types)]
            enum #action { Completed(#output), #(#action_variants),* }
            #[allow(unused_mut, unused_variables)]
            let #result: ::core::result::Result<#action, #error> = (|| {
                #prelude
                let mut #cursor = #initial;
                loop { match #cursor { #(#arms),* } }
            })();
            match #result {
                ::core::result::Result::Ok(#action::Completed(#value)) => {
                    ::core::result::Result::Ok(::durable_runtime::Transition::Completed(
                        ::durable_runtime::cosmwasm_std::to_json_binary(&#value)?,
                    ))
                }
                ::core::result::Result::Err(#value) => {
                    ::core::result::Result::Ok(::durable_runtime::Transition::Failed(
                        ::durable_runtime::cosmwasm_std::to_json_binary(&#value)?,
                    ))
                }
                #(#waiting),*
            }
        }
    }
}
