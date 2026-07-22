//! Runtime-level coverage for the treasury health cluster (SQ-205, SQ-207,
//! SQ-180): the oracle → constitution + treasury reserve-health seam, the
//! INSURANCE sweep, and the 08 §4.2 minimum-viable-NAV arming gate.
//!
//! Kept out of `tests.rs` deliberately — that file is large and concurrently
//! edited; `tests_s5.rs` / `tests_telemetry.rs` set the precedent.

use crate::configs::insurance_account;
use crate::tests::development_ext;
use crate::*;
use frame_support::traits::fungibles::{Inspect, Mutate};
use frame_support::{
    assert_noop, assert_ok,
    traits::{Contains, Get},
    weights::Weight,
};
use futarchy_primitives::{chain_identity, currency, ProposalClass};
use pallet_futarchy_treasury::BudgetLine;
use pallet_oracle::{OracleParamsProvider as _, ProbeDispatch as _};
use parity_scale_codec::{Decode, Encode};
use staging_xcm::latest::prelude::*;
use staging_xcm_executor::traits::WeightBounds;

// ---------------------------------------------------------------- SQ-205 ---
//
// 07 §8 owns the reserve-health flag `R`; 08 §1.2 makes `spendable_nav` zero
// exactly while it is set. The seam between them must move both the
// constitution's 02 §7.3 bit-7 mirror and the treasury haircut, or neither.

/// The runtime sink composes the two sibling writes into one act.
#[test]
fn reserve_health_sink_moves_the_constitution_mirror_and_the_treasury_haircut() {
    development_ext().execute_with(|| {
        use pallet_oracle::ReserveHealthSink;

        assert!(!FutarchyTreasury::treasury().reserve_impaired);
        assert_eq!(
            Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG,
            0
        );

        assert_ok!(configs::ReserveHealthToConstitutionAndTreasury::reserve_health_changed(true));

        // Both consequences landed together.
        assert!(FutarchyTreasury::treasury().reserve_impaired);
        assert_eq!(
            Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG,
            pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG
        );
        // 08 §1.2(2): fail-static — spendable NAV is zero for new commitments.
        assert_eq!(FutarchyTreasury::nav().spendable_nav, 0);

        // And the clearing edge reverses both.
        assert_ok!(configs::ReserveHealthToConstitutionAndTreasury::reserve_health_changed(false));
        assert!(!FutarchyTreasury::treasury().reserve_impaired);
        assert_eq!(
            Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG,
            0
        );
    });
}

/// 02 §4 requires `nav().haircut_flag` to follow `welfare_current().reserve_flag`
/// (the authoritative oracle value). With the sink applied they agree; this pins
/// the agreement the SQ-205 row reported as violated.
#[test]
fn nav_haircut_flag_follows_the_authoritative_oracle_reserve_flag() {
    development_ext().execute_with(|| {
        use pallet_oracle::ReserveHealthSink;

        assert_eq!(
            FutarchyTreasury::nav().reserve_impaired,
            pallet_oracle::Pallet::<Runtime>::reserve_unhealthy()
        );

        // Drive the oracle's own storage to unhealthy, then apply the seam as
        // the oracle would inside its transition.
        pallet_oracle::ReserveHealth::<Runtime>::mutate(|health| health.unhealthy = true);
        assert_ok!(configs::ReserveHealthToConstitutionAndTreasury::reserve_health_changed(true));

        assert!(pallet_oracle::Pallet::<Runtime>::reserve_unhealthy());
        assert!(FutarchyTreasury::nav().reserve_impaired);
        assert_eq!(
            FutarchyTreasury::nav().reserve_impaired,
            pallet_oracle::Pallet::<Runtime>::reserve_unhealthy()
        );
    });
}

#[test]
fn production_reserve_probe_feed_is_live_and_the_flagged_response_is_fully_weighed() {
    development_ext().execute_with(|| {
        let params = <Runtime as pallet_oracle::Config>::Params::get();
        assert!(!<Runtime as pallet_oracle::Config>::ProbeDispatch::live(
            &params
        ));
        fund_reserve_probe_line();
        assert!(<Runtime as pallet_oracle::Config>::ProbeDispatch::live(
            &params
        ));
        let callback = configs::RuntimeProbeCallbackWeight::get();
        assert!(callback.ref_time() <= configs::ReservedXcmpWeight::get().ref_time());
        assert!(callback.proof_size() <= configs::ReservedXcmpWeight::get().proof_size());

        let mut message = Xcm(vec![QueryResponse {
            query_id: 1 | bleavit_xcm::probe::PROBE_QUERY_ID_FLAG,
            response: Response::ExecutionResult(None),
            max_weight: callback,
            querier: Some(Location::here()),
        }]);
        let mut base_message = message.clone();
        let base = configs::xcm_config::BaseWeigher::weight(&mut base_message, Weight::MAX)
            .expect("base response instruction must be weighable");
        let weighed = configs::xcm_config::Weigher::weight(&mut message, Weight::MAX)
            .expect("flagged response must be weighable");
        assert_eq!(weighed, base.saturating_add(callback));
    });
}

pub(crate) fn fund_reserve_probe_line() {
    // The development fixture keeps treasury accounting empty; prime the core
    // MAIN mirror only for this non-payout maintenance-line test, then exercise
    // the real governed line-funding call.
    pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
        state.main_usdc = state.main_usdc.saturating_add(100 * currency::USDC);
    });
    assert_ok!(FutarchyTreasury::fund_budget_line(
        pallet_origins::Origin::FutarchyTreasury.into(),
        BudgetLine::OpsReserveProbe,
        100 * currency::USDC,
    ));
}

fn reserve_probe_interval() -> u32 {
    <configs::RuntimeOracleParams as pallet_oracle::OracleParamsProvider>::get().probe_interval
}

fn reserve_probe_line_debit(fee: Balance, rate: Balance) -> Balance {
    fee.checked_mul(rate)
        .expect("fixture conversion fits")
        .saturating_add(chain_identity::DOT_PLANCKS_PER_DOT - 1)
        / chain_identity::DOT_PLANCKS_PER_DOT
}

fn reserve_probe_runway() -> Balance {
    let params = configs::RuntimeOracleParams::get();
    pallet_futarchy_treasury::reserve_probe_runway_debit(
        configs::balance_param(b"ops.probe_fee"),
        configs::balance_param(b"ops.probe_rate"),
        params.fail_threshold,
        params.recover_threshold,
    )
    .expect("live probe runway is valid")
}

fn take_queued_probe_program() -> Xcm<()> {
    use cumulus_primitives_core::{XcmpMessageFormat, XcmpMessageSource};

    let mut queued = XcmpQueue::take_outbound_messages(1, &[]);
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].0, chain_identity::ASSET_HUB_PARA_ID.into());
    let queued_message = queued.remove(0);
    let mut encoded = queued_message.1.as_slice();
    assert_eq!(
        XcmpMessageFormat::decode(&mut encoded).expect("XCMP format"),
        XcmpMessageFormat::ConcatenatedVersionedXcm,
    );
    match staging_xcm::VersionedXcm::<()>::decode(&mut encoded).expect("versioned probe XCM") {
        staging_xcm::VersionedXcm::V5(program) => program,
        other => panic!("reserve probe used unexpected XCM version: {other:?}"),
    }
}

fn expected_probe_program(query_id: u64, fee: Balance) -> Xcm<()> {
    bleavit_xcm::probe::reserve_probe_program(
        query_id,
        <configs::RuntimeOracleParams as pallet_oracle::OracleParamsProvider>::get().probe_amount,
        fee,
        configs::ProbeExecWeightBudget::get(),
        configs::ProbeMaxResponseWeight::get(),
        configs::RuntimeParaId::get(),
    )
}

fn assert_program_prefix_and_topic(actual: &Xcm<()>, expected: &Xcm<()>) {
    assert_eq!(&actual.0[..expected.0.len()], expected.0.as_slice());
    assert!(matches!(actual.0.last(), Some(SetTopic(_))));
    assert_eq!(actual.0.len(), expected.0.len() + 1);
}

fn try_execute_probe_response(
    origin: Location,
    wire_query_id: u64,
    querier: Option<Location>,
    response: Response,
) -> Result<(), InstructionError> {
    let callback = configs::RuntimeProbeCallbackWeight::get();
    let mut message = Xcm(vec![QueryResponse {
        query_id: wire_query_id,
        response,
        max_weight: callback,
        querier,
    }]);
    let execution_weight = configs::xcm_config::Weigher::weight(&mut message, Weight::MAX)
        .expect("the production weigher must admit the authenticated response");
    let mut message_id = [wire_query_id as u8; 32];
    configs::xcm_config::Executor::prepare_and_execute(
        origin,
        message,
        &mut message_id,
        execution_weight,
        Weight::zero(),
    )
    .ensure_complete()
}

fn execute_probe_response(query_id: u64, response: Response) {
    let result = try_execute_probe_response(
        bleavit_xcm::identity::asset_hub_location(),
        query_id | bleavit_xcm::probe::PROBE_QUERY_ID_FLAG,
        Some(Location::here()),
        response,
    );
    assert!(
        result.is_ok(),
        "probe response execution did not complete: {result:?}"
    );
}

fn health_snapshot() -> (Vec<u8>, u32, Vec<u8>, usize) {
    let health_events = System::events()
        .iter()
        .filter(|record| {
            matches!(
                &record.event,
                RuntimeEvent::Oracle(
                    pallet_oracle::Event::ReserveProbeResult { .. }
                        | pallet_oracle::Event::ReserveUnhealthy
                        | pallet_oracle::Event::ReserveRecovered
                ) | RuntimeEvent::Constitution(pallet_constitution::Event::PhaseFlagSet { .. })
                    | RuntimeEvent::FutarchyTreasury(
                        pallet_futarchy_treasury::Event::NavHaircutFlagged { .. }
                    )
            )
        })
        .count();
    (
        pallet_oracle::ReserveHealth::<Runtime>::get().encode(),
        Constitution::phase_flags(),
        pallet_futarchy_treasury::State::<Runtime>::get().encode(),
        health_events,
    )
}

#[test]
fn production_reserve_probe_queues_to_asset_hub_and_commits_exact_governed_debit() {
    development_ext().execute_with(|| {
        fund_reserve_probe_line();
        ParachainSystem::open_outbound_hrmp_channel_for_benchmarks_or_tests(
            cumulus_primitives_core::ParaId::from(chain_identity::ASSET_HUB_PARA_ID),
        );
        let before = FutarchyTreasury::line_balance(BudgetLine::OpsReserveProbe);
        let fee = configs::balance_param(b"ops.probe_fee");
        let rate = configs::balance_param(b"ops.probe_rate");
        let expected = reserve_probe_line_debit(fee, rate);
        let interval = reserve_probe_interval();
        System::set_block_number(interval);

        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
            AccountId::from([9; 32])
        )));

        assert_eq!(
            before - FutarchyTreasury::line_balance(BudgetLine::OpsReserveProbe),
            expected
        );
        let program = take_queued_probe_program();
        assert_program_prefix_and_topic(&program, &expected_probe_program(1, fee));
        assert_eq!(
            pallet_oracle::ReserveHealth::<Runtime>::get().pending_since,
            Some(interval)
        );
    });
}

#[test]
fn production_probe_arm_gate_requires_the_complete_fail_and_recovery_runway() {
    development_ext().execute_with(|| {
        let interval = reserve_probe_interval();
        System::set_block_number(interval);
        let before = health_snapshot();
        assert_noop!(
            Oracle::crank_reserve_probe(RuntimeOrigin::signed(AccountId::from([9; 32]))),
            pallet_oracle::Error::<Runtime>::ProbeUnavailable
        );
        assert_eq!(health_snapshot(), before);
        assert!(!Oracle::reserve_probe_armed());

        // A present but underfunded line is not an arm signal.
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = state.main_usdc.saturating_add(1);
        });
        assert_ok!(FutarchyTreasury::fund_budget_line(
            pallet_origins::Origin::FutarchyTreasury.into(),
            BudgetLine::OpsReserveProbe,
            1,
        ));
        assert_noop!(
            Oracle::crank_reserve_probe(RuntimeOrigin::signed(AccountId::from([9; 32]))),
            pallet_oracle::Error::<Runtime>::ProbeUnavailable
        );
        assert!(!Oracle::reserve_probe_armed());
    });

    development_ext().execute_with(|| {
        ParachainSystem::open_outbound_hrmp_channel_for_benchmarks_or_tests(
            cumulus_primitives_core::ParaId::from(chain_identity::ASSET_HUB_PARA_ID),
        );
        let exact = reserve_probe_runway();
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = state.main_usdc.saturating_add(exact);
        });
        assert_ok!(FutarchyTreasury::fund_budget_line(
            pallet_origins::Origin::FutarchyTreasury.into(),
            BudgetLine::OpsReserveProbe,
            exact,
        ));
        let interval = reserve_probe_interval();
        System::set_block_number(interval);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
            AccountId::from([9; 32])
        )));
        assert!(Oracle::reserve_probe_armed());
        assert_eq!(
            pallet_oracle::ReserveHealth::<Runtime>::get().pending_since,
            Some(interval)
        );
        assert_eq!(
            FutarchyTreasury::line_balance(BudgetLine::OpsReserveProbe),
            exact
                - reserve_probe_line_debit(
                    configs::balance_param(b"ops.probe_fee"),
                    configs::balance_param(b"ops.probe_rate"),
                )
        );
    });
}

#[test]
fn production_ops_authority_is_runway_capped_until_treasury_reserve_handover() {
    development_ext().execute_with(|| {
        assert!(!pallet_futarchy_treasury::BootstrapOpsFundingClosed::<
            Runtime,
        >::get());
        let authority = pallet_futarchy_treasury::CoretimeQuoteAuthority::<Runtime>::get()
            .expect("development ops authority");
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = state.main_usdc.saturating_add(60_000_000 * currency::USDC);
        });

        let runway = reserve_probe_runway();
        assert_ok!(FutarchyTreasury::fund_budget_line(
            RuntimeOrigin::signed(authority.clone()),
            BudgetLine::OpsReserveProbe,
            runway - 1,
        ));
        assert_noop!(
            FutarchyTreasury::fund_budget_line(
                RuntimeOrigin::signed(authority.clone()),
                BudgetLine::Keeper,
                currency::USDC,
            ),
            pallet_futarchy_treasury::Error::<Runtime>::BootstrapOpsLineOnly
        );

        assert_ok!(Constitution::set_phase_flag(
            RuntimeOrigin::root(),
            pallet_constitution::PhaseFlagsValue::TREASURY_ARMED,
            true,
        ));
        assert!(!pallet_futarchy_treasury::BootstrapOpsFundingClosed::<
            Runtime,
        >::get());
        assert_ok!(FutarchyTreasury::fund_budget_line(
            RuntimeOrigin::signed(authority.clone()),
            BudgetLine::OpsReserveProbe,
            1,
        ));
        let before = pallet_futarchy_treasury::State::<Runtime>::get().encode();
        assert_noop!(
            FutarchyTreasury::fund_budget_line(
                RuntimeOrigin::signed(authority.clone()),
                BudgetLine::OpsReserveProbe,
                1,
            ),
            pallet_futarchy_treasury::Error::<Runtime>::BootstrapOpsFundingLimit
        );
        assert_eq!(
            pallet_futarchy_treasury::State::<Runtime>::get().encode(),
            before
        );

        // Only a successful positive binding-governance refill of this exact
        // line closes the handover. Zero and other-line calls do not.
        assert_ok!(FutarchyTreasury::fund_budget_line(
            pallet_origins::Origin::FutarchyTreasury.into(),
            BudgetLine::OpsReserveProbe,
            0,
        ));
        assert_ok!(FutarchyTreasury::fund_budget_line(
            pallet_origins::Origin::FutarchyTreasury.into(),
            BudgetLine::OpsMonitoring,
            1,
        ));
        assert!(!pallet_futarchy_treasury::BootstrapOpsFundingClosed::<
            Runtime,
        >::get());
        assert_ok!(FutarchyTreasury::fund_budget_line(
            pallet_origins::Origin::FutarchyTreasury.into(),
            BudgetLine::OpsReserveProbe,
            1,
        ));
        assert!(pallet_futarchy_treasury::BootstrapOpsFundingClosed::<Runtime>::get());

        assert_ok!(Constitution::set_phase_flag(
            RuntimeOrigin::root(),
            pallet_constitution::PhaseFlagsValue::TREASURY_ARMED,
            false,
        ));
        assert!(pallet_futarchy_treasury::BootstrapOpsFundingClosed::<Runtime>::get());
        assert_noop!(
            FutarchyTreasury::fund_budget_line(
                RuntimeOrigin::signed(authority),
                BudgetLine::OpsReserveProbe,
                1,
            ),
            pallet_futarchy_treasury::Error::<Runtime>::BootstrapOpsFundingClosed
        );
    });
}

#[test]
fn production_filter_admits_only_the_bootstrap_ops_funding_shape_as_public() {
    let ops = RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
        line: BudgetLine::OpsReserveProbe,
        amount: currency::USDC,
    });
    let non_ops = RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
        line: BudgetLine::Keeper,
        amount: currency::USDC,
    });
    let other_ops =
        RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
            line: BudgetLine::OpsCoretime,
            amount: currency::USDC,
        });

    assert!(crate::classifier::RuntimeBaseCallFilter::contains(&ops));
    assert!(!crate::classifier::RuntimeBaseCallFilter::contains(
        &non_ops
    ));
    assert!(!crate::classifier::RuntimeBaseCallFilter::contains(
        &other_ops
    ));
}

#[test]
fn live_probe_fee_and_rate_amendments_change_the_next_production_debit() {
    development_ext().execute_with(|| {
        fund_reserve_probe_line();
        ParachainSystem::open_outbound_hrmp_channel_for_benchmarks_or_tests(
            cumulus_primitives_core::ParaId::from(chain_identity::ASSET_HUB_PARA_ID),
        );

        let default_fee = configs::balance_param(b"ops.probe_fee");
        let default_rate = configs::balance_param(b"ops.probe_rate");
        let amended_fee = 10_000_000_000;
        let amended_rate = 10_000_000;
        assert_ne!(default_fee, amended_fee);
        assert_ne!(default_rate, amended_rate);

        // Exercise the real TREASURY-class amendment path after its cooldown.
        pallet_epoch::EpochOf::<Runtime>::mutate(|clock| clock.index = 1);
        assert_ok!(Constitution::set_param(
            pallet_origins::Origin::FutarchyTreasury.into(),
            pallet_constitution::key16(b"ops.probe_fee"),
            pallet_constitution::ParamValue::Balance(amended_fee),
        ));
        let first_before = FutarchyTreasury::line_balance(BudgetLine::OpsReserveProbe);

        System::set_block_number(reserve_probe_interval());
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
            AccountId::from([9; 32])
        )));

        assert_eq!(
            first_before - FutarchyTreasury::line_balance(BudgetLine::OpsReserveProbe),
            reserve_probe_line_debit(amended_fee, default_rate)
        );
        let first = take_queued_probe_program();
        assert_program_prefix_and_topic(&first, &expected_probe_program(1, amended_fee));
        execute_probe_response(1, Response::ExecutionResult(None));

        // Repricing the accounting rate changes only the USDC line debit; the
        // bounded DOT holding remains the independently governed fee amount.
        pallet_epoch::EpochOf::<Runtime>::mutate(|clock| clock.index = 2);
        assert_ok!(Constitution::set_param(
            pallet_origins::Origin::FutarchyTreasury.into(),
            pallet_constitution::key16(b"ops.probe_rate"),
            pallet_constitution::ParamValue::Balance(amended_rate),
        ));
        let second_before = FutarchyTreasury::line_balance(BudgetLine::OpsReserveProbe);
        System::set_block_number(reserve_probe_interval() * 2);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
            AccountId::from([9; 32])
        )));
        assert_eq!(
            second_before - FutarchyTreasury::line_balance(BudgetLine::OpsReserveProbe),
            reserve_probe_line_debit(amended_fee, amended_rate)
        );
        let second = take_queued_probe_program();
        assert_program_prefix_and_topic(&second, &expected_probe_program(2, amended_fee));
    });
}

#[test]
fn missing_malformed_or_zero_live_probe_params_fail_closed_across_both_consumers() {
    use pallet_constitution::ParamValue;

    let cases = [
        (
            b"ops.probe_fee".as_slice(),
            ParamValue::U32(1),
            ParamValue::Balance(0),
        ),
        (
            b"ops.probe_rate".as_slice(),
            ParamValue::U32(1),
            ParamValue::Balance(0),
        ),
        (
            b"res.probe_int".as_slice(),
            ParamValue::Balance(1),
            ParamValue::U32(0),
        ),
        (
            b"res.probe_to".as_slice(),
            ParamValue::Balance(1),
            ParamValue::U32(0),
        ),
        (
            b"res.probe_amount".as_slice(),
            ParamValue::U32(1),
            ParamValue::Balance(0),
        ),
        (
            b"res.fail_thr".as_slice(),
            ParamValue::Balance(1),
            ParamValue::U8(0),
        ),
        (
            b"res.recover_thr".as_slice(),
            ParamValue::Balance(1),
            ParamValue::U8(0),
        ),
    ];

    for (name, malformed, zero) in cases {
        for mode in 0u8..3 {
            development_ext().execute_with(|| {
                use cumulus_primitives_core::XcmpMessageSource;

                ParachainSystem::open_outbound_hrmp_channel_for_benchmarks_or_tests(
                    cumulus_primitives_core::ParaId::from(chain_identity::ASSET_HUB_PARA_ID),
                );
                let interval = reserve_probe_interval();
                let key = pallet_constitution::key16(name);
                let original = pallet_constitution::Params::<Runtime>::get(key)
                    .expect("canonical probe parameter");
                let corrupt = || match mode {
                    0 => pallet_constitution::Params::<Runtime>::remove(key),
                    1 => pallet_constitution::Params::<Runtime>::mutate(key, |record| {
                        record.as_mut().expect("canonical probe parameter").value = malformed;
                    }),
                    _ => pallet_constitution::Params::<Runtime>::mutate(key, |record| {
                        record.as_mut().expect("canonical probe parameter").value = zero;
                    }),
                };
                corrupt();

                // The same exact live envelope caps the temporary Signed bootstrap
                // path. An unavailable field refuses before moving MAIN, the line,
                // events or the one-way handover latch.
                let authority = pallet_futarchy_treasury::CoretimeQuoteAuthority::<Runtime>::get()
                    .expect("development ops authority");
                let bootstrap_state = pallet_futarchy_treasury::State::<Runtime>::get().encode();
                let bootstrap_latch =
                    pallet_futarchy_treasury::BootstrapOpsFundingClosed::<Runtime>::get();
                assert_noop!(
                    FutarchyTreasury::fund_budget_line(
                        RuntimeOrigin::signed(authority),
                        BudgetLine::OpsReserveProbe,
                        1,
                    ),
                    pallet_futarchy_treasury::Error::<Runtime>::BootstrapOpsFundingLimit
                );
                assert_eq!(
                    pallet_futarchy_treasury::State::<Runtime>::get().encode(),
                    bootstrap_state,
                    "signed bootstrap state changed for {name:?}, mode={mode}",
                );
                assert_eq!(
                    pallet_futarchy_treasury::BootstrapOpsFundingClosed::<Runtime>::get(),
                    bootstrap_latch,
                    "signed bootstrap latch changed for {name:?}, mode={mode}",
                );

                // Binding governance may still fund the dedicated line. That does
                // not make an incomplete live probe envelope ready to arm.
                fund_reserve_probe_line();
                System::reset_events();
                let treasury_before = pallet_futarchy_treasury::State::<Runtime>::get().encode();
                let health_before = pallet_oracle::ReserveHealth::<Runtime>::get();
                System::set_block_number(interval);

                assert_noop!(
                    Oracle::crank_reserve_probe(RuntimeOrigin::signed(AccountId::from([9; 32]))),
                    pallet_oracle::Error::<Runtime>::ProbeUnavailable
                );

                assert_eq!(
                    pallet_futarchy_treasury::State::<Runtime>::get().encode(),
                    treasury_before,
                    "{name:?}, mode={mode}",
                );
                assert_eq!(
                    pallet_oracle::ReserveHealth::<Runtime>::get(),
                    health_before
                );
                assert!(!Oracle::reserve_probe_armed());
                let queued_after_refusal = XcmpQueue::take_outbound_messages(1, &[]);
                assert!(
                    queued_after_refusal.is_empty(),
                    "unexpected pre-arm probe queue for {name:?}, mode={mode}: {} messages",
                    queued_after_refusal.len(),
                );

                // Arm once under a complete envelope, then corrupt the same input
                // again. The latch is monotone, so the next due attempt opens and
                // will timeout fail-static even though no message can be sent.
                pallet_constitution::Params::<Runtime>::insert(key, original);
                assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
                    AccountId::from([9; 32])
                )));
                let _ = take_queued_probe_program();
                // The queue emits its one-time negotiated-format notification only
                // after the first data page is consumed. Drain and identify that
                // signal so the later assertion distinguishes it from a probe.
                let format_signal = XcmpQueue::take_outbound_messages(1, &[]);
                assert_eq!(format_signal.len(), 1);
                let mut encoded = format_signal[0].1.as_slice();
                assert_eq!(
                    cumulus_primitives_core::XcmpMessageFormat::decode(&mut encoded)
                        .expect("XCMP format signal"),
                    cumulus_primitives_core::XcmpMessageFormat::ConcatenatedOpaqueVersionedXcm,
                );
                execute_probe_response(1, Response::ExecutionResult(None));
                corrupt();
                System::reset_events();
                let after_arm_probe_line =
                    FutarchyTreasury::line_balance(BudgetLine::OpsReserveProbe);
                let after_arm_latch =
                    pallet_futarchy_treasury::BootstrapOpsFundingClosed::<Runtime>::get();
                System::set_block_number(interval.saturating_mul(2));
                assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
                    AccountId::from([9; 32])
                )));
                assert!(Oracle::reserve_probe_armed());
                assert_eq!(
                    pallet_oracle::ReserveHealth::<Runtime>::get().pending_since,
                    Some(interval.saturating_mul(2)),
                );
                assert_eq!(
                    FutarchyTreasury::line_balance(BudgetLine::OpsReserveProbe),
                    after_arm_probe_line,
                    "refused send charged the probe line for {name:?}, mode={mode}",
                );
                assert_eq!(
                    pallet_futarchy_treasury::BootstrapOpsFundingClosed::<Runtime>::get(),
                    after_arm_latch,
                    "refused send changed the handover latch for {name:?}, mode={mode}",
                );
                let queued_after_refusal = XcmpQueue::take_outbound_messages(1, &[]);
                assert!(
                    queued_after_refusal.is_empty(),
                    "unexpected post-arm probe queue for {name:?}, mode={mode}: {} messages",
                    queued_after_refusal.len(),
                );
                assert!(!System::events().iter().any(|record| matches!(
                    &record.event,
                    RuntimeEvent::FutarchyTreasury(
                        pallet_futarchy_treasury::Event::ReserveProbeFeeCharged { .. }
                    )
                )));

                let timeout =
                    <configs::RuntimeOracleParams as pallet_oracle::OracleParamsProvider>::get()
                        .probe_timeout;
                System::set_block_number(interval.saturating_mul(2).saturating_add(timeout));
                assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
                    AccountId::from([9; 32])
                )));
                let health = pallet_oracle::ReserveHealth::<Runtime>::get();
                assert!(
                    health.consecutive_fails >= 1,
                    "timeout did not score fail-static for {name:?}, mode={mode}",
                );
                assert_eq!(
                    FutarchyTreasury::line_balance(BudgetLine::OpsReserveProbe),
                    after_arm_probe_line,
                    "timeout/fresh refusal charged the probe line for {name:?}, mode={mode}",
                );
                let queued_after_timeout = XcmpQueue::take_outbound_messages(1, &[]);
                assert!(
                    queued_after_timeout.is_empty(),
                    "unexpected timeout/fresh probe queue for {name:?}, mode={mode}: {} messages",
                    queued_after_timeout.len(),
                );
            });
        }
    }
}

#[test]
fn production_probe_authentication_refusals_and_replay_leave_health_unchanged() {
    let flag = bleavit_xcm::probe::PROBE_QUERY_ID_FLAG;
    let cases = [
        (
            Location::parent(),
            1 | flag,
            Some(Location::here()),
            "wrong origin",
        ),
        (
            bleavit_xcm::identity::asset_hub_location(),
            1 | flag,
            None,
            "missing querier",
        ),
        (
            bleavit_xcm::identity::asset_hub_location(),
            1 | flag,
            Some(Location::parent()),
            "wrong querier",
        ),
        (
            bleavit_xcm::identity::asset_hub_location(),
            2 | flag,
            Some(Location::here()),
            "unknown flagged id",
        ),
        (
            bleavit_xcm::identity::asset_hub_location(),
            1,
            Some(Location::here()),
            "equal unflagged id",
        ),
    ];

    for (origin, query_id, querier, label) in cases {
        development_ext().execute_with(|| {
            fund_reserve_probe_line();
            ParachainSystem::open_outbound_hrmp_channel_for_benchmarks_or_tests(
                cumulus_primitives_core::ParaId::from(chain_identity::ASSET_HUB_PARA_ID),
            );
            System::set_block_number(reserve_probe_interval());
            assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
                AccountId::from([9; 32])
            )));
            let _ = take_queued_probe_program();
            System::reset_events();
            let before = health_snapshot();

            assert!(
                try_execute_probe_response(
                    origin,
                    query_id,
                    querier,
                    Response::ExecutionResult(None),
                )
                .is_err(),
                "{label}",
            );
            assert_eq!(health_snapshot(), before, "{label}");
        });
    }

    development_ext().execute_with(|| {
        fund_reserve_probe_line();
        ParachainSystem::open_outbound_hrmp_channel_for_benchmarks_or_tests(
            cumulus_primitives_core::ParaId::from(chain_identity::ASSET_HUB_PARA_ID),
        );
        System::set_block_number(reserve_probe_interval());
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
            AccountId::from([9; 32])
        )));
        execute_probe_response(1, Response::ExecutionResult(None));
        System::reset_events();
        let after_first = health_snapshot();
        assert!(try_execute_probe_response(
            bleavit_xcm::identity::asset_hub_location(),
            1 | flag,
            Some(Location::here()),
            Response::ExecutionResult(None),
        )
        .is_err());
        assert_eq!(health_snapshot(), after_first, "replayed response");
    });
}

#[test]
fn production_probe_query_namespace_routes_the_maximum_oracle_id_losslessly() {
    let flag = bleavit_xcm::probe::PROBE_QUERY_ID_FLAG;
    let maximum = pallet_oracle::MAX_RESERVE_PROBE_QUERY_ID;
    assert_eq!(maximum, flag - 1);
    let wire_query_id = maximum | flag;
    assert_eq!(wire_query_id, u64::MAX);
    assert_eq!(wire_query_id & !flag, maximum);

    development_ext().execute_with(|| {
        pallet_oracle::ReserveHealth::<Runtime>::put(pallet_oracle::ReserveHealthValue {
            consecutive_fails: 0,
            consecutive_passes: 0,
            unhealthy: false,
            last_query_id: maximum,
            last_probe_at: 1,
            pending_since: Some(1),
        });

        assert!(try_execute_probe_response(
            bleavit_xcm::identity::asset_hub_location(),
            wire_query_id,
            Some(Location::here()),
            Response::ExecutionResult(None),
        )
        .is_ok());
        let health = pallet_oracle::ReserveHealth::<Runtime>::get();
        assert_eq!(health.last_query_id, maximum);
        assert_eq!(health.pending_since, None);
        assert_eq!(health.consecutive_passes, 1);
    });
}

#[test]
fn production_probe_response_at_timeout_boundary_is_fail_static() {
    development_ext().execute_with(|| {
        fund_reserve_probe_line();
        ParachainSystem::open_outbound_hrmp_channel_for_benchmarks_or_tests(
            cumulus_primitives_core::ParaId::from(chain_identity::ASSET_HUB_PARA_ID),
        );
        let interval = reserve_probe_interval();
        System::set_block_number(interval);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
            AccountId::from([9; 32])
        )));
        let timeout = <configs::RuntimeOracleParams as pallet_oracle::OracleParamsProvider>::get()
            .probe_timeout;
        System::set_block_number(interval.saturating_add(timeout));
        execute_probe_response(1, Response::ExecutionResult(None));
        let health = pallet_oracle::ReserveHealth::<Runtime>::get();
        assert_eq!(health.pending_since, None);
        assert_eq!(health.consecutive_fails, 1);
        assert_eq!(health.consecutive_passes, 0);
    });
}

#[test]
fn production_probe_sink_refusal_rolls_back_all_health_mirrors() {
    development_ext().execute_with(|| {
        fund_reserve_probe_line();
        ParachainSystem::open_outbound_hrmp_channel_for_benchmarks_or_tests(
            cumulus_primitives_core::ParaId::from(chain_identity::ASSET_HUB_PARA_ID),
        );
        pallet_oracle::ReserveHealth::<Runtime>::mutate(|health| {
            health.consecutive_fails = pallet_oracle::OracleParams::DEFAULT
                .fail_threshold
                .saturating_sub(1)
        });
        System::set_block_number(reserve_probe_interval());
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
            AccountId::from([9; 32])
        )));
        pallet_constitution::PhaseFlags::<Runtime>::put(1_u32 << 31);
        System::reset_events();
        let before = health_snapshot();

        execute_probe_response(
            1,
            Response::ExecutionResult(Some((0, XcmError::Unimplemented))),
        );
        assert_eq!(health_snapshot(), before);
        assert_eq!(
            pallet_oracle::ReserveHealth::<Runtime>::get().pending_since,
            Some(reserve_probe_interval()),
        );
    });
}

#[test]
fn production_probe_without_hrmp_channel_rolls_back_debit_but_counts_one_local_failure() {
    development_ext().execute_with(|| {
        fund_reserve_probe_line();
        let before = FutarchyTreasury::line_balance(BudgetLine::OpsReserveProbe);
        let interval = reserve_probe_interval();
        System::set_block_number(interval);
        let epoch = pallet_epoch::EpochOf::<Runtime>::get().index;
        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        let day = u8::try_from(
            interval.saturating_sub(schedule.epoch_start_block)
                / futarchy_primitives::kernel::BLOCKS_PER_DAY,
        )
        .unwrap_or(u8::MAX);

        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
            AccountId::from([9; 32])
        )));

        assert_eq!(
            FutarchyTreasury::line_balance(BudgetLine::OpsReserveProbe),
            before
        );
        let traffic = Welfare::xcm_traffic(epoch, day);
        assert_eq!(traffic.accepted, 0);
        assert_eq!(traffic.failed, 1);
        assert_eq!(traffic.probe_timeouts, 0);
        assert_eq!(
            pallet_oracle::ReserveHealth::<Runtime>::get().pending_since,
            Some(interval)
        );
        assert!(Oracle::reserve_probe_armed());
        assert!(!System::events().iter().any(|record| {
            matches!(
                &record.event,
                RuntimeEvent::FutarchyTreasury(
                    pallet_futarchy_treasury::Event::ReserveProbeFeeCharged { .. }
                )
            )
        }));
    });
}

#[test]
fn production_probe_failures_and_funded_recovery_keep_all_health_mirrors_atomic() {
    development_ext().execute_with(|| {
        fund_reserve_probe_line();
        ParachainSystem::open_outbound_hrmp_channel_for_benchmarks_or_tests(
            cumulus_primitives_core::ParaId::from(chain_identity::ASSET_HUB_PARA_ID),
        );
        let keeper = AccountId::from([9; 32]);

        for query_id in 1..=2 {
            System::set_block_number(reserve_probe_interval().saturating_mul(query_id as u32));
            assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
                keeper.clone()
            )));
            execute_probe_response(
                query_id,
                Response::ExecutionResult(Some((0, XcmError::Unimplemented))),
            );
        }

        assert!(Oracle::reserve_unhealthy());
        assert!(FutarchyTreasury::treasury().reserve_impaired);
        assert_eq!(FutarchyTreasury::nav().spendable_nav, 0);
        assert_ne!(
            Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG,
            0
        );

        // The dedicated probe line is the narrow maintenance carve-out that
        // remains usable while the reserve haircut is active; three paid passes
        // clear the oracle and both mirrors together.
        for query_id in 3..=5 {
            System::set_block_number(reserve_probe_interval().saturating_mul(query_id as u32));
            assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(
                keeper.clone()
            )));
            execute_probe_response(query_id, Response::ExecutionResult(None));
        }

        assert!(!Oracle::reserve_unhealthy());
        assert!(!FutarchyTreasury::treasury().reserve_impaired);
        assert!(FutarchyTreasury::nav().spendable_nav > 0);
        assert_eq!(
            Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG,
            0
        );
    });
}

// ---------------------------------------------------------------- SQ-207 ---

#[test]
fn sweep_insurance_moves_real_usdc_from_insurance_to_main_and_raises_nav() {
    development_ext().execute_with(|| {
        let amount: Balance = 5_000 * futarchy_primitives::currency::USDC;
        let main = crate::genesis::treasury_account();

        assert_ok!(<ForeignAssets as Mutate<AccountId>>::mint_into(
            bleavit_xcm::identity::usdc_location(),
            &insurance_account(),
            amount * 4,
        ));
        let insurance_before = <ForeignAssets as Inspect<AccountId>>::balance(
            bleavit_xcm::identity::usdc_location(),
            &insurance_account(),
        );
        let main_before = <ForeignAssets as Inspect<AccountId>>::balance(
            bleavit_xcm::identity::usdc_location(),
            &main,
        );
        let nav_before = FutarchyTreasury::nav().nav;

        assert_ok!(FutarchyTreasury::sweep_insurance(
            pallet_origins::Origin::FutarchyTreasury.into(),
            amount
        ));

        // Real custody moved INSURANCE → MAIN, and only there.
        assert_eq!(
            <ForeignAssets as Inspect<AccountId>>::balance(
                bleavit_xcm::identity::usdc_location(),
                &insurance_account()
            ),
            insurance_before - amount
        );
        assert_eq!(
            <ForeignAssets as Inspect<AccountId>>::balance(
                bleavit_xcm::identity::usdc_location(),
                &main
            ),
            main_before + amount
        );
        // 08 §1.2: INSURANCE is outside NAV, so NAV rises by exactly `amount`.
        assert_eq!(FutarchyTreasury::nav().nav, nav_before + amount);
    });
}

#[test]
fn sweep_insurance_is_refused_to_every_non_treasury_origin() {
    development_ext().execute_with(|| {
        let amount: Balance = 1_000 * futarchy_primitives::currency::USDC;
        // The complete 06 §3.2 matrix row: only the `FutarchyTreasury` column
        // carries a tick, so every other column — and Root/Signed/none — must be
        // refused. 08 §1.2: "No guardian power, no playbook and no admin origin
        // can reach it."
        for bad in [
            RuntimeOrigin::signed(crate::genesis::treasury_account()),
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
            pallet_origins::Origin::FutarchyParam.into(),
            pallet_origins::Origin::FutarchyCode.into(),
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_origins::Origin::ConstitutionalValues.into(),
            pallet_origins::Origin::OracleResolution.into(),
            pallet_origins::Origin::GuardianHold.into(),
            pallet_origins::Origin::EmergencyPlaybook.into(),
        ] {
            assert_noop!(
                FutarchyTreasury::sweep_insurance(bad, amount),
                sp_runtime::DispatchError::BadOrigin
            );
        }
    });
}

#[test]
fn sweep_insurance_preserves_the_insurance_account_rather_than_reaping_it() {
    development_ext().execute_with(|| {
        // 03 §7 R-4 / 08 §1.4: at most `balance - min_balance` is sweepable and
        // an over-large request fails whole (G-1) instead of reaping INSURANCE.
        assert_ok!(<ForeignAssets as Mutate<AccountId>>::mint_into(
            bleavit_xcm::identity::usdc_location(),
            &insurance_account(),
            2_000 * futarchy_primitives::currency::USDC,
        ));
        // The account is genesis-endowed, so read the live balance rather than
        // assuming the mint is all of it.
        let held = <ForeignAssets as Inspect<AccountId>>::balance(
            bleavit_xcm::identity::usdc_location(),
            &insurance_account(),
        );
        let nav_before = FutarchyTreasury::nav().nav;

        // Sweeping the *whole* balance would reap the account, so Preserve must
        // refuse it outright rather than partially satisfying the request.
        assert!(FutarchyTreasury::sweep_insurance(
            pallet_origins::Origin::FutarchyTreasury.into(),
            held,
        )
        .is_err());

        // Nothing moved and NAV never recorded USDC the treasury did not get.
        assert_eq!(
            <ForeignAssets as Inspect<AccountId>>::balance(
                bleavit_xcm::identity::usdc_location(),
                &insurance_account()
            ),
            held
        );
        assert_eq!(FutarchyTreasury::nav().nav, nav_before);
    });
}

// ---------------------------------------------------------------- SQ-180 ---
//
// 08 §4.2: arming a proposal class REQUIRES published spendable NAV ≥ the class
// floor of 08 §4.1, and under the 08 §1.2 haircut spendable NAV is 0 so every
// class fails (fail-static).

#[test]
fn arming_a_class_below_its_nav_floor_is_refused_and_leaves_flags_unchanged() {
    development_ext().execute_with(|| {
        let flags_before = Constitution::phase_flags();
        // The development genesis is far below the 08 §4.1 PARAM floor.
        assert!(
            FutarchyTreasury::nav().spendable_nav < FutarchyTreasury::floor(ProposalClass::Param)
        );

        assert_noop!(
            Constitution::set_phase_flag(
                RuntimeOrigin::root(),
                pallet_constitution::PhaseFlagsValue::PARAM_ARMED,
                true
            ),
            pallet_constitution::Error::<Runtime>::NavFloorUnmet
        );
        assert_eq!(Constitution::phase_flags(), flags_before);
    });
}

#[test]
fn arming_is_fail_static_under_the_reserve_health_haircut() {
    development_ext().execute_with(|| {
        use pallet_oracle::ReserveHealthSink;

        // Fund MAIN well past every 08 §4.1 floor so only the haircut can bite —
        // through the real INSURANCE sweep rather than a test-only backdoor.
        let far_above = FutarchyTreasury::floor(ProposalClass::Meta) * 4;
        assert_ok!(<ForeignAssets as Mutate<AccountId>>::mint_into(
            bleavit_xcm::identity::usdc_location(),
            &insurance_account(),
            far_above,
        ));
        assert_ok!(FutarchyTreasury::sweep_insurance(
            pallet_origins::Origin::FutarchyTreasury.into(),
            far_above
        ));
        assert!(
            FutarchyTreasury::nav().spendable_nav >= FutarchyTreasury::floor(ProposalClass::Meta)
        );

        // Armable now…
        assert_ok!(Constitution::set_phase_flag(
            RuntimeOrigin::root(),
            pallet_constitution::PhaseFlagsValue::PARAM_ARMED,
            true
        ));

        // …and refused once the 08 §1.2 flag zeroes spendable NAV.
        assert_ok!(configs::ReserveHealthToConstitutionAndTreasury::reserve_health_changed(true));
        assert_eq!(FutarchyTreasury::nav().spendable_nav, 0);
        let flags_before = Constitution::phase_flags();
        assert_noop!(
            Constitution::set_phase_flag(
                RuntimeOrigin::root(),
                pallet_constitution::PhaseFlagsValue::TREASURY_ARMED,
                true
            ),
            pallet_constitution::Error::<Runtime>::NavFloorUnmet
        );
        assert_eq!(Constitution::phase_flags(), flags_before);

        // Disarming stays available so the chain is never stranded armed.
        assert_ok!(Constitution::set_phase_flag(
            RuntimeOrigin::root(),
            pallet_constitution::PhaseFlagsValue::PARAM_ARMED,
            false
        ));
    });
}

// --- SQ-381: the loud signal is the durable extrinsic failure, not an event ---
//
// 08 §4.2 (as amended, Option A / SQ-381): a below-floor arming attempt is
// refused with the module error `NavFloorUnmet`, leaving the arming bits
// unchanged (fail-static). FRAME cannot both return that `Err` and deposit a
// pallet event — the `Err` rolls any in-dispatch event back — and the
// unchanged-flags requirement is what mandates the `Err`. So on the *blocking*
// path the loud signal is the extrinsic failure itself, surfaced durably by the
// runtime. This drives the real production arming caller — bootstrap sudo
// (09 §5.4) — and pins all three properties at once: the durable, operator/FE-
// observable `Sudid { Err(NavFloorUnmet) }`; the untouched flags; and the
// absence of any pallet `NavFloorUnmet` event on this blocking path. It would
// fail against any regression that swallowed the error (returned `Ok`) or that
// switched to the Option-B "emit an event and return `Ok` without arming" shape.

#[test]
fn arming_below_floor_surfaces_the_module_error_durably_via_sudo_with_no_pallet_event() {
    development_ext().execute_with(|| {
        // frame_system records events from block 1 onwards.
        System::set_block_number(1);

        // The dev preset installs Alice as the bootstrap sudo key (09 §5.4).
        let sudo_key = AccountId::new(crate::genesis::ALICE_PUBLIC);
        assert_eq!(pallet_sudo::Key::<Runtime>::get(), Some(sudo_key.clone()));

        // Dev genesis NAV is far below the 08 §4.1 PARAM floor, so arming refuses.
        assert!(
            FutarchyTreasury::nav().spendable_nav < FutarchyTreasury::floor(ProposalClass::Param)
        );
        let flags_before = Constitution::phase_flags();

        let arm = RuntimeCall::Constitution(pallet_constitution::Call::set_phase_flag {
            flag: pallet_constitution::PhaseFlagsValue::PARAM_ARMED,
            enabled: true,
        });
        // The OUTER sudo extrinsic SUCCEEDS: sudo dispatches the inner call with
        // Root, captures its `Err` in the `Sudid` event, and returns `Ok`, so the
        // event is committed (not rolled back with the failed inner dispatch).
        assert_ok!(Sudo::sudo(RuntimeOrigin::signed(sudo_key), Box::new(arm)));

        // The module error is durably observable: `Sudid` carries exactly it.
        // (`ModuleError`'s PartialEq ignores the codec-skipped `message`, so this
        // holds across the event's SCALE round-trip through storage.)
        let expected: sp_runtime::DispatchResult =
            Err(pallet_constitution::Error::<Runtime>::NavFloorUnmet.into());
        assert!(
            System::events().iter().any(|record| matches!(
                &record.event,
                RuntimeEvent::Sudo(pallet_sudo::Event::Sudid { sudo_result })
                    if sudo_result == &expected
            )),
            "a below-floor arming must surface NavFloorUnmet durably as Sudid {{ Err(..) }}"
        );

        // Fail-static: the arming bits are exactly as they were.
        assert_eq!(Constitution::phase_flags(), flags_before);

        // The loud signal is the extrinsic failure, NOT a pallet event: nothing on
        // this blocking path deposits the field-carrying treasury `NavFloorUnmet`
        // event (that event is the non-blocking `flag_nav_floor` variant's job).
        assert!(
            !System::events().iter().any(|record| matches!(
                record.event,
                RuntimeEvent::FutarchyTreasury(
                    pallet_futarchy_treasury::Event::NavFloorUnmet { .. }
                )
            )),
            "the blocking arming path must not deposit a pallet NavFloorUnmet event"
        );
    });
}

// ------------------- SQ-207: the real screening path (spec-review fix) ------
//
// The first cut of this file only ever constructed `Origin::FutarchyTreasury`
// directly, so it never exercised T4 screening — and three exhaustive-match
// sites had no `sweep_insurance` arm. Each failed *closed*, and the capability
// one failed closed into `SlashAll(ConstitutionViolation)`: a lawful sweep would
// have confiscated 100 % of the proposer's intake bond. These tests walk the
// path a real TREASURY decision takes, so an omitted arm can never pass again.

/// 05 §1.4 family `0x0B`: the leaf must classify, or T4 cancels the payload.
#[test]
fn sweep_insurance_derives_its_canonical_resource_key() {
    development_ext().execute_with(|| {
        let call = RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::sweep_insurance {
            amount: 1_000 * futarchy_primitives::currency::USDC,
        });
        let footprint = crate::classifier::derive_resource_footprint(&[call]);
        assert!(
            footprint.is_ok(),
            "sweep_insurance must be classifiable (05 §1.4 `0x0B`), not Unclassifiable"
        );
        let Ok(footprint) = footprint else { return };
        assert_eq!(footprint.len(), 1);
        // Singleton family: the amount must not enter the discriminator, so two
        // different sweeps contend on the same INSURANCE lock.
        let other =
            RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::sweep_insurance {
                amount: 7 * futarchy_primitives::currency::USDC,
            });
        let Ok(other_footprint) = crate::classifier::derive_resource_footprint(&[other]) else {
            panic!("second sweep must classify");
        };
        assert_eq!(footprint[0], other_footprint[0]);
        assert_eq!(footprint[0][0], 0x0B, "family tag must be 05 §1.4 `0x0B`");
    });
}

/// The capability gap that would have slashed the whole bond.
#[test]
fn a_treasury_sweep_proposal_passes_static_screening() {
    development_ext().execute_with(|| {
        let call = RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::sweep_insurance {
            amount: 1_000 * futarchy_primitives::currency::USDC,
        });
        let Ok(footprint) =
            crate::classifier::derive_resource_footprint(std::slice::from_ref(&call))
        else {
            panic!("sweep must classify");
        };
        let Some((payload_hash, payload_len)) = crate::tests::note_runtime_batch(vec![call]) else {
            panic!("payload must note");
        };

        let mut proposal = crate::tests::empty_param_proposal(
            9_301,
            crate::tests::account(77),
            payload_hash,
            payload_len,
        );
        proposal.class = ProposalClass::Treasury;
        proposal.bond = crate::configs::balance_param(b"prop.bond.trs");
        let Ok(resources) = futarchy_primitives::BoundedVec::try_from(footprint.to_vec()) else {
            panic!("footprint must fit the 05 §1.4 lock bound");
        };
        proposal.resources = resources;

        let disposition =
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                AccountId,
            >>::static_check(&proposal);
        assert_eq!(
            disposition,
            pallet_epoch::StaticCheckDisposition::Eligible,
            "a lawful INSURANCE sweep must screen clean — an omitted capability \
             arm previously made this SlashAll(ConstitutionViolation)"
        );
    });
}

/// 05 §1.4 / 08 §1.4: the sweep is an inflow, so its derived ask is zero — and a
/// `None` ask (the pre-fix behaviour) blocks Adopt at sizing.
#[test]
fn sweep_insurance_derives_a_zero_treasury_ask_so_sizing_can_complete() {
    development_ext().execute_with(|| {
        let call = RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::sweep_insurance {
            amount: 4_000 * futarchy_primitives::currency::USDC,
        });
        let Ok(footprint) =
            crate::classifier::derive_resource_footprint(std::slice::from_ref(&call))
        else {
            panic!("sweep must classify");
        };
        let Some((payload_hash, payload_len)) = crate::tests::note_runtime_batch(vec![call]) else {
            panic!("payload must note");
        };
        let mut proposal = crate::tests::empty_param_proposal(
            9_302,
            crate::tests::account(78),
            payload_hash,
            payload_len,
        );
        proposal.class = ProposalClass::Treasury;
        proposal.bond = crate::configs::balance_param(b"prop.bond.trs");
        let Ok(resources) = futarchy_primitives::BoundedVec::try_from(footprint.to_vec()) else {
            panic!("footprint must fit the 05 §1.4 lock bound");
        };
        proposal.resources = resources;

        let prize =
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                AccountId,
            >>::in_cap_prize(&proposal);
        assert!(
            prize.is_some(),
            "a derivable zero-outflow sweep must yield a prize, not None"
        );
    });
}
