mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind = "bad", version = 1)]
async fn bad(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let (count,): (u32,) = (2,);
    let value: u32 = ctx.wait::<NumberWait>(count).checkpoint(count).await?;
    Ok(value)
}
fn main() {}
