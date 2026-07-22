//! Reserve-transferability probe program, dispatch and response routing (07 §8; I-24).

use alloc::vec;
use core::marker::PhantomData;
use frame_support::traits::Get;
use sp_runtime::DispatchError;
use staging_xcm::latest::{
    send_xcm, Asset, AssetFilter, AssetId, Assets, Fungibility, Instruction, Location,
    QueryResponseInfo, Response, SendXcm, Weight, WeightLimit, WildAsset, Xcm, XcmContext,
};
use staging_xcm_executor::traits::OnResponse;
use staging_xcm_executor::traits::WeightBounds;

use crate::health::LocalXcmHealthSink;

use crate::identity::{
    asset_hub_location, bleavit_as_seen_from_asset_hub, dot_on_asset_hub_location,
    usdc_on_asset_hub_location,
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
    /// Returns the measured worst-case weight consumed by the callback. The
    /// XCM weigher charges the same bound before execution; the return value is
    /// retained for the `OnResponse` contract and auditability.
    fn probe_result(query_id: u64, passed: bool) -> Weight;
}

/// Pre-dispatch accounting seam for the bounded Asset Hub DOT fee envelope
/// (07 §8; 08 §1.1; SQ-114).
pub trait ProbeBudget {
    /// Whether the complete fail+recovery runway is presently fundable. This is
    /// consulted only by the oracle's monotone first-arm latch; after arming,
    /// budget exhaustion must not disable fail-static attempts.
    fn ready_to_arm(_: &pallet_oracle::OracleParams) -> bool {
        false
    }

    /// Validate the complete live probe envelope, atomically reserve one fee
    /// envelope, and return its DOT-planck amount. `probe_amount` binds the
    /// dispatcher's program to that same live snapshot. An error refuses the
    /// send. The dispatcher runs this in the same storage layer as local XCM
    /// send validation, so a locally-rejected send unwinds the reservation.
    fn reserve_fee(probe_amount: u128) -> Result<u128, DispatchError>;
}

enum ProbeDispatchFailure {
    Budget,
    Send,
}

impl From<DispatchError> for ProbeDispatchFailure {
    fn from(_: DispatchError) -> Self {
        Self::Budget
    }
}

impl ProbeBudget for () {
    fn reserve_fee(_: u128) -> Result<u128, DispatchError> {
        Err(DispatchError::Other("reserve probe budget unavailable"))
    }
}

/// Builds the paid Asset Hub probe from an unflagged oracle query id (07 §8).
///
/// The paid prefix (`WithdrawAsset` + `BuyExecution`) admits exactly the USDC
/// probe amount and a governed DOT fee envelope into holding. The appendix
/// refunds unused execution fees, re-deposits the exact USDC amount, and only
/// then reports. Response delivery is paid from the bounded DOT still in
/// holding; JIT withdrawal is never enabled. If USDC re-deposit or response
/// delivery fails, no passing response reaches Bleavit and the pending probe
/// times out fail-static. A success proves the USDC round trip and response
/// emission; the trailing best-effort DOT cleanup is deliberately outside that
/// health boundary. No instruction is `Transact` (I-24).
pub fn reserve_probe_program(
    query_id: u64,
    probe_amount: u128,
    fee_amount: u128,
    exec_weight_budget: Weight,
    max_response_weight: Weight,
    our_para_id: u32,
) -> Xcm<()> {
    let probe_asset = Asset {
        id: AssetId(usdc_on_asset_hub_location()),
        fun: Fungibility::Fungible(probe_amount),
    };
    let fee_asset = Asset {
        id: AssetId(dot_on_asset_hub_location()),
        fun: Fungibility::Fungible(fee_amount),
    };
    let response = QueryResponseInfo {
        destination: bleavit_as_seen_from_asset_hub(our_para_id),
        query_id: query_id | PROBE_QUERY_ID_FLAG,
        max_weight: max_response_weight,
    };
    Xcm(vec![
        // Both assets enter holding under one bounded withdrawal. DOT pays all
        // remote execution and response delivery; no JIT withdrawal can reach
        // any other sovereign balance (SQ-114).
        Instruction::WithdrawAsset(vec![probe_asset.clone(), fee_asset.clone()].into()),
        Instruction::BuyExecution {
            fees: fee_asset,
            weight_limit: WeightLimit::Limited(exec_weight_budget),
        },
        Instruction::SetAppendix(Xcm(vec![
            Instruction::RefundSurplus,
            // A success response is unreachable until the exact USDC probe
            // amount has been re-deposited. Failure here aborts the appendix
            // before `ReportError`, leaving the local timeout fail-static.
            Instruction::DepositAsset {
                assets: AssetFilter::Definite(Assets::from(probe_asset)),
                beneficiary: bleavit_as_seen_from_asset_hub(our_para_id),
            },
            Instruction::ReportError(response),
            // `ReportError` consumes its delivery fee from bounded DOT still in
            // holding. Return any conservative over-envelope remainder after
            // the response; its cleanup cannot turn a failed USDC round-trip
            // into a pass because that deposit already happened above.
            Instruction::DepositAsset {
                assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
                beneficiary: bleavit_as_seen_from_asset_hub(our_para_id),
            },
        ])),
    ])
}

/// B4 implementation of the oracle's XCM-free post-commit probe seam (07 §8).
///
/// Send failures are deliberately swallowed: the committed pending query then
/// reaches the oracle's bounded timeout and degrades reserve health fail-static.
pub struct XcmProbeDispatcher<
    Router,
    Budget,
    ExecWeightBudget,
    MaxResponseWeight,
    OurParaId,
    HealthSink,
>(
    PhantomData<(
        Router,
        Budget,
        ExecWeightBudget,
        MaxResponseWeight,
        OurParaId,
        HealthSink,
    )>,
);

impl<Router, Budget, ExecWeightBudget, MaxResponseWeight, OurParaId, HealthSink>
    pallet_oracle::ProbeDispatch
    for XcmProbeDispatcher<
        Router,
        Budget,
        ExecWeightBudget,
        MaxResponseWeight,
        OurParaId,
        HealthSink,
    >
where
    Router: SendXcm,
    Budget: ProbeBudget,
    ExecWeightBudget: Get<Weight>,
    MaxResponseWeight: Get<Weight>,
    OurParaId: Get<u32>,
    HealthSink: LocalXcmHealthSink,
{
    fn live(params: &pallet_oracle::OracleParams) -> bool {
        params.reserve_probe_config_valid() && Budget::ready_to_arm(params)
    }

    fn probe_due(query_id: u64, probe_amount: u128) {
        // A zero-value round trip proves nothing. Refuse before the budget or
        // router can move; the committed pending attempt will timeout fail-static.
        if probe_amount == 0 {
            return;
        }
        let result = frame_support::storage::with_storage_layer(|| {
            let fee_amount =
                Budget::reserve_fee(probe_amount).map_err(|_| ProbeDispatchFailure::Budget)?;
            let program = reserve_probe_program(
                query_id,
                probe_amount,
                fee_amount,
                ExecWeightBudget::get(),
                MaxResponseWeight::get(),
                OurParaId::get(),
            );
            send_xcm::<Router>(asset_hub_location(), program)
                .map(|_| ())
                .map_err(|_| ProbeDispatchFailure::Send)
        });
        match result {
            Ok(()) => HealthSink::note_sent(),
            // A budget refusal happens before the router and is therefore not
            // an XCM transport failure. The outstanding probe still times out.
            Err(ProbeDispatchFailure::Budget) => {}
            // This observation must sit outside the rollback layer: the debit,
            // treasury event and partial queue writes unwind, but 09 §6.4's
            // local-failure counter remains durable exactly once.
            Err(ProbeDispatchFailure::Send) => HealthSink::note_send_failure(),
        }
    }
}

/// Charges the measured reserve-probe callback for high-bit `QueryResponse`
/// instructions. The SDK executor does not add the weight returned by
/// `OnResponse`, so relying on that return value alone underweights the message.
/// Unknown high-bit ids pay the same conservative surcharge before the barrier
/// rejects them; ordinary pallet-xcm response ids retain `Inner`'s weight.
pub struct ProbeAwareWeightBounds<Inner, ProbeWeight>(PhantomData<(Inner, ProbeWeight)>);

impl<Inner, ProbeWeight> ProbeAwareWeightBounds<Inner, ProbeWeight> {
    /// Additional callback work performed only by locally-executed nested XCM.
    /// Remote programs carried by transfer/export instructions are not executed
    /// by this executor and therefore must not be charged here.
    fn callback_weight<Call>(
        instruction: &Instruction<Call>,
    ) -> Result<Weight, staging_xcm::latest::prelude::XcmError>
    where
        ProbeWeight: Get<Weight>,
    {
        let direct = if matches!(
            instruction,
            Instruction::QueryResponse { query_id, .. }
                if *query_id & PROBE_QUERY_ID_FLAG != 0
        ) {
            ProbeWeight::get()
        } else {
            Weight::zero()
        };
        let nested = match instruction {
            Instruction::SetErrorHandler(xcm)
            | Instruction::SetAppendix(xcm)
            | Instruction::ExecuteWithOrigin { xcm, .. } => {
                let mut total = Weight::zero();
                for nested in &xcm.0 {
                    total = total
                        .checked_add(&Self::callback_weight(nested)?)
                        .ok_or(staging_xcm::latest::prelude::XcmError::Overflow)?;
                }
                total
            }
            _ => Weight::zero(),
        };
        direct
            .checked_add(&nested)
            .ok_or(staging_xcm::latest::prelude::XcmError::Overflow)
    }
}

impl<Call, Inner, ProbeWeight> WeightBounds<Call> for ProbeAwareWeightBounds<Inner, ProbeWeight>
where
    Inner: WeightBounds<Call>,
    ProbeWeight: Get<Weight>,
{
    fn weight(
        message: &mut Xcm<Call>,
        weight_limit: Weight,
    ) -> Result<Weight, staging_xcm::latest::InstructionError> {
        let mut total = Inner::weight(message, weight_limit)?;
        for (index, instruction) in message.0.iter().enumerate() {
            let index = index.try_into().unwrap_or(u8::MAX);
            let callback = Self::callback_weight(instruction)
                .map_err(|error| staging_xcm::latest::InstructionError { index, error })?;
            total = total
                .checked_add(&callback)
                .ok_or(staging_xcm::latest::InstructionError {
                    index,
                    error: staging_xcm::latest::prelude::XcmError::Overflow,
                })?;
            if total.any_gt(weight_limit) {
                return Err(staging_xcm::latest::InstructionError {
                    index,
                    error: staging_xcm::latest::prelude::XcmError::WeightLimitReached(total),
                });
            }
        }
        Ok(total)
    }

    fn instr_weight(
        instruction: &mut Instruction<Call>,
    ) -> Result<Weight, staging_xcm::latest::prelude::XcmError> {
        Inner::instr_weight(instruction)?
            .checked_add(&Self::callback_weight(instruction)?)
            .ok_or(staging_xcm::latest::prelude::XcmError::Overflow)
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
            )
            .min(max_weight)
        } else {
            Inner::on_response(origin, query_id, querier, response, max_weight, context)
        }
    }
}
