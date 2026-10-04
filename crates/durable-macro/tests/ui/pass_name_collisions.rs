#![deny(unused_imports)]
mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
pub struct Input { value: u32 }
#[derive(Serialize, Deserialize)]
pub struct State { value: u32 }
#[derive(Serialize, Deserialize)]
pub struct Workflow { value: u32 }
type Result<T> = std::result::Result<T, WaitError>;
fn require_persistable(value: u32) -> u32 { value }
#[durable_workflow(kind = "names", version = 1)]
async fn names(ctx: WorkflowCtx, input: Input, state: State, workflow: Workflow) -> std::result::Result<Input, WaitError> {
    let first: u32 = ctx.wait::<NumberWait>(require_persistable(input.value)).checkpoint(input, state, workflow).await?;
    let output: Input = Input { value: require_persistable(input.value + state.value + workflow.value + first) };
    Ok(output)
}
fn main() {
    let _: Result<()> = Ok(());
    let _ = names::Input { input: Input { value: 1 }, state: State { value: 2 }, workflow: Workflow { value: 3 } };
}
