//! Milestone S5 behavioral pinning over the metadata-owned call inventory.

use alloc::{boxed::Box, collections::BTreeSet, format, string::String, vec, vec::Vec};

use frame_support::{
    dispatch::GetDispatchInfo,
    traits::{
        tokens::fungibles::{Create, Inspect},
        Contains,
    },
    weights::Weight,
};
use futarchy_primitives::{chain_identity, currency, kernel};
use origins_core::{BoxedCall, CallDomain, Origin as ClassOrigin, RuntimeCall as FilterCall};
use pallet_origins::{SafetyClassifier, SafetyFilter};
use parity_scale_codec::Encode;
use sp_keyring::Sr25519Keyring;
use sp_runtime::{
    traits::{AccountIdConversion, Dispatchable},
    MultiAddress,
};

use crate::{
    classifier::{is_values_enactment_leaf, BleavitSafetyClassifier, RuntimeBaseCallFilter},
    tests::{
        account, development_ext, remark, seed_parachain_upgrade_boundary, set_pending_upgrade,
        upgrade_ext,
    },
    tests_s5::{
        ConditionalKind, ExpectedTreatment, InventoryRow, RuntimeMetadataModel, WrapperShape,
        INVENTORY,
    },
    ForeignAssets, Multisig, Runtime, RuntimeCall, RuntimeOrigin, System,
};

#[allow(clippy::needless_match)] // Intentionally compile-time exhaustive over ClassOrigin growth.
fn exhaustively_checked_class_origin(origin: ClassOrigin) -> ClassOrigin {
    match origin {
        ClassOrigin::FutarchyParam => ClassOrigin::FutarchyParam,
        ClassOrigin::FutarchyTreasury => ClassOrigin::FutarchyTreasury,
        ClassOrigin::FutarchyCode => ClassOrigin::FutarchyCode,
        ClassOrigin::FutarchyMeta => ClassOrigin::FutarchyMeta,
        ClassOrigin::ConstitutionalValues => ClassOrigin::ConstitutionalValues,
        ClassOrigin::OracleResolution => ClassOrigin::OracleResolution,
        ClassOrigin::GuardianHold => ClassOrigin::GuardianHold,
        ClassOrigin::EmergencyPlaybook => ClassOrigin::EmergencyPlaybook,
    }
}

fn all_class_origins() -> [ClassOrigin; 8] {
    [
        ClassOrigin::FutarchyParam,
        ClassOrigin::FutarchyTreasury,
        ClassOrigin::FutarchyCode,
        ClassOrigin::FutarchyMeta,
        ClassOrigin::ConstitutionalValues,
        ClassOrigin::OracleResolution,
        ClassOrigin::GuardianHold,
        ClassOrigin::EmergencyPlaybook,
    ]
    .map(exhaustively_checked_class_origin)
}

fn futarchy_origins() -> [ClassOrigin; 4] {
    [
        ClassOrigin::FutarchyParam,
        ClassOrigin::FutarchyTreasury,
        ClassOrigin::FutarchyCode,
        ClassOrigin::FutarchyMeta,
    ]
}

fn row_name(row: &InventoryRow) -> String {
    format!("{}.{}", row.pallet, row.call)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrivilegedWrapperPolicy {
    SameOriginRecursive,
    RejectPrivileged,
    RejectAll,
    HashOnly,
}

fn privileged_wrapper_policy(shape: WrapperShape) -> PrivilegedWrapperPolicy {
    match shape {
        WrapperShape::UtilityBatch
        | WrapperShape::UtilityBatchAll
        | WrapperShape::UtilityForceBatch
        | WrapperShape::UtilityWithWeight
        | WrapperShape::Sudo
        | WrapperShape::SudoUncheckedWeight => PrivilegedWrapperPolicy::SameOriginRecursive,
        WrapperShape::Proxy
        | WrapperShape::ProxyAnnounced
        | WrapperShape::MultisigAsMultiThreshold1
        | WrapperShape::MultisigAsMulti => PrivilegedWrapperPolicy::RejectPrivileged,
        WrapperShape::UtilityAsDerivative
        | WrapperShape::UtilityDispatchAs
        | WrapperShape::UtilityIfElse
        | WrapperShape::UtilityDispatchAsFallible
        | WrapperShape::SchedulerSchedule
        | WrapperShape::SchedulerScheduleNamed
        | WrapperShape::SchedulerScheduleAfter
        | WrapperShape::SchedulerScheduleNamedAfter
        | WrapperShape::SudoAs => PrivilegedWrapperPolicy::RejectAll,
        WrapperShape::MultisigApproveAsMulti => PrivilegedWrapperPolicy::HashOnly,
    }
}

struct WrapperConstruction {
    pallet: &'static str,
    call: &'static str,
    cases: Vec<(String, RuntimeCall)>,
}

fn wrapper_construction(shape: WrapperShape, target: RuntimeCall) -> WrapperConstruction {
    let who = account(73);
    let signed_origin: <RuntimeOrigin as frame_support::traits::OriginTrait>::PalletsOrigin =
        frame_system::RawOrigin::Signed(who.clone()).into();
    match shape {
        WrapperShape::UtilityBatch => WrapperConstruction {
            pallet: "Utility",
            call: "batch",
            cases: vec![
                (
                    String::from("utility.batch(target,public)"),
                    RuntimeCall::Utility(pallet_utility::Call::batch {
                        calls: vec![target.clone(), remark()],
                    }),
                ),
                (
                    String::from("utility.batch(public,target)"),
                    RuntimeCall::Utility(pallet_utility::Call::batch {
                        calls: vec![remark(), target],
                    }),
                ),
            ],
        },
        WrapperShape::UtilityAsDerivative => WrapperConstruction {
            pallet: "Utility",
            call: "as_derivative",
            cases: vec![(
                String::from("utility.as_derivative"),
                RuntimeCall::Utility(pallet_utility::Call::as_derivative {
                    index: 0,
                    call: Box::new(target),
                }),
            )],
        },
        WrapperShape::UtilityBatchAll => WrapperConstruction {
            pallet: "Utility",
            call: "batch_all",
            cases: vec![
                (
                    String::from("utility.batch_all(target,public)"),
                    RuntimeCall::Utility(pallet_utility::Call::batch_all {
                        calls: vec![target.clone(), remark()],
                    }),
                ),
                (
                    String::from("utility.batch_all(public,target)"),
                    RuntimeCall::Utility(pallet_utility::Call::batch_all {
                        calls: vec![remark(), target],
                    }),
                ),
            ],
        },
        WrapperShape::UtilityDispatchAs => WrapperConstruction {
            pallet: "Utility",
            call: "dispatch_as",
            cases: vec![(
                String::from("utility.dispatch_as"),
                RuntimeCall::Utility(pallet_utility::Call::dispatch_as {
                    as_origin: Box::new(signed_origin.clone()),
                    call: Box::new(target),
                }),
            )],
        },
        WrapperShape::UtilityForceBatch => WrapperConstruction {
            pallet: "Utility",
            call: "force_batch",
            cases: vec![
                (
                    String::from("utility.force_batch(target,public)"),
                    RuntimeCall::Utility(pallet_utility::Call::force_batch {
                        calls: vec![target.clone(), remark()],
                    }),
                ),
                (
                    String::from("utility.force_batch(public,target)"),
                    RuntimeCall::Utility(pallet_utility::Call::force_batch {
                        calls: vec![remark(), target],
                    }),
                ),
            ],
        },
        WrapperShape::UtilityWithWeight => WrapperConstruction {
            pallet: "Utility",
            call: "with_weight",
            cases: vec![(
                String::from("utility.with_weight"),
                RuntimeCall::Utility(pallet_utility::Call::with_weight {
                    call: Box::new(target),
                    weight: Weight::zero(),
                }),
            )],
        },
        WrapperShape::UtilityIfElse => WrapperConstruction {
            pallet: "Utility",
            call: "if_else",
            cases: vec![
                (
                    String::from("utility.if_else(main=target)"),
                    RuntimeCall::Utility(pallet_utility::Call::if_else {
                        main: Box::new(target.clone()),
                        fallback: Box::new(remark()),
                    }),
                ),
                (
                    String::from("utility.if_else(fallback=target)"),
                    RuntimeCall::Utility(pallet_utility::Call::if_else {
                        main: Box::new(remark()),
                        fallback: Box::new(target),
                    }),
                ),
            ],
        },
        WrapperShape::UtilityDispatchAsFallible => WrapperConstruction {
            pallet: "Utility",
            call: "dispatch_as_fallible",
            cases: vec![(
                String::from("utility.dispatch_as_fallible"),
                RuntimeCall::Utility(pallet_utility::Call::dispatch_as_fallible {
                    as_origin: Box::new(signed_origin),
                    call: Box::new(target),
                }),
            )],
        },
        WrapperShape::Proxy => WrapperConstruction {
            pallet: "Proxy",
            call: "proxy",
            cases: vec![(
                String::from("proxy.proxy"),
                RuntimeCall::Proxy(pallet_proxy::Call::proxy {
                    real: MultiAddress::Id(who.clone()),
                    force_proxy_type: None,
                    call: Box::new(target),
                }),
            )],
        },
        WrapperShape::ProxyAnnounced => WrapperConstruction {
            pallet: "Proxy",
            call: "proxy_announced",
            cases: vec![(
                String::from("proxy.proxy_announced"),
                RuntimeCall::Proxy(pallet_proxy::Call::proxy_announced {
                    delegate: MultiAddress::Id(who.clone()),
                    real: MultiAddress::Id(account(74)),
                    force_proxy_type: None,
                    call: Box::new(target),
                }),
            )],
        },
        WrapperShape::MultisigAsMultiThreshold1 => WrapperConstruction {
            pallet: "Multisig",
            call: "as_multi_threshold_1",
            cases: vec![(
                String::from("multisig.as_multi_threshold_1"),
                RuntimeCall::Multisig(pallet_multisig::Call::as_multi_threshold_1 {
                    other_signatories: vec![who.clone()],
                    call: Box::new(target),
                }),
            )],
        },
        WrapperShape::MultisigAsMulti => WrapperConstruction {
            pallet: "Multisig",
            call: "as_multi",
            cases: vec![(
                String::from("multisig.as_multi"),
                RuntimeCall::Multisig(pallet_multisig::Call::as_multi {
                    threshold: 2,
                    other_signatories: vec![who],
                    maybe_timepoint: None,
                    call: Box::new(target),
                    max_weight: Weight::zero(),
                }),
            )],
        },
        WrapperShape::MultisigApproveAsMulti => WrapperConstruction {
            pallet: "Multisig",
            call: "approve_as_multi",
            cases: Vec::new(),
        },
        WrapperShape::SchedulerSchedule => WrapperConstruction {
            pallet: "Scheduler",
            call: "schedule",
            cases: vec![(
                String::from("scheduler.schedule"),
                RuntimeCall::Scheduler(pallet_scheduler::Call::schedule {
                    when: 1,
                    maybe_periodic: None,
                    priority: 0,
                    call: Box::new(target),
                }),
            )],
        },
        WrapperShape::SchedulerScheduleNamed => WrapperConstruction {
            pallet: "Scheduler",
            call: "schedule_named",
            cases: vec![(
                String::from("scheduler.schedule_named"),
                RuntimeCall::Scheduler(pallet_scheduler::Call::schedule_named {
                    id: [0; 32],
                    when: 1,
                    maybe_periodic: None,
                    priority: 0,
                    call: Box::new(target),
                }),
            )],
        },
        WrapperShape::SchedulerScheduleAfter => WrapperConstruction {
            pallet: "Scheduler",
            call: "schedule_after",
            cases: vec![(
                String::from("scheduler.schedule_after"),
                RuntimeCall::Scheduler(pallet_scheduler::Call::schedule_after {
                    after: 1,
                    maybe_periodic: None,
                    priority: 0,
                    call: Box::new(target),
                }),
            )],
        },
        WrapperShape::SchedulerScheduleNamedAfter => WrapperConstruction {
            pallet: "Scheduler",
            call: "schedule_named_after",
            cases: vec![(
                String::from("scheduler.schedule_named_after"),
                RuntimeCall::Scheduler(pallet_scheduler::Call::schedule_named_after {
                    id: [0; 32],
                    after: 1,
                    maybe_periodic: None,
                    priority: 0,
                    call: Box::new(target),
                }),
            )],
        },
        WrapperShape::Sudo => WrapperConstruction {
            pallet: "Sudo",
            call: "sudo",
            cases: vec![(
                String::from("sudo.sudo"),
                RuntimeCall::Sudo(pallet_sudo::Call::sudo {
                    call: Box::new(target),
                }),
            )],
        },
        WrapperShape::SudoUncheckedWeight => WrapperConstruction {
            pallet: "Sudo",
            call: "sudo_unchecked_weight",
            cases: vec![(
                String::from("sudo.sudo_unchecked_weight"),
                RuntimeCall::Sudo(pallet_sudo::Call::sudo_unchecked_weight {
                    call: Box::new(target),
                    weight: Weight::zero(),
                }),
            )],
        },
        WrapperShape::SudoAs => WrapperConstruction {
            pallet: "Sudo",
            call: "sudo_as",
            cases: vec![(
                String::from("sudo.sudo_as"),
                RuntimeCall::Sudo(pallet_sudo::Call::sudo_as {
                    who: MultiAddress::Id(who),
                    call: Box::new(target),
                }),
            )],
        },
    }
}

fn wrapper_rows() -> impl Iterator<Item = (&'static InventoryRow, WrapperShape)> {
    INVENTORY.iter().filter_map(|row| match row.expected {
        ExpectedTreatment::Wrapper(shape) => Some((row, shape)),
        _ => None,
    })
}

fn one_level_wrapper_compositions(target: RuntimeCall) -> Vec<(String, RuntimeCall)> {
    wrapper_rows()
        .filter(|(_, shape)| shape.carries_call())
        .flat_map(|(row, shape)| {
            let construction = wrapper_construction(shape, target.clone());
            assert_eq!(
                (construction.pallet, construction.call),
                (row.pallet, row.call)
            );
            construction.cases
        })
        .collect()
}

fn recursive_wrapper_shapes() -> Vec<WrapperShape> {
    wrapper_rows()
        .filter_map(|(_, shape)| {
            matches!(
                privileged_wrapper_policy(shape),
                PrivilegedWrapperPolicy::SameOriginRecursive
                    | PrivilegedWrapperPolicy::RejectPrivileged
            )
            .then_some(shape)
        })
        .collect()
}

fn canonical_wrap(shape: WrapperShape, target: RuntimeCall) -> (String, RuntimeCall) {
    wrapper_construction(shape, target)
        .cases
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{shape:?} is not a call-carrying wrapper"))
}

fn pairwise_recursive_compositions(
    target: RuntimeCall,
) -> Vec<(String, RuntimeCall, WrapperShape, WrapperShape)> {
    let shapes = recursive_wrapper_shapes();
    let mut compositions = Vec::new();
    for outer in &shapes {
        for inner in &shapes {
            let (inner_label, inner_call) = canonical_wrap(*inner, target.clone());
            let (outer_label, call) = canonical_wrap(*outer, inner_call);
            compositions.push((
                format!("{outer_label}({inner_label}(target))"),
                call,
                *outer,
                *inner,
            ));
        }
    }
    compositions
}

fn representative_three_level_compositions(
    target: RuntimeCall,
) -> Vec<(String, RuntimeCall, [WrapperShape; 3])> {
    // Ordered inner-to-outer. The first chain is wholly same-origin recursive;
    // the other two place proxy/multisig at different depths.
    const CHAINS: [[WrapperShape; 3]; 3] = [
        [
            WrapperShape::UtilityBatch,
            WrapperShape::UtilityWithWeight,
            WrapperShape::Sudo,
        ],
        [
            WrapperShape::ProxyAnnounced,
            WrapperShape::UtilityBatchAll,
            WrapperShape::SudoUncheckedWeight,
        ],
        [
            WrapperShape::UtilityForceBatch,
            WrapperShape::MultisigAsMulti,
            WrapperShape::UtilityBatch,
        ],
    ];
    CHAINS
        .into_iter()
        .map(|chain| {
            let mut call = target.clone();
            let mut labels = Vec::new();
            for shape in chain {
                let (label, wrapped) = canonical_wrap(shape, call);
                labels.push(label);
                call = wrapped;
            }
            (format!("three-level {labels:?}"), call, chain)
        })
        .collect()
}

fn generated_closed_wrapper_compositions(call: RuntimeCall) -> Vec<(String, RuntimeCall)> {
    let mut compositions = one_level_wrapper_compositions(call.clone());
    compositions.extend(
        pairwise_recursive_compositions(call.clone())
            .into_iter()
            .map(|(label, call, _, _)| (label, call)),
    );
    compositions.extend(
        representative_three_level_compositions(call)
            .into_iter()
            .map(|(label, call, _)| (label, call)),
    );
    compositions
}

#[test]
fn wrapper_constructor_table_is_mechanically_closed_over_the_inventory() {
    development_ext().execute_with(|| {
        let metadata = RuntimeMetadataModel::load();
        let mut seen_shapes = BTreeSet::new();
        let mut constructed = BTreeSet::new();
        let pinned: BTreeSet<_> = wrapper_rows()
            .filter(|(_, shape)| shape.carries_call())
            .map(|(row, _)| (String::from(row.pallet), String::from(row.call)))
            .collect();

        for (row, shape) in wrapper_rows() {
            assert!(
                seen_shapes.insert(shape),
                "duplicate wrapper shape: {shape:?}"
            );
            let construction = wrapper_construction(shape, remark());
            assert_eq!(
                (construction.pallet, construction.call),
                (row.pallet, row.call),
                "constructor table identity drift for {shape:?}"
            );
            if shape.carries_call() {
                assert!(
                    !construction.cases.is_empty(),
                    "call-carrying wrapper has no constructor cases: {shape:?}"
                );
                for (_, call) in construction.cases {
                    let actual = metadata.call_name(&call);
                    assert_eq!(
                        actual,
                        (String::from(row.pallet), String::from(row.call)),
                        "constructed RuntimeCall identity drift for {shape:?}"
                    );
                    constructed.insert(actual);
                }
            } else {
                assert_eq!(
                    privileged_wrapper_policy(shape),
                    PrivilegedWrapperPolicy::HashOnly
                );
                assert!(construction.cases.is_empty());
            }
        }
        assert_eq!(constructed, pinned);

        let recursive = recursive_wrapper_shapes();
        assert_eq!(
            recursive.len(),
            10,
            "06 §3.3 has six same-origin plus four proxy/multisig recursing shapes"
        );
        assert_eq!(
            recursive.iter().copied().collect::<BTreeSet<_>>().len(),
            recursive.len(),
            "recursing wrapper table contains a duplicate shape"
        );
        assert_eq!(
            pairwise_recursive_compositions(remark()).len(),
            recursive.len().saturating_mul(recursive.len()),
            "pairwise recursing-wrapper closure must be complete"
        );
    });
}

fn denied_projection() -> FilterCall {
    FilterCall::Leaf(CallDomain::Nobody)
}

fn public_projection() -> FilterCall {
    FilterCall::Leaf(CallDomain::Public)
}

fn expected_wrapper_projection(shape: WrapperShape) -> FilterCall {
    let public = public_projection();
    let boxed_public = || BoxedCall(Box::new(public.clone()));
    match shape {
        WrapperShape::UtilityBatch => FilterCall::UtilityBatch(vec![public]),
        WrapperShape::UtilityAsDerivative => FilterCall::UtilityAsDerivative(boxed_public()),
        WrapperShape::UtilityBatchAll => FilterCall::UtilityBatchAll(vec![public]),
        WrapperShape::UtilityDispatchAs => FilterCall::UtilityDispatchAs(boxed_public()),
        WrapperShape::UtilityForceBatch => FilterCall::UtilityForceBatch(vec![public]),
        WrapperShape::UtilityWithWeight => FilterCall::UtilityWithWeight(boxed_public()),
        WrapperShape::Proxy => FilterCall::Proxy(boxed_public()),
        WrapperShape::ProxyAnnounced => FilterCall::ProxyAnnounced(boxed_public()),
        WrapperShape::MultisigAsMultiThreshold1 => {
            FilterCall::MultisigAsMultiThreshold1(boxed_public())
        }
        WrapperShape::MultisigAsMulti => FilterCall::MultisigAsMulti(boxed_public()),
        WrapperShape::MultisigApproveAsMulti => FilterCall::MultisigApproveAsMulti,
        WrapperShape::Sudo | WrapperShape::SudoUncheckedWeight => FilterCall::Sudo(boxed_public()),
        WrapperShape::UtilityIfElse
        | WrapperShape::UtilityDispatchAsFallible
        | WrapperShape::SchedulerSchedule
        | WrapperShape::SchedulerScheduleNamed
        | WrapperShape::SchedulerScheduleAfter
        | WrapperShape::SchedulerScheduleNamedAfter
        | WrapperShape::SudoAs => denied_projection(),
    }
}

fn wrapper_is_origin_blind_admissible(shape: WrapperShape) -> bool {
    matches!(
        shape,
        WrapperShape::UtilityBatch
            | WrapperShape::UtilityBatchAll
            | WrapperShape::UtilityForceBatch
            | WrapperShape::UtilityWithWeight
            | WrapperShape::Proxy
            | WrapperShape::ProxyAnnounced
            | WrapperShape::MultisigAsMultiThreshold1
            | WrapperShape::MultisigAsMulti
            | WrapperShape::MultisigApproveAsMulti
            | WrapperShape::Sudo
            | WrapperShape::SudoUncheckedWeight
    )
}

fn assert_state_independent_projection(
    row: &InventoryRow,
    call: &RuntimeCall,
    covered: &mut BTreeSet<(String, String)>,
) {
    let projected = BleavitSafetyClassifier::project(call);
    match row.expected {
        ExpectedTreatment::Leaf(domain) => {
            assert_eq!(
                projected,
                FilterCall::Leaf(domain),
                "classifier domain drift for {}",
                row_name(row)
            );
            if domain != CallDomain::Public {
                assert!(!SafetyFilter::<BleavitSafetyClassifier>::contains(call));
                assert_eq!(
                    RuntimeBaseCallFilter::contains(call),
                    is_values_enactment_leaf(call),
                    "only a bare values-enactment leaf may widen the raw filter: {}",
                    row_name(row)
                );
                covered.insert((String::from(row.pallet), String::from(row.call)));
            }
        }
        ExpectedTreatment::Denied => {
            assert_eq!(projected, denied_projection(), "{}", row_name(row));
            assert!(!SafetyFilter::<BleavitSafetyClassifier>::contains(call));
            assert!(!RuntimeBaseCallFilter::contains(call));
            for origin in all_class_origins() {
                assert!(!RuntimeBaseCallFilter::contains_for(origin, call));
            }
            covered.insert((String::from(row.pallet), String::from(row.call)));
        }
        ExpectedTreatment::Wrapper(shape) => {
            assert_eq!(
                projected,
                expected_wrapper_projection(shape),
                "wrapper projection drift for {}",
                row_name(row)
            );
            assert_eq!(
                SafetyFilter::<BleavitSafetyClassifier>::contains(call),
                wrapper_is_origin_blind_admissible(shape),
                "raw wrapper admission drift for {}",
                row_name(row)
            );
            assert_eq!(
                RuntimeBaseCallFilter::contains(call),
                wrapper_is_origin_blind_admissible(shape),
                "base wrapper admission drift for {}",
                row_name(row)
            );
            covered.insert((String::from(row.pallet), String::from(row.call)));
        }
        ExpectedTreatment::Conditional(_) => {}
    }
}

fn set_param_call(
    key: futarchy_primitives::ParamKey,
    value: pallet_constitution::ParamValue,
) -> RuntimeCall {
    RuntimeCall::Constitution(pallet_constitution::Call::set_param { key, value })
}

fn amend_registry_call(
    key: futarchy_primitives::ParamKey,
    record: pallet_constitution::ParamRecord,
) -> RuntimeCall {
    RuntimeCall::Constitution(pallet_constitution::Call::amend_registry {
        key,
        min: record.min,
        max: record.max,
        max_delta: record.max_delta,
        cooldown_epochs: record.cooldown_epochs,
    })
}

fn expected_param_domain(class: pallet_constitution::ParamClass) -> CallDomain {
    match class {
        pallet_constitution::ParamClass::Param => CallDomain::Param,
        pallet_constitution::ParamClass::Treasury => CallDomain::Treasury,
        pallet_constitution::ParamClass::Meta | pallet_constitution::ParamClass::MetaAndValues => {
            CallDomain::Meta
        }
        pallet_constitution::ParamClass::Const | pallet_constitution::ParamClass::Entrenched => {
            CallDomain::ConstitutionalValues
        }
    }
}

fn matching_origin(domain: CallDomain) -> Option<ClassOrigin> {
    match domain {
        CallDomain::Param => Some(ClassOrigin::FutarchyParam),
        CallDomain::Treasury => Some(ClassOrigin::FutarchyTreasury),
        CallDomain::Code => Some(ClassOrigin::FutarchyCode),
        CallDomain::Meta => Some(ClassOrigin::FutarchyMeta),
        CallDomain::ConstitutionalValues => Some(ClassOrigin::ConstitutionalValues),
        CallDomain::OracleResolution => Some(ClassOrigin::OracleResolution),
        CallDomain::GuardianHold => Some(ClassOrigin::GuardianHold),
        CallDomain::EmergencyPlaybook => Some(ClassOrigin::EmergencyPlaybook),
        CallDomain::Public | CallDomain::Nobody | CallDomain::InternalRoot => None,
    }
}

fn epoch_privileged_calls() -> [(String, RuntimeCall, CallDomain); 5] {
    [
        (
            String::from("Epoch.set_next_epoch_length"),
            RuntimeCall::Epoch(pallet_epoch::Call::set_next_epoch_length {}),
            CallDomain::ConstitutionalValues,
        ),
        (
            String::from("Epoch.delay_once"),
            RuntimeCall::Epoch(pallet_epoch::Call::delay_once {
                pid: Default::default(),
                justification_hash: Default::default(),
            }),
            CallDomain::GuardianHold,
        ),
        (
            String::from("Epoch.force_reject_process_hold"),
            RuntimeCall::Epoch(pallet_epoch::Call::force_reject_process_hold {
                pid: Default::default(),
            }),
            CallDomain::GuardianHold,
        ),
        (
            String::from("Epoch.void_cohort"),
            RuntimeCall::Epoch(pallet_epoch::Call::void_cohort {
                epoch: Default::default(),
            }),
            CallDomain::EmergencyPlaybook,
        ),
        (
            String::from("Epoch.set_intake_paused"),
            RuntimeCall::Epoch(pallet_epoch::Call::set_intake_paused {
                paused: false,
                expiry: 0,
            }),
            CallDomain::EmergencyPlaybook,
        ),
    ]
}

#[test]
fn every_non_public_inventory_row_is_behaviorally_exercised() {
    let mut covered = BTreeSet::new();
    development_ext().execute_with(|| {
        let metadata = RuntimeMetadataModel::load();
        for row in INVENTORY {
            if !matches!(row.expected, ExpectedTreatment::Conditional(_)) {
                let call = metadata.materialize(row);
                assert_state_independent_projection(row, &call, &mut covered);
            }
        }

        let mut observed_param_classes = [false; 5];
        for (key, record) in pallet_constitution::Params::<Runtime>::iter() {
            let call = set_param_call(key, record.value);
            let domain = expected_param_domain(record.class);
            assert_eq!(
                BleavitSafetyClassifier::project(&call),
                FilterCall::Leaf(domain)
            );
            assert!(!SafetyFilter::<BleavitSafetyClassifier>::contains(&call));
            assert_eq!(
                RuntimeBaseCallFilter::contains(&call),
                matches!(
                    record.class,
                    pallet_constitution::ParamClass::Const
                        | pallet_constitution::ParamClass::Entrenched
                )
            );
            if let Some(origin) = matching_origin(domain) {
                assert!(RuntimeBaseCallFilter::contains_for(origin, &call));
            }
            let index = match record.class {
                pallet_constitution::ParamClass::Param => 0,
                pallet_constitution::ParamClass::Treasury => 1,
                pallet_constitution::ParamClass::Meta => 2,
                pallet_constitution::ParamClass::MetaAndValues => 3,
                pallet_constitution::ParamClass::Const
                | pallet_constitution::ParamClass::Entrenched => 4,
            };
            observed_param_classes[index] = true;
        }
        assert!(observed_param_classes
            .into_iter()
            .all(core::convert::identity));
        covered.insert((String::from("Constitution"), String::from("set_param")));

        // 05 §2.1/§3.2 and 06 §3.2: pin the concrete runtime constructors for
        // every non-Public Epoch leaf, including both custom-origin domains.
        for (name, call, domain) in epoch_privileged_calls() {
            assert_eq!(
                BleavitSafetyClassifier::project(&call),
                FilterCall::Leaf(domain),
                "classifier domain drift for {name}"
            );
            assert!(!SafetyFilter::<BleavitSafetyClassifier>::contains(&call));
            assert_eq!(
                RuntimeBaseCallFilter::contains(&call),
                is_values_enactment_leaf(&call),
                "origin-blind admission drift for {name}"
            );
            for origin in all_class_origins() {
                assert_eq!(
                    RuntimeBaseCallFilter::contains_for(origin, &call),
                    Some(origin) == matching_origin(domain),
                    "authority mismatch: {origin:?} -> {name} ({domain:?})"
                );
            }
        }
    });

    upgrade_ext().execute_with(|| {
        let call =
            RuntimeCall::System(frame_system::Call::apply_authorized_upgrade { code: vec![1] });
        System::set_block_number(10);
        seed_parachain_upgrade_boundary(1);
        set_pending_upgrade(None);
        assert_eq!(BleavitSafetyClassifier::project(&call), denied_projection());
        assert!(!RuntimeBaseCallFilter::contains(&call));
        set_pending_upgrade(Some(11));
        assert_eq!(BleavitSafetyClassifier::project(&call), denied_projection());
        set_pending_upgrade(Some(10));
        assert_eq!(BleavitSafetyClassifier::project(&call), public_projection());
        assert!(SafetyFilter::<BleavitSafetyClassifier>::contains(&call));
        assert!(RuntimeBaseCallFilter::contains(&call));
        set_pending_upgrade(None);
        covered.insert((
            String::from("System"),
            String::from("apply_authorized_upgrade"),
        ));
    });

    // SQ-150 (ruled 2026-07-21): `amend_registry` is now an ordinary
    // FutarchyMeta leaf, exercised by the state-independent loop above like any
    // other non-conditional row — no longer a contested scope carve-out.
    let expected: BTreeSet<_> = INVENTORY
        .iter()
        .filter(|row| !matches!(row.expected, ExpectedTreatment::Leaf(CallDomain::Public)))
        .map(|row| (String::from(row.pallet), String::from(row.call)))
        .collect();
    assert_eq!(
        covered, expected,
        "a non-Public inventory row lacks a behavioral pin"
    );
}

fn wrapper_is_unconditionally_denied(shape: WrapperShape) -> bool {
    matches!(
        shape,
        WrapperShape::UtilityAsDerivative
            | WrapperShape::UtilityDispatchAs
            | WrapperShape::UtilityIfElse
            | WrapperShape::UtilityDispatchAsFallible
            | WrapperShape::SchedulerSchedule
            | WrapperShape::SchedulerScheduleNamed
            | WrapperShape::SchedulerScheduleAfter
            | WrapperShape::SchedulerScheduleNamedAfter
            | WrapperShape::SudoAs
    )
}

fn is_unconditionally_denied(row: &InventoryRow) -> bool {
    matches!(
        row.expected,
        ExpectedTreatment::Denied
            | ExpectedTreatment::Leaf(CallDomain::Nobody | CallDomain::InternalRoot)
    ) || matches!(
        row.expected,
        ExpectedTreatment::Wrapper(shape) if wrapper_is_unconditionally_denied(shape)
    )
}

fn assert_filtered(call: &RuntimeCall, context: &str) {
    assert!(
        !SafetyFilter::<BleavitSafetyClassifier>::contains(call),
        "raw SafetyFilter admitted {context}: {call:?}"
    );
    assert!(
        !RuntimeBaseCallFilter::contains(call),
        "RuntimeBaseCallFilter admitted {context}: {call:?}"
    );
    for origin in all_class_origins() {
        assert!(
            !RuntimeBaseCallFilter::contains_for(origin, call),
            "origin {origin:?} admitted {context}: {call:?}"
        );
    }
}

#[test]
fn every_nobody_treatment_is_denied_bare_wrapped_and_multiply_nested() {
    development_ext().execute_with(|| {
        let metadata = RuntimeMetadataModel::load();
        let mut exercised = 0usize;
        for row in INVENTORY
            .iter()
            .filter(|row| is_unconditionally_denied(row))
        {
            let call = metadata.materialize(row);
            let name = row_name(row);
            assert_filtered(&call, &format!("bare {name}"));
            for (wrapper, wrapped) in generated_closed_wrapper_compositions(call) {
                assert_filtered(&wrapped, &format!("{wrapper}({name})"));
            }
            exercised = exercised.saturating_add(1);
        }
        assert!(exercised > 0);
    });
}

fn privileged_inventory_and_set_param_calls(
    metadata: &RuntimeMetadataModel,
) -> Vec<(String, RuntimeCall, CallDomain)> {
    let mut calls: Vec<_> = INVENTORY
        .iter()
        .filter_map(|row| match row.expected {
            ExpectedTreatment::Leaf(domain) if domain.is_privileged() => {
                Some((row_name(row), metadata.materialize(row), domain))
            }
            _ => None,
        })
        .collect();
    calls.extend(
        pallet_constitution::Params::<Runtime>::iter().map(|(key, record)| {
            (
                format!("Constitution.set_param({key:?})"),
                set_param_call(key, record.value),
                expected_param_domain(record.class),
            )
        }),
    );
    calls
}

#[test]
fn privileged_bare_leaves_follow_the_exhaustive_origin_authority_matrix() {
    development_ext().execute_with(|| {
        let metadata = RuntimeMetadataModel::load();
        let calls = privileged_inventory_and_set_param_calls(&metadata);
        assert!(!calls.is_empty());
        for (name, call, expected_domain) in calls {
            assert!(
                !SafetyFilter::<BleavitSafetyClassifier>::contains(&call),
                "{name}"
            );
            assert_eq!(
                RuntimeBaseCallFilter::contains(&call),
                is_values_enactment_leaf(&call),
                "privileged bare admission must be exactly the SQ-32 values set: {name}"
            );
            for origin in all_class_origins() {
                assert_eq!(
                    RuntimeBaseCallFilter::contains_for(origin, &call),
                    Some(origin) == matching_origin(expected_domain),
                    "bare authority mismatch: {origin:?} -> {name} ({expected_domain:?})"
                );
            }
        }
    });
}

#[test]
fn privileged_wrappers_follow_same_origin_recursion_and_proxyish_denial() {
    // 06 §3.3's explicit rows make utility batch/with-weight and sudo recurse
    // with the SAME custom origin; proxy/multisig alone add the privileged-call
    // denial, and authority-changing/best-effort/scheduler/sudo_as shapes are
    // denied outright. This remediation follows that plain SQ-152 reading;
    // the broader user confirmation tracked by SQ-152 remains pending, and any
    // later ratification must preserve I-11.
    development_ext().execute_with(|| {
        let metadata = RuntimeMetadataModel::load();
        for (name, call, expected_domain) in privileged_inventory_and_set_param_calls(&metadata) {
            for (row, shape) in wrapper_rows().filter(|(_, shape)| shape.carries_call()) {
                let construction = wrapper_construction(shape, call.clone());
                assert_eq!(
                    (construction.pallet, construction.call),
                    (row.pallet, row.call)
                );
                for (wrapper, wrapped) in construction.cases {
                    assert!(
                        !RuntimeBaseCallFilter::contains(&wrapped),
                        "origin-blind base filter admitted {wrapper}({name})"
                    );
                    for origin in all_class_origins() {
                        let expected = matches!(
                            privileged_wrapper_policy(shape),
                            PrivilegedWrapperPolicy::SameOriginRecursive
                        ) && Some(origin) == matching_origin(expected_domain);
                        assert_eq!(
                            RuntimeBaseCallFilter::contains_for(origin, &wrapped),
                            expected,
                            "one-level authority mismatch: {origin:?} -> {wrapper}({name})"
                        );
                    }
                }
            }

            // Complete 10×10 closure over all recursively inspected shapes:
            // the six same-origin shapes plus proxy/proxy_announced and both
            // call-carrying multisig variants.
            for (label, wrapped, outer, inner) in pairwise_recursive_compositions(call.clone()) {
                for origin in all_class_origins() {
                    let same_origin_chain = [outer, inner].into_iter().all(|shape| {
                        matches!(
                            privileged_wrapper_policy(shape),
                            PrivilegedWrapperPolicy::SameOriginRecursive
                        )
                    });
                    assert_eq!(
                        RuntimeBaseCallFilter::contains_for(origin, &wrapped),
                        same_origin_chain && Some(origin) == matching_origin(expected_domain),
                        "pairwise authority mismatch: {origin:?} -> {label} for {name}"
                    );
                }
            }

            for (label, wrapped, chain) in representative_three_level_compositions(call) {
                for origin in all_class_origins() {
                    let same_origin_chain = chain.into_iter().all(|shape| {
                        matches!(
                            privileged_wrapper_policy(shape),
                            PrivilegedWrapperPolicy::SameOriginRecursive
                        )
                    });
                    assert_eq!(
                        RuntimeBaseCallFilter::contains_for(origin, &wrapped),
                        same_origin_chain && Some(origin) == matching_origin(expected_domain),
                        "three-level authority mismatch: {origin:?} -> {label} for {name}"
                    );
                }
            }
        }
    });
}

/// SQ-150 (ruled 2026-07-21), positive META + negative values, exercising the
/// runtime's real `pallet-origins` GovernanceOrigin resolution through the
/// pallet dispatchable (belief-side calls are base-filtered when dispatched
/// externally, so the origin check is exercised via the direct call — the
/// tests.rs:1928 pattern): a non-kernel registry row is amendable by
/// `FutarchyMeta` and refused (`BadOrigin`) for `ConstitutionalValues` and
/// every other class origin. Replaces the ignored `sq_135_*` counterexamples.
#[test]
fn sq_150_amend_registry_is_futarchy_meta_only_in_the_runtime() {
    use sp_runtime::DispatchError;
    development_ext().execute_with(|| {
        let (key, record) = pallet_constitution::Params::<Runtime>::iter()
            .find(|(_, record)| !record.kernel_bounded)
            .expect("genesis must contain a non-kernel registry row");

        // Positive: FutarchyMeta amends within the row's meta-bounds.
        let ok = crate::Constitution::amend_registry(
            pallet_origins::Origin::FutarchyMeta.into(),
            key,
            record.min,
            record.max,
            record.max_delta,
            record.cooldown_epochs,
        );
        assert!(
            ok.is_ok(),
            "SQ-150: FutarchyMeta must amend non-kernel key {key:?}: {ok:?}"
        );

        // Negative: no values/track/class origin may amend a non-kernel row.
        for origin in [
            pallet_origins::Origin::ConstitutionalValues,
            pallet_origins::Origin::OracleResolution,
            pallet_origins::Origin::FutarchyParam,
            pallet_origins::Origin::FutarchyTreasury,
            pallet_origins::Origin::FutarchyCode,
            pallet_origins::Origin::GuardianHold,
            pallet_origins::Origin::EmergencyPlaybook,
        ] {
            let result = crate::Constitution::amend_registry(
                origin.into(),
                key,
                record.min,
                record.max,
                record.max_delta,
                record.cooldown_epochs,
            );
            assert_eq!(
                result,
                Err(DispatchError::BadOrigin),
                "SQ-150: {origin:?} must not amend non-kernel key {key:?}"
            );
        }
    });
}

/// SQ-150 negative-kernel leg in the runtime: kernel-bounded rows are immutable —
/// `FutarchyMeta` clears the origin gate but is refused with the reason-naming
/// `KernelBoundImmutable`, and every other origin is stopped at the origin gate
/// (`BadOrigin`). Either way no origin moves a kernel bound.
#[test]
fn sq_150_kernel_bounded_rows_are_immutable_to_every_origin_in_the_runtime() {
    use sp_runtime::DispatchError;
    development_ext().execute_with(|| {
        let (key, record) = pallet_constitution::Params::<Runtime>::iter()
            .find(|(_, record)| record.kernel_bounded)
            .expect("genesis must contain a kernel-bounded registry row");

        let meta_result = crate::Constitution::amend_registry(
            pallet_origins::Origin::FutarchyMeta.into(),
            key,
            record.min,
            record.max,
            record.max_delta,
            record.cooldown_epochs,
        );
        assert_eq!(
            meta_result,
            Err(pallet_constitution::Error::<Runtime>::KernelBoundImmutable.into()),
            "SQ-150: META on kernel key {key:?} must be KernelBoundImmutable"
        );

        for origin in [
            pallet_origins::Origin::ConstitutionalValues,
            pallet_origins::Origin::FutarchyParam,
            pallet_origins::Origin::FutarchyTreasury,
            pallet_origins::Origin::GuardianHold,
        ] {
            let result = crate::Constitution::amend_registry(
                origin.into(),
                key,
                record.min,
                record.max,
                record.max_delta,
                record.cooldown_epochs,
            );
            assert_eq!(
                result,
                Err(DispatchError::BadOrigin),
                "SQ-150: {origin:?} on kernel key {key:?} must be BadOrigin"
            );
        }
    });
}

/// SQ-150 classifier leg: `amend_registry` projects to the FutarchyMeta domain
/// for **every** registry row (kernel or not — the classifier is not
/// target-aware; immutability binds at dispatch), is NOT a values-enactment
/// leaf, and is denied by the origin-blind base filter (belief-side).
#[test]
fn sq_150_amend_registry_projects_meta_and_is_not_a_values_leaf() {
    development_ext().execute_with(|| {
        let mut observed_kernel = false;
        let mut observed_non_kernel = false;
        for (key, record) in pallet_constitution::Params::<Runtime>::iter() {
            observed_kernel |= record.kernel_bounded;
            observed_non_kernel |= !record.kernel_bounded;
            let call = amend_registry_call(key, record);
            assert_eq!(
                BleavitSafetyClassifier::project(&call),
                FilterCall::Leaf(CallDomain::Meta),
                "SQ-150: amend_registry({key:?}) must project to the FutarchyMeta domain"
            );
            assert!(
                !is_values_enactment_leaf(&call),
                "SQ-150: amend_registry({key:?}) must NOT be a values-enactment leaf"
            );
            assert!(
                !RuntimeBaseCallFilter::contains(&call),
                "SQ-150: amend_registry({key:?}) is belief-side; bare base filter must deny it"
            );
            for origin in all_class_origins() {
                assert_eq!(
                    RuntimeBaseCallFilter::contains_for(origin, &call),
                    matching_origin(CallDomain::Meta) == Some(origin),
                    "SQ-150: {origin:?} has the wrong authority over amend_registry({key:?})"
                );
            }
        }
        assert!(observed_kernel, "genesis must contain a kernel-bounded row");
        assert!(
            observed_non_kernel,
            "genesis must contain a non-kernel registry row"
        );
    });
}

fn all_inventory_and_param_calls(metadata: &RuntimeMetadataModel) -> Vec<(String, RuntimeCall)> {
    let mut calls: Vec<_> = INVENTORY
        .iter()
        .map(|row| (row_name(row), metadata.materialize(row)))
        .collect();
    calls.extend(
        pallet_constitution::Params::<Runtime>::iter().map(|(key, record)| {
            (
                format!("Constitution.set_param({key:?})"),
                set_param_call(key, record.value),
            )
        }),
    );
    calls
}

fn inventory_derived_values_enactment_calls(
    metadata: &RuntimeMetadataModel,
) -> Vec<(String, RuntimeCall)> {
    let mut expected: Vec<_> = INVENTORY
        .iter()
        .filter(|row| {
            matches!(
                row.expected,
                ExpectedTreatment::Leaf(
                    CallDomain::ConstitutionalValues | CallDomain::OracleResolution
                )
            )
        })
        .map(|row| (row_name(row), metadata.materialize(row)))
        .collect();
    expected.extend(
        pallet_constitution::Params::<Runtime>::iter()
            .filter(|(_, record)| {
                matches!(
                    record.class,
                    pallet_constitution::ParamClass::Const
                        | pallet_constitution::ParamClass::Entrenched
                )
            })
            .map(|(key, record)| {
                (
                    format!("Constitution.set_param({key:?})"),
                    set_param_call(key, record.value),
                )
            }),
    );
    expected
}

#[test]
fn inventory_derived_values_enactment_set_is_exact_and_cannot_be_laundered() {
    development_ext().execute_with(|| {
        let metadata = RuntimeMetadataModel::load();
        let expected_calls = inventory_derived_values_enactment_calls(&metadata);
        let expected: BTreeSet<_> = expected_calls
            .iter()
            .map(|(name, _)| name.clone())
            .collect();

        // Every ordinary inventory leaf/denial plus every live set_param class
        // remains eligible to expose an unexpected values-set addition. Only the
        // `set_param` ParamKeyClass conditional (whose values membership is
        // class-derived and covered by the live set_param arm below) is excluded;
        // SQ-150 retired the `amend_registry` conditional — it is now an ordinary
        // FutarchyMeta leaf, so it stays in the universe and MUST NOT appear in
        // the values set.
        let mut comparison_universe: Vec<_> = INVENTORY
            .iter()
            .filter(|row| {
                !matches!(
                    row.expected,
                    ExpectedTreatment::Conditional(ConditionalKind::ParamKeyClass)
                )
            })
            .map(|row| (row_name(row), metadata.materialize(row)))
            .collect();
        comparison_universe.extend(pallet_constitution::Params::<Runtime>::iter().map(
            |(key, record)| {
                (
                    format!("Constitution.set_param({key:?})"),
                    set_param_call(key, record.value),
                )
            },
        ));
        let actual: BTreeSet<_> = comparison_universe
            .into_iter()
            .filter(|(_, call)| is_values_enactment_leaf(call))
            .map(|(name, _)| name)
            .collect();

        let missing: BTreeSet<_> = expected.difference(&actual).cloned().collect();
        assert!(
            missing.is_empty(),
            "missing values-enactment leaves: {missing:?}"
        );
        let extra: BTreeSet<_> = actual.difference(&expected).cloned().collect();
        assert!(
            extra.is_empty(),
            "unexpected values-enactment leaves: {extra:?}"
        );

        for (name, call) in expected_calls {
            assert!(
                is_values_enactment_leaf(&call),
                "independently derived values membership drift: {name}"
            );
            assert!(
                RuntimeBaseCallFilter::contains(&call),
                "bare values admission drift: {name}"
            );
            assert!(
                !SafetyFilter::<BleavitSafetyClassifier>::contains(&call),
                "{name}"
            );
            for (wrapper, wrapped) in generated_closed_wrapper_compositions(call) {
                assert!(
                    !SafetyFilter::<BleavitSafetyClassifier>::contains(&wrapped),
                    "raw filter admitted values laundering via {wrapper}({name})"
                );
                assert!(
                    !RuntimeBaseCallFilter::contains(&wrapped),
                    "base filter admitted values laundering via {wrapper}({name})"
                );
            }
        }
    });
}

#[test]
fn values_domain_leaves_including_foreign_assets_create_are_scheduler_admissible() {
    // SQ-151: every inventory-derived values leaf clears the origin-blind
    // scheduler filter while retaining its origin-aware authority check.
    development_ext().execute_with(|| {
        let metadata = RuntimeMetadataModel::load();
        for (name, call) in inventory_derived_values_enactment_calls(&metadata) {
            assert!(
                is_values_enactment_leaf(&call),
                "values-domain leaf is absent from scheduler admission: {name}"
            );
            assert!(
                RuntimeBaseCallFilter::contains(&call),
                "values-domain leaf is filtered before its origin check: {name}"
            );
            let FilterCall::Leaf(domain) = BleavitSafetyClassifier::project(&call) else {
                panic!("values-domain inventory leaf projected as a wrapper: {name}");
            };
            let origin = matching_origin(domain)
                .unwrap_or_else(|| panic!("values-domain leaf has no matching origin: {name}"));
            assert!(
                RuntimeBaseCallFilter::contains_for(origin, &call),
                "values-domain leaf does not reach its origin check: {name}"
            );
            for (wrapper, wrapped) in generated_closed_wrapper_compositions(call) {
                assert!(
                    !RuntimeBaseCallFilter::contains(&wrapped),
                    "values-domain leaf escaped bare-only admission via {wrapper}({name})"
                );
            }
        }
    });
}

#[test]
fn foreign_assets_create_reaches_and_retains_its_constitutional_origin_check() {
    development_ext().execute_with(|| {
        let asset_id = bleavit_xcm::identity::asset_hub_asset_location(
            chain_identity::USDC_ASSET_INDEX.saturating_add(1),
        );
        let create = RuntimeCall::ForeignAssets(pallet_assets::Call::create {
            id: asset_id.clone(),
            admin: MultiAddress::Id(account(78)),
            min_balance: currency::USDC_CENT,
        });

        assert!(is_values_enactment_leaf(&create));
        assert!(RuntimeBaseCallFilter::contains(&create));
        assert!(RuntimeBaseCallFilter::contains_for(
            ClassOrigin::ConstitutionalValues,
            &create
        ));

        let asset_owner = crate::configs::LedgerPalletId::get().into_account_truncating();
        crate::Balances::force_set_balance(
            RuntimeOrigin::root(),
            MultiAddress::Id(asset_owner),
            currency::VIT,
        )
        .expect("the configured asset owner must be funded for its creation deposit");

        let signed_result = create
            .clone()
            .dispatch(RuntimeOrigin::signed(account(79)));
        assert!(
            matches!(&signed_result, Err(error) if error.error == sp_runtime::DispatchError::BadOrigin),
            "signed ForeignAssets.create must reach and fail CreateOrigin, got {signed_result:?}"
        );
        assert!(!ForeignAssets::asset_exists(asset_id.clone()));

        let values_result =
            create.dispatch(pallet_origins::Origin::ConstitutionalValues.into());
        assert!(
            values_result.is_ok(),
            "ConstitutionalValues ForeignAssets.create must pass the base filter and CreateOrigin: {values_result:?}"
        );
        assert!(ForeignAssets::asset_exists(asset_id.clone()));
    });
}

#[test]
fn nesting_and_total_call_budgets_admit_the_exact_kernel_boundaries_only() {
    let mut at_depth_limit = remark();
    for _ in 0..kernel::MAX_NESTED_LEVELS {
        at_depth_limit = RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![at_depth_limit],
        });
    }
    assert!(SafetyFilter::<BleavitSafetyClassifier>::contains(
        &at_depth_limit
    ));
    assert!(RuntimeBaseCallFilter::contains(&at_depth_limit));
    let past_depth_limit = RuntimeCall::Utility(pallet_utility::Call::batch {
        calls: vec![at_depth_limit],
    });
    assert!(!SafetyFilter::<BleavitSafetyClassifier>::contains(
        &past_depth_limit
    ));
    assert!(!RuntimeBaseCallFilter::contains(&past_depth_limit));

    let inner_at_limit = kernel::MAX_NESTED_CALLS
        .checked_sub(1)
        .expect("a wrapper budget includes at least the outer call");
    let inner_at_limit = usize::try_from(inner_at_limit).expect("kernel call bound fits usize");
    let at_call_limit = RuntimeCall::Utility(pallet_utility::Call::batch {
        calls: vec![remark(); inner_at_limit],
    });
    assert!(SafetyFilter::<BleavitSafetyClassifier>::contains(
        &at_call_limit
    ));
    assert!(RuntimeBaseCallFilter::contains(&at_call_limit));
    let past_call_limit = RuntimeCall::Utility(pallet_utility::Call::batch {
        calls: vec![remark(); inner_at_limit.saturating_add(1)],
    });
    assert!(!SafetyFilter::<BleavitSafetyClassifier>::contains(
        &past_call_limit
    ));
    assert!(!RuntimeBaseCallFilter::contains(&past_call_limit));
}

#[test]
fn internal_root_authorize_upgrade_is_unreachable_bare_and_through_all_compositions() {
    let authorize = RuntimeCall::System(frame_system::Call::authorize_upgrade {
        code_hash: sp_core::H256::repeat_byte(81),
    });
    assert_filtered(&authorize, "bare system.authorize_upgrade");
    for (wrapper, wrapped) in generated_closed_wrapper_compositions(authorize) {
        assert_filtered(&wrapped, &format!("{wrapper}(system.authorize_upgrade)"));
    }
}

fn assert_dispatch_call_filtered(call: RuntimeCall, origin: RuntimeOrigin, context: &str) {
    let result = call.dispatch(origin);
    assert!(
        matches!(result, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()),
        "expected frame_system::CallFiltered for {context}, got {result:?}"
    );
}

#[test]
fn real_dispatch_rejects_every_nobody_treatment_bare_and_at_each_outer_wrapper() {
    development_ext().execute_with(|| {
        let metadata = RuntimeMetadataModel::load();
        let caller = account(75);
        for row in INVENTORY
            .iter()
            .filter(|row| is_unconditionally_denied(row))
        {
            let call = metadata.materialize(row);
            let name = row_name(row);
            assert_dispatch_call_filtered(
                call.clone(),
                RuntimeOrigin::signed(caller.clone()),
                &format!("bare {name}"),
            );
            for (wrapper, wrapped) in one_level_wrapper_compositions(call) {
                assert_dispatch_call_filtered(
                    wrapped,
                    RuntimeOrigin::signed(caller.clone()),
                    &format!("{wrapper}({name})"),
                );
            }
        }

        let apply =
            RuntimeCall::System(frame_system::Call::apply_authorized_upgrade { code: vec![1] });
        set_pending_upgrade(None);
        assert_dispatch_call_filtered(
            apply,
            RuntimeOrigin::signed(caller),
            "state-gated system.apply_authorized_upgrade without pending state",
        );
    });
}

#[test]
fn genesis_sudo_key_is_filtered_before_it_can_bypass_dispatch_for_every_nobody_treatment() {
    development_ext().execute_with(|| {
        let metadata = RuntimeMetadataModel::load();
        let sudo_key = Sr25519Keyring::Alice.to_account_id();
        for row in INVENTORY
            .iter()
            .filter(|row| is_unconditionally_denied(row))
        {
            let name = row_name(row);
            let sudo = RuntimeCall::Sudo(pallet_sudo::Call::sudo {
                call: Box::new(metadata.materialize(row)),
            });
            assert_dispatch_call_filtered(
                sudo,
                RuntimeOrigin::signed(sudo_key.clone()),
                &format!("genesis sudo key -> {name}"),
            );
        }
    });
}

fn is_belief_domain(domain: CallDomain) -> bool {
    matches!(
        domain,
        CallDomain::Param
            | CallDomain::Treasury
            | CallDomain::Code
            | CallDomain::Meta
            | CallDomain::InternalRoot
    )
}

fn assert_i8_for_call(name: &str, call: &RuntimeCall) {
    let projected = BleavitSafetyClassifier::project(call);
    let values = is_values_enactment_leaf(call);
    let FilterCall::Leaf(domain) = projected else {
        assert!(
            !values,
            "values-enactment membership is forbidden for wrapper projection {projected:?}: {name}"
        );
        return;
    };
    if values {
        assert!(
            matches!(
                domain,
                CallDomain::ConstitutionalValues | CallDomain::OracleResolution
            ),
            "values-enactment call projects to belief-side {domain:?}: {name}"
        );
        for origin in futarchy_origins() {
            assert!(
                !RuntimeBaseCallFilter::contains_for(origin, call),
                "Futarchy origin {origin:?} admitted values call {name}"
            );
        }
    }
    if is_belief_domain(domain) {
        assert!(!values, "belief-side call is in the values set: {name}");
        for origin in [
            ClassOrigin::ConstitutionalValues,
            ClassOrigin::OracleResolution,
        ] {
            assert!(
                !RuntimeBaseCallFilter::contains_for(origin, call),
                "values origin {origin:?} admitted belief call {name}"
            );
        }
    }
}

#[test]
fn i8_inventory_values_and_belief_scopes_are_fully_disjoint() {
    // SQ-150 (ruled 2026-07-21) closed the last I-8 crossing: `amend_registry`
    // is now a belief-side FutarchyMeta call and is NO longer in the
    // values-enactment set, so the whole inventory (plus every live set_param
    // class) is disjoint with no carve-out.
    development_ext().execute_with(|| {
        let metadata = RuntimeMetadataModel::load();
        for (name, call) in all_inventory_and_param_calls(&metadata) {
            assert_i8_for_call(&name, &call);
        }
    });
}

#[test]
fn hash_only_multisig_approval_remains_public_and_dispatches_no_inner_call() {
    development_ext().execute_with(|| {
        let alice = Sr25519Keyring::Alice.to_account_id();
        let bob = Sr25519Keyring::Bob.to_account_id();
        let mut signatories = vec![alice.clone(), bob.clone()];
        signatories.sort();
        let multisig_account = Multisig::multi_account_id(&signatories, 2);
        let asset_id = bleavit_xcm::identity::asset_hub_asset_location(
            chain_identity::USDC_ASSET_INDEX.saturating_add(1),
        );
        <ForeignAssets as Create<_>>::create(
            asset_id.clone(),
            multisig_account.clone(),
            true,
            currency::USDC_CENT,
        )
        .expect("test asset must be created for the multisig signed-origin proof");
        let beneficiary = account(76);
        let amount = currency::USDC_CENT;
        let nobody_call = RuntimeCall::ForeignAssets(pallet_assets::Call::mint {
            id: asset_id.clone(),
            beneficiary: MultiAddress::Id(beneficiary.clone()),
            amount,
        });
        assert_eq!(
            BleavitSafetyClassifier::project(&nobody_call),
            denied_projection()
        );
        assert!(!RuntimeBaseCallFilter::contains(&nobody_call));
        let call_hash = sp_io::hashing::blake2_256(&nobody_call.encode());

        let approval = RuntimeCall::Multisig(pallet_multisig::Call::approve_as_multi {
            threshold: 2,
            other_signatories: vec![bob.clone()],
            maybe_timepoint: None,
            call_hash,
            max_weight: nobody_call.get_dispatch_info().call_weight,
        });
        assert_eq!(
            BleavitSafetyClassifier::project(&approval),
            FilterCall::MultisigApproveAsMulti
        );
        assert!(SafetyFilter::<BleavitSafetyClassifier>::contains(&approval));
        assert!(RuntimeBaseCallFilter::contains(&approval));
        let approval_result = approval.dispatch(RuntimeOrigin::signed(alice.clone()));
        assert!(
            approval_result.is_ok(),
            "hash-only approval must remain Public: {approval_result:?}"
        );
        assert_eq!(ForeignAssets::balance(asset_id.clone(), &beneficiary), 0);

        let pending = pallet_multisig::Multisigs::<Runtime>::get(&multisig_account, call_hash)
            .expect("approve_as_multi must prepare real multisig state");
        let terminal = RuntimeCall::Multisig(pallet_multisig::Call::as_multi {
            threshold: 2,
            other_signatories: vec![alice],
            maybe_timepoint: Some(pending.when),
            call: Box::new(nobody_call.clone()),
            max_weight: nobody_call.get_dispatch_info().call_weight,
        });
        assert_dispatch_call_filtered(
            terminal,
            RuntimeOrigin::signed(bob),
            "terminal multisig.as_multi carrying ForeignAssets.mint",
        );
        assert_eq!(
            ForeignAssets::balance(asset_id.clone(), &beneficiary),
            0,
            "hash approval cannot smuggle the nobody-row mint; only the filtered terminal carries it"
        );
        assert!(
            pallet_multisig::Multisigs::<Runtime>::contains_key(&multisig_account, call_hash),
            "outer CallFiltered rejection must occur before terminal multisig state mutation"
        );
    });
}
