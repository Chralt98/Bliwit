#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::{Balance, BlockNumber, FixedU64, ParamKey, ProposalClass};
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub use futarchy_primitives::kernel;
pub use futarchy_primitives::INTEGRATION_CONTRACT_VERSION as CONTRACT_VERSION;

/// `twox128("Constitution") ++ twox128("ReleaseChannel")`.
pub const RELEASE_CHANNEL_STORAGE_KEY: [u8; 32] = [
    0xfb, 0x8c, 0xcb, 0xf6, 0x77, 0xa3, 0xd2, 0xce, 0x27, 0xab, 0x85, 0x16, 0x5f, 0x32, 0xdf, 0x6a,
    0xfe, 0xc7, 0x19, 0x4a, 0x53, 0x68, 0xa5, 0x8e, 0x1f, 0x6b, 0xf5, 0x74, 0x57, 0x13, 0x4a, 0x6c,
];
pub const RELEASE_CHANNEL_LEN: usize = 168;
pub const MAX_PARAMS: usize = 128;
pub const MAX_CAPABILITIES: usize = 64;
pub const MAX_METERS: usize = 16;
/// 13 rule 7 meta-bound: `amend_registry` may not set a cooldown above this.
pub const META_MAX_COOLDOWN_EPOCHS: u32 = 8;

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
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

impl ParamClass {
    /// 13 rule 7: the `ParamView.class` projection (02 §4) — CONST/entrenched
    /// project onto `Constitutional`, META+values onto `Meta`.
    pub const fn as_proposal_class(self) -> ProposalClass {
        match self {
            Self::Param => ProposalClass::Param,
            Self::Treasury => ProposalClass::Treasury,
            Self::Meta | Self::MetaAndValues => ProposalClass::Meta,
            Self::Const | Self::Entrenched => ProposalClass::Constitutional,
        }
    }
}

/// Per-decision rate limit for a constitution key, mirroring the three
/// Max Δ/decision semantics of the 13 §1 table: absolute steps in the key's
/// own unit (e.g. `2`, `5`), steps relative to the current value (e.g. `10%`),
/// and multiplicative bounds (e.g. `×2`).
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
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
    /// 13 rule 7: bounds carry a kernel floor/ceiling and are genesis-fixed —
    /// `amend_registry` cannot move them.
    pub kernel_bounded: bool,
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

    /// `constitution.amend_registry` core (06 §2.1/§3.2; 13 rule 2/7): amend a
    /// key's governance metadata — bounds, max-Δ, cooldown — never its value,
    /// class, or key, within the compile-time meta-bounds. Kernel-bounded rows
    /// keep their bounds genesis-fixed.
    pub fn checked_amend(
        &self,
        min: ParamValue,
        max: ParamValue,
        max_delta: Option<MaxDelta>,
        cooldown_epochs: u32,
    ) -> Result<Self, Error> {
        ensure!(
            self.value.same_kind(min) && self.value.same_kind(max),
            Error::WrongType
        );
        // 13 rule 2: for kernel-bounded keys the WHOLE governance-metadata
        // tuple (min/max/max-delta/cooldown/class) is genesis-fixed — an
        // amendment could otherwise gut a rate-limit defense (e.g. widen
        // `dec.window`'s Δ) while the kernel value-envelope still held.
        if self.kernel_bounded {
            return Err(Error::KernelBoundImmutable);
        }
        ensure!(min.as_u128() <= max.as_u128(), Error::MetaBoundViolation);
        ensure!(
            self.value.as_u128() >= min.as_u128() && self.value.as_u128() <= max.as_u128(),
            Error::MetaBoundViolation
        );
        match max_delta {
            None => {}
            Some(MaxDelta::Absolute(bound)) => {
                ensure!(self.value.same_kind(bound), Error::WrongType);
            }
            Some(MaxDelta::Percent(percent)) => {
                ensure!((1..=100).contains(&percent), Error::MetaBoundViolation);
            }
            Some(MaxDelta::Factor(factor)) => {
                ensure!(factor >= 1, Error::MetaBoundViolation);
            }
        }
        ensure!(
            cooldown_epochs <= META_MAX_COOLDOWN_EPOCHS,
            Error::MetaBoundViolation
        );
        Ok(Self {
            min,
            max,
            max_delta,
            cooldown_epochs,
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
        // Compute on a copy so a refused charge is a strict no-op (G-1):
        // the lazy window reset must not persist through a failure — the
        // FRAME shell's transactional storage would roll it back, and the
        // randomized shell-vs-core differential pins the two paths equal.
        let mut next = *self;
        if epoch > next.reset_epoch {
            next.spent = 0;
            next.reset_epoch = epoch;
        }
        let spent = next.spent.checked_add(amount).ok_or(Error::MeterOverflow)?;
        ensure!(spent <= next.limit, Error::MeterExhausted);
        next.spent = spent;
        *self = next;
        Ok(())
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
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

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
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
    /// Bits the bootstrap-sudo path may write ("arming phase flags", 09 §5.4):
    /// the four arming bits + the sudo-present marker. Machinery bits 5–7 are
    /// owned by sibling-pallet state (PB-LEDGER-FREEZE / dead-man / reserve
    /// probe — 02 §7.3) and are writable only through their dedicated
    /// runtime-internal setters.
    pub const SUDO_ARMABLE_MASK: u32 = Self::SHADOW_MODE
        | Self::PARAM_ARMED
        | Self::TREASURY_ARMED
        | Self::CODE_META_ARMED
        | Self::SUDO_PRESENT;

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
    /// 02 §12: flags word bits — 0 `SECURITY`, 1 `EXPEDITED`,
    /// 2 `URGENT_UPGRADE`; "bits 3–31 reserved zero".
    pub const FLAGS_MASK: u32 = 0x7;

    pub fn new(bytes: [u8; RELEASE_CHANNEL_LEN]) -> Result<Self, Error> {
        ensure!(bytes[0] == 1, Error::BadReleaseSchema);
        // 02 §12 frozen layout: reserved flag bits MUST be zero — a writer
        // publishing them would make metadata-less readers diverge without a
        // schema bump.
        let flags = le_u32_at(&bytes, 164);
        ensure!(flags & !Self::FLAGS_MASK == 0, Error::BadReleaseSchema);
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
    /// 06 §3.2 authority matrix, per key class. META+values keys are enacted
    /// by the **beliefs** layer (`FutarchyMeta`) — 06 §1 is explicit that
    /// `ConstitutionalValues` cannot invoke PARAM/META parameter keys; the
    /// values half of the dual consent is the execute-time ratification check
    /// (06 §2.2, guard step 4), not a direct dispatch path (see PLAN SQ-6).
    /// CONST/entrenched-class keys are values-layer business (06 §1, §2.4).
    ///
    /// No Root arm: 09 §5.4's bootstrap-sudo power list is exhaustive
    /// ("incident response, arming phase flags, the Phase-3→4 upgrade") and
    /// does not include parameter administration; 06 §3.2 has no Root column
    /// (PLAN SQ-11 tracks whether Phase-0–3 calibration needs a writer —
    /// until answered, the conservative narrow reading holds, R-7).
    pub const fn can_set_param(self, class: ParamClass) -> bool {
        matches!(
            (self, class),
            (Self::FutarchyParam, ParamClass::Param)
                | (Self::FutarchyTreasury, ParamClass::Treasury)
                | (Self::FutarchyMeta, ParamClass::Meta)
                | (Self::FutarchyMeta, ParamClass::MetaAndValues)
                | (Self::ConstitutionalValues, ParamClass::Const)
                | (Self::ConstitutionalValues, ParamClass::Entrenched)
        )
    }
    /// 06 §3.2 row 4: `constitution.set_capability` is a `FutarchyMeta` call;
    /// the ConstitutionalValues column reads "ratify where rule-altering",
    /// i.e. values participates via ratification only, never direct dispatch.
    /// No Root arm (09 §5.4 exhaustive sudo scope — see `can_set_param`).
    pub const fn can_set_capability(self) -> bool {
        matches!(self, Self::FutarchyMeta)
    }
    /// 06 §2.1 (`constitution` track) and 06 §3.2 row 4 / 13 rule 2: registry
    /// amendments are values business (`ConstitutionalValues`) and
    /// META-amendable within meta-bounds (`FutarchyMeta`).
    pub const fn can_amend_registry(self) -> bool {
        matches!(self, Self::FutarchyMeta | Self::ConstitutionalValues)
    }
    /// 02 §12: the release channel's writers are exhaustive — (a) the execution
    /// guard's runtime-internal path (not origin-mediated) and (b) the
    /// `ConstitutionalValues` origin via `constitution.set_release_channel`.
    /// "No other origin can write it" — including bootstrap Root/sudo.
    pub const fn can_set_release_channel(self) -> bool {
        matches!(self, Self::ConstitutionalValues)
    }
    /// No document defines a phase-flag *call*; the only origin-mediated
    /// writer the spec names is bootstrap sudo — 09 §5.4 limits sudo to
    /// "arming phase flags on evidence" (Phases 0–3, D-13). Machinery bits
    /// (ledger frozen / dead-man / reserve — 02 §7.3 bits 5–7) are written by
    /// sibling pallets through the runtime-internal path, not an origin
    /// (see PLAN SQ-5).
    pub const fn can_set_phase_flag(self) -> bool {
        matches!(self, Self::Root)
    }
    /// Meter charging is machinery (guard/treasury paths), modelled here with
    /// the origins those paths carry; the FRAME shell exposes no extrinsic
    /// for it — sibling pallets use the runtime-internal API (see PLAN SQ-12).
    /// No Root arm (09 §5.4 exhaustive sudo scope — see `can_set_param`).
    pub const fn can_charge_meter(self) -> bool {
        matches!(self, Self::FutarchyTreasury | Self::EmergencyPlaybook)
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

    pub fn dispatch_amend_registry(
        &mut self,
        origin: ConstitutionOrigin,
        key: ParamKey,
        min: ParamValue,
        max: ParamValue,
        max_delta: Option<MaxDelta>,
        cooldown_epochs: u32,
    ) -> Result<(), Error> {
        ensure!(origin.can_amend_registry(), Error::BadOrigin);
        let record = self
            .params
            .iter_mut()
            .find(|r| r.key == key)
            .ok_or(Error::UnknownParam)?;
        *record = record.checked_amend(min, max, max_delta, cooldown_epochs)?;
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
        // 09 §5.4: the origin-mediated path may touch arming bits only; the
        // machinery bits (5–7) belong to sibling-pallet state and have
        // dedicated internal setters in the FRAME shell.
        ensure!(
            flag & !PhaseFlags::SUDO_ARMABLE_MASK == 0,
            Error::FlagNotArmable
        );
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
    FlagNotArmable,
    KernelBoundImmutable,
    MetaBoundViolation,
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

/// Canonical `ParamKey` encoding (13 rule 6): UTF-8 name, zero-padded to 16
/// bytes. Names longer than 16 bytes have explicit short keys in the 13 §1
/// registry — silent truncation is forbidden.
pub fn key16(name: &[u8]) -> ParamKey {
    assert!(
        name.len() <= 16,
        "ParamKey names longer than 16 bytes need an explicit canonical key (13 rule 6)"
    );
    let mut out = [0u8; 16];
    out[..name.len()].copy_from_slice(name);
    out
}

pub fn empty_release_channel() -> ReleaseChannel {
    let mut bytes = [0u8; RELEASE_CHANNEL_LEN];
    bytes[0] = 1;
    ReleaseChannel { bytes }
}

/// Empty at genesis: I-17's envelope meters live with their owning pallets
/// (treasury issuance/outflow, guard upgrade-spacing — 15 §1); the
/// constitution keeps the generic bounded-meter primitive for kernel
/// envelopes wired later.
pub fn genesis_meters() -> Vec<Meter> {
    Vec::new()
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
            // 06 §3.2 row 3: `system.authorize_upgrade` is the CODE-class
            // capability; the release channel has no origin-mediated CODE
            // writer (02 §12).
            class: ProposalClass::Code,
            capability: Capability::AuthorizeUpgrade,
            enabled: true
        },
        CapabilityRecord {
            class: ProposalClass::Treasury,
            capability: Capability::TreasurySpend,
            enabled: true
        },
    ]
}

/// The materialized 13 §1 registry: every row with a scalar concrete default
/// and no open `[VERIFY]` tag, keyed per 13 rule 6 (explicit short keys for
/// names longer than 16 bytes; `.param/.trs/.code/.meta` per-class suffixes).
/// Seeding criterion for `[VERIFY]`/sim-gated rows: a concrete numeric
/// default is a simulation hypothesis (13 rule 4) and is seeded; rows whose
/// default is a formula, unset, or TGE-dependent stay out — currently
/// `fee.vit_usdc` (TGE ref), `keeper.rebate` (fee-basis formula),
/// `collator.bond` and `sec.*`/`ops.*` (uncalibrated). `gate.v_min` and
/// `dis.merit_min` carry derived defaults and bind at their consuming
/// engines. Kernel-bounded flags follow the enumeration in 13 rule 7.
#[allow(clippy::too_many_lines)]
pub fn genesis_params() -> Vec<ParamRecord> {
    #[allow(clippy::too_many_arguments)]
    fn row(
        key: &[u8],
        value: ParamValue,
        min: ParamValue,
        max: ParamValue,
        max_delta: Option<MaxDelta>,
        cooldown_epochs: u32,
        class: ParamClass,
        kernel_bounded: bool,
    ) -> ParamRecord {
        ParamRecord {
            key: key16(key),
            value,
            min,
            max,
            max_delta,
            cooldown_epochs,
            last_changed_epoch: 0,
            class,
            kernel_bounded,
        }
    }
    alloc::vec![
        row(
            b"epoch.length",
            ParamValue::U32(302_400),
            ParamValue::U32(201_600),
            ParamValue::U32(604_800),
            Some(MaxDelta::Percent(10)),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"epoch.slots",
            ParamValue::U8(5),
            ParamValue::U8(1),
            ParamValue::U8(12),
            Some(MaxDelta::Absolute(ParamValue::U8(2))),
            1,
            ParamClass::Meta,
            false
        ),
        row(
            b"epoch.horizon_k",
            ParamValue::U8(2),
            ParamValue::U8(1),
            ParamValue::U8(4),
            Some(MaxDelta::Absolute(ParamValue::U8(1))),
            4,
            ParamClass::MetaAndValues,
            false
        ),
        row(
            b"mkt.obs_interval",
            ParamValue::U32(10),
            ParamValue::U32(5),
            ParamValue::U32(50),
            Some(MaxDelta::Absolute(ParamValue::U32(5))),
            1,
            ParamClass::Param,
            false
        ),
        row(
            b"mkt.kappa",
            ParamValue::Fixed(FixedU64(5_000_000)),
            ParamValue::Fixed(FixedU64(1_000_000)),
            ParamValue::Fixed(FixedU64(20_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(2_000_000)))),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"mkt.fee",
            ParamValue::Perbill(3_000_000),
            ParamValue::Perbill(500_000),
            ParamValue::Perbill(10_000_000),
            Some(MaxDelta::Absolute(ParamValue::Perbill(1_000_000))),
            1,
            ParamClass::Param,
            false
        ),
        row(
            b"dec.window",
            ParamValue::U32(43_200),
            ParamValue::U32(14_400),
            ParamValue::U32(86_400),
            Some(MaxDelta::Percent(20)),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.trailing",
            ParamValue::U32(14_400),
            ParamValue::U32(3_600),
            ParamValue::U32(28_800),
            None,
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"dec.delta_max",
            ParamValue::Fixed(FixedU64(50_000_000)),
            ParamValue::Fixed(FixedU64(20_000_000)),
            ParamValue::Fixed(FixedU64(100_000_000)),
            None,
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"dec.coverage",
            ParamValue::Percent(95),
            ParamValue::Percent(90),
            ParamValue::Percent(99),
            None,
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"gate.p_max",
            ParamValue::Fixed(FixedU64(50_000_000)),
            ParamValue::Fixed(FixedU64(0)),
            ParamValue::Fixed(FixedU64(kernel::GATE_P_MAX_CEILING_1E9)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(10_000_000)))),
            4,
            ParamClass::MetaAndValues,
            true
        ),
        row(
            b"gate.eps",
            ParamValue::Fixed(FixedU64(20_000_000)),
            ParamValue::Fixed(FixedU64(5_000_000)),
            ParamValue::Fixed(FixedU64(50_000_000)),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"gate.nb_coverage",
            ParamValue::Percent(98),
            ParamValue::Percent(95),
            ParamValue::Percent(100),
            None,
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"gate.nb_conv",
            ParamValue::Fixed(FixedU64(10_000_000)),
            ParamValue::Fixed(FixedU64(5_000_000)),
            ParamValue::Fixed(FixedU64(20_000_000)),
            None,
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"exec.grace",
            ParamValue::U32(201_600),
            ParamValue::U32(100_800),
            ParamValue::U32(432_000),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"code.spacing",
            ParamValue::U32(432_000),
            ParamValue::U32(201_600),
            ParamValue::U32(u32::MAX),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.delta.param",
            ParamValue::Fixed(FixedU64(15_000_000)),
            ParamValue::Fixed(FixedU64(5_000_000)),
            ParamValue::Fixed(FixedU64(100_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(5_000_000)))),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.delta.trs",
            ParamValue::Fixed(FixedU64(25_000_000)),
            ParamValue::Fixed(FixedU64(5_000_000)),
            ParamValue::Fixed(FixedU64(100_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(5_000_000)))),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.delta.code",
            ParamValue::Fixed(FixedU64(40_000_000)),
            ParamValue::Fixed(FixedU64(5_000_000)),
            ParamValue::Fixed(FixedU64(100_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(5_000_000)))),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.delta.meta",
            ParamValue::Fixed(FixedU64(60_000_000)),
            ParamValue::Fixed(FixedU64(5_000_000)),
            ParamValue::Fixed(FixedU64(100_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(5_000_000)))),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.sigma.param",
            ParamValue::Fixed(FixedU64(3_000_000)),
            ParamValue::Fixed(FixedU64(0)),
            ParamValue::Fixed(FixedU64(50_000_000)),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.sigma.trs",
            ParamValue::Fixed(FixedU64(5_000_000)),
            ParamValue::Fixed(FixedU64(0)),
            ParamValue::Fixed(FixedU64(50_000_000)),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.sigma.code",
            ParamValue::Fixed(FixedU64(8_000_000)),
            ParamValue::Fixed(FixedU64(0)),
            ParamValue::Fixed(FixedU64(50_000_000)),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.sigma.meta",
            ParamValue::Fixed(FixedU64(10_000_000)),
            ParamValue::Fixed(FixedU64(0)),
            ParamValue::Fixed(FixedU64(50_000_000)),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.v_min.param",
            ParamValue::Balance(100_000_000_000),
            ParamValue::Balance(10_000_000_000),
            ParamValue::Balance(1_000_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"dec.v_min.trs",
            ParamValue::Balance(250_000_000_000),
            ParamValue::Balance(25_000_000_000),
            ParamValue::Balance(2_500_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"dec.v_min.code",
            ParamValue::Balance(600_000_000_000),
            ParamValue::Balance(60_000_000_000),
            ParamValue::Balance(6_000_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"dec.v_min.meta",
            ParamValue::Balance(1_200_000_000_000),
            ParamValue::Balance(120_000_000_000),
            ParamValue::Balance(12_000_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"prop.bond.param",
            ParamValue::Balance(1_000_000_000),
            ParamValue::Balance(100_000_000),
            ParamValue::Balance(10_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"prop.bond.trs",
            ParamValue::Balance(5_000_000_000),
            ParamValue::Balance(500_000_000),
            ParamValue::Balance(50_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"prop.bond.code",
            ParamValue::Balance(25_000_000_000),
            ParamValue::Balance(2_500_000_000),
            ParamValue::Balance(250_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"prop.bond.meta",
            ParamValue::Balance(50_000_000_000),
            ParamValue::Balance(5_000_000_000),
            ParamValue::Balance(500_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"exec.lock.param",
            ParamValue::U32(28_800),
            ParamValue::U32(14_400),
            ParamValue::U32(432_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"exec.lock.trs",
            ParamValue::U32(43_200),
            ParamValue::U32(14_400),
            ParamValue::U32(432_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"exec.lock.code",
            ParamValue::U32(100_800),
            ParamValue::U32(14_400),
            ParamValue::U32(432_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"exec.lock.meta",
            ParamValue::U32(201_600),
            ParamValue::U32(14_400),
            ParamValue::U32(432_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"intake.max_acct",
            ParamValue::U8(4),
            ParamValue::U8(2),
            ParamValue::U8(8),
            Some(MaxDelta::Absolute(ParamValue::U8(2))),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"intake.slash_pct",
            ParamValue::Percent(10),
            ParamValue::Percent(5),
            ParamValue::Percent(25),
            Some(MaxDelta::Absolute(ParamValue::Percent(5))),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"pol.b.param",
            ParamValue::Balance(10_000_000_000),
            ParamValue::Balance(0),
            ParamValue::Balance(u128::MAX),
            Some(MaxDelta::Percent(25)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            b"pol.b.trs",
            ParamValue::Balance(25_000_000_000),
            ParamValue::Balance(0),
            ParamValue::Balance(u128::MAX),
            Some(MaxDelta::Percent(25)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            b"pol.b.code",
            ParamValue::Balance(60_000_000_000),
            ParamValue::Balance(0),
            ParamValue::Balance(u128::MAX),
            Some(MaxDelta::Percent(25)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            b"pol.b.meta",
            ParamValue::Balance(100_000_000_000),
            ParamValue::Balance(0),
            ParamValue::Balance(u128::MAX),
            Some(MaxDelta::Percent(25)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            b"pol.b_gate",
            ParamValue::Balance(7_500_000_000),
            ParamValue::Balance(0),
            ParamValue::Balance(u128::MAX),
            Some(MaxDelta::Percent(25)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            b"pol.budget_epoch",
            ParamValue::Perbill(7_500_000),
            ParamValue::Perbill(0),
            ParamValue::Perbill(15_000_000),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"trs.cap_proposal",
            ParamValue::Percent(5),
            ParamValue::Percent(0),
            ParamValue::Percent(10),
            Some(MaxDelta::Absolute(ParamValue::Percent(1))),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"trs.cap_30d",
            ParamValue::Percent(10),
            ParamValue::Percent(0),
            ParamValue::Percent(15),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"trs.cap_180d",
            ParamValue::Percent(30),
            ParamValue::Percent(0),
            ParamValue::Percent(40),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"trs.stream_thr",
            ParamValue::Perbill(10_000_000),
            ParamValue::Perbill(5_000_000),
            ParamValue::Perbill(50_000_000),
            None,
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"trs.reward.param",
            ParamValue::Balance(500_000_000),
            ParamValue::Balance(50_000_000),
            ParamValue::Balance(5_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"trs.reward.trs",
            ParamValue::Balance(25_000_000_000),
            ParamValue::Balance(2_500_000_000),
            ParamValue::Balance(250_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"trs.reward.code",
            ParamValue::Balance(25_000_000_000),
            ParamValue::Balance(2_500_000_000),
            ParamValue::Balance(250_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"trs.reward.meta",
            ParamValue::Balance(25_000_000_000),
            ParamValue::Balance(2_500_000_000),
            ParamValue::Balance(250_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"iss.inflation",
            ParamValue::Percent(2),
            ParamValue::Percent(0),
            ParamValue::Percent(2),
            None,
            0,
            ParamClass::Const,
            true
        ),
        row(
            b"welfare.thS_lo",
            ParamValue::Fixed(FixedU64(900_000_000)),
            ParamValue::Fixed(FixedU64(900_000_000)),
            ParamValue::Fixed(FixedU64(1_000_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(10_000_000)))),
            4,
            ParamClass::Const,
            true
        ),
        row(
            b"welfare.thS_hi",
            ParamValue::Fixed(FixedU64(980_000_000)),
            ParamValue::Fixed(FixedU64(900_000_000)),
            ParamValue::Fixed(FixedU64(1_000_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(10_000_000)))),
            4,
            ParamClass::Const,
            false
        ),
        row(
            b"welfare.thC_lo",
            ParamValue::Fixed(FixedU64(850_000_000)),
            ParamValue::Fixed(FixedU64(850_000_000)),
            ParamValue::Fixed(FixedU64(1_000_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(10_000_000)))),
            4,
            ParamClass::Const,
            true
        ),
        row(
            b"welfare.thC_hi",
            ParamValue::Fixed(FixedU64(950_000_000)),
            ParamValue::Fixed(FixedU64(850_000_000)),
            ParamValue::Fixed(FixedU64(1_000_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(10_000_000)))),
            4,
            ParamClass::Const,
            false
        ),
        row(
            b"welfare.wP",
            ParamValue::Fixed(FixedU64(600_000_000)),
            ParamValue::Fixed(FixedU64(300_000_000)),
            ParamValue::Fixed(FixedU64(700_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(50_000_000)))),
            4,
            ParamClass::Const,
            false
        ),
        row(
            b"welfare.wA",
            ParamValue::Fixed(FixedU64(400_000_000)),
            ParamValue::Fixed(FixedU64(300_000_000)),
            ParamValue::Fixed(FixedU64(700_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(50_000_000)))),
            4,
            ParamClass::Const,
            false
        ),
        row(
            b"orc.bond_floor",
            ParamValue::Balance(10_000_000_000),
            ParamValue::Balance(2_500_000_000),
            ParamValue::Balance(100_000_000_000),
            None,
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"orc.bond_bps",
            ParamValue::Perbill(25_000_000),
            ParamValue::Perbill(15_000_000),
            ParamValue::Perbill(100_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"orc.rounds",
            ParamValue::U8(3),
            ParamValue::U8(2),
            ParamValue::U8(4),
            None,
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"orc.window",
            ParamValue::U32(43_200),
            ParamValue::U32(43_200),
            ParamValue::U32(72_000),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"orc.rep_stake",
            ParamValue::Balance(100_000_000_000),
            ParamValue::Balance(25_000_000_000),
            ParamValue::Balance(500_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"orc.n_min",
            ParamValue::U8(3),
            ParamValue::U8(3),
            ParamValue::U8(16),
            Some(MaxDelta::Absolute(ParamValue::U8(1))),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"wt.quorum",
            ParamValue::U8(2),
            ParamValue::U8(2),
            ParamValue::U8(5),
            Some(MaxDelta::Absolute(ParamValue::U8(1))),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"wt.stake",
            ParamValue::Balance(25_000_000_000),
            ParamValue::Balance(10_000_000_000),
            ParamValue::Balance(100_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"reg.bond_inc",
            ParamValue::Balance(5_000_000_000),
            ParamValue::Balance(2_500_000_000),
            ParamValue::Balance(50_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"reg.bond_mile",
            ParamValue::Balance(2_500_000_000),
            ParamValue::Balance(1_250_000_000),
            ParamValue::Balance(25_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"res.probe_int",
            ParamValue::U32(14_400),
            ParamValue::U32(0),
            ParamValue::U32(u32::MAX),
            None,
            1,
            ParamClass::Param,
            false
        ),
        row(
            b"res.probe_to",
            ParamValue::U32(600),
            ParamValue::U32(0),
            ParamValue::U32(u32::MAX),
            None,
            1,
            ParamClass::Param,
            false
        ),
        row(
            b"res.probe_amount",
            ParamValue::Balance(100_000),
            ParamValue::Balance(0),
            ParamValue::Balance(u128::MAX),
            None,
            1,
            ParamClass::Param,
            false
        ),
        row(
            b"res.fail_thr",
            ParamValue::U8(2),
            ParamValue::U8(0),
            ParamValue::U8(u8::MAX),
            None,
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"res.recover_thr",
            ParamValue::U8(3),
            ParamValue::U8(0),
            ParamValue::U8(u8::MAX),
            None,
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"grd.review_dl",
            ParamValue::U32(2),
            ParamValue::U32(1),
            ParamValue::U32(4),
            Some(MaxDelta::Absolute(ParamValue::U32(1))),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"att.bond",
            ParamValue::Balance(25_000_000_000_000_000),
            ParamValue::Balance(12_500_000_000_000_000),
            ParamValue::Balance(250_000_000_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Entrenched,
            false
        ),
        row(
            b"att.window",
            ParamValue::U32(43_200),
            ParamValue::U32(43_200),
            ParamValue::U32(72_000),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"keeper.budget",
            ParamValue::Balance(12_000_000_000),
            ParamValue::Balance(kernel::KEEPER_BUDGET_EPOCH_FLOOR_USDC),
            ParamValue::Balance(60_000_000_000),
            Some(MaxDelta::Factor(2)),
            1,
            ParamClass::Param,
            true
        ),
        row(
            b"collator.comp",
            ParamValue::Balance(2_000_000_000),
            ParamValue::Balance(500_000_000),
            ParamValue::Balance(10_000_000_000),
            Some(MaxDelta::Factor(2)),
            1,
            ParamClass::Param,
            false
        ),
        row(
            b"collator.n_min",
            ParamValue::U8(4),
            ParamValue::U8(3),
            ParamValue::U8(12),
            Some(MaxDelta::Absolute(ParamValue::U8(1))),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"ledger.min_split",
            ParamValue::Balance(kernel::MIN_SPLIT_USDC),
            ParamValue::Balance(kernel::MIN_SPLIT_USDC),
            ParamValue::Balance(1_000_000),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"ledger.archive",
            ParamValue::U32(5_256_000),
            ParamValue::U32(1_296_000),
            ParamValue::U32(u32::MAX),
            None,
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"ledger.pos_dep",
            ParamValue::Balance(100_000),
            ParamValue::Balance(100_000),
            ParamValue::Balance(1_000_000),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"pol.b_baseline",
            ParamValue::Balance(25_000_000_000),
            ParamValue::Balance(10_000_000_000),
            ParamValue::Balance(100_000_000_000),
            Some(MaxDelta::Percent(25)),
            1,
            ParamClass::Treasury,
            false,
        ),
        row(
            b"collator.n_tgt",
            ParamValue::U8(5),
            ParamValue::U8(4),
            ParamValue::U8(12),
            Some(MaxDelta::Absolute(ParamValue::U8(1))),
            2,
            ParamClass::Meta,
            false,
        ),
        row(
            b"phase3.tvl_cap",
            ParamValue::Balance(2_000_000_000_000),
            ParamValue::Balance(0),
            ParamValue::Balance(u128::MAX),
            None,
            0,
            ParamClass::MetaAndValues,
            false
        ),
        row(
            b"phase3.dep_cap",
            ParamValue::Balance(20_000_000_000),
            ParamValue::Balance(0),
            ParamValue::Balance(u128::MAX),
            None,
            0,
            ParamClass::MetaAndValues,
            false
        ),
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
        state.dispatch_set_release_channel(ConstitutionOrigin::ConstitutionalValues, bytes)
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
            .find(|record| record.key == key16(b"keeper.budget"))
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
            .any(|record| record.key == key16(b"intake.max_acct")));
        assert!(params
            .iter()
            .any(|record| record.key == key16(b"keeper.budget")));
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
        // 09 §5.4: bootstrap sudo's exhaustive power list excludes parameter
        // administration — Root must be refused for every class.
        assert_eq!(
            state.dispatch_set_param(
                ConstitutionOrigin::Root,
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
        // Genesis carries no meters (I-17 envelopes live with their owning
        // pallets); seed the primitive directly for the mechanics test.
        state.meters = alloc::vec![Meter::new(10, 0), Meter::new(0, 0)];
        assert_eq!(
            state.dispatch_charge_meter(ConstitutionOrigin::Signed, 0, 1, 0),
            Err(Error::BadOrigin)
        );
        assert_eq!(
            state.dispatch_charge_meter(ConstitutionOrigin::Root, 0, 1, 0),
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
        // 09 §5.4: bootstrap sudo (Root) is the only origin-mediated flag
        // writer; guardian/playbook/META dispatch is refused (06 §5.2, I-23).
        for refused in [
            ConstitutionOrigin::Signed,
            ConstitutionOrigin::GuardianHold,
            ConstitutionOrigin::EmergencyPlaybook,
            ConstitutionOrigin::FutarchyMeta,
            ConstitutionOrigin::ConstitutionalValues,
        ] {
            assert_eq!(
                state.dispatch_set_phase_flag(refused, PhaseFlags::SUDO_PRESENT, true),
                Err(Error::BadOrigin)
            );
        }
        state
            .dispatch_set_phase_flag(ConstitutionOrigin::Root, PhaseFlags::SUDO_PRESENT, true)
            .unwrap();
        assert!(state.phase_flags.contains(PhaseFlags::SUDO_PRESENT));
        // Outside the armable mask: machinery bits and reserved bits alike.
        assert_eq!(
            state.dispatch_set_phase_flag(
                ConstitutionOrigin::Root,
                PhaseFlags::LEDGER_FROZEN,
                true
            ),
            Err(Error::FlagNotArmable)
        );
        assert_eq!(
            state.dispatch_set_phase_flag(ConstitutionOrigin::Root, 1 << 8, true),
            Err(Error::FlagNotArmable)
        );
        // The raw setter still rejects reserved bits (try-state guard).
        assert_eq!(
            state.phase_flags.set(1 << 8, true),
            Err(Error::ReservedPhaseFlag)
        );
    }

    #[test]
    fn meta_and_values_keys_are_enacted_by_futarchy_meta_only() {
        // 06 §1: values cannot invoke parameter keys; the values half of the
        // META+values dual consent is execute-time ratification (06 §2.2).
        let mut state = ConstitutionState::genesis();
        assert_eq!(
            state.dispatch_set_param(
                ConstitutionOrigin::ConstitutionalValues,
                key16(b"epoch.horizon_k"),
                ParamValue::U8(3),
                4
            ),
            Err(Error::BadOrigin)
        );
        state
            .dispatch_set_param(
                ConstitutionOrigin::FutarchyMeta,
                key16(b"epoch.horizon_k"),
                ParamValue::U8(3),
                4,
            )
            .unwrap();
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
        // 02 §12: flags bits 3–31 are reserved zero — a schema-1 value
        // carrying them must be refused, not published.
        let mut reserved_flag = [0u8; RELEASE_CHANNEL_LEN];
        reserved_flag[0] = 1;
        reserved_flag[164..168].copy_from_slice(&(1u32 << 3).to_le_bytes());
        assert_eq!(
            ReleaseChannel::new(reserved_flag),
            Err(Error::BadReleaseSchema)
        );
        let mut all_defined_flags = [0u8; RELEASE_CHANNEL_LEN];
        all_defined_flags[0] = 1;
        all_defined_flags[164..168].copy_from_slice(&ReleaseChannel::FLAGS_MASK.to_le_bytes());
        assert!(ReleaseChannel::new(all_defined_flags).is_ok());
        let mut state = ConstitutionState::genesis();
        assert_eq!(
            state.dispatch_set_release_channel(ConstitutionOrigin::Signed, bad),
            Err(Error::BadOrigin)
        );
        let mut good = [0u8; RELEASE_CHANNEL_LEN];
        good[0] = 1;
        // 02 §12: exhaustive writer list — ConstitutionalValues is the only
        // origin-mediated writer; CODE/META/Root paths must all be refused.
        for refused in [
            ConstitutionOrigin::FutarchyCode,
            ConstitutionOrigin::FutarchyMeta,
            ConstitutionOrigin::Root,
        ] {
            assert_eq!(
                state.dispatch_set_release_channel(refused, good),
                Err(Error::BadOrigin)
            );
        }
        state
            .dispatch_set_release_channel(ConstitutionOrigin::ConstitutionalValues, good)
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
        // 06 §3.2 row 4: values participates via ratification, not dispatch;
        // Root is outside the 09 §5.4 sudo scope.
        assert_eq!(
            state.dispatch_set_capability(ConstitutionOrigin::ConstitutionalValues, cap),
            Err(Error::BadOrigin)
        );
        assert_eq!(
            state.dispatch_set_capability(ConstitutionOrigin::Root, cap),
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
        bad_meter.meters.push(Meter::new(5, 0));
        bad_meter.meters[0].spent = bad_meter.meters[0].limit + 1;
        assert_eq!(bad_meter.try_state(), Err(Error::MeterExhausted));
    }
}
