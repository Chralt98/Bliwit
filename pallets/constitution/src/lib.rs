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

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct ParamRecord {
    pub key: ParamKey,
    pub value: ParamValue,
    pub min: ParamValue,
    pub max: ParamValue,
    pub max_delta: Option<ParamValue>,
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
        if let Some(max_delta) = self.max_delta {
            ensure!(max_delta.same_kind(next), Error::WrongType);
            let old = self.value.as_u128();
            let new = next.as_u128();
            let delta = old.abs_diff(new);
            ensure!(delta <= max_delta.as_u128(), Error::DeltaTooLarge);
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

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct ConstitutionState {
    pub params: Vec<ParamRecord>,
    pub meters: Vec<Meter>,
    pub capabilities: Vec<CapabilityRecord>,
    pub phase_flags: PhaseFlags,
    pub release_channel: ReleaseChannel,
}

impl ConstitutionState {
    pub fn set_param(&mut self, key: ParamKey, next: ParamValue, epoch: u32) -> Result<(), Error> {
        let record = self
            .params
            .iter_mut()
            .find(|r| r.key == key)
            .ok_or(Error::UnknownParam)?;
        *record = record.checked_update(next, epoch)?;
        Ok(())
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

    pub fn capability_enabled(&self, class: ProposalClass, capability: Capability) -> bool {
        self.capabilities
            .iter()
            .any(|c| c.class == class && c.capability == capability && c.enabled)
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Error {
    UnknownParam,
    WrongType,
    BelowMin,
    AboveMax,
    DeltaTooLarge,
    CooldownActive,
    MeterOverflow,
    MeterExhausted,
    ReservedPhaseFlag,
    BadReleaseSchema,
    TooManyCapabilities,
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

pub fn genesis_params() -> Vec<ParamRecord> {
    alloc::vec![
        ParamRecord {
            key: key16(b"epoch.length"),
            value: ParamValue::U32(302_400),
            min: ParamValue::U32(201_600),
            max: ParamValue::U32(604_800),
            max_delta: Some(ParamValue::U32(30_240)),
            cooldown_epochs: 2,
            last_changed_epoch: 0,
            class: ParamClass::Meta
        },
        ParamRecord {
            key: key16(b"epoch.slots"),
            value: ParamValue::U8(5),
            min: ParamValue::U8(1),
            max: ParamValue::U8(12),
            max_delta: Some(ParamValue::U8(2)),
            cooldown_epochs: 1,
            last_changed_epoch: 0,
            class: ParamClass::Meta
        },
        ParamRecord {
            key: key16(b"mkt.obs_interval"),
            value: ParamValue::U32(10),
            min: ParamValue::U32(5),
            max: ParamValue::U32(50),
            max_delta: Some(ParamValue::U32(5)),
            cooldown_epochs: 1,
            last_changed_epoch: 0,
            class: ParamClass::Param
        },
        ParamRecord {
            key: key16(b"intake.max_acct"),
            value: ParamValue::U8(4),
            min: ParamValue::U8(2),
            max: ParamValue::U8(8),
            max_delta: Some(ParamValue::U8(2)),
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
            key: key16(b"keeper.budget"),
            value: ParamValue::Balance(12_000_000_000),
            min: ParamValue::Balance(kernel::KEEPER_BUDGET_EPOCH_FLOOR_USDC),
            max: ParamValue::Balance(60_000_000_000),
            max_delta: Some(ParamValue::Balance(12_000_000_000)),
            cooldown_epochs: 1,
            last_changed_epoch: 0,
            class: ParamClass::Param
        },
    ]
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
    fn meters_reset_by_epoch_and_never_overspend() {
        let mut meter = Meter::new(10, 0);
        meter.charge(7, 0).unwrap();
        assert_eq!(meter.charge(4, 0), Err(Error::MeterExhausted));
        meter.charge(4, 1).unwrap();
        assert_eq!(meter.spent, 4);
    }

    #[test]
    fn phase_flags_reject_reserved_bits() {
        let mut flags = PhaseFlags::empty();
        flags.set(PhaseFlags::SUDO_PRESENT, true).unwrap();
        assert!(flags.contains(PhaseFlags::SUDO_PRESENT));
        assert_eq!(flags.set(1 << 8, true), Err(Error::ReservedPhaseFlag));
    }

    #[test]
    fn release_channel_is_fixed_width_and_offset_readable() {
        let channel = release_channel();
        assert_eq!(RELEASE_CHANNEL_STORAGE_KEY.len(), 32);
        assert_eq!(channel.updated_at(), 42);
        assert_eq!(channel.spec_version(), 7);
        assert_eq!(channel.pending_authorized_at(), 11);
        assert_eq!(channel.flags(), 5);
        let mut bad = [0u8; RELEASE_CHANNEL_LEN];
        bad[0] = 2;
        assert_eq!(ReleaseChannel::new(bad), Err(Error::BadReleaseSchema));
    }

    #[test]
    fn capability_table_is_bounded_and_queryable() {
        let mut state = ConstitutionState {
            params: genesis_params(),
            meters: Vec::new(),
            capabilities: Vec::new(),
            phase_flags: PhaseFlags::empty(),
            release_channel: release_channel(),
        };
        let cap = CapabilityRecord {
            class: ProposalClass::Meta,
            capability: Capability::SetCapability,
            enabled: true,
        };
        state.set_capability(cap).unwrap();
        assert!(state.capability_enabled(ProposalClass::Meta, Capability::SetCapability));
    }
}
