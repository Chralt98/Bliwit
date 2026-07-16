use alloc::{vec, vec::Vec};

use cumulus_primitives_core::ParaId;
use frame_support::build_struct_json_patch;
use serde_json::Value;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_genesis_builder::PresetId;
use sp_runtime::traits::AccountIdConversion;

use crate::{
    configs::LedgerPalletId, AccountId, BalancesConfig, CollatorSelectionConfig,
    ConstitutionConfig, ForeignAssetsConfig, ParachainInfoConfig, PolkadotXcmConfig,
    RuntimeGenesisConfig, SessionConfig, SessionKeys, SudoConfig, USDC_ASSET_ID,
};

const SAFE_XCM_VERSION: u32 = staging_xcm::prelude::XCM_VERSION;
/// Phase-0 bootstrap phase-flags (02 §7.3): shadow mode on, sudo present. The
/// preset installs a sudo key, so bit 4 (`SUDO_PRESENT`) MUST be set — the FE
/// binds its persistent "bootstrap governance (sudo active)" banner to it
/// (09 §5.2); leaving it clear would misrepresent sudo-era state as
/// trust-equivalent to post-sudo state.
const BOOTSTRAP_PHASE_FLAGS: u32 = pallet_constitution::PhaseFlagsValue::SHADOW_MODE
    | pallet_constitution::PhaseFlagsValue::SUDO_PRESENT;
const ALICE_PUBLIC: [u8; 32] = [
    0xd4, 0x35, 0x93, 0xc7, 0x15, 0xfd, 0xd3, 0x1c, 0x61, 0x14, 0x1a, 0xbd, 0x04, 0xa9, 0x9f, 0xd6,
    0x82, 0x2c, 0x85, 0x58, 0x85, 0x4c, 0xcd, 0xe3, 0x9a, 0x56, 0x84, 0xe7, 0xa5, 0x6d, 0xa2, 0x7d,
];
const BOB_PUBLIC: [u8; 32] = [
    0x8e, 0xaf, 0x04, 0x15, 0x16, 0x87, 0x73, 0x63, 0x26, 0xc9, 0xfe, 0xa1, 0x7e, 0x25, 0xfc, 0x52,
    0x87, 0x61, 0x36, 0x93, 0xc9, 0x12, 0x90, 0x9c, 0xb2, 0x26, 0xaa, 0x47, 0x94, 0xf2, 0x6a, 0x48,
];

pub fn session_keys(aura: AuraId) -> SessionKeys {
    SessionKeys { aura }
}

fn testnet_genesis(
    invulnerables: Vec<(AccountId, AuraId)>,
    root: AccountId,
    para_id: ParaId,
) -> Value {
    let alice = AccountId::new(ALICE_PUBLIC);
    let bob = AccountId::new(BOB_PUBLIC);
    let half = futarchy_primitives::currency::VIT_TOTAL_SUPPLY / 2;
    let owner = LedgerPalletId::get().into_account_truncating();

    build_struct_json_patch!(RuntimeGenesisConfig {
        balances: BalancesConfig {
            balances: vec![
                (alice, half),
                (bob, futarchy_primitives::currency::VIT_TOTAL_SUPPLY - half),
            ],
        },
        foreign_assets: ForeignAssetsConfig {
            assets: vec![(
                USDC_ASSET_ID,
                owner,
                true,
                futarchy_primitives::currency::USDC_CENT,
            )],
            metadata: vec![(
                USDC_ASSET_ID,
                b"USD Coin".to_vec(),
                b"USDC".to_vec(),
                futarchy_primitives::currency::USDC_DECIMALS
            )],
            accounts: vec![],
            next_asset_id: None,
            reserves: vec![],
        },
        parachain_info: ParachainInfoConfig {
            parachain_id: para_id
        },
        collator_selection: CollatorSelectionConfig {
            invulnerables: invulnerables
                .iter()
                .map(|(account, _)| account.clone())
                .collect::<Vec<_>>(),
            candidacy_bond: futarchy_primitives::currency::VIT_EXISTENTIAL_DEPOSIT * 16,
            desired_candidates: 2,
        },
        session: SessionConfig {
            keys: invulnerables
                .into_iter()
                .map(|(account, aura)| { (account.clone(), account, session_keys(aura)) })
                .collect::<Vec<_>>(),
        },
        polkadot_xcm: PolkadotXcmConfig {
            safe_xcm_version: Some(SAFE_XCM_VERSION)
        },
        constitution: ConstitutionConfig {
            phase_flags: BOOTSTRAP_PHASE_FLAGS,
            ..Default::default()
        },
        sudo: SudoConfig { key: Some(root) },
    })
}

fn development_genesis() -> Value {
    let alice = AccountId::new(ALICE_PUBLIC);
    let bob = AccountId::new(BOB_PUBLIC);
    testnet_genesis(
        vec![
            (
                alice.clone(),
                AuraId::from(sp_core::sr25519::Public::from_raw(ALICE_PUBLIC)),
            ),
            (
                bob.clone(),
                AuraId::from(sp_core::sr25519::Public::from_raw(BOB_PUBLIC)),
            ),
        ],
        alice,
        futarchy_primitives::chain_identity::FIXTURE_PARA_ID.into(),
    )
}

fn local_testnet_genesis() -> Value {
    development_genesis()
}

pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
    let patch = match id.as_ref() {
        sp_genesis_builder::DEV_RUNTIME_PRESET => development_genesis(),
        sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => local_testnet_genesis(),
        _ => return None,
    };
    match serde_json::to_string(&patch) {
        Ok(json) => Some(json.into_bytes()),
        Err(_) => None,
    }
}

pub fn preset_names() -> Vec<PresetId> {
    vec![
        PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET),
        PresetId::from(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET),
    ]
}
