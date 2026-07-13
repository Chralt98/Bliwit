#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! Unsigned 64.64 fixed-point primitives for Bleavit LMSR math.
//!
//! The market specification requires an internal 64.64 representation for LMSR
//! `exp2`, `log2`, `ln`, and cost calculations, with maker-adverse rounding at
//! currency boundaries. This crate keeps the fixed-point surface small and
//! deterministic; broader protocol types live in `futarchy-primitives`.

use core::cmp;
use core::fmt;
use core::ops::{Add, Div, Mul, Sub};

/// Number of fractional bits in [`FixedU64x64`].
pub const FRAC_BITS: u32 = 64;
/// One unit in raw 64.64 representation.
pub const ONE_RAW: u128 = 1u128 << FRAC_BITS;
/// Natural logarithm of two as an unsigned 64.64 constant.
pub const LN_2: FixedU64x64 = FixedU64x64(12_786_308_645_202_655_660);
/// Maximum approximation error target for a primitive transcendental call.
pub const PRIMITIVE_MAX_ULP: u32 = 2;
/// Maximum composed LMSR cost-function error target.
pub const COMPOSED_COST_MAX_ULP: u32 = 8;
/// Protocol domain clamp for LMSR exponent displacement `|q_l - q_s| / b`.
pub const LMSR_DOMAIN_CLAMP: FixedU64x64 = FixedU64x64(48u128 << FRAC_BITS);

const LN_2_F64: f64 = core::f64::consts::LN_2;
const INV_LN_2_F64: f64 = core::f64::consts::LOG2_E;

fn floor_f64(x: f64) -> f64 {
    let truncated = x as i64;
    if x < 0.0 && (truncated as f64) != x {
        (truncated - 1) as f64
    } else {
        truncated as f64
    }
}

fn ceil_f64(x: f64) -> f64 {
    let truncated = x as i64;
    if x > 0.0 && (truncated as f64) != x {
        (truncated + 1) as f64
    } else {
        truncated as f64
    }
}

fn round_f64(x: f64) -> f64 {
    if x >= 0.0 {
        floor_f64(x + 0.5)
    } else {
        ceil_f64(x - 0.5)
    }
}

fn exp_f64(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    if x < -745.0 {
        return 0.0;
    }
    if x > 709.0 {
        return f64::INFINITY;
    }
    let n = round_f64(x * INV_LN_2_F64);
    let r = x - n * LN_2_F64;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut k = 1.0;
    while k <= 18.0 {
        term *= r / k;
        sum += term;
        k += 1.0;
    }
    scale_pow2(sum, n as i32)
}

fn exp2_f64(x: f64) -> f64 {
    let n = floor_f64(x);
    exp_f64((x - n) * LN_2_F64) * scale_pow2(1.0, n as i32)
}

fn ln_f64(x: f64) -> f64 {
    log2_f64(x) * LN_2_F64
}

fn log2_f64(x: f64) -> f64 {
    if x <= 0.0 {
        return f64::NAN;
    }
    let bits = x.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    if exponent == 0 {
        return log2_f64(x * scale_pow2(1.0, 64)) - 64.0;
    }
    let e = exponent - 1023;
    let mantissa_bits = (bits & ((1u64 << 52) - 1)) | (1023u64 << 52);
    let m = f64::from_bits(mantissa_bits);
    let z = (m - 1.0) / (m + 1.0);
    let z2 = z * z;
    let mut term = z;
    let mut sum = 0.0;
    let mut denom = 1.0;
    for _ in 0..32 {
        sum += term / denom;
        term *= z2;
        denom += 2.0;
    }
    e as f64 + (2.0 * sum) * INV_LN_2_F64
}

fn scale_pow2(x: f64, n: i32) -> f64 {
    if x == 0.0 {
        return x;
    }
    if n > 1023 {
        return f64::INFINITY;
    }
    if n < -1074 {
        return 0.0;
    }
    x * f64::from_bits(((n + 1023) as u64) << 52)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedError {
    DivisionByZero,
    Domain,
    Overflow,
    NonFinite,
}

impl fmt::Display for FixedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DivisionByZero => f.write_str("division by zero"),
            Self::Domain => f.write_str("input outside fixed-point function domain"),
            Self::Overflow => f.write_str("fixed-point overflow"),
            Self::NonFinite => f.write_str("non-finite floating-point conversion"),
        }
    }
}

/// Unsigned 64.64 fixed-point value backed by `u128`.
#[derive(Clone, Copy, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct FixedU64x64(u128);

impl FixedU64x64 {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(ONE_RAW);

    pub const fn from_raw(raw: u128) -> Self {
        Self(raw)
    }
    pub const fn raw(self) -> u128 {
        self.0
    }
    pub const fn from_integer(value: u64) -> Self {
        Self((value as u128) << FRAC_BITS)
    }

    pub fn checked_add(self, rhs: Self) -> Result<Self, FixedError> {
        self.0
            .checked_add(rhs.0)
            .map(Self)
            .ok_or(FixedError::Overflow)
    }
    pub fn checked_sub(self, rhs: Self) -> Result<Self, FixedError> {
        self.0
            .checked_sub(rhs.0)
            .map(Self)
            .ok_or(FixedError::Domain)
    }
    pub fn checked_mul(self, rhs: Self) -> Result<Self, FixedError> {
        let a_hi = self.0 >> FRAC_BITS;
        let a_lo = self.0 & (ONE_RAW - 1);
        let b_hi = rhs.0 >> FRAC_BITS;
        let b_lo = rhs.0 & (ONE_RAW - 1);

        let whole = a_hi
            .checked_mul(b_hi)
            .and_then(|v| v.checked_shl(FRAC_BITS))
            .ok_or(FixedError::Overflow)?;
        let cross_ab = a_hi.checked_mul(b_lo).ok_or(FixedError::Overflow)?;
        let cross_ba = b_hi.checked_mul(a_lo).ok_or(FixedError::Overflow)?;
        let frac = a_lo
            .checked_mul(b_lo)
            .and_then(|v| v.checked_add(ONE_RAW - 1))
            .ok_or(FixedError::Overflow)?
            >> FRAC_BITS;
        whole
            .checked_add(cross_ab)
            .and_then(|v| v.checked_add(cross_ba))
            .and_then(|v| v.checked_add(frac))
            .map(Self)
            .ok_or(FixedError::Overflow)
    }
    pub fn checked_div(self, rhs: Self) -> Result<Self, FixedError> {
        if rhs.0 == 0 {
            return Err(FixedError::DivisionByZero);
        }
        if self.0 <= u128::MAX >> FRAC_BITS {
            Ok(Self((self.0 << FRAC_BITS) / rhs.0))
        } else {
            Self::from_f64(self.to_f64() / rhs.to_f64())
        }
    }

    /// Convert to a nearest `f64` for deterministic transcendental kernels and tests.
    pub fn to_f64(self) -> f64 {
        (self.0 as f64) / (ONE_RAW as f64)
    }

    /// Convert a finite non-negative `f64` to nearest 64.64.
    pub fn from_f64(value: f64) -> Result<Self, FixedError> {
        if !value.is_finite() {
            return Err(FixedError::NonFinite);
        }
        if value < 0.0 {
            return Err(FixedError::Domain);
        }
        if value >= (u64::MAX as f64) {
            return Err(FixedError::Overflow);
        }
        let whole = floor_f64(value);
        let frac = value - whole;
        let frac_raw = (frac * (ONE_RAW as f64) + 0.5) as u128;
        let whole_raw = (whole as u128)
            .checked_shl(FRAC_BITS)
            .ok_or(FixedError::Overflow)?;
        whole_raw
            .checked_add(frac_raw)
            .map(Self)
            .ok_or(FixedError::Overflow)
    }

    /// Fixed-point base-2 exponent.
    ///
    /// The implementation is available in `no_std` builds through deterministic
    /// in-crate range reduction and series helpers; the full integer polynomial
    /// kernel and MPFR corpus gate are tracked in M2.
    pub fn exp2(self) -> Result<Self, FixedError> {
        Self::from_f64(exp2_f64(self.to_f64()))
    }
    /// Fixed-point base-2 logarithm. Zero is outside the logarithm domain.
    pub fn log2(self) -> Result<Self, FixedError> {
        if self.0 == 0 {
            return Err(FixedError::Domain);
        }
        Self::from_f64(log2_f64(self.to_f64()))
    }
    /// Fixed-point natural logarithm.
    pub fn ln(self) -> Result<Self, FixedError> {
        if self.0 == 0 {
            return Err(FixedError::Domain);
        }
        Self::from_f64(ln_f64(self.to_f64()))
    }
}

impl fmt::Debug for FixedU64x64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FixedU64x64").field(&self.0).finish()
    }
}

/// Round a fixed-point charge up to integer base units.
pub const fn round_charge_up(value: FixedU64x64) -> u128 {
    let whole = value.raw() >> FRAC_BITS;
    let frac = value.raw() & (ONE_RAW - 1);
    whole + if frac == 0 { 0 } else { 1 }
}

/// Round a fixed-point payout/proceed down to integer base units.
pub const fn round_payout_down(value: FixedU64x64) -> u128 {
    value.raw() >> FRAC_BITS
}

fn ensure_lmsr_domain(
    q_l: FixedU64x64,
    q_s: FixedU64x64,
    b: FixedU64x64,
) -> Result<(), FixedError> {
    if b.raw() == 0 {
        return Err(FixedError::DivisionByZero);
    }
    let max_q = cmp::max(q_l, q_s);
    let min_q = cmp::min(q_l, q_s);
    let displacement = max_q.checked_sub(min_q)?.checked_div(b)?;
    if displacement > LMSR_DOMAIN_CLAMP {
        return Err(FixedError::Domain);
    }
    Ok(())
}

/// Stable two-outcome LMSR cost `max(q_l,q_s) + b*ln(1 + exp(-|q_l-q_s|/b))`.
///
/// The protocol domain is enforced rather than silently clamped: states with
/// `|q_l - q_s| / b > 48` must be rejected by callers before accepting a trade.
pub fn lmsr_cost(
    q_l: FixedU64x64,
    q_s: FixedU64x64,
    b: FixedU64x64,
) -> Result<FixedU64x64, FixedError> {
    ensure_lmsr_domain(q_l, q_s, b)?;
    let max_q = cmp::max(q_l, q_s);
    let min_q = cmp::min(q_l, q_s);
    let displacement = max_q.checked_sub(min_q)?.checked_div(b)?;
    // exp(-d) is evaluated through the same in-crate helper in std and no_std builds.
    let exp_neg = FixedU64x64::from_f64(exp_f64(-displacement.to_f64()))?;
    let log_term = FixedU64x64::ONE.checked_add(exp_neg)?.ln()?;
    max_q.checked_add(b.checked_mul(log_term)?)
}

/// Stable two-outcome LMSR long-side marginal price `p_L`.
///
/// The price is evaluated as a logistic function of `(q_L - q_S) / b` and
/// is subject to the same `|q_L - q_S| / b <= 48` domain as the cost path.
pub fn lmsr_price_long(
    q_l: FixedU64x64,
    q_s: FixedU64x64,
    b: FixedU64x64,
) -> Result<FixedU64x64, FixedError> {
    ensure_lmsr_domain(q_l, q_s, b)?;
    let max_q = cmp::max(q_l, q_s);
    let min_q = cmp::min(q_l, q_s);
    let displacement = max_q.checked_sub(min_q)?.checked_div(b)?;
    let exp_neg = FixedU64x64::from_f64(exp_f64(-displacement.to_f64()))?;
    let denom = FixedU64x64::ONE.checked_add(exp_neg)?;
    if q_l >= q_s {
        FixedU64x64::ONE.checked_div(denom)
    } else {
        exp_neg.checked_div(denom)
    }
}

/// Stable two-outcome LMSR short-side marginal price `p_S = 1 - p_L`.
pub fn lmsr_price_short(
    q_l: FixedU64x64,
    q_s: FixedU64x64,
    b: FixedU64x64,
) -> Result<FixedU64x64, FixedError> {
    FixedU64x64::ONE.checked_sub(lmsr_price_long(q_l, q_s, b)?)
}

/// Quantity displacement required to move a two-outcome LMSR from one price
/// to another: `b * (logit(p_to) - logit(p_from))`.
///
/// Both prices must be strictly inside `(0, 1)`. The returned value is an
/// unsigned magnitude; callers choose which side to buy or sell from the sign
/// implied by the price movement.
pub fn lmsr_displacement_between_prices(
    b: FixedU64x64,
    p_from: FixedU64x64,
    p_to: FixedU64x64,
) -> Result<FixedU64x64, FixedError> {
    fn logit(price: FixedU64x64) -> Result<f64, FixedError> {
        if price.raw() == 0 || price >= FixedU64x64::ONE {
            return Err(FixedError::Domain);
        }
        Ok(ln_f64(price.to_f64() / (1.0 - price.to_f64())))
    }

    let from = logit(p_from)?;
    let to = logit(p_to)?;
    FixedU64x64::from_f64((to - from).abs())?.checked_mul(b)
}

/// Side of a two-outcome LMSR book.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LmsrSide {
    Long,
    Short,
}

/// Exact fixed-point cost of buying `amount` on `side`, before maker-adverse
/// currency rounding and fees.
pub fn lmsr_buy_cost(
    q_l: FixedU64x64,
    q_s: FixedU64x64,
    b: FixedU64x64,
    side: LmsrSide,
    amount: FixedU64x64,
) -> Result<FixedU64x64, FixedError> {
    let before = lmsr_cost(q_l, q_s, b)?;
    let (new_l, new_s) = match side {
        LmsrSide::Long => (q_l.checked_add(amount)?, q_s),
        LmsrSide::Short => (q_l, q_s.checked_add(amount)?),
    };
    let after = lmsr_cost(new_l, new_s, b)?;
    after.checked_sub(before)
}

/// Exact fixed-point proceeds from selling `amount` on `side`, before
/// maker-adverse currency rounding and fees.
pub fn lmsr_sell_proceeds(
    q_l: FixedU64x64,
    q_s: FixedU64x64,
    b: FixedU64x64,
    side: LmsrSide,
    amount: FixedU64x64,
) -> Result<FixedU64x64, FixedError> {
    let before = lmsr_cost(q_l, q_s, b)?;
    let (new_l, new_s) = match side {
        LmsrSide::Long => (q_l.checked_sub(amount)?, q_s),
        LmsrSide::Short => (q_l, q_s.checked_sub(amount)?),
    };
    let after = lmsr_cost(new_l, new_s, b)?;
    before.checked_sub(after)
}

impl Add for FixedU64x64 {
    type Output = Result<Self, FixedError>;
    fn add(self, rhs: Self) -> Self::Output {
        self.checked_add(rhs)
    }
}
impl Sub for FixedU64x64 {
    type Output = Result<Self, FixedError>;
    fn sub(self, rhs: Self) -> Self::Output {
        self.checked_sub(rhs)
    }
}
impl Mul for FixedU64x64 {
    type Output = Result<Self, FixedError>;
    fn mul(self, rhs: Self) -> Self::Output {
        self.checked_mul(rhs)
    }
}
impl Div for FixedU64x64 {
    type Output = Result<Self, FixedError>;
    fn div(self, rhs: Self) -> Self::Output {
        self.checked_div(rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(actual: FixedU64x64, expected: f64, tolerance: f64) {
        let diff = (actual.to_f64() - expected).abs();
        assert!(
            diff <= tolerance,
            "actual={} expected={} diff={} tolerance={}",
            actual.to_f64(),
            expected,
            diff,
            tolerance
        );
    }

    fn assert_raw_within(actual: FixedU64x64, expected_raw: u128, max_raw_error: u128) {
        let actual_raw = actual.raw();
        let diff = actual_raw.abs_diff(expected_raw);
        assert!(
            diff <= max_raw_error,
            "actual_raw={actual_raw} expected_raw={expected_raw} diff={diff} max={max_raw_error}"
        );
    }

    fn corpus_value(name: &str) -> FixedU64x64 {
        match name {
            "cost_0_0" => lmsr_cost(
                FixedU64x64::ZERO,
                FixedU64x64::ZERO,
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "v1_buy_1000_long_cost" => lmsr_buy_cost(
                FixedU64x64::ZERO,
                FixedU64x64::ZERO,
                FixedU64x64::from_integer(10_000),
                LmsrSide::Long,
                FixedU64x64::from_integer(1_000),
            )
            .unwrap(),
            "v2_price_after_v1" => lmsr_price_long(
                FixedU64x64::from_integer(1_000),
                FixedU64x64::ZERO,
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "v3_displace_0_5_to_0_6" => lmsr_displacement_between_prices(
                FixedU64x64::from_integer(10_000),
                FixedU64x64::from_f64(0.5).unwrap(),
                FixedU64x64::from_f64(0.6).unwrap(),
            )
            .unwrap(),
            "v3_cost_0_5_to_0_6" => {
                let delta = lmsr_displacement_between_prices(
                    FixedU64x64::from_integer(10_000),
                    FixedU64x64::from_f64(0.5).unwrap(),
                    FixedU64x64::from_f64(0.6).unwrap(),
                )
                .unwrap();
                lmsr_buy_cost(
                    FixedU64x64::ZERO,
                    FixedU64x64::ZERO,
                    FixedU64x64::from_integer(10_000),
                    LmsrSide::Long,
                    delta,
                )
                .unwrap()
            }
            "v4_worst_case_loss" => FixedU64x64::from_integer(10_000).checked_mul(LN_2).unwrap(),
            "domain_edge_cost_480000_0" => lmsr_cost(
                FixedU64x64::from_integer(480_000),
                FixedU64x64::ZERO,
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "cost_2500_0" => lmsr_cost(
                FixedU64x64::from_integer(2_500),
                FixedU64x64::ZERO,
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "price_2500_0" => lmsr_price_long(
                FixedU64x64::from_integer(2_500),
                FixedU64x64::ZERO,
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "cost_0_2500" => lmsr_cost(
                FixedU64x64::ZERO,
                FixedU64x64::from_integer(2_500),
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "price_0_2500" => lmsr_price_long(
                FixedU64x64::ZERO,
                FixedU64x64::from_integer(2_500),
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "cost_12345_6789" => lmsr_cost(
                FixedU64x64::from_integer(12_345),
                FixedU64x64::from_integer(6_789),
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "price_12345_6789" => lmsr_price_long(
                FixedU64x64::from_integer(12_345),
                FixedU64x64::from_integer(6_789),
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "cost_6789_12345" => lmsr_cost(
                FixedU64x64::from_integer(6_789),
                FixedU64x64::from_integer(12_345),
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "price_6789_12345" => lmsr_price_long(
                FixedU64x64::from_integer(6_789),
                FixedU64x64::from_integer(12_345),
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "cost_240000_0" => lmsr_cost(
                FixedU64x64::from_integer(240_000),
                FixedU64x64::ZERO,
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "price_240000_0" => lmsr_price_long(
                FixedU64x64::from_integer(240_000),
                FixedU64x64::ZERO,
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "cost_0_240000" => lmsr_cost(
                FixedU64x64::ZERO,
                FixedU64x64::from_integer(240_000),
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            "price_0_240000" => lmsr_price_long(
                FixedU64x64::ZERO,
                FixedU64x64::from_integer(240_000),
                FixedU64x64::from_integer(10_000),
            )
            .unwrap(),
            other => panic!("unknown corpus row: {other}"),
        }
    }

    #[test]
    fn generated_lmsr_corpus_matches_committed_values() {
        let corpus = include_str!("../fixtures/lmsr_corpus.csv");
        let mut checked = 0u32;
        for line in corpus.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields = line.split(',');
            let name = fields.next().unwrap();
            let expected = fields.next().unwrap().parse::<f64>().unwrap();
            let expected_raw = fields.next().unwrap().parse::<u128>().unwrap();
            let actual = corpus_value(name);
            approx(actual, expected, 1e-8);
            assert_raw_within(actual, expected_raw, 200_000_000_000);
            checked += 1;
        }
        assert_eq!(checked, 19);
    }

    #[test]
    fn constants_match_reference_values() {
        approx(LN_2, core::f64::consts::LN_2, 1e-18);
        assert_eq!(PRIMITIVE_MAX_ULP, 2);
        assert_eq!(COMPOSED_COST_MAX_ULP, 8);
    }

    #[test]
    fn exp2_log2_ln_match_reference_samples() {
        for value in [0.125_f64, 0.5, 1.0, 1.5, 2.0, 8.0, 48.0] {
            let fixed = FixedU64x64::from_f64(value).unwrap();
            approx(fixed.exp2().unwrap(), value.exp2(), 1e-12);
        }
        for value in [1.0_f64, 1.5, 2.0, 8.0, 48.0] {
            let fixed = FixedU64x64::from_f64(value).unwrap();
            approx(fixed.ln().unwrap(), value.ln(), 1e-12);
            approx(fixed.log2().unwrap(), value.log2(), 1e-12);
        }
    }

    #[test]
    fn log_zero_is_rejected() {
        assert_eq!(FixedU64x64::ZERO.ln(), Err(FixedError::Domain));
        assert_eq!(FixedU64x64::ZERO.log2(), Err(FixedError::Domain));
    }

    #[test]
    fn lmsr_vectors_v2_and_v3_match_spec() {
        let b = FixedU64x64::from_integer(10_000);
        let price_after_v1 =
            lmsr_price_long(FixedU64x64::from_integer(1_000), FixedU64x64::ZERO, b).unwrap();
        approx(price_after_v1, 0.524979187479, 1e-12);
        approx(
            lmsr_price_short(FixedU64x64::from_integer(1_000), FixedU64x64::ZERO, b).unwrap(),
            0.475020812521,
            1e-12,
        );

        let displacement = lmsr_displacement_between_prices(
            b,
            FixedU64x64::from_f64(0.5).unwrap(),
            FixedU64x64::from_f64(0.6).unwrap(),
        )
        .unwrap();
        approx(displacement, 4054.65108108, 1e-8);

        let cost = lmsr_buy_cost(
            FixedU64x64::ZERO,
            FixedU64x64::ZERO,
            b,
            LmsrSide::Long,
            displacement,
        )
        .unwrap();
        approx(cost, 2231.43551314, 1e-8);
    }

    #[test]
    fn lmsr_vectors_v1_and_v4_match_spec() {
        let b = FixedU64x64::from_integer(10_000);
        let c0 = lmsr_cost(FixedU64x64::ZERO, FixedU64x64::ZERO, b).unwrap();
        approx(c0, 6931.47180560, 1e-8);
        let c1 = lmsr_cost(FixedU64x64::from_integer(1_000), FixedU64x64::ZERO, b).unwrap();
        approx(c1.checked_sub(c0).unwrap(), 512.494795136, 1e-8);
        let buy = lmsr_buy_cost(
            FixedU64x64::ZERO,
            FixedU64x64::ZERO,
            b,
            LmsrSide::Long,
            FixedU64x64::from_integer(1_000),
        )
        .unwrap();
        approx(buy, 512.494795136, 1e-8);
        let sell = lmsr_sell_proceeds(
            FixedU64x64::from_integer(1_000),
            FixedU64x64::ZERO,
            b,
            LmsrSide::Long,
            FixedU64x64::from_integer(1_000),
        )
        .unwrap();
        approx(sell, 512.494795136, 1e-8);
    }

    #[test]
    fn lmsr_domain_edge_rejects_past_clamp() {
        let b = FixedU64x64::from_integer(10_000);
        assert_eq!(
            lmsr_cost(FixedU64x64::from_integer(480_001), FixedU64x64::ZERO, b),
            Err(FixedError::Domain)
        );
        assert_eq!(
            lmsr_buy_cost(
                FixedU64x64::from_integer(480_000),
                FixedU64x64::ZERO,
                b,
                LmsrSide::Long,
                FixedU64x64::from_integer(1),
            ),
            Err(FixedError::Domain)
        );
        assert!(lmsr_buy_cost(
            FixedU64x64::from_integer(479_999),
            FixedU64x64::ZERO,
            b,
            LmsrSide::Long,
            FixedU64x64::from_integer(1),
        )
        .is_ok());
    }

    #[test]
    fn maker_adverse_rounding_charges_up_and_payouts_down() {
        let exact = FixedU64x64::from_integer(42);
        assert_eq!(round_charge_up(exact), 42);
        assert_eq!(round_payout_down(exact), 42);
        let fractional = FixedU64x64::from_raw((42u128 << FRAC_BITS) + 1);
        assert_eq!(round_charge_up(fractional), 43);
        assert_eq!(round_payout_down(fractional), 42);
    }
}
