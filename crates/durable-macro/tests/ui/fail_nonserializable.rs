mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
struct Secret;
#[durable_workflow(kind = "bad", version = 1)]
async fn bad(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let secret: Secret = Secret;
    let value: u32 = ctx.wait::<NumberWait>(2).checkpoint(secret).await?;
    Ok(value)
}
fn main() {}
