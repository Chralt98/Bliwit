#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use core::convert::TryFrom;

use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub const INTEGRATION_CONTRACT_VERSION: u32 = 2;

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

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
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

    pub fn try_push(&mut self, value: T) -> Result<(), BoundExceeded> {
        if self.0.len() >= N as usize {
            return Err(BoundExceeded);
        }
        self.0.push(value);
        Ok(())
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

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct BoundExceeded;

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Branch {
    Accept,
    Reject,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum ScalarSide {
    Long,
    Short,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum GateType {
    Survival,
    Security,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum PositionKind {
    BranchUsdc,
    Long,
    Short,
    GateYes(GateType),
    GateNo(GateType),
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
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

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum VaultState {
    Open,
    Resolved(Branch),
    ScalarSettled { winner: Branch, s: FixedU64 },
    Voided,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum ProposalClass {
    Param,
    Treasury,
    Code,
    Meta,
    Constitutional,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
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

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
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

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
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

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum DecisionOutcome {
    Adopt,
    Reject(RejectReason),
    Extend,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum DispatchOutcomeCode {
    Ok,
    Failed { call_index: u8, error: [u8; 4] },
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum RatificationStatus {
    NotRequired,
    Pending { referendum: u32 },
    Passed { referendum: u32 },
    Failed { referendum: u32 },
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum TradeSide {
    BuyLong,
    BuyShort,
    SellLong,
    SellShort,
}

#[derive(Clone, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct RuntimeVersionConstraint {
    pub spec_name: BoundedVec<u8, 32>,
    pub spec_version: u32,
}

#[derive(Clone, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct EpochStatusView {
    pub index: EpochId,
    pub phase: EpochPhase,
    pub phase_start_block: BlockNumber,
    pub next_boundary: BlockNumber,
    pub dead_man_armed: bool,
    pub ledger_frozen: bool,
    pub phase_flags: u32,
}

#[derive(Clone, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
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

#[derive(Clone, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct QuoteView {
    pub cost: Balance,
    pub fee: Balance,
    pub p_after_1e9: FixedU64,
    pub max_trade: Balance,
    pub within_domain: bool,
}

#[derive(Clone, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
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

#[derive(Clone, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct PositionView {
    pub position: PositionId,
    pub balance: Balance,
    pub vault_state: VaultState,
}

#[derive(Clone, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
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

#[derive(Clone, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
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

#[derive(Clone, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
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

#[derive(Clone, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
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

#[derive(Clone, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct CohortSummary {
    pub epoch: EpochId,
    pub s_1e9: FixedU64,
    pub baseline_twap_1e9: FixedU64,
    pub proposals: BoundedVec<(ProposalId, ProposalClass, DecisionOutcome), 5>,
    pub voided: bool,
    pub settled_at: BlockNumber,
}
pub type CohortSummaryView = CohortSummary;

#[derive(Clone, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct OracleRoundView {
    pub component: MetricId,
    pub epoch: EpochId,
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
    pub const USDC_CENT: u128 = 10_000;
    pub const VIT_EXISTENTIAL_DEPOSIT: u128 = 10_000_000_000;
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
    pub const QUOTE_CLAMP_MIN_1E9: u64 = 1_000_000;
    pub const QUOTE_CLAMP_MAX_1E9: u64 = 999_000_000;
    pub const GATE_P_MAX_CEILING_1E9: u64 = 100_000_000;
    pub const ORC_MAX_PROOF_BYTES: u32 = 256 * 1024;
    pub const REG_MAX_FILINGS_EPOCH: u32 = 64;
    pub const WT_MAX: u32 = 16;
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

impl Branch {
    pub const fn codec_index(self) -> u8 {
        match self {
            Self::Accept => 0,
            Self::Reject => 1,
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
    fn contract_version_is_v2() {
        assert_eq!(INTEGRATION_CONTRACT_VERSION, 2);
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
}
