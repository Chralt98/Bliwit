#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! # `pallet-guardian` — 7-seat council, powers and emergency playbooks (A10)
//!
//! Production FRAME shell over the frame-free functional core
//! [`guardian_core`], which stays the differential oracle (Python M3 ≡ Rust core
//! ≡ this pallet) and the auditor-consumable port. Every extrinsic follows the
//! Track-A shell loop: resolve an explicit origin → **load** the decomposed
//! storage into a concrete [`guardian_core::Guardian`] aggregate → **call** the
//! core state machine → **persist** the mutated fields back into bounded storage
//! → **drain** the core's event log into `deposit_event`.
//!
//! Spec: `docs/architecture/06` §5 (guardians: membership/bonds/5-of-7, powers,
//! `force_rerun`, review/ratification/recall) and §6 (emergency playbooks incl.
//! `PB-LEDGER-FREEZE`); authority matrix §3.2 rows 5–7; `02 §6` (frozen guardian
//! event schema); `13 §9` (parameter change list); `15 §1` (try-state).
//!
//! ## Origins (06 §3.2)
//!
//! - `set_members`, `ratify_action`, `renew_playbook` require the
//!   `ConstitutionalValues` origin (rows 5–6: the guardian/ratify values
//!   tracks), resolved through [`Config::ValuesOrigin`].
//! - `propose_action`, `approve_action` are `Signed` council-member workflow
//!   calls (06 §8); the member check is enforced inside the core. The fifth
//!   approval dispatches the action's effect with the `GuardianHold` /
//!   `EmergencyPlaybook` origin **at runtime** (B1a wiring); in this pallet the
//!   dispatch is recorded and surfaced as the frozen `GuardianAction` /
//!   `ForceRerun` / `PlaybookActivated` events, exactly as the core models it.
//!
//! ## Scope boundary (deferred, like A1's PoV weights → B5)
//!
//! - **Bonds** mirror the core's arithmetic bond ledger (`MemberBonds`,
//!   slashed 50% on a failed review). Real `fungible` reserve/slash against
//!   member accounts (destination, election-time funding) is runtime-integration
//!   work — A10 is not audit-scope-A (R-7 lists A1/A2/A11).
//! - The downstream **effect dispatch** of a guardian action (pause intake in
//!   `pallet-epoch`, freeze the ledger/market, TWAP reset) travels through the
//!   `GuardianHold` / `EmergencyPlaybook` runtime origins wired in B1a.

extern crate alloc;

pub use guardian_core;
pub use pallet::*;
pub use weights::WeightInfo;

pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

// The functional core is the semantic source of truth; re-export the surface
// the runtime and tests consume (named, not glob — the pallet defines its own
// `Error`/`Event`).
pub use guardian_core::{
    ActionId, ActivePlaybook, DispatchContext, Error as CoreError, Event as CoreEvent, Guardian,
    GuardianPower, PendingAction, PlaybookId, PlaybookTrigger, ProposalStatus, ReviewRecord,
    TriggerState, ACTION_EXPIRY_BLOCKS, GUARDIAN_BOND, GUARDIAN_SEATS, GUARDIAN_THRESHOLD,
    REVIEW_DEADLINE_EPOCHS, REVIEW_SLASH_PERCENT,
};

use futarchy_primitives::{AccountId as CoreAccountId, EpochId, ProposalId};

/// Pallet-owned storage bounds (13 §4 has no guardian rows yet; per 13 rule 1
/// these per-pallet storage-bound arguments live with the owning pallet). The
/// v4 02 §7.5 amendment freezes the attestor storage only; guardian bounds remain
/// owned here. Each matches the corresponding internal cap in `guardian_core`.
///
/// `PendingActions`: the core admits `< 64` live proposed actions.
pub const MAX_PENDING_ACTIONS: u32 = 64;
/// `Approvals`: ≤ [`MAX_PENDING_ACTIONS`] × [`GUARDIAN_SEATS`] distinct
/// `(action, member)` pairs (no duplicate approvals — core-enforced).
pub const MAX_APPROVALS: u32 = MAX_PENDING_ACTIONS * GUARDIAN_SEATS as u32;
/// `ReviewDeadlines`: the core admits `< 128` open review records.
pub const MAX_REVIEWS: u32 = 128;
/// `ActivePlaybooks`: the six enumerated playbooks (06 §6.2).
pub const MAX_ACTIVE_PLAYBOOKS: u32 = 6;
/// `RerunUsed`: bounds the "one guardian rerun per proposal, ever" ledger. The
/// core keeps this set unbounded; the pallet caps it (overflow ⇒ a rejected
/// no-op per G-1). At ≤ 1 rerun/epoch the cap spans centuries; migrating the
/// ledger to a reaped map keyed by proposal id is a follow-up (PLAN spec-note).
pub const MAX_RERUN_USED: u32 = 512;

/// Guardian-side proposal status feed (reads `pallet-epoch`). Supplies the
/// admissibility context for `delay_once` / `force_rerun` (06 §5.3): the
/// proposal's current [`ProposalStatus`] and whether it is already inside a
/// rerun. The runtime wires this to `pallet-epoch` (A8/B1a).
pub trait GuardianProposalStatus {
    /// `(status, in_rerun)` for `pid`; unknown proposals read as
    /// `(ProposalStatus::Other, false)` so the core rejects the action.
    fn status(pid: ProposalId) -> (ProposalStatus, bool);
}

/// Verified on-chain trigger feed for playbook activation / `suspend_on_gate`
/// (06 §6.2). The runtime aggregates the constitution `PhaseFlags`, oracle and
/// ledger signals (B1a); a guardian can never activate a playbook whose trigger
/// is not live.
pub trait GuardianTriggers {
    /// The current [`TriggerState`].
    fn current() -> TriggerState;
}

/// Atomic downstream effect of a fifth-approved guardian action. The runtime
/// maps the exhaustive power enum to the narrow epoch/playbook producers; an
/// unavailable effect rejects and rolls the approval back (G-1).
pub trait GuardianEffectDispatcher {
    fn dispatch(
        power: GuardianPower,
        justification_hash: futarchy_primitives::H256,
    ) -> Result<(), sp_runtime::DispatchError>;
}

/// Retrospective-review scheduler (06 §5.4). On dispatch the guardian pallet
/// auto-submits a `ratify`-track referendum; the runtime wires this to
/// `pallet-referenda` (B1a) and returns the referendum index that the frozen
/// `ReviewScheduled { action, referendum }` event carries (02 §6).
pub trait GuardianReviewScheduler {
    /// Submit the review referendum for `action_id`; return its index.
    fn schedule_review(action_id: ActionId) -> Result<u32, sp_runtime::DispatchError>;
    /// Refund the closed review's pro-rata submission-deposit fronting.
    fn refund_review(action_id: ActionId, referendum: u32)
        -> Result<(), sp_runtime::DispatchError>;
}

/// Recall scheduler (06 §5.4): when a retrospective review misses its 2-epoch
/// deadline the approving members are slashed **and** "a recall referendum is
/// auto-scheduled on the `guardian` track". Mirrors [`GuardianReviewScheduler`];
/// the runtime wires this to `pallet-referenda` (B1a) and returns the recall
/// referendum index. Scheduling failure must roll back the maintenance step.
pub trait GuardianRecallScheduler {
    /// Submit the recall referendum for the failed `action_id`; return its index.
    fn schedule_recall(action_id: ActionId) -> Result<u32, sp_runtime::DispatchError>;
}

/// Maps an authority role to a concrete origin so benchmarks can exercise each
/// call with its exact 06 §3.2 authority.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<RuntimeOrigin> {
    /// A `Signed` origin for the given council member account.
    fn signed(who: CoreAccountId) -> RuntimeOrigin;
    /// An origin that [`Config::ValuesOrigin`] accepts (`ConstitutionalValues`).
    fn values() -> RuntimeOrigin;
    /// Prime the cross-pallet feeds so a dispatching approval succeeds: a
    /// rerunnable/`Queued` proposal status and every verified trigger live. In a
    /// real runtime this seeds the equivalent `pallet-epoch`/oracle/ledger state.
    fn prime_for_worst_case();
    /// Advance the real epoch feed for the maintenance benchmark.
    fn prime_maintenance_epoch(epoch: EpochId);
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use alloc::vec::Vec;
    use frame_support::pallet_prelude::*;
    use frame_support::traits::EnsureOrigin;
    use frame_system::pallet_prelude::*;
    use sp_runtime::{SaturatedConversion, TryRuntimeError};

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    // The council is the concrete 32-byte runtime account (AccountId32, 02 §8);
    // the frame-free core is written against `[u8; 32]`, so the pallet bridges
    // `T::AccountId ↔ [u8; 32]` at the call/event boundary. Bounding the
    // supertrait's associated type propagates it to every `impl<T: Config>`.
    #[pallet::config]
    pub trait Config:
        frame_system::Config<
        AccountId: Into<CoreAccountId> + From<CoreAccountId>,
        RuntimeEvent: From<Event<Self>>,
    >
    {
        /// The `ConstitutionalValues` origin for the guardian/ratify values
        /// tracks (06 §3.2 rows 5–6). Governs `set_members`, `ratify_action`
        /// and `renew_playbook`. No signed/unsigned origin resolves here.
        type ValuesOrigin: EnsureOrigin<Self::RuntimeOrigin>;

        /// Current epoch index (06 §5.2 allowances, §5.4 review deadlines).
        /// Wired to `pallet-epoch`'s clock by the runtime; a constant in mocks.
        type CurrentEpoch: Get<EpochId>;

        /// Proposal-status feed for rerun admissibility (06 §5.3).
        type ProposalStatusProvider: GuardianProposalStatus;

        /// Verified-trigger feed for playbook activation (06 §6.2).
        type TriggerProvider: GuardianTriggers;

        /// Atomic cross-pallet effect of the fifth approval (06 §5.1).
        type EffectDispatcher: GuardianEffectDispatcher;

        /// Retrospective-review referendum scheduler (06 §5.4).
        type ReviewScheduler: GuardianReviewScheduler;

        /// Recall referendum scheduler for review-deadline failures (06 §5.4).
        type RecallScheduler: GuardianRecallScheduler;

        /// Weight information for extrinsics and the maintenance hook.
        type WeightInfo: WeightInfo;

        /// Origin construction for benchmarking (see [`BenchmarkHelper`]).
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: BenchmarkHelper<Self::RuntimeOrigin>;
    }

    /// Per-epoch / per-window allowance counters (06 §5.2), surfaced to the FE
    /// as `guardian.Allowances` (06 §8). Mirrors the core's allowance fields.
    #[derive(
        Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo, Default,
    )]
    pub struct AllowanceState {
        /// `delay_once` uses this epoch (≤ 2 per epoch).
        pub delay_used_this_epoch: u8,
        /// `force_rerun` uses this epoch (≤ 1 per epoch).
        pub force_rerun_used_this_epoch: u8,
        /// First epoch of the current `pause_intake` 4-epoch window.
        pub pause_window_start: EpochId,
        /// `pause_intake` uses in the current window (≤ 1 per 4 epochs).
        pub pause_used_in_window: u8,
    }

    /// The seven elected council members (06 §5.1). `None` until genesis or the
    /// first `set_members`; every workflow call requires it (`NotInitialized`).
    #[pallet::storage]
    pub type Members<T: Config> = StorageValue<_, [CoreAccountId; GUARDIAN_SEATS], OptionQuery>;

    /// Per-seat bond ledger, parallel to [`Members`] (06 §5.1: 50,000 VIT held).
    /// Slashed 50% on a failed review (§5.4); real `fungible` holds are B-track.
    #[pallet::storage]
    pub type MemberBonds<T: Config> =
        StorageValue<_, [futarchy_primitives::Balance; GUARDIAN_SEATS], ValueQuery>;

    /// Live proposed actions awaiting their fifth approval (06 §5.1; FE:
    /// `guardian.PendingActions`). Expire un-dispatched after 3 days.
    #[pallet::storage]
    pub type PendingActions<T: Config> =
        StorageValue<_, BoundedVec<PendingAction, ConstU32<MAX_PENDING_ACTIONS>>, ValueQuery>;

    /// `(action_id, member)` approval tallies (06 §5.1; FE: `guardian.Approvals`).
    #[pallet::storage]
    pub type Approvals<T: Config> =
        StorageValue<_, BoundedVec<(ActionId, CoreAccountId), ConstU32<MAX_APPROVALS>>, ValueQuery>;

    /// Open retrospective-review records with their 2-epoch deadlines (06 §5.4;
    /// FE: `guardian.ReviewDeadlines`).
    #[pallet::storage]
    pub type ReviewDeadlines<T: Config> =
        StorageValue<_, BoundedVec<ReviewRecord, ConstU32<MAX_REVIEWS>>, ValueQuery>;

    /// Currently active playbooks with expiry/renewal state (06 §6.2; FE:
    /// `guardian.ActivePlaybooks`).
    #[pallet::storage]
    pub type ActivePlaybooks<T: Config> =
        StorageValue<_, BoundedVec<ActivePlaybook, ConstU32<MAX_ACTIVE_PLAYBOOKS>>, ValueQuery>;

    /// The "one guardian rerun per proposal, ever" ledger (06 §5.3).
    #[pallet::storage]
    pub type RerunUsed<T: Config> =
        StorageValue<_, BoundedVec<ProposalId, ConstU32<MAX_RERUN_USED>>, ValueQuery>;

    /// Allowance counters (06 §5.2; FE: `guardian.Allowances`).
    #[pallet::storage]
    pub type Allowances<T: Config> = StorageValue<_, AllowanceState, ValueQuery>;

    /// Monotonic action-id cursor.
    #[pallet::storage]
    pub type NextActionId<T: Config> = StorageValue<_, ActionId, ValueQuery>;

    /// Last epoch observed, for lazy per-epoch allowance resets (mirrors the
    /// core's `set_epoch`).
    #[pallet::storage]
    pub type LastSeenEpoch<T: Config> = StorageValue<_, EpochId, ValueQuery>;

    /// Internal action→referendum join used to refund the review deposit.
    /// Live cardinality is bounded by [`ReviewDeadlines`].
    #[pallet::storage]
    pub type ReviewReferenda<T: Config> =
        StorageMap<_, Blake2_128Concat, ActionId, u32, OptionQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        // ---- Frozen guardian event schema (02 §6, byte-for-byte) ----
        /// A 5-of-7 action dispatched (06 §5.4).
        GuardianAction {
            action_id: ActionId,
            power: GuardianPower,
            target: guardian_core::ActionTarget,
            justification_hash: futarchy_primitives::H256,
        },
        /// A `force_rerun` reopened a proposal's books (06 §5.3).
        ForceRerun {
            pid: ProposalId,
            justification_hash: futarchy_primitives::H256,
            window_end: futarchy_primitives::BlockNumber,
        },
        /// A playbook was activated on a live trigger (06 §6.2).
        PlaybookActivated {
            id: PlaybookId,
            trigger: PlaybookTrigger,
            expiry: futarchy_primitives::BlockNumber,
        },
        /// `PB-LEDGER-FREEZE` renewed once via a values referendum (06 §6.3).
        PlaybookRenewed { id: PlaybookId },
        /// A playbook expired and its effects reverted (06 §6.2).
        PlaybookExpired { id: PlaybookId },
        /// A retrospective review was scheduled on the `ratify` track (06 §5.4);
        /// `referendum` is the index returned by [`Config::ReviewScheduler`].
        ReviewScheduled { action: ActionId, referendum: u32 },

        // ---- Workflow / accountability events (append-only additions) ----
        /// The council membership was (re)elected (06 §5.1).
        MembersSet {
            members: [T::AccountId; GUARDIAN_SEATS],
        },
        /// A member proposed an action (06 §5.1).
        ActionProposed {
            action_id: ActionId,
            power: GuardianPower,
        },
        /// A member approved an action (06 §5.1).
        ActionApproved {
            action_id: ActionId,
            who: T::AccountId,
            approvals: u8,
        },
        /// A retrospective review passed and was ratified (06 §5.4).
        ActionRatified { action: ActionId },
        /// A review missed its deadline: each approver slashed 50% (06 §5.4).
        ReviewFailed {
            action: ActionId,
            slashed_each: futarchy_primitives::Balance,
        },
        /// A recall referendum was auto-scheduled on the `guardian` track for a
        /// failed review (06 §5.4); `referendum` is the index returned by
        /// [`Config::RecallScheduler`].
        RecallScheduled { action: ActionId, referendum: u32 },
    }

    /// 1:1 with [`CoreError`]; `CoreError::BadOrigin` maps to
    /// `DispatchError::BadOrigin` (FRAME convention).
    #[pallet::error]
    pub enum Error<T> {
        /// The council has not been elected yet (no `Members`).
        NotInitialized,
        /// Caller is not a current council member.
        NotMember,
        /// A proposed member set contains a duplicate seat.
        DuplicateMember,
        /// The member already approved this action.
        DuplicateApproval,
        /// No pending action with that id.
        ActionNotFound,
        /// The action's 3-day window elapsed.
        ActionExpired,
        /// The action already dispatched.
        AlreadyDispatched,
        /// Live pending-action set is full (`MAX_PENDING_ACTIONS`).
        TooManyPending,
        /// Approval ledger is full (`MAX_APPROVALS`).
        TooManyApprovals,
        /// Open-review set is full (`MAX_REVIEWS`).
        TooManyReviews,
        /// Active-playbook set is full (`MAX_ACTIVE_PLAYBOOKS`).
        TooManyActivePlaybooks,
        /// Rerun ledger is full (`MAX_RERUN_USED`).
        TooManyReruns,
        /// Fewer than five approvals (should not surface — internal).
        ThresholdNotMet,
        /// The power's allowance is exhausted this epoch/window (06 §5.2).
        AllowanceExhausted,
        /// A hold/playbook duration exceeds its kernel maximum (06 §5.2/§6.3).
        DurationTooLong,
        /// The playbook's verified on-chain trigger is not live (06 §6.2).
        TriggerInactive,
        /// The playbook/trigger pairing is not admissible (06 §6.2).
        BadPlaybookTrigger,
        /// The proposal was already rerun, or is inside a rerun (06 §5.3).
        AlreadyRerun,
        /// The proposal is not in a rerunnable state (06 §5.3).
        NotRerunnable,
        /// No review record for that action.
        ReviewNotFound,
        /// The review was already ratified.
        AlreadyRatified,
        /// Renewal is inadmissible (not `PB-LEDGER-FREEZE`, or already renewed —
        /// 06 §6.3: one renewal only).
        RenewalNotAllowed,
        /// The playbook is already active.
        PlaybookAlreadyActive,
        /// Arithmetic overflow — rejected, never wrapped (G-1).
        Overflow,
        /// Core state validator rejected the aggregate (try-state only).
        TryStateViolation,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// Bounded maintenance crank (06 §5.4 review deadlines, §6.2 playbook
        /// expiry). Both act on bounded collections (≤ [`MAX_REVIEWS`] reviews,
        /// ≤ [`MAX_ACTIVE_PLAYBOOKS`] playbooks) and are idempotent, so a
        /// per-block sweep is bounded and self-healing (a keeper is not trusted
        /// with safety-critical slashing).
        fn on_initialize(_n: BlockNumberFor<T>) -> Weight {
            Self::sync_epoch();
            Self::run_maintenance();
            T::WeightInfo::on_initialize()
        }

        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), TryRuntimeError> {
            Self::do_try_state()
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// `guardian.set_members` — install the seven elected council members
        /// (06 §5.1). Authority: `ConstitutionalValues` (06 §3.2 row 5). Resets
        /// every seat's bond to the full 50,000 VIT and, on a re-election, drops
        /// the outgoing council's un-dispatched actions + approvals (the core's
        /// `set_members`) so no recalled member's live approval carries over —
        /// then persists the whole cleared aggregate.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::set_members())]
        pub fn set_members(
            origin: OriginFor<T>,
            members: [T::AccountId; GUARDIAN_SEATS],
        ) -> DispatchResult {
            T::ValuesOrigin::ensure_origin(origin)?;
            let raw = Self::members_to_core(&members);
            // Validate + install via the core (uniqueness, values-origin shape,
            // bond reset, pending/approvals clear); persist the full result so
            // the workflow clear reaches storage.
            let mut g = Self::load().unwrap_or_else(|| Self::empty_core(raw));
            g.set_members(guardian_core::GuardianOrigin::ConstitutionalValues, raw)
                .map_err(Self::map_core_error)?;
            Self::persist(&g)?;
            Self::drain_events(&mut g)?;
            Ok(())
        }

        /// `guardian.propose_action` — a member proposes an action (06 §5.1).
        /// `Signed`; the member check is enforced in the core. The proposer's
        /// own approval is recorded automatically.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::propose_action())]
        pub fn propose_action(
            origin: OriginFor<T>,
            power: GuardianPower,
            justification_hash: futarchy_primitives::H256,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::sync_epoch();
            let mut g = Self::load().ok_or(Error::<T>::NotInitialized)?;
            let now = Self::now();
            g.propose_action(
                Self::to_core_authorized(&who)?,
                power,
                justification_hash,
                now,
            )
            .map_err(Self::map_core_error)?;
            Self::persist(&g)?;
            Self::drain_events(&mut g)?;
            Ok(())
        }

        /// `guardian.approve_action` — a member approves a pending action
        /// (06 §5.1). `Signed`; the fifth approval dispatches the action's
        /// effect atomically (records it + schedules the retrospective review).
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::approve_action())]
        pub fn approve_action(origin: OriginFor<T>, action_id: ActionId) -> DispatchResult {
            frame_support::storage::with_storage_layer(|| {
                let who = ensure_signed(origin)?;
                Self::sync_epoch();
                let mut g = Self::load().ok_or(Error::<T>::NotInitialized)?;
                let now = Self::now();
                let ctx = Self::dispatch_context_for(&g, action_id);
                let dispatched = g
                    .approve_action(Self::to_core_authorized(&who)?, action_id, now, ctx)
                    .map_err(Self::map_core_error)?;
                if dispatched {
                    let action = g
                        .pending
                        .iter()
                        .find(|action| action.id == action_id)
                        .copied()
                        .ok_or(Error::<T>::ActionNotFound)?;
                    T::EffectDispatcher::dispatch(action.power, action.justification_hash)?;
                }
                Self::persist(&g)?;
                Self::drain_events(&mut g)?;
                Ok(())
            })
        }

        /// `guardian.ratify_action` — the `ratify` referendum records a passed
        /// retrospective review (06 §5.4; 06 §3.2 row 6). Authority:
        /// `ConstitutionalValues`.
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::ratify_action())]
        pub fn ratify_action(origin: OriginFor<T>, action_id: ActionId) -> DispatchResult {
            frame_support::storage::with_storage_layer(|| {
                T::ValuesOrigin::ensure_origin(origin)?;
                let mut g = Self::load().ok_or(Error::<T>::NotInitialized)?;
                g.ratify_action(
                    guardian_core::GuardianOrigin::ConstitutionalValues,
                    action_id,
                )
                .map_err(Self::map_core_error)?;
                let referendum =
                    ReviewReferenda::<T>::get(action_id).ok_or(Error::<T>::ReviewNotFound)?;
                T::ReviewScheduler::refund_review(action_id, referendum)?;
                Self::persist(&g)?;
                Self::drain_events(&mut g)?;
                ReviewReferenda::<T>::remove(action_id);
                Ok(())
            })
        }

        /// `guardian.renew_playbook` — the single admissible `PB-LEDGER-FREEZE`
        /// renewal via a `guardian`-track values referendum (06 §6.3; 06 §3.2
        /// row 6). Authority: `ConstitutionalValues`.
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::renew_playbook())]
        pub fn renew_playbook(origin: OriginFor<T>, id: PlaybookId) -> DispatchResult {
            T::ValuesOrigin::ensure_origin(origin)?;
            Self::sync_epoch();
            let mut g = Self::load().ok_or(Error::<T>::NotInitialized)?;
            let now = Self::now();
            g.renew_playbook(guardian_core::GuardianOrigin::ConstitutionalValues, id, now)
                .map_err(Self::map_core_error)?;
            Self::persist(&g)?;
            Self::drain_events(&mut g)?;
            Ok(())
        }
    }

    #[pallet::extra_constants]
    impl<T: Config> Pallet<T> {
        /// 06 §5.1: council size (7 seats).
        #[pallet::constant_name(GuardianSeats)]
        fn guardian_seats() -> u32 {
            GUARDIAN_SEATS as u32
        }
        /// 06 §5.1: approval threshold (5-of-7).
        #[pallet::constant_name(GuardianThreshold)]
        fn guardian_threshold() -> u8 {
            GUARDIAN_THRESHOLD
        }
        /// 06 §5.1: per-member bond (50,000 VIT).
        #[pallet::constant_name(GuardianBond)]
        fn guardian_bond() -> futarchy_primitives::Balance {
            GUARDIAN_BOND
        }
        /// 06 §5.4: retrospective-review deadline (2 epochs).
        #[pallet::constant_name(ReviewDeadlineEpochs)]
        fn review_deadline_epochs() -> EpochId {
            REVIEW_DEADLINE_EPOCHS
        }
    }

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Optional initial council. Empty ⇒ uninitialized (await `set_members`);
        /// otherwise exactly [`GUARDIAN_SEATS`] unique accounts.
        pub members: Vec<T::AccountId>,
        #[serde(skip)]
        pub _config: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                members: Vec::new(),
                _config: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            if self.members.is_empty() {
                return;
            }
            assert!(
                self.members.len() == GUARDIAN_SEATS,
                "guardian genesis: exactly {} members required",
                GUARDIAN_SEATS
            );
            let mut raw = [[0u8; 32]; GUARDIAN_SEATS];
            for (slot, m) in raw.iter_mut().zip(self.members.iter()) {
                *slot = Self::acct_to_core(m);
            }
            // Uniqueness via the core constructor (genesis-time assert is the
            // FRAME convention for an invalid chain spec — runs before any block).
            assert!(
                Guardian::new(raw).is_ok(),
                "guardian genesis: members must be unique (06 §5.1)"
            );
            Members::<T>::put(raw);
            MemberBonds::<T>::put([GUARDIAN_BOND; GUARDIAN_SEATS]);
            LastSeenEpoch::<T>::put(T::CurrentEpoch::get());
        }
    }

    impl<T: Config> Pallet<T> {
        // ---- account bridging (T::AccountId ↔ core `[u8; 32]`) ----
        /// Round-trip-checked conversion for **authorization** boundaries: a
        /// lossy `Into<[u8; 32]>` could alias two distinct runtime accounts to
        /// the same member bytes and let an outsider act as a council member
        /// (A10 Codex adversarial finding). The canonical runtime account is
        /// `AccountId32` (02 §8), whose conversion is bijective, so this always
        /// passes; it hardens against a misconfigured lossy `AccountId`.
        fn to_core_authorized(who: &T::AccountId) -> Result<CoreAccountId, DispatchError> {
            let raw = who.clone().into();
            ensure!(&T::AccountId::from(raw) == who, DispatchError::BadOrigin);
            Ok(raw)
        }
        pub(crate) fn from_core(raw: CoreAccountId) -> T::AccountId {
            T::AccountId::from(raw)
        }
        pub(crate) fn members_to_core(
            members: &[T::AccountId; GUARDIAN_SEATS],
        ) -> [CoreAccountId; GUARDIAN_SEATS] {
            let mut raw = [[0u8; 32]; GUARDIAN_SEATS];
            for (slot, m) in raw.iter_mut().zip(members.iter()) {
                *slot = m.clone().into();
            }
            raw
        }

        /// Current block as the core's `u32` block number (real runtime is u32;
        /// mocks stay well below the ceiling — `saturated_into` is exact there).
        fn now() -> futarchy_primitives::BlockNumber {
            frame_system::Pallet::<T>::block_number().saturated_into::<u32>()
        }

        /// Build a fresh core aggregate around a member set (for `set_members`
        /// before any storage exists).
        fn empty_core(members: [CoreAccountId; GUARDIAN_SEATS]) -> Guardian {
            Guardian {
                members,
                member_bonds: [GUARDIAN_BOND; GUARDIAN_SEATS],
                pending: Vec::new(),
                approvals: Vec::new(),
                reviews: Vec::new(),
                active_playbooks: Vec::new(),
                rerun_used: Vec::new(),
                delay_used_this_epoch: 0,
                force_rerun_used_this_epoch: 0,
                pause_used_epoch_window_start: 0,
                pause_used_in_window: 0,
                current_epoch: T::CurrentEpoch::get(),
                next_action_id: 0,
                events: Vec::new(),
            }
        }

        /// Assemble the concrete core aggregate from decomposed storage; `None`
        /// until the council is elected.
        fn load() -> Option<Guardian> {
            let members = Members::<T>::get()?;
            let alloc = Allowances::<T>::get();
            Some(Guardian {
                members,
                member_bonds: MemberBonds::<T>::get(),
                pending: PendingActions::<T>::get().into_inner(),
                approvals: Approvals::<T>::get().into_inner(),
                reviews: ReviewDeadlines::<T>::get().into_inner(),
                active_playbooks: ActivePlaybooks::<T>::get().into_inner(),
                rerun_used: RerunUsed::<T>::get().into_inner(),
                delay_used_this_epoch: alloc.delay_used_this_epoch,
                force_rerun_used_this_epoch: alloc.force_rerun_used_this_epoch,
                pause_used_epoch_window_start: alloc.pause_window_start,
                pause_used_in_window: alloc.pause_used_in_window,
                current_epoch: T::CurrentEpoch::get(),
                next_action_id: NextActionId::<T>::get(),
                events: Vec::new(),
            })
        }

        /// Persist the mutated aggregate. All bounded conversions are computed
        /// first (fallibly) so a bound violation is a clean no-op (G-1); only
        /// then are the infallible writes applied.
        fn persist(g: &Guardian) -> DispatchResult {
            let pending =
                BoundedVec::try_from(g.pending.clone()).map_err(|_| Error::<T>::TooManyPending)?;
            let approvals = BoundedVec::try_from(g.approvals.clone())
                .map_err(|_| Error::<T>::TooManyApprovals)?;
            let reviews =
                BoundedVec::try_from(g.reviews.clone()).map_err(|_| Error::<T>::TooManyReviews)?;
            let playbooks = BoundedVec::try_from(g.active_playbooks.clone())
                .map_err(|_| Error::<T>::TooManyActivePlaybooks)?;
            let reruns = BoundedVec::try_from(g.rerun_used.clone())
                .map_err(|_| Error::<T>::TooManyReruns)?;

            Members::<T>::put(g.members);
            MemberBonds::<T>::put(g.member_bonds);
            PendingActions::<T>::put(pending);
            Approvals::<T>::put(approvals);
            ReviewDeadlines::<T>::put(reviews);
            ActivePlaybooks::<T>::put(playbooks);
            RerunUsed::<T>::put(reruns);
            Allowances::<T>::put(AllowanceState {
                delay_used_this_epoch: g.delay_used_this_epoch,
                force_rerun_used_this_epoch: g.force_rerun_used_this_epoch,
                pause_window_start: g.pause_used_epoch_window_start,
                pause_used_in_window: g.pause_used_in_window,
            });
            NextActionId::<T>::put(g.next_action_id);
            Ok(())
        }

        /// Translate the core's event log into pallet events. `ReviewScheduled`
        /// is where the pallet injects the referendum index (02 §6).
        fn drain_events(g: &mut Guardian) -> DispatchResult {
            for ev in core::mem::take(&mut g.events) {
                match ev {
                    CoreEvent::MembersSet { members } => {
                        let mut mapped: [T::AccountId; GUARDIAN_SEATS] =
                            core::array::from_fn(|_| T::AccountId::from([0u8; 32]));
                        for (slot, raw) in mapped.iter_mut().zip(members.iter()) {
                            *slot = Self::from_core(*raw);
                        }
                        Self::deposit_event(Event::MembersSet { members: mapped });
                    }
                    CoreEvent::ActionProposed { action_id, power } => {
                        Self::deposit_event(Event::ActionProposed { action_id, power });
                    }
                    CoreEvent::ActionApproved {
                        action_id,
                        who,
                        approvals,
                    } => {
                        Self::deposit_event(Event::ActionApproved {
                            action_id,
                            who: Self::from_core(who),
                            approvals,
                        });
                    }
                    CoreEvent::GuardianAction {
                        action_id,
                        power,
                        target,
                        justification_hash,
                    } => {
                        Self::deposit_event(Event::GuardianAction {
                            action_id,
                            power,
                            target,
                            justification_hash,
                        });
                    }
                    CoreEvent::ForceRerun {
                        pid,
                        justification_hash,
                        window_end,
                    } => {
                        Self::deposit_event(Event::ForceRerun {
                            pid,
                            justification_hash,
                            window_end,
                        });
                    }
                    CoreEvent::PlaybookActivated {
                        id,
                        trigger,
                        expiry,
                    } => {
                        Self::deposit_event(Event::PlaybookActivated {
                            id,
                            trigger,
                            expiry,
                        });
                    }
                    CoreEvent::PlaybookRenewed { id } => {
                        Self::deposit_event(Event::PlaybookRenewed { id });
                    }
                    CoreEvent::PlaybookExpired { id } => {
                        Self::deposit_event(Event::PlaybookExpired { id });
                    }
                    CoreEvent::ReviewScheduled { action } => {
                        let referendum = T::ReviewScheduler::schedule_review(action)?;
                        ReviewReferenda::<T>::insert(action, referendum);
                        Self::deposit_event(Event::ReviewScheduled { action, referendum });
                    }
                    CoreEvent::ActionRatified { action } => {
                        Self::deposit_event(Event::ActionRatified { action });
                    }
                    CoreEvent::ReviewFailed {
                        action,
                        slashed_each,
                    } => {
                        // 06 §5.4: the missed deadline also auto-schedules a
                        // recall referendum on the `guardian` track.
                        // The recall substrate can fail (SQ-146). Isolate it
                        // so a partial scheduler write rolls back without
                        // erasing the already-persisted slash, review cleanup,
                        // and `ReviewFailed` accountability signal.
                        let scheduled = frame_support::storage::with_storage_layer(|| {
                            T::RecallScheduler::schedule_recall(action)
                        });
                        // Emit/write accountability only after the child
                        // layer has committed or rolled back; a failed child
                        // must not erase these outer effects.
                        ReviewReferenda::<T>::remove(action);
                        Self::deposit_event(Event::ReviewFailed {
                            action,
                            slashed_each,
                        });
                        if let Ok(referendum) = scheduled {
                            Self::deposit_event(Event::RecallScheduled { action, referendum });
                        }
                    }
                }
            }
            Ok(())
        }

        /// Build the [`DispatchContext`] for approving `action_id`: triggers
        /// from [`Config::TriggerProvider`], and (for pid-targeted powers) the
        /// proposal status from [`Config::ProposalStatusProvider`].
        fn dispatch_context_for(g: &Guardian, action_id: ActionId) -> DispatchContext {
            let triggers = T::TriggerProvider::current();
            let (proposal_status, in_rerun) = g
                .pending
                .iter()
                .find(|a| a.id == action_id)
                .and_then(|a| match a.power {
                    GuardianPower::DelayOnce { pid } | GuardianPower::ForceRerun { pid } => {
                        Some(pid)
                    }
                    _ => None,
                })
                .map(T::ProposalStatusProvider::status)
                .unwrap_or((ProposalStatus::Other, false));
            DispatchContext {
                proposal_status,
                in_rerun,
                triggers,
            }
        }

        /// Lazy per-epoch allowance reset (mirrors the core's `set_epoch`).
        fn sync_epoch() {
            let now_epoch = T::CurrentEpoch::get();
            if now_epoch != LastSeenEpoch::<T>::get() {
                Allowances::<T>::mutate(|a| {
                    a.delay_used_this_epoch = 0;
                    a.force_rerun_used_this_epoch = 0;
                });
                LastSeenEpoch::<T>::put(now_epoch);
            }
        }

        /// Bounded maintenance (06 §5.4/§6.2): expire due playbooks, enforce
        /// overdue review deadlines, then reap terminal actions/approvals/reviews
        /// so the live-slot caps stay concurrent (not lifetime). Idempotent and
        /// no-op-safe.
        fn run_maintenance() {
            let _ = frame_support::storage::with_storage_layer(|| -> DispatchResult {
                let Some(mut g) = Self::load() else {
                    return Ok(());
                };
                let before = g.clone();
                g.expire_playbooks(Self::now());
                g.enforce_reviews(T::CurrentEpoch::get())
                    .map_err(Self::map_core_error)?;
                g.reap_terminal(Self::now());
                if g.events.is_empty()
                    && g.active_playbooks == before.active_playbooks
                    && g.reviews == before.reviews
                    && g.member_bonds == before.member_bonds
                    && g.pending == before.pending
                    && g.approvals == before.approvals
                {
                    return Ok(()); // nothing changed — skip the writes
                }
                Self::persist(&g)?;
                Self::drain_events(&mut g)
            });
        }

        /// Read helper: the current council (view for the FE / sibling pallets).
        pub fn members() -> Option<[T::AccountId; GUARDIAN_SEATS]> {
            Members::<T>::get().map(|raw| core::array::from_fn(|i| Self::from_core(raw[i])))
        }

        /// Read helper: is a playbook currently active?
        pub fn playbook_active(id: PlaybookId) -> bool {
            ActivePlaybooks::<T>::get().iter().any(|p| p.id == id)
        }

        /// Rebuild the core aggregate and run its reviewed validator plus the
        /// FRAME-side bound checks (15 §1). Pure read.
        pub fn do_try_state() -> Result<(), TryRuntimeError> {
            // Bounds are enforced by the `BoundedVec` storage types; assert them
            // again for defence in depth, then delegate machine invariants.
            ensure!(
                PendingActions::<T>::get().len() as u32 <= MAX_PENDING_ACTIONS,
                TryRuntimeError::Other("guardian: PendingActions over bound")
            );
            ensure!(
                ReviewDeadlines::<T>::get().len() as u32 <= MAX_REVIEWS,
                TryRuntimeError::Other("guardian: ReviewDeadlines over bound")
            );
            ensure!(
                ActivePlaybooks::<T>::get().len() as u32 <= MAX_ACTIVE_PLAYBOOKS,
                TryRuntimeError::Other("guardian: ActivePlaybooks over bound")
            );
            if let Some(g) = Self::load() {
                g.try_state().map_err(|_| {
                    TryRuntimeError::Other("guardian core try_state failed (06 §5/§6)")
                })?;
                // I-23 (15 §1, "guardian: I-23 scope"): guardian actions ∈ the
                // enumerated power set (structural: the `GuardianPower` enum) and
                // **every action is reviewed** — so every dispatched action still
                // resident in `PendingActions` must have a `ReviewDeadlines`
                // record (the maintenance crank reaps the two together).
                for action in g.pending.iter().filter(|a| a.dispatched) {
                    ensure!(
                        g.reviews.iter().any(|r| r.action_id == action.id),
                        TryRuntimeError::Other(
                            "guardian I-23: a dispatched action has no review record"
                        )
                    );
                }
            }
            Ok(())
        }

        pub(crate) fn map_core_error(err: CoreError) -> DispatchError {
            match err {
                CoreError::BadOrigin => DispatchError::BadOrigin,
                CoreError::NotMember => Error::<T>::NotMember.into(),
                CoreError::DuplicateMember => Error::<T>::DuplicateMember.into(),
                CoreError::DuplicateApproval => Error::<T>::DuplicateApproval.into(),
                CoreError::ActionNotFound => Error::<T>::ActionNotFound.into(),
                CoreError::ActionExpired => Error::<T>::ActionExpired.into(),
                CoreError::AlreadyDispatched => Error::<T>::AlreadyDispatched.into(),
                CoreError::TooManyPending => Error::<T>::TooManyPending.into(),
                CoreError::TooManyApprovals => Error::<T>::TooManyApprovals.into(),
                CoreError::TooManyReviews => Error::<T>::TooManyReviews.into(),
                CoreError::TooManyActivePlaybooks => Error::<T>::TooManyActivePlaybooks.into(),
                CoreError::ThresholdNotMet => Error::<T>::ThresholdNotMet.into(),
                CoreError::AllowanceExhausted => Error::<T>::AllowanceExhausted.into(),
                CoreError::DurationTooLong => Error::<T>::DurationTooLong.into(),
                CoreError::TriggerInactive => Error::<T>::TriggerInactive.into(),
                CoreError::BadPlaybookTrigger => Error::<T>::BadPlaybookTrigger.into(),
                CoreError::AlreadyRerun => Error::<T>::AlreadyRerun.into(),
                CoreError::NotRerunnable => Error::<T>::NotRerunnable.into(),
                CoreError::ReviewNotFound => Error::<T>::ReviewNotFound.into(),
                CoreError::AlreadyRatified => Error::<T>::AlreadyRatified.into(),
                CoreError::RenewalNotAllowed => Error::<T>::RenewalNotAllowed.into(),
                CoreError::PlaybookAlreadyActive => Error::<T>::PlaybookAlreadyActive.into(),
                CoreError::Overflow => Error::<T>::Overflow.into(),
            }
        }
    }

    impl<T: Config> GenesisConfig<T> {
        fn acct_to_core(who: &T::AccountId) -> CoreAccountId {
            who.clone().into()
        }
    }
}
