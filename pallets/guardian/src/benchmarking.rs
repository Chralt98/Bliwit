//! `frame-benchmarking` v2 benchmarks for every extrinsic and the maintenance
//! hook (Track-A DoD, 15 §4.5). B5 turns the generated output into the
//! PoV-calibrated `weights.rs`; the harness + worst-case setup are the milestone.

use super::*;
use crate::pallet::{ActivePlaybooks, Members, PendingActions, ReviewDeadlines};

use frame_benchmarking::v2::*;
use frame_support::traits::{Get, OnInitialize};
use futarchy_primitives::H256;

/// A distinct member account by index (matches the genesis council).
fn member<T: Config>(i: u8) -> T::AccountId {
    T::AccountId::from([i + 1; 32])
}

/// Install the seven-seat council and return it.
fn seed_council<T: Config>() -> [T::AccountId; GUARDIAN_SEATS] {
    let members: [T::AccountId; GUARDIAN_SEATS] = core::array::from_fn(|i| member::<T>(i as u8));
    let raw = Pallet::<T>::members_to_core(&members);
    Members::<T>::put(raw);
    crate::pallet::MemberBonds::<T>::put([GUARDIAN_BOND; GUARDIAN_SEATS]);
    members
}

/// Drive a `ForceRerun` action to four approvals (proposer + three), leaving the
/// fifth for the measured call.
fn action_at_four<T: Config>() -> ActionId {
    T::BenchmarkHelper::prime_for_worst_case();
    let power = GuardianPower::ForceRerun { pid: 1 };
    Pallet::<T>::propose_action(T::BenchmarkHelper::signed([1; 32]), power, H256::default())
        .expect("propose");
    let id = 0;
    for i in 1..4u8 {
        Pallet::<T>::approve_action(T::BenchmarkHelper::signed([i + 1; 32]), id).expect("approve");
    }
    id
}

#[benchmarks(where T: Config)]
mod benches {
    use super::*;

    #[benchmark]
    fn set_members() {
        let members: [T::AccountId; GUARDIAN_SEATS] =
            core::array::from_fn(|i| member::<T>((i as u8) + 10));

        #[extrinsic_call]
        _(T::BenchmarkHelper::values() as T::RuntimeOrigin, members);

        assert!(Members::<T>::get().is_some());
    }

    #[benchmark]
    fn propose_action() {
        seed_council::<T>();
        T::BenchmarkHelper::prime_for_worst_case();

        #[extrinsic_call]
        _(
            T::BenchmarkHelper::signed([1; 32]),
            GuardianPower::SuspendOnGate,
            H256::default(),
        );

        assert_eq!(PendingActions::<T>::get().len(), 1);
    }

    #[benchmark]
    fn approve_action() {
        seed_council::<T>();
        let id = action_at_four::<T>();

        #[extrinsic_call]
        _(T::BenchmarkHelper::signed([5; 32]), id);

        // The fifth approval dispatched the action and scheduled its review.
        assert_eq!(ReviewDeadlines::<T>::get().len(), 1);
    }

    #[benchmark]
    fn ratify_action() {
        seed_council::<T>();
        let id = action_at_four::<T>();
        Pallet::<T>::approve_action(T::BenchmarkHelper::signed([5; 32]), id).expect("dispatch");

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
        };
        Pallet::<T>::propose_action(T::BenchmarkHelper::signed([1; 32]), power, H256::default())
            .expect("propose");
        for i in 1..5u8 {
            Pallet::<T>::approve_action(T::BenchmarkHelper::signed([i + 1; 32]), 0)
                .expect("approve");
        }

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
    fn on_initialize() {
        seed_council::<T>();
        // An active playbook past expiry plus a dispatched action's review.
        let id = action_at_four::<T>();
        Pallet::<T>::approve_action(T::BenchmarkHelper::signed([5; 32]), id).expect("dispatch");
        ActivePlaybooks::<T>::mutate(|p| {
            let _ = p.try_push(ActivePlaybook {
                id: PlaybookId::Depeg,
                expiry: 1,
                renewals_used: 0,
            });
        });

        #[block]
        {
            Pallet::<T>::on_initialize(frame_system::Pallet::<T>::block_number());
        }

        assert!(!ActivePlaybooks::<T>::get()
            .iter()
            .any(|p| p.id == PlaybookId::Depeg));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext_empty(), crate::mock::Test);
}
