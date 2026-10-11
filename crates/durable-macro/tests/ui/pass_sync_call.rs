mod support;
use support::NumberWait;
use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};

struct Service;
impl Service {
    fn call(&self) -> u32 { 4 }
}

#[durable_workflow(kind = "sync-call", version = 1)]
async fn sync_call(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let service = Service;
    let request = service.call();
    let request = request + 1;
    let value: u32 = ctx.wait::<NumberWait>(request).checkpoint().await?;
    Ok(value)
}
fn main() {}
