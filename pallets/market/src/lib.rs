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

#[frame_support::pallet]
pub mod pallet {
    use crate::weights::WeightInfo;
    use core::marker::PhantomData;
    use frame_support::{pallet_prelude::*, traits::Contains, PalletId};
    use frame_system::pallet_prelude::*;
    use futarchy_primitives::{
        bounds,
        keeper::{CrankClass, KeeperRebateSink},
        Balance, Branch, EpochId, FixedU64, GateType, MarketId, MarketKind, PositionId, ProposalId,
        ScalarSide, TradeSide,
    };
    use market_core::{BookKind, MarketBook, MarketParams, MarketPhase};
    use sp_runtime::{
        traits::{AccountIdConversion, Saturating, UniqueSaturatedInto},
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

    /// Epoch-to-Baseline-book lookup (02 §7.4, frozen name).
    #[pallet::storage]
    pub type BaselineMarketOf<T: Config> =
        StorageMap<_, Blake2_128Concat, EpochId, MarketId, OptionQuery>;

    /// Block at which a book closed, used by the archive-delay reap gate.
    #[pallet::storage]
    pub type ClosedAt<T: Config> =
        StorageMap<_, Blake2_128Concat, MarketId, BlockNumberFor<T>, OptionQuery>;

    /// Markets whose POL headroom has already been seeded (04 §10). Guards `seed`
    /// against re-splitting POL into an already-collateralized book (idempotence);
    /// removed at reap.
    #[pallet::storage]
    pub type SeededMarkets<T: Config> = StorageMap<_, Blake2_128Concat, MarketId, (), OptionQuery>;

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
            let mut book = Markets::<T>::get(market).ok_or(Error::<T>::UnknownMarket)?;
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
            let mut book = Markets::<T>::get(market).ok_or(Error::<T>::UnknownMarket)?;
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
            let mut book = Markets::<T>::get(market).ok_or(Error::<T>::UnknownMarket)?;
            // The accumulator is sealed at Close (04 §2): a permissionless keeper must
            // not record observations on a Closed/Settled book (it would mutate the
            // frozen TWAP). buy/sell already gate on `ensure_trading`; this closes the
            // standalone crank path.
            ensure!(
                matches!(book.phase, MarketPhase::Trading | MarketPhase::Extended),
                Error::<T>::NotTrading
            );
            if let Some(event) =
                market_core::observe_book(&mut book, &Self::params(), Self::now_u64())
                    .map_err(Error::<T>::from)?
            {
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
            let closed = ClosedAt::<T>::get(market).ok_or(Error::<T>::NotReapable)?;
            ensure!(
                frame_system::Pallet::<T>::block_number()
                    >= closed.saturating_add(<T as Config>::ArchiveDelay::get()),
                Error::<T>::NotReapable
            );
            if let BookKind::Baseline { epoch } = book.kind {
                if BaselineMarketOf::<T>::get(epoch) == Some(market) {
                    BaselineMarketOf::<T>::remove(epoch);
                }
            }
            Markets::<T>::remove(market);
            ClosedAt::<T>::remove(market);
            SeededMarkets::<T>::remove(market);
            Self::deposit_event(Event::MarketReaped { market });
            <T as Config>::KeeperRebate::rebate(&who, CrankClass::General);
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
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
            let book = Markets::<T>::get(id).ok_or(Error::<T>::UnknownMarket)?;
            // Seed once: re-seeding splits fresh POL headroom into an already
            // collateralized book (04 §10), double-spending the subsidy.
            ensure!(
                !SeededMarkets::<T>::contains_key(id),
                Error::<T>::AlreadySeeded
            );
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
                Self::deposit_event(Event::Seeded {
                    market: id,
                    headroom,
                });
                Ok(())
            })
        }

        /// Internal epoch-authority API: close a book and start its archive delay.
        pub fn close(origin: OriginFor<T>, id: MarketId) -> DispatchResult {
            T::MarketAdmin::ensure_origin(origin).map_err(|_| Error::<T>::BadOrigin)?;
            Markets::<T>::try_mutate(id, |maybe_book| -> DispatchResult {
                let book = maybe_book.as_mut().ok_or(Error::<T>::UnknownMarket)?;
                book.phase = MarketPhase::Closed;
                Ok(())
            })?;
            ClosedAt::<T>::insert(id, frame_system::Pallet::<T>::block_number());
            Self::deposit_event(Event::MarketClosed { market: id });
            Ok(())
        }

        /// Market sovereign account used to sign the ledger's internal API.
        pub fn account_id() -> T::AccountId {
            <T as Config>::PalletId::get().into_account_truncating()
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
            for (_id, book) in Markets::<T>::iter() {
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
                ensure!(vault_exists, Error::<T>::TryStateViolation);
                // I-13 (accumulator sanity): no future observation, and the
                // price-weighted sum ≤ max-price × elapsed blocks.
                ensure!(
                    book.last_observed_block <= now,
                    Error::<T>::TryStateViolation
                );
                let max_accum = u128::from(book.last_observed_block)
                    .checked_mul(u128::from(market_core::PRICE_ONE_1E9))
                    .ok_or(Error::<T>::TryStateViolation)?;
                ensure!(
                    book.cumulative_price_blocks <= max_accum,
                    Error::<T>::TryStateViolation
                );
            }
            ensure!(
                Markets::<T>::count() <= bounds::MAX_LIVE_MARKETS,
                Error::<T>::TryStateViolation
            );
            for (epoch, market) in BaselineMarketOf::<T>::iter() {
                let book = Markets::<T>::get(market).ok_or(Error::<T>::TryStateViolation)?;
                ensure!(
                    matches!(book.kind, BookKind::Baseline { epoch: e } if e == epoch),
                    Error::<T>::TryStateViolation
                );
            }
            Ok(())
        }
    }
}
