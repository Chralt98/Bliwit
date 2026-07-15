#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use core::convert::TryFrom;

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub const INTEGRATION_CONTRACT_VERSION: u32 = 3;

pub type Balance = u128;
pub type ProposalId = u64;
pub type EpochId = u32;
pub type CohortId = EpochId;
pub type MarketId = u64;
pub type MetricId = u16;
pub type MetricSpecVersion = u16;
pub type ResourceId = [u8; 8];
pub type ParamKey = [u8; 16];
pub type AccountId = [u8; 32];
pub type H256 = [u8; 32];
pub type BlockNumber = u32;

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub struct FixedU64(pub u64);

#[derive(Clone, Debug, Eq, PartialEq, TypeInfo)]
pub struct BoundedVec<T, const N: u32>(Vec<T>);

impl<T, const N: u32> BoundedVec<T, N> {
    pub const BOUND: u32 = N;

    pub const fn new() -> Self {
        Self(Vec::new())
    }

    pub fn into_inner(self) -> Vec<T> {
        self.0
    }

    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> core::slice::Iter<'_, T> {
        self.0.iter()
    }

    pub fn try_push(&mut self, value: T) -> Result<(), BoundExceeded> {
        if self.0.len() >= N as usize {
            return Err(BoundExceeded);
        }
        self.0.push(value);
        Ok(())
    }
}

impl<T, const N: u32> IntoIterator for BoundedVec<T, N> {
    type Item = T;
    type IntoIter = alloc::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T, const N: u32> IntoIterator for &'a BoundedVec<T, N> {
    type Item = &'a T;
    type IntoIter = core::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: Encode, const N: u32> Encode for BoundedVec<T, N> {
    fn size_hint(&self) -> usize {
        self.0.size_hint()
    }

    fn encode_to<W: parity_scale_codec::Output + ?Sized>(&self, dest: &mut W) {
        self.0.encode_to(dest);
    }
}

impl<T: Decode, const N: u32> Decode for BoundedVec<T, N> {
    fn decode<I: parity_scale_codec::Input>(
        input: &mut I,
    ) -> Result<Self, parity_scale_codec::Error> {
        // Enforce the bound at the decode boundary: reject an oversized advertised
        // length before allocating or decoding any element, so untrusted input
        // cannot force work above the declared bound.
        let len = <parity_scale_codec::Compact<u32>>::decode(input)?.0;
        if len > N {
            return Err("BoundedVec length exceeds declared bound".into());
        }
        let items = parity_scale_codec::decode_vec_with_len(input, len as usize)?;
        Ok(Self(items))
    }
}

// Marker: the bounded `Decode` above rejects oversized lengths before allocating
// and delegates element decoding to `decode_vec_with_len`, so it honours the
// input's memory accounting (FRAME PoV requirement, codec ≥ 3.7).
impl<T: DecodeWithMemTracking, const N: u32> DecodeWithMemTracking for BoundedVec<T, N> {}

impl<T: MaxEncodedLen, const N: u32> MaxEncodedLen for BoundedVec<T, N> {
    fn max_encoded_len() -> usize {
        parity_scale_codec::Compact(N).encoded_size()
            + (N as usize).saturating_mul(T::max_encoded_len())
    }
}

impl<T, const N: u32> Default for BoundedVec<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: u32> TryFrom<Vec<T>> for BoundedVec<T, N> {
    type Error = BoundExceeded;

    fn try_from(value: Vec<T>) -> Result<Self, Self::Error> {
        if value.len() > N as usize {
            return Err(BoundExceeded);
        }
        Ok(Self(value))
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub struct BoundExceeded;

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum Branch {
    Accept,
    Reject,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum ScalarSide {
    Long,
    Short,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum GateType {
    Survival,
    Security,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum PositionKind {
    BranchUsdc,
    Long,
    Short,
    GateYes(GateType),
    GateNo(GateType),
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum PositionId {
    Proposal {
        proposal: ProposalId,
        branch: Branch,
        kind: PositionKind,
    },
    Baseline {
        epoch: EpochId,
        side: ScalarSide,
    },
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum VaultState {
    Open,
    Resolved(Branch),
    ScalarSettled { winner: Branch, s: FixedU64 },
    Voided,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum ProposalClass {
    Param,
    Treasury,
    Code,
    Meta,
    Constitutional,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum RejectReason {
    NotDecisionGrade,
    GateVetoSurvival,
    GateVetoSecurity,
    HurdleNotMet,
    ConvergenceFailed,
    SecondExtensionFailed,
    ProcessHold,
    ConstitutionViolation,
    ResourceConflict,
    RateLimited,
    VetoUpheldByReview,
    StaleQueue,
    PayloadReverted,
    NotRatified,
    SecuritySizing,
    AttestationMissing,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum ProposalState {
    Submitted,
    Screening,
    Qualified,
    Trading,
    Extended,
    Queued,
    Suspended,
    Rerun,
    Rejected(RejectReason),
    Executed,
    FailedExecuted,
    Measuring,
    Settled,
    Cancelled,
    Expired,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum EpochPhase {
    Intake,
    Qualify,
    Seed,
    Trade,
    Decide,
    Review,
    Execute,
    Housekeeping,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum DecisionOutcome {
    Adopt,
    Reject(RejectReason),
    Extend,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum DispatchOutcomeCode {
    Ok,
    Failed { call_index: u8, error: [u8; 4] },
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum RatificationStatus {
    NotRequired,
    Pending { referendum: u32 },
    Passed { referendum: u32 },
    Failed { referendum: u32 },
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum TradeSide {
    BuyLong,
    BuyShort,
    SellLong,
    SellShort,
}

/// Book kind carried by the `MarketCreated` event (02 §5). Declaration order is
/// the SCALE index order and is frozen by the contract surface. Variant spelling
/// is 02 §5's byte-for-byte: `02` is canonical for any name that appears on the
/// contract surface (02 line 5; runtime-code rule 5), and the frontend decodes
/// `MarketCreated.kind` by its TypeInfo variant name — so the underscored
/// `GateS_Adopt`/`GateS_Reject`/`GateC_Adopt`/`GateC_Reject` spelling is
/// load-bearing. `#[allow(non_camel_case_types)]` preserves that frozen spelling
/// (SQ-37 resolved: the code conformed to the contract; `02` is unchanged, so no
/// `INTEGRATION_CONTRACT_VERSION` bump and no joint sign-off are required).
#[allow(non_camel_case_types)]
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum MarketKind {
    DecisionAccept,
    DecisionReject,
    GateS_Adopt,
    GateS_Reject,
    GateC_Adopt,
    GateC_Reject,
    Baseline,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct RuntimeVersionConstraint {
    pub spec_name: BoundedVec<u8, 32>,
    pub spec_version: u32,
}

/// Book ids seeded for a proposal (04). Carried by [`Proposal::markets`].
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub struct MarketSet {
    pub accept: MarketId,
    pub reject: MarketId,
    pub gates: Option<[MarketId; 4]>,
    pub baseline: MarketId,
}

/// Canonical proposal record. Layout frozen by inclusion in `futarchy-primitives`
/// (02 §2); declaration order **is** the SCALE layout (05 §1.2, enumerated in full
/// there). Generic over the runtime `AccountId` (concrete: `AccountId32`, 02 §8).
/// `MaxEncodedLen` is derived so `pallet-epoch`'s `Proposals` map is bounded
/// (02 §114 ≤512 B; I-20/I-21, G-6).
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct Proposal<AccountId> {
    pub id: ProposalId,
    pub proposer: AccountId,
    pub class: ProposalClass,
    pub state: ProposalState,
    pub epoch: EpochId,
    pub submitted_at: BlockNumber,
    pub payload_hash: H256,
    /// Preimage byte length; `(payload_hash, payload_len)` is the pinned commitment
    /// read by decide()'s §5.6 preimage check (09 §1.2(2)).
    pub payload_len: u32,
    pub ask: Balance,
    pub bond: Balance,
    /// Declared resource-domain keys (bound: 13 §4 "Resource locks" = 8).
    pub resources: BoundedVec<[u8; 8], 8>,
    pub metric_spec: MetricSpecVersion,
    pub decide_at: BlockNumber,
    pub rerun: bool,
    pub extended: bool,
    pub delayed_once: bool,
    pub markets: Option<MarketSet>,
    pub maturity: Option<BlockNumber>,
    pub grace_end: Option<BlockNumber>,
    pub version_constraint: Option<RuntimeVersionConstraint>,
    pub decision: Option<DecisionOutcome>,
}

/// Terminal execution-queue record (09). Layout single-homed here per 02 §2.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct ExecutionRecord {
    pub pid: ProposalId,
    pub payload_hash: H256,
    pub class: ProposalClass,
    pub executed_at: BlockNumber,
    pub result: DispatchOutcomeCode,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct EpochStatusView {
    pub index: EpochId,
    pub phase: EpochPhase,
    pub phase_start_block: BlockNumber,
    pub next_boundary: BlockNumber,
    pub dead_man_armed: bool,
    pub ledger_frozen: bool,
    pub phase_flags: u32,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct ProposalSummaryView {
    pub id: ProposalId,
    pub class: ProposalClass,
    pub state: ProposalState,
    pub proposer: AccountId,
    pub epoch: EpochId,
    pub payload_hash: H256,
    pub ask: Balance,
    pub decision_market: Option<(MarketId, MarketId)>,
    pub gate_markets: Option<[MarketId; 4]>,
    pub decide_at: BlockNumber,
    pub maturity: Option<BlockNumber>,
    pub ratification: RatificationStatus,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct QuoteView {
    pub cost: Balance,
    pub fee: Balance,
    pub p_after_1e9: FixedU64,
    pub max_trade: Balance,
    pub within_domain: bool,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct DecisionStatsView {
    pub pid: ProposalId,
    pub twap_accept_1e9: FixedU64,
    pub twap_reject_1e9: FixedU64,
    pub twap_baseline_1e9: FixedU64,
    pub r_eff_1e9: FixedU64,
    pub trailing_accept_1e9: FixedU64,
    pub trailing_reject_1e9: FixedU64,
    pub coverage_pct: u8,
    pub traded_volume: Balance,
    pub v_min_required: Balance,
    pub converged: bool,
    pub gate_twaps_1e9: Option<[FixedU64; 4]>,
    pub attack_cost_hat: Balance,
    pub in_cap_prize: Balance,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct PositionView {
    pub position: PositionId,
    pub balance: Balance,
    pub vault_state: VaultState,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct QueuedExecutionView {
    pub pid: ProposalId,
    pub class: ProposalClass,
    pub payload_hash: H256,
    pub maturity: BlockNumber,
    pub grace_end: BlockNumber,
    pub version_constraint: RuntimeVersionConstraint,
    pub cancelled: bool,
    pub ratification: RatificationStatus,
    pub meters_clear: bool,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct WelfareView {
    pub epoch: EpochId,
    pub spec_version: MetricSpecVersion,
    pub s_pillar_1e9: FixedU64,
    pub c_onchain_1e9: FixedU64,
    pub c_attested_1e9: FixedU64,
    pub p_pillar_1e9: FixedU64,
    pub a_pillar_1e9: FixedU64,
    pub gate_s_1e9: FixedU64,
    pub gate_c_1e9: FixedU64,
    pub w_current_1e9: FixedU64,
    pub s_breached: bool,
    pub c_breached: bool,
    pub reserve_flag: bool,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct ParamView {
    pub key: ParamKey,
    pub value: u128,
    pub min: u128,
    pub max: u128,
    pub max_delta: u128,
    pub cooldown_blocks: u32,
    pub last_change: BlockNumber,
    pub class: ProposalClass,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct NavView {
    pub total: Balance,
    pub main: Balance,
    pub pol: Balance,
    pub insurance: Balance,
    pub keeper: Balance,
    pub oracle: Balance,
    pub rewards: Balance,
    pub stream_remainders: Balance,
    pub obligations: Balance,
    pub haircut_flag: bool,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct CohortSummary {
    pub epoch: EpochId,
    pub s_1e9: FixedU64,
    pub baseline_twap_1e9: FixedU64,
    pub proposals: BoundedVec<(ProposalId, ProposalClass, DecisionOutcome), 5>,
    pub voided: bool,
    pub settled_at: BlockNumber,
}
pub type CohortSummaryView = CohortSummary;

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct OracleRoundView {
    pub component: MetricId,
    pub epoch: EpochId,
    // Per-version game key (contract v3, 07 §2(4)): an activation boundary keeps
    // two games live for one (component, epoch); the FE keys rounds by the triple.
    pub spec_version: MetricSpecVersion,
    pub round: u8,
    pub reporter: AccountId,
    pub value_1e9: FixedU64,
    pub evidence_hash: H256,
    pub bond: Balance,
    pub challenge_deadline: BlockNumber,
    pub acked_by_watchtowers: u8,
    pub escalated: bool,
}

pub mod bounds {
    pub const MAX_PROPOSAL_SUMMARIES: u32 = 32;
    pub const MAX_ACCOUNT_POSITIONS: u32 = 64;
    pub const MAX_EXECUTION_QUEUE: u32 = 32;
    pub const MAX_PARAM_KEYS: u32 = 64;
    pub const RECENT_COHORT_SUMMARIES: u32 = 32;
    pub const MAX_OPEN_ORACLE_ROUNDS: u32 = 192;
    pub const MAX_COHORT_PROPOSALS: u32 = 5;
    pub const INTAKE_QUEUE: u32 = 64;
    pub const MAX_LIVE_PROPOSALS: u32 = 32;
    pub const MAX_LIVE_MARKETS: u32 = 196;
    pub const BOOKS_PER_PROPOSAL: u32 = 6;
    pub const BASELINE_BOOKS: u32 = 4;
}

pub mod currency {
    pub const USDC_DECIMALS: u8 = 6;
    pub const VIT_DECIMALS: u8 = 12;
    /// One whole USDC (6 decimals) and one whole VIT (12 decimals) in base units.
    pub const USDC: u128 = 1_000_000;
    pub const VIT: u128 = 1_000_000_000_000;
    pub const USDC_CENT: u128 = 10_000;
    pub const VIT_EXISTENTIAL_DEPOSIT: u128 = 10_000_000_000;
    /// Genesis VIT supply (02 §8 / 13 §3.5 identity, D-17): 1,000,000,000 VIT,
    /// fixed at genesis. The single home for this chain-identity constant.
    pub const VIT_TOTAL_SUPPLY: u128 = 1_000_000_000 * VIT;
}

pub mod chain_identity {
    pub const SS58_PREFIX: u16 = 7777;
    pub const FIXTURE_PARA_ID: u32 = 4242;
}

pub mod kernel {
    pub const MILLISECS_PER_BLOCK: u64 = 6_000;
    pub const MIN_SPLIT_USDC: u128 = super::currency::USDC_CENT;
    pub const MIN_TRANSFER_USDC: u128 = super::currency::USDC_CENT;
    pub const MIN_TRADE_USDC: u128 = 1_000_000;
    /// Max observation gap before a decision-window staleness event (04 §7; 13 §3.2).
    pub const MKT_STALE_GAP_BLOCKS: u64 = 50;
    pub const POSITION_DEPOSIT_USDC: u128 = 100_000;
    pub const DEC_EXTENSION_BLOCKS: u32 = 43_200;
    pub const DESCRIPTOR_LEAD_TIME_BLOCKS: u32 = 43_200;
    pub const WATCHTOWER_EXTENSION_BLOCKS: u32 = 28_800;
    pub const MAX_NESTED_LEVELS: u32 = 4;
    pub const MAX_NESTED_CALLS: u32 = 16;
    pub const MAX_CALLS: u32 = 16;
    pub const MAX_BYTES: u32 = 64 * 1024;
    pub const MAX_WEIGHT_PERCENT: u8 = 25;
    pub const LMSR_DOMAIN_BOUND: u32 = 48;
    /// Maximum approximation error for a primitive transcendental (`exp2`/`log2`/`ln`),
    /// in units of 1 ulp = 2⁻⁶⁴ (04 §4). Single home for the `futarchy-fixed` kernel bound
    /// (13 rule 1: fixed imports domain/error bounds).
    pub const PRIMITIVE_MAX_ULP: u32 = 2;
    /// Maximum composed LMSR cost-function and marginal-price error, in ulp of 2⁻⁶⁴ (04 §4).
    pub const COMPOSED_COST_MAX_ULP: u32 = 8;
    pub const QUOTE_CLAMP_MIN_1E9: u64 = 1_000_000;
    pub const QUOTE_CLAMP_MAX_1E9: u64 = 999_000_000;
    pub const GATE_P_MAX_CEILING_1E9: u64 = 100_000_000;
    pub const ORC_MAX_PROOF_BYTES: u32 = 256 * 1024;
    pub const REG_MAX_FILINGS_EPOCH: u32 = 64;
    pub const WT_MAX: u32 = 16;
    /// Watchtower acknowledgement quorum (`wt.quorum` K floor, 07). Single home for
    /// the value the oracle and registry cores previously each re-declared.
    pub const WT_QUORUM: u8 = 2;
    pub const ATT_MIN_MEMBERS: u32 = 3;
    pub const ATT_QUORUM: u32 = 2;
    pub const DEAD_MAN_RELAY_BLOCKS: u32 = 4_800;
    pub const DEAD_MAN_SNAPSHOT_OVERDUE_BLOCKS: u32 = 57_600;
    pub const STALE_EPOCH_BOUND_BLOCKS: u32 = 100_800;
    pub const TICK_BATCH: u32 = 10;
    pub const REAP_BATCH: u32 = 100;
    pub const SETTLE_COHORT_MAX_ITEMS: u32 = 100;
    pub const KEEPER_BUDGET_EPOCH_FLOOR_USDC: u128 = 6_000_000_000;
}

/// Epoch phase-start offsets as fractions of `epoch.length` (13 §3.1). The pairs
/// (numerator, [`DENOMINATOR`]) are kernel constants exposed to clients as pallet
/// metadata constants — never `Params` storage. Review/Execute are per-class /
/// per-proposal and carry no fixed fraction.
pub mod phase_offsets {
    /// Common denominator for every epoch phase-offset fraction (13 §3.1).
    pub const DENOMINATOR: u32 = 21;
    pub const INTAKE_NUM: u32 = 0;
    pub const QUALIFY_NUM: u32 = 3;
    pub const SEED_NUM: u32 = 4;
    pub const TRADE_NUM: u32 = 5;
    /// Decision-window accrual start (final 72 h; trailing = final 24 h).
    pub const DECIDE_WINDOW_NUM: u32 = 15;
    pub const DECIDE_NUM: u32 = 18;
    pub const HOUSEKEEPING_NUM: u32 = 20;
}

impl Branch {
    pub const fn codec_index(self) -> u8 {
        match self {
            Self::Accept => 0,
            Self::Reject => 1,
        }
    }
}
impl MarketKind {
    pub const fn codec_index(self) -> u8 {
        match self {
            Self::DecisionAccept => 0,
            Self::DecisionReject => 1,
            Self::GateS_Adopt => 2,
            Self::GateS_Reject => 3,
            Self::GateC_Adopt => 4,
            Self::GateC_Reject => 5,
            Self::Baseline => 6,
        }
    }
}
impl RejectReason {
    pub const fn codec_index(self) -> u8 {
        match self {
            Self::NotDecisionGrade => 0,
            Self::GateVetoSurvival => 1,
            Self::GateVetoSecurity => 2,
            Self::HurdleNotMet => 3,
            Self::ConvergenceFailed => 4,
            Self::SecondExtensionFailed => 5,
            Self::ProcessHold => 6,
            Self::ConstitutionViolation => 7,
            Self::ResourceConflict => 8,
            Self::RateLimited => 9,
            Self::VetoUpheldByReview => 10,
            Self::StaleQueue => 11,
            Self::PayloadReverted => 12,
            Self::NotRatified => 13,
            Self::SecuritySizing => 14,
            Self::AttestationMissing => 15,
        }
    }
}
impl TradeSide {
    pub const fn codec_index(self) -> u8 {
        match self {
            Self::BuyLong => 0,
            Self::BuyShort => 1,
            Self::SellLong => 2,
            Self::SellShort => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_version_is_v3() {
        // Bumped 2 → 3 for the oracle per-version triple-key reconciliation (A5;
        // 02 §7.2/§13). A change to §2–§12 of the frozen contract bumps this.
        assert_eq!(INTEGRATION_CONTRACT_VERSION, 3);
    }

    #[test]
    fn bounded_vec_rejects_over_bound() {
        let values = alloc::vec![1_u8, 2, 3];
        assert!(BoundedVec::<_, 2>::try_from(values).is_err());
    }

    #[test]
    fn scale_decode_enforces_bounded_vec_limit() {
        let encoded = alloc::vec![1_u8, 2, 3].encode();
        assert!(BoundedVec::<u8, 2>::decode(&mut &encoded[..]).is_err());
    }

    struct CountingInput<'a> {
        data: &'a [u8],
        read: usize,
        alloc_mem: usize,
    }

    impl parity_scale_codec::Input for CountingInput<'_> {
        fn remaining_len(&mut self) -> Result<Option<usize>, parity_scale_codec::Error> {
            Ok(Some(self.data.len().saturating_sub(self.read)))
        }

        fn read(&mut self, into: &mut [u8]) -> Result<(), parity_scale_codec::Error> {
            let end = self
                .read
                .checked_add(into.len())
                .filter(|end| *end <= self.data.len())
                .ok_or_else(|| parity_scale_codec::Error::from("unexpected end of input"))?;
            into.copy_from_slice(&self.data[self.read..end]);
            self.read = end;
            Ok(())
        }

        fn on_before_alloc_mem(&mut self, size: usize) -> Result<(), parity_scale_codec::Error> {
            self.alloc_mem = self.alloc_mem.saturating_add(size);
            Ok(())
        }
    }

    #[test]
    fn scale_decode_rejects_oversized_length_before_reading_elements() {
        // 1000 advertised elements against a bound of 4: the decoder must fail
        // after the compact length prefix, without consuming element bytes.
        let encoded = alloc::vec![7_u8; 1000].encode();
        let prefix_len = parity_scale_codec::Compact(1000_u32).encoded_size();
        let mut input = CountingInput {
            data: &encoded,
            read: 0,
            alloc_mem: 0,
        };
        assert!(BoundedVec::<u8, 4>::decode(&mut input).is_err());
        assert_eq!(input.read, prefix_len);
        assert_eq!(input.alloc_mem, 0);
    }

    #[test]
    fn scale_decode_charges_allocation_for_in_bound_length() {
        let encoded = alloc::vec![7_u8; 4].encode();
        let mut input = CountingInput {
            data: &encoded,
            read: 0,
            alloc_mem: 0,
        };
        let decoded = BoundedVec::<u8, 4>::decode(&mut input).unwrap();
        assert_eq!(decoded.as_slice(), &[7, 7, 7, 7]);
        assert_eq!(input.alloc_mem, 4);
    }

    #[test]
    fn enum_indices_are_stable() {
        assert_eq!(Branch::Accept.codec_index(), 0);
        assert_eq!(RejectReason::AttestationMissing.codec_index(), 15);
        assert_eq!(TradeSide::SellShort.codec_index(), 3);
    }

    #[test]
    fn market_kind_indices_are_stable() {
        let variants = [
            MarketKind::DecisionAccept,
            MarketKind::DecisionReject,
            MarketKind::GateS_Adopt,
            MarketKind::GateS_Reject,
            MarketKind::GateC_Adopt,
            MarketKind::GateC_Reject,
            MarketKind::Baseline,
        ];
        for (index, kind) in variants.iter().enumerate() {
            let index = index as u8;
            assert_eq!(kind.codec_index(), index);
            // 02 §7 SCALE index = declaration order.
            assert_eq!(kind.encode(), alloc::vec![index]);
        }
    }

    #[test]
    fn market_kind_variant_names_match_contract_02_section_5() {
        use scale_info::TypeDef;
        // 02 §5 (`MarketCreated` row) freezes these exact names, and `02` is canonical
        // for any name that appears on the contract surface (02 line 5). The canonical
        // frontend decodes `MarketCreated.kind` by its TypeInfo variant name, so this
        // locks the spelling byte-for-byte against an accidental future rename (SQ-37).
        const CONTRACT_NAMES: [&str; 7] = [
            "DecisionAccept",
            "DecisionReject",
            "GateS_Adopt",
            "GateS_Reject",
            "GateC_Adopt",
            "GateC_Reject",
            "Baseline",
        ];
        let type_info = MarketKind::type_info();
        let names: alloc::vec::Vec<&str> = match &type_info.type_def {
            TypeDef::Variant(variant) => variant.variants.iter().map(|v| v.name).collect(),
            _ => panic!("MarketKind must encode as a SCALE variant type"),
        };
        assert_eq!(names, CONTRACT_NAMES);
    }

    #[test]
    fn oracle_round_view_fields_match_contract_02_section_4() {
        use scale_info::TypeDef;
        // 02 §4 (contract v3) freezes the FE-facing `OracleRoundView` projection.
        // The canonical frontend keys per-version games by these fields (incl.
        // `spec_version`, added in v3), so lock the name + SCALE order against
        // re-divergence (rule 5) — this is the §4 half of the SQ-58 reconciliation
        // that `RoundState`'s lock in `oracle-core` does not cover.
        const CONTRACT_FIELDS: [&str; 11] = [
            "component",
            "epoch",
            "spec_version",
            "round",
            "reporter",
            "value_1e9",
            "evidence_hash",
            "bond",
            "challenge_deadline",
            "acked_by_watchtowers",
            "escalated",
        ];
        let type_info = OracleRoundView::type_info();
        let names: alloc::vec::Vec<&str> = match &type_info.type_def {
            TypeDef::Composite(c) => c.fields.iter().filter_map(|f| f.name).collect(),
            _ => panic!("OracleRoundView must encode as a SCALE composite type"),
        };
        assert_eq!(names, CONTRACT_FIELDS);
    }

    #[test]
    fn proposal_scale_round_trips_and_bounds_resources() {
        let proposal = Proposal::<AccountId> {
            id: 7,
            proposer: [1u8; 32],
            class: ProposalClass::Treasury,
            state: ProposalState::Trading,
            epoch: 3,
            submitted_at: 100,
            payload_hash: [2u8; 32],
            payload_len: 4096,
            ask: 1_000_000,
            bond: 50_000,
            resources: BoundedVec::try_from(alloc::vec![[9u8; 8], [8u8; 8]]).unwrap(),
            metric_spec: 1,
            decide_at: 200,
            rerun: false,
            extended: true,
            delayed_once: false,
            markets: Some(MarketSet {
                accept: 1,
                reject: 2,
                gates: Some([3, 4, 5, 6]),
                baseline: 7,
            }),
            maturity: Some(300),
            grace_end: None,
            version_constraint: Some(RuntimeVersionConstraint {
                spec_name: BoundedVec::try_from(alloc::vec![98, 108, 101, 97, 118]).unwrap(),
                spec_version: 42,
            }),
            decision: Some(DecisionOutcome::Adopt),
        };
        let bytes = proposal.encode();
        // Declaration order is the SCALE layout: id (u64 LE) leads.
        assert_eq!(&bytes[..8], &7u64.to_le_bytes());
        assert_eq!(
            Proposal::<AccountId>::decode(&mut &bytes[..]).unwrap(),
            proposal
        );
        // Golden order-lock: independently concatenate every field's encoding in
        // the 05 §1.2 declaration order and require byte-equality, so a reordering
        // of fields 1–21 (which a plain round-trip would not catch) fails here.
        let mut ordered = Vec::new();
        ordered.extend(proposal.id.encode());
        ordered.extend(proposal.proposer.encode());
        ordered.extend(proposal.class.encode());
        ordered.extend(proposal.state.encode());
        ordered.extend(proposal.epoch.encode());
        ordered.extend(proposal.submitted_at.encode());
        ordered.extend(proposal.payload_hash.encode());
        ordered.extend(proposal.payload_len.encode());
        ordered.extend(proposal.ask.encode());
        ordered.extend(proposal.bond.encode());
        ordered.extend(proposal.resources.encode());
        ordered.extend(proposal.metric_spec.encode());
        ordered.extend(proposal.decide_at.encode());
        ordered.extend(proposal.rerun.encode());
        ordered.extend(proposal.extended.encode());
        ordered.extend(proposal.delayed_once.encode());
        ordered.extend(proposal.markets.encode());
        ordered.extend(proposal.maturity.encode());
        ordered.extend(proposal.grace_end.encode());
        ordered.extend(proposal.version_constraint.encode());
        ordered.extend(proposal.decision.encode());
        assert_eq!(
            bytes, ordered,
            "SCALE layout must follow 05 §1.2 field order"
        );
        // 02 §114: the record is bounded ≤ 512 B so `pallet-epoch`'s map is bounded.
        assert!(Proposal::<AccountId>::max_encoded_len() <= 512);

        // resources is bounded at 8 (13 §4): a 9-element encoding is rejected at decode.
        let nine = alloc::vec![[0u8; 8]; 9];
        let mut over = 7u64.encode();
        over.extend_from_slice(&[1u8; 32]); // proposer
        over.extend_from_slice(&ProposalClass::Treasury.encode());
        over.extend_from_slice(&ProposalState::Trading.encode());
        over.extend_from_slice(&3u32.encode()); // epoch
        over.extend_from_slice(&100u32.encode()); // submitted_at
        over.extend_from_slice(&[2u8; 32]); // payload_hash
        over.extend_from_slice(&4096u32.encode()); // payload_len
        over.extend_from_slice(&1_000_000u128.encode()); // ask
        over.extend_from_slice(&50_000u128.encode()); // bond
        over.extend_from_slice(&nine.encode()); // resources: 9 > bound 8
        assert!(Proposal::<AccountId>::decode(&mut &over[..]).is_err());
    }

    #[test]
    fn execution_record_scale_round_trips() {
        let record = ExecutionRecord {
            pid: 7,
            payload_hash: [2u8; 32],
            class: ProposalClass::Code,
            executed_at: 900,
            result: DispatchOutcomeCode::Ok,
        };
        let bytes = record.encode();
        assert_eq!(&bytes[..8], &7u64.to_le_bytes());
        assert_eq!(ExecutionRecord::decode(&mut &bytes[..]).unwrap(), record);
    }

    #[test]
    fn view_types_have_pinned_encoded_bounds() {
        // 02 §3/§4: the FutarchyApi view types are fully defined here and bounded.
        // Pinning the MaxEncodedLen locks their SCALE layout as a regression.
        assert_eq!(
            (
                EpochStatusView::max_encoded_len(),
                ProposalSummaryView::max_encoded_len(),
                QuoteView::max_encoded_len(),
                DecisionStatsView::max_encoded_len(),
                QueuedExecutionView::max_encoded_len(),
                RatificationStatus::max_encoded_len(),
            ),
            (19, 159, 57, 155, 93, 5)
        );
    }

    #[test]
    fn phase_offsets_are_monotonic_fractions_over_21() {
        use phase_offsets::*;
        assert_eq!(DENOMINATOR, 21);
        let boundaries = [
            INTAKE_NUM,
            QUALIFY_NUM,
            SEED_NUM,
            TRADE_NUM,
            DECIDE_WINDOW_NUM,
            DECIDE_NUM,
            HOUSEKEEPING_NUM,
        ];
        assert!(boundaries.windows(2).all(|w| w[0] < w[1]));
        assert!(*boundaries.last().unwrap() < DENOMINATOR);
        // Pin the exact 13 §3.1 numerators, not just their ordering.
        assert_eq!(boundaries, [0, 3, 4, 5, 15, 18, 20]);
    }
}
