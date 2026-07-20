#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! # `pallet-market`
//!
//! Production FRAME wrapper over the frame-free [`market_core`] LMSR engine.
//! The core remains the differential oracle; this pallet owns the frozen runtime
//! storage/events/calls, origin checks, the real conditional-ledger adapter, and
//! mandatory try-state validation (02 §5/§7.4, 04, 15 §1).

extern crate alloc;

pub use market_core as core_market;
pub use pallet::*;
pub use weights::WeightInfo;

pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

/// Runtime fixture hooks for rebate-bearing market benchmarks. Mock runtimes
/// may keep the defaults; the assembled runtime primes the treasury payout.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper {
    fn prime_keeper_rebate() {}
    fn assert_keeper_rebate_paid(_: futarchy_primitives::keeper::CrankClass) {}
}

#[cfg(feature = "runtime-benchmarks")]
impl BenchmarkHelper for () {}

/// Raw per-check facts behind the boolean decision grade
/// (`Pallet::decision_grade_at`). The runtime adapter partitions them into
/// the 05 §5.2 tri-state welfare-book grade: the remediable-by-time
/// shortfalls (`!contest_ok`, `!coverage_ok`, `stale_events == 1`) grade
/// Insufficient; every other failure grades Invalid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecisionGradeFacts {
    /// TWAP inside the [0.02, 0.98] sanity band (or the band not required).
    pub sane: bool,
    /// The exact decision window record is sealed (I-16 frozen boundary).
    pub sealed: bool,
    /// 04 §7 stale observation events inside the window.
    pub stale_events: u8,
    /// Scheduled-interval coverage at or above the required percentage.
    pub coverage_ok: bool,
    /// Seeded POL present with `b` at or above the class floor (POL floor
    /// met and undisturbed).
    pub pol_ok: bool,
    /// Contest-capital accounting intact (cleared on accumulator overflow;
    /// an invalid window never grades — G-1).
    pub contest_valid: bool,
    /// Time-averaged 04 §7a contest capital at or above the class floor.
    pub contest_ok: bool,
    /// `|close spot − TWAP|` within the convergence bound.
    pub converged: bool,
}

/// Runtime treasury mirror for 08 §1.2 live-book POL obligations. The market
/// pallet owns lifecycle timing but remains treasury-free; production binds the
/// aggregate scan at the runtime boundary. A failed sync is returned to the
/// enclosing storage transaction so a lifecycle mutation can never outlive its
/// matching solvency obligation.
pub trait PolCommitmentSync {
    fn sync_pol_commitments() -> frame_support::dispatch::DispatchResult;
    fn pol_commitments_synced() -> bool;
}

impl PolCommitmentSync for () {
    fn sync_pol_commitments() -> frame_support::dispatch::DispatchResult {
        Ok(())
    }

    fn pol_commitments_synced() -> bool {
        true
    }
}

#[frame_support::pallet]
pub mod pallet {
    use crate::weights::WeightInfo;
    use crate::DecisionGradeFacts;
    use crate::PolCommitmentSync;
    use alloc::vec::Vec;
    use core::marker::PhantomData;
    use frame_support::{pallet_prelude::*, traits::Contains, PalletId};
    use frame_system::pallet_prelude::*;
    use futarchy_primitives::{
        bounds,
        keeper::{CrankClass, KeeperRebateSink},
        kernel, Balance, BlockNumber, Branch, EpochId, FixedU64, GateType, MarketId, MarketKind,
        PositionId, ProposalId, ScalarSide, TradeSide,
    };
    use market_core::{
        BookKind, MarketBook, MarketParams, MarketPhase, TwapCumulative, TwapWindow,
    };
    use sp_runtime::{
        traits::{AccountIdConversion, CheckedAdd, Saturating, UniqueSaturatedInto},
        DispatchError,
    };

    #[pallet::config]
    pub trait Config:
        frame_system::Config<RuntimeEvent: From<Event<Self>>> + pallet_conditional_ledger::Config
    {
        /// Benchmarked weights for all public calls and internal admin operations.
        type WeightInfo: WeightInfo;

        /// `mkt.fee`, in basis points (13 §1).
        #[pallet::constant]
        type Fee: Get<u128>;

        /// `mkt.obs_interval`, in blocks (13 §1).
        #[pallet::constant]
        type ObsInterval: Get<u64>;

        /// `mkt.kappa`, represented on the 1e9 fixed grid (13 §1).
        #[pallet::constant]
        type Kappa1e9: Get<u64>;

        /// Internal `pallet-epoch` authority for create/seed/close (06 §3.2).
        type MarketAdmin: EnsureOrigin<Self::RuntimeOrigin>;

        /// Kernel-enumerated playbook effect origin (06 §6.2/§6.3).
        type EmergencyPlaybookOrigin: EnsureOrigin<Self::RuntimeOrigin>;

        /// Delay from close until permissionless reaping (04 §2).
        #[pallet::constant]
        type ArchiveDelay: Get<BlockNumberFor<Self>>;

        /// Market sovereign account; also the ledger's configured MarketAuthority.
        #[pallet::constant]
        type PalletId: Get<PalletId>;

        /// Fail-soft keeper rebate endpoint (08 §6.3).
        type KeeperRebate: KeeperRebateSink<Self::AccountId>;

        /// Classifies observations made inside a proposal decision window.
        type InDecisionWindow: frame_support::traits::Contains<MarketId>;

        /// Transactional treasury obligation mirror. A lifecycle transition is
        /// rolled back if its exact NAV obligation cannot be mirrored.
        type PolCommitmentSync: crate::PolCommitmentSync;

        /// Cross-pallet keeper-rebate fixture used only by runtime benchmarks.
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: crate::BenchmarkHelper;
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    /// Live market books (02 §7.4). A `CountedStorageMap` so `create_market` can
    /// enforce the `kernel::MAX_LIVE_MARKETS = 196` bound in O(1) at dispatch time
    /// (I-21), not just in `try_state`; each value is statically `MaxEncodedLen`
    /// bounded. The map key/value shape the frontend reads is unchanged.
    #[pallet::storage]
    pub type Markets<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, MarketId, MarketBook<T::AccountId>, OptionQuery>;

    /// O(1) membership index for dynamically allocated book and fee custody
    /// accounts. Refcounts make the index correct even when a runtime or test
    /// deliberately reuses an account across books; the live entry count is
    /// dispatch-bounded by `2 * MaxLiveMarkets`.
    #[pallet::storage]
    pub type MarketProtocolAccounts<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, T::AccountId, u16, OptionQuery>;

    /// Proposal-to-book inverse used to observe one ledger terminal marker in
    /// O(BooksPerProposal), rather than scanning all live markets.
    #[pallet::storage]
    pub type ProposalMarketIds<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        ProposalId,
        BoundedVec<MarketId, ConstU32<{ bounds::BOOKS_PER_PROPOSAL }>>,
        ValueQuery,
    >;

    /// Epoch-to-Baseline-book lookup (02 §7.4, frozen name).
    #[pallet::storage]
    pub type BaselineMarketOf<T: Config> =
        StorageMap<_, Blake2_128Concat, EpochId, MarketId, OptionQuery>;

    /// Block at which a book closed, retained for the frozen integration
    /// surface and lifecycle observability. Reap delay is settlement-anchored.
    #[pallet::storage]
    pub type ClosedAt<T: Config> =
        StorageMap<_, Blake2_128Concat, MarketId, BlockNumberFor<T>, OptionQuery>;

    /// Markets whose POL headroom has already been seeded (04 §10). Guards `seed`
    /// against re-splitting POL into an already-collateralized book (idempotence);
    /// removed at reap.
    #[pallet::storage]
    pub type SeededMarkets<T: Config> = StorageMap<_, Blake2_128Concat, MarketId, (), OptionQuery>;

    /// Monotonic internal id allocator used by epoch's bounded market-opening
    /// orchestration. Zero means no id has yet been allocated.
    #[pallet::storage]
    pub type NextMarketId<T: Config> = StorageValue<_, MarketId, ValueQuery>;

    /// O(1) accumulator checkpoints at registered full/trailing boundaries
    /// (04 §7). Internal backing outside the frozen 02 §7.4 surface.
    #[pallet::storage]
    pub type TwapCheckpoints<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        MarketId,
        BoundedVec<
            (BlockNumber, TwapCumulative),
            ConstU32<{ bounds::MAX_TWAP_WINDOWS_PER_MARKET }>,
        >,
        ValueQuery,
    >;

    /// Per-window coverage and staleness counters. A Baseline can serve
    /// several proposal pairs, hence the same eight-entry bound as checkpoints.
    #[pallet::storage]
    pub type DecisionWindows<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        MarketId,
        BoundedVec<TwapWindow, ConstU32<{ bounds::MAX_TWAP_WINDOWS_PER_MARKET }>>,
        ValueQuery,
    >;

    /// Logical proposal consumers of registered windows. Baseline windows may
    /// be shared by several proposals with identical boundaries; a sealed
    /// window is prunable only after every listed decision has consumed it.
    #[pallet::storage]
    pub type DecisionWindowOwners<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        MarketId,
        BoundedVec<
            (ProposalId, BlockNumber, BlockNumber, BlockNumber),
            ConstU32<{ bounds::MAX_LIVE_PROPOSALS * bounds::MAX_TWAP_WINDOWS_PER_MARKET }>,
        >,
        ValueQuery,
    >;

    /// Idempotence marker for the one extra seed that brings a guardian rerun
    /// from its original POL allocation to the specified 2× allocation.
    #[pallet::storage]
    pub type RerunSeededMarkets<T: Config> =
        StorageMap<_, Blake2_128Concat, MarketId, (), OptionQuery>;

    /// Durable market-side observation of the ledger terminal block. Unlike
    /// the ledger's permissionlessly swept marker, this latch lives until the
    /// corresponding market is reaped and therefore cannot resurrect POL.
    #[pallet::storage]
    pub type SettlementObservedAt<T: Config> =
        StorageMap<_, Blake2_128Concat, MarketId, BlockNumberFor<T>, OptionQuery>;

    /// Exact live POL obligations, sorted by market id. Lifecycle mutations
    /// update this one bounded value transactionally, so the treasury mirror
    /// needs one storage read instead of a 196-key `Markets` trie scan.
    #[pallet::storage]
    pub type LivePolCommitments<T: Config> = StorageValue<
        _,
        BoundedVec<(MarketId, Balance), ConstU32<{ bounds::MAX_LIVE_MARKETS }>>,
        ValueQuery,
    >;

    /// PB-DEPEG backstop: new book creation/seeding is disabled only while
    /// `now < until` (06 §6.2).
    #[pallet::storage]
    pub type CreationFrozenUntil<T: Config> = StorageValue<_, BlockNumberFor<T>, OptionQuery>;

    /// PB-LEDGER-FREEZE backstop for trading/observation calls (06 §6.3).
    #[pallet::storage]
    pub type FrozenUntil<T: Config> = StorageValue<_, BlockNumberFor<T>, OptionQuery>;

    /// Pallet-level one-renewal latch. Guardian core independently enforces the
    /// same invariant; this prevents a miswired runtime adapter extending twice.
    #[pallet::storage]
    pub type FreezeRenewed<T: Config> = StorageValue<_, bool, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// Frozen 02 §5 trade event.
        Traded {
            market: MarketId,
            who: T::AccountId,
            side: TradeSide,
            amount: Balance,
            cost: Balance,
            p_after: FixedU64,
        },
        /// Frozen 02 §5 observation event.
        Observed { market: MarketId, o_t: FixedU64 },
        /// Frozen 02 §5 creation event.
        MarketCreated {
            market: MarketId,
            kind: MarketKind,
            pid: Option<ProposalId>,
            epoch: EpochId,
            b: Balance,
        },
        /// Frozen 02 §5 close event.
        MarketClosed { market: MarketId },
        /// Frozen 02 §5 reap event.
        MarketReaped { market: MarketId },
        /// Append-only operational event; not part of the frozen §5 ingest set.
        Seeded { market: MarketId, headroom: Balance },
        /// Operational event outside the frozen 02 ingest schema.
        CreationFreezeSet { until: BlockNumberFor<T> },
        /// Operational event outside the frozen 02 ingest schema.
        CreationFreezeCleared,
        /// Operational event outside the frozen 02 ingest schema.
        FreezeSet { until: BlockNumberFor<T> },
        /// Operational event outside the frozen 02 ingest schema.
        FreezeCleared,
        /// Operational event outside the frozen 02 ingest schema.
        FreezeExtended { until: BlockNumberFor<T> },
    }

    #[pallet::error]
    pub enum Error<T> {
        UnknownMarket,
        DuplicateMarket,
        DuplicateBaselineMarket,
        NotTrading,
        AmountTooSmall,
        AmountTooLarge,
        SlippageExceeded,
        PriceBoundExceeded,
        ArithmeticOverflow,
        Ledger,
        TryStateViolation,
        BadOrigin,
        NotReapable,
        /// Creating this book would exceed `MaxLiveMarkets = 196` (I-21).
        TooManyMarkets,
        /// The book's POL headroom has already been seeded (04 §10, idempotence).
        AlreadySeeded,
        /// PB-DEPEG blocks book creation/seeding until its bounded expiry.
        CreationFrozen,
        /// PB-LEDGER-FREEZE blocks trading/observation until its bounded expiry.
        Frozen,
        /// The requested expiry is in the past or beyond its kernel bound.
        FreezeOutOfBounds,
        /// The one pallet-level LedgerFreeze renewal was already consumed.
        FreezeRenewalExhausted,
    }

    impl<T: Config> From<market_core::Error> for Error<T> {
        fn from(error: market_core::Error) -> Self {
            use market_core::Error as Core;
            match error {
                Core::UnknownMarket => Self::UnknownMarket,
                Core::DuplicateMarket => Self::DuplicateMarket,
                Core::DuplicateBaselineMarket => Self::DuplicateBaselineMarket,
                Core::BadOrigin => Self::BadOrigin,
                Core::NotTrading => Self::NotTrading,
                Core::AmountTooSmall => Self::AmountTooSmall,
                Core::AmountTooLarge => Self::AmountTooLarge,
                Core::SlippageExceeded => Self::SlippageExceeded,
                Core::PriceBoundExceeded => Self::PriceBoundExceeded,
                Core::ArithmeticOverflow => Self::ArithmeticOverflow,
                Core::Ledger => Self::Ledger,
                Core::TryStateViolation => Self::TryStateViolation,
            }
        }
    }

    /// Zero-sized production adapter from the core wrapper to the real ledger pallet.
    pub struct PalletLedger<T>(PhantomData<T>);

    impl<T: Config> PalletLedger<T> {
        fn new() -> Self {
            Self(PhantomData)
        }

        fn authority_origin() -> OriginFor<T> {
            frame_system::RawOrigin::Signed(Pallet::<T>::account_id()).into()
        }
    }

    impl<T: Config> market_core::LedgerOps<T::AccountId> for PalletLedger<T> {
        fn do_split(
            &mut self,
            pid: ProposalId,
            who: &T::AccountId,
            amount: Balance,
        ) -> Result<(), ()> {
            pallet_conditional_ledger::Pallet::<T>::do_split(
                Self::authority_origin(),
                pid,
                who.clone(),
                amount,
            )
            .map_err(|_| ())
        }

        fn do_transfer(
            &mut self,
            id: PositionId,
            from: &T::AccountId,
            to: &T::AccountId,
            amount: Balance,
        ) -> Result<(), ()> {
            pallet_conditional_ledger::Pallet::<T>::do_transfer(
                Self::authority_origin(),
                id,
                from.clone(),
                to.clone(),
                amount,
            )
            .map_err(|_| ())
        }

        fn do_split_scalar(
            &mut self,
            pid: ProposalId,
            branch: Branch,
            who: &T::AccountId,
            amount: Balance,
        ) -> Result<(), ()> {
            pallet_conditional_ledger::Pallet::<T>::do_split_scalar(
                Self::authority_origin(),
                pid,
                branch,
                who.clone(),
                amount,
            )
            .map_err(|_| ())
        }

        fn do_split_gate(
            &mut self,
            pid: ProposalId,
            branch: Branch,
            gate: GateType,
            who: &T::AccountId,
            amount: Balance,
        ) -> Result<(), ()> {
            pallet_conditional_ledger::Pallet::<T>::do_split_gate(
                Self::authority_origin(),
                pid,
                branch,
                gate,
                who.clone(),
                amount,
            )
            .map_err(|_| ())
        }

        fn do_split_baseline(
            &mut self,
            epoch: EpochId,
            who: &T::AccountId,
            amount: Balance,
        ) -> Result<(), ()> {
            pallet_conditional_ledger::Pallet::<T>::do_split_baseline(
                Self::authority_origin(),
                epoch,
                who.clone(),
                amount,
            )
            .map_err(|_| ())
        }

        fn do_merge(
            &mut self,
            pid: ProposalId,
            who: &T::AccountId,
            amount: Balance,
        ) -> Result<(), ()> {
            pallet_conditional_ledger::Pallet::<T>::do_merge(
                Self::authority_origin(),
                pid,
                who.clone(),
                amount,
            )
            .map_err(|_| ())
        }

        fn do_merge_scalar(
            &mut self,
            pid: ProposalId,
            branch: Branch,
            who: &T::AccountId,
            amount: Balance,
        ) -> Result<(), ()> {
            pallet_conditional_ledger::Pallet::<T>::do_merge_scalar(
                Self::authority_origin(),
                pid,
                branch,
                who.clone(),
                amount,
            )
            .map_err(|_| ())
        }

        fn do_merge_gate(
            &mut self,
            pid: ProposalId,
            branch: Branch,
            gate: GateType,
            who: &T::AccountId,
            amount: Balance,
        ) -> Result<(), ()> {
            pallet_conditional_ledger::Pallet::<T>::do_merge_gate(
                Self::authority_origin(),
                pid,
                branch,
                gate,
                who.clone(),
                amount,
            )
            .map_err(|_| ())
        }

        fn do_merge_baseline(
            &mut self,
            epoch: EpochId,
            who: &T::AccountId,
            amount: Balance,
        ) -> Result<(), ()> {
            pallet_conditional_ledger::Pallet::<T>::do_merge_baseline(
                Self::authority_origin(),
                epoch,
                who.clone(),
                amount,
            )
            .map_err(|_| ())
        }

        fn note_protocol_account(&mut self, _who: T::AccountId) {
            // Protocol-account status is statically owned by the ledger Config.
        }

        fn position_balance(&self, id: PositionId, who: &T::AccountId) -> Balance {
            pallet_conditional_ledger::Positions::<T>::get(id, who)
        }
    }

    #[pallet::extra_constants]
    impl<T: Config> Pallet<T> {
        #[pallet::constant_name(MinTrade)]
        fn min_trade() -> Balance {
            kernel::MIN_TRADE_USDC
        }

        #[pallet::constant_name(MaxTradeRatio)]
        fn max_trade_ratio() -> (u32, u32) {
            kernel::MAX_TRADE_RATIO
        }

        #[pallet::constant_name(MaxLiveMarkets)]
        fn max_live_markets() -> u32 {
            bounds::MAX_LIVE_MARKETS
        }

        #[pallet::constant_name(GatePMaxCeiling)]
        fn gate_p_max_ceiling() -> FixedU64 {
            FixedU64(kernel::GATE_P_MAX_CEILING_1E9)
        }

        #[pallet::constant_name(GateEpsFloor)]
        fn gate_eps_floor() -> FixedU64 {
            kernel::GATE_EPS_FLOOR
        }
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// No block hooks: observations are keeper-cranked (04 §7).
        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), sp_runtime::TryRuntimeError> {
            Self::do_try_state()
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Buy LONG or SHORT from an LMSR book (04 §6).
        #[pallet::call_index(0)]
        #[pallet::weight(<T as Config>::WeightInfo::buy())]
        pub fn buy(
            origin: OriginFor<T>,
            market: MarketId,
            side: ScalarSide,
            amount: Balance,
            max_cost: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::ensure_not_frozen()?;
            let mut book = Markets::<T>::get(market).ok_or(Error::<T>::UnknownMarket)?;
            Self::ensure_trade_admissible(market, &book)?;
            let before = book.clone();
            Self::seal_due_windows(market, &before, Self::now_u64(), false)?;
            Self::accrue_contest(market, &before, Self::now_u64());
            let mut ledger = PalletLedger::<T>::new();
            let events = market_core::buy_book(
                &mut book,
                &mut ledger,
                &Self::params(),
                &who,
                side,
                amount,
                max_cost,
                Self::now_u64(),
            )
            .map_err(Error::<T>::from)?;
            Self::record_observation(market, &before, &book);
            Markets::<T>::insert(market, book);
            for event in events {
                Self::deposit_trade_event(event)?;
            }
            Ok(())
        }

        /// Sell LONG or SHORT into an LMSR book (04 §6).
        #[pallet::call_index(1)]
        #[pallet::weight(<T as Config>::WeightInfo::sell())]
        pub fn sell(
            origin: OriginFor<T>,
            market: MarketId,
            side: ScalarSide,
            amount: Balance,
            min_proceeds: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::ensure_not_frozen()?;
            let mut book = Markets::<T>::get(market).ok_or(Error::<T>::UnknownMarket)?;
            Self::ensure_trade_admissible(market, &book)?;
            let before = book.clone();
            Self::seal_due_windows(market, &before, Self::now_u64(), false)?;
            Self::accrue_contest(market, &before, Self::now_u64());
            let mut ledger = PalletLedger::<T>::new();
            let events = market_core::sell_book(
                &mut book,
                &mut ledger,
                &Self::params(),
                &who,
                side,
                amount,
                min_proceeds,
                Self::now_u64(),
            )
            .map_err(Error::<T>::from)?;
            Self::record_observation(market, &before, &book);
            Markets::<T>::insert(market, book);
            for event in events {
                Self::deposit_trade_event(event)?;
            }
            Ok(())
        }

        /// Permissionless TWAP observation keeper (04 §7).
        #[pallet::call_index(2)]
        // B5: recalibrate for the keeper-rebate sink's additional storage path.
        #[pallet::weight(<T as Config>::WeightInfo::crank_observe())]
        pub fn crank_observe(origin: OriginFor<T>, market: MarketId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::ensure_not_frozen()?;
            let mut book = Markets::<T>::get(market).ok_or(Error::<T>::UnknownMarket)?;
            // The accumulator is sealed at Close (04 §2): a permissionless keeper must
            // not record observations on a Closed/Settled book (it would mutate the
            // frozen TWAP). The shared trade preflight closes the standalone
            // crank path too.
            Self::ensure_trade_admissible(market, &book)?;
            let before = book.clone();
            Self::seal_due_windows(market, &before, Self::now_u64(), false)?;
            Self::accrue_contest(market, &before, Self::now_u64());
            if let Some(event) =
                market_core::observe_book(&mut book, &Self::params(), Self::now_u64())
                    .map_err(Error::<T>::from)?
            {
                Self::record_observation(market, &before, &book);
                Markets::<T>::insert(market, book);
                Self::deposit_trade_event(event)?;
                let class = if T::InDecisionWindow::contains(&market) {
                    CrankClass::DecisionCritical
                } else {
                    CrankClass::General
                };
                <T as Config>::KeeperRebate::rebate(&who, class);
            }
            Ok(())
        }

        /// Permissionlessly reap a closed book after `ArchiveDelay` (04 §2).
        #[pallet::call_index(3)]
        // B5: recalibrate for the keeper-rebate sink's additional storage path.
        #[pallet::weight(<T as Config>::WeightInfo::reap())]
        pub fn reap(origin: OriginFor<T>, market: MarketId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let book = Markets::<T>::get(market).ok_or(Error::<T>::UnknownMarket)?;
            ensure!(
                matches!(book.phase, MarketPhase::Closed),
                Error::<T>::NotReapable
            );
            let terminal = SettlementObservedAt::<T>::get(market).ok_or(Error::<T>::NotReapable)?;
            ensure!(
                frame_system::Pallet::<T>::block_number()
                    >= terminal.saturating_add(<T as Config>::ArchiveDelay::get()),
                Error::<T>::NotReapable
            );
            // Closing freezes prices, but the headroom remains an obligation until
            // the corresponding ledger vault settles. The durable market-side latch
            // is the archive-delay anchor because the ledger marker may already have
            // been permissionlessly swept.
            ensure!(
                !Self::pol_obligation_live(market, &book),
                Error::<T>::NotReapable
            );
            frame_support::storage::with_storage_layer(|| -> DispatchResult {
                if let BookKind::Baseline { epoch } = book.kind {
                    if BaselineMarketOf::<T>::get(epoch) == Some(market) {
                        BaselineMarketOf::<T>::remove(epoch);
                    }
                }
                Markets::<T>::remove(market);
                ClosedAt::<T>::remove(market);
                SeededMarkets::<T>::remove(market);
                RerunSeededMarkets::<T>::remove(market);
                SettlementObservedAt::<T>::remove(market);
                Self::remove_pol_commitment(market);
                Self::unregister_market_accounts(&book)?;
                if let BookKind::Decision { proposal, .. } | BookKind::Gate { proposal, .. } =
                    book.kind
                {
                    ProposalMarketIds::<T>::mutate(proposal, |ids| {
                        ids.retain(|id| *id != market);
                    });
                    if ProposalMarketIds::<T>::get(proposal).is_empty() {
                        ProposalMarketIds::<T>::remove(proposal);
                    }
                }
                TwapCheckpoints::<T>::remove(market);
                DecisionWindows::<T>::remove(market);
                DecisionWindowOwners::<T>::remove(market);
                T::PolCommitmentSync::sync_pol_commitments()?;
                Self::deposit_event(Event::MarketReaped { market });
                Ok(())
            })?;
            <T as Config>::KeeperRebate::rebate(&who, CrankClass::General);
            Ok(())
        }

        /// PB-DEPEG effect endpoint: freeze only new market creation/seeding.
        #[pallet::call_index(4)]
        #[pallet::weight(<T as Config>::WeightInfo::freeze_creation())]
        pub fn freeze_creation(origin: OriginFor<T>, expiry: BlockNumberFor<T>) -> DispatchResult {
            <T as Config>::EmergencyPlaybookOrigin::ensure_origin(origin)?;
            let now = frame_system::Pallet::<T>::block_number();
            let max: BlockNumberFor<T> = kernel::MIN_EPOCH_LENGTH_BLOCKS.into();
            ensure!(
                expiry >= now && expiry.saturating_sub(now) <= max,
                Error::<T>::FreezeOutOfBounds
            );
            CreationFrozenUntil::<T>::put(expiry);
            Self::deposit_event(Event::CreationFreezeSet { until: expiry });
            Ok(())
        }

        /// PB-LEDGER-FREEZE effect endpoint. `true` installs exactly the
        /// kernel 14-day backstop; `false` clears early/reverts expiry.
        #[pallet::call_index(5)]
        #[pallet::weight(<T as Config>::WeightInfo::set_frozen())]
        pub fn set_frozen(origin: OriginFor<T>, frozen: bool) -> DispatchResult {
            <T as Config>::EmergencyPlaybookOrigin::ensure_origin(origin)?;
            if frozen {
                let now = frame_system::Pallet::<T>::block_number();
                ensure!(
                    FrozenUntil::<T>::get().is_none_or(|until| now >= until),
                    Error::<T>::Frozen
                );
                let until = now
                    .checked_add(&kernel::PLAYBOOK_FREEZE_WINDOW_BLOCKS.into())
                    .ok_or(Error::<T>::ArithmeticOverflow)?;
                FrozenUntil::<T>::put(until);
                FreezeRenewed::<T>::put(false);
                Self::deposit_event(Event::FreezeSet { until });
            } else {
                FrozenUntil::<T>::kill();
                FreezeRenewed::<T>::put(false);
                Self::deposit_event(Event::FreezeCleared);
            }
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Allocate one monotonic market id. Epoch calls this a bounded number
        /// of times (at most six proposal books plus one Baseline).
        pub fn allocate_market_id(origin: OriginFor<T>) -> Result<MarketId, DispatchError> {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            NextMarketId::<T>::try_mutate(|next| {
                let id = (*next).max(1);
                ensure!(!Markets::<T>::contains_key(id), Error::<T>::DuplicateMarket);
                *next = id.checked_add(1).ok_or(Error::<T>::ArithmeticOverflow)?;
                Ok(id)
            })
        }

        /// O(1) lookup consumed by the conditional ledger's deposit exemption.
        pub fn is_market_protocol_account(who: &T::AccountId) -> bool {
            MarketProtocolAccounts::<T>::contains_key(who)
        }

        /// Return the exact, sorted commitment vector mirrored into treasury.
        pub fn live_pol_commitments() -> Vec<Balance> {
            LivePolCommitments::<T>::get()
                .into_iter()
                .map(|(_, amount)| amount)
                .collect()
        }

        /// Register exact full/trailing TWAP boundaries for one deciding pair.
        /// Duplicate registrations are idempotent; capacity exhaustion rejects
        /// the caller so epoch cannot open an ungradeable book.
        pub fn register_decision_window(
            origin: OriginFor<T>,
            id: MarketId,
            proposal: ProposalId,
            start: BlockNumber,
            trailing_start: BlockNumber,
            end: BlockNumber,
        ) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            let book = Markets::<T>::get(id).ok_or(Error::<T>::UnknownMarket)?;
            ensure!(
                start < trailing_start && trailing_start < end,
                Error::<T>::TryStateViolation
            );
            let candidate = TwapWindow {
                start,
                trailing_start,
                end,
                observations: 0,
                stale_events: 0,
                contest_capital_blocks: 0,
                contest_accrued_until: start,
                contest_valid: true,
                close_spot: None,
                sealed: false,
            };
            let exact_exists = DecisionWindows::<T>::get(id).iter().any(|window| {
                window.start == start
                    && window.trailing_start == trailing_start
                    && window.end == end
            });
            let owner = (proposal, start, trailing_start, end);
            if DecisionWindowOwners::<T>::get(id).contains(&owner) {
                return Ok(());
            }
            frame_support::storage::with_storage_layer(|| -> DispatchResult {
                if !exact_exists {
                    let mut boundaries = DecisionWindows::<T>::get(id)
                        .iter()
                        .flat_map(|window| [window.start, window.trailing_start, window.end])
                        .collect::<Vec<_>>();
                    boundaries.extend([start, trailing_start, end]);
                    boundaries.sort_unstable();
                    boundaries.dedup();
                    let max_boundaries = usize::try_from(bounds::MAX_TWAP_WINDOWS_PER_MARKET)
                        .map_err(|_| Error::<T>::TryStateViolation)?;
                    ensure!(
                        boundaries.len() <= max_boundaries,
                        Error::<T>::TryStateViolation
                    );
                    DecisionWindows::<T>::try_mutate(id, |windows| {
                        windows
                            .try_push(candidate)
                            .map_err(|_| Error::<T>::TryStateViolation)
                    })?;
                    if u64::from(start) == book.last_observed_block {
                        Self::insert_checkpoint(id, start, book.cumulative_price_blocks);
                    }
                }
                DecisionWindowOwners::<T>::try_mutate(id, |owners| {
                    owners
                        .try_push(owner)
                        .map_err(|_| Error::<T>::TryStateViolation)
                })?;
                Ok(())
            })?;
            Ok(())
        }

        /// Shift every still-live logical consumer of one exact window after
        /// a dead-man pause. Replacing the unsealed record atomically avoids a
        /// transient ninth boundary when several proposals share a Baseline.
        pub fn shift_decision_window(
            origin: OriginFor<T>,
            id: MarketId,
            old_end: BlockNumber,
            shift_by: BlockNumber,
        ) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            let new_end = old_end
                .checked_add(shift_by)
                .ok_or(Error::<T>::ArithmeticOverflow)?;
            ensure!(Markets::<T>::contains_key(id), Error::<T>::UnknownMarket);
            frame_support::storage::with_storage_layer(|| -> DispatchResult {
                let mut shifted = None;
                DecisionWindows::<T>::try_mutate(id, |windows| -> DispatchResult {
                    let old = windows.iter_mut().find(|window| window.end == old_end);
                    if let Some(window) = old {
                        ensure!(!window.sealed, Error::<T>::TryStateViolation);
                        let old_boundaries = (window.start, window.trailing_start, window.end);
                        window.end = new_end;
                        shifted = Some((
                            old_boundaries,
                            (old_boundaries.0, old_boundaries.1, new_end),
                        ));
                    }
                    Ok(())
                })?;
                let Some((old_boundaries, new_boundaries)) = shifted else {
                    let already_shifted = DecisionWindows::<T>::get(id)
                        .iter()
                        .any(|window| window.end == new_end);
                    ensure!(already_shifted, Error::<T>::TryStateViolation);
                    return Ok(());
                };
                DecisionWindowOwners::<T>::mutate(id, |owners| {
                    for (_, start, trailing_start, end) in owners.iter_mut().filter(|record| {
                        record.1 == old_boundaries.0
                            && record.2 == old_boundaries.1
                            && record.3 == old_boundaries.2
                    }) {
                        *start = new_boundaries.0;
                        *trailing_start = new_boundaries.1;
                        *end = new_boundaries.2;
                    }
                });
                Self::prune_unreferenced_checkpoints(id);
                Ok(())
            })
        }

        /// Record one terminal decision's consumption of every TWAP window it
        /// used, including an earlier sealed extension attempt. A window and
        /// its now-unreferenced boundaries are evicted only after the last
        /// Baseline-sharing proposal has consumed them.
        pub fn consume_decision_windows(
            origin: OriginFor<T>,
            id: MarketId,
            proposal: ProposalId,
        ) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            frame_support::storage::with_storage_layer(|| -> DispatchResult {
                let owned = DecisionWindowOwners::<T>::get(id)
                    .iter()
                    .filter(|record| record.0 == proposal)
                    .copied()
                    .collect::<Vec<_>>();
                ensure!(!owned.is_empty(), Error::<T>::TryStateViolation);
                let windows = DecisionWindows::<T>::get(id);
                ensure!(
                    owned.iter().all(|record| windows.iter().any(|window| {
                        window.start == record.1
                            && window.trailing_start == record.2
                            && window.end == record.3
                            && window.sealed
                    })),
                    Error::<T>::TryStateViolation
                );
                DecisionWindowOwners::<T>::try_mutate(id, |owners| -> DispatchResult {
                    let before = owners.len();
                    owners.retain(|record| record.0 != proposal);
                    ensure!(owners.len() < before, Error::<T>::TryStateViolation);
                    Ok(())
                })?;
                let remaining = DecisionWindowOwners::<T>::get(id);
                DecisionWindows::<T>::mutate(id, |windows| {
                    windows.retain(|window| {
                        remaining.iter().any(|record| {
                            record.1 == window.start
                                && record.2 == window.trailing_start
                                && record.3 == window.end
                        })
                    });
                });
                Self::prune_unreferenced_checkpoints(id);
                Ok(())
            })
        }

        fn prune_unreferenced_checkpoints(id: MarketId) {
            let windows = DecisionWindows::<T>::get(id);
            TwapCheckpoints::<T>::mutate(id, |checkpoints| {
                checkpoints.retain(|(block, _)| {
                    windows.iter().any(|window| {
                        [window.start, window.trailing_start, window.end].contains(block)
                    })
                });
            });
            if TwapCheckpoints::<T>::get(id).is_empty() {
                TwapCheckpoints::<T>::remove(id);
            }
        }

        /// Reopen a guardian-rerun book with positions intact and a fresh TWAP
        /// accumulator (05 T13). Baseline books are never rerun targets.
        pub fn reopen_for_rerun(origin: OriginFor<T>, id: MarketId) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            let now = Self::now_u64();
            frame_support::storage::with_storage_layer(|| -> DispatchResult {
                Markets::<T>::try_mutate(id, |maybe_book| -> DispatchResult {
                    let book = maybe_book.as_mut().ok_or(Error::<T>::UnknownMarket)?;
                    ensure!(
                        !matches!(book.kind, BookKind::Baseline { .. }),
                        Error::<T>::BadOrigin
                    );
                    book.phase = MarketPhase::Extended;
                    book.last_observation_1e9 = book.last_quote_1e9;
                    book.last_observed_block = now;
                    book.cumulative_price_blocks = TwapCumulative::ZERO;
                    book.stale_events = 0;
                    Ok(())
                })?;
                TwapCheckpoints::<T>::remove(id);
                DecisionWindows::<T>::remove(id);
                DecisionWindowOwners::<T>::remove(id);
                RerunSeededMarkets::<T>::remove(id);
                Ok(())
            })
        }

        /// Reopen the shared Baseline when a delayed proposal starts a later
        /// decision window after the original cohort had already closed it.
        /// Baseline is never a guardian-rerun target and receives no rerun POL;
        /// this only restarts its observation accumulator so the fresh window
        /// can be graded (04 §8.4; 05 T13).
        pub fn reopen_baseline_for_rerun(origin: OriginFor<T>, id: MarketId) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            let now = Self::now_u64();
            frame_support::storage::with_storage_layer(|| -> DispatchResult {
                Markets::<T>::try_mutate(id, |maybe_book| -> DispatchResult {
                    let book = maybe_book.as_mut().ok_or(Error::<T>::UnknownMarket)?;
                    ensure!(
                        matches!(book.kind, BookKind::Baseline { .. }),
                        Error::<T>::BadOrigin
                    );
                    if matches!(book.phase, MarketPhase::Closed) {
                        let elapsed = now
                            .checked_sub(book.last_observed_block)
                            .ok_or(Error::<T>::TryStateViolation)?;
                        book.cumulative_price_blocks = book
                            .cumulative_price_blocks
                            .checked_add_product(book.last_observation_1e9.0, elapsed)
                            .ok_or(Error::<T>::ArithmeticOverflow)?;
                        book.phase = MarketPhase::Trading;
                        book.last_observation_1e9 = book.last_quote_1e9;
                        book.last_observed_block = now;
                        ClosedAt::<T>::remove(id);
                    }
                    ensure!(
                        matches!(book.phase, MarketPhase::Trading | MarketPhase::Extended),
                        Error::<T>::NotTrading
                    );
                    Ok(())
                })?;
                T::PolCommitmentSync::sync_pol_commitments()
            })
        }

        /// Mark an existing proposal book as extended before registering its
        /// fresh exact window. Baseline books stay shared/trading.
        pub fn mark_extended(origin: OriginFor<T>, id: MarketId) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            Markets::<T>::try_mutate(id, |maybe_book| -> DispatchResult {
                let book = maybe_book.as_mut().ok_or(Error::<T>::UnknownMarket)?;
                ensure!(
                    !matches!(book.kind, BookKind::Baseline { .. }),
                    Error::<T>::BadOrigin
                );
                ensure!(
                    matches!(book.phase, MarketPhase::Trading | MarketPhase::Extended),
                    Error::<T>::NotTrading
                );
                book.phase = MarketPhase::Extended;
                Ok(())
            })
        }

        /// Seed an Accept/Reject pair from one dual-mint collateral split.
        pub fn seed_branch_pair(
            origin: OriginFor<T>,
            accept: MarketId,
            reject: MarketId,
            treasury: T::AccountId,
        ) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            Self::ensure_creation_open()?;
            ensure!(
                !SeededMarkets::<T>::contains_key(accept)
                    && !SeededMarkets::<T>::contains_key(reject),
                Error::<T>::AlreadySeeded
            );
            let accept_book = Markets::<T>::get(accept).ok_or(Error::<T>::UnknownMarket)?;
            let reject_book = Markets::<T>::get(reject).ok_or(Error::<T>::UnknownMarket)?;
            Self::ensure_market_book_indexed(&accept_book)?;
            Self::ensure_market_book_indexed(&reject_book)?;
            frame_support::storage::with_storage_layer(|| -> DispatchResult {
                let mut ledger = PalletLedger::<T>::new();
                let headroom = market_core::seed_branch_pair(
                    &accept_book,
                    &reject_book,
                    &mut ledger,
                    &treasury,
                )
                .map_err(Error::<T>::from)?;
                for id in [accept, reject] {
                    SeededMarkets::<T>::insert(id, ());
                    Self::insert_pol_commitment(id, headroom)?;
                    Self::deposit_event(Event::Seeded {
                        market: id,
                        headroom,
                    });
                }
                T::PolCommitmentSync::sync_pol_commitments()?;
                Ok(())
            })
        }

        /// Add one pair-sized rerun headroom split and double both books' LMSR
        /// depth while preserving every trader position.
        pub fn seed_rerun_branch_pair(
            origin: OriginFor<T>,
            accept: MarketId,
            reject: MarketId,
            treasury: T::AccountId,
        ) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            ensure!(
                SeededMarkets::<T>::contains_key(accept)
                    && SeededMarkets::<T>::contains_key(reject)
                    && !RerunSeededMarkets::<T>::contains_key(accept)
                    && !RerunSeededMarkets::<T>::contains_key(reject),
                Error::<T>::AlreadySeeded
            );
            let accept_book = Markets::<T>::get(accept).ok_or(Error::<T>::UnknownMarket)?;
            let reject_book = Markets::<T>::get(reject).ok_or(Error::<T>::UnknownMarket)?;
            Self::ensure_market_book_indexed(&accept_book)?;
            Self::ensure_market_book_indexed(&reject_book)?;
            frame_support::storage::with_storage_layer(|| -> DispatchResult {
                let mut ledger = PalletLedger::<T>::new();
                let headroom = market_core::seed_branch_pair(
                    &accept_book,
                    &reject_book,
                    &mut ledger,
                    &treasury,
                )
                .map_err(Error::<T>::from)?;
                for id in [accept, reject] {
                    Markets::<T>::try_mutate(id, |maybe_book| -> DispatchResult {
                        let book = maybe_book.as_mut().ok_or(Error::<T>::UnknownMarket)?;
                        book.b = book
                            .b
                            .checked_mul(2)
                            .ok_or(Error::<T>::ArithmeticOverflow)?;
                        Ok(())
                    })?;
                    RerunSeededMarkets::<T>::insert(id, ());
                    Self::increase_pol_commitment(id, headroom)?;
                    Self::deposit_event(Event::Seeded {
                        market: id,
                        headroom,
                    });
                }
                T::PolCommitmentSync::sync_pol_commitments()?;
                Ok(())
            })
        }

        /// Add exactly one original-size headroom seed and double LMSR `b`,
        /// yielding 2× total POL/depth while preserving positions (05 T13).
        pub fn seed_rerun(
            origin: OriginFor<T>,
            id: MarketId,
            treasury: T::AccountId,
        ) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            ensure!(
                SeededMarkets::<T>::contains_key(id),
                Error::<T>::TryStateViolation
            );
            ensure!(
                !RerunSeededMarkets::<T>::contains_key(id),
                Error::<T>::AlreadySeeded
            );
            let book = Markets::<T>::get(id).ok_or(Error::<T>::UnknownMarket)?;
            Self::ensure_market_book_indexed(&book)?;
            frame_support::storage::with_storage_layer(|| -> DispatchResult {
                let mut ledger = PalletLedger::<T>::new();
                let headroom = market_core::seed_book(&book, &mut ledger, &treasury)
                    .map_err(Error::<T>::from)?;
                Markets::<T>::try_mutate(id, |maybe_book| -> DispatchResult {
                    let stored = maybe_book.as_mut().ok_or(Error::<T>::UnknownMarket)?;
                    stored.b = stored
                        .b
                        .checked_mul(2)
                        .ok_or(Error::<T>::ArithmeticOverflow)?;
                    Ok(())
                })?;
                RerunSeededMarkets::<T>::insert(id, ());
                Self::increase_pol_commitment(id, headroom)?;
                Self::deposit_event(Event::Seeded {
                    market: id,
                    headroom,
                });
                T::PolCommitmentSync::sync_pol_commitments()?;
                Ok(())
            })
        }

        /// Exact TWAP for a registered window ending at `end`.
        pub fn twap_at(id: MarketId, end: BlockNumber, window: BlockNumber) -> Option<FixedU64> {
            let start = end.checked_sub(window)?;
            let registered = DecisionWindows::<T>::get(id).iter().any(|record| {
                (record.start == start || record.trailing_start == start) && record.end == end
            });
            if !registered {
                return None;
            }
            let checkpoints = TwapCheckpoints::<T>::get(id);
            let start_cumulative = checkpoints
                .iter()
                .find_map(|(block, cumulative)| (*block == start).then_some(*cumulative))?;
            let end_cumulative = checkpoints
                .iter()
                .find_map(|(block, cumulative)| (*block == end).then_some(*cumulative))?;
            market_core::twap_between(start_cumulative, end_cumulative, window)
        }

        /// Actual full/trailing widths of the registered window ending at
        /// `end`. Dead-man recovery extends only the end boundary, so callers
        /// must not substitute the current constitution defaults here.
        pub fn registered_window_lengths(
            id: MarketId,
            end: BlockNumber,
        ) -> Option<(BlockNumber, BlockNumber)> {
            DecisionWindows::<T>::get(id)
                .iter()
                .find(|record| record.end == end)
                .and_then(|record| {
                    let full = record.end.checked_sub(record.start)?;
                    let trailing = record.end.checked_sub(record.trailing_start)?;
                    Some((full, trailing))
                })
        }

        pub fn spot_at(id: MarketId, end: BlockNumber) -> Option<FixedU64> {
            DecisionWindows::<T>::get(id)
                .iter()
                .find_map(|window| (window.end == end).then_some(window.close_spot).flatten())
        }

        /// Instantaneous gross open interest (`q_long + q_short`), kept as
        /// telemetry only. Since the SQ-231 amendment gross notional is NOT
        /// the graded measure and never feeds validity floors or the step-9
        /// certificate — those consume the 04 §7a contest capital via
        /// [`Self::average_contest_at`].
        pub fn gross_open_interest(id: MarketId) -> Option<Balance> {
            let book = Markets::<T>::get(id)?;
            book.q_long.checked_add(book.q_short)
        }

        /// Full-window time-averaged non-POL contest capital (04 §7a:
        /// `ContestCapital(w) = (N(end) − N(start)) / blocks`), rounded down.
        pub fn average_contest_at(
            id: MarketId,
            end: BlockNumber,
            window: BlockNumber,
        ) -> Option<Balance> {
            DecisionWindows::<T>::get(id)
                .iter()
                .find(|record| {
                    record.end == end
                        && end
                            .checked_sub(window)
                            .is_some_and(|start| record.start == start)
                })
                .filter(|record| record.contest_valid)
                .and_then(|record| {
                    record
                        .contest_capital_blocks
                        .checked_div(u128::from(window))
                })
        }

        /// Per-check facts behind [`Self::decision_grade_at`], for the
        /// runtime adapter that needs the 05 §5.2 tri-state welfare-book
        /// partition (Insufficient vs Invalid) rather than the boolean fold.
        /// `None` when the book, its TWAP, the exact window record, or the
        /// observation-interval constant is unavailable (never gradable).
        #[allow(clippy::too_many_arguments)]
        pub fn decision_grade_facts_at(
            id: MarketId,
            end: BlockNumber,
            window: BlockNumber,
            coverage_pct: u8,
            convergence: FixedU64,
            contest_floor: Balance,
            pol_floor: Balance,
            require_sanity_band: bool,
        ) -> Option<DecisionGradeFacts> {
            let book = Markets::<T>::get(id)?;
            let twap = Self::twap_at(id, end, window)?;
            let start = end.checked_sub(window)?;
            let windows = DecisionWindows::<T>::get(id);
            let stats = windows
                .iter()
                .find(|record| record.start == start && record.end == end)?;
            let interval = u32::try_from(T::ObsInterval::get()).ok()?;
            Some(DecisionGradeFacts {
                sane: !require_sanity_band
                    || (twap.0 >= futarchy_primitives::kernel::DECISION_SANITY_MIN_1E9
                        && twap.0 <= futarchy_primitives::kernel::DECISION_SANITY_MAX_1E9),
                sealed: stats.sealed,
                stale_events: stats.stale_events,
                coverage_ok: market_core::coverage_at_least(
                    stats.observations,
                    window,
                    interval,
                    coverage_pct,
                ),
                pol_ok: SeededMarkets::<T>::contains_key(id) && book.b >= pol_floor,
                contest_valid: stats.contest_valid,
                contest_ok: stats.contest_valid
                    && stats
                        .contest_capital_blocks
                        .checked_div(u128::from(window))
                        .is_some_and(|contest| contest >= contest_floor),
                converged: stats
                    .close_spot
                    .is_some_and(|spot| spot.0.abs_diff(twap.0) <= convergence.0),
            })
        }

        /// Coverage/staleness/POL/contest/convergence grade shared by the
        /// runtime role-specific adapter: the boolean fold of
        /// [`Self::decision_grade_facts_at`].
        #[allow(clippy::too_many_arguments)]
        pub fn decision_grade_at(
            id: MarketId,
            end: BlockNumber,
            window: BlockNumber,
            coverage_pct: u8,
            convergence: FixedU64,
            contest_floor: Balance,
            pol_floor: Balance,
            require_sanity_band: bool,
        ) -> bool {
            Self::decision_grade_facts_at(
                id,
                end,
                window,
                coverage_pct,
                convergence,
                contest_floor,
                pol_floor,
                require_sanity_band,
            )
            .is_some_and(|facts| {
                facts.sane
                    && facts.sealed
                    && facts.stale_events == 0
                    && facts.coverage_ok
                    && facts.pol_ok
                    && facts.contest_valid
                    && facts.contest_ok
                    && facts.converged
            })
        }

        fn insert_checkpoint(id: MarketId, block: BlockNumber, cumulative: TwapCumulative) {
            TwapCheckpoints::<T>::mutate(id, |checkpoints| {
                // A boundary is a historical accumulator snapshot. Once
                // present it is immutable, including when another overlapping
                // window later crosses the same boundary.
                if checkpoints.iter().any(|(at, _)| *at == block) {
                    return;
                }
                if checkpoints.try_push((block, cumulative)).is_ok() {
                    checkpoints.sort_by_key(|(at, _)| *at);
                }
                // Capacity failure deliberately leaves the checkpoint absent;
                // the grade read then returns false (G-1).
            });
        }

        fn record_observation(
            id: MarketId,
            before: &MarketBook<T::AccountId>,
            after: &MarketBook<T::AccountId>,
        ) {
            // A trade in the exact close block must leave the final quote as
            // the window's spot even when the observation cadence does not
            // advance in that block.  Update this before the observation
            // early-return so a second close-block trade cannot leave a stale
            // first-trade quote behind (04 §7.2; 05 §5.2).
            if let Ok(now) = u32::try_from(Self::now_u64()) {
                DecisionWindows::<T>::mutate(id, |windows| {
                    for window in windows
                        .iter_mut()
                        .filter(|window| window.end == now && !window.sealed)
                    {
                        window.close_spot = Some(after.last_quote_1e9);
                    }
                });
            }
            if before.last_observed_block == after.last_observed_block {
                return;
            }
            let Ok(previous_block) = u32::try_from(before.last_observed_block) else {
                return;
            };
            let Ok(observed_block) = u32::try_from(after.last_observed_block) else {
                return;
            };
            let observation = after.last_observation_1e9;
            DecisionWindows::<T>::mutate(id, |windows| {
                for window in windows.iter_mut() {
                    if window.sealed {
                        continue;
                    }
                    // A crank after the close may not backfill an end checkpoint
                    // using information first observed after that close.
                    if observed_block > window.end {
                        continue;
                    }
                    for boundary in [window.start, window.trailing_start, window.end] {
                        if previous_block < boundary && boundary <= observed_block {
                            if let Some(cumulative) = market_core::accumulator_at_boundary(
                                previous_block,
                                before.cumulative_price_blocks,
                                observation,
                                boundary,
                            ) {
                                Self::insert_checkpoint(id, boundary, cumulative);
                            }
                        }
                    }
                    if observed_block > window.start {
                        window.observations = window.observations.saturating_add(1);
                        let stale_gap = match u32::try_from(market_core::STALE_GAP_BLOCKS) {
                            Ok(value) => value,
                            Err(_) => return,
                        };
                        if observed_block.saturating_sub(previous_block) > stale_gap {
                            window.stale_events = window.stale_events.saturating_add(1);
                        }
                    }
                    if observed_block == window.end {
                        window.close_spot = Some(after.last_quote_1e9);
                    }
                }
            });
        }

        /// Internal epoch-authority API: create the vault and its trading book.
        #[allow(clippy::too_many_arguments)]
        pub fn create_market(
            origin: OriginFor<T>,
            id: MarketId,
            kind: BookKind,
            account: T::AccountId,
            fees_account: T::AccountId,
            b: Balance,
        ) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            Self::ensure_creation_open()?;
            ensure!(!Markets::<T>::contains_key(id), Error::<T>::DuplicateMarket);
            ensure!(b > 0, Error::<T>::TryStateViolation);
            // I-21: cap the live-book set at dispatch, not only in try_state (which
            // does not run in production blocks). `CountedStorageMap::count()` is O(1).
            ensure!(
                Markets::<T>::count() < bounds::MAX_LIVE_MARKETS,
                Error::<T>::TooManyMarkets
            );

            if let BookKind::Baseline { epoch } = kind {
                ensure!(
                    !BaselineMarketOf::<T>::contains_key(epoch),
                    Error::<T>::DuplicateBaselineMarket
                );
            }

            let (market_kind, pid, epoch) = Self::describe_kind(kind);
            // Same reasoning as `seed`: this internal path creates a ledger vault and
            // writes market storage, so wrap it in a storage layer so a partial failure
            // cannot outlive its caller's error handling (G-1).
            frame_support::storage::with_storage_layer(|| -> DispatchResult {
                match kind {
                    BookKind::Decision { proposal, .. } | BookKind::Gate { proposal, .. } => {
                        // A proposal's ≤ 6 books (2 decision + 4 gate, 04 §1.1) share ONE
                        // conditional-ledger vault (03 §2.1). Create it on the first book
                        // for this proposal and reuse it for the rest: the ledger rejects a
                        // duplicate `create_vault`, so without this guard every multi-book
                        // proposal — including a bare PARAM decision *pair* — would fail on
                        // its second `create_market` (G-1: reuse, never error out).
                        if !pallet_conditional_ledger::Vaults::<T>::contains_key(proposal) {
                            pallet_conditional_ledger::Pallet::<T>::create_vault(
                                PalletLedger::<T>::authority_origin(),
                                proposal,
                                0,
                            )?;
                        }
                    }
                    BookKind::Baseline { epoch } => {
                        pallet_conditional_ledger::Pallet::<T>::create_baseline_vault(
                            PalletLedger::<T>::authority_origin(),
                            epoch,
                        )?;
                        BaselineMarketOf::<T>::insert(epoch, id);
                    }
                }

                Self::register_market_accounts(&account, &fees_account)?;
                if let BookKind::Decision { proposal, .. } | BookKind::Gate { proposal, .. } = kind
                {
                    ProposalMarketIds::<T>::try_mutate(proposal, |ids| -> DispatchResult {
                        ensure!(!ids.contains(&id), Error::<T>::DuplicateMarket);
                        ids.try_push(id).map_err(|_| Error::<T>::TooManyMarkets)?;
                        Ok(())
                    })?;
                }
                Markets::<T>::insert(id, MarketBook::open(id, kind, account, fees_account, b));
                Self::deposit_event(Event::MarketCreated {
                    market: id,
                    kind: market_kind,
                    pid,
                    epoch,
                    b,
                });
                Ok(())
            })
        }

        /// Internal epoch-authority API: seed worst-case-loss headroom (04 §10).
        pub fn seed(origin: OriginFor<T>, id: MarketId, treasury: T::AccountId) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            Self::ensure_creation_open()?;
            let book = Markets::<T>::get(id).ok_or(Error::<T>::UnknownMarket)?;
            // Seed once: re-seeding splits fresh POL headroom into an already
            // collateralized book (04 §10), double-spending the subsidy.
            ensure!(
                !SeededMarkets::<T>::contains_key(id),
                Error::<T>::AlreadySeeded
            );
            Self::ensure_market_book_indexed(&book)?;
            // Internal (non-`#[pallet::call]`) path: FRAME's per-dispatch storage layer
            // wraps only public extrinsics, so an epoch-tick caller that swallows the
            // error would strand a partial seed (`seed_book` drives several ledger
            // `do_*` writes that can `Err` after the scoped adapter already persisted,
            // and `SeededMarkets` would go unwritten → a retry could double-seed). Wrap
            // the whole sequence so any partial failure rolls back atomically (G-1).
            frame_support::storage::with_storage_layer(|| -> DispatchResult {
                let mut ledger = PalletLedger::<T>::new();
                let headroom = market_core::seed_book(&book, &mut ledger, &treasury)
                    .map_err(Error::<T>::from)?;
                SeededMarkets::<T>::insert(id, ());
                Self::insert_pol_commitment(id, headroom)?;
                Self::deposit_event(Event::Seeded {
                    market: id,
                    headroom,
                });
                T::PolCommitmentSync::sync_pol_commitments()?;
                Ok(())
            })
        }

        /// Internal epoch-authority API: close a book and start its archive delay.
        pub fn close(origin: OriginFor<T>, id: MarketId) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            frame_support::storage::with_storage_layer(|| Self::close_book(id))
        }

        /// Runtime expiry hook for PB-DEPEG. No public origin can call this;
        /// guardian maintenance reaches it through the effect dispatcher.
        pub fn clear_creation_freeze() {
            CreationFrozenUntil::<T>::kill();
            Self::deposit_event(Event::CreationFreezeCleared);
        }

        /// Runtime renewal hook for PB-LEDGER-FREEZE. Installs one fresh
        /// 14-day window from ratification time and never twice.
        pub fn extend_freeze_once() -> DispatchResult {
            let now = frame_system::Pallet::<T>::block_number();
            let current = FrozenUntil::<T>::get().ok_or(Error::<T>::Frozen)?;
            ensure!(now < current, Error::<T>::Frozen);
            ensure!(
                !FreezeRenewed::<T>::get(),
                Error::<T>::FreezeRenewalExhausted
            );
            let until = now
                .checked_add(&kernel::PLAYBOOK_FREEZE_WINDOW_BLOCKS.into())
                .ok_or(Error::<T>::ArithmeticOverflow)?;
            FrozenUntil::<T>::put(until);
            FreezeRenewed::<T>::put(true);
            Self::deposit_event(Event::FreezeExtended { until });
            Ok(())
        }

        fn ensure_creation_open() -> DispatchResult {
            let now = frame_system::Pallet::<T>::block_number();
            ensure!(
                CreationFrozenUntil::<T>::get().is_none_or(|until| now >= until),
                Error::<T>::CreationFrozen
            );
            Ok(())
        }

        fn ensure_not_frozen() -> DispatchResult {
            let now = frame_system::Pallet::<T>::block_number();
            ensure!(
                FrozenUntil::<T>::get().is_none_or(|until| now >= until),
                Error::<T>::Frozen
            );
            Ok(())
        }

        /// Epoch-authority boundary seal for a particular proposal window.
        /// Shared Baseline books remain open, but this window becomes immutable.
        pub fn seal_decision_window(
            origin: OriginFor<T>,
            id: MarketId,
            end: BlockNumber,
        ) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            let book = Markets::<T>::get(id).ok_or(Error::<T>::UnknownMarket)?;
            ensure!(Self::now_u64() >= u64::from(end), Error::<T>::NotTrading);
            Self::seal_window(id, &book, end)
        }

        /// Market sovereign account used to sign the ledger's internal API.
        pub fn account_id() -> T::AccountId {
            <T as Config>::PalletId::get().into_account_truncating()
        }

        /// Observe a proposal ledger terminal marker exactly once, release all
        /// of its bounded book obligations, and mirror treasury in the same
        /// transaction. The latch survives later ledger sweeping.
        pub fn observe_proposal_terminal(proposal: ProposalId) -> DispatchResult {
            let terminal = pallet_conditional_ledger::VaultTerminalAt::<T>::get(proposal)
                .ok_or(Error::<T>::TryStateViolation)?;
            frame_support::storage::with_storage_layer(|| -> DispatchResult {
                let ids = ProposalMarketIds::<T>::get(proposal);
                ensure!(!ids.is_empty(), Error::<T>::TryStateViolation);
                for id in ids {
                    let book = Markets::<T>::get(id).ok_or(Error::<T>::TryStateViolation)?;
                    ensure!(
                        matches!(book.kind,
                            BookKind::Decision { proposal: owner, .. }
                            | BookKind::Gate { proposal: owner, .. }
                            if owner == proposal
                        ),
                        Error::<T>::TryStateViolation
                    );
                    if !matches!(book.phase, MarketPhase::Closed) {
                        Self::close_book(id)?;
                    }
                    if let Some(observed) = SettlementObservedAt::<T>::get(id) {
                        ensure!(observed == terminal, Error::<T>::TryStateViolation);
                    } else {
                        SettlementObservedAt::<T>::insert(id, terminal);
                    }
                    Self::remove_pol_commitment(id);
                }
                T::PolCommitmentSync::sync_pol_commitments()
            })
        }

        /// Baseline counterpart of `observe_proposal_terminal`.
        pub fn observe_baseline_terminal(epoch: EpochId) -> DispatchResult {
            let terminal = pallet_conditional_ledger::BaselineTerminalAt::<T>::get(epoch)
                .ok_or(Error::<T>::TryStateViolation)?;
            let id = BaselineMarketOf::<T>::get(epoch).ok_or(Error::<T>::TryStateViolation)?;
            frame_support::storage::with_storage_layer(|| -> DispatchResult {
                let book = Markets::<T>::get(id).ok_or(Error::<T>::TryStateViolation)?;
                ensure!(
                    matches!(book.kind, BookKind::Baseline { epoch: owner } if owner == epoch),
                    Error::<T>::TryStateViolation
                );
                // Close first, exactly as `observe_proposal_terminal` does
                // (SQ-92). On the ordinary path the Baseline book is already
                // closed — `close_markets` closes it once the epoch's last
                // proposal leaves Trading/Extended — but an **epoch VOID**
                // force-rejects through `void_cohort` without ever passing
                // through `decide`, so nothing closed it. Latching
                // `SettlementObservedAt` on a still-open book violates
                // try-state, which requires every observed entry to carry
                // `MarketPhase::Closed` + `ClosedAt`.
                if !matches!(book.phase, MarketPhase::Closed) {
                    Self::close_book(id)?;
                }
                if let Some(observed) = SettlementObservedAt::<T>::get(id) {
                    ensure!(observed == terminal, Error::<T>::TryStateViolation);
                } else {
                    SettlementObservedAt::<T>::insert(id, terminal);
                }
                Self::remove_pol_commitment(id);
                T::PolCommitmentSync::sync_pol_commitments()
            })
        }

        /// Whether a seeded book still carries its worst-case-loss obligation.
        /// Close only freezes the price path; the obligation ends at the ledger's
        /// terminal settlement marker and is then durably latched by the market
        /// before the ledger marker can be swept.
        pub fn pol_obligation_live(id: MarketId, book: &MarketBook<T::AccountId>) -> bool {
            if !SeededMarkets::<T>::contains_key(id) {
                return false;
            }
            let _ = book;
            !SettlementObservedAt::<T>::contains_key(id)
        }

        /// Shared close sequence for the ordinary decision boundary and an
        /// early terminal VOID. Callers provide the surrounding storage layer
        /// so close, terminal latching, and POL release can commit atomically.
        fn close_book(id: MarketId) -> DispatchResult {
            let mut book = Markets::<T>::get(id).ok_or(Error::<T>::UnknownMarket)?;
            let now = Self::now_u64();
            Self::seal_due_windows(id, &book, now, true)?;
            Self::accrue_contest(id, &book, now);
            book.phase = MarketPhase::Closed;
            Markets::<T>::insert(id, book);
            ClosedAt::<T>::insert(id, frame_system::Pallet::<T>::block_number());
            Self::deposit_event(Event::MarketClosed { market: id });
            Ok(())
        }

        fn register_market_accounts(
            account: &T::AccountId,
            fees_account: &T::AccountId,
        ) -> DispatchResult {
            for who in [account, fees_account] {
                MarketProtocolAccounts::<T>::try_mutate(who, |references| -> DispatchResult {
                    match references {
                        Some(count) => {
                            *count = count.checked_add(1).ok_or(Error::<T>::ArithmeticOverflow)?;
                        }
                        None => {
                            let bound = bounds::MAX_LIVE_MARKETS
                                .checked_mul(2)
                                .ok_or(Error::<T>::ArithmeticOverflow)?;
                            ensure!(
                                MarketProtocolAccounts::<T>::count() < bound,
                                Error::<T>::TooManyMarkets
                            );
                            *references = Some(1);
                        }
                    }
                    Ok(())
                })?;
            }
            Ok(())
        }

        fn unregister_market_accounts(book: &MarketBook<T::AccountId>) -> DispatchResult {
            for who in [&book.account, &book.fees_account] {
                MarketProtocolAccounts::<T>::try_mutate_exists(
                    who,
                    |references| -> DispatchResult {
                        let count = references.ok_or(Error::<T>::TryStateViolation)?;
                        if count == 1 {
                            *references = None;
                        } else {
                            *references =
                                Some(count.checked_sub(1).ok_or(Error::<T>::TryStateViolation)?);
                        }
                        Ok(())
                    },
                )?;
            }
            Ok(())
        }

        fn ensure_market_book_indexed(book: &MarketBook<T::AccountId>) -> DispatchResult {
            ensure!(
                MarketProtocolAccounts::<T>::contains_key(&book.account)
                    && MarketProtocolAccounts::<T>::contains_key(&book.fees_account),
                Error::<T>::TryStateViolation
            );
            Ok(())
        }

        fn insert_pol_commitment(id: MarketId, amount: Balance) -> DispatchResult {
            ensure!(amount > 0, Error::<T>::TryStateViolation);
            LivePolCommitments::<T>::try_mutate(|commitments| -> DispatchResult {
                ensure!(
                    !commitments.iter().any(|(market, _)| *market == id),
                    Error::<T>::AlreadySeeded
                );
                commitments
                    .try_push((id, amount))
                    .map_err(|_| Error::<T>::TooManyMarkets)?;
                commitments.sort_by_key(|(market, _)| *market);
                Ok(())
            })
        }

        fn increase_pol_commitment(id: MarketId, amount: Balance) -> DispatchResult {
            ensure!(amount > 0, Error::<T>::TryStateViolation);
            LivePolCommitments::<T>::try_mutate(|commitments| {
                let (_, stored) = commitments
                    .iter_mut()
                    .find(|(market, _)| *market == id)
                    .ok_or(Error::<T>::TryStateViolation)?;
                *stored = stored
                    .checked_add(amount)
                    .ok_or(Error::<T>::ArithmeticOverflow)?;
                Ok(())
            })
        }

        fn remove_pol_commitment(id: MarketId) {
            LivePolCommitments::<T>::mutate(|commitments| {
                commitments.retain(|(market, _)| *market != id);
            });
        }

        fn params() -> MarketParams {
            MarketParams {
                fee_bps: T::Fee::get(),
                obs_interval: T::ObsInterval::get(),
                kappa_1e9: T::Kappa1e9::get(),
                stale_gap_blocks: market_core::STALE_GAP_BLOCKS,
            }
        }

        fn now_u64() -> u64 {
            frame_system::Pallet::<T>::block_number().unique_saturated_into()
        }

        /// Advance every unsealed window's 04 §7a contest-capital integral
        /// `N += noi_t · Δblocks` through `through`. `book` is the *stored*
        /// (pre-dispatch) book, so every accrued segment is priced and sized
        /// from the previous block's stored `q` and quote — a trade can never
        /// contribute its own state to the segment it closes. The segment grid
        /// is the exact event grid (finer than the once-per-interval sample of
        /// 04 §7a, and never larger: a position flashed across an observation
        /// boundary receives its held blocks only, not a backward interval —
        /// the A8-review flash-credit fix; under-counting is §7a's stated
        /// conservative direction). `noi_t` rounds DOWN; any overflow marks
        /// the window invalid, and an invalid window never grades (G-1).
        fn accrue_contest(id: MarketId, book: &MarketBook<T::AccountId>, through: u64) {
            let Ok(through) = u32::try_from(through) else {
                DecisionWindows::<T>::mutate(id, |windows| {
                    for window in windows.iter_mut() {
                        if !window.sealed {
                            window.contest_valid = false;
                        }
                    }
                });
                return;
            };
            let noi = market_core::contest_capital(book.q_long, book.q_short, book.last_quote_1e9);
            DecisionWindows::<T>::mutate(id, |windows| {
                for window in windows.iter_mut() {
                    if window.sealed {
                        continue;
                    }
                    let from = window.contest_accrued_until.max(window.start);
                    let to = through.min(window.end);
                    if from >= to {
                        continue;
                    }
                    if window.contest_valid {
                        let addition =
                            noi.and_then(|value| value.checked_mul(u128::from(to - from)));
                        match addition
                            .and_then(|value| window.contest_capital_blocks.checked_add(value))
                        {
                            Some(value) => window.contest_capital_blocks = value,
                            None => window.contest_valid = false,
                        }
                    }
                    window.contest_accrued_until = to;
                }
            });
        }

        fn seal_due_windows(
            id: MarketId,
            book: &MarketBook<T::AccountId>,
            now: u64,
            include_current: bool,
        ) -> DispatchResult {
            let now = u32::try_from(now).map_err(|_| Error::<T>::ArithmeticOverflow)?;
            let ends = DecisionWindows::<T>::get(id)
                .iter()
                .filter(|window| {
                    !window.sealed && (window.end < now || (include_current && window.end == now))
                })
                .map(|window| window.end)
                .collect::<Vec<_>>();
            for end in ends {
                Self::seal_window(id, book, end)?;
            }
            Ok(())
        }

        fn seal_window(
            id: MarketId,
            book: &MarketBook<T::AccountId>,
            end: BlockNumber,
        ) -> DispatchResult {
            ensure!(
                DecisionWindows::<T>::get(id)
                    .iter()
                    .any(|window| window.end == end),
                Error::<T>::TryStateViolation
            );
            if DecisionWindows::<T>::get(id)
                .iter()
                .filter(|window| window.end == end)
                .all(|window| window.sealed)
            {
                return Ok(());
            }
            Self::accrue_contest(id, book, u64::from(end));
            let checkpoints = TwapCheckpoints::<T>::get(id);
            if !checkpoints.iter().any(|(at, _)| *at == end) {
                let previous = u32::try_from(book.last_observed_block)
                    .map_err(|_| Error::<T>::ArithmeticOverflow)?;
                ensure!(previous <= end, Error::<T>::TryStateViolation);
                let cumulative = market_core::accumulator_at_boundary(
                    previous,
                    book.cumulative_price_blocks,
                    book.last_observation_1e9,
                    end,
                )
                .ok_or(Error::<T>::ArithmeticOverflow)?;
                Self::insert_checkpoint(id, end, cumulative);
            }
            DecisionWindows::<T>::mutate(id, |windows| {
                for window in windows.iter_mut().filter(|window| window.end == end) {
                    if window.close_spot.is_none() {
                        window.close_spot = Some(book.last_quote_1e9);
                    }
                    window.sealed = true;
                }
            });
            Ok(())
        }

        /// Read-only trade-admission preflight shared by dispatch and runtime
        /// views (02 §3; 04 §6.4). Registered decision-window expiry is a
        /// trading precondition even when a keeper has not moved the stored
        /// phase out of `Trading`; the core owns the phase predicate.
        pub fn ensure_trade_admissible(
            id: MarketId,
            book: &MarketBook<T::AccountId>,
        ) -> DispatchResult {
            let windows = DecisionWindows::<T>::get(id);
            if let Some(latest_end) = windows.iter().map(|window| window.end).max() {
                ensure!(
                    Self::now_u64() <= u64::from(latest_end),
                    Error::<T>::NotTrading
                );
            }
            market_core::ensure_trade_phase(book.phase).map_err(Error::<T>::from)?;
            Ok(())
        }

        fn describe_kind(kind: BookKind) -> (MarketKind, Option<ProposalId>, EpochId) {
            match kind {
                BookKind::Decision { proposal, branch } => (
                    if matches!(branch, Branch::Accept) {
                        MarketKind::DecisionAccept
                    } else {
                        MarketKind::DecisionReject
                    },
                    Some(proposal),
                    0,
                ),
                BookKind::Gate {
                    proposal,
                    branch,
                    gate,
                } => {
                    let kind = match (gate, branch) {
                        (GateType::Survival, Branch::Accept) => MarketKind::GateS_Adopt,
                        (GateType::Survival, Branch::Reject) => MarketKind::GateS_Reject,
                        (GateType::Security, Branch::Accept) => MarketKind::GateC_Adopt,
                        (GateType::Security, Branch::Reject) => MarketKind::GateC_Reject,
                    };
                    (kind, Some(proposal), 0)
                }
                BookKind::Baseline { epoch } => (MarketKind::Baseline, None, epoch),
            }
        }

        fn deposit_trade_event(event: market_core::Event<T::AccountId>) -> DispatchResult {
            match event {
                market_core::Event::Traded {
                    market,
                    who,
                    side,
                    amount,
                    cost,
                    p_after,
                } => Self::deposit_event(Event::Traded {
                    market,
                    who,
                    side,
                    amount,
                    cost,
                    p_after,
                }),
                market_core::Event::Observed { market, o_t } => {
                    Self::deposit_event(Event::Observed { market, o_t });
                }
                _ => return Err(Error::<T>::TryStateViolation.into()),
            }
            Ok(())
        }

        /// Storage-level try-state (15 §1 market coverage): the I-21 bound, per-book
        /// LMSR domain sanity, **I-12 structural collateralization** (every live book
        /// is backed by a live conditional-ledger vault — the escrow ≥ obligations
        /// solvency identity itself is proven by the ledger's own try_state, L-1/L-2),
        /// **I-13 accumulator sanity** (the TWAP accumulator cannot exceed max-price ×
        /// elapsed and cannot observe the future), and the `BaselineMarketOf` inverse.
        /// The per-book maker-loss ≤ b·ln2 bound is an S1 property/differential
        /// obligation (15 I-12: "differential vs MPFR; fuzz"), not a try-state check.
        pub fn do_try_state() -> Result<(), DispatchError> {
            let now: u64 = Self::now_u64();
            for (id, book) in Markets::<T>::iter() {
                ensure!(book.b > 0, Error::<T>::TryStateViolation);
                let domain = book
                    .b
                    .checked_mul(48)
                    .ok_or(Error::<T>::TryStateViolation)?;
                ensure!(
                    (book.q_long <= domain && book.q_short <= domain)
                        || book.q_long.abs_diff(book.q_short) <= domain,
                    Error::<T>::TryStateViolation
                );
                ensure!(
                    book.last_quote_1e9.0 <= market_core::PRICE_ONE_1E9
                        && book.last_observation_1e9.0 <= market_core::PRICE_ONE_1E9,
                    Error::<T>::TryStateViolation
                );
                // I-12 (structural): the book is backed by a live ledger vault.
                let vault_exists = match book.kind {
                    BookKind::Decision { proposal, .. } | BookKind::Gate { proposal, .. } => {
                        pallet_conditional_ledger::Vaults::<T>::contains_key(proposal)
                    }
                    BookKind::Baseline { epoch } => {
                        pallet_conditional_ledger::BaselineVaults::<T>::contains_key(epoch)
                    }
                };
                ensure!(
                    vault_exists || SettlementObservedAt::<T>::contains_key(id),
                    Error::<T>::TryStateViolation
                );
                // I-13 (accumulator sanity): no future observation, and the
                // price-weighted sum ≤ max-price × elapsed blocks.
                ensure!(
                    book.last_observed_block <= now,
                    Error::<T>::TryStateViolation
                );
                let max_accum = TwapCumulative::from(
                    u128::from(book.last_observed_block)
                        .checked_mul(u128::from(market_core::PRICE_ONE_1E9))
                        .ok_or(Error::<T>::TryStateViolation)?,
                );
                ensure!(
                    book.cumulative_price_blocks <= max_accum,
                    Error::<T>::TryStateViolation
                );
            }
            ensure!(
                Markets::<T>::count() <= bounds::MAX_LIVE_MARKETS,
                Error::<T>::TryStateViolation
            );
            ensure!(
                MarketProtocolAccounts::<T>::count() <= bounds::MAX_LIVE_MARKETS.saturating_mul(2),
                Error::<T>::TryStateViolation
            );
            for (who, references) in MarketProtocolAccounts::<T>::iter() {
                let expected = Markets::<T>::iter_values().try_fold(0_u16, |count, book| {
                    let additions = u16::from(book.account == who)
                        .checked_add(u16::from(book.fees_account == who))?;
                    count.checked_add(additions)
                });
                ensure!(
                    references > 0 && expected == Some(references),
                    Error::<T>::TryStateViolation
                );
            }
            for (id, book) in Markets::<T>::iter() {
                Self::ensure_market_book_indexed(&book)?;
                if let BookKind::Decision { proposal, .. } | BookKind::Gate { proposal, .. } =
                    book.kind
                {
                    ensure!(
                        ProposalMarketIds::<T>::get(proposal).contains(&id),
                        Error::<T>::TryStateViolation
                    );
                }
            }
            for (proposal, ids) in ProposalMarketIds::<T>::iter() {
                ensure!(!ids.is_empty(), Error::<T>::TryStateViolation);
                let mut previous = None;
                for id in ids {
                    let book = Markets::<T>::get(id).ok_or(Error::<T>::TryStateViolation)?;
                    ensure!(
                        previous.is_none_or(|prior| prior < id)
                            && matches!(book.kind,
                                BookKind::Decision { proposal: owner, .. }
                                | BookKind::Gate { proposal: owner, .. }
                                if owner == proposal
                            ),
                        Error::<T>::TryStateViolation
                    );
                    previous = Some(id);
                }
            }
            for (epoch, market) in BaselineMarketOf::<T>::iter() {
                let book = Markets::<T>::get(market).ok_or(Error::<T>::TryStateViolation)?;
                ensure!(
                    matches!(book.kind, BookKind::Baseline { epoch: e } if e == epoch),
                    Error::<T>::TryStateViolation
                );
            }
            for (id, checkpoints) in TwapCheckpoints::<T>::iter() {
                let _book = Markets::<T>::get(id).ok_or(Error::<T>::TryStateViolation)?;
                let windows = DecisionWindows::<T>::get(id);
                let mut previous = None;
                let mut previous_cumulative = None;
                for (block, cumulative) in checkpoints {
                    let max_at_boundary = TwapCumulative::from(
                        u128::from(block)
                            .checked_mul(u128::from(market_core::PRICE_ONE_1E9))
                            .ok_or(Error::<T>::TryStateViolation)?,
                    );
                    ensure!(
                        previous.is_none_or(|prior| prior < block)
                            && u64::from(block) <= now
                            && previous_cumulative.is_none_or(|prior| prior <= cumulative)
                            && cumulative <= max_at_boundary,
                        Error::<T>::TryStateViolation
                    );
                    ensure!(
                        windows.iter().any(|window| {
                            [window.start, window.trailing_start, window.end].contains(&block)
                        }),
                        Error::<T>::TryStateViolation
                    );
                    previous = Some(block);
                    previous_cumulative = Some(cumulative);
                }
            }
            for (id, windows) in DecisionWindows::<T>::iter() {
                ensure!(
                    Markets::<T>::contains_key(id),
                    Error::<T>::TryStateViolation
                );
                for window in windows {
                    ensure!(
                        DecisionWindowOwners::<T>::get(id).iter().any(|record| {
                            record.1 == window.start
                                && record.2 == window.trailing_start
                                && record.3 == window.end
                        }),
                        Error::<T>::TryStateViolation
                    );
                    ensure!(
                        window.start < window.trailing_start
                            && window.trailing_start < window.end
                            && window
                                .close_spot
                                .is_none_or(|spot| spot.0 <= market_core::PRICE_ONE_1E9),
                        Error::<T>::TryStateViolation
                    );
                    // I-13 accumulator sanity, 04 §7a contest-capital leg:
                    // the integral cursor stays inside its window, never runs
                    // ahead of the chain, and a window that has accrued
                    // nothing holds exactly the zero integral (`N` itself is
                    // monotone non-decreasing by construction — the cursor
                    // checks here are the state-observable checkpoint
                    // discipline of that accumulator).
                    ensure!(
                        window.contest_accrued_until >= window.start
                            && window.contest_accrued_until <= window.end,
                        Error::<T>::TryStateViolation
                    );
                    ensure!(
                        window.contest_accrued_until == window.start
                            || window.sealed
                            || u64::from(window.contest_accrued_until) <= now,
                        Error::<T>::TryStateViolation
                    );
                    if window.contest_accrued_until == window.start {
                        ensure!(
                            window.contest_capital_blocks == 0,
                            Error::<T>::TryStateViolation
                        );
                    }
                    if window.sealed {
                        ensure!(
                            window.close_spot.is_some()
                                && window.contest_accrued_until == window.end
                                && TwapCheckpoints::<T>::get(id)
                                    .iter()
                                    .any(|(block, _)| *block == window.end),
                            Error::<T>::TryStateViolation
                        );
                    }
                }
            }
            for (id, owners) in DecisionWindowOwners::<T>::iter() {
                ensure!(
                    Markets::<T>::contains_key(id),
                    Error::<T>::TryStateViolation
                );
                for (position, owner) in owners.iter().enumerate() {
                    ensure!(
                        !owners.iter().take(position).any(|seen| seen == owner)
                            && DecisionWindows::<T>::get(id).iter().any(|window| {
                                window.start == owner.1
                                    && window.trailing_start == owner.2
                                    && window.end == owner.3
                            }),
                        Error::<T>::TryStateViolation
                    );
                }
            }
            for id in SeededMarkets::<T>::iter_keys() {
                ensure!(
                    Markets::<T>::contains_key(id),
                    Error::<T>::TryStateViolation
                );
            }
            let commitments = LivePolCommitments::<T>::get();
            let mut previous_commitment = None;
            for (id, amount) in &commitments {
                let book = Markets::<T>::get(id).ok_or(Error::<T>::TryStateViolation)?;
                let expected = if RerunSeededMarkets::<T>::contains_key(id) {
                    let original_b = book.b.checked_div(2).ok_or(Error::<T>::TryStateViolation)?;
                    market_core::seed_headroom(original_b)
                        .ok()
                        .and_then(|headroom| headroom.checked_mul(2))
                } else {
                    market_core::seed_headroom(book.b).ok()
                };
                ensure!(
                    previous_commitment.is_none_or(|previous| previous < *id)
                        && *amount > 0
                        && expected == Some(*amount)
                        && SeededMarkets::<T>::contains_key(id)
                        && !SettlementObservedAt::<T>::contains_key(id),
                    Error::<T>::TryStateViolation
                );
                previous_commitment = Some(*id);
            }
            for id in SeededMarkets::<T>::iter_keys() {
                let indexed = commitments.iter().any(|(market, _)| *market == id);
                ensure!(
                    indexed != SettlementObservedAt::<T>::contains_key(id),
                    Error::<T>::TryStateViolation
                );
            }
            for (id, terminal) in SettlementObservedAt::<T>::iter() {
                let book = Markets::<T>::get(id).ok_or(Error::<T>::TryStateViolation)?;
                ensure!(
                    terminal <= frame_system::Pallet::<T>::block_number()
                        && !commitments.iter().any(|(market, _)| *market == id)
                        && matches!(book.phase, MarketPhase::Closed)
                        && ClosedAt::<T>::get(id).is_some_and(|closed| closed <= terminal),
                    Error::<T>::TryStateViolation
                );
                match book.kind {
                    BookKind::Decision { proposal, .. } | BookKind::Gate { proposal, .. } => {
                        let ledger_terminal =
                            pallet_conditional_ledger::VaultTerminalAt::<T>::get(proposal);
                        if pallet_conditional_ledger::Vaults::<T>::contains_key(proposal) {
                            ensure!(ledger_terminal.is_some(), Error::<T>::TryStateViolation);
                        }
                        if let Some(ledger_terminal) = ledger_terminal {
                            ensure!(ledger_terminal == terminal, Error::<T>::TryStateViolation);
                        }
                    }
                    BookKind::Baseline { epoch } => {
                        let ledger_terminal =
                            pallet_conditional_ledger::BaselineTerminalAt::<T>::get(epoch);
                        if pallet_conditional_ledger::BaselineVaults::<T>::contains_key(epoch) {
                            ensure!(ledger_terminal.is_some(), Error::<T>::TryStateViolation);
                        }
                        if let Some(ledger_terminal) = ledger_terminal {
                            ensure!(ledger_terminal == terminal, Error::<T>::TryStateViolation);
                        }
                    }
                }
            }
            for id in RerunSeededMarkets::<T>::iter_keys() {
                ensure!(
                    Markets::<T>::contains_key(id) && SeededMarkets::<T>::contains_key(id),
                    Error::<T>::TryStateViolation
                );
            }
            ensure!(
                T::PolCommitmentSync::pol_commitments_synced(),
                Error::<T>::TryStateViolation
            );
            Ok(())
        }
    }
}
