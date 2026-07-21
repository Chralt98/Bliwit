use crate::{mock::*, CumulativeDeposits};
use frame_support::assert_ok;

#[test]
fn mint_admits_exact_cap_and_refuses_cap_plus_one() {
    new_test_ext().execute_with(|| {
        UsdcIssuance::set(900);
        assert_ok!(InflowCaps::mint_admissible(100));
        assert_eq!(InflowCaps::mint_admissible(101), Err(()));
        assert_eq!(UsdcIssuance::get(), 900);
        assert_eq!(CumulativeDeposits::<Test>::iter().count(), 0);
    });
}

#[test]
fn escrow_gate_is_a_pure_read_and_refuses_only_while_already_over_cap() {
    new_test_ext().execute_with(|| {
        TvlCap::set(900);
        UsdcIssuance::set(900);
        DepositCap::set(10);
        CumulativeDeposits::<Test>::insert(1, 10);
        assert!(InflowCaps::escrow_admissible(&1));

        UsdcIssuance::set(901);
        assert!(!InflowCaps::escrow_admissible(&1));
        UsdcIssuance::set(900);

        CumulativeDeposits::<Test>::insert(1, 11);
        assert!(!InflowCaps::escrow_admissible(&1));
        assert_eq!(UsdcIssuance::get(), 900);
        assert_eq!(CumulativeDeposits::<Test>::get(1), 11);
    });
}

#[test]
fn zero_mint_is_always_admissible_and_sentinel_is_unbounded() {
    new_test_ext().execute_with(|| {
        TvlCap::set(0);
        UsdcIssuance::set(u128::MAX);
        assert_ok!(InflowCaps::mint_admissible(0));

        TvlCap::set(u128::MAX);
        assert_ok!(InflowCaps::mint_admissible(u128::MAX));
    });
}

#[test]
fn inflows_accumulate_to_the_exact_cap() {
    new_test_ext().execute_with(|| {
        DepositCap::set(10);
        assert_ok!(InflowCaps::note_inflow(&1, 4));
        assert_ok!(InflowCaps::note_inflow(&1, 6));
        assert_eq!(CumulativeDeposits::<Test>::get(1), 10);
        assert_ok!(InflowCaps::do_try_state());
    });
}

#[test]
fn refused_inflow_writes_nothing() {
    new_test_ext().execute_with(|| {
        DepositCap::set(10);
        assert_ok!(InflowCaps::note_inflow(&1, 4));
        assert_eq!(InflowCaps::note_inflow(&1, 7), Err(()));
        assert_eq!(CumulativeDeposits::<Test>::get(1), 4);

        DepositCap::set(10);
        CumulativeDeposits::<Test>::insert(1, u128::MAX);
        assert_eq!(InflowCaps::note_inflow(&1, 1), Err(()));
        assert_eq!(CumulativeDeposits::<Test>::get(1), u128::MAX);
    });
}

#[test]
fn meters_are_isolated_per_account() {
    new_test_ext().execute_with(|| {
        DepositCap::set(10);
        assert_ok!(InflowCaps::note_inflow(&1, 10));
        assert_ok!(InflowCaps::note_inflow(&2, 7));
        assert_eq!(CumulativeDeposits::<Test>::get(1), 10);
        assert_eq!(CumulativeDeposits::<Test>::get(2), 7);
    });
}

#[test]
fn sentinel_retires_meter_without_reading_or_writing_it() {
    new_test_ext().execute_with(|| {
        CumulativeDeposits::<Test>::insert(1, 7);
        DepositCap::set(u128::MAX);
        assert_ok!(InflowCaps::note_inflow(&1, u128::MAX));
        assert_ok!(InflowCaps::note_inflow(&2, 10));
        assert_eq!(CumulativeDeposits::<Test>::get(1), 7);
        assert!(!CumulativeDeposits::<Test>::contains_key(2));
        assert_ok!(InflowCaps::do_try_state());
    });
}

#[test]
fn try_state_rejects_over_cap_and_zero_entries() {
    new_test_ext().execute_with(|| {
        DepositCap::set(10);
        CumulativeDeposits::<Test>::insert(1, 11);
        assert!(InflowCaps::do_try_state().is_err());

        CumulativeDeposits::<Test>::remove(1);
        CumulativeDeposits::<Test>::insert(2, 0);
        assert!(InflowCaps::do_try_state().is_err());
    });
}

#[test]
fn try_state_rejects_total_issuance_over_a_finite_tvl_cap() {
    new_test_ext().execute_with(|| {
        TvlCap::set(999);
        UsdcIssuance::set(1_000);
        assert!(InflowCaps::do_try_state().is_err());

        TvlCap::set(u128::MAX);
        assert_ok!(InflowCaps::do_try_state());
    });
}

#[test]
fn inflow_admissible_is_a_pure_read_agreeing_with_note_inflow() {
    // 09 §5.2 (SQ-129): the barrier's pre-mint gate must answer exactly the
    // question the deposit leg answers, without reserving anything.
    new_test_ext().execute_with(|| {
        DepositCap::set(100);
        assert!(InflowCaps::inflow_admissible(&1, 100));
        assert!(!InflowCaps::inflow_admissible(&1, 101));
        // Pure: the read created no meter entry.
        assert_eq!(CumulativeDeposits::<Test>::iter().count(), 0);

        assert_ok!(InflowCaps::note_inflow(&1, 60));
        assert_eq!(CumulativeDeposits::<Test>::get(1), 60);
        assert!(InflowCaps::inflow_admissible(&1, 40));
        assert!(!InflowCaps::inflow_admissible(&1, 41));
        assert_eq!(InflowCaps::note_inflow(&1, 41), Err(()));
        assert_eq!(CumulativeDeposits::<Test>::get(1), 60);
    });
}

#[test]
fn inflow_admissible_honours_the_unbounded_sentinel_and_saturating_edges() {
    new_test_ext().execute_with(|| {
        // 13 §1 unbounded sentinel retires the per-account meter entirely.
        DepositCap::set(u128::MAX);
        assert!(InflowCaps::inflow_admissible(&1, u128::MAX));

        // A bounded cap rejects an addition that would overflow rather than wrap.
        DepositCap::set(100);
        CumulativeDeposits::<Test>::insert(1, 1);
        assert!(!InflowCaps::inflow_admissible(&1, u128::MAX));
        // Zero never moves the meter, so it is always admissible.
        assert!(InflowCaps::inflow_admissible(&1, 0));
    });
}
