use crate::{
    assets::{BleavitReserves, PinnedAssetMatcher},
    barrier::{AcceptedXcmOrigins, DenyTransact},
    caps::CappedInflows,
    coretime::coretime_renewal_program,
    filter::{classify_pallet_xcm_call, ReserveTransferFilter, XcmCallDisposition},
    health::HealthTrackingRouter,
    identity::{
        asset_hub_location, bleavit_as_seen_from_asset_hub, coretime_location, dot_location,
        relay_location, usdc_location, XCM_VERSION_PINNED,
    },
    mock::*,
    probe::{
        reserve_probe_program, ProbeAwareResponseHandler, ProbeAwareWeightBounds, ProbeSink,
        PROBE_QUERY_ID_FLAG,
    },
    trader::GovernedWeightTrader,
};
use frame_support::{
    assert_ok,
    traits::{fungibles::Mutate, Contains, ContainsPair, Get},
    weights::Weight,
};
use futarchy_primitives::chain_identity::{
    ASSET_HUB_PARA_ID, CORETIME_PARA_ID, FIXTURE_PARA_ID, USDC_ASSET_INDEX, USDC_PALLET_INSTANCE,
};
use oracle_core::{RES_PROBE_INTERVAL, RES_PROBE_TIMEOUT};
use pallet_futarchy_treasury::{BudgetLine, RenewalDispatch};
use pallet_oracle::ProbeDispatch as _;
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
        WeightBounds, WeightTrader,
    },
    AssetsInHolding,
};
use std::{
    cell::{Cell, RefCell},
    vec,
};

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

fn contains_jit<Call>(message: &Xcm<Call>) -> bool {
    message.0.iter().any(|instruction| match instruction {
        SetFeesMode { jit_withdraw: true } => true,
        SetAppendix(nested) | SetErrorHandler(nested) => contains_jit(nested),
        TransferReserveAsset { xcm, .. }
        | DepositReserveAsset { xcm, .. }
        | InitiateReserveWithdraw { xcm, .. }
        | InitiateTeleport { xcm, .. }
        | ExportMessage { xcm, .. }
        | InitiateTransfer {
            remote_xcm: xcm, ..
        } => contains_jit(xcm),
        ExecuteWithOrigin { xcm, .. } => contains_jit(xcm),
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
            // The stable2606 sender appends this topic to real ReportError responses.
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
    static ROUTER_PENDING_READS: Cell<u32> = const { Cell::new(0) };
    static ROUTER_RESULTS: RefCell<Vec<(u64, bool)>> = const { RefCell::new(Vec::new()) };
    static INNER_RESPONSES: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

struct RecordingProbeSink;
impl ProbeSink for RecordingProbeSink {
    fn pending_query_id() -> Option<u64> {
        ROUTER_PENDING_READS.with(|reads| reads.set(reads.get().saturating_add(1)));
        ROUTER_PENDING.with(|pending| *pending.borrow())
    }

    fn probe_result(query_id: u64, passed: bool) -> Weight {
        ROUTER_RESULTS.with(|results| results.borrow_mut().push((query_id, passed)));
        MAX_WEIGHT
    }
}

#[test]
fn probe_group_full_authenticated_route_performs_two_pending_id_reads() {
    type RecordingProbeBarrier = crate::barrier::BleavitBarrier<
        RecordingProbeRouter,
        UniversalLocation,
        MaxPrefixes,
        TestCaps,
        TestLocationToAccountId,
        AccountId,
    >;

    ROUTER_PENDING.with(|pending| *pending.borrow_mut() = Some(7));
    ROUTER_PENDING_READS.with(|reads| reads.set(0));
    let wire_id = 7 | PROBE_QUERY_ID_FLAG;
    let response = Xcm(vec![QueryResponse {
        query_id: wire_id,
        response: Response::ExecutionResult(None),
        max_weight: MAX_WEIGHT,
        querier: Some(Location::here()),
    }]);
    assert!(barrier_result::<RecordingProbeBarrier>(
        asset_hub_location(),
        response,
    ));
    assert_eq!(ROUTER_PENDING_READS.with(Cell::get), 1);

    route_probe(7, Response::ExecutionResult(None));
    assert_eq!(ROUTER_PENDING_READS.with(Cell::get), 2);
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
    let fee_amount = 100;
    let program = reserve_probe_program(
        42,
        amount,
        fee_amount,
        MAX_WEIGHT,
        MAX_WEIGHT,
        FIXTURE_PARA_ID,
    );
    assert_eq!(program.0.len(), 3);
    assert!(
        matches!(&program.0[0], WithdrawAsset(assets) if assets == &vec![
            asset(crate::identity::usdc_on_asset_hub_location(), amount),
            asset(crate::identity::dot_on_asset_hub_location(), fee_amount),
        ].into())
    );
    assert!(
        matches!(&program.0[1], BuyExecution { fees, weight_limit: Limited(weight) } if fees == &asset(crate::identity::dot_on_asset_hub_location(), fee_amount) && weight == &MAX_WEIGHT)
    );
    assert!(
        matches!(&program.0[2], SetAppendix(Xcm(items)) if matches!(items.as_slice(), [
            RefundSurplus,
            DepositAsset { assets: AssetFilter::Definite(probe), beneficiary },
            ReportError(info),
            DepositAsset { assets: AssetFilter::Wild(WildAsset::AllCounted(1)), beneficiary: refund_beneficiary },
        ] if beneficiary == &bleavit_as_seen_from_asset_hub(FIXTURE_PARA_ID)
            && probe == &Assets::from(asset(crate::identity::usdc_on_asset_hub_location(), amount))
            && refund_beneficiary == &bleavit_as_seen_from_asset_hub(FIXTURE_PARA_ID)
            && info.query_id == (42 | PROBE_QUERY_ID_FLAG)
            && info.max_weight == MAX_WEIGHT
            && info.destination == bleavit_as_seen_from_asset_hub(FIXTURE_PARA_ID)))
    );
    assert!(!contains_transact(&program));
}

#[test]
fn probe_group_response_delivery_is_bounded_by_holding_and_never_uses_jit() {
    new_test_ext().execute_with(|| {
        let origin = asset_hub_location();
        let converted = TestLocationToAccountId::convert_location(&origin);
        assert!(converted.is_some());
        let sovereign = converted.unwrap_or_else(alice);
        let probe_amount = 10;
        let fee_envelope = 100;
        assert_ok!(<ForeignAssets as Mutate<AccountId>>::mint_into(
            usdc_location(),
            &sovereign,
            probe_amount,
        ));
        assert_ok!(<ForeignAssets as Mutate<AccountId>>::mint_into(
            dot_location(),
            &sovereign,
            fee_envelope + 50,
        ));
        set_send_fee(Assets::from(asset(dot_location(), 1)));
        let info = QueryResponseInfo {
            destination: Location::new(1, [Parachain(FIXTURE_PARA_ID)]),
            query_id: 42,
            max_weight: MAX_WEIGHT,
        };
        let bounded_program = Xcm(vec![
            WithdrawAsset(
                vec![
                    asset(usdc_location(), probe_amount),
                    asset(dot_location(), fee_envelope),
                ]
                .into(),
            ),
            BuyExecution {
                fees: asset(dot_location(), fee_envelope),
                weight_limit: Limited(MAX_WEIGHT),
            },
            SetAppendix(Xcm(vec![
                RefundSurplus,
                DepositAsset {
                    assets: AssetFilter::Definite(Assets::from(asset(
                        usdc_location(),
                        probe_amount,
                    ))),
                    beneficiary: origin.clone(),
                },
                ReportError(info.clone()),
                DepositAsset {
                    assets: Wild(WildAsset::AllCounted(1)),
                    beneficiary: origin.clone(),
                },
            ])),
        ]);
        assert!(!contains_jit(&bounded_program));
        let mut id = [11; 32];
        let outcome = BleavitXcmExecutor::prepare_and_execute(
            origin.clone(),
            bounded_program,
            &mut id,
            MAX_WEIGHT,
            MAX_WEIGHT,
        );
        assert!(outcome.ensure_complete().is_ok());
        assert_eq!(sent_messages().len(), 1);
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &sovereign),
            probe_amount
        );
        let remaining = ForeignAssets::balance(dot_location(), &sovereign);
        assert!(remaining >= 50);
        assert!(remaining < fee_envelope + 50);

        // A response fee above the entire envelope cannot draw the sovereign's
        // extra DOT: no response is emitted even though 50 planck remains
        // outside holding. This is the mutation-killing no-JIT assertion.
        reset_test_state();
        set_send_fee(Assets::from(asset(dot_location(), fee_envelope + 1)));
        let mut id = [12; 32];
        let outcome = BleavitXcmExecutor::prepare_and_execute(
            origin,
            Xcm(vec![
                WithdrawAsset(
                    vec![
                        asset(usdc_location(), probe_amount),
                        asset(dot_location(), fee_envelope),
                    ]
                    .into(),
                ),
                BuyExecution {
                    fees: asset(dot_location(), fee_envelope),
                    weight_limit: Limited(MAX_WEIGHT),
                },
                SetAppendix(Xcm(vec![RefundSurplus, ReportError(info)])),
            ]),
            &mut id,
            MAX_WEIGHT,
            MAX_WEIGHT,
        );
        assert!(outcome.ensure_complete().is_err());
        assert!(sent_messages().is_empty());
        assert_eq!(
            ForeignAssets::balance(dot_location(), sovereign),
            remaining.saturating_sub(fee_envelope)
        );
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
fn probe_group_response_return_weight_clamps_to_the_instruction_maximum() {
    ROUTER_PENDING.with(|pending| *pending.borrow_mut() = Some(7));
    ROUTER_RESULTS.with(|results| results.borrow_mut().clear());
    let querier = Location::here();
    let maximum = Weight::from_parts(7, 3);

    let used = RecordingProbeRouter::on_response(
        &asset_hub_location(),
        7 | PROBE_QUERY_ID_FLAG,
        Some(&querier),
        Response::ExecutionResult(None),
        maximum,
        &XcmContext::with_message_id([32; 32]),
    );

    assert_eq!(used, maximum);
    assert_eq!(
        ROUTER_RESULTS.with(|results| results.borrow().clone()),
        vec![(7, true)]
    );
}

#[test]
fn probe_group_wrong_or_missing_querier_is_rejected_by_handler_and_barrier() {
    type RecordingProbeBarrier = crate::barrier::BleavitBarrier<
        RecordingProbeRouter,
        UniversalLocation,
        MaxPrefixes,
        TestCaps,
        TestLocationToAccountId,
        AccountId,
    >;

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
fn probe_group_flagged_query_response_is_weighed_at_or_above_callback_bound() {
    let callback = ProbeMaxResponseWeight::get();
    let mut message = Xcm(vec![QueryResponse {
        query_id: 7 | PROBE_QUERY_ID_FLAG,
        response: Response::ExecutionResult(None),
        max_weight: callback,
        querier: Some(Location::here()),
    }]);
    assert_eq!(
        TestWeigher::weight(&mut message, Weight::MAX),
        Ok(UnitWeightCost::get().saturating_add(callback))
    );
}

fn flagged_response() -> Instruction<RuntimeCall> {
    QueryResponse {
        query_id: 7 | PROBE_QUERY_ID_FLAG,
        response: Response::ExecutionResult(None),
        max_weight: ProbeMaxResponseWeight::get(),
        querier: Some(Location::here()),
    }
}

#[test]
fn probe_group_callback_surcharge_never_clamps_to_an_insufficient_limit() {
    let mut message = Xcm(vec![flagged_response()]);
    let limit = Weight::from_parts(99, 99);
    assert!(matches!(
        TestWeigher::weight(&mut message, limit),
        Err(InstructionError {
            index: 0,
            error: XcmError::WeightLimitReached(weight),
        }) if weight == Weight::from_parts(101, 101)
    ));
}

#[test]
fn probe_group_callback_surcharge_follows_locally_executed_nesting() {
    let mut one_level = Xcm(vec![SetAppendix(Xcm(vec![flagged_response()]))]);
    assert_eq!(
        TestWeigher::weight(&mut one_level, Weight::MAX),
        Ok(Weight::from_parts(102, 102)),
    );

    let mut two_levels = Xcm(vec![SetErrorHandler(Xcm(vec![SetAppendix(Xcm(vec![
        flagged_response(),
    ]))]))]);
    assert_eq!(
        TestWeigher::weight(&mut two_levels, Weight::MAX),
        Ok(Weight::from_parts(103, 103)),
    );
}

struct OverflowProbeWeight;
impl Get<Weight> for OverflowProbeWeight {
    fn get() -> Weight {
        Weight::MAX
    }
}
type OverflowProbeWeigher = ProbeAwareWeightBounds<TestBaseWeigher, OverflowProbeWeight>;

#[test]
fn probe_group_callback_surcharge_rejects_weight_overflow() {
    let mut message = Xcm(vec![flagged_response(), flagged_response()]);
    assert!(matches!(
        OverflowProbeWeigher::weight(&mut message, Weight::MAX),
        Err(InstructionError {
            index: 0,
            error: XcmError::Overflow,
        })
    ));
}

#[test]
fn probe_group_real_oracle_pass_fail_and_timeout_remain_fail_static() {
    new_test_ext().execute_with(|| {
        System::set_block_number(RES_PROBE_INTERVAL.into());
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(alice())));
        assert_eq!(OracleProbeSink::pending_query_id(), Some(1));
        let program = reserve_probe_program(
            1,
            10,
            ProbeFeeBudget::get(),
            MAX_WEIGHT,
            MAX_WEIGHT,
            FIXTURE_PARA_ID,
        );
        let sent = sent_messages();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, asset_hub_location());
        assert_eq!(&sent[0].1 .0[..program.0.len()], program.0.as_slice());
        assert!(matches!(sent[0].1 .0.last(), Some(SetTopic(_))));
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

fn reserve_probe_fee_event_count() -> usize {
    System::events()
        .iter()
        .filter(|record| {
            matches!(
                record.event,
                RuntimeEvent::Treasury(pallet_futarchy_treasury::Event::ReserveProbeFeeCharged {
                    line: BudgetLine::OpsReserveProbe,
                    ..
                })
            )
        })
        .count()
}

#[test]
fn probe_group_success_commits_exact_budget_debit_event_queue_and_health_count() {
    new_test_ext().execute_with(|| {
        let before = Treasury::line_balance(BudgetLine::OpsReserveProbe);
        System::reset_events();
        System::set_block_number(RES_PROBE_INTERVAL.into());

        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(alice())));

        assert_eq!(
            before - Treasury::line_balance(BudgetLine::OpsReserveProbe),
            ProbeFeeBudget::get()
        );
        assert_eq!(reserve_probe_fee_event_count(), 1);
        assert_eq!(health_counts(), (1, 0));
        assert_eq!(sent_messages().len(), 1);
    });
}

#[test]
fn probe_group_budget_refusal_never_reaches_router_or_counts_xcm_failure() {
    new_test_ext().execute_with(|| {
        set_probe_line_balance(0);
        System::reset_events();
        System::set_block_number(RES_PROBE_INTERVAL.into());

        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(alice())));

        assert_eq!(Treasury::line_balance(BudgetLine::OpsReserveProbe), 0);
        assert_eq!(reserve_probe_fee_event_count(), 0);
        assert_eq!(health_counts(), (0, 0));
        assert!(sent_messages().is_empty());
        assert_eq!(OracleProbeSink::pending_query_id(), Some(1));
    });
}

#[test]
fn probe_group_zero_amount_refuses_before_budget_or_router() {
    new_test_ext().execute_with(|| {
        let before = Treasury::line_balance(BudgetLine::OpsReserveProbe);
        let treasury_before = pallet_futarchy_treasury::State::<Test>::get().encode();
        System::reset_events();

        TestProbeDispatcher::probe_due(1, 0);

        assert_eq!(Treasury::line_balance(BudgetLine::OpsReserveProbe), before);
        assert_eq!(
            pallet_futarchy_treasury::State::<Test>::get().encode(),
            treasury_before,
        );
        assert_eq!(reserve_probe_fee_event_count(), 0);
        assert_eq!(health_counts(), (0, 0));
        assert!(sent_messages().is_empty());
    });
}

#[test]
fn probe_group_validate_and_delivery_failures_rollback_budget_but_count_once() {
    for mode in [SendMode::ValidateFailure, SendMode::DeliverFailure] {
        new_test_ext().execute_with(|| {
            let before = Treasury::line_balance(BudgetLine::OpsReserveProbe);
            let treasury_before = pallet_futarchy_treasury::State::<Test>::get().encode();
            System::reset_events();
            set_send_mode(mode);
            System::set_block_number(RES_PROBE_INTERVAL.into());

            assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(alice())));

            assert_eq!(Treasury::line_balance(BudgetLine::OpsReserveProbe), before);
            assert_eq!(
                pallet_futarchy_treasury::State::<Test>::get().encode(),
                treasury_before,
            );
            assert_eq!(reserve_probe_fee_event_count(), 0);
            assert_eq!(health_counts(), (0, 1));
            assert!(sent_messages().is_empty());
            assert!(!partial_send_marker_exists());
            assert_eq!(OracleProbeSink::pending_query_id(), Some(1));
        });
    }
}

#[test]
fn probe_group_dispatch_send_failure_is_swallowed_then_times_out_fail_static() {
    new_test_ext().execute_with(|| {
        set_send_mode(SendMode::ValidateFailure);
        System::set_block_number(RES_PROBE_INTERVAL.into());
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(alice())));
        assert_eq!(OracleProbeSink::pending_query_id(), Some(1));
        assert!(sent_messages().is_empty());
        assert_eq!(health_counts(), (0, 1));

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
        assert_ok!(Treasury::note_coretime_quote(
            RuntimeOrigin::signed(alice()),
            7,
            500,
        ));
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
            line_before - 600
        );
        assert!(Treasury::treasury().funded_coretime_periods.contains(&7));
        let messages = sent_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].0, Location::parent());
        assert!(matches!(messages[0].1 .0.first(), Some(WithdrawAsset(_))));
    });
}

#[test]
fn coretime_group_unset_renewal_account_fails_before_xcm_state_changes() {
    new_test_ext().execute_with(|| {
        assert_ok!(<ForeignAssets as Mutate<AccountId>>::mint_into(
            dot_location(),
            &alice(),
            600,
        ));
        let before = ForeignAssets::balance(dot_location(), alice());
        RenewalAccount::set(None);

        assert_eq!(
            TestRenewalDispatcher::dispatch_renewal(7, 500),
            Err(sp_runtime::DispatchError::Other(
                "coretime renewal account is not configured",
            )),
        );
        assert_eq!(ForeignAssets::balance(dot_location(), alice()), before);
        assert!(sent_messages().is_empty());
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
        assert_ok!(Treasury::note_coretime_quote(
            RuntimeOrigin::signed(alice()),
            8,
            500,
        ));
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
        assert!(state
            .coretime_quotes
            .iter()
            .any(|quote| quote.period_index == 8 && quote.price == 500));
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
fn caps_group_over_account_cap_is_refused_before_any_mint_and_never_traps() {
    // 09 §5.2 (normative, SQ-129 resolution 2026-07-20): BOTH caps are enforced
    // before any local mint, and a cap refusal leaves nothing minted and nothing
    // trapped locally. An inbound trap is keyed under the *sending* chain, so the
    // beneficiary could never self-claim it — trapping here would strand the funds
    // permanently (09 §6.1 trapped-assets row).
    new_test_ext().execute_with(|| {
        set_caps(u128::MAX, 1);
        let issuance_before = ForeignAssets::total_supply(usdc_location());
        let outcome = execute_inbound_usdc(100, ALICE_BYTES, 43);
        assert!(
            matches!(
                outcome,
                Outcome::Incomplete {
                    error: InstructionError {
                        error: XcmError::Barrier,
                        ..
                    },
                    ..
                }
            ),
            "expected a barrier refusal before mint, got {outcome:?}"
        );
        assert_eq!(
            ForeignAssets::total_supply(usdc_location()),
            issuance_before,
            "nothing may be minted on a per-account cap refusal"
        );
        assert_eq!(ForeignAssets::balance(usdc_location(), alice()), 0);
        assert_eq!(recorded_inflow(&alice()), 0);
        assert_eq!(
            trapped_assets_events(),
            0,
            "a cap refusal must never produce a remote-keyed trap"
        );
    });
}

#[test]
fn caps_group_account_cap_admits_exactly_at_the_cap() {
    // The pre-mint read is a bound check, not an off-by-one refusal: a program
    // minting exactly the remaining per-account headroom stays admissible.
    new_test_ext().execute_with(|| {
        set_caps(u128::MAX, 100);
        assert!(execute_inbound_usdc(100, ALICE_BYTES, 46)
            .ensure_complete()
            .is_ok());
        assert!(recorded_inflow(&alice()) > 0);
        assert_eq!(trapped_assets_events(), 0);
    });
}

#[test]
fn caps_group_pre_mint_read_accumulates_across_messages() {
    // The pre-mint check reads the same cumulative meter the deposit leg writes,
    // so a second message that would breach the cap is refused before minting.
    new_test_ext().execute_with(|| {
        set_caps(u128::MAX, 150);
        assert!(execute_inbound_usdc(100, ALICE_BYTES, 47)
            .ensure_complete()
            .is_ok());
        let recorded = recorded_inflow(&alice());
        assert!(recorded > 0);
        let issuance_before = ForeignAssets::total_supply(usdc_location());

        let outcome = execute_inbound_usdc(100, ALICE_BYTES, 48);
        assert!(
            matches!(
                outcome,
                Outcome::Incomplete {
                    error: InstructionError {
                        error: XcmError::Barrier,
                        ..
                    },
                    ..
                }
            ),
            "expected the second message to be refused pre-mint, got {outcome:?}"
        );
        assert_eq!(
            ForeignAssets::total_supply(usdc_location()),
            issuance_before
        );
        assert_eq!(recorded_inflow(&alice()), recorded);
        assert_eq!(trapped_assets_events(), 0);
    });
}

#[test]
fn caps_group_unconvertible_beneficiary_fails_closed_before_mint() {
    // Beneficiary conversion failure must fail closed (G-1): an inbound USDC mint
    // whose deposit leg names a location this chain cannot resolve to a local
    // account is refused rather than admitted unmetered.
    new_test_ext().execute_with(|| {
        set_caps(u128::MAX, 1);
        let incoming = asset(usdc_location(), 100);
        let program: Xcm<()> = Xcm(vec![
            ReserveAssetDeposited(Assets::from(incoming.clone())),
            ClearOrigin,
            BuyExecution {
                fees: incoming,
                weight_limit: Limited(MAX_WEIGHT),
            },
            DepositAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                // A parent (relay) location has no local AccountId conversion.
                beneficiary: Location::parent(),
            },
        ]);
        let mut message_id = [49_u8; 32];
        let outcome = BleavitXcmExecutor::prepare_and_execute(
            asset_hub_location(),
            program.into::<RuntimeCall>(),
            &mut message_id,
            MAX_WEIGHT,
            MAX_WEIGHT,
        );
        assert!(
            matches!(
                outcome,
                Outcome::Incomplete {
                    error: InstructionError {
                        error: XcmError::Barrier,
                        ..
                    },
                    ..
                }
            ),
            "unconvertible beneficiary must fail closed, got {outcome:?}"
        );
        assert_eq!(ForeignAssets::total_supply(usdc_location()), 0);
        assert_eq!(trapped_assets_events(), 0);
    });
}

#[test]
fn caps_group_local_claim_recovery_is_exempt_from_the_pre_mint_gate() {
    // 09 §5.2 mint-step scope (normative, SQ-253): pallet-xcm's trapped-imbalance
    // reconstruction is exempt from the prospective cap. The pre-mint gate keys on
    // `ReserveAssetDeposited` (the issuance-increasing mint) and must not fire on a
    // `ClaimAsset` recovery program, whose deposit leg stays metered instead.
    new_test_ext().execute_with(|| {
        set_caps(u128::MAX, 1);
        let claimed = asset(usdc_location(), 100);
        let program: Xcm<()> = Xcm(vec![
            ClaimAsset {
                assets: Assets::from(claimed),
                ticket: Location::here(),
            },
            DepositAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                beneficiary: account_location(ALICE_BYTES),
            },
        ]);
        let mut instructions = program.into::<RuntimeCall>().0;
        let mut properties = Properties {
            weight_credit: MAX_WEIGHT,
            message_id: None,
        };
        // The barrier's deny tuple must not reject the recovery program itself;
        // the per-account cap binds it at the deposit leg, not before the mint.
        assert!(
            <TestBarrier as ShouldExecute>::should_execute(
                &account_location(ALICE_BYTES),
                &mut instructions,
                MAX_WEIGHT,
                &mut properties,
            )
            .is_ok(),
            "ClaimAsset recovery must not be refused by the pre-mint inflow gate"
        );
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
// Pre-mint gate meters the post-fee credited amount (09 §5.2; SQ-481)
// -------------------------------------------------------------------------

type TestInflowGate =
    crate::barrier::DenyOverCapInflows<TestCaps, TestLocationToAccountId, AccountId>;

/// Run only the pre-mint per-account inflow gate over `program`; `true` == admitted.
fn inflow_gate_admits(program: Xcm<()>) -> bool {
    let mut instructions = program.0;
    let mut properties = Properties {
        weight_credit: Weight::zero(),
        message_id: None,
    };
    <TestInflowGate as DenyExecution>::deny_execution::<()>(
        &asset_hub_location(),
        &mut instructions,
        MAX_WEIGHT,
        &mut properties,
    )
    .is_ok()
}

#[test]
fn caps_group_pre_mint_gate_meters_the_post_fee_amount_for_usdc_payfees() {
    // A program that pays its execution fee in USDC via `PayFees` deposits only the
    // holding that survives the fee — exactly what `CappedInflows::deposit_asset`
    // meters. The pre-mint gate must bound the beneficiary against that post-fee
    // amount (100 − 10 = 90), not the whole pre-fee holding (100): a user with 90
    // remaining headroom is entitled to receive the 90 they will actually be
    // credited (SQ-481). Before the fix this program was wrongly refused (bound 100).
    let payfees_inbound = || {
        Xcm(vec![
            ReserveAssetDeposited(Assets::from(asset(usdc_location(), 100))),
            ClearOrigin,
            PayFees {
                asset: asset(usdc_location(), 10),
            },
            DepositAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                beneficiary: account_location(ALICE_BYTES),
            },
        ])
    };
    new_test_ext().execute_with(|| {
        // Remaining headroom exactly equals the post-fee credited amount.
        set_caps(u128::MAX, 90);
        assert!(
            inflow_gate_admits(payfees_inbound()),
            "post-fee deposit (90) fits the 90 headroom and must be admitted"
        );
    });
    new_test_ext().execute_with(|| {
        // One unit less headroom than the post-fee amount: the metered deposit (90)
        // would breach the cap, so the gate must still refuse. This pins the bound at
        // exactly the post-fee 90 — never looser — so it can never admit an over-cap
        // deposit (R-7).
        set_caps(u128::MAX, 89);
        assert!(
            !inflow_gate_admits(payfees_inbound()),
            "a metered deposit of 90 over an 89 cap must be refused"
        );
    });
}

#[test]
fn caps_group_pre_mint_gate_admits_a_fee_only_usdc_program() {
    // A program whose USDC is entirely consumed by the fee credits zero to its
    // beneficiary, so its real `deposit_asset` meter never moves. It must not be
    // refused for the pre-fee holding it never deposits (SQ-481).
    new_test_ext().execute_with(|| {
        set_caps(u128::MAX, 1);
        let fee_only = Xcm(vec![
            WithdrawAsset(Assets::from(asset(usdc_location(), 100))),
            PayFees {
                asset: asset(usdc_location(), 100),
            },
            DepositAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                beneficiary: account_location(ALICE_BYTES),
            },
        ]);
        assert!(
            inflow_gate_admits(fee_only),
            "a fee-only program credits zero and must not be rejected"
        );
    });
}

#[test]
fn caps_group_pre_mint_gate_still_refuses_an_over_cap_post_fee_deposit() {
    // The fee subtraction must not loosen the gate into admitting an over-cap
    // deposit: with 85 headroom, the metered post-fee deposit of 90 still breaches
    // the cap and must be refused (R-7).
    new_test_ext().execute_with(|| {
        set_caps(u128::MAX, 85);
        let program = Xcm(vec![
            ReserveAssetDeposited(Assets::from(asset(usdc_location(), 100))),
            PayFees {
                asset: asset(usdc_location(), 10),
            },
            DepositAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                beneficiary: account_location(ALICE_BYTES),
            },
        ]);
        assert!(
            !inflow_gate_admits(program),
            "a 90 post-fee deposit over an 85 cap must be refused"
        );
    });
}

#[test]
fn caps_group_pre_mint_gate_ignores_a_pay_fees_ordered_after_a_deposit() {
    // A `PayFees` that follows the `DepositAsset` removes nothing before it: the
    // deposit meters the full pre-fee holding (100). The gate must therefore *not*
    // subtract the fee, and must refuse a 100 deposit over a 90 cap — otherwise a
    // reordered program would evade the cap (R-7).
    new_test_ext().execute_with(|| {
        set_caps(u128::MAX, 90);
        let reordered = Xcm(vec![
            ReserveAssetDeposited(Assets::from(asset(usdc_location(), 100))),
            DepositAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                beneficiary: account_location(ALICE_BYTES),
            },
            PayFees {
                asset: asset(usdc_location(), 10),
            },
        ]);
        assert!(
            !inflow_gate_admits(reordered),
            "a deposit before the fee meters the full holding and must be refused"
        );
    });
}

#[test]
fn caps_group_pre_mint_gate_ignores_a_refundable_pay_fees() {
    // `RefundSurplus` can merge the unspent `PayFees` back into holding for a later
    // deposit, so its presence forbids any subtraction: the deposit could still move
    // the full holding (100), which must be refused over a 90 cap (R-7).
    new_test_ext().execute_with(|| {
        set_caps(u128::MAX, 90);
        let refunded = Xcm(vec![
            ReserveAssetDeposited(Assets::from(asset(usdc_location(), 100))),
            PayFees {
                asset: asset(usdc_location(), 10),
            },
            RefundSurplus,
            DepositAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                beneficiary: account_location(ALICE_BYTES),
            },
        ]);
        assert!(
            !inflow_gate_admits(refunded),
            "a refundable fee must not be subtracted from the deposit bound"
        );
    });
}

#[test]
fn caps_group_pre_mint_gate_ignores_fee_when_error_handler_can_deposit_full_holding() {
    // A failing `PayFees` transfers nothing to the fee register and immediately runs
    // the installed error handler. If that handler contains a local deposit, it can
    // therefore meter the full pre-fee holding. The pre-mint gate must not subtract
    // the nominal fee and admit a mint that can only fail later at the deposit leg.
    new_test_ext().execute_with(|| {
        set_caps(u128::MAX, 90);
        let incoming = asset(usdc_location(), 100);
        let program: Xcm<()> = Xcm(vec![
            ReserveAssetDeposited(Assets::from(incoming)),
            ClearOrigin,
            SetErrorHandler(Xcm(vec![DepositAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                beneficiary: account_location(ALICE_BYTES),
            }])),
            PayFees {
                // More than holding: this instruction fails without removing any
                // USDC, then the error handler attempts to deposit all 100.
                asset: asset(usdc_location(), 110),
            },
        ]);
        let outcome = execute_inbound_program(program, 64);
        assert!(
            matches!(
                outcome,
                Outcome::Incomplete {
                    error: InstructionError {
                        error: XcmError::Barrier,
                        ..
                    },
                    ..
                }
            ),
            "a fallback deposit must be bounded against the full holding: {outcome:?}"
        );
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
fn filter_group_every_stable2606_call_variant_has_the_conservative_disposition() {
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
fn reserve_transfer_filter_accepts_only_local_signed_dot_or_usdc() {
    let signed = Location::new(
        0,
        [Junction::AccountId32 {
            network: None,
            id: [7; 32],
        }],
    );
    let allowed = vec![asset(dot_location(), 1), asset(usdc_location(), 2)];
    assert!(ReserveTransferFilter::contains(&(signed.clone(), allowed,)));
    assert!(!ReserveTransferFilter::contains(&(
        Location::new(1, [Junction::Parachain(CORETIME_PARA_ID)]),
        vec![asset(dot_location(), 1)],
    )));
    assert!(!ReserveTransferFilter::contains(&(
        signed,
        vec![asset(Location::new(1, [Junction::Parachain(9_999)]), 1)],
    )));
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

/// Run an arbitrary inbound program from Asset Hub through the production barrier
/// + executor, so barrier evasions are visible as mints/traps rather than opinions.
fn execute_inbound_program(program: Xcm<()>, message_byte: u8) -> Outcome {
    let mut message_id = [message_byte; 32];
    BleavitXcmExecutor::prepare_and_execute(
        asset_hub_location(),
        program.into::<RuntimeCall>(),
        &mut message_id,
        MAX_WEIGHT,
        MAX_WEIGHT,
    )
}

#[test]
fn caps_group_withdraw_fed_holding_cannot_evade_the_pre_mint_gate() {
    // Adversarial: `WithdrawAsset` is an admitted instruction that fills holding
    // from the sender's local sovereign account with **no** `ReserveAssetDeposited`.
    // If the gate keys only on the mint, an over-cap deposit slips past it and the
    // deposit leg strands the holding in an Asset-Hub-keyed trap — exactly what
    // 09 §6.1 forbids ("no cap refusal can produce a remote-keyed trap at all").
    new_test_ext().execute_with(|| {
        let converted = TestLocationToAccountId::convert_location(&asset_hub_location());
        assert!(
            converted.is_some(),
            "Asset Hub must have a local sovereign account"
        );
        let sovereign = converted.unwrap_or(AccountId::new([0_u8; 32]));
        assert_ok!(<ForeignAssets as Mutate<AccountId>>::mint_into(
            usdc_location(),
            &sovereign,
            1_000,
        ));
        set_caps(u128::MAX, 1);
        let taken = asset(usdc_location(), 100);
        let program: Xcm<()> = Xcm(vec![
            WithdrawAsset(Assets::from(taken.clone())),
            BuyExecution {
                fees: taken,
                weight_limit: Limited(MAX_WEIGHT),
            },
            DepositAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                beneficiary: account_location(ALICE_BYTES),
            },
        ]);
        let outcome = execute_inbound_program(program, 60);
        assert!(
            matches!(
                outcome,
                Outcome::Incomplete {
                    error: InstructionError {
                        error: XcmError::Barrier,
                        ..
                    },
                    ..
                }
            ),
            "withdraw-fed over-cap deposit must be refused pre-execution: {outcome:?}"
        );
        assert_eq!(recorded_inflow(&alice()), 0);
        assert_eq!(
            trapped_assets_events(),
            0,
            "a cap refusal must never produce a remote-keyed trap"
        );
    });
}

#[test]
fn caps_group_deposit_reserve_asset_is_a_metered_local_deposit_leg() {
    // Adversarial: `DepositReserveAsset` deposits into `dest`'s local sovereign
    // account before sending onward, so it is a second metered deposit leg. With no
    // `DepositAsset` present the beneficiary list is empty and a naive `all()` is
    // vacuously true, letting the mint execute and the refusal strand the holding.
    new_test_ext().execute_with(|| {
        set_caps(u128::MAX, 1);
        let incoming = asset(usdc_location(), 100);
        let program: Xcm<()> = Xcm(vec![
            ReserveAssetDeposited(Assets::from(incoming.clone())),
            ClearOrigin,
            BuyExecution {
                fees: incoming,
                weight_limit: Limited(MAX_WEIGHT),
            },
            DepositReserveAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                dest: relay_location(),
                xcm: Xcm(vec![]),
            },
        ]);
        let issuance_before = ForeignAssets::total_supply(usdc_location());
        let outcome = execute_inbound_program(program, 61);
        assert!(
            matches!(
                outcome,
                Outcome::Incomplete {
                    error: InstructionError {
                        error: XcmError::Barrier,
                        ..
                    },
                    ..
                }
            ),
            "an over-cap DepositReserveAsset leg must be refused pre-mint: {outcome:?}"
        );
        assert_eq!(
            ForeignAssets::total_supply(usdc_location()),
            issuance_before,
            "nothing may be minted on a per-account cap refusal"
        );
        assert_eq!(trapped_assets_events(), 0);
    });
}

#[test]
fn caps_group_mixed_mint_and_withdraw_sources_are_counted_together() {
    // A program fed from *both* sources must have the per-account bound computed
    // over the whole holding: counting only the mint under-states what the deposit
    // leg will move and lets the surplus strand.
    new_test_ext().execute_with(|| {
        let converted = TestLocationToAccountId::convert_location(&asset_hub_location());
        assert!(
            converted.is_some(),
            "Asset Hub must have a local sovereign account"
        );
        let sovereign = converted.unwrap_or(AccountId::new([0_u8; 32]));
        assert_ok!(<ForeignAssets as Mutate<AccountId>>::mint_into(
            usdc_location(),
            &sovereign,
            1_000,
        ));
        set_caps(u128::MAX, 120);
        let minted = asset(usdc_location(), 100);
        let taken = asset(usdc_location(), 100);
        let program: Xcm<()> = Xcm(vec![
            ReserveAssetDeposited(Assets::from(minted.clone())),
            WithdrawAsset(Assets::from(taken)),
            ClearOrigin,
            BuyExecution {
                fees: minted,
                weight_limit: Limited(MAX_WEIGHT),
            },
            DepositAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(2)),
                beneficiary: account_location(ALICE_BYTES),
            },
        ]);
        let outcome = execute_inbound_program(program, 62);
        assert!(
            matches!(
                outcome,
                Outcome::Incomplete {
                    error: InstructionError {
                        error: XcmError::Barrier,
                        ..
                    },
                    ..
                }
            ),
            "the bound must cover mint + withdraw (200 > 120): {outcome:?}"
        );
        assert_eq!(trapped_assets_events(), 0);
    });
}

#[test]
fn caps_group_nested_appendix_deposit_leg_is_scanned() {
    // `SetAppendix` executes locally, so a deposit leg hidden there is as real as a
    // top-level one and must be bound by the same per-account check.
    new_test_ext().execute_with(|| {
        set_caps(u128::MAX, 1);
        let incoming = asset(usdc_location(), 100);
        let program: Xcm<()> = Xcm(vec![
            ReserveAssetDeposited(Assets::from(incoming.clone())),
            ClearOrigin,
            SetAppendix(Xcm(vec![DepositAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                beneficiary: account_location(ALICE_BYTES),
            }])),
            BuyExecution {
                fees: incoming,
                weight_limit: Limited(MAX_WEIGHT),
            },
        ]);
        let outcome = execute_inbound_program(program, 63);
        assert!(
            matches!(
                outcome,
                Outcome::Incomplete {
                    error: InstructionError {
                        error: XcmError::Barrier,
                        ..
                    },
                    ..
                }
            ),
            "an appendix deposit leg must be bound too: {outcome:?}"
        );
        assert_eq!(trapped_assets_events(), 0);
    });
}
