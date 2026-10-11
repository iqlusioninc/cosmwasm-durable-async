use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind="child", version=1)]
pub(crate) async fn child(ctx: WorkflowCtx) -> Result<u32, WaitError> { Ok(1) }
#[durable_workflow(kind="parent", version=1)]
async fn parent(ctx: WorkflowCtx, __durable_child: u32, __durable_transition: u32) -> Result<u32, WaitError> {
    let result: u32 = ctx.call::<child::Workflow>(child::Input {}).checkpoint(__durable_child, __durable_transition).await?;
    Ok(result + __durable_child + __durable_transition)
}
fn main() {}
