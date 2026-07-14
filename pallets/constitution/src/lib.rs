#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::{Balance, BlockNumber, FixedU64, ParamKey, ProposalClass};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub use futarchy_primitives::kernel;
pub use futarchy_primitives::INTEGRATION_CONTRACT_VERSION as CONTRACT_VERSION;

/// `twox128("Constitution") ++ twox128("ReleaseChannel")`.
pub const RELEASE_CHANNEL_STORAGE_KEY: [u8; 32] = [
    0xfb, 0x8c, 0xcb, 0xf6, 0x77, 0xa3, 0xd2, 0xce, 0x27, 0xab, 0x85, 0x16, 0x5f, 0x32, 0xdf, 0x6a,
    0xfe, 0xc7, 0x19, 0x4a, 0x53, 0x68, 0xa5, 0x8e, 0x1f, 0x6b, 0xf5, 0x74, 0x57, 0x13, 0x4a, 0x6c,
];
pub const RELEASE_CHANNEL_LEN: usize = 168;
pub const MAX_PARAMS: usize = 64;
pub const MAX_CAPABILITIES: usize = 64;
pub const MAX_METERS: usize = 16;

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum ParamValue {
    U8(u8),
    U32(u32),
    Balance(Balance),
    Fixed(FixedU64),
    Percent(u8),
    Perbill(u32),
}

impl ParamValue {
    pub const fn as_u128(self) -> u128 {
        match self {
            Self::U8(v) => v as u128,
            Self::U32(v) => v as u128,
            Self::Balance(v) => v,
            Self::Fixed(v) => v.0 as u128,
            Self::Percent(v) => v as u128,
            Self::Perbill(v) => v as u128,
        }
    }

    pub const fn same_kind(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::U8(_), Self::U8(_))
                | (Self::U32(_), Self::U32(_))
                | (Self::Balance(_), Self::Balance(_))
                | (Self::Fixed(_), Self::Fixed(_))
                | (Self::Percent(_), Self::Percent(_))
                | (Self::Perbill(_), Self::Perbill(_))
        )
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum ParamClass {
    Param,
    Treasury,
    Meta,
    Const,
    Entrenched,
    MetaAndValues,
}

/// Per-decision rate limit for a constitution key, mirroring the three
/// Max Δ/decision semantics of the 13 §1 table: absolute steps in the key's
/// own unit (e.g. `2`, `5`), steps relative to the current value (e.g. `10%`),
/// and multiplicative bounds (e.g. `×2`).
#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum MaxDelta {
    /// Absolute bound in the parameter's own unit.
    Absolute(ParamValue),
    /// Bound relative to the current value, in percent of it.
    Percent(u8),
    /// Multiplicative bound: `next ∈ [value / factor, value × factor]`.
    Factor(u8),
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct ParamRecord {
    pub key: ParamKey,
    pub value: ParamValue,
    pub min: ParamValue,
    pub max: ParamValue,
    pub max_delta: Option<MaxDelta>,
    pub cooldown_epochs: u32,
    pub last_changed_epoch: u32,
    pub class: ParamClass,
}

impl ParamRecord {
    pub fn checked_update(&self, next: ParamValue, epoch: u32) -> Result<Self, Error> {
        ensure!(self.value.same_kind(next), Error::WrongType);
        ensure!(
            self.min.same_kind(next) && self.max.same_kind(next),
            Error::WrongType
        );
        ensure!(next.as_u128() >= self.min.as_u128(), Error::BelowMin);
        ensure!(next.as_u128() <= self.max.as_u128(), Error::AboveMax);
        ensure!(
            epoch >= self.last_changed_epoch.saturating_add(self.cooldown_epochs),
            Error::CooldownActive
        );
        match self.max_delta {
            None => {}
            Some(MaxDelta::Absolute(bound)) => {
                ensure!(bound.same_kind(next), Error::WrongType);
                let delta = self.value.as_u128().abs_diff(next.as_u128());
                ensure!(delta <= bound.as_u128(), Error::DeltaTooLarge);
            }
            Some(MaxDelta::Percent(percent)) => {
                // Allowance is recomputed from the current value on every
                // decision; flooring keeps the limit conservative.
                let allowed = self
                    .value
                    .as_u128()
                    .saturating_mul(u128::from(percent))
                    .checked_div(100)
                    .unwrap_or(0);
                let delta = self.value.as_u128().abs_diff(next.as_u128());
                ensure!(delta <= allowed, Error::DeltaTooLarge);
            }
            Some(MaxDelta::Factor(factor)) => {
                let factor = u128::from(factor);
                let value = self.value.as_u128();
                let next_raw = next.as_u128();
                ensure!(
                    next_raw <= value.saturating_mul(factor)
                        && next_raw.saturating_mul(factor) >= value,
                    Error::DeltaTooLarge
                );
            }
        }
        Ok(Self {
            value: next,
            last_changed_epoch: epoch,
            ..*self
        })
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct Meter {
    pub limit: u128,
    pub spent: u128,
    pub reset_epoch: u32,
}

impl Meter {
    pub const fn new(limit: u128, reset_epoch: u32) -> Self {
        Self {
            limit,
            spent: 0,
            reset_epoch,
        }
    }

    pub fn charge(&mut self, amount: u128, epoch: u32) -> Result<(), Error> {
        if epoch > self.reset_epoch {
            self.spent = 0;
            self.reset_epoch = epoch;
        }
        let next = self.spent.checked_add(amount).ok_or(Error::MeterOverflow)?;
        ensure!(next <= self.limit, Error::MeterExhausted);
        self.spent = next;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Capability {
    SetParam(ParamKey),
    SetCapability,
    AmendRegistry,
    SetReleaseChannel,
    AuthorizeUpgrade,
    TreasurySpend,
    OracleConfig,
    MarketTemplate,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct CapabilityRecord {
    pub class: ProposalClass,
    pub capability: Capability,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct PhaseFlags(u32);

impl PhaseFlags {
    pub const SHADOW_MODE: u32 = 1 << 0;
    pub const PARAM_ARMED: u32 = 1 << 1;
    pub const TREASURY_ARMED: u32 = 1 << 2;
    pub const CODE_META_ARMED: u32 = 1 << 3;
    pub const SUDO_PRESENT: u32 = 1 << 4;
    pub const LEDGER_FROZEN: u32 = 1 << 5;
    pub const DEAD_MAN_ENGAGED: u32 = 1 << 6;
    pub const RESERVE_HEALTH_FLAG: u32 = 1 << 7;
    pub const RESERVED_MASK: u32 = !0xff;

    pub const fn empty() -> Self {
        Self(0)
    }
    pub const fn from_bits(bits: u32) -> Result<Self, Error> {
        if bits & Self::RESERVED_MASK == 0 {
            Ok(Self(bits))
        } else {
            Err(Error::ReservedPhaseFlag)
        }
    }
    pub const fn bits(self) -> u32 {
        self.0
    }
    pub fn contains(self, flag: u32) -> bool {
        self.0 & flag == flag
    }
    pub fn set(&mut self, flag: u32, enabled: bool) -> Result<(), Error> {
        ensure!(flag & Self::RESERVED_MASK == 0, Error::ReservedPhaseFlag);
        if enabled {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct ReleaseChannel {
    pub bytes: [u8; RELEASE_CHANNEL_LEN],
}

impl ReleaseChannel {
    pub fn new(bytes: [u8; RELEASE_CHANNEL_LEN]) -> Result<Self, Error> {
        ensure!(bytes[0] == 1, Error::BadReleaseSchema);
        Ok(Self { bytes })
    }
    pub fn updated_at(&self) -> BlockNumber {
        le_u32_at(&self.bytes, 108)
    }
    pub fn spec_version(&self) -> u32 {
        le_u32_at(&self.bytes, 112)
    }
    pub fn pending_authorized_at(&self) -> u32 {
        le_u32_at(&self.bytes, 116)
    }
    pub fn flags(&self) -> u32 {
        le_u32_at(&self.bytes, 164)
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum ConstitutionOrigin {
    FutarchyParam,
    FutarchyTreasury,
    FutarchyCode,
    FutarchyMeta,
    ConstitutionalValues,
    GuardianHold,
    EmergencyPlaybook,
    Root,
    Signed,
}

impl ConstitutionOrigin {
    const fn can_set_param(self, class: ParamClass) -> bool {
        matches!(
            (self, class),
            (Self::FutarchyParam, ParamClass::Param)
                | (Self::FutarchyTreasury, ParamClass::Treasury)
                | (Self::FutarchyMeta, ParamClass::Meta)
                | (Self::ConstitutionalValues, ParamClass::Const)
                | (Self::ConstitutionalValues, ParamClass::Entrenched)
                | (Self::ConstitutionalValues, ParamClass::MetaAndValues)
                | (Self::Root, _)
        )
    }
    const fn can_set_capability(self) -> bool {
        matches!(
            self,
            Self::FutarchyMeta | Self::ConstitutionalValues | Self::Root
        )
    }
    const fn can_set_release_channel(self) -> bool {
        matches!(self, Self::FutarchyCode | Self::FutarchyMeta | Self::Root)
    }
    const fn can_set_phase_flag(self) -> bool {
        matches!(
            self,
            Self::FutarchyMeta | Self::GuardianHold | Self::EmergencyPlaybook | Self::Root
        )
    }
    const fn can_charge_meter(self) -> bool {
        matches!(
            self,
            Self::FutarchyTreasury | Self::EmergencyPlaybook | Self::Root
        )
    }
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct ConstitutionState {
    pub params: Vec<ParamRecord>,
    pub meters: Vec<Meter>,
    pub capabilities: Vec<CapabilityRecord>,
    pub phase_flags: PhaseFlags,
    pub release_channel: ReleaseChannel,
}

impl ConstitutionState {
    pub fn genesis() -> Self {
        Self {
            params: genesis_params(),
            meters: genesis_meters(),
            capabilities: genesis_capabilities(),
            phase_flags: PhaseFlags::empty(),
            release_channel: empty_release_channel(),
        }
    }

    pub fn set_param(&mut self, key: ParamKey, next: ParamValue, epoch: u32) -> Result<(), Error> {
        let record = self
            .params
            .iter_mut()
            .find(|r| r.key == key)
            .ok_or(Error::UnknownParam)?;
        *record = record.checked_update(next, epoch)?;
        Ok(())
    }

    pub fn dispatch_set_param(
        &mut self,
        origin: ConstitutionOrigin,
        key: ParamKey,
        next: ParamValue,
        epoch: u32,
    ) -> Result<(), Error> {
        let class = self
            .params
            .iter()
            .find(|r| r.key == key)
            .ok_or(Error::UnknownParam)?
            .class;
        ensure!(origin.can_set_param(class), Error::BadOrigin);
        self.set_param(key, next, epoch)
    }

    pub fn set_capability(&mut self, capability: CapabilityRecord) -> Result<(), Error> {
        if let Some(existing) = self
            .capabilities
            .iter_mut()
            .find(|c| c.class == capability.class && c.capability == capability.capability)
        {
            *existing = capability;
            return Ok(());
        }
        ensure!(
            self.capabilities.len() < MAX_CAPABILITIES,
            Error::TooManyCapabilities
        );
        self.capabilities.push(capability);
        Ok(())
    }

    pub fn dispatch_set_capability(
        &mut self,
        origin: ConstitutionOrigin,
        capability: CapabilityRecord,
    ) -> Result<(), Error> {
        ensure!(origin.can_set_capability(), Error::BadOrigin);
        self.set_capability(capability)
    }

    pub fn capability_enabled(&self, class: ProposalClass, capability: Capability) -> bool {
        self.capabilities
            .iter()
            .any(|c| c.class == class && c.capability == capability && c.enabled)
    }

    pub fn dispatch_set_phase_flag(
        &mut self,
        origin: ConstitutionOrigin,
        flag: u32,
        enabled: bool,
    ) -> Result<(), Error> {
        ensure!(origin.can_set_phase_flag(), Error::BadOrigin);
        self.phase_flags.set(flag, enabled)
    }

    pub fn dispatch_set_release_channel(
        &mut self,
        origin: ConstitutionOrigin,
        bytes: [u8; RELEASE_CHANNEL_LEN],
    ) -> Result<(), Error> {
        ensure!(origin.can_set_release_channel(), Error::BadOrigin);
        self.release_channel = ReleaseChannel::new(bytes)?;
        Ok(())
    }

    pub fn dispatch_charge_meter(
        &mut self,
        origin: ConstitutionOrigin,
        index: usize,
        amount: u128,
        epoch: u32,
    ) -> Result<(), Error> {
        ensure!(origin.can_charge_meter(), Error::BadOrigin);
        let meter = self.meters.get_mut(index).ok_or(Error::UnknownMeter)?;
        meter.charge(amount, epoch)
    }

    pub fn try_state(&self) -> Result<(), Error> {
        ensure!(self.params.len() <= MAX_PARAMS, Error::TooManyParams);
        ensure!(
            self.capabilities.len() <= MAX_CAPABILITIES,
            Error::TooManyCapabilities
        );
        ensure!(self.meters.len() <= MAX_METERS, Error::TooManyMeters);
        PhaseFlags::from_bits(self.phase_flags.bits())?;
        for record in &self.params {
            ensure!(
                record.value.same_kind(record.min) && record.value.same_kind(record.max),
                Error::WrongType
            );
            ensure!(
                record.min.as_u128() <= record.max.as_u128(),
                Error::TryStateViolation
            );
            ensure!(
                record.value.as_u128() >= record.min.as_u128(),
                Error::BelowMin
            );
            ensure!(
                record.value.as_u128() <= record.max.as_u128(),
                Error::AboveMax
            );
            match record.max_delta {
                None => {}
                Some(MaxDelta::Absolute(bound)) => {
                    ensure!(record.value.same_kind(bound), Error::WrongType);
                }
                Some(MaxDelta::Percent(percent)) => {
                    ensure!((1..=100).contains(&percent), Error::WrongType);
                }
                Some(MaxDelta::Factor(factor)) => {
                    ensure!(factor >= 1, Error::WrongType);
                }
            }
        }
        for meter in &self.meters {
            ensure!(meter.spent <= meter.limit, Error::MeterExhausted);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Error {
    UnknownParam,
    UnknownMeter,
    WrongType,
    BelowMin,
    AboveMax,
    DeltaTooLarge,
    CooldownActive,
    MeterOverflow,
    MeterExhausted,
    ReservedPhaseFlag,
    BadReleaseSchema,
    TooManyParams,
    TooManyMeters,
    TooManyCapabilities,
    BadOrigin,
    TryStateViolation,
}

macro_rules! ensure {
    ($cond:expr, $err:expr) => {
        if !$cond {
            return Err($err);
        }
    };
}
use ensure;

fn le_u32_at(bytes: &[u8; RELEASE_CHANNEL_LEN], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

pub fn key16(name: &[u8]) -> ParamKey {
    let mut out = [0u8; 16];
    let len = core::cmp::min(name.len(), out.len());
    out[..len].copy_from_slice(&name[..len]);
    out
}

pub fn empty_release_channel() -> ReleaseChannel {
    let mut bytes = [0u8; RELEASE_CHANNEL_LEN];
    bytes[0] = 1;
    ReleaseChannel { bytes }
}

pub fn genesis_meters() -> Vec<Meter> {
    alloc::vec![
        Meter::new(kernel::KEEPER_BUDGET_EPOCH_FLOOR_USDC, 0),
        Meter::new(0, 0)
    ]
}

pub fn genesis_capabilities() -> Vec<CapabilityRecord> {
    alloc::vec![
        CapabilityRecord {
            class: ProposalClass::Param,
            capability: Capability::SetParam(key16(b"mkt.obs_interval")),
            enabled: true
        },
        CapabilityRecord {
            class: ProposalClass::Meta,
            capability: Capability::SetCapability,
            enabled: true
        },
        CapabilityRecord {
            class: ProposalClass::Code,
            capability: Capability::SetReleaseChannel,
            enabled: true
        },
        CapabilityRecord {
            class: ProposalClass::Treasury,
            capability: Capability::TreasurySpend,
            enabled: true
        },
    ]
}

pub fn genesis_params() -> Vec<ParamRecord> {
    alloc::vec![
        ParamRecord {
            key: key16(b"epoch.length"),
            value: ParamValue::U32(302_400),
            min: ParamValue::U32(201_600),
            max: ParamValue::U32(604_800),
            // 13 §1: Max Δ/decision = 10% — relative to the current value.
            max_delta: Some(MaxDelta::Percent(10)),
            cooldown_epochs: 2,
            last_changed_epoch: 0,
            class: ParamClass::Meta
        },
        ParamRecord {
            key: key16(b"epoch.slots"),
            value: ParamValue::U8(5),
            min: ParamValue::U8(1),
            max: ParamValue::U8(12),
            max_delta: Some(MaxDelta::Absolute(ParamValue::U8(2))),
            cooldown_epochs: 1,
            last_changed_epoch: 0,
            class: ParamClass::Meta
        },
        ParamRecord {
            key: key16(b"mkt.obs_interval"),
            value: ParamValue::U32(10),
            min: ParamValue::U32(5),
            max: ParamValue::U32(50),
            max_delta: Some(MaxDelta::Absolute(ParamValue::U32(5))),
            cooldown_epochs: 1,
            last_changed_epoch: 0,
            class: ParamClass::Param
        },
        ParamRecord {
            key: key16(b"intake.max_per_account"),
            value: ParamValue::U8(4),
            min: ParamValue::U8(2),
            max: ParamValue::U8(8),
            max_delta: Some(MaxDelta::Absolute(ParamValue::U8(2))),
            cooldown_epochs: 2,
            last_changed_epoch: 0,
            class: ParamClass::Meta
        },
        ParamRecord {
            key: key16(b"orc.window"),
            value: ParamValue::U32(43_200),
            min: ParamValue::U32(43_200),
            max: ParamValue::U32(72_000),
            max_delta: None,
            cooldown_epochs: 2,
            last_changed_epoch: 0,
            class: ParamClass::Meta
        },
        ParamRecord {
            key: key16(b"keeper.budget_epoch"),
            value: ParamValue::Balance(12_000_000_000),
            min: ParamValue::Balance(kernel::KEEPER_BUDGET_EPOCH_FLOOR_USDC),
            max: ParamValue::Balance(60_000_000_000),
            // 13 §1: Max Δ/decision = ×2.
            max_delta: Some(MaxDelta::Factor(2)),
            cooldown_epochs: 1,
            last_changed_epoch: 0,
            class: ParamClass::Param
        },
    ]
}

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking {
    use super::*;

    pub fn benchmark_set_param() -> Result<(), Error> {
        let mut state = ConstitutionState::genesis();
        state.dispatch_set_param(
            ConstitutionOrigin::FutarchyParam,
            key16(b"mkt.obs_interval"),
            ParamValue::U32(12),
            1,
        )
    }

    pub fn benchmark_set_release_channel() -> Result<(), Error> {
        let mut state = ConstitutionState::genesis();
        let mut bytes = [0u8; RELEASE_CHANNEL_LEN];
        bytes[0] = 1;
        state.dispatch_set_release_channel(ConstitutionOrigin::FutarchyCode, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_channel() -> ReleaseChannel {
        let mut bytes = [0u8; RELEASE_CHANNEL_LEN];
        bytes[0] = 1;
        bytes[108..112].copy_from_slice(&42u32.to_le_bytes());
        bytes[112..116].copy_from_slice(&7u32.to_le_bytes());
        bytes[116..120].copy_from_slice(&11u32.to_le_bytes());
        bytes[164..168].copy_from_slice(&5u32.to_le_bytes());
        ReleaseChannel::new(bytes).unwrap()
    }

    #[test]
    fn reexports_kernel_and_contract_version() {
        assert_eq!(
            CONTRACT_VERSION,
            futarchy_primitives::INTEGRATION_CONTRACT_VERSION
        );
        assert_eq!(kernel::DESCRIPTOR_LEAD_TIME_BLOCKS, 43_200);
    }

    #[test]
    fn param_update_enforces_bounds_delta_and_cooldown() {
        let mut rec = genesis_params()[0];
        assert_eq!(
            rec.checked_update(ParamValue::U32(200_000), 3),
            Err(Error::BelowMin)
        );
        assert_eq!(
            rec.checked_update(ParamValue::U32(400_000), 3),
            Err(Error::DeltaTooLarge)
        );
        assert_eq!(
            rec.checked_update(ParamValue::U32(310_000), 1),
            Err(Error::CooldownActive)
        );
        rec = rec.checked_update(ParamValue::U32(310_000), 2).unwrap();
        assert_eq!(rec.value, ParamValue::U32(310_000));
    }

    #[test]
    fn percent_delta_is_recomputed_from_the_current_value() {
        // 13 §1: epoch.length Max Δ/decision = 10%. A fixed absolute step
        // would let a lowered value be raised by more than 10% per decision.
        let mut rec = genesis_params()[0];
        assert_eq!(rec.max_delta, Some(MaxDelta::Percent(10)));
        rec = rec.checked_update(ParamValue::U32(275_000), 2).unwrap();
        rec = rec.checked_update(ParamValue::U32(250_000), 4).unwrap();
        rec = rec.checked_update(ParamValue::U32(226_000), 6).unwrap();
        rec = rec.checked_update(ParamValue::U32(204_000), 8).unwrap();
        rec = rec.checked_update(ParamValue::U32(201_600), 10).unwrap();
        // At 201,600 the 10% allowance is 20,160 — a 30,240 raise (15%) that
        // the old absolute bound accepted must now fail.
        assert_eq!(
            rec.checked_update(ParamValue::U32(231_840), 12),
            Err(Error::DeltaTooLarge)
        );
        rec = rec.checked_update(ParamValue::U32(221_760), 12).unwrap();
        assert_eq!(rec.value, ParamValue::U32(221_760));
    }

    #[test]
    fn factor_delta_bounds_both_directions() {
        // 13 §1: keeper.budget_epoch Max Δ/decision = ×2.
        let mut rec = genesis_params()
            .into_iter()
            .find(|record| record.key == key16(b"keeper.budget_epoch"))
            .unwrap();
        assert_eq!(rec.max_delta, Some(MaxDelta::Factor(2)));
        assert_eq!(
            rec.checked_update(ParamValue::Balance(24_000_000_001), 1),
            Err(Error::DeltaTooLarge)
        );
        rec = rec
            .checked_update(ParamValue::Balance(24_000_000_000), 1)
            .unwrap();
        assert_eq!(
            rec.checked_update(ParamValue::Balance(11_999_999_999), 2),
            Err(Error::DeltaTooLarge)
        );
        rec = rec
            .checked_update(ParamValue::Balance(12_000_000_000), 2)
            .unwrap();
        assert_eq!(rec.value, ParamValue::Balance(12_000_000_000));
    }

    #[test]
    fn genesis_param_keys_are_canonical_and_distinct() {
        let params = genesis_params();
        // 13 §1 canonical spellings (Codex review, PR #14): the seeded keys
        // must match the names downstream binders derive with key16.
        assert!(params
            .iter()
            .any(|record| record.key == key16(b"intake.max_per_account")));
        assert!(params
            .iter()
            .any(|record| record.key == key16(b"keeper.budget_epoch")));
        for (index, record) in params.iter().enumerate() {
            for other in params.iter().skip(index + 1) {
                assert_ne!(record.key, other.key, "duplicate ParamKey after key16");
            }
        }
    }

    #[test]
    fn dispatch_set_param_checks_origin_and_error_paths() {
        let mut state = ConstitutionState::genesis();
        assert_eq!(
            state.dispatch_set_param(
                ConstitutionOrigin::Signed,
                key16(b"mkt.obs_interval"),
                ParamValue::U32(12),
                1
            ),
            Err(Error::BadOrigin)
        );
        assert_eq!(
            state.dispatch_set_param(
                ConstitutionOrigin::FutarchyTreasury,
                key16(b"mkt.obs_interval"),
                ParamValue::U32(12),
                1
            ),
            Err(Error::BadOrigin)
        );
        assert_eq!(
            state.dispatch_set_param(
                ConstitutionOrigin::FutarchyParam,
                key16(b"missing"),
                ParamValue::U32(12),
                1
            ),
            Err(Error::UnknownParam)
        );
        state
            .dispatch_set_param(
                ConstitutionOrigin::FutarchyParam,
                key16(b"mkt.obs_interval"),
                ParamValue::U32(12),
                1,
            )
            .unwrap();
        assert_eq!(
            state
                .params
                .iter()
                .find(|r| r.key == key16(b"mkt.obs_interval"))
                .unwrap()
                .value,
            ParamValue::U32(12)
        );
    }

    #[test]
    fn meters_reset_by_epoch_and_never_overspend() {
        let mut meter = Meter::new(10, 0);
        meter.charge(7, 0).unwrap();
        assert_eq!(meter.charge(4, 0), Err(Error::MeterExhausted));
        meter.charge(4, 1).unwrap();
        assert_eq!(meter.spent, 4);
    }

    #[test]
    fn dispatch_charge_meter_checks_origin_and_bounds() {
        let mut state = ConstitutionState::genesis();
        assert_eq!(
            state.dispatch_charge_meter(ConstitutionOrigin::Signed, 0, 1, 0),
            Err(Error::BadOrigin)
        );
        assert_eq!(
            state.dispatch_charge_meter(ConstitutionOrigin::FutarchyTreasury, 99, 1, 0),
            Err(Error::UnknownMeter)
        );
        assert_eq!(
            state.dispatch_charge_meter(ConstitutionOrigin::FutarchyTreasury, 1, 1, 0),
            Err(Error::MeterExhausted)
        );
        state
            .dispatch_charge_meter(ConstitutionOrigin::FutarchyTreasury, 0, 1, 0)
            .unwrap();
    }

    #[test]
    fn phase_flags_reject_reserved_bits_and_origin_misuse() {
        let mut state = ConstitutionState::genesis();
        assert_eq!(
            state.dispatch_set_phase_flag(
                ConstitutionOrigin::Signed,
                PhaseFlags::SUDO_PRESENT,
                true
            ),
            Err(Error::BadOrigin)
        );
        state
            .dispatch_set_phase_flag(
                ConstitutionOrigin::GuardianHold,
                PhaseFlags::SUDO_PRESENT,
                true,
            )
            .unwrap();
        assert!(state.phase_flags.contains(PhaseFlags::SUDO_PRESENT));
        assert_eq!(
            state.dispatch_set_phase_flag(ConstitutionOrigin::GuardianHold, 1 << 8, true),
            Err(Error::ReservedPhaseFlag)
        );
    }

    #[test]
    fn release_channel_is_fixed_width_offset_readable_and_origin_checked() {
        let channel = release_channel();
        assert_eq!(RELEASE_CHANNEL_STORAGE_KEY.len(), 32);
        assert_eq!(channel.updated_at(), 42);
        assert_eq!(channel.spec_version(), 7);
        assert_eq!(channel.pending_authorized_at(), 11);
        assert_eq!(channel.flags(), 5);
        let mut bad = [0u8; RELEASE_CHANNEL_LEN];
        bad[0] = 2;
        assert_eq!(ReleaseChannel::new(bad), Err(Error::BadReleaseSchema));
        let mut state = ConstitutionState::genesis();
        assert_eq!(
            state.dispatch_set_release_channel(ConstitutionOrigin::Signed, bad),
            Err(Error::BadOrigin)
        );
        let mut good = [0u8; RELEASE_CHANNEL_LEN];
        good[0] = 1;
        state
            .dispatch_set_release_channel(ConstitutionOrigin::FutarchyCode, good)
            .unwrap();
    }

    #[test]
    fn capability_table_is_bounded_origin_checked_and_queryable() {
        let mut state = ConstitutionState::genesis();
        let cap = CapabilityRecord {
            class: ProposalClass::Meta,
            capability: Capability::SetCapability,
            enabled: true,
        };
        assert_eq!(
            state.dispatch_set_capability(ConstitutionOrigin::Signed, cap),
            Err(Error::BadOrigin)
        );
        state
            .dispatch_set_capability(ConstitutionOrigin::FutarchyMeta, cap)
            .unwrap();
        assert!(state.capability_enabled(ProposalClass::Meta, Capability::SetCapability));
    }

    #[test]
    fn try_state_rejects_corrupt_storage_shapes() {
        let state = ConstitutionState::genesis();
        state.try_state().unwrap();
        let mut bad = state.clone();
        bad.phase_flags = PhaseFlags(1 << 8);
        assert_eq!(bad.try_state(), Err(Error::ReservedPhaseFlag));
        let mut bad_meter = state;
        bad_meter.meters[0].spent = bad_meter.meters[0].limit + 1;
        assert_eq!(bad_meter.try_state(), Err(Error::MeterExhausted));
    }
}
