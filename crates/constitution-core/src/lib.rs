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
pub const RELEASE_CHANNEL_UPDATED_AT: core::ops::Range<usize> = 108..112;
pub const RELEASE_CHANNEL_SPEC_VERSION: core::ops::Range<usize> = 112..116;
pub const RELEASE_CHANNEL_PENDING_AUTHORIZED_AT: core::ops::Range<usize> = 116..120;
pub const RELEASE_CHANNEL_FLAGS: core::ops::Range<usize> = 164..168;
pub const RELEASE_CHANNEL_FLAG_URGENT_UPGRADE: u32 = 1 << 2;
pub const MAX_PARAMS: usize = 128;
pub const MAX_CAPABILITIES: usize = 64;
pub const MAX_METERS: usize = 16;
/// 13 rule 7 meta-bound: `amend_registry` may not set a cooldown above this.
pub const META_MAX_COOLDOWN_EPOCHS: u32 = 8;
/// Genesis defaults for the B10 POL budget reader, ordered PARAM/TREASURY/
/// CODE/META as frozen by 02 §9. These remain tunable at runtime; the named
/// constants are only the fail-closed terminal fallback when both live and
/// genesis registry records are absent or kind-mismatched.
pub const POL_B_DEFAULTS: [Balance; 4] = [
    10_000_000_000,
    25_000_000_000,
    60_000_000_000,
    100_000_000_000,
];
pub const POL_GATE_B_DEFAULT: Balance = 7_500_000_000;
pub const POL_BUDGET_EPOCH_DEFAULT_PPB: u32 = 7_500_000;

pub fn gate_v_min_pair(key: ParamKey) -> Option<ParamKey> {
    for (decision, gate) in [
        (
            b"dec.v_min.param".as_slice(),
            b"gate.v_min.param".as_slice(),
        ),
        (b"dec.v_min.trs".as_slice(), b"gate.v_min.trs".as_slice()),
        (b"dec.v_min.code".as_slice(), b"gate.v_min.code".as_slice()),
        (b"dec.v_min.meta".as_slice(), b"gate.v_min.meta".as_slice()),
    ] {
        let decision = key16(decision);
        let gate = key16(gate);
        if key == decision {
            return Some(gate);
        }
        if key == gate {
            return Some(decision);
        }
    }
    None
}

pub const fn gate_v_min_coupled(decision: Balance, gate: Balance) -> bool {
    gate >= decision / 20 && gate <= decision / 2
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
    /// Para-block at which `value` last changed. Genesis rows use zero; this
    /// is the source of `ParamView.last_change` (02 §4, 13 reading rule 2).
    pub last_change_block: BlockNumber,
    pub class: ParamClass,
    /// 13 rule 7: bounds carry a kernel floor/ceiling and are genesis-fixed —
    /// `amend_registry` cannot move them.
    pub kernel_bounded: bool,
}

impl ParamRecord {
    /// Conservative absolute step represented by this record's current
    /// max-delta rule in both directions. The v6 `ParamView.max_delta`
    /// projection is deliberately lossy for factor rules and therefore uses
    /// the smaller allowance; [`Self::admissible_next_interval`] exposes the
    /// exact inclusive interval beside it.
    pub fn max_delta_allowance(&self) -> Result<u128, Error> {
        match self.max_delta {
            None => Ok(0),
            Some(MaxDelta::Absolute(bound)) => {
                ensure!(bound.same_kind(self.value), Error::WrongType);
                Ok(bound.as_u128())
            }
            Some(MaxDelta::Percent(percent)) => {
                let value = self.value.as_u128();
                let scaled = match value.checked_mul(u128::from(percent)) {
                    Some(scaled) => scaled,
                    None => u128::MAX,
                };
                // Unlike Factor, checked_update applies this same absolute,
                // rounded-down allowance to increases and decreases, so the
                // Percent rule itself has no directional asymmetry to project.
                Ok(scaled / 100)
            }
            Some(MaxDelta::Factor(factor)) => {
                let factor = u128::from(factor);
                ensure!(factor >= 1, Error::MetaBoundViolation);
                let value = self.value.as_u128();
                let upper = match value.checked_mul(factor) {
                    Some(upper) => upper,
                    None => u128::MAX,
                };
                let upward = upper.checked_sub(value).map_or(0, |allowance| allowance);
                let lower_floor = value / factor;
                let lower = match lower_floor.checked_add(u128::from(value % factor != 0)) {
                    Some(lower) => lower,
                    None => value,
                };
                let downward = value.checked_sub(lower).map_or(0, |allowance| allowance);
                Ok(upward.min(downward))
            }
        }
    }

    /// Exact inclusive interval admitted by the current record bounds and
    /// max-delta rule. Arithmetic mirrors [`Self::checked_update`]: percent
    /// allowances floor, factor lower bounds use `ceil(value / factor)`, and
    /// upward arithmetic saturates.
    pub fn admissible_next_interval(&self) -> Result<(u128, u128), Error> {
        ensure!(
            self.value.same_kind(self.min) && self.value.same_kind(self.max),
            Error::WrongType
        );
        let value = self.value.as_u128();
        let record_min = self.min.as_u128();
        let record_max = self.max.as_u128();
        ensure!(record_min <= record_max, Error::MetaBoundViolation);

        let (delta_min, delta_max) = match self.max_delta {
            None => (record_min, record_max),
            Some(MaxDelta::Absolute(bound)) => {
                ensure!(bound.same_kind(self.value), Error::WrongType);
                let allowance = bound.as_u128();
                (
                    value.saturating_sub(allowance),
                    value.saturating_add(allowance),
                )
            }
            Some(MaxDelta::Percent(_)) => {
                let allowance = self.max_delta_allowance()?;
                (
                    value.saturating_sub(allowance),
                    value.saturating_add(allowance),
                )
            }
            Some(MaxDelta::Factor(factor)) => {
                let factor = u128::from(factor);
                ensure!(factor >= 1, Error::MetaBoundViolation);
                let quotient = value / factor;
                let lower = quotient.saturating_add(u128::from(value % factor != 0));
                (lower, value.saturating_mul(factor))
            }
        };

        let min_next = record_min.max(delta_min);
        let max_next = record_max.min(delta_max);
        ensure!(min_next <= max_next, Error::MetaBoundViolation);
        Ok((min_next, max_next))
    }

    pub fn checked_update(
        &self,
        next: ParamValue,
        epoch: u32,
        block: BlockNumber,
    ) -> Result<Self, Error> {
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
            Some(MaxDelta::Percent(_)) => {
                // Allowance is recomputed from the current value on every
                // decision; flooring keeps the limit conservative.
                let allowed = self.max_delta_allowance()?;
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
            last_change_block: block,
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
    /// Move protocol INSURANCE custody back into MAIN without granting the
    /// broader treasury outflow surface (08 §1.2/§1.4; SQ-384).
    ///
    /// Appended deliberately: Capability is SCALE-encoded in stored records
    /// and resource keys, so the pre-existing discriminants are immutable.
    InsuranceSweep,
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
        le_u32_at(&self.bytes, RELEASE_CHANNEL_UPDATED_AT.start)
    }
    pub fn spec_version(&self) -> u32 {
        le_u32_at(&self.bytes, RELEASE_CHANNEL_SPEC_VERSION.start)
    }
    pub fn pending_authorized_at(&self) -> u32 {
        le_u32_at(&self.bytes, RELEASE_CHANNEL_PENDING_AUTHORIZED_AT.start)
    }
    pub fn flags(&self) -> u32 {
        le_u32_at(&self.bytes, RELEASE_CHANNEL_FLAGS.start)
    }

    /// Apply a 02 §12 writer-(b) update without allowing it to overwrite the
    /// execution guard's exclusive fields. The caller owns the release
    /// descriptor, minimum-version/key-revocation tail and flag bits 0–1;
    /// offsets 112–119 and flag bit 2 remain byte-for-byte guard-owned.
    /// `updated_at` is supplied by the dispatch path, never by the caller's
    /// bytes: 02 §12 makes offset 108 the block of the last write, and a
    /// caller-chosen value would let a lawful writer backdate or future-date
    /// the freshness a stranded reader depends on.
    pub fn merge_writer_b(
        &self,
        bytes: [u8; RELEASE_CHANNEL_LEN],
        updated_at: u32,
    ) -> Result<Self, Error> {
        let caller = Self::new(bytes)?;
        let mut merged = caller.bytes;
        merged[RELEASE_CHANNEL_UPDATED_AT].copy_from_slice(&updated_at.to_le_bytes());
        merged[RELEASE_CHANNEL_SPEC_VERSION.start..RELEASE_CHANNEL_PENDING_AUTHORIZED_AT.end]
            .copy_from_slice(
                &self.bytes
                    [RELEASE_CHANNEL_SPEC_VERSION.start..RELEASE_CHANNEL_PENDING_AUTHORIZED_AT.end],
            );
        let flags = (caller.flags() & !RELEASE_CHANNEL_FLAG_URGENT_UPGRADE)
            | (self.flags() & RELEASE_CHANNEL_FLAG_URGENT_UPGRADE);
        merged[RELEASE_CHANNEL_FLAGS].copy_from_slice(&flags.to_le_bytes());
        Self::new(merged)
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum ConstitutionOrigin {
    FutarchyParam,
    FutarchyTreasury,
    FutarchyCode,
    FutarchyMeta,
    ConstitutionTrack,
    EntrenchedTrack,
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
    /// CONST/entrenched-class keys are values-layer business (06 §1, §2.4),
    /// with the direction-scoped welfare-knee exception enforced by
    /// [`authorize_param_update`].
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
                | (Self::ConstitutionTrack, ParamClass::Const)
                | (Self::EntrenchedTrack, ParamClass::Entrenched)
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
    /// 06 §3.2 row 4 / 13 rule 7 (SQ-150 ruling 2026-07-21): registry
    /// amendments are **`FutarchyMeta`-only**. Non-kernel rows are amended by
    /// META within the compile-time meta-bounds; kernel-bounded rows are
    /// **immutable** — [`ParamRecord::checked_amend`] refuses them with
    /// `KernelBoundImmutable` even for the one origin that clears this gate, so
    /// no origin can move a kernel floor/ceiling. The former dual-authority
    /// reading (a `ConstitutionalValues`/`constitution`-track path, 06 §2.1 as
    /// superseded) is removed: it let a values referendum retune META metadata
    /// while the classifier simultaneously projected the same call as
    /// FutarchyMeta (the I-8 crossing S5 pinned), and minimising the authority
    /// cannot weaken a defence through an ambiguous values path (R-7).
    ///
    /// The `class` argument is retained for the authority-matrix signature and
    /// for future per-class scoping; the resolved policy does not branch on it.
    pub const fn can_amend_registry(self, _class: ParamClass) -> bool {
        matches!(self, Self::FutarchyMeta)
    }
    /// 02 §12: the release channel's writers are exhaustive — (a) the execution
    /// guard's runtime-internal path (not origin-mediated) and (b) the scoped
    /// constitution track (or its internal bare `ConstitutionalValues` form)
    /// via `constitution.set_release_channel`.
    /// "No other origin can write it" — including bootstrap Root/sudo.
    pub const fn can_set_release_channel(self) -> bool {
        matches!(self, Self::ConstitutionTrack | Self::ConstitutionalValues)
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

/// Direction-aware authorization for `constitution.set_param`.
///
/// Most rows are authorized solely by [`ParamClass`]. One 13 §1 family is
/// stricter:
///
/// - the welfare low knees tighten through the constitution track and may be
///   un-tightened only through the entrenched track (05 §4.1; 06 §2.1);
///
/// Equality retains the welfare rows' CONST-class route (the constitution
/// track). The record's normal bounds, delta and cooldown checks still run
/// afterward.
pub fn authorize_param_update(
    origin: ConstitutionOrigin,
    record: &ParamRecord,
    next: ParamValue,
) -> Result<(), Error> {
    let welfare_low_knee =
        record.key == key16(b"welfare.thS_lo") || record.key == key16(b"welfare.thC_lo");
    if welfare_low_knee {
        ensure!(
            matches!(
                origin,
                ConstitutionOrigin::ConstitutionTrack | ConstitutionOrigin::EntrenchedTrack
            ),
            Error::BadOrigin
        );
        ensure!(record.value.same_kind(next), Error::WrongType);
        let current = record.value.as_u128();
        let proposed = next.as_u128();
        ensure!(
            (proposed >= current && matches!(origin, ConstitutionOrigin::ConstitutionTrack))
                || (proposed < current && matches!(origin, ConstitutionOrigin::EntrenchedTrack)),
            Error::BadOrigin
        );
        return Ok(());
    }

    ensure!(origin.can_set_param(record.class), Error::BadOrigin);
    Ok(())
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

    pub fn set_param(
        &mut self,
        key: ParamKey,
        next: ParamValue,
        epoch: u32,
        block: BlockNumber,
    ) -> Result<(), Error> {
        let index = self
            .params
            .iter()
            .position(|r| r.key == key)
            .ok_or(Error::UnknownParam)?;
        let updated = self.params[index].checked_update(next, epoch, block)?;
        if let Some(pair) = gate_v_min_pair(key) {
            let paired = self
                .params
                .iter()
                .find(|record| record.key == pair)
                .ok_or(Error::TryStateViolation)?;
            let (decision, gate) = if key.as_slice().starts_with(b"dec.") {
                (updated.value, paired.value)
            } else {
                (paired.value, updated.value)
            };
            match (decision, gate) {
                (ParamValue::Balance(decision), ParamValue::Balance(gate)) => {
                    ensure!(
                        gate_v_min_coupled(decision, gate),
                        Error::MetaBoundViolation
                    );
                }
                _ => return Err(Error::WrongType),
            }
        }
        self.params[index] = updated;
        Ok(())
    }

    pub fn dispatch_set_param(
        &mut self,
        origin: ConstitutionOrigin,
        key: ParamKey,
        next: ParamValue,
        epoch: u32,
        block: BlockNumber,
    ) -> Result<(), Error> {
        let record = self
            .params
            .iter()
            .find(|r| r.key == key)
            .ok_or(Error::UnknownParam)?;
        authorize_param_update(origin, record, next)?;
        self.set_param(key, next, epoch, block)
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
        let record = self
            .params
            .iter_mut()
            .find(|r| r.key == key)
            .ok_or(Error::UnknownParam)?;
        ensure!(origin.can_amend_registry(record.class), Error::BadOrigin);
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

    /// `updated_at` is supplied by the caller's dispatch context (the current
    /// block), never read out of `bytes` — see [`ReleaseChannelRecord::merge_writer_b`].
    pub fn dispatch_set_release_channel(
        &mut self,
        origin: ConstitutionOrigin,
        bytes: [u8; RELEASE_CHANNEL_LEN],
        updated_at: u32,
    ) -> Result<(), Error> {
        ensure!(origin.can_set_release_channel(), Error::BadOrigin);
        self.release_channel = self.release_channel.merge_writer_b(bytes, updated_at)?;
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
        for decision_key in [
            b"dec.v_min.param".as_slice(),
            b"dec.v_min.trs".as_slice(),
            b"dec.v_min.code".as_slice(),
            b"dec.v_min.meta".as_slice(),
        ] {
            let decision = self
                .params
                .iter()
                .find(|record| record.key == key16(decision_key))
                .ok_or(Error::TryStateViolation)?;
            let gate_key = gate_v_min_pair(decision.key).ok_or(Error::TryStateViolation)?;
            let gate = self
                .params
                .iter()
                .find(|record| record.key == gate_key)
                .ok_or(Error::TryStateViolation)?;
            match (decision.value, gate.value) {
                (ParamValue::Balance(decision), ParamValue::Balance(gate)) => {
                    ensure!(gate_v_min_coupled(decision, gate), Error::TryStateViolation);
                }
                _ => return Err(Error::WrongType),
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
        CapabilityRecord {
            class: ProposalClass::Treasury,
            capability: Capability::InsuranceSweep,
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
/// `collator.bond` and the remaining `sec.*`/`ops.*` rows (uncalibrated).
/// The three calibrated `ops.ct_*` Coretime controls are explicit exceptions.
/// `gate.v_min` and
/// `dis.merit_min` carry derived defaults and bind at their consuming
/// engines. Kernel-bounded flags follow the enumeration in 13 rule 7.
///
/// Epoch-timing genesis seeds (`epoch.length`, `dec.window`, `dec.trailing`) are
/// read through [`timing_defaults`] so the default-off `fast-timing` build
/// (SQ-128, G1 drill 09) can seed a compressed epoch clock derived from
/// `kernel::FAST_DAY_BLOCKS`. The `cfg(not(fast-timing))` arm below carries the
/// exact frozen 13 §1 values, so the release registry — and the fixture that
/// byte-asserts it (`tools/limit-coverage/genesis-keys.json`, the constitution
/// genesis test) — is unchanged. The compressed arm keeps the relationships
/// `EpochParams::validate` enforces (D3 `dec.window <= epoch·13/21`, D4
/// `trailing <= window`); every other duration Param stays at its frozen value so
/// the emergency/execution/oracle windows can never fire inside a minute-scale
/// drill.
#[cfg(not(feature = "fast-timing"))]
mod timing_defaults {
    pub const EPOCH_LENGTH: u32 = 302_400;
    pub const EPOCH_LENGTH_MAX: u32 = 604_800;
    pub const DEC_WINDOW: u32 = 43_200;
    pub const DEC_WINDOW_MAX: u32 = 86_400;
    pub const DEC_TRAILING: u32 = 14_400;
    pub const DEC_TRAILING_MIN: u32 = 3_600;
    pub const DEC_TRAILING_MAX: u32 = 28_800;
}
#[cfg(feature = "fast-timing")]
mod timing_defaults {
    use super::kernel::FAST_DAY_BLOCKS as DAY;
    /// 21 · FAST_DAY (matches the 21-phase-unit epoch); clears the compressed
    /// `MIN_EPOCH_LENGTH_BLOCKS` (14 · FAST_DAY) and stays a multiple of 21.
    pub const EPOCH_LENGTH: u32 = 21 * DAY;
    pub const EPOCH_LENGTH_MAX: u32 = 42 * DAY;
    /// 3 · FAST_DAY (72 h); `<= EPOCH_LENGTH · 13/21` holds (3 <= 13).
    pub const DEC_WINDOW: u32 = 3 * DAY;
    pub const DEC_WINDOW_MAX: u32 = 6 * DAY;
    /// 1 · FAST_DAY (24 h) trailing window; `<= DEC_WINDOW` holds.
    pub const DEC_TRAILING: u32 = DAY;
    pub const DEC_TRAILING_MIN: u32 = DAY / 4;
    pub const DEC_TRAILING_MAX: u32 = 2 * DAY;
}

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
            last_change_block: 0,
            class,
            kernel_bounded,
        }
    }
    alloc::vec![
        row(
            b"epoch.length",
            ParamValue::U32(timing_defaults::EPOCH_LENGTH),
            ParamValue::U32(kernel::MIN_EPOCH_LENGTH_BLOCKS),
            ParamValue::U32(timing_defaults::EPOCH_LENGTH_MAX),
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
            ParamValue::U32(timing_defaults::DEC_WINDOW),
            ParamValue::U32(kernel::DECISION_WINDOW_FLOOR_BLOCKS),
            ParamValue::U32(timing_defaults::DEC_WINDOW_MAX),
            Some(MaxDelta::Percent(20)),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.trailing",
            ParamValue::U32(timing_defaults::DEC_TRAILING),
            ParamValue::U32(timing_defaults::DEC_TRAILING_MIN),
            ParamValue::U32(timing_defaults::DEC_TRAILING_MAX),
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
            ParamValue::Fixed(kernel::GATE_EPS_FLOOR),
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
            ParamValue::U32(kernel::EXECUTION_GRACE_FLOOR_BLOCKS),
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
            ParamValue::Fixed(FixedU64(37_500_000)),
            ParamValue::Fixed(kernel::DECISION_DELTA_FLOOR),
            ParamValue::Fixed(FixedU64(100_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(5_000_000)))),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.delta.trs",
            ParamValue::Fixed(FixedU64(37_500_000)),
            ParamValue::Fixed(kernel::DECISION_DELTA_FLOOR),
            ParamValue::Fixed(FixedU64(100_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(5_000_000)))),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.delta.code",
            ParamValue::Fixed(FixedU64(60_000_000)),
            ParamValue::Fixed(kernel::DECISION_DELTA_FLOOR),
            ParamValue::Fixed(FixedU64(100_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(5_000_000)))),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.delta.meta",
            ParamValue::Fixed(FixedU64(90_000_000)),
            ParamValue::Fixed(kernel::DECISION_DELTA_FLOOR),
            ParamValue::Fixed(FixedU64(100_000_000)),
            Some(MaxDelta::Absolute(ParamValue::Fixed(FixedU64(5_000_000)))),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.sigma.param",
            ParamValue::Fixed(FixedU64(3_000_000)),
            ParamValue::Fixed(kernel::DECISION_SIGMA_FLOOR),
            ParamValue::Fixed(FixedU64(50_000_000)),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.sigma.trs",
            ParamValue::Fixed(FixedU64(5_000_000)),
            ParamValue::Fixed(kernel::DECISION_SIGMA_FLOOR),
            ParamValue::Fixed(FixedU64(50_000_000)),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.sigma.code",
            ParamValue::Fixed(FixedU64(8_000_000)),
            ParamValue::Fixed(kernel::DECISION_SIGMA_FLOOR),
            ParamValue::Fixed(FixedU64(50_000_000)),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"dec.sigma.meta",
            ParamValue::Fixed(FixedU64(10_000_000)),
            ParamValue::Fixed(kernel::DECISION_SIGMA_FLOOR),
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
            b"gate.v_min.param",
            ParamValue::Balance(10_000_000_000),
            ParamValue::Balance(5_000_000_000),
            ParamValue::Balance(50_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"gate.v_min.trs",
            ParamValue::Balance(25_000_000_000),
            ParamValue::Balance(12_500_000_000),
            ParamValue::Balance(125_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"gate.v_min.code",
            ParamValue::Balance(60_000_000_000),
            ParamValue::Balance(30_000_000_000),
            ParamValue::Balance(300_000_000_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"gate.v_min.meta",
            ParamValue::Balance(120_000_000_000),
            ParamValue::Balance(60_000_000_000),
            ParamValue::Balance(600_000_000_000),
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
            ParamValue::U32(kernel::EXECUTION_TIMELOCK_FLOOR_BLOCKS),
            ParamValue::U32(432_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"exec.lock.trs",
            ParamValue::U32(43_200),
            ParamValue::U32(kernel::EXECUTION_TIMELOCK_FLOOR_BLOCKS),
            ParamValue::U32(432_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"exec.lock.code",
            ParamValue::U32(100_800),
            ParamValue::U32(kernel::EXECUTION_TIMELOCK_FLOOR_BLOCKS),
            ParamValue::U32(432_000),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"exec.lock.meta",
            ParamValue::U32(201_600),
            ParamValue::U32(kernel::EXECUTION_TIMELOCK_FLOOR_BLOCKS),
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
            ParamValue::Balance(POL_B_DEFAULTS[0]),
            ParamValue::Balance(0),
            ParamValue::Balance(u128::MAX),
            Some(MaxDelta::Percent(25)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            b"pol.b.trs",
            ParamValue::Balance(POL_B_DEFAULTS[1]),
            ParamValue::Balance(0),
            ParamValue::Balance(u128::MAX),
            Some(MaxDelta::Percent(25)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            b"pol.b.code",
            ParamValue::Balance(POL_B_DEFAULTS[2]),
            ParamValue::Balance(0),
            ParamValue::Balance(u128::MAX),
            Some(MaxDelta::Percent(25)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            b"pol.b.meta",
            ParamValue::Balance(POL_B_DEFAULTS[3]),
            ParamValue::Balance(0),
            ParamValue::Balance(u128::MAX),
            Some(MaxDelta::Percent(25)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            b"pol.b_gate",
            ParamValue::Balance(POL_GATE_B_DEFAULT),
            ParamValue::Balance(0),
            ParamValue::Balance(u128::MAX),
            Some(MaxDelta::Percent(25)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            b"pol.budget_epoch",
            ParamValue::Perbill(POL_BUDGET_EPOCH_DEFAULT_PPB),
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
            ParamValue::U8(kernel::ORC_ROUNDS_MIN),
            ParamValue::U8(kernel::ORC_ROUNDS_MAX),
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
            ParamValue::U32(1),
            ParamValue::U32(u32::MAX),
            None,
            1,
            ParamClass::Param,
            true
        ),
        row(
            b"res.probe_to",
            ParamValue::U32(600),
            ParamValue::U32(1),
            ParamValue::U32(u32::MAX),
            None,
            1,
            ParamClass::Param,
            true
        ),
        row(
            b"res.probe_amount",
            ParamValue::Balance(100_000),
            ParamValue::Balance(1),
            ParamValue::Balance(u128::MAX),
            None,
            1,
            ParamClass::Param,
            true
        ),
        row(
            b"res.fail_thr",
            ParamValue::U8(2),
            ParamValue::U8(1),
            ParamValue::U8(u8::MAX),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            b"res.recover_thr",
            ParamValue::U8(3),
            ParamValue::U8(1),
            ParamValue::U8(u8::MAX),
            None,
            2,
            ParamClass::Meta,
            true
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
            // SQ-117 (ruled 2026-07-21): genesis-seeded from the 08 §6.2 crank-
            // fee basis so B9's rebate pipeline stops paying zero. Default 3×,
            // hard min 1×, hard max 10× the SAME basis (13 §1), so the whole row
            // scales with one number. The basis is the [VERIFY] placeholder of
            // 08 §6.2 (0.03 USDC); the seed is replaced — rounded DOWN against
            // the claimant (R-7) — once launch `fee.vit_usdc_rate` fixes it.
            b"keeper.rebate",
            ParamValue::Balance(kernel::KEEPER_REBATE_FEE_BASIS_USDC.saturating_mul(3)),
            ParamValue::Balance(kernel::KEEPER_REBATE_FEE_BASIS_USDC),
            ParamValue::Balance(kernel::KEEPER_REBATE_FEE_BASIS_USDC.saturating_mul(10)),
            None,
            1,
            ParamClass::Param,
            false
        ),
        row(
            // SQ-158 (owner A13): a distinct 13 §1 key so the values layer can
            // raise the ProcessHold merit floor independently of B_1 (07 §12,
            // default equality). Floor `orc.bond_floor`, ceiling `Balance::MAX`,
            // factor-2 step, 2-epoch cooldown, META. The consumer composes
            // `max(live key, frozen B_1)` so a lowering can never make
            // censorship cheaper than the game's own round-1 bond (R-7).
            b"dis.merit_min",
            ParamValue::Balance(10_000_000_000),
            ParamValue::Balance(10_000_000_000),
            ParamValue::Balance(u128::MAX),
            Some(MaxDelta::Factor(2)),
            2,
            ParamClass::Meta,
            false
        ),
        row(
            b"ops.ct_dot_rate",
            ParamValue::Balance(5_000_000),
            ParamValue::Balance(500_000),
            ParamValue::Balance(500_000_000),
            Some(MaxDelta::Factor(2)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            b"ops.ct_fee_dot",
            ParamValue::Balance(5_000_000_000),
            ParamValue::Balance(100_000_000),
            ParamValue::Balance(100_000_000_000),
            Some(MaxDelta::Factor(2)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            // SQ-114: bounded DOT held by one reserve probe for Asset Hub
            // execution + response delivery. The launch placeholder inherits
            // the already-conservative two-leg Coretime envelope and retains
            // its [VERIFY] status pending live Asset Hub fee calibration.
            b"ops.probe_fee",
            ParamValue::Balance(5_000_000_000),
            ParamValue::Balance(100_000_000),
            ParamValue::Balance(100_000_000_000),
            Some(MaxDelta::Factor(2)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            // Dedicated reserve-probe DOT→USDC accounting rate. It starts at
            // the same conservative placeholder as Coretime but remains an
            // independently governed key so repricing one maintenance route
            // cannot silently resize the other (SQ-114).
            b"ops.probe_rate",
            ParamValue::Balance(5_000_000),
            ParamValue::Balance(500_000),
            ParamValue::Balance(500_000_000),
            Some(MaxDelta::Factor(2)),
            1,
            ParamClass::Treasury,
            false
        ),
        row(
            b"ops.ct_quote_ttl",
            ParamValue::U32(100_800),
            ParamValue::U32(7_200),
            ParamValue::U32(403_200),
            Some(MaxDelta::Factor(2)),
            1,
            ParamClass::Treasury,
            false
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
            ParamValue::U32(kernel::MAX_ARCHIVE_DELAY_BLOCKS),
            ParamValue::U32(1_296_000),
            ParamValue::U32(kernel::MAX_ARCHIVE_DELAY_BLOCKS),
            None,
            2,
            ParamClass::Meta,
            true
        ),
        row(
            // SQ-36 (ruled 2026-07-21): frozen key — max == min == default.
            // The ledger charges/refunds/reconciles DepositsHeld at the LIVE
            // unit and 03 §10 gives no hook to rebase held deposits, so a raise
            // would over-refund old entries out of pooled collateral (L-2/L-6).
            // Per-entry vintages (the only tunable-preserving design) need an
            // unbounded migration and are refused (R-7); see 13 §2 freeze note.
            b"ledger.pos_dep",
            ParamValue::Balance(100_000),
            ParamValue::Balance(100_000),
            ParamValue::Balance(100_000),
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
        row(
            b"xcm.dot_per_sec",
            ParamValue::Balance(100_000_000_000),
            ParamValue::Balance(1_000_000_000),
            ParamValue::Balance(10_000_000_000_000),
            Some(MaxDelta::Factor(2)),
            1,
            ParamClass::Param,
            false
        ),
        row(
            b"xcm.dot_per_mb",
            ParamValue::Balance(10_000_000_000),
            ParamValue::Balance(100_000_000),
            ParamValue::Balance(1_000_000_000_000),
            Some(MaxDelta::Factor(2)),
            1,
            ParamClass::Param,
            false
        ),
        row(
            b"xcm.usdc_per_sec",
            ParamValue::Balance(50_000_000),
            ParamValue::Balance(500_000),
            ParamValue::Balance(5_000_000_000),
            Some(MaxDelta::Factor(2)),
            1,
            ParamClass::Param,
            false
        ),
        row(
            b"xcm.usdc_per_mb",
            ParamValue::Balance(5_000_000),
            ParamValue::Balance(50_000),
            ParamValue::Balance(500_000_000),
            Some(MaxDelta::Factor(2)),
            1,
            ParamClass::Param,
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
            1,
        )
    }

    pub fn benchmark_set_release_channel() -> Result<(), Error> {
        let mut state = ConstitutionState::genesis();
        let mut bytes = [0u8; RELEASE_CHANNEL_LEN];
        bytes[0] = 1;
        state.dispatch_set_release_channel(ConstitutionOrigin::ConstitutionalValues, bytes, 7)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_scale_discriminants_are_append_only() {
        // Capability values are embedded in stored records and the 0x02
        // resource-key discriminator. Existing values must retain their
        // SCALE tags when a new authority is introduced.
        assert_eq!(Capability::OracleConfig.encode(), vec![6]);
        assert_eq!(Capability::MarketTemplate.encode(), vec![7]);
        assert_eq!(Capability::InsuranceSweep.encode(), vec![8]);
    }

    #[test]
    fn param_record_fields_match_contract_02_section_7_3() {
        use scale_info::TypeDef;
        // 02 §7.3 freezes `Params: map ParamKey -> ParamRecord` as a surface the
        // frontend reads directly, and the release manifest freezes this value's
        // rendered SCALE layout. Adding `last_change_block` (contract v4, so
        // `ParamView.last_change` can be a real block number) silently changed
        // that layout and only the release gate would have caught it. Lock the
        // field names and SCALE order so the surface cannot drift unnoticed
        // again: a change here MUST be a deliberate contract revision that also
        // re-freezes `storage.constitution.params` in the surface manifest.
        const CONTRACT_FIELDS: [&str; 9] = [
            "key",
            "value",
            "min",
            "max",
            "max_delta",
            "cooldown_epochs",
            "last_changed_epoch",
            "last_change_block",
            "class",
        ];
        let type_info = ParamRecord::type_info();
        let names: Vec<&str> = match &type_info.type_def {
            TypeDef::Composite(c) => c.fields.iter().filter_map(|f| f.name).collect(),
            _ => panic!("ParamRecord must encode as a SCALE composite type"),
        };
        assert_eq!(&names[..CONTRACT_FIELDS.len()], &CONTRACT_FIELDS);
        // `kernel_bounded` (13 rule 7) trails the contract-visible prefix.
        assert_eq!(names.last(), Some(&"kernel_bounded"));
    }

    fn release_channel() -> ReleaseChannel {
        let mut bytes = [0u8; RELEASE_CHANNEL_LEN];
        bytes[0] = 1;
        bytes[108..112].copy_from_slice(&42u32.to_le_bytes());
        bytes[112..116].copy_from_slice(&7u32.to_le_bytes());
        bytes[116..120].copy_from_slice(&11u32.to_le_bytes());
        bytes[164..168].copy_from_slice(&5u32.to_le_bytes());
        ReleaseChannel::new(bytes).unwrap()
    }

    fn value_from_raw(kind: ParamValue, raw: u128) -> Option<ParamValue> {
        match kind {
            ParamValue::U8(_) => u8::try_from(raw).ok().map(ParamValue::U8),
            ParamValue::U32(_) => u32::try_from(raw).ok().map(ParamValue::U32),
            ParamValue::Balance(_) => Some(ParamValue::Balance(raw)),
            ParamValue::Fixed(_) => u64::try_from(raw)
                .ok()
                .map(|value| ParamValue::Fixed(FixedU64(value))),
            ParamValue::Percent(_) => u8::try_from(raw).ok().map(ParamValue::Percent),
            ParamValue::Perbill(_) => u32::try_from(raw).ok().map(ParamValue::Perbill),
        }
    }

    #[test]
    fn reexports_kernel_and_contract_version() {
        assert_eq!(
            CONTRACT_VERSION,
            futarchy_primitives::INTEGRATION_CONTRACT_VERSION
        );
        // Release-invariance pin; under `fast-timing` this kernel value is compressed
        // (SQ-128) and the canonical frozen-value guard lives in `futarchy-primitives`.
        #[cfg(not(feature = "fast-timing"))]
        assert_eq!(kernel::DESCRIPTOR_LEAD_TIME_BLOCKS, 43_200);
    }

    // epoch.length-specific 13 §1 admission boundaries (BelowMin/DeltaTooLarge/cooldown)
    // verified at production magnitudes; the fast-timing build compresses those bounds
    // (SQ-128), so this production-boundary case runs in the default build only.
    #[cfg(not(feature = "fast-timing"))]
    #[test]
    fn param_update_enforces_bounds_delta_and_cooldown() {
        let mut rec = genesis_params()[0];
        assert_eq!(
            rec.checked_update(ParamValue::U32(200_000), 3, 30),
            Err(Error::BelowMin)
        );
        assert_eq!(
            rec.checked_update(ParamValue::U32(400_000), 3, 30),
            Err(Error::DeltaTooLarge)
        );
        assert_eq!(
            rec.checked_update(ParamValue::U32(310_000), 1, 10),
            Err(Error::CooldownActive)
        );
        rec = rec.checked_update(ParamValue::U32(310_000), 2, 20).unwrap();
        assert_eq!(rec.value, ParamValue::U32(310_000));
        assert_eq!(rec.last_change_block, 20);
    }

    // epoch.length percent-delta recomputation at production magnitudes (13 §1); the
    // machinery is also covered timing-agnostically by `factor_delta_bounds_both_directions`,
    // so under the compressed fast-timing build this production case runs default-only.
    #[cfg(not(feature = "fast-timing"))]
    #[test]
    fn percent_delta_is_recomputed_from_the_current_value() {
        // 13 §1: epoch.length Max Δ/decision = 10%. A fixed absolute step
        // would let a lowered value be raised by more than 10% per decision.
        let mut rec = genesis_params()[0];
        assert_eq!(rec.max_delta, Some(MaxDelta::Percent(10)));
        rec = rec.checked_update(ParamValue::U32(275_000), 2, 20).unwrap();
        rec = rec.checked_update(ParamValue::U32(250_000), 4, 40).unwrap();
        rec = rec.checked_update(ParamValue::U32(226_000), 6, 60).unwrap();
        rec = rec.checked_update(ParamValue::U32(204_000), 8, 80).unwrap();
        rec = rec
            .checked_update(ParamValue::U32(201_600), 10, 100)
            .unwrap();
        // At 201,600 the 10% allowance is 20,160 — a 30,240 raise (15%) that
        // the old absolute bound accepted must now fail.
        assert_eq!(
            rec.checked_update(ParamValue::U32(231_840), 12, 120),
            Err(Error::DeltaTooLarge)
        );
        rec = rec
            .checked_update(ParamValue::U32(221_760), 12, 120)
            .unwrap();
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
            rec.checked_update(ParamValue::Balance(24_000_000_001), 1, 10),
            Err(Error::DeltaTooLarge)
        );
        rec = rec
            .checked_update(ParamValue::Balance(24_000_000_000), 1, 10)
            .unwrap();
        assert_eq!(
            rec.checked_update(ParamValue::Balance(11_999_999_999), 2, 20),
            Err(Error::DeltaTooLarge)
        );
        rec = rec
            .checked_update(ParamValue::Balance(12_000_000_000), 2, 20)
            .unwrap();
        assert_eq!(rec.value, ParamValue::Balance(12_000_000_000));
    }

    #[test]
    fn max_delta_allowance_matches_every_admission_rule() {
        let mut record = genesis_params()[0];
        // epoch.length carries a Percent(10) Δ cap, so the scalar allowance is 10% of
        // its current (default) value — timing-agnostic across the fast-timing build.
        assert_eq!(
            record.max_delta_allowance(),
            Ok((timing_defaults::EPOCH_LENGTH / 10) as u128)
        );

        record.max_delta = Some(MaxDelta::Absolute(ParamValue::U32(17)));
        assert_eq!(record.max_delta_allowance(), Ok(17));

        record.max_delta = None;
        assert_eq!(record.max_delta_allowance(), Ok(0));

        record.value = ParamValue::Balance(5);
        record.min = ParamValue::Balance(0);
        record.max = ParamValue::Balance(u128::MAX);
        record.max_delta = Some(MaxDelta::Factor(2));
        // checked_update admits [ceil(5 / 2), 5 * 2] = [3, 10], so the
        // 02 §4 scalar is conservatively the smaller directional allowance
        // under R-7; which side this lossy projection denotes remains an open
        // contract question.
        assert_eq!(record.max_delta_allowance(), Ok(2));

        record.value = ParamValue::Balance(u128::MAX - 1);
        // Saturation leaves one unit upward, which is the conservative side.
        assert_eq!(record.max_delta_allowance(), Ok(1));

        record.max_delta = Some(MaxDelta::Factor(0));
        assert_eq!(record.max_delta_allowance(), Err(Error::MetaBoundViolation));
    }

    #[test]
    fn admissible_next_interval_matches_admission_rounding_and_record_bounds() {
        let mut record = genesis_params()[0];
        let value = record.value.as_u128();
        let allowance = value / 10;
        assert_eq!(
            record.admissible_next_interval(),
            Ok((
                record.min.as_u128().max(value.saturating_sub(allowance)),
                record.max.as_u128().min(value.saturating_add(allowance)),
            ))
        );

        record.value = ParamValue::Balance(5);
        record.min = ParamValue::Balance(1);
        record.max = ParamValue::Balance(9);
        record.max_delta = Some(MaxDelta::Absolute(ParamValue::Balance(2)));
        assert_eq!(record.admissible_next_interval(), Ok((3, 7)));

        record.max_delta = None;
        assert_eq!(record.admissible_next_interval(), Ok((1, 9)));

        record.max_delta = Some(MaxDelta::Factor(2));
        assert_eq!(record.admissible_next_interval(), Ok((3, 9)));
        for next in 1..=10 {
            let admitted = record
                .checked_update(ParamValue::Balance(next), u32::MAX, 1)
                .is_ok();
            assert_eq!(admitted, (3..=9).contains(&next), "next={next}");
        }

        record.value = ParamValue::Balance(u128::MAX - 1);
        record.min = ParamValue::Balance(0);
        record.max = ParamValue::Balance(u128::MAX);
        assert_eq!(
            record.admissible_next_interval(),
            Ok(((u128::MAX - 1).div_ceil(2), u128::MAX))
        );

        record.max_delta = Some(MaxDelta::Factor(0));
        assert_eq!(
            record.admissible_next_interval(),
            Err(Error::MetaBoundViolation)
        );
    }

    #[test]
    fn factor_allowance_and_exact_interval_project_exec_lock_code() {
        // Contract v6 retains the conservative scalar and exposes the exact
        // inclusive interval for the asymmetric exec.lock.* factor rule.
        let record = genesis_params()
            .into_iter()
            .find(|record| record.key == key16(b"exec.lock.code"))
            .expect("the canonical exec.lock.code record exists");
        let value = record.value.as_u128();
        assert!(matches!(record.max_delta, Some(MaxDelta::Factor(_))));
        let factor = match record.max_delta {
            Some(MaxDelta::Factor(factor)) => u128::from(factor),
            _ => 1,
        };
        assert!(factor >= 1);
        let lower = value / factor + u128::from(value % factor != 0);
        let downward = value.saturating_sub(lower);
        let upward = value.saturating_mul(factor).saturating_sub(value);

        assert_eq!(record.max_delta_allowance(), Ok(downward.min(upward)));
        assert_eq!(record.max_delta_allowance(), Ok(downward));
        assert!(downward < upward);
        assert_eq!(value, 100_800);
        assert_eq!(record.max.as_u128(), 432_000);
        assert_eq!(record.admissible_next_interval(), Ok((50_400, 201_600)));
    }

    #[test]
    fn genesis_param_keys_are_canonical_and_distinct() {
        let params = genesis_params();
        // 13 §1 canonical spellings (Codex review, PR #14): the seeded keys
        // must match the names downstream binders derive with key16.
        for name in [
            b"intake.max_acct".as_slice(),
            b"keeper.budget".as_slice(),
            b"ops.ct_dot_rate".as_slice(),
            b"ops.ct_fee_dot".as_slice(),
            b"ops.probe_fee".as_slice(),
            b"ops.probe_rate".as_slice(),
            b"ops.ct_quote_ttl".as_slice(),
            b"xcm.dot_per_sec".as_slice(),
            b"xcm.dot_per_mb".as_slice(),
            b"xcm.usdc_per_sec".as_slice(),
            b"xcm.usdc_per_mb".as_slice(),
        ] {
            assert!(
                params.iter().any(|record| record.key == key16(name)),
                "missing canonical genesis Param key: {name:?}"
            );
        }
        for (name, value, min, max) in [
            (
                b"ops.ct_dot_rate".as_slice(),
                5_000_000,
                500_000,
                500_000_000,
            ),
            (
                b"ops.ct_fee_dot".as_slice(),
                5_000_000_000,
                100_000_000,
                100_000_000_000,
            ),
            (
                b"ops.probe_fee".as_slice(),
                5_000_000_000,
                100_000_000,
                100_000_000_000,
            ),
            (
                b"ops.probe_rate".as_slice(),
                5_000_000,
                500_000,
                500_000_000,
            ),
        ] {
            let Some(record) = params.iter().find(|record| record.key == key16(name)) else {
                assert!(
                    params.iter().any(|record| record.key == key16(name)),
                    "missing Coretime Balance Param: {name:?}"
                );
                continue;
            };
            assert_eq!(record.value, ParamValue::Balance(value));
            assert_eq!(record.min, ParamValue::Balance(min));
            assert_eq!(record.max, ParamValue::Balance(max));
            assert_eq!(record.max_delta, Some(MaxDelta::Factor(2)));
            assert_eq!(record.cooldown_epochs, 1);
            assert_eq!(record.class, ParamClass::Treasury);
            assert!(!record.kernel_bounded);
        }
        let Some(ttl) = params
            .iter()
            .find(|record| record.key == key16(b"ops.ct_quote_ttl"))
        else {
            assert!(
                params
                    .iter()
                    .any(|record| record.key == key16(b"ops.ct_quote_ttl")),
                "missing Coretime TTL Param"
            );
            return;
        };
        assert_eq!(ttl.value, ParamValue::U32(100_800));
        assert_eq!(ttl.min, ParamValue::U32(7_200));
        assert_eq!(ttl.max, ParamValue::U32(403_200));
        assert_eq!(ttl.max_delta, Some(MaxDelta::Factor(2)));
        assert_eq!(ttl.cooldown_epochs, 1);
        assert_eq!(ttl.class, ParamClass::Treasury);
        assert!(!ttl.kernel_bounded);
        for (name, value, min, max) in [
            (
                b"xcm.dot_per_sec".as_slice(),
                100_000_000_000,
                1_000_000_000,
                10_000_000_000_000,
            ),
            (
                b"xcm.dot_per_mb".as_slice(),
                10_000_000_000,
                100_000_000,
                1_000_000_000_000,
            ),
            (
                b"xcm.usdc_per_sec".as_slice(),
                50_000_000,
                500_000,
                5_000_000_000,
            ),
            (
                b"xcm.usdc_per_mb".as_slice(),
                5_000_000,
                50_000,
                500_000_000,
            ),
        ] {
            let mut matches = 0_u8;
            for record in params.iter().filter(|record| record.key == key16(name)) {
                matches = matches.saturating_add(1);
                assert_eq!(record.value, ParamValue::Balance(value));
                assert_eq!(record.min, ParamValue::Balance(min));
                assert_eq!(record.max, ParamValue::Balance(max));
                assert_eq!(record.max_delta, Some(MaxDelta::Factor(2)));
                assert_eq!(record.cooldown_epochs, 1);
                assert_eq!(record.class, ParamClass::Param);
                assert!(!record.kernel_bounded);
            }
            assert_eq!(matches, 1, "missing or duplicate governed XCM rate");
        }
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
                1,
                10
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
                1,
                10
            ),
            Err(Error::BadOrigin)
        );
        assert_eq!(
            state.dispatch_set_param(
                ConstitutionOrigin::FutarchyTreasury,
                key16(b"mkt.obs_interval"),
                ParamValue::U32(12),
                1,
                10
            ),
            Err(Error::BadOrigin)
        );
        assert_eq!(
            state.dispatch_set_param(
                ConstitutionOrigin::FutarchyParam,
                key16(b"missing"),
                ParamValue::U32(12),
                1,
                10
            ),
            Err(Error::UnknownParam)
        );
        state
            .dispatch_set_param(
                ConstitutionOrigin::FutarchyParam,
                key16(b"mkt.obs_interval"),
                ParamValue::U32(12),
                1,
                10,
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
    fn welfare_low_knees_require_the_track_matching_the_direction() {
        for key_name in [b"welfare.thS_lo".as_slice(), b"welfare.thC_lo".as_slice()] {
            let key = key16(key_name);
            let initial = ConstitutionState::genesis();
            let record = initial.params.iter().find(|record| record.key == key);
            assert!(record.is_some(), "welfare low-knee key must be seeded");
            let Some(record) = record else {
                return;
            };
            let interval = record.admissible_next_interval();
            assert!(interval.is_ok(), "welfare low-knee interval must be valid");
            let Ok((_, upper)) = interval else {
                return;
            };
            let raised = value_from_raw(record.value, upper);
            assert!(
                raised.is_some(),
                "welfare low-knee upper value must preserve its kind"
            );
            let Some(raised) = raised else {
                return;
            };
            assert!(upper > record.value.as_u128());

            // Increase: constitution succeeds; entrenched and the legacy bare
            // values origin fail without writing.
            let mut wrong_increase = initial.clone();
            let before = wrong_increase.clone();
            assert_eq!(
                wrong_increase.dispatch_set_param(
                    ConstitutionOrigin::EntrenchedTrack,
                    key,
                    raised,
                    record.cooldown_epochs,
                    1,
                ),
                Err(Error::BadOrigin)
            );
            assert_eq!(wrong_increase, before);
            let mut bare_values = initial.clone();
            let before = bare_values.clone();
            assert_eq!(
                bare_values.dispatch_set_param(
                    ConstitutionOrigin::ConstitutionalValues,
                    key,
                    raised,
                    record.cooldown_epochs,
                    1,
                ),
                Err(Error::BadOrigin)
            );
            assert_eq!(bare_values, before);

            let mut raised_state = initial.clone();
            assert_eq!(
                raised_state.dispatch_set_param(
                    ConstitutionOrigin::ConstitutionTrack,
                    key,
                    raised,
                    record.cooldown_epochs,
                    1,
                ),
                Ok(())
            );

            // Decrease from the tightened value: entrenched succeeds and the
            // constitution track cannot walk its own tightening back.
            let decrease_epoch = record.cooldown_epochs.saturating_mul(2);
            let mut wrong_decrease = raised_state.clone();
            let before = wrong_decrease.clone();
            assert_eq!(
                wrong_decrease.dispatch_set_param(
                    ConstitutionOrigin::ConstitutionTrack,
                    key,
                    record.value,
                    decrease_epoch,
                    2,
                ),
                Err(Error::BadOrigin)
            );
            assert_eq!(wrong_decrease, before);
            assert_eq!(
                raised_state.dispatch_set_param(
                    ConstitutionOrigin::EntrenchedTrack,
                    key,
                    record.value,
                    decrease_epoch,
                    2,
                ),
                Ok(())
            );

            // Equality retains the row's CONST-class constitution route; it
            // neither grants entrenched authority nor admits bare values.
            let mut equal = initial.clone();
            assert_eq!(
                equal.dispatch_set_param(
                    ConstitutionOrigin::ConstitutionTrack,
                    key,
                    record.value,
                    record.cooldown_epochs,
                    3,
                ),
                Ok(())
            );
            for origin in [
                ConstitutionOrigin::EntrenchedTrack,
                ConstitutionOrigin::ConstitutionalValues,
            ] {
                let mut refused_equal = initial.clone();
                let before = refused_equal.clone();
                assert_eq!(
                    refused_equal.dispatch_set_param(
                        origin,
                        key,
                        record.value,
                        record.cooldown_epochs,
                        3,
                    ),
                    Err(Error::BadOrigin)
                );
                assert_eq!(refused_equal, before);
            }

            // The launch floor remains absolute even for the entrenched path.
            let below_floor_raw = record.min.as_u128().checked_sub(1);
            assert!(
                below_floor_raw.is_some(),
                "welfare launch floor must be non-zero"
            );
            let Some(below_floor_raw) = below_floor_raw else {
                return;
            };
            let below_floor = value_from_raw(record.value, below_floor_raw);
            assert!(
                below_floor.is_some(),
                "welfare below-floor value must preserve its kind"
            );
            let Some(below_floor) = below_floor else {
                return;
            };
            let mut floor_state = initial.clone();
            let before = floor_state.clone();
            assert_eq!(
                floor_state.dispatch_set_param(
                    ConstitutionOrigin::EntrenchedTrack,
                    key,
                    below_floor,
                    record.cooldown_epochs,
                    4,
                ),
                Err(Error::BelowMin)
            );
            assert_eq!(floor_state, before);
        }
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
                4,
                40
            ),
            Err(Error::BadOrigin)
        );
        state
            .dispatch_set_param(
                ConstitutionOrigin::FutarchyMeta,
                key16(b"epoch.horizon_k"),
                ParamValue::U8(3),
                4,
                40,
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
            state.dispatch_set_release_channel(ConstitutionOrigin::Signed, bad, 7),
            Err(Error::BadOrigin)
        );
        let mut good = [0u8; RELEASE_CHANNEL_LEN];
        good[0] = 1;
        // 02 §12 / 06 §2.1: the scoped constitution track and its internal
        // bare form are the only origin-mediated writers; CODE/META/Root paths
        // must all be refused.
        for refused in [
            ConstitutionOrigin::FutarchyCode,
            ConstitutionOrigin::FutarchyMeta,
            ConstitutionOrigin::Root,
        ] {
            assert_eq!(
                state.dispatch_set_release_channel(refused, good, 7),
                Err(Error::BadOrigin)
            );
        }
        assert_eq!(
            state.dispatch_set_release_channel(ConstitutionOrigin::ConstitutionTrack, good, 7),
            Ok(())
        );
        assert_eq!(
            state.dispatch_set_release_channel(ConstitutionOrigin::ConstitutionalValues, good, 7),
            Ok(())
        );

        state.release_channel = channel;
        let mut writer_b = good;
        writer_b[108..112].copy_from_slice(&43u32.to_le_bytes());
        writer_b[112..116].copy_from_slice(&99u32.to_le_bytes());
        writer_b[116..120].copy_from_slice(&0u32.to_le_bytes());
        writer_b[164..168].copy_from_slice(&2u32.to_le_bytes());
        // 02 §12: offset 108 is the block of the last write, stamped by the
        // dispatch path. The caller's 43 MUST be ignored — a lawful writer
        // must not be able to backdate or future-date the freshness a
        // stranded reader depends on.
        assert_eq!(
            state.dispatch_set_release_channel(
                ConstitutionOrigin::ConstitutionalValues,
                writer_b,
                5_000
            ),
            Ok(())
        );
        assert_eq!(state.release_channel.updated_at(), 5_000);
        assert_eq!(state.release_channel.spec_version(), 7);
        assert_eq!(state.release_channel.pending_authorized_at(), 11);
        assert_eq!(state.release_channel.flags(), 6);
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
