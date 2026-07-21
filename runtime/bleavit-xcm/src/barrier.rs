//! Default-deny inbound XCM barrier (09 §6.1).

use alloc::vec::Vec;
use core::marker::PhantomData;
use frame_support::traits::{Contains, Get, ProcessMessageError};
use staging_xcm::latest::{
    Asset, AssetId, Fungibility, Instruction, InteriorLocation, Location, Weight,
};
use staging_xcm::{MAX_INSTRUCTIONS_TO_DECODE, MAX_XCM_DECODE_DEPTH};
use staging_xcm_builder::{
    AllowKnownQueryResponses, AllowSubscriptionsFrom, AllowTopLevelPaidExecutionFrom, DenyThenTry,
    TakeWeightCredit, TrailingSetTopicAsId, WithComputedOrigin,
};
use staging_xcm_executor::traits::{ConvertLocation, DenyExecution, OnResponse, Properties};

use crate::caps::InflowCaps;
use crate::identity::{asset_hub_location, coretime_location, relay_location, usdc_location};

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

/// Refuses an inbound program whose local USDC mint would breach either Phase-3
/// inflow cap, **before** the executor is constructed (09 §5.2, SQ-129).
///
/// 09 §5.2 is normative that *both* caps bind before any local mint and that a cap
/// refusal leaves "nothing minted and nothing trapped locally": an inbound message's
/// trap is keyed under the *sending* chain's origin, so a beneficiary could never
/// self-claim it, and a deposit-leg refusal would convert a recoverable upstream
/// failure into a permanently stranded one (09 §6.1 trapped-assets row; R-7, G-1).
/// The per-account check therefore binds the beneficiary extracted from the program's
/// `DepositAsset`, which the `AssetTransactor` interface hides but the message carries.
///
/// The gate fires whenever a program brings **new** USDC into local circulation —
/// `ReserveAssetDeposited` (the issuance-increasing mint) or `WithdrawAsset` (which
/// fills holding from the sender's local sovereign account with no mint at all).
/// A program whose only USDC source is `ClaimAsset` is left alone: 09 §5.2's
/// mint-step scope (SQ-253) exempts `pallet-xcm`'s trapped-imbalance reconstruction,
/// and a refusal at its metered deposit leg simply returns the assets to the trap
/// they came from, stranding nothing new. When the gate does fire, the bound covers
/// the **whole** holding — reclaimed assets included — because a deposit leg cannot
/// tell the sources apart.
///
/// Both reads are pure: nothing is reserved here, and the cumulative meter is still
/// written exactly once, at the deposit leg.
pub struct DenyOverCapInflows<Caps, LocationToAccountId, AccountId>(
    PhantomData<(Caps, LocationToAccountId, AccountId)>,
);

/// What a scan of the locally-executing instructions found.
#[derive(Default)]
struct LocalInflow {
    /// USDC entering local circulation anew (`ReserveAssetDeposited` + `WithdrawAsset`).
    entering_usdc: u128,
    /// USDC re-entering holding from the trap register (`ClaimAsset`).
    reclaimed_usdc: u128,
    /// Every local deposit target reachable in the same scopes: `DepositAsset`'s
    /// beneficiary and `DepositReserveAsset`'s `dest` (which is credited to `dest`'s
    /// local sovereign account *before* the onward message is sent).
    targets: Vec<Location>,
}

impl<Caps, LocationToAccountId, AccountId> DenyOverCapInflows<Caps, LocationToAccountId, AccountId>
where
    Caps: InflowCaps<AccountId>,
    LocationToAccountId: ConvertLocation<AccountId>,
{
    /// Sum USDC into `slot` from one instruction's asset list. Overflow fails closed.
    fn accrue_usdc(assets: &[Asset], slot: &mut u128) -> Result<(), ()> {
        for asset in assets {
            let (AssetId(id), Fungibility::Fungible(amount)) = (&asset.id, &asset.fun) else {
                continue;
            };
            if id != &usdc_location() {
                continue;
            }
            *slot = slot.checked_add(*amount).ok_or(())?;
        }
        Ok(())
    }

    /// Walk only the scopes that execute **on this chain**. `SetAppendix` and
    /// `SetErrorHandler` run locally and are descended into; the programs carried by
    /// `DepositReserveAsset`/`InitiateReserveWithdraw`/`InitiateTeleport` run on the
    /// *remote* chain and are not — though `DepositReserveAsset`'s own local deposit
    /// leg is recorded. `TransferReserveAsset` is deliberately absent: it moves value
    /// through `transfer_asset`, which `CappedInflows` does not meter, so it can never
    /// produce a cap refusal. Overflow and budget exhaustion fail closed.
    fn scan<Call>(
        instructions: &[Instruction<Call>],
        depth: u32,
        remaining: &mut usize,
        found: &mut LocalInflow,
    ) -> Result<(), ()> {
        if depth > MAX_XCM_DECODE_DEPTH || instructions.len() > *remaining {
            return Err(());
        }
        let Some(next_remaining) = remaining.checked_sub(instructions.len()) else {
            return Err(());
        };
        *remaining = next_remaining;
        for instruction in instructions {
            match instruction {
                Instruction::ReserveAssetDeposited(assets) => {
                    Self::accrue_usdc(assets.inner(), &mut found.entering_usdc)?;
                }
                Instruction::WithdrawAsset(assets) => {
                    Self::accrue_usdc(assets.inner(), &mut found.entering_usdc)?;
                }
                Instruction::ClaimAsset { assets, .. } => {
                    Self::accrue_usdc(assets.inner(), &mut found.reclaimed_usdc)?;
                }
                Instruction::DepositAsset { beneficiary, .. } => {
                    found.targets.push(beneficiary.clone());
                }
                Instruction::DepositReserveAsset { dest, .. } => {
                    found.targets.push(dest.clone());
                }
                Instruction::SetAppendix(xcm) | Instruction::SetErrorHandler(xcm) => {
                    let next = depth.checked_add(1).ok_or(())?;
                    Self::scan(&xcm.0, next, remaining, found)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn admissible<Call>(instructions: &[Instruction<Call>]) -> bool {
        let mut remaining = usize::from(MAX_INSTRUCTIONS_TO_DECODE);
        let mut found = LocalInflow::default();
        if Self::scan(instructions, 0, &mut remaining, &mut found).is_err() {
            return false;
        }
        // Pure recovery (09 §5.2 mint-step scope, SQ-253): nothing new enters local
        // circulation, and a refusal at the metered deposit leg returns the assets to
        // the trap they came from. Leave it to that leg.
        if found.entering_usdc == 0 {
            return true;
        }
        // Only the per-account cap is enforced here. `phase3.tvl_cap` is already
        // prospective at its own leg — 09 §5.2 names the mint step its *binding*
        // enforcement point, and an over-global-cap transfer already fails on the
        // first local instruction with nothing minted and nothing trapped. It is
        // `phase3.deposit_cap` that lacked a pre-mint home, because the beneficiary
        // is invisible to the `AssetTransactor` interface; this gate supplies it.
        let Some(bound) = found.entering_usdc.checked_add(found.reclaimed_usdc) else {
            return false;
        };
        // The whole holding is a sound upper bound on what any single deposit leg can
        // move, so checking every target against it can only ever be stricter than the
        // deposit leg — never laxer. A target this chain cannot resolve to a local
        // account fails closed. Targets are deduplicated so a program cannot inflate
        // the number of storage reads by repeating one.
        let mut checked: Vec<&Location> = Vec::new();
        for target in &found.targets {
            if checked.contains(&target) {
                continue;
            }
            checked.push(target);
            let admitted = LocationToAccountId::convert_location(target)
                .is_some_and(|who| Caps::usdc_inflow_admissible(&who, bound).is_ok());
            if !admitted {
                return false;
            }
        }
        true
    }
}

impl<Caps, LocationToAccountId, AccountId> DenyExecution
    for DenyOverCapInflows<Caps, LocationToAccountId, AccountId>
where
    Caps: InflowCaps<AccountId>,
    LocationToAccountId: ConvertLocation<AccountId>,
{
    fn deny_execution<RuntimeCall>(
        _origin: &Location,
        instructions: &mut [Instruction<RuntimeCall>],
        _max_weight: Weight,
        _properties: &mut Properties,
    ) -> Result<(), ProcessMessageError> {
        if Self::admissible(instructions) {
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
pub type BleavitBarrier<
    ResponseHandler,
    UniversalLocation,
    MaxPrefixes,
    Caps,
    LocationToAccountId,
    AccountId,
> = DenyThenTry<
    (
        DenyTransact,
        DenyUnsupportedInstructions,
        DenyOverCapInflows<Caps, LocationToAccountId, AccountId>,
    ),
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
