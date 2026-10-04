mod support;
use support::StringWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind = "bad", version = 1)]
async fn bad(ctx: WorkflowCtx, text: String) -> Result<u32, WaitError> {
    let value: u32 = ctx.wait::<StringWait>(text).checkpoint(text).await?;
    Ok(value)
}
fn main() {}
