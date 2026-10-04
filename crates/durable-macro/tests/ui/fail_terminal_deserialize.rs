mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
use serde::Serialize;
#[derive(Serialize)]
struct Output(u32);
#[durable_workflow(kind = "bad", version = 1)]
async fn bad(ctx: WorkflowCtx) -> Result<Output, WaitError> {
    let value: u32 = ctx.wait::<NumberWait>(2).checkpoint().await?;
    Ok(Output(value))
}
fn main() {}
