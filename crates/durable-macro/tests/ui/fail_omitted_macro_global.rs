mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
fn number() -> u32 { 99 }
macro_rules! identity { ($value:expr) => { $value }; }
#[durable_workflow(kind = "bad", version = 1)]
async fn bad(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let number = || 7u32;
    let value: u32 = ctx.wait::<NumberWait>(number()).checkpoint().await?;
    Ok(value + identity!(number()))
}
fn main() {}
