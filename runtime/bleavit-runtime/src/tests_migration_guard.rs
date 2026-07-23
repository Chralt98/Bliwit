//! B16 — migration stall predicate, MBM lockdown, the `MigrationHalted`
//! diagnostic, and this runtime's first storage migration (SQ-132, SQ-146,
//! SQ-309). Owning spec: 09 §3.1–§3.2. These live in their own file per the
//! standing instruction that `tests.rs` not grow concurrently.

use alloc::vec;

#[cfg(feature = "bootstrap")]
use alloc::borrow::Cow;
#[cfg(feature = "bootstrap")]
use core::sync::atomic::Ordering;

use frame_support::{
    migrations::{FailedMigrationHandler, MultiStepMigrator, SteppedMigrations},
    storage::unhashed,
    traits::{GetStorageVersion, OnRuntimeUpgrade, StorageVersion},
    BoundedVec,
};
use futarchy_primitives::kernel;
use pallet_execution_guard::PhaseState;
use parity_scale_codec::MaxEncodedLen;
use sp_runtime::traits::Header as _;

#[cfg(feature = "bootstrap")]
use frame_support::traits::EnsureOrigin;
#[cfg(any(feature = "bootstrap", feature = "recovery"))]
use frame_support::traits::{QueryPreimage, StorePreimage};

use crate::{tests, ExecutionGuard, Runtime, RuntimeEvent, System};

/// SQ-132(d): "stalled" is a pure function of the SDK cursor's own `started_at`,
/// never of the cursor bytes failing to change. The retired byte-equality
/// predicate false-raised on a lawful migration that drains a map for hours
/// while returning byte-identical cursors; this one cannot.
#[test]
fn stall_is_derived_from_started_at_not_cursor_bytes() {
    tests::development_ext().execute_with(|| {
        System::set_block_number(10_000);
        // Bytes are identical across every case below; only `started_at` varies.
        let at = |started_at| pallet_migrations::ActiveCursor {
            index: 3,
            inner_cursor: Some(BoundedVec::truncate_from(vec![7u8, 7, 7])),
            started_at,
        };
        let now = System::block_number();

        // A cursor that has only just started is never stalled, regardless of
        // how long an identical-byte cursor may have existed before it.
        assert!(!crate::configs::active_migration_stall_is_live(&at(now)));

        // Exactly at the budget (now - started_at == MIGRATION_STALL_BLOCKS): the
        // predicate is strict `>`, so this is NOT yet stalled.
        assert!(!crate::configs::active_migration_stall_is_live(&at(
            now - kernel::MIGRATION_STALL_BLOCKS
        )));

        // One block past the budget: stalled.
        assert!(crate::configs::active_migration_stall_is_live(&at(now
            - kernel::MIGRATION_STALL_BLOCKS
            - 1)));
    });
}

/// SQ-309 / SQ-132(d)(ii)/(iii): ordinary MBMs stay prohibited until a future
/// release provides and tests a cutpoint-total repair for each registration.
/// The bounds below remain the release-time requirements once that deliberate
/// profile-specific repair exists. Gated to the production `Migrations`
/// config (benchmarking swaps in `MockedMigrations`).
#[cfg(not(feature = "runtime-benchmarks"))]
#[test]
fn registered_mbms_obey_the_b16_lockdown() {
    type Migrations = <Runtime as pallet_migrations::Config>::Migrations;

    assert_eq!(
        <Migrations as SteppedMigrations>::len(),
        0,
        "a primary profile must not register an MBM without a cutpoint-total paired recovery repair",
    );
    let mut aggregate: u64 = 0;
    for i in 0..<Migrations as SteppedMigrations>::len() {
        let max = Migrations::nth_max_steps(i)
            .expect("index < len is in range")
            .expect("every registered MBM must declare max_steps = Some(n) (09 §3.2(d)(ii))");
        assert!(
            max < kernel::MIGRATION_STALL_BLOCKS,
            "each MBM max_steps must be strictly < MIGRATION_STALL_BLOCKS",
        );
        aggregate = aggregate.saturating_add(u64::from(max));
    }
    assert!(
        aggregate < u64::from(kernel::MIGRATION_STALL_BLOCKS),
        "the aggregate MBM run budget must be strictly < MIGRATION_STALL_BLOCKS (09 §3.2(d)(iii))",
    );
}

#[cfg(all(feature = "recovery", not(feature = "runtime-benchmarks")))]
#[test]
fn recovery_profile_has_zero_multi_block_migrations() {
    type Migrations = <Runtime as pallet_migrations::Config>::Migrations;

    assert_eq!(
        <Migrations as SteppedMigrations>::len(),
        0,
        "the terminal recovery image must not introduce an MBM cursor of its own",
    );
}

#[cfg(feature = "bootstrap")]
#[test]
fn phase_three_predicate_accepts_only_exact_shadow_plus_live_sudo() {
    tests::development_ext().execute_with(|| {
        let exact = pallet_constitution::PhaseFlagsValue::SHADOW_MODE
            | pallet_constitution::PhaseFlagsValue::SUDO_PRESENT;
        assert_eq!(pallet_constitution::PhaseFlags::<Runtime>::get(), exact);
        assert!(crate::configs::RuntimePhaseState::exact_phase_three());
        for class in [
            futarchy_primitives::ProposalClass::Param,
            futarchy_primitives::ProposalClass::Treasury,
            futarchy_primitives::ProposalClass::Code,
            futarchy_primitives::ProposalClass::Meta,
        ] {
            assert!(
                !crate::configs::RuntimePhaseState::class_execution_enabled(class),
                "{class:?} must remain shadow-only in exact Phase 3",
            );
        }

        for flags in [
            0,
            pallet_constitution::PhaseFlagsValue::SHADOW_MODE,
            pallet_constitution::PhaseFlagsValue::SUDO_PRESENT,
            exact | pallet_constitution::PhaseFlagsValue::PARAM_ARMED,
            exact | pallet_constitution::PhaseFlagsValue::TREASURY_ARMED,
            exact | pallet_constitution::PhaseFlagsValue::CODE_META_ARMED,
            exact | pallet_constitution::PhaseFlagsValue::LEDGER_FROZEN,
            exact | pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED,
            exact | pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG,
            exact | (1 << 31),
        ] {
            pallet_constitution::PhaseFlags::<Runtime>::put(flags);
            assert!(
                !crate::configs::RuntimePhaseState::exact_phase_three(),
                "non-exact PhaseFlags value {flags:#010x} must refuse the bridge",
            );
        }

        pallet_constitution::PhaseFlags::<Runtime>::put(exact);
        pallet_sudo::Key::<Runtime>::kill();
        assert!(
            !crate::configs::RuntimePhaseState::exact_phase_three(),
            "SUDO_PRESENT storage cannot substitute for a live sudo key",
        );
    });
}

#[cfg(feature = "bootstrap")]
#[test]
fn phase_four_bridge_origin_is_only_the_signed_current_sudo_key() {
    tests::development_ext().execute_with(|| {
        let sudo = pallet_sudo::Key::<Runtime>::get().expect("bootstrap preset has sudo");
        let admitted = match crate::configs::EnsureCurrentSudoKey::try_origin(
            crate::RuntimeOrigin::signed(sudo.clone()),
        ) {
            Ok(who) => who,
            Err(_) => {
                assert!(false, "the signed current sudo key must be admitted");
                return;
            }
        };
        assert_eq!(admitted, sudo);
        assert!(
            crate::configs::EnsureCurrentSudoKey::try_origin(crate::RuntimeOrigin::signed(
                tests::account(0xfe),
            ))
            .is_err()
        );
        assert!(
            crate::configs::EnsureCurrentSudoKey::try_origin(crate::RuntimeOrigin::root()).is_err()
        );

        pallet_sudo::Key::<Runtime>::kill();
        assert!(
            crate::configs::EnsureCurrentSudoKey::try_origin(crate::RuntimeOrigin::signed(sudo,))
                .is_err()
        );
    });
}

#[test]
fn consumed_phase_four_bridge_accepts_later_binding_masks_but_never_sudo_or_shadow() {
    tests::development_ext().execute_with(|| {
        let mut sudo_key = [0u8; 32];
        sudo_key[..16].copy_from_slice(&sp_io::hashing::twox_128(b"Sudo"));
        sudo_key[16..].copy_from_slice(&sp_io::hashing::twox_128(b"Key"));
        sp_io::storage::clear(&sudo_key);
        pallet_execution_guard::PhaseFourBridge::<Runtime>::put(
            pallet_execution_guard::PhaseFourBridgeState::Consumed,
        );

        for flags in [
            pallet_constitution::PhaseFlagsValue::PARAM_ARMED,
            pallet_constitution::PhaseFlagsValue::PARAM_ARMED
                | pallet_constitution::PhaseFlagsValue::TREASURY_ARMED,
            pallet_constitution::PhaseFlagsValue::PARAM_ARMED
                | pallet_constitution::PhaseFlagsValue::TREASURY_ARMED
                | pallet_constitution::PhaseFlagsValue::CODE_META_ARMED,
        ] {
            pallet_constitution::PhaseFlags::<Runtime>::put(flags);
            assert!(
                crate::configs::RuntimePhaseState::post_sudo_phase(),
                "post-Sudo history must accept monotone later binding mask {flags:#010x}",
            );
            assert!(
                ExecutionGuard::do_try_state().is_ok(),
                "Consumed bridge try-state must accept later binding mask {flags:#010x}",
            );
        }

        for flags in [
            pallet_constitution::PhaseFlagsValue::TREASURY_ARMED
                | pallet_constitution::PhaseFlagsValue::CODE_META_ARMED,
            pallet_constitution::PhaseFlagsValue::PARAM_ARMED
                | pallet_constitution::PhaseFlagsValue::SHADOW_MODE,
            pallet_constitution::PhaseFlagsValue::PARAM_ARMED
                | pallet_constitution::PhaseFlagsValue::SUDO_PRESENT,
        ] {
            pallet_constitution::PhaseFlags::<Runtime>::put(flags);
            assert!(
                !crate::configs::RuntimePhaseState::post_sudo_phase(),
                "post-Sudo history must reject regressive mask {flags:#010x}",
            );
            assert!(
                ExecutionGuard::do_try_state().is_err(),
                "Consumed bridge try-state must reject regressive mask {flags:#010x}",
            );
        }
    });
}

/// The production Phase-3→4 payload is one exact META batch: primary
/// authorization, terminal recovery commitment, and both scheduled exposure
/// cap raises. Pin the whole path against the real classifier, constitution
/// origins, attestor records, queue, and guard dispatcher.
#[cfg(feature = "bootstrap")]
#[test]
fn exact_phase_four_meta_payload_queues_and_commits_both_cap_raises() {
    let recovery = b"phase-four-terminal-recovery-runtime".to_vec();
    let wrong_name_recovery = b"phase-four-wrong-name-recovery-runtime".to_vec();
    let mut recovery_version = crate::VERSION;
    recovery_version.spec_version = recovery_version.spec_version.saturating_add(2);
    let mut wrong_name_version = recovery_version.clone();
    wrong_name_version.spec_name = Cow::Borrowed("not-bleavit");
    let (mut ext, version_reads) = tests::upgrade_ext_with_artifact_versions_and_counter(vec![
        (recovery.clone(), recovery_version),
        (wrong_name_recovery.clone(), wrong_name_version),
    ]);
    ext.execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 4_003;
        const RATIFY_REF: u32 = 403;
        let candidate = b"phase-four-primary-runtime".to_vec();
        let candidate_hash = sp_io::hashing::blake2_256(&candidate);
        let recovery_hash = sp_io::hashing::blake2_256(&recovery);
        let wrong_name_recovery_hash = sp_io::hashing::blake2_256(&wrong_name_recovery);
        let members = [
            tests::account(0xd0),
            tests::account(0xd1),
            tests::account(0xd2),
        ];
        assert!(crate::Attestor::set_members(
            pallet_origins::Origin::ConstitutionalValues.into(),
            members.to_vec(),
        )
        .is_ok());
        for (artifact, statement_base) in [
            (candidate_hash, 0x41_u8),
            (recovery_hash, 0x51_u8),
            (wrong_name_recovery_hash, 0x61_u8),
        ] {
            for (member, statement) in members
                .iter()
                .take(2)
                .zip([statement_base, statement_base.saturating_add(1)])
            {
                assert!(crate::Attestor::attest(
                    crate::RuntimeOrigin::signed(member.clone()),
                    PID,
                    artifact,
                    [statement; 32],
                )
                .is_ok());
            }
        }
        let records = pallet_attestor::Attestations::<Runtime>::get();
        let primary_attestation = records
            .iter()
            .find(|record| record.pid == PID && record.artifact_hash == candidate_hash)
            .expect("primary attestation is stored")
            .clone();
        let recovery_attestation = records
            .iter()
            .find(|record| record.pid == PID && record.artifact_hash == recovery_hash)
            .expect("recovery attestation is stored")
            .clone();
        let wrong_name_recovery_attestation = records
            .iter()
            .find(|record| record.pid == PID && record.artifact_hash == wrong_name_recovery_hash)
            .expect("wrong-name recovery attestation is stored")
            .clone();
        System::set_block_number(
            primary_attestation
                .challenge_deadline
                .max(recovery_attestation.challenge_deadline)
                .max(wrong_name_recovery_attestation.challenge_deadline)
                .saturating_add(1),
        );
        assert!(crate::Attestor::has_quorum(PID, candidate_hash));
        assert!(crate::Attestor::has_quorum(PID, recovery_hash));
        assert!(crate::Attestor::has_quorum(PID, wrong_name_recovery_hash));

        let noted_recovery = <crate::Preimage as StorePreimage>::note(recovery.clone().into())
            .expect("terminal recovery preimage is stored");
        assert_eq!(noted_recovery.0, recovery_hash);
        let noted_wrong_name_recovery =
            <crate::Preimage as StorePreimage>::note(wrong_name_recovery.clone().into())
                .expect("wrong-name recovery preimage is stored");
        assert_eq!(noted_wrong_name_recovery.0, wrong_name_recovery_hash);
        tests::seed_parachain_upgrade_boundary(recovery.len().max(wrong_name_recovery.len()));
        let current = pallet_execution_guard::CurrentSpecName::<Runtime>::get()
            .expect("genesis seeds the frozen runtime identity");
        let tvl_key = pallet_constitution::key16(b"phase3.tvl_cap");
        let deposit_key = pallet_constitution::key16(b"phase3.dep_cap");
        let tvl_before =
            pallet_constitution::Params::<Runtime>::get(tvl_key).expect("TVL cap is registered");
        let deposit_before = pallet_constitution::Params::<Runtime>::get(deposit_key)
            .expect("deposit cap is registered");
        let tvl_after =
            pallet_constitution::ParamValue::Balance(tvl_before.value.as_u128().saturating_add(1));
        let deposit_after = pallet_constitution::ParamValue::Balance(
            deposit_before.value.as_u128().saturating_add(1),
        );
        let calls = vec![
            crate::RuntimeCall::System(frame_system::Call::authorize_upgrade {
                code_hash: sp_core::H256::from(candidate_hash),
            }),
            crate::RuntimeCall::ExecutionGuard(
                pallet_execution_guard::Call::commit_recovery_image {
                    hash: recovery_hash,
                    len: u32::try_from(recovery.len()).expect("test recovery length fits u32"),
                    target_spec_version: current.spec_version.saturating_add(2),
                    attestation_id: recovery_attestation.id,
                },
            ),
            crate::RuntimeCall::Constitution(pallet_constitution::Call::set_param {
                key: tvl_key,
                value: tvl_after,
            }),
            crate::RuntimeCall::Constitution(pallet_constitution::Call::set_param {
                key: deposit_key,
                value: deposit_after,
            }),
        ];
        let footprint = crate::classifier::derive_resource_footprint(&calls)
            .expect("the exact bridge has a bounded resource footprint");
        let (payload_hash, payload_len) =
            tests::note_runtime_batch(calls.clone()).expect("exact bridge batch is encodable");
        <crate::Preimage as QueryPreimage>::request(&payload_hash);

        let now = System::block_number();
        let maturity = now.saturating_add(
            <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_timelock(
                futarchy_primitives::ProposalClass::Meta,
            ),
        );
        let grace_end = maturity.saturating_add(
            <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_grace(
                futarchy_primitives::ProposalClass::Meta,
            ),
        );
        let epoch = pallet_epoch::EpochOf::<Runtime>::get().index;
        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        let first_market = PID.saturating_mul(10);
        let mut proposal = futarchy_primitives::Proposal {
            id: PID,
            proposer: tests::account(0xd3),
            class: futarchy_primitives::ProposalClass::Meta,
            state: futarchy_primitives::ProposalState::Submitted,
            epoch,
            submitted_at: now,
            payload_hash: payload_hash.0,
            payload_len,
            ask: 0,
            bond: 0,
            resources: futarchy_primitives::BoundedVec::try_from(footprint.clone().into_inner())
                .expect("bridge resources fit the proposal bound"),
            metric_spec: 1,
            decide_at: now,
            rerun: false,
            extended: false,
            delayed_once: false,
            markets: Some(futarchy_primitives::MarketSet {
                accept: first_market.saturating_add(1),
                reject: first_market.saturating_add(2),
                gates: Some([
                    first_market.saturating_add(3),
                    first_market.saturating_add(4),
                    first_market.saturating_add(5),
                    first_market.saturating_add(6),
                ]),
                baseline: 9_000_u64.saturating_add(epoch.into()),
            }),
            maturity: Some(maturity),
            grace_end: Some(grace_end),
            version_constraint: Some(current.clone()),
            decision: Some(futarchy_primitives::DecisionOutcome::Adopt),
        };
        proposal.bond =
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                crate::AccountId,
            >>::required_bond(&proposal)
            .expect("META bond floor is configured");
        pallet_epoch::IntakeProposals::<Runtime>::insert(PID, proposal.clone());
        assert!(crate::ExecutionGuard::qualify_recovery_image(
            crate::RuntimeOrigin::signed(tests::account(0xd4)),
            PID,
        )
        .is_ok());
        assert!(
            version_reads.load(Ordering::Relaxed) > 0,
            "qualification must inspect the full recovery image in its own operation",
        );
        assert_eq!(
            pallet_execution_guard::QualifiedRecoveryImages::<Runtime>::get(PID),
            Some(pallet_execution_guard::QualifiedRecoveryImage {
                payload_hash: payload_hash.0,
                primary_hash: candidate_hash,
                version_constraint: current.clone(),
                descriptor: pallet_execution_guard::RecoveryImageDescriptor {
                    hash: recovery_hash,
                    len: u32::try_from(recovery.len()).expect("test recovery length fits u32"),
                    target_spec_version: current.spec_version.saturating_add(2),
                    attestation_id: recovery_attestation.id,
                },
            }),
        );

        let qualified = pallet_execution_guard::QualifiedRecoveryImages::<Runtime>::take(PID)
            .expect("qualification is cached");
        assert_eq!(
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                crate::AccountId,
            >>::static_check(&proposal),
            pallet_epoch::StaticCheckDisposition::Refund(
                futarchy_primitives::RejectReason::ProcessHold,
            ),
            "static screening must fail closed while qualification is missing",
        );
        pallet_execution_guard::QualifiedRecoveryImages::<Runtime>::insert(PID, qualified);

        version_reads.store(0, Ordering::Relaxed);
        assert_eq!(
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                crate::AccountId,
            >>::static_check(&proposal),
            pallet_epoch::StaticCheckDisposition::Eligible,
            "the exact four-call bridge must survive production static screening",
        );
        assert_eq!(
            version_reads.load(Ordering::Relaxed),
            0,
            "static screening must consume only the cached descriptor, never full recovery Wasm",
        );

        let mut wrong_name_calls = calls.clone();
        wrong_name_calls[1] = crate::RuntimeCall::ExecutionGuard(
            pallet_execution_guard::Call::commit_recovery_image {
                hash: wrong_name_recovery_hash,
                len: u32::try_from(wrong_name_recovery.len())
                    .expect("test recovery length fits u32"),
                target_spec_version: current.spec_version.saturating_add(2),
                attestation_id: wrong_name_recovery_attestation.id,
            },
        );
        let (wrong_name_hash, wrong_name_len) = tests::note_runtime_batch(wrong_name_calls)
            .expect("wrong-name bridge batch is encodable");
        <crate::Preimage as QueryPreimage>::request(&wrong_name_hash);
        let mut wrong_name_proposal = proposal.clone();
        wrong_name_proposal.payload_hash = wrong_name_hash.0;
        wrong_name_proposal.payload_len = wrong_name_len;
        pallet_epoch::IntakeProposals::<Runtime>::insert(PID, wrong_name_proposal.clone());
        assert_eq!(
            crate::ExecutionGuard::qualify_recovery_image(
                crate::RuntimeOrigin::signed(tests::account(0xd4)),
                PID,
            ),
            Err(pallet_execution_guard::Error::<Runtime>::UpgradeVersionMismatch.into()),
            "a terminal image with the wrong spec_name must fail in the qualifier",
        );
        assert_eq!(
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                crate::AccountId,
            >>::static_check(&wrong_name_proposal),
            pallet_epoch::StaticCheckDisposition::Refund(
                futarchy_primitives::RejectReason::ProcessHold,
            ),
            "a terminal recovery image with the wrong spec_name must fail qualification",
        );

        let mut wrong_target_calls = calls.clone();
        wrong_target_calls[1] = crate::RuntimeCall::ExecutionGuard(
            pallet_execution_guard::Call::commit_recovery_image {
                hash: recovery_hash,
                len: u32::try_from(recovery.len()).expect("test recovery length fits u32"),
                target_spec_version: current.spec_version.saturating_add(1),
                attestation_id: recovery_attestation.id,
            },
        );
        let (wrong_target_hash, wrong_target_len) = tests::note_runtime_batch(wrong_target_calls)
            .expect("wrong-target bridge batch is encodable");
        <crate::Preimage as QueryPreimage>::request(&wrong_target_hash);
        let mut wrong_target_proposal = proposal.clone();
        wrong_target_proposal.payload_hash = wrong_target_hash.0;
        wrong_target_proposal.payload_len = wrong_target_len;
        pallet_epoch::IntakeProposals::<Runtime>::insert(PID, wrong_target_proposal.clone());
        assert_eq!(
            crate::ExecutionGuard::qualify_recovery_image(
                crate::RuntimeOrigin::signed(tests::account(0xd4)),
                PID,
            ),
            Err(pallet_execution_guard::Error::<Runtime>::RecoveryImageInvalid.into()),
            "a terminal image targeting N+1 instead of N+2 must fail in the qualifier",
        );
        assert_eq!(
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                crate::AccountId,
            >>::static_check(&wrong_target_proposal),
            pallet_epoch::StaticCheckDisposition::Refund(
                futarchy_primitives::RejectReason::ProcessHold,
            ),
            "a terminal recovery image targeting N+1 instead of N+2 must fail qualification",
        );

        let saved_host = cumulus_pallet_parachain_system::HostConfiguration::<Runtime>::get()
            .expect("the valid preflight boundary is seeded");
        pallet_epoch::IntakeProposals::<Runtime>::insert(PID, proposal.clone());
        cumulus_pallet_parachain_system::HostConfiguration::<Runtime>::mutate(|host| {
            host.as_mut()
                .expect("the host boundary remains present")
                .max_code_size = u32::try_from(recovery.len().saturating_sub(1))
                .expect("test recovery length fits u32");
        });
        assert_eq!(
            crate::ExecutionGuard::qualify_recovery_image(
                crate::RuntimeOrigin::signed(tests::account(0xd4)),
                PID,
            ),
            Err(pallet_execution_guard::Error::<Runtime>::RecoveryImageInvalid.into()),
            "a recovery image over the relay host maximum must fail qualifier preflight",
        );
        cumulus_pallet_parachain_system::HostConfiguration::<Runtime>::put(saved_host);

        pallet_epoch::IntakeProposals::<Runtime>::remove(PID);
        proposal.state = futarchy_primitives::ProposalState::Queued;
        pallet_epoch::Proposals::<Runtime>::insert(PID, proposal);
        pallet_epoch::ProposalSchedules::<Runtime>::insert(
            PID,
            pallet_epoch::ProposalSchedule {
                epoch,
                epoch_start_block: schedule.epoch_start_block,
                epoch_length: schedule.length,
                decide_at: now,
                metric_spec: 1,
            },
        );
        pallet_conditional_ledger::Vaults::<Runtime>::insert(
            PID,
            pallet_conditional_ledger::core_ledger::VaultInfo::open(1),
        );
        let declared_domains = pallet_execution_guard::pallet::StoredDomains::try_from(vec![
            pallet_execution_guard::CallDomain::InternalRootAuthorizeUpgrade,
            pallet_execution_guard::CallDomain::Code,
            pallet_execution_guard::CallDomain::Meta,
        ])
        .expect("bridge domains fit");
        let meters_declared =
            pallet_execution_guard::pallet::StoredMeters::try_from(footprint.clone().into_inner())
                .expect("bridge resources fit");
        assert!(crate::ExecutionGuard::enqueue(
            crate::RuntimeOrigin::signed(crate::configs::epoch_account()),
            pallet_execution_guard::pallet::StoredQueuedExecution {
                pid: PID,
                payload_hash: payload_hash.0,
                payload_len,
                class: futarchy_primitives::ProposalClass::Meta,
                maturity,
                grace_end,
                version_constraint: current,
                meters_declared,
                ratify_ref: None,
                ratification_passed: false,
                attestation_id: Some(primary_attestation.id),
                pre_upgrade_checkpoint: None,
                cancelled: false,
                declared_domains,
                failed_at: None,
            },
            false,
        )
        .is_ok());
        assert!(crate::ExecutionGuard::ratify(
            pallet_origins::Origin::ConstitutionalValues.into(),
            PID,
            RATIFY_REF,
        )
        .is_ok());
        System::set_block_number(maturity);
        assert!(
            <crate::classifier::RuntimeDispatcher as pallet_execution_guard::BatchDispatcher<
                crate::RuntimeCall,
            >>::phase_four_plan(futarchy_primitives::ProposalClass::Meta, &calls)
            .is_some(),
            "the exact queued payload must remain the Phase-4 bridge at authorization time",
        );
        let sudo = pallet_sudo::Key::<Runtime>::get().expect("bootstrap preset has sudo");
        let authorization = crate::ExecutionGuard::authorize_phase_four(
            crate::RuntimeOrigin::signed(sudo),
            PID,
            [0x61; 32],
        );
        assert!(
            authorization.is_ok(),
            "exact bridge authorization failed: {authorization:?}",
        );

        assert_eq!(
            pallet_constitution::Params::<Runtime>::get(tvl_key).map(|record| record.value),
            Some(tvl_before.value),
            "the cap commitment must not dispatch before the no-Sudo image applies",
        );
        assert_eq!(
            pallet_constitution::Params::<Runtime>::get(deposit_key).map(|record| record.value),
            Some(deposit_before.value),
            "the cap commitment must not dispatch before the no-Sudo image applies",
        );
        assert_eq!(
            System::authorized_upgrade().map(|authorization| authorization.code_hash().0),
            Some(candidate_hash),
        );
        assert!(matches!(
            pallet_execution_guard::PhaseFourBridge::<Runtime>::get(),
            pallet_execution_guard::PhaseFourBridgeState::Pending {
                pid: PID,
                code_hash,
                plan,
            } if code_hash == candidate_hash
                && plan == (pallet_execution_guard::PhaseFourPlan {
                    tvl_cap: tvl_after.as_u128(),
                    deposit_cap: deposit_after.as_u128(),
                })
        ));
        assert!(
            pallet_execution_guard::RecoveryImage::<Runtime>::get().is_some_and(|commitment| {
                commitment.pid == PID
                    && commitment.primary_hash == candidate_hash
                    && commitment.hash == recovery_hash
            })
        );
    });
}

/// The phase transition must lock transaction execution at the exact boundary
/// where Cumulus accepts the validation code. A merely authorized bridge is
/// still relay-abortable and must not lock; a durably scheduled one must become
/// `Scheduled` and force `OnlyInherents` before the next block.
#[cfg(feature = "bootstrap")]
#[test]
fn real_upgrade_dispatcher_schedules_and_locks_the_phase_four_bridge_atomically() {
    tests::upgrade_ext().execute_with(|| {
        type Dispatcher = crate::classifier::RuntimeDispatcher;

        let candidate = b"phase-four-dispatch-boundary".to_vec();
        let candidate_hash = sp_io::hashing::blake2_256(&candidate);
        pallet_execution_guard::PhaseFourBridge::<Runtime>::put(
            pallet_execution_guard::PhaseFourBridgeState::Pending {
                pid: 4_004,
                code_hash: candidate_hash,
                plan: pallet_execution_guard::PhaseFourPlan {
                    tvl_cap: 2,
                    deposit_cap: 3,
                },
            },
        );

        assert!(
            <Dispatcher as pallet_execution_guard::BatchDispatcher<crate::RuntimeCall>>::dispatch_authorize_upgrade(
                candidate_hash,
            )
            .is_ok()
        );
        let current = pallet_execution_guard::CurrentSpecName::<Runtime>::get()
            .expect("genesis seeds the frozen runtime identity");
        let now = System::block_number();
        pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::put(
            pallet_execution_guard::PendingUpgrade {
                hash: candidate_hash,
                authorized_at: now,
                applicable_at: now,
                target_spec_version: current.spec_version.saturating_add(1),
            },
        );
        assert!(!crate::configs::PhaseTransitionLock::get());
        assert!(!crate::configs::RecoveryAwareMigrations::ongoing());

        tests::seed_parachain_upgrade_boundary(candidate.len());
        assert!(
            <Dispatcher as pallet_execution_guard::BatchDispatcher<crate::RuntimeCall>>::dispatch_apply_authorized_upgrade(
                candidate.clone(),
            )
            .is_ok()
        );
        assert_eq!(
            cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::get(),
            candidate,
        );
        assert_eq!(
            pallet_execution_guard::PhaseFourBridge::<Runtime>::get(),
            pallet_execution_guard::PhaseFourBridgeState::Scheduled {
                pid: 4_004,
                code_hash: candidate_hash,
                plan: pallet_execution_guard::PhaseFourPlan {
                    tvl_cap: 2,
                    deposit_cap: 3,
                },
            },
        );
        assert!(crate::configs::PhaseTransitionLock::get());
        assert!(crate::configs::RecoveryAwareMigrations::ongoing());
    });
}

/// The pallet-removal image consumes the exact Phase-3 source state once. The
/// migration may remain wired in later Phase-4 images, but must never reset a
/// subsequently expanded phase mask back to PARAM-only.
#[cfg(feature = "phase-four")]
#[test]
fn phase_four_profile_removes_sudo_and_never_rearms_the_one_shot_transition() {
    tests::development_ext().execute_with(|| {
        let mut sudo_key = [0u8; 32];
        sudo_key[..16].copy_from_slice(&sp_io::hashing::twox_128(b"Sudo"));
        sudo_key[16..].copy_from_slice(&sp_io::hashing::twox_128(b"Key"));
        sp_io::storage::set(&sudo_key, &[1]);

        let source_flags = pallet_constitution::PhaseFlagsValue::SHADOW_MODE
            | pallet_constitution::PhaseFlagsValue::SUDO_PRESENT;
        pallet_constitution::PhaseFlags::<Runtime>::put(source_flags);
        crate::configs::PhaseTransitionLock::put(true);
        crate::configs::PhaseTransitionApplied::put(true);
        let tvl_key = pallet_constitution::key16(b"phase3.tvl_cap");
        let deposit_key = pallet_constitution::key16(b"phase3.dep_cap");
        let tvl_cap = pallet_constitution::Params::<Runtime>::get(tvl_key)
            .expect("TVL cap is registered")
            .value
            .as_u128()
            .saturating_add(1);
        let deposit_cap = pallet_constitution::Params::<Runtime>::get(deposit_key)
            .expect("deposit cap is registered")
            .value
            .as_u128()
            .saturating_add(1);
        let current = pallet_execution_guard::CurrentSpecName::<Runtime>::get()
            .expect("genesis seeds the frozen runtime identity");
        let authorized_at = System::block_number();
        let applicable_at =
            authorized_at.saturating_add(pallet_execution_guard::DESCRIPTOR_LEAD_TIME);
        let target_spec_version = current.spec_version.saturating_add(1);
        System::set_block_number(applicable_at);
        pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::put(
            pallet_execution_guard::PendingUpgrade {
                hash: [0x44; 32],
                authorized_at,
                applicable_at,
                target_spec_version,
            },
        );
        pallet_execution_guard::RecoveryImage::<Runtime>::put(
            pallet_execution_guard::RecoveryImageCommitment {
                pid: 4_004,
                primary_hash: [0x44; 32],
                hash: [0x55; 32],
                len: 1,
                target_spec_version: target_spec_version.saturating_add(1),
                attestation_id: 7,
                committed_at: authorized_at,
            },
        );
        pallet_execution_guard::PhaseFourBridge::<Runtime>::put(
            pallet_execution_guard::PhaseFourBridgeState::Scheduled {
                pid: 4_004,
                code_hash: [0x44; 32],
                plan: pallet_execution_guard::PhaseFourPlan {
                    tvl_cap,
                    deposit_cap,
                },
            },
        );

        let _ = crate::migrations::PhaseFourTransition::on_runtime_upgrade();
        assert!(!sp_io::storage::exists(&sudo_key));
        assert_eq!(
            pallet_constitution::PhaseFlags::<Runtime>::get(),
            pallet_constitution::PhaseFlagsValue::PARAM_ARMED,
        );
        assert_eq!(
            pallet_constitution::Params::<Runtime>::get(tvl_key)
                .map(|record| record.value.as_u128()),
            Some(tvl_cap),
        );
        assert_eq!(
            pallet_constitution::Params::<Runtime>::get(deposit_key)
                .map(|record| record.value.as_u128()),
            Some(deposit_cap),
        );
        assert!(!crate::configs::PhaseTransitionApplied::get());
        assert!(!crate::configs::PhaseTransitionLock::get());
        assert_eq!(
            pallet_execution_guard::PhaseFourBridge::<Runtime>::get(),
            pallet_execution_guard::PhaseFourBridgeState::Consumed,
        );
        assert!(crate::configs::RuntimePhaseState::exact_phase_four());

        let later_flags = pallet_constitution::PhaseFlagsValue::PARAM_ARMED
            | pallet_constitution::PhaseFlagsValue::TREASURY_ARMED;
        pallet_constitution::PhaseFlags::<Runtime>::put(later_flags);

        let _ = crate::migrations::PhaseFourTransition::on_runtime_upgrade();
        assert_eq!(
            pallet_constitution::PhaseFlags::<Runtime>::get(),
            later_flags,
            "a later runtime upgrade must not replay the Phase-3→4 flag rewrite",
        );
        assert!(!crate::configs::PhaseTransitionApplied::get());
        assert!(!sp_io::storage::exists(&sudo_key));
    });
}

#[cfg(feature = "recovery")]
#[test]
fn phase_transition_terminal_recovery_applies_the_committed_plan_and_unlocks() {
    let recovery = b"phase-transition-terminal-recovery".to_vec();
    tests::upgrade_ext_with_artifact_versions(vec![(recovery.clone(), crate::VERSION)])
        .execute_with(|| {
            let recovery_hash = sp_io::hashing::blake2_256(&recovery);
            let noted = <crate::Preimage as StorePreimage>::note(recovery.clone().into())
                .expect("recovery preimage is stored");
            assert_eq!(noted.0, recovery_hash);
            <crate::Preimage as QueryPreimage>::request(&noted);
            sp_io::storage::set(sp_core::storage::well_known_keys::CODE, &recovery);

            let mut sudo_key = [0u8; 32];
            sudo_key[..16].copy_from_slice(&sp_io::hashing::twox_128(b"Sudo"));
            sudo_key[16..].copy_from_slice(&sp_io::hashing::twox_128(b"Key"));
            sp_io::storage::set(&sudo_key, &[1]);
            pallet_constitution::PhaseFlags::<Runtime>::put(
                pallet_constitution::PhaseFlagsValue::SHADOW_MODE
                    | pallet_constitution::PhaseFlagsValue::SUDO_PRESENT,
            );
            let tvl_key = pallet_constitution::key16(b"phase3.tvl_cap");
            let deposit_key = pallet_constitution::key16(b"phase3.dep_cap");
            let tvl_cap = pallet_constitution::Params::<Runtime>::get(tvl_key)
                .expect("TVL cap exists")
                .value
                .as_u128()
                .saturating_add(1);
            let deposit_cap = pallet_constitution::Params::<Runtime>::get(deposit_key)
                .expect("deposit cap exists")
                .value
                .as_u128()
                .saturating_add(1);
            let primary_hash = [0x44; 32];
            let pid = 4_006;
            pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::put(
                pallet_execution_guard::PendingUpgrade {
                    hash: primary_hash,
                    authorized_at: 1,
                    applicable_at: 1,
                    target_spec_version: crate::VERSION.spec_version.saturating_sub(1),
                },
            );
            pallet_execution_guard::RecoveryImage::<Runtime>::put(
                pallet_execution_guard::RecoveryImageCommitment {
                    pid,
                    primary_hash,
                    hash: recovery_hash,
                    len: u32::try_from(recovery.len()).expect("recovery length fits"),
                    target_spec_version: crate::VERSION.spec_version,
                    attestation_id: 8,
                    committed_at: 1,
                },
            );
            pallet_execution_guard::PhaseFourBridge::<Runtime>::put(
                pallet_execution_guard::PhaseFourBridgeState::Scheduled {
                    pid,
                    code_hash: primary_hash,
                    plan: pallet_execution_guard::PhaseFourPlan {
                        tvl_cap,
                        deposit_cap,
                    },
                },
            );
            crate::configs::PhaseTransitionLock::put(true);
            crate::configs::PhaseTransitionApplied::put(true);
            crate::configs::RecoveryCodeApplied::put(true);
            crate::configs::RecoveryLockdown::put(true);
            crate::configs::RecoveryScheduledHash::put(recovery_hash);

            let _ = crate::migrations::TerminalRecoveryTransition::on_runtime_upgrade();

            assert!(!sp_io::storage::exists(&sudo_key));
            assert_eq!(
                pallet_constitution::PhaseFlags::<Runtime>::get(),
                pallet_constitution::PhaseFlagsValue::PARAM_ARMED,
            );
            assert_eq!(
                pallet_execution_guard::PhaseFourBridge::<Runtime>::get(),
                pallet_execution_guard::PhaseFourBridgeState::Consumed,
            );
            assert!(!crate::configs::RecoveryAwareMigrations::ongoing());
            assert!(!pallet_execution_guard::RecoveryImage::<Runtime>::exists());
            assert!(!crate::configs::RecoveryScheduledHash::exists());
        });
}

#[cfg(feature = "recovery")]
#[test]
fn terminal_recovery_without_a_release_specific_mbm_repair_fails_closed() {
    let recovery = b"terminal-recovery-with-unrepairable-retired-cursor".to_vec();
    tests::upgrade_ext_with_artifact_versions(vec![(recovery.clone(), crate::VERSION)])
        .execute_with(|| {
            let recovery_hash = sp_io::hashing::blake2_256(&recovery);
            let retired =
                pallet_migrations::MigrationCursor::Active(pallet_migrations::ActiveCursor {
                    index: 0,
                    inner_cursor: Some(BoundedVec::truncate_from(vec![0x71, 0x72])),
                    started_at: 3,
                });
            sp_io::storage::set(sp_core::storage::well_known_keys::CODE, &recovery);
            crate::configs::RecoveryCodeApplied::put(true);
            crate::configs::RecoveryLockdown::put(true);
            crate::configs::RecoveryScheduledHash::put(recovery_hash);
            crate::configs::RetiredMigrationCursor::put(retired.clone());
            pallet_execution_guard::RecoveryImage::<Runtime>::put(
                pallet_execution_guard::RecoveryImageCommitment {
                    pid: 4_005,
                    primary_hash: [0x44; 32],
                    hash: recovery_hash,
                    len: u32::try_from(recovery.len()).expect("test recovery length fits u32"),
                    target_spec_version: crate::VERSION.spec_version,
                    attestation_id: 8,
                    committed_at: System::block_number(),
                },
            );

            let _ = crate::migrations::TerminalRecoveryTransition::on_runtime_upgrade();

            assert!(crate::configs::RecoveryLockdown::get());
            assert!(crate::configs::RecoveryCodeApplied::get());
            assert_eq!(
                crate::configs::RecoveryScheduledHash::get(),
                Some(recovery_hash),
            );
            assert_eq!(crate::configs::RetiredMigrationCursor::get(), Some(retired),);
            assert!(
                pallet_execution_guard::RecoveryImage::<Runtime>::exists(),
                "the unconsumed recovery commitment must remain pinned for a corrected image",
            );
            assert!(
                crate::configs::RecoveryAwareMigrations::ongoing(),
                "an unrepairable ordinary MBM cutpoint must leave OnlyInherents engaged",
            );
            assert!(
                pallet_execution_guard::MigrationHalt::<Runtime>::get(),
                "the failed terminal repair must preserve the migration halt",
            );
        });
}

#[test]
fn recovery_lockdown_persists_only_inherents_after_cursor_retirement() {
    tests::development_ext().execute_with(|| {
        assert!(!pallet_migrations::Cursor::<Runtime>::exists());
        crate::configs::RecoveryLockdown::put(true);
        assert!(crate::configs::RecoveryAwareMigrations::ongoing());
        assert_eq!(
            crate::configs::RecoveryAwareMigrations::step(),
            frame_support::weights::Weight::zero(),
        );
        assert!(crate::configs::RecoveryLockdown::get());

        crate::configs::RecoveryBypass::put(true);
        assert!(
            !crate::configs::RecoveryAwareMigrations::ongoing(),
            "only the internal scheduling scope may temporarily bypass lockdown",
        );
        crate::configs::RecoveryBypass::kill();
        assert!(
            crate::configs::RecoveryAwareMigrations::ongoing(),
            "removing the scheduling bypass immediately restores lockdown",
        );

        let header = crate::Header::new(
            1,
            Default::default(),
            Default::default(),
            System::block_hash(0),
            Default::default(),
        );
        let mode = crate::Executive::initialize_block(&header);
        assert!(matches!(
            mode,
            sp_runtime::ExtrinsicInclusionMode::OnlyInherents
        ));
        assert!(
            crate::configs::RecoveryLockdown::get(),
            "block initialization must not consume the relay-wait lockdown",
        );
        assert!(!pallet_migrations::Cursor::<Runtime>::exists());
    });
}

#[test]
fn relay_abort_restores_exact_cursor_and_keeps_recovery_lockdown() {
    use cumulus_pallet_parachain_system::OnSystemEvent;
    use cumulus_primitives_core::relay_chain::UpgradeGoAhead;

    tests::development_ext().execute_with(|| {
        let retired = pallet_migrations::MigrationCursor::Active(pallet_migrations::ActiveCursor {
            index: 3,
            inner_cursor: Some(BoundedVec::truncate_from(vec![0x31, 0x32, 0x33])),
            started_at: 17,
        });
        let recovery_hash = [0xa5; 32];
        crate::configs::RecoveryLockdown::put(true);
        crate::configs::RetiredMigrationCursor::put(retired.clone());
        crate::configs::RecoveryScheduledHash::put(recovery_hash);
        cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::kill();
        cumulus_pallet_parachain_system::UpgradeGoAhead::<Runtime>::put(Some(
            UpgradeGoAhead::Abort,
        ));
        pallet_migrations::Cursor::<Runtime>::kill();

        crate::configs::ExecutionGuardSystemEvent::on_validation_data(
            &cumulus_primitives_core::PersistedValidationData::default(),
        );

        assert_eq!(pallet_migrations::Cursor::<Runtime>::get(), Some(retired));
        assert!(crate::configs::RecoveryLockdown::get());
        assert!(crate::configs::RecoveryAborted::get());
        assert!(crate::configs::RecoveryScheduledHash::get().is_none());
        assert!(crate::configs::RetiredMigrationCursor::exists());
        assert!(crate::configs::RecoveryAwareMigrations::ongoing());

        // The Abort is consumed exactly once. A repeated validation-data hook
        // cannot erase the restored cursor or silently reopen transactions.
        crate::configs::ExecutionGuardSystemEvent::on_validation_data(
            &cumulus_primitives_core::PersistedValidationData::default(),
        );
        assert!(pallet_migrations::Cursor::<Runtime>::exists());
        assert!(crate::configs::RecoveryLockdown::get());
        assert!(crate::configs::RecoveryAwareMigrations::ongoing());
    });
}

/// 09 §3.2(4) requires the diagnostic to carry the SDK cursor's exact bytes.
/// `CursorMaxLen` bounds only the migration-owned inner cursor, so the event
/// envelope must also cover the outer enum/index/option/length/block fields.
#[test]
fn migration_halted_cursor_bound_covers_the_full_sdk_cursor_encoding() {
    let sdk_max = <pallet_migrations::CursorOf<Runtime> as MaxEncodedLen>::max_encoded_len();
    assert!(
        sdk_max <= pallet_execution_guard::MAX_MIGRATION_HALT_CURSOR_BOUND as usize,
        "MigrationHalted cursor bound must cover the full encoded SDK cursor: {sdk_max} > {}",
        pallet_execution_guard::MAX_MIGRATION_HALT_CURSOR_BOUND,
    );
}

/// SQ-132 + SQ-146: the first storage migration retires both inert items and
/// advances `pallet-execution-guard`'s storage version 0 -> 1, and its version
/// gate makes a re-run a no-op. It creates no `pallet-migrations` cursor, so it
/// never engages the `OnlyInherents` lockdown.
#[test]
fn first_migration_retires_both_keys_bumps_version_and_is_idempotent() {
    tests::development_ext().execute_with(|| {
        // Model a pre-B16 upgraded chain: version 0, both keys populated.
        StorageVersion::new(0).put::<ExecutionGuard>();
        let bm = crate::migrations::retired::blocked_meters_key();
        let pm = crate::migrations::retired::progress_marker_key();
        // Cross-check each retired key against a *live* sibling that shares its
        // prefix, so a future pallet/instance rename cannot silently point the
        // migration at a non-existent key and orphan the real bytes (SQ-66 class).
        assert_eq!(
            bm[..16],
            pallet_execution_guard::HardGateBreach::<Runtime>::hashed_key()[..16],
            "BlockedMeters must share the live ExecutionGuard pallet prefix",
        );
        assert_eq!(
            pm[..16],
            crate::configs::MigrationHaltSources::hashed_key()[..16],
            "ProgressMarker must share the live BleavitRuntimeMigration instance prefix",
        );
        unhashed::put_raw(&bm, &[1u8, 2, 3]);
        unhashed::put_raw(&pm, &[4u8, 5, 6, 7]);
        assert!(unhashed::exists(&bm) && unhashed::exists(&pm));

        let _weight = <crate::migrations::RetireB16State as OnRuntimeUpgrade>::on_runtime_upgrade();

        assert!(!unhashed::exists(&bm), "BlockedMeters key retired");
        assert!(
            !unhashed::exists(&pm),
            "MigrationProgressMarker key retired"
        );
        assert_eq!(
            ExecutionGuard::on_chain_storage_version(),
            StorageVersion::new(1),
            "execution-guard storage version advanced 0 -> 1",
        );
        assert!(
            !pallet_migrations::Cursor::<Runtime>::exists(),
            "a single-block migration creates no MBM cursor (09 §3.2)",
        );

        // Re-run on the already-migrated chain: version 1 != FROM(0) -> no-op, so
        // re-seeded state is left untouched (the version gate, SQ-66 discipline).
        unhashed::put_raw(&bm, &[9u8]);
        let _ = <crate::migrations::RetireB16State as OnRuntimeUpgrade>::on_runtime_upgrade();
        assert!(
            unhashed::exists(&bm),
            "a version-gated re-run is a no-op and clears nothing",
        );
    });
}

/// The migration's try-runtime hooks prove the before/after transition: pre
/// records both keys present, post asserts both absent. Compiles only under
/// `try-runtime` (the hooks are `#[cfg(feature = \"try-runtime\")]`).
#[cfg(feature = "try-runtime")]
#[test]
fn first_migration_pre_and_post_upgrade_prove_the_transition() {
    use frame_support::traits::UncheckedOnRuntimeUpgrade;

    tests::development_ext().execute_with(|| {
        let bm = crate::migrations::retired::blocked_meters_key();
        let pm = crate::migrations::retired::progress_marker_key();
        unhashed::put_raw(&bm, &[1u8, 2, 3]);
        unhashed::put_raw(&pm, &[4u8]);

        let recorded = crate::migrations::RetireB16StateInner::pre_upgrade()
            .expect("pre_upgrade records present state");
        assert_eq!(
            recorded,
            vec![1u8, 1u8],
            "pre_upgrade sees both keys present"
        );

        let _ = crate::migrations::RetireB16StateInner::on_runtime_upgrade();

        crate::migrations::RetireB16StateInner::post_upgrade(recorded)
            .expect("post_upgrade proves both keys absent");
    });
}

/// 09 §3.2(4): the `MigrationHalted` diagnostic is emitted exactly once, on the
/// first activation of an execution-halt source, carrying the SDK failed step.
/// A second activation while the halt already stands does not re-emit.
#[test]
fn migration_halted_diagnostic_emits_once_on_first_activation() {
    tests::development_ext().execute_with(|| {
        System::set_block_number(1);

        let count_halted = || {
            System::events()
                .into_iter()
                .filter(|record| {
                    matches!(
                        record.event,
                        RuntimeEvent::ExecutionGuard(
                            pallet_execution_guard::Event::MigrationHalted { .. }
                        )
                    )
                })
                .count()
        };
        assert_eq!(count_halted(), 0);

        // First activation of the failure source emits the diagnostic.
        assert_eq!(
            crate::configs::MigrationFailureToGuard::failed(Some(5)),
            frame_support::migrations::FailedMigrationHandling::KeepStuck,
        );

        let emitted = System::events()
            .into_iter()
            .find_map(|record| match record.event {
                RuntimeEvent::ExecutionGuard(pallet_execution_guard::Event::MigrationHalted {
                    cursor,
                    failed_step,
                }) => Some((cursor, failed_step)),
                _ => None,
            });
        let (cursor, failed_step) = emitted.expect("a MigrationHalted diagnostic was emitted");
        assert_eq!(failed_step, Some(5), "carries the SDK-reported failed step");
        assert!(
            cursor.is_empty(),
            "no active cursor in this scenario -> empty diagnostic cursor bytes",
        );
        assert_eq!(count_halted(), 1);

        // A second source activation while already halted does NOT re-emit.
        let _ = crate::configs::MigrationFailureToGuard::failed(Some(6));
        assert_eq!(
            count_halted(),
            1,
            "first-activation-only: no re-emit while the halt still stands",
        );
    });
}
