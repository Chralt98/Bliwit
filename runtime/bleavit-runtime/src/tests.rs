//! Runtime-level B1a composition and safety-filter regression suite.

#![allow(clippy::assertions_on_constants, clippy::manual_unwrap_or_default)]

use alloc::{boxed::Box, vec, vec::Vec};

use frame_support::{
    dispatch::GetDispatchInfo,
    traits::{
        fungible::Inspect as FungibleInspect, fungibles::Inspect as FungiblesInspect,
        tokens::ConversionToAssetBalance, Contains, EnsureOrigin, PalletInfo, PalletsInfoAccess,
    },
    weights::Weight,
};
use futarchy_primitives::{chain_identity, currency, kernel, ProposalClass};
use origins_core::Origin as ClassOrigin;
use pallet_guardian::WeightInfo as GuardianWeightInfo;
use parity_scale_codec::Encode;
use sp_core::H256;
use sp_genesis_builder::PresetId;
use sp_inherents::InherentData;
use sp_keyring::Sr25519Keyring;
use sp_runtime::{
    generic::{Era, SignedPayload},
    traits::{Block as BlockT, Dispatchable, Header as HeaderT},
    BuildStorage, DispatchError, MultiAddress, MultiSignature,
};

use crate::{
    classifier::{set_test_applicable_at, RuntimeBaseCallFilter},
    AccountId, AllPalletsWithSystem, AssetTxPayment, Attestor, Aura, AuraExt, Authorship, Balances,
    BlockNumber, CollatorSelection, ConditionalLedger, Constitution, ConvictionVoting, CumulusXcm,
    ForeignAssets, FutarchyTreasury, Guardian, IncidentRegistry, Market, MessageQueue, Migrations,
    MilestoneRegistry, Multisig, Oracle, Origins, PalletInfo as RuntimePalletInfo, ParachainInfo,
    ParachainSystem, PolkadotXcm, Preimage, Proxy, Referenda, Runtime, RuntimeCall,
    RuntimeGenesisConfig, RuntimeOrigin, Scheduler, Session, Sudo, System, Timestamp,
    TransactionPayment, TxExtension, UncheckedExtrinsic, Utility, Welfare, XcmpQueue,
    FEE_VIT_USDC_RATE_KEY, MILLISECS_PER_BLOCK, SS58_PREFIX, USDC_ASSET_ID, USDC_DECIMALS,
    USDC_LOCATION, VERSION, VIT_DECIMALS,
};

fn account(seed: u8) -> AccountId {
    AccountId::new([seed; 32])
}

fn merge_json(base: &mut serde_json::Value, patch: serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(base), serde_json::Value::Object(patch)) => {
            for (key, value) in patch {
                match base.get_mut(&key) {
                    Some(slot) => merge_json(slot, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, patch) => *base = patch,
    }
}

fn development_ext() -> sp_io::TestExternalities {
    let preset =
        match crate::genesis::get_preset(&PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET)) {
            Some(bytes) => bytes,
            None => Vec::new(),
        };
    assert!(!preset.is_empty());
    let mut merged = match serde_json::to_value(RuntimeGenesisConfig::default()) {
        Ok(value) => value,
        Err(error) => {
            assert!(false, "default genesis must encode: {error}");
            serde_json::Value::Null
        }
    };
    let patch = match serde_json::from_slice::<serde_json::Value>(&preset) {
        Ok(value) => value,
        Err(error) => {
            assert!(false, "development preset patch must decode: {error}");
            serde_json::Value::Null
        }
    };
    merge_json(&mut merged, patch);
    let config = match serde_json::from_value::<RuntimeGenesisConfig>(merged) {
        Ok(config) => config,
        Err(error) => {
            assert!(false, "development preset must decode: {error}");
            RuntimeGenesisConfig::default()
        }
    };
    let storage = match config.build_storage() {
        Ok(storage) => storage,
        Err(error) => {
            assert!(false, "development preset must build: {error}");
            Default::default()
        }
    };
    sp_io::TestExternalities::new(storage)
}

fn remark() -> RuntimeCall {
    RuntimeCall::System(frame_system::Call::remark { remark: vec![1] })
}

fn nobody_system_calls() -> Vec<RuntimeCall> {
    vec![
        RuntimeCall::System(frame_system::Call::set_heap_pages { pages: 64 }),
        RuntimeCall::System(frame_system::Call::set_code { code: vec![1] }),
        RuntimeCall::System(frame_system::Call::set_code_without_checks { code: vec![1] }),
        RuntimeCall::System(frame_system::Call::set_storage {
            items: vec![(vec![1], vec![2])],
        }),
        RuntimeCall::System(frame_system::Call::kill_storage {
            keys: vec![vec![1]],
        }),
        RuntimeCall::System(frame_system::Call::kill_prefix {
            prefix: vec![1],
            subkeys: 1,
        }),
        RuntimeCall::System(frame_system::Call::authorize_upgrade_without_checks {
            code_hash: H256::repeat_byte(9),
        }),
    ]
}

fn closed_wrappers(call: RuntimeCall) -> Vec<RuntimeCall> {
    let who = account(7);
    vec![
        RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![call.clone()],
        }),
        RuntimeCall::Utility(pallet_utility::Call::batch_all {
            calls: vec![call.clone()],
        }),
        RuntimeCall::Utility(pallet_utility::Call::force_batch {
            calls: vec![call.clone()],
        }),
        RuntimeCall::Proxy(pallet_proxy::Call::proxy {
            real: MultiAddress::Id(who.clone()),
            force_proxy_type: None,
            call: Box::new(call.clone()),
        }),
        RuntimeCall::Proxy(pallet_proxy::Call::proxy_announced {
            delegate: MultiAddress::Id(who.clone()),
            real: MultiAddress::Id(account(8)),
            force_proxy_type: None,
            call: Box::new(call.clone()),
        }),
        RuntimeCall::Multisig(pallet_multisig::Call::as_multi {
            threshold: 2,
            other_signatories: vec![who.clone()],
            maybe_timepoint: None,
            call: Box::new(call.clone()),
            max_weight: Weight::zero(),
        }),
        RuntimeCall::Multisig(pallet_multisig::Call::as_multi_threshold_1 {
            other_signatories: vec![who.clone()],
            call: Box::new(call.clone()),
        }),
        RuntimeCall::Sudo(pallet_sudo::Call::sudo {
            call: Box::new(call.clone()),
        }),
        RuntimeCall::Sudo(pallet_sudo::Call::sudo_unchecked_weight {
            call: Box::new(call.clone()),
            weight: Weight::zero(),
        }),
        RuntimeCall::Sudo(pallet_sudo::Call::sudo_as {
            who: MultiAddress::Id(who),
            call: Box::new(call),
        }),
    ]
}

fn signed_vit_transfer(destination: AccountId, amount: crate::Balance) -> UncheckedExtrinsic {
    let call = RuntimeCall::Balances(pallet_balances::Call::transfer_allow_death {
        dest: MultiAddress::Id(destination),
        value: amount,
    });
    let extensions: TxExtension = (
        frame_system::AuthorizeCall::<Runtime>::new(),
        frame_system::CheckNonZeroSender::<Runtime>::new(),
        frame_system::CheckSpecVersion::<Runtime>::new(),
        frame_system::CheckTxVersion::<Runtime>::new(),
        frame_system::CheckGenesis::<Runtime>::new(),
        frame_system::CheckEra::<Runtime>::from(Era::Immortal),
        frame_system::CheckNonce::<Runtime>::from(0),
        frame_system::CheckWeight::<Runtime>::new(),
        pallet_asset_tx_payment::ChargeAssetTxPayment::<Runtime>::from(0, None),
        (
            frame_metadata_hash_extension::CheckMetadataHash::<Runtime>::new(false),
            crate::StorageWeightReclaim::new(),
        ),
    );
    let payload = match SignedPayload::new(call, extensions) {
        Ok(payload) => payload,
        Err(error) => {
            assert!(false, "signed payload must be constructible: {error:?}");
            return UncheckedExtrinsic::new_bare(remark());
        }
    };
    let signature = payload.using_encoded(|bytes| Sr25519Keyring::Alice.sign(bytes));
    let (call, extensions, _) = payload.deconstruct();
    UncheckedExtrinsic::new_signed(
        call,
        MultiAddress::Id(Sr25519Keyring::Alice.to_account_id()),
        MultiSignature::Sr25519(signature),
        extensions,
    )
}

fn build_executive_smoke_block(destination: AccountId) -> crate::Block {
    let builder = cumulus_test_relay_sproof_builder::RelayStateSproofBuilder {
        para_id: futarchy_primitives::chain_identity::FIXTURE_PARA_ID.into(),
        current_slot: 1u64.into(),
        included_para_head: Some(cumulus_primitives_core::relay_chain::HeadData(Vec::new())),
        ..Default::default()
    };
    let (relay_parent_storage_root, relay_chain_state) = builder.into_state_root_and_proof();
    let validation_data = cumulus_primitives_core::PersistedValidationData {
        relay_parent_number: 1,
        relay_parent_storage_root,
        ..Default::default()
    };
    let parachain_data = cumulus_primitives_parachain_inherent::ParachainInherentData {
        validation_data,
        relay_chain_state,
        downward_messages: Default::default(),
        horizontal_messages: Default::default(),
        relay_parent_descendants: Default::default(),
        collator_peer_id: None,
    };
    let mut inherent_data = InherentData::new();
    assert!(inherent_data
        .put_data(*b"timstap0", &kernel::MILLISECS_PER_BLOCK)
        .is_ok());
    assert!(inherent_data
        .put_data(
            cumulus_primitives_parachain_inherent::INHERENT_IDENTIFIER,
            &parachain_data,
        )
        .is_ok());
    let mut extrinsics = crate::InherentDataExt::create_extrinsics(&inherent_data);
    assert_eq!(extrinsics.len(), 2);
    extrinsics.push(signed_vit_transfer(
        destination,
        currency::VIT_EXISTENTIAL_DEPOSIT,
    ));

    let header = crate::Header::new(
        1,
        Default::default(),
        Default::default(),
        System::block_hash(0),
        sp_runtime::Digest {
            logs: vec![sp_runtime::DigestItem::PreRuntime(
                sp_consensus_aura::AURA_ENGINE_ID,
                1u64.encode(),
            )],
        },
    );
    crate::Executive::initialize_block(&header);
    for extrinsic in extrinsics.iter().cloned() {
        assert!(crate::Executive::apply_extrinsic(extrinsic).is_ok());
    }
    let finalized = crate::Executive::finalize_block();
    crate::Block::new(finalized, extrinsics)
}

#[test]
fn composition_contains_all_b1a_pallets_and_only_future_slots_are_absent() {
    macro_rules! assert_pallet {
        ($pallet:ty, $index:expr, $name:expr) => {{
            assert_eq!(
                <RuntimePalletInfo as PalletInfo>::index::<$pallet>(),
                Some($index)
            );
            assert_eq!(
                <RuntimePalletInfo as PalletInfo>::name::<$pallet>(),
                Some($name)
            );
        }};
    }

    assert_pallet!(System, 0, "System");
    assert_pallet!(Timestamp, 1, "Timestamp");
    assert_pallet!(ParachainSystem, 2, "ParachainSystem");
    assert_pallet!(ParachainInfo, 3, "ParachainInfo");
    assert_pallet!(Balances, 10, "Balances");
    assert_pallet!(ForeignAssets, 11, "ForeignAssets");
    assert_pallet!(TransactionPayment, 12, "TransactionPayment");
    assert_pallet!(AssetTxPayment, 13, "AssetTxPayment");
    assert_pallet!(Referenda, 20, "Referenda");
    assert_pallet!(ConvictionVoting, 21, "ConvictionVoting");
    assert_pallet!(Preimage, 22, "Preimage");
    assert_pallet!(Scheduler, 23, "Scheduler");
    assert_pallet!(Utility, 24, "Utility");
    assert_pallet!(Proxy, 25, "Proxy");
    assert_pallet!(Multisig, 26, "Multisig");
    assert_pallet!(Migrations, 27, "Migrations");
    assert_pallet!(Sudo, 28, "Sudo");
    assert_pallet!(XcmpQueue, 30, "XcmpQueue");
    assert_pallet!(MessageQueue, 31, "MessageQueue");
    assert_pallet!(CumulusXcm, 32, "CumulusXcm");
    assert_pallet!(PolkadotXcm, 33, "PolkadotXcm");
    assert_pallet!(Authorship, 40, "Authorship");
    assert_pallet!(CollatorSelection, 41, "CollatorSelection");
    assert_pallet!(Session, 42, "Session");
    assert_pallet!(Aura, 43, "Aura");
    assert_pallet!(AuraExt, 44, "AuraExt");
    assert_pallet!(Origins, 50, "Origins");
    assert_pallet!(Constitution, 51, "Constitution");
    assert_pallet!(ConditionalLedger, 52, "ConditionalLedger");
    assert_pallet!(Market, 53, "Market");
    assert_pallet!(Welfare, 54, "Welfare");
    assert_pallet!(Oracle, 55, "Oracle");
    assert_pallet!(IncidentRegistry, 56, "IncidentRegistry");
    assert_pallet!(MilestoneRegistry, 57, "MilestoneRegistry");
    assert_pallet!(FutarchyTreasury, 58, "FutarchyTreasury");
    assert_pallet!(Guardian, 59, "Guardian");
    assert_pallet!(Attestor, 60, "Attestor");
    assert_eq!(
        <AllPalletsWithSystem as PalletsInfoAccess>::infos().len(),
        37
    );
}

#[test]
fn identity_and_version_pins_match_the_integration_contract() {
    assert_eq!(SS58_PREFIX, 7_777);
    assert_eq!(SS58_PREFIX, chain_identity::SS58_PREFIX);
    assert_eq!(MILLISECS_PER_BLOCK, kernel::MILLISECS_PER_BLOCK);
    assert_eq!(MILLISECS_PER_BLOCK, 6_000);
    assert_eq!(currency::VIT_EXISTENTIAL_DEPOSIT, 10_000_000_000);
    assert_eq!(VIT_DECIMALS, 12);
    assert_eq!(USDC_DECIMALS, 6);
    assert_eq!(USDC_ASSET_ID, 1_337);
    assert_eq!(FEE_VIT_USDC_RATE_KEY, *b"fee.vit_usdc\0\0\0\0");
    assert_eq!(VERSION.spec_name.as_ref(), "bleavit");
    assert_eq!(VERSION.impl_name.as_ref(), "bleavit-runtime");
    assert_eq!(VERSION.spec_version, 1);
    assert_eq!(
        VERSION.transaction_version,
        futarchy_primitives::INTEGRATION_CONTRACT_VERSION
    );
    assert_eq!(VERSION.transaction_version, 3);
    assert_eq!(
        USDC_LOCATION,
        [
            1, 0, 0, 0, 232, 3, 0, 0, 50, 0, 0, 0, 57, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]
    );
}

#[test]
fn usdc_admin_and_fee_posture_is_fail_closed() {
    let create = RuntimeCall::ForeignAssets(pallet_assets::Call::create {
        id: USDC_ASSET_ID,
        admin: MultiAddress::Id(account(1)),
        min_balance: currency::USDC_CENT,
    });
    let mint = RuntimeCall::ForeignAssets(pallet_assets::Call::mint {
        id: USDC_ASSET_ID,
        beneficiary: MultiAddress::Id(account(2)),
        amount: currency::USDC_CENT,
    });
    assert!(!RuntimeBaseCallFilter::contains(&create));
    assert!(RuntimeBaseCallFilter::contains_for(
        ClassOrigin::ConstitutionalValues,
        &create
    ));
    assert!(!RuntimeBaseCallFilter::contains(&mint));
    assert!(!RuntimeBaseCallFilter::contains_for(
        ClassOrigin::ConstitutionalValues,
        &mint
    ));

    development_ext().execute_with(|| {
        assert!(crate::configs::LiveFeeConversion::to_asset_balance(1, USDC_ASSET_ID).is_err());
        assert!(crate::configs::LiveFeeConversion::to_asset_balance(1, USDC_ASSET_ID + 1).is_err());
    });
}

#[test]
fn usdc_fee_conversion_scales_decimals_and_rounds_against_the_payer() {
    development_ext().execute_with(|| {
        pallet_constitution::Params::<Runtime>::insert(
            FEE_VIT_USDC_RATE_KEY,
            pallet_constitution::ParamRecord {
                key: FEE_VIT_USDC_RATE_KEY,
                value: pallet_constitution::ParamValue::Fixed(futarchy_primitives::FixedU64(
                    2_000_000_000,
                )),
                min: pallet_constitution::ParamValue::Fixed(futarchy_primitives::FixedU64(1)),
                max: pallet_constitution::ParamValue::Fixed(futarchy_primitives::FixedU64(
                    u64::MAX,
                )),
                max_delta: None,
                cooldown_epochs: 0,
                last_changed_epoch: 0,
                class: pallet_constitution::ParamClass::Treasury,
                kernel_bounded: false,
            },
        );
        assert_eq!(
            crate::configs::LiveFeeConversion::to_asset_balance(currency::VIT, USDC_ASSET_ID),
            Ok(2 * currency::USDC)
        );
        assert_eq!(
            crate::configs::LiveFeeConversion::to_asset_balance(1, USDC_ASSET_ID),
            Ok(1)
        );
        assert_eq!(
            crate::configs::LiveFeeConversion::to_asset_balance(0, USDC_ASSET_ID),
            Ok(0)
        );
    });
}

#[test]
fn development_preset_builds_and_pins_usdc_and_para_identity() {
    development_ext().execute_with(|| {
        assert_eq!(
            u32::from(ParachainInfo::parachain_id()),
            chain_identity::FIXTURE_PARA_ID
        );
        assert!(ForeignAssets::asset_exists(USDC_ASSET_ID));
        assert_eq!(
            ForeignAssets::minimum_balance(USDC_ASSET_ID),
            currency::USDC_CENT
        );
        let details = pallet_assets::Asset::<Runtime, pallet_assets::Instance1>::get(USDC_ASSET_ID);
        assert!(details.is_some_and(|asset| asset.is_sufficient));
        assert_eq!(
            Balances::minimum_balance(),
            currency::VIT_EXISTENTIAL_DEPOSIT
        );
        assert_eq!(Balances::total_issuance(), currency::VIT_TOTAL_SUPPLY);
    });
}

#[test]
fn metadata_generates_and_runtime_constants_are_visible() {
    development_ext().execute_with(|| {
        let encoded = Runtime::metadata().encode();
        assert!(encoded.len() > 128);
        assert_eq!(
            crate::configs::Ss58Prefix::get(),
            chain_identity::SS58_PREFIX
        );
        assert_eq!(pallet_guardian::GUARDIAN_SEATS, 7);
    });
}

#[test]
fn d13_system_calls_are_denied_bare_and_through_every_closed_wrapper() {
    for call in nobody_system_calls() {
        assert!(!RuntimeBaseCallFilter::contains(&call));
        for wrapped in closed_wrappers(call.clone()) {
            assert!(!RuntimeBaseCallFilter::contains(&wrapped));
        }
    }
    let mut nested = RuntimeCall::System(frame_system::Call::set_code { code: vec![1] });
    for depth in 0..kernel::MAX_NESTED_LEVELS {
        nested = match depth % 3 {
            0 => RuntimeCall::Utility(pallet_utility::Call::batch {
                calls: vec![nested],
            }),
            1 => RuntimeCall::Proxy(pallet_proxy::Call::proxy {
                real: MultiAddress::Id(account(15)),
                force_proxy_type: None,
                call: Box::new(nested),
            }),
            _ => RuntimeCall::Sudo(pallet_sudo::Call::sudo {
                call: Box::new(nested),
            }),
        };
        assert!(!RuntimeBaseCallFilter::contains(&nested));
    }
    assert!(RuntimeBaseCallFilter::contains(&remark()));
}

#[test]
fn nesting_budget_accepts_the_limit_and_fails_closed_beyond_it() {
    let mut at_limit = remark();
    for _ in 0..kernel::MAX_NESTED_LEVELS {
        at_limit = RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![at_limit],
        });
    }
    assert!(RuntimeBaseCallFilter::contains(&at_limit));
    let beyond = RuntimeCall::Utility(pallet_utility::Call::batch {
        calls: vec![at_limit],
    });
    assert!(!RuntimeBaseCallFilter::contains(&beyond));

    let oversized = RuntimeCall::Utility(pallet_utility::Call::batch {
        calls: (0..=kernel::MAX_NESTED_CALLS).map(|_| remark()).collect(),
    });
    assert!(!RuntimeBaseCallFilter::contains(&oversized));
}

#[test]
fn upgrade_filter_requires_internal_root_and_a_mature_pending_descriptor() {
    let authorize = RuntimeCall::System(frame_system::Call::authorize_upgrade {
        code_hash: H256::repeat_byte(1),
    });
    let all_origins = [
        ClassOrigin::FutarchyParam,
        ClassOrigin::FutarchyTreasury,
        ClassOrigin::FutarchyCode,
        ClassOrigin::FutarchyMeta,
        ClassOrigin::ConstitutionalValues,
        ClassOrigin::OracleResolution,
        ClassOrigin::GuardianHold,
        ClassOrigin::EmergencyPlaybook,
    ];
    assert!(!RuntimeBaseCallFilter::contains(&authorize));
    for origin in all_origins {
        assert!(!RuntimeBaseCallFilter::contains_for(origin, &authorize));
    }

    development_ext().execute_with(|| {
        let apply =
            RuntimeCall::System(frame_system::Call::apply_authorized_upgrade { code: vec![1] });
        System::set_block_number(10);
        set_test_applicable_at(None);
        assert!(!RuntimeBaseCallFilter::contains(&apply));
        for wrapped in closed_wrappers(apply.clone()) {
            assert!(!RuntimeBaseCallFilter::contains(&wrapped));
        }
        set_test_applicable_at(Some(11));
        assert!(!RuntimeBaseCallFilter::contains(&apply));
        set_test_applicable_at(Some(10));
        assert!(RuntimeBaseCallFilter::contains(&apply));
        assert!(RuntimeBaseCallFilter::contains(&RuntimeCall::Utility(
            pallet_utility::Call::batch {
                calls: vec![apply.clone()],
            }
        )));
        set_test_applicable_at(Some(9));
        assert!(RuntimeBaseCallFilter::contains(&apply));
        set_test_applicable_at(None);
    });
}

#[test]
fn proposal_classes_map_to_the_frozen_belief_origins() {
    assert_eq!(
        pallet_origins::Origin::from_proposal_class(ProposalClass::Param),
        Some(pallet_origins::Origin::FutarchyParam)
    );
    assert_eq!(
        pallet_origins::Origin::from_proposal_class(ProposalClass::Treasury),
        Some(pallet_origins::Origin::FutarchyTreasury)
    );
    assert_eq!(
        pallet_origins::Origin::from_proposal_class(ProposalClass::Code),
        Some(pallet_origins::Origin::FutarchyCode)
    );
    assert_eq!(
        pallet_origins::Origin::from_proposal_class(ProposalClass::Meta),
        Some(pallet_origins::Origin::FutarchyMeta)
    );
    assert_eq!(
        pallet_origins::Origin::from_proposal_class(ProposalClass::Constitutional),
        None
    );
}

fn assert_custom_origin_refuses_system_origins<E>()
where
    E: EnsureOrigin<RuntimeOrigin, Success = ()>,
{
    assert!(E::try_origin(RuntimeOrigin::signed(account(1))).is_err());
    assert!(E::try_origin(RuntimeOrigin::root()).is_err());
    assert!(E::try_origin(RuntimeOrigin::none()).is_err());
}

#[test]
fn all_eight_custom_origins_refuse_signed_root_and_none() {
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureFutarchyParam>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureFutarchyTreasury>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureFutarchyCode>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureFutarchyMeta>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureConstitutionalValues>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureOracleResolution>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureGuardianHold>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureEmergencyPlaybook>();
}

#[test]
fn domain_delegation_and_privileged_laundering_are_pinned() {
    let treasury =
        RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
            line: pallet_futarchy_treasury::BudgetLine::Pol,
            amount: 1,
        });
    assert!(RuntimeBaseCallFilter::contains(&remark()));
    assert!(!RuntimeBaseCallFilter::contains(&treasury));
    assert!(RuntimeBaseCallFilter::contains_for(
        ClassOrigin::FutarchyTreasury,
        &treasury
    ));
    assert!(!RuntimeBaseCallFilter::contains_for(
        ClassOrigin::FutarchyParam,
        &treasury
    ));
    for (index, wrapped) in closed_wrappers(treasury.clone()).into_iter().enumerate() {
        assert!(!RuntimeBaseCallFilter::contains(&wrapped));
        if (3..=6).contains(&index) {
            assert!(!RuntimeBaseCallFilter::contains_for(
                ClassOrigin::FutarchyTreasury,
                &wrapped
            ));
        }
    }
    let nested = RuntimeCall::Proxy(pallet_proxy::Call::proxy_announced {
        delegate: MultiAddress::Id(account(11)),
        real: MultiAddress::Id(account(12)),
        force_proxy_type: None,
        call: Box::new(RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![treasury],
        })),
    });
    assert!(!RuntimeBaseCallFilter::contains_for(
        ClassOrigin::FutarchyTreasury,
        &nested
    ));
}

#[test]
fn classifier_sweeps_every_callable_pallet_and_every_closed_wrapper_shape() {
    let who = account(31);
    let mut calls = vec![
        remark(),
        RuntimeCall::Timestamp(pallet_timestamp::Call::set { now: 6_000 }),
        RuntimeCall::ParachainSystem(
            cumulus_pallet_parachain_system::Call::sudo_send_upward_message { message: vec![1] },
        ),
        RuntimeCall::Balances(pallet_balances::Call::transfer_keep_alive {
            dest: MultiAddress::Id(who.clone()),
            value: 1,
        }),
        RuntimeCall::ForeignAssets(pallet_assets::Call::transfer {
            id: USDC_ASSET_ID,
            target: MultiAddress::Id(who.clone()),
            amount: 1,
        }),
        RuntimeCall::Referenda(pallet_referenda::Call::cancel { index: 0 }),
        RuntimeCall::ConvictionVoting(pallet_conviction_voting::Call::remove_vote {
            class: None,
            index: 0,
        }),
        RuntimeCall::Preimage(pallet_preimage::Call::unnote_preimage { hash: H256::zero() }),
        RuntimeCall::Scheduler(pallet_scheduler::Call::cancel { when: 1, index: 0 }),
        RuntimeCall::Utility(pallet_utility::Call::batch { calls: Vec::new() }),
        RuntimeCall::Proxy(pallet_proxy::Call::remove_proxies {}),
        RuntimeCall::Multisig(pallet_multisig::Call::poke_deposit {
            threshold: 2,
            other_signatories: vec![who.clone()],
            call_hash: [0; 32],
        }),
        RuntimeCall::Migrations(pallet_migrations::Call::clear_historic {
            selector: pallet_migrations::HistoricCleanupSelector::Specific(Vec::new()),
        }),
        RuntimeCall::Sudo(pallet_sudo::Call::remove_key {}),
        RuntimeCall::XcmpQueue(cumulus_pallet_xcmp_queue::Call::suspend_xcm_execution {}),
        RuntimeCall::MessageQueue(pallet_message_queue::Call::reap_page {
            message_origin: cumulus_primitives_core::AggregateMessageOrigin::Parent,
            page_index: 0,
        }),
        RuntimeCall::PolkadotXcm(pallet_xcm::Call::force_suspension { suspended: false }),
        RuntimeCall::CollatorSelection(pallet_collator_selection::Call::register_as_candidate {}),
        RuntimeCall::Session(pallet_session::Call::purge_keys {}),
        RuntimeCall::Constitution(pallet_constitution::Call::set_phase_flag {
            flag: 1,
            enabled: false,
        }),
        RuntimeCall::ConditionalLedger(pallet_conditional_ledger::Call::transfer {
            position: futarchy_primitives::PositionId::Proposal {
                proposal: 0,
                branch: futarchy_primitives::Branch::Accept,
                kind: futarchy_primitives::PositionKind::BranchUsdc,
            },
            to: who.clone(),
            amount: 1,
        }),
        RuntimeCall::Market(pallet_market::Call::crank_observe { market: 0 }),
        RuntimeCall::Welfare(pallet_welfare::Call::record_snapshot {
            epoch: 0,
            spec_version: 0,
        }),
        RuntimeCall::Oracle(pallet_oracle::Call::crank_round_close { batch: 1 }),
        RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
            line: pallet_futarchy_treasury::BudgetLine::Pol,
            amount: 1,
        }),
        RuntimeCall::Guardian(pallet_guardian::Call::propose_action {
            power: pallet_guardian::GuardianPower::SuspendOnGate,
            justification_hash: H256::zero().into(),
        }),
        RuntimeCall::Attestor(pallet_attestor::Call::attest {
            pid: 0,
            artifact_hash: H256::zero().into(),
            statement_hash: H256::zero().into(),
        }),
    ];
    calls.extend(
        registry_calls::<()>()
            .into_iter()
            .take(1)
            .map(RuntimeCall::IncidentRegistry),
    );
    calls.extend(
        registry_calls::<pallet_registry::Instance1>()
            .into_iter()
            .take(1)
            .map(RuntimeCall::MilestoneRegistry),
    );
    calls.extend(closed_wrappers(remark()));
    let signed_caller: <RuntimeOrigin as frame_support::traits::OriginTrait>::PalletsOrigin =
        frame_system::RawOrigin::Signed(who.clone()).into();
    calls.extend([
        RuntimeCall::Utility(pallet_utility::Call::as_derivative {
            index: 0,
            call: Box::new(remark()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::dispatch_as {
            as_origin: Box::new(signed_caller.clone()),
            call: Box::new(remark()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::with_weight {
            call: Box::new(remark()),
            weight: Weight::zero(),
        }),
        RuntimeCall::Utility(pallet_utility::Call::if_else {
            main: Box::new(remark()),
            fallback: Box::new(remark()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::dispatch_as_fallible {
            as_origin: Box::new(signed_caller),
            call: Box::new(remark()),
        }),
        RuntimeCall::Multisig(pallet_multisig::Call::approve_as_multi {
            threshold: 2,
            other_signatories: vec![who],
            maybe_timepoint: None,
            call_hash: [0; 32],
            max_weight: Weight::zero(),
        }),
        RuntimeCall::Scheduler(pallet_scheduler::Call::schedule {
            when: 1,
            maybe_periodic: None,
            priority: 0,
            call: Box::new(remark()),
        }),
    ]);
    assert!(calls.len() >= 34);
    for call in calls {
        let _ = RuntimeBaseCallFilter::contains(&call);
        let _ = RuntimeBaseCallFilter::contains_for(ClassOrigin::ConstitutionalValues, &call);
    }
}

fn registry_calls<I: 'static>() -> Vec<pallet_registry::Call<Runtime, I>>
where
    Runtime: pallet_registry::Config<I>,
{
    vec![
        pallet_registry::Call::file {
            epoch: 1,
            class: registry_core::FilingClass::S1,
            points: 1,
            evidence_hash: H256::repeat_byte(1).into(),
            spec_version: 1,
        },
        pallet_registry::Call::challenge_filing {
            epoch: 1,
            filing_id: 0,
            evidence_hash: H256::repeat_byte(2).into(),
        },
        pallet_registry::Call::ack_observed {
            epoch: 1,
            filing_id: 0,
        },
        pallet_registry::Call::crank_close { epoch: 1, batch: 1 },
        pallet_registry::Call::resolve_challenge {
            epoch: 1,
            filing_id: 0,
            uphold: false,
        },
        pallet_registry::Call::close_epoch { epoch: 1 },
        pallet_registry::Call::reap_epoch { epoch: 1 },
    ]
}

#[test]
fn sq75_both_registry_instances_are_base_filter_public_and_resolve_is_origin_gated() {
    let incident: Vec<RuntimeCall> = registry_calls::<()>()
        .into_iter()
        .map(RuntimeCall::IncidentRegistry)
        .collect();
    let milestone: Vec<RuntimeCall> = registry_calls::<pallet_registry::Instance1>()
        .into_iter()
        .map(RuntimeCall::MilestoneRegistry)
        .collect();
    for call in incident.iter().chain(milestone.iter()) {
        assert!(RuntimeBaseCallFilter::contains(call));
        let wrapped = RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![call.clone()],
        });
        assert!(RuntimeBaseCallFilter::contains(&wrapped));
    }

    development_ext().execute_with(|| {
        let result = incident[4]
            .clone()
            .dispatch(RuntimeOrigin::signed(account(9)));
        assert!(matches!(result, Err(error) if error.error == DispatchError::BadOrigin));
        let result = milestone[4]
            .clone()
            .dispatch(RuntimeOrigin::signed(account(9)));
        assert!(matches!(result, Err(error) if error.error == DispatchError::BadOrigin));
    });
}

#[test]
fn signed_custom_pallet_row_is_admitted_by_the_base_filter() {
    let calls = vec![
        RuntimeCall::ConditionalLedger(pallet_conditional_ledger::Call::transfer {
            position: futarchy_primitives::PositionId::Proposal {
                proposal: 0,
                branch: futarchy_primitives::Branch::Accept,
                kind: futarchy_primitives::PositionKind::BranchUsdc,
            },
            to: account(2),
            amount: 1,
        }),
        RuntimeCall::Market(pallet_market::Call::crank_observe { market: 0 }),
        RuntimeCall::Welfare(pallet_welfare::Call::record_snapshot {
            epoch: 0,
            spec_version: 0,
        }),
        RuntimeCall::Oracle(pallet_oracle::Call::crank_round_close { batch: 1 }),
        RuntimeCall::Guardian(pallet_guardian::Call::propose_action {
            power: pallet_guardian::GuardianPower::SuspendOnGate,
            justification_hash: H256::zero().into(),
        }),
        RuntimeCall::Attestor(pallet_attestor::Call::attest {
            pid: 0,
            artifact_hash: H256::zero().into(),
            statement_hash: H256::zero().into(),
        }),
    ];
    for call in calls {
        assert!(RuntimeBaseCallFilter::contains(&call));
        assert!(call.get_dispatch_info().call_weight.ref_time() > 0);
    }
}

#[test]
fn values_leaf_dispatches_with_values_origin_and_signed_dies_in_pallet() {
    let members = [
        account(1),
        account(2),
        account(3),
        account(4),
        account(5),
        account(6),
        account(7),
    ];
    let call = RuntimeCall::Guardian(pallet_guardian::Call::set_members { members });
    assert!(RuntimeBaseCallFilter::contains(&call));
    development_ext().execute_with(|| {
        let signed = call.clone().dispatch(RuntimeOrigin::signed(account(1)));
        assert!(matches!(signed, Err(error) if error.error == DispatchError::BadOrigin));
        let values = call
            .clone()
            .dispatch(pallet_origins::Origin::ConstitutionalValues.into());
        assert!(values.is_ok());

        let nobody = RuntimeCall::System(frame_system::Call::set_storage { items: vec![] });
        assert!(!RuntimeBaseCallFilter::contains_for(
            ClassOrigin::ConstitutionalValues,
            &nobody
        ));
    });
}

#[test]
fn guardian_pending_empty_membership_on_initialize_is_a_no_op() {
    development_ext().execute_with(|| {
        System::set_block_number(1);
        let before = System::events().len();
        let weight = <Guardian as frame_support::traits::Hooks<BlockNumber>>::on_initialize(1);
        assert_eq!(
            weight,
            pallet_guardian::weights::SubstrateWeight::<Runtime>::on_initialize()
        );
        assert_eq!(System::events().len(), before);
    });
}

#[test]
fn pending_a8_welfare_cranks_reject_without_locking_empty_inputs() {
    development_ext().execute_with(|| {
        System::set_block_number(1);
        let result = RuntimeCall::Welfare(pallet_welfare::Call::record_snapshot {
            epoch: 0,
            spec_version: 0,
        })
        .dispatch(RuntimeOrigin::signed(Sr25519Keyring::Alice.to_account_id()));
        assert!(result.is_err());
        assert!(pallet_welfare::Snapshots::<Runtime>::iter()
            .next()
            .is_none());
    });
}

#[test]
fn executive_builds_and_executes_inherents_and_a_fee_paying_vit_transfer() {
    let destination = account(42);
    let block = development_ext().execute_with(|| build_executive_smoke_block(destination.clone()));
    development_ext().execute_with(|| {
        let alice = Sr25519Keyring::Alice.to_account_id();
        let before = Balances::free_balance(&alice);
        crate::Executive::execute_block(block.into());
        assert_eq!(Timestamp::get(), kernel::MILLISECS_PER_BLOCK);
        assert_eq!(
            Balances::free_balance(&destination),
            currency::VIT_EXISTENTIAL_DEPOSIT
        );
        assert!(Balances::free_balance(&alice) < before - currency::VIT_EXISTENTIAL_DEPOSIT);
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Balances(pallet_balances::Event::Transfer { .. })
        )));
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::TransactionPayment(
                pallet_transaction_payment::Event::TransactionFeePaid { .. }
            )
        )));
    });
}

#[cfg(feature = "try-runtime")]
#[test]
fn executive_smoke_state_passes_all_try_state_checks() {
    use frame_support::traits::TryState;

    let destination = account(43);
    let block = development_ext().execute_with(|| build_executive_smoke_block(destination));
    development_ext().execute_with(|| {
        crate::Executive::execute_block(block.into());
        assert!(
            <crate::AllPalletsWithSystem as TryState<crate::BlockNumber>>::try_state(
                System::block_number(),
                frame_try_runtime::TryStateSelect::All,
            )
            .is_ok()
        );
    });
}

// --- Post-authoring review regressions (session fixes over the Codex draft) ---

#[test]
fn ump_send_and_balances_force_calls_are_nobody_even_under_sudo() {
    let mut calls = vec![
        RuntimeCall::ParachainSystem(
            cumulus_pallet_parachain_system::Call::sudo_send_upward_message { message: vec![1] },
        ),
        RuntimeCall::Balances(pallet_balances::Call::force_transfer {
            source: MultiAddress::Id(account(1)),
            dest: MultiAddress::Id(account(2)),
            value: 1,
        }),
        RuntimeCall::Balances(pallet_balances::Call::force_unreserve {
            who: MultiAddress::Id(account(1)),
            amount: 1,
        }),
        RuntimeCall::Balances(pallet_balances::Call::force_set_balance {
            who: MultiAddress::Id(account(1)),
            new_free: 1,
        }),
        RuntimeCall::Balances(pallet_balances::Call::force_adjust_total_issuance {
            direction: pallet_balances::AdjustmentDirection::Increase,
            delta: 1,
        }),
    ];
    for call in calls.drain(..) {
        assert!(
            !RuntimeBaseCallFilter::contains(&call),
            "bare force/UMP call must be nobody: {call:?}"
        );
        for origin in pallet_origins::Origin::ALL {
            assert!(
                !RuntimeBaseCallFilter::contains_for(origin.to_model(), &call),
                "no custom origin may reach the nobody row: {call:?}"
            );
        }
        let sudo_wrapped = RuntimeCall::Sudo(pallet_sudo::Call::sudo {
            call: Box::new(call.clone()),
        });
        assert!(
            !RuntimeBaseCallFilter::contains(&sudo_wrapped),
            "sudo wrapper must not launder the nobody row: {call:?}"
        );
    }
    assert!(RuntimeBaseCallFilter::contains(&RuntimeCall::Balances(
        pallet_balances::Call::transfer_keep_alive {
            dest: MultiAddress::Id(account(2)),
            value: 1,
        }
    )));
}

#[test]
fn origin_aware_matrix_is_not_widened_by_the_values_leaf_admission() {
    let adjudicate = RuntimeCall::Oracle(pallet_oracle::Call::adjudicate {
        component: 1,
        epoch: 1,
        spec_version: 1,
        value: futarchy_primitives::FixedU64(0),
        reporter_wrong: false,
    });
    // The stock-scheduler accommodation admits the bare leaf origin-blind …
    assert!(RuntimeBaseCallFilter::contains(&adjudicate));
    // … but the origin-aware matrix check stays exact: only OracleResolution.
    assert!(RuntimeBaseCallFilter::contains_for(
        ClassOrigin::OracleResolution,
        &adjudicate
    ));
    assert!(!RuntimeBaseCallFilter::contains_for(
        ClassOrigin::ConstitutionalValues,
        &adjudicate
    ));
    // And a values leaf is admitted as a BARE leaf only — wrappers still deny.
    let resolve = RuntimeCall::Attestor(pallet_attestor::Call::resolve_challenge {
        attestation_id: 0,
        attestation_upheld: false,
    });
    assert!(RuntimeBaseCallFilter::contains(&resolve));
    assert!(!RuntimeBaseCallFilter::contains(&RuntimeCall::Utility(
        pallet_utility::Call::batch {
            calls: vec![resolve.clone()],
        }
    )));
    assert!(!RuntimeBaseCallFilter::contains(&RuntimeCall::Proxy(
        pallet_proxy::Call::proxy {
            real: MultiAddress::Id(account(1)),
            force_proxy_type: None,
            call: Box::new(resolve),
        }
    )));
}

#[test]
fn set_param_domain_follows_the_registry_key_class() {
    development_ext().execute_with(|| {
        let set = |name: &[u8]| {
            RuntimeCall::Constitution(pallet_constitution::Call::set_param {
                key: pallet_constitution::key16(name),
                value: pallet_constitution::ParamValue::Balance(1),
            })
        };
        // PARAM-class key (mkt.fee) — FutarchyParam only (06 §3.2 row 1).
        assert!(RuntimeBaseCallFilter::contains_for(
            ClassOrigin::FutarchyParam,
            &set(b"mkt.fee")
        ));
        assert!(!RuntimeBaseCallFilter::contains_for(
            ClassOrigin::FutarchyTreasury,
            &set(b"mkt.fee")
        ));
        // TREASURY-class key (pol.b_gate) — FutarchyTreasury only (row 2).
        assert!(RuntimeBaseCallFilter::contains_for(
            ClassOrigin::FutarchyTreasury,
            &set(b"pol.b_gate")
        ));
        assert!(!RuntimeBaseCallFilter::contains_for(
            ClassOrigin::FutarchyParam,
            &set(b"pol.b_gate")
        ));
        // Unknown key fails closed for every origin and origin-less.
        assert!(!RuntimeBaseCallFilter::contains(&set(b"no.such_key")));
        for origin in pallet_origins::Origin::ALL {
            assert!(!RuntimeBaseCallFilter::contains_for(
                origin.to_model(),
                &set(b"no.such_key")
            ));
        }
        // Origin-less submission of any real set_param stays denied (privileged).
        assert!(!RuntimeBaseCallFilter::contains(&set(b"mkt.fee")));
    });
}

#[test]
fn live_param_adapters_resolve_their_registry_keys() {
    use frame_support::traits::Get;
    development_ext().execute_with(|| {
        // A typo'd key name would silently fall through to 0 — pin every
        // adapter to its 13 §1 genesis value (rule 4).
        assert_eq!(
            crate::configs::LedgerMinSplit::get(),
            kernel::MIN_SPLIT_USDC
        );
        assert_eq!(
            crate::configs::LedgerPositionDeposit::get(),
            kernel::POSITION_DEPOSIT_USDC
        );
        assert_eq!(crate::configs::MarketFee::get(), 30);
        assert_eq!(crate::configs::MarketObsInterval::get(), 10);
        assert_eq!(crate::configs::MarketKappa::get(), 5_000_000);
        assert!(crate::configs::LedgerArchiveDelay::get() > 0);
    });
}

#[test]
fn pending_incident_multiplier_defaults_to_the_neutral_identity() {
    use pallet_welfare::MetricInputs;
    development_ext().execute_with(|| {
        // No closed registry epoch ⇒ the neutral 1.0 multiplier (a zero would
        // erase C_attested outright — fail-destructive, not fail-safe).
        assert_eq!(
            crate::configs::PendingMetricInputs::incident_multiplier(5),
            futarchy_primitives::FixedU64(1_000_000_000)
        );
    });
}

#[test]
fn sudo_as_is_denied_so_the_founding_multisig_cannot_impersonate_accounts() {
    // P1 (Codex adversarial review): `sudo_as(who, call)` dispatches as
    // `Signed(who)` for a CHOSEN `who`, so recursing it would let the founding
    // multisig forge any signed origin — steal VIT (`transfer`) or, worse,
    // impersonate the welfare settlement account to drive ledger settlement,
    // defeating 06 §3.1's "SettleAuthority reachable through exactly one path".
    // `sudo_as` is denied outright; `sudo`/`sudo_unchecked_weight` (Root
    // dispatch) stay recursed.
    let victim_transfer = RuntimeCall::Balances(pallet_balances::Call::transfer_keep_alive {
        dest: MultiAddress::Id(account(99)),
        value: 1,
    });
    let forge_settlement =
        RuntimeCall::ConditionalLedger(pallet_conditional_ledger::Call::settle_scalar {
            pid: 0,
            s: futarchy_primitives::FixedU64(0),
        });
    for inner in [victim_transfer, forge_settlement, remark()] {
        let sudo_as = RuntimeCall::Sudo(pallet_sudo::Call::sudo_as {
            who: MultiAddress::Id(crate::configs::welfare_settlement_account()),
            call: Box::new(inner.clone()),
        });
        assert!(
            !RuntimeBaseCallFilter::contains(&sudo_as),
            "sudo_as must be denied for every inner call: {inner:?}"
        );
        // …and it must not become reachable by wrapping it further.
        assert!(!RuntimeBaseCallFilter::contains(&RuntimeCall::Utility(
            pallet_utility::Call::batch {
                calls: vec![sudo_as.clone()],
            }
        )));
        // The Root-dispatching variants still recurse a benign public inner.
        let sudo_root = RuntimeCall::Sudo(pallet_sudo::Call::sudo {
            call: Box::new(remark()),
        });
        assert!(RuntimeBaseCallFilter::contains(&sudo_root));
    }
}

#[test]
fn const_and_entrenched_set_param_are_enactable_by_constitutional_values() {
    // P2#5: a passed `constitution`/`entrenched` values referendum enacting
    // `set_param` on a CONST/entrenched key must survive the origin-blind base
    // filter (stock scheduler dispatches filtered, SQ-32) — its produced origin
    // is ConstitutionalValues and its `GovernanceOrigin` check is the second
    // gate. PARAM/TREASURY/META keys must NOT get this bare-leaf admission.
    development_ext().execute_with(|| {
        let set = |name: &[u8]| {
            RuntimeCall::Constitution(pallet_constitution::Call::set_param {
                key: pallet_constitution::key16(name),
                value: pallet_constitution::ParamValue::Fixed(futarchy_primitives::FixedU64(
                    950_000_000,
                )),
            })
        };
        // CONST key + Entrenched key: admitted origin-blind (values-enactment leaf).
        for key in [b"welfare.thS_lo".as_slice(), b"att.bond".as_slice()] {
            assert!(
                RuntimeBaseCallFilter::contains(&set(key)),
                "CONST/entrenched set_param must be enactable: {key:?}"
            );
            assert!(crate::classifier::is_values_enactment_leaf(&set(key)));
            // Still bare-leaf only — a wrapper carrying it is denied.
            assert!(!RuntimeBaseCallFilter::contains(&RuntimeCall::Utility(
                pallet_utility::Call::batch {
                    calls: vec![set(key)],
                }
            )));
        }
        // PARAM key (mkt.fee) is NOT a values-enactment leaf — belief side.
        assert!(!crate::classifier::is_values_enactment_leaf(&set(
            b"mkt.fee"
        )));
        assert!(!RuntimeBaseCallFilter::contains(&set(b"mkt.fee")));
    });
}

#[test]
fn genesis_phase_flags_advertise_sudo_present_alongside_the_sudo_key() {
    // P2#7: the preset installs a sudo key, so bit 4 (SUDO_PRESENT) MUST be set
    // — the FE binds its bootstrap-governance banner to it (09 §5.2).
    development_ext().execute_with(|| {
        let flags = pallet_constitution::PhaseFlags::<Runtime>::get();
        assert_eq!(
            flags,
            pallet_constitution::PhaseFlagsValue::SHADOW_MODE
                | pallet_constitution::PhaseFlagsValue::SUDO_PRESENT
        );
        assert!(
            pallet_sudo::Key::<Runtime>::get().is_some(),
            "preset installs a sudo key"
        );
        assert_ne!(
            flags & pallet_constitution::PhaseFlagsValue::SUDO_PRESENT,
            0,
            "SUDO_PRESENT must be set whenever a sudo key exists"
        );
    });
}

#[test]
fn referenda_support_curves_decay_high_to_low_without_underflow() {
    // Major (spec-reviewer): a floor/ceil-swapped `make_linear` underflows
    // `Perbill::sub` in `Curve::threshold` — panic under overflow-checks, or a
    // wrapped ~419% support requirement in release — making EVERY values track
    // unable to confirm. Drive each support curve at turnout 0/½/1 and assert
    // the monotone high→low shape and the exact endpoints. The shared CV track
    // carries the strongest (entrenched) 06 §2.1 thresholds (20%→10%, PR #57 bot
    // P1); oracle keeps its own (10%→3%).
    use sp_runtime::Perbill;
    let eval = |curve: &pallet_referenda::Curve, x: Perbill| curve.threshold(x);
    let cases = [
        (
            &crate::configs::CV_SUPPORT,
            Perbill::from_percent(20),
            Perbill::from_percent(10),
        ),
        (
            &crate::configs::ORACLE_SUPPORT,
            Perbill::from_percent(10),
            Perbill::from_percent(3),
        ),
    ];
    for (curve, at_zero, at_one) in cases {
        let lo = eval(curve, Perbill::zero());
        let mid = eval(curve, Perbill::from_rational(1u32, 2u32));
        let hi = eval(curve, Perbill::one());
        assert_eq!(lo, at_zero, "support requirement at turnout 0 is the ceil");
        assert_eq!(hi, at_one, "support requirement at turnout 1 is the floor");
        // Monotone high→low with the exact endpoints proves the curve is not
        // floor/ceil-swapped: a swapped curve wraps (mid far above the ceil) or
        // panics under overflow-checks before reaching here.
        assert!(
            lo >= mid && mid >= hi,
            "support requirement must decay monotonically"
        );
        assert!(
            mid < at_zero && mid > at_one,
            "midpoint strictly between endpoints"
        );
    }
    // Approval curves are flat at their single value (order-immaterial).
    assert_eq!(
        crate::configs::CV_APPROVAL.threshold(Perbill::from_rational(1u32, 3u32)),
        Perbill::from_percent(80)
    );
    assert_eq!(
        crate::configs::ORACLE_APPROVAL.threshold(Perbill::from_rational(3u32, 4u32)),
        Perbill::from_percent(60)
    );
}

#[test]
fn shared_cv_track_dominates_every_values_track_threshold() {
    // PR #57 Codex-bot P1: the five `ConstitutionalValues` 06 §2.1 tracks
    // collapse onto one (stock referenda routes by origin), so the shared track
    // MUST demand at least the strongest track's approval/support at every
    // turnout — otherwise an entrenched-scope action (e.g. lowering the
    // entrenched-class `att.bond`) could pass at a weaker bar. Assert the shared
    // CV curves dominate every 06 §2.1 CV track (metric 60%→50%/10%→2%,
    // constitution 67%/15%→5%, guardian 55%/5%, ratify 50%/5%, entrenched
    // 80%/20%→10%) pointwise.
    use sp_runtime::Perbill;
    let strongest_approval = Perbill::from_percent(80); // entrenched
    let strongest_support_ceil = Perbill::from_percent(20); // entrenched at turnout 0
    for num in 0u32..=4 {
        let x = Perbill::from_rational(num, 4u32);
        assert!(
            crate::configs::CV_APPROVAL.threshold(x) >= strongest_approval,
            "shared CV approval must be ≥ the strongest (entrenched 80%) at every turnout"
        );
    }
    // Support requirement at any turnout is ≥ the strongest track's requirement
    // at that turnout (both decay; the CV ceil equals entrenched's ceil).
    assert_eq!(
        crate::configs::CV_SUPPORT.threshold(Perbill::zero()),
        strongest_support_ceil
    );
    // No weaker legacy value leaked in (a 67%/15% constitution-track config
    // would fail the approval dominance above).
    assert_eq!(
        crate::configs::CV_APPROVAL.threshold(Perbill::zero()),
        Perbill::from_percent(80)
    );
}

#[test]
fn referenda_cancel_and_kill_are_enactable_by_constitutional_values() {
    // PR #57 Codex-bot P2: `referenda.cancel`/`kill` are ConstitutionalValues-
    // domain (the runtime's Cancel/Kill origins), so a values referendum
    // enacting them must clear the origin-blind base filter (bare-leaf values
    // enactment); otherwise the scheduler's filtered dispatch rejects
    // `CallFiltered` before the origin check, leaving both controls unreachable.
    for call in [
        RuntimeCall::Referenda(pallet_referenda::Call::cancel { index: 0 }),
        RuntimeCall::Referenda(pallet_referenda::Call::kill { index: 0 }),
    ] {
        assert!(crate::classifier::is_values_enactment_leaf(&call));
        assert!(
            RuntimeBaseCallFilter::contains(&call),
            "cancel/kill must pass the base filter as a bare values-enactment leaf: {call:?}"
        );
        // Bare leaf only — a wrapper carrying it stays denied.
        assert!(!RuntimeBaseCallFilter::contains(&RuntimeCall::Utility(
            pallet_utility::Call::batch {
                calls: vec![call.clone()]
            }
        )));
        // Signed origin still dies at the pallet's Cancel/KillOrigin (BadOrigin),
        // not at the filter — the base filter admits, the EnsureOrigin rejects.
        assert!(RuntimeBaseCallFilter::contains_for(
            ClassOrigin::ConstitutionalValues,
            &call
        ));
    }
}
