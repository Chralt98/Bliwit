//! Reserve-transferability probe program, dispatch and response routing (07 §8; I-24).

use alloc::vec;
use core::marker::PhantomData;
use frame_support::traits::Get;
use staging_xcm::latest::{
    send_xcm, Asset, AssetFilter, AssetId, Assets, Fungibility, Instruction, Location,
    QueryResponseInfo, Response, SendXcm, Weight, WeightLimit, WildAsset, Xcm, XcmContext,
};
use staging_xcm_executor::traits::OnResponse;

use crate::identity::{
    asset_hub_location, bleavit_as_seen_from_asset_hub, usdc_on_asset_hub_location,
};

/// High-bit flag partitioning probe query ids from pallet-xcm's counter (which
/// starts at 0 and increments; it cannot reach 2^63). On-wire probe id =
/// oracle id | FLAG; the oracle only ever sees its own unflagged id.
pub const PROBE_QUERY_ID_FLAG: u64 = 1 << 63;

/// Runtime seam implemented over `pallet-oracle::ReserveHealth` by B1a (07 §8).
pub trait ProbeSink {
    /// Outstanding unflagged oracle query id, if one is pending.
    fn pending_query_id() -> Option<u64>;
    /// Deliver a fail-static probe result to `pallet-oracle::reserve_probe_result`.
    fn probe_result(query_id: u64, passed: bool);
}

/// Builds the paid Asset Hub probe from an unflagged oracle query id (07 §8).
///
/// The paid prefix (`WithdrawAsset` + `BuyExecution`) is the live-verified V-6
/// shape. Its appendix runs after the main program in every outcome. It first
/// enables JIT payment of the response-delivery fee from the sovereign account,
/// then refunds unused trader fees to holding, re-deposits the complete holding,
/// and only then reports. If re-deposit fails the appendix aborts before
/// `ReportError`, so the absent response times out fail-static. Consequently an
/// `ExecutionResult(None)` proves withdrawal, fee purchase, refund and re-deposit
/// all succeeded. No instruction is `Transact` (I-24).
pub fn reserve_probe_program(
    query_id: u64,
    probe_amount: u128,
    exec_weight_budget: Weight,
    max_response_weight: Weight,
    our_para_id: u32,
) -> Xcm<()> {
    let probe_asset = Asset {
        id: AssetId(usdc_on_asset_hub_location()),
        fun: Fungibility::Fungible(probe_amount),
    };
    let response = QueryResponseInfo {
        destination: bleavit_as_seen_from_asset_hub(our_para_id),
        query_id: query_id | PROBE_QUERY_ID_FLAG,
        max_weight: max_response_weight,
    };
    Xcm(vec![
        Instruction::WithdrawAsset(Assets::from(probe_asset.clone())),
        Instruction::BuyExecution {
            fees: probe_asset,
            weight_limit: WeightLimit::Limited(exec_weight_budget),
        },
        Instruction::SetAppendix(Xcm(vec![
            Instruction::SetFeesMode { jit_withdraw: true },
            Instruction::RefundSurplus,
            Instruction::DepositAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                beneficiary: bleavit_as_seen_from_asset_hub(our_para_id),
            },
            Instruction::ReportError(response),
        ])),
    ])
}

/// B4 implementation of the oracle's XCM-free post-commit probe seam (07 §8).
///
/// Send failures are deliberately swallowed: the committed pending query then
/// reaches the oracle's bounded timeout and degrades reserve health fail-static.
pub struct XcmProbeDispatcher<Router, ProbeAmount, ExecWeightBudget, MaxResponseWeight, OurParaId>(
    PhantomData<(
        Router,
        ProbeAmount,
        ExecWeightBudget,
        MaxResponseWeight,
        OurParaId,
    )>,
);

impl<Router, ProbeAmount, ExecWeightBudget, MaxResponseWeight, OurParaId>
    pallet_oracle::ProbeDispatch
    for XcmProbeDispatcher<Router, ProbeAmount, ExecWeightBudget, MaxResponseWeight, OurParaId>
where
    Router: SendXcm,
    ProbeAmount: Get<u128>,
    ExecWeightBudget: Get<Weight>,
    MaxResponseWeight: Get<Weight>,
    OurParaId: Get<u32>,
{
    fn probe_due(query_id: u64) {
        let program = reserve_probe_program(
            query_id,
            ProbeAmount::get(),
            ExecWeightBudget::get(),
            MaxResponseWeight::get(),
            OurParaId::get(),
        );
        let _ = send_xcm::<Router>(asset_hub_location(), program);
    }
}

/// Routes only the authenticated, partitioned Asset Hub probe response to the
/// oracle and delegates every other response untouched (07 §8; I-24).
pub struct ProbeAwareResponseHandler<Inner, Sink>(PhantomData<(Inner, Sink)>);

impl<Inner: OnResponse, Sink: ProbeSink> ProbeAwareResponseHandler<Inner, Sink> {
    fn oracle_query_id(
        origin: &Location,
        query_id: u64,
        querier: Option<&Location>,
    ) -> Option<u64> {
        // A genuine Asset Hub `ReportError` identifies the original probe's
        // origin as its querier. Reanchored at Bleavit, our chain is `Here`.
        // A report induced by an Asset Hub account instead carries that account
        // as querier and must never authenticate as a reserve probe.
        if origin != &asset_hub_location()
            || querier != Some(&Location::here())
            || query_id & PROBE_QUERY_ID_FLAG == 0
        {
            return None;
        }
        let oracle_query_id = query_id & !PROBE_QUERY_ID_FLAG;
        Sink::pending_query_id()
            .is_some_and(|pending| pending == oracle_query_id)
            .then_some(oracle_query_id)
    }
}

impl<Inner: OnResponse, Sink: ProbeSink> OnResponse for ProbeAwareResponseHandler<Inner, Sink> {
    fn expecting_response(origin: &Location, query_id: u64, querier: Option<&Location>) -> bool {
        Self::oracle_query_id(origin, query_id, querier).is_some()
            || Inner::expecting_response(origin, query_id, querier)
    }

    fn on_response(
        origin: &Location,
        query_id: u64,
        querier: Option<&Location>,
        response: Response,
        max_weight: Weight,
        context: &XcmContext,
    ) -> Weight {
        if let Some(oracle_query_id) = Self::oracle_query_id(origin, query_id, querier) {
            Sink::probe_result(
                oracle_query_id,
                matches!(response, Response::ExecutionResult(None)),
            );
            // The sink mutates oracle storage. Until B1a exposes a dedicated measured weight,
            // charge the full response budget instead of under-accounting the callback.
            max_weight
        } else {
            Inner::on_response(origin, query_id, querier, response, max_weight, context)
        }
    }
}
