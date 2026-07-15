//! 15 §4.1 suites for `pallet-origins`: the `#[pallet::origin]` ↔ core SCALE
//! differential, the `EnsureOrigin` set proving no signed/root/none origin
//! yields a custom governance origin (G-5, I-10), the closed-wrapper-set
//! negative suite over the base filter (I-10/I-11), the values⟂beliefs scope
//! split (I-8), and the stateless try-state.

use crate::mock::*;
use crate::{
    BoxedCall, CallDomain, ClassOrigin, EnsureConstitutionalValues, EnsureEmergencyPlaybook,
    EnsureFutarchyCode, EnsureFutarchyMeta, EnsureFutarchyOrigin, EnsureFutarchyParam,
    EnsureFutarchyTreasury, EnsureGuardianHold, EnsureOracleResolution, FilterCall, FilterError,
    ModelClassifier, Origin, SafetyClassifier, SafetyFilter, MAX_NESTED_CALLS, MAX_NESTED_LEVELS,
};
use frame_support::traits::{Contains, EnsureOrigin};
use futarchy_primitives::ProposalClass;
use parity_scale_codec::Encode;

// ------------------------------------------------------------ helpers --------

fn boxed(call: FilterCall) -> BoxedCall {
    BoxedCall::new(call)
}

/// Build the runtime origin carrying a custom governance origin.
fn custom(o: Origin) -> RuntimeOrigin {
    o.into()
}

/// Filter admissibility through the FRAME `Contains` over the model call.
fn admits(call: &FilterCall) -> bool {
    <SafetyFilter<ModelClassifier> as Contains<FilterCall>>::contains(call)
}

/// Origin-aware admissibility (guard step 5 / scheduler re-entry).
fn admits_for(origin: Origin, call: &FilterCall) -> bool {
    SafetyFilter::<ModelClassifier>::contains_for(origin, call)
}

fn validate(origin: Option<ClassOrigin>, call: &FilterCall) -> Result<(), FilterError> {
    SafetyFilter::<ModelClassifier>::validate(origin, call)
}

// ------------------------------------------- origin surface & differential ---

#[test]
fn eight_custom_origins_in_frozen_order() {
    assert_eq!(Origin::ALL.len(), 8);
    // 06 §3.1 declaration/index order.
    assert_eq!(
        Origin::ALL,
        [
            Origin::FutarchyParam,
            Origin::FutarchyTreasury,
            Origin::FutarchyCode,
            Origin::FutarchyMeta,
            Origin::ConstitutionalValues,
            Origin::OracleResolution,
            Origin::GuardianHold,
            Origin::EmergencyPlaybook,
        ]
    );
}

#[test]
fn frame_origin_encodes_byte_for_byte_with_core_origin() {
    // Shell-vs-core lockstep: the FRAME `#[pallet::origin]` and the frame-free
    // model origin must share SCALE indices so the differential oracle, the
    // execution-guard core (I-11) and the frontend port all agree.
    for (i, origin) in Origin::ALL.iter().enumerate() {
        let model: ClassOrigin = (*origin).into();
        assert_eq!(origin.encode(), model.encode(), "variant {i} diverged");
        // Index equals declaration order.
        assert_eq!(origin.encode(), alloc_vec_u8(i as u8));
        // Round-trips both ways.
        assert_eq!(Origin::from(model), *origin);
        assert_eq!(origin.to_model(), model);
    }
}

fn alloc_vec_u8(b: u8) -> alloc::vec::Vec<u8> {
    alloc::vec![b]
}

#[test]
fn from_proposal_class_covers_the_belief_classes() {
    assert_eq!(
        Origin::from_proposal_class(ProposalClass::Param),
        Some(Origin::FutarchyParam)
    );
    assert_eq!(
        Origin::from_proposal_class(ProposalClass::Treasury),
        Some(Origin::FutarchyTreasury)
    );
    assert_eq!(
        Origin::from_proposal_class(ProposalClass::Code),
        Some(Origin::FutarchyCode)
    );
    assert_eq!(
        Origin::from_proposal_class(ProposalClass::Meta),
        Some(Origin::FutarchyMeta)
    );
    // Constitutional routes to the values track — no belief-side origin (D-7).
    assert_eq!(
        Origin::from_proposal_class(ProposalClass::Constitutional),
        None
    );
    // Agrees with the frame-free core.
    for class in [
        ProposalClass::Param,
        ProposalClass::Treasury,
        ProposalClass::Code,
        ProposalClass::Meta,
        ProposalClass::Constitutional,
    ] {
        assert_eq!(
            Origin::from_proposal_class(class).map(Origin::to_model),
            ClassOrigin::from_proposal_class(class)
        );
    }
}

// ---------------------------------------------- EnsureOrigin set (G-5/I-10) --

/// Each unit `EnsureOrigin` accepts exactly its own custom origin and rejects
/// every other custom origin, every signed account, root, and none — the
/// type-level proof that no external origin can synthesize a governance origin.
#[test]
fn unit_ensures_accept_only_their_own_custom_origin() {
    macro_rules! check {
        ($ensure:ty, $origin:expr) => {{
            // Accepts its own.
            assert!(<$ensure>::try_origin(custom($origin)).is_ok());
            // Rejects the other seven custom origins.
            for other in Origin::ALL.iter().filter(|o| **o != $origin) {
                assert!(
                    <$ensure>::try_origin(custom(*other)).is_err(),
                    "{:?} must not satisfy {}",
                    other,
                    stringify!($ensure)
                );
            }
            // Rejects signed / root / none — no external origin escalates.
            assert!(<$ensure>::try_origin(RuntimeOrigin::signed(1)).is_err());
            assert!(<$ensure>::try_origin(RuntimeOrigin::signed(42)).is_err());
            assert!(<$ensure>::try_origin(RuntimeOrigin::root()).is_err());
            assert!(<$ensure>::try_origin(RuntimeOrigin::none()).is_err());
        }};
    }
    check!(EnsureFutarchyParam, Origin::FutarchyParam);
    check!(EnsureFutarchyTreasury, Origin::FutarchyTreasury);
    check!(EnsureFutarchyCode, Origin::FutarchyCode);
    check!(EnsureFutarchyMeta, Origin::FutarchyMeta);
    check!(EnsureConstitutionalValues, Origin::ConstitutionalValues);
    check!(EnsureOracleResolution, Origin::OracleResolution);
    check!(EnsureGuardianHold, Origin::GuardianHold);
    check!(EnsureEmergencyPlaybook, Origin::EmergencyPlaybook);
}

#[test]
fn ensure_futarchy_origin_accepts_any_custom_origin_and_returns_it() {
    // `RuntimeOrigin` has no `PartialEq`, so compare on the `Ok` projection.
    for o in Origin::ALL {
        assert_eq!(EnsureFutarchyOrigin::try_origin(custom(o)).ok(), Some(o));
    }
    assert!(EnsureFutarchyOrigin::try_origin(RuntimeOrigin::signed(1)).is_err());
    assert!(EnsureFutarchyOrigin::try_origin(RuntimeOrigin::root()).is_err());
    assert!(EnsureFutarchyOrigin::try_origin(RuntimeOrigin::none()).is_err());
}

#[test]
fn ensure_origin_returns_the_original_origin_on_rejection() {
    // FRAME contract: a rejected `try_origin` hands the origin back untouched
    // so the next `EnsureOrigin` in an `EitherOfDiverse` chain can try it.
    let treasury = custom(Origin::FutarchyTreasury);
    let returned = EnsureFutarchyParam::try_origin(treasury)
        .expect_err("param ensure must reject a treasury origin");
    // Handed back intact: it still satisfies its own ensure.
    assert!(EnsureFutarchyTreasury::try_origin(returned).is_ok());

    let signed = RuntimeOrigin::signed(7);
    let returned = EnsureFutarchyParam::try_origin(signed)
        .expect_err("param ensure must reject a signed origin");
    // Still the original signed origin.
    let raw: Result<frame_system::RawOrigin<u64>, RuntimeOrigin> = returned.into();
    assert!(matches!(raw, Ok(frame_system::RawOrigin::Signed(7))));
}

// --------------------------------------------- base filter: closed wrappers --

#[test]
fn nobody_row_is_denied_bare_and_under_every_wrapper() {
    assert_eq!(
        validate(None, &FilterCall::leaf(CallDomain::Nobody)),
        Err(FilterError::NobodyCall)
    );
    // Exhaustive over *every* call-carrying wrapper variant (06 §3.3 closed set):
    // the "nobody" row is denied no matter how it is nested. `proxy_announced`,
    // `as_multi`, `as_multi_threshold_1` and `scheduler` are included so the
    // "under every wrapper" claim is literally true (A4 Codex-review gap).
    for wrapped in [
        FilterCall::UtilityBatch(alloc::vec![FilterCall::leaf(CallDomain::Nobody)]),
        FilterCall::UtilityBatchAll(alloc::vec![FilterCall::leaf(CallDomain::Nobody)]),
        FilterCall::UtilityForceBatch(alloc::vec![FilterCall::leaf(CallDomain::Nobody)]),
        FilterCall::UtilityWithWeight(boxed(FilterCall::leaf(CallDomain::Nobody))),
        FilterCall::Sudo(boxed(FilterCall::leaf(CallDomain::Nobody))),
        FilterCall::Proxy(boxed(FilterCall::leaf(CallDomain::Nobody))),
        FilterCall::ProxyAnnounced(boxed(FilterCall::leaf(CallDomain::Nobody))),
        FilterCall::MultisigAsMulti(boxed(FilterCall::leaf(CallDomain::Nobody))),
        FilterCall::MultisigAsMultiThreshold1(boxed(FilterCall::leaf(CallDomain::Nobody))),
        // Deeply buried under a values-scheduled batch: still caught.
        FilterCall::Scheduler {
            origin: ClassOrigin::ConstitutionalValues,
            call: boxed(FilterCall::UtilityBatch(alloc::vec![FilterCall::leaf(
                CallDomain::Nobody
            )])),
        },
    ] {
        assert!(!admits(&wrapped), "{wrapped:?} must be denied");
        assert_eq!(validate(None, &wrapped), Err(FilterError::NobodyCall));
    }
}

#[test]
fn public_calls_pass_the_base_filter() {
    assert!(admits(&FilterCall::leaf(CallDomain::Public)));
    assert!(admits(&FilterCall::UtilityBatch(alloc::vec![
        FilterCall::leaf(CallDomain::Public),
        FilterCall::leaf(CallDomain::Public),
    ])));
    assert!(admits(&FilterCall::Proxy(boxed(FilterCall::leaf(
        CallDomain::Public
    )))));
}

#[test]
fn bare_privileged_leaf_needs_a_matching_custom_origin() {
    // Origin-less base filter refuses it (a signed submitter has no matching
    // custom origin); the guard's origin-aware check admits it only for the
    // matching origin. This is the "two independent checks" of 06 §3.3.
    let param = FilterCall::leaf(CallDomain::Param);
    assert!(!admits(&param));
    assert_eq!(validate(None, &param), Err(FilterError::BadOrigin));
    assert!(admits_for(Origin::FutarchyParam, &param));
    assert!(!admits_for(Origin::FutarchyTreasury, &param));
    assert_eq!(
        validate(Some(ClassOrigin::FutarchyTreasury), &param),
        Err(FilterError::BadOrigin)
    );
}

#[test]
fn proxy_and_multisig_deny_privileged_inner_including_proxy_announced_and_threshold1() {
    // 06 §3.3: the previously-bypassable `proxy_announced` and
    // `as_multi_threshold_1` are now recursed identically to `proxy`/`as_multi`
    // (I-10 closed wrapper set).
    for call in [
        FilterCall::Proxy(boxed(FilterCall::leaf(CallDomain::Param))),
        FilterCall::ProxyAnnounced(boxed(FilterCall::leaf(CallDomain::Meta))),
        FilterCall::MultisigAsMulti(boxed(FilterCall::leaf(CallDomain::Code))),
        FilterCall::MultisigAsMultiThreshold1(boxed(FilterCall::leaf(CallDomain::Treasury))),
    ] {
        assert!(!admits(&call));
        assert_eq!(validate(None, &call), Err(FilterError::PrivilegedWrapper));
        // Even the matching custom origin cannot launder a privileged call
        // through a proxy/multisig — no governance flow does this (G-5).
        for o in Origin::ALL {
            assert!(!admits_for(o, &call), "{o:?} laundered a privileged proxy");
        }
    }
}

#[test]
fn privileged_leaf_cannot_be_laundered_through_nested_wrappers_inside_a_proxy() {
    // A batch / with_weight / sudo layer between the proxy and the privileged
    // leaf must not launder the 06 §3.3 denial (Codex PR #18 regression).
    for call in [
        FilterCall::Proxy(boxed(FilterCall::UtilityBatch(alloc::vec![
            FilterCall::leaf(CallDomain::Param),
        ]))),
        FilterCall::ProxyAnnounced(boxed(FilterCall::UtilityWithWeight(boxed(
            FilterCall::leaf(CallDomain::Meta),
        )))),
        FilterCall::MultisigAsMulti(boxed(FilterCall::UtilityBatchAll(alloc::vec![
            FilterCall::leaf(CallDomain::Code),
        ]))),
        FilterCall::Proxy(boxed(FilterCall::Sudo(boxed(FilterCall::leaf(
            CallDomain::Param,
        ))))),
    ] {
        assert_eq!(validate(None, &call), Err(FilterError::PrivilegedWrapper));
        assert!(!admits_for(Origin::FutarchyParam, &call));
        assert!(!admits_for(Origin::FutarchyCode, &call));
    }
    // A public payload under the same shape stays admissible.
    assert!(admits(&FilterCall::Proxy(boxed(FilterCall::UtilityBatch(
        alloc::vec![FilterCall::leaf(CallDomain::Public)]
    )))));
}

#[test]
fn dispatch_as_and_as_derivative_are_denied_entirely() {
    for call in [
        FilterCall::UtilityDispatchAs(boxed(FilterCall::leaf(CallDomain::Public))),
        FilterCall::UtilityAsDerivative(boxed(FilterCall::leaf(CallDomain::Public))),
    ] {
        assert!(!admits(&call));
        assert_eq!(validate(None, &call), Err(FilterError::DispatchAsDenied));
    }
}

#[test]
fn scheduler_is_values_only_and_revalidates_the_captured_origin() {
    // Only the values-enactment origins may be captured; the inner call is then
    // re-checked against that captured origin (06 §3.4).
    let good = FilterCall::Scheduler {
        origin: ClassOrigin::ConstitutionalValues,
        call: boxed(FilterCall::leaf(CallDomain::ConstitutionalValues)),
    };
    assert!(admits(&good));

    let bad_origin = FilterCall::Scheduler {
        origin: ClassOrigin::GuardianHold,
        call: boxed(FilterCall::leaf(CallDomain::GuardianHold)),
    };
    assert_eq!(
        validate(None, &bad_origin),
        Err(FilterError::SchedulerDenied)
    );

    let bad_call = FilterCall::Scheduler {
        origin: ClassOrigin::ConstitutionalValues,
        call: boxed(FilterCall::leaf(CallDomain::Treasury)),
    };
    assert_eq!(validate(None, &bad_call), Err(FilterError::BadOrigin));
}

#[test]
fn nesting_depth_and_total_call_budgets_are_enforced() {
    // MAX_NESTED_LEVELS + 1 batch levels overflow the depth budget.
    let mut too_deep = FilterCall::leaf(CallDomain::Public);
    for _ in 0..=MAX_NESTED_LEVELS {
        too_deep = FilterCall::UtilityBatch(alloc::vec![too_deep]);
    }
    assert_eq!(validate(None, &too_deep), Err(FilterError::TooDeep));

    // MAX_NESTED_CALLS + 1 leaves in one batch overflow the call budget.
    let too_many = FilterCall::UtilityBatch(
        (0..=MAX_NESTED_CALLS)
            .map(|_| FilterCall::leaf(CallDomain::Public))
            .collect(),
    );
    assert_eq!(validate(None, &too_many), Err(FilterError::TooManyCalls));
}

// ----------------------------------------------- I-8 values ⟂ beliefs scope --

#[test]
fn values_and_belief_scopes_are_disjoint() {
    // A values origin cannot reach any belief-class domain…
    for belief in [
        CallDomain::Param,
        CallDomain::Treasury,
        CallDomain::Code,
        CallDomain::Meta,
    ] {
        assert!(!admits_for(
            Origin::ConstitutionalValues,
            &FilterCall::leaf(belief)
        ));
    }
    // …and a belief origin cannot reach the values / oracle domains.
    for beliefs in [
        Origin::FutarchyParam,
        Origin::FutarchyTreasury,
        Origin::FutarchyCode,
        Origin::FutarchyMeta,
    ] {
        assert!(!admits_for(
            beliefs,
            &FilterCall::leaf(CallDomain::ConstitutionalValues)
        ));
        assert!(!admits_for(
            beliefs,
            &FilterCall::leaf(CallDomain::OracleResolution)
        ));
    }
    // Each scope reaches exactly its own domain.
    assert!(admits_for(
        Origin::ConstitutionalValues,
        &FilterCall::leaf(CallDomain::ConstitutionalValues)
    ));
    assert!(admits_for(
        Origin::OracleResolution,
        &FilterCall::leaf(CallDomain::OracleResolution)
    ));
    assert!(admits_for(
        Origin::FutarchyTreasury,
        &FilterCall::leaf(CallDomain::Treasury)
    ));
    // The oracle origin is confined to oracle adjudication.
    assert!(!admits_for(
        Origin::OracleResolution,
        &FilterCall::leaf(CallDomain::ConstitutionalValues)
    ));
}

// --------------------------------------------------- classifier projection ---

/// A non-identity classifier over a bespoke call type, proving `project` is
/// actually consulted (the real B1a classifier is exactly this shape).
enum DemoCall {
    Harmless,
    Governed,
    Dangerous,
}

struct DemoClassifier;

impl SafetyClassifier for DemoClassifier {
    type Call = DemoCall;
    fn project(call: &DemoCall) -> FilterCall {
        match call {
            DemoCall::Harmless => FilterCall::leaf(CallDomain::Public),
            DemoCall::Governed => FilterCall::leaf(CallDomain::Meta),
            DemoCall::Dangerous => FilterCall::leaf(CallDomain::Nobody),
        }
    }
}

#[test]
fn classifier_projection_drives_the_generic_filter() {
    assert!(<SafetyFilter<DemoClassifier> as Contains<DemoCall>>::contains(&DemoCall::Harmless));
    assert!(!<SafetyFilter<DemoClassifier> as Contains<DemoCall>>::contains(&DemoCall::Dangerous));
    // Governed is a bare privileged leaf: refused origin-less, admitted only for
    // the matching origin.
    assert!(!<SafetyFilter<DemoClassifier> as Contains<DemoCall>>::contains(&DemoCall::Governed));
    assert!(SafetyFilter::<DemoClassifier>::contains_for(
        Origin::FutarchyMeta,
        &DemoCall::Governed
    ));
    assert!(!SafetyFilter::<DemoClassifier>::contains_for(
        Origin::FutarchyParam,
        &DemoCall::Governed
    ));
}

// -------------------------------------------------- runtime integration -----

#[test]
fn origin_pallet_composes_into_a_runtime() {
    // The mock's `construct_runtime!` includes `Origins` with its
    // `#[pallet::origin]`; prove the externalities build and run, and that a
    // custom origin round-trips through the aggregate `RuntimeOrigin`.
    new_test_ext().execute_with(|| {
        assert_eq!(System::block_number(), 1);
        let origin = custom(Origin::GuardianHold);
        assert!(EnsureGuardianHold::try_origin(origin).is_ok());
    });
}

// --------------------------------------------------------------- try-state ---

#[cfg(feature = "try-runtime")]
#[test]
fn stateless_try_state_is_green() {
    use frame_support::traits::Hooks;
    new_test_ext().execute_with(|| {
        assert!(crate::Pallet::<Test>::try_state(1).is_ok());
    });
}
