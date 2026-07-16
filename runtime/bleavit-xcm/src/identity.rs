//! Pinned XCM identity constructors (02 §8; 09 §6.1).

use futarchy_primitives::chain_identity::{
    ASSET_HUB_PARA_ID, CORETIME_PARA_ID, USDC_ASSET_INDEX, USDC_PALLET_INSTANCE,
};
use staging_xcm::latest::{Junction, Location};

/// XCM v5 is the stable2603 wire-version pin; negotiation remains enabled (09 §6.1).
pub const XCM_VERSION_PINNED: u32 = 5;

/// The canonical USDC identifier as seen from Bleavit (02 §8; 09 §6.1).
pub fn usdc_location() -> Location {
    Location::new(
        1,
        [
            Junction::Parachain(ASSET_HUB_PARA_ID),
            Junction::PalletInstance(USDC_PALLET_INSTANCE),
            Junction::GeneralIndex(USDC_ASSET_INDEX),
        ],
    )
}

/// The canonical DOT identifier as seen from Bleavit (09 §6.1).
pub fn dot_location() -> Location {
    Location::parent()
}

/// Asset Hub as a sibling of Bleavit (02 §8; 09 §6.1).
pub fn asset_hub_location() -> Location {
    Location::new(1, [Junction::Parachain(ASSET_HUB_PARA_ID)])
}

/// The relay chain as seen from Bleavit (09 §6.1).
pub fn relay_location() -> Location {
    Location::parent()
}

/// The Coretime chain as a sibling of Bleavit (09 §4; 09 §6.1).
pub fn coretime_location() -> Location {
    Location::new(1, [Junction::Parachain(CORETIME_PARA_ID)])
}

/// USDC's local Asset Hub identifier, for programs executed on Asset Hub (07 §8).
pub fn usdc_on_asset_hub_location() -> Location {
    Location::new(
        0,
        [
            Junction::PalletInstance(USDC_PALLET_INSTANCE),
            Junction::GeneralIndex(USDC_ASSET_INDEX),
        ],
    )
}

/// Bleavit's chain location as seen from Asset Hub (07 §8).
///
/// The location serves two related roles in the probe: it is the `ReportError`
/// destination, and converting it as an Asset Hub beneficiary yields Bleavit's
/// sovereign account there.
pub fn bleavit_as_seen_from_asset_hub(our_para_id: u32) -> Location {
    Location::new(1, [Junction::Parachain(our_para_id)])
}

/// The Coretime-chain account funded for broker renewal (09 §4).
pub fn renewal_account_location(account: [u8; 32]) -> Location {
    Location::new(
        0,
        [Junction::AccountId32 {
            network: None,
            id: account,
        }],
    )
}
