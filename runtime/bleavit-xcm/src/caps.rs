//! Phase-3 USDC inflow-cap enforcement at the XCM mint and deposit legs (09 §5.2).

use core::marker::PhantomData;
use frame_support::storage::{with_transaction, TransactionOutcome};
use sp_runtime::DispatchError;
use staging_xcm::latest::{
    Asset, AssetId, Error as XcmError, Fungibility, Location, Weight, XcmContext,
};
use staging_xcm_executor::{
    traits::{ConvertLocation, TransactAsset},
    AssetsInHolding,
};

use crate::identity::usdc_location;

/// Runtime seam for the global and per-account Phase-3 meters (09 §5.2).
///
/// B1a binds the tunable caps to constitution params; the per-account cumulative
/// meter's storage home is B1a. Implementations reject without writes (G-1).
pub trait InflowCaps<AccountId> {
    /// Global gate, checked at the MINT leg (`ReserveAssetDeposited` → `mint_asset`),
    /// before any local credit exists: `Err` fails the message's first instruction, so
    /// nothing is minted and nothing can be trapped locally (09 §5.2 "fail politely").
    #[allow(clippy::result_unit_err)]
    fn usdc_mint_admissible(amount: u128) -> Result<(), ()>;

    /// Per-account check-and-record at the deposit leg (beneficiary known only here).
    #[allow(clippy::result_unit_err)] // The seam intentionally exposes only admit/refuse to XCM.
    fn note_usdc_inflow(who: &AccountId, amount: u128) -> Result<(), ()>;
}

/// `TransactAsset` wrapper gating global USDC mint and per-account deposit (09 §5.2).
///
/// A per-account rejection can still trap the already-minted holding because the
/// beneficiary is unknowable at mint time. The global mint gate is the primary
/// fail-politely path; any deposit-leg trap follows 09 §6.1's self-scoped
/// `claim_assets` recovery rules.
pub struct CappedInflows<Inner, Caps, LocationToAccountId, AccountId>(
    PhantomData<(Inner, Caps, LocationToAccountId, AccountId)>,
);

enum CapTransactionError {
    CapExceeded,
    Inner(XcmError),
    Storage,
}

impl From<DispatchError> for CapTransactionError {
    fn from(_error: DispatchError) -> Self {
        Self::Storage
    }
}

impl<Inner, Caps, LocationToAccountId, AccountId>
    CappedInflows<Inner, Caps, LocationToAccountId, AccountId>
where
    Inner: TransactAsset,
    Caps: InflowCaps<AccountId>,
    LocationToAccountId: ConvertLocation<AccountId>,
{
    fn usdc_amount(assets: &AssetsInHolding) -> Result<u128, XcmError> {
        assets
            .fungible_assets_iter()
            .filter_map(|asset| match (asset.id, asset.fun) {
                (AssetId(id), Fungibility::Fungible(amount)) if id == usdc_location() => {
                    Some(amount)
                }
                _ => None,
            })
            .try_fold(0_u128, |total, amount| {
                total.checked_add(amount).ok_or(XcmError::Overflow)
            })
    }

    fn account(who: &Location) -> Result<AccountId, XcmError> {
        LocationToAccountId::convert_location(who).ok_or(XcmError::FailedToTransactAsset(
            "beneficiary conversion failed",
        ))
    }

    /// Keep the meter write and the beneficiary credit in one nested transaction. The executor's
    /// deposit retry catches the first inner error, so relying only on its outer transaction would
    /// otherwise double-record a credit on the second attempt (09 §5.2; G-1).
    fn capped_deposit<R>(
        what: AssetsInHolding,
        account: &AccountId,
        amount: u128,
        deposit: impl FnOnce(AssetsInHolding) -> Result<R, (AssetsInHolding, XcmError)>,
    ) -> Result<R, (AssetsInHolding, XcmError)> {
        let mut pending = Some(what);
        let mut deposit = Some(deposit);
        let result: Result<R, CapTransactionError> = with_transaction(|| {
            if Caps::note_usdc_inflow(account, amount).is_err() {
                return TransactionOutcome::Rollback(Err(CapTransactionError::CapExceeded));
            }
            let Some(input) = pending.take() else {
                return TransactionOutcome::Rollback(Err(CapTransactionError::Storage));
            };
            let Some(inner_deposit) = deposit.take() else {
                pending = Some(input);
                return TransactionOutcome::Rollback(Err(CapTransactionError::Storage));
            };
            match inner_deposit(input) {
                Ok(value) => TransactionOutcome::Commit(Ok(value)),
                Err((unspent, error)) => {
                    pending = Some(unspent);
                    TransactionOutcome::Rollback(Err(CapTransactionError::Inner(error)))
                }
            }
        });

        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                let unspent = pending.unwrap_or_else(AssetsInHolding::new);
                let xcm_error = match error {
                    CapTransactionError::CapExceeded => {
                        XcmError::FailedToTransactAsset("USDC inflow cap exceeded")
                    }
                    CapTransactionError::Inner(error) => error,
                    CapTransactionError::Storage => {
                        XcmError::FailedToTransactAsset("USDC cap transaction unavailable")
                    }
                };
                Err((unspent, xcm_error))
            }
        }
    }
}

impl<Inner, Caps, LocationToAccountId, AccountId> TransactAsset
    for CappedInflows<Inner, Caps, LocationToAccountId, AccountId>
where
    Inner: TransactAsset,
    Caps: InflowCaps<AccountId>,
    LocationToAccountId: ConvertLocation<AccountId>,
{
    fn can_check_in(origin: &Location, what: &Asset, context: &XcmContext) -> Result<(), XcmError> {
        Inner::can_check_in(origin, what, context)
    }

    fn check_in(origin: &Location, what: &Asset, context: &XcmContext) {
        Inner::check_in(origin, what, context);
    }

    fn can_check_out(dest: &Location, what: &Asset, context: &XcmContext) -> Result<(), XcmError> {
        Inner::can_check_out(dest, what, context)
    }

    fn check_out(dest: &Location, what: &Asset, context: &XcmContext) {
        Inner::check_out(dest, what, context);
    }

    fn deposit_asset(
        what: AssetsInHolding,
        who: &Location,
        context: Option<&XcmContext>,
    ) -> Result<(), (AssetsInHolding, XcmError)> {
        let amount = match Self::usdc_amount(&what) {
            Ok(amount) => amount,
            Err(error) => return Err((what, error)),
        };
        if amount == 0 {
            return Inner::deposit_asset(what, who, context);
        }
        let account = match Self::account(who) {
            Ok(account) => account,
            Err(error) => return Err((what, error)),
        };

        Self::capped_deposit(what, &account, amount, |what| {
            Inner::deposit_asset(what, who, context)
        })
    }

    fn deposit_asset_with_surplus(
        what: AssetsInHolding,
        who: &Location,
        context: Option<&XcmContext>,
    ) -> Result<Weight, (AssetsInHolding, XcmError)> {
        let amount = match Self::usdc_amount(&what) {
            Ok(amount) => amount,
            Err(error) => return Err((what, error)),
        };
        if amount == 0 {
            return Inner::deposit_asset_with_surplus(what, who, context);
        }
        let account = match Self::account(who) {
            Ok(account) => account,
            Err(error) => return Err((what, error)),
        };

        Self::capped_deposit(what, &account, amount, |what| {
            Inner::deposit_asset_with_surplus(what, who, context)
        })
    }

    fn withdraw_asset(
        what: &Asset,
        who: &Location,
        context: Option<&XcmContext>,
    ) -> Result<AssetsInHolding, XcmError> {
        Inner::withdraw_asset(what, who, context)
    }

    fn withdraw_asset_with_surplus(
        what: &Asset,
        who: &Location,
        context: Option<&XcmContext>,
    ) -> Result<(AssetsInHolding, Weight), XcmError> {
        Inner::withdraw_asset_with_surplus(what, who, context)
    }

    fn internal_transfer_asset(
        asset: &Asset,
        from: &Location,
        to: &Location,
        context: &XcmContext,
    ) -> Result<Asset, XcmError> {
        Inner::internal_transfer_asset(asset, from, to, context)
    }

    fn internal_transfer_asset_with_surplus(
        asset: &Asset,
        from: &Location,
        to: &Location,
        context: &XcmContext,
    ) -> Result<(Asset, Weight), XcmError> {
        Inner::internal_transfer_asset_with_surplus(asset, from, to, context)
    }

    fn transfer_asset(
        asset: &Asset,
        from: &Location,
        to: &Location,
        context: &XcmContext,
    ) -> Result<Asset, XcmError> {
        Inner::transfer_asset(asset, from, to, context)
    }

    fn transfer_asset_with_surplus(
        asset: &Asset,
        from: &Location,
        to: &Location,
        context: &XcmContext,
    ) -> Result<(Asset, Weight), XcmError> {
        Inner::transfer_asset_with_surplus(asset, from, to, context)
    }

    fn mint_asset(what: &Asset, context: &XcmContext) -> Result<AssetsInHolding, XcmError> {
        if let (AssetId(id), Fungibility::Fungible(amount)) = (&what.id, &what.fun) {
            if id == &usdc_location() && Caps::usdc_mint_admissible(*amount).is_err() {
                return Err(XcmError::FailedToTransactAsset(
                    "USDC global inflow cap exceeded",
                ));
            }
        }
        // The check precedes the inner mint. In a multi-mint message, each
        // subsequent instruction therefore observes issuance committed by the
        // previous successful mint (09 §5.2).
        Inner::mint_asset(what, context)
    }
}
