//! Pinned foreign-asset matching and reserve policy (02 §7.4/§8; 09 §6.1).

use cumulus_primitives_core::ParaId;
use frame_support::traits::{Contains, ContainsPair, Get};
use sp_runtime::traits::Identity;
use staging_xcm::latest::{Asset, AssetId, Fungibility, Location, NetworkId};
use staging_xcm_builder::{
    AccountId32Aliases, FungiblesAdapter, MatchedConvertedConcreteId, NoChecking, ParentIsPreset,
    SiblingParachainConvertsVia,
};

use crate::identity::{asset_hub_location, dot_location, relay_location, usdc_location};

/// Matches exactly the two v1 fungible assets; all other identifiers are refused (09 §6.1).
pub struct SupportedAssetLocations;

impl Contains<Location> for SupportedAssetLocations {
    fn contains(location: &Location) -> bool {
        location == &usdc_location() || location == &dot_location()
    }
}

/// Converts each admitted concrete location to the same `ForeignAssets` `Location` key
/// (02 §7.4; 09 §6.1).
pub type PinnedAssetMatcher =
    MatchedConvertedConcreteId<Location, u128, SupportedAssetLocations, Identity, Identity>;

/// Standard sovereign/sibling/account converter stack without a superuser conversion (09 §6.1).
pub type StandardLocationToAccountId<AccountId, RelayNetwork> = (
    ParentIsPreset<AccountId>,
    SiblingParachainConvertsVia<ParaId, AccountId>,
    AccountId32Aliases<RelayNetwork, AccountId>,
);

/// `ForeignAssets` transactor for the two location-keyed v1 assets (02 §7.4; 09 §6.1).
pub type AssetTransactors<ForeignAssets, LocationToAccountId, AccountId, CheckingAccount> =
    FungiblesAdapter<
        ForeignAssets,
        PinnedAssetMatcher,
        LocationToAccountId,
        AccountId,
        NoChecking,
        CheckingAccount,
    >;

/// Reserve recognition is explicit: USDC only on Asset Hub; DOT on relay or Asset Hub (09 §6.1).
pub struct BleavitReserves;

impl ContainsPair<Asset, Location> for BleavitReserves {
    fn contains(asset: &Asset, location: &Location) -> bool {
        match (&asset.id, &asset.fun) {
            (AssetId(id), Fungibility::Fungible(_)) if id == &usdc_location() => {
                location == &asset_hub_location()
            }
            (AssetId(id), Fungibility::Fungible(_)) if id == &dot_location() => {
                location == &relay_location() || location == &asset_hub_location()
            }
            _ => false,
        }
    }
}

/// Teleports are disabled by binding `XcmExecutor::IsTeleporter` to this type (09 §6.1).
pub type NoTeleporters = ();

// These bounds are deliberately represented by the SDK's converter types; keep the imports live
// so rustdoc shows the expected B1a seam rather than a bespoke account conversion.
#[allow(dead_code)]
struct StandardConverterBounds<N: Get<Option<NetworkId>>>(PhantomData<N>);

use core::marker::PhantomData;
