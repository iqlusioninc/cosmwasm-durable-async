use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};

use crate::parse::{Binding, ParsedWorkflow, Wait};

pub(crate) fn expand(workflow: ParsedWorkflow) -> TokenStream {
    let ParsedWorkflow {
        item,
        kind,
        version,
        context,
        inputs,
        output,
        error,
        waits,
        segments,
        flow,
    } = workflow;
    let source_context_type = match item.sig.inputs.first().unwrap() {
        syn::FnArg::Typed(argument) => &argument.ty,
        syn::FnArg::Receiver(_) => unreachable!("the parser rejects methods"),
    };
    let name = item.sig.ident;
    let state_type = state_ident(&name);
    let types = SegmentTypes {
        output: &output,
        error: &error,
        state: &state_type,
    };
    let visibility = item.vis;
    let workflow_visibility = match &visibility {
        syn::Visibility::Inherited => quote!(pub(super)),
        syn::Visibility::Public(_) => quote!(pub),
        syn::Visibility::Restricted(restricted) => {
            let mut path = (*restricted.path).clone();
            if path.is_ident("self") {
                quote!(pub(super))
            } else {
                if path
                    .segments
                    .first()
                    .is_some_and(|segment| segment.ident == "super")
                {
                    path.segments.insert(0, syn::parse_quote!(super));
                }
                quote!(pub(in #path))
            }
        }
    };
    let attributes = item.attrs;
    let input_fields = inputs.iter().map(|binding| {
        let ident = &binding.ident;
        let ty = &binding.ty;
        quote!(pub #ident: #ty)
    });
    let variants: Vec<_> = (0..waits.len())
        .map(|index| format_ident!("Waiting{}", index + 1))
        .collect();
    let state_variants = waits.iter().zip(&variants).map(|(wait, variant)| {
        let fields = wait.checkpoint.iter().map(|binding| {
            let ident = &binding.ident;
            let ty = &binding.ty;
            quote!(#ident: #ty)
        });
        if wait.child {
            let child = child_ident(wait);
            quote!(#variant { #(#fields,)* #child: ::durable_runtime::ChildContinuation })
        } else {
            quote!(#variant { #(#fields),* })
        }
    });
    let binary_input = hidden("__durable_input");
    let binary_state = hidden("__durable_state");
    let resolution = hidden("__durable_resolution");
    let require_persistable = hidden("__durable_require_persistable");
    let input_pattern = inputs.iter().map(|binding| {
        let ident = &binding.ident;
        let mutable = binding.mutable.then(|| quote!(mut));
        quote!(#mutable #ident)
    });
    let start_prelude =
        quote!(#[allow(unused_mut)] let Input { #(#input_pattern),* } = #binary_input;);
    let start_body = if let Some(graph) = &flow {
        graph.execute(0, start_prelude, &context, &waits, &variants, &types)
    } else {
        segment(
            &context,
            &segments[0],
            start_prelude,
            waits.first(),
            variants.first(),
            &types,
        )
    };
    let resume_arms = waits.iter().enumerate().map(|(index, wait)| {
        let variant = &variants[index];
        let awaited_value = hidden("__durable_awaited_value");
        let operation = &wait.operation;
        let restored = wait.checkpoint.iter().enumerate().map(|(field_index, binding)| {
            let ident = &binding.ident;
            let temporary = format_ident!("__durable_restored_{}", field_index, span = Span::mixed_site());
            quote!(#ident: #temporary)
        });
        let declarations = restore_bindings(&wait.checkpoint);
        let local_attributes = &wait.local.attrs;
        let local_pattern = &wait.local.pat;
        let bound_value = if wait.propagate { quote!(#awaited_value?) } else { quote!(#awaited_value) };
        let prelude = quote! {
            #declarations
            #[allow(unused_mut)]
            #(#local_attributes)* let #local_pattern = #bound_value;
        };
        let body = if let Some(graph) = &flow { graph.execute(graph.resumes[index], prelude, &context, &waits, &variants, &types) } else { segment(&context, &segments[index + 1], prelude, waits.get(index + 1), variants.get(index + 1), &types) };
        if wait.child {
            let child = child_ident(wait);
            let transition = hidden("__durable_transition");
            let prepared = hidden("__durable_prepared");
            let child_state = hidden("__durable_child_state");
            let fields = wait.checkpoint.iter().enumerate().map(|(i, b)| {
                let name = &b.ident;
                let tmp = format_ident!("__durable_restored_{}", i, span = Span::mixed_site());
                quote!(#name: #tmp)
            });
            return quote! {
                #state_type::#variant { #(#restored,)* #child } => {
                    #child.check::<#operation>()?;
                    <#operation as ::durable_runtime::Workflow>::validate(&#child.state, &#resolution)?;
                    let #transition = <#operation as ::durable_runtime::Workflow>::resume(#context, #child.state, #resolution)?;
                    if let ::durable_runtime::Transition::Wait { state: #child_state, wait: #prepared } = #transition {
                        let #child = ::durable_runtime::ChildContinuation::new::<#operation>(#child_state);
                        return ::core::result::Result::Ok(::durable_runtime::Transition::Wait {
                            state: ::durable_runtime::cosmwasm_std::to_json_binary(&#state_type::#variant { #(#fields,)* #child })?,
                            wait: #prepared,
                        });
                    }
                    let #awaited_value = ::durable_runtime::decode_child_result::<#operation>(#transition)?;
                    #body
                }
            };
        }
        quote! {
            #state_type::#variant { #(#restored),* } => {
                let #awaited_value = ::durable_runtime::decode_resolution::<#operation>(#resolution)?;
                #body
            }
        }
    });
    let validation_arms = waits.iter().zip(&variants).map(|(wait, variant)| {
        let operation = &wait.operation;
        if wait.child {
            let child = child_ident(wait);
            return quote! { #state_type::#variant { #child, .. } => { #child.check::<#operation>()?; <#operation as ::durable_runtime::Workflow>::validate(&#child.state, #resolution) } };
        }
        quote! { #state_type::#variant { .. } => { let _ = ::durable_runtime::decode_resolution::<#operation>(#resolution.clone())?; ::core::result::Result::Ok(()) } }
    });
    let operation_arms = waits.iter().zip(&variants).map(|(wait, variant)| {
        let operation = &wait.operation;
        if wait.child {
            let child = child_ident(wait);
            return quote! { #state_type::#variant { #child, .. } => { #child.check::<#operation>()?; <#operation as ::durable_runtime::Workflow>::operation(&#child.state) } };
        }
        quote! { #state_type::#variant { .. } => ::core::result::Result::Ok(<#operation as ::durable_runtime::Operation>::KIND) }
    });
    let depths = waits.iter().filter(|w| w.child).map(|w| {
        let child = &w.operation;
        quote! { let child_depth = <#child as ::durable_runtime::TypedWorkflow>::COMPOSITION_DEPTH + 1; if child_depth > depth { depth = child_depth; } }
    });
    quote! {
        #(#attributes)*
        #visibility mod #name {
            #[allow(unused_imports)]
            use super::*;

            #[derive(::durable_runtime::serde::Serialize, ::durable_runtime::serde::Deserialize)]
            #[serde(crate = "::durable_runtime::serde")]
            pub struct Input { #(#input_fields),* }

            #[derive(::durable_runtime::serde::Serialize, ::durable_runtime::serde::Deserialize)]
            #[serde(crate = "::durable_runtime::serde")]
            enum #state_type { #(#state_variants),* }

            #workflow_visibility struct Workflow;

            impl ::durable_runtime::TypedWorkflow for Workflow {
                type Output = #output;
                type Error = #error;
                const COMPOSITION_DEPTH: usize = {
                    #[allow(unused_mut)] let mut depth = 0;
                    #(#depths)*
                    assert!(depth <= 16, "durable helper composition exceeds maximum depth 16");
                    depth
                };
            }
            const _: usize = <Workflow as ::durable_runtime::TypedWorkflow>::COMPOSITION_DEPTH;

            impl ::durable_runtime::Workflow for Workflow {
                const KIND: &'static str = #kind;
                const VERSION: u32 = #version;
                type Input = Input;

                fn start(#context: &mut ::durable_runtime::WorkflowCtx<'_>, #binary_input: ::durable_runtime::cosmwasm_std::Binary) -> ::durable_runtime::RuntimeResult<::durable_runtime::Transition> {
                    let _: &#source_context_type = &*#context;
                    {
                        fn #require_persistable<T: ::durable_runtime::serde::Serialize + ::durable_runtime::serde::de::DeserializeOwned>() {}
                        #require_persistable::<#output>();
                        #require_persistable::<#error>();
                    }
                    let #binary_input: Input = ::durable_runtime::cosmwasm_std::from_json(#binary_input)?;
                    #start_body
                }

                fn validate(#binary_state: &::durable_runtime::cosmwasm_std::Binary, #resolution: &::durable_runtime::Resolution) -> ::durable_runtime::RuntimeResult<()> {
                    let _ = &#resolution;
                    let #binary_state: #state_type = ::durable_runtime::cosmwasm_std::from_json(#binary_state)?;
                    match #binary_state { #(#validation_arms),* }
                }

                fn operation(#binary_state: &::durable_runtime::cosmwasm_std::Binary) -> ::durable_runtime::RuntimeResult<&'static str> {
                    let #binary_state: #state_type = ::durable_runtime::cosmwasm_std::from_json(#binary_state)?;
                    match #binary_state { #(#operation_arms),* }
                }

                fn resume(#context: &mut ::durable_runtime::WorkflowCtx<'_>, #binary_state: ::durable_runtime::cosmwasm_std::Binary, #resolution: ::durable_runtime::Resolution) -> ::durable_runtime::RuntimeResult<::durable_runtime::Transition> {
                    let _ = &#context;
                    let _ = &#resolution;
                    let #binary_state: #state_type = ::durable_runtime::cosmwasm_std::from_json(#binary_state)?;
                    match #binary_state { #(#resume_arms),* }
                }
            }
        }
    }
}

fn hidden(name: &str) -> syn::Ident {
    syn::Ident::new(name, Span::mixed_site())
}

pub(crate) fn child_ident(wait: &Wait) -> syn::Ident {
    let mut name = "__durable_child".to_owned();
    while wait.checkpoint.iter().any(|binding| binding.ident == name) {
        name.push('_');
    }
    hidden(&name)
}

pub(crate) fn state_ident(function: &syn::Ident) -> syn::Ident {
    let hash = function.to_string().bytes().fold(0u64, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u64::from(byte))
    });
    format_ident!("__DurableContinuation{}", hash, span = Span::mixed_site())
}

fn restore_bindings(bindings: &[Binding]) -> TokenStream {
    let declarations = bindings.iter().enumerate().map(|(index, binding)| {
        let ident = &binding.ident;
        let ty = &binding.ty;
        let mutable = binding.mutable.then(|| quote!(mut));
        let temporary = format_ident!("__durable_restored_{}", index, span = Span::mixed_site());
        quote! {
            #[allow(unused_variables, unused_mut)]
            let #mutable #ident: #ty = #temporary;
        }
    });
    quote!(#(#declarations)*)
}

pub(crate) struct SegmentTypes<'a> {
    pub output: &'a syn::Type,
    pub error: &'a syn::Type,
    pub state: &'a syn::Ident,
}

fn segment(
    context: &syn::Ident,
    statements: &[syn::Stmt],
    prelude: TokenStream,
    wait: Option<&Wait>,
    variant: Option<&syn::Ident>,
    types: &SegmentTypes<'_>,
) -> TokenStream {
    let SegmentTypes {
        output,
        error,
        state: state_type,
    } = types;
    let statements = statements
        .iter()
        .map(|statement| {
            if let syn::Stmt::Local(local) = statement {
                let pattern = if let syn::Pat::Type(pattern) = &local.pat {
                    pattern.pat.as_ref()
                } else {
                    &local.pat
                };
                if matches!(pattern, syn::Pat::Ident(pattern) if pattern.mutability.is_some()) {
                    return quote!(#[allow(unused_mut)] #statement);
                }
            }
            quote!(#statement)
        })
        .collect::<Vec<_>>();
    let result = hidden("__durable_segment_result");
    let value = hidden("__durable_value");
    let application_error = hidden("__durable_application_error");
    if let Some(wait) = wait {
        let variant = variant.unwrap();
        let operation = &wait.operation;
        let request = &wait.request;
        let fields = wait.checkpoint.iter().map(|binding| &binding.ident);
        let request_value = hidden("__durable_request");
        let prepared = hidden("__durable_prepared");
        let state = hidden("__durable_next_state");
        quote! {
            let #result: ::core::result::Result<(#state_type, <#operation as ::durable_runtime::Operation>::Request), #error> = (|| {
                #prelude
                #(#statements)*
                let #request_value: <#operation as ::durable_runtime::Operation>::Request = #request;
                ::core::result::Result::Ok((#state_type::#variant { #(#fields),* }, #request_value))
            })();
            match #result {
                ::core::result::Result::Ok((#state, #request_value)) => {
                    let #prepared = #context.prepare::<#operation>(#request_value)?;
                    let #state = ::durable_runtime::cosmwasm_std::to_json_binary(&#state)?;
                    ::core::result::Result::Ok(::durable_runtime::Transition::Wait { state: #state, wait: #prepared })
                }
                ::core::result::Result::Err(#application_error) => ::core::result::Result::Ok(::durable_runtime::Transition::Failed(::durable_runtime::cosmwasm_std::to_json_binary(&#application_error)?)),
            }
        }
    } else {
        quote! {
            let #result: ::core::result::Result<#output, #error> = (|| { #prelude #(#statements)* })();
            match #result {
                ::core::result::Result::Ok(#value) => ::core::result::Result::Ok(::durable_runtime::Transition::Completed(::durable_runtime::cosmwasm_std::to_json_binary(&#value)?)),
                ::core::result::Result::Err(#application_error) => ::core::result::Result::Ok(::durable_runtime::Transition::Failed(::durable_runtime::cosmwasm_std::to_json_binary(&#application_error)?)),
            }
        }
    }
}
