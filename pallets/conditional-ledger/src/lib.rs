#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! # `pallet-conditional-ledger`
//!
//! Production FRAME wrapper (Track-A DoD) over the frame-free functional core
//! [`conditional_ledger_core`]. The core is the single implementation of the
//! ledger state machine, conservation arithmetic and rounding discipline (03
//! §2/§6) and doubles as the Python-M3 ≡ Rust-core ≡ FRAME differential oracle;
//! this pallet adds only the runtime-facing surface it is responsible for:
//!
//! * bounded `#[pallet::storage]` matching the 02 §7.4 / 03 §4 names and shapes;
//! * origin-checked `#[pallet::call]` extrinsics (Signed for the public
//!   mint/transfer/redeem families, `ResolveAuthority`/`SettleAuthority` for the
//!   authority calls — 03 §5);
//! * the `MarketAuthority`-gated internal API the D-3 trade wrapper consumes
//!   (03 §5.5, no extrinsic surface);
//! * real USDC custody via `T::Collateral` (`fungibles::Mutate`) — escrow and
//!   position-storage-deposit moves are derived from the state delta the core
//!   produces, so the ledger and the sovereign balance can never disagree
//!   (invariant L-2);
//! * keeper-cranked reaping (`sweep_dust`/`sweep_dust_baseline`, 03 §5.4/§7.2) —
//!   the one behaviour the core does not model, implemented here against the
//!   `Positions` prefixes;
//! * the mandatory `try_state` hook (03 §9, 15 §1); **no block hooks** (03 §10).
//!
//! Every mutating call loads only the bounded set of storage cells the core op
//! can touch (a proposal vault has 14 `PositionId`s, a Baseline vault 2), runs
//! the core op — which is atomic and rolls back internally on any failure — and
//! persists the post-image. FRAME wraps every `#[pallet::call]` dispatch in
//! [`frame_support::storage::with_storage_layer`], so returning **any** error —
//! including a late `T::Collateral::transfer` failure — rolls the already-persisted
//! storage changes back with it; the core post-image can never outlive its custody
//! move (G-1). `#[transactional]` would only nest a redundant second layer.

extern crate alloc;

pub use conditional_ledger_core as core_ledger;
pub use pallet::*;
pub use weights::WeightInfo;

pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

/// Runtime fixture hooks for benchmarks whose measured path pays a keeper
/// rebate through a sibling pallet. Mock runtimes may keep the defaults.
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
    use alloc::{collections::BTreeMap, vec::Vec};
    use conditional_ledger_core::{
        baseline as baseline_id, position as proposal_position, BaselineVaultInfo,
        Event as CoreEvent, LedgerOrigin, LedgerState, VaultInfo,
    };
    use frame_support::{
        pallet_prelude::*,
        traits::{
            fungibles::{self, Mutate},
            tokens::Preservation,
            Contains,
        },
        PalletId,
    };
    use frame_system::pallet_prelude::*;
    use futarchy_primitives::{
        keeper::{CrankClass, KeeperRebateSink},
        kernel, Balance, Branch, EpochId, FixedU64, GateType, MetricSpecVersion, PositionId,
        PositionKind, ProposalId, ScalarSide,
    };
    use sp_runtime::{traits::AccountIdConversion, Saturating};

    /// The concrete asset identifier the configured collateral fungible uses. The
    /// ledger never names it (rule 7 — no XCM types here); the runtime pins it to
    /// the USDC `Location` via [`Config::UsdcAssetId`].
    pub type AssetIdOf<T> = <<T as Config>::Collateral as fungibles::Inspect<
        <T as frame_system::Config>::AccountId,
    >>::AssetId;

    /// Per-proposal instrument fan-out (03 §2.1): 2 branches × 7 kinds = 14 ids.
    pub(crate) const KINDS: [PositionKind; 7] = [
        PositionKind::BranchUsdc,
        PositionKind::Long,
        PositionKind::Short,
        PositionKind::GateYes(GateType::Survival),
        PositionKind::GateNo(GateType::Survival),
        PositionKind::GateYes(GateType::Security),
        PositionKind::GateNo(GateType::Security),
    ];

    #[pallet::config]
    pub trait Config: frame_system::Config<RuntimeEvent: From<Event<Self>>> {
        /// USDC collateral (the `ForeignAssets` instance in production, 03 §1/§3).
        /// Balances are the shared `u128` kernel `Balance`.
        type Collateral: fungibles::Mutate<Self::AccountId>
            + fungibles::Inspect<Self::AccountId, Balance = Balance>;

        /// The asset id of USDC inside [`Config::Collateral`] (the USDC `Location`).
        type UsdcAssetId: Get<AssetIdOf<Self>>;

        /// Internal — `pallet-market` only: the D-3 wrapper's ledger operations and
        /// vault creation (03 §5.5).
        type MarketAuthority: EnsureOrigin<Self::RuntimeOrigin>;

        /// Internal — `pallet-epoch` only: `resolve()` and `void()` (03 §5.2).
        type ResolveAuthority: EnsureOrigin<Self::RuntimeOrigin>;

        /// Internal — the single welfare→ledger settlement path:
        /// `settle_scalar`/`settle_gate`/`settle_baseline` (03 §5.2).
        type SettleAuthority: EnsureOrigin<Self::RuntimeOrigin>;

        /// `MinSplit = MinTransfer = ledger.min_split` (13 §1; K floor
        /// `kernel::MIN_SPLIT_USDC`). Wired to `pallet-constitution::Params` in the
        /// runtime; the core enforces the K floor as a backstop.
        #[pallet::constant]
        type MinSplit: Get<Balance>;

        /// `ledger.position_deposit = 0.1 USDC` per `Positions` entry (13 §4).
        #[pallet::constant]
        type PositionDeposit: Get<Balance>;

        /// `MaxPositionsPerAccount = 64`, counter-enforced for non-protocol
        /// accounts (13 §4). The core enforces the same K value mid-op for atomicity.
        #[pallet::constant]
        type MaxPositionsPerAccount: Get<u32>;

        /// `ledger.archive_delay` (13 §1, default 1 yr): a terminal vault is
        /// reap-eligible only once this many blocks have elapsed since it settled.
        #[pallet::constant]
        type ArchiveDelay: Get<BlockNumberFor<Self>>;

        /// `ReapBatch = 100` (13 §4): max `Positions` entries drained per
        /// `sweep_dust*` call.
        #[pallet::constant]
        type ReapBatch: Get<u32>;

        /// POL / book / treasury-sub / INSURANCE accounts — exempt from the
        /// position cap and the storage deposit (03 §3/§4).
        type ProtocolAccounts: Contains<Self::AccountId>;

        /// Destination for swept residue (rounding dust + unredeemed-after-archive):
        /// the INSURANCE sub-account (03 §7 R-5).
        type InsuranceAccount: Get<Self::AccountId>;

        /// The ledger's own `PalletId`; its derived sovereign account custodies all
        /// escrow and held deposits (03 §1).
        #[pallet::constant]
        type PalletId: Get<PalletId>;

        /// Fail-soft keeper rebate sink (08 §6). It must never affect a crank.
        type KeeperRebate: KeeperRebateSink<Self::AccountId>;

        /// Benchmarked weights.
        type WeightInfo: WeightInfo;

        /// Cross-pallet keeper-rebate fixture used only by runtime benchmarks.
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: crate::BenchmarkHelper;
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    // ------------------------------------------------------------------ storage

    /// Proposal vaults — `map ProposalId → VaultInfo` (03 §4; `VaultInfo` ≤ 224 B).
    /// Count-bounded to `MaxLiveProposals(=32) + settling cohorts` by the pallets
    /// that create vaults (there is no structural map bound; each value is
    /// `MaxEncodedLen`).
    #[pallet::storage]
    pub type Vaults<T: Config> =
        StorageMap<_, Blake2_128Concat, ProposalId, VaultInfo, OptionQuery>;

    /// Baseline vaults — `map EpochId → BaselineVaultInfo` (03 §4; ≤ 64 B).
    #[pallet::storage]
    pub type BaselineVaults<T: Config> =
        StorageMap<_, Blake2_128Concat, EpochId, BaselineVaultInfo, OptionQuery>;

    /// Positions — `double_map (PositionId, AccountId) → Balance` (02 §7.4 / 03 §4).
    /// Key order is `(PositionId, AccountId)` so per-vault reaping drains a prefix.
    /// Global growth is priced by [`Config::PositionDeposit`] (the economic bound).
    #[pallet::storage]
    pub type Positions<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        PositionId,
        Blake2_128Concat,
        T::AccountId,
        Balance,
        ValueQuery,
    >;

    /// Live `Positions` entries per account — `map AccountId → u32`, ≤
    /// `MaxPositionsPerAccount` for non-protocol accounts (03 §4, L-6).
    #[pallet::storage]
    pub type PositionCount<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, u32, ValueQuery>;

    /// Outstanding supply per instrument — `map PositionId → Balance` (03 §4).
    #[pallet::storage]
    pub type PositionTotals<T: Config> =
        StorageMap<_, Blake2_128Concat, PositionId, Balance, ValueQuery>;

    /// Total position storage deposits currently held by the sovereign account,
    /// accounted strictly outside `escrowed` (03 §4, L-2/L-6).
    #[pallet::storage]
    pub type DepositsHeld<T: Config> = StorageValue<_, Balance, ValueQuery>;

    /// Block at which a proposal vault entered a terminal state, for the
    /// `sweep_dust` archive-delay gate (03 §4/§5.4). Ledger-internal; not a FE
    /// surface.
    #[pallet::storage]
    pub type VaultTerminalAt<T: Config> =
        StorageMap<_, Blake2_128Concat, ProposalId, BlockNumberFor<T>, OptionQuery>;

    /// Block at which a Baseline vault settled, for `sweep_dust_baseline`.
    #[pallet::storage]
    pub type BaselineTerminalAt<T: Config> =
        StorageMap<_, Blake2_128Concat, EpochId, BlockNumberFor<T>, OptionQuery>;

    // -------------------------------------------------------------------- events

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// `split(pid, a)`: minted `a` of both branch-USDC to the caller.
        Split { pid: ProposalId, amount: Balance },
        /// `merge(pid, a)`: burned both branch-USDC, paid `a` USDC out.
        Merged { pid: ProposalId, amount: Balance },
        /// `split_scalar(pid, b, a)`.
        ScalarSplit {
            pid: ProposalId,
            branch: Branch,
            amount: Balance,
        },
        /// `merge_scalar(pid, b, a)`.
        ScalarMerged {
            pid: ProposalId,
            branch: Branch,
            amount: Balance,
        },
        /// `split_gate(pid, b, g, a)`.
        GateSplit {
            pid: ProposalId,
            branch: Branch,
            gate: GateType,
            amount: Balance,
        },
        /// `merge_gate(pid, b, g, a)`.
        GateMerged {
            pid: ProposalId,
            branch: Branch,
            gate: GateType,
            amount: Balance,
        },
        /// `transfer(position, to, a)`.
        PositionTransferred {
            position: PositionId,
            amount: Balance,
        },
        /// `split_baseline(epoch, a)`.
        BaselineSplit { epoch: EpochId, amount: Balance },
        /// `merge_baseline(epoch, a)`.
        BaselineMerged { epoch: EpochId, amount: Balance },
        /// `resolve(pid, w)` — winning branch (02 §6).
        VaultResolved { pid: ProposalId, branch: Branch },
        /// `void(pid)` (02 §6, D-1/X-11f).
        VaultVoided { pid: ProposalId },
        /// `settle_scalar(pid, s)` — carries the winning branch (02 §6, B-low).
        ScalarSettlementSet {
            pid: ProposalId,
            branch: Branch,
            s: FixedU64,
        },
        /// `settle_gate(pid, g, outcome)` — winning-branch breach outcome (02 §6, B-2).
        GateSettled {
            pid: ProposalId,
            branch: Branch,
            gate: GateType,
            outcome: bool,
        },
        /// `settle_baseline(epoch, s)`.
        BaselineSettled { epoch: EpochId, s: FixedU64 },
        /// `redeem(pid, a)`.
        Redeemed { pid: ProposalId, amount: Balance },
        /// `redeem_scalar(pid, kind, a)` — `payout` is the post-rounding amount.
        ScalarRedeemed {
            pid: ProposalId,
            side: ScalarSide,
            payout: Balance,
        },
        /// `redeem_scalar_pair(pid, a)` (02 §6, B-5).
        ScalarPairRedeemed { pid: ProposalId, amount: Balance },
        /// `redeem_gate(pid, g, a)`.
        GateRedeemed {
            pid: ProposalId,
            gate: GateType,
            amount: Balance,
        },
        /// `redeem_void(pid, kind, a)` (02 §6, D-1) — `amount` burned, `payout` paid.
        VoidRedeemed {
            pid: ProposalId,
            kind: PositionKind,
            amount: Balance,
            payout: Balance,
        },
        /// `redeem_baseline*`.
        BaselineRedeemed {
            epoch: EpochId,
            side: ScalarSide,
            payout: Balance,
        },
        /// `sweep_dust(pid)` completed — residual escrow swept to INSURANCE (02 §6).
        VaultReaped { pid: ProposalId, residue: Balance },
        /// `sweep_dust_baseline(epoch)` completed.
        BaselineVaultReaped { epoch: EpochId, residue: Balance },
    }

    // -------------------------------------------------------------------- errors

    #[pallet::error]
    pub enum Error<T> {
        /// Origin was not the internal authority the call requires (defensive; the
        /// pallet checks origins before the core, so the happy path never sees it).
        BadOrigin,
        /// No proposal vault exists for the given id.
        UnknownVault,
        /// No Baseline vault exists for the given epoch.
        UnknownBaselineVault,
        /// The vault/Baseline vault is not in a state that admits this operation
        /// (03 §2.3 transition table; the coarse status-quo default, G-1).
        WrongVaultState,
        /// Amount is below `MinSplit`/`MinTransfer` (03 §7 R-2).
        BelowMinimum,
        /// Checked conservation arithmetic overflowed (03 §6/§8).
        ArithmeticOverflow,
        /// Caller does not hold enough of the required instrument.
        InsufficientPosition,
        /// Creating the entry would exceed `MaxPositionsPerAccount` (03 §4).
        TooManyPositions,
        /// Settlement score `s` is outside `[0, 1]` (1e9 scale).
        InvalidScore,
        /// The gate outcome for this gate is already recorded.
        GateAlreadySettled,
        /// The gate outcome for this gate is not yet recorded.
        GateNotSettled,
        /// Branch/side mismatch on a redemption.
        WrongBranch,
        /// A conservation invariant was violated (surfaces only from the core's
        /// internal consistency guards; try-state maps drift to I-4).
        TryStateViolation,
        /// The vault is not yet reap-eligible: not terminal, or `ArchiveDelay` has
        /// not elapsed (03 §5.4).
        ReapNotDue,
        /// The position-storage deposit could not be taken from the entry owner
        /// (03 §4 / §8).
        DepositFailed,
    }

    impl<T: Config> From<conditional_ledger_core::Error> for Error<T> {
        fn from(e: conditional_ledger_core::Error) -> Self {
            use conditional_ledger_core::Error as C;
            match e {
                C::BadOrigin => Error::BadOrigin,
                C::UnknownVault => Error::UnknownVault,
                C::UnknownBaselineVault => Error::UnknownBaselineVault,
                C::WrongVaultState => Error::WrongVaultState,
                C::AmountTooSmall => Error::BelowMinimum,
                C::ArithmeticOverflow => Error::ArithmeticOverflow,
                C::InsufficientPosition => Error::InsufficientPosition,
                C::PositionCapExceeded => Error::TooManyPositions,
                C::InvalidScore => Error::InvalidScore,
                C::GateAlreadySettled => Error::GateAlreadySettled,
                C::GateNotSettled => Error::GateNotSettled,
                C::WrongBranch => Error::WrongBranch,
                C::TryStateViolation => Error::TryStateViolation,
            }
        }
    }

    // --------------------------------------------------------------------- hooks

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// `MaxPositionsPerAccount` is the fixed 13 §4 bound (64) that the frame-free
        /// core enforces mid-op; the Config `Get` (03 §3 surface) must equal it or a
        /// runtime could advertise a cap the core silently ignores. The position
        /// deposit is instead single-sourced from `T::PositionDeposit` throughout
        /// custody AND `DepositsHeld` accounting, so it stays a live META tunable
        /// (13 §1) without diverging — no pin needed there.
        fn integrity_test() {
            assert_eq!(
                T::MaxPositionsPerAccount::get(),
                conditional_ledger_core::MAX_POSITIONS_PER_ACCOUNT,
                "Config::MaxPositionsPerAccount must equal the core's cap (03 §4 / 13 §4)"
            );
        }

        /// The ledger does **no** automatic per-block work (03 §10; I-20 trivial).
        /// Only `try_state` is implemented (15 §1).
        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), sp_runtime::TryRuntimeError> {
            Self::do_try_state()
        }
    }

    // ---------------------------------------------------------------------- calls

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// 03 §5.1. Split `a` USDC into `a` Accept-USDC + `a` Reject-USDC.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::split())]
        pub fn split(origin: OriginFor<T>, pid: ProposalId, amount: Balance) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(amount >= T::MinSplit::get(), Error::<T>::BelowMinimum);
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.split(LedgerOrigin::Signed, pid, &who, amount)
            })
        }

        /// 03 §5.1. Burn a complete Accept+Reject pair, pay `a` USDC out (par).
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::merge())]
        pub fn merge(origin: OriginFor<T>, pid: ProposalId, amount: Balance) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.merge(LedgerOrigin::Signed, pid, &who, amount)
            })
        }

        /// 03 §5.1. Split branch-USDC into a LONG/SHORT scalar set.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::split_scalar())]
        pub fn split_scalar(
            origin: OriginFor<T>,
            pid: ProposalId,
            branch: Branch,
            amount: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            // 03 §7 R-2 creation floor at the LIVE `ledger.min_split`: the minted
            // LONG/SHORT legs are new non-protocol entries, which R-2 forbids below
            // the floor even though the §5.1 row omits it. The core guards only the
            // stale compile-time K floor; `do_split_scalar` (`MarketAuthority`,
            // exact by construction) stays exempt.
            ensure!(amount >= T::MinSplit::get(), Error::<T>::BelowMinimum);
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.split_scalar(LedgerOrigin::Signed, pid, branch, &who, amount)
            })
        }

        /// 03 §5.1. Merge a LONG/SHORT set back to branch-USDC.
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::merge_scalar())]
        pub fn merge_scalar(
            origin: OriginFor<T>,
            pid: ProposalId,
            branch: Branch,
            amount: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.merge_scalar(LedgerOrigin::Signed, pid, branch, &who, amount)
            })
        }

        /// 03 §5.1. Split branch-USDC into a gate YES/NO set.
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::split_gate())]
        pub fn split_gate(
            origin: OriginFor<T>,
            pid: ProposalId,
            branch: Branch,
            gate: GateType,
            amount: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            // 03 §7 R-2 creation floor at the LIVE `ledger.min_split` (as
            // `split_scalar`): the minted gate legs are new non-protocol entries.
            ensure!(amount >= T::MinSplit::get(), Error::<T>::BelowMinimum);
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.split_gate(LedgerOrigin::Signed, pid, branch, gate, &who, amount)
            })
        }

        /// 03 §5.1. Merge a gate YES/NO set back to branch-USDC.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::merge_gate())]
        pub fn merge_gate(
            origin: OriginFor<T>,
            pid: ProposalId,
            branch: Branch,
            gate: GateType,
            amount: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.merge_gate(LedgerOrigin::Signed, pid, branch, gate, &who, amount)
            })
        }

        /// 03 §5.1. Move `a` of a position to another account. The recipient pays
        /// the storage deposit (03 §4); the R-2 remainder sweep applies to Signed
        /// senders.
        #[pallet::call_index(6)]
        #[pallet::weight(T::WeightInfo::transfer())]
        pub fn transfer(
            origin: OriginFor<T>,
            position: PositionId,
            to: T::AccountId,
            amount: Balance,
        ) -> DispatchResult {
            let from = ensure_signed(origin)?;
            let (pid, epoch, is_proposal) = Self::id_home(position);
            // 03 §7 R-2, both sub-rules enforced at the LIVE `ledger.min_split` (the
            // core only guards the compile-time K floor, which is stale once META
            // raises the tunable up to its 1-USDC ceiling — 13 §1):
            let mut amount = amount;
            // (b) remainder sweep — a Signed transfer that would strand `from` a
            // sub-`min_split` remainder MUST move the whole balance. Computed first so
            // the creation floor sees the amount that actually moves; the core's own
            // sweep (at the K floor) is then a no-op. Protocol senders are exact by
            // construction and exempt (as in the core).
            if !T::ProtocolAccounts::contains(&from) {
                let bal = Positions::<T>::get(position, &from);
                let remainder = bal.saturating_sub(amount);
                if remainder > 0 && remainder < T::MinSplit::get() {
                    amount = bal;
                }
            }
            // (a) creation floor — a new non-protocol entry cannot be created below it.
            if !Positions::<T>::contains_key(position, &to) && !T::ProtocolAccounts::contains(&to) {
                ensure!(amount >= T::MinSplit::get(), Error::<T>::BelowMinimum);
            }
            // Escrow never moves on transfer; `from` is a valid (unused) escrow party.
            // The account set MUST be deduplicated: a self-transfer (`from == to`)
            // would otherwise hydrate/persist/settle the same cell twice and
            // double-refund the deposit (an escrow drain).
            let accts = Self::distinct(&from, &to);
            let run = |st: &mut LedgerState<T::AccountId>| {
                st.transfer(LedgerOrigin::Signed, position, &from, &to, amount)
            };
            if is_proposal {
                Self::run_proposal(pid, &accts, &from, run)
            } else {
                Self::run_baseline(epoch, &accts, &from, run)
            }
        }

        /// 03 §5.1. Baseline split.
        #[pallet::call_index(7)]
        #[pallet::weight(T::WeightInfo::split_baseline())]
        pub fn split_baseline(
            origin: OriginFor<T>,
            epoch: EpochId,
            amount: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(amount >= T::MinSplit::get(), Error::<T>::BelowMinimum);
            Self::run_baseline(epoch, core::slice::from_ref(&who), &who, |st| {
                st.split_baseline(LedgerOrigin::Signed, epoch, &who, amount)
            })
        }

        /// 03 §5.1. Baseline merge.
        #[pallet::call_index(8)]
        #[pallet::weight(T::WeightInfo::merge_baseline())]
        pub fn merge_baseline(
            origin: OriginFor<T>,
            epoch: EpochId,
            amount: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::run_baseline(epoch, core::slice::from_ref(&who), &who, |st| {
                st.merge_baseline(LedgerOrigin::Signed, epoch, &who, amount)
            })
        }

        // ---- 03 §5.2 authority calls -------------------------------------------

        /// 03 §5.2. `Open → Resolved(w)` (`ResolveAuthority`, exactly once, I-3).
        #[pallet::call_index(9)]
        #[pallet::weight(T::WeightInfo::resolve())]
        pub fn resolve(origin: OriginFor<T>, pid: ProposalId, winner: Branch) -> DispatchResult {
            T::ResolveAuthority::ensure_origin(origin)?;
            Self::run_proposal_authority(pid, |st| {
                st.resolve(LedgerOrigin::ResolveAuthority, pid, winner)
            })
        }

        /// 03 §5.2. `Open|Resolved → Voided` (`ResolveAuthority`, not from
        /// `ScalarSettled`). Records the terminal block for reaping.
        #[pallet::call_index(10)]
        #[pallet::weight(T::WeightInfo::void())]
        pub fn void(origin: OriginFor<T>, pid: ProposalId) -> DispatchResult {
            T::ResolveAuthority::ensure_origin(origin)?;
            Self::run_proposal_authority(pid, |st| st.void(LedgerOrigin::ResolveAuthority, pid))?;
            VaultTerminalAt::<T>::insert(pid, frame_system::Pallet::<T>::block_number());
            Ok(())
        }

        /// 03 §5.2. `Resolved(w) → ScalarSettled{w,s}` (`SettleAuthority`).
        #[pallet::call_index(11)]
        #[pallet::weight(T::WeightInfo::settle_scalar())]
        pub fn settle_scalar(origin: OriginFor<T>, pid: ProposalId, s: FixedU64) -> DispatchResult {
            T::SettleAuthority::ensure_origin(origin)?;
            Self::run_proposal_authority(pid, |st| {
                st.settle_scalar(LedgerOrigin::SettleAuthority, pid, s)
            })?;
            VaultTerminalAt::<T>::insert(pid, frame_system::Pallet::<T>::block_number());
            Ok(())
        }

        /// 03 §5.2. Record a winning-branch gate breach outcome (`SettleAuthority`).
        #[pallet::call_index(12)]
        #[pallet::weight(T::WeightInfo::settle_gate())]
        pub fn settle_gate(
            origin: OriginFor<T>,
            pid: ProposalId,
            gate: GateType,
            outcome: bool,
        ) -> DispatchResult {
            T::SettleAuthority::ensure_origin(origin)?;
            Self::run_proposal_authority(pid, |st| {
                st.settle_gate(LedgerOrigin::SettleAuthority, pid, gate, outcome)
            })
        }

        /// 03 §5.2. Settle a Baseline vault (`SettleAuthority`).
        #[pallet::call_index(13)]
        #[pallet::weight(T::WeightInfo::settle_baseline())]
        pub fn settle_baseline(
            origin: OriginFor<T>,
            epoch: EpochId,
            s: FixedU64,
        ) -> DispatchResult {
            T::SettleAuthority::ensure_origin(origin)?;
            Self::run_baseline_authority(epoch, |st| {
                st.settle_baseline(LedgerOrigin::SettleAuthority, epoch, s)
            })?;
            BaselineTerminalAt::<T>::insert(epoch, frame_system::Pallet::<T>::block_number());
            Ok(())
        }

        // ---- 03 §5.3 redemption calls (terminal states only) --------------------

        /// 03 §5.3. Redeem winning branch-USDC 1:1 (`ScalarSettled`).
        #[pallet::call_index(14)]
        #[pallet::weight(T::WeightInfo::redeem())]
        pub fn redeem(origin: OriginFor<T>, pid: ProposalId, amount: Balance) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.redeem(pid, &who, amount)
            })
        }

        /// 03 §5.3. Redeem a single scalar leg with maker-adverse flooring (B-5).
        #[pallet::call_index(15)]
        #[pallet::weight(T::WeightInfo::redeem_scalar())]
        pub fn redeem_scalar(
            origin: OriginFor<T>,
            pid: ProposalId,
            side: ScalarSide,
            amount: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.redeem_scalar(pid, side, &who, amount)
            })
        }

        /// 03 §5.3. Redeem a complete LONG+SHORT pair for exactly `a` (no double
        /// flooring, R-1).
        #[pallet::call_index(16)]
        #[pallet::weight(T::WeightInfo::redeem_scalar_pair())]
        pub fn redeem_scalar_pair(
            origin: OriginFor<T>,
            pid: ProposalId,
            amount: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.redeem_scalar_pair(pid, &who, amount)
            })
        }

        /// 03 §5.3. Redeem the winning side of a settled gate 1:1.
        #[pallet::call_index(17)]
        #[pallet::weight(T::WeightInfo::redeem_gate())]
        pub fn redeem_gate(
            origin: OriginFor<T>,
            pid: ProposalId,
            gate: GateType,
            amount: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.redeem_gate(pid, gate, &who, amount)
            })
        }

        /// 03 §5.3. VOID redemption: branch-USDC `floor(a/2)`, legs `floor(a/4)`.
        #[pallet::call_index(18)]
        #[pallet::weight(T::WeightInfo::redeem_void())]
        pub fn redeem_void(
            origin: OriginFor<T>,
            pid: ProposalId,
            branch: Branch,
            kind: PositionKind,
            amount: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.redeem_void(pid, branch, kind, &who, amount)
            })
        }

        /// 03 §5.3. Redeem a single Baseline leg.
        #[pallet::call_index(19)]
        #[pallet::weight(T::WeightInfo::redeem_baseline())]
        pub fn redeem_baseline(
            origin: OriginFor<T>,
            epoch: EpochId,
            side: ScalarSide,
            amount: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::run_baseline(epoch, core::slice::from_ref(&who), &who, |st| {
                st.redeem_baseline(epoch, side, &who, amount)
            })
        }

        /// 03 §5.3. Redeem a complete Baseline pair for exactly `a`.
        #[pallet::call_index(20)]
        #[pallet::weight(T::WeightInfo::redeem_baseline_pair())]
        pub fn redeem_baseline_pair(
            origin: OriginFor<T>,
            epoch: EpochId,
            amount: Balance,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::run_baseline(epoch, core::slice::from_ref(&who), &who, |st| {
                st.redeem_baseline_pair(epoch, &who, amount)
            })
        }

        // ---- 03 §5.4 housekeeping ----------------------------------------------

        /// 03 §5.4. Keeper crank: drain ≤ `ReapBatch` `Positions` entries of a
        /// terminal, archive-elapsed proposal vault, refunding deposits; when fully
        /// drained, sweep residual escrow to INSURANCE and remove the vault.
        #[pallet::call_index(21)]
        #[pallet::weight(T::WeightInfo::sweep_dust())]
        pub fn sweep_dust(origin: OriginFor<T>, pid: ProposalId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let advanced = Self::do_sweep_proposal(pid)?;
            if advanced {
                // B5 recalibrates this weight for the rebate sink's treasury writes.
                T::KeeperRebate::rebate(&who, CrankClass::General);
            }
            Ok(())
        }

        /// 03 §5.4. Keeper crank for Baseline vaults.
        #[pallet::call_index(22)]
        #[pallet::weight(T::WeightInfo::sweep_dust_baseline())]
        pub fn sweep_dust_baseline(origin: OriginFor<T>, epoch: EpochId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let advanced = Self::do_sweep_baseline(epoch)?;
            if advanced {
                // B5 recalibrates this weight for the rebate sink's treasury writes.
                T::KeeperRebate::rebate(&who, CrankClass::General);
            }
            Ok(())
        }
    }

    // ------------------------------------------- internal MarketAuthority API (§5.5)

    impl<T: Config> Pallet<T> {
        /// Create an `Open` proposal vault at book-seed time. `MarketAuthority`
        /// only (03 §5.5; vault creation rides book deployment).
        pub fn create_vault(
            origin: OriginFor<T>,
            pid: ProposalId,
            spec: MetricSpecVersion,
        ) -> DispatchResult {
            T::MarketAuthority::ensure_origin(origin)?;
            ensure!(!Vaults::<T>::contains_key(pid), Error::<T>::WrongVaultState);
            Vaults::<T>::insert(pid, VaultInfo::open(spec));
            Ok(())
        }

        /// Create an `Open` Baseline vault at epoch Baseline-book seed. `MarketAuthority`.
        pub fn create_baseline_vault(origin: OriginFor<T>, epoch: EpochId) -> DispatchResult {
            T::MarketAuthority::ensure_origin(origin)?;
            ensure!(
                !BaselineVaults::<T>::contains_key(epoch),
                Error::<T>::WrongVaultState
            );
            BaselineVaults::<T>::insert(epoch, BaselineVaultInfo::open());
            Ok(())
        }

        /// D-3 wrapper buy leg: `split` on behalf of `who`. `MarketAuthority` only.
        pub fn do_split(
            origin: OriginFor<T>,
            pid: ProposalId,
            who: T::AccountId,
            amount: Balance,
        ) -> DispatchResult {
            T::MarketAuthority::ensure_origin(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.do_split(pid, &who, amount)
            })
        }

        /// D-3 wrapper: move book/mirror branch-USDC. `MarketAuthority` only.
        pub fn do_transfer(
            origin: OriginFor<T>,
            position: PositionId,
            from: T::AccountId,
            to: T::AccountId,
            amount: Balance,
        ) -> DispatchResult {
            T::MarketAuthority::ensure_origin(origin)?;
            let (pid, epoch, is_proposal) = Self::id_home(position);
            let accts = Self::distinct(&from, &to);
            let run =
                |st: &mut LedgerState<T::AccountId>| st.do_transfer(position, &from, &to, amount);
            if is_proposal {
                Self::run_proposal(pid, &accts, &from, run)
            } else {
                Self::run_baseline(epoch, &accts, &from, run)
            }
        }

        /// D-3 wrapper: recycle book revenue into complete scalar sets. `MarketAuthority`.
        pub fn do_split_scalar(
            origin: OriginFor<T>,
            pid: ProposalId,
            branch: Branch,
            who: T::AccountId,
            amount: Balance,
        ) -> DispatchResult {
            T::MarketAuthority::ensure_origin(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.do_split_scalar(pid, branch, &who, amount)
            })
        }

        /// D-3 wrapper: recycle gate-book revenue into complete gate sets. `MarketAuthority`.
        pub fn do_split_gate(
            origin: OriginFor<T>,
            pid: ProposalId,
            branch: Branch,
            gate: GateType,
            who: T::AccountId,
            amount: Balance,
        ) -> DispatchResult {
            T::MarketAuthority::ensure_origin(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.split_gate(
                    LedgerOrigin::MarketAuthority,
                    pid,
                    branch,
                    gate,
                    &who,
                    amount,
                )
            })
        }

        /// D-3 wrapper: Baseline buy leg / revenue recycling. `MarketAuthority`.
        pub fn do_split_baseline(
            origin: OriginFor<T>,
            epoch: EpochId,
            who: T::AccountId,
            amount: Balance,
        ) -> DispatchResult {
            T::MarketAuthority::ensure_origin(origin)?;
            Self::run_baseline(epoch, core::slice::from_ref(&who), &who, |st| {
                st.do_split_baseline(epoch, &who, amount)
            })
        }

        /// D-3 wrapper sell path: `merge` on behalf of `who`. `MarketAuthority`.
        pub fn do_merge(
            origin: OriginFor<T>,
            pid: ProposalId,
            who: T::AccountId,
            amount: Balance,
        ) -> DispatchResult {
            T::MarketAuthority::ensure_origin(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.do_merge(pid, &who, amount)
            })
        }

        /// D-3 wrapper sell path: `merge_scalar`. `MarketAuthority`.
        pub fn do_merge_scalar(
            origin: OriginFor<T>,
            pid: ProposalId,
            branch: Branch,
            who: T::AccountId,
            amount: Balance,
        ) -> DispatchResult {
            T::MarketAuthority::ensure_origin(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.do_merge_scalar(pid, branch, &who, amount)
            })
        }

        /// D-3 wrapper sell path: `merge_gate`. `MarketAuthority`.
        pub fn do_merge_gate(
            origin: OriginFor<T>,
            pid: ProposalId,
            branch: Branch,
            gate: GateType,
            who: T::AccountId,
            amount: Balance,
        ) -> DispatchResult {
            T::MarketAuthority::ensure_origin(origin)?;
            Self::run_proposal(pid, core::slice::from_ref(&who), &who, |st| {
                st.do_merge_gate(pid, branch, gate, &who, amount)
            })
        }

        /// D-3 wrapper sell path: `merge_baseline`. `MarketAuthority`.
        pub fn do_merge_baseline(
            origin: OriginFor<T>,
            epoch: EpochId,
            who: T::AccountId,
            amount: Balance,
        ) -> DispatchResult {
            T::MarketAuthority::ensure_origin(origin)?;
            Self::run_baseline(epoch, core::slice::from_ref(&who), &who, |st| {
                st.do_merge_baseline(epoch, &who, amount)
            })
        }
    }

    // -------------------------------------------------------- scoped-state adapter

    impl<T: Config> Pallet<T> {
        /// The ledger's sovereign account — custodies escrow and held deposits.
        pub fn account_id() -> T::AccountId {
            T::PalletId::get().into_account_truncating()
        }

        fn usdc() -> AssetIdOf<T> {
            T::UsdcAssetId::get()
        }

        fn is_protocol(who: &T::AccountId) -> bool {
            T::ProtocolAccounts::contains(who)
        }

        fn deposit_slot(who: &T::AccountId) -> Balance {
            if Self::is_protocol(who) {
                0
            } else {
                kernel::POSITION_DEPOSIT_USDC
            }
        }

        /// The 14 `PositionId`s of a proposal vault (03 §2.1).
        fn proposal_ids(pid: ProposalId) -> impl Iterator<Item = PositionId> {
            [Branch::Accept, Branch::Reject]
                .into_iter()
                .flat_map(move |b| KINDS.iter().map(move |k| proposal_position(pid, b, *k)))
        }

        /// The 2 `PositionId`s of a Baseline vault.
        fn baseline_ids(epoch: EpochId) -> [PositionId; 2] {
            [
                baseline_id(epoch, ScalarSide::Long),
                baseline_id(epoch, ScalarSide::Short),
            ]
        }

        fn id_home(id: PositionId) -> (ProposalId, EpochId, bool) {
            match id {
                PositionId::Proposal { proposal, .. } => (proposal, 0, true),
                PositionId::Baseline { epoch, .. } => (0, epoch, false),
            }
        }

        /// A deduplicated `{from, to}` account set for the two-party ops. Passing a
        /// duplicated account would hydrate/persist/settle the same storage cell
        /// twice; a self-transfer must therefore scope to a single account.
        fn distinct(from: &T::AccountId, to: &T::AccountId) -> Vec<T::AccountId> {
            if from == to {
                alloc::vec![from.clone()]
            } else {
                alloc::vec![from.clone(), to.clone()]
            }
        }

        /// Load exactly the storage cells a proposal op on `pid` for `accts` can
        /// touch into a fresh [`LedgerState`] (bounded: 14 ids × ≤ 2 accounts).
        fn load_proposal(
            pid: ProposalId,
            accts: &[T::AccountId],
        ) -> Option<LedgerState<T::AccountId>> {
            let info = Vaults::<T>::get(pid)?;
            let mut st = LedgerState::new();
            st.vaults.push(conditional_ledger_core::VaultRecord {
                proposal: pid,
                info,
            });
            st.deposits_held = DepositsHeld::<T>::get();
            Self::hydrate_positions(&mut st, Self::proposal_ids(pid), accts);
            Some(st)
        }

        fn load_baseline(
            epoch: EpochId,
            accts: &[T::AccountId],
        ) -> Option<LedgerState<T::AccountId>> {
            let info = BaselineVaults::<T>::get(epoch)?;
            let mut st = LedgerState::new();
            st.baseline_vaults
                .push(conditional_ledger_core::BaselineVaultRecord { epoch, info });
            st.deposits_held = DepositsHeld::<T>::get();
            Self::hydrate_positions(&mut st, Self::baseline_ids(epoch).into_iter(), accts);
            Some(st)
        }

        fn hydrate_positions(
            st: &mut LedgerState<T::AccountId>,
            ids: impl Iterator<Item = PositionId>,
            accts: &[T::AccountId],
        ) {
            for id in ids {
                let total = PositionTotals::<T>::get(id);
                if total > 0 {
                    st.position_totals
                        .push(conditional_ledger_core::PositionTotal { id, total });
                }
                for who in accts {
                    let bal = Positions::<T>::get(id, who);
                    if bal > 0 {
                        st.positions.push(conditional_ledger_core::PositionRecord {
                            id,
                            owner: who.clone(),
                            balance: bal,
                            deposit: Self::deposit_slot(who),
                        });
                    }
                }
            }
            for who in accts {
                st.position_counts
                    .push(conditional_ledger_core::PositionCount {
                        owner: who.clone(),
                        count: PositionCount::<T>::get(who),
                    });
                if Self::is_protocol(who) {
                    st.add_protocol_account(who.clone());
                }
            }
        }

        /// Persist the post-image of a proposal-scoped op back to storage.
        fn persist_proposal(
            pid: ProposalId,
            accts: &[T::AccountId],
            st: &LedgerState<T::AccountId>,
        ) {
            if let Some(rec) = st.vaults.iter().find(|v| v.proposal == pid) {
                Vaults::<T>::insert(pid, rec.info);
            }
            Self::persist_positions(Self::proposal_ids(pid), accts, st);
        }

        fn persist_baseline(
            epoch: EpochId,
            accts: &[T::AccountId],
            st: &LedgerState<T::AccountId>,
        ) {
            if let Some(rec) = st.baseline_vaults.iter().find(|v| v.epoch == epoch) {
                BaselineVaults::<T>::insert(epoch, rec.info);
            }
            Self::persist_positions(Self::baseline_ids(epoch).into_iter(), accts, st);
        }

        fn persist_positions(
            ids: impl Iterator<Item = PositionId>,
            accts: &[T::AccountId],
            st: &LedgerState<T::AccountId>,
        ) {
            // `DepositsHeld` is maintained by `settle_deposits` from
            // `T::PositionDeposit` × count-delta (single source), not copied from the
            // core's kernel-constant bookkeeping.
            for id in ids {
                let total = st
                    .position_totals
                    .iter()
                    .find(|t| t.id == id)
                    .map_or(0, |t| t.total);
                if total == 0 {
                    PositionTotals::<T>::remove(id);
                } else {
                    PositionTotals::<T>::insert(id, total);
                }
                for who in accts {
                    let bal = st
                        .positions
                        .iter()
                        .find(|p| p.id == id && &p.owner == who)
                        .map_or(0, |p| p.balance);
                    if bal == 0 {
                        Positions::<T>::remove(id, who);
                    } else {
                        Positions::<T>::insert(id, who, bal);
                    }
                }
            }
            for who in accts {
                let c = st
                    .position_counts
                    .iter()
                    .find(|c| &c.owner == who)
                    .map_or(0, |c| c.count);
                if c == 0 {
                    PositionCount::<T>::remove(who);
                } else {
                    PositionCount::<T>::insert(who, c);
                }
            }
        }

        fn vault_escrow(st: &LedgerState<T::AccountId>, pid: ProposalId) -> Balance {
            st.vaults
                .iter()
                .find(|v| v.proposal == pid)
                .map_or(0, |v| v.info.escrowed)
        }

        fn baseline_escrow(st: &LedgerState<T::AccountId>, epoch: EpochId) -> Balance {
            st.baseline_vaults
                .iter()
                .find(|v| v.epoch == epoch)
                .map_or(0, |v| v.info.escrowed)
        }

        fn account_count(st: &LedgerState<T::AccountId>, who: &T::AccountId) -> u32 {
            st.position_counts
                .iter()
                .find(|c| &c.owner == who)
                .map_or(0, |c| c.count)
        }

        /// Run a proposal-scoped op end to end: load → core → persist → move USDC →
        /// emit. `escrow_party` receives/funds the escrow delta (the single
        /// balance-affected account; `accts` may additionally include a transfer
        /// counterparty for deposit accounting).
        fn run_proposal(
            pid: ProposalId,
            accts: &[T::AccountId],
            escrow_party: &T::AccountId,
            op: impl FnOnce(
                &mut LedgerState<T::AccountId>,
            ) -> Result<(), conditional_ledger_core::Error>,
        ) -> DispatchResult {
            let mut st = Self::load_proposal(pid, accts).ok_or(Error::<T>::UnknownVault)?;
            let escrow_before = Self::vault_escrow(&st, pid);
            let counts_before: Vec<u32> =
                accts.iter().map(|w| Self::account_count(&st, w)).collect();
            op(&mut st).map_err(Error::<T>::from)?;
            let escrow_after = Self::vault_escrow(&st, pid);
            let counts_after: Vec<u32> =
                accts.iter().map(|w| Self::account_count(&st, w)).collect();
            Self::persist_proposal(pid, accts, &st);
            Self::settle_collateral(escrow_party, escrow_before, escrow_after)?;
            Self::settle_deposits(accts, &counts_before, &counts_after)?;
            Self::emit_core_events(&st);
            Ok(())
        }

        fn run_baseline(
            epoch: EpochId,
            accts: &[T::AccountId],
            escrow_party: &T::AccountId,
            op: impl FnOnce(
                &mut LedgerState<T::AccountId>,
            ) -> Result<(), conditional_ledger_core::Error>,
        ) -> DispatchResult {
            let mut st =
                Self::load_baseline(epoch, accts).ok_or(Error::<T>::UnknownBaselineVault)?;
            let escrow_before = Self::baseline_escrow(&st, epoch);
            let counts_before: Vec<u32> =
                accts.iter().map(|w| Self::account_count(&st, w)).collect();
            op(&mut st).map_err(Error::<T>::from)?;
            let escrow_after = Self::baseline_escrow(&st, epoch);
            let counts_after: Vec<u32> =
                accts.iter().map(|w| Self::account_count(&st, w)).collect();
            Self::persist_baseline(epoch, accts, &st);
            Self::settle_collateral(escrow_party, escrow_before, escrow_after)?;
            Self::settle_deposits(accts, &counts_before, &counts_after)?;
            Self::emit_core_events(&st);
            Ok(())
        }

        /// Authority-only ops touch just the vault (no positions, no collateral).
        fn run_proposal_authority(
            pid: ProposalId,
            op: impl FnOnce(
                &mut LedgerState<T::AccountId>,
            ) -> Result<(), conditional_ledger_core::Error>,
        ) -> DispatchResult {
            let mut st = Self::load_proposal(pid, &[]).ok_or(Error::<T>::UnknownVault)?;
            op(&mut st).map_err(Error::<T>::from)?;
            Self::persist_proposal(pid, &[], &st);
            Self::emit_core_events(&st);
            Ok(())
        }

        fn run_baseline_authority(
            epoch: EpochId,
            op: impl FnOnce(
                &mut LedgerState<T::AccountId>,
            ) -> Result<(), conditional_ledger_core::Error>,
        ) -> DispatchResult {
            let mut st = Self::load_baseline(epoch, &[]).ok_or(Error::<T>::UnknownBaselineVault)?;
            op(&mut st).map_err(Error::<T>::from)?;
            Self::persist_baseline(epoch, &[], &st);
            Self::emit_core_events(&st);
            Ok(())
        }

        /// Move the escrow delta between `party` and the sovereign account. Positive
        /// delta (escrow grew) → `party` pays in; negative → `party` is paid out.
        fn settle_collateral(
            party: &T::AccountId,
            before: Balance,
            after: Balance,
        ) -> DispatchResult {
            let sovereign = Self::account_id();
            if after > before {
                T::Collateral::transfer(
                    Self::usdc(),
                    party,
                    &sovereign,
                    after.saturating_sub(before),
                    Preservation::Preserve,
                )?;
            } else if before > after {
                T::Collateral::transfer(
                    Self::usdc(),
                    &sovereign,
                    party,
                    before.saturating_sub(after),
                    Preservation::Preserve,
                )?;
            }
            Ok(())
        }

        /// Move position-storage deposits for each account whose live-entry count
        /// changed, and keep `DepositsHeld` in lockstep. The deposit amount is
        /// single-sourced from `T::PositionDeposit` for BOTH the real USDC movement
        /// and the `DepositsHeld` counter, so `position_deposit` stays a live META
        /// tunable (13 §1) without ever diverging from custody (L-6). A new entry has
        /// its owner (the recipient on `transfer`, the caller otherwise) fund the
        /// deposit; a deleted entry refunds it (03 §4 / §7 R-7).
        fn settle_deposits(
            accts: &[T::AccountId],
            before: &[u32],
            after: &[u32],
        ) -> DispatchResult {
            let sovereign = Self::account_id();
            let unit = T::PositionDeposit::get();
            let mut added: Balance = 0;
            let mut removed: Balance = 0;
            for (i, who) in accts.iter().enumerate() {
                let b = before[i];
                let a = after[i];
                if a > b {
                    let amt = unit.saturating_mul(Balance::from(a.saturating_sub(b)));
                    T::Collateral::transfer(
                        Self::usdc(),
                        who,
                        &sovereign,
                        amt,
                        Preservation::Preserve,
                    )
                    .map_err(|_| Error::<T>::DepositFailed)?;
                    added = added.saturating_add(amt);
                } else if b > a {
                    let amt = unit.saturating_mul(Balance::from(b.saturating_sub(a)));
                    T::Collateral::transfer(
                        Self::usdc(),
                        &sovereign,
                        who,
                        amt,
                        Preservation::Preserve,
                    )?;
                    removed = removed.saturating_add(amt);
                }
            }
            if added != removed {
                let held = DepositsHeld::<T>::get()
                    .checked_add(added)
                    .ok_or(Error::<T>::ArithmeticOverflow)?
                    .checked_sub(removed)
                    .ok_or(Error::<T>::ArithmeticOverflow)?;
                DepositsHeld::<T>::put(held);
            }
            Ok(())
        }

        /// Translate the core events appended during the op into runtime events.
        fn emit_core_events(st: &LedgerState<T::AccountId>) {
            for ev in &st.events {
                Self::deposit_event(Self::map_event(ev));
            }
        }

        fn map_event(ev: &CoreEvent) -> Event<T> {
            match *ev {
                CoreEvent::Split(pid, amount) => Event::Split { pid, amount },
                CoreEvent::Merged(pid, amount) => Event::Merged { pid, amount },
                CoreEvent::ScalarSplit(pid, branch, amount) => Event::ScalarSplit {
                    pid,
                    branch,
                    amount,
                },
                CoreEvent::ScalarMerged(pid, branch, amount) => Event::ScalarMerged {
                    pid,
                    branch,
                    amount,
                },
                CoreEvent::GateSplit(pid, branch, gate, amount) => Event::GateSplit {
                    pid,
                    branch,
                    gate,
                    amount,
                },
                CoreEvent::GateMerged(pid, branch, gate, amount) => Event::GateMerged {
                    pid,
                    branch,
                    gate,
                    amount,
                },
                CoreEvent::PositionTransferred(position, amount) => {
                    Event::PositionTransferred { position, amount }
                }
                CoreEvent::BaselineSplit(epoch, amount) => Event::BaselineSplit { epoch, amount },
                CoreEvent::BaselineMerged(epoch, amount) => Event::BaselineMerged { epoch, amount },
                CoreEvent::VaultResolved(pid, branch) => Event::VaultResolved { pid, branch },
                CoreEvent::VaultVoided(pid) => Event::VaultVoided { pid },
                CoreEvent::ScalarSettlementSet(pid, branch, s) => {
                    Event::ScalarSettlementSet { pid, branch, s }
                }
                CoreEvent::GateSettled(pid, branch, gate, outcome) => Event::GateSettled {
                    pid,
                    branch,
                    gate,
                    outcome,
                },
                CoreEvent::BaselineSettled(epoch, s) => Event::BaselineSettled { epoch, s },
                CoreEvent::Redeemed(pid, amount) => Event::Redeemed { pid, amount },
                CoreEvent::ScalarRedeemed(pid, side, payout) => {
                    Event::ScalarRedeemed { pid, side, payout }
                }
                CoreEvent::ScalarPairRedeemed(pid, amount) => {
                    Event::ScalarPairRedeemed { pid, amount }
                }
                CoreEvent::GateRedeemed(pid, gate, amount) => {
                    Event::GateRedeemed { pid, gate, amount }
                }
                CoreEvent::VoidRedeemed(pid, kind, amount, payout) => Event::VoidRedeemed {
                    pid,
                    kind,
                    amount,
                    payout,
                },
                CoreEvent::BaselineRedeemed(epoch, side, payout) => Event::BaselineRedeemed {
                    epoch,
                    side,
                    payout,
                },
                CoreEvent::VaultReaped(pid, residue) => Event::VaultReaped { pid, residue },
                CoreEvent::BaselineVaultReaped(epoch, residue) => {
                    Event::BaselineVaultReaped { epoch, residue }
                }
            }
        }

        // ------------------------------------------------------- reaping (03 §5.4)

        fn reap_eligible_at(terminal_at: BlockNumberFor<T>) -> BlockNumberFor<T> {
            terminal_at.saturating_add(T::ArchiveDelay::get())
        }

        fn do_sweep_proposal(pid: ProposalId) -> Result<bool, DispatchError> {
            let terminal_at = VaultTerminalAt::<T>::get(pid).ok_or(Error::<T>::ReapNotDue)?;
            ensure!(
                frame_system::Pallet::<T>::block_number() >= Self::reap_eligible_at(terminal_at),
                Error::<T>::ReapNotDue
            );
            let budget = T::ReapBatch::get();
            let mut drained = 0u32;
            let sovereign = Self::account_id();
            for id in Self::proposal_ids(pid) {
                if drained >= budget {
                    break;
                }
                let holders: Vec<(T::AccountId, Balance)> = Positions::<T>::iter_prefix(id)
                    .take(budget.saturating_sub(drained) as usize)
                    .collect();
                for (who, bal) in holders {
                    Self::reap_one(id, &who, bal, &sovereign)?;
                    drained += 1;
                }
            }
            // Fully drained iff no `Positions` entry remains across the 14 prefixes.
            let fully_drained =
                Self::proposal_ids(pid).all(|id| Positions::<T>::iter_prefix(id).next().is_none());
            if fully_drained {
                let residue = Vaults::<T>::get(pid).map_or(0, |v| v.escrowed);
                if residue > 0 {
                    T::Collateral::transfer(
                        Self::usdc(),
                        &sovereign,
                        &T::InsuranceAccount::get(),
                        residue,
                        Preservation::Preserve,
                    )?;
                }
                Vaults::<T>::remove(pid);
                VaultTerminalAt::<T>::remove(pid);
                Self::deposit_event(Event::VaultReaped { pid, residue });
            }
            Ok(drained > 0 || fully_drained)
        }

        fn do_sweep_baseline(epoch: EpochId) -> Result<bool, DispatchError> {
            let terminal_at = BaselineTerminalAt::<T>::get(epoch).ok_or(Error::<T>::ReapNotDue)?;
            ensure!(
                frame_system::Pallet::<T>::block_number() >= Self::reap_eligible_at(terminal_at),
                Error::<T>::ReapNotDue
            );
            let budget = T::ReapBatch::get();
            let mut drained = 0u32;
            let sovereign = Self::account_id();
            for id in Self::baseline_ids(epoch) {
                if drained >= budget {
                    break;
                }
                let holders: Vec<(T::AccountId, Balance)> = Positions::<T>::iter_prefix(id)
                    .take(budget.saturating_sub(drained) as usize)
                    .collect();
                for (who, bal) in holders {
                    Self::reap_one(id, &who, bal, &sovereign)?;
                    drained += 1;
                }
            }
            let fully_drained = Self::baseline_ids(epoch)
                .into_iter()
                .all(|id| Positions::<T>::iter_prefix(id).next().is_none());
            if fully_drained {
                let residue = BaselineVaults::<T>::get(epoch).map_or(0, |v| v.escrowed);
                if residue > 0 {
                    T::Collateral::transfer(
                        Self::usdc(),
                        &sovereign,
                        &T::InsuranceAccount::get(),
                        residue,
                        Preservation::Preserve,
                    )?;
                }
                BaselineVaults::<T>::remove(epoch);
                BaselineTerminalAt::<T>::remove(epoch);
                Self::deposit_event(Event::BaselineVaultReaped { epoch, residue });
            }
            Ok(drained > 0 || fully_drained)
        }

        /// Reap one `Positions` entry: drop it, decrement its instrument total, and
        /// (for non-protocol owners) release the storage deposit — refunded to the
        /// owner, or forfeited to INSURANCE if the owner no longer exists (R-7). Any
        /// failure propagates so FRAME rolls the whole `sweep_dust` back (G-1),
        /// keeping `PositionTotals`/`DepositsHeld`/counts exact.
        fn reap_one(
            id: PositionId,
            who: &T::AccountId,
            bal: Balance,
            sovereign: &T::AccountId,
        ) -> DispatchResult {
            Positions::<T>::remove(id, who);
            let remaining = PositionTotals::<T>::get(id)
                .checked_sub(bal)
                .ok_or(Error::<T>::ArithmeticOverflow)?;
            if remaining == 0 {
                PositionTotals::<T>::remove(id);
            } else {
                PositionTotals::<T>::insert(id, remaining);
            }
            if Self::is_protocol(who) {
                // protocol accounts hold no deposit and are not counted
                return Ok(());
            }
            let cnt = PositionCount::<T>::get(who);
            if cnt <= 1 {
                PositionCount::<T>::remove(who);
            } else {
                PositionCount::<T>::insert(who, cnt.saturating_sub(1));
            }
            let unit = T::PositionDeposit::get();
            let held = DepositsHeld::<T>::get()
                .checked_sub(unit)
                .ok_or(Error::<T>::ArithmeticOverflow)?;
            DepositsHeld::<T>::put(held);
            let dest = if frame_system::Pallet::<T>::account_exists(who) {
                who.clone()
            } else {
                T::InsuranceAccount::get()
            };
            T::Collateral::transfer(Self::usdc(), sovereign, &dest, unit, Preservation::Preserve)?;
            Ok(())
        }

        // ---------------------------------------------------------- try-state (§9)

        /// Load the full ledger state and run the core conservation checks (L-1…L-6),
        /// then the sovereign-solvency check L-2 the core cannot see.
        pub fn do_try_state() -> Result<(), sp_runtime::DispatchError> {
            let mut st = LedgerState::<T::AccountId>::new();
            for (pid, info) in Vaults::<T>::iter() {
                st.vaults.push(conditional_ledger_core::VaultRecord {
                    proposal: pid,
                    info,
                });
            }
            for (epoch, info) in BaselineVaults::<T>::iter() {
                st.baseline_vaults
                    .push(conditional_ledger_core::BaselineVaultRecord { epoch, info });
            }
            for (id, who, balance) in Positions::<T>::iter() {
                if balance > 0 {
                    st.positions.push(conditional_ledger_core::PositionRecord {
                        id,
                        owner: who.clone(),
                        balance,
                        deposit: Self::deposit_slot(&who),
                    });
                }
            }
            for (who, count) in PositionCount::<T>::iter() {
                st.position_counts
                    .push(conditional_ledger_core::PositionCount { owner: who, count });
            }
            for (id, total) in PositionTotals::<T>::iter() {
                st.position_totals
                    .push(conditional_ledger_core::PositionTotal { id, total });
            }
            // protocol-account exemptions must be visible to the cap/deposit checks
            let protocol_owners: Vec<T::AccountId> = st
                .position_counts
                .iter()
                .map(|c| c.owner.clone())
                .chain(st.positions.iter().map(|p| p.owner.clone()))
                .filter(|o| Self::is_protocol(o))
                .collect();
            for o in protocol_owners {
                st.add_protocol_account(o);
            }
            st.deposits_held = DepositsHeld::<T>::get();
            st.try_state()
                .map_err(|e| sp_runtime::DispatchError::from(Error::<T>::from(e)))?;

            // L-6 (explicit, storage-level): every owner holding a live position has a
            // matching `PositionCount`, and `DepositsHeld` equals the deposit unit
            // times the non-exempt live-entry count. The core validates count *rows*
            // but cannot see a *missing* row for an owner that still holds positions,
            // nor reconcile the aggregate `DepositsHeld` against real entries.
            let unit = T::PositionDeposit::get();
            let mut derived: BTreeMap<T::AccountId, u32> = BTreeMap::new();
            for (_id, who, balance) in Positions::<T>::iter() {
                if balance > 0 {
                    *derived.entry(who).or_default() += 1;
                }
            }
            let mut expected_deposits: Balance = 0;
            for (who, count) in &derived {
                if Self::is_protocol(who) {
                    continue;
                }
                ensure!(
                    PositionCount::<T>::get(who) == *count,
                    Error::<T>::TryStateViolation
                );
                let entry_deposits = unit
                    .checked_mul(Balance::from(*count))
                    .ok_or(Error::<T>::ArithmeticOverflow)?;
                expected_deposits = expected_deposits
                    .checked_add(entry_deposits)
                    .ok_or(Error::<T>::ArithmeticOverflow)?;
            }
            ensure!(
                DepositsHeld::<T>::get() == expected_deposits,
                Error::<T>::TryStateViolation
            );

            // L-2: sovereign USDC covers all escrow plus held deposits (checked — an
            // overflow is itself a conservation violation, never masked by saturation).
            let mut escrow: Balance = 0;
            for v in st.vaults.iter() {
                escrow = escrow
                    .checked_add(v.info.escrowed)
                    .ok_or(Error::<T>::ArithmeticOverflow)?;
            }
            for v in st.baseline_vaults.iter() {
                escrow = escrow
                    .checked_add(v.info.escrowed)
                    .ok_or(Error::<T>::ArithmeticOverflow)?;
            }
            let held = DepositsHeld::<T>::get();
            let bal = <T::Collateral as fungibles::Inspect<T::AccountId>>::balance(
                Self::usdc(),
                &Self::account_id(),
            );
            let liability = escrow
                .checked_add(held)
                .ok_or(Error::<T>::ArithmeticOverflow)?;
            ensure!(liability <= bal, Error::<T>::TryStateViolation);
            Ok(())
        }
    }
}
