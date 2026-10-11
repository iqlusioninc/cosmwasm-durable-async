use durable_macro::durable_workflow;
use durable_runtime::{WorkflowCtx, WaitError};
#[durable_workflow(kind="depth0", version=1)]
async fn depth0(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    Ok(1)
}
#[durable_workflow(kind="depth1", version=1)]
async fn depth1(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth0::Workflow>(depth0::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth2", version=1)]
async fn depth2(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth1::Workflow>(depth1::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth3", version=1)]
async fn depth3(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth2::Workflow>(depth2::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth4", version=1)]
async fn depth4(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth3::Workflow>(depth3::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth5", version=1)]
async fn depth5(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth4::Workflow>(depth4::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth6", version=1)]
async fn depth6(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth5::Workflow>(depth5::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth7", version=1)]
async fn depth7(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth6::Workflow>(depth6::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth8", version=1)]
async fn depth8(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth7::Workflow>(depth7::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth9", version=1)]
async fn depth9(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth8::Workflow>(depth8::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth10", version=1)]
async fn depth10(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth9::Workflow>(depth9::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth11", version=1)]
async fn depth11(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth10::Workflow>(depth10::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth12", version=1)]
async fn depth12(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth11::Workflow>(depth11::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth13", version=1)]
async fn depth13(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth12::Workflow>(depth12::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth14", version=1)]
async fn depth14(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth13::Workflow>(depth13::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth15", version=1)]
async fn depth15(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth14::Workflow>(depth14::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth16", version=1)]
async fn depth16(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth15::Workflow>(depth15::Input {}).checkpoint().await?;
    Ok(value)
}
#[durable_workflow(kind="depth17", version=1)]
async fn depth17(ctx: WorkflowCtx) -> Result<u32, WaitError> {
    let value: u32 = ctx.call::<depth16::Workflow>(depth16::Input {}).checkpoint().await?;
    Ok(value)
}
fn main() {}
