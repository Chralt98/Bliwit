use alloc::{vec, vec::Vec};

use cumulus_primitives_core::ParaId;
use frame_support::build_struct_json_patch;
use serde_json::Value;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_genesis_builder::PresetId;
use sp_runtime::traits::AccountIdConversion;

use crate::{
    configs::{LedgerPalletId, TreasuryPalletId},
    usdc_location, AccountId, Balance, BalancesConfig, CollatorSelectionConfig, ConstitutionConfig,
    EpochConfig, ExecutionGuardConfig, ForeignAssetsConfig, ParachainInfoConfig, PolkadotXcmConfig,
    RuntimeGenesisConfig, SessionConfig, SessionKeys, SudoConfig, VestingConfig,
    MILLISECS_PER_BLOCK,
};

const SAFE_XCM_VERSION: u32 = staging_xcm::prelude::XCM_VERSION;
/// Phase-0 bootstrap phase-flags (02 §7.3): shadow mode on, sudo present. The
/// preset installs a sudo key, so bit 4 (`SUDO_PRESENT`) MUST be set — the FE
/// binds its persistent "bootstrap governance (sudo active)" banner to it
/// (09 §5.2); leaving it clear would misrepresent sudo-era state as
/// trust-equivalent to post-sudo state.
const BOOTSTRAP_PHASE_FLAGS: u32 = pallet_constitution::PhaseFlagsValue::SHADOW_MODE
    | pallet_constitution::PhaseFlagsValue::SUDO_PRESENT;
pub const ALICE_PUBLIC: [u8; 32] = [
    0xd4, 0x35, 0x93, 0xc7, 0x15, 0xfd, 0xd3, 0x1c, 0x61, 0x14, 0x1a, 0xbd, 0x04, 0xa9, 0x9f, 0xd6,
    0x82, 0x2c, 0x85, 0x58, 0x85, 0x4c, 0xcd, 0xe3, 0x9a, 0x56, 0x84, 0xe7, 0xa5, 0x6d, 0xa2, 0x7d,
];
pub const BOB_PUBLIC: [u8; 32] = [
    0x8e, 0xaf, 0x04, 0x15, 0x16, 0x87, 0x73, 0x63, 0x26, 0xc9, 0xfe, 0xa1, 0x7e, 0x25, 0xfc, 0x52,
    0x87, 0x61, 0x36, 0x93, 0xc9, 0x12, 0x90, 0x9c, 0xb2, 0x26, 0xaa, 0x47, 0x94, 0xf2, 0x6a, 0x48,
];
pub const CHARLIE_PUBLIC: [u8; 32] = [
    0x90, 0xb5, 0xab, 0x20, 0x5c, 0x69, 0x74, 0xc9, 0xea, 0x84, 0x1b, 0xe6, 0x88, 0x86, 0x46, 0x33,
    0xdc, 0x9c, 0xa8, 0xa3, 0x57, 0x84, 0x3e, 0xea, 0xcf, 0x23, 0x14, 0x64, 0x99, 0x65, 0xfe, 0x22,
];
pub const DAVE_PUBLIC: [u8; 32] = [
    0x30, 0x67, 0x21, 0x21, 0x1d, 0x54, 0x04, 0xbd, 0x9d, 0xa8, 0x8e, 0x02, 0x04, 0x36, 0x0a, 0x1a,
    0x9a, 0xb8, 0xb8, 0x7c, 0x66, 0xc1, 0xbc, 0x2f, 0xcd, 0xd3, 0x7f, 0x3c, 0x22, 0x22, 0xcc, 0x20,
];

pub const TREASURY_RESERVE: Balance = 300_000_000 * futarchy_primitives::currency::VIT;
pub const COMMUNITY_DISTRIBUTION: Balance = 250_000_000 * futarchy_primitives::currency::VIT;
pub const FOUNDING_TEAM: Balance = 200_000_000 * futarchy_primitives::currency::VIT;
pub const ECOSYSTEM_OPS: Balance = 150_000_000 * futarchy_primitives::currency::VIT;
pub const INCENTIVE_PROGRAMS: Balance = 100_000_000 * futarchy_primitives::currency::VIT;
pub const FOUNDING_TEAM_ACCOUNT: Balance = FOUNDING_TEAM / 2;
pub const ECOSYSTEM_OPS_ACCOUNT: Balance = ECOSYSTEM_OPS / 2;
pub const BLOCKS_PER_YEAR: u32 = ((365_u64 * 24 * 60 * 60 * 1_000) / MILLISECS_PER_BLOCK) as u32;

const _: () = assert!(
    TREASURY_RESERVE + COMMUNITY_DISTRIBUTION + FOUNDING_TEAM + ECOSYSTEM_OPS + INCENTIVE_PROGRAMS
        == futarchy_primitives::currency::VIT_TOTAL_SUPPLY
);

/// 08 §2.1 treasury-reserve holder (`MAIN`); VIT remains marked zero in NAV.
pub fn treasury_account() -> AccountId {
    TreasuryPalletId::get().into_account_truncating()
}

/// 08 §2.1 community-distribution pot, held until Phase-4 arming. Its
/// 24-month distribution cannot start as a genesis schedule because the
/// Phase-4 arming block is unknowable at genesis.
pub fn community_account() -> AccountId {
    TreasuryPalletId::get().into_sub_account_truncating(b"communty")
}

/// 08 §2.1 Phase 3-4 incentive-program pot.
pub fn incentives_account() -> AccountId {
    TreasuryPalletId::get().into_sub_account_truncating(b"incentiv")
}

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
    let charlie = AccountId::new(CHARLIE_PUBLIC);
    let dave = AccountId::new(DAVE_PUBLIC);
    let owner = LedgerPalletId::get().into_account_truncating();

    build_struct_json_patch!(RuntimeGenesisConfig {
        balances: BalancesConfig {
            balances: vec![
                // 08 §2.1 ecosystem/ops fund: dev stand-ins for the ops multisig.
                (alice, ECOSYSTEM_OPS_ACCOUNT),
                (bob, ECOSYSTEM_OPS_ACCOUNT),
                // 08 §2.1 founding team: fully locked by the schedules below.
                (charlie.clone(), FOUNDING_TEAM_ACCOUNT),
                (dave.clone(), FOUNDING_TEAM_ACCOUNT),
                (treasury_account(), TREASURY_RESERVE),
                (community_account(), COMMUNITY_DISTRIBUTION),
                (incentives_account(), INCENTIVE_PROGRAMS),
            ],
        },
        vesting: VestingConfig {
            // 08 §2.1's four-year linear vest with one-year cliff is represented
            // conservatively by one schedule: no unlock before year one, then
            // linear unlock through year four. A two-schedule catch-up curve is
            // unsafe here because genesis set_lock replaces rather than adds;
            // the cliff tranche could otherwise be spendable until the first vest().
            vesting: vec![
                (charlie, BLOCKS_PER_YEAR, 3 * BLOCKS_PER_YEAR, 0),
                (dave, BLOCKS_PER_YEAR, 3 * BLOCKS_PER_YEAR, 0),
            ],
        },
        foreign_assets: ForeignAssetsConfig {
            assets: vec![(
                usdc_location(),
                owner,
                true,
                futarchy_primitives::currency::USDC_CENT,
            )],
            metadata: vec![(
                usdc_location(),
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
        epoch: EpochConfig {
            index: 1,
            start_block: 0,
            ..Default::default()
        },
        // Seeds CurrentSpecName from the live RuntimeVersion; all other guard
        // state is empty until the epoch lawfully enqueues a passed proposal.
        execution_guard: ExecutionGuardConfig::default(),

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
