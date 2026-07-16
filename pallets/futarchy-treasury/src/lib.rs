#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! # `pallet-futarchy-treasury` — solvency, NAV, outflow controls (A9)
//!
//! Production FRAME shell over the frame-free functional core
//! [`futarchy_treasury_core`], which remains the differential oracle
//! (Python M3 ≡ Rust core ≡ this pallet) and the auditor-consumable port.
//!
//! Spec: `docs/architecture/08` (§1 accounts/NAV/outflow controls/streams/
//! meters/calls, §6 keeper economics, §7 intake economics, §8 POL seeding,
//! §9 fees), `02 §4` (`NavView` view type consumed by the B2 `FutarchyApi`),
//! `13 §1/§4` (tunables + storage bounds), `15 §1` (I-7 monotone-meter and
//! solvency try-state coverage — the I-7/I-17 *limits* live as constitution
//! `Params`, the *metering* lives here per the amended 15 §1 / PLAN SQ-12).
//!
//! ## Scope boundary (custody vs. accounting)
//!
//! This pallet is the treasury **policy and accounting** layer: sub-account
//! balances, NAV with reserve haircuts, per-proposal / rolling outflow meters,
//! mandatory-stream vesting, budget lines, VIT issuance metering, and the
//! coretime-renewal call. The **custody** of the underlying assets — moving
//! bridged USDC (`ForeignAssets`, 02 §8) and native VIT via
//! `fungible(s)`/XCM — is wired at the runtime level (B1a/B4); the core (and
//! therefore this shell) tracks the authoritative accounting the custody layer
//! must honour, exactly as the ledger (A2) and market (A3) cores do.
//!
//! ## Origins (08 §1.1)
//!
//! Every outflow call (`spend`, `open_stream`, `cancel_stream`,
//! `fund_budget_line`, `issue_vit`, `recover_foreign`) requires the
//! `FutarchyTreasury` origin, dispatched by the execution guard when a
//! TREASURY-class decision executes (06 §3 / 09). `claim_stream` is a Signed
//! recipient call; `execute_coretime_renewal` is a permissionless Signed keeper
//! call, freeze-exempt (D-9). The runtime wires [`Config::TreasuryOrigin`] over
//! `pallet-origins` (A4/B1a); the mock provides a test resolver.
//!
//! ## Parameters (rule 4)
//!
//! The outflow caps (`trs.cap_proposal`/`cap_30d`/`cap_180d`), the mandatory-
//! stream threshold (`trs.stream_threshold`) and the issuance cap
//! (`iss.inflation_cap`) are **read from `pallet-constitution::Params`** via
//! [`Config::Params`] on every dispatch and threaded into the core's tunable
//! seams — never hardcoded in this pallet. The frame-free core carries the
//! 13 §1 defaults so it and the M3 model behave identically at default
//! parameters; the runtime overwrites them from live records (B1a).

extern crate alloc;

pub use pallet::*;
pub use weights::WeightInfo;

pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

// The functional core is the semantic source of truth; re-export its surface
// (named, not glob — the pallet defines its own `Event`/`Error`).
pub use futarchy_treasury_core::{
    bps, vested_amount, AssetKind, BudgetLine, Error as CoreError, Event as CoreEvent, KeeperMeter,
    KeeperMeterClass, NavComponents, RollingMeter, Stream, StreamInput, Treasury, TreasuryAccount,
    DEFAULT_VIT_SUPPLY, ISS_INFLATION_CAP_BPS, MAX_BUDGET_LINES, MAX_FUNDED_CORETIME_PERIODS,
    MAX_PENDING_OUTFLOWS, MAX_POL_COMMITMENTS, MAX_STREAMS, TRS_CAP_180D_BPS, TRS_CAP_30D_BPS,
    TRS_CAP_PROPOSAL_BPS, TRS_STREAM_THRESHOLD_BPS, USDC, VIT,
};
pub use origins_core::Origin;

/// 13 §4 storage bounds, in the `u32` form `ConstU32` needs (each references
/// the single core definition — no duplicated literal, rule 4).
pub const MAX_BUDGET_LINES_BOUND: u32 = MAX_BUDGET_LINES as u32;
/// See [`MAX_BUDGET_LINES_BOUND`].
pub const MAX_STREAMS_BOUND: u32 = MAX_STREAMS as u32;
/// See [`MAX_BUDGET_LINES_BOUND`].
pub const MAX_PENDING_OUTFLOWS_BOUND: u32 = MAX_PENDING_OUTFLOWS as u32;
/// See [`MAX_BUDGET_LINES_BOUND`].
pub const MAX_POL_COMMITMENTS_BOUND: u32 = MAX_POL_COMMITMENTS as u32;
/// See [`MAX_BUDGET_LINES_BOUND`].
pub const MAX_FUNDED_CORETIME_BOUND: u32 = MAX_FUNDED_CORETIME_PERIODS as u32;

/// Live treasury tunables (rule 4). The runtime implements this over
/// `pallet-constitution::Params` (B1a), converting each typed record to basis
/// points; the mock implements it with overridable statics defaulting to the
/// 13 §1 defaults. Values are basis points of NAV (`cap_*`, `stream_threshold`)
/// or of supply-at-window-start (`inflation_cap`).
pub trait TreasuryParams {
    /// `trs.cap_proposal` — per-proposal outflow ceiling (default 5% = 500 bps).
    fn cap_proposal_bps() -> u32;
    /// `trs.cap_30d` — rolling 30-day outflow ceiling (default 10% = 1000 bps).
    fn cap_30d_bps() -> u32;
    /// `trs.cap_180d` — rolling 180-day outflow ceiling (default 30% = 3000 bps).
    fn cap_180d_bps() -> u32;
    /// `trs.stream_threshold` — grants above this MUST stream (default 1% = 100 bps).
    fn stream_threshold_bps() -> u32;
    /// `iss.inflation_cap` — rolling 365-day issuance ceiling (default 2% = 200 bps).
    fn inflation_cap_bps() -> u32;
    /// `keeper.budget_epoch` (raw key `keeper.budget`) — per-epoch metered
    /// keeper budget, default 12,000 USDC (13 §1).
    fn keeper_budget_epoch() -> futarchy_primitives::Balance;
    /// `keeper.rebate` — rebate per sanctioned crank. This formula-default row
    /// is deliberately absent from the genesis Params registry pending B5 fee
    /// calibration; adapters MUST return zero until the raw row exists. A zero
    /// rebate makes the entire path a structural no-op and cannot create an
    /// unbacked outflow.
    fn keeper_rebate() -> futarchy_primitives::Balance;
}

/// Custody account from which a rebate is paid.
#[derive(
    Clone,
    Copy,
    Debug,
    parity_scale_codec::Decode,
    parity_scale_codec::DecodeWithMemTracking,
    parity_scale_codec::Encode,
    Eq,
    parity_scale_codec::MaxEncodedLen,
    PartialEq,
    scale_info::TypeInfo,
)]
pub enum PayoutLine {
    Keeper,
    Oracle,
}

/// Narrow runtime custody seam for real-USDC keeper payouts.
pub trait RebatePayout<AccountId> {
    fn pay(
        who: &AccountId,
        amount: futarchy_primitives::Balance,
        line: PayoutLine,
    ) -> frame_support::pallet_prelude::DispatchResult;

    /// Real USDC held by the custody pot corresponding to `line`.
    fn pot_balance(line: PayoutLine) -> futarchy_primitives::Balance;
}

impl<AccountId> RebatePayout<AccountId> for () {
    fn pay(
        _: &AccountId,
        _: futarchy_primitives::Balance,
        _: PayoutLine,
    ) -> frame_support::pallet_prelude::DispatchResult {
        Ok(())
    }

    fn pot_balance(_: PayoutLine) -> futarchy_primitives::Balance {
        0
    }
}

/// B4/B1a seam (09 §4): dispatch the DOT funding transfer for a renewal the
/// accounting just committed. An `Err` rolls back the whole extrinsic (quote
/// restored, period not funded) so the keeper can retry — bounded retry via
/// re-cranks (09 §4); remote failures after a successful local dispatch remain
/// keeper-monitored and never enter a decision path (I-24).
pub trait RenewalDispatch {
    fn dispatch_renewal(
        period_index: u32,
        amount: futarchy_primitives::Balance,
    ) -> frame_support::pallet_prelude::DispatchResult;
}

impl RenewalDispatch for () {
    fn dispatch_renewal(
        _: u32,
        _: futarchy_primitives::Balance,
    ) -> frame_support::pallet_prelude::DispatchResult {
        Ok(())
    }
}

/// Maps a benchmark scenario to concrete runtime origins so the benchmark
/// harness can exercise every call with its exact 08 §1.1 authority.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<RuntimeOrigin, AccountId> {
    /// A runtime origin that [`Config::TreasuryOrigin`] admits.
    fn treasury_origin() -> RuntimeOrigin;
    /// A funded keeper/recipient account for Signed calls.
    fn account(seed: u8) -> AccountId;
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use alloc::vec::Vec;
    use frame_support::pallet_prelude::*;
    use frame_support::traits::EnsureOrigin;
    use frame_system::pallet_prelude::*;
    use sp_runtime::traits::SaturatedConversion;
    use sp_runtime::TryRuntimeError;

    use futarchy_primitives::{
        keeper::{CrankClass, KeeperRebateSink},
        Balance, BlockNumber, EpochId, ProposalClass,
    };

    /// The in-code storage version of this pallet.
    const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config:
        frame_system::Config<
        RuntimeEvent: From<Event<Self>>,
        AccountId: From<[u8; 32]> + Into<[u8; 32]>,
    >
    {
        /// Admits the `FutarchyTreasury` origin (08 §1.1) — the only origin the
        /// outflow calls accept. The runtime wires this to `pallet-origins`
        /// (A4/B1a); no signed or unsigned origin may satisfy it. Narrow by
        /// construction (rule 6): a mis-wired origin cannot reach an outflow.
        type TreasuryOrigin: EnsureOrigin<Self::RuntimeOrigin>;

        /// Live 13 §1 treasury tunables, read from `pallet-constitution::Params`
        /// (rule 4). See [`TreasuryParams`].
        type Params: TreasuryParams;

        /// Current epoch index, for the `NavHaircutFlagged` epoch stamp and
        /// issuance windows. Wired to `pallet-epoch`'s clock by the runtime
        /// (A8/B1a); a constant in the mock.
        type CurrentEpoch: Get<EpochId>;

        /// Runtime XCM adapter for the DOT renewal-funding leg (09 §4). The
        /// accounting pallet stays XCM-free; an error rolls the extrinsic back.
        type RenewalDispatch: RenewalDispatch;

        /// Runtime custody adapter which transfers real USDC from the selected
        /// treasury sub-account. Errors are swallowed by `do_keeper_rebate`.
        type RebatePayout: RebatePayout<Self::AccountId>;

        /// Weight information for extrinsics.
        type WeightInfo: WeightInfo;

        /// Origin/account construction for benchmarking.
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: BenchmarkHelper<Self::RuntimeOrigin, Self::AccountId>;
    }

    /// Bounded storage mirror of the core [`Treasury`] aggregate (rule 3:
    /// `MaxEncodedLen`, every collection a `BoundedVec` bounded by 13 §4). The
    /// transient `events` vec and the Params-derived tunable seams are **not**
    /// persisted: events are deposited per-dispatch, the seams are re-read from
    /// `Params` on every load. The pallet delegates every state transition to
    /// the core over this mirror, so shell ≡ core is near-tautological and the
    /// differential test guards the conversion.
    #[derive(Clone, Debug, Encode, Decode, MaxEncodedLen, TypeInfo, PartialEq, Eq)]
    pub struct TreasuryState {
        pub main_usdc: Balance,
        pub vit_supply: Balance,
        pub reserve_impaired: bool,
        pub lines: BoundedVec<(BudgetLine, Balance), ConstU32<MAX_BUDGET_LINES_BOUND>>,
        pub streams: BoundedVec<Stream, ConstU32<MAX_STREAMS_BOUND>>,
        pub pending_outflows: BoundedVec<Balance, ConstU32<MAX_PENDING_OUTFLOWS_BOUND>>,
        pub pol_commitments: BoundedVec<Balance, ConstU32<MAX_POL_COMMITMENTS_BOUND>>,
        pub meter_30d: RollingMeter<31>,
        pub meter_180d: RollingMeter<181>,
        pub issuance: RollingMeter<366>,
        pub next_stream_id: u64,
        pub vit_lines: BoundedVec<(BudgetLine, Balance), ConstU32<MAX_BUDGET_LINES_BOUND>>,
        pub funded_coretime_periods: BoundedVec<u32, ConstU32<MAX_FUNDED_CORETIME_BOUND>>,
        pub coretime_quotes: BoundedVec<(u32, Balance), ConstU32<MAX_FUNDED_CORETIME_BOUND>>,
        pub keeper_meter: KeeperMeter,
    }

    impl Default for TreasuryState {
        fn default() -> Self {
            // Mirror the core default (1e9 VIT supply, zeroed meters at the
            // 13 defaults). Every collection is empty, so `truncate_from` can
            // drop nothing — the pre-genesis `ValueQuery` default is infallible.
            Self::truncating_from_core(&Treasury::default())
        }
    }

    impl TreasuryState {
        /// Infallible conversion used only for the empty pre-genesis default and
        /// genesis (guarded by a `try_state` assert): `truncate_from` is a no-op
        /// on already-bounded input.
        fn truncating_from_core(t: &Treasury) -> Self {
            Self {
                main_usdc: t.main_usdc,
                vit_supply: t.vit_supply,
                reserve_impaired: t.reserve_impaired,
                lines: BoundedVec::truncate_from(t.lines.clone()),
                streams: BoundedVec::truncate_from(t.streams.clone()),
                pending_outflows: BoundedVec::truncate_from(t.pending_outflows.clone()),
                pol_commitments: BoundedVec::truncate_from(t.pol_commitments.clone()),
                meter_30d: t.meter_30d,
                meter_180d: t.meter_180d,
                issuance: t.issuance,
                next_stream_id: t.next_stream_id,
                vit_lines: BoundedVec::truncate_from(t.vit_lines.clone()),
                funded_coretime_periods: BoundedVec::truncate_from(
                    t.funded_coretime_periods.clone(),
                ),
                coretime_quotes: BoundedVec::truncate_from(t.coretime_quotes.clone()),
                keeper_meter: t.keeper_meter,
            }
        }
    }

    /// The whole treasury accounting state (08 §1). Kept as one bounded value:
    /// NAV — the base every outflow check reads — sums `main_usdc`, all line
    /// balances, open stream remainders, pending outflows and POL commitments,
    /// so the hot path needs the whole aggregate regardless. `MaxEncodedLen` is
    /// bounded (rule 3); B5 may split hot items if PoV benchmarks demand it.
    #[pallet::storage]
    pub type State<T: Config> = StorageValue<_, TreasuryState, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A direct in-cap grant paid from a budget line (08 §1.3).
        Spent {
            line: BudgetLine,
            dest: T::AccountId,
            amount: Balance,
        },
        /// A vesting stream was opened (grant > `trs.stream_threshold`).
        StreamOpened {
            id: u64,
            recipient: T::AccountId,
            total: Balance,
        },
        /// A recipient claimed vested funds from a stream.
        StreamClaimed {
            id: u64,
            recipient: T::AccountId,
            amount: Balance,
        },
        /// A TREASURY decision cancelled a stream; the remainder reverts to `MAIN`.
        StreamCancelled { id: u64, reverted: Balance },
        /// A budget line was funded from `MAIN` (08 §1.1).
        BudgetLineFunded { line: BudgetLine, amount: Balance },
        /// VIT was minted within the `iss.inflation_cap` window (08 §2.3).
        VitIssued {
            amount: Balance,
            line: BudgetLine,
            meter_after: Balance,
        },
        /// The reserve-health flag `R` transitioned (08 §1.2, 07 §8).
        NavHaircutFlagged { epoch: EpochId, flag: bool },
        /// Mistakenly-sent foreign assets were recovered (TREASURY-only, 08 §1.3).
        ForeignRecovered {
            asset: AssetKind,
            dest: T::AccountId,
            amount: Balance,
        },
        /// A coretime renewal was paid from `ops.coretime` (09 §4, dead-man exempt).
        CoretimeRenewalCalled { line: BudgetLine, amount: Balance },
        /// A class-arming attempt failed the minimum-viable-NAV floor (08 §4.2, loud).
        NavFloorUnmet {
            class: ProposalClass,
            nav: Balance,
            floor: Balance,
        },
        /// The metered keeper budget passed 80% (08 §6.3).
        KeeperBudgetLow { remaining: Balance },
        /// The metered keeper budget is exhausted (08 §6.3).
        KeeperBudgetExhausted { epoch: EpochId, spent: Balance },
    }

    /// 1:1 with [`CoreError`]; `CoreError::BadOrigin` maps to
    /// `DispatchError::BadOrigin` (FRAME convention).
    #[pallet::error]
    pub enum Error<T> {
        /// No such budget line exists in the treasury.
        UnknownBudgetLine,
        /// The source (main or a line) lacks the funds for the debit.
        InsufficientFunds,
        /// The reserve-health flag is set: spendable NAV is 0, no new commitments.
        ReserveImpaired,
        /// Outflow exceeds `trs.cap_proposal` × spendable NAV.
        ProposalCapExceeded,
        /// Grant exceeds `trs.stream_threshold`: it MUST be a stream, not a spend.
        StreamRequired,
        /// A rolling outflow (30d/180d) or issuance meter would be exceeded (I-7).
        MeterExhausted,
        /// No stream with the given id.
        StreamNotFound,
        /// Nothing vested-but-unclaimed on the stream.
        StreamNotClaimable,
        /// Caller is not the stream's recipient.
        NotRecipient,
        /// The stream is already cancelled.
        AlreadyCancelled,
        /// Stream duration must be non-zero.
        BadDuration,
        /// No coretime renewal quote is open for this period (window closed).
        RenewalWindowClosed,
        /// This coretime period is already funded (renewal idempotency).
        PeriodAlreadyFunded,
        /// The `Streams` bound (13 §4) would be exceeded.
        TooManyStreams,
        /// The budget-line bound (13 §4) would be exceeded.
        TooManyBudgetLines,
        /// A pending-outflow / POL / coretime obligation bound (13 §4) would be exceeded.
        TooManyObligations,
        /// `issue_vit` targets a line other than `REWARDS`/`ops.*` (08 §2.3).
        IssuanceLineNotAllowed,
        /// Minting would exceed `iss.inflation_cap` × supply-at-window-start.
        IssuanceCapExceeded,
        /// `recover_foreign` was asked to move a protocol asset (USDC/VIT).
        UnknownForeignAsset,
        /// Published spendable NAV is below the class arming floor (08 §4.1).
        NavFloorUnmet,
        /// A coretime renewal quote of zero was rejected (09 §4).
        ZeroQuote,
        /// Arithmetic overflow — rejected, never wrapped (G-1).
        Overflow,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        // 15 §1 try-state coverage: the core validator (bounded collections,
        // per-stream `claimed ≤ total` & `duration > 0`, `Σ vit_lines ≤
        // vit_supply`) plus the FRAME-side solvency identities below. No cranks:
        // 08 gives the treasury no cursor-bounded hooks (metering is per-call).
        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), TryRuntimeError> {
            Self::do_try_state()
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// `treasury.fund_budget_line(line, amount)` — move `amount` from `MAIN`
        /// into a budget line (08 §1.1). Origin: `FutarchyTreasury`.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::fund_budget_line())]
        pub fn fund_budget_line(
            origin: OriginFor<T>,
            line: BudgetLine,
            amount: Balance,
        ) -> DispatchResult {
            T::TreasuryOrigin::ensure_origin(origin)?;
            Self::mutate(|t| t.fund_budget_line(Origin::FutarchyTreasury, line, amount))
        }

        /// `treasury.spend(line, dest, amount)` — a direct in-cap grant
        /// (08 §1.3/§1.4). Rejected above `trs.stream_threshold` (`StreamRequired`),
        /// above `trs.cap_proposal`×NAV (`ProposalCapExceeded`), under the
        /// reserve haircut (`ReserveImpaired`), or over a rolling meter
        /// (`MeterExhausted`). Origin: `FutarchyTreasury`.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::spend())]
        pub fn spend(
            origin: OriginFor<T>,
            line: BudgetLine,
            dest: T::AccountId,
            amount: Balance,
        ) -> DispatchResult {
            T::TreasuryOrigin::ensure_origin(origin)?;
            let now = Self::now();
            let dest = Self::to_core_account(dest);
            Self::mutate(|t| t.spend(Origin::FutarchyTreasury, now, line, dest, amount))
        }

        /// `treasury.open_stream(line, recipient, total, start, duration)` — a
        /// mandatory vesting stream for a grant > `trs.stream_threshold`
        /// (08 §1.3/§1.4). The `line` names the funding budget line (08 §1.1:
        /// outflow calls MUST name a line; the 08 §1.4 signature omits it — see
        /// PLAN note). Origin: `FutarchyTreasury`.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::open_stream())]
        pub fn open_stream(
            origin: OriginFor<T>,
            line: BudgetLine,
            recipient: T::AccountId,
            total: Balance,
            start: BlockNumberFor<T>,
            duration: BlockNumberFor<T>,
        ) -> DispatchResult {
            T::TreasuryOrigin::ensure_origin(origin)?;
            let now = Self::now();
            let input = StreamInput {
                line,
                recipient: Self::to_core_account(recipient),
                total,
                start: start.saturated_into::<BlockNumber>(),
                duration: duration.saturated_into::<BlockNumber>(),
            };
            Self::mutate(|t| {
                t.open_stream(Origin::FutarchyTreasury, now, input)
                    .map(|_| ())
            })
        }

        /// `treasury.claim_stream(id)` — the recipient claims vested funds
        /// (08 §1.4, Signed recipient).
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::claim_stream())]
        pub fn claim_stream(origin: OriginFor<T>, id: u64) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let now = Self::now();
            let who = Self::to_core_account(who);
            Self::mutate(|t| t.claim_stream(who, now, id).map(|_| ()))
        }

        /// `treasury.cancel_stream(id)` — a later TREASURY decision cancels a
        /// stream; the undisbursed remainder reverts to `MAIN` (08 §1.3).
        /// Origin: `FutarchyTreasury`.
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::cancel_stream())]
        pub fn cancel_stream(origin: OriginFor<T>, id: u64) -> DispatchResult {
            T::TreasuryOrigin::ensure_origin(origin)?;
            Self::mutate(|t| t.cancel_stream(Origin::FutarchyTreasury, id).map(|_| ()))
        }

        /// `treasury.issue_vit(amount, line)` — mint VIT within the rolling
        /// `iss.inflation_cap` window to a `REWARDS`/`ops.*` line (08 §2.3).
        /// Origin: `FutarchyTreasury`.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::issue_vit())]
        pub fn issue_vit(
            origin: OriginFor<T>,
            amount: Balance,
            line: BudgetLine,
        ) -> DispatchResult {
            T::TreasuryOrigin::ensure_origin(origin)?;
            let now = Self::now();
            Self::mutate(|t| t.issue_vit(Origin::FutarchyTreasury, now, amount, line))
        }

        /// `treasury.recover_foreign(asset, dest, amount)` — sweep assets sent to
        /// pallet accounts outside protocol flows (08 §1.3, TREASURY-class only,
        /// never a protocol asset). Origin: `FutarchyTreasury`.
        #[pallet::call_index(6)]
        #[pallet::weight(T::WeightInfo::recover_foreign())]
        pub fn recover_foreign(
            origin: OriginFor<T>,
            asset: AssetKind,
            dest: T::AccountId,
            amount: Balance,
        ) -> DispatchResult {
            T::TreasuryOrigin::ensure_origin(origin)?;
            let dest = Self::to_core_account(dest);
            Self::mutate(|t| t.recover_foreign(Origin::FutarchyTreasury, asset, dest, amount))
        }

        /// `treasury.execute_coretime_renewal(period_index)` — pay the
        /// runtime-noted renewal quote from `ops.coretime` (09 §4). Permissionless
        /// Signed keeper, idempotent per period, freeze-exempt (D-9), bounded by
        /// the pre-authorized line balance and the noted quote (a keeper can
        /// neither fund a period for free nor choose the amount).
        #[pallet::call_index(7)]
        #[pallet::weight(T::WeightInfo::execute_coretime_renewal())]
        pub fn execute_coretime_renewal(origin: OriginFor<T>, period_index: u32) -> DispatchResult {
            let keeper = ensure_signed(origin)?;
            let core_keeper = Self::to_core_account(keeper.clone());
            let amount = Self::mutate(|t| t.execute_coretime_renewal(core_keeper, period_index))?;
            T::RenewalDispatch::dispatch_renewal(period_index, amount)?;
            // B5 recalibrates this call's weight for the additional bounded
            // keeper-meter read/write and custody-transfer path.
            Self::do_keeper_rebate(&keeper, CrankClass::General);
            Ok(())
        }
    }

    #[pallet::extra_constants]
    impl<T: Config> Pallet<T> {
        /// 02 §2/§8: `INTEGRATION_CONTRACT_VERSION`, metadata-readable.
        #[pallet::constant_name(INTEGRATION_CONTRACT_VERSION)]
        fn integration_contract_version() -> u32 {
            futarchy_primitives::INTEGRATION_CONTRACT_VERSION
        }
        /// 13 §4 bound on open vesting streams.
        #[pallet::constant_name(MaxStreams)]
        fn max_streams() -> u32 {
            MAX_STREAMS_BOUND
        }
        /// 13 §4 bound on budget lines.
        #[pallet::constant_name(MaxBudgetLines)]
        fn max_budget_lines() -> u32 {
            MAX_BUDGET_LINES_BOUND
        }
        /// 13 §4 bound on POL commitments (= `MaxLiveMarkets`).
        #[pallet::constant_name(MaxPolCommitments)]
        fn max_pol_commitments() -> u32 {
            MAX_POL_COMMITMENTS_BOUND
        }
    }

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Initial liquid USDC in `MAIN` (08 §2.5 funding target; normally
        /// transferred in via XCM before Phase-4 arming).
        ///
        /// The VIT supply is NOT configurable: 08 §2.1 fixes it at exactly
        /// 1,000,000,000 VIT ([`DEFAULT_VIT_SUPPLY`], a chain identity), so a
        /// chain spec cannot mint a different total (Codex review).
        pub main_usdc: Balance,
        #[serde(skip)]
        pub _config: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                main_usdc: 0,
                _config: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        // Genesis sets only the deployment-specific `MAIN` USDC funding; the VIT
        // supply is the fixed 08 §2.1 identity (`Treasury::default()` = 1e9 VIT).
        // Budget lines are funded post-genesis by TREASURY decisions once USDC
        // has arrived over XCM (08 §2.5), never from the chain spec; the metered
        // caps come from `Params` at first dispatch. Genesis-time `assert!` is
        // the FRAME convention for an invalid chain spec — it runs before any
        // block, so the G-1 status-quo rule for dispatch paths does not apply.
        fn build(&self) {
            let t = Treasury {
                main_usdc: self.main_usdc,
                ..Treasury::default()
            };
            assert!(
                t.try_state().is_ok(),
                "treasury genesis violates the 13 §4 storage bounds or I-7 solvency invariants"
            );
            // `try_state` above proves every collection is within bound, so the
            // truncating conversion drops nothing.
            State::<T>::put(TreasuryState::truncating_from_core(&t));
        }
    }

    impl<T: Config> Pallet<T> {
        // ---- runtime-internal (non-extrinsic) entry points -----------------

        /// 07 §8 / 08 §1.2: the oracle's reserve-health probe sets/clears the
        /// haircut flag `R`. Runtime-internal (a sibling-pallet Rust call, wired
        /// at B1a); emits `NavHaircutFlagged` on every transition.
        pub fn set_reserve_impaired(flag: bool) {
            let mut t = Self::load();
            t.set_reserve_impaired(T::CurrentEpoch::get(), flag);
            // `set_reserve_impaired` only flips the flag / pushes an event; the
            // conversion cannot exceed a bound, but persist defensively anyway.
            let _ = Self::persist(t);
        }

        /// Infallible, fail-soft keeper rebate endpoint (08 §6.3 / 07).
        ///
        /// The core mutation is prepared in memory. Custody pays first; only a
        /// successful payout commits the line debit and meter charge. Any
        /// payout or conversion failure silently drops the prepared state and
        /// therefore can never affect the useful crank which called this.
        /// Caller dispatch weights include this bounded accounting/custody
        /// work provisionally; milestone B5 recalibrates them by benchmark.
        pub fn do_keeper_rebate(who: &T::AccountId, class: CrankClass) {
            let mut t = Self::load();
            let rebate = T::Params::keeper_rebate();
            let payable = match class {
                CrankClass::DecisionCritical => t.keeper_rebate(
                    T::CurrentEpoch::get(),
                    KeeperMeterClass::DecisionCritical,
                    rebate,
                    T::Params::keeper_budget_epoch(),
                ),
                CrankClass::General => t.keeper_rebate(
                    T::CurrentEpoch::get(),
                    KeeperMeterClass::General,
                    rebate,
                    T::Params::keeper_budget_epoch(),
                ),
                CrankClass::OracleLine => t.oracle_line_rebate(rebate),
            };

            let events = core::mem::take(&mut t.events);
            if payable == 0 {
                // Only threshold flags/events (not ordinary zero/unfunded
                // no-ops) are durable on the zero-pay path.
                if events.is_empty() {
                    return;
                }
                if let Ok(state) = Self::checked_state(&t) {
                    State::<T>::put(state);
                    for event in events {
                        Self::deposit_core_event(event);
                    }
                }
                return;
            }

            // Preflight the only fallible accounting conversion before moving
            // real USDC, so a custody success can always be followed by the
            // corresponding line/meter commit.
            let Ok(state) = Self::checked_state(&t) else {
                return;
            };
            let line = match class {
                CrankClass::OracleLine => PayoutLine::Oracle,
                CrankClass::DecisionCritical | CrankClass::General => PayoutLine::Keeper,
            };
            if T::RebatePayout::pay(who, payable, line).is_err() {
                return;
            }
            State::<T>::put(state);
            for event in events {
                Self::deposit_core_event(event);
            }
        }

        /// 09 §4/§6: the runtime notes the current coretime renewal quote read
        /// from Coretime-chain state (never a user-facing value). A present quote
        /// is what opens the renewal window. Runtime-internal (B4 wires it).
        pub fn note_coretime_renewal_quote(period_index: u32, price: Balance) -> DispatchResult {
            let mut t = Self::load();
            t.note_coretime_renewal_quote(period_index, price)
                .map_err(Self::map_core_error)?;
            Self::persist(t)
        }

        /// 09 §4/§6: drop a stale open coretime quote so the bounded quote slot
        /// cannot be clogged. Runtime-internal (B4 prunes on re-read; quote
        /// validity intervals are PLAN SQ-53). No-op if there is no open quote.
        pub fn prune_coretime_quote(period_index: u32) -> DispatchResult {
            let mut t = Self::load();
            t.prune_coretime_quote(period_index);
            Self::persist(t)
        }

        /// 08 §1.2/§8.2: sync the live-book POL subsidy commitments `nav()` nets
        /// as obligations. Runtime-internal — the POL/market lifecycle (A3) owns
        /// the set; B1a wires this so NAV reflects live commitments. The
        /// registration trigger/keying is a cross-pallet concern (PLAN SQ-47).
        pub fn set_pol_commitments(commitments: Vec<Balance>) -> DispatchResult {
            let mut t = Self::load();
            t.set_pol_commitments(&commitments)
                .map_err(Self::map_core_error)?;
            Self::persist(t)
        }

        /// 08 §1.2/§1.3: sync the queued in-cap proposal outflows `nav()` nets as
        /// obligations. Runtime-internal — the execution-guard queue (A11) owns
        /// them; B1a wires this (PLAN SQ-47).
        pub fn set_pending_outflows(outflows: Vec<Balance>) -> DispatchResult {
            let mut t = Self::load();
            t.set_pending_outflows(&outflows)
                .map_err(Self::map_core_error)?;
            Self::persist(t)
        }

        /// 08 §4.2 minimum-viable-NAV arming gate — the **hard** variant: returns
        /// `Err(NavFloorUnmet)` when spendable NAV is below the class floor, and
        /// emits **no** event. A caller that propagates this `Err` fails its
        /// dispatch, which rolls back any event anyway; so the loud event is the
        /// caller's job via [`Self::flag_nav_floor`] (Codex review). Under the
        /// reserve haircut spendable NAV is 0, so every class fails.
        pub fn ensure_nav_floor(class: ProposalClass) -> DispatchResult {
            ensure!(
                Self::nav().spendable_nav >= Treasury::floor(class),
                Error::<T>::NavFloorUnmet
            );
            Ok(())
        }

        /// 08 §4.2/§4.4 minimum-viable-NAV arming gate — the **loud** variant:
        /// if spendable NAV is below the class floor it deposits the durable
        /// `NavFloorUnmet { class, nav, floor }` event and returns `true`;
        /// otherwise returns `false`. `pallet-epoch`'s arming crank (A8) calls
        /// this on its **`Ok`-returning** path (arming "rejects as deferred",
        /// 08 §4.4) so the event survives — unlike an `Err`, which FRAME rolls
        /// back. No balance changes, so nothing is persisted.
        pub fn flag_nav_floor(class: ProposalClass) -> bool {
            let mut t = Self::load();
            let below = t.ensure_nav_floor(class).is_err();
            for ev in core::mem::take(&mut t.events) {
                Self::deposit_core_event(ev);
            }
            below
        }

        // ---- read helpers (views / sibling pallets) ------------------------

        /// 08 §1.2 NAV components (the core view: `{ nav, spendable_nav,
        /// reserve_impaired, meter_utilization_bps }`). The B2 `FutarchyApi`
        /// builds the account-decomposed 02 §4 `NavView` from this + the line
        /// balances.
        pub fn nav() -> NavComponents {
            Self::load().nav()
        }

        /// The minimum-viable-NAV floor for a class (08 §4.1).
        pub fn floor(class: ProposalClass) -> Balance {
            Treasury::floor(class)
        }

        /// USDC balance of one budget line.
        pub fn line_balance(line: BudgetLine) -> Balance {
            Self::load().line_balance(line)
        }

        /// Minted-VIT balance credited to one line by `issue_vit` (08 §2.3).
        pub fn vit_line_balance(line: BudgetLine) -> Balance {
            Self::load().vit_line_balance(line)
        }

        /// The full core aggregate rebuilt from storage (views / tests /
        /// differential). The Params-derived seams are refreshed from live
        /// records; the `events` vec is empty.
        pub fn treasury() -> Treasury {
            Self::load()
        }

        /// Seed arbitrary (bounded) accounting state for tests and benchmarks.
        /// Not compiled into production runtimes.
        #[cfg(any(test, feature = "runtime-benchmarks"))]
        pub(crate) fn seed(t: &Treasury) {
            State::<T>::put(TreasuryState::truncating_from_core(t));
        }

        // ---- core <-> storage plumbing -------------------------------------

        /// Rebuild the core aggregate from storage and overlay the live 13 §1
        /// tunables from `Params` (rule 4). The single seam where the pallet
        /// reads parameters — never a hardcode.
        fn load() -> Treasury {
            let s = State::<T>::get();
            let mut t = Treasury {
                main_usdc: s.main_usdc,
                vit_supply: s.vit_supply,
                reserve_impaired: s.reserve_impaired,
                lines: s.lines.into_inner(),
                streams: s.streams.into_inner(),
                pending_outflows: s.pending_outflows.into_inner(),
                pol_commitments: s.pol_commitments.into_inner(),
                meter_30d: s.meter_30d,
                meter_180d: s.meter_180d,
                issuance: s.issuance,
                events: Vec::new(),
                next_stream_id: s.next_stream_id,
                vit_lines: s.vit_lines.into_inner(),
                funded_coretime_periods: s.funded_coretime_periods.into_inner(),
                coretime_quotes: s.coretime_quotes.into_inner(),
                cap_proposal_bps: T::Params::cap_proposal_bps(),
                stream_threshold_bps: T::Params::stream_threshold_bps(),
                keeper_meter: s.keeper_meter,
            };
            // Refresh the metered caps from live Params (rule 4), including the
            // rolling issuance cap `iss.inflation`.
            t.meter_30d.limit_bps = T::Params::cap_30d_bps();
            t.meter_180d.limit_bps = T::Params::cap_180d_bps();
            t.issuance.limit_bps = T::Params::inflation_cap_bps();
            t
        }

        /// Load → run the core transition → (on `Ok`) persist and deposit its
        /// events; on `Err`, return the mapped error with storage untouched
        /// (G-1 status-quo default — nothing was written).
        fn mutate<R>(
            op: impl FnOnce(&mut Treasury) -> Result<R, CoreError>,
        ) -> Result<R, DispatchError> {
            let mut t = Self::load();
            let result = op(&mut t).map_err(Self::map_core_error)?;
            Self::persist(t)?;
            Ok(result)
        }

        /// Convert the mutated aggregate back to bounded storage and deposit its
        /// events. The bounded conversion is the last fallible step before the
        /// single write, so an (unreachable, core enforces the same bounds)
        /// over-bound state rejects the whole dispatch with nothing persisted.
        fn persist(t: Treasury) -> DispatchResult {
            let state = Self::checked_state(&t)?;
            State::<T>::put(state);
            for ev in t.events {
                Self::deposit_core_event(ev);
            }
            Ok(())
        }

        /// Fallible mirror conversion (rejects over-bound; G-1). Distinct from
        /// [`TreasuryState::truncating_from_core`], which is used only for the
        /// empty default and try_state-guarded genesis.
        fn checked_state(t: &Treasury) -> Result<TreasuryState, DispatchError> {
            Ok(TreasuryState {
                main_usdc: t.main_usdc,
                vit_supply: t.vit_supply,
                reserve_impaired: t.reserve_impaired,
                lines: BoundedVec::try_from(t.lines.clone())
                    .map_err(|_| Error::<T>::TooManyBudgetLines)?,
                streams: BoundedVec::try_from(t.streams.clone())
                    .map_err(|_| Error::<T>::TooManyStreams)?,
                pending_outflows: BoundedVec::try_from(t.pending_outflows.clone())
                    .map_err(|_| Error::<T>::TooManyObligations)?,
                pol_commitments: BoundedVec::try_from(t.pol_commitments.clone())
                    .map_err(|_| Error::<T>::TooManyObligations)?,
                meter_30d: t.meter_30d,
                meter_180d: t.meter_180d,
                issuance: t.issuance,
                next_stream_id: t.next_stream_id,
                vit_lines: BoundedVec::try_from(t.vit_lines.clone())
                    .map_err(|_| Error::<T>::TooManyBudgetLines)?,
                funded_coretime_periods: BoundedVec::try_from(t.funded_coretime_periods.clone())
                    .map_err(|_| Error::<T>::TooManyObligations)?,
                coretime_quotes: BoundedVec::try_from(t.coretime_quotes.clone())
                    .map_err(|_| Error::<T>::TooManyObligations)?,
                keeper_meter: t.keeper_meter,
            })
        }

        fn to_core_account(a: T::AccountId) -> futarchy_primitives::AccountId {
            a.into()
        }

        fn from_core_account(a: futarchy_primitives::AccountId) -> T::AccountId {
            a.into()
        }

        fn now() -> BlockNumber {
            frame_system::Pallet::<T>::block_number().saturated_into::<BlockNumber>()
        }

        fn deposit_core_event(ev: CoreEvent) {
            let fe = match ev {
                CoreEvent::Spent { line, dest, amount } => Event::Spent {
                    line,
                    dest: Self::from_core_account(dest),
                    amount,
                },
                CoreEvent::StreamOpened {
                    id,
                    recipient,
                    total,
                } => Event::StreamOpened {
                    id,
                    recipient: Self::from_core_account(recipient),
                    total,
                },
                CoreEvent::StreamClaimed {
                    id,
                    recipient,
                    amount,
                } => Event::StreamClaimed {
                    id,
                    recipient: Self::from_core_account(recipient),
                    amount,
                },
                CoreEvent::StreamCancelled { id, reverted } => {
                    Event::StreamCancelled { id, reverted }
                }
                CoreEvent::BudgetLineFunded { line, amount } => {
                    Event::BudgetLineFunded { line, amount }
                }
                CoreEvent::VitIssued {
                    amount,
                    line,
                    meter_after,
                } => Event::VitIssued {
                    amount,
                    line,
                    meter_after,
                },
                CoreEvent::NavHaircutFlagged { epoch, flag } => {
                    Event::NavHaircutFlagged { epoch, flag }
                }
                CoreEvent::ForeignRecovered {
                    asset,
                    dest,
                    amount,
                } => Event::ForeignRecovered {
                    asset,
                    dest: Self::from_core_account(dest),
                    amount,
                },
                CoreEvent::CoretimeRenewalCalled { line, amount } => {
                    Event::CoretimeRenewalCalled { line, amount }
                }
                CoreEvent::NavFloorUnmet { class, nav, floor } => {
                    Event::NavFloorUnmet { class, nav, floor }
                }
                CoreEvent::KeeperBudgetLow { remaining } => Event::KeeperBudgetLow { remaining },
                CoreEvent::KeeperBudgetExhausted { epoch, spent } => {
                    Event::KeeperBudgetExhausted { epoch, spent }
                }
            };
            Self::deposit_event(fe);
        }

        /// Rebuild the aggregate and run the core validator plus the FRAME-side
        /// solvency identities (15 §1). Public so genesis and tests can call it.
        pub fn do_try_state() -> Result<(), TryRuntimeError> {
            let t = Self::load();
            t.try_state().map_err(|_| {
                TryRuntimeError::Other("treasury core try_state failed (I-7 / solvency bounds)")
            })?;
            // FRAME-side solvency identity: minted VIT credited to lines never
            // exceeds total supply (marked 0 in NAV, but must stay backed).
            let vit_in_lines = t
                .vit_lines
                .iter()
                .map(|(_, b)| *b)
                .fold(0u128, u128::saturating_add);
            if vit_in_lines > t.vit_supply {
                return Err(TryRuntimeError::Other(
                    "treasury: minted VIT credited to lines exceeds total supply",
                ));
            }
            // Loud custody-drift alarm: accounting a funded payout line does
            // not itself transfer USDC into its real custody pot. Until the
            // custody-sync follow-up lands, operators must fund both legs.
            if t.line_balance(BudgetLine::Keeper) > T::RebatePayout::pot_balance(PayoutLine::Keeper)
            {
                return Err(TryRuntimeError::Other(
                    "treasury: KEEPER line exceeds real USDC custody pot",
                ));
            }
            if t.line_balance(BudgetLine::Oracle) > T::RebatePayout::pot_balance(PayoutLine::Oracle)
            {
                return Err(TryRuntimeError::Other(
                    "treasury: ORACLE line exceeds real USDC custody pot",
                ));
            }
            Ok(())
        }

        pub(crate) fn map_core_error(err: CoreError) -> DispatchError {
            match err {
                CoreError::BadOrigin => DispatchError::BadOrigin,
                CoreError::UnknownBudgetLine => Error::<T>::UnknownBudgetLine.into(),
                CoreError::InsufficientFunds => Error::<T>::InsufficientFunds.into(),
                CoreError::ReserveImpaired => Error::<T>::ReserveImpaired.into(),
                CoreError::ProposalCapExceeded => Error::<T>::ProposalCapExceeded.into(),
                CoreError::StreamRequired => Error::<T>::StreamRequired.into(),
                CoreError::MeterExhausted => Error::<T>::MeterExhausted.into(),
                CoreError::StreamNotFound => Error::<T>::StreamNotFound.into(),
                CoreError::StreamNotClaimable => Error::<T>::StreamNotClaimable.into(),
                CoreError::NotRecipient => Error::<T>::NotRecipient.into(),
                CoreError::AlreadyCancelled => Error::<T>::AlreadyCancelled.into(),
                CoreError::BadDuration => Error::<T>::BadDuration.into(),
                CoreError::RenewalWindowClosed => Error::<T>::RenewalWindowClosed.into(),
                CoreError::PeriodAlreadyFunded => Error::<T>::PeriodAlreadyFunded.into(),
                CoreError::TooManyStreams => Error::<T>::TooManyStreams.into(),
                CoreError::TooManyBudgetLines => Error::<T>::TooManyBudgetLines.into(),
                CoreError::TooManyObligations => Error::<T>::TooManyObligations.into(),
                CoreError::IssuanceLineNotAllowed => Error::<T>::IssuanceLineNotAllowed.into(),
                CoreError::IssuanceCapExceeded => Error::<T>::IssuanceCapExceeded.into(),
                CoreError::UnknownForeignAsset => Error::<T>::UnknownForeignAsset.into(),
                CoreError::NavFloorUnmet => Error::<T>::NavFloorUnmet.into(),
                CoreError::ZeroQuote => Error::<T>::ZeroQuote.into(),
                CoreError::Overflow => Error::<T>::Overflow.into(),
            }
        }
    }

    impl<T: Config> KeeperRebateSink<T::AccountId> for Pallet<T> {
        fn rebate(who: &T::AccountId, class: CrankClass) {
            Self::do_keeper_rebate(who, class);
        }
    }
}
