mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
fn count() -> u32 { 100 }
#[durable_workflow(kind = "bad", version = 1)]
async fn bad(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let count = || 7u32;
    let value: u32 = ctx.wait::<NumberWait>(count()).checkpoint().await?;
    Ok(value + count())
}
fn main() {}
