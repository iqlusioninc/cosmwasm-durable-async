use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind="child", version=1)]
async fn child(ctx: WorkflowCtx) -> Result<u32, WaitError> { Ok(1) }
#[durable_workflow(kind="parent", version=1)]
async fn parent(ctx: WorkflowCtx, value: u32) -> Result<u32, WaitError> {
    let result: u32 = ctx.call::<child::Workflow>(child::Input {}).checkpoint().await?;
    Ok(result + value)
}
fn main() {}
