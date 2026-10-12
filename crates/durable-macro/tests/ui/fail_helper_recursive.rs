use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind="recursive", version=1)]
async fn recursive(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let result: u32 = ctx.call::<recursive::Workflow>(recursive::Input {}).checkpoint().await?;
    Ok(result)
}
fn main() {}
