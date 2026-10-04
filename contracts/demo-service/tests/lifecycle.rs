use cosmwasm_std::{to_json_binary, Addr, Binary};
use cw_multi_test::{App, ContractWrapper, Executor};
use demo_service::{ExecuteMsg as S, InstantiateMsg as SI, Pending, QueryMsg as Q};
use durable_runtime::{Instance, Outcome, Status};
use fulfill_example::{
    ExecuteMsg as F, InstantiateMsg as FI, Order, Payment, QueryMsg as FQ, Shipment,
};

struct Suite {
    app: App,
    owner: Addr,
    workflow: Addr,
    service: Addr,
}
impl Suite {
    fn new() -> Self {
        let mut app = App::default();
        let owner = app.api().addr_make("owner");
        let sc = app.store_code(Box::new(ContractWrapper::new(
            demo_service::execute,
            demo_service::instantiate,
            demo_service::query,
        )));
        let service = app
            .instantiate_contract(sc, owner.clone(), &SI {}, &[], "service", None)
            .unwrap();
        let fc = app.store_code(Box::new(ContractWrapper::new(
            fulfill_example::execute,
            fulfill_example::instantiate,
            fulfill_example::query,
        )));
        let workflow = app
            .instantiate_contract(
                fc,
                owner.clone(),
                &FI {
                    payment_service: service.to_string(),
                    shipment_service: service.to_string(),
                    deadline_blocks: 10,
                },
                &[],
                "workflow",
                None,
            )
            .unwrap();
        app.execute_contract(
            owner.clone(),
            service.clone(),
            &S::Bind {
                workflow: workflow.to_string(),
            },
            &[],
        )
        .unwrap();
        Self {
            app,
            owner,
            workflow,
            service,
        }
    }
    fn start(&mut self) {
        self.app
            .execute_contract(
                self.owner.clone(),
                self.workflow.clone(),
                &F::Start {
                    order: Order { id: 1 },
                },
                &[],
            )
            .unwrap();
    }
    fn pending(&self) -> Vec<Pending> {
        self.app
            .wrap()
            .query_wasm_smart(
                &self.service,
                &Q::Pending {
                    start_after: None,
                    limit: Some(10),
                },
            )
            .unwrap()
    }
    fn status(&self) -> Status {
        self.app
            .wrap()
            .query_wasm_smart::<Instance>(&self.workflow, &FQ::Instance { workflow_id: 1 })
            .unwrap()
            .status
    }
}
#[test]
fn successful_workflow_and_failed_callback_retry() {
    let mut s = Suite::new();
    s.start();
    let p = s.pending();
    assert_eq!(p.len(), 1);
    let deliver = |s: &mut Suite, outcome| {
        s.app.execute_contract(
            s.owner.clone(),
            s.service.clone(),
            &S::Deliver { id: 1, outcome },
            &[],
        )
    };
    assert!(deliver(&mut s, Outcome::Success(Binary::from(b"invalid"))).is_err());
    assert_eq!(s.pending(), p);
    assert!(matches!(s.status(), Status::Waiting { .. }));
    deliver(
        &mut s,
        Outcome::Success(
            to_json_binary(&Payment {
                reference: "demo".into(),
            })
            .unwrap(),
        ),
    )
    .unwrap();
    assert_eq!(s.pending()[0].id, 2);
    s.app
        .execute_contract(
            s.owner.clone(),
            s.service.clone(),
            &S::Deliver {
                id: 2,
                outcome: Outcome::Success(
                    to_json_binary(&Shipment {
                        tracking: "demo".into(),
                    })
                    .unwrap(),
                ),
            },
            &[],
        )
        .unwrap();
    assert!(s.pending().is_empty());
    assert!(matches!(s.status(), Status::Completed { .. }));
}
#[test]
fn remote_failure_expiry_and_permissions() {
    let mut s = Suite::new();
    s.start();
    let stranger = s.app.api().addr_make("stranger");
    assert!(s
        .app
        .execute_contract(
            stranger,
            s.service.clone(),
            &S::Deliver {
                id: 1,
                outcome: Outcome::Error {
                    code: "declined".into(),
                    message: "demo".into()
                }
            },
            &[]
        )
        .is_err());
    s.app
        .execute_contract(
            s.owner.clone(),
            s.service.clone(),
            &S::Deliver {
                id: 1,
                outcome: Outcome::Error {
                    code: "declined".into(),
                    message: "demo".into(),
                },
            },
            &[],
        )
        .unwrap();
    assert!(matches!(s.status(), Status::Failed { .. }));
    assert!(s.pending().is_empty());
    let mut s = Suite::new();
    s.start();
    assert!(s
        .app
        .execute_contract(s.owner.clone(), s.service.clone(), &S::Prune { id: 1 }, &[])
        .is_err());
    s.app.update_block(|b| b.height += 10);
    s.app
        .execute_contract(
            s.owner.clone(),
            s.workflow.clone(),
            &F::Expire {
                workflow_id: 1,
                wait_sequence: 1,
            },
            &[],
        )
        .unwrap();
    s.app
        .execute_contract(s.owner.clone(), s.service.clone(), &S::Prune { id: 1 }, &[])
        .unwrap();
    assert!(matches!(s.status(), Status::Failed { .. }));
    assert!(s.pending().is_empty());
}

#[test]
fn queue_bounds_binding_and_authentication() {
    use durable_runtime::Correlation;
    let mut s = Suite::new();
    assert!(s
        .app
        .execute_contract(
            s.owner.clone(),
            s.service.clone(),
            &S::Bind {
                workflow: s.workflow.to_string()
            },
            &[]
        )
        .is_err());
    assert!(s
        .app
        .execute_contract(
            s.owner.clone(),
            s.service.clone(),
            &S::Begin {
                correlation: Correlation {
                    workflow_id: 77,
                    wait_sequence: 1
                },
                request: Binary::default()
            },
            &[]
        )
        .is_err());
    s.start();
    s.start();
    let page: Vec<Pending> = s
        .app
        .wrap()
        .query_wasm_smart(
            &s.service,
            &Q::Pending {
                start_after: None,
                limit: Some(1),
            },
        )
        .unwrap();
    assert_eq!(page.len(), 1);
    let second: Vec<Pending> = s
        .app
        .wrap()
        .query_wasm_smart(
            &s.service,
            &Q::Pending {
                start_after: Some(page[0].id),
                limit: Some(1000),
            },
        )
        .unwrap();
    assert_eq!(second.len(), 1);
    assert!(second[0].id > page[0].id);
    s.app
        .sudo(cw_multi_test::SudoMsg::Bank(
            cw_multi_test::BankSudo::Mint {
                to_address: s.owner.to_string(),
                amount: vec![cosmwasm_std::coin(1, "stake")],
            },
        ))
        .unwrap();
    assert!(s
        .app
        .execute_contract(
            s.owner.clone(),
            s.service.clone(),
            &S::Deliver {
                id: 1,
                outcome: Outcome::Error {
                    code: "test".into(),
                    message: "test".into()
                }
            },
            &[cosmwasm_std::coin(1, "stake")]
        )
        .is_err());
}
