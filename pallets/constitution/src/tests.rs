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
use frame_support::{assert_noop, assert_ok};
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
        assert_ok!(Constitution::set_release_channel(
            RuntimeOrigin::signed(VALUES_ACC),
            channel_bytes()
        ));
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
    assert_eq!(CONTRACT_VERSION, 3); // v3: oracle per-version triple key (A5; 02 §7.2/§13)
    assert_eq!(MAX_PARAMS, 128); // 13 §4 registry bound
    assert_eq!(MAX_CAPABILITIES, 64);
    assert_eq!(crate::MAX_METERS, 16);
    assert_eq!(crate::kernel::DESCRIPTOR_LEAD_TIME_BLOCKS, 43_200);
}

// -------------------------------------------------------------- set_param --

#[test]
fn set_param_updates_in_bounds_key_and_emits() {
    new_test_ext().execute_with(|| {
        set_epoch(1); // cooldown 1 elapsed
        assert_ok!(Constitution::set_param(
            RuntimeOrigin::signed(PARAM_ACC),
            key16(OBS_KEY),
            ParamValue::U32(12)
        ));
        let record = Params::<Test>::get(key16(OBS_KEY)).unwrap();
        assert_eq!(record.value, ParamValue::U32(12));
        assert_eq!(record.last_changed_epoch, 1);
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
        assert_ok!(Constitution::set_release_channel(
            RuntimeOrigin::signed(VALUES_ACC),
            channel_bytes()
        ));
        assert_eq!(ReleaseChannel::<Test>::get().spec_version(), 7);
        assert_eq!(
            last_event(),
            RuntimeEvent::Constitution(Event::ReleaseChannelSet {
                spec_version: 7,
                updated_at: 42,
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
        // [VERIFY] tag is seeded (87 total, incl. per-class suffix keys and
        // rule-6 short keys); spot-pin the unit encodings per kind.
        assert_eq!(Params::<Test>::count(), 87);

        // Per-class suffix keys (13 rule 6) — δ floors, kernel-capped.
        let delta_meta = Params::<Test>::get(key16(b"dec.delta.meta")).unwrap();
        assert_eq!(
            delta_meta.value,
            ParamValue::Fixed(futarchy_primitives::FixedU64(60_000_000))
        );
        assert!(delta_meta.kernel_bounded);

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
        assert_eq!(window.min, ParamValue::U32(14_400));
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

fn governance_origin_for(class: ParamClass) -> RuntimeOrigin {
    let account = match class {
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
                        governance_origin_for(record.class),
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
                        governance_origin_for(record.class),
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
                        governance_origin_for(record.class),
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
                    governance_origin_for(record.class),
                    record.key,
                    record.value
                ));
                assert_noop!(
                    Constitution::set_param(
                        governance_origin_for(record.class),
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
        let origins: [(u64, ConstitutionOrigin); 8] = [
            (PARAM_ACC, ConstitutionOrigin::FutarchyParam),
            (TREASURY_ACC, ConstitutionOrigin::FutarchyTreasury),
            (CODE_ACC, ConstitutionOrigin::FutarchyCode),
            (META_ACC, ConstitutionOrigin::FutarchyMeta),
            (VALUES_ACC, ConstitutionOrigin::ConstitutionalValues),
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
            let (acc, authority) = origins[(rng.next() % 8) as usize];
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
                        .dispatch_set_param(authority, key, value, epoch)
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
                        .dispatch_set_release_channel(authority, bytes)
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
        // ConstitutionalValues (constitution track) may amend too.
        assert_ok!(Constitution::amend_registry(
            RuntimeOrigin::signed(VALUES_ACC),
            key16(b"mkt.fee"),
            ParamValue::Perbill(500_000),
            ParamValue::Perbill(10_000_000),
            None,
            1,
        ));
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
        // Adversarial sequence (R-7): even after a values-side amendment of a
        // NON-kernel row, set_param stays inside the amended bounds; and a
        // kernel-bounded ceiling can never be raised to admit a larger value.
        set_epoch(4);
        assert_noop!(
            Constitution::amend_registry(
                RuntimeOrigin::signed(VALUES_ACC),
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
