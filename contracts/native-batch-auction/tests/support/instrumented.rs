//! Observations deliberately live outside the transaction; auction state does not.
use cosmwasm_std::*;
use native_async::{PollOutcome, PollResponse, RuntimeMsg, RuntimeQuery, TaskSudoMsg};
use native_batch_auction::{error::ContractError, msg::ExecuteMsg};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    panic::{catch_unwind, panic_any, AssertUnwindSafe},
};

const FAULT_KEY: &[u8] = b"auction-test/poll-fault";
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Fault {
    Helper,
    Trap,
    OutOfGas,
}

#[derive(Clone, Debug, Default)]
pub struct Trace {
    /// Distinct bid/tick keys read during this observation; repeated reads count once.
    pub distinct_record_keys: BTreeSet<Vec<u8>>,
    pub bid_writes: usize,
    pub writes: usize,
    pub ranges: usize,
    pub checkpoint_bytes: usize,
    pub envelope_bytes: usize,
    pub messages: usize,
}
thread_local! { static TRACE: RefCell<Trace> = RefCell::new(Trace::default()); }
pub fn take_trace() -> Trace {
    TRACE.with(|t| std::mem::take(&mut *t.borrow_mut()))
}
pub fn fault(storage: &mut dyn Storage, fault: Option<Fault>) {
    match fault {
        Some(fault) => storage.set(FAULT_KEY, &to_json_vec(&fault).unwrap()),
        None => storage.remove(FAULT_KEY),
    }
}
fn item(key: &[u8]) -> bool {
    key.starts_with(b"auction/v1/bid/") || key.starts_with(b"auction/v1/demand/")
}
struct Instrumented<'a> {
    inner: &'a mut dyn Storage,
    fault: Option<Fault>,
    writes: usize,
}
impl Storage for Instrumented<'_> {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        if item(key) {
            TRACE.with(|t| {
                t.borrow_mut().distinct_record_keys.insert(key.to_vec());
            });
            if matches!(self.fault, Some(Fault::Helper)) && self.writes > 0 {
                return Some(b"malformed JSON after a real bid write".to_vec());
            }
        }
        self.inner.get(key)
    }
    fn set(&mut self, key: &[u8], value: &[u8]) {
        self.inner.set(key, value);
        TRACE.with(|t| t.borrow_mut().writes += 1);
        if key.starts_with(b"auction/v1/bid/") {
            self.writes += 1;
            TRACE.with(|t| {
                t.borrow_mut().bid_writes += 1;
            });
            if let Some(fault @ (Fault::Trap | Fault::OutOfGas)) = &self.fault {
                panic_any(fault.clone());
            }
        }
    }
    fn remove(&mut self, key: &[u8]) {
        self.inner.remove(key);
    }
    fn range<'a>(
        &'a self,
        start: Option<&[u8]>,
        end: Option<&[u8]>,
        order: Order,
    ) -> Box<dyn Iterator<Item = Record> + 'a> {
        TRACE.with(|t| {
            t.borrow_mut().ranges += 1;
        });
        self.inner.range(start, end, order)
    }
}

pub fn execute(
    deps: DepsMut<RuntimeQuery>,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response<RuntimeMsg>, ContractError> {
    TRACE.with(|t| *t.borrow_mut() = Trace::default());
    let mut storage = Instrumented {
        inner: deps.storage,
        fault: None,
        writes: 0,
    };
    native_batch_auction::execute(
        DepsMut {
            storage: &mut storage,
            api: deps.api,
            querier: deps.querier,
        },
        env,
        info,
        msg,
    )
}

pub fn sudo(deps: DepsMut, env: Env, msg: TaskSudoMsg) -> Result<Response, ContractError> {
    TRACE.with(|t| *t.borrow_mut() = Trace::default());
    let fault = deps.storage.get(FAULT_KEY).map(from_json).transpose()?;
    let mut storage = Instrumented {
        inner: deps.storage,
        fault,
        writes: 0,
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        native_batch_auction::sudo(
            DepsMut {
                storage: &mut storage,
                api: deps.api,
                querier: deps.querier,
            },
            env,
            msg,
        )
    }));
    let response = match result {
        Ok(result) => result?,
        Err(payload) => match payload.downcast_ref::<Fault>() {
            Some(Fault::OutOfGas) => {
                return Err(StdError::generic_err("test-only auction out of gas").into())
            }
            Some(Fault::Trap) => return Err(StdError::generic_err("test-only auction trap").into()),
            _ => std::panic::resume_unwind(payload),
        },
    };
    let data = response.data.as_ref().unwrap();
    let poll: PollResponse = from_json(data)?;
    TRACE.with(|t| {
        let mut trace = t.borrow_mut();
        trace.envelope_bytes = data.len();
        trace.messages = response.messages.len() + poll.messages.len();
        if let PollOutcome::Pending { state, .. } = poll.outcome {
            trace.checkpoint_bytes = state.len();
        }
    });
    Ok(response)
}
