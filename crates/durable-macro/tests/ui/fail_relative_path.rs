use durable_macro::durable_workflow;
fn number() -> u32 { 99 }
mod nested {
    use super::durable_workflow;
    use durable_runtime::{WorkflowCtx, WaitError};
    fn number() -> u32 { 7 }
    #[durable_workflow(kind = "relative", version = 1)]
    async fn relative(ctx: WorkflowCtx) -> Result<u32, WaitError> {
        Ok(super::number())
    }
}
fn main() {}
