mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind = "bad", version = 1)]
async fn bad(ctx: WorkflowCtx, limit: u32) -> Result<u32, WaitError> {
    if limit > 0 {
        let value: u32 = ctx.wait::<NumberWait>(1).checkpoint().await?;
    }
    Ok(limit)
}
fn main() {}
