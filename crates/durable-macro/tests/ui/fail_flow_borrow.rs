#![allow(unused_imports)]
mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind = "bad", version = 1)]
async fn bad(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = 1;
    let held: &u32 = &value;
    loop {
        let result: u32 = ctx.wait::<NumberWait>(1).checkpoint(held).await?;
        break;
    }
    Ok(0)
}
fn main() {}
