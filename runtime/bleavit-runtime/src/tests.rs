//! Runtime-level B1a composition and safety-filter regression suite.

#![allow(clippy::assertions_on_constants, clippy::manual_unwrap_or_default)]

use alloc::{boxed::Box, vec, vec::Vec};

use frame_support::{
    assert_noop, assert_ok,
    dispatch::{DispatchClass, GetDispatchInfo},
    traits::{
        fungible::Inspect as FungibleInspect,
        fungibles::{Inspect as FungiblesInspect, Mutate as FungiblesMutate},
        tokens::ConversionToAssetBalance,
        Contains, EnsureOrigin, Get, Hooks, PalletInfo, PalletsInfoAccess, StorePreimage,
        VestingSchedule,
    },
    weights::Weight,
};
use futarchy_primitives::{chain_identity, currency, kernel, ProposalClass};
use origins_core::Origin as ClassOrigin;
use parity_scale_codec::Encode;
use sp_core::H256;
use sp_genesis_builder::PresetId;
use sp_inherents::InherentData;
use sp_keyring::Sr25519Keyring;
use sp_runtime::{
    generic::{Era, SignedPayload},
    traits::{Block as BlockT, Dispatchable, Header as HeaderT},
    transaction_validity::{InvalidTransaction, TransactionValidityError},
    BuildStorage, DispatchError, MultiAddress, MultiSignature,
};

use crate::{
    classifier::{RuntimeBaseCallFilter, RuntimeDispatcher},
    usdc_location, AccountId, AllPalletsWithSystem, AssetTxPayment, Attestor, Aura, AuraExt,
    Authorship, Balances, BlockNumber, CollatorSelection, ConditionalLedger, Constitution,
    ConvictionVoting, CumulusXcm, Epoch, ExecutionGuard, ForeignAssets, FutarchyTreasury, Guardian,
    IncidentRegistry, Market, MessageQueue, Migrations, MilestoneRegistry, Multisig, Oracle,
    Origins, PalletInfo as RuntimePalletInfo, ParachainInfo, ParachainSystem, PolkadotXcm,
    Preimage, Proxy, Referenda, Runtime, RuntimeCall, RuntimeGenesisConfig, RuntimeOrigin,
    Scheduler, Session, Sudo, System, Timestamp, TransactionPayment, TxExtension,
    UncheckedExtrinsic, Utility, Vesting, Welfare, XcmpQueue, FEE_VIT_USDC_RATE_KEY,
    MILLISECS_PER_BLOCK, SS58_PREFIX, USDC_DECIMALS, USDC_LOCATION_ENCODED, VERSION, VIT_DECIMALS,
};

trait SameType<Rhs> {}
impl<T> SameType<T> for T {}

fn assert_same_type<Left, Right>()
where
    Left: SameType<Right>,
{
}

pub(crate) fn account(seed: u8) -> AccountId {
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

pub(crate) fn development_ext() -> sp_io::TestExternalities {
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

struct CandidateRuntimeVersion(Vec<u8>);

impl sp_core::traits::ReadRuntimeVersion for CandidateRuntimeVersion {
    fn read_runtime_version(
        &self,
        _: &[u8],
        _: &mut dyn sp_externalities::Externalities,
    ) -> Result<Vec<u8>, String> {
        Ok(self.0.clone())
    }
}

pub(crate) fn upgrade_ext() -> sp_io::TestExternalities {
    let mut version = VERSION;
    version.spec_version = version.spec_version.saturating_add(1);
    let mut ext = development_ext();
    ext.register_extension(sp_core::traits::ReadRuntimeVersionExt::new(
        CandidateRuntimeVersion(version.encode()),
    ));
    ext
}

fn release_channel_raw() -> Option<Vec<u8>> {
    let mut key = sp_io::hashing::twox_128(b"Constitution").to_vec();
    key.extend_from_slice(&sp_io::hashing::twox_128(b"ReleaseChannel"));
    sp_io::storage::get(&key).map(|bytes| bytes.to_vec())
}

fn raw_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let source = bytes.get(offset..offset.checked_add(4)?)?;
    let mut encoded = [0u8; 4];
    encoded.copy_from_slice(source);
    Some(u32::from_le_bytes(encoded))
}

fn assert_raw_unchanged_outside(before: &[u8], after: &[u8], owned: &[core::ops::Range<usize>]) {
    assert_eq!(before.len(), after.len());
    for (index, (before, after)) in before.iter().zip(after).enumerate() {
        if !owned.iter().any(|range| range.contains(&index)) {
            assert_eq!(before, after, "unexpected ReleaseChannel write at {index}");
        }
    }
}

fn enqueue_attested_code_upgrade(
    pid: futarchy_primitives::ProposalId,
    candidate: &[u8],
    referendum_index: u32,
) -> Option<(BlockNumber, H256)> {
    let members = [account(90), account(91), account(92)];
    assert_ok!(Attestor::set_members(
        pallet_origins::Origin::ConstitutionalValues.into(),
        members.to_vec(),
    ));
    let artifact = H256::from(sp_io::hashing::blake2_256(candidate));
    for (member, statement) in members.iter().take(2).zip([101u8, 102u8]) {
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(member.clone()),
            pid,
            artifact.0,
            [statement; 32],
        ));
    }
    let first = pallet_attestor::Attestations::<Runtime>::get()
        .into_iter()
        .find(|record| record.pid == pid && record.artifact_hash == artifact.0)?;
    System::set_block_number(first.challenge_deadline.saturating_add(1));
    assert!(Attestor::has_quorum(pid, artifact.0));

    let call = RuntimeCall::System(frame_system::Call::authorize_upgrade {
        code_hash: artifact,
    });
    let batch =
        pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![call]).ok()?;
    let bytes = batch.encode();
    let payload_len = u32::try_from(bytes.len()).ok()?;
    let payload_hash = <Preimage as StorePreimage>::note(bytes.into()).ok()?;
    crate::configs::set_test_execution_payload(pid, payload_hash.0);

    let now = System::block_number();
    let maturity = now.checked_add(
        <crate::configs::RuntimeGuardParams as pallet_execution_guard::Params>::exec_timelock(
            ProposalClass::Code,
        ),
    )?;
    let grace_end = maturity.checked_add(
        <crate::configs::RuntimeGuardParams as pallet_execution_guard::Params>::exec_grace(
            ProposalClass::Code,
        ),
    )?;
    let version_constraint = pallet_execution_guard::CurrentSpecName::<Runtime>::get()?;
    let declared_domains = pallet_execution_guard::pallet::StoredDomains::try_from(vec![
        pallet_execution_guard::CallDomain::InternalRootAuthorizeUpgrade,
    ])
    .ok()?;
    assert_ok!(ExecutionGuard::enqueue(
        crate::configs::test_execution_enqueue_origin(),
        pallet_execution_guard::pallet::StoredQueuedExecution {
            pid,
            payload_hash: payload_hash.0,
            payload_len,
            class: ProposalClass::Code,
            maturity,
            grace_end,
            version_constraint,
            meters_declared: Default::default(),
            ratify_ref: Some(referendum_index),
            ratification_passed: false,
            attestation_id: Some(first.id),
            pre_upgrade_checkpoint: None,
            cancelled: false,
            declared_domains,
            failed_at: None,
        },
        false,
    ));
    assert_ok!(ExecutionGuard::ratify(
        pallet_origins::Origin::ConstitutionalValues.into(),
        pid,
        referendum_index,
    ));
    Some((maturity, artifact))
}

fn enqueue_treasury_call(
    pid: futarchy_primitives::ProposalId,
    call: RuntimeCall,
) -> Option<BlockNumber> {
    let batch =
        pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![call]).ok()?;
    let bytes = batch.encode();
    let payload_len = u32::try_from(bytes.len()).ok()?;
    let payload_hash = <Preimage as StorePreimage>::note(bytes.into()).ok()?;
    crate::configs::set_test_execution_payload(pid, payload_hash.0);
    let now = System::block_number();
    let maturity = now.checked_add(
        <crate::configs::RuntimeGuardParams as pallet_execution_guard::Params>::exec_timelock(
            ProposalClass::Treasury,
        ),
    )?;
    let grace_end = maturity.checked_add(
        <crate::configs::RuntimeGuardParams as pallet_execution_guard::Params>::exec_grace(
            ProposalClass::Treasury,
        ),
    )?;
    let version_constraint = pallet_execution_guard::CurrentSpecName::<Runtime>::get()?;
    let declared_domains = pallet_execution_guard::pallet::StoredDomains::try_from(vec![
        pallet_execution_guard::CallDomain::Treasury,
    ])
    .ok()?;
    assert_ok!(ExecutionGuard::enqueue(
        crate::configs::test_execution_enqueue_origin(),
        pallet_execution_guard::pallet::StoredQueuedExecution {
            pid,
            payload_hash: payload_hash.0,
            payload_len,
            class: ProposalClass::Treasury,
            maturity,
            grace_end,
            version_constraint,
            meters_declared: Default::default(),
            ratify_ref: None,
            ratification_passed: false,
            attestation_id: None,
            pre_upgrade_checkpoint: None,
            cancelled: false,
            declared_domains,
            failed_at: None,
        },
        false,
    ));
    Some(maturity)
}

pub(crate) fn seed_parachain_upgrade_boundary(candidate_len: usize) {
    let max_code_size = u32::try_from(candidate_len).map_or(u32::MAX, |len| len.saturating_add(1));
    cumulus_pallet_parachain_system::ValidationData::<Runtime>::put(
        cumulus_primitives_core::PersistedValidationData::default(),
    );
    cumulus_pallet_parachain_system::HostConfiguration::<Runtime>::put(
        cumulus_primitives_core::AbridgedHostConfiguration {
            max_code_size,
            max_head_data_size: 0,
            max_upward_queue_count: 0,
            max_upward_queue_size: 0,
            max_upward_message_size: 0,
            max_upward_message_num_per_candidate: 0,
            hrmp_max_message_num_per_candidate: 0,
            validation_upgrade_cooldown: 0,
            validation_upgrade_delay: 0,
            async_backing_params: cumulus_primitives_core::relay_chain::AsyncBackingParams {
                max_candidate_depth: 0,
                allowed_ancestry_len: 0,
            },
        },
    );
    cumulus_pallet_parachain_system::UpgradeRestrictionSignal::<Runtime>::kill();
}

fn submit_relay_upgrade_go_ahead() {
    submit_relay_upgrade_signal(cumulus_primitives_core::relay_chain::UpgradeGoAhead::GoAhead);
}

fn submit_relay_upgrade_abort() {
    submit_relay_upgrade_signal(cumulus_primitives_core::relay_chain::UpgradeGoAhead::Abort);
}

fn submit_relay_upgrade_signal(signal: cumulus_primitives_core::relay_chain::UpgradeGoAhead) {
    let builder = cumulus_test_relay_sproof_builder::RelayStateSproofBuilder {
        para_id: futarchy_primitives::chain_identity::FIXTURE_PARA_ID.into(),
        upgrade_go_ahead: Some(signal),
        included_para_head: Some(cumulus_primitives_core::relay_chain::HeadData(Vec::new())),
        ..Default::default()
    };
    let (relay_parent_storage_root, relay_chain_state) = builder.into_state_root_and_proof();
    let data = cumulus_pallet_parachain_system::parachain_inherent::BasicParachainInherentData {
        validation_data: cumulus_primitives_core::PersistedValidationData {
            relay_parent_number: 1,
            relay_parent_storage_root,
            ..Default::default()
        },
        relay_chain_state,
        relay_parent_descendants: Default::default(),
        collator_peer_id: None,
    };
    let inbound = cumulus_pallet_parachain_system::parachain_inherent::InboundMessagesData::new(
        Default::default(),
        Default::default(),
    );
    // `seed_parachain_upgrade_boundary` models the scheduling block. The real
    // next-block initialize removes its validation data before this inherent.
    cumulus_pallet_parachain_system::ValidationData::<Runtime>::kill();
    assert_ok!(ParachainSystem::set_validation_data(
        RuntimeOrigin::none(),
        data,
        inbound,
    ));
}

pub(crate) fn remark() -> RuntimeCall {
    RuntimeCall::System(frame_system::Call::remark { remark: vec![1] })
}

pub(crate) fn set_pending_upgrade(applicable_at: Option<BlockNumber>) {
    match applicable_at {
        Some(applicable_at) => {
            pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::put(
                pallet_execution_guard::PendingUpgrade {
                    hash: sp_io::hashing::blake2_256(&[1]),
                    authorized_at: applicable_at
                        .saturating_sub(kernel::DESCRIPTOR_LEAD_TIME_BLOCKS),
                    applicable_at,
                    target_spec_version: VERSION.spec_version.saturating_add(1),
                },
            );
        }
        None => pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::kill(),
    }
}

pub(crate) fn nobody_system_calls() -> Vec<RuntimeCall> {
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
        RuntimeCall::System(frame_system::Call::authorize_upgrade {
            code_hash: H256::repeat_byte(8),
        }),
        RuntimeCall::System(frame_system::Call::authorize_upgrade_without_checks {
            code_hash: H256::repeat_byte(9),
        }),
    ]
}

pub(crate) fn closed_wrappers(call: RuntimeCall) -> Vec<RuntimeCall> {
    let who = account(7);
    let signed_origin: <RuntimeOrigin as frame_support::traits::OriginTrait>::PalletsOrigin =
        frame_system::RawOrigin::Signed(who.clone()).into();
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
        RuntimeCall::Utility(pallet_utility::Call::as_derivative {
            index: 0,
            call: Box::new(call.clone()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::dispatch_as {
            as_origin: Box::new(signed_origin.clone()),
            call: Box::new(call.clone()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::with_weight {
            call: Box::new(call.clone()),
            weight: Weight::zero(),
        }),
        RuntimeCall::Utility(pallet_utility::Call::if_else {
            main: Box::new(call.clone()),
            fallback: Box::new(remark()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::if_else {
            main: Box::new(remark()),
            fallback: Box::new(call.clone()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::dispatch_as_fallible {
            as_origin: Box::new(signed_origin),
            call: Box::new(call.clone()),
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
    assert_pallet!(Vesting, 14, "Vesting");
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
    assert_pallet!(Epoch, 61, "Epoch");
    assert_pallet!(ExecutionGuard, 62, "ExecutionGuard");
    assert_eq!(
        <AllPalletsWithSystem as PalletsInfoAccess>::infos().len(),
        40
    );
}

#[test]
fn epoch_clock_is_live_across_sibling_configs() {
    use frame_support::traits::Get;

    development_ext().execute_with(|| {
        pallet_epoch::EpochOf::<Runtime>::mutate(|epoch| epoch.index = 7);
        assert_eq!(Epoch::current_epoch(), 7);
        assert_eq!(pallet_epoch::CurrentEpoch::<Runtime>::get(), 7);
        assert_eq!(
            <<Runtime as pallet_welfare::Config>::CurrentEpoch as Get<u32>>::get(),
            7
        );
        assert_eq!(
            <<Runtime as pallet_guardian::Config>::CurrentEpoch as Get<u32>>::get(),
            7
        );
        assert_eq!(
            <<Runtime as pallet_futarchy_treasury::Config>::CurrentEpoch as Get<u32>>::get(),
            7
        );
    });
}

#[test]
fn execution_guard_enqueue_rejects_signed_callers() {
    development_ext().execute_with(|| {
        let version = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => return assert!(false, "guard genesis must seed its runtime version"),
        };
        let item = pallet_execution_guard::StoredQueuedExecution {
            pid: 1,
            payload_hash: [1; 32],
            payload_len: 0,
            class: ProposalClass::Param,
            maturity: 1,
            grace_end: 2,
            version_constraint: version,
            meters_declared: Default::default(),
            ratify_ref: None,
            ratification_passed: false,
            attestation_id: None,
            pre_upgrade_checkpoint: None,
            cancelled: false,
            declared_domains: Default::default(),
            failed_at: None,
        };
        assert_eq!(
            ExecutionGuard::enqueue(RuntimeOrigin::signed(account(77)), item, false),
            Err(DispatchError::BadOrigin)
        );
    });
}

#[test]
fn guard_rejects_best_effort_wrappers_and_admits_atomic_batch_all() {
    use pallet_execution_guard::BatchDispatcher;

    development_ext().execute_with(|| {
        let leaf = RuntimeCall::Constitution(pallet_constitution::Call::set_param {
            key: pallet_constitution::key16(b"mkt.obs_interval"),
            value: pallet_constitution::ParamValue::U32(10),
        });
        let batch = RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![leaf.clone()],
        });
        let force_batch = RuntimeCall::Utility(pallet_utility::Call::force_batch {
            calls: vec![leaf.clone()],
        });
        let batch_all = RuntimeCall::Utility(pallet_utility::Call::batch_all { calls: vec![leaf] });
        assert!(!RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &batch
        ));
        assert!(!RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &force_batch
        ));
        assert!(RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &batch_all
        ));
        pallet_epoch::EpochOf::<Runtime>::mutate(|epoch| epoch.index = 10);
        assert!(RuntimeDispatcher::dispatch_with_class_origin(
            batch_all.clone(),
            ProposalClass::Param,
        )
        .is_ok());
        pallet_constitution::Capabilities::<Runtime>::mutate(|rows| {
            if let Some(row) = rows.iter_mut().find(|row| {
                row.class == ProposalClass::Param
                    && row.capability
                        == pallet_constitution::Capability::SetParam(pallet_constitution::key16(
                            b"mkt.obs_interval",
                        ))
            }) {
                row.enabled = false;
            }
        });
        assert!(!RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &batch_all
        ));
        assert!(RuntimeDispatcher::dispatch_with_class_origin(
            batch_all.clone(),
            ProposalClass::Param,
        )
        .is_err());
        pallet_constitution::Capabilities::<Runtime>::mutate(|rows| {
            if let Some(row) = rows.iter_mut().find(|row| {
                row.class == ProposalClass::Param
                    && row.capability
                        == pallet_constitution::Capability::SetParam(pallet_constitution::key16(
                            b"mkt.obs_interval",
                        ))
            }) {
                row.enabled = true;
            }
        });
        let live_epoch = pallet_epoch::EpochOf::<Runtime>::get().index;
        pallet_welfare::GateBreachFlags::<Runtime>::insert(
            live_epoch,
            pallet_welfare::CoreGateBreachFlags {
                s_breached: true,
                c_breached: false,
                day_bitmap: [1, 0],
            },
        );
        assert!(!RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &batch_all
        ));
        // PR #66 Codex P1: only the CURRENT epoch's gate record freezes
        // execution. A breached record retained from a prior epoch (welfare's
        // rolling window; pruning is keeper-driven) must auto-release once the
        // epoch has moved on (06 §5).
        pallet_epoch::EpochOf::<Runtime>::mutate(|epoch| epoch.index = live_epoch + 1);
        assert!(RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &batch_all
        ));
        pallet_epoch::EpochOf::<Runtime>::mutate(|epoch| epoch.index = live_epoch);
        pallet_welfare::GateBreachFlags::<Runtime>::remove(live_epoch);
        pallet_constitution::PhaseFlags::<Runtime>::mutate(|flags| {
            *flags |= pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED;
        });
        assert!(!RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &batch_all
        ));
    });
}

#[test]
fn epoch_to_guard_terminal_dequeue_cleans_guard_owned_state() {
    use pallet_epoch::ExecutionGuardAccess;

    development_ext().execute_with(|| {
        let version = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => return assert!(false, "guard genesis must seed its runtime version"),
        };
        let pid = 91;
        let payload_hash = [91; 32];
        pallet_execution_guard::Queue::<Runtime>::insert(
            pid,
            pallet_execution_guard::StoredQueuedExecution {
                pid,
                payload_hash,
                payload_len: 0,
                class: ProposalClass::Param,
                maturity: 1,
                grace_end: 2,
                version_constraint: version,
                meters_declared: Default::default(),
                ratify_ref: None,
                ratification_passed: false,
                attestation_id: None,
                pre_upgrade_checkpoint: None,
                cancelled: false,
                declared_domains: Default::default(),
                failed_at: None,
            },
        );
        pallet_execution_guard::Expedited::<Runtime>::insert(pid, true);
        pallet_execution_guard::AttestationBindings::<Runtime>::insert(pid, (7, payload_hash));
        pallet_execution_guard::Ratifications::<Runtime>::insert(
            pid,
            pallet_execution_guard::RatificationRecord {
                referendum_index: 4,
                payload_hash,
                ratified_at: 1,
            },
        );
        assert!(crate::configs::RuntimeEpochGuard::dequeue_terminal(pid).is_ok());
        assert!(!pallet_execution_guard::Queue::<Runtime>::contains_key(pid));
        assert!(!pallet_execution_guard::Expedited::<Runtime>::contains_key(
            pid
        ));
        assert!(!pallet_execution_guard::AttestationBindings::<Runtime>::contains_key(pid));
        assert!(!pallet_execution_guard::Ratifications::<Runtime>::contains_key(pid));
    });
}

#[test]
fn epoch_enqueue_pins_preimage_and_converts_grace_duration_to_deadline() {
    use pallet_epoch::ExecutionGuardAccess;
    use pallet_execution_guard::Params;
    use sp_runtime::traits::Hash as HashT;

    development_ext().execute_with(|| {
        System::set_block_number(1);
        let batch = match pallet_execution_guard::RuntimeBatch::<Runtime>::try_from(vec![remark()])
        {
            Ok(batch) => batch,
            Err(_) => return assert!(false, "one call must fit the guard batch bound"),
        };
        let bytes = batch.encode();
        let payload_hash = <Runtime as frame_system::Config>::Hashing::hash(&bytes).0;
        let proposer = Sr25519Keyring::Alice.to_account_id();
        assert!(
            Preimage::note_preimage(RuntimeOrigin::signed(proposer.clone()), bytes.clone()).is_ok()
        );
        let version = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => return assert!(false, "guard genesis must seed its runtime version"),
        };
        let pid = 92;
        pallet_epoch::Proposals::<Runtime>::insert(
            pid,
            futarchy_primitives::Proposal {
                id: pid,
                proposer,
                class: ProposalClass::Param,
                state: futarchy_primitives::ProposalState::Queued,
                epoch: 1,
                submitted_at: 0,
                payload_hash,
                payload_len: bytes.len() as u32,
                ask: 0,
                bond: 1,
                resources: Default::default(),
                metric_spec: 1,
                decide_at: 0,
                rerun: false,
                extended: false,
                delayed_once: false,
                markets: None,
                maturity: None,
                grace_end: None,
                version_constraint: Some(version.clone()),
                decision: None,
            },
        );
        let maturity = System::block_number()
            + crate::configs::RuntimeGuardParams::exec_timelock(ProposalClass::Param);
        let grace = crate::configs::RuntimeGuardParams::exec_grace(ProposalClass::Param);
        assert!(crate::configs::RuntimeEpochGuard::enqueue(
            pid,
            payload_hash,
            Some(version),
            maturity,
            grace,
            false,
        )
        .is_ok());
        assert!(
            <Preimage as frame_support::traits::QueryPreimage>::is_requested(&payload_hash.into())
        );
        assert_eq!(
            pallet_execution_guard::Queue::<Runtime>::get(pid).map(|queued| queued.grace_end),
            Some(maturity + grace)
        );
        assert!(crate::configs::RuntimeEpochGuard::dequeue_terminal(pid).is_ok());
        assert!(
            !<Preimage as frame_support::traits::QueryPreimage>::is_requested(&payload_hash.into())
        );
    });
}

#[test]
fn explicitly_pending_b5_inputs_remain_fail_closed() {
    use pallet_epoch::{ConstitutionAccess, GuardianAccess, MarketAccess};
    use pallet_guardian::{GuardianRecallScheduler, GuardianReviewScheduler};
    use pallet_oracle::ReportingContext;
    use pallet_registry::EpochContext;
    use pallet_welfare::MetricInputs;

    development_ext().execute_with(|| {
        System::set_block_number(1);
        let proposal = futarchy_primitives::Proposal {
            id: 1,
            proposer: account(1),
            class: ProposalClass::Param,
            state: futarchy_primitives::ProposalState::Submitted,
            epoch: 1,
            submitted_at: 0,
            payload_hash: [0; 32],
            payload_len: 0,
            ask: 0,
            bond: 0,
            resources: Default::default(),
            metric_spec: 1,
            decide_at: 0,
            rerun: false,
            extended: false,
            delayed_once: false,
            markets: None,
            maturity: None,
            grace_end: None,
            version_constraint: None,
            decision: None,
        };
        assert_eq!(crate::configs::RuntimeReporting::stake_at_risk(1, 1), 0);
        assert!(!crate::configs::RuntimeReporting::is_expected_spec_version(
            1, 1, 1
        ));
        assert_eq!(crate::configs::RuntimeEpochMarket::twap_full(1), None);
        assert_eq!(
            crate::configs::RuntimeEpochMarket::twap_trailing(1, 1),
            None
        );
        assert!(!crate::configs::RuntimeEpochMarket::decision_grade(
            1,
            pallet_epoch::BookRole::Decision,
            ProposalClass::Param,
            &pallet_epoch::CoreEpochParams::DEFAULT,
        ));
        assert_eq!(crate::configs::RuntimeEpochMarket::measured_depth(1), 0);
        assert_eq!(
            crate::configs::RuntimeEpochMarket::published_flow_per_day(1),
            None
        );
        assert_eq!(
            crate::configs::RuntimeEpochMarket::previous_settled_baseline_twap(1),
            None
        );
        assert!(crate::configs::RuntimeEpochMarket::open_markets(&proposal, false, false).is_err());
        assert!(crate::configs::RuntimeMetricInputs::daily_components(1, 1, 0).is_empty());
        assert_eq!(
            crate::configs::RuntimeRegistryEpoch::frozen_spec_version(1),
            0
        );
        assert_eq!(
            crate::configs::RuntimeRegistryEpoch::filing_window_end(1),
            0
        );
        assert_eq!(crate::configs::RuntimeRegistryEpoch::milestone_target(1), 0);
        assert!(!crate::configs::RuntimeEpochConstitution::queue_time_check(
            &proposal
        ));
        assert_eq!(
            crate::configs::RuntimeEpochConstitution::in_cap_prize(&proposal),
            None
        );
        assert!(!crate::configs::RuntimeEpochGuardian::review_window_closed(
            1
        ));
        let triggers =
            <crate::configs::RuntimeGuardianTriggers as pallet_guardian::GuardianTriggers>::current(
            );
        assert!(!triggers.depeg);
        assert!(!triggers.oracle_deadlock);
        assert!(!triggers.ledger_drift);
        assert_eq!(
            crate::configs::PendingGuardianReviewScheduler::schedule_review(1),
            u32::MAX
        );
        assert_eq!(
            crate::configs::PendingGuardianRecallScheduler::schedule_recall(1),
            u32::MAX
        );
    });
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
    assert_eq!(FEE_VIT_USDC_RATE_KEY, *b"fee.vit_usdc\0\0\0\0");
    assert_eq!(VERSION.spec_name.as_ref(), "bleavit");
    assert_eq!(VERSION.impl_name.as_ref(), "bleavit-runtime");
    assert_eq!(VERSION.spec_version, 1);
    assert_eq!(
        VERSION.transaction_version,
        futarchy_primitives::INTEGRATION_CONTRACT_VERSION
    );
    assert_eq!(VERSION.transaction_version, 3);
    assert_eq!(usdc_location().encode(), USDC_LOCATION_ENCODED);
}

#[test]
fn usdc_admin_and_fee_posture_is_fail_closed() {
    let create = RuntimeCall::ForeignAssets(pallet_assets::Call::create {
        id: usdc_location(),
        admin: MultiAddress::Id(account(1)),
        min_balance: currency::USDC_CENT,
    });
    let mint = RuntimeCall::ForeignAssets(pallet_assets::Call::mint {
        id: usdc_location(),
        beneficiary: MultiAddress::Id(account(2)),
        amount: currency::USDC_CENT,
    });
    // SQ-151: the bare scheduler leaf must clear the origin-blind base filter;
    // the pallet's CreateOrigin remains the independent authority check.
    assert!(RuntimeBaseCallFilter::contains(&create));
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
        assert!(crate::configs::LiveFeeConversion::to_asset_balance(1, usdc_location()).is_err());
        let other_asset = bleavit_xcm::identity::asset_hub_asset_location(
            chain_identity::USDC_ASSET_INDEX.saturating_add(1),
        );
        assert!(crate::configs::LiveFeeConversion::to_asset_balance(1, other_asset).is_err());
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
            crate::configs::LiveFeeConversion::to_asset_balance(currency::VIT, usdc_location()),
            Ok(2 * currency::USDC)
        );
        assert_eq!(
            crate::configs::LiveFeeConversion::to_asset_balance(1, usdc_location()),
            Ok(1)
        );
        assert_eq!(
            crate::configs::LiveFeeConversion::to_asset_balance(0, usdc_location()),
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
        assert!(ForeignAssets::asset_exists(usdc_location()));
        assert_eq!(
            ForeignAssets::minimum_balance(usdc_location()),
            currency::USDC_CENT
        );
        let details =
            pallet_assets::Asset::<Runtime, pallet_assets::Instance1>::get(usdc_location());
        assert!(details.is_some_and(|asset| asset.is_sufficient));
        let metadata =
            pallet_assets::Metadata::<Runtime, pallet_assets::Instance1>::get(usdc_location());
        assert_eq!(metadata.decimals, currency::USDC_DECIMALS);
        assert_eq!(
            Balances::minimum_balance(),
            currency::VIT_EXISTENTIAL_DEPOSIT
        );
        assert_eq!(Balances::total_issuance(), currency::VIT_TOTAL_SUPPLY);
    });
}

#[test]
fn usdc_storage_keys_match_the_frozen_surface_manifest() {
    fn storage_key(item: &[u8], encoded_location: &[u8]) -> Vec<u8> {
        let mut key = Vec::with_capacity(64 + 16 + encoded_location.len());
        key.extend_from_slice(&sp_core::hashing::twox_128(b"ForeignAssets"));
        key.extend_from_slice(&sp_core::hashing::twox_128(item));
        key.extend_from_slice(&sp_core::hashing::blake2_128(encoded_location));
        key.extend_from_slice(encoded_location);
        key
    }

    let encoded_location = usdc_location().encode();
    let asset_key = storage_key(b"Asset", &encoded_location);
    let metadata_key = storage_key(b"Metadata", &encoded_location);

    assert_eq!(
        format!("0x{}", sp_core::hexdisplay::HexDisplay::from(&asset_key)),
        "0x30e64a56026f4b5e3c2d196283a9a17dd34371a193a751eea5883e9553457b2e484550ecc01d89e5e7bb33be1915aaef010300a10f043205e514"
    );
    assert_eq!(
        format!("0x{}", sp_core::hexdisplay::HexDisplay::from(&metadata_key)),
        "0x30e64a56026f4b5e3c2d196283a9a17db5f3822e35ca2f31ce3526eab1363fd2484550ecc01d89e5e7bb33be1915aaef010300a10f043205e514"
    );

    development_ext().execute_with(|| {
        assert_eq!(
            pallet_assets::Asset::<Runtime, pallet_assets::Instance1>::hashed_key_for(
                usdc_location()
            ),
            asset_key
        );
        assert_eq!(
            pallet_assets::Metadata::<Runtime, pallet_assets::Instance1>::hashed_key_for(
                usdc_location()
            ),
            metadata_key
        );
        assert!(ForeignAssets::asset_exists(usdc_location()));
    });
}

#[test]
fn development_allocations_match_the_genesis_economics_exactly() {
    use crate::genesis::{
        community_account, incentives_account, treasury_account, ALICE_PUBLIC, BOB_PUBLIC,
        CHARLIE_PUBLIC, COMMUNITY_DISTRIBUTION, DAVE_PUBLIC, ECOSYSTEM_OPS, ECOSYSTEM_OPS_ACCOUNT,
        FOUNDING_TEAM, FOUNDING_TEAM_ACCOUNT, INCENTIVE_PROGRAMS, TREASURY_RESERVE,
    };

    assert_eq!(
        TREASURY_RESERVE
            + COMMUNITY_DISTRIBUTION
            + FOUNDING_TEAM
            + ECOSYSTEM_OPS
            + INCENTIVE_PROGRAMS,
        currency::VIT_TOTAL_SUPPLY
    );

    development_ext().execute_with(|| {
        assert_eq!(Balances::free_balance(treasury_account()), TREASURY_RESERVE);
        assert_eq!(
            Balances::free_balance(community_account()),
            COMMUNITY_DISTRIBUTION
        );
        assert_eq!(
            Balances::free_balance(incentives_account()),
            INCENTIVE_PROGRAMS
        );
        for public in [CHARLIE_PUBLIC, DAVE_PUBLIC] {
            assert_eq!(
                Balances::free_balance(AccountId::new(public)),
                FOUNDING_TEAM_ACCOUNT
            );
        }
        for public in [ALICE_PUBLIC, BOB_PUBLIC] {
            assert_eq!(
                Balances::free_balance(AccountId::new(public)),
                ECOSYSTEM_OPS_ACCOUNT
            );
        }
        assert_eq!(Balances::total_issuance(), currency::VIT_TOTAL_SUPPLY);
    });
}

#[test]
fn treasury_rebate_payout_moves_real_usdc_from_the_selected_pot() {
    use crate::configs::{treasury_keeper_account, treasury_oracle_account, TreasuryRebatePayout};
    use pallet_futarchy_treasury::{PayoutLine, RebatePayout, TreasuryParams as _};

    development_ext().execute_with(|| {
        // `keeper.rebate` is deliberately unseeded until B5 calibration.
        assert_eq!(crate::configs::TreasuryParams::keeper_rebate(), 0);
        assert_eq!(
            crate::configs::TreasuryParams::keeper_budget_epoch(),
            12_000 * currency::USDC
        );

        let keeper = account(77);
        let keeper_pot = treasury_keeper_account();
        let oracle_pot = treasury_oracle_account();
        let amount = 10 * currency::USDC;
        let retained = currency::USDC_CENT;
        assert!(<ForeignAssets as FungiblesMutate<AccountId>>::mint_into(
            usdc_location(),
            &keeper_pot,
            amount + retained,
        )
        .is_ok());
        assert!(<ForeignAssets as FungiblesMutate<AccountId>>::mint_into(
            usdc_location(),
            &oracle_pot,
            amount + retained,
        )
        .is_ok());
        assert_eq!(
            TreasuryRebatePayout::pot_balance(PayoutLine::Keeper),
            amount + retained
        );
        assert_eq!(
            TreasuryRebatePayout::pot_balance(PayoutLine::Oracle),
            amount + retained
        );

        assert!(<TreasuryRebatePayout as RebatePayout<AccountId>>::pay(
            &keeper,
            amount,
            PayoutLine::Keeper,
        )
        .is_ok());
        assert_eq!(ForeignAssets::balance(usdc_location(), &keeper), amount);
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &keeper_pot),
            retained
        );

        assert!(<TreasuryRebatePayout as RebatePayout<AccountId>>::pay(
            &keeper,
            amount,
            PayoutLine::Oracle,
        )
        .is_ok());
        assert_eq!(ForeignAssets::balance(usdc_location(), &keeper), 2 * amount);
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &oracle_pot),
            retained
        );
    });
}

#[test]
fn development_key_constants_match_the_well_known_sr25519_keys() {
    assert_eq!(
        crate::genesis::ALICE_PUBLIC,
        Sr25519Keyring::Alice.to_raw_public()
    );
    assert_eq!(
        crate::genesis::BOB_PUBLIC,
        Sr25519Keyring::Bob.to_raw_public()
    );
    assert_eq!(
        crate::genesis::CHARLIE_PUBLIC,
        Sr25519Keyring::Charlie.to_raw_public()
    );
    assert_eq!(
        crate::genesis::DAVE_PUBLIC,
        Sr25519Keyring::Dave.to_raw_public()
    );
}

#[test]
fn team_allocations_are_transfer_locked() {
    development_ext().execute_with(|| {
        let alice = Sr25519Keyring::Alice.to_account_id();
        for team_member in [
            Sr25519Keyring::Charlie.to_account_id(),
            Sr25519Keyring::Dave.to_account_id(),
        ] {
            assert_eq!(Balances::usable_balance(&team_member), 0);
            let transfer = RuntimeCall::Balances(pallet_balances::Call::transfer_allow_death {
                dest: MultiAddress::Id(alice.clone()),
                value: 1,
            });
            assert!(transfer
                .dispatch(RuntimeOrigin::signed(team_member.clone()))
                .is_err());
            assert_eq!(
                Balances::free_balance(&team_member),
                crate::genesis::FOUNDING_TEAM_ACCOUNT
            );
        }
    });
}

#[test]
fn fully_vesting_locked_account_cannot_pay_native_transaction_fees() {
    type NativeFeeCharger = <Runtime as pallet_transaction_payment::Config>::OnChargeTransaction;

    development_ext().execute_with(|| {
        let charlie = Sr25519Keyring::Charlie.to_account_id();
        let fee_call = remark();
        let dispatch_info = fee_call.get_dispatch_info();
        let result = <NativeFeeCharger as pallet_transaction_payment::OnChargeTransaction<
            Runtime,
        >>::withdraw_fee(&charlie, &fee_call, &dispatch_info, 1, 0);
        assert!(matches!(
            result,
            Err(TransactionValidityError::Invalid(
                InvalidTransaction::Payment
            ))
        ));
        assert_eq!(
            Balances::free_balance(&charlie),
            crate::genesis::FOUNDING_TEAM_ACCOUNT
        );
        assert!(Balances::locks(&charlie)
            .iter()
            .any(|lock| lock.id == *b"vesting "));
    });
}

#[test]
fn team_vesting_curve_is_cliffed_and_never_faster_than_the_ideal_curve() {
    let charlie = Sr25519Keyring::Charlie.to_account_id();
    let year = crate::genesis::BLOCKS_PER_YEAR;
    let total = crate::genesis::FOUNDING_TEAM_ACCOUNT;
    let horizon = 4 * year;

    development_ext().execute_with(|| {
        let locked_at = |block| {
            System::set_block_number(block);
            match Vesting::vesting_balance(&charlie) {
                Some(locked) => locked,
                None => {
                    assert!(false, "Charlie must have a genesis vesting schedule");
                    0
                }
            }
        };

        assert_eq!(locked_at(0), total);
        assert_eq!(locked_at(year - 1), total);
        assert_eq!(locked_at(year), total);

        let mut unlocked_samples = Vec::new();
        for block in [year, 2 * year, 3 * year, horizon] {
            let unlocked = total - locked_at(block);
            assert!(
                unlocked * crate::Balance::from(horizon) <= total * crate::Balance::from(block),
                "genesis vesting must never dominate the ideal t/4 unlock curve"
            );
            unlocked_samples.push(unlocked);
        }
        assert!(unlocked_samples.windows(2).all(|pair| pair[0] < pair[1]));

        // pallet-vesting floors `per_block` during genesis construction. The
        // exact 100M allocation is not divisible by the exact three-year block
        // length, so a sub-VIT remainder conservatively clears one block after
        // the nominal four-year horizon rather than one block before it.
        let duration = 3 * year;
        let per_block = total / crate::Balance::from(duration);
        let rounding_tail = total - per_block * crate::Balance::from(duration);
        assert_eq!(locked_at(horizon), rounding_tail);
        assert!(rounding_tail > 0);
        assert_eq!(locked_at(horizon + 1), 0);
    });
}

#[test]
fn vesting_force_calls_are_nobody_and_public_calls_remain_public() {
    let schedule = pallet_vesting::VestingInfo::new(currency::VIT, 1, 0);
    let force_calls = [
        RuntimeCall::Vesting(pallet_vesting::Call::force_vested_transfer {
            source: MultiAddress::Id(account(1)),
            target: MultiAddress::Id(account(2)),
            schedule,
        }),
        RuntimeCall::Vesting(pallet_vesting::Call::force_remove_vesting_schedule {
            target: MultiAddress::Id(account(1)),
            schedule_index: 0,
        }),
    ];
    for call in force_calls {
        assert!(!RuntimeBaseCallFilter::contains(&call));
        for origin in pallet_origins::Origin::ALL {
            assert!(!RuntimeBaseCallFilter::contains_for(
                origin.to_model(),
                &call
            ));
        }
        for wrapped in closed_wrappers(call) {
            assert!(!RuntimeBaseCallFilter::contains(&wrapped));
        }
    }

    assert!(RuntimeBaseCallFilter::contains(&RuntimeCall::Vesting(
        pallet_vesting::Call::vest {}
    )));
    assert!(RuntimeBaseCallFilter::contains(&RuntimeCall::Vesting(
        pallet_vesting::Call::vested_transfer {
            target: MultiAddress::Id(account(2)),
            schedule,
        }
    )));
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
    let calls = nobody_system_calls();
    for call in &calls {
        assert!(!RuntimeBaseCallFilter::contains(call));
        for wrapped in closed_wrappers(call.clone()) {
            assert!(!RuntimeBaseCallFilter::contains(&wrapped));
        }
    }
    development_ext().execute_with(|| {
        for call in calls {
            let result = call.clone().dispatch(RuntimeOrigin::signed(account(70)));
            assert!(matches!(result, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));
            for wrapped in closed_wrappers(call) {
                let result = wrapped.dispatch(RuntimeOrigin::signed(account(70)));
                assert!(matches!(result, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));
            }
        }
    });
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
fn bare_system_upgrade_calls_stay_denied_when_guard_descriptor_matures() {
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

    upgrade_ext().execute_with(|| {
        let apply =
            RuntimeCall::System(frame_system::Call::apply_authorized_upgrade { code: vec![1] });
        System::set_block_number(10);
        seed_parachain_upgrade_boundary(1);
        set_pending_upgrade(None);
        assert!(!RuntimeBaseCallFilter::contains(&apply));
        for wrapped in closed_wrappers(apply.clone()) {
            assert!(!RuntimeBaseCallFilter::contains(&wrapped));
        }
        set_pending_upgrade(Some(11));
        assert!(!RuntimeBaseCallFilter::contains(&apply));
        set_pending_upgrade(Some(10));
        assert!(RuntimeBaseCallFilter::contains(&apply));
        assert!(RuntimeBaseCallFilter::contains(&RuntimeCall::Utility(
            pallet_utility::Call::batch {
                calls: vec![apply.clone()],
            }
        )));
        set_pending_upgrade(Some(9));
        assert!(RuntimeBaseCallFilter::contains(&apply));
        set_pending_upgrade(None);
    });
}

#[test]
fn upgrade_path_authorizes_schedules_and_clears_only_after_validation_code_applies() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_001;
        const RATIFY_REF: u32 = 71;
        let candidate = b"bleavit-b6-candidate-runtime-v2".to_vec();
        let (maturity, artifact) = match enqueue_attested_code_upgrade(PID, &candidate, RATIFY_REF) {
            Some(setup) => setup,
            None => {
                assert!(false, "attested upgrade fixture must be constructible");
                return;
            }
        };

        System::set_block_number(maturity);
        let release_before = release_channel_raw();
        let checkpoint_parent = System::parent_hash();
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(75)),
            PID,
        ));

        let authorization = System::authorized_upgrade();
        assert!(authorization
            .is_some_and(|authorization| authorization.code_hash() == &artifact));
        let pending = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending,
            None => {
                assert!(false, "successful CODE execution must create PendingUpgrade");
                return;
            }
        };
        assert_eq!(pending.hash, artifact.0);
        assert_eq!(pending.authorized_at, maturity);
        assert_eq!(
            pending.applicable_at,
            maturity.saturating_add(kernel::DESCRIPTOR_LEAD_TIME_BLOCKS)
        );
        assert_eq!(
            pending.target_spec_version,
            VERSION.spec_version.saturating_add(1)
        );
        let checkpoint = pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get();
        assert!(checkpoint.is_some_and(|(parent, state_root)| {
            parent == checkpoint_parent.0 && state_root != [0; 32]
        }));
        assert!(System::events().iter().any(|record| matches!(
            &record.event,
            crate::RuntimeEvent::ExecutionGuard(
                pallet_execution_guard::Event::UpgradeAuthorized {
                    code_hash,
                    authorized_at,
                }
            ) if *code_hash == artifact.0 && *authorized_at == maturity
        )));

        let raw = match release_channel_raw() {
            Some(raw) => raw,
            None => {
                assert!(false, "frozen ReleaseChannel raw key must exist");
                return;
            }
        };
        assert_eq!(raw.len(), pallet_constitution::RELEASE_CHANNEL_LEN);
        assert_eq!(raw_u32(&raw, 108), Some(maturity));
        assert_eq!(raw_u32(&raw, 112), Some(pending.target_spec_version));
        assert_eq!(raw_u32(&raw, 116), Some(maturity));
        assert!(raw_u32(&raw, 164).is_some_and(|flags| flags & (1 << 2) != 0));
        if let Some(before) = release_before {
            assert_raw_unchanged_outside(&before, &raw, &[108..120, 164..168]);
            assert_eq!(
                raw_u32(&before, 164).map(|flags| flags & !(1 << 2)),
                raw_u32(&raw, 164).map(|flags| flags & !(1 << 2))
            );
        } else {
            assert!(false, "genesis ReleaseChannel raw key must exist");
        }

        let system_apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: candidate.clone(),
        });
        System::set_block_number(pending.applicable_at.saturating_sub(1));
        assert!(!RuntimeBaseCallFilter::contains(&system_apply));
        let early = system_apply
            .clone()
            .dispatch(RuntimeOrigin::signed(account(76)));
        assert!(matches!(early, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));
        assert!(System::authorized_upgrade().is_some());

        System::set_block_number(pending.applicable_at);
        let wrong_apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: b"wrong-authorized-artifact".to_vec(),
        });
        assert!(!RuntimeBaseCallFilter::contains(&wrong_apply));
        let wrong = wrong_apply.dispatch(RuntimeOrigin::signed(account(76)));
        assert!(matches!(wrong, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));
        assert!(System::authorized_upgrade().is_some());

        seed_parachain_upgrade_boundary(candidate.len());
        assert!(RuntimeBaseCallFilter::contains(&system_apply));
        assert!(system_apply
            .dispatch(RuntimeOrigin::signed(account(76)))
            .is_ok());
        assert_eq!(
            cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::get(),
            candidate
        );
        assert_eq!(
            cumulus_pallet_parachain_system::NewValidationCode::<Runtime>::get(),
            Some(candidate.clone())
        );
        assert!(System::authorized_upgrade().is_none());
        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_some());
        let authorized_raw = raw.clone();

        // The next block's guard initialization observes the successful
        // Cumulus schedule before the relay inherent can consume its signal.
        System::set_block_number(System::block_number().saturating_add(1));
        let _ = ExecutionGuard::on_initialize(System::block_number());
        assert_eq!(
            pallet_execution_guard::ScheduledUpgrade::<Runtime>::get(),
            Some(artifact.0)
        );

        // Exercise the production Cumulus boundary: relay-state proof decode,
        // `GoAhead`, `:code` installation, and the configured OnSystemEvent.
        submit_relay_upgrade_go_ahead();

        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get().is_none());
        let applied_raw = match release_channel_raw() {
            Some(raw) => raw,
            None => {
                assert!(false, "ReleaseChannel must survive applied-upgrade callback");
                return;
            }
        };
        assert_eq!(raw_u32(&applied_raw, 108), Some(System::block_number()));
        assert_eq!(raw_u32(&applied_raw, 116), Some(0));
        assert!(raw_u32(&applied_raw, 164).is_some_and(|flags| flags & (1 << 2) == 0));
        assert_raw_unchanged_outside(
            &authorized_raw,
            &applied_raw,
            &[108..112, 116..120, 164..168],
        );
        assert_eq!(
            raw_u32(&authorized_raw, 164).map(|flags| flags & !(1 << 2)),
            raw_u32(&applied_raw, 164).map(|flags| flags & !(1 << 2))
        );
        assert!(System::events().iter().any(|record| matches!(
            &record.event,
            crate::RuntimeEvent::ExecutionGuard(pallet_execution_guard::Event::UpgradeApplied {
                code_hash,
                spec_version,
            }) if *code_hash == artifact.0 && *spec_version == pending.target_spec_version
        )));
        assert!(!System::events().iter().any(|record| matches!(
            &record.event,
            crate::RuntimeEvent::ExecutionGuard(pallet_execution_guard::Event::UpgradeAborted {
                ..
            })
        )));
    });
}

#[test]
fn relay_abort_clears_pending_state_alarms_and_allows_normal_reproposal() {
    use pallet_guardian::GuardianTriggers;

    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_007;
        const RETRY_PID: futarchy_primitives::ProposalId = 6_008;
        let candidate = b"bleavit-b6-relay-aborted-runtime-v2".to_vec();
        let (maturity, artifact) = match enqueue_attested_code_upgrade(PID, &candidate, 77) {
            Some(setup) => setup,
            None => {
                assert!(false, "abort fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(85)),
            PID,
        ));
        let pending = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending,
            None => {
                assert!(false, "abort fixture must authorize an upgrade");
                return;
            }
        };
        System::set_block_number(pending.applicable_at);
        seed_parachain_upgrade_boundary(candidate.len());
        let apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: candidate.clone(),
        });
        assert!(apply.dispatch(RuntimeOrigin::signed(account(86))).is_ok());
        assert!(System::authorized_upgrade().is_none());
        assert!(cumulus_pallet_parachain_system::PendingValidationCode::<
            Runtime,
        >::exists());

        System::set_block_number(System::block_number().saturating_add(1));
        let _ = ExecutionGuard::on_initialize(System::block_number());
        assert_eq!(
            pallet_execution_guard::ScheduledUpgrade::<Runtime>::get(),
            Some(artifact.0)
        );
        let release_before_abort = match release_channel_raw() {
            Some(raw) => raw,
            None => {
                assert!(false, "abort fixture release channel must exist");
                return;
            }
        };

        submit_relay_upgrade_abort();

        assert!(
            cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::get().is_empty()
        );
        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::ScheduledUpgrade::<Runtime>::get().is_none());
        assert!(System::authorized_upgrade().is_none());
        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert!(crate::configs::RuntimeGuardianTriggers::current().migration_halt);
        assert!(System::events().iter().any(|record| matches!(
            &record.event,
            crate::RuntimeEvent::ExecutionGuard(pallet_execution_guard::Event::UpgradeAborted {
                code_hash,
            }) if *code_hash == artifact.0
        )));
        let release_after_abort = match release_channel_raw() {
            Some(raw) => raw,
            None => {
                assert!(false, "abort cleanup must preserve ReleaseChannel");
                return;
            }
        };
        assert_eq!(
            raw_u32(&release_after_abort, 108),
            Some(System::block_number())
        );
        assert_eq!(raw_u32(&release_after_abort, 116), Some(0));
        assert!(raw_u32(&release_after_abort, 164).is_some_and(|flags| flags & (1 << 2) == 0));
        assert_raw_unchanged_outside(
            &release_before_abort,
            &release_after_abort,
            &[108..112, 116..120, 164..168],
        );

        // No callback re-arms frame-system. A fresh proposal must perform the
        // full attestation/queue/ratification/execution path again.
        let spacing_end = pallet_execution_guard::LastUpgradeAuthorized::<Runtime>::get()
            .and_then(|last| {
                last.checked_add(
                    <crate::configs::RuntimeGuardParams as pallet_execution_guard::Params>::code_spacing(),
                )
            })
            .unwrap_or_else(System::block_number);
        System::set_block_number(System::block_number().max(spacing_end));
        let (retry_maturity, _) = match enqueue_attested_code_upgrade(RETRY_PID, &candidate, 78) {
            Some(setup) => setup,
            None => {
                assert!(false, "the aborted artifact must be re-proposable");
                return;
            }
        };
        assert!(System::authorized_upgrade().is_none());
        System::set_block_number(retry_maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(87)),
            RETRY_PID,
        ));
        assert!(System::authorized_upgrade()
            .is_some_and(|authorization| authorization.code_hash() == &artifact));
    });
}

#[test]
fn relay_abort_cleanup_survives_a_writer_b_release_channel_rewrite() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_009;
        let candidate = b"bleavit-b6-abort-writer-b-runtime-v2".to_vec();
        let (maturity, artifact) = match enqueue_attested_code_upgrade(PID, &candidate, 81) {
            Some(setup) => setup,
            None => {
                assert!(false, "writer-b abort fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(88)),
            PID,
        ));
        let pending = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending,
            None => {
                assert!(false, "writer-b abort fixture must authorize an upgrade");
                return;
            }
        };
        System::set_block_number(pending.applicable_at);
        seed_parachain_upgrade_boundary(candidate.len());
        let apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: candidate.clone(),
        });
        assert!(apply.dispatch(RuntimeOrigin::signed(account(89))).is_ok());
        System::set_block_number(System::block_number().saturating_add(1));
        let _ = ExecutionGuard::on_initialize(System::block_number());
        assert_eq!(
            pallet_execution_guard::ScheduledUpgrade::<Runtime>::get(),
            Some(artifact.0)
        );

        // Writer (b) lawfully repoints the channel mid-flight, zeroing the
        // guard-owned pending fields (the SQ-134 interaction). The abort
        // cleanup must tolerate this — never wedge `PendingUpgrade` — and
        // must leave writer (b)'s newer value byte-identical.
        let mut rewritten = [0u8; pallet_constitution::RELEASE_CHANNEL_LEN];
        match release_channel_raw() {
            Some(raw) if raw.len() == rewritten.len() => rewritten.copy_from_slice(&raw),
            _ => {
                assert!(false, "writer-b fixture release channel must exist");
                return;
            }
        }
        rewritten[116..120].copy_from_slice(&0u32.to_le_bytes());
        let flags = raw_u32(&rewritten, 164).unwrap_or(0) & !(1 << 2);
        rewritten[164..168].copy_from_slice(&flags.to_le_bytes());
        assert_ok!(Constitution::set_release_channel(
            pallet_origins::Origin::ConstitutionalValues.into(),
            rewritten,
        ));

        submit_relay_upgrade_abort();

        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::ScheduledUpgrade::<Runtime>::get().is_none());
        assert!(System::events().iter().any(|record| matches!(
            &record.event,
            crate::RuntimeEvent::ExecutionGuard(pallet_execution_guard::Event::UpgradeAborted {
                code_hash,
            }) if *code_hash == artifact.0
        )));
        assert_eq!(release_channel_raw().as_deref(), Some(&rewritten[..]));
    });
}

#[test]
fn applied_cleanup_survives_a_writer_b_release_channel_rewrite() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_010;
        let candidate = b"bleavit-b6-applied-writer-b-runtime-v2".to_vec();
        let (maturity, artifact) = match enqueue_attested_code_upgrade(PID, &candidate, 82) {
            Some(setup) => setup,
            None => {
                assert!(false, "applied writer-b fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(93)),
            PID,
        ));
        let pending = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending,
            None => {
                assert!(false, "applied writer-b fixture must authorize an upgrade");
                return;
            }
        };
        System::set_block_number(pending.applicable_at);
        seed_parachain_upgrade_boundary(candidate.len());
        let apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: candidate.clone(),
        });
        assert!(apply.dispatch(RuntimeOrigin::signed(account(94))).is_ok());
        System::set_block_number(System::block_number().saturating_add(1));
        let _ = ExecutionGuard::on_initialize(System::block_number());

        // Writer (b) lawfully repoints the channel between scheduling and the
        // relay GoAhead, zeroing the guard-owned pending fields. An applied
        // upgrade cannot be retried, so the applied cleanup must tolerate the
        // rewrite (PR #65 P1): guard state records the application, writer
        // (b)'s newer channel value stays byte-identical, and no halt source
        // is raised.
        let mut rewritten = [0u8; pallet_constitution::RELEASE_CHANNEL_LEN];
        match release_channel_raw() {
            Some(raw) if raw.len() == rewritten.len() => rewritten.copy_from_slice(&raw),
            _ => {
                assert!(false, "applied writer-b fixture release channel must exist");
                return;
            }
        }
        rewritten[116..120].copy_from_slice(&0u32.to_le_bytes());
        let flags = raw_u32(&rewritten, 164).unwrap_or(0) & !(1 << 2);
        rewritten[164..168].copy_from_slice(&flags.to_le_bytes());
        assert_ok!(Constitution::set_release_channel(
            pallet_origins::Origin::ConstitutionalValues.into(),
            rewritten,
        ));

        submit_relay_upgrade_go_ahead();

        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::ScheduledUpgrade::<Runtime>::get().is_none());
        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert!(System::events().iter().any(|record| matches!(
            &record.event,
            crate::RuntimeEvent::ExecutionGuard(pallet_execution_guard::Event::UpgradeApplied {
                code_hash,
                ..
            }) if *code_hash == artifact.0
        )));
        assert_eq!(release_channel_raw().as_deref(), Some(&rewritten[..]));
    });
}

#[test]
fn upgrade_apply_without_pending_descriptor_is_filter_denied() {
    development_ext().execute_with(|| {
        let apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: b"no-pending-upgrade".to_vec(),
        });
        assert!(!RuntimeBaseCallFilter::contains(&apply));
        let result = apply.dispatch(RuntimeOrigin::signed(account(77)));
        assert!(matches!(result, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));
    });
}

#[test]
fn system_authorization_survives_cumulus_overlap_preflight_rejection() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_006;
        let candidate = b"bleavit-b6-overlap-preflight-runtime-v2".to_vec();
        let (maturity, artifact) = match enqueue_attested_code_upgrade(PID, &candidate, 76) {
            Some(setup) => setup,
            None => {
                assert!(false, "overlap preflight fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(82)),
            PID,
        ));
        let pending_before = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get()
        {
            Some(pending) => pending,
            None => {
                assert!(false, "CODE execution must leave a guard pending upgrade");
                return;
            }
        };
        let checkpoint_before =
            pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get();
        let release_before = release_channel_raw();
        System::set_block_number(pending_before.applicable_at);
        seed_parachain_upgrade_boundary(candidate.len());
        let existing = b"already-scheduled-validation-code".to_vec();
        cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::put(existing.clone());

        let apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: candidate,
        });
        assert!(!RuntimeBaseCallFilter::contains(&apply));
        let result = apply.dispatch(RuntimeOrigin::signed(account(83)));
        assert!(matches!(result, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));

        assert!(System::authorized_upgrade()
            .is_some_and(|authorization| authorization.code_hash() == &artifact));
        assert_eq!(
            pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get(),
            Some(pending_before)
        );
        assert_eq!(
            pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get(),
            checkpoint_before
        );
        assert_eq!(release_channel_raw(), release_before);
        assert_eq!(
            cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::get(),
            existing
        );
    });
}

#[test]
fn migration_halt_keeps_forward_remediation_upgrade_applicable() {
    use frame_support::migrations::FailedMigrationHandler;

    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_003;
        let candidate = b"bleavit-b6-dispatcher-runtime-v2".to_vec();
        let (maturity, _) = match enqueue_attested_code_upgrade(PID, &candidate, 73) {
            Some(setup) => setup,
            None => {
                assert!(false, "dispatcher upgrade fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(80)),
            PID,
        ));
        let applicable_at = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending.applicable_at,
            None => {
                assert!(false, "dispatcher fixture must authorize an upgrade");
                return;
            }
        };
        System::set_block_number(applicable_at);
        seed_parachain_upgrade_boundary(candidate.len());

        pallet_migrations::Cursor::<Runtime>::put(pallet_migrations::MigrationCursor::Stuck);
        assert_eq!(
            crate::configs::MigrationFailureToGuard::failed(Some(3)),
            frame_support::migrations::FailedMigrationHandling::KeepStuck
        );
        assert!(System::authorized_upgrade().is_some());
        let bounded = match pallet_execution_guard::pallet::RuntimeCode::<Runtime>::try_from(
            candidate.clone(),
        ) {
            Ok(code) => code,
            Err(_) => {
                assert!(false, "remediation runtime must fit the code bound");
                return;
            }
        };
        assert_ok!(ExecutionGuard::apply_authorized_upgrade(
            RuntimeOrigin::signed(account(84)),
            bounded,
        ));
        assert_eq!(
            cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::get(),
            candidate
        );
        assert!(System::authorized_upgrade().is_none());
        assert!(pallet_migrations::Cursor::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_some());
    });
}

#[test]
fn applied_code_alarm_does_not_retire_a_healthy_active_migration_cursor() {
    use cumulus_pallet_parachain_system::OnSystemEvent;

    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_010;
        let candidate = b"bleavit-b6-healthy-active-cursor-runtime-v2".to_vec();
        let (maturity, _) = match enqueue_attested_code_upgrade(PID, &candidate, 79) {
            Some(setup) => setup,
            None => {
                assert!(false, "healthy cursor fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(89)),
            PID,
        ));
        let pending = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending,
            None => {
                assert!(false, "healthy cursor fixture must authorize an upgrade");
                return;
            }
        };
        System::set_block_number(pending.applicable_at);
        seed_parachain_upgrade_boundary(candidate.len());
        let cursor = pallet_migrations::MigrationCursor::Active(pallet_migrations::ActiveCursor {
            index: 0,
            inner_cursor: None,
            started_at: System::block_number(),
        });
        pallet_migrations::Cursor::<Runtime>::put(cursor.clone());
        crate::configs::ExecutionGuardSystemEvent::on_validation_code_applied();
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
        let authorization_hash_before =
            System::authorized_upgrade().map(|authorization| *authorization.code_hash());
        let release_before = release_channel_raw();
        let bounded =
            match pallet_execution_guard::pallet::RuntimeCode::<Runtime>::try_from(candidate) {
                Ok(code) => code,
                Err(_) => {
                    assert!(false, "healthy cursor runtime must fit the code bound");
                    return;
                }
            };

        assert_noop!(
            ExecutionGuard::apply_authorized_upgrade(RuntimeOrigin::signed(account(90)), bounded,),
            frame_system::Error::<Runtime>::MultiBlockMigrationsOngoing
        );
        assert_eq!(pallet_migrations::Cursor::<Runtime>::get(), Some(cursor));
        assert_eq!(
            System::authorized_upgrade().map(|authorization| *authorization.code_hash()),
            authorization_hash_before
        );
        assert_eq!(release_channel_raw(), release_before);
        assert_eq!(
            pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get(),
            Some(pending)
        );
    });
}

#[test]
fn code_queue_rejects_real_under_quorum_attestation_without_storage_changes() {
    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_004;
        let candidate = b"bleavit-b6-under-quorum-candidate".to_vec();
        let members = [account(94), account(95), account(96)];
        assert_ok!(Attestor::set_members(
            pallet_origins::Origin::ConstitutionalValues.into(),
            members.to_vec(),
        ));
        let artifact = sp_io::hashing::blake2_256(&candidate);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(members[0].clone()),
            PID,
            artifact,
            [104; 32],
        ));
        let record = match pallet_attestor::Attestations::<Runtime>::get()
            .into_iter()
            .find(|record| record.pid == PID && record.artifact_hash == artifact)
        {
            Some(record) => record,
            None => {
                assert!(
                    false,
                    "the real attestor adapter fixture must store one record"
                );
                return;
            }
        };
        System::set_block_number(record.challenge_deadline.saturating_add(1));
        assert!(!Attestor::has_quorum(PID, artifact));

        let call = RuntimeCall::System(frame_system::Call::authorize_upgrade {
            code_hash: H256::from(artifact),
        });
        let batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![call]) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "single-call upgrade batch must fit");
                    return;
                }
            };
        let bytes = batch.encode();
        let payload_len = match u32::try_from(bytes.len()) {
            Ok(len) => len,
            Err(_) => {
                assert!(false, "bounded batch length must fit u32");
                return;
            }
        };
        let payload_hash = match <Preimage as StorePreimage>::note(bytes.into()) {
            Ok(hash) => hash,
            Err(_) => {
                assert!(false, "bounded batch preimage must be accepted");
                return;
            }
        };
        crate::configs::set_test_execution_payload(PID, payload_hash.0);
        let now = System::block_number();
        let maturity = now.saturating_add(
            <crate::configs::RuntimeGuardParams as pallet_execution_guard::Params>::exec_timelock(
                ProposalClass::Code,
            ),
        );
        let grace_end = maturity.saturating_add(
            <crate::configs::RuntimeGuardParams as pallet_execution_guard::Params>::exec_grace(
                ProposalClass::Code,
            ),
        );
        let version_constraint = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => {
                assert!(
                    false,
                    "guard genesis must store the current runtime version"
                );
                return;
            }
        };
        let declared_domains = match pallet_execution_guard::pallet::StoredDomains::try_from(vec![
            pallet_execution_guard::CallDomain::InternalRootAuthorizeUpgrade,
        ]) {
            Ok(domains) => domains,
            Err(_) => {
                assert!(false, "single upgrade domain must fit");
                return;
            }
        };
        assert_noop!(
            ExecutionGuard::enqueue(
                crate::configs::test_execution_enqueue_origin(),
                pallet_execution_guard::pallet::StoredQueuedExecution {
                    pid: PID,
                    payload_hash: payload_hash.0,
                    payload_len,
                    class: ProposalClass::Code,
                    maturity,
                    grace_end,
                    version_constraint,
                    meters_declared: Default::default(),
                    ratify_ref: Some(74),
                    ratification_passed: false,
                    attestation_id: Some(record.id),
                    pre_upgrade_checkpoint: None,
                    cancelled: false,
                    declared_domains,
                    failed_at: None,
                },
                false,
            ),
            pallet_execution_guard::Error::<Runtime>::AttestationMissing
        );
        assert!(!pallet_execution_guard::pallet::Queue::<Runtime>::contains_key(PID));
        assert!(!pallet_execution_guard::AttestationBindings::<Runtime>::contains_key(PID));
    });
}

#[test]
fn code_execution_losing_live_attestor_quorum_is_a_storage_noop() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_002;
        let candidate = b"bleavit-b6-unattested-runtime-v2".to_vec();
        let (maturity, _) = match enqueue_attested_code_upgrade(PID, &candidate, 72) {
            Some(setup) => setup,
            None => {
                assert!(false, "attested upgrade fixture must be constructible");
                return;
            }
        };
        // Member 91 supplied one of the two attestations. Replacing it makes
        // the still-present record live-below-quorum before execution.
        assert_ok!(Attestor::set_members(
            pallet_origins::Origin::ConstitutionalValues.into(),
            vec![account(90), account(92), account(93)],
        ));
        assert!(!Attestor::has_quorum(
            PID,
            sp_io::hashing::blake2_256(&candidate),
        ));
        System::set_block_number(maturity);
        let queued_before = pallet_execution_guard::pallet::Queue::<Runtime>::get(PID);
        let release_before = release_channel_raw();
        // `execute` refunds via `DispatchResultWithPostInfo` (B5), so the error
        // carries a checks-only post-info; the surrounding asserts pin the
        // storage no-op that `assert_noop!` used to check.
        let execute_error = ExecutionGuard::execute(RuntimeOrigin::signed(account(78)), PID)
            .expect_err("guard execute must reject");
        assert_eq!(
            execute_error.error,
            pallet_execution_guard::Error::<Runtime>::AttestationMissing.into()
        );
        assert_eq!(
            pallet_execution_guard::pallet::Queue::<Runtime>::get(PID),
            queued_before
        );
        assert_eq!(release_channel_raw(), release_before);
        assert!(System::authorized_upgrade().is_none());
        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_none());
    });
}

#[test]
fn live_code_capability_disables_and_reenables_upgrade_authorization() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_005;
        let capability = pallet_constitution::Capability::AuthorizeUpgrade;
        assert_ok!(Constitution::set_capability(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::CapabilityRecord {
                class: ProposalClass::Code,
                capability,
                enabled: false,
            },
        ));
        let candidate = b"bleavit-b6-capability-gated-runtime-v2".to_vec();
        let (maturity, _) = match enqueue_attested_code_upgrade(PID, &candidate, 75) {
            Some(setup) => setup,
            None => {
                assert!(false, "capability fixture must be constructible");
                return;
            }
        };
        assert!(pallet_execution_guard::pallet::Queue::<Runtime>::contains_key(PID));
        System::set_block_number(maturity);

        assert!(!Constitution::capability_enabled(
            ProposalClass::Code,
            capability,
        ));
        // `execute` refunds via `DispatchResultWithPostInfo` (B5), so the error
        // carries a checks-only post-info; the surrounding asserts pin the
        // storage no-op that `assert_noop!` used to check.
        let execute_error = ExecutionGuard::execute(RuntimeOrigin::signed(account(81)), PID)
            .expect_err("guard execute must reject");
        assert_eq!(
            execute_error.error,
            pallet_execution_guard::Error::<Runtime>::CapabilityDenied.into()
        );
        assert!(System::authorized_upgrade().is_none());
        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::pallet::Queue::<Runtime>::contains_key(PID));

        assert_ok!(Constitution::set_capability(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::CapabilityRecord {
                class: ProposalClass::Code,
                capability,
                enabled: true,
            },
        ));
        assert!(Constitution::capability_enabled(
            ProposalClass::Code,
            capability,
        ));
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(81)),
            PID,
        ));
        assert!(System::authorized_upgrade().is_some());
        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_some());
    });
}

#[test]
fn live_treasury_capability_disables_queued_call_without_state_change_then_reenables() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_009;
        let capability = pallet_constitution::Capability::TreasurySpend;
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| state.main_usdc = 10);
        let call =
            RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
                line: pallet_futarchy_treasury::BudgetLine::Pol,
                amount: 1,
            });
        let maturity = match enqueue_treasury_call(PID, call) {
            Some(maturity) => maturity,
            None => {
                assert!(false, "treasury capability fixture must be constructible");
                return;
            }
        };
        assert_ok!(Constitution::set_capability(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::CapabilityRecord {
                class: ProposalClass::Treasury,
                capability,
                enabled: false,
            },
        ));
        System::set_block_number(maturity);
        let state_before = pallet_futarchy_treasury::State::<Runtime>::get();
        let queue_before = pallet_execution_guard::pallet::Queue::<Runtime>::get(PID);

        // `execute` refunds via `DispatchResultWithPostInfo` (B5), so the error
        // carries a checks-only post-info; the surrounding asserts pin the
        // storage no-op that `assert_noop!` used to check.
        let execute_error = ExecutionGuard::execute(RuntimeOrigin::signed(account(88)), PID)
            .expect_err("guard execute must reject");
        assert_eq!(
            execute_error.error,
            pallet_execution_guard::Error::<Runtime>::CapabilityDenied.into()
        );
        assert_eq!(
            pallet_futarchy_treasury::State::<Runtime>::get(),
            state_before
        );
        assert_eq!(
            pallet_execution_guard::pallet::Queue::<Runtime>::get(PID),
            queue_before
        );

        assert_ok!(Constitution::set_capability(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::CapabilityRecord {
                class: ProposalClass::Treasury,
                capability,
                enabled: true,
            },
        ));
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(88)),
            PID,
        ));
        assert!(pallet_execution_guard::pallet::Queue::<Runtime>::get(PID).is_none());
        assert_ne!(
            pallet_futarchy_treasury::State::<Runtime>::get(),
            state_before
        );
    });
}

#[test]
fn failed_migration_handler_sets_the_guard_machine_signal() {
    use frame_support::migrations::FailedMigrationHandler;
    use pallet_guardian::GuardianTriggers;

    development_ext().execute_with(|| {
        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert_eq!(
            crate::configs::MigrationFailureToGuard::failed(Some(3)),
            frame_support::migrations::FailedMigrationHandling::KeepStuck
        );
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert_eq!(crate::configs::MigrationFailedStep::get(), Some(3));
        assert!(crate::configs::RuntimeGuardianTriggers::current().migration_halt);
    });
}

#[test]
fn migration_completion_clears_a_migration_failure_halt() {
    use frame_support::migrations::{FailedMigrationHandler, MigrationStatusHandler};

    development_ext().execute_with(|| {
        assert_eq!(
            crate::configs::MigrationFailureToGuard::failed(Some(4)),
            frame_support::migrations::FailedMigrationHandling::KeepStuck
        );
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
        crate::configs::MigrationStatusToGuard::completed();
        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert!(crate::configs::MigrationFailedStep::get().is_none());
    });
}

#[test]
fn valid_zero_mbm_recovery_image_clears_migration_failure_and_stall_sources() {
    use cumulus_pallet_parachain_system::OnSystemEvent;
    use frame_support::migrations::FailedMigrationHandler;

    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_011;
        let candidate = b"bleavit-b6-zero-mbm-recovery-runtime-v2".to_vec();
        let (maturity, artifact) = match enqueue_attested_code_upgrade(PID, &candidate, 80) {
            Some(setup) => setup,
            None => {
                assert!(false, "zero-MBM recovery fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(91)),
            PID,
        ));
        let pending = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending,
            None => {
                assert!(false, "zero-MBM recovery fixture must authorize an upgrade");
                return;
            }
        };
        assert_eq!(
            crate::configs::MigrationFailureToGuard::failed(Some(5)),
            frame_support::migrations::FailedMigrationHandling::KeepStuck
        );
        let observed_at = System::block_number();
        pallet_migrations::Cursor::<Runtime>::put(pallet_migrations::MigrationCursor::Active(
            pallet_migrations::ActiveCursor {
                index: 0,
                inner_cursor: None,
                started_at: observed_at,
            },
        ));
        crate::configs::ExecutionGuardSystemEvent::on_validation_data(
            &cumulus_primitives_core::PersistedValidationData::default(),
        );
        System::set_block_number(
            observed_at
                .saturating_add(kernel::MIGRATION_STALL_BLOCKS)
                .saturating_add(1),
        );
        crate::configs::ExecutionGuardSystemEvent::on_validation_data(
            &cumulus_primitives_core::PersistedValidationData::default(),
        );
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert_eq!(crate::configs::MigrationHaltSources::get() & 0b011, 0b011);
        // The recovery image contains no MBMs; model the abandoned cursor as
        // already retired before its application boundary.
        pallet_migrations::Cursor::<Runtime>::kill();
        System::set_block_number(System::block_number().max(pending.applicable_at));
        seed_parachain_upgrade_boundary(candidate.len());
        let apply =
            RuntimeCall::System(frame_system::Call::apply_authorized_upgrade { code: candidate });
        assert!(apply.dispatch(RuntimeOrigin::signed(account(92))).is_ok());
        System::set_block_number(System::block_number().saturating_add(1));
        let _ = ExecutionGuard::on_initialize(System::block_number());
        assert_eq!(
            pallet_execution_guard::ScheduledUpgrade::<Runtime>::get(),
            Some(artifact.0)
        );

        submit_relay_upgrade_go_ahead();

        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert_eq!(crate::configs::MigrationHaltSources::get(), 0);
        assert!(crate::configs::MigrationFailedStep::get().is_none());
        assert!(crate::configs::MigrationProgressMarker::get().is_none());
    });
}

#[test]
fn migration_completion_does_not_clear_an_applied_code_mismatch_halt() {
    use cumulus_pallet_parachain_system::OnSystemEvent;
    use frame_support::migrations::MigrationStatusHandler;

    upgrade_ext().execute_with(|| {
        crate::configs::ExecutionGuardSystemEvent::on_validation_code_applied();
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
        crate::configs::MigrationStatusToGuard::completed();
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
    });
}

#[test]
fn active_migration_cursor_halts_only_after_stall_threshold() {
    use cumulus_pallet_parachain_system::OnSystemEvent;

    development_ext().execute_with(|| {
        let first_observed = 10;
        pallet_migrations::Cursor::<Runtime>::put(pallet_migrations::MigrationCursor::Active(
            pallet_migrations::ActiveCursor {
                index: 0,
                inner_cursor: None,
                started_at: first_observed,
            },
        ));
        System::set_block_number(first_observed);
        let mandatory_before = *System::block_weight().get(DispatchClass::Mandatory);
        crate::configs::ExecutionGuardSystemEvent::on_validation_data(
            &cumulus_primitives_core::PersistedValidationData::default(),
        );
        let mandatory_after = *System::block_weight().get(DispatchClass::Mandatory);
        assert!(mandatory_after.ref_time() > mandatory_before.ref_time());
        assert!(mandatory_after.proof_size() > mandatory_before.proof_size());
        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());

        System::set_block_number(first_observed.saturating_add(kernel::MIGRATION_STALL_BLOCKS));
        crate::configs::ExecutionGuardSystemEvent::on_validation_data(
            &cumulus_primitives_core::PersistedValidationData::default(),
        );
        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());

        System::set_block_number(
            first_observed
                .saturating_add(kernel::MIGRATION_STALL_BLOCKS)
                .saturating_add(1),
        );
        crate::configs::ExecutionGuardSystemEvent::on_validation_data(
            &cumulus_primitives_core::PersistedValidationData::default(),
        );
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
    });
}

#[test]
fn runtime_type_wiring_pins_migration_and_upgrade_event_bridges() {
    assert_same_type::<
        <Runtime as pallet_migrations::Config>::FailedMigrationHandler,
        crate::configs::MigrationFailureToGuard,
    >();
    assert_same_type::<
        <Runtime as pallet_migrations::Config>::MigrationStatusHandler,
        crate::configs::MigrationStatusToGuard,
    >();
    assert_same_type::<
        <Runtime as cumulus_pallet_parachain_system::Config>::OnSystemEvent,
        crate::configs::ExecutionGuardSystemEvent,
    >();
    assert_eq!(
        <<Runtime as pallet_migrations::Config>::CursorMaxLen as Get<u32>>::get(),
        futarchy_primitives::bounds::MIGRATION_CURSOR_MAX_LEN,
    );
    assert_eq!(
        <<Runtime as pallet_migrations::Config>::IdentifierMaxLen as Get<u32>>::get(),
        futarchy_primitives::bounds::MIGRATION_IDENTIFIER_MAX_LEN,
    );
    let expected_service_weight = sp_runtime::Perbill::from_percent(
        futarchy_primitives::bounds::MIGRATION_SERVICE_WEIGHT_PERCENT,
    ) * crate::configs::RuntimeBlockWeights::get().max_block;
    assert_eq!(
        <<Runtime as pallet_migrations::Config>::MaxServiceWeight as Get<Weight>>::get(),
        expected_service_weight,
    );
}

#[test]
fn sq104_migration_admin_calls_are_denied_bare_and_under_sudo() {
    let calls = vec![
        RuntimeCall::Migrations(pallet_migrations::Call::force_set_cursor { cursor: None }),
        RuntimeCall::Migrations(pallet_migrations::Call::force_set_active_cursor {
            index: 0,
            inner_cursor: None,
            started_at: None,
        }),
        RuntimeCall::Migrations(pallet_migrations::Call::force_onboard_mbms {}),
        RuntimeCall::Migrations(pallet_migrations::Call::clear_historic {
            selector: pallet_migrations::HistoricCleanupSelector::Specific(Vec::new()),
        }),
    ];
    development_ext().execute_with(|| {
        for call in calls {
            assert!(!RuntimeBaseCallFilter::contains(&call));
            for wrapped in closed_wrappers(call) {
                assert!(!RuntimeBaseCallFilter::contains(&wrapped));
                let result = wrapped.dispatch(RuntimeOrigin::signed(account(79)));
                assert!(matches!(result, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));
            }
        }
    });
}

#[test]
fn guard_dispatcher_rechecks_the_dynamic_classifier_at_dispatch_time() {
    use pallet_execution_guard::BatchDispatcher;

    development_ext().execute_with(|| {
        let key = pallet_constitution::key16(b"mkt.obs_interval");
        let value = match pallet_constitution::Params::<Runtime>::take(key) {
            Some(record) => record.value,
            None => {
                assert!(false, "Param-class benchmark key must exist");
                return;
            }
        };
        let call = RuntimeCall::Constitution(pallet_constitution::Call::set_param { key, value });
        assert_eq!(
            crate::classifier::RuntimeDispatcher::dispatch_with_class_origin(
                call,
                ProposalClass::Param,
            ),
            Err(DispatchError::Other("guard dispatch-time safety filter"))
        );
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
        // Proxy and multisig may project the wrapped domain, but they cannot
        // carry a privileged class origin across the delegation boundary.
        if (9..=12).contains(&index) {
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
        RuntimeCall::Vesting(pallet_vesting::Call::vest {}),
        RuntimeCall::ForeignAssets(pallet_assets::Call::transfer {
            id: usdc_location(),
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
        RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::execute { pid: 0 }),
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
        RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::apply_authorized_upgrade {
            code: Default::default(),
        }),
        RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::expire_failed_execution {
            pid: 0,
        }),
        RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::reject_stale { pid: 0 }),
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
            <<Runtime as pallet_guardian::Config>::WeightInfo as pallet_guardian::WeightInfo>::on_initialize()
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

#[cfg(feature = "try-runtime")]
#[test]
fn try_runtime_api_executes_genesis_upgrade_and_try_state_checks() {
    development_ext().execute_with(|| {
        let input = frame_try_runtime::UpgradeCheckSelect::All.encode();
        let Some(output) = crate::apis::api::dispatch("TryRuntime_on_runtime_upgrade", &input)
        else {
            assert!(false, "TryRuntime runtime API method must be generated");
            return;
        };
        let decoded = <(Weight, Weight) as parity_scale_codec::Decode>::decode(&mut &output[..]);
        match decoded {
            Ok((used, maximum)) => assert!(used.all_lte(maximum)),
            Err(error) => assert!(false, "TryRuntime result must decode: {error}"),
        }
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
fn metric_inputs_incident_multiplier_defaults_to_the_neutral_identity() {
    use pallet_welfare::MetricInputs;
    development_ext().execute_with(|| {
        // No closed registry epoch ⇒ the neutral 1.0 multiplier (a zero would
        // erase C_attested outright — fail-destructive, not fail-safe).
        assert_eq!(
            crate::configs::RuntimeMetricInputs::incident_multiplier(5),
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
