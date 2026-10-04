mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind = "bad", version = 1)]
async fn bad(ctx: WorkflowCtx, count: u32) -> Result<u32, WaitError> {
    let value: u32 = ctx.wait::<NumberWait>(count).checkpoint(count, count).await?;
    Ok(value)
}
fn main() {}
