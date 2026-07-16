//! Coretime-renewal DOT funding program and dispatch (09 §4; 09 §6.1).

use alloc::vec;
use core::marker::PhantomData;
use frame_support::{pallet_prelude::DispatchResult, traits::Get, weights::Weight, Hashable};
use sp_runtime::DispatchError;
use staging_xcm::latest::{
    Asset, AssetFilter, AssetId, Assets, ExecuteXcm, Fungibility, Instruction, Junction, Location,
    WeightLimit, WildAsset, Xcm,
};

use futarchy_primitives::chain_identity::CORETIME_PARA_ID;

use crate::identity::{dot_location, renewal_account_location};

/// A local executor plan for the canonical DOT remote-reserve route (09 §4).
///
/// The local program withdraws DOT under the treasury origin. Its
/// `InitiateReserveWithdraw` sends the nested leg to DOT's relay reserve, so no
/// separate local `SendXcm` destination exists.
pub struct CoretimeRenewalPlan {
    pub local_program: Xcm<()>,
}

/// Typed refusal when a renewal transfer cannot safely fund both remote
/// execution legs (09 §4; G-1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoretimeProgramError {
    FeeBudgetTooSmall,
    Arithmetic,
}

/// Builds the DOT funding route through the relay reserve to Coretime (09 §4).
///
/// The renewal quote and an independent fee budget are withdrawn locally. Half
/// the fee budget buys relay execution; the remainder buys Coretime execution.
/// The relay then teleports its native DOT to Coretime. This relay→Coretime leg
/// follows the live Coretime-chain configuration (`IsReserve = ()`, DOT accepted
/// as a relay teleport). Bleavit's own `IsTeleporter = ()` remains untouched:
/// locally we only initiate a reserve withdrawal of DOT already held here.
///
/// Both remote buys draw exclusively from `fee_budget`, so the renewal account
/// receives at least `renewal_amount`. Unused fee surplus is deposited there as
/// useful pre-funding for later renewal fees. The treasury owns the period
/// idempotency key and bounded retry; no remote outcome enters decisions (I-24).
pub fn coretime_renewal_program(
    renewal_amount: u128,
    fee_budget: u128,
    renewal_account: [u8; 32],
    relay_weight_limit: WeightLimit,
    coretime_weight_limit: WeightLimit,
) -> Result<CoretimeRenewalPlan, CoretimeProgramError> {
    let total = renewal_amount
        .checked_add(fee_budget)
        .ok_or(CoretimeProgramError::Arithmetic)?;
    let relay_fee_budget = fee_budget / 2;
    let coretime_fee_budget = fee_budget
        .checked_sub(relay_fee_budget)
        .ok_or(CoretimeProgramError::Arithmetic)?;
    if relay_fee_budget == 0 || coretime_fee_budget == 0 {
        return Err(CoretimeProgramError::FeeBudgetTooSmall);
    }

    let local_dot = Asset {
        id: AssetId(dot_location()),
        fun: Fungibility::Fungible(total),
    };
    let relay_dot = Asset {
        id: AssetId(Location::here()),
        fun: Fungibility::Fungible(relay_fee_budget),
    };
    let coretime_dot = Asset {
        id: AssetId(dot_location()),
        fun: Fungibility::Fungible(coretime_fee_budget),
    };

    let on_coretime = Xcm(vec![
        Instruction::BuyExecution {
            fees: coretime_dot,
            weight_limit: coretime_weight_limit,
        },
        Instruction::DepositAsset {
            assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
            beneficiary: renewal_account_location(renewal_account),
        },
    ]);
    let on_relay = Xcm(vec![
        Instruction::BuyExecution {
            fees: relay_dot,
            weight_limit: relay_weight_limit,
        },
        // Verified live route: Coretime trusts relay-native DOT only as a
        // teleport from the relay, not as a reserve deposit from Bleavit.
        Instruction::InitiateTeleport {
            assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
            dest: Location::new(0, [Junction::Parachain(CORETIME_PARA_ID)]),
            xcm: on_coretime,
        },
    ]);
    let local_program = Xcm(vec![
        Instruction::WithdrawAsset(Assets::from(local_dot)),
        Instruction::InitiateReserveWithdraw {
            assets: AssetFilter::Wild(WildAsset::AllCounted(1)),
            reserve: Location::parent(),
            xcm: on_relay,
        },
    ]);
    Ok(CoretimeRenewalPlan { local_program })
}

/// B4 implementation of the treasury's XCM-free renewal dispatch seam (09 §4).
pub struct XcmRenewalDispatcher<
    Executor,
    RuntimeCall,
    TreasuryLocation,
    FeeBudget,
    RenewalAccount,
    RelayLimit,
    CoretimeLimit,
    LocalExecLimit,
>(
    #[allow(clippy::type_complexity)]
    PhantomData<(
        Executor,
        RuntimeCall,
        TreasuryLocation,
        FeeBudget,
        RenewalAccount,
        RelayLimit,
        CoretimeLimit,
        LocalExecLimit,
    )>,
);

impl<
        Executor,
        RuntimeCall,
        TreasuryLocation,
        FeeBudget,
        RenewalAccount,
        RelayLimit,
        CoretimeLimit,
        LocalExecLimit,
    > pallet_futarchy_treasury::RenewalDispatch
    for XcmRenewalDispatcher<
        Executor,
        RuntimeCall,
        TreasuryLocation,
        FeeBudget,
        RenewalAccount,
        RelayLimit,
        CoretimeLimit,
        LocalExecLimit,
    >
where
    Executor: ExecuteXcm<RuntimeCall>,
    TreasuryLocation: Get<Location>,
    FeeBudget: Get<u128>,
    RenewalAccount: Get<[u8; 32]>,
    RelayLimit: Get<Weight>,
    CoretimeLimit: Get<Weight>,
    LocalExecLimit: Get<Weight>,
{
    fn dispatch_renewal(_period_index: u32, amount: u128) -> DispatchResult {
        let relay_limit = RelayLimit::get();
        let coretime_limit = CoretimeLimit::get();
        let plan = coretime_renewal_program(
            amount,
            FeeBudget::get(),
            RenewalAccount::get(),
            WeightLimit::Limited(relay_limit),
            WeightLimit::Limited(coretime_limit),
        )
        .map_err(|_| DispatchError::Other("invalid coretime renewal XCM plan"))?;

        // The local execution budget is a separate scale from the relay-leg fee
        // bound: Bleavit's weigher prices the local withdraw + reserve-withdraw
        // legs, the relay's its own. A B1a value sized for one must not
        // spuriously fail the other (re-review minor, 2026-07-16); a failure
        // here still only rolls back for keeper retry (09 §4).
        let local_limit = LocalExecLimit::get();
        let mut message_id = plan.local_program.blake2_256();
        Executor::prepare_and_execute(
            TreasuryLocation::get(),
            plan.local_program.into::<RuntimeCall>(),
            &mut message_id,
            local_limit,
            local_limit,
        )
        .ensure_complete()
        .map_err(|_| DispatchError::Other("coretime renewal XCM execution failed"))
    }
}
