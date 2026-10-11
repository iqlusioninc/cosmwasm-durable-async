use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind="child", version=1)]
async fn child(ctx: WorkflowCtx) -> Result<u32, WaitError> { Ok(1) }
#[durable_workflow(kind="parent", version=1)]
async fn parent(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: &str = "borrowed";
    let result: u32 = ctx.call::<child::Workflow>(child::Input {}).checkpoint(value).await?;
    Ok(result)
}
fn main() {}
