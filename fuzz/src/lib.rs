#![deny(unsafe_code)]

use arbitrary::{Arbitrary, Unstructured};
use execution_guard_core::{
    hash_payload, AttestationView, CallDomain as GuardDomain, DispatchCall, EpochHandoff,
    Error as GuardError, ExecutionGuard, GuardOrigin, GuardianView, Payload, QueuedExecution,
    MAX_CALLS, MAX_PAYLOAD_BYTES,
};
use futarchy_fixed::{
    lmsr_buy_cost, lmsr_cost, lmsr_price_long, lmsr_price_short, FixedError, FixedU64x64, LmsrSide,
    LN_2,
};
use futarchy_primitives::{
    currency::USDC,
    kernel::{LMSR_DOMAIN_BOUND, MAX_NESTED_CALLS, MAX_NESTED_LEVELS},
    Balance, BoundedVec, Branch, GateType, PositionId, PositionKind, ProposalClass, ScalarSide,
};
use market_core::{
    buy_book, fee_up, seed_book, sell_book, BookKind, Error as MarketError, Event as MarketEvent,
    LedgerOps, MarketBook, MarketParams, MarketState, FEE_BPS, MIN_TRADE,
};
use origins_core::{
    BoxedCall, CallDomain, Error as FilterError, Origin, RuntimeCall, SafetyFilter,
};
use parity_scale_codec::{DecodeLimit, Encode, MemTrackingInput};
use std::convert::TryFrom;

/// Enough codec reference depth for the payload vector plus the four protocol
/// wrapper levels. The allocation budget rejects adversarial vector lengths
/// before the post-decode protocol bounds are checked.
pub const SCALE_DECODE_DEPTH_LIMIT: u32 = MAX_NESTED_LEVELS + 4;
pub const SCALE_DECODE_MEMORY_LIMIT: usize = 1024 * 1024;

pub fn decode_payload_bounded(data: &[u8]) -> Result<Payload, parity_scale_codec::Error> {
    let mut bytes = data;
    let mut input = MemTrackingInput::new(&mut bytes, SCALE_DECODE_MEMORY_LIMIT);
    Payload::decode_with_depth_limit(SCALE_DECODE_DEPTH_LIMIT, &mut input)
}

pub fn decode_call_bounded(data: &[u8]) -> Result<RuntimeCall, parity_scale_codec::Error> {
    let mut bytes = data;
    let mut input = MemTrackingInput::new(&mut bytes, SCALE_DECODE_MEMORY_LIMIT);
    RuntimeCall::decode_with_depth_limit(SCALE_DECODE_DEPTH_LIMIT, &mut input)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OracleError {
    NobodyCall,
    BadOrigin,
    PrivilegedWrapper,
    DispatchAsDenied,
    SchedulerDenied,
    TooDeep,
    TooManyCalls,
}

#[derive(Clone, Copy, Debug)]
struct OracleBudget {
    depth: u32,
    calls: u32,
}

impl OracleBudget {
    const fn new() -> Self {
        Self { depth: 0, calls: 0 }
    }

    fn count(&mut self) -> Result<(), OracleError> {
        self.calls = self.calls.checked_add(1).ok_or(OracleError::TooManyCalls)?;
        if self.calls > MAX_NESTED_CALLS {
            return Err(OracleError::TooManyCalls);
        }
        Ok(())
    }

    fn enter(&mut self) -> Result<(), OracleError> {
        self.depth = self.depth.checked_add(1).ok_or(OracleError::TooDeep)?;
        if self.depth > MAX_NESTED_LEVELS {
            return Err(OracleError::TooDeep);
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }
}

/// Independent executable form of the closed wrapper table in 06 §3.3.
pub fn oracle_validate(origin: Option<Origin>, call: &RuntimeCall) -> Result<(), OracleError> {
    oracle_walk(origin, call, &mut OracleBudget::new(), false)
}

/// The execution guard's top-level payload calls share one total-call budget.
pub fn oracle_validate_batch(
    origin: Option<Origin>,
    calls: &[RuntimeCall],
) -> Result<(), OracleError> {
    let mut budget = OracleBudget::new();
    for call in calls {
        oracle_walk(origin, call, &mut budget, false)?;
    }
    Ok(())
}

fn oracle_walk(
    origin: Option<Origin>,
    call: &RuntimeCall,
    budget: &mut OracleBudget,
    in_proxyish_wrapper: bool,
) -> Result<(), OracleError> {
    budget.count()?;
    match call {
        RuntimeCall::Leaf(domain) => {
            if *domain == CallDomain::Nobody {
                Err(OracleError::NobodyCall)
            } else if domain.is_privileged() && in_proxyish_wrapper {
                Err(OracleError::PrivilegedWrapper)
            } else if domain.allowed_for(origin) {
                Ok(())
            } else {
                Err(OracleError::BadOrigin)
            }
        }
        RuntimeCall::UtilityBatch(calls)
        | RuntimeCall::UtilityBatchAll(calls)
        | RuntimeCall::UtilityForceBatch(calls) => {
            oracle_many(origin, calls, budget, in_proxyish_wrapper)
        }
        RuntimeCall::UtilityWithWeight(inner) | RuntimeCall::Sudo(inner) => {
            oracle_wrapped(origin, &inner.0, budget, in_proxyish_wrapper)
        }
        RuntimeCall::Proxy(inner)
        | RuntimeCall::ProxyAnnounced(inner)
        | RuntimeCall::MultisigAsMulti(inner)
        | RuntimeCall::MultisigAsMultiThreshold1(inner) => {
            oracle_wrapped(origin, &inner.0, budget, true)
        }
        RuntimeCall::MultisigApproveAsMulti => Ok(()),
        RuntimeCall::UtilityDispatchAs(_) | RuntimeCall::UtilityAsDerivative(_) => {
            Err(OracleError::DispatchAsDenied)
        }
        RuntimeCall::Scheduler { origin, call } => {
            if !matches!(
                origin,
                Origin::ConstitutionalValues | Origin::OracleResolution
            ) {
                return Err(OracleError::SchedulerDenied);
            }
            oracle_wrapped(Some(*origin), &call.0, budget, in_proxyish_wrapper)
        }
    }
}

fn oracle_many(
    origin: Option<Origin>,
    calls: &[RuntimeCall],
    budget: &mut OracleBudget,
    in_proxyish_wrapper: bool,
) -> Result<(), OracleError> {
    budget.enter()?;
    for call in calls {
        if let Err(error) = oracle_walk(origin, call, budget, in_proxyish_wrapper) {
            budget.leave();
            return Err(error);
        }
    }
    budget.leave();
    Ok(())
}

fn oracle_wrapped(
    origin: Option<Origin>,
    call: &RuntimeCall,
    budget: &mut OracleBudget,
    in_proxyish_wrapper: bool,
) -> Result<(), OracleError> {
    budget.enter()?;
    let result = oracle_walk(origin, call, budget, in_proxyish_wrapper);
    budget.leave();
    result
}

/// 1:1 projection of the filter's error taxonomy onto the harness copy, so the
/// differential can compare error *variants* (not just accept/deny). A missing
/// arm here would be a compile error if `origins_core::Error` ever grows a
/// variant — an intentional tripwire.
pub fn map_filter_error(error: FilterError) -> OracleError {
    match error {
        FilterError::NobodyCall => OracleError::NobodyCall,
        FilterError::BadOrigin => OracleError::BadOrigin,
        FilterError::PrivilegedWrapper => OracleError::PrivilegedWrapper,
        FilterError::DispatchAsDenied => OracleError::DispatchAsDenied,
        FilterError::SchedulerDenied => OracleError::SchedulerDenied,
        FilterError::TooDeep => OracleError::TooDeep,
        FilterError::TooManyCalls => OracleError::TooManyCalls,
    }
}

/// An **independent** statement of the 06 §3.3 admission rule that does NOT share
/// `SafetyFilter::validate`'s single-pass budget walk: admission is the AND of
/// separately-computed properties (node count, container-nesting depth, a
/// structural-denial scan, and a per-leaf privilege/origin predicate). Because
/// it is derived from the closed-table invariants rather than transcribed from
/// the implementation, a divergence catches a shared logic/spec error — not just
/// a one-sided refactor. Used to differentially check the filter for BOTH the
/// origin-less and the `Some(class_origin)` (guard dispatch) paths.
pub fn independent_admits(origin: Option<Origin>, call: &RuntimeCall) -> bool {
    node_count(call) <= MAX_NESTED_CALLS
        && container_depth(call) <= MAX_NESTED_LEVELS
        && admissible_tree(call, false, origin)
}

/// Every `RuntimeCall` node is visited once by `validate_inner`'s `count_call`
/// (the container node counts too). `dispatch_as`/`as_derivative` short-circuit
/// before descending, but such a tree is structurally denied regardless of
/// count, so counting their (unvisited) inner is immaterial to accept/deny.
fn node_count(call: &RuntimeCall) -> u32 {
    1 + match call {
        RuntimeCall::Leaf(_) | RuntimeCall::MultisigApproveAsMulti => 0,
        RuntimeCall::UtilityBatch(calls)
        | RuntimeCall::UtilityBatchAll(calls)
        | RuntimeCall::UtilityForceBatch(calls) => calls.iter().map(node_count).sum(),
        RuntimeCall::UtilityDispatchAs(inner)
        | RuntimeCall::UtilityAsDerivative(inner)
        | RuntimeCall::UtilityWithWeight(inner)
        | RuntimeCall::Proxy(inner)
        | RuntimeCall::ProxyAnnounced(inner)
        | RuntimeCall::MultisigAsMulti(inner)
        | RuntimeCall::MultisigAsMultiThreshold1(inner)
        | RuntimeCall::Sudo(inner) => node_count(&inner.0),
        RuntimeCall::Scheduler { call, .. } => node_count(&call.0),
    }
}

/// Container-nesting depth = the number of `enter`/`leave` levels `validate_inner`
/// descends (batches and every single-child wrapper; leaves and the hash-only
/// `approve_as_multi` do not descend).
fn container_depth(call: &RuntimeCall) -> u32 {
    match call {
        RuntimeCall::Leaf(_)
        | RuntimeCall::MultisigApproveAsMulti
        | RuntimeCall::UtilityDispatchAs(_)
        | RuntimeCall::UtilityAsDerivative(_) => 0,
        RuntimeCall::UtilityBatch(calls)
        | RuntimeCall::UtilityBatchAll(calls)
        | RuntimeCall::UtilityForceBatch(calls) => {
            1 + calls.iter().map(container_depth).max().unwrap_or(0)
        }
        RuntimeCall::UtilityWithWeight(inner)
        | RuntimeCall::Proxy(inner)
        | RuntimeCall::ProxyAnnounced(inner)
        | RuntimeCall::MultisigAsMulti(inner)
        | RuntimeCall::MultisigAsMultiThreshold1(inner)
        | RuntimeCall::Sudo(inner) => 1 + container_depth(&inner.0),
        RuntimeCall::Scheduler { call, .. } => 1 + container_depth(&call.0),
    }
}

/// Structural + privilege admissibility (assumes count/depth already checked):
/// no `dispatch_as`/`as_derivative` or `Nobody` leaf anywhere; every scheduler
/// carries a values origin (CV|OR) and re-scopes its subtree to that captured
/// origin; every privileged leaf is not under a proxyish wrapper and satisfies
/// `allowed_for(effective_origin)`.
fn admissible_tree(call: &RuntimeCall, under_proxyish: bool, origin: Option<Origin>) -> bool {
    match call {
        RuntimeCall::Leaf(CallDomain::Nobody) => false,
        RuntimeCall::Leaf(domain) => {
            if domain.is_privileged() && under_proxyish {
                false
            } else {
                domain.allowed_for(origin)
            }
        }
        RuntimeCall::MultisigApproveAsMulti => true,
        RuntimeCall::UtilityDispatchAs(_) | RuntimeCall::UtilityAsDerivative(_) => false,
        RuntimeCall::UtilityBatch(calls)
        | RuntimeCall::UtilityBatchAll(calls)
        | RuntimeCall::UtilityForceBatch(calls) => calls
            .iter()
            .all(|inner| admissible_tree(inner, under_proxyish, origin)),
        RuntimeCall::UtilityWithWeight(inner) | RuntimeCall::Sudo(inner) => {
            admissible_tree(&inner.0, under_proxyish, origin)
        }
        RuntimeCall::Proxy(inner)
        | RuntimeCall::ProxyAnnounced(inner)
        | RuntimeCall::MultisigAsMulti(inner)
        | RuntimeCall::MultisigAsMultiThreshold1(inner) => admissible_tree(&inner.0, true, origin),
        RuntimeCall::Scheduler {
            origin: captured,
            call,
        } => {
            matches!(
                captured,
                Origin::ConstitutionalValues | Origin::OracleResolution
            ) && admissible_tree(&call.0, under_proxyish, Some(*captured))
        }
    }
}

#[derive(Clone, Debug)]
pub struct WrapperCase {
    pub origin: Option<Origin>,
    pub calls: Vec<RuntimeCall>,
}

const GENERATED_TREE_DEPTH: u8 = 6;
const GENERATED_CALL_LIMIT: u8 = 24;

impl<'a> Arbitrary<'a> for WrapperCase {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let origin = arbitrary_optional_origin(u)?;
        let roots = u.int_in_range(1u8..=4)?;
        let mut remaining = GENERATED_CALL_LIMIT;
        let mut calls = Vec::with_capacity(roots as usize);
        for _ in 0..roots {
            calls.push(arbitrary_call(u, 0, &mut remaining)?);
        }
        Ok(Self { origin, calls })
    }
}

fn arbitrary_optional_origin(u: &mut Unstructured<'_>) -> arbitrary::Result<Option<Origin>> {
    let discriminant = u.int_in_range(0u8..=8)?;
    Ok(if discriminant == 0 {
        None
    } else {
        Some(origin_from_byte(discriminant - 1))
    })
}

fn origin_from_byte(value: u8) -> Origin {
    match value % 8 {
        0 => Origin::FutarchyParam,
        1 => Origin::FutarchyTreasury,
        2 => Origin::FutarchyCode,
        3 => Origin::FutarchyMeta,
        4 => Origin::ConstitutionalValues,
        5 => Origin::OracleResolution,
        6 => Origin::GuardianHold,
        _ => Origin::EmergencyPlaybook,
    }
}

fn domain_from_byte(value: u8) -> CallDomain {
    match value % 11 {
        0 => CallDomain::Public,
        1 => CallDomain::Nobody,
        2 => CallDomain::Param,
        3 => CallDomain::Treasury,
        4 => CallDomain::Code,
        5 => CallDomain::Meta,
        6 => CallDomain::ConstitutionalValues,
        7 => CallDomain::OracleResolution,
        8 => CallDomain::GuardianHold,
        9 => CallDomain::EmergencyPlaybook,
        _ => CallDomain::InternalRoot,
    }
}

fn arbitrary_call(
    u: &mut Unstructured<'_>,
    depth: u8,
    remaining: &mut u8,
) -> arbitrary::Result<RuntimeCall> {
    if *remaining == 0 {
        return Ok(RuntimeCall::Leaf(CallDomain::Public));
    }
    *remaining -= 1;
    let variant = if depth >= GENERATED_TREE_DEPTH {
        0
    } else {
        u.int_in_range(0u8..=14)?
    };
    let boxed = |call| BoxedCall::new(call);
    Ok(match variant {
        0 => RuntimeCall::Leaf(domain_from_byte(u.arbitrary()?)),
        1..=3 => {
            let child_count = u.int_in_range(0u8..=4)?.min(*remaining);
            let mut children = Vec::with_capacity(child_count as usize);
            for _ in 0..child_count {
                children.push(arbitrary_call(u, depth + 1, remaining)?);
            }
            match variant {
                1 => RuntimeCall::UtilityBatch(children),
                2 => RuntimeCall::UtilityBatchAll(children),
                _ => RuntimeCall::UtilityForceBatch(children),
            }
        }
        4 => RuntimeCall::UtilityDispatchAs(boxed(arbitrary_call(u, depth + 1, remaining)?)),
        5 => RuntimeCall::UtilityAsDerivative(boxed(arbitrary_call(u, depth + 1, remaining)?)),
        6 => RuntimeCall::UtilityWithWeight(boxed(arbitrary_call(u, depth + 1, remaining)?)),
        7 => RuntimeCall::Proxy(boxed(arbitrary_call(u, depth + 1, remaining)?)),
        8 => RuntimeCall::ProxyAnnounced(boxed(arbitrary_call(u, depth + 1, remaining)?)),
        9 => RuntimeCall::MultisigAsMulti(boxed(arbitrary_call(u, depth + 1, remaining)?)),
        10 => {
            RuntimeCall::MultisigAsMultiThreshold1(boxed(arbitrary_call(u, depth + 1, remaining)?))
        }
        11 => RuntimeCall::MultisigApproveAsMulti,
        12 => RuntimeCall::Scheduler {
            origin: origin_from_byte(u.arbitrary()?),
            call: boxed(arbitrary_call(u, depth + 1, remaining)?),
        },
        _ => RuntimeCall::Sudo(boxed(arbitrary_call(u, depth + 1, remaining)?)),
    })
}

pub fn assert_wrapper_case(case: &WrapperCase) {
    for call in &case.calls {
        let filter = SafetyFilter::validate(case.origin, call);
        let actual = filter.is_ok();

        // (1) Ordered structural oracle — agrees on accept/deny AND, where both
        // reject, on the exact error variant (catches error-taxonomy drift).
        let oracle = oracle_validate(case.origin, call);
        assert_eq!(
            actual,
            oracle.is_ok(),
            "single-call oracle disagreement: {call:?}"
        );
        if let (Err(filter_err), Err(oracle_err)) = (filter, oracle) {
            assert_eq!(
                map_filter_error(filter_err),
                oracle_err,
                "oracle/filter error-variant disagreement: {call:?}"
            );
        }

        // (2) INDEPENDENT admission predicate (property AND, not a walk clone) —
        // checked for BOTH the origin-less and the Some(class_origin) paths, so
        // the guard's privileged-wrapper dispatch case is covered, not just the
        // filter-vs-clone agreement.
        assert_eq!(
            actual,
            independent_admits(case.origin, call),
            "independent-admission disagreement: origin={:?} {call:?}",
            case.origin
        );

        match case.origin {
            None => assert_eq!(SafetyFilter::contains(call), actual),
            Some(origin) => assert_eq!(SafetyFilter::contains_for(origin, call), actual),
        }

        if SafetyFilter::validate(None, call).is_ok() {
            assert!(!contains_nobody(call));
            // A valid scheduler establishes a new, values-only captured origin.
            // Privileged leaves below that boundary are not caller-controlled.
            assert!(!contains_unscoped_privileged(call, false));
        }
    }

    let actual_batch = SafetyFilter::validate_batch(case.origin, &case.calls).is_ok();
    assert_eq!(
        actual_batch,
        oracle_validate_batch(case.origin, &case.calls).is_ok(),
        "shared-budget batch oracle disagreement: {:?}",
        case.calls
    );
    // The batch shares one budget across top-level calls; the independent
    // predicate, applied per top-level call, must agree on the batch verdict
    // (a batch admits iff every top-level call admits under the shared budget —
    // equivalently, iff the aggregate node count / max depth stay within bounds
    // and every call is individually admissible).
    let independent_batch = case.calls.iter().map(node_count).sum::<u32>() <= MAX_NESTED_CALLS
        && case.calls.iter().map(container_depth).max().unwrap_or(0) <= MAX_NESTED_LEVELS
        && case
            .calls
            .iter()
            .all(|call| admissible_tree(call, false, case.origin));
    assert_eq!(
        actual_batch, independent_batch,
        "independent shared-budget batch disagreement: {:?}",
        case.calls
    );
}

pub fn assert_raw_call_decode(data: &[u8]) {
    if let Ok(call) = decode_call_bounded(data) {
        let encoded = call.encode();
        let decoded = decode_call_bounded(&encoded).expect("encoded call must decode");
        assert_eq!(decoded, call);
        let case = WrapperCase {
            origin: None,
            calls: vec![call],
        };
        assert_wrapper_case(&case);
    }
}

fn contains_nobody(call: &RuntimeCall) -> bool {
    match call {
        RuntimeCall::Leaf(domain) => *domain == CallDomain::Nobody,
        RuntimeCall::UtilityBatch(calls)
        | RuntimeCall::UtilityBatchAll(calls)
        | RuntimeCall::UtilityForceBatch(calls) => calls.iter().any(contains_nobody),
        RuntimeCall::UtilityDispatchAs(inner)
        | RuntimeCall::UtilityAsDerivative(inner)
        | RuntimeCall::UtilityWithWeight(inner)
        | RuntimeCall::Proxy(inner)
        | RuntimeCall::ProxyAnnounced(inner)
        | RuntimeCall::MultisigAsMulti(inner)
        | RuntimeCall::MultisigAsMultiThreshold1(inner)
        | RuntimeCall::Sudo(inner) => contains_nobody(&inner.0),
        RuntimeCall::Scheduler { call, .. } => contains_nobody(&call.0),
        RuntimeCall::MultisigApproveAsMulti => false,
    }
}

fn contains_unscoped_privileged(call: &RuntimeCall, under_scheduler: bool) -> bool {
    match call {
        RuntimeCall::Leaf(domain) => !under_scheduler && domain.is_privileged(),
        RuntimeCall::UtilityBatch(calls)
        | RuntimeCall::UtilityBatchAll(calls)
        | RuntimeCall::UtilityForceBatch(calls) => calls
            .iter()
            .any(|call| contains_unscoped_privileged(call, under_scheduler)),
        RuntimeCall::UtilityDispatchAs(inner)
        | RuntimeCall::UtilityAsDerivative(inner)
        | RuntimeCall::UtilityWithWeight(inner)
        | RuntimeCall::Proxy(inner)
        | RuntimeCall::ProxyAnnounced(inner)
        | RuntimeCall::MultisigAsMulti(inner)
        | RuntimeCall::MultisigAsMultiThreshold1(inner)
        | RuntimeCall::Sudo(inner) => contains_unscoped_privileged(&inner.0, under_scheduler),
        RuntimeCall::Scheduler { call, .. } => contains_unscoped_privileged(&call.0, true),
        RuntimeCall::MultisigApproveAsMulti => false,
    }
}

#[derive(Default)]
struct NoopEpoch;

impl EpochHandoff for NoopEpoch {
    fn mark_executed(&mut self, _: u64) -> Result<(), GuardError> {
        Ok(())
    }
    fn mark_failed_executed(&mut self, _: u64) -> Result<(), GuardError> {
        Ok(())
    }
    fn retry_exhausted_to_measurement(&mut self, _: u64) -> Result<(), GuardError> {
        Ok(())
    }
    fn reject_or_stale(
        &mut self,
        _: u64,
        _: futarchy_primitives::RejectReason,
    ) -> Result<(), GuardError> {
        Ok(())
    }
}

struct OpenGuardian;
impl GuardianView for OpenGuardian {
    fn rerun_held(&self, _: u64) -> bool {
        false
    }
    fn gate_suspended(&self) -> bool {
        false
    }
    fn ledger_freeze_active(&self, _: u32) -> bool {
        false
    }
}

struct QuorateAttestations;
impl AttestationView for QuorateAttestations {
    fn present_and_quorate(&self, _: u64, _: [u8; 32], _: u32, _: u32) -> bool {
        true
    }
}

fn version() -> futarchy_primitives::RuntimeVersionConstraint {
    futarchy_primitives::RuntimeVersionConstraint {
        spec_name: BoundedVec::try_from(b"bleavit-fuzz".to_vec()).expect("bounded literal"),
        spec_version: 1,
    }
}

fn class_for_calls(calls: &[DispatchCall]) -> ProposalClass {
    calls
        .iter()
        .find_map(|call| match call.domain {
            GuardDomain::Public => None,
            GuardDomain::Param => Some(ProposalClass::Param),
            GuardDomain::Treasury => Some(ProposalClass::Treasury),
            GuardDomain::Code
            | GuardDomain::InternalRootAuthorizeUpgrade
            | GuardDomain::InternalRootApplyUpgrade => Some(ProposalClass::Code),
            GuardDomain::Meta => Some(ProposalClass::Meta),
        })
        .unwrap_or(ProposalClass::Param)
}

fn declared_domains(calls: &[DispatchCall]) -> Vec<GuardDomain> {
    let mut domains = Vec::new();
    for call in calls {
        if !domains.contains(&call.domain) {
            domains.push(call.domain);
        }
    }
    domains
}

fn guard_probe(payload: Payload, committed_hash: [u8; 32]) -> Result<(), GuardError> {
    let payload_len = payload
        .calls
        .iter()
        .try_fold(0u32, |sum, call| sum.checked_add(call.encoded_len))
        .unwrap_or(MAX_PAYLOAD_BYTES.saturating_add(1));
    let class = class_for_calls(&payload.calls);
    let mut guard = ExecutionGuard::new(version());
    let queued = QueuedExecution {
        pid: 1,
        payload_hash: committed_hash,
        payload_len,
        class,
        maturity: 0,
        grace_end: u32::MAX,
        version_constraint: version(),
        meters_declared: Vec::new(),
        ratify_ref: None,
        ratification_passed: true,
        attestation_id: Some(1),
        pre_upgrade_checkpoint: None,
        cancelled: false,
        declared_domains: declared_domains(&payload.calls),
        failed_at: None,
    };
    guard.enqueue(GuardOrigin::EpochDecision, queued)?;
    let result = guard.execute_with(
        GuardOrigin::Signed,
        &mut NoopEpoch,
        &OpenGuardian,
        &QuorateAttestations,
        1,
        payload,
        0,
        [1; 32],
        [2; 32],
    );
    if result.is_ok() {
        assert_eq!(guard.try_state(), Ok(()));
    }
    result
}

/// Raw-bytes decode path: decode an untrusted `Payload` behind the recursion /
/// allocation limits, round-trip it, and drive the guard. Random bytes rarely
/// form a valid `Payload`, so this leans on the committed corpus; the structured
/// generator (`assert_structured_payload`) is what explores the guard's pass and
/// bound-rejection paths on arbitrary input.
pub fn assert_payload_bytes(data: &[u8]) {
    let Ok(payload) = decode_payload_bounded(data) else {
        return;
    };
    let encoded = payload.encode();
    assert_eq!(
        decode_payload_bounded(&encoded).expect("encoded payload must decode"),
        payload
    );
    assert_payload_invariants(payload);
}

/// The guard invariants a decoded/generated `Payload` must satisfy: the
/// committed hash binds the declared fields but NOT the outcome fields
/// (`succeeds`/`error`); the guard returns only typed errors (never a
/// panic/trap) and, when it admits, the count/length bounds hold; the guard's
/// verdict is independent of the queue-value `hash` field (only `hash_payload`
/// of the calls binds); and a one-bit mutation of a hash-bound field breaks the
/// preimage.
fn assert_payload_invariants(payload: Payload) {
    let committed_hash = hash_payload(&payload.calls);
    let mut outcome_mutated = payload.clone();
    for call in &mut outcome_mutated.calls {
        call.succeeds = !call.succeeds;
        call.error[0] ^= 0xff;
    }
    assert_eq!(hash_payload(&outcome_mutated.calls), committed_hash);

    let result = guard_probe(payload.clone(), committed_hash);
    if let Err(error) = result {
        assert!(is_known_guard_error(error));
    } else {
        let total_len = payload
            .calls
            .iter()
            .try_fold(0u32, |sum, call| sum.checked_add(call.encoded_len))
            .expect("admitted payload length cannot overflow");
        assert!(payload.calls.len() <= MAX_CALLS);
        assert!(total_len <= MAX_PAYLOAD_BYTES);
    }

    let mut hash_field_mutated = payload.clone();
    hash_field_mutated.hash[0] ^= 0xff;
    assert_eq!(
        guard_probe(hash_field_mutated, committed_hash),
        guard_probe(payload.clone(), committed_hash)
    );

    if !payload.calls.is_empty() {
        let mut substituted = payload;
        substituted.calls[0].declared_weight ^= 1;
        assert_eq!(
            guard_probe(substituted, committed_hash),
            Err(GuardError::BadPreimage)
        );
    }
}

/// A structurally-generated payload case: unlike raw bytes (which almost never
/// SCALE-decode into a valid `Payload`), this reaches the guard's admit and
/// bound-rejection paths on arbitrary fuzzer input.
#[derive(Clone, Debug)]
pub struct PayloadCase {
    pub payload: Payload,
    /// When true, `payload.hash` is set to the true `hash_payload(calls)` so the
    /// queue commitment matches; otherwise it is left arbitrary (the guard must
    /// still fail closed, never trap).
    pub committed: bool,
}

impl<'a> Arbitrary<'a> for PayloadCase {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let count = u.int_in_range(0u8..=18)?;
        let mut budget = GENERATED_CALL_LIMIT;
        let mut calls = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let domain = guard_domain_from_byte(u.arbitrary()?);
            // A mix of small, boundary, and overflow-inducing declared lengths so
            // both the `<= MAX_PAYLOAD_BYTES` gate and the checked-add overflow
            // path are exercised.
            let encoded_len = match u.int_in_range(0u8..=3)? {
                0 => u.int_in_range(0u32..=64)?,
                1 => MAX_PAYLOAD_BYTES,
                2 => u.arbitrary()?,
                _ => u32::MAX,
            };
            calls.push(DispatchCall {
                domain,
                encoded_len,
                declared_weight: u.arbitrary()?,
                call: arbitrary_call(u, 0, &mut budget)?,
                succeeds: u.arbitrary()?,
                error: u.arbitrary()?,
                upgrade_hash: None,
                target_spec_version: None,
            });
        }
        let committed = u.arbitrary()?;
        let hash = if committed {
            hash_payload(&calls)
        } else {
            u.arbitrary::<[u8; 32]>()?
        };
        Ok(Self {
            payload: Payload { hash, calls },
            committed,
        })
    }
}

fn guard_domain_from_byte(value: u8) -> GuardDomain {
    match value % 5 {
        0 => GuardDomain::Public,
        1 => GuardDomain::Param,
        2 => GuardDomain::Treasury,
        3 => GuardDomain::Code,
        _ => GuardDomain::Meta,
    }
}

pub fn assert_structured_payload(case: PayloadCase) {
    // Round-trips through the bounded decoder (the generated tree respects the
    // decode depth/mem caps), then runs the full guard invariant battery.
    let encoded = case.payload.encode();
    if let Ok(decoded) = decode_payload_bounded(&encoded) {
        assert_eq!(decoded, case.payload);
    }
    assert_payload_invariants(case.payload);
}

// Enumerates every current `GuardError`, so this predicate is presently always
// true — its purpose is a compile-plus-run tripwire: if the guard ever grows a
// variant, this arm list (and the reviewer's attention) must be updated. The
// real assertion value on the guard's error path is that it returns SOME typed
// error and never panics/traps on adversarial input.
fn is_known_guard_error(error: GuardError) -> bool {
    matches!(
        error,
        GuardError::BadOrigin
            | GuardError::QueueFull
            | GuardError::NotFound
            | GuardError::Cancelled
            | GuardError::NotMature
            | GuardError::GraceExpired
            | GuardError::BadPreimage
            | GuardError::StaleQueue
            | GuardError::NotRatified
            | GuardError::AttestationMissing
            | GuardError::CapabilityDenied
            | GuardError::MetersBlocked
            | GuardError::ResourceLockMissing
            | GuardError::GuardianHold
            | GuardError::FreezeActive
            | GuardError::PayloadTooLarge
            | GuardError::TooManyCalls
            | GuardError::TooManyDomains
            | GuardError::TooManyLocks
            | GuardError::BadDomainDeclaration
            | GuardError::SafetyFilter
            | GuardError::DispatchFailed
            | GuardError::BadUpgradePayload
            | GuardError::PendingUpgradeExists
            | GuardError::NoPendingUpgrade
            | GuardError::DescriptorLeadTime
            | GuardError::UpgradeHashMismatch
            | GuardError::UpgradeVersionMismatch
            | GuardError::RetryWindowOpen
            | GuardError::Overflow
    )
}

#[derive(Clone, Copy, Debug)]
pub struct TradeOp {
    pub buy: bool,
    pub side: ScalarSide,
    pub selector: u16,
}

#[derive(Clone, Debug)]
pub struct TradeCase {
    pub b: Balance,
    /// Which LMSR book the sequence runs against — Decision (either branch),
    /// Gate (either branch, either `GateType`), or the unbranched Baseline. The
    /// solvency invariants (04 §6.3, I-12) hold identically across all three
    /// wrapper shapes; `sell_baseline` in particular was historically
    /// solvency-buggy, so exercising every kind is load-bearing coverage.
    pub kind: BookKind,
    pub round_trip_selector: u16,
    pub trades: Vec<TradeOp>,
}

/// Derive the book kind from the high bits of the first trade record's flag
/// bytes. Those bits are 0 in every committed decision-book seed (the generator
/// writes only bit 0 of each flag), so this keeps the committed corpus on the
/// Decision/Accept path while letting the fuzzer reach Gate and Baseline books.
fn kind_from_seed(data: &[u8]) -> BookKind {
    // bit 0 of `data[10]` is the first op's buy flag; bits 1+ are unused there.
    let sel = data.get(10).copied().unwrap_or(0) >> 1;
    let branch = if sel & 0b100 == 0 {
        Branch::Accept
    } else {
        Branch::Reject
    };
    match sel % 4 {
        0 | 1 => BookKind::Decision {
            proposal: 1,
            branch,
        },
        2 => {
            let gate = if sel & 0b1000 == 0 {
                GateType::Survival
            } else {
                GateType::Security
            };
            BookKind::Gate {
                proposal: 1,
                branch,
                gate,
            }
        }
        _ => BookKind::Baseline { epoch: 1 },
    }
}

impl TradeCase {
    /// Deterministic seed format: little-endian `b_usdc` offset, a little-endian
    /// round-trip selector, then `(buy, side, selector_le)` records; the book
    /// kind is derived from the first record's flag high-bits (see
    /// [`kind_from_seed`]).
    pub fn from_seed_bytes(data: &[u8]) -> Self {
        let mut b_bytes = [0u8; 8];
        let b_len = data.len().min(8);
        b_bytes[..b_len].copy_from_slice(&data[..b_len]);
        // b ranges over [100, 10_000_000] USDC (a sensible book-subsidy span);
        // `USDC` is the kernel-owned base-unit scale (rule 4, not a hardcode).
        let b_offset = u64::from_le_bytes(b_bytes) % (10_000_000 - 100 + 1);
        let b = Balance::from(100 + b_offset) * USDC;
        let round_trip_selector = if data.len() >= 10 {
            u16::from_le_bytes([data[8], data[9]])
        } else {
            0
        };
        let mut trades = Vec::new();
        for chunk in data.get(10..).unwrap_or_default().chunks_exact(4).take(24) {
            trades.push(TradeOp {
                buy: chunk[0] & 1 == 0,
                side: if chunk[1] & 1 == 0 {
                    ScalarSide::Long
                } else {
                    ScalarSide::Short
                },
                selector: u16::from_le_bytes([chunk[2], chunk[3]]),
            });
        }
        Self {
            b,
            kind: kind_from_seed(data),
            round_trip_selector,
            trades,
        }
    }
}

impl<'a> Arbitrary<'a> for TradeCase {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let bytes = u.bytes(u.len())?;
        Ok(Self::from_seed_bytes(bytes))
    }
}

#[derive(Clone, Debug, Default)]
pub struct MockLedger {
    balances: Vec<(PositionId, u8, Balance)>,
}

impl MockLedger {
    fn balance(&self, id: PositionId, who: &u8) -> Balance {
        self.balances
            .iter()
            .find(|(held, owner, _)| *held == id && owner == who)
            .map_or(0, |(_, _, balance)| *balance)
    }

    fn credit(&mut self, id: PositionId, who: u8, amount: Balance) -> Result<(), ()> {
        if let Some((_, _, balance)) = self
            .balances
            .iter_mut()
            .find(|(held, owner, _)| *held == id && *owner == who)
        {
            *balance = balance.checked_add(amount).ok_or(())?;
        } else if amount > 0 {
            self.balances.push((id, who, amount));
        }
        Ok(())
    }

    fn debit(&mut self, id: PositionId, who: u8, amount: Balance) -> Result<(), ()> {
        let (_, _, balance) = self
            .balances
            .iter_mut()
            .find(|(held, owner, _)| *held == id && *owner == who)
            .ok_or(())?;
        *balance = balance.checked_sub(amount).ok_or(())?;
        Ok(())
    }

    fn transfer(&mut self, id: PositionId, from: u8, to: u8, amount: Balance) -> Result<(), ()> {
        self.debit(id, from, amount)?;
        self.credit(id, to, amount)
    }
}

fn proposal_position(proposal: u64, branch: Branch, kind: PositionKind) -> PositionId {
    PositionId::Proposal {
        proposal,
        branch,
        kind,
    }
}

fn baseline_position(epoch: u32, side: ScalarSide) -> PositionId {
    PositionId::Baseline { epoch, side }
}

fn scalar_kind(side: ScalarSide) -> PositionKind {
    match side {
        ScalarSide::Long => PositionKind::Long,
        ScalarSide::Short => PositionKind::Short,
    }
}

/// Gate books map Long ↦ YES, Short ↦ NO (04 §11.2 / market-core `gate_kind`).
fn gate_side_kind(gate: GateType, side: ScalarSide) -> PositionKind {
    match side {
        ScalarSide::Long => PositionKind::GateYes(gate),
        ScalarSide::Short => PositionKind::GateNo(gate),
    }
}

/// The inventory position a `side` leg lives in for the given book kind — read
/// on both the book account (held inventory) and the trader (delivered legs).
/// Decision uses `Long`/`Short`, Gate uses `GateYes`/`GateNo`, Baseline uses the
/// unbranched `PositionId::Baseline{epoch, side}` (finding D / 04 §6.1, §8.3).
fn side_position(kind: BookKind, side: ScalarSide) -> PositionId {
    match kind {
        BookKind::Decision { proposal, branch } => {
            proposal_position(proposal, branch, scalar_kind(side))
        }
        BookKind::Gate {
            proposal,
            branch,
            gate,
        } => proposal_position(proposal, branch, gate_side_kind(gate, side)),
        BookKind::Baseline { epoch } => baseline_position(epoch, side),
    }
}

/// The book's liquid branch-USDC position (where recycled revenue lands between
/// the split and the scalar re-split). Baseline books are unbranched and hold no
/// branch-USDC leg, so the liquid term is absent there.
fn liquid_position(kind: BookKind) -> Option<PositionId> {
    match kind {
        BookKind::Decision { proposal, branch }
        | BookKind::Gate {
            proposal, branch, ..
        } => Some(proposal_position(
            proposal,
            branch,
            PositionKind::BranchUsdc,
        )),
        BookKind::Baseline { .. } => None,
    }
}

/// USDC-equivalent the book's inventory absorbs on a buy. Decision/Gate route the
/// fee to the fees account, so the book keeps `cost`; the unbranched Baseline
/// wrapper retains the whole `cost + fee` complete pair in the book (04 §6.1), so
/// its inventory grows by the fee-inclusive total.
///
/// Sells need no kind split: the 04 §6.1 payout-sized merge converts exactly
/// `net + fee == proceeds` of paired legs into USDC on every kind (Decision/Gate
/// pay the fees account and the seller out of the liquid branch-USDC leg;
/// Baseline re-splits `net` into the seller's pairs and holds the withheld fee
/// as USDC custody outside the tracked legs) while the seller's returned
/// `amount` joins book inventory — every sell drains the tracked inventory by
/// the gross `proceeds`.
fn inventory_inflow_on_buy(kind: BookKind, cost: Balance) -> Balance {
    match kind {
        BookKind::Baseline { .. } => cost + fee_up(cost, FEE_BPS).expect("fee in range"),
        _ => cost,
    }
}

impl LedgerOps<u8> for MockLedger {
    fn do_split(&mut self, pid: u64, who: &u8, amount: Balance) -> Result<(), ()> {
        for branch in [Branch::Accept, Branch::Reject] {
            self.credit(
                proposal_position(pid, branch, PositionKind::BranchUsdc),
                *who,
                amount,
            )?;
        }
        Ok(())
    }

    fn do_transfer(
        &mut self,
        id: PositionId,
        from: &u8,
        to: &u8,
        amount: Balance,
    ) -> Result<(), ()> {
        self.transfer(id, *from, *to, amount)
    }

    fn do_split_scalar(
        &mut self,
        pid: u64,
        branch: Branch,
        who: &u8,
        amount: Balance,
    ) -> Result<(), ()> {
        self.debit(
            proposal_position(pid, branch, PositionKind::BranchUsdc),
            *who,
            amount,
        )?;
        for kind in [PositionKind::Long, PositionKind::Short] {
            self.credit(proposal_position(pid, branch, kind), *who, amount)?;
        }
        Ok(())
    }

    fn do_split_gate(
        &mut self,
        pid: u64,
        branch: Branch,
        gate: GateType,
        who: &u8,
        amount: Balance,
    ) -> Result<(), ()> {
        self.debit(
            proposal_position(pid, branch, PositionKind::BranchUsdc),
            *who,
            amount,
        )?;
        for kind in [PositionKind::GateYes(gate), PositionKind::GateNo(gate)] {
            self.credit(proposal_position(pid, branch, kind), *who, amount)?;
        }
        Ok(())
    }

    fn do_split_baseline(&mut self, epoch: u32, who: &u8, amount: Balance) -> Result<(), ()> {
        self.credit(baseline_position(epoch, ScalarSide::Long), *who, amount)?;
        self.credit(baseline_position(epoch, ScalarSide::Short), *who, amount)
    }

    fn do_merge(&mut self, pid: u64, who: &u8, amount: Balance) -> Result<(), ()> {
        for branch in [Branch::Accept, Branch::Reject] {
            self.debit(
                proposal_position(pid, branch, PositionKind::BranchUsdc),
                *who,
                amount,
            )?;
        }
        Ok(())
    }

    fn do_merge_scalar(
        &mut self,
        pid: u64,
        branch: Branch,
        who: &u8,
        amount: Balance,
    ) -> Result<(), ()> {
        for kind in [PositionKind::Long, PositionKind::Short] {
            self.debit(proposal_position(pid, branch, kind), *who, amount)?;
        }
        self.credit(
            proposal_position(pid, branch, PositionKind::BranchUsdc),
            *who,
            amount,
        )
    }

    fn do_merge_gate(
        &mut self,
        pid: u64,
        branch: Branch,
        gate: GateType,
        who: &u8,
        amount: Balance,
    ) -> Result<(), ()> {
        for kind in [PositionKind::GateYes(gate), PositionKind::GateNo(gate)] {
            self.debit(proposal_position(pid, branch, kind), *who, amount)?;
        }
        self.credit(
            proposal_position(pid, branch, PositionKind::BranchUsdc),
            *who,
            amount,
        )
    }

    fn do_merge_baseline(&mut self, epoch: u32, who: &u8, amount: Balance) -> Result<(), ()> {
        self.debit(baseline_position(epoch, ScalarSide::Long), *who, amount)?;
        self.debit(baseline_position(epoch, ScalarSide::Short), *who, amount)
    }

    fn note_protocol_account(&mut self, _: u8) {}

    fn position_balance(&self, id: PositionId, who: &u8) -> Balance {
        self.balance(id, who)
    }
}

fn seeded_book(kind: BookKind, b: Balance) -> (MarketBook<u8>, MockLedger, Balance) {
    let book = MarketBook::open(7, kind, 9, 8, b);
    let mut ledger = MockLedger::default();
    let headroom = seed_book(&book, &mut ledger, &1).expect("sensible b must seed");
    (book, ledger, headroom)
}

fn amount_from_selector(b: Balance, selector: u16) -> Balance {
    let max = b / 4;
    MIN_TRADE + (max - MIN_TRADE) * Balance::from(selector) / Balance::from(u16::MAX)
}

fn sell_amount(held: Balance, b: Balance, selector: u16) -> Option<Balance> {
    let max = held.min(b / 4);
    if max < MIN_TRADE {
        None
    } else {
        Some(MIN_TRADE + (max - MIN_TRADE) * Balance::from(selector) / Balance::from(u16::MAX))
    }
}

fn atomic_buy(
    book: &mut MarketBook<u8>,
    ledger: &mut MockLedger,
    side: ScalarSide,
    amount: Balance,
    block: u64,
) -> Result<Vec<MarketEvent<u8>>, MarketError> {
    let book_before = *book;
    let ledger_before = ledger.clone();
    let result = buy_book(
        book,
        ledger,
        &MarketParams::default(),
        &2,
        side,
        amount,
        Balance::MAX,
        block,
    );
    if result.is_err() {
        *book = book_before;
        *ledger = ledger_before;
    }
    result
}

fn atomic_sell(
    book: &mut MarketBook<u8>,
    ledger: &mut MockLedger,
    side: ScalarSide,
    amount: Balance,
    block: u64,
) -> Result<Vec<MarketEvent<u8>>, MarketError> {
    let book_before = *book;
    let ledger_before = ledger.clone();
    let result = sell_book(
        book,
        ledger,
        &MarketParams::default(),
        &2,
        side,
        amount,
        0,
        block,
    );
    if result.is_err() {
        *book = book_before;
        *ledger = ledger_before;
    }
    result
}

fn traded_cost(events: &[MarketEvent<u8>]) -> Balance {
    events
        .iter()
        .find_map(|event| match event {
            MarketEvent::Traded { cost, .. } => Some(*cost),
            _ => None,
        })
        .expect("trade emits Traded")
}

pub fn balance_to_fixed(value: Balance) -> FixedU64x64 {
    // `USDC` (= 1e6 base units) is the kernel-owned USDC scale (rule 4).
    let units = value / USDC;
    let fraction = value % USDC;
    FixedU64x64::from_integer(u64::try_from(units).expect("fuzz range fits u64"))
        .checked_add(FixedU64x64::from_raw((fraction << 64) / USDC))
        .expect("fuzz range fits fixed point")
}

/// Conservation and solvency check for one realized book state, generic over the
/// book kind (finding D). `net_revenue` is the signed gross CASH flow
/// (Σ buy cost − Σ sell proceeds), used only for the ≥ 0 gross-revenue property;
/// `inventory` is the kind-specific USDC-equivalent the book's held inventory has
/// absorbed net of delivery, used for the exact bookkeeping identity and the
/// I-12 drain ceiling.
fn assert_book_state(
    book: &MarketBook<u8>,
    ledger: &MockLedger,
    headroom: Balance,
    net_revenue: i128,
    inventory: i128,
) {
    let state = MarketState {
        markets: vec![*book],
        baseline_market_of: Vec::new(),
        events: Vec::new(),
    };
    assert_eq!(state.try_state(), Ok(()));
    // Cumulative gross book revenue telescopes to `C(q_now) − C(0,0) ≥ 0` (the
    // book starts at the cost-minimizing symmetric `(0,0)` and maker-adverse
    // rounding only raises it). Precondition: a single trader (= the book's
    // counterparty) and the neutral seed — a second trader or non-neutral seed
    // would break this identity, which this harness does not construct.
    assert!(
        net_revenue >= 0,
        "an inventory-backed path lost gross revenue"
    );

    let liquid = liquid_position(book.kind).map_or(0, |id| ledger.balance(id, &book.account));
    for (side, q) in [
        (ScalarSide::Long, book.q_long),
        (ScalarSide::Short, book.q_short),
    ] {
        let scalar = ledger.balance(side_position(book.kind, side), &book.account);
        let expected_available = i128::try_from(headroom).expect("range") + inventory
            - i128::try_from(q).expect("range");
        assert!(expected_available >= 0, "seeded headroom was exhausted");
        assert_eq!(
            scalar + liquid,
            u128::try_from(expected_available).expect("non-negative"),
            "mock inventory diverged from exact 04 §6.3 drain accounting"
        );
        // I-12 (04 §6.3): the exact one-sided drain is `< b·ln2`. Measured
        // `inventory` recycled is maker-adverse-rounded (≥ the exact value), so
        // measured `drain = q − inventory ≤` the exact drain `< b·ln2 ≤
        // ceil(b·ln2) = headroom`. The exact `headroom` ceiling (no per-trade
        // slack) makes this catch a genuine over-drain of even one base unit.
        let drain = i128::try_from(q).expect("range") - inventory;
        if drain > 0 {
            assert!(
                u128::try_from(drain).expect("positive") <= headroom,
                "I-12 maker drain exceeded the ceil(b*ln2) headroom ceiling"
            );
        }
    }
}

fn assert_round_trip(kind: BookKind, b: Balance, selector: u16) {
    let (mut book, mut ledger, headroom) = seeded_book(kind, b);
    let amount = amount_from_selector(b, selector);
    let before = lmsr_cost(
        balance_to_fixed(book.q_long),
        balance_to_fixed(book.q_short),
        balance_to_fixed(book.b),
    )
    .expect("neutral state in domain");
    let buy_events = atomic_buy(&mut book, &mut ledger, ScalarSide::Long, amount, 10)
        .expect("bounded round-trip buy");
    let after = lmsr_cost(
        balance_to_fixed(book.q_long),
        balance_to_fixed(book.q_short),
        balance_to_fixed(book.b),
    )
    .expect("post-buy state in domain");
    assert!(after > before, "LMSR cost must strictly increase on a buy");
    let buy_cost = traded_cost(&buy_events);
    let sell_events = atomic_sell(&mut book, &mut ledger, ScalarSide::Long, amount, 20)
        .expect("matching round-trip sell");
    let sell_proceeds = traded_cost(&sell_events);
    let buy_total = buy_cost + fee_up(buy_cost, FEE_BPS).expect("fee");
    let sell_net = sell_proceeds - fee_up(sell_proceeds, FEE_BPS).expect("fee");
    assert!(
        sell_net <= buy_total,
        "round trip made positive trader profit: {sell_net} > {buy_total}"
    );
    assert_eq!((book.q_long, book.q_short), (0, 0));
    let cash =
        i128::try_from(buy_cost).expect("range") - i128::try_from(sell_proceeds).expect("range");
    let inventory = i128::try_from(inventory_inflow_on_buy(kind, buy_cost)).expect("range")
        - i128::try_from(sell_proceeds).expect("range");
    assert_book_state(&book, &ledger, headroom, cash, inventory);
}

pub fn assert_lmsr_case(case: &TradeCase) {
    let kind = case.kind;
    assert_round_trip(kind, case.b, case.round_trip_selector);
    let (mut book, mut ledger, headroom) = seeded_book(kind, case.b);
    // `net_revenue` is the signed gross cash flow (≥ 0 property); `inventory` is
    // the kind-specific USDC-equivalent the book has recycled net of delivery
    // (exact bookkeeping identity + I-12 drain ceiling). They coincide for
    // Decision/Gate and diverge for the fee-retaining Baseline wrapper.
    let mut net_revenue = 0i128;
    let mut inventory = 0i128;

    for (index, operation) in case.trades.iter().enumerate() {
        let before = lmsr_cost(
            balance_to_fixed(book.q_long),
            balance_to_fixed(book.q_short),
            balance_to_fixed(book.b),
        )
        .expect("reachable book state is in domain");
        // `market-core` is a total, no-panic `Result` API (G-1): a trade the
        // book cannot honor fail-closed — e.g. an interleaving that drains the
        // delivered side's inventory below the request, or an LMSR-domain edge —
        // returns a typed `Error` and the atomic wrapper rolls the book+ledger
        // back to the pre-trade state. A rejection is a legitimate no-op, not a
        // finding, so we skip it without advancing accounting; the invariants
        // below still run on every realized state, and an over-payment (the
        // solvency direction that WOULD be a finding) is caught by
        // `assert_book_state`/`try_state` on the trades that do succeed.
        if operation.buy {
            let amount = amount_from_selector(case.b, operation.selector);
            if let Ok(events) = atomic_buy(
                &mut book,
                &mut ledger,
                operation.side,
                amount,
                (index as u64 + 1) * 10,
            ) {
                let after = lmsr_cost(
                    balance_to_fixed(book.q_long),
                    balance_to_fixed(book.q_short),
                    balance_to_fixed(book.b),
                )
                .expect("post-buy state is in domain");
                assert!(after > before, "LMSR cost must strictly increase on buys");
                let cost = traded_cost(&events);
                net_revenue += i128::try_from(cost).expect("range");
                inventory += i128::try_from(inventory_inflow_on_buy(kind, cost)).expect("range");
            }
        } else {
            let held = ledger.balance(side_position(kind, operation.side), &2);
            if let Some(amount) = sell_amount(held, case.b, operation.selector) {
                if let Ok(events) = atomic_sell(
                    &mut book,
                    &mut ledger,
                    operation.side,
                    amount,
                    (index as u64 + 1) * 10,
                ) {
                    let proceeds = traded_cost(&events);
                    net_revenue -= i128::try_from(proceeds).expect("range");
                    inventory -= i128::try_from(proceeds).expect("range");
                }
            }
        }
        assert_book_state(&book, &ledger, headroom, net_revenue, inventory);
    }

    assert_fixed_domain(case.b, case.round_trip_selector);
}

fn assert_fixed_domain(b: Balance, selector: u16) {
    let b_fx = balance_to_fixed(b);
    let factor = u128::from(selector % 51);
    let q_l = balance_to_fixed(b.checked_mul(factor).expect("bounded factor"));
    let q_s = FixedU64x64::ZERO;
    let cost = lmsr_cost(q_l, q_s, b_fx);
    if factor <= u128::from(LMSR_DOMAIN_BOUND) {
        let value = cost.unwrap_or_else(|error| match error {
            FixedError::Overflow | FixedError::NonFinite => {
                panic!("in-domain kernel produced {error:?}")
            }
            other => panic!("in-domain kernel rejected with {other:?}"),
        });
        assert!(value >= q_l);
        let long = lmsr_price_long(q_l, q_s, b_fx).expect("in-domain long price");
        let short = lmsr_price_short(q_l, q_s, b_fx).expect("in-domain short price");
        // Definitional identity, not an approximation bound: the two LMSR marginal
        // prices are a probability pair, so `p_L + p_S == 1` exactly (04 §4); the
        // fixed kernel must reproduce it with no rounding slack.
        assert_eq!(
            long.checked_add(short).expect("price sum"),
            FixedU64x64::ONE
        );
        if factor < u128::from(LMSR_DOMAIN_BOUND) {
            let amount = balance_to_fixed(b / 4);
            let buy = lmsr_buy_cost(q_l, q_s, b_fx, LmsrSide::Long, amount).expect("in-domain buy");
            assert!(buy.raw() > 0);
            assert!(
                lmsr_cost(q_l.checked_add(amount).expect("bounded add"), q_s, b_fx)
                    .expect("new state in domain")
                    > value
            );
        }
    } else {
        assert_eq!(cost, Err(FixedError::Domain));
        assert_eq!(lmsr_price_long(q_l, q_s, b_fx), Err(FixedError::Domain));
    }
    assert_eq!(
        lmsr_cost(q_l, q_s, FixedU64x64::ZERO),
        Err(FixedError::DivisionByZero)
    );
    let seeded = balance_to_fixed(b)
        .checked_mul(LN_2)
        .expect("sensible b times ln2");
    assert!(seeded.raw() > 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(call: RuntimeCall) -> BoxedCall {
        BoxedCall::new(call)
    }

    #[test]
    fn wrapper_oracle_covers_proxy_announced_and_threshold_one() {
        for call in [
            RuntimeCall::ProxyAnnounced(boxed(RuntimeCall::Leaf(CallDomain::Param))),
            RuntimeCall::MultisigAsMultiThreshold1(boxed(RuntimeCall::Leaf(CallDomain::Code))),
        ] {
            assert_eq!(
                oracle_validate(Some(Origin::FutarchyParam), &call),
                Err(OracleError::PrivilegedWrapper)
            );
            assert_wrapper_case(&WrapperCase {
                origin: Some(Origin::FutarchyParam),
                calls: vec![call],
            });
        }
    }

    #[test]
    fn wrapper_oracle_models_scheduler_origin_capture() {
        let good = RuntimeCall::Scheduler {
            origin: Origin::ConstitutionalValues,
            call: boxed(RuntimeCall::Leaf(CallDomain::ConstitutionalValues)),
        };
        assert_eq!(oracle_validate(None, &good), Ok(()));
        assert!(SafetyFilter::validate(None, &good).is_ok());
        assert!(!contains_unscoped_privileged(&good, false));

        let bad = RuntimeCall::Scheduler {
            origin: Origin::GuardianHold,
            call: boxed(RuntimeCall::Leaf(CallDomain::GuardianHold)),
        };
        assert_eq!(
            oracle_validate(None, &bad),
            Err(OracleError::SchedulerDenied)
        );
    }

    #[test]
    fn wrapper_oracle_enforces_shared_batch_budget() {
        let ten = RuntimeCall::UtilityBatch(
            (0..9)
                .map(|_| RuntimeCall::Leaf(CallDomain::Public))
                .collect(),
        );
        assert!(oracle_validate(None, &ten).is_ok());
        assert_eq!(
            oracle_validate_batch(None, &[ten.clone(), ten]),
            Err(OracleError::TooManyCalls)
        );
    }

    #[test]
    fn wrapper_oracle_enforces_depth_and_dispatch_as() {
        let mut deep = RuntimeCall::Leaf(CallDomain::Public);
        for _ in 0..=MAX_NESTED_LEVELS {
            deep = RuntimeCall::Sudo(boxed(deep));
        }
        assert_eq!(oracle_validate(None, &deep), Err(OracleError::TooDeep));
        let dispatch_as =
            RuntimeCall::UtilityDispatchAs(boxed(RuntimeCall::Leaf(CallDomain::Public)));
        assert_eq!(
            oracle_validate(None, &dispatch_as),
            Err(OracleError::DispatchAsDenied)
        );
    }

    #[test]
    fn wrapper_oracle_matches_public_and_origin_scoped_leaves() {
        for origin in [
            None,
            Some(Origin::FutarchyParam),
            Some(Origin::OracleResolution),
        ] {
            for domain in [
                CallDomain::Public,
                CallDomain::Nobody,
                CallDomain::Param,
                CallDomain::OracleResolution,
                CallDomain::InternalRoot,
            ] {
                let call = RuntimeCall::Leaf(domain);
                assert_eq!(
                    oracle_validate(origin, &call).is_ok(),
                    SafetyFilter::validate(origin, &call).is_ok()
                );
            }
        }
    }

    /// Genuinely independent enumerated truth-table (06 §3.3 + `CallDomain`):
    /// each `expected` is HAND-computed (not derived from the implementation),
    /// so a shared logic error in the recursive oracle, `independent_admits`,
    /// and `validate_inner` cannot hide behind mutual agreement. Covers the
    /// `Some(class_origin)` privileged-wrapper path, scheduler origin capture
    /// (which OVERRIDES the outer origin), transitive proxyish denial through a
    /// non-proxyish wrapper, count, and depth.
    #[test]
    fn wrapper_admission_truth_table() {
        use CallDomain::{Code, ConstitutionalValues, InternalRoot, Nobody, Param, Public};
        use Origin::{ConstitutionalValues as OCV, FutarchyCode, FutarchyParam, GuardianHold};

        let param = || RuntimeCall::Leaf(Param);
        let public = || RuntimeCall::Leaf(Public);
        // (label, call, origin, expected-admit)
        let cases: Vec<(&str, RuntimeCall, Option<Origin>, bool)> = vec![
            ("bare public / no origin", public(), None, true),
            ("bare nobody", RuntimeCall::Leaf(Nobody), None, false),
            ("bare param / no origin", param(), None, false),
            (
                "bare param / param origin",
                param(),
                Some(FutarchyParam),
                true,
            ),
            (
                "bare param / wrong origin",
                param(),
                Some(FutarchyCode),
                false,
            ),
            (
                "bare internal-root / any",
                RuntimeCall::Leaf(InternalRoot),
                Some(FutarchyParam),
                false,
            ),
            // proxyish wrappers deny ANY privileged leaf regardless of origin match
            (
                "proxy(public) / param origin",
                RuntimeCall::Proxy(boxed(public())),
                Some(FutarchyParam),
                true,
            ),
            (
                "proxy(param) / param origin",
                RuntimeCall::Proxy(boxed(param())),
                Some(FutarchyParam),
                false,
            ),
            (
                "proxy_announced(public)",
                RuntimeCall::ProxyAnnounced(boxed(public())),
                None,
                true,
            ),
            (
                "as_multi(param) / param origin",
                RuntimeCall::MultisigAsMulti(boxed(param())),
                Some(FutarchyParam),
                false,
            ),
            (
                "as_multi_threshold_1(code) / code origin",
                RuntimeCall::MultisigAsMultiThreshold1(boxed(RuntimeCall::Leaf(Code))),
                Some(FutarchyCode),
                false,
            ),
            (
                "approve_as_multi",
                RuntimeCall::MultisigApproveAsMulti,
                None,
                true,
            ),
            // non-proxyish wrappers allow privileged leaves iff the origin matches
            (
                "sudo(param) / param origin",
                RuntimeCall::Sudo(boxed(param())),
                Some(FutarchyParam),
                true,
            ),
            (
                "with_weight(param) / no origin",
                RuntimeCall::UtilityWithWeight(boxed(param())),
                None,
                false,
            ),
            // transitive proxyish: proxy over a non-proxyish wrapper still denies
            (
                "proxy(sudo(param)) / param origin",
                RuntimeCall::Proxy(boxed(RuntimeCall::Sudo(boxed(param())))),
                Some(FutarchyParam),
                false,
            ),
            // dispatch_as / as_derivative denied entirely
            (
                "dispatch_as(public)",
                RuntimeCall::UtilityDispatchAs(boxed(public())),
                None,
                false,
            ),
            (
                "as_derivative(public)",
                RuntimeCall::UtilityAsDerivative(boxed(public())),
                None,
                false,
            ),
            // scheduler: values-only, and it OVERRIDES the outer origin for its subtree
            (
                "scheduler(CV, cv-leaf) / no origin",
                RuntimeCall::Scheduler {
                    origin: OCV,
                    call: boxed(RuntimeCall::Leaf(ConstitutionalValues)),
                },
                None,
                true,
            ),
            (
                "scheduler(bad origin)",
                RuntimeCall::Scheduler {
                    origin: GuardianHold,
                    call: boxed(public()),
                },
                None,
                false,
            ),
            (
                "scheduler(CV, param-leaf) / param origin — capture overrides",
                RuntimeCall::Scheduler {
                    origin: OCV,
                    call: boxed(param()),
                },
                Some(FutarchyParam),
                false,
            ),
            // batch: admits iff every child admits; count at/over the limit
            (
                "batch(param,public) / param origin",
                RuntimeCall::UtilityBatch(vec![param(), public()]),
                Some(FutarchyParam),
                true,
            ),
            (
                "batch(15 public) = 16 nodes",
                RuntimeCall::UtilityBatch((0..15).map(|_| public()).collect()),
                None,
                true,
            ),
            (
                "batch(16 public) = 17 nodes",
                RuntimeCall::UtilityBatch((0..16).map(|_| public()).collect()),
                None,
                false,
            ),
        ];

        for (label, call, origin, expected) in cases {
            assert_eq!(
                SafetyFilter::validate(origin, &call).is_ok(),
                expected,
                "filter disagreed with hand truth-table: {label}"
            );
            assert_eq!(
                independent_admits(origin, &call),
                expected,
                "independent_admits disagreed with hand truth-table: {label}"
            );
        }

        // Over-depth (5 nested batches = container depth 5 > MAX_NESTED_LEVELS).
        let mut deep = public();
        for _ in 0..=MAX_NESTED_LEVELS {
            deep = RuntimeCall::UtilityBatch(vec![deep]);
        }
        assert!(SafetyFilter::validate(None, &deep).is_err());
        assert!(!independent_admits(None, &deep));
    }

    #[test]
    fn lmsr_harness_v1_v5_round_trip_and_inventory_accounting() {
        let case = TradeCase {
            // 04 §5 vector anchor: b = 10,000 USDC (= 10_000 · USDC base units).
            b: 10_000 * USDC,
            kind: BookKind::Decision {
                proposal: 1,
                branch: Branch::Accept,
            },
            round_trip_selector: 26_200,
            trades: vec![
                TradeOp {
                    buy: true,
                    side: ScalarSide::Long,
                    selector: 26_200,
                },
                TradeOp {
                    buy: false,
                    side: ScalarSide::Long,
                    selector: u16::MAX,
                },
            ],
        };
        assert_lmsr_case(&case);
    }

    /// Finding D: the LMSR solvency battery must run against Gate (both branches,
    /// both `GateType`s) and the unbranched Baseline book, not only Decision —
    /// `assert_lmsr_case` reads the kind-correct position set and asserts the
    /// conservation identity + I-12 drain for each. `sell_baseline` was
    /// historically solvency-buggy, so a buy→sell→interleave sequence on every
    /// kind is the load-bearing coverage.
    #[test]
    fn lmsr_harness_covers_gate_and_baseline_books() {
        let trades = vec![
            TradeOp {
                buy: true,
                side: ScalarSide::Long,
                selector: 20_000,
            },
            TradeOp {
                buy: true,
                side: ScalarSide::Short,
                selector: 12_000,
            },
            TradeOp {
                buy: false,
                side: ScalarSide::Long,
                selector: u16::MAX,
            },
            TradeOp {
                buy: false,
                side: ScalarSide::Short,
                selector: u16::MAX,
            },
        ];
        let kinds = [
            BookKind::Gate {
                proposal: 1,
                branch: Branch::Accept,
                gate: GateType::Survival,
            },
            BookKind::Gate {
                proposal: 1,
                branch: Branch::Reject,
                gate: GateType::Security,
            },
            BookKind::Baseline { epoch: 1 },
            BookKind::Decision {
                proposal: 1,
                branch: Branch::Reject,
            },
        ];
        for kind in kinds {
            let case = TradeCase {
                b: 10_000 * USDC,
                kind,
                round_trip_selector: 26_200,
                trades: trades.clone(),
            };
            assert_lmsr_case(&case);
        }
    }

    /// Finding D: the seed-derived book-kind picker keeps every committed
    /// decision seed on the Decision/Accept path (its flag high-bits are 0) yet
    /// lets the fuzzer reach Gate and Baseline books.
    #[test]
    fn kind_from_seed_defaults_committed_seeds_to_decision_accept() {
        // A committed-style seed: b + selector + one `(buy=Long)` op (flag bytes
        // 0/0), i.e. every high bit clear.
        let seed = [0u8; 14];
        assert_eq!(
            kind_from_seed(&seed),
            BookKind::Decision {
                proposal: 1,
                branch: Branch::Accept,
            }
        );
        // Reaches Gate and Baseline as the first op's flag high-bits vary.
        assert!(matches!(
            kind_from_seed(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0b0100, 0, 0, 0]),
            BookKind::Gate { .. }
        ));
        assert!(matches!(
            kind_from_seed(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0b0110, 0, 0, 0]),
            BookKind::Baseline { .. }
        ));
    }

    #[test]
    fn lmsr_harness_tolerates_fail_closed_trades() {
        // Regression for the `depletion_sell_rejection` corpus seed: an
        // interleaving that buys LONG until the book's LONG inventory is drained,
        // then sells SHORT — `sell_branch`'s complete-set re-merge cannot be
        // collateralized, so `market-core` fail-closes with `Error::Ledger` and
        // the atomic wrapper rolls back. The harness must treat that as a no-op,
        // not a finding, so `assert_lmsr_case` must complete without panicking.
        let case = TradeCase::from_seed_bytes(&[
            172, 38, 0, 0, 0, 0, 0, 0, 86, 102, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0,
            0, 255, 255, 0, 0, 255, 255, 172, 38, 0, 0, 0, 0, 255, 255, 255, 0, 0, 86, 102, 0, 0,
            255, 255, 0, 0, 255, 0, 0, 255, 255, 128, 255, 255, 255, 255, 255, 255, 254, 0, 0, 255,
            255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0,
        ]);
        assert_lmsr_case(&case);
    }
}
