#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_fixed::FixedU64x64;
use futarchy_primitives::{EpochId, FixedU64, MetricId, MetricSpecVersion, WelfareView};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

macro_rules! ensure {
    ($cond:expr, $err:expr $(,)?) => {
        if !$cond {
            return Err($err);
        }
    };
}

pub const ONE: u64 = 1_000_000_000;
pub const EPSILON: FixedU64 = FixedU64(1);
pub const EPSILON_PILLAR: FixedU64 = FixedU64(10_000_000);
pub const MAX_METRIC_SPECS: usize = 16;
pub const MAX_SNAPSHOTS: usize = 20;
pub const MAX_GATE_FLAGS: usize = MAX_SNAPSHOTS;
pub const MAX_COMPONENTS_PER_SPEC: usize = 16;
pub const HISTORY_PRIORS: usize = 12;
pub const THETA_S_LO: FixedU64 = FixedU64(900_000_000);
pub const THETA_S_HI: FixedU64 = FixedU64(980_000_000);
pub const THETA_C_LO: FixedU64 = FixedU64(850_000_000);
pub const THETA_C_HI: FixedU64 = FixedU64(950_000_000);
pub const W_P: FixedU64 = FixedU64(600_000_000);
pub const W_A: FixedU64 = FixedU64(400_000_000);

#[derive(
    Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, Ord, PartialEq, PartialOrd, TypeInfo,
)]
pub enum Pillar {
    S,
    COnchain,
    CAttested,
    P,
    A,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum SourceClass {
    Onchain,
    RelayDerived,
    Attested,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct MetricSpec {
    pub id: MetricId,
    pub version: MetricSpecVersion,
    pub pillar: Pillar,
    pub weight: FixedU64,
    pub epsilon_floor: FixedU64,
    pub activation_epoch: EpochId,
    pub source: SourceClass,
    pub formula_ref: [u8; 32],
    pub units: [u8; 16],
    pub repr: [u8; 16],
    pub cadence_blocks: u32,
    pub sanity_min: FixedU64,
    pub sanity_max: FixedU64,
    pub has_normalization_rule: bool,
    pub has_missing_data_rule: bool,
    pub has_gaming_vectors: bool,
    pub has_challenge_procedure: bool,
    pub prior_bounds: [FixedU64; HISTORY_PRIORS],
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct ComponentValue {
    pub id: MetricId,
    pub value: FixedU64,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct Snapshot {
    pub epoch: EpochId,
    pub spec_version: MetricSpecVersion,
    pub s_pillar: FixedU64,
    pub c_onchain: FixedU64,
    pub c_attested: FixedU64,
    pub p_pillar: FixedU64,
    pub a_pillar: FixedU64,
    pub gate_s: FixedU64,
    pub gate_c: FixedU64,
    pub welfare: FixedU64,
    pub components: Vec<ComponentValue>,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct GateBreachFlags {
    pub s_breached: bool,
    pub c_breached: bool,
    pub day_bitmap: [u32; 2],
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Event {
    MetricSpecRegistered {
        version: MetricSpecVersion,
    },
    SnapshotRecorded {
        epoch: EpochId,
        spec_version: MetricSpecVersion,
        welfare: FixedU64,
    },
    GateBreachRecorded {
        epoch: EpochId,
        day: u8,
        s_breached: bool,
        c_breached: bool,
    },
    SettlementComputed {
        epoch: EpochId,
        spec_version: MetricSpecVersion,
        score: FixedU64,
    },
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Error {
    BadOrigin,
    TooManyMetricSpecs,
    TooManySnapshots,
    TooManyComponents,
    TooManyGateFlags,
    DuplicateSpecVersion,
    SpecNotFound,
    BadActivationEpoch,
    MissingMetricDiscipline,
    BadWeightSum,
    ValueOutOfRange,
    MissingComponent,
    DuplicateComponent,
    DuplicateSnapshot,
    ArithmeticOverflow,
    TryStateViolation,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct WelfareState {
    pub specs: Vec<(MetricSpecVersion, Vec<MetricSpec>)>,
    pub snapshots: Vec<Snapshot>,
    pub gate_flags: Vec<(EpochId, GateBreachFlags)>,
    pub events: Vec<Event>,
}

impl Default for WelfareState {
    fn default() -> Self {
        Self::new()
    }
}

impl WelfareState {
    pub const fn new() -> Self {
        Self {
            specs: Vec::new(),
            snapshots: Vec::new(),
            gate_flags: Vec::new(),
            events: Vec::new(),
        }
    }

    pub fn register_metric_spec(
        &mut self,
        current_epoch: EpochId,
        version: MetricSpecVersion,
        mut specs: Vec<MetricSpec>,
    ) -> Result<(), Error> {
        ensure!(
            self.specs.len() < MAX_METRIC_SPECS,
            Error::TooManyMetricSpecs
        );
        ensure!(
            self.specs.iter().all(|(v, _)| *v != version),
            Error::DuplicateSpecVersion
        );
        ensure!(
            !specs.is_empty() && specs.len() <= MAX_COMPONENTS_PER_SPEC,
            Error::TooManyComponents
        );
        specs.sort_by_key(|s| s.id);
        let mut prev = None;
        for spec in &specs {
            ensure!(spec.version == version, Error::SpecNotFound);
            ensure!(
                spec.activation_epoch >= current_epoch.saturating_add(2),
                Error::BadActivationEpoch
            );
            ensure!(
                spec.weight.0 <= ONE
                    && spec.epsilon_floor.0 <= ONE
                    && spec.sanity_min.0 <= spec.sanity_max.0
                    && spec.sanity_max.0 <= ONE,
                Error::ValueOutOfRange
            );
            ensure!(
                spec.has_normalization_rule
                    && spec.has_missing_data_rule
                    && spec.has_gaming_vectors
                    && spec.has_challenge_procedure,
                Error::MissingMetricDiscipline
            );
            ensure!(
                prev.replace(spec.id).is_none_or(|p| p < spec.id),
                Error::DuplicateComponent
            );
        }
        // 05 §4.4: every weight vector sums to 1 exactly, checked here. S is
        // min-aggregated (no weights); the C vector is one vector across
        // C_onchain and C_attested jointly - C_daily then renormalizes over
        // the on-chain subset, which is only meaningful for a joint vector.
        let weight_sum = |pillars: &[Pillar]| -> Result<u64, Error> {
            specs
                .iter()
                .filter(|s| pillars.contains(&s.pillar))
                .try_fold(0u64, |acc, s| {
                    acc.checked_add(s.weight.0).ok_or(Error::ArithmeticOverflow)
                })
        };
        ensure!(
            weight_sum(&[Pillar::COnchain, Pillar::CAttested])? == ONE,
            Error::BadWeightSum
        );
        ensure!(weight_sum(&[Pillar::P])? == ONE, Error::BadWeightSum);
        ensure!(weight_sum(&[Pillar::A])? == ONE, Error::BadWeightSum);
        self.specs.push((version, specs));
        self.events.push(Event::MetricSpecRegistered { version });
        Ok(())
    }

    pub fn record_snapshot(
        &mut self,
        epoch: EpochId,
        spec_version: MetricSpecVersion,
        components: Vec<ComponentValue>,
        incident_multiplier: FixedU64,
    ) -> Result<FixedU64, Error> {
        ensure!(
            self.snapshots.len() < MAX_SNAPSHOTS,
            Error::TooManySnapshots
        );
        ensure!(
            components.len() <= MAX_COMPONENTS_PER_SPEC,
            Error::TooManyComponents
        );
        ensure!(incident_multiplier.0 <= ONE, Error::ValueOutOfRange);
        ensure!(
            self.snapshots
                .iter()
                .all(|s| s.epoch != epoch || s.spec_version != spec_version),
            Error::DuplicateSnapshot
        );
        let specs = self.spec(spec_version)?.to_vec();
        let pillars = compute_pillars(&specs, &components, incident_multiplier)?;
        let welfare = compute_welfare(pillars.s, pillars.c_settlement, pillars.p, pillars.a)?;
        self.snapshots.push(Snapshot {
            epoch,
            spec_version,
            s_pillar: pillars.s,
            c_onchain: pillars.c_onchain,
            c_attested: pillars.c_attested,
            p_pillar: pillars.p,
            a_pillar: pillars.a,
            gate_s: gate(pillars.s, THETA_S_LO, THETA_S_HI)?,
            gate_c: gate(pillars.c_settlement, THETA_C_LO, THETA_C_HI)?,
            welfare,
            components,
        });
        self.events.push(Event::SnapshotRecorded {
            epoch,
            spec_version,
            welfare,
        });
        Ok(welfare)
    }

    pub fn record_daily_gate(
        &mut self,
        epoch: EpochId,
        day: u8,
        spec_version: MetricSpecVersion,
        components: Vec<ComponentValue>,
    ) -> Result<GateBreachFlags, Error> {
        ensure!(day < 64, Error::ValueOutOfRange);
        let specs = self.spec(spec_version)?.to_vec();
        let (s_daily, c_daily) = compute_daily_gates(&specs, &components)?;
        let s_breach = s_daily.0 < THETA_S_LO.0;
        let c_breach = c_daily.0 < THETA_C_LO.0;
        let idx = self.gate_flags.iter().position(|(e, _)| *e == epoch);
        let mut flags = idx
            .map(|i| self.gate_flags[i].1)
            .unwrap_or(GateBreachFlags {
                s_breached: false,
                c_breached: false,
                day_bitmap: [0; 2],
            });
        if s_breach || c_breach {
            flags.day_bitmap[(day / 32) as usize] |= 1u32 << (day % 32);
        }
        flags.s_breached |= s_breach;
        flags.c_breached |= c_breach;
        if let Some(i) = idx {
            self.gate_flags[i].1 = flags;
        } else {
            ensure!(
                self.gate_flags.len() < MAX_GATE_FLAGS,
                Error::TooManyGateFlags
            );
            self.gate_flags.push((epoch, flags));
        }
        self.events.push(Event::GateBreachRecorded {
            epoch,
            day,
            s_breached: s_breach,
            c_breached: c_breach,
        });
        Ok(flags)
    }

    pub fn compute_settlement(
        &mut self,
        cohort_epoch: EpochId,
        spec_version: MetricSpecVersion,
    ) -> Result<FixedU64, Error> {
        let w1 = self
            .snapshot(cohort_epoch.saturating_add(1), spec_version)?
            .welfare;
        let w2 = self
            .snapshot(cohort_epoch.saturating_add(2), spec_version)?
            .welfare;
        let score = settlement_score(w1, w2)?;
        self.events.push(Event::SettlementComputed {
            epoch: cohort_epoch,
            spec_version,
            score,
        });
        Ok(score)
    }

    pub fn current_view(
        &self,
        epoch: EpochId,
        spec_version: MetricSpecVersion,
        reserve_flag: bool,
    ) -> Result<WelfareView, Error> {
        let s = self.snapshot(epoch, spec_version)?;
        let flags = self
            .gate_flags
            .iter()
            .find(|(e, _)| *e == epoch)
            .map(|(_, f)| *f)
            .unwrap_or(GateBreachFlags {
                s_breached: false,
                c_breached: false,
                day_bitmap: [0; 2],
            });
        Ok(WelfareView {
            epoch,
            spec_version,
            s_pillar_1e9: s.s_pillar,
            c_onchain_1e9: s.c_onchain,
            c_attested_1e9: s.c_attested,
            p_pillar_1e9: s.p_pillar,
            a_pillar_1e9: s.a_pillar,
            gate_s_1e9: s.gate_s,
            gate_c_1e9: s.gate_c,
            w_current_1e9: s.welfare,
            s_breached: flags.s_breached,
            c_breached: flags.c_breached,
            reserve_flag,
        })
    }

    pub fn try_state(&self) -> Result<(), Error> {
        ensure!(
            self.specs.len() <= MAX_METRIC_SPECS
                && self.snapshots.len() <= MAX_SNAPSHOTS
                && self.gate_flags.len() <= MAX_GATE_FLAGS,
            Error::TryStateViolation
        );
        for (index, (version, specs)) in self.specs.iter().enumerate() {
            ensure!(
                self.specs[..index].iter().all(|(seen, _)| seen != version),
                Error::TryStateViolation
            );
            ensure!(
                !specs.is_empty() && specs.len() <= MAX_COMPONENTS_PER_SPEC,
                Error::TryStateViolation
            );
            let mut prev = None;
            for spec in specs {
                ensure!(
                    spec.version == *version
                        && spec.weight.0 <= ONE
                        && spec.epsilon_floor.0 <= ONE
                        && spec.sanity_min.0 <= spec.sanity_max.0
                        && spec.sanity_max.0 <= ONE
                        && prev.replace(spec.id).is_none_or(|p| p < spec.id),
                    Error::TryStateViolation
                );
            }
        }
        for (index, s) in self.snapshots.iter().enumerate() {
            ensure!(
                self.snapshots[..index]
                    .iter()
                    .all(|seen| seen.epoch != s.epoch || seen.spec_version != s.spec_version)
                    && s.components.len() <= MAX_COMPONENTS_PER_SPEC,
                Error::TryStateViolation
            );
            ensure!(
                [
                    s.s_pillar,
                    s.c_onchain,
                    s.c_attested,
                    s.p_pillar,
                    s.a_pillar,
                    s.gate_s,
                    s.gate_c,
                    s.welfare
                ]
                .iter()
                .all(|v| v.0 <= ONE),
                Error::TryStateViolation
            );
        }
        for (index, (epoch, flags)) in self.gate_flags.iter().enumerate() {
            ensure!(
                self.gate_flags[..index]
                    .iter()
                    .all(|(seen, _)| seen != epoch)
                    && flags.day_bitmap.len() == 2,
                Error::TryStateViolation
            );
        }
        Ok(())
    }

    fn spec(&self, version: MetricSpecVersion) -> Result<&[MetricSpec], Error> {
        self.specs
            .iter()
            .find(|(v, _)| *v == version)
            .map(|(_, s)| s.as_slice())
            .ok_or(Error::SpecNotFound)
    }
    fn snapshot(&self, epoch: EpochId, version: MetricSpecVersion) -> Result<&Snapshot, Error> {
        self.snapshots
            .iter()
            .find(|s| s.epoch == epoch && s.spec_version == version)
            .ok_or(Error::MissingComponent)
    }
}

#[derive(Clone, Copy)]
struct Pillars {
    s: FixedU64,
    c_onchain: FixedU64,
    c_attested: FixedU64,
    c_settlement: FixedU64,
    p: FixedU64,
    a: FixedU64,
}

fn compute_pillars(
    specs: &[MetricSpec],
    components: &[ComponentValue],
    incident: FixedU64,
) -> Result<Pillars, Error> {
    let s = specs
        .iter()
        .filter(|m| m.pillar == Pillar::S)
        .try_fold(FixedU64(ONE), |acc, m| {
            Ok(FixedU64(acc.0.min(value_for(m.id, components)?.0)))
        })?;
    // View partials (per sub-pillar, spec weights as registered)...
    let c_onchain = weighted_geo(specs, components, &[Pillar::COnchain], None)?;
    let c_attested_geo = weighted_geo(specs, components, &[Pillar::CAttested], None)?;
    let c_attested = mul_down(incident, c_attested_geo)?;
    // ...while the settlement C_e evaluates the joint weight vector as ONE
    // exp2(sum(w * log2(max(c, eps)))) composite (05 §4.4 (2)), incident-
    // multiplied (05 §4.4: I is a pure multiplier).
    let c_joint = weighted_geo(
        specs,
        components,
        &[Pillar::COnchain, Pillar::CAttested],
        None,
    )?;
    let c_settlement = mul_down(incident, c_joint)?;
    let p = weighted_geo(specs, components, &[Pillar::P], None)?;
    let a = weighted_geo(specs, components, &[Pillar::A], None)?;
    Ok(Pillars {
        s,
        c_onchain,
        c_attested,
        c_settlement,
        p,
        a,
    })
}

fn compute_daily_gates(
    specs: &[MetricSpec],
    components: &[ComponentValue],
) -> Result<(FixedU64, FixedU64), Error> {
    let s = specs
        .iter()
        .filter(|m| m.pillar == Pillar::S)
        .try_fold(FixedU64(ONE), |acc, m| {
            Ok(FixedU64(acc.0.min(value_for(m.id, components)?.0)))
        })?;
    // C_daily renormalizes the joint C weight vector over the on-chain
    // subset (05 §4.4): w_j / sum_onchain(w). No attested term, ever.
    let onchain_sum = specs
        .iter()
        .filter(|m| m.pillar == Pillar::COnchain)
        .try_fold(0u64, |acc, m| {
            acc.checked_add(m.weight.0).ok_or(Error::ArithmeticOverflow)
        })?;
    let c_daily = weighted_geo(specs, components, &[Pillar::COnchain], Some(onchain_sum))?;
    Ok((s, c_daily))
}

/// Weighted geometric composite per the 05 §4.4 determinism discipline:
/// evaluated in ascending `MetricId` order (specs are stored sorted) as one
/// `exp2(sum(w_i * log2(max(x_i, eps))))` in 64.64. Every true product
/// `w_i * log2(x_i)` is <= 0 and must round toward negative infinity, which
/// in the inverse domain used here (`log2(1/x) >= 0`) is a ceiling. With
/// `renormalize = Some(total)`, each weight is divided by `total` first
/// (the C_daily rule).
fn weighted_geo(
    specs: &[MetricSpec],
    components: &[ComponentValue],
    pillars: &[Pillar],
    renormalize: Option<u64>,
) -> Result<FixedU64, Error> {
    let mut exponent = FixedU64x64::ZERO;
    for m in specs.iter().filter(|m| pillars.contains(&m.pillar)) {
        let v = value_for(m.id, components)?;
        let value = v.0.max(m.epsilon_floor.0);
        if m.weight.0 == 0 || value >= ONE {
            continue;
        }
        ensure!(value > 0, Error::ValueOutOfRange);
        let inv = FixedU64x64::ONE
            .checked_div(q64_from_1e9(value)?)
            .map_err(|_| Error::ArithmeticOverflow)?;
        let log = inv.log2().map_err(|_| Error::ArithmeticOverflow)?;
        let mut weight = q64_from_1e9(m.weight.0)?;
        if let Some(total) = renormalize {
            ensure!(total > 0, Error::BadWeightSum);
            weight = weight
                .checked_div(q64_from_1e9(total)?)
                .map_err(|_| Error::ArithmeticOverflow)?;
        }
        exponent = exponent
            .checked_add(mul_ceil_q64(log, weight)?)
            .map_err(|_| Error::ArithmeticOverflow)?;
    }
    exp2_inverse_down(exponent)
}

/// `ceil(a * b)` at 64.64 raw granularity. The low 64 bits of the full
/// 256-bit raw product are exactly `a.raw() as u64 * b.raw() as u64`
/// (wrapping), so they detect whether the crate's flooring multiply
/// truncated.
fn mul_ceil_q64(a: FixedU64x64, b: FixedU64x64) -> Result<FixedU64x64, Error> {
    let floor = a.checked_mul(b).map_err(|_| Error::ArithmeticOverflow)?;
    let truncated = (a.raw() as u64).wrapping_mul(b.raw() as u64) != 0;
    if truncated {
        floor
            .checked_add(FixedU64x64::from_raw(1))
            .map_err(|_| Error::ArithmeticOverflow)
    } else {
        Ok(floor)
    }
}

/// `2^(-exponent)` floored to the 1e9 grid (the composite's closing step).
fn exp2_inverse_down(exponent: FixedU64x64) -> Result<FixedU64, Error> {
    if exponent.raw() == 0 {
        return Ok(FixedU64(ONE));
    }
    let denom = exponent.exp2().map_err(|_| Error::ArithmeticOverflow)?;
    let term = FixedU64x64::ONE
        .checked_div(denom)
        .map_err(|_| Error::ArithmeticOverflow)?;
    q64_to_1e9_down(term)
}

fn value_for(id: MetricId, components: &[ComponentValue]) -> Result<FixedU64, Error> {
    let mut found = None;
    for c in components {
        if c.id == id {
            ensure!(found.is_none(), Error::DuplicateComponent);
            ensure!(c.value.0 <= ONE, Error::ValueOutOfRange);
            found = Some(c.value);
        }
    }
    found.ok_or(Error::MissingComponent)
}

/// `x1^w1 * x2^w2` as one exp2/log2 composite (the P/A GeoComposite of the
/// W product, same discipline as [`weighted_geo`]).
fn geo_pair(first: (FixedU64, FixedU64), second: (FixedU64, FixedU64)) -> Result<FixedU64, Error> {
    let mut exponent = FixedU64x64::ZERO;
    for (value, weight) in [first, second] {
        if weight.0 == 0 || value.0 >= ONE {
            continue;
        }
        if value.0 == 0 {
            return Ok(FixedU64(0));
        }
        let inv = FixedU64x64::ONE
            .checked_div(q64_from_1e9(value.0)?)
            .map_err(|_| Error::ArithmeticOverflow)?;
        let log = inv.log2().map_err(|_| Error::ArithmeticOverflow)?;
        exponent = exponent
            .checked_add(mul_ceil_q64(log, q64_from_1e9(weight.0)?)?)
            .map_err(|_| Error::ArithmeticOverflow)?;
    }
    exp2_inverse_down(exponent)
}

pub fn compute_welfare(
    s: FixedU64,
    c: FixedU64,
    p: FixedU64,
    a: FixedU64,
) -> Result<FixedU64, Error> {
    let gs = gate(s, THETA_S_LO, THETA_S_HI)?;
    let gc = gate(c, THETA_C_LO, THETA_C_HI)?;
    let pa = geo_pair((p, W_P), (a, W_A))?;
    mul_down(mul_down(gs, gc)?, pa)
}

/// 05 §4.4 (4): `s = exp2((log2 max(W1, eps_W) + log2 max(W2, eps_W)) / 2)`,
/// eps_W = 1e-9, rounded down to the FixedU64 grid. Evaluated through the
/// prescribed exp2/log2 pipeline (in the inverse domain, with the halving
/// ceiled so the score itself rounds down).
pub fn settlement_score(w1: FixedU64, w2: FixedU64) -> Result<FixedU64, Error> {
    let mut exponent = FixedU64x64::ZERO;
    for value in [w1.0.max(EPSILON.0), w2.0.max(EPSILON.0)] {
        if value >= ONE {
            continue;
        }
        let inv = FixedU64x64::ONE
            .checked_div(q64_from_1e9(value)?)
            .map_err(|_| Error::ArithmeticOverflow)?;
        let log = inv.log2().map_err(|_| Error::ArithmeticOverflow)?;
        exponent = exponent
            .checked_add(log)
            .map_err(|_| Error::ArithmeticOverflow)?;
    }
    let half = FixedU64x64::from_raw(exponent.raw().div_ceil(2));
    exp2_inverse_down(half)
}

pub fn gate(x: FixedU64, lo: FixedU64, hi: FixedU64) -> Result<FixedU64, Error> {
    ensure!(
        lo.0 < hi.0 && hi.0 <= ONE && x.0 <= ONE,
        Error::ValueOutOfRange
    );
    if x.0 <= lo.0 {
        return Ok(FixedU64(0));
    }
    if x.0 >= hi.0 {
        return Ok(FixedU64(ONE));
    }
    let t = (u128::from(x.0 - lo.0) * u128::from(ONE) / u128::from(hi.0 - lo.0)) as u64;
    let t2 = (u128::from(t) * u128::from(t) / u128::from(ONE)) as u64;
    let three_minus_2t = 3 * ONE - 2 * t;
    Ok(FixedU64(
        (u128::from(t2) * u128::from(three_minus_2t) / u128::from(ONE)) as u64,
    ))
}

fn mul_down(a: FixedU64, b: FixedU64) -> Result<FixedU64, Error> {
    Ok(FixedU64(
        (u128::from(a.0) * u128::from(b.0) / u128::from(ONE)) as u64,
    ))
}
fn q64_from_1e9(v: u64) -> Result<FixedU64x64, Error> {
    Ok(FixedU64x64::from_raw(
        (u128::from(v) << 64) / u128::from(ONE),
    ))
}
fn q64_to_1e9_down(v: FixedU64x64) -> Result<FixedU64, Error> {
    Ok(FixedU64(((v.raw() * u128::from(ONE)) >> 64) as u64))
}

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking {
    pub fn benchmark_stub() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    fn spec(id: MetricId, pillar: Pillar, weight: u64, version: u16) -> MetricSpec {
        MetricSpec {
            id,
            version,
            pillar,
            weight: FixedU64(weight),
            epsilon_floor: EPSILON_PILLAR,
            activation_epoch: 3,
            source: SourceClass::Onchain,
            formula_ref: [1; 32],
            units: [2; 16],
            repr: [3; 16],
            cadence_blocks: 1,
            sanity_min: FixedU64(0),
            sanity_max: FixedU64(ONE),
            has_normalization_rule: true,
            has_missing_data_rule: true,
            has_gaming_vectors: true,
            has_challenge_procedure: true,
            prior_bounds: [FixedU64(ONE); HISTORY_PRIORS],
        }
    }
    fn default_specs(version: u16) -> Vec<MetricSpec> {
        vec![
            spec(1, Pillar::S, ONE, version),
            spec(2, Pillar::COnchain, ONE, version),
            spec(3, Pillar::P, ONE, version),
            spec(4, Pillar::A, ONE, version),
        ]
    }
    #[test]
    fn metric_spec_registration_enforces_activation_disciplines_and_weight_sums() {
        let mut w = WelfareState::new();
        let mut specs = default_specs(1);
        specs[0].activation_epoch = 1;
        assert_eq!(
            w.register_metric_spec(0, 1, specs),
            Err(Error::BadActivationEpoch)
        );
        let mut specs = default_specs(1);
        specs[0].has_gaming_vectors = false;
        assert_eq!(
            w.register_metric_spec(0, 1, specs),
            Err(Error::MissingMetricDiscipline)
        );
        assert_eq!(w.register_metric_spec(0, 1, default_specs(1)), Ok(()));
    }
    #[test]
    fn daily_gate_uses_only_s_and_onchain_c_components() {
        let mut w = WelfareState::new();
        w.register_metric_spec(0, 1, default_specs(1)).unwrap();
        let flags = w
            .record_daily_gate(
                2,
                0,
                1,
                vec![
                    ComponentValue {
                        id: 1,
                        value: FixedU64(ONE),
                    },
                    ComponentValue {
                        id: 2,
                        value: FixedU64(ONE),
                    },
                ],
            )
            .unwrap();
        assert!(!flags.s_breached);
        assert!(!flags.c_breached);
    }
    #[test]
    fn c_weight_vector_is_joint_across_onchain_and_attested() {
        // 05 §4.4: one C weight vector across C_onchain and C_attested,
        // summing to 1 - which is what makes C_daily's renormalization over
        // the on-chain subset meaningful. Per-sub-pillar sums of 1 (joint 2)
        // must reject.
        let mut w = WelfareState::new();
        let mut specs = default_specs(1);
        specs.push(spec(5, Pillar::CAttested, ONE, 1));
        specs.sort_by_key(|s| s.id);
        assert_eq!(
            w.register_metric_spec(0, 1, specs),
            Err(Error::BadWeightSum)
        );
        // A split joint vector (0.8 on-chain + 0.2 attested) registers.
        let mut specs = default_specs(1);
        specs[1].weight = FixedU64(800_000_000);
        specs.push(spec(5, Pillar::CAttested, 200_000_000, 1));
        specs.sort_by_key(|s| s.id);
        assert_eq!(w.register_metric_spec(0, 1, specs), Ok(()));
    }

    #[test]
    fn daily_c_renormalizes_over_the_onchain_subset() {
        // 05 §4.4: C_daily = product over C_onchain of max(c, eps)^(w / sum_onchain w).
        // With the on-chain share at weight 0.8 of the joint vector, a daily
        // value of 0.84 renormalizes to 0.84^1 = 0.84 < theta_C_lo = 0.85
        // (breach); unrenormalized it would be 0.84^0.8 ~= 0.87 (no breach).
        let mut w = WelfareState::new();
        let mut specs = default_specs(1);
        specs[1].weight = FixedU64(800_000_000);
        specs.push(spec(5, Pillar::CAttested, 200_000_000, 1));
        specs.sort_by_key(|s| s.id);
        w.register_metric_spec(0, 1, specs).unwrap();
        let flags = w
            .record_daily_gate(
                2,
                0,
                1,
                vec![
                    ComponentValue {
                        id: 1,
                        value: FixedU64(ONE),
                    },
                    ComponentValue {
                        id: 2,
                        value: FixedU64(840_000_000),
                    },
                ],
            )
            .unwrap();
        assert!(flags.c_breached);
        assert!(!flags.s_breached);
    }

    #[test]
    fn composites_follow_the_normative_exp2_log2_pipeline() {
        // 05 §4.4 (2)/(4): weighted geometric terms evaluate as one
        // exp2(sum(w * log2(...))) and the settlement score as
        // exp2((log2 + log2)/2); the crate primitives are <= 2 ulp, so the
        // 1e9-grid results sit within a couple of units of the exact values.
        let within = |actual: FixedU64, expected: u64, tol: u64| {
            assert!(
                actual.0.abs_diff(expected) <= tol,
                "actual {} expected {expected}",
                actual.0
            );
        };
        // geo: 0.25^0.5 * 1^0.5 = 0.5
        within(
            geo_pair(
                (FixedU64(250_000_000), FixedU64(500_000_000)),
                (FixedU64(ONE), FixedU64(500_000_000)),
            )
            .unwrap(),
            500_000_000,
            2,
        );
        // settlement: geomean(1, 1) = 1 exactly; geomean(0.64, 0.25) = 0.4.
        assert_eq!(
            settlement_score(FixedU64(ONE), FixedU64(ONE)).unwrap(),
            FixedU64(ONE)
        );
        within(
            settlement_score(FixedU64(640_000_000), FixedU64(250_000_000)).unwrap(),
            400_000_000,
            2,
        );
        // The eps_W floor keeps a zeroed epoch's log finite:
        // geomean(1e-9, 0.5) ~= 2.2360679e-5.
        within(
            settlement_score(FixedU64(0), FixedU64(500_000_000)).unwrap(),
            22_360,
            2,
        );
    }

    #[test]
    fn try_state_rejects_corrupt_duplicate_storage() {
        let mut w = WelfareState::new();
        w.specs.push((1, default_specs(1)));
        w.specs.push((1, default_specs(1)));
        assert_eq!(w.try_state(), Err(Error::TryStateViolation));

        let mut w = WelfareState::new();
        w.snapshots.push(Snapshot {
            epoch: 1,
            spec_version: 1,
            s_pillar: FixedU64(ONE),
            c_onchain: FixedU64(ONE),
            c_attested: FixedU64(ONE),
            p_pillar: FixedU64(ONE),
            a_pillar: FixedU64(ONE),
            gate_s: FixedU64(ONE),
            gate_c: FixedU64(ONE),
            welfare: FixedU64(ONE),
            components: Vec::new(),
        });
        w.snapshots.push(w.snapshots[0].clone());
        assert_eq!(w.try_state(), Err(Error::TryStateViolation));
    }

    #[test]
    fn snapshot_rejects_duplicate_epoch_spec_and_bad_incident_multiplier() {
        let mut w = WelfareState::new();
        w.register_metric_spec(0, 1, default_specs(1)).unwrap();
        let comps = vec![
            ComponentValue {
                id: 1,
                value: FixedU64(ONE),
            },
            ComponentValue {
                id: 2,
                value: FixedU64(ONE),
            },
            ComponentValue {
                id: 3,
                value: FixedU64(ONE),
            },
            ComponentValue {
                id: 4,
                value: FixedU64(ONE),
            },
        ];
        assert_eq!(
            w.record_snapshot(7, 1, comps.clone(), FixedU64(ONE + 1)),
            Err(Error::ValueOutOfRange)
        );
        w.record_snapshot(7, 1, comps.clone(), FixedU64(ONE))
            .unwrap();
        assert_eq!(
            w.record_snapshot(7, 1, comps, FixedU64(ONE)),
            Err(Error::DuplicateSnapshot)
        );
    }

    #[test]
    fn snapshots_and_settlement_bind_creation_time_spec_version() {
        let mut w = WelfareState::new();
        w.register_metric_spec(0, 1, default_specs(1)).unwrap();
        w.register_metric_spec(0, 2, default_specs(2)).unwrap();
        let comps = vec![
            ComponentValue {
                id: 1,
                value: FixedU64(ONE),
            },
            ComponentValue {
                id: 2,
                value: FixedU64(ONE),
            },
            ComponentValue {
                id: 3,
                value: FixedU64(ONE),
            },
            ComponentValue {
                id: 4,
                value: FixedU64(ONE),
            },
        ];
        w.record_snapshot(11, 1, comps.clone(), FixedU64(ONE))
            .unwrap();
        w.record_snapshot(12, 1, comps, FixedU64(ONE)).unwrap();
        assert_eq!(w.compute_settlement(10, 1), Ok(FixedU64(ONE)));
        assert_eq!(w.compute_settlement(10, 2), Err(Error::MissingComponent));
    }
    #[test]
    fn gate_and_welfare_zero_on_security_breach() {
        assert_eq!(
            gate(FixedU64(850_000_000), THETA_C_LO, THETA_C_HI).unwrap(),
            FixedU64(0)
        );
        assert_eq!(
            compute_welfare(
                FixedU64(ONE),
                FixedU64(850_000_000),
                FixedU64(ONE),
                FixedU64(ONE)
            )
            .unwrap(),
            FixedU64(0)
        );
    }
    #[test]
    fn settlement_score_is_geometric_mean_with_epsilon_floor() {
        assert_eq!(
            settlement_score(FixedU64(ONE), FixedU64(ONE)).unwrap(),
            FixedU64(ONE)
        );
        assert_eq!(
            settlement_score(FixedU64(0), FixedU64(ONE)).unwrap().0,
            31_622
        );
    }
}
