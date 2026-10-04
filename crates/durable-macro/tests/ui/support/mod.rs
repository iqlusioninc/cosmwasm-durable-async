use durable_runtime::{Correlation, Deadline, Operation, PreparedWait, RuntimeResult, WorkflowCtx};
use durable_runtime::cosmwasm_std::{Addr, to_json_binary};
pub struct NumberWait;
impl Operation for NumberWait {
    const KIND: &'static str = "number";
    type Request = u32;
    type Output = u32;
    fn prepare(ctx: &mut WorkflowCtx<'_>, _: Correlation, request: u32) -> RuntimeResult<PreparedWait> {
        Ok(PreparedWait { operation: Self::KIND.into(), request: to_json_binary(&request)?, resolver: Addr::unchecked("resolver"), deadline: Deadline::Height(ctx.env().block.height + 1), messages: vec![] })
    }
}
pub struct StringWait;
impl Operation for StringWait {
    const KIND: &'static str = "string";
    type Request = String;
    type Output = u32;
    fn prepare(ctx: &mut WorkflowCtx<'_>, _: Correlation, request: String) -> RuntimeResult<PreparedWait> {
        Ok(PreparedWait { operation: Self::KIND.into(), request: to_json_binary(&request)?, resolver: Addr::unchecked("resolver"), deadline: Deadline::Height(ctx.env().block.height + 1), messages: vec![] })
    }
}
