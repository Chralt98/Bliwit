#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! # `pallet-registry` — Incident / Milestone bonded filings (A6)
//!
//! Production FRAME shell (Track-A DoD) over the frame-free functional core
//! [`registry_core`]. The core is the single implementation of the filing /
//! challenge / quorum / slashing state machine (07 §7) and doubles as the
//! Python-M3 ≡ Rust-core ≡ FRAME differential oracle; this pallet adds only the
//! runtime-facing surface it is responsible for:
//!
//! * bounded `#[pallet::storage]` matching the 07 §7 names/shapes (`Filings`
//!   double-map, `FilingCount`, `Aggregates`) plus the ledger-internal
//!   `AckRecords` dedup set;
//! * origin-checked `#[pallet::call]` extrinsics — `Signed` for the
//!   permissionless bonded workflow (`file` / `challenge_filing` / `ack_observed`
//!   / `crank_close` / `close_epoch` / `reap_epoch`), and
//!   [`Config::ResolutionAuthority`] for `resolve_challenge` (the round-2 outcome
//!   arrives from a keeper `recompute_proof` or the `OracleResolution` track — 07
//!   §7 / §5.4, wired in B1a);
//! * real USDC bond custody via `T::Collateral` ([`fungibles::Mutate`]): the
//!   `reg.bond_*` bond is escrowed on `file`/`challenge_filing` and refunded or
//!   slashed 40 / 60 on resolution (07 §5.5/§7), so the registry's held bonds and
//!   the sovereign balance can never disagree;
//! * the cross-pallet reads the core models as pure inputs, injected as Config
//!   seams so A6 does not hard-depend on the A5/A7/A8 FRAME pallets (all wired in
//!   B1a): [`Config::Watchtowers`] (the 07 §4 bonded watchtower registry, →
//!   `pallet-oracle`), [`Config::Welfare`] (the settlement-time consumer, →
//!   `pallet-welfare::note_external_component`), [`Config::Epoch`] (the trusted
//!   per-epoch filing window + frozen spec version, → `pallet-epoch` /
//!   `pallet-welfare`) and [`Config::Params`] (the live `reg.bond_*` tunables, →
//!   `pallet-constitution::Params`, rule 4);
//! * the mandatory `try_state` hook (07 §7 / 15 §1); **no block hooks** (07 §7
//!   "Hooks: none (I-20)").
//!
//! ## One pallet, two instances (07 §7)
//!
//! The pallet is instantiable (`<T, I>`); the runtime deploys it twice — one
//! instance with `Kind = Incident` (`IncidentRegistry`, feeds `C_attested`) and
//! one with `Kind = Milestone` (`MilestoneRegistry`, feeds the A pillar). The
//! per-instance discriminant is [`Config::Kind`]; every other seam (watchtowers,
//! welfare, epoch clock, Params) is shared and routes by that kind.
//!
//! Every mutating call loads exactly the bounded storage cells the core op for
//! `epoch` can touch (that epoch's ≤ 64 filings + the ≤ 4 live `FilingCount` and
//! ≤ 4 `Aggregates` entries), runs the core op, persists the delta, moves USDC,
//! then drains the core event log into `deposit_event`. FRAME wraps every
//! dispatch in [`frame_support::storage::with_storage_layer`], so any later
//! error — including a failed `T::Collateral::transfer` — rolls the persisted
//! storage back with it; the core post-image can never outlive its custody move
//! (G-1).

extern crate alloc;

pub use pallet::*;
pub use registry_core;
pub use weights::WeightInfo;

pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

use futarchy_primitives::{
    AccountId as CoreAccountId, Balance, BlockNumber, EpochId, FixedU64, MetricSpecVersion, H256,
};
use registry_core::{FilingClass, FilingId, RegistryKind};

/// Live `reg.bond_incident` / `reg.bond_milestone` (13 §1, META), read from
/// `pallet-constitution::Params` (rule 4). The runtime converts the typed
/// `Params` records to base USDC units (B1a); the pallet threads them into the
/// core's [`registry_core::Registry`] bond seams on every load, so the bond a
/// filing escrows is always the live tunable, never a compile-time literal.
pub trait RegistryParams {
    /// `reg.bond_incident` in base USDC units (default 5,000 USDC).
    fn bond_incident() -> Balance;
    /// `reg.bond_milestone` in base USDC units (default 2,500 USDC).
    fn bond_milestone() -> Balance;
}

/// The 07 §4 bonded watchtower registry, read at `ack_observed`. Only bonded,
/// slashable seats registered on `pallet-oracle` may count toward `WT_QUORUM`;
/// the runtime wires this to the oracle's `Watchtowers` map (A5/B1a). An
/// unregistered acker is rejected (`Error::NotRegistered`).
pub trait WatchtowerRegistry<AccountId> {
    /// Is `who` a currently-registered, bonded watchtower?
    fn is_registered_watchtower(who: &AccountId) -> bool;
}

/// The settlement-time consumer of a closed epoch's aggregate (07 §7:
/// `close_epoch` "hands it to `pallet-welfare::note_external_component`"). The
/// runtime wires this to `pallet-welfare` (A7/B1a); a no-op stub is a valid
/// pre-B1a wiring (the aggregate is also durably surfaced by the
/// `RegistryEpochClosed` event).
pub trait WelfareSink {
    /// Hand the derived aggregate for `(kind, epoch)` to welfare. Returns a
    /// `DispatchResult` so a refusal (duplicate, full bounded map, spec mismatch)
    /// propagates and rolls the whole `close_epoch` back (G-1) rather than
    /// silently desyncing registry and welfare; a pull-model stub returns `Ok`.
    fn note_external_component(
        kind: RegistryKind,
        epoch: EpochId,
        aggregate: FixedU64,
    ) -> sp_runtime::DispatchResult;
}

/// Trusted per-epoch context the registry cannot derive itself (07 §7): the
/// filing-window-end block (from `pallet-epoch`'s clock) and the **frozen**
/// MetricSpec version a filing for that measurement epoch must attest under
/// (I-16 — the version every consuming cohort froze at creation, from
/// `pallet-welfare`'s MetricSpec registry). Both are wired in B1a. Crucially,
/// the frozen version is read here, **never** taken from the filer — a
/// caller-supplied "expected" version would make the `SpecVersionMismatch` gate
/// vacuous.
pub trait EpochContext {
    /// The last block at which a filing for measurement `epoch` is admissible.
    fn filing_window_end(epoch: EpochId) -> BlockNumber;
    /// The frozen MetricSpec version filings for `epoch` must attest under (I-16).
    fn frozen_spec_version(epoch: EpochId) -> Option<MetricSpecVersion>;
    /// The Milestone-instance completion **target** for `epoch` — the
    /// frozen-MetricSpec denominator of `points ÷ target` (07 §7 / 05 §4.4). A
    /// per-spec field (I-16), never a 13/kernel constant. Unused by the Incident
    /// instance.
    fn milestone_target(epoch: EpochId) -> u32;
}

/// Maps a benchmark scenario to concrete runtime origins/accounts so the harness
/// can exercise every call with its exact 07 §7 authority and a funded bond.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<RuntimeOrigin, AccountId> {
    /// An origin [`Config::ResolutionAuthority`] admits (the recompute / oracle
    /// adjudication authority).
    fn resolution_origin() -> RuntimeOrigin;
    /// A USDC-funded account for `Signed` filing/challenge/keeper calls.
    fn funded_account(seed: u8) -> AccountId;
    /// Make [`Config::Watchtowers`] report `who` as a registered watchtower.
    fn register_watchtower(who: &AccountId);
    /// Install the real epoch/spec filing context for the benchmark epoch.
    fn prime_epoch(epoch: EpochId);
    fn prime_keeper_rebate() {}
    fn assert_keeper_rebate_paid(_: futarchy_primitives::keeper::CrankClass) {}
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use alloc::vec::Vec;
    use frame_support::{
        pallet_prelude::*,
        traits::{
            fungibles::{self, Inspect, Mutate},
            tokens::Preservation,
        },
        PalletId,
    };
    use frame_system::pallet_prelude::*;
    use futarchy_primitives::keeper::{CrankClass, KeeperRebateSink};
    use registry_core::{Error as CoreError, Event as CoreEvent, Filing, FilingState, Registry};
    use sp_runtime::{traits::AccountIdConversion, SaturatedConversion, Saturating};

    /// The concrete asset id the configured collateral fungible uses for USDC
    /// bonds. The registry never names it (rule 7 — no XCM types here); the
    /// runtime pins it to the USDC `Location` via [`Config::UsdcAssetId`].
    pub type AssetIdOf<T, I> = <<T as Config<I>>::Collateral as fungibles::Inspect<
        <T as frame_system::Config>::AccountId,
    >>::AssetId;

    /// The scoped pre-image `run_scoped` threads from [`Pallet::load`] through
    /// persist + custody: the hydrated core aggregate, `epoch`'s filings before
    /// the op, and the live `FilingCount` / `Aggregates` key sets to diff against.
    struct LoadCtx {
        reg: Registry,
        pre: Vec<(FilingId, Filing)>,
        count_epochs: Vec<EpochId>,
        agg_epochs: Vec<EpochId>,
    }

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T, I = ()>(_);

    // The filer/challenger/watchtower accounts are the concrete 32-byte runtime
    // account (`AccountId32`, 02 §8); the frame-free core is written against
    // `[u8; 32]`, so the pallet bridges `T::AccountId ↔ [u8; 32]` at the
    // call/event/custody boundary. Bounding the supertrait's associated type
    // propagates it to every `impl<T: Config<I>, I>`.
    #[pallet::config]
    pub trait Config<I: 'static = ()>:
        frame_system::Config<
        AccountId: From<[u8; 32]> + Into<[u8; 32]>,
        RuntimeEvent: From<Event<Self, I>>,
    >
    {
        /// USDC collateral (the `ForeignAssets` instance in production). Balances
        /// are the shared `u128` kernel [`Balance`].
        type Collateral: fungibles::Mutate<Self::AccountId>
            + fungibles::Inspect<Self::AccountId, Balance = Balance>;

        /// The asset id of USDC inside [`Config::Collateral`] (the USDC `Location`).
        type UsdcAssetId: Get<AssetIdOf<Self, I>>;

        /// This instance's registry discriminant (07 §7: `Incident` feeds
        /// `C_attested`, `Milestone` feeds the A pillar).
        #[pallet::constant]
        type Kind: Get<RegistryKind>;

        /// Live `reg.bond_*` tunables (rule 4). See [`RegistryParams`].
        type Params: RegistryParams;

        /// The 07 §4 bonded watchtower registry (→ `pallet-oracle`, B1a).
        type Watchtowers: WatchtowerRegistry<Self::AccountId>;

        /// Settlement-time aggregate consumer (→ `pallet-welfare`, B1a).
        type Welfare: WelfareSink;

        /// Trusted per-epoch context: filing window + frozen spec version
        /// (→ `pallet-epoch` / `pallet-welfare`, B1a).
        type Epoch: EpochContext;

        /// Resolves a challenged filing's round-2 outcome (07 §7): a keeper
        /// `recompute_proof` or the `OracleResolution` values track. No signed
        /// origin resolves here; the runtime wires it in B1a. Narrow by
        /// construction (rule 6) — a public caller can never force an outcome.
        type ResolutionAuthority: EnsureOrigin<Self::RuntimeOrigin>;

        /// Destination for the 60 % INSURANCE share of a slashed bond (07 §5.5).
        type InsuranceAccount: Get<Self::AccountId>;

        /// This instance's `PalletId`; its derived sovereign account custodies all
        /// escrowed filing bonds. Instances MUST use distinct ids.
        #[pallet::constant]
        type PalletId: Get<PalletId>;

        /// The registry archive delay — a closed epoch is reap-eligible only once
        /// this many blocks have elapsed since it closed, so welfare has consumed
        /// its aggregate at snapshot time before the records are destroyed (07 §7
        /// "reaped at cohort settlement + archive delay"). Prevents a griefer
        /// erasing an incident before settlement. This is a `Get` the runtime
        /// sources at B1a — reusing `ledger.archive_delay` (13 §1) or a new
        /// `reg.archive_delay` key, a 13 decision pending (PLAN SQ-76); the code
        /// hardcodes no literal (rule 4).
        #[pallet::constant]
        type ArchiveDelay: Get<BlockNumberFor<Self>>;

        /// `reg.max_filings_epoch = 64` (13 §4, K). MUST equal the core's
        /// [`registry_core::MAX_FILINGS_PER_EPOCH`]; pinned by `integrity_test`.
        #[pallet::constant]
        type MaxFilingsPerEpoch: Get<u32>;

        /// Evidence is a 32-byte content hash only (07 §7 Config
        /// `MaxEvidenceLen`); the on-chain object is a fixed [`H256`], so this
        /// bound documents the contract surface (the runtime pins it to 32).
        #[pallet::constant]
        type MaxEvidenceLen: Get<u32>;

        /// Benchmarked weights.
        type WeightInfo: WeightInfo;

        /// Infallible, fail-soft rebate sink for useful registry cranks (07 §7,
        /// 08 §6.3). This associated type is instance-scoped, like every other
        /// registry seam. The funding line is per-crank, not per-pallet (07 §7
        /// *Crank funding lines*): the dispute machinery — `ack_observed` and
        /// `crank_close` — pays from the separate ORACLE budget line, while the
        /// archival `reap_epoch` pays from the metered **general** keeper
        /// tranche. Each call site passes its own [`CrankClass`].
        type KeeperRebate: KeeperRebateSink<Self::AccountId>;

        /// Origin/account construction for benchmarking.
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: BenchmarkHelper<Self::RuntimeOrigin, Self::AccountId>;
    }

    // ------------------------------------------------------------------ storage

    /// Bonded filings — `double_map (EpochId, FilingId) → Filing` (07 §7). Each
    /// value is `MaxEncodedLen`; the key count is bounded logically by
    /// [`FilingCount`] (≤ `MaxFilingsPerEpoch` per epoch) × ≤
    /// [`registry_core::MAX_LIVE_EPOCHS`] non-closed epochs, enforced by the core
    /// and asserted in `try_state` (the ledger's `Positions` precedent — a map's
    /// bound is its accounting, not a structural `BoundedVec`).
    #[pallet::storage]
    pub type Filings<T: Config<I>, I: 'static = ()> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        EpochId,
        Blake2_128Concat,
        FilingId,
        Filing,
        OptionQuery,
    >;

    /// Live per-epoch filing count / id cursor — `map EpochId → u32` (07 §7).
    /// Reaped at `close_epoch` so the ≤ 4-live-epoch bound stays concurrent.
    #[pallet::storage]
    pub type FilingCount<T: Config<I>, I: 'static = ()> =
        StorageMap<_, Blake2_128Concat, EpochId, u32, ValueQuery>;

    /// Derived epoch aggregates — `map EpochId → FixedU64` (07 §7): the `I` input
    /// (Incident) or milestone-points input (Milestone) handed to welfare.
    #[pallet::storage]
    pub type Aggregates<T: Config<I>, I: 'static = ()> =
        StorageMap<_, Blake2_128Concat, EpochId, FixedU64, OptionQuery>;

    /// Watchtower-acknowledgment dedup set — `(EpochId, FilingId, AccountId) → ()`
    /// (07 §4/§7). Ledger-internal; the pallet enforces the "one ack per
    /// watchtower per filing" rule the core models with its `ack_records` vec, so
    /// the loaded core aggregate never has to carry it. Reaped with its epoch.
    #[pallet::storage]
    pub type AckRecords<T: Config<I>, I: 'static = ()> = StorageNMap<
        _,
        (
            NMapKey<Blake2_128Concat, EpochId>,
            NMapKey<Blake2_128Concat, FilingId>,
            NMapKey<Blake2_128Concat, CoreAccountId>,
        ),
        (),
        OptionQuery,
    >;

    /// Block at which each epoch was closed out, for the `reap_epoch`
    /// archive-delay gate (07 §7). Set at `close_epoch`, removed at `reap_epoch`;
    /// ≤ `MAX_AGGREGATES` live keys. Also the durable "already closed" marker that,
    /// together with the `FilingCount`-present precondition, makes close idempotent
    /// across a reap (a reaped epoch has neither, so it cannot be re-closed).
    #[pallet::storage]
    pub type ClosedAt<T: Config<I>, I: 'static = ()> =
        StorageMap<_, Blake2_128Concat, EpochId, BlockNumberFor<T>, OptionQuery>;

    // -------------------------------------------------------------------- events

    /// Exactly the 10 frozen 02 §6 (pallet-registry row) event names,
    /// byte-for-byte (07 §7: "MUST NOT drift from it"). The 07 §4 quorum machinery
    /// (`ack_observed` / the 48 h extension) updates the `Filing` state a client
    /// reads from `Filings`; it emits **no** registry event, because
    /// `WindowAcknowledged` / `WindowExtended` are frozen in 02 §7.2 for the
    /// **oracle**, not the registry (dual-review finding — see PLAN.md SQ).
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config<I>, I: 'static = ()> {
        /// `file` on the Incident instance (07 §7).
        IncidentFiled {
            epoch: EpochId,
            filing_id: FilingId,
            who: T::AccountId,
            class: FilingClass,
            evidence_hash: H256,
            bond: Balance,
        },
        /// `file` on the Milestone instance (07 §7).
        MilestoneFiled {
            epoch: EpochId,
            filing_id: FilingId,
            who: T::AccountId,
            class: FilingClass,
            points: u16,
            evidence_hash: H256,
            bond: Balance,
        },
        /// `challenge_filing` on the Incident instance (07 §7).
        IncidentChallenged {
            epoch: EpochId,
            filing_id: FilingId,
            challenger: T::AccountId,
            evidence_hash: H256,
            bond: Balance,
        },
        /// `challenge_filing` on the Milestone instance (07 §7).
        MilestoneChallenged {
            epoch: EpochId,
            filing_id: FilingId,
            challenger: T::AccountId,
            evidence_hash: H256,
            bond: Balance,
        },
        /// An Incident filing closed as upheld (07 §7).
        IncidentUpheld { epoch: EpochId, filing_id: FilingId },
        /// An Incident filing closed as rejected (07 §7).
        IncidentRejected { epoch: EpochId, filing_id: FilingId },
        /// A Milestone filing closed as accepted (07 §7).
        MilestoneAccepted { epoch: EpochId, filing_id: FilingId },
        /// A Milestone filing closed as rejected (07 §7).
        MilestoneRejected { epoch: EpochId, filing_id: FilingId },
        /// A challenge resolved: the loser's bond was slashed 40 / 60 (07 §5.5/§7).
        FilingBondSlashed {
            epoch: EpochId,
            filing_id: FilingId,
            loser: T::AccountId,
            amount: Balance,
            challenger_share: Balance,
            insurance_share: Balance,
        },
        /// `close_epoch` derived the epoch aggregate and handed it to welfare (07 §7).
        RegistryEpochClosed {
            kind: RegistryKind,
            epoch: EpochId,
            aggregate: FixedU64,
        },
    }

    // -------------------------------------------------------------------- errors

    /// 1:1 with [`registry_core::Error`]; the pallet adds only the custody /
    /// bridge failure paths the core does not model.
    #[pallet::error]
    pub enum Error<T, I = ()> {
        /// The per-epoch filing cap (`MaxFilingsPerEpoch`) is reached.
        EpochFull,
        /// More than `MAX_LIVE_EPOCHS` epochs have live filings.
        TooManyLiveEpochs,
        /// More than `MAX_AGGREGATES` closed-epoch aggregates are retained.
        TooManyAggregates,
        /// The filing/challenge window has closed.
        WindowClosed,
        /// The window/challenge round is still open (premature close/resolve).
        WindowOpen,
        /// The filing is already challenged (registry games do not escalate).
        AlreadyChallenged,
        /// The filing is already terminal.
        AlreadyFinal,
        /// The report names a spec version other than the frozen one (I-16).
        SpecVersionMismatch,
        /// The required bond is zero / below minimum.
        BondBelowMinimum,
        /// No filing with that `(epoch, filing_id)`.
        FilingNotFound,
        /// This watchtower already acknowledged this filing.
        DuplicateAck,
        /// The close batch exceeds `REG_CLOSE_BATCH`.
        BatchTooLarge,
        /// The filing class is invalid for this instance's kind.
        InvalidClass,
        /// Checked arithmetic overflowed (G-1).
        Overflow,
        /// The acker is not a registered bonded watchtower (07 §4).
        NotRegistered,
        /// The core state validator rejected the aggregate (try-state only).
        TryStateViolation,
        /// A lossy `AccountId` bridge would alias distinct accounts (02 §8).
        BadAccount,
        /// The filing already has `WT_QUORUM` acknowledgments (07 §4).
        AlreadyQuorum,
        /// The epoch is not yet reap-eligible: not closed, or `ArchiveDelay` has
        /// not elapsed since close (07 §7).
        ReapNotDue,
        /// `close_epoch` on an epoch with no live filings — nothing to close
        /// (an empty epoch is welfare's "no record ⇒ 1" default, and a reaped
        /// epoch must never re-close, 07 §7).
        NothingToClose,
        /// The Milestone instance's frozen-MetricSpec completion `target` is zero
        /// or absent, so `min(1, points ÷ target)` is undefined: `file` and
        /// `close_epoch` both refuse rather than record a fabricated `0.0`
        /// A-pillar component (07 §7 *Milestone normalization*). Appended last —
        /// the preceding variant indices are metadata-stable (02 §13).
        MilestoneTargetUnset,
    }

    // --------------------------------------------------------------------- hooks

    #[pallet::hooks]
    impl<T: Config<I>, I: 'static> Hooks<BlockNumberFor<T>> for Pallet<T, I> {
        /// The Config caps MUST equal the core's compile-time caps, or a runtime
        /// could advertise a bound the core silently ignores.
        fn integrity_test() {
            assert_eq!(
                T::MaxFilingsPerEpoch::get(),
                registry_core::MAX_FILINGS_PER_EPOCH,
                "Config::MaxFilingsPerEpoch must equal the core cap (07 §7 / 13 §4)"
            );
            assert_eq!(
                T::MaxEvidenceLen::get(),
                32,
                "evidence is a 32-byte content hash only (07 §7)"
            );
        }

        /// The registry does **no** automatic per-block work (07 §7 "Hooks: none",
        /// I-20). Only `try_state` is implemented (15 §1).
        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), sp_runtime::TryRuntimeError> {
            Self::do_try_state()
        }
    }

    // ---------------------------------------------------------------------- calls

    #[pallet::call]
    impl<T: Config<I>, I: 'static> Pallet<T, I> {
        /// 07 §7. File a bonded claim about an off-chain fact. Holds
        /// `reg.bond_{incident,milestone}` (from `Params`), opens a 72 h challenge
        /// window under the §4 quorum rule.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::file())]
        pub fn file(
            origin: OriginFor<T>,
            epoch: EpochId,
            class: FilingClass,
            points: u16,
            evidence_hash: H256,
            spec_version: MetricSpecVersion,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let raw = Self::to_core_authorized(&who)?;
            let now = Self::now();
            let bond = Self::required_bond();
            // The window and the frozen spec version are trusted per-epoch reads
            // (I-16), never taken from the filer.
            let filing_window_end = T::Epoch::filing_window_end(epoch);
            let expected_spec =
                T::Epoch::frozen_spec_version(epoch).ok_or(Error::<T, I>::SpecVersionMismatch)?;
            Self::run_scoped(epoch, |reg| {
                reg.file(registry_core::FileInput {
                    who: raw,
                    now,
                    epoch,
                    class,
                    points,
                    evidence_hash,
                    spec_version,
                    expected_spec,
                    filing_window_end,
                })
                .map(|_| ())
            })?;
            // Escrow the bond after the core admits the filing; a failed transfer
            // (insufficient USDC) rolls the whole dispatch back (G-1).
            Self::hold_bond(&who, bond)
        }

        /// 07 §7. Challenge a live filing, posting the matching bond; opens the
        /// single counter-round (registry games do not escalate).
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::challenge_filing())]
        pub fn challenge_filing(
            origin: OriginFor<T>,
            epoch: EpochId,
            filing_id: FilingId,
            evidence_hash: H256,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let raw = Self::to_core_authorized(&who)?;
            let now = Self::now();
            // The matching bond equals the filing's escrowed bond (07 §7).
            let bond = Filings::<T, I>::get(epoch, filing_id)
                .ok_or(Error::<T, I>::FilingNotFound)?
                .bond;
            Self::run_scoped(epoch, |reg| {
                reg.challenge_filing(raw, now, epoch, filing_id, evidence_hash)
            })?;
            Self::hold_bond(&who, bond)
        }

        /// 07 §4/§7. A registered watchtower acknowledges a filing's
        /// observability. O(1); the runtime rebates the keeper-class fee.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::ack_observed())]
        pub fn ack_observed(
            origin: OriginFor<T>,
            epoch: EpochId,
            filing_id: FilingId,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let raw = Self::to_core_authorized(&who)?;
            ensure!(
                T::Watchtowers::is_registered_watchtower(&who),
                Error::<T, I>::NotRegistered
            );
            // Pallet-level dedup (the core's `ack_records` role): one ack per
            // watchtower per filing, so the loaded aggregate never carries acks.
            ensure!(
                !AckRecords::<T, I>::contains_key((epoch, filing_id, raw)),
                Error::<T, I>::DuplicateAck
            );
            let now = Self::now();
            Self::run_scoped(epoch, |reg| {
                // `is_registered_watchtower = true` and empty `ack_records` — the
                // gate + dedup are enforced above; the core only increments the
                // count and stamps the event.
                reg.ack_observed(raw, now, true, epoch, filing_id)
            })?;
            AckRecords::<T, I>::insert((epoch, filing_id, raw), ());
            // B5 recalibrates this weight for the post-commit rebate write/payout.
            T::KeeperRebate::rebate(&who, CrankClass::OracleLine);
            Ok(())
        }

        /// 07 §7. Keeper crank: close ≤ `REG_CLOSE_BATCH` due filings of `epoch` —
        /// unchallenged + quorum ⇒ upheld (bond refunded); quorum failure ⇒ one
        /// 48 h extension, then rejected-as-unobservable (bond refunded, §4).
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::crank_close())]
        pub fn crank_close(origin: OriginFor<T>, epoch: EpochId, batch: u32) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let now = Self::now();
            let batch = batch as usize;
            let rebate_progress =
                Self::run_scoped_with_rebate_progress(epoch, |reg| reg.crank_close(now, batch))?;
            if rebate_progress {
                // B5 recalibrates this weight for the post-commit rebate write/payout.
                T::KeeperRebate::rebate(&who, CrankClass::OracleLine);
            }
            Ok(())
        }

        /// 07 §7. Resolve a challenged filing's counter-round: the loser forfeits
        /// the bond 40 / 60. The outcome arrives from a keeper `recompute_proof`
        /// or the `OracleResolution` track (07 §5.4) — [`Config::ResolutionAuthority`].
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::resolve_challenge())]
        pub fn resolve_challenge(
            origin: OriginFor<T>,
            epoch: EpochId,
            filing_id: FilingId,
            uphold: bool,
        ) -> DispatchResult {
            T::ResolutionAuthority::ensure_origin(origin)?;
            Self::run_scoped(epoch, |reg| reg.resolve_challenge(epoch, filing_id, uphold))
        }

        /// 07 §7. Keeper: once every filing of `epoch` is terminal, derive the
        /// aggregate (Incident: `max(0, 1 − Σ severity)`, "no filings ⇒ 1";
        /// Milestone: `points ÷ target`) and hand it to welfare.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::close_epoch())]
        pub fn close_epoch(origin: OriginFor<T>, epoch: EpochId) -> DispatchResult {
            ensure_signed(origin)?;
            // Close only an epoch that was actually filed (has a live `FilingCount`
            // entry). This (a) makes close idempotent across a reap — a reaped
            // epoch has no `FilingCount`, so it can never be re-closed to the
            // favorable "no filings ⇒ 1" value; and (b) leaves genuinely-empty
            // epochs to welfare's pull-side "no record ⇒ 1" default rather than a
            // permissionless griefing close (dual-review finding).
            ensure!(
                FilingCount::<T, I>::contains_key(epoch),
                Error::<T, I>::NothingToClose
            );
            let now = Self::now();
            let filing_window_end = T::Epoch::filing_window_end(epoch);
            Self::run_scoped(epoch, |reg| {
                reg.close_epoch(epoch, now, filing_window_end).map(|_| ())
            })?;
            // Stamp the close block for the reap archive-delay gate.
            ClosedAt::<T, I>::insert(epoch, frame_system::Pallet::<T>::block_number());
            Ok(())
        }

        /// 07 §7. Keeper: reap a closed epoch's archived filings + acks + the
        /// aggregate — only once `ArchiveDelay` blocks have elapsed since close, so
        /// welfare has consumed the aggregate (cohort settlement) before the
        /// records are destroyed. A permissionless reap without this gate would let
        /// a griefer erase an incident before settlement and re-open the epoch.
        #[pallet::call_index(6)]
        #[pallet::weight(T::WeightInfo::reap_epoch())]
        pub fn reap_epoch(origin: OriginFor<T>, epoch: EpochId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let closed_at = ClosedAt::<T, I>::get(epoch).ok_or(Error::<T, I>::ReapNotDue)?;
            let due = closed_at.saturating_add(T::ArchiveDelay::get());
            ensure!(
                frame_system::Pallet::<T>::block_number() >= due,
                Error::<T, I>::ReapNotDue
            );
            Self::run_scoped(epoch, |reg| reg.reap_epoch(epoch))?;
            ClosedAt::<T, I>::remove(epoch);
            // The durable ack set is the pallet's; clear the whole `(epoch, *, *)`
            // prefix with an explicit bound (≤ `WT_QUORUM` acks per filing ×
            // `MaxFilingsPerEpoch` filings — the cap enforced in `ack_observed`).
            let limit = registry_core::WT_QUORUM as u32 * T::MaxFilingsPerEpoch::get() + 1;
            let _ = AckRecords::<T, I>::clear_prefix((epoch,), limit, None);
            // 07 §7 *Crank funding lines*: reaping is archival cleanup with no
            // dispute content, so it is rebated from the metered **general**
            // keeper tranche (08 §6.3) — NOT the oracle budget line that funds
            // `ack_observed` / `crank_close`. Charging storage housekeeping to
            // the oracle line would couple dispute liveness to archival work;
            // the general tranche is capped at 20 % of the keeper budget, so its
            // exhaustion can never starve a decision-critical crank and only
            // stops the rebate — reaping stays permissionless (SQ-297).
            // B5 recalibrates this weight for the post-commit rebate write/payout.
            T::KeeperRebate::rebate(&who, CrankClass::General);
            Ok(())
        }
    }

    // ---------------------------------------------------------- scoped adapter

    impl<T: Config<I>, I: 'static> Pallet<T, I> {
        /// This instance's sovereign account — custodies escrowed filing bonds.
        pub fn account_id() -> T::AccountId {
            T::PalletId::get().into_account_truncating()
        }

        fn usdc() -> AssetIdOf<T, I> {
            T::UsdcAssetId::get()
        }

        /// The live required bond for this instance's kind (from `Params`, rule 4).
        fn required_bond() -> Balance {
            match T::Kind::get() {
                RegistryKind::Incident => T::Params::bond_incident(),
                RegistryKind::Milestone => T::Params::bond_milestone(),
            }
        }

        /// Current block as the core's `u32` block number (real runtime is u32;
        /// mocks stay well below the ceiling).
        fn now() -> BlockNumber {
            frame_system::Pallet::<T>::block_number().saturated_into::<u32>()
        }

        /// Round-trip-checked `T::AccountId → [u8; 32]` for authorization/custody:
        /// a lossy `Into` could alias two runtime accounts to the same bytes and
        /// misroute a bond refund. `AccountId32` (02 §8) is bijective, so this
        /// always passes; it hardens against a misconfigured lossy `AccountId`.
        fn to_core_authorized(who: &T::AccountId) -> Result<CoreAccountId, DispatchError> {
            let raw: CoreAccountId = who.clone().into();
            ensure!(&T::AccountId::from(raw) == who, Error::<T, I>::BadAccount);
            Ok(raw)
        }

        /// Assemble the scoped core aggregate: this instance's kind + live bonds,
        /// `epoch`'s filings, and every live `FilingCount` / `Aggregates` entry
        /// (needed for the `TooManyLiveEpochs` / `AlreadyFinal` cross-epoch
        /// checks). `ack_records` stays empty — the pallet owns the dedup set.
        /// Returns the aggregate plus the pre-image the persist/custody steps diff
        /// against.
        fn load(epoch: EpochId) -> LoadCtx {
            let mut reg = Registry::new(T::Kind::get());
            reg.bond_incident = T::Params::bond_incident();
            reg.bond_milestone = T::Params::bond_milestone();
            // The Milestone divisor is the frozen-MetricSpec target for this
            // epoch (I-16), never a hardcode (rule 4). Incident ignores it.
            reg.milestone_target = T::Epoch::milestone_target(epoch);
            let mut pre: Vec<(FilingId, Filing)> = Vec::new();
            for (id, f) in Filings::<T, I>::iter_prefix(epoch) {
                reg.filings.push(((epoch, id), f));
                pre.push((id, f));
            }
            let mut count_epochs: Vec<EpochId> = Vec::new();
            for (e, c) in FilingCount::<T, I>::iter() {
                reg.filing_count.push((e, c));
                count_epochs.push(e);
            }
            let mut agg_epochs: Vec<EpochId> = Vec::new();
            for (e, a) in Aggregates::<T, I>::iter() {
                reg.aggregates.push((e, a));
                agg_epochs.push(e);
            }
            LoadCtx {
                reg,
                pre,
                count_epochs,
                agg_epochs,
            }
        }

        /// Persist the post-image of a scoped op. `epoch`'s filings are diffed
        /// against the pre-image (adds inserted, mutations updated, reaped keys
        /// removed); the full `FilingCount` / `Aggregates` sets are diffed against
        /// the loaded key sets (a `close_epoch` drops a count, adds an aggregate;
        /// a `reap_epoch` drops the aggregate).
        fn persist(
            epoch: EpochId,
            reg: &Registry,
            pre: &[(FilingId, Filing)],
            count_epochs: &[EpochId],
            agg_epochs: &[EpochId],
        ) {
            // Filings for `epoch` — write only the filings that actually changed
            // (or are new), so `file`/`challenge`/`ack`/`resolve` touch O(1)
            // storage cells rather than re-writing the whole ≤ 64-filing epoch
            // (dual-review: keeps the calls at their 07 §7 O(1) weight class).
            let post: Vec<(FilingId, Filing)> = reg
                .filings
                .iter()
                .filter(|((e, _), _)| *e == epoch)
                .map(|((_, id), f)| (*id, *f))
                .collect();
            for (id, f) in &post {
                let unchanged = pre.iter().any(|(pid, pf)| pid == id && pf == f);
                if !unchanged {
                    Filings::<T, I>::insert(epoch, id, f);
                }
            }
            for (id, _) in pre {
                if !post.iter().any(|(pid, _)| pid == id) {
                    Filings::<T, I>::remove(epoch, id);
                }
            }
            // FilingCount (all live epochs).
            for (e, c) in &reg.filing_count {
                FilingCount::<T, I>::insert(e, c);
            }
            for e in count_epochs {
                if !reg.filing_count.iter().any(|(pe, _)| pe == e) {
                    FilingCount::<T, I>::remove(e);
                }
            }
            // Aggregates (all).
            for (e, a) in &reg.aggregates {
                Aggregates::<T, I>::insert(e, a);
            }
            for e in agg_epochs {
                if !reg.aggregates.iter().any(|(pe, _)| pe == e) {
                    Aggregates::<T, I>::remove(e);
                }
            }
        }

        /// Load → core op → persist → move USDC → drain events. Custody and the
        /// welfare hand-off are derived from the core's own event log against the
        /// pre-image, so the pallet never re-derives the 40 / 60 split (07 §5.5).
        fn run_scoped(
            epoch: EpochId,
            op: impl FnOnce(&mut Registry) -> Result<(), CoreError>,
        ) -> DispatchResult {
            Self::run_scoped_with_rebate_progress(epoch, op).map(|_| ())
        }

        /// Run a scoped op and report whether a close crank made payable progress:
        /// a surviving latch-once extension or an upheld/accepted closure. A
        /// quorum-failure rejection is deliberately unpaid hygiene work. The
        /// boolean is returned only after persistence, custody and event sinks
        /// all succeed, so a failed/no-op crank can never earn a rebate.
        fn run_scoped_with_rebate_progress(
            epoch: EpochId,
            op: impl FnOnce(&mut Registry) -> Result<(), CoreError>,
        ) -> Result<bool, DispatchError> {
            let LoadCtx {
                mut reg,
                pre,
                count_epochs,
                agg_epochs,
            } = Self::load(epoch);
            op(&mut reg).map_err(Self::map_core_error)?;
            let favorable_closure = reg.events.iter().any(|event| {
                matches!(
                    event,
                    CoreEvent::IncidentUpheld { .. } | CoreEvent::MilestoneAccepted { .. }
                )
            });
            let surviving_extension = reg.events.iter().any(|event| {
                let CoreEvent::WindowExtended {
                    epoch, filing_id, ..
                } = event
                else {
                    return false;
                };
                reg.filings.iter().any(|((e, id), filing)| {
                    e == epoch
                        && id == filing_id
                        && matches!(filing.state, FilingState::Filed { extended: true, .. })
                })
            });
            let rebate_progress = favorable_closure || surviving_extension;
            Self::persist(epoch, &reg, &pre, &count_epochs, &agg_epochs);
            Self::settle_custody(epoch, &reg.events, &pre)?;
            // Draining may invoke the welfare sink, which can refuse — propagate
            // so the whole op (storage + custody) rolls back on refusal (G-1).
            Self::drain_events(&mut reg)?;
            Ok(rebate_progress)
        }

        // ------------------------------------------------------------- custody

        /// Escrow `amount` USDC from `who` into the sovereign account.
        fn hold_bond(who: &T::AccountId, amount: Balance) -> DispatchResult {
            ensure!(amount > 0, Error::<T, I>::BondBelowMinimum);
            T::Collateral::transfer(
                Self::usdc(),
                who,
                &Self::account_id(),
                amount,
                Preservation::Preserve,
            )?;
            Ok(())
        }

        /// Apply the custody consequences of a scoped op from its drained event
        /// log against the pre-image filings:
        ///
        /// * `FilingBondSlashed` (challenged resolution): the winner (the non-loser
        ///   party) is refunded its own bond + the 40 % share; INSURANCE takes the
        ///   remainder; the loser forfeits everything.
        /// * a terminal event **without** a slash (unchallenged close): the filer's
        ///   bond is refunded in full (07 §4/§7 — an upheld filing, or a
        ///   quorum-short rejection, both return the reporter's bond).
        fn settle_custody(
            epoch: EpochId,
            events: &[CoreEvent],
            pre: &[(FilingId, Filing)],
        ) -> DispatchResult {
            let sovereign = Self::account_id();
            let slashed: Vec<FilingId> = events
                .iter()
                .filter_map(|e| match e {
                    CoreEvent::FilingBondSlashed { filing_id, .. } => Some(*filing_id),
                    _ => None,
                })
                .collect();
            for ev in events {
                match ev {
                    CoreEvent::FilingBondSlashed {
                        filing_id,
                        loser,
                        amount,
                        challenger_share,
                        insurance_share,
                        ..
                    } => {
                        let filing = pre
                            .iter()
                            .find(|(id, _)| id == filing_id)
                            .map(|(_, f)| *f)
                            .ok_or(Error::<T, I>::FilingNotFound)?;
                        let challenger = match filing.state {
                            FilingState::Challenged { challenger, .. } => challenger,
                            // The pre-image of a resolved challenge is always
                            // `Challenged`; anything else is an internal invariant
                            // breach — reject (G-1) rather than misroute funds.
                            _ => return Err(Error::<T, I>::TryStateViolation.into()),
                        };
                        let winner = if *loser == filing.who {
                            challenger
                        } else {
                            filing.who
                        };
                        let winner = T::AccountId::from(winner);
                        // Winner: own bond back + 40 % of the loser's forfeited bond.
                        let winner_payout = filing
                            .bond
                            .checked_add(*challenger_share)
                            .ok_or(Error::<T, I>::Overflow)?;
                        Self::pay(&sovereign, &winner, winner_payout)?;
                        Self::pay(&sovereign, &T::InsuranceAccount::get(), *insurance_share)?;
                        // `amount` (the loser's bond) is fully accounted:
                        // challenger_share + insurance_share == amount (07 §5.5).
                        debug_assert_eq!(
                            challenger_share.saturating_add(*insurance_share),
                            *amount
                        );
                    }
                    CoreEvent::IncidentUpheld { filing_id, .. }
                    | CoreEvent::IncidentRejected { filing_id, .. }
                    | CoreEvent::MilestoneAccepted { filing_id, .. }
                    | CoreEvent::MilestoneRejected { filing_id, .. } => {
                        if slashed.contains(filing_id) {
                            continue; // challenged path — handled above
                        }
                        let filing = pre
                            .iter()
                            .find(|(id, _)| id == filing_id)
                            .map(|(_, f)| *f)
                            .ok_or(Error::<T, I>::FilingNotFound)?;
                        let filer = T::AccountId::from(filing.who);
                        Self::pay(&sovereign, &filer, filing.bond)?;
                    }
                    _ => {}
                }
            }
            let _ = epoch;
            Ok(())
        }

        /// Sovereign → `to` USDC payout, skipping zero amounts. The sovereign is a
        /// protocol account holding exactly the live escrows, so a payout that
        /// drains its last bond (all filings resolved) MUST be admissible —
        /// `Expendable` (unlike the `Preserve` used when a live user escrows a
        /// bond, which keeps the user's account alive).
        fn pay(from: &T::AccountId, to: &T::AccountId, amount: Balance) -> DispatchResult {
            if amount == 0 {
                return Ok(());
            }
            T::Collateral::transfer(Self::usdc(), from, to, amount, Preservation::Expendable)?;
            Ok(())
        }

        // -------------------------------------------------------------- events

        /// Translate the core event log into pallet events, bridging `[u8; 32] →
        /// T::AccountId` and firing the [`Config::Welfare`] side-effect on close.
        /// Returns the welfare sink's `DispatchResult` so a refusal aborts the op.
        fn drain_events(reg: &mut Registry) -> DispatchResult {
            for ev in core::mem::take(&mut reg.events) {
                match ev {
                    CoreEvent::IncidentFiled {
                        epoch,
                        filing_id,
                        who,
                        class,
                        evidence_hash,
                        bond,
                    } => Self::deposit_event(Event::IncidentFiled {
                        epoch,
                        filing_id,
                        who: T::AccountId::from(who),
                        class,
                        evidence_hash,
                        bond,
                    }),
                    CoreEvent::MilestoneFiled {
                        epoch,
                        filing_id,
                        who,
                        class,
                        points,
                        evidence_hash,
                        bond,
                    } => Self::deposit_event(Event::MilestoneFiled {
                        epoch,
                        filing_id,
                        who: T::AccountId::from(who),
                        class,
                        points,
                        evidence_hash,
                        bond,
                    }),
                    CoreEvent::IncidentChallenged {
                        epoch,
                        filing_id,
                        challenger,
                        evidence_hash,
                        bond,
                    } => Self::deposit_event(Event::IncidentChallenged {
                        epoch,
                        filing_id,
                        challenger: T::AccountId::from(challenger),
                        evidence_hash,
                        bond,
                    }),
                    CoreEvent::MilestoneChallenged {
                        epoch,
                        filing_id,
                        challenger,
                        evidence_hash,
                        bond,
                    } => Self::deposit_event(Event::MilestoneChallenged {
                        epoch,
                        filing_id,
                        challenger: T::AccountId::from(challenger),
                        evidence_hash,
                        bond,
                    }),
                    CoreEvent::IncidentUpheld { epoch, filing_id } => {
                        Self::deposit_event(Event::IncidentUpheld { epoch, filing_id })
                    }
                    CoreEvent::IncidentRejected { epoch, filing_id } => {
                        Self::deposit_event(Event::IncidentRejected { epoch, filing_id })
                    }
                    CoreEvent::MilestoneAccepted { epoch, filing_id } => {
                        Self::deposit_event(Event::MilestoneAccepted { epoch, filing_id })
                    }
                    CoreEvent::MilestoneRejected { epoch, filing_id } => {
                        Self::deposit_event(Event::MilestoneRejected { epoch, filing_id })
                    }
                    CoreEvent::FilingBondSlashed {
                        epoch,
                        filing_id,
                        loser,
                        amount,
                        challenger_share,
                        insurance_share,
                    } => Self::deposit_event(Event::FilingBondSlashed {
                        epoch,
                        filing_id,
                        loser: T::AccountId::from(loser),
                        amount,
                        challenger_share,
                        insurance_share,
                    }),
                    CoreEvent::RegistryEpochClosed {
                        kind,
                        epoch,
                        aggregate,
                    } => {
                        // 07 §7: hand the derived aggregate to welfare; a refusal
                        // propagates and rolls the close back (G-1).
                        T::Welfare::note_external_component(kind, epoch, aggregate)?;
                        Self::deposit_event(Event::RegistryEpochClosed {
                            kind,
                            epoch,
                            aggregate,
                        });
                    }
                    // The 07 §4 ack/extension machinery emits no registry event —
                    // it lives in 02 §7.2 (oracle), not 02 §6. Clients read the
                    // acks/extended fields from `Filings` storage. The core still
                    // logs these internally; the pallet drops them here.
                    CoreEvent::WindowAcknowledged { .. } | CoreEvent::WindowExtended { .. } => {}
                }
            }
            Ok(())
        }

        // ------------------------------------------------------------ try-state

        /// Rebuild the whole-registry aggregate (all epochs) and run the core's
        /// reviewed validator, plus FRAME-side bound/consistency assertions (15
        /// §1). Pure read; loads every live epoch's filings (bounded ≤
        /// `MAX_LIVE_EPOCHS × MaxFilingsPerEpoch`).
        pub fn do_try_state() -> Result<(), sp_runtime::TryRuntimeError> {
            use sp_runtime::TryRuntimeError;
            let mut reg = Registry::new(T::Kind::get());
            reg.bond_incident = T::Params::bond_incident();
            reg.bond_milestone = T::Params::bond_milestone();
            for (epoch, id, f) in Filings::<T, I>::iter() {
                reg.filings.push(((epoch, id), f));
            }
            for (e, c) in FilingCount::<T, I>::iter() {
                reg.filing_count.push((e, c));
            }
            for (e, a) in Aggregates::<T, I>::iter() {
                reg.aggregates.push((e, a));
            }
            ensure!(
                reg.filing_count.len() <= registry_core::MAX_LIVE_EPOCHS,
                TryRuntimeError::Other("registry: FilingCount over MAX_LIVE_EPOCHS")
            );
            ensure!(
                reg.aggregates.len() <= registry_core::MAX_AGGREGATES,
                TryRuntimeError::Other("registry: Aggregates over MAX_AGGREGATES")
            );
            // Every stored count matches the live filings for its epoch, and every
            // per-epoch count is within cap.
            for (epoch, count) in &reg.filing_count {
                ensure!(
                    *count <= T::MaxFilingsPerEpoch::get(),
                    TryRuntimeError::Other("registry: per-epoch FilingCount over cap")
                );
                let live = Filings::<T, I>::iter_prefix(*epoch).count() as u32;
                ensure!(
                    live <= *count,
                    TryRuntimeError::Other("registry: more filings than FilingCount")
                );
            }
            // Custody solvency (R-7, oracle 07 §13 analog): the sovereign's USDC
            // must cover every live escrow — one bond per `Filed` filing, two per
            // `Challenged` (filer + matching challenger), zero for terminal. And
            // the per-filing ack set is bounded by `WT_QUORUM` (07 §4).
            let mut liability: Balance = 0;
            for ((epoch, id), f) in &reg.filings {
                let sides = match f.state {
                    FilingState::Filed { .. } => 1u128,
                    FilingState::Challenged { .. } => 2,
                    FilingState::Upheld | FilingState::Rejected => 0,
                };
                liability = liability.saturating_add(f.bond.saturating_mul(sides));
                let acks = AckRecords::<T, I>::iter_key_prefix((*epoch, *id)).count() as u8;
                ensure!(
                    acks <= registry_core::WT_QUORUM,
                    TryRuntimeError::Other("registry: AckRecords over WT_QUORUM for a filing")
                );
            }
            let held = T::Collateral::balance(Self::usdc(), &Self::account_id());
            ensure!(
                held >= liability,
                TryRuntimeError::Other("registry: sovereign USDC below escrowed bond liability")
            );
            reg.try_state()
                .map_err(|_| TryRuntimeError::Other("registry core try_state failed (07 §7)"))?;
            Ok(())
        }

        /// 1:1 core-error map; `BadOrigin` never surfaces (origins are checked
        /// before the core).
        fn map_core_error(err: CoreError) -> DispatchError {
            match err {
                CoreError::EpochFull => Error::<T, I>::EpochFull.into(),
                CoreError::TooManyLiveEpochs => Error::<T, I>::TooManyLiveEpochs.into(),
                CoreError::TooManyAggregates => Error::<T, I>::TooManyAggregates.into(),
                CoreError::WindowClosed => Error::<T, I>::WindowClosed.into(),
                CoreError::WindowOpen => Error::<T, I>::WindowOpen.into(),
                CoreError::AlreadyChallenged => Error::<T, I>::AlreadyChallenged.into(),
                CoreError::AlreadyFinal => Error::<T, I>::AlreadyFinal.into(),
                CoreError::SpecVersionMismatch => Error::<T, I>::SpecVersionMismatch.into(),
                CoreError::BondBelowMinimum => Error::<T, I>::BondBelowMinimum.into(),
                CoreError::FilingNotFound => Error::<T, I>::FilingNotFound.into(),
                CoreError::DuplicateAck => Error::<T, I>::DuplicateAck.into(),
                CoreError::BatchTooLarge => Error::<T, I>::BatchTooLarge.into(),
                CoreError::InvalidClass => Error::<T, I>::InvalidClass.into(),
                CoreError::Overflow => Error::<T, I>::Overflow.into(),
                CoreError::NotRegistered => Error::<T, I>::NotRegistered.into(),
                CoreError::AlreadyQuorum => Error::<T, I>::AlreadyQuorum.into(),
                CoreError::MilestoneTargetUnset => Error::<T, I>::MilestoneTargetUnset.into(),
            }
        }
    }
}
