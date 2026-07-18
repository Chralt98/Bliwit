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
//! - `ratify_action` and `uphold_veto` use the ratify-scoped values origin;
//!   `set_members`, `renew_playbook` and `recall` use the guardian-scoped
//!   values origin. The runtime adapters also accept the conservative legacy
//!   `ConstitutionalValues` origin.
//! - `propose_action`, `approve_action` are `Signed` council-member workflow
//!   calls (06 §8); the member check is enforced inside the core. The fifth
//!   approval dispatches the action's effect with the `GuardianHold` /
//!   `EmergencyPlaybook` origin **at runtime** (B1a wiring); in this pallet the
//!   dispatch is recorded and surfaced as the frozen `GuardianAction` /
//!   `ForceRerun` / `PlaybookActivated` events, exactly as the core models it.
//!
//! Seat bonds are real native-currency holds. Review deposits are temporarily
//! fronted from those holds through the pallet sovereign, restored on a verdict,
//! or counted into the 50% deadline slash before the net recall funding reaches
//! treasury MAIN.
//!
//! The downstream **effect dispatch** of a guardian action (pause intake in
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
    TriggerState, ACTION_EXPIRY_BLOCKS, ALL_PLAYBOOKS, GUARDIAN_BOND, GUARDIAN_SEATS,
    GUARDIAN_THRESHOLD, REVIEW_DEADLINE_EPOCHS, REVIEW_SLASH_PERCENT,
};

use futarchy_primitives::{AccountId as CoreAccountId, BlockNumber, EpochId, ProposalId};

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
/// At most two complete outgoing councils can await their one-epoch release;
/// a further election fails closed until maintenance frees capacity.
pub const MAX_PENDING_BOND_RELEASES: u32 = (GUARDIAN_SEATS as u32) * 2;
/// Failed accountability records share the open-review concurrency ceiling.
pub const MAX_FAILED_ACTIONS: u32 = MAX_REVIEWS;

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

    /// Fail-soft expiry reversion for a playbook's pallet effects.
    fn revert_playbook(id: PlaybookId) -> Result<(), sp_runtime::DispatchError>;

    /// Atomic LedgerFreeze renewal extension across all downstream pallets.
    fn renew_playbook(id: PlaybookId) -> Result<(), sp_runtime::DispatchError>;
}

/// Narrow T24 producer seam. The runtime implementation calls the epoch
/// pallet's internal review-authority entry point; no public epoch call exists.
pub trait GuardianProposalVeto {
    fn uphold(pid: ProposalId) -> Result<(), sp_runtime::DispatchError>;
}

/// Verdict carried by an automatically-submitted retrospective referendum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewVerdict {
    Ratify,
    UpholdVeto,
}

/// Retrospective-review scheduler (06 §5.4). Every action submits a ratification
/// referendum; `DelayOnce` additionally submits an upheld-veto referendum on
/// the same track. The runtime wires both to `pallet-referenda` (B1a), while the
/// frozen `ReviewScheduled { action, referendum }` event carries the ratify
/// index (02 §6).
pub trait GuardianReviewScheduler {
    /// Total liquid VIT needed for one review (submission + ratify decision
    /// deposits). The pallet releases this amount pro-rata from seat holds.
    fn review_deposit() -> futarchy_primitives::Balance;
    /// Submit one verdict referendum for `action_id`; return its index.
    fn schedule_review(
        action_id: ActionId,
        verdict: ReviewVerdict,
    ) -> Result<u32, sp_runtime::DispatchError>;
    /// Cancel a competing verdict if it is still ongoing. Already-closed
    /// referenda are accepted so both verdict paths remain race-safe and
    /// one-shot.
    fn cancel_review(referendum: u32) -> Result<(), sp_runtime::DispatchError>;
    /// Refund both deposits of a closed review into the guardian sovereign.
    fn refund_review(referendum: u32) -> Result<(), sp_runtime::DispatchError>;
}

/// Recall scheduler (06 §5.4): when a retrospective review misses its 2-epoch
/// deadline the approving members are slashed **and** "a recall referendum is
/// auto-scheduled on the `guardian` track". Mirrors [`GuardianReviewScheduler`];
/// the runtime wires this to `pallet-referenda` (B1a) and returns the recall
/// referendum index. Scheduling failure must roll back the maintenance step.
pub trait GuardianRecallScheduler {
    /// Submit and decision-fund the recall referendum from `slash_pool`, then
    /// forward the net remainder to treasury MAIN. A failure is returned so
    /// the pallet can forward the complete pool instead of stranding funds.
    fn schedule_recall(
        action_id: ActionId,
        slash_pool: futarchy_primitives::Balance,
    ) -> Result<u32, sp_runtime::DispatchError>;
    /// Refund a concluded recall's two deposits and forward them to MAIN.
    fn refund_recall(referendum: u32) -> Result<(), sp_runtime::DispatchError>;
    /// Forward a failed scheduling attempt's complete slash pool to MAIN.
    fn forward_failed_recall_pool(
        amount: futarchy_primitives::Balance,
    ) -> Result<(), sp_runtime::DispatchError>;
}

/// Maps an authority role to a concrete origin so benchmarks can exercise each
/// call with its exact 06 §3.2 authority.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<RuntimeOrigin> {
    /// A `Signed` origin for the given council member account.
    fn signed(who: CoreAccountId) -> RuntimeOrigin;
    /// An origin that [`Config::ValuesOrigin`] accepts (`ConstitutionalValues`).
    fn values() -> RuntimeOrigin;
    /// An origin that [`Config::AdminOrigin`] accepts (`GuardianTrack`).
    fn admin() -> RuntimeOrigin;
    /// Prime the cross-pallet feeds so a dispatching approval succeeds: a
    /// rerunnable/`Queued` proposal status and every verified trigger live. In a
    /// real runtime this seeds the equivalent `pallet-epoch`/oracle/ledger state.
    fn prime_for_worst_case();
    /// Mark the auto-submitted retrospective review approved so the measured
    /// ratification exercises the real closed-referendum refund path.
    fn prime_review_approved(action: ActionId);
    /// Advance the real epoch feed for the maintenance benchmark.
    fn prime_maintenance_epoch(epoch: EpochId);
    /// Close a seeded review so the measured verdict may refund both deposits.
    fn close_review(referendum: u32) -> Result<(), sp_runtime::DispatchError>;
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use alloc::vec::Vec;
    use frame_support::pallet_prelude::*;
    use frame_support::traits::{
        fungible::{Inspect, InspectHold, Mutate, MutateHold},
        tokens::{Fortitude, Precision, Preservation, Restriction},
        EnsureOrigin,
    };
    use frame_system::pallet_prelude::*;
    use sp_runtime::{SaturatedConversion, TryRuntimeError};

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    /// Guardian-owned VIT hold namespace (06 §5.1).
    #[pallet::composite_enum]
    pub enum HoldReason {
        SeatBond,
    }

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
        /// The `ratify`-track values origin for review verdicts.
        type ValuesOrigin: EnsureOrigin<Self::RuntimeOrigin>;

        /// The `guardian`-track values origin for membership/admin calls.
        type AdminOrigin: EnsureOrigin<Self::RuntimeOrigin>;

        /// Native VIT custody for seat bonds and fronted deposits.
        type Currency: Inspect<Self::AccountId, Balance = futarchy_primitives::Balance>
            + Mutate<Self::AccountId>
            + InspectHold<Self::AccountId, Reason = Self::RuntimeHoldReason>
            + MutateHold<Self::AccountId>;

        /// Aggregate runtime hold reason.
        type RuntimeHoldReason: From<HoldReason>;

        /// PalletId-derived account which submits and funds accountability
        /// referenda.
        type SovereignAccount: Get<Self::AccountId>;

        /// Current epoch index (06 §5.2 allowances, §5.4 review deadlines).
        /// Wired to `pallet-epoch`'s clock by the runtime; a constant in mocks.
        type CurrentEpoch: Get<EpochId>;

        /// Proposal-status feed for rerun admissibility (06 §5.3).
        type ProposalStatusProvider: GuardianProposalStatus;

        /// Verified-trigger feed for playbook activation (06 §6.2).
        type TriggerProvider: GuardianTriggers;

        /// Atomic cross-pallet effect of the fifth approval (06 §5.1).
        type EffectDispatcher: GuardianEffectDispatcher;

        /// T24 callback into the epoch proposal machine.
        type ProposalVeto: GuardianProposalVeto;

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
    pub type Members<T: Config> =
        StorageValue<_, [Option<CoreAccountId>; GUARDIAN_SEATS], OptionQuery>;

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

    /// Values-governed availability toggle for the six kernel-enumerated
    /// routines. All six are enabled at genesis (06 §6.2).
    #[pallet::storage]
    pub type PlaybookRegistered<T: Config> =
        StorageMap<_, Blake2_128Concat, PlaybookId, bool, ValueQuery>;

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

    /// Internal action→ratify-referendum join used to refund the review deposit.
    /// Live cardinality is bounded by [`ReviewDeadlines`]. This value stays a
    /// single `u32` so existing v0 storage remains decodable.
    #[pallet::storage]
    pub type ReviewReferenda<T: Config> =
        StorageMap<_, Blake2_128Concat, ActionId, u32, OptionQuery>;

    /// The second, upheld-veto referendum scheduled exactly for `DelayOnce`
    /// actions (06 §5.4). A parallel map preserves the original v0 storage
    /// encoding of [`ReviewReferenda`].
    #[pallet::storage]
    pub type VetoReviewReferenda<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ActionId, u32, OptionQuery>;

    #[derive(
        Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo, Default,
    )]
    pub struct ReviewFronting {
        pub referendum: u32,
        pub approvers: [CoreAccountId; GUARDIAN_SEATS],
        pub approver_count: u8,
        pub obligations: [futarchy_primitives::Balance; GUARDIAN_SEATS],
        pub slices: [futarchy_primitives::Balance; GUARDIAN_SEATS],
    }

    /// Exact per-action slices temporarily moved out of approver seat holds.
    #[pallet::storage]
    pub type ReviewFrontingOf<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ActionId, ReviewFronting, OptionQuery>;

    #[derive(Clone, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
    pub struct BondRelease<AccountId> {
        pub who: AccountId,
        pub amount: futarchy_primitives::Balance,
        pub release_epoch: EpochId,
    }

    /// Departed members' residual bonds, held through term plus one epoch.
    #[pallet::storage]
    pub type PendingBondReleases<T: Config> = StorageValue<
        _,
        BoundedVec<BondRelease<T::AccountId>, ConstU32<MAX_PENDING_BOND_RELEASES>>,
        ValueQuery,
    >;

    #[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
    pub struct FailedAction {
        pub approvers: [CoreAccountId; GUARDIAN_SEATS],
        pub approver_count: u8,
        pub failed_epoch: EpochId,
        pub recall_referendum: Option<u32>,
    }

    /// Deterministic recall substrate, retained for at most four epochs after
    /// failure (longer only while a recall deposit is not yet refundable).
    #[pallet::storage]
    pub type FailedActions<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ActionId, FailedAction, OptionQuery>;

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
        /// A guardian-track recall enacted; listed approvers' seats are vacant.
        RecallEnacted {
            action: ActionId,
            removed: BoundedVec<T::AccountId, ConstU32<7>>,
        },
        /// Guardian-track availability toggle for an enumerated playbook.
        PlaybookRegistrationSet { id: PlaybookId, enabled: bool },
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
        /// OracleVoid requires a cohort target; every other playbook forbids one.
        BadPlaybookTarget,
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
        /// The failed-action recall record is absent or already reaped.
        FailedActionNotFound,
        /// `uphold_veto` targets a non-delay action.
        NotDelayAction,
        /// The bounded post-term bond-release queue is full.
        TooManyBondReleases,
        /// Held funds, the obligation ledger and fronting slices disagree.
        BondAccounting,
        /// The values-governed availability toggle is disabled.
        PlaybookNotRegistered,
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
            frame_support::storage::with_storage_layer(|| {
                T::AdminOrigin::ensure_origin(origin)?;
                Self::install_members(members)
            })
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
                    if let GuardianPower::ActivatePlaybook { id, .. } = action.power {
                        ensure!(
                            PlaybookRegistered::<T>::get(id),
                            Error::<T>::PlaybookNotRegistered
                        );
                    }
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
                let _referendum =
                    ReviewReferenda::<T>::get(action_id).ok_or(Error::<T>::ReviewNotFound)?;
                if let Some(uphold_veto) = VetoReviewReferenda::<T>::get(action_id) {
                    T::ReviewScheduler::cancel_review(uphold_veto)?;
                }
                Self::refund_review_fronting(action_id)?;
                Self::persist(&g)?;
                Self::drain_events(&mut g)?;
                ReviewReferenda::<T>::remove(action_id);
                VetoReviewReferenda::<T>::remove(action_id);
                ReviewFrontingOf::<T>::remove(action_id);
                Ok(())
            })
        }

        /// `guardian.renew_playbook` — the single admissible `PB-LEDGER-FREEZE`
        /// renewal via a `guardian`-track referendum (06 §6.3; 06 §3.2 row 6).
        /// Authority: the scoped `GuardianTrack` AdminOrigin.
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::renew_playbook())]
        pub fn renew_playbook(origin: OriginFor<T>, id: PlaybookId) -> DispatchResult {
            frame_support::storage::with_storage_layer(|| {
                T::AdminOrigin::ensure_origin(origin)?;
                Self::sync_epoch();
                let mut g = Self::load().ok_or(Error::<T>::NotInitialized)?;
                let now = Self::now();
                g.renew_playbook(guardian_core::GuardianOrigin::ConstitutionalValues, id, now)
                    .map_err(Self::map_core_error)?;
                T::EffectDispatcher::renew_playbook(id)?;
                Self::persist(&g)?;
                Self::drain_events(&mut g)?;
                Ok(())
            })
        }

        /// Uphold a `delay_once` veto through its live ratify-track review. The
        /// verdict and T24 transition are one storage transaction.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::uphold_veto())]
        pub fn uphold_veto(origin: OriginFor<T>, action_id: ActionId) -> DispatchResult {
            frame_support::storage::with_storage_layer(|| {
                T::ValuesOrigin::ensure_origin(origin)?;
                let mut g = Self::load().ok_or(Error::<T>::NotInitialized)?;
                let pid = g
                    .pending
                    .iter()
                    .find(|action| action.id == action_id)
                    .and_then(|action| match action.power {
                        GuardianPower::DelayOnce { pid } => Some(pid),
                        _ => None,
                    })
                    .ok_or(Error::<T>::NotDelayAction)?;
                g.ratify_action(
                    guardian_core::GuardianOrigin::ConstitutionalValues,
                    action_id,
                )
                .map_err(Self::map_core_error)?;
                T::ProposalVeto::uphold(pid)?;
                let referendum =
                    ReviewReferenda::<T>::get(action_id).ok_or(Error::<T>::ReviewNotFound)?;
                T::ReviewScheduler::cancel_review(referendum)?;
                Self::refund_review_fronting(action_id)?;
                Self::persist(&g)?;
                Self::drain_events(&mut g)?;
                ReviewReferenda::<T>::remove(action_id);
                VetoReviewReferenda::<T>::remove(action_id);
                ReviewFrontingOf::<T>::remove(action_id);
                Ok(())
            })
        }

        /// Enact a guardian-track recall for a failed action. Every recorded
        /// approver still seated is removed; residual bonds remain held for one
        /// further epoch and live approvals are cleared fail-closed.
        #[pallet::call_index(6)]
        #[pallet::weight(T::WeightInfo::recall())]
        pub fn recall(origin: OriginFor<T>, action_id: ActionId) -> DispatchResult {
            frame_support::storage::with_storage_layer(|| {
                T::AdminOrigin::ensure_origin(origin)?;
                let failed =
                    FailedActions::<T>::get(action_id).ok_or(Error::<T>::FailedActionNotFound)?;
                let mut g = Self::load().ok_or(Error::<T>::NotInitialized)?;
                let release_epoch = T::CurrentEpoch::get()
                    .checked_add(1)
                    .ok_or(Error::<T>::Overflow)?;
                let mut releases = PendingBondReleases::<T>::get();
                let mut removed: BoundedVec<T::AccountId, ConstU32<7>> = BoundedVec::default();

                for approver in failed
                    .approvers
                    .iter()
                    .take(usize::from(failed.approver_count))
                {
                    let Some(index) = g
                        .members
                        .iter()
                        .position(|member| member.as_ref() == Some(approver))
                    else {
                        continue;
                    };
                    let who = Self::from_core(*approver);
                    removed
                        .try_push(who.clone())
                        .map_err(|_| Error::<T>::TooManyBondReleases)?;
                    let amount = g.member_bonds[index];
                    if amount > 0 {
                        releases
                            .try_push(BondRelease {
                                who,
                                amount,
                                release_epoch,
                            })
                            .map_err(|_| Error::<T>::TooManyBondReleases)?;
                    }
                }

                g.recall_members(
                    guardian_core::GuardianOrigin::ConstitutionalValues,
                    &failed.approvers[..usize::from(failed.approver_count)],
                )
                .map_err(Self::map_core_error)?;
                if let Some(referendum) = failed.recall_referendum {
                    T::RecallScheduler::refund_recall(referendum)?;
                }
                PendingBondReleases::<T>::put(releases);
                Self::persist(&g)?;
                FailedActions::<T>::remove(action_id);
                Self::deposit_event(Event::RecallEnacted {
                    action: action_id,
                    removed,
                });
                Ok(())
            })
        }

        /// Enable/disable one of the six kernel-enumerated playbooks. This is
        /// availability only; adding/amending a routine is a runtime change.
        #[pallet::call_index(7)]
        #[pallet::weight(T::WeightInfo::set_playbook_registered())]
        pub fn set_playbook_registered(
            origin: OriginFor<T>,
            id: PlaybookId,
            enabled: bool,
        ) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            PlaybookRegistered::<T>::insert(id, enabled);
            Self::deposit_event(Event::PlaybookRegistrationSet { id, enabled });
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
        /// 06 §5.2/§6.2/§6.3: hard pallet-level effect backstop.
        #[pallet::constant_name(PlaybookFreezeWindowBlocks)]
        fn playbook_freeze_window_blocks() -> BlockNumber {
            futarchy_primitives::kernel::PLAYBOOK_FREEZE_WINDOW_BLOCKS
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
            // Keep the PalletId account alive independently of Balances. This
            // lets it place the exact fronted submission + decision deposits
            // without retaining an extra existential-deposit slice.
            frame_system::Pallet::<T>::inc_providers(&T::SovereignAccount::get());
            for id in ALL_PLAYBOOKS {
                PlaybookRegistered::<T>::insert(id, true);
            }
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
            let seated = raw.map(Some);
            Members::<T>::put(seated);
            MemberBonds::<T>::put([GUARDIAN_BOND; GUARDIAN_SEATS]);
            let reason: T::RuntimeHoldReason = HoldReason::SeatBond.into();
            for member in &self.members {
                assert!(
                    T::Currency::hold(&reason, member, GUARDIAN_BOND).is_ok(),
                    "guardian genesis: each member must fund the seat bond"
                );
            }
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

        fn seat_reason() -> T::RuntimeHoldReason {
            HoldReason::SeatBond.into()
        }

        /// Install a complete incoming council while preserving outgoing bond
        /// custody for one epoch. All holds and storage writes participate in
        /// the caller's storage layer, so one underfunded account rolls the
        /// election back in full.
        fn install_members(members: [T::AccountId; GUARDIAN_SEATS]) -> DispatchResult {
            let raw = Self::members_to_core(&members);
            // Validate uniqueness before touching balances.
            Guardian::new(raw).map_err(Self::map_core_error)?;
            let existing = Self::load();
            let was_initialized = existing.is_some();
            let mut g = existing.unwrap_or_else(|| Self::empty_core(raw));
            let mut releases = PendingBondReleases::<T>::get();
            let release_epoch = T::CurrentEpoch::get()
                .checked_add(1)
                .ok_or(Error::<T>::Overflow)?;
            let reason = Self::seat_reason();

            for (who, raw_who) in members.iter().zip(raw.iter()) {
                let seated = if was_initialized {
                    g.members
                        .iter()
                        .position(|member| member.as_ref() == Some(raw_who))
                        .map(|index| g.member_bonds[index])
                } else {
                    None
                };
                let released = releases
                    .iter()
                    .position(|release| release.who == *who)
                    .map(|index| releases.remove(index).amount);
                let existing = seated.or(released).unwrap_or(0);
                ensure!(existing <= GUARDIAN_BOND, Error::<T>::BondAccounting);
                let top_up = GUARDIAN_BOND.saturating_sub(existing);
                if top_up > 0 {
                    T::Currency::hold(&reason, who, top_up)?;
                }
            }

            for (index, member) in g.members.iter().enumerate() {
                let Some(raw_member) = member else {
                    continue;
                };
                if raw.contains(raw_member) {
                    continue;
                }
                let amount = g.member_bonds[index];
                if amount > 0 {
                    releases
                        .try_push(BondRelease {
                            who: Self::from_core(*raw_member),
                            amount,
                            release_epoch,
                        })
                        .map_err(|_| Error::<T>::TooManyBondReleases)?;
                }
            }

            g.set_members(guardian_core::GuardianOrigin::ConstitutionalValues, raw)
                .map_err(Self::map_core_error)?;
            PendingBondReleases::<T>::put(releases);
            Self::persist(&g)?;
            Self::drain_events(&mut g)
        }

        fn front_review(action: ActionId) -> Result<(u32, Option<u32>), DispatchError> {
            let review = ReviewDeadlines::<T>::get()
                .iter()
                .find(|review| review.action_id == action)
                .copied()
                .ok_or(Error::<T>::ReviewNotFound)?;
            let count = usize::from(review.approver_count);
            ensure!(
                count > 0 && count <= GUARDIAN_SEATS,
                Error::<T>::BondAccounting
            );
            let is_delay_once = PendingActions::<T>::get()
                .iter()
                .find(|pending| pending.id == action)
                .is_some_and(|pending| matches!(pending.power, GuardianPower::DelayOnce { .. }));
            let referendum_count = if is_delay_once { 2 } else { 1 };
            let total = T::ReviewScheduler::review_deposit()
                .checked_mul(referendum_count)
                .ok_or(Error::<T>::Overflow)?;
            let divisor = futarchy_primitives::Balance::from(review.approver_count);
            let base = total / divisor;
            let remainder =
                usize::try_from(total % divisor).map_err(|_| Error::<T>::BondAccounting)?;
            let reason = Self::seat_reason();
            let sovereign = T::SovereignAccount::get();
            let bonds = MemberBonds::<T>::get();
            let members = Members::<T>::get().ok_or(Error::<T>::NotInitialized)?;
            let releases = PendingBondReleases::<T>::get();
            let mut fronting = ReviewFronting {
                referendum: 0,
                approvers: review.approvers,
                approver_count: review.approver_count,
                obligations: [0; GUARDIAN_SEATS],
                slices: [0; GUARDIAN_SEATS],
            };

            for (position, raw) in review.approvers.iter().take(count).enumerate() {
                let who = Self::from_core(*raw);
                let obligation = members
                    .iter()
                    .position(|member| member.as_ref() == Some(raw))
                    .map(|index| bonds[index])
                    .or_else(|| {
                        releases
                            .iter()
                            .find(|release| release.who == who)
                            .map(|release| release.amount)
                    })
                    .ok_or(Error::<T>::BondAccounting)?;
                let slice =
                    base.saturating_add(futarchy_primitives::Balance::from(position < remainder));
                let held = T::Currency::balance_on_hold(&reason, &who);
                let accounted = held
                    .checked_add(Self::outstanding_fronting(&who))
                    .ok_or(Error::<T>::BondAccounting)?;
                ensure!(accounted == obligation, Error::<T>::BondAccounting);
                ensure!(slice <= held, Error::<T>::BondAccounting);
                let moved = T::Currency::transfer_on_hold(
                    &reason,
                    &who,
                    &sovereign,
                    slice,
                    Precision::Exact,
                    Restriction::Free,
                    Fortitude::Force,
                )?;
                ensure!(moved == slice, Error::<T>::BondAccounting);
                fronting.obligations[position] = obligation;
                fronting.slices[position] = slice;
            }

            let ratify = T::ReviewScheduler::schedule_review(action, ReviewVerdict::Ratify)?;
            let uphold_veto = if is_delay_once {
                Some(T::ReviewScheduler::schedule_review(
                    action,
                    ReviewVerdict::UpholdVeto,
                )?)
            } else {
                None
            };
            fronting.referendum = ratify;
            ReviewFrontingOf::<T>::insert(action, fronting);
            Ok((ratify, uphold_veto))
        }

        fn refund_review_fronting(action: ActionId) -> DispatchResult {
            let fronting = ReviewFrontingOf::<T>::get(action).ok_or(Error::<T>::ReviewNotFound)?;
            if fronting.slices.iter().all(|slice| *slice == 0) {
                return Ok(());
            }
            let referendum = ReviewReferenda::<T>::get(action).ok_or(Error::<T>::ReviewNotFound)?;
            let veto_referendum = VetoReviewReferenda::<T>::get(action);
            ensure!(
                fronting.referendum == referendum,
                Error::<T>::BondAccounting
            );
            T::ReviewScheduler::refund_review(referendum)?;
            if let Some(uphold_veto) = veto_referendum {
                T::ReviewScheduler::refund_review(uphold_veto)?;
            }
            let reason = Self::seat_reason();
            let sovereign = T::SovereignAccount::get();
            for (position, raw) in fronting
                .approvers
                .iter()
                .take(usize::from(fronting.approver_count))
                .enumerate()
            {
                let slice = fronting.slices[position];
                if slice == 0 {
                    continue;
                }
                let who = Self::from_core(*raw);
                T::Currency::transfer(&sovereign, &who, slice, Preservation::Expendable)?;
                T::Currency::hold(&reason, &who, slice)?;
            }
            Ok(())
        }

        /// Return one concluded review's fronted slices to their seat holds
        /// without consuming the review record. Maintenance performs this for
        /// every refundable overdue review before computing any slash, so the
        /// current hold is the authoritative bounded liability.
        fn restore_due_review_fronting(action: ActionId) -> DispatchResult {
            let mut fronting =
                ReviewFrontingOf::<T>::get(action).ok_or(Error::<T>::ReviewNotFound)?;
            if fronting.slices.iter().all(|slice| *slice == 0) {
                return Ok(());
            }
            let referendum = ReviewReferenda::<T>::get(action).ok_or(Error::<T>::ReviewNotFound)?;
            let veto_referendum = VetoReviewReferenda::<T>::get(action);
            ensure!(
                fronting.referendum == referendum,
                Error::<T>::BondAccounting
            );
            T::ReviewScheduler::cancel_review(referendum)?;
            if let Some(uphold_veto) = veto_referendum {
                T::ReviewScheduler::cancel_review(uphold_veto)?;
            }
            T::ReviewScheduler::refund_review(referendum)?;
            if let Some(uphold_veto) = veto_referendum {
                T::ReviewScheduler::refund_review(uphold_veto)?;
            }
            let reason = Self::seat_reason();
            let sovereign = T::SovereignAccount::get();
            for (position, raw) in fronting
                .approvers
                .iter()
                .take(usize::from(fronting.approver_count))
                .enumerate()
            {
                let slice = fronting.slices[position];
                if slice == 0 {
                    continue;
                }
                let who = Self::from_core(*raw);
                T::Currency::transfer(&sovereign, &who, slice, Preservation::Expendable)?;
                T::Currency::hold(&reason, &who, slice)?;
                fronting.slices[position] = 0;
            }
            ReviewFrontingOf::<T>::insert(action, fronting);
            Ok(())
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
                members: members.map(Some),
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
                        // Expiry removal/event is durable even if a downstream
                        // early-clear fails. Each effect also has a lazy
                        // pallet-level time bound, so failure cannot extend it.
                        let _ = frame_support::storage::with_storage_layer(|| {
                            T::EffectDispatcher::revert_playbook(id)
                        });
                        Self::deposit_event(Event::PlaybookExpired { id });
                    }
                    CoreEvent::ReviewScheduled { action } => {
                        let (referendum, veto_referendum) = Self::front_review(action)?;
                        ReviewReferenda::<T>::insert(action, referendum);
                        if let Some(veto_referendum) = veto_referendum {
                            VetoReviewReferenda::<T>::insert(action, veto_referendum);
                        }
                        Self::deposit_event(Event::ReviewScheduled { action, referendum });
                    }
                    CoreEvent::ActionRatified { action } => {
                        Self::deposit_event(Event::ActionRatified { action });
                    }
                    CoreEvent::ReviewFailed {
                        action,
                        slashed_each,
                    } => {
                        Self::restore_due_review_fronting(action)?;
                        let (scheduled, _) = Self::settle_failed_review(action, slashed_each)?;
                        Self::deposit_event(Event::ReviewFailed {
                            action,
                            slashed_each,
                        });
                        if let Some(referendum) = scheduled {
                            Self::deposit_event(Event::RecallScheduled { action, referendum });
                        }
                    }
                }
            }
            Ok(())
        }

        /// Materialize a missed review against real holds. Review-fronted
        /// slices count toward each approver's slash; only the residual is
        /// transferred from the seat hold. Recall submission and its net
        /// treasury forwarding run in a child layer. If submission fails, the
        /// child rolls back and the complete pool is forwarded to MAIN in the
        /// outer layer, so funds are never silently stranded while the slash,
        /// `FailedActions` and `ReviewFailed` remain durable.
        fn settle_failed_review(
            action: ActionId,
            slashed_each: futarchy_primitives::Balance,
        ) -> Result<(Option<u32>, [futarchy_primitives::Balance; GUARDIAN_SEATS]), DispatchError>
        {
            let fronting = ReviewFrontingOf::<T>::get(action).ok_or(Error::<T>::ReviewNotFound)?;
            ensure!(
                fronting.slices.iter().all(|slice| *slice == 0),
                Error::<T>::BondAccounting
            );
            ensure!(
                FailedActions::<T>::contains_key(action)
                    || FailedActions::<T>::count() < MAX_FAILED_ACTIONS,
                Error::<T>::TooManyReviews
            );

            let reason = Self::seat_reason();
            let sovereign = T::SovereignAccount::get();
            let members = Members::<T>::get().ok_or(Error::<T>::NotInitialized)?;
            let mut releases = PendingBondReleases::<T>::get();
            let mut pool = 0u128;
            let mut actual_slashes = [0u128; GUARDIAN_SEATS];

            for (position, raw) in fronting
                .approvers
                .iter()
                .take(usize::from(fronting.approver_count))
                .enumerate()
            {
                let who = Self::from_core(*raw);
                let held = T::Currency::balance_on_hold(&reason, &who);
                let effective_slash = fronting.obligations[position].min(slashed_each).min(held);
                if effective_slash > 0 {
                    let moved = T::Currency::transfer_on_hold(
                        &reason,
                        &who,
                        &sovereign,
                        effective_slash,
                        Precision::Exact,
                        Restriction::Free,
                        Fortitude::Force,
                    )?;
                    ensure!(moved == effective_slash, Error::<T>::BondAccounting);
                }
                actual_slashes[position] = effective_slash;

                if members.iter().all(|member| member.as_ref() != Some(raw)) {
                    let release = releases
                        .iter_mut()
                        .find(|release| release.who == who)
                        .ok_or(Error::<T>::BondAccounting)?;
                    release.amount = release.amount.saturating_sub(effective_slash);
                }
                pool = pool
                    .checked_add(effective_slash)
                    .ok_or(Error::<T>::Overflow)?;
            }

            PendingBondReleases::<T>::put(releases);
            let mut failed = FailedAction {
                approvers: fronting.approvers,
                approver_count: fronting.approver_count,
                failed_epoch: T::CurrentEpoch::get(),
                recall_referendum: None,
            };
            let scheduled = frame_support::storage::with_storage_layer(|| {
                T::RecallScheduler::schedule_recall(action, pool)
            });
            let referendum = match scheduled {
                Ok(index) => {
                    failed.recall_referendum = Some(index);
                    Some(index)
                }
                Err(_) => {
                    T::RecallScheduler::forward_failed_recall_pool(pool)?;
                    None
                }
            };
            FailedActions::<T>::insert(action, failed);
            ReviewReferenda::<T>::remove(action);
            VetoReviewReferenda::<T>::remove(action);
            ReviewFrontingOf::<T>::remove(action);
            Ok((referendum, actual_slashes))
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
            let epoch = T::CurrentEpoch::get();
            let overdue = match Self::load() {
                Some(g) => g
                    .reviews
                    .iter()
                    .filter(|review| {
                        !review.ratified
                            && !review.recall_scheduled
                            && epoch >= review.deadline_epoch
                    })
                    .map(|review| review.action_id)
                    .collect::<Vec<_>>(),
                None => Vec::new(),
            };

            // Refund and restore every independently-refundable due slice
            // first. Each child layer is durable on its own; one unfinished or
            // corrupt review cannot erase another review's restored liability.
            for action in &overdue {
                let _ = frame_support::storage::with_storage_layer(|| {
                    Self::restore_due_review_fronting(*action)
                });
            }

            // Settle each missed review in its own transaction. A failed
            // refund/slash/recall for one action remains retryable without
            // rolling back already-settled peers.
            for action in overdue {
                let _ = frame_support::storage::with_storage_layer(|| {
                    let mut g = Self::load().ok_or(Error::<T>::NotInitialized)?;
                    let nominal = GUARDIAN_BOND
                        .saturating_mul(futarchy_primitives::Balance::from(REVIEW_SLASH_PERCENT))
                        / 100;
                    let (scheduled, actual_slashes) = Self::settle_failed_review(action, nominal)?;
                    g.mark_review_failed(action, epoch, actual_slashes)
                        .map_err(Self::map_core_error)?;
                    g.reap_terminal(Self::now());
                    Self::persist(&g)?;
                    Self::deposit_event(Event::ReviewFailed {
                        action,
                        slashed_each: nominal,
                    });
                    if let Some(referendum) = scheduled {
                        Self::deposit_event(Event::RecallScheduled { action, referendum });
                    }
                    Ok::<(), DispatchError>(())
                });
            }

            let _ = frame_support::storage::with_storage_layer(|| -> DispatchResult {
                if let Some(mut g) = Self::load() {
                    let before = g.clone();
                    g.expire_playbooks(Self::now());
                    g.reap_terminal(Self::now());
                    if !(g.events.is_empty()
                        && g.active_playbooks == before.active_playbooks
                        && g.reviews == before.reviews
                        && g.member_bonds == before.member_bonds
                        && g.pending == before.pending
                        && g.approvals == before.approvals)
                    {
                        Self::persist(&g)?;
                        Self::drain_events(&mut g)?;
                    }
                }
                Self::release_due_bonds()?;
                Self::reap_failed_actions();
                Ok(())
            });
        }

        fn outstanding_fronting(who: &T::AccountId) -> futarchy_primitives::Balance {
            ReviewFrontingOf::<T>::iter_values().fold(0u128, |total, fronting| {
                let addition = fronting
                    .approvers
                    .iter()
                    .take(usize::from(fronting.approver_count))
                    .enumerate()
                    .filter(|(_, raw)| Self::from_core(**raw) == *who)
                    .fold(0u128, |sum, (position, _)| {
                        sum.saturating_add(fronting.slices[position])
                    });
                total.saturating_add(addition)
            })
        }

        fn release_due_bonds() -> DispatchResult {
            let epoch = T::CurrentEpoch::get();
            let reason = Self::seat_reason();
            PendingBondReleases::<T>::try_mutate(|releases| -> DispatchResult {
                let mut kept: BoundedVec<
                    BondRelease<T::AccountId>,
                    ConstU32<MAX_PENDING_BOND_RELEASES>,
                > = BoundedVec::default();
                for release in releases.iter() {
                    if epoch >= release.release_epoch
                        && Self::outstanding_fronting(&release.who) == 0
                    {
                        let actual = T::Currency::release(
                            &reason,
                            &release.who,
                            release.amount,
                            Precision::Exact,
                        )?;
                        ensure!(actual == release.amount, Error::<T>::BondAccounting);
                    } else {
                        kept.try_push(release.clone())
                            .map_err(|_| Error::<T>::TooManyBondReleases)?;
                    }
                }
                *releases = kept;
                Ok(())
            })
        }

        fn reap_failed_actions() {
            let epoch = T::CurrentEpoch::get();
            for (action, failed) in FailedActions::<T>::iter() {
                if epoch < failed.failed_epoch.saturating_add(4) {
                    continue;
                }
                match failed.recall_referendum {
                    Some(referendum) => {
                        if T::RecallScheduler::refund_recall(referendum).is_ok() {
                            FailedActions::<T>::remove(action);
                        }
                    }
                    None => FailedActions::<T>::remove(action),
                }
            }
        }

        /// Read helper: the current council (view for the FE / sibling pallets).
        pub fn members() -> Option<[Option<T::AccountId>; GUARDIAN_SEATS]> {
            Members::<T>::get().map(|raw| core::array::from_fn(|i| raw[i].map(Self::from_core)))
        }

        /// Read helper: is a playbook currently active?
        pub fn playbook_active(id: PlaybookId) -> bool {
            let now = Self::now();
            ActivePlaybooks::<T>::get()
                .iter()
                .any(|playbook| playbook.id == id && now < playbook.expiry)
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
            ensure!(
                PendingBondReleases::<T>::get().len() as u32 <= MAX_PENDING_BOND_RELEASES,
                TryRuntimeError::Other("guardian: PendingBondReleases over bound")
            );
            ensure!(
                FailedActions::<T>::count() <= MAX_FAILED_ACTIONS,
                TryRuntimeError::Other("guardian: FailedActions over bound")
            );
            ensure!(
                ReviewFrontingOf::<T>::count() <= MAX_REVIEWS,
                TryRuntimeError::Other("guardian: ReviewFrontingOf over bound")
            );
            ensure!(
                VetoReviewReferenda::<T>::count() <= MAX_REVIEWS,
                TryRuntimeError::Other("guardian: VetoReviewReferenda over bound")
            );
            ensure!(
                ALL_PLAYBOOKS
                    .iter()
                    .all(PlaybookRegistered::<T>::contains_key)
                    && PlaybookRegistered::<T>::iter_keys().count() == ALL_PLAYBOOKS.len(),
                TryRuntimeError::Other("guardian: playbook registry is incomplete")
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
                for review in g
                    .reviews
                    .iter()
                    .filter(|review| !review.ratified && !review.recall_scheduled)
                {
                    ensure!(
                        ReviewFrontingOf::<T>::contains_key(review.action_id)
                            && ReviewReferenda::<T>::contains_key(review.action_id),
                        TryRuntimeError::Other(
                            "guardian I-23: an open review has no funded referendum"
                        )
                    );
                }

                let reason = Self::seat_reason();
                for (index, member) in g.members.iter().enumerate() {
                    match member {
                        Some(raw) => {
                            let who = Self::from_core(*raw);
                            let held = T::Currency::balance_on_hold(&reason, &who);
                            let fronted = Self::outstanding_fronting(&who);
                            ensure!(
                                held.saturating_add(fronted) == g.member_bonds[index],
                                TryRuntimeError::Other(
                                    "guardian bond: seated hold + fronting != obligation"
                                )
                            );
                        }
                        None => ensure!(
                            g.member_bonds[index] == 0,
                            TryRuntimeError::Other("guardian bond: vacant seat has obligation")
                        ),
                    }
                }
                for release in PendingBondReleases::<T>::get() {
                    let held = T::Currency::balance_on_hold(&reason, &release.who);
                    let fronted = Self::outstanding_fronting(&release.who);
                    ensure!(
                        held.saturating_add(fronted) == release.amount,
                        TryRuntimeError::Other(
                            "guardian bond: departed hold + fronting != release obligation"
                        )
                    );
                }
                for (action, fronting) in ReviewFrontingOf::<T>::iter() {
                    let Some(referendum) = ReviewReferenda::<T>::get(action) else {
                        return Err(TryRuntimeError::Other(
                            "guardian bond: fronting has no referendum record",
                        ));
                    };
                    let veto_referendum = VetoReviewReferenda::<T>::get(action);
                    ensure!(
                        fronting.referendum == referendum,
                        TryRuntimeError::Other(
                            "guardian bond: fronting and referendum records disagree"
                        )
                    );
                    let is_delay_once = g
                        .pending
                        .iter()
                        .find(|pending| pending.id == action)
                        .is_some_and(|pending| {
                            matches!(pending.power, GuardianPower::DelayOnce { .. })
                        });
                    ensure!(
                        veto_referendum.is_some() == is_delay_once,
                        TryRuntimeError::Other(
                            "guardian review: verdict set does not match action power"
                        )
                    );
                    ensure!(
                        veto_referendum != Some(referendum),
                        TryRuntimeError::Other("guardian review: duplicate verdict referenda")
                    );
                    let fronted = fronting
                        .slices
                        .iter()
                        .take(usize::from(fronting.approver_count))
                        .copied()
                        .fold(0u128, u128::saturating_add);
                    let expected = T::ReviewScheduler::review_deposit()
                        .checked_mul(if veto_referendum.is_some() { 2 } else { 1 })
                        .ok_or(TryRuntimeError::Other(
                            "guardian bond: review deposit total overflow",
                        ))?;
                    ensure!(
                        fronting.approver_count > 0
                            && usize::from(fronting.approver_count) <= GUARDIAN_SEATS
                            && (fronted == 0 || fronted == expected),
                        TryRuntimeError::Other("guardian bond: malformed review fronting")
                    );
                }
                ensure!(
                    VetoReviewReferenda::<T>::iter_keys().all(ReviewFrontingOf::<T>::contains_key),
                    TryRuntimeError::Other(
                        "guardian review: veto referendum has no fronting record"
                    )
                );
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
                CoreError::BadPlaybookTarget => Error::<T>::BadPlaybookTarget.into(),
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
