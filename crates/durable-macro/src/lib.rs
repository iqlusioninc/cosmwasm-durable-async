//! Compile explicit owned checkpoints into synchronous durable workflow handlers.

use proc_macro::TokenStream;

mod expand;
mod parse;

/// Define a versioned workflow using top-level durable waits and explicit checkpoints.
#[proc_macro_attribute]
pub fn durable_workflow(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let item = syn::parse_macro_input!(item as syn::ItemFn);
    match parse::parse(attributes.into(), item) {
        Ok(workflow) => expand::expand(workflow).into(),
        Err(error) => error.into_compile_error().into(),
    }
}

#[cfg(test)]
mod tests {
    use quote::quote;

    fn validate(
        body: proc_macro2::TokenStream,
    ) -> Result<crate::parse::ParsedWorkflow, syn::Error> {
        crate::parse::parse(
            quote!(kind = "sample", version = 1),
            syn::parse2(quote! {
                async fn sample(ctx: WorkflowCtx, count: u32) -> Result<u32, WaitError> { #body }
            })
            .unwrap(),
        )
    }

    #[test]
    fn parser_accepts_two_waits_and_empty_checkpoint() {
        let workflow = validate(quote! {
            let first: u32 = ctx.wait::<NumberWait>(count).checkpoint(count).await?;
            let second: Result<u32, WaitError> = ctx.wait::<NumberWait>(count + first).checkpoint().await;
            Ok(second?)
        }).unwrap();
        assert_eq!(workflow.waits.len(), 2);
        assert_eq!(workflow.waits[0].checkpoint.len(), 1);
        assert!(workflow.waits[1].checkpoint.is_empty());
    }

    #[test]
    fn parser_rejects_implicit_checkpoint_type() {
        let error = validate(quote! {
            let value = 2;
            let first: u32 = ctx.wait::<NumberWait>(value).checkpoint(value).await?;
            Ok(first)
        })
        .err()
        .unwrap();
        assert!(error.to_string().contains("explicit type"));
    }

    #[test]
    fn parser_rejects_nested_await() {
        let error = validate(quote! {
            let first: u32 = 1 + ctx.wait::<NumberWait>(count).checkpoint().await?;
            Ok(first)
        })
        .err()
        .unwrap();
        assert!(error.to_string().contains("entire right-hand side"));
    }

    #[test]
    fn parser_rejects_context_checkpoint() {
        let error = validate(quote! {
            let first: u32 = ctx.wait::<NumberWait>(count).checkpoint(ctx).await?;
            Ok(first)
        })
        .err()
        .unwrap();
        assert!(error.to_string().contains("context"));
    }

    #[test]
    fn parser_rejects_destructured_checkpoint() {
        let error = validate(quote! {
            let (value, other): (u32, u32) = (1, 2);
            let first: u32 = ctx.wait::<NumberWait>(count).checkpoint(value).await?;
            Ok(first)
        })
        .err()
        .unwrap();
        assert!(error.to_string().contains("simple local"));
    }

    #[test]
    fn parser_rejects_shadowed_context() {
        let result = validate(quote! {
            let ctx: u32 = 2;
            let first: u32 = ctx.wait::<NumberWait>(ctx).checkpoint().await?;
            Ok(first)
        });
        assert!(
            result.is_err(),
            "a shadowed name cannot retain runtime capabilities"
        );
    }

    #[test]
    fn parser_rejects_duplicate_input_names() {
        let function = syn::parse2(quote! {
            async fn sample(ctx: WorkflowCtx, count: u32, count: String) -> Result<u32, WaitError> { Ok(1) }
        }).unwrap();
        let result = crate::parse::parse(quote!(kind = "sample", version = 1), function);
        assert!(
            result.is_err(),
            "arguments must retain normal Rust uniqueness rules"
        );
    }
}
