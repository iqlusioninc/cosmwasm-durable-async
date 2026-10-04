mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind = "valid", version = 7)]
pub async fn valid(ctx: WorkflowCtx, mut count: u32) -> Result<u32, WaitError> {
    let first: u32 = ctx.wait::<NumberWait>(count).checkpoint(count).await?;
    count += first;
    let second: Result<u32, WaitError> = ctx.wait::<NumberWait>(count).checkpoint(count).await;
    Ok(count + second?)
}
fn main() { let _ = valid::Input { count: 2 }; }
