use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[allow(non_snake_case)]
fn Input() -> u32 { 99 }
#[durable_workflow(kind = "reserved", version = 1)]
async fn reserved(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    Ok((|Input: fn() -> u32| Input())(|| 7))
}
fn main() {}
