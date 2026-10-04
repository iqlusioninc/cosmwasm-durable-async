use cosmwasm_std::{
    from_json,
    testing::{message_info, mock_dependencies, mock_env},
    to_json_binary, Binary,
};
use durable_runtime::{
    active_count, assert_supported_versions, load_instance, migrate_waiting, registration, resume,
    start, Limits, Outcome, Registry, Resolution, RuntimeResult, Status, Transition, Workflow,
    WorkflowCtx,
};
use fulfill_example::{fulfill, instantiate, InstantiateMsg, Order, Payment, Receipt, Shipment};

/// A version adapter illustrates retaining old handlers during a compatible upgrade.
/// Real changed definitions should use a separate #[durable_workflow(version = 2)].
struct FulfillV2;

impl Workflow for FulfillV2 {
    const KIND: &'static str = "fulfill";
    const VERSION: u32 = 2;
    type Input = fulfill::Input;

    fn start(ctx: &mut WorkflowCtx<'_>, input: Binary) -> RuntimeResult<Transition> {
        fulfill::Workflow::start(ctx, input)
    }

    fn validate(state: &Binary, resolution: &Resolution) -> RuntimeResult<()> {
        fulfill::Workflow::validate(state, resolution)
    }

    fn operation(state: &Binary) -> RuntimeResult<&'static str> {
        fulfill::Workflow::operation(state)
    }

    fn resume(
        ctx: &mut WorkflowCtx<'_>,
        state: Binary,
        resolution: Resolution,
    ) -> RuntimeResult<Transition> {
        fulfill::Workflow::resume(ctx, state, resolution)
    }
}

#[test]
fn active_versions_must_be_retained_until_explicit_migration() {
    let mut deps = mock_dependencies();
    let env = mock_env();
    let owner = deps.api.addr_make("owner");
    let payment = deps.api.addr_make("payment");
    let shipping = deps.api.addr_make("shipping");
    instantiate(
        deps.as_mut(),
        env.clone(),
        message_info(&owner, &[]),
        InstantiateMsg {
            payment_service: payment.to_string(),
            shipment_service: shipping.to_string(),
            deadline_blocks: 10,
        },
    )
    .unwrap();
    let limits = Limits::default();
    let (old, _) = start::<fulfill::Workflow>(
        deps.as_mut(),
        env.clone(),
        message_info(&owner, &[]),
        fulfill::Input {
            order: Order { id: 1 },
        },
        &limits,
    )
    .unwrap();
    let (new, _) = start::<FulfillV2>(
        deps.as_mut(),
        env.clone(),
        message_info(&owner, &[]),
        fulfill::Input {
            order: Order { id: 2 },
        },
        &limits,
    )
    .unwrap();
    let only_v2 = Registry::new(vec![registration::<FulfillV2>()]).unwrap();
    assert!(assert_supported_versions(deps.as_ref().storage, &only_v2).is_err());
    let both = Registry::new(vec![
        registration::<fulfill::Workflow>(),
        registration::<FulfillV2>(),
    ])
    .unwrap();
    assert_supported_versions(deps.as_ref().storage, &both).unwrap();
    assert_eq!(
        active_count(deps.as_ref().storage, "fulfill", 1).unwrap(),
        1
    );
    assert_eq!(
        active_count(deps.as_ref().storage, "fulfill", 2).unwrap(),
        1
    );

    let before = load_instance(deps.as_ref().storage, old).unwrap();
    let Status::Waiting { state, wait } = before.status else {
        panic!("not waiting")
    };
    let migrated = migrate_waiting(deps.as_mut().storage, old, 2, state, &both, &limits).unwrap();
    assert_eq!(migrated.version, 2);
    assert!(matches!(migrated.status, Status::Waiting { wait: ref after, .. } if after == &wait));
    assert_eq!(
        active_count(deps.as_ref().storage, "fulfill", 1).unwrap(),
        0
    );
    assert_eq!(
        active_count(deps.as_ref().storage, "fulfill", 2).unwrap(),
        2
    );
    assert_supported_versions(deps.as_ref().storage, &only_v2).unwrap();

    for id in [old, new] {
        resume(
            deps.as_mut(),
            env.clone(),
            message_info(&payment, &[]),
            id,
            1,
            Outcome::Success(
                to_json_binary(&Payment {
                    reference: "paid".into(),
                })
                .unwrap(),
            ),
            &both,
            &limits,
        )
        .unwrap();
        resume(
            deps.as_mut(),
            env.clone(),
            message_info(&shipping, &[]),
            id,
            2,
            Outcome::Success(
                to_json_binary(&Shipment {
                    tracking: "track".into(),
                })
                .unwrap(),
            ),
            &both,
            &limits,
        )
        .unwrap();
        let instance = load_instance(deps.as_ref().storage, id).unwrap();
        assert_eq!(instance.version, 2);
        let Status::Completed { output } = instance.status else {
            panic!("not completed")
        };
        let receipt: Receipt = from_json(output).unwrap();
        assert!(matches!(receipt.order_id, 1 | 2));
    }
    assert_eq!(
        active_count(deps.as_ref().storage, "fulfill", 2).unwrap(),
        0
    );
}
