#![allow(unused_imports)]
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind = "bad", version = 1)]
async fn bad(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    loop {
        #[cfg_attr(all(), cfg(any()))]
        break;
    }
    Ok(0)
}
fn main() {}
