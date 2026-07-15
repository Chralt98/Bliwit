//! `frame-benchmarking` v2 benchmarks for every extrinsic (Track-A DoD,
//! 15 §4.5). The constitution has no weight-bearing hooks — the spec gives it
//! no cranks, and `try_state` is try-runtime-only — so the call set below is
//! the complete benchmark surface. B5 turns the generated output into the
//! PoV-calibrated `weights.rs`.
//!
//! Each benchmark drives its call with the exact 06 §3.2 authority via
//! [`crate::BenchmarkHelper`], seeding worst-case storage first.

use super::*;
use crate::pallet::{Capabilities, Params};

use alloc::vec::Vec;
use frame_benchmarking::v2::*;
use frame_support::pallet_prelude::*;
use futarchy_primitives::ProposalClass;

fn authority_origin<T: Config>(authority: ConstitutionOrigin) -> T::RuntimeOrigin {
    T::BenchmarkHelper::origin(authority)
}

#[benchmarks]
mod benches {
    use super::*;

    #[benchmark]
    fn set_param() {
        let key = key16(b"bench.param");
        let record = ParamRecord {
            key,
            value: ParamValue::U32(10),
            min: ParamValue::U32(0),
            max: ParamValue::U32(1_000_000),
            max_delta: None,
            cooldown_epochs: 0,
            last_changed_epoch: 0,
            class: ParamClass::Param,
            kernel_bounded: false,
        };
        Params::<T>::insert(key, record);
        let origin = authority_origin::<T>(ConstitutionOrigin::FutarchyParam);

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, key, ParamValue::U32(12));

        assert_eq!(
            Params::<T>::get(key).map(|r| r.value),
            Some(ParamValue::U32(12))
        );
    }

    #[benchmark]
    fn set_capability() {
        // Worst case: a full-table scan and a push at the last free slot.
        let mut table: Vec<CapabilityRecord> = Vec::new();
        for i in 0..(MAX_CAPABILITIES_BOUND - 1) {
            table.push(CapabilityRecord {
                class: ProposalClass::Param,
                capability: Capability::SetParam(key16(&i.to_le_bytes())),
                enabled: true,
            });
        }
        Capabilities::<T>::put(BoundedVec::<
            CapabilityRecord,
            ConstU32<MAX_CAPABILITIES_BOUND>,
        >::truncate_from(table));
        let record = CapabilityRecord {
            class: ProposalClass::Meta,
            capability: Capability::AmendRegistry,
            enabled: true,
        };
        let origin = authority_origin::<T>(ConstitutionOrigin::FutarchyMeta);

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, record);

        assert!(Pallet::<T>::capability_enabled(
            ProposalClass::Meta,
            Capability::AmendRegistry
        ));
    }

    #[benchmark]
    fn set_phase_flag() {
        // 09 §5.4: bootstrap Root is the only origin-mediated flag writer.
        let origin = authority_origin::<T>(ConstitutionOrigin::Root);

        #[extrinsic_call]
        _(
            origin as T::RuntimeOrigin,
            PhaseFlagsValue::SUDO_PRESENT,
            true,
        );

        assert_eq!(
            Pallet::<T>::phase_flags() & PhaseFlagsValue::SUDO_PRESENT,
            PhaseFlagsValue::SUDO_PRESENT
        );
    }

    #[benchmark]
    fn set_release_channel() {
        let mut bytes = [0u8; RELEASE_CHANNEL_LEN];
        bytes[0] = 1;
        bytes[108..112].copy_from_slice(&9u32.to_le_bytes());
        bytes[112..116].copy_from_slice(&3u32.to_le_bytes());
        let origin = authority_origin::<T>(ConstitutionOrigin::ConstitutionalValues);

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, bytes);

        assert_eq!(crate::pallet::ReleaseChannel::<T>::get().spec_version(), 3);
    }

    #[benchmark]
    fn amend_registry() {
        // mkt.fee is a non-kernel-bounded genesis row; widen its Δ within
        // the meta-bounds.
        let key = key16(b"mkt.fee");
        let origin = authority_origin::<T>(ConstitutionOrigin::FutarchyMeta);

        #[extrinsic_call]
        _(
            origin as T::RuntimeOrigin,
            key,
            ParamValue::Perbill(500_000),
            ParamValue::Perbill(10_000_000),
            Some(MaxDelta::Absolute(ParamValue::Perbill(2_000_000))),
            2u32,
        );

        assert_eq!(
            Params::<T>::get(key).and_then(|r| r.max_delta),
            Some(MaxDelta::Absolute(ParamValue::Perbill(2_000_000)))
        );
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
