//! 15 §4.1 suites for `pallet-constitution`: per-extrinsic × error-path ×
//! origin-misuse coverage, limit/boundary cases, try-state assertions, the
//! D-14 raw-key/layout pins, and a shell-vs-core differential check.

use crate::mock::*;
use crate::{
    empty_release_channel, genesis_capabilities, genesis_meters, genesis_params, key16,
    Capabilities, Capability, CapabilityRecord, ConstitutionOrigin, ConstitutionState, Error,
    Event, Meters, ParamClass, ParamRecord, ParamValue, Params, PhaseFlags, PhaseFlagsValue,
    ReleaseChannel, ReleaseChannelValue, CONTRACT_VERSION, MAX_CAPABILITIES, MAX_PARAMS,
    RELEASE_CHANNEL_LEN, RELEASE_CHANNEL_STORAGE_KEY,
};
use frame_support::dispatch::DispatchResult;
use frame_support::{assert_noop, assert_ok, traits::StorageVersion};
use sp_runtime::DispatchError;

use futarchy_primitives::{ParamKey, ProposalClass};

const OBS_KEY: &[u8] = b"mkt.obs_interval"; // PARAM class, Δ=5 abs, cooldown 1
const SLOTS_KEY: &[u8] = b"epoch.slots"; // META class, Δ=2 abs, cooldown 1
const LENGTH_KEY: &[u8] = b"epoch.length"; // META class, Δ=10 %, cooldown 2
const KEEPER_KEY: &[u8] = b"keeper.budget"; // PARAM class, Δ=×2, cooldown 1 (13 rule 6 key)
const HORIZON_KEY: &[u8] = b"epoch.horizon_k"; // META+values class, Δ=1 abs, cooldown 4

/// A valid 168-byte D-14 layout with recognizable field values.
fn channel_bytes() -> [u8; RELEASE_CHANNEL_LEN] {
    let mut bytes = [0u8; RELEASE_CHANNEL_LEN];
    bytes[0] = 1; // schema
    bytes[108..112].copy_from_slice(&42u32.to_le_bytes()); // updated_at
    bytes[112..116].copy_from_slice(&7u32.to_le_bytes()); // spec_version
    bytes[116..120].copy_from_slice(&11u32.to_le_bytes()); // pending_authorized_at
    bytes[164..168].copy_from_slice(&5u32.to_le_bytes()); // flags
    bytes
}

fn last_event() -> RuntimeEvent {
    frame_system::Pallet::<Test>::events()
        .pop()
        .expect("an event was deposited")
        .event
}

#[allow(clippy::too_many_arguments)]
fn assert_set_param_matches_core(
    core: &mut ConstitutionState,
    account: u64,
    authority: ConstitutionOrigin,
    key: ParamKey,
    value: ParamValue,
    epoch: u32,
    block: u32,
    expected: DispatchResult,
    step: u32,
) {
    set_epoch(epoch);
    System::set_block_number(block.into());
    let shell_before = Params::<Test>::get(key);
    let core_before = core.clone();
    let events_before = System::events().len();
    let shell = Constitution::set_param(RuntimeOrigin::signed(account), key, value);
    let model = core
        .dispatch_set_param(authority, key, value, epoch, block)
        .map_err(crate::Pallet::<Test>::map_core_error);
    assert_eq!(shell, model, "set_param shell/core result diverged");
    assert_eq!(shell, expected, "unexpected set_param result");
    if shell.is_err() {
        assert_eq!(Params::<Test>::get(key), shell_before);
        assert_eq!(*core, core_before);
        assert_eq!(System::events().len(), events_before);
    } else {
        assert_eq!(System::events().len(), events_before.saturating_add(1));
    }
    assert_states_agree(core, step);
}

// ---------------------------------------------------------------- genesis --

#[test]
fn genesis_seeds_code_owned_registry_and_passes_try_state() {
    new_test_ext().execute_with(|| {
        let seeded = genesis_params();
        assert_eq!(Params::<Test>::count() as usize, seeded.len());
        for record in &seeded {
            assert_eq!(Params::<Test>::get(record.key), Some(*record));
        }
        assert_eq!(Meters::<Test>::get().into_inner(), genesis_meters());
        assert!(Meters::<Test>::get().is_empty()); // I-17 envelopes live with their owners
        assert_eq!(
            Capabilities::<Test>::get().into_inner(),
            genesis_capabilities()
        );
        assert_eq!(PhaseFlags::<Test>::get(), 0);
        assert_eq!(ReleaseChannel::<Test>::get(), empty_release_channel());
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn genesis_overrides_phase_flags_and_release_channel() {
    let config = crate::GenesisConfig::<Test> {
        phase_flags: PhaseFlagsValue::SHADOW_MODE | PhaseFlagsValue::SUDO_PRESENT,
        release_channel: channel_bytes().to_vec(),
        ..Default::default()
    };
    new_test_ext_with(config).execute_with(|| {
        assert_eq!(
            PhaseFlags::<Test>::get(),
            PhaseFlagsValue::SHADOW_MODE | PhaseFlagsValue::SUDO_PRESENT
        );
        assert_eq!(ReleaseChannel::<Test>::get().spec_version(), 7);
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
#[should_panic(expected = "reserved PhaseFlags bits")]
fn genesis_rejects_reserved_phase_flag_bits() {
    let config = crate::GenesisConfig::<Test> {
        phase_flags: 1 << 8,
        ..Default::default()
    };
    new_test_ext_with(config);
}

#[test]
#[should_panic(expected = "exactly 168 bytes")]
fn genesis_rejects_wrong_length_release_channel() {
    let config = crate::GenesisConfig::<Test> {
        release_channel: vec![1u8; 167],
        ..Default::default()
    };
    new_test_ext_with(config);
}

#[test]
#[should_panic(expected = "schema-1 layout")]
fn genesis_rejects_bad_schema_release_channel() {
    let mut bytes = channel_bytes();
    bytes[0] = 2;
    let config = crate::GenesisConfig::<Test> {
        release_channel: bytes.to_vec(),
        ..Default::default()
    };
    new_test_ext_with(config);
}

// ------------------------------------------------------------------- D-14 --

#[test]
fn release_channel_raw_key_and_value_layout_are_frozen() {
    new_test_ext().execute_with(|| {
        // Raw key: twox128("Constitution") ++ twox128("ReleaseChannel").
        assert_eq!(
            ReleaseChannel::<Test>::hashed_key(),
            RELEASE_CHANNEL_STORAGE_KEY
        );
        // Raw value: exactly 168 bytes, no SCALE length prefix, offset-parsable.
        assert_ok!(Constitution::note_release_channel(channel_bytes()));
        let raw = sp_io::storage::get(&RELEASE_CHANNEL_STORAGE_KEY)
            .expect("release channel exists under the frozen raw key");
        assert_eq!(raw.len(), RELEASE_CHANNEL_LEN);
        assert_eq!(raw[0], 1);
        assert_eq!(&raw[112..116], &7u32.to_le_bytes());
        assert_eq!(&raw[164..168], &5u32.to_le_bytes());
    });
}

#[test]
fn phase_flag_bit_assignments_match_02_7_3() {
    // 02 §7.3 (frozen, append-only): 0 shadow, 1 PARAM, 2 TREASURY,
    // 3 CODE/META, 4 sudo, 5 ledger frozen, 6 dead-man, 7 reserve health.
    assert_eq!(PhaseFlagsValue::SHADOW_MODE, 1 << 0);
    assert_eq!(PhaseFlagsValue::PARAM_ARMED, 1 << 1);
    assert_eq!(PhaseFlagsValue::TREASURY_ARMED, 1 << 2);
    assert_eq!(PhaseFlagsValue::CODE_META_ARMED, 1 << 3);
    assert_eq!(PhaseFlagsValue::SUDO_PRESENT, 1 << 4);
    assert_eq!(PhaseFlagsValue::LEDGER_FROZEN, 1 << 5);
    assert_eq!(PhaseFlagsValue::DEAD_MAN_ENGAGED, 1 << 6);
    assert_eq!(PhaseFlagsValue::RESERVE_HEALTH_FLAG, 1 << 7);
}

#[test]
fn contract_version_and_bounds_reexports_hold() {
    assert_eq!(CONTRACT_VERSION, 9); // v9: paired primary/recovery metadata commitments
    assert_eq!(MAX_PARAMS, 128); // 13 §4 registry bound
    assert_eq!(MAX_CAPABILITIES, 64);
    assert_eq!(crate::MAX_METERS, 16);
    // Release-invariance pin; `fast-timing` (SQ-128) compresses this kernel value and
    // the canonical frozen-value guard lives in `futarchy-primitives`.
    #[cfg(not(feature = "fast-timing"))]
    assert_eq!(crate::kernel::DESCRIPTOR_LEAD_TIME_BLOCKS, 43_200);
}

// -------------------------------------------------------------- set_param --

#[test]
fn set_param_updates_in_bounds_key_and_emits() {
    new_test_ext().execute_with(|| {
        set_epoch(1); // cooldown 1 elapsed
        System::set_block_number(42);
        assert_ok!(Constitution::set_param(
            RuntimeOrigin::signed(PARAM_ACC),
            key16(OBS_KEY),
            ParamValue::U32(12)
        ));
        let record = Params::<Test>::get(key16(OBS_KEY)).unwrap();
        assert_eq!(record.value, ParamValue::U32(12));
        assert_eq!(record.last_changed_epoch, 1);
        assert_eq!(record.last_change_block, 42);
        assert_eq!(
            last_event(),
            RuntimeEvent::Constitution(Event::ParamUpdated {
                key: key16(OBS_KEY),
                value: ParamValue::U32(12),
            })
        );
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn set_param_authority_matrix_is_exact_per_class() {
    new_test_ext().execute_with(|| {
        set_epoch(1);
        // PARAM-class key: only FutarchyParam may write.
        for refused in [
            TREASURY_ACC,
            CODE_ACC,
            META_ACC,
            VALUES_ACC,
            GUARDIAN_ACC,
            PLAYBOOK_ACC,
            NOBODY_ACC,
        ] {
            assert_noop!(
                Constitution::set_param(
                    RuntimeOrigin::signed(refused),
                    key16(OBS_KEY),
                    ParamValue::U32(12)
                ),
                DispatchError::BadOrigin
            );
        }
        assert_noop!(
            Constitution::set_param(RuntimeOrigin::none(), key16(OBS_KEY), ParamValue::U32(12)),
            DispatchError::BadOrigin
        );
        // 09 §5.4: bootstrap sudo's exhaustive power list excludes parameter
        // administration — Root is refused for every class.
        assert_noop!(
            Constitution::set_param(RuntimeOrigin::root(), key16(OBS_KEY), ParamValue::U32(12)),
            DispatchError::BadOrigin
        );
        assert_ok!(Constitution::set_param(
            RuntimeOrigin::signed(PARAM_ACC),
            key16(OBS_KEY),
            ParamValue::U32(12)
        ));

        // META-class key: only FutarchyMeta.
        for refused in [PARAM_ACC, TREASURY_ACC, CODE_ACC, VALUES_ACC, GUARDIAN_ACC] {
            assert_noop!(
                Constitution::set_param(
                    RuntimeOrigin::signed(refused),
                    key16(SLOTS_KEY),
                    ParamValue::U8(6)
                ),
                DispatchError::BadOrigin
            );
        }
        assert_ok!(Constitution::set_param(
            RuntimeOrigin::signed(META_ACC),
            key16(SLOTS_KEY),
            ParamValue::U8(6)
        ));

        // META+values key (dual consent): enacted by FutarchyMeta — 06 §1
        // bars values from parameter keys; its consent is the guard's
        // execute-time ratification (06 §2.2). Cooldown 4 (13 §1).
        set_epoch(4);
        for refused in [PARAM_ACC, TREASURY_ACC, CODE_ACC, VALUES_ACC, GUARDIAN_ACC] {
            assert_noop!(
                Constitution::set_param(
                    RuntimeOrigin::signed(refused),
                    key16(HORIZON_KEY),
                    ParamValue::U8(3)
                ),
                DispatchError::BadOrigin
            );
        }
        assert_ok!(Constitution::set_param(
            RuntimeOrigin::signed(META_ACC),
            key16(HORIZON_KEY),
            ParamValue::U8(3)
        ));
    });
}

#[test]
fn set_param_error_paths_are_exact() {
    new_test_ext().execute_with(|| {
        set_epoch(1);
        assert_noop!(
            Constitution::set_param(
                RuntimeOrigin::signed(PARAM_ACC),
                key16(b"missing"),
                ParamValue::U32(12)
            ),
            Error::<Test>::UnknownParam
        );
        assert_noop!(
            Constitution::set_param(
                RuntimeOrigin::signed(PARAM_ACC),
                key16(OBS_KEY),
                ParamValue::U8(12)
            ),
            Error::<Test>::WrongType
        );
        assert_noop!(
            Constitution::set_param(
                RuntimeOrigin::signed(PARAM_ACC),
                key16(OBS_KEY),
                ParamValue::U32(4)
            ),
            Error::<Test>::BelowMin
        );
        assert_noop!(
            Constitution::set_param(
                RuntimeOrigin::signed(PARAM_ACC),
                key16(OBS_KEY),
                ParamValue::U32(51)
            ),
            Error::<Test>::AboveMax
        );
        // Δ boundary (absolute 5): 10 → 15 passes, 10 → 16 fails.
        assert_noop!(
            Constitution::set_param(
                RuntimeOrigin::signed(PARAM_ACC),
                key16(OBS_KEY),
                ParamValue::U32(16)
            ),
            Error::<Test>::DeltaTooLarge
        );
        assert_ok!(Constitution::set_param(
            RuntimeOrigin::signed(PARAM_ACC),
            key16(OBS_KEY),
            ParamValue::U32(15)
        ));
    });
}

#[test]
fn set_param_cooldown_boundary_is_exact() {
    new_test_ext().execute_with(|| {
        // Genesis: last_changed 0, cooldown 1 ⇒ epoch 0 refuses, epoch 1 admits.
        set_epoch(0);
        assert_noop!(
            Constitution::set_param(
                RuntimeOrigin::signed(PARAM_ACC),
                key16(OBS_KEY),
                ParamValue::U32(12)
            ),
            Error::<Test>::CooldownActive
        );
        set_epoch(1);
        assert_ok!(Constitution::set_param(
            RuntimeOrigin::signed(PARAM_ACC),
            key16(OBS_KEY),
            ParamValue::U32(12)
        ));
        // Updated at epoch 1: next admission at epoch 2, not 1.
        assert_noop!(
            Constitution::set_param(
                RuntimeOrigin::signed(PARAM_ACC),
                key16(OBS_KEY),
                ParamValue::U32(14)
            ),
            Error::<Test>::CooldownActive
        );
        set_epoch(2);
        assert_ok!(Constitution::set_param(
            RuntimeOrigin::signed(PARAM_ACC),
            key16(OBS_KEY),
            ParamValue::U32(14)
        ));
    });
}

#[test]
fn set_param_percent_and_factor_deltas_bind_at_the_boundary() {
    new_test_ext().execute_with(|| {
        // epoch.length: 10 % of 302,400 = 30,240 ⇒ 332,640 passes, 332,641 fails.
        set_epoch(2);
        assert_noop!(
            Constitution::set_param(
                RuntimeOrigin::signed(META_ACC),
                key16(LENGTH_KEY),
                ParamValue::U32(332_641)
            ),
            Error::<Test>::DeltaTooLarge
        );
        assert_ok!(Constitution::set_param(
            RuntimeOrigin::signed(META_ACC),
            key16(LENGTH_KEY),
            ParamValue::U32(332_640)
        ));
        // keeper.budget_epoch: ×2 factor ⇒ 24e9 passes, 24e9+1 fails; the
        // downward mirror (< value/2) fails symmetrically.
        set_epoch(3);
        assert_noop!(
            Constitution::set_param(
                RuntimeOrigin::signed(PARAM_ACC),
                key16(KEEPER_KEY),
                ParamValue::Balance(24_000_000_001)
            ),
            Error::<Test>::DeltaTooLarge
        );
        assert_ok!(Constitution::set_param(
            RuntimeOrigin::signed(PARAM_ACC),
            key16(KEEPER_KEY),
            ParamValue::Balance(24_000_000_000)
        ));
        set_epoch(4);
        assert_noop!(
            Constitution::set_param(
                RuntimeOrigin::signed(PARAM_ACC),
                key16(KEEPER_KEY),
                ParamValue::Balance(11_999_999_999)
            ),
            Error::<Test>::DeltaTooLarge
        );
    });
}

#[test]
fn welfare_low_knee_direction_matrix_matches_the_core() {
    for (key_index, key_name) in [b"welfare.thS_lo".as_slice(), b"welfare.thC_lo".as_slice()]
        .into_iter()
        .enumerate()
    {
        new_test_ext().execute_with(|| {
            let key = key16(key_name);
            let mut core = ConstitutionState::genesis();
            let record = Params::<Test>::get(key);
            assert!(record.is_some(), "welfare low-knee key must be seeded");
            let Some(record) = record else {
                return;
            };
            let interval = record.admissible_next_interval();
            assert!(interval.is_ok(), "welfare low-knee interval must be valid");
            let Ok((_, upper)) = interval else {
                return;
            };
            assert!(upper > record.value.as_u128());
            let raised = param_value_from_raw(record.value, upper);
            assert!(
                raised.is_some(),
                "welfare low-knee upper value must preserve its kind"
            );
            let Some(raised) = raised else {
                return;
            };
            let base_step = (key_index as u32).saturating_mul(16);

            // Increase matrix: only the constitution track may tighten.
            assert_set_param_matches_core(
                &mut core,
                ENTRENCHED_ACC,
                ConstitutionOrigin::EntrenchedTrack,
                key,
                raised,
                record.cooldown_epochs,
                1,
                Err(DispatchError::BadOrigin),
                base_step,
            );
            assert_set_param_matches_core(
                &mut core,
                VALUES_ACC,
                ConstitutionOrigin::ConstitutionalValues,
                key,
                raised,
                record.cooldown_epochs,
                2,
                Err(DispatchError::BadOrigin),
                base_step.saturating_add(1),
            );
            assert_set_param_matches_core(
                &mut core,
                CONSTITUTION_ACC,
                ConstitutionOrigin::ConstitutionTrack,
                key,
                raised,
                record.cooldown_epochs,
                3,
                Ok(()),
                base_step.saturating_add(2),
            );

            // Decrease matrix: only the entrenched track may un-tighten.
            let decrease_epoch = record.cooldown_epochs.saturating_mul(2);
            assert_set_param_matches_core(
                &mut core,
                CONSTITUTION_ACC,
                ConstitutionOrigin::ConstitutionTrack,
                key,
                record.value,
                decrease_epoch,
                4,
                Err(DispatchError::BadOrigin),
                base_step.saturating_add(3),
            );
            assert_set_param_matches_core(
                &mut core,
                ENTRENCHED_ACC,
                ConstitutionOrigin::EntrenchedTrack,
                key,
                record.value,
                decrease_epoch,
                5,
                Ok(()),
                base_step.saturating_add(4),
            );

            // Equality retains the row's CONST-class constitution route and
            // cannot be used by entrenched or bare values to stamp cooldown.
            assert_set_param_matches_core(
                &mut core,
                CONSTITUTION_ACC,
                ConstitutionOrigin::ConstitutionTrack,
                key,
                record.value,
                record.cooldown_epochs.saturating_mul(3),
                6,
                Ok(()),
                base_step.saturating_add(5),
            );
            assert_set_param_matches_core(
                &mut core,
                ENTRENCHED_ACC,
                ConstitutionOrigin::EntrenchedTrack,
                key,
                record.value,
                record.cooldown_epochs.saturating_mul(4),
                7,
                Err(DispatchError::BadOrigin),
                base_step.saturating_add(6),
            );
            assert_set_param_matches_core(
                &mut core,
                VALUES_ACC,
                ConstitutionOrigin::ConstitutionalValues,
                key,
                record.value,
                record.cooldown_epochs.saturating_mul(5),
                8,
                Err(DispatchError::BadOrigin),
                base_step.saturating_add(7),
            );

            // Entrenchment never authorizes crossing the launch floor.
            let below_floor_raw = record.min.as_u128().checked_sub(1);
            assert!(
                below_floor_raw.is_some(),
                "welfare launch floor must be non-zero"
            );
            let Some(below_floor_raw) = below_floor_raw else {
                return;
            };
            let below_floor = param_value_from_raw(record.value, below_floor_raw);
            assert!(
                below_floor.is_some(),
                "welfare below-floor value must preserve its kind"
            );
            let Some(below_floor) = below_floor else {
                return;
            };
            assert_set_param_matches_core(
                &mut core,
                ENTRENCHED_ACC,
                ConstitutionOrigin::EntrenchedTrack,
                key,
                below_floor,
                record.cooldown_epochs.saturating_mul(5),
                9,
                Err(Error::<Test>::BelowMin.into()),
                base_step.saturating_add(8),
            );
        });
    }
}

// --------------------------------------------------------- set_capability --

#[test]
fn set_capability_upserts_and_emits() {
    new_test_ext().execute_with(|| {
        let len_before = Capabilities::<Test>::get().len();
        let record = CapabilityRecord {
            class: ProposalClass::Meta,
            capability: Capability::AmendRegistry,
            enabled: true,
        };
        assert_ok!(Constitution::set_capability(
            RuntimeOrigin::signed(META_ACC),
            record
        ));
        assert!(Constitution::capability_enabled(
            ProposalClass::Meta,
            Capability::AmendRegistry
        ));
        assert_eq!(
            last_event(),
            RuntimeEvent::Constitution(Event::CapabilitySet {
                class: ProposalClass::Meta,
                capability: Capability::AmendRegistry,
                enabled: true,
            })
        );
        assert_eq!(Capabilities::<Test>::get().len(), len_before + 1);

        // Upsert replaces in place — the table must not grow.
        let disabled = CapabilityRecord {
            enabled: false,
            ..record
        };
        assert_ok!(Constitution::set_capability(
            RuntimeOrigin::signed(META_ACC),
            disabled
        ));
        assert!(!Constitution::capability_enabled(
            ProposalClass::Meta,
            Capability::AmendRegistry
        ));
        assert_eq!(Capabilities::<Test>::get().len(), len_before + 1);
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn set_capability_origin_misuse_is_refused() {
    new_test_ext().execute_with(|| {
        let record = CapabilityRecord {
            class: ProposalClass::Meta,
            capability: Capability::AmendRegistry,
            enabled: true,
        };
        // 06 §3.2 row 4: FutarchyMeta only — values participates via
        // ratification (06 §2.2), so VALUES_ACC is refused too.
        for refused in [
            PARAM_ACC,
            TREASURY_ACC,
            CODE_ACC,
            VALUES_ACC,
            GUARDIAN_ACC,
            PLAYBOOK_ACC,
            NOBODY_ACC,
        ] {
            assert_noop!(
                Constitution::set_capability(RuntimeOrigin::signed(refused), record),
                DispatchError::BadOrigin
            );
        }
        assert_noop!(
            Constitution::set_capability(RuntimeOrigin::root(), record),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Constitution::set_capability(RuntimeOrigin::none(), record),
            DispatchError::BadOrigin
        );
    });
}

#[test]
fn set_capability_limit_binds_at_the_bound() {
    // limit-coverage: Capabilities table
    new_test_ext().execute_with(|| {
        // Fill the table to exactly MAX_CAPABILITIES distinct rows…
        let mut i: u32 = 0;
        while (Capabilities::<Test>::get().len()) < MAX_CAPABILITIES {
            assert_ok!(Constitution::set_capability(
                RuntimeOrigin::signed(META_ACC),
                CapabilityRecord {
                    class: ProposalClass::Param,
                    capability: Capability::SetParam(key16(&i.to_le_bytes())),
                    enabled: true,
                }
            ));
            i += 1;
        }
        // …the 65th distinct row must refuse, an upsert must still pass.
        assert_noop!(
            Constitution::set_capability(
                RuntimeOrigin::signed(META_ACC),
                CapabilityRecord {
                    class: ProposalClass::Meta,
                    capability: Capability::AmendRegistry,
                    enabled: true,
                }
            ),
            Error::<Test>::TooManyCapabilities
        );
        assert_ok!(Constitution::set_capability(
            RuntimeOrigin::signed(META_ACC),
            CapabilityRecord {
                class: ProposalClass::Param,
                capability: Capability::SetParam(key16(&0u32.to_le_bytes())),
                enabled: false,
            }
        ));
        assert_ok!(Constitution::do_try_state());
    });
}

// --------------------------------------------------------- set_phase_flag --

#[test]
fn set_phase_flag_is_root_only_and_emits() {
    new_test_ext().execute_with(|| {
        // 09 §5.4: bootstrap sudo (Root) arms flags in Phases 0–3.
        assert_ok!(Constitution::set_phase_flag(
            RuntimeOrigin::root(),
            PhaseFlagsValue::SUDO_PRESENT,
            true
        ));
        assert_eq!(PhaseFlags::<Test>::get(), PhaseFlagsValue::SUDO_PRESENT);
        assert_eq!(
            last_event(),
            RuntimeEvent::Constitution(Event::PhaseFlagSet {
                flag: PhaseFlagsValue::SUDO_PRESENT,
                enabled: true,
                bits: PhaseFlagsValue::SUDO_PRESENT,
            })
        );
        assert_ok!(Constitution::set_phase_flag(
            RuntimeOrigin::root(),
            PhaseFlagsValue::PARAM_ARMED,
            true
        ));
        assert_ok!(Constitution::set_phase_flag(
            RuntimeOrigin::root(),
            PhaseFlagsValue::SUDO_PRESENT,
            false
        ));
        assert_eq!(PhaseFlags::<Test>::get(), PhaseFlagsValue::PARAM_ARMED);
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn machinery_bits_have_dedicated_internal_setters() {
    new_test_ext().execute_with(|| {
        // 02 §7.3 bits 5–7 are machinery-written (sibling pallets) through
        // bit-specific runtime-internal setters — no origin, no arbitrary-bit
        // surface: an internal caller cannot reach arming bits at all.
        assert_ok!(Constitution::note_ledger_frozen(true));
        assert_ok!(Constitution::note_reserve_health(true));
        assert_ok!(Constitution::note_dead_man_engaged(true));
        assert_eq!(
            PhaseFlags::<Test>::get(),
            PhaseFlagsValue::LEDGER_FROZEN
                | PhaseFlagsValue::RESERVE_HEALTH_FLAG
                | PhaseFlagsValue::DEAD_MAN_ENGAGED
        );
        assert_ok!(Constitution::note_ledger_frozen(false));
        assert_ok!(Constitution::note_dead_man_engaged(false));
        assert_eq!(
            PhaseFlags::<Test>::get(),
            PhaseFlagsValue::RESERVE_HEALTH_FLAG
        );
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn set_phase_flag_rejects_reserved_bits_and_origin_misuse() {
    new_test_ext().execute_with(|| {
        // Everything outside the 09 §5.4 armable mask is refused — the
        // machinery bits (5–7) and the reserved bits (8–31) alike.
        assert_noop!(
            Constitution::set_phase_flag(RuntimeOrigin::root(), 1 << 8, true),
            Error::<Test>::FlagNotArmable
        );
        for machinery in [
            PhaseFlagsValue::LEDGER_FROZEN,
            PhaseFlagsValue::DEAD_MAN_ENGAGED,
            PhaseFlagsValue::RESERVE_HEALTH_FLAG,
        ] {
            assert_noop!(
                Constitution::set_phase_flag(RuntimeOrigin::root(), machinery, true),
                Error::<Test>::FlagNotArmable
            );
        }
        // No custom origin may arm flags — guardian and playbook included
        // (their powers are exhaustively enumerated, 06 §5.2, I-23).
        for refused in [
            PARAM_ACC,
            TREASURY_ACC,
            CODE_ACC,
            META_ACC,
            VALUES_ACC,
            GUARDIAN_ACC,
            PLAYBOOK_ACC,
            NOBODY_ACC,
        ] {
            assert_noop!(
                Constitution::set_phase_flag(
                    RuntimeOrigin::signed(refused),
                    PhaseFlagsValue::SUDO_PRESENT,
                    true
                ),
                DispatchError::BadOrigin
            );
        }
        assert_noop!(
            Constitution::set_phase_flag(
                RuntimeOrigin::none(),
                PhaseFlagsValue::SUDO_PRESENT,
                true
            ),
            DispatchError::BadOrigin
        );
    });
}

// ----------------------------------------------------- set_release_channel --

#[test]
fn set_release_channel_is_constitutional_values_only() {
    new_test_ext().execute_with(|| {
        // 02 §12: writer (b) is exactly ConstitutionalValues — CODE, META,
        // guardian, playbook, signed, none AND bootstrap Root are refused.
        for refused in [
            PARAM_ACC,
            TREASURY_ACC,
            CODE_ACC,
            META_ACC,
            GUARDIAN_ACC,
            PLAYBOOK_ACC,
            NOBODY_ACC,
        ] {
            assert_noop!(
                Constitution::set_release_channel(RuntimeOrigin::signed(refused), channel_bytes()),
                DispatchError::BadOrigin
            );
        }
        assert_noop!(
            Constitution::set_release_channel(RuntimeOrigin::root(), channel_bytes()),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Constitution::set_release_channel(RuntimeOrigin::none(), channel_bytes()),
            DispatchError::BadOrigin
        );

        // Seed writer (a)'s fields, then have writer (b) attempt to erase and
        // replace them while changing its own descriptor metadata.
        assert_ok!(Constitution::note_release_channel(channel_bytes()));
        let mut caller = channel_bytes();
        caller[108..112].copy_from_slice(&43u32.to_le_bytes());
        caller[112..116].copy_from_slice(&99u32.to_le_bytes());
        caller[116..120].copy_from_slice(&0u32.to_le_bytes());
        caller[164..168].copy_from_slice(&2u32.to_le_bytes());
        assert_ok!(Constitution::set_release_channel(
            RuntimeOrigin::signed(VALUES_ACC),
            caller
        ));
        let stored = ReleaseChannel::<Test>::get();
        // 02 §12: offset 108 is stamped from the current block, so the
        // caller's 43 is ignored. A lawful writer must not be able to
        // backdate the freshness a stranded reader depends on.
        assert_eq!(stored.updated_at(), System::block_number() as u32);
        assert_ne!(stored.updated_at(), 43);
        assert_eq!(stored.spec_version(), 7);
        assert_eq!(stored.pending_authorized_at(), 11);
        assert_eq!(stored.flags(), 6);
        assert_eq!(
            last_event(),
            RuntimeEvent::Constitution(Event::ReleaseChannelSet {
                spec_version: 7,
                updated_at: System::block_number() as u32,
            })
        );
    });
}

#[test]
fn set_release_channel_rejects_bad_schema_and_reserved_flags() {
    new_test_ext().execute_with(|| {
        let mut bad = channel_bytes();
        bad[0] = 2;
        assert_noop!(
            Constitution::set_release_channel(RuntimeOrigin::signed(VALUES_ACC), bad),
            Error::<Test>::BadReleaseSchema
        );
        // 02 §12: flags bits 3–31 are reserved zero.
        let mut reserved = channel_bytes();
        reserved[164..168].copy_from_slice(&(1u32 << 5).to_le_bytes());
        assert_noop!(
            Constitution::set_release_channel(RuntimeOrigin::signed(VALUES_ACC), reserved),
            Error::<Test>::BadReleaseSchema
        );
        // All three defined flag bits together remain valid.
        let mut defined = channel_bytes();
        defined[164..168].copy_from_slice(&ReleaseChannelValue::FLAGS_MASK.to_le_bytes());
        assert_ok!(Constitution::set_release_channel(
            RuntimeOrigin::signed(VALUES_ACC),
            defined
        ));
    });
}

#[test]
fn note_release_channel_is_the_guard_write_path() {
    new_test_ext().execute_with(|| {
        // 02 §12 writer (a): runtime-internal, no origin — still validates
        // the frozen layout.
        assert_ok!(Constitution::note_release_channel(channel_bytes()));
        assert_eq!(ReleaseChannel::<Test>::get().updated_at(), 42);
        let mut bad = channel_bytes();
        bad[0] = 0;
        assert_noop!(
            Constitution::note_release_channel(bad),
            Error::<Test>::BadReleaseSchema
        );
    });
}

// ------------------------------------------------------------ charge_meter --
// No spec document defines a `charge_meter` extrinsic (06 §3.2 closed matrix),
// so meter charging is runtime-internal only (PLAN SQ-12); the pallet's call
// enum must not contain it, and the internal API keeps I-7/I-17 semantics.

#[test]
fn charge_meter_is_not_an_extrinsic() {
    // The call enum carries exactly the four spec-named dispatchables.
    let calls = <crate::pallet::Call<Test> as frame_support::traits::GetCallName>::get_call_names();
    assert_eq!(
        calls,
        &[
            "set_param",
            "set_capability",
            "set_phase_flag",
            "set_release_channel",
            "amend_registry"
        ]
    );
}

/// Genesis carries no meters (I-17 envelopes live with their owning pallets,
/// 15 §1); tests seed the generic primitive directly.
fn seed_meters(limits: &[u128]) {
    let meters: Vec<crate::Meter> = limits.iter().map(|l| crate::Meter::new(*l, 0)).collect();
    Meters::<Test>::put(frame_support::BoundedVec::<
        crate::Meter,
        frame_support::traits::ConstU32<16>,
    >::truncate_from(meters));
}

#[test]
fn charge_meter_internal_envelope_boundary_and_epoch_reset() {
    new_test_ext().execute_with(|| {
        let limit = 6_000_000_000u128;
        seed_meters(&[limit, 0]);
        // Exactly the envelope passes (I-7 monotone within the window)…
        assert_ok!(Constitution::charge_meter_internal(0, limit));
        assert_eq!(
            last_event(),
            RuntimeEvent::Constitution(Event::MeterCharged {
                index: 0,
                amount: limit,
                spent: limit,
            })
        );
        // …one more unit does not (I-17 envelope never exceeded).
        assert_noop!(
            Constitution::charge_meter_internal(0, 1),
            Error::<Test>::MeterExhausted
        );
        // A later epoch opens a fresh window.
        set_epoch(1);
        assert_ok!(Constitution::charge_meter_internal(0, 1));
        assert_eq!(Meters::<Test>::get()[0].spent, 1);
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn charge_meter_internal_error_paths() {
    new_test_ext().execute_with(|| {
        seed_meters(&[6_000_000_000, 0]);
        assert_noop!(
            Constitution::charge_meter_internal(99, 1),
            Error::<Test>::UnknownMeter
        );
        // Overflow is rejected, never wrapped (G-1).
        assert_ok!(Constitution::charge_meter_internal(0, 1));
        assert_noop!(
            Constitution::charge_meter_internal(0, u128::MAX),
            Error::<Test>::MeterOverflow
        );
        // Zero-limit meter refuses any positive charge.
        assert_noop!(
            Constitution::charge_meter_internal(1, 1),
            Error::<Test>::MeterExhausted
        );
    });
}

// -------------------------------------------------------------- try-state --

#[test]
fn try_state_rejects_corrupt_storage_shapes() {
    new_test_ext().execute_with(|| {
        assert_ok!(Constitution::do_try_state());

        // Map key diverging from the embedded record key.
        let mut stray = genesis_params()[0];
        stray.key = key16(b"not.the.map.key");
        Params::<Test>::insert(key16(b"evil"), stray);
        assert!(Constitution::do_try_state().is_err());
        Params::<Test>::remove(key16(b"evil"));
        assert_ok!(Constitution::do_try_state());

        // Value outside its own [min, max] (I-6).
        let mut oob = Params::<Test>::get(key16(OBS_KEY)).unwrap();
        oob.value = ParamValue::U32(1_000);
        Params::<Test>::insert(key16(OBS_KEY), oob);
        assert!(Constitution::do_try_state().is_err());
    });
}

#[test]
fn try_state_rejects_reserved_flags_overspent_meters_and_overflown_registry() {
    new_test_ext().execute_with(|| {
        PhaseFlags::<Test>::put(1 << 9);
        assert!(Constitution::do_try_state().is_err());
        PhaseFlags::<Test>::put(0);
        assert_ok!(Constitution::do_try_state());

        seed_meters(&[5]);
        Meters::<Test>::mutate(|m| m[0].spent = m[0].limit + 1);
        assert!(Constitution::do_try_state().is_err());
        Meters::<Test>::mutate(|m| m[0].spent = 0);
        assert_ok!(Constitution::do_try_state());

        // 13 §4 / core bound: a 65-key registry must fail try-state (I-21).
        let template = genesis_params()[0];
        let mut n: u32 = 0;
        while (Params::<Test>::count() as usize) <= MAX_PARAMS {
            let key = key16(&n.to_le_bytes());
            Params::<Test>::insert(key, ParamRecord { key, ..template });
            n += 1;
        }
        assert!(Constitution::do_try_state().is_err());
    });
}

#[test]
fn try_state_requires_v2_and_all_probe_rows() {
    new_test_ext().execute_with(|| {
        StorageVersion::new(0).put::<Constitution>();
        assert!(Constitution::do_try_state().is_err());
        StorageVersion::new(2).put::<Constitution>();

        for name in [
            b"ops.probe_fee".as_slice(),
            b"ops.probe_rate".as_slice(),
            b"res.probe_int".as_slice(),
            b"res.probe_to".as_slice(),
            b"res.probe_amount".as_slice(),
            b"res.fail_thr".as_slice(),
            b"res.recover_thr".as_slice(),
        ] {
            let key = key16(name);
            let record = Params::<Test>::get(key).expect("probe pricing row");
            Params::<Test>::remove(key);
            assert!(
                Constitution::do_try_state().is_err(),
                "missing {name:?} was not rejected",
            );
            Params::<Test>::insert(key, record);
            assert_ok!(Constitution::do_try_state());
        }
    });
}

#[test]
fn try_state_accepts_a_lawfully_amended_probe_pricing_row() {
    new_test_ext().execute_with(|| {
        let key = key16(b"ops.probe_rate");
        let mut record = Params::<Test>::get(key).expect("probe rate row");
        let amended = record
            .value
            .as_u128()
            .checked_add(1)
            .expect("fixture value has room");
        assert!(amended <= record.max.as_u128());
        record.value = ParamValue::Balance(amended);
        Params::<Test>::insert(key, record);

        assert_ok!(Constitution::do_try_state());
    });
}

// ------------------------------------------------------------ differential --

#[test]
fn shell_and_core_agree_on_the_same_operation_sequence() {
    new_test_ext().execute_with(|| {
        set_epoch(1);
        let mut core = ConstitutionState::genesis();

        // set_param
        assert_ok!(Constitution::set_param(
            RuntimeOrigin::signed(PARAM_ACC),
            key16(OBS_KEY),
            ParamValue::U32(12)
        ));
        core.dispatch_set_param(
            ConstitutionOrigin::FutarchyParam,
            key16(OBS_KEY),
            ParamValue::U32(12),
            1,
            1,
        )
        .unwrap();

        // set_capability (insert + upsert)
        let cap = CapabilityRecord {
            class: ProposalClass::Meta,
            capability: Capability::AmendRegistry,
            enabled: true,
        };
        assert_ok!(Constitution::set_capability(
            RuntimeOrigin::signed(META_ACC),
            cap
        ));
        core.dispatch_set_capability(ConstitutionOrigin::FutarchyMeta, cap)
            .unwrap();
        let cap_off = CapabilityRecord {
            enabled: false,
            ..cap
        };
        assert_ok!(Constitution::set_capability(
            RuntimeOrigin::signed(META_ACC),
            cap_off
        ));
        core.dispatch_set_capability(ConstitutionOrigin::FutarchyMeta, cap_off)
            .unwrap();

        // set_phase_flag (Root-only, 09 §5.4)
        assert_ok!(Constitution::set_phase_flag(
            RuntimeOrigin::root(),
            PhaseFlagsValue::SUDO_PRESENT,
            true
        ));
        core.dispatch_set_phase_flag(
            ConstitutionOrigin::Root,
            PhaseFlagsValue::SUDO_PRESENT,
            true,
        )
        .unwrap();

        // charge_meter (runtime-internal; core models the treasury path);
        // seed the same meter fixture on both sides — genesis is empty.
        seed_meters(&[1_000_000]);
        core.meters = vec![crate::Meter::new(1_000_000, 0)];
        assert_ok!(Constitution::charge_meter_internal(0, 1_000));
        core.dispatch_charge_meter(ConstitutionOrigin::FutarchyTreasury, 0, 1_000, 1)
            .unwrap();

        // set_release_channel
        assert_ok!(Constitution::set_release_channel(
            RuntimeOrigin::signed(VALUES_ACC),
            channel_bytes()
        ));
        core.dispatch_set_release_channel(
            ConstitutionOrigin::ConstitutionalValues,
            channel_bytes(),
            System::block_number() as u32,
        )
        .unwrap();

        // Compare end states (params order-normalized by key).
        let mut shell_params: Vec<ParamRecord> = Params::<Test>::iter_values().collect();
        shell_params.sort_by_key(|r| r.key);
        let mut core_params = core.params.clone();
        core_params.sort_by_key(|r| r.key);
        assert_eq!(shell_params, core_params);
        assert_eq!(Meters::<Test>::get().into_inner(), core.meters);
        assert_eq!(Capabilities::<Test>::get().into_inner(), core.capabilities);
        assert_eq!(PhaseFlags::<Test>::get(), core.phase_flags.bits());
        assert_eq!(ReleaseChannel::<Test>::get(), core.release_channel);
        assert_ok!(Constitution::do_try_state());
        core.try_state().unwrap();
    });
}

// --------------------------------------------------------------- misc API --

#[test]
fn read_helpers_serve_sibling_pallets() {
    new_test_ext().execute_with(|| {
        assert_eq!(Constitution::phase_flags(), 0);
        assert_eq!(
            Constitution::param(&key16(OBS_KEY)).map(|r| r.value),
            Some(ParamValue::U32(10))
        );
        assert!(Constitution::param(&key16(b"missing")).is_none());
        assert!(Constitution::capability_enabled(
            ProposalClass::Treasury,
            Capability::TreasurySpend
        ));
        assert!(!Constitution::capability_enabled(
            ProposalClass::Meta,
            Capability::AmendRegistry
        ));
    });
}

#[test]
fn extrinsic_value_types_do_not_admit_wrong_kinds_via_scale() {
    // ParamValue is a closed typed enum: a U8 payload cannot masquerade as a
    // U32 record update (WrongType), pinned above; here we pin the SCALE
    // width of the release-channel argument (no length prefix on [u8; 168]).
    use parity_scale_codec::Encode;
    assert_eq!(channel_bytes().encode().len(), RELEASE_CHANNEL_LEN);
    assert_eq!(
        ReleaseChannelValue::new(channel_bytes()).unwrap().encode(),
        channel_bytes().to_vec()
    );
}

// ------------------------------------------------- 13 §1 registry encodings --

#[test]
fn genesis_registry_matches_13_1_row_encodings() {
    new_test_ext().execute_with(|| {
        // Every 13 §1 row with a scalar concrete default and no open
        // [VERIFY] tag is seeded (102 total, incl. per-class suffix keys and
        // rule-6 short keys; +2 for keeper.rebate/dis.merit_min and +2 for the
        // reserve-probe pricing rows); spot-pin the unit encodings per kind.
        assert_eq!(Params::<Test>::count(), 102);

        // Per-class suffix keys (13 rule 6) — δ floors, kernel-capped.
        // Phase-0-calibrated (V-12): dec.delta.meta 0.090 on the 1e9 grid.
        let delta_meta = Params::<Test>::get(key16(b"dec.delta.meta")).unwrap();
        assert_eq!(
            delta_meta.value,
            ParamValue::Fixed(futarchy_primitives::FixedU64(90_000_000))
        );
        assert!(delta_meta.kernel_bounded);

        for (key, value, min, max) in [
            (
                b"gate.v_min.param".as_slice(),
                10_000_000_000,
                5_000_000_000,
                50_000_000_000,
            ),
            (
                b"gate.v_min.trs".as_slice(),
                25_000_000_000,
                12_500_000_000,
                125_000_000_000,
            ),
            (
                b"gate.v_min.code".as_slice(),
                60_000_000_000,
                30_000_000_000,
                300_000_000_000,
            ),
            (
                b"gate.v_min.meta".as_slice(),
                120_000_000_000,
                60_000_000_000,
                600_000_000_000,
            ),
        ] {
            assert_eq!(
                Params::<Test>::get(key16(key))
                    .map(|record| { (record.value, record.min, record.max, record.class) }),
                Some((
                    ParamValue::Balance(value),
                    ParamValue::Balance(min),
                    ParamValue::Balance(max),
                    ParamClass::Meta,
                )),
                "{key:?} must be a bounded genesis-seeded META record",
            );
        }

        // Rule-6 short key for a >16-byte dotted name.
        let slash = Params::<Test>::get(key16(b"intake.slash_pct")).unwrap();
        assert_eq!(slash.value, ParamValue::Percent(10));
        assert!(slash.kernel_bounded); // 5 % K floor

        // Fractional-percent rows are Perbill (13 type fix).
        let budget = Params::<Test>::get(key16(b"pol.budget_epoch")).unwrap();
        assert_eq!(budget.value, ParamValue::Perbill(7_500_000)); // 0.75 %

        let fee = Params::<Test>::get(key16(b"mkt.fee")).unwrap();
        assert_eq!(fee.value, ParamValue::Perbill(3_000_000)); // 30 bps
        assert_eq!(fee.max, ParamValue::Perbill(10_000_000)); // 100 bps
        assert_eq!(fee.class, ParamClass::Param);

        let window = Params::<Test>::get(key16(b"dec.window")).unwrap();
        assert_eq!(window.value, ParamValue::U32(43_200));
        assert_eq!(
            window.min,
            ParamValue::U32(crate::kernel::DECISION_WINDOW_FLOOR_BLOCKS)
        );
        assert_eq!(window.max, ParamValue::U32(86_400));
        assert_eq!(window.max_delta, Some(crate::MaxDelta::Percent(20)));

        let p_max = Params::<Test>::get(key16(b"gate.p_max")).unwrap();
        // 0.10 K ceiling comes from the kernel constant, not a literal copy.
        assert_eq!(
            p_max.max,
            ParamValue::Fixed(futarchy_primitives::FixedU64(
                crate::kernel::GATE_P_MAX_CEILING_1E9
            ))
        );
        assert_eq!(p_max.class, ParamClass::MetaAndValues);
        assert_eq!(p_max.cooldown_epochs, 4);

        let att_bond = Params::<Test>::get(key16(b"att.bond")).unwrap();
        assert_eq!(att_bond.value, ParamValue::Balance(25_000_000_000_000_000)); // 25k VIT, 12 dp
        assert_eq!(att_bond.class, ParamClass::Entrenched);
        assert_eq!(att_bond.max_delta, Some(crate::MaxDelta::Factor(2)));

        let min_split = Params::<Test>::get(key16(b"ledger.min_split")).unwrap();
        assert_eq!(
            min_split.value,
            ParamValue::Balance(crate::kernel::MIN_SPLIT_USDC)
        );
        assert_eq!(min_split.min, min_split.value); // K floor

        let b_gate = Params::<Test>::get(key16(b"pol.b_gate")).unwrap();
        assert_eq!(b_gate.value, ParamValue::Balance(7_500_000_000)); // 7,500 USDC
        assert_eq!(b_gate.class, ParamClass::Treasury);

        assert_ok!(Constitution::do_try_state());
    });
}

fn param_key_name(key: ParamKey) -> String {
    let length = key.iter().position(|byte| *byte == 0).unwrap_or(key.len());
    String::from_utf8(key[..length].to_vec()).expect("genesis ParamKeys are valid UTF-8")
}

fn param_value_from_raw(kind: ParamValue, raw: u128) -> Option<ParamValue> {
    match kind {
        ParamValue::U8(_) => u8::try_from(raw).ok().map(ParamValue::U8),
        ParamValue::U32(_) => u32::try_from(raw).ok().map(ParamValue::U32),
        ParamValue::Balance(_) => Some(ParamValue::Balance(raw)),
        ParamValue::Fixed(_) => u64::try_from(raw)
            .ok()
            .map(|value| ParamValue::Fixed(futarchy_primitives::FixedU64(value))),
        ParamValue::Percent(_) => u8::try_from(raw).ok().map(ParamValue::Percent),
        ParamValue::Perbill(_) => u32::try_from(raw).ok().map(ParamValue::Perbill),
    }
}

fn governance_origin_for(record: ParamRecord, next: ParamValue) -> RuntimeOrigin {
    let welfare_low_knee =
        record.key == key16(b"welfare.thS_lo") || record.key == key16(b"welfare.thC_lo");
    if welfare_low_knee {
        let account = if next.as_u128() < record.value.as_u128() {
            ENTRENCHED_ACC
        } else {
            CONSTITUTION_ACC
        };
        return RuntimeOrigin::signed(account);
    }
    let account = match record.class {
        ParamClass::Param => PARAM_ACC,
        ParamClass::Treasury => TREASURY_ACC,
        ParamClass::Meta | ParamClass::MetaAndValues => META_ACC,
        ParamClass::Const | ParamClass::Entrenched => VALUES_ACC,
    };
    RuntimeOrigin::signed(account)
}

fn delta_past_limit(record: ParamRecord) -> Option<ParamValue> {
    let value = record.value.as_u128();
    let min = record.min.as_u128();
    let max = record.max.as_u128();
    let outside_distance = match record.max_delta? {
        crate::MaxDelta::Absolute(bound) => bound.as_u128().checked_add(1)?,
        crate::MaxDelta::Percent(percent) => value
            .saturating_mul(u128::from(percent))
            .checked_div(100)?
            .checked_add(1)?,
        crate::MaxDelta::Factor(factor) => {
            let factor = u128::from(factor);
            let upper = value.saturating_mul(factor);
            if upper < max {
                return param_value_from_raw(record.value, upper.checked_add(1)?);
            }
            if value > 0 {
                let lower = value.checked_sub(1)?.checked_div(factor)?;
                if lower >= min {
                    return param_value_from_raw(record.value, lower);
                }
            }
            return None;
        }
    };
    if let Some(upper) = value.checked_add(outside_distance) {
        if upper <= max {
            return param_value_from_raw(record.value, upper);
        }
    }
    let lower = value.checked_sub(outside_distance)?;
    if lower >= min {
        param_value_from_raw(record.value, lower)
    } else {
        None
    }
}

#[test]
fn generated_genesis_key_fixture_matches_the_seeded_registry() {
    let mut keys: Vec<String> = genesis_params()
        .into_iter()
        .map(|record| param_key_name(record.key))
        .collect();
    keys.sort();
    let body = keys
        .iter()
        .map(|key| format!("  \"{key}\""))
        .collect::<Vec<_>>()
        .join(",\n");
    let rendered = format!("[\n{body}\n]\n");
    assert_eq!(
        include_str!("../../../tools/limit-coverage/genesis-keys.json"),
        rendered,
        "regenerate genesis-keys.json from constitution_core::genesis_params"
    );
}

/// Covers amendment dispatch bounds only; keys annotated `consumer_binding` in
/// the S3 registry still have kernel-constant consumers until B10 rewires them.
#[test]
fn generated_registry_suite_rejects_every_seeded_key_past_its_amendment_limits() {
    for expected in genesis_params() {
        let key_name = param_key_name(expected.key);
        new_test_ext().execute_with(|| {
            let record = Params::<Test>::get(expected.key)
                .unwrap_or_else(|| panic!("generated key {key_name} is not seeded"));

            if let Some(above_max) = record
                .max
                .as_u128()
                .checked_add(1)
                .and_then(|raw| param_value_from_raw(record.value, raw))
            {
                assert_noop!(
                    Constitution::set_param(
                        governance_origin_for(record, above_max),
                        record.key,
                        above_max
                    ),
                    Error::<Test>::AboveMax
                );
            }
            if let Some(below_min) = record
                .min
                .as_u128()
                .checked_sub(1)
                .and_then(|raw| param_value_from_raw(record.value, raw))
            {
                assert_noop!(
                    Constitution::set_param(
                        governance_origin_for(record, below_min),
                        record.key,
                        below_min
                    ),
                    Error::<Test>::BelowMin
                );
            }

            if record.max_delta.is_some() {
                let candidate = delta_past_limit(record).unwrap_or_else(|| {
                    panic!("generated key {key_name} has no in-bounds past-Δ candidate")
                });
                set_epoch(record.cooldown_epochs);
                assert_noop!(
                    Constitution::set_param(
                        governance_origin_for(record, candidate),
                        record.key,
                        candidate
                    ),
                    Error::<Test>::DeltaTooLarge
                );
            }

            if record.kernel_bounded {
                assert_noop!(
                    Constitution::amend_registry(
                        RuntimeOrigin::signed(META_ACC),
                        record.key,
                        record.min,
                        record.max,
                        record.max_delta,
                        record.cooldown_epochs,
                    ),
                    Error::<Test>::KernelBoundImmutable
                );
            }

            if record.cooldown_epochs > 0 {
                set_epoch(record.cooldown_epochs);
                assert_ok!(Constitution::set_param(
                    governance_origin_for(record, record.value),
                    record.key,
                    record.value
                ));
                assert_noop!(
                    Constitution::set_param(
                        governance_origin_for(record, record.value),
                        record.key,
                        record.value
                    ),
                    Error::<Test>::CooldownActive
                );
            }
        });
    }
}

// ------------------------------------------- randomized differential (PRNG) --

/// Deterministic xorshift64 — tests must not use ambient randomness.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

fn assert_states_agree(core: &ConstitutionState, step: u32) {
    let mut shell_params: Vec<ParamRecord> = Params::<Test>::iter_values().collect();
    shell_params.sort_by_key(|r| r.key);
    let mut core_params = core.params.clone();
    core_params.sort_by_key(|r| r.key);
    assert_eq!(shell_params, core_params, "params diverged at step {step}");
    assert_eq!(
        Meters::<Test>::get().into_inner(),
        core.meters,
        "meters diverged at step {step}"
    );
    assert_eq!(
        Capabilities::<Test>::get().into_inner(),
        core.capabilities,
        "capabilities diverged at step {step}"
    );
    assert_eq!(
        PhaseFlags::<Test>::get(),
        core.phase_flags.bits(),
        "phase flags diverged at step {step}"
    );
    assert_eq!(
        ReleaseChannel::<Test>::get(),
        core.release_channel,
        "release channel diverged at step {step}"
    );
}

#[test]
fn randomized_differential_covers_errors_origins_and_epochs() {
    // 600 deterministic pseudo-random operations mixing every call, valid and
    // invalid arguments, all nine authorities and shifting epochs; after every
    // step the shell's Result (exact mapped error) and full storage state must
    // equal the functional core's. Extends the happy-path differential per the
    // Codex adversarial-review finding.
    new_test_ext().execute_with(|| {
        let mut rng = XorShift(0x5EED_CAFE_F00D_D00D);
        let mut core = ConstitutionState::genesis();
        let keys: Vec<[u8; 16]> = genesis_params().iter().map(|r| r.key).collect();
        let origins: [(u64, ConstitutionOrigin); 10] = [
            (PARAM_ACC, ConstitutionOrigin::FutarchyParam),
            (TREASURY_ACC, ConstitutionOrigin::FutarchyTreasury),
            (CODE_ACC, ConstitutionOrigin::FutarchyCode),
            (META_ACC, ConstitutionOrigin::FutarchyMeta),
            (VALUES_ACC, ConstitutionOrigin::ConstitutionalValues),
            (CONSTITUTION_ACC, ConstitutionOrigin::ConstitutionTrack),
            (ENTRENCHED_ACC, ConstitutionOrigin::EntrenchedTrack),
            (GUARDIAN_ACC, ConstitutionOrigin::GuardianHold),
            (PLAYBOOK_ACC, ConstitutionOrigin::EmergencyPlaybook),
            (NOBODY_ACC, ConstitutionOrigin::Signed),
        ];
        let mut epoch: u32 = 0;
        // Shared meter fixture (genesis is meter-empty).
        seed_meters(&[6_000_000_000, 0]);
        core.meters = vec![crate::Meter::new(6_000_000_000, 0), crate::Meter::new(0, 0)];

        for step in 0..600u32 {
            // Epochs advance monotonically but in random strides (incl. 0).
            epoch += (rng.next() % 3) as u32;
            set_epoch(epoch);
            let (acc, authority) = origins[(rng.next() as usize) % origins.len()];
            let (runtime_origin, authority) = if rng.next() % 8 == 0 {
                (RuntimeOrigin::root(), ConstitutionOrigin::Root)
            } else {
                (RuntimeOrigin::signed(acc), authority)
            };

            match rng.next() % 6 {
                0 => {
                    // set_param: sometimes an unknown key, mixed value kinds.
                    let key = if rng.next() % 8 == 0 {
                        key16(b"no.such.key")
                    } else {
                        keys[(rng.next() as usize) % keys.len()]
                    };
                    let value = match rng.next() % 6 {
                        0 => ParamValue::U8((rng.next() % 20) as u8),
                        1 => ParamValue::U32((rng.next() % 700_000) as u32),
                        2 => ParamValue::Balance(rng.next() as u128 * 1_000),
                        3 => ParamValue::Fixed(futarchy_primitives::FixedU64(
                            rng.next() % 200_000_000,
                        )),
                        4 => ParamValue::Percent((rng.next() % 110) as u8),
                        _ => ParamValue::Perbill((rng.next() % 120_000_000) as u32),
                    };
                    let shell = Constitution::set_param(runtime_origin, key, value);
                    let model = core
                        .dispatch_set_param(authority, key, value, epoch, 1)
                        .map_err(crate::Pallet::<Test>::map_core_error);
                    assert_eq!(shell, model, "set_param result diverged at step {step}");
                }
                1 => {
                    // set_capability: 5 classes × 20 capability shapes → the
                    // 64-row bound is reachable and must bind identically.
                    let class = match rng.next() % 5 {
                        0 => ProposalClass::Param,
                        1 => ProposalClass::Treasury,
                        2 => ProposalClass::Code,
                        3 => ProposalClass::Meta,
                        _ => ProposalClass::Constitutional,
                    };
                    let capability = match rng.next() % 4 {
                        0 => Capability::SetParam(key16(&((rng.next() % 16) as u32).to_le_bytes())),
                        1 => Capability::AmendRegistry,
                        2 => Capability::TreasurySpend,
                        _ => Capability::OracleConfig,
                    };
                    let record = CapabilityRecord {
                        class,
                        capability,
                        enabled: rng.next() % 2 == 0,
                    };
                    let shell = Constitution::set_capability(runtime_origin, record);
                    let model = core
                        .dispatch_set_capability(authority, record)
                        .map_err(crate::Pallet::<Test>::map_core_error);
                    assert_eq!(shell, model, "set_capability diverged at step {step}");
                }
                2 => {
                    // set_phase_flag: armable, machinery and reserved bits.
                    let flag = 1u32 << (rng.next() % 10);
                    let enabled = rng.next() % 2 == 0;
                    let shell = Constitution::set_phase_flag(runtime_origin, flag, enabled);
                    let model = core
                        .dispatch_set_phase_flag(authority, flag, enabled)
                        .map_err(crate::Pallet::<Test>::map_core_error);
                    assert_eq!(shell, model, "set_phase_flag diverged at step {step}");
                }
                3 => {
                    // charge_meter (runtime-internal ↔ the core treasury path):
                    // valid and invalid indices, amounts up to overflow scale.
                    let index = (rng.next() % 4) as u32;
                    let amount = if rng.next() % 6 == 0 {
                        u128::MAX / 2 + rng.next() as u128
                    } else {
                        (rng.next() % 3_000_000_000) as u128
                    };
                    let shell = Constitution::charge_meter_internal(index, amount);
                    let model = core
                        .dispatch_charge_meter(
                            ConstitutionOrigin::FutarchyTreasury,
                            index as usize,
                            amount,
                            epoch,
                        )
                        .map_err(crate::Pallet::<Test>::map_core_error);
                    assert_eq!(shell, model, "charge_meter diverged at step {step}");
                }
                4 => {
                    // amend_registry: random bounds/Δ/cooldown incl. kernel-
                    // bounded rows, inverted bounds and over-cap cooldowns.
                    let key = if rng.next() % 8 == 0 {
                        key16(b"no.such.key")
                    } else {
                        keys[(rng.next() as usize) % keys.len()]
                    };
                    let kind = rng.next() % 6;
                    let mk = |x: u64| match kind {
                        0 => ParamValue::U8((x % 200) as u8),
                        1 => ParamValue::U32((x % 800_000) as u32),
                        2 => ParamValue::Balance((x as u128) * 1_000),
                        3 => ParamValue::Fixed(futarchy_primitives::FixedU64(x % 1_000_000_000)),
                        4 => ParamValue::Percent((x % 110) as u8),
                        _ => ParamValue::Perbill((x % 120_000_000) as u32),
                    };
                    let min = mk(rng.next() % 1_000);
                    let max = mk(rng.next());
                    let max_delta = match rng.next() % 3 {
                        0 => None,
                        1 => Some(crate::MaxDelta::Percent((rng.next() % 130) as u8)),
                        _ => Some(crate::MaxDelta::Factor((rng.next() % 4) as u8)),
                    };
                    let cooldown = (rng.next() % 12) as u32;
                    let shell = Constitution::amend_registry(
                        runtime_origin,
                        key,
                        min,
                        max,
                        max_delta,
                        cooldown,
                    );
                    let model = core
                        .dispatch_amend_registry(authority, key, min, max, max_delta, cooldown)
                        .map_err(crate::Pallet::<Test>::map_core_error);
                    assert_eq!(shell, model, "amend_registry diverged at step {step}");
                }
                _ => {
                    // set_release_channel: schema byte and flags word sweep.
                    let mut bytes = [0u8; RELEASE_CHANNEL_LEN];
                    bytes[0] = (rng.next() % 3) as u8; // 0 | 1 | 2
                    let flags = match rng.next() % 4 {
                        0 => 0u32,
                        1 => 5,
                        2 => ReleaseChannelValue::FLAGS_MASK,
                        _ => 1 << 9, // reserved
                    };
                    bytes[164..168].copy_from_slice(&flags.to_le_bytes());
                    bytes[112..116].copy_from_slice(&((rng.next() % 90) as u32).to_le_bytes());
                    let shell = Constitution::set_release_channel(runtime_origin, bytes);
                    let model = core
                        .dispatch_set_release_channel(
                            authority,
                            bytes,
                            System::block_number() as u32,
                        )
                        .map_err(crate::Pallet::<Test>::map_core_error);
                    assert_eq!(shell, model, "set_release_channel diverged at step {step}");
                }
            }
            assert_states_agree(&core, step);
        }
        assert_ok!(Constitution::do_try_state());
        core.try_state().unwrap();
    });
}

// ---------------------------------------------------------- amend_registry --

#[test]
fn amend_registry_updates_governance_metadata_within_meta_bounds() {
    new_test_ext().execute_with(|| {
        // mkt.fee is non-kernel-bounded: both amendment origins may retune
        // its bounds/Δ/cooldown; value, class and key stay fixed.
        let before = Params::<Test>::get(key16(b"mkt.fee")).unwrap();
        assert_ok!(Constitution::amend_registry(
            RuntimeOrigin::signed(META_ACC),
            key16(b"mkt.fee"),
            ParamValue::Perbill(1_000_000),
            ParamValue::Perbill(8_000_000),
            Some(crate::MaxDelta::Absolute(ParamValue::Perbill(2_000_000))),
            2,
        ));
        assert_eq!(
            last_event(),
            RuntimeEvent::Constitution(Event::RegistryAmended {
                key: key16(b"mkt.fee"),
            })
        );
        let after = Params::<Test>::get(key16(b"mkt.fee")).unwrap();
        assert_eq!(after.min, ParamValue::Perbill(1_000_000));
        assert_eq!(after.max, ParamValue::Perbill(8_000_000));
        assert_eq!(after.value, before.value);
        assert_eq!(after.class, before.class);
        assert_eq!(after.last_changed_epoch, before.last_changed_epoch);
        // SQ-150 (ruled 2026-07-21): `ConstitutionalValues` may NO LONGER amend —
        // amend_registry is FutarchyMeta-only. The former CV/track authority is
        // removed; the dedicated `sq_150_*` cases pin the full policy.
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::signed(VALUES_ACC),
                key16(b"mkt.fee"),
                ParamValue::Perbill(500_000),
                ParamValue::Perbill(10_000_000),
                None,
                1,
            ),
            DispatchError::BadOrigin
        );
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn amend_registry_origin_misuse_is_refused() {
    new_test_ext().execute_with(|| {
        for refused in [
            PARAM_ACC,
            TREASURY_ACC,
            CODE_ACC,
            GUARDIAN_ACC,
            PLAYBOOK_ACC,
            NOBODY_ACC,
        ] {
            assert_noop!(
                Constitution::amend_registry(
                    RuntimeOrigin::signed(refused),
                    key16(b"mkt.fee"),
                    ParamValue::Perbill(500_000),
                    ParamValue::Perbill(10_000_000),
                    None,
                    1,
                ),
                DispatchError::BadOrigin
            );
        }
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::root(),
                key16(b"mkt.fee"),
                ParamValue::Perbill(500_000),
                ParamValue::Perbill(10_000_000),
                None,
                1,
            ),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::none(),
                key16(b"mkt.fee"),
                ParamValue::Perbill(500_000),
                ParamValue::Perbill(10_000_000),
                None,
                1,
            ),
            DispatchError::BadOrigin
        );
    });
}

#[test]
fn amend_registry_error_paths_are_exact() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::signed(META_ACC),
                key16(b"no.such.key"),
                ParamValue::U32(1),
                ParamValue::U32(2),
                None,
                1,
            ),
            Error::<Test>::UnknownParam
        );
        // Kernel-bounded rows: bounds are genesis-fixed (13 rule 7)…
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::signed(META_ACC),
                key16(b"gate.p_max"),
                ParamValue::Fixed(futarchy_primitives::FixedU64(0)),
                ParamValue::Fixed(futarchy_primitives::FixedU64(200_000_000)),
                None,
                4,
            ),
            Error::<Test>::KernelBoundImmutable
        );
        // …and 13 rule 2 freezes the WHOLE tuple: even restating the bounds
        // verbatim while touching Δ/cooldown is refused.
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::signed(META_ACC),
                key16(b"gate.p_max"),
                ParamValue::Fixed(futarchy_primitives::FixedU64(0)),
                ParamValue::Fixed(futarchy_primitives::FixedU64(
                    crate::kernel::GATE_P_MAX_CEILING_1E9
                )),
                Some(crate::MaxDelta::Absolute(ParamValue::Fixed(
                    futarchy_primitives::FixedU64(5_000_000)
                ))),
                4,
            ),
            Error::<Test>::KernelBoundImmutable
        );
        // Kind mismatch, inverted bounds, value stranded outside, over-cap
        // cooldown, degenerate deltas.
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::signed(META_ACC),
                key16(b"mkt.fee"),
                ParamValue::U32(1),
                ParamValue::U32(2),
                None,
                1,
            ),
            Error::<Test>::WrongType
        );
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::signed(META_ACC),
                key16(b"mkt.fee"),
                ParamValue::Perbill(9_000_000),
                ParamValue::Perbill(1_000_000),
                None,
                1,
            ),
            Error::<Test>::MetaBoundViolation
        );
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::signed(META_ACC),
                key16(b"mkt.fee"),
                ParamValue::Perbill(5_000_000),
                ParamValue::Perbill(9_000_000),
                None,
                1,
            ),
            Error::<Test>::MetaBoundViolation // value 3_000_000 < new min
        );
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::signed(META_ACC),
                key16(b"mkt.fee"),
                ParamValue::Perbill(500_000),
                ParamValue::Perbill(10_000_000),
                None,
                9, // > META_MAX_COOLDOWN_EPOCHS = 8
            ),
            Error::<Test>::MetaBoundViolation
        );
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::signed(META_ACC),
                key16(b"mkt.fee"),
                ParamValue::Perbill(500_000),
                ParamValue::Perbill(10_000_000),
                Some(crate::MaxDelta::Factor(0)),
                1,
            ),
            Error::<Test>::MetaBoundViolation
        );
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn amend_registry_cannot_unlock_a_value_change_beyond_kernel_bounds() {
    new_test_ext().execute_with(|| {
        // Adversarial sequence (R-7): a kernel-bounded ceiling can never be
        // raised to admit a larger value — even by META, the sole origin that
        // clears the SQ-150 FutarchyMeta-only origin gate, the amendment is
        // refused with `KernelBoundImmutable`.
        set_epoch(4);
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::signed(META_ACC),
                key16(b"gate.p_max"),
                ParamValue::Fixed(futarchy_primitives::FixedU64(0)),
                ParamValue::Fixed(futarchy_primitives::FixedU64(500_000_000)), // 0.5!
                None,
                4,
            ),
            Error::<Test>::KernelBoundImmutable
        );
        assert_noop!(
            Constitution::set_param(
                RuntimeOrigin::signed(META_ACC),
                key16(b"gate.p_max"),
                ParamValue::Fixed(futarchy_primitives::FixedU64(150_000_000)), // > 0.10 K
            ),
            Error::<Test>::AboveMax
        );
    });
}

// -------------------------------- 08 §4.2 arming NAV gate (SQ-180) ---------
//
// "Arming a proposal class REQUIRES published spendable NAV ≥ the class floor
// of 08 §4.1", and under the 08 §1.2 reserve-health flag spendable NAV is 0 so
// every class fails (fail-static). The refusal must leave `PhaseFlags` intact.

#[test]
fn arming_below_the_class_nav_floor_is_refused_with_flags_unchanged() {
    new_test_ext().execute_with(|| {
        UnarmableClasses::set(vec![ProposalClass::Param]);
        assert_noop!(
            Constitution::set_phase_flag(RuntimeOrigin::root(), PhaseFlagsValue::PARAM_ARMED, true),
            Error::<Test>::NavFloorUnmet
        );
        assert_eq!(PhaseFlags::<Test>::get(), 0);
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn arming_at_or_above_the_floor_succeeds_and_consults_the_gate() {
    new_test_ext().execute_with(|| {
        assert_ok!(Constitution::set_phase_flag(
            RuntimeOrigin::root(),
            PhaseFlagsValue::TREASURY_ARMED,
            true
        ));
        assert_eq!(PhaseFlags::<Test>::get(), PhaseFlagsValue::TREASURY_ARMED);
        assert_eq!(ArmingGateCalls::get(), vec![ProposalClass::Treasury]);
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn code_meta_bit_is_gated_on_both_classes_so_the_higher_floor_binds() {
    new_test_ext().execute_with(|| {
        // Bit 3 arms CODE *and* META (08 §4.1 floors 13,862,944 / 21,256,533),
        // so a NAV that clears CODE but not META must still refuse.
        UnarmableClasses::set(vec![ProposalClass::Meta]);
        assert_noop!(
            Constitution::set_phase_flag(
                RuntimeOrigin::root(),
                PhaseFlagsValue::CODE_META_ARMED,
                true
            ),
            Error::<Test>::NavFloorUnmet
        );
        assert_eq!(PhaseFlags::<Test>::get(), 0);
        assert_eq!(
            ArmingGateCalls::get(),
            vec![ProposalClass::Code, ProposalClass::Meta]
        );
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn a_multi_bit_arming_write_is_refused_whole_when_any_class_is_below_floor() {
    new_test_ext().execute_with(|| {
        // G-1: partial arming would be worse than none — the write is atomic.
        UnarmableClasses::set(vec![ProposalClass::Treasury]);
        assert_noop!(
            Constitution::set_phase_flag(
                RuntimeOrigin::root(),
                PhaseFlagsValue::PARAM_ARMED | PhaseFlagsValue::TREASURY_ARMED,
                true
            ),
            Error::<Test>::NavFloorUnmet
        );
        assert_eq!(PhaseFlags::<Test>::get(), 0);
        assert_ok!(Constitution::do_try_state());
    });
}

// ------------------------------- A13 · constitution/params cluster (batch X) --

#[test]
fn sq_485_reserve_probe_controls_have_immutable_nonzero_floors() {
    new_test_ext().execute_with(|| {
        for (name, minimum, origin) in [
            (b"res.probe_int".as_slice(), ParamValue::U32(1), PARAM_ACC),
            (b"res.probe_to".as_slice(), ParamValue::U32(1), PARAM_ACC),
            (
                b"res.probe_amount".as_slice(),
                ParamValue::Balance(1),
                PARAM_ACC,
            ),
            (b"res.fail_thr".as_slice(), ParamValue::U8(1), META_ACC),
            (b"res.recover_thr".as_slice(), ParamValue::U8(1), META_ACC),
        ] {
            let key = key16(name);
            let before = Params::<Test>::get(key).unwrap();
            assert_eq!(before.min, minimum);
            assert!(before.kernel_bounded);
            let zero = match minimum {
                ParamValue::U32(_) => ParamValue::U32(0),
                ParamValue::U8(_) => ParamValue::U8(0),
                ParamValue::Balance(_) => ParamValue::Balance(0),
                _ => unreachable!(),
            };
            assert_noop!(
                Constitution::set_param(RuntimeOrigin::signed(origin), key, zero),
                Error::<Test>::BelowMin
            );
            assert_eq!(Params::<Test>::get(key), Some(before));

            set_epoch(before.cooldown_epochs);
            assert_ok!(Constitution::set_param(
                RuntimeOrigin::signed(origin),
                key,
                minimum
            ));
            assert_eq!(Params::<Test>::get(key).unwrap().value, minimum);

            assert_noop!(
                Constitution::amend_registry(
                    RuntimeOrigin::signed(META_ACC),
                    key,
                    zero,
                    before.max,
                    before.max_delta,
                    before.cooldown_epochs,
                ),
                Error::<Test>::KernelBoundImmutable
            );
        }
        assert_ok!(Constitution::do_try_state());
    });
}

/// SQ-36 (ruled 2026-07-21): `ledger.pos_dep` is frozen — the registry maximum
/// equals its minimum and default, so no governance path can raise the deposit
/// unit while entries created at the old unit are still live. The ledger
/// charges, refunds and reconciles `DepositsHeld` at the *live* unit
/// (`pallets/conditional-ledger/src/lib.rs` `settle_deposits` / `reap_one` /
/// L-6), and has no 03 §10 hook to rebase held deposits; a raise would
/// over-refund old entries out of pooled collateral. 13 §1 states the freeze.
#[test]
fn sq_36_ledger_position_deposit_is_frozen_at_its_kernel_floor() {
    new_test_ext().execute_with(|| {
        let record = Params::<Test>::get(key16(b"ledger.pos_dep")).unwrap();
        assert_eq!(
            record.min, record.max,
            "13 §1: ledger.pos_dep max must equal its K floor (frozen key)"
        );
        assert_eq!(
            record.value, record.min,
            "13 §1: ledger.pos_dep default must equal its frozen bound"
        );
        assert!(record.kernel_bounded);
        // Every raise is refused, at the smallest possible step.
        let raise = ParamValue::Balance(record.max.as_u128().saturating_add(1));
        assert_noop!(
            Constitution::set_param(
                RuntimeOrigin::signed(META_ACC),
                key16(b"ledger.pos_dep"),
                raise
            ),
            Error::<Test>::AboveMax
        );
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn disarming_is_never_gated_even_below_the_floor() {
    new_test_ext().execute_with(|| {
        assert_ok!(Constitution::set_phase_flag(
            RuntimeOrigin::root(),
            PhaseFlagsValue::PARAM_ARMED,
            true
        ));
        // NAV then collapses (e.g. the reserve-health haircut lands).
        UnarmableClasses::set(vec![
            ProposalClass::Param,
            ProposalClass::Treasury,
            ProposalClass::Code,
            ProposalClass::Meta,
        ]);
        ArmingGateCalls::set(Vec::new());
        // Clearing a bit only removes capability; gating it would strand the
        // chain armed below its own floor.
        assert_ok!(Constitution::set_phase_flag(
            RuntimeOrigin::root(),
            PhaseFlagsValue::PARAM_ARMED,
            false
        ));
        assert_eq!(PhaseFlags::<Test>::get(), 0);
        assert!(ArmingGateCalls::get().is_empty());
        assert_ok!(Constitution::do_try_state());
    });
}

/// SQ-117 (ruled 2026-07-21): `keeper.rebate` is seeded at genesis from the
/// 08 §6.2 crank-fee basis, so B9's rebate pipeline stops paying zero. The
/// basis itself stays `[VERIFY]`-tagged in 13 §1 until launch benchmarking
/// fixes `fee.vit_usdc_rate`; only the seeding mechanism is pinned here.
#[test]
fn sq_117_keeper_rebate_is_seeded_within_its_13_bounds() {
    new_test_ext().execute_with(|| {
        let record = Params::<Test>::get(key16(b"keeper.rebate"))
            .expect("13 §1 keeper.rebate must be genesis-seeded");
        assert_eq!(record.class, ParamClass::Param);
        assert!(!record.kernel_bounded);
        assert_eq!(record.cooldown_epochs, 1);
        assert_eq!(record.max_delta, None);
        // 13 §1: hard min 1x and hard max 10x the same crank-fee basis the
        // default is 3x of, so the whole row scales with one number.
        let value = record.value.as_u128();
        let min = record.min.as_u128();
        assert!(value > 0, "a zero seed leaves the A-1 incentive inert");
        assert_eq!(value, min.saturating_mul(3));
        assert_eq!(record.max.as_u128(), min.saturating_mul(10));
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn non_class_arming_bits_carry_no_nav_floor() {
    new_test_ext().execute_with(|| {
        // Bits 0 (shadow) and 4 (sudo-present) admit no proposal class, so they
        // are armable regardless of NAV.
        UnarmableClasses::set(vec![
            ProposalClass::Param,
            ProposalClass::Treasury,
            ProposalClass::Code,
            ProposalClass::Meta,
        ]);
        assert_ok!(Constitution::set_phase_flag(
            RuntimeOrigin::root(),
            PhaseFlagsValue::SHADOW_MODE | PhaseFlagsValue::SUDO_PRESENT,
            true
        ));
        assert!(ArmingGateCalls::get().is_empty());
        assert_ok!(Constitution::do_try_state());
    });
}

/// SQ-158 (owner A13): `dis.merit_min` is a distinct 13 §1 key precisely so the
/// merit floor can be raised independently of `B_1` (07 §12 *Merit floor*).
/// Seeding it restores that lever; the consumer composes `max(live key, frozen
/// B_1)` so a lowering can never make censorship cheaper than the game's own
/// round-1 bond (R-7).
#[test]
fn sq_158_dis_merit_min_is_seeded_as_an_independent_meta_lever() {
    new_test_ext().execute_with(|| {
        let record = Params::<Test>::get(key16(b"dis.merit_min"))
            .expect("13 §1 dis.merit_min must be genesis-seeded");
        let floor = Params::<Test>::get(key16(b"orc.bond_floor")).unwrap();
        assert_eq!(record.class, ParamClass::Meta);
        assert!(!record.kernel_bounded);
        assert_eq!(record.cooldown_epochs, 2);
        assert_eq!(record.max_delta, Some(crate::MaxDelta::Factor(2)));
        // 13 §1: floor `orc.bond_floor`, no ceiling.
        assert_eq!(record.min, floor.value);
        assert_eq!(record.value, floor.value);
        assert_eq!(record.max, ParamValue::Balance(u128::MAX));
        // The lever really moves: a META raise inside the factor-2 step lands.
        set_epoch(record.cooldown_epochs);
        assert_ok!(Constitution::set_param(
            RuntimeOrigin::signed(META_ACC),
            key16(b"dis.merit_min"),
            ParamValue::Balance(record.value.as_u128().saturating_mul(2)),
        ));
        assert_ok!(Constitution::do_try_state());
    });
}

#[test]
fn arming_gate_runs_after_the_origin_check_so_misuse_still_reports_bad_origin() {
    new_test_ext().execute_with(|| {
        UnarmableClasses::set(vec![ProposalClass::Param]);
        // A non-Root origin must fail on authority, never leak the NAV state.
        assert_noop!(
            Constitution::set_phase_flag(
                RuntimeOrigin::signed(PARAM_ACC),
                PhaseFlagsValue::PARAM_ARMED,
                true
            ),
            DispatchError::BadOrigin
        );
        assert!(ArmingGateCalls::get().is_empty());
        assert_eq!(PhaseFlags::<Test>::get(), 0);
        assert_ok!(Constitution::do_try_state());
    });
}

/// SQ-150 (ruled 2026-07-21): non-kernel registry rows are META-only.
/// Positive leg — `FutarchyMeta` amends a non-kernel row of any class.
#[test]
fn sq_150_futarchy_meta_amends_every_non_kernel_registry_class() {
    new_test_ext().execute_with(|| {
        let mut seen: Vec<ParamClass> = Vec::new();
        for record in genesis_params() {
            if record.kernel_bounded || seen.contains(&record.class) {
                continue;
            }
            seen.push(record.class);
            assert_ok!(Constitution::amend_registry(
                RuntimeOrigin::signed(META_ACC),
                record.key,
                record.min,
                record.max,
                record.max_delta,
                record.cooldown_epochs,
            ));
        }
        assert!(
            seen.len() >= 3,
            "genesis must exercise several non-kernel classes, saw {seen:?}"
        );
        assert_ok!(Constitution::do_try_state());
    });
}

/// SQ-150 negative leg (values): `ConstitutionalValues` — and every other
/// non-META origin — is refused on a non-kernel row. This is the I-8 crossing
/// the S5 suite pinned: one call was reachable from both the values and the
/// belief scope.
#[test]
fn sq_150_no_values_origin_may_amend_a_non_kernel_registry_row() {
    new_test_ext().execute_with(|| {
        let record = genesis_params()
            .into_iter()
            .find(|record| !record.kernel_bounded)
            .expect("genesis must contain a non-kernel registry row");
        for refused in [
            VALUES_ACC,
            CONSTITUTION_ACC,
            ENTRENCHED_ACC,
            PARAM_ACC,
            TREASURY_ACC,
            CODE_ACC,
            GUARDIAN_ACC,
            PLAYBOOK_ACC,
            NOBODY_ACC,
        ] {
            assert_noop!(
                Constitution::amend_registry(
                    RuntimeOrigin::signed(refused),
                    record.key,
                    record.min,
                    record.max,
                    record.max_delta,
                    record.cooldown_epochs,
                ),
                DispatchError::BadOrigin
            );
        }
        assert_ok!(Constitution::do_try_state());
    });
}

/// SQ-150 negative leg (kernel): kernel-bounded rows are immutable. The one
/// origin that clears the FutarchyMeta-only origin gate (META) is refused by
/// `checked_amend` with the reason-naming `KernelBoundImmutable` (13 rule 7,
/// not a bare `BadOrigin`); every other origin — values, tracks, class origins,
/// Root, nobody — is refused at the origin gate with `BadOrigin`. Either way
/// no origin can move a kernel floor/ceiling.
#[test]
fn sq_150_no_origin_may_amend_a_kernel_bounded_registry_row() {
    new_test_ext().execute_with(|| {
        let record = genesis_params()
            .into_iter()
            .find(|record| record.kernel_bounded)
            .expect("genesis must contain a kernel-bounded registry row");
        // META clears the origin gate, then hits the kernel-immutability wall.
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::signed(META_ACC),
                record.key,
                record.min,
                record.max,
                record.max_delta,
                record.cooldown_epochs,
            ),
            Error::<Test>::KernelBoundImmutable
        );
        // Every other origin is stopped at the origin gate.
        for refused in [
            RuntimeOrigin::signed(VALUES_ACC),
            RuntimeOrigin::signed(CONSTITUTION_ACC),
            RuntimeOrigin::signed(ENTRENCHED_ACC),
            RuntimeOrigin::signed(PARAM_ACC),
            RuntimeOrigin::signed(TREASURY_ACC),
            RuntimeOrigin::signed(CODE_ACC),
            RuntimeOrigin::signed(GUARDIAN_ACC),
            RuntimeOrigin::signed(PLAYBOOK_ACC),
            RuntimeOrigin::signed(NOBODY_ACC),
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
        ] {
            assert_noop!(
                Constitution::amend_registry(
                    refused,
                    record.key,
                    record.min,
                    record.max,
                    record.max_delta,
                    record.cooldown_epochs,
                ),
                DispatchError::BadOrigin
            );
        }
        assert_ok!(Constitution::do_try_state());
    });
}
