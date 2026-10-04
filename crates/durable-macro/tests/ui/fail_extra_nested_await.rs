mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind = "bad", version = 1)]
async fn bad(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = 1 + ctx.wait::<NumberWait>(2).checkpoint().await?;
    Ok(value)
}
fn main() {}
