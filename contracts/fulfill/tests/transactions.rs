use cosmwasm_std::{
    from_json, to_json_binary, Addr, Binary, Deps, DepsMut, Empty, Env, MessageInfo, Response,
    StdError, StdResult,
};
use cw_multi_test::{App, Contract, ContractWrapper, Executor};
use durable_runtime::{Deadline, Instance, Outcome, Status, WaitError};
use fulfill_example::{
    execute, instantiate, query, ExecuteMsg, InstantiateMsg, Order, Payment, QueryMsg, Receipt,
    ServiceMsg, Shipment, WorkflowError,
};
use serde::{Deserialize, Serialize};

fn contract() -> Box<dyn Contract<Empty>> {
    Box::new(ContractWrapper::new(execute, instantiate, query))
}

#[derive(Debug, Serialize, Deserialize)]
struct ServiceConfig {
    fail: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum TestServiceExecute {
    Request(ServiceMsg),
    Configure(ServiceConfig),
}

fn service() -> Box<dyn Contract<Empty>> {
    fn instantiate(
        deps: DepsMut,
        _: Env,
        _: MessageInfo,
        msg: ServiceConfig,
    ) -> StdResult<Response> {
        deps.storage.set(b"fail", &to_json_binary(&msg.fail)?);
        Ok(Response::new())
    }
    fn execute(
        deps: DepsMut,
        _: Env,
        _: MessageInfo,
        msg: TestServiceExecute,
    ) -> StdResult<Response> {
        if let TestServiceExecute::Configure(config) = msg {
            deps.storage.set(b"fail", &to_json_binary(&config.fail)?);
            return Ok(Response::new());
        }
        let fail: bool = from_json(deps.storage.get(b"fail").unwrap())?;
        if fail {
            Err(StdError::generic_err("service rejected request"))
        } else {
            Ok(Response::new())
        }
    }
    fn query(_: Deps, _: Env, _: Empty) -> StdResult<Binary> {
        to_json_binary(&())
    }
    Box::new(ContractWrapper::new(execute, instantiate, query))
}

struct Suite {
    app: App,
    owner: Addr,
    payment_service: Addr,
    shipment_service: Addr,
    contract: Addr,
}

impl Suite {
    fn new(fail_payment: bool, fail_shipment: bool) -> Self {
        let mut app = App::default();
        let owner = app.api().addr_make("owner");
        let service_code = app.store_code(service());
        let payment_service = app
            .instantiate_contract(
                service_code,
                owner.clone(),
                &ServiceConfig { fail: fail_payment },
                &[],
                "payment service",
                None,
            )
            .unwrap();
        let shipment_service = app
            .instantiate_contract(
                service_code,
                owner.clone(),
                &ServiceConfig {
                    fail: fail_shipment,
                },
                &[],
                "shipment service",
                None,
            )
            .unwrap();
        let code = app.store_code(contract());
        let contract = app
            .instantiate_contract(
                code,
                owner.clone(),
                &InstantiateMsg {
                    payment_service: payment_service.to_string(),
                    shipment_service: shipment_service.to_string(),
                    deadline_blocks: 10,
                },
                &[],
                "fulfillment",
                None,
            )
            .unwrap();
        Self {
            app,
            owner,
            payment_service,
            shipment_service,
            contract,
        }
    }

    fn start(&mut self, order_id: u64) -> u64 {
        let response = self
            .app
            .execute_contract(
                self.owner.clone(),
                self.contract.clone(),
                &ExecuteMsg::Start {
                    order: Order { id: order_id },
                },
                &[],
            )
            .unwrap();
        from_json(response.data.unwrap()).unwrap()
    }

    fn instance(&self, id: u64) -> Instance {
        self.app
            .wrap()
            .query_wasm_smart(&self.contract, &QueryMsg::Instance { workflow_id: id })
            .unwrap()
    }

    fn payment_seen(&self, order_id: u64) -> Option<Payment> {
        self.app
            .wrap()
            .query_wasm_smart(&self.contract, &QueryMsg::PaymentSeen { order_id })
            .unwrap()
    }

    fn payment(&mut self, id: u64, reference: &str) -> cw_multi_test::AppResponse {
        self.app
            .execute_contract(
                self.payment_service.clone(),
                self.contract.clone(),
                &ExecuteMsg::Resume {
                    workflow_id: id,
                    wait_sequence: 1,
                    outcome: Outcome::Success(
                        to_json_binary(&Payment {
                            reference: reference.into(),
                        })
                        .unwrap(),
                    ),
                },
                &[],
            )
            .unwrap()
    }
}

#[test]
fn two_callbacks_in_separate_transactions_restore_checkpoints() {
    let mut s = Suite::new(false, false);
    let id = s.start(42);
    assert!(matches!(s.instance(id).status, Status::Waiting { .. }));
    s.app.update_block(|b| b.height += 1);
    s.payment(id, "paid");
    let Status::Waiting { wait, .. } = s.instance(id).status else {
        panic!("not waiting")
    };
    assert_eq!(wait.sequence, 2);
    assert_eq!(wait.resolver, s.shipment_service);
    assert_eq!(
        s.payment_seen(42),
        Some(Payment {
            reference: "paid".into()
        })
    );
    s.app.update_block(|b| b.height += 1);
    s.app
        .execute_contract(
            s.shipment_service.clone(),
            s.contract.clone(),
            &ExecuteMsg::Resume {
                workflow_id: id,
                wait_sequence: 2,
                outcome: Outcome::Success(
                    to_json_binary(&Shipment {
                        tracking: "TRACK-42".into(),
                    })
                    .unwrap(),
                ),
            },
            &[],
        )
        .unwrap();
    let Status::Completed { output } = s.instance(id).status else {
        panic!("not complete")
    };
    let receipt: Receipt = from_json(output).unwrap();
    assert_eq!(receipt.order_id, 42);
    assert_eq!(receipt.shipment.tracking, "TRACK-42");
}

#[test]
fn unauthorized_and_malformed_callbacks_leave_wait_unchanged() {
    let mut s = Suite::new(false, false);
    let id = s.start(7);
    let before = s.instance(id);
    for (sender, outcome) in [
        (
            s.owner.clone(),
            Outcome::Success(
                to_json_binary(&Payment {
                    reference: "fake".into(),
                })
                .unwrap(),
            ),
        ),
        (
            s.payment_service.clone(),
            Outcome::Success(Binary::from(b"invalid JSON".as_slice())),
        ),
    ] {
        assert!(s
            .app
            .execute_contract(
                sender,
                s.contract.clone(),
                &ExecuteMsg::Resume {
                    workflow_id: id,
                    wait_sequence: 1,
                    outcome
                },
                &[]
            )
            .is_err());
        assert_eq!(s.instance(id), before);
        assert_eq!(s.payment_seen(7), None);
    }
}

#[test]
fn duplicate_callback_and_stale_expiration_cannot_advance_next_wait() {
    let mut s = Suite::new(false, false);
    let id = s.start(1);
    s.payment(id, "paid");
    let before = s.instance(id);
    assert!(s
        .app
        .execute_contract(
            s.payment_service.clone(),
            s.contract.clone(),
            &ExecuteMsg::Resume {
                workflow_id: id,
                wait_sequence: 1,
                outcome: Outcome::Success(
                    to_json_binary(&Payment {
                        reference: "twice".into()
                    })
                    .unwrap()
                )
            },
            &[]
        )
        .is_err());
    s.app.update_block(|b| b.height += 20);
    assert!(s
        .app
        .execute_contract(
            s.owner.clone(),
            s.contract.clone(),
            &ExecuteMsg::Expire {
                workflow_id: id,
                wait_sequence: 1
            },
            &[]
        )
        .is_err());
    assert_eq!(s.instance(id), before);
}

#[test]
fn failing_initiation_message_rolls_back_instance_and_id_allocation() {
    let mut s = Suite::new(true, false);
    assert!(s
        .app
        .execute_contract(
            s.owner.clone(),
            s.contract.clone(),
            &ExecuteMsg::Start {
                order: Order { id: 1 }
            },
            &[]
        )
        .is_err());
    assert!(s
        .app
        .wrap()
        .query_wasm_smart::<Instance>(&s.contract, &QueryMsg::Instance { workflow_id: 1 })
        .is_err());
    let count: u64 = s
        .app
        .wrap()
        .query_wasm_smart(&s.contract, &QueryMsg::ActiveCount {})
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn failing_resume_message_rolls_back_continuation_and_application_storage() {
    let mut s = Suite::new(false, true);
    let id = s.start(9);
    let before = s.instance(id);
    let result = s.app.execute_contract(
        s.payment_service.clone(),
        s.contract.clone(),
        &ExecuteMsg::Resume {
            workflow_id: id,
            wait_sequence: 1,
            outcome: Outcome::Success(
                to_json_binary(&Payment {
                    reference: "paid".into(),
                })
                .unwrap(),
            ),
        },
        &[],
    );
    assert!(result.is_err());
    assert_eq!(s.instance(id), before);
    assert_eq!(s.payment_seen(9), None);
    // The same callback remains valid: retry reaches the failing downstream service again.
    let retry = s.app.execute_contract(
        s.payment_service.clone(),
        s.contract.clone(),
        &ExecuteMsg::Resume {
            workflow_id: id,
            wait_sequence: 1,
            outcome: Outcome::Success(
                to_json_binary(&Payment {
                    reference: "paid".into(),
                })
                .unwrap(),
            ),
        },
        &[],
    );
    assert!(format!("{:#}", retry.unwrap_err()).contains("service rejected request"));

    // Repair the remote service, then retry exactly the previously failed wait.
    s.app
        .execute_contract(
            s.owner.clone(),
            s.shipment_service.clone(),
            &ServiceConfig { fail: false },
            &[],
        )
        .unwrap();
    s.payment(id, "paid");
    assert!(matches!(s.instance(id).status, Status::Waiting { wait, .. } if wait.sequence == 2));
    assert_eq!(
        s.payment_seen(9),
        Some(Payment {
            reference: "paid".into()
        })
    );
}

#[test]
fn application_failure_commits_failure_record_and_segment_writes() {
    let mut s = Suite::new(false, false);
    let id = s.start(12);
    s.payment(id, "");
    let Status::Failed { error } = s.instance(id).status else {
        panic!("not failed")
    };
    assert_eq!(
        from_json::<WorkflowError>(error).unwrap(),
        WorkflowError::PaymentRejected
    );
    assert_eq!(
        s.payment_seen(12),
        Some(Payment {
            reference: String::new()
        })
    );
    let count: u64 = s
        .app
        .wrap()
        .query_wasm_smart(&s.contract, &QueryMsg::ActiveCount {})
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn remote_error_is_a_durable_workflow_failure() {
    let mut s = Suite::new(false, false);
    let id = s.start(13);
    s.app
        .execute_contract(
            s.payment_service.clone(),
            s.contract.clone(),
            &ExecuteMsg::Resume {
                workflow_id: id,
                wait_sequence: 1,
                outcome: Outcome::Error {
                    code: "declined".into(),
                    message: "insufficient funds".into(),
                },
            },
            &[],
        )
        .unwrap();
    let Status::Failed { error } = s.instance(id).status else {
        panic!("not failed")
    };
    assert_eq!(
        from_json::<WorkflowError>(error).unwrap(),
        WorkflowError::Wait(WaitError::Remote {
            code: "declined".into(),
            message: "insufficient funds".into(),
        })
    );
}

#[test]
fn deadline_equality_rejects_callback_and_allows_permissionless_expiration() {
    let mut s = Suite::new(false, false);
    let id = s.start(14);
    let Status::Waiting { wait, .. } = s.instance(id).status else {
        panic!("not waiting")
    };
    let Deadline::Height(height) = wait.deadline else {
        panic!("wrong deadline")
    };
    s.app.update_block(|b| b.height = height);
    assert!(s
        .app
        .execute_contract(
            s.payment_service.clone(),
            s.contract.clone(),
            &ExecuteMsg::Resume {
                workflow_id: id,
                wait_sequence: 1,
                outcome: Outcome::Success(
                    to_json_binary(&Payment {
                        reference: "late".into()
                    })
                    .unwrap()
                )
            },
            &[]
        )
        .is_err());
    let stranger = s.app.api().addr_make("stranger");
    s.app
        .execute_contract(
            stranger,
            s.contract.clone(),
            &ExecuteMsg::Expire {
                workflow_id: id,
                wait_sequence: 1,
            },
            &[],
        )
        .unwrap();
    let Status::Failed { error } = s.instance(id).status else {
        panic!("not failed")
    };
    assert_eq!(
        from_json::<WorkflowError>(error).unwrap(),
        WorkflowError::Wait(WaitError::Timeout)
    );
}

#[test]
fn only_owner_can_start_and_separate_instances_keep_distinct_state() {
    let mut s = Suite::new(false, false);
    let stranger = s.app.api().addr_make("stranger");
    assert!(s
        .app
        .execute_contract(
            stranger,
            s.contract.clone(),
            &ExecuteMsg::Start {
                order: Order { id: 99 }
            },
            &[]
        )
        .is_err());
    let a = s.start(100);
    let b = s.start(200);
    assert_ne!(a, b);
    s.payment(b, "second");
    assert_eq!(s.payment_seen(100), None);
    assert!(matches!(s.instance(a).status, Status::Waiting { wait, .. } if wait.sequence == 1));
}
