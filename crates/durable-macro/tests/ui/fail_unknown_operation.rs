use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind = "bad", version = 1)]
async fn bad(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.wait::<MissingOperation>(2).checkpoint().await?;
    Ok(value)
}
fn main() {}
