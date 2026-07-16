//! Default-deny inbound XCM barrier (09 §6.1).

use core::marker::PhantomData;
use frame_support::traits::{Contains, Get, ProcessMessageError};
use staging_xcm::latest::{Instruction, InteriorLocation, Location, Weight};
use staging_xcm::{MAX_INSTRUCTIONS_TO_DECODE, MAX_XCM_DECODE_DEPTH};
use staging_xcm_builder::{
    AllowKnownQueryResponses, AllowSubscriptionsFrom, AllowTopLevelPaidExecutionFrom, DenyThenTry,
    TakeWeightCredit, TrailingSetTopicAsId, WithComputedOrigin,
};
use staging_xcm_executor::traits::{DenyExecution, OnResponse, Properties};

use crate::identity::{asset_hub_location, coretime_location, relay_location};

/// Exactly the three remote origins admitted by the v1 rule table (09 §6.1).
pub struct AcceptedXcmOrigins;

impl Contains<Location> for AcceptedXcmOrigins {
    fn contains(location: &Location) -> bool {
        location == &asset_hub_location()
            || location == &relay_location()
            || location == &coretime_location()
    }
}

/// Denies `Transact` at any nesting depth and any explicit unpaid execution (09 §6.1).
pub struct DenyTransact;

impl DenyTransact {
    fn contains_transact<Call>(instructions: &[Instruction<Call>]) -> bool {
        let mut remaining = usize::from(MAX_INSTRUCTIONS_TO_DECODE);
        Self::contains_transact_bounded(instructions, 0, &mut remaining)
    }

    fn contains_transact_bounded<Call>(
        instructions: &[Instruction<Call>],
        depth: u32,
        remaining: &mut usize,
    ) -> bool {
        if depth > MAX_XCM_DECODE_DEPTH || instructions.len() > *remaining {
            return true;
        }
        let Some(next_remaining) = remaining.checked_sub(instructions.len()) else {
            return true;
        };
        *remaining = next_remaining;
        instructions.iter().any(|instruction| match instruction {
            Instruction::Transact { .. } | Instruction::UnpaidExecution { .. } => true,
            Instruction::SetAppendix(xcm)
            | Instruction::SetErrorHandler(xcm)
            | Instruction::ExecuteWithOrigin { xcm, .. } => depth
                .checked_add(1)
                .is_none_or(|next| Self::contains_transact_bounded(&xcm.0, next, remaining)),
            Instruction::TransferReserveAsset { xcm, .. }
            | Instruction::DepositReserveAsset { xcm, .. }
            | Instruction::InitiateReserveWithdraw { xcm, .. }
            | Instruction::InitiateTeleport { xcm, .. }
            | Instruction::ExportMessage { xcm, .. } => depth
                .checked_add(1)
                .is_none_or(|next| Self::contains_transact_bounded(&xcm.0, next, remaining)),
            Instruction::InitiateTransfer { remote_xcm, .. } => depth
                .checked_add(1)
                .is_none_or(|next| Self::contains_transact_bounded(&remote_xcm.0, next, remaining)),
            _ => false,
        })
    }
}

impl DenyExecution for DenyTransact {
    fn deny_execution<RuntimeCall>(
        _origin: &Location,
        instructions: &mut [Instruction<RuntimeCall>],
        _max_weight: Weight,
        _properties: &mut Properties,
    ) -> Result<(), ProcessMessageError> {
        if Self::contains_transact(instructions) {
            Err(ProcessMessageError::Unsupported)
        } else {
            Ok(())
        }
    }
}

/// Rejects every instruction outside the reserve-transfer, fee, query and version-negotiation
/// surface; nested programs are checked recursively (09 §6.1).
pub struct DenyUnsupportedInstructions;

impl DenyUnsupportedInstructions {
    fn all_supported<Call>(instructions: &[Instruction<Call>]) -> bool {
        let mut remaining = usize::from(MAX_INSTRUCTIONS_TO_DECODE);
        Self::all_supported_bounded(instructions, 0, &mut remaining)
    }

    fn all_supported_bounded<Call>(
        instructions: &[Instruction<Call>],
        depth: u32,
        remaining: &mut usize,
    ) -> bool {
        if depth > MAX_XCM_DECODE_DEPTH || instructions.len() > *remaining {
            return false;
        }
        let Some(next_remaining) = remaining.checked_sub(instructions.len()) else {
            return false;
        };
        *remaining = next_remaining;
        instructions.iter().all(|instruction| match instruction {
            // This is the closed v1 surface needed by canonical reserve transfers, fee
            // purchase/refund, trapped-asset handling, query responses and version discovery.
            // Origin-changing and assertion instructions are deliberately absent: accepting
            // one in a future flow requires an explicit review of this list (09 §6.1).
            Instruction::WithdrawAsset(_)
            | Instruction::ReserveAssetDeposited(_)
            | Instruction::QueryResponse { .. }
            | Instruction::ClearOrigin
            | Instruction::ReportError(_)
            | Instruction::DepositAsset { .. }
            | Instruction::BuyExecution { .. }
            | Instruction::RefundSurplus
            | Instruction::ClaimAsset { .. }
            | Instruction::SubscribeVersion { .. }
            | Instruction::UnsubscribeVersion
            | Instruction::SetFeesMode { .. }
            | Instruction::SetTopic(_)
            | Instruction::ClearTopic
            | Instruction::PayFees { .. } => true,
            Instruction::SetAppendix(xcm) | Instruction::SetErrorHandler(xcm) => depth
                .checked_add(1)
                .is_some_and(|next| Self::all_supported_bounded(&xcm.0, next, remaining)),
            Instruction::TransferReserveAsset { xcm, .. }
            | Instruction::DepositReserveAsset { xcm, .. }
            | Instruction::InitiateReserveWithdraw { xcm, .. }
            | Instruction::InitiateTeleport { xcm, .. } => depth
                .checked_add(1)
                .is_some_and(|next| Self::all_supported_bounded(&xcm.0, next, remaining)),
            _ => false,
        })
    }
}

impl DenyExecution for DenyUnsupportedInstructions {
    fn deny_execution<RuntimeCall>(
        _origin: &Location,
        instructions: &mut [Instruction<RuntimeCall>],
        _max_weight: Weight,
        _properties: &mut Properties,
    ) -> Result<(), ProcessMessageError> {
        if Self::all_supported(instructions) {
            Ok(())
        } else {
            Err(ProcessMessageError::Unsupported)
        }
    }
}

/// The reusable Bleavit barrier (09 §6.1).
///
/// Pre-paid local execution may consume weight credit; remote execution must otherwise be a
/// known query response, paid from an accepted origin, or a version subscription from one.
/// There is deliberately no unpaid-execution allow path and no superuser conversion.
pub type BleavitBarrier<ResponseHandler, UniversalLocation, MaxPrefixes> = DenyThenTry<
    (DenyTransact, DenyUnsupportedInstructions),
    TrailingSetTopicAsId<(
        TakeWeightCredit,
        AllowKnownQueryResponses<ResponseHandler>,
        WithComputedOrigin<
            (
                AllowTopLevelPaidExecutionFrom<AcceptedXcmOrigins>,
                AllowSubscriptionsFrom<AcceptedXcmOrigins>,
            ),
            UniversalLocation,
            MaxPrefixes,
        >,
    )>,
>;

// Keep the generic obligations close to the alias so B1a gets a short diagnostic on drift.
#[allow(dead_code)]
struct BarrierBounds<ResponseHandler, UniversalLocation, MaxPrefixes>(
    PhantomData<(ResponseHandler, UniversalLocation, MaxPrefixes)>,
)
where
    ResponseHandler: OnResponse,
    UniversalLocation: Get<InteriorLocation>,
    MaxPrefixes: Get<u32>;
