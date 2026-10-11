use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind="parent", version=1)]
async fn parent(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let result: u32 = ctx.call::<u32>(1).checkpoint().await?;
    Ok(result)
}
fn main() {}
