//! `frame-benchmarking` v2 benchmarks for every extrinsic and the maintenance
//! hook (Track-A DoD, 15 §4.5). B5 turns the generated output into the
//! PoV-calibrated `weights.rs`; the harness + worst-case setup are the milestone.

use super::*;
use crate::pallet::{
    ActivePlaybooks, Approvals, FailedAction, FailedActions, Members, NextActionId, PendingActions,
    RerunUsed, ReviewDeadlines, ReviewReferenda, VetoReviewReferenda,
};

use frame_benchmarking::v2::*;
use frame_support::traits::{
    fungible::{Mutate, MutateHold},
    OnInitialize,
};
use futarchy_primitives::H256;

/// A distinct member account by index (matches the genesis council).
fn member<T: Config>(i: u8) -> T::AccountId {
    T::AccountId::from([i + 1; 32])
}

/// Install the seven-seat council and return it.
fn seed_council<T: Config>() -> [T::AccountId; GUARDIAN_SEATS] {
    let members: [T::AccountId; GUARDIAN_SEATS] = core::array::from_fn(|i| member::<T>(i as u8));
    let raw = Pallet::<T>::members_to_core(&members);
    let reason: T::RuntimeHoldReason = HoldReason::SeatBond.into();
    for who in &members {
        T::Currency::mint_into(who, GUARDIAN_BOND.saturating_mul(2)).expect("benchmark mint");
        T::Currency::hold(&reason, who, GUARDIAN_BOND).expect("benchmark seat hold");
    }
    Members::<T>::put(raw.map(Some));
    crate::pallet::MemberBonds::<T>::put([GUARDIAN_BOND; GUARDIAN_SEATS]);
    members
}

/// Drive a `ForceRerun` action to four approvals (proposer + three), leaving the
/// fifth for the measured call.
fn action_at_four<T: Config>(power: GuardianPower) -> ActionId {
    T::BenchmarkHelper::prime_for_worst_case();
    Pallet::<T>::propose_action(T::BenchmarkHelper::signed([1; 32]), power, H256::default())
        .expect("propose");
    let id = 0;
    for i in 1..4u8 {
        Pallet::<T>::approve_action(T::BenchmarkHelper::signed([i + 1; 32]), id).expect("approve");
    }
    id
}

fn pending(id: ActionId, dispatched: bool) -> PendingAction {
    PendingAction {
        id,
        proposer: [1; 32],
        power: GuardianPower::SuspendOnGate,
        justification_hash: H256::default(),
        created_at: 0,
        expires_at: ACTION_EXPIRY_BLOCKS,
        dispatched,
    }
}

fn fill_pending_with_five<T: Config>(start: ActionId) {
    // Five approvals dispatch an action; the core rejects every later approval.
    // Thus 64 dispatched-but-not-yet-reaped actions x five approvals is the
    // largest reachable live approval ledger (320 entries).
    let members = [[1; 32], [2; 32], [3; 32], [4; 32], [5; 32]];
    PendingActions::<T>::mutate(|actions| {
        for id in start..MAX_PENDING_ACTIONS {
            actions.try_push(pending(id, true)).expect("pending bound");
        }
    });
    Approvals::<T>::mutate(|approvals| {
        for id in start..MAX_PENDING_ACTIONS {
            for who in members {
                approvals.try_push((id, who)).expect("approval bound");
            }
        }
    });
    NextActionId::<T>::put(MAX_PENDING_ACTIONS);
}

fn review(action_id: ActionId, deadline_epoch: EpochId) -> ReviewRecord {
    ReviewRecord {
        action_id,
        deadline_epoch,
        ratified: false,
        recall_scheduled: false,
        approvers: [
            [1; 32], [2; 32], [3; 32], [4; 32], [5; 32], [0; 32], [0; 32],
        ],
        approver_count: GUARDIAN_THRESHOLD,
    }
}

fn fill_reviews<T: Config>(target_len: u32) {
    ReviewDeadlines::<T>::mutate(|reviews| {
        while reviews.len() < target_len as usize {
            let id = 1_000u32.saturating_add(reviews.len() as u32);
            reviews.try_push(review(id, 0)).expect("review bound");
        }
    });
}

fn fill_reruns<T: Config>(target_len: u32) {
    RerunUsed::<T>::mutate(|reruns| {
        while reruns.len() < target_len as usize {
            reruns
                .try_push(10_000u64.saturating_add(reruns.len() as u64))
                .expect("rerun bound");
        }
    });
}

fn fill_playbooks<T: Config>(expiry: u32) {
    let ids = [
        PlaybookId::Depeg,
        PlaybookId::Migration,
        PlaybookId::OracleVoid,
        PlaybookId::HaltIntake,
        PlaybookId::Reserve,
        PlaybookId::LedgerFreeze,
    ];
    ActivePlaybooks::<T>::mutate(|playbooks| {
        for id in ids {
            if !playbooks.iter().any(|playbook| playbook.id == id) {
                playbooks
                    .try_push(ActivePlaybook {
                        id,
                        expiry,
                        renewals_used: 0,
                    })
                    .expect("playbook bound");
            }
        }
    });
}

fn fill_read_collections<T: Config>() {
    fill_reviews::<T>(MAX_REVIEWS);
    fill_playbooks::<T>(ACTION_EXPIRY_BLOCKS);
    fill_reruns::<T>(MAX_RERUN_USED);
}

#[benchmarks(where T: Config)]
mod benches {
    use super::*;

    #[benchmark]
    fn set_members() {
        seed_council::<T>();
        fill_pending_with_five::<T>(0);
        fill_read_collections::<T>();
        let members: [T::AccountId; GUARDIAN_SEATS] =
            core::array::from_fn(|i| member::<T>((i as u8) + 10));
        for who in &members {
            T::Currency::mint_into(who, GUARDIAN_BOND.saturating_mul(2)).expect("benchmark mint");
        }

        #[extrinsic_call]
        _(T::BenchmarkHelper::values() as T::RuntimeOrigin, members);

        assert!(Members::<T>::get().is_some());
    }

    #[benchmark]
    fn propose_action() {
        seed_council::<T>();
        T::BenchmarkHelper::prime_for_worst_case();
        fill_pending_with_five::<T>(0);
        // Leave one pending slot for the measured proposal.
        PendingActions::<T>::mutate(|actions| {
            let _ = actions.pop();
        });
        Approvals::<T>::mutate(|approvals| {
            approvals.retain(|(id, _)| *id < MAX_PENDING_ACTIONS - 1);
        });
        NextActionId::<T>::put(MAX_PENDING_ACTIONS - 1);
        fill_read_collections::<T>();

        #[extrinsic_call]
        _(
            T::BenchmarkHelper::signed([1; 32]),
            GuardianPower::SuspendOnGate,
            H256::default(),
        );

        assert_eq!(
            PendingActions::<T>::get().len(),
            MAX_PENDING_ACTIONS as usize
        );
    }

    #[benchmark]
    fn approve_action() {
        seed_council::<T>();
        let id = action_at_four::<T>(GuardianPower::DelayOnce { pid: 1 });
        fill_pending_with_five::<T>(1);
        fill_reviews::<T>(MAX_REVIEWS - 1);
        fill_playbooks::<T>(ACTION_EXPIRY_BLOCKS);
        fill_reruns::<T>(MAX_RERUN_USED - 1);

        #[extrinsic_call]
        _(T::BenchmarkHelper::signed([5; 32]), id);

        // The fifth approval dispatched the action and scheduled its review.
        assert_eq!(ReviewDeadlines::<T>::get().len(), MAX_REVIEWS as usize);
    }

    #[benchmark]
    fn ratify_action() {
        seed_council::<T>();
        let id = action_at_four::<T>(GuardianPower::DelayOnce { pid: 1 });
        Pallet::<T>::approve_action(T::BenchmarkHelper::signed([5; 32]), id).expect("dispatch");
        let referendum = ReviewReferenda::<T>::get(id).expect("review referendum");
        T::BenchmarkHelper::close_review(referendum).expect("close review");
        fill_pending_with_five::<T>(1);
        fill_reviews::<T>(MAX_REVIEWS);
        fill_playbooks::<T>(ACTION_EXPIRY_BLOCKS);
        fill_reruns::<T>(MAX_RERUN_USED);

        #[extrinsic_call]
        _(T::BenchmarkHelper::values() as T::RuntimeOrigin, id);

        assert!(ReviewDeadlines::<T>::get().iter().any(|r| r.ratified));
    }

    #[benchmark]
    fn renew_playbook() {
        seed_council::<T>();
        T::BenchmarkHelper::prime_for_worst_case();
        let power = GuardianPower::ActivatePlaybook {
            id: PlaybookId::LedgerFreeze,
            trigger: PlaybookTrigger::LedgerDrift,
            expiry: 100,
            target: None,
        };
        Pallet::<T>::propose_action(T::BenchmarkHelper::signed([1; 32]), power, H256::default())
            .expect("propose");
        for i in 1..5u8 {
            Pallet::<T>::approve_action(T::BenchmarkHelper::signed([i + 1; 32]), 0)
                .expect("approve");
        }
        fill_pending_with_five::<T>(1);
        fill_reviews::<T>(MAX_REVIEWS);
        fill_playbooks::<T>(100);
        fill_reruns::<T>(MAX_RERUN_USED);

        #[extrinsic_call]
        _(
            T::BenchmarkHelper::values() as T::RuntimeOrigin,
            PlaybookId::LedgerFreeze,
        );

        assert!(ActivePlaybooks::<T>::get()
            .iter()
            .any(|p| p.id == PlaybookId::LedgerFreeze && p.renewals_used == 1));
    }

    #[benchmark]
    fn uphold_veto() -> Result<(), BenchmarkError> {
        seed_council::<T>();
        let id = action_at_four::<T>(GuardianPower::DelayOnce { pid: 1 });
        Pallet::<T>::approve_action(T::BenchmarkHelper::signed([5; 32]), id)
            .map_err(|_| BenchmarkError::Stop("dispatch"))?;
        let referendum = VetoReviewReferenda::<T>::get(id)
            .ok_or(BenchmarkError::Stop("veto review referendum"))?;
        T::BenchmarkHelper::close_review(referendum)
            .map_err(|_| BenchmarkError::Stop("close review"))?;

        #[extrinsic_call]
        _(T::BenchmarkHelper::values() as T::RuntimeOrigin, id);

        assert!(ReviewDeadlines::<T>::get().iter().any(|r| r.ratified));
        Ok(())
    }

    #[benchmark]
    fn recall() {
        seed_council::<T>();
        let approvers = [
            [1; 32], [2; 32], [3; 32], [4; 32], [5; 32], [0; 32], [0; 32],
        ];
        FailedActions::<T>::insert(
            0,
            FailedAction {
                approvers,
                approver_count: GUARDIAN_THRESHOLD,
                failed_epoch: 0,
                recall_referendum: None,
            },
        );

        #[extrinsic_call]
        _(T::BenchmarkHelper::values() as T::RuntimeOrigin, 0);

        assert_eq!(
            Members::<T>::get()
                .expect("seeded council")
                .iter()
                .filter(|member| member.is_some())
                .count(),
            GUARDIAN_SEATS - usize::from(GUARDIAN_THRESHOLD)
        );
    }

    #[benchmark]
    fn set_playbook_registered() {
        #[extrinsic_call]
        _(
            T::BenchmarkHelper::admin() as T::RuntimeOrigin,
            PlaybookId::Depeg,
            false,
        );

        assert!(!crate::pallet::PlaybookRegistered::<T>::get(
            PlaybookId::Depeg
        ));
    }

    #[benchmark]
    fn on_initialize() -> Result<(), BenchmarkError> {
        seed_council::<T>();
        T::BenchmarkHelper::prime_for_worst_case();
        // Use a non-delay action here so the benchmark exercises the full
        // terminal reap path. A delayed action intentionally retains its veto
        // referendum until T12 (SQ-311) and is covered by pallet tests.
        let overdue = action_at_four::<T>(GuardianPower::SuspendOnGate);
        Pallet::<T>::approve_action(T::BenchmarkHelper::signed([5; 32]), overdue)
            .map_err(|_| BenchmarkError::Stop("dispatch"))?;
        let deadline = ReviewDeadlines::<T>::get()
            .iter()
            .find(|review| review.action_id == overdue)
            .map(|review| review.deadline_epoch)
            .ok_or(BenchmarkError::Stop("overdue review deadline"))?;
        T::BenchmarkHelper::prime_maintenance_epoch(deadline.saturating_add(1));
        let approvers = [[1; 32], [2; 32], [3; 32], [4; 32], [5; 32]];
        PendingActions::<T>::mutate(|actions| {
            for id in 1..MAX_PENDING_ACTIONS {
                actions.try_push(pending(id, true)).expect("pending bound");
            }
        });
        Approvals::<T>::mutate(|approvals| {
            for id in 1..MAX_PENDING_ACTIONS {
                for who in approvers {
                    approvals.try_push((id, who)).expect("approval bound");
                }
            }
        });
        ReviewDeadlines::<T>::mutate(|reviews| {
            while reviews.len() < MAX_REVIEWS as usize {
                let id = 1_000u32.saturating_add(reviews.len() as u32);
                let mut terminal = review(id, 0);
                // Terminal reviews exercise the full bounded reap without
                // inventing unreachable fronting records for 128 simultaneous
                // failures. The real `overdue` record exercises cancellation,
                // refund, slash and recall at the strict deadline boundary.
                terminal.ratified = true;
                reviews.try_push(terminal).expect("review bound");
            }
        });
        fill_playbooks::<T>(0);
        ActivePlaybooks::<T>::mutate(|playbooks| {
            let ledger = playbooks
                .iter_mut()
                .find(|playbook| playbook.id == PlaybookId::LedgerFreeze)
                .expect("ledger freeze fixture exists");
            ledger.expiry = u32::MAX;
        });
        fill_reruns::<T>(MAX_RERUN_USED);
        NextActionId::<T>::put(MAX_PENDING_ACTIONS);

        #[block]
        {
            Pallet::<T>::on_initialize(frame_system::Pallet::<T>::block_number());
        }

        assert_eq!(ActivePlaybooks::<T>::get().len(), 1);
        assert_eq!(ActivePlaybooks::<T>::get()[0].id, PlaybookId::LedgerFreeze);
        assert!(PendingActions::<T>::get().is_empty());
        assert!(ReviewDeadlines::<T>::get().is_empty());
        let failed = FailedActions::<T>::get(overdue).expect("overdue review settled");
        assert!(failed.recall_referendum.is_some());
        assert!(
            crate::pallet::MemberBonds::<T>::get()[..usize::from(GUARDIAN_THRESHOLD)]
                .iter()
                .all(|bond| *bond == GUARDIAN_BOND / 2)
        );
        assert!(!ReviewReferenda::<T>::contains_key(overdue));
        assert!(!VetoReviewReferenda::<T>::contains_key(overdue));
        Ok(())
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext_empty(), crate::mock::Test);
}
