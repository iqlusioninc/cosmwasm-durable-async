mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
const total: u32 = 100;
#[durable_workflow(kind = "bad", version = 1)]
async fn bad(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let mut total: u32 = 0;
    while total < 10 {
        let value: u32 = ctx.wait::<NumberWait>(1).checkpoint().await?;
        total += value;
    }
    Ok(total)
}
fn main() {}
