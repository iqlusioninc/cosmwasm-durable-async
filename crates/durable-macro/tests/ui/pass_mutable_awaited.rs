#![deny(unused_mut)]
mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind = "mutable", version = 1)]
async fn mutable(ctx: WorkflowCtx, input: u32) -> Result<u32, WaitError> {
    let mut count: u32 = ctx.wait::<NumberWait>(input).checkpoint().await?;
    let extra: u32 = ctx.wait::<NumberWait>(count).checkpoint(count).await?;
    count += extra;
    Ok(count)
}
fn main() { let _ = mutable::Input { input: 1 }; }
