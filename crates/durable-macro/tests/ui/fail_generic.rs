use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind = "bad", version = 1)]
async fn bad<T>(ctx: WorkflowCtx, value: T) -> Result<u32, WaitError> { Ok(1) }
fn main() {}
