use crate::{
    assets::{BleavitReserves, PinnedAssetMatcher},
    barrier::{AcceptedXcmOrigins, DenyTransact},
    caps::CappedInflows,
    coretime::coretime_renewal_program,
    filter::{classify_pallet_xcm_call, XcmCallDisposition},
    health::HealthTrackingRouter,
    identity::{
        asset_hub_location, bleavit_as_seen_from_asset_hub, coretime_location, dot_location,
        relay_location, usdc_location, XCM_VERSION_PINNED,
    },
    mock::*,
    probe::{reserve_probe_program, ProbeAwareResponseHandler, ProbeSink, PROBE_QUERY_ID_FLAG},
    trader::GovernedWeightTrader,
};
use frame_support::{
    assert_ok,
    traits::{fungibles::Mutate, Contains, ContainsPair},
    weights::Weight,
};
use futarchy_primitives::chain_identity::{
    ASSET_HUB_PARA_ID, CORETIME_PARA_ID, FIXTURE_PARA_ID, USDC_ASSET_INDEX, USDC_PALLET_INSTANCE,
};
use oracle_core::{RES_PROBE_INTERVAL, RES_PROBE_TIMEOUT};
use pallet_futarchy_treasury::BudgetLine;
use parity_scale_codec::Encode;
use staging_xcm::{
    latest::{prelude::*, validate_send},
    VersionedAssetId, VersionedAssets, VersionedLocation, VersionedXcm,
};
use staging_xcm_builder::TakeRevenue;
use staging_xcm_executor::{
    test_helpers::mock_asset_to_holding,
    traits::{
        ConvertLocation, DenyExecution, MatchesFungibles, OnResponse, Properties, ShouldExecute,
        WeightTrader,
    },
    AssetsInHolding,
};
use std::{cell::RefCell, vec};

const MAX_WEIGHT: Weight = Weight::from_parts(100, 100);

fn asset(location: Location, amount: u128) -> Asset {
    Asset {
        id: AssetId(location),
        fun: Fungible(amount),
    }
}

fn account_location(id: [u8; 32]) -> Location {
    Location::new(0, [AccountId32 { network: None, id }])
}

fn transact() -> Instruction<()> {
    Transact {
        origin_kind: OriginKind::SovereignAccount,
        fallback_max_weight: None,
        call: ().encode().into(),
    }
}

fn paid_message(fee_asset: Asset) -> Xcm<()> {
    Xcm(vec![
        WithdrawAsset(vec![fee_asset.clone()].into()),
        BuyExecution {
            fees: fee_asset,
            weight_limit: Limited(MAX_WEIGHT),
        },
    ])
}

fn barrier_result<B: ShouldExecute>(origin: Location, xcm: Xcm<()>) -> bool {
    let mut instructions = xcm.0;
    let mut properties = Properties {
        weight_credit: Weight::zero(),
        message_id: None,
    };
    B::should_execute::<()>(&origin, &mut instructions, MAX_WEIGHT, &mut properties).is_ok()
}

fn contains_transact(message: &Xcm<()>) -> bool {
    message.0.iter().any(|instruction| match instruction {
        Transact { .. } => true,
        SetAppendix(nested) | SetErrorHandler(nested) => contains_transact(nested),
        TransferReserveAsset { xcm, .. }
        | DepositReserveAsset { xcm, .. }
        | InitiateReserveWithdraw { xcm, .. }
        | InitiateTeleport { xcm, .. }
        | ExportMessage { xcm, .. }
        | InitiateTransfer {
            remote_xcm: xcm, ..
        } => contains_transact(xcm),
        ExecuteWithOrigin { xcm, .. } => contains_transact(xcm),
        _ => false,
    })
}

// -------------------------------------------------------------------------
// Identity + assets/reserves (02 §8; 09 §6.1)
// -------------------------------------------------------------------------

#[test]
fn identity_group_pins_v5_and_all_canonical_locations() {
    assert_eq!(XCM_VERSION_PINNED, staging_xcm::latest::VERSION);
    assert_eq!(relay_location(), Location::parent());
    assert_eq!(dot_location(), Location::parent());
    assert_eq!(
        asset_hub_location(),
        Location::new(1, [Parachain(ASSET_HUB_PARA_ID)])
    );
    assert_eq!(
        coretime_location(),
        Location::new(1, [Parachain(CORETIME_PARA_ID)])
    );
    assert_eq!(
        usdc_location(),
        Location::new(
            1,
            [
                Parachain(ASSET_HUB_PARA_ID),
                PalletInstance(USDC_PALLET_INSTANCE),
                GeneralIndex(USDC_ASSET_INDEX),
            ]
        )
    );
}

#[test]
fn assets_group_matcher_admits_only_pinned_usdc_and_dot() {
    let usdc = asset(usdc_location(), 11);
    let dot = asset(dot_location(), 12);
    assert_eq!(
        <PinnedAssetMatcher as MatchesFungibles<Location, u128>>::matches_fungibles(&usdc),
        Ok((usdc_location(), 11))
    );
    assert_eq!(
        <PinnedAssetMatcher as MatchesFungibles<Location, u128>>::matches_fungibles(&dot),
        Ok((dot_location(), 12))
    );

    for unknown in [
        Location::new(1, [Parachain(1000), PalletInstance(50), GeneralIndex(1338)]),
        Location::new(1, [Parachain(1000), PalletInstance(51), GeneralIndex(1337)]),
        Location::new(1, [Parachain(1001), PalletInstance(50), GeneralIndex(1337)]),
    ] {
        assert!(
            <PinnedAssetMatcher as MatchesFungibles<Location, u128>>::matches_fungibles(&asset(
                unknown, 1
            ))
            .is_err()
        );
    }
}

#[test]
fn assets_group_reserves_are_exact_and_teleports_are_disabled() {
    let usdc = asset(usdc_location(), 1);
    let dot = asset(dot_location(), 1);
    assert!(BleavitReserves::contains(&usdc, &asset_hub_location()));
    assert!(!BleavitReserves::contains(&usdc, &relay_location()));
    assert!(BleavitReserves::contains(&dot, &relay_location()));
    assert!(BleavitReserves::contains(&dot, &asset_hub_location()));
    assert!(!BleavitReserves::contains(&usdc, &coretime_location()));
    assert!(!BleavitReserves::contains(&dot, &coretime_location()));
    assert!(!<() as ContainsPair<Asset, Location>>::contains(
        &usdc,
        &asset_hub_location()
    ));
}

// -------------------------------------------------------------------------
// Barrier (09 §6.1 rule table)
// -------------------------------------------------------------------------

#[test]
fn barrier_group_accepted_origins_are_exact() {
    assert!(AcceptedXcmOrigins::contains(&asset_hub_location()));
    assert!(AcceptedXcmOrigins::contains(&relay_location()));
    assert!(AcceptedXcmOrigins::contains(&coretime_location()));
    assert!(!AcceptedXcmOrigins::contains(&Location::new(
        1,
        [Parachain(2000)]
    )));
    assert!(!AcceptedXcmOrigins::contains(&Location::here()));
}

#[test]
fn barrier_group_transact_is_denied_from_every_origin_and_when_nested() {
    for origin in [
        asset_hub_location(),
        relay_location(),
        coretime_location(),
        Location::new(1, [Parachain(2000)]),
    ] {
        let mut direct = vec![transact()];
        let mut properties = Properties {
            weight_credit: MAX_WEIGHT,
            message_id: None,
        };
        assert!(
            DenyTransact::deny_execution(&origin, &mut direct, MAX_WEIGHT, &mut properties)
                .is_err()
        );

        let nested = Xcm(vec![
            WithdrawAsset(vec![asset(usdc_location(), 1)].into()),
            BuyExecution {
                fees: asset(usdc_location(), 1),
                weight_limit: Limited(MAX_WEIGHT),
            },
            SetAppendix(Xcm(vec![transact()])),
        ]);
        assert!(!barrier_result::<BarrierWithKnownResponse>(origin, nested));
    }
}

#[test]
fn barrier_group_unpaid_and_unknown_paid_origins_are_refused() {
    let unpaid = Xcm(vec![UnpaidExecution {
        weight_limit: Unlimited,
        check_origin: None,
    }]);
    assert!(!barrier_result::<BarrierWithKnownResponse>(
        asset_hub_location(),
        unpaid
    ));
    assert!(!barrier_result::<BarrierWithKnownResponse>(
        Location::new(1, [Parachain(2000)]),
        paid_message(asset(usdc_location(), 1000))
    ));
}

#[test]
fn barrier_group_unneeded_instructions_remain_denied_even_with_weight_credit() {
    let mut instructions = vec![Trap(1)];
    let mut properties = Properties {
        weight_credit: MAX_WEIGHT,
        message_id: None,
    };
    assert!(BarrierWithKnownResponse::should_execute::<()>(
        &asset_hub_location(),
        &mut instructions,
        MAX_WEIGHT,
        &mut properties,
    )
    .is_err());
}

#[test]
fn barrier_group_origin_mutation_and_assertion_instructions_are_default_denied() {
    // Origin mutation and assertion opcodes are outside the v1 reserve-transfer surface
    // even when an accepted origin supplied weight credit (09 §6.1).
    for instruction in [
        Instruction::<()>::AliasOrigin(account_location([8; 32])),
        Instruction::<()>::UniversalOrigin(GlobalConsensus(NetworkId::Polkadot)),
        Instruction::<()>::ExpectAsset(Assets::from(asset(usdc_location(), 1))),
        Instruction::<()>::TransferAsset {
            assets: Assets::from(asset(usdc_location(), 1)),
            beneficiary: account_location([8; 32]),
        },
    ] {
        let mut instructions = vec![instruction];
        let mut properties = Properties {
            weight_credit: MAX_WEIGHT,
            message_id: None,
        };
        assert!(BarrierWithKnownResponse::should_execute(
            &asset_hub_location(),
            &mut instructions,
            MAX_WEIGHT,
            &mut properties,
        )
        .is_err());
    }
}

#[test]
fn barrier_group_paid_asset_hub_known_responses_and_version_negotiation_work() {
    assert!(barrier_result::<BarrierWithKnownResponse>(
        asset_hub_location(),
        paid_message(asset(usdc_location(), 1000))
    ));
    assert!(barrier_result::<BarrierWithKnownResponse>(
        asset_hub_location(),
        Xcm(vec![
            QueryResponse {
                query_id: 77,
                response: Response::ExecutionResult(None),
                max_weight: MAX_WEIGHT,
                querier: None,
            },
            // The stable2603 sender appends this topic to real ReportError responses.
            SetTopic([77; 32]),
        ])
    ));

    let subscribe = Xcm(vec![SubscribeVersion {
        query_id: 8,
        max_response_weight: MAX_WEIGHT,
    }]);
    assert!(barrier_result::<BarrierWithKnownResponse>(
        coretime_location(),
        subscribe.clone()
    ));
    assert!(!barrier_result::<BarrierWithKnownResponse>(
        Location::new(1, [Parachain(2000)]),
        subscribe
    ));
}

// -------------------------------------------------------------------------
// Trader (09 §6.1; G-1/PT-3 rounding direction)
// -------------------------------------------------------------------------

fn buy_with(asset: Asset, weight: Weight) -> Result<AssetsInHolding, XcmError> {
    let context = XcmContext::with_message_id([1; 32]);
    let payment = mock_asset_to_holding(asset);
    GovernedWeightTrader::<TestRates, ()>::new()
        .buy_weight(weight, payment, &context)
        .map_err(|(_, error)| error)
}

#[test]
fn trader_group_accepts_dot_and_usdc_but_refuses_every_other_asset() {
    assert!(buy_with(asset(usdc_location(), 1_000_000), Weight::from_parts(1, 0)).is_ok());
    assert!(buy_with(asset(dot_location(), 1_000_000), Weight::from_parts(1, 0)).is_ok());
    assert!(buy_with(
        asset(Location::new(1, [Parachain(2000)]), 1_000_000),
        Weight::from_parts(1, 0)
    )
    .is_err());
}

#[test]
fn trader_group_fractional_charge_rounds_up_against_the_payer() {
    let context = XcmContext::with_message_id([2; 32]);
    let mut trader = GovernedWeightTrader::<TestRates, ()>::new();
    let quote = trader.quote_weight(Weight::from_parts(1, 0), AssetId(usdc_location()), &context);
    assert!(quote.is_ok());
    let charged = quote.map(|value| value.fun).unwrap_or(Fungible(0));
    assert_eq!(charged, Fungible(1));
}

thread_local! {
    static TRADER_REVENUE: RefCell<Vec<Asset>> = const { RefCell::new(Vec::new()) };
}

struct RecordingRevenue;
impl TakeRevenue for RecordingRevenue {
    fn take_revenue(revenue: AssetsInHolding) {
        TRADER_REVENUE.with(|recorded| recorded.borrow_mut().extend(revenue.into_assets_iter()));
    }
}

fn holding_amount(holding: &AssetsInHolding, id: &Location) -> u128 {
    holding
        .fungible_assets_iter()
        .find_map(|asset| match asset {
            Asset {
                id: AssetId(location),
                fun: Fungible(amount),
            } if &location == id => Some(amount),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn trader_group_partial_refund_returns_surplus_and_drop_takes_only_remainder_as_revenue() {
    use frame_support::weights::constants::WEIGHT_REF_TIME_PER_SECOND;

    TRADER_REVENUE.with(|recorded| recorded.borrow_mut().clear());
    let context = XcmContext::with_message_id([21; 32]);
    let mut trader = GovernedWeightTrader::<TestRates, RecordingRevenue>::new();
    let bought = Weight::from_parts(WEIGHT_REF_TIME_PER_SECOND, 0);
    let payment = mock_asset_to_holding(asset(usdc_location(), 10));
    let surplus = trader
        .buy_weight(bought, payment, &context)
        .unwrap_or_else(|_| AssetsInHolding::new());
    assert_eq!(holding_amount(&surplus, &usdc_location()), 7);

    let refund = trader
        .refund_weight(
            Weight::from_parts(WEIGHT_REF_TIME_PER_SECOND / 2, 0),
            &context,
        )
        .unwrap_or_else(AssetsInHolding::new);
    // 1.5 USDC units floors to 1 against the payer.
    assert_eq!(holding_amount(&refund, &usdc_location()), 1);
    drop(trader);
    assert_eq!(
        TRADER_REVENUE.with(|recorded| recorded.borrow().clone()),
        vec![asset(usdc_location(), 2)]
    );
}

#[test]
fn trader_group_repeat_buys_accumulate_and_other_asset_cannot_change_the_refund_ledger() {
    use frame_support::weights::constants::WEIGHT_REF_TIME_PER_SECOND;

    TRADER_REVENUE.with(|recorded| recorded.borrow_mut().clear());
    let context = XcmContext::with_message_id([22; 32]);
    let mut trader = GovernedWeightTrader::<TestRates, RecordingRevenue>::new();
    let half = Weight::from_parts(WEIGHT_REF_TIME_PER_SECOND / 2, 0);
    let first = mock_asset_to_holding(asset(usdc_location(), 10));
    assert!(trader.buy_weight(half, first, &context).is_ok());
    let second = mock_asset_to_holding(asset(usdc_location(), 10));
    assert!(trader.buy_weight(half, second, &context).is_ok());

    assert!(trader
        .quote_weight(half, AssetId(dot_location()), &context)
        .is_err());
    let wrong_asset = mock_asset_to_holding(asset(dot_location(), 10));
    assert!(trader.buy_weight(half, wrong_asset, &context).is_err());

    let refund = trader
        .refund_weight(Weight::from_parts(WEIGHT_REF_TIME_PER_SECOND, 0), &context)
        .unwrap_or_else(AssetsInHolding::new);
    assert_eq!(holding_amount(&refund, &usdc_location()), 3);
    drop(trader);
    // Two half-second buys each rounded up to 2; refunding the accumulated
    // second floors to 3, leaving one unit as revenue.
    assert_eq!(
        TRADER_REVENUE.with(|recorded| recorded.borrow().clone()),
        vec![asset(usdc_location(), 1)]
    );
}

// -------------------------------------------------------------------------
// Probe response route + real oracle fail-static lifecycle (07 §8; I-24)
// -------------------------------------------------------------------------

thread_local! {
    static ROUTER_PENDING: RefCell<Option<u64>> = const { RefCell::new(None) };
    static ROUTER_RESULTS: RefCell<Vec<(u64, bool)>> = const { RefCell::new(Vec::new()) };
    static INNER_RESPONSES: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

struct RecordingProbeSink;
impl ProbeSink for RecordingProbeSink {
    fn pending_query_id() -> Option<u64> {
        ROUTER_PENDING.with(|pending| *pending.borrow())
    }

    fn probe_result(query_id: u64, passed: bool) {
        ROUTER_RESULTS.with(|results| results.borrow_mut().push((query_id, passed)));
    }
}

struct RecordingInner;
impl OnResponse for RecordingInner {
    fn expecting_response(_origin: &Location, query_id: u64, _querier: Option<&Location>) -> bool {
        query_id == 999
    }

    fn on_response(
        _origin: &Location,
        query_id: u64,
        _querier: Option<&Location>,
        _response: Response,
        _max_weight: Weight,
        _context: &XcmContext,
    ) -> Weight {
        INNER_RESPONSES.with(|responses| responses.borrow_mut().push(query_id));
        Weight::from_parts(9, 0)
    }
}

type RecordingProbeRouter = ProbeAwareResponseHandler<RecordingInner, RecordingProbeSink>;

fn route_probe(query_id: u64, response: Response) -> Weight {
    let querier = Location::here();
    RecordingProbeRouter::on_response(
        &asset_hub_location(),
        query_id | PROBE_QUERY_ID_FLAG,
        Some(&querier),
        response,
        MAX_WEIGHT,
        &XcmContext::with_message_id([3; 32]),
    )
}

#[test]
fn probe_group_program_is_golden_and_contains_no_transact_at_any_depth() {
    let amount = 10;
    let program = reserve_probe_program(42, amount, MAX_WEIGHT, MAX_WEIGHT, FIXTURE_PARA_ID);
    assert_eq!(program.0.len(), 3);
    assert!(
        matches!(&program.0[0], WithdrawAsset(assets) if assets == &vec![asset(crate::identity::usdc_on_asset_hub_location(), amount)].into())
    );
    assert!(
        matches!(&program.0[1], BuyExecution { fees, weight_limit: Limited(weight) } if fees == &asset(crate::identity::usdc_on_asset_hub_location(), amount) && weight == &MAX_WEIGHT)
    );
    assert!(
        matches!(&program.0[2], SetAppendix(Xcm(items)) if matches!(items.as_slice(), [
            SetFeesMode { jit_withdraw: true },
            RefundSurplus,
            DepositAsset { assets: AssetFilter::Wild(WildAsset::AllCounted(1)), beneficiary },
            ReportError(info),
        ] if beneficiary == &bleavit_as_seen_from_asset_hub(FIXTURE_PARA_ID)
            && info.query_id == (42 | PROBE_QUERY_ID_FLAG)
            && info.max_weight == MAX_WEIGHT
            && info.destination == bleavit_as_seen_from_asset_hub(FIXTURE_PARA_ID)))
    );
    assert!(!contains_transact(&program));
}

#[test]
fn probe_group_paid_report_uses_jit_so_nonzero_delivery_fee_is_sendable() {
    new_test_ext().execute_with(|| {
        let origin = asset_hub_location();
        let converted = TestLocationToAccountId::convert_location(&origin);
        assert!(converted.is_some());
        let sovereign = converted.unwrap_or_else(alice);
        assert_ok!(<ForeignAssets as Mutate<AccountId>>::mint_into(
            dot_location(),
            &sovereign,
            2,
        ));
        set_send_fee(Assets::from(asset(dot_location(), 1)));
        let info = QueryResponseInfo {
            destination: Location::new(1, [Parachain(FIXTURE_PARA_ID)]),
            query_id: 42,
            max_weight: MAX_WEIGHT,
        };
        let mut id = [11; 32];
        let outcome = BleavitXcmExecutor::prepare_and_execute(
            origin,
            Xcm(vec![SetAppendix(Xcm(vec![
                SetFeesMode { jit_withdraw: true },
                ReportError(info),
            ]))]),
            &mut id,
            MAX_WEIGHT,
            MAX_WEIGHT,
        );
        assert!(outcome.ensure_complete().is_ok());
        assert_eq!(sent_messages().len(), 1);
        assert_eq!(ForeignAssets::balance(dot_location(), sovereign), 1);
    });
}

#[test]
fn probe_group_response_router_maps_pass_fail_and_delegates_unknown_ids() {
    ROUTER_PENDING.with(|pending| *pending.borrow_mut() = Some(7));
    ROUTER_RESULTS.with(|results| results.borrow_mut().clear());
    INNER_RESPONSES.with(|responses| responses.borrow_mut().clear());

    route_probe(7, Response::ExecutionResult(None));
    assert_eq!(
        ROUTER_RESULTS.with(|results| results.borrow().clone()),
        vec![(7, true)]
    );

    route_probe(
        7,
        Response::ExecutionResult(Some((0, XcmError::Unimplemented))),
    );
    route_probe(7, Response::Version(5));
    assert_eq!(
        ROUTER_RESULTS.with(|results| results.borrow().clone()),
        vec![(7, true), (7, false), (7, false)]
    );

    let querier = Location::here();
    let delegated_weight = RecordingProbeRouter::on_response(
        &asset_hub_location(),
        999,
        Some(&querier),
        Response::ExecutionResult(None),
        MAX_WEIGHT,
        &XcmContext::with_message_id([31; 32]),
    );
    assert_eq!(delegated_weight, Weight::from_parts(9, 0));
    assert_eq!(
        INNER_RESPONSES.with(|responses| responses.borrow().clone()),
        vec![999]
    );

    // 07 §8: a matching number from any location other than Asset Hub is
    // not a probe response and must remain entirely under the inner handler.
    assert!(!RecordingProbeRouter::expecting_response(
        &coretime_location(),
        7,
        None,
    ));
    RecordingProbeRouter::on_response(
        &coretime_location(),
        7,
        None,
        Response::ExecutionResult(None),
        MAX_WEIGHT,
        &XcmContext::with_message_id([33; 32]),
    );
    assert_eq!(
        ROUTER_RESULTS.with(|results| results.borrow().clone()),
        vec![(7, true), (7, false), (7, false)]
    );
    assert_eq!(
        INNER_RESPONSES.with(|responses| responses.borrow().clone()),
        vec![999, 7]
    );
}

#[test]
fn probe_group_wrong_or_missing_querier_is_rejected_by_handler_and_barrier() {
    type RecordingProbeBarrier =
        crate::barrier::BleavitBarrier<RecordingProbeRouter, UniversalLocation, MaxPrefixes>;

    ROUTER_PENDING.with(|pending| *pending.borrow_mut() = Some(7));
    ROUTER_RESULTS.with(|results| results.borrow_mut().clear());
    INNER_RESPONSES.with(|responses| responses.borrow_mut().clear());
    let wire_id = 7 | PROBE_QUERY_ID_FLAG;
    let here = Location::here();
    let attacker = account_location([88; 32]);

    assert!(RecordingProbeRouter::expecting_response(
        &asset_hub_location(),
        wire_id,
        Some(&here),
    ));
    assert!(!RecordingProbeRouter::expecting_response(
        &asset_hub_location(),
        wire_id,
        None,
    ));
    assert!(!RecordingProbeRouter::expecting_response(
        &asset_hub_location(),
        wire_id,
        Some(&attacker),
    ));

    for querier in [None, Some(attacker.clone())] {
        RecordingProbeRouter::on_response(
            &asset_hub_location(),
            wire_id,
            querier.as_ref(),
            Response::ExecutionResult(None),
            MAX_WEIGHT,
            &XcmContext::with_message_id([34; 32]),
        );
    }
    assert!(ROUTER_RESULTS.with(|results| results.borrow().is_empty()));

    let response = |querier| {
        Xcm(vec![QueryResponse {
            query_id: wire_id,
            response: Response::ExecutionResult(None),
            max_weight: MAX_WEIGHT,
            querier,
        }])
    };
    assert!(barrier_result::<RecordingProbeBarrier>(
        asset_hub_location(),
        response(Some(here)),
    ));
    assert!(!barrier_result::<RecordingProbeBarrier>(
        asset_hub_location(),
        response(None),
    ));
    assert!(!barrier_result::<RecordingProbeBarrier>(
        asset_hub_location(),
        response(Some(attacker)),
    ));
}

#[test]
fn probe_group_partition_routes_flagged_and_unflagged_ids_to_disjoint_consumers() {
    ROUTER_PENDING.with(|pending| *pending.borrow_mut() = Some(999));
    ROUTER_RESULTS.with(|results| results.borrow_mut().clear());
    INNER_RESPONSES.with(|responses| responses.borrow_mut().clear());

    // The flagged wire id belongs exclusively to the oracle.
    assert_eq!(
        route_probe(999, Response::ExecutionResult(None)),
        MAX_WEIGHT
    );
    assert_eq!(
        ROUTER_RESULTS.with(|results| results.borrow().clone()),
        vec![(999, true)]
    );
    assert!(INNER_RESPONSES.with(|responses| responses.borrow().is_empty()));

    // The numerically-equal unflagged id remains untouched for pallet-xcm.
    let here = Location::here();
    assert_eq!(
        RecordingProbeRouter::on_response(
            &asset_hub_location(),
            999,
            Some(&here),
            Response::ExecutionResult(None),
            MAX_WEIGHT,
            &XcmContext::with_message_id([35; 32]),
        ),
        Weight::from_parts(9, 0)
    );
    assert_eq!(
        INNER_RESPONSES.with(|responses| responses.borrow().clone()),
        vec![999]
    );
}

#[test]
fn probe_group_real_oracle_pass_fail_and_timeout_remain_fail_static() {
    new_test_ext().execute_with(|| {
        System::set_block_number(RES_PROBE_INTERVAL.into());
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(alice())));
        assert_eq!(OracleProbeSink::pending_query_id(), Some(1));
        let program = reserve_probe_program(1, 10, MAX_WEIGHT, MAX_WEIGHT, FIXTURE_PARA_ID);
        assert_eq!(sent_messages(), vec![(asset_hub_location(), program)]);
        let here = Location::here();
        TestResponseHandler::on_response(
            &asset_hub_location(),
            1 | PROBE_QUERY_ID_FLAG,
            Some(&here),
            Response::ExecutionResult(None),
            MAX_WEIGHT,
            &XcmContext::with_message_id([4; 32]),
        );
        assert_eq!(
            pallet_oracle::ReserveHealth::<Test>::get().consecutive_passes,
            1
        );

        System::set_block_number((RES_PROBE_INTERVAL * 2).into());
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(alice())));
        TestResponseHandler::on_response(
            &asset_hub_location(),
            2 | PROBE_QUERY_ID_FLAG,
            Some(&here),
            Response::ExecutionResult(Some((0, XcmError::Unimplemented))),
            MAX_WEIGHT,
            &XcmContext::with_message_id([5; 32]),
        );
        assert_eq!(
            pallet_oracle::ReserveHealth::<Test>::get().consecutive_fails,
            1
        );

        System::set_block_number((RES_PROBE_INTERVAL * 3).into());
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(alice())));
        System::set_block_number((RES_PROBE_INTERVAL * 3 + RES_PROBE_TIMEOUT).into());
        TestResponseHandler::on_response(
            &asset_hub_location(),
            3 | PROBE_QUERY_ID_FLAG,
            Some(&here),
            Response::ExecutionResult(None),
            MAX_WEIGHT,
            &XcmContext::with_message_id([6; 32]),
        );
        let health = pallet_oracle::ReserveHealth::<Test>::get();
        assert_eq!(health.consecutive_fails, 2);
        assert_eq!(health.consecutive_passes, 0);
        assert!(health.unhealthy);
    });
}

#[test]
fn probe_group_dispatch_send_failure_is_swallowed_then_times_out_fail_static() {
    new_test_ext().execute_with(|| {
        set_send_mode(SendMode::ValidateFailure);
        System::set_block_number(RES_PROBE_INTERVAL.into());
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(alice())));
        assert_eq!(OracleProbeSink::pending_query_id(), Some(1));
        assert!(sent_messages().is_empty());

        // Before the next daily interval, the pending probe reaches its one-hour
        // timeout. The timeout-only crank records a failure and sends nothing.
        System::set_block_number((RES_PROBE_INTERVAL + RES_PROBE_TIMEOUT).into());
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(alice())));
        let health = pallet_oracle::ReserveHealth::<Test>::get();
        assert_eq!(health.consecutive_fails, 1);
        assert!(health.pending_since.is_none());
        assert!(sent_messages().is_empty());
    });
}

// -------------------------------------------------------------------------
// Coretime funding (09 §4)
// -------------------------------------------------------------------------

#[test]
fn coretime_group_program_routes_dot_via_parent_to_the_renewal_account_without_transact() {
    let renewal = [44; 32];
    let result =
        coretime_renewal_program(500, 100, renewal, Limited(MAX_WEIGHT), Limited(MAX_WEIGHT));
    assert!(result.is_ok());
    let program = result
        .map(|plan| plan.local_program)
        .unwrap_or_else(|_| Xcm(vec![]));
    assert!(!contains_transact(&program));
    assert!(matches!(
        program.0.as_slice(),
        [
            WithdrawAsset(assets),
            InitiateReserveWithdraw {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                reserve,
                xcm: Xcm(on_relay),
            },
        ] if assets.inner().contains(&asset(dot_location(), 600))
            && reserve == &relay_location()
            && matches!(
                on_relay.as_slice(),
                [
                    BuyExecution { fees, weight_limit: Limited(weight) },
                    InitiateTeleport {
                        assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                        dest,
                        xcm: Xcm(on_coretime),
                    },
                ] if fees == &asset(Location::here(), 50)
                    && weight == &MAX_WEIGHT
                    && dest == &Location::new(0, [Parachain(CORETIME_PARA_ID)])
                    && matches!(
                        on_coretime.as_slice(),
                        [
                            BuyExecution { fees, weight_limit: Limited(weight) },
                            DepositAsset {
                                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                                beneficiary,
                            },
                        ] if fees == &asset(dot_location(), 50)
                            && weight == &MAX_WEIGHT
                            && beneficiary == &account_location(renewal)
                    )
            )
    ));
    assert!(
        coretime_renewal_program(1, 1, renewal, Limited(MAX_WEIGHT), Limited(MAX_WEIGHT)).is_err()
    );
    assert!(coretime_renewal_program(
        u128::MAX,
        2,
        renewal,
        Limited(MAX_WEIGHT),
        Limited(MAX_WEIGHT),
    )
    .is_err());
}

#[test]
fn coretime_group_treasury_extrinsic_executes_local_program_and_sends_to_parent() {
    new_test_ext().execute_with(|| {
        assert_ok!(Treasury::fund_budget_line(
            RuntimeOrigin::signed(alice()),
            BudgetLine::OpsCoretime,
            1_000,
        ));
        assert_ok!(pallet_futarchy_treasury::Pallet::<Test>::note_coretime_renewal_quote(7, 500));
        assert_ok!(<ForeignAssets as Mutate<AccountId>>::mint_into(
            dot_location(),
            &alice(),
            600,
        ));
        let line_before = Treasury::line_balance(BudgetLine::OpsCoretime);
        assert_ok!(Treasury::execute_coretime_renewal(
            RuntimeOrigin::signed(bob()),
            7,
        ));
        assert_eq!(ForeignAssets::balance(dot_location(), alice()), 0);
        assert_eq!(
            Treasury::line_balance(BudgetLine::OpsCoretime),
            line_before - 500
        );
        assert!(Treasury::treasury().funded_coretime_periods.contains(&7));
        let messages = sent_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].0, Location::parent());
        assert!(matches!(messages[0].1 .0.first(), Some(WithdrawAsset(_))));
    });
}

#[test]
fn coretime_group_local_router_failure_rolls_back_custody_and_quote_for_retry() {
    new_test_ext().execute_with(|| {
        assert_ok!(Treasury::fund_budget_line(
            RuntimeOrigin::signed(alice()),
            BudgetLine::OpsCoretime,
            1_000,
        ));
        assert_ok!(pallet_futarchy_treasury::Pallet::<Test>::note_coretime_renewal_quote(8, 500));
        assert_ok!(<ForeignAssets as Mutate<AccountId>>::mint_into(
            dot_location(),
            &alice(),
            600,
        ));
        let line_before = Treasury::line_balance(BudgetLine::OpsCoretime);
        set_send_mode(SendMode::DeliverFailure);
        assert!(Treasury::execute_coretime_renewal(RuntimeOrigin::signed(bob()), 8).is_err());

        let state = Treasury::treasury();
        assert_eq!(Treasury::line_balance(BudgetLine::OpsCoretime), line_before);
        assert!(state.coretime_quotes.contains(&(8, 500)));
        assert!(!state.funded_coretime_periods.contains(&8));
        assert_eq!(ForeignAssets::balance(dot_location(), alice()), 600);
        assert!(sent_messages().is_empty());
    });
}

// -------------------------------------------------------------------------
// Inflow caps + real ForeignAssets adapter (09 §5.2)
// -------------------------------------------------------------------------

fn execute_inbound_usdc(amount: u128, beneficiary: [u8; 32], message_byte: u8) -> Outcome {
    let incoming = asset(usdc_location(), amount);
    let program: Xcm<()> = Xcm(vec![
        ReserveAssetDeposited(Assets::from(incoming.clone())),
        ClearOrigin,
        BuyExecution {
            fees: incoming,
            weight_limit: Limited(MAX_WEIGHT),
        },
        DepositAsset {
            assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
            beneficiary: account_location(beneficiary),
        },
    ]);
    let mut message_id = [message_byte; 32];
    BleavitXcmExecutor::prepare_and_execute(
        asset_hub_location(),
        program.into::<RuntimeCall>(),
        &mut message_id,
        MAX_WEIGHT,
        MAX_WEIGHT,
    )
}

fn trapped_assets_events() -> usize {
    System::events()
        .iter()
        .filter(|record| {
            matches!(
                record.event,
                RuntimeEvent::PalletXcm(pallet_xcm::Event::AssetsTrapped { .. })
            )
        })
        .count()
}

#[test]
fn caps_group_full_inbound_program_under_caps_credits_and_records_the_beneficiary() {
    new_test_ext().execute_with(|| {
        set_caps(100, 100);
        assert!(execute_inbound_usdc(100, ALICE_BYTES, 41)
            .ensure_complete()
            .is_ok());
        let credited = ForeignAssets::balance(usdc_location(), alice());
        assert!(credited > 0);
        assert_eq!(recorded_inflow(&alice()), credited);
        assert_eq!(trapped_assets_events(), 0);
    });
}

#[test]
fn caps_group_over_global_cap_fails_at_mint_with_zero_issuance_and_zero_trap() {
    // limit-coverage: phase3.tvl_cap
    new_test_ext().execute_with(|| {
        set_caps(99, u128::MAX);
        let issuance_before = ForeignAssets::total_supply(usdc_location());
        let outcome = execute_inbound_usdc(100, ALICE_BYTES, 42);
        assert!(matches!(
            outcome,
            Outcome::Incomplete {
                error: InstructionError {
                    index: 0,
                    error: XcmError::FailedToTransactAsset("USDC global inflow cap exceeded"),
                },
                ..
            }
        ));
        assert_eq!(
            ForeignAssets::total_supply(usdc_location()),
            issuance_before
        );
        assert_eq!(ForeignAssets::balance(usdc_location(), alice()), 0);
        assert_eq!(recorded_inflow(&alice()), 0);
        assert_eq!(trapped_assets_events(), 0);
    });
}

#[test]
fn caps_group_per_account_rejection_still_gates_deposit_and_traps_minted_holding() {
    // limit-coverage: phase3.dep_cap
    new_test_ext().execute_with(|| {
        set_caps(u128::MAX, 1);
        let outcome = execute_inbound_usdc(100, ALICE_BYTES, 43);
        assert!(matches!(
            outcome,
            Outcome::Incomplete {
                error: InstructionError {
                    index: 3,
                    error: XcmError::FailedToTransactAsset("USDC inflow cap exceeded"),
                },
                ..
            }
        ));
        assert_eq!(ForeignAssets::balance(usdc_location(), alice()), 0);
        assert_eq!(recorded_inflow(&alice()), 0);
        // The beneficiary is unknowable at mint time. This deposit-leg failure
        // therefore traps the already-minted holding for 09 §6.1 recovery.
        assert_eq!(trapped_assets_events(), 1);
    });
}

#[test]
fn caps_group_fee_only_usdc_program_is_still_subject_to_the_global_mint_gate() {
    new_test_ext().execute_with(|| {
        // This mock trader consumes both units (one for each weight dimension),
        // leaving no beneficiary deposit, but the mint leg still admits/counts it.
        set_caps(2, u128::MAX);
        assert!(execute_inbound_usdc(2, ALICE_BYTES, 44)
            .ensure_complete()
            .is_ok());
        assert_eq!(ForeignAssets::balance(usdc_location(), alice()), 0);
        assert_eq!(recorded_inflow(&alice()), 0);
    });

    new_test_ext().execute_with(|| {
        set_caps(1, u128::MAX);
        let outcome = execute_inbound_usdc(2, ALICE_BYTES, 45);
        assert!(matches!(
            outcome,
            Outcome::Incomplete {
                error: InstructionError { index: 0, .. },
                ..
            }
        ));
        assert_eq!(ForeignAssets::total_supply(usdc_location()), 0);
        assert_eq!(trapped_assets_events(), 0);
    });
}

// -------------------------------------------------------------------------
// Health tracking (09 §6.4; I-24)
// -------------------------------------------------------------------------

type ObservedRouter = HealthTrackingRouter<RecordingSender, TestHealthSink>;

fn validate_and_deliver<R: SendXcm>(
    destination: Location,
    message: Xcm<()>,
) -> Result<XcmHash, SendError> {
    let (ticket, _cost) = validate_send::<R>(destination, message)?;
    R::deliver(ticket)
}

#[test]
fn health_group_success_and_failures_increment_only_the_observation_counters() {
    new_test_ext().execute_with(|| {
        assert!(validate_and_deliver::<ObservedRouter>(asset_hub_location(), Xcm(vec![])).is_ok());
        assert_eq!(health_counts(), (1, 0));
        assert_eq!(sent_messages().len(), 1);

        set_send_mode(SendMode::ValidateFailure);
        assert!(validate_and_deliver::<ObservedRouter>(asset_hub_location(), Xcm(vec![])).is_err());
        assert_eq!(health_counts(), (1, 1));

        set_send_mode(SendMode::DeliverFailure);
        assert!(validate_and_deliver::<ObservedRouter>(asset_hub_location(), Xcm(vec![])).is_err());
        assert_eq!(health_counts(), (1, 2));
    });
}

#[test]
fn health_group_router_outcomes_are_identical_to_the_inner_sender() {
    new_test_ext().execute_with(|| {
        for mode in [
            SendMode::Success,
            SendMode::ValidateFailure,
            SendMode::DeliverFailure,
        ] {
            set_send_mode(mode);
            let inner = validate_and_deliver::<RecordingSender>(asset_hub_location(), Xcm(vec![]));
            reset_test_state();
            set_send_mode(mode);
            let wrapped = validate_and_deliver::<ObservedRouter>(asset_hub_location(), Xcm(vec![]));
            assert_eq!(inner, wrapped);
        }
    });
}

// -------------------------------------------------------------------------
// pallet_xcm SafetyFilter projection (09 §6.1/§6.2)
// -------------------------------------------------------------------------

#[test]
#[allow(deprecated)]
fn filter_group_every_stable2603_call_variant_has_the_conservative_disposition() {
    let location: VersionedLocation = Location::parent().into();
    let assets: VersionedAssets = Assets::new().into();
    let unit_message: VersionedXcm<()> = VersionedXcm::V5(Xcm(vec![]));
    let runtime_message: VersionedXcm<RuntimeCall> = VersionedXcm::V5(Xcm(vec![]));
    let asset_id: VersionedAssetId = AssetId(dot_location()).into();

    let denied = vec![
        pallet_xcm::Call::<Test>::send {
            dest: Box::new(location.clone()),
            message: Box::new(unit_message.clone()),
        },
        pallet_xcm::Call::<Test>::teleport_assets {
            dest: Box::new(location.clone()),
            beneficiary: Box::new(location.clone()),
            assets: Box::new(assets.clone()),
            fee_asset_item: 0,
        },
        pallet_xcm::Call::<Test>::reserve_transfer_assets {
            dest: Box::new(location.clone()),
            beneficiary: Box::new(location.clone()),
            assets: Box::new(assets.clone()),
            fee_asset_item: 0,
        },
        pallet_xcm::Call::<Test>::execute {
            message: Box::new(runtime_message),
            max_weight: MAX_WEIGHT,
        },
        pallet_xcm::Call::<Test>::force_xcm_version {
            location: Box::new(Location::parent()),
            version: 5,
        },
        pallet_xcm::Call::<Test>::force_default_xcm_version {
            maybe_xcm_version: Some(5),
        },
        pallet_xcm::Call::<Test>::force_subscribe_version_notify {
            location: Box::new(location.clone()),
        },
        pallet_xcm::Call::<Test>::force_unsubscribe_version_notify {
            location: Box::new(location.clone()),
        },
        pallet_xcm::Call::<Test>::limited_teleport_assets {
            dest: Box::new(location.clone()),
            beneficiary: Box::new(location.clone()),
            assets: Box::new(assets.clone()),
            fee_asset_item: 0,
            weight_limit: Limited(MAX_WEIGHT),
        },
        pallet_xcm::Call::<Test>::force_suspension { suspended: true },
        pallet_xcm::Call::<Test>::transfer_assets {
            dest: Box::new(location.clone()),
            beneficiary: Box::new(location.clone()),
            assets: Box::new(assets.clone()),
            fee_asset_item: 0,
            weight_limit: Limited(MAX_WEIGHT),
        },
        pallet_xcm::Call::<Test>::transfer_assets_using_type_and_then {
            dest: Box::new(location.clone()),
            assets: Box::new(assets.clone()),
            assets_transfer_type: Box::new(
                staging_xcm_executor::traits::TransferType::LocalReserve,
            ),
            remote_fees_id: Box::new(asset_id),
            fees_transfer_type: Box::new(staging_xcm_executor::traits::TransferType::LocalReserve),
            custom_xcm_on_dest: Box::new(unit_message),
            weight_limit: Limited(MAX_WEIGHT),
        },
        pallet_xcm::Call::<Test>::add_authorized_alias {
            aliaser: Box::new(location.clone()),
            expires: None,
        },
        pallet_xcm::Call::<Test>::remove_authorized_alias {
            aliaser: Box::new(location.clone()),
        },
        pallet_xcm::Call::<Test>::remove_all_authorized_aliases {},
    ];
    for call in denied {
        assert_eq!(
            classify_pallet_xcm_call(&call),
            XcmCallDisposition::DeniedAllOrigins
        );
    }

    let limited = pallet_xcm::Call::<Test>::limited_reserve_transfer_assets {
        dest: Box::new(asset_hub_location().into()),
        beneficiary: Box::new(location.clone()),
        assets: Box::new(assets.clone()),
        fee_asset_item: 0,
        weight_limit: Limited(MAX_WEIGHT),
    };
    assert_eq!(
        classify_pallet_xcm_call(&limited),
        XcmCallDisposition::SignedAllowed
    );

    let sibling = pallet_xcm::Call::<Test>::limited_reserve_transfer_assets {
        dest: Box::new(coretime_location().into()),
        beneficiary: Box::new(location.clone()),
        assets: Box::new(assets.clone()),
        fee_asset_item: 0,
        weight_limit: Limited(MAX_WEIGHT),
    };
    assert_eq!(
        classify_pallet_xcm_call(&sibling),
        XcmCallDisposition::DeniedAllOrigins
    );

    let stale_or_garbage_version = VersionedLocation::V3(staging_xcm::v3::MultiLocation::new(
        u8::MAX,
        staging_xcm::v3::Junctions::Here,
    ));
    let garbage = pallet_xcm::Call::<Test>::limited_reserve_transfer_assets {
        dest: Box::new(stale_or_garbage_version),
        beneficiary: Box::new(location.clone()),
        assets: Box::new(assets.clone()),
        fee_asset_item: 0,
        weight_limit: Limited(MAX_WEIGHT),
    };
    assert_eq!(
        classify_pallet_xcm_call(&garbage),
        XcmCallDisposition::DeniedAllOrigins
    );

    let claim = pallet_xcm::Call::<Test>::claim_assets {
        assets: Box::new(assets),
        beneficiary: Box::new(location),
    };
    assert_eq!(
        classify_pallet_xcm_call(&claim),
        XcmCallDisposition::SignedAllowed
    );
}

#[test]
fn composability_group_executor_satisfies_pallet_xcm_and_executor_bounds() {
    fn assert_executor<T>()
    where
        T: staging_xcm::latest::ExecuteXcm<RuntimeCall>
            + staging_xcm_executor::traits::XcmAssetTransfers
            + staging_xcm_executor::traits::FeeManager,
    {
    }
    assert_executor::<BleavitXcmExecutor>();

    let _: Option<
        CappedInflows<TestAssetTransactors, TestCaps, TestLocationToAccountId, AccountId>,
    > = None;
}
