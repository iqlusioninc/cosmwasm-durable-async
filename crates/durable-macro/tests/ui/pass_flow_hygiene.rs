#![allow(non_snake_case)]
mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind = "hygiene", version = 1)]
async fn hygiene(ctx: WorkflowCtx, mut __durable_cursor: u32, __durable_result: u32) -> Result<u32, WaitError> {
    loop {
        let __durable_request: u32 = ctx.wait::<NumberWait>(__durable_cursor).checkpoint(__durable_cursor, __durable_result).await?;
        __durable_cursor += __durable_request;
        break;
    }
    Ok(__durable_cursor + __durable_result)
}
fn main() {}
