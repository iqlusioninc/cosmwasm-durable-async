#![cfg(feature = "host")]
use cosmwasm_std::{
    testing::{MockApi, MockStorage},
    Addr, Binary, Coin, CosmosMsg, Decimal, StdResult, Storage, Uint128,
};
use native_async::{host::Keeper, runtime::*, *};
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    name: String,
    height: u64,
    expiry: u64,
    kind: String,
    due: u64,
    resolver: String,
    expected_accept: bool,
}

struct WaitHost {
    wait: Wait,
    dispatches: usize,
}
impl PollHost for WaitHost {
    fn code_checksum(&self, _: &str) -> StdResult<Binary> {
        Ok(Binary::from(b"code".as_slice()))
    }
    fn poll(
        &mut self,
        s: &mut dyn Storage,
        _: &str,
        _: &PollRequest,
        _: u64,
    ) -> StdResult<Metered<PollResponse>> {
        s.set(b"guest/effect", b"written");
        Ok(Metered {
            gas_used: 7,
            result: Ok(PollResponse {
                outcome: PollOutcome::Pending {
                    state: Binary::from(b"checkpoint".as_slice()),
                    wait: self.wait.clone(),
                },
                messages: vec![CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
                    to_address: "recipient".into(),
                    amount: vec![Coin::new(1u128, "utest")],
                })],
            }),
        })
    }
    fn dispatch(
        &mut self,
        s: &mut dyn Storage,
        _: &str,
        messages: &[CosmosMsg],
        _: u64,
    ) -> StdResult<Metered<()>> {
        self.dispatches += messages.len();
        s.set(b"outbound/effect", b"sent");
        Ok(Metered {
            gas_used: 0,
            result: Ok(()),
        })
    }
    fn transfer(&mut self, _: &mut dyn Storage, _: &Addr, _: &Addr, _: &Coin) -> StdResult<()> {
        Ok(())
    }
}

fn runtime(api: &MockApi) -> Runtime {
    Runtime {
        keeper: Keeper {
            escrow: api.addr_make("escrow"),
            config: FundingConfig {
                denom: "ugas".into(),
                gas_price: Decimal::one(),
                min_base_price: Decimal::one(),
                target_gas: 60,
                price_grace_blocks: 3,
                last_block_poll_gas_used: 0,
                max_gas_per_poll: 100,
                max_lifetime_blocks: 100,
            },
        },
        config: RuntimeConfig {
            block_gas: 120,
            host_gas: 2,
            maintenance_gas: 100,
            maintenance_gas_per_item: 1,
            price_grace_blocks: 3,
            price_adjust_denominator: 8,
            max_price_pauses: 10,
            max_visits: 10,
            max_promotions: 10,
            max_expirations: 10,
            max_prunes: 10,
            max_tasks: 10,
            max_contract_tasks: 10,
            max_allocated_bytes: 100_000,
            max_task_bytes: 10_000,
            max_messages: 10,
            max_message_bytes: 10_000,
            lease_price_per_byte_block: Uint128::zero(),
            cleanup_fee: Uint128::one(),
            retention_blocks: 2,
            fee_collector: api.addr_make("fees"),
        },
    }
}

// Catches accepting an out-of-lease wait or bypassing trusted address policy,
// and proves rejected raw guest responses never dispatch or commit checkpoints.
#[test]
fn shared_wait_admission_precedes_checkpoint_and_dispatch() {
    // Both hosts use cosmos and a 1..=255 decoded-byte account policy here.
    let api = MockApi::default().with_prefix("cosmos");
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../fixtures/native-async/wait-admission.json"
    ))
    .unwrap();
    let mut mismatches = Vec::new();
    for case in cases {
        let rt = runtime(&api);
        let mut s = MockStorage::new();
        let c = api.addr_make("contract");
        let wait = match case.kind.as_str() {
            "timer" => Wait::Height { height: case.due },
            "operation" => Wait::Operation {
                resolver: case.resolver,
                deadline: case.due,
                max_result_bytes: 20,
            },
            _ => panic!("unknown fixture kind"),
        };
        let mut host = WaitHost {
            wait: wait.clone(),
            dispatches: 0,
        };
        let id = rt
            .register(
                &api,
                &mut s,
                1,
                &c,
                SchedulerMsg::RegisterTask {
                    control: c.to_string(),
                    refund: c.to_string(),
                    gas_per_poll: 40,
                    max_gas_price: Decimal::one(),
                    expires_at: case.expiry,
                    funds: Coin::new(1000u128, "ugas"),
                },
                TaskProgram {
                    kind: "test".into(),
                    version: 1,
                    input: Binary::from(b"input".as_slice()),
                },
                4096,
                &mut host,
            )
            .unwrap();
        let report = rt.run_block(&api, &mut s, case.height, &mut host).unwrap();
        assert_eq!(report.polls, 1, "{}", case.name);
        let task = rt.task(&s, &c, id, case.height).unwrap();
        let funding: TaskFunding = cosmwasm_std::from_json(
            rt.keeper
                .query(
                    &api,
                    &s,
                    case.height,
                    SchedulerQuery::TaskFunding {
                        contract: c.to_string(),
                        task_id: id,
                    },
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            funding.cumulative_execution_fees,
            Uint128::new(9),
            "{} charges consumed gas",
            case.name
        );
        let accepted = task.status == ExecutionStatus::Waiting;
        if accepted != case.expected_accept {
            mismatches.push(case.name.clone());
            continue;
        }
        if accepted {
            assert_eq!(task.wait, Some(wait), "{}", case.name);
            assert_eq!(
                task.state,
                Some(Binary::from(b"checkpoint".as_slice())),
                "{}",
                case.name
            );
            assert_eq!(host.dispatches, 1, "{}", case.name);
            assert_eq!(s.get(b"guest/effect"), Some(b"written".to_vec()));
        } else {
            assert_eq!(
                funding.status,
                TaskStatus::Paused {
                    reason: PauseReason::InvalidResponse,
                    resume: ResumeState::Runnable
                },
                "{}",
                case.name
            );
            assert_eq!(host.dispatches, 0, "{}", case.name);
            assert!(
                task.wait.is_none() && task.state.is_none(),
                "{} retained checkpoint",
                case.name
            );
            assert_eq!(task.wait_sequence, 0);
            assert_eq!(task.program.input, Binary::from(b"input".as_slice()));
            assert!(
                s.get(b"guest/effect").is_none() && s.get(b"outbound/effect").is_none(),
                "{} committed guest effects",
                case.name
            );
        }
    }
    assert!(
        mismatches.is_empty(),
        "admission disagrees with shared vectors: {mismatches:?}"
    );
}
