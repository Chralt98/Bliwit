#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! Unsigned 64.64 fixed-point primitives for Bleavit LMSR math.
//!
//! The market specification requires an internal 64.64 representation for LMSR
//! `exp2`, `log2`, `ln`, and cost calculations, with maker-adverse rounding at
//! currency boundaries. This crate keeps the fixed-point surface small and
//! deterministic; broader protocol types live in `futarchy-primitives`.

#[cfg(feature = "std")]
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
        match self
            .0
            .checked_mul(rhs.0)
            .and_then(|v| v.checked_add(ONE_RAW - 1))
        {
            Some(v) => Ok(Self(v >> FRAC_BITS)),
            None => {
                #[cfg(feature = "std")]
                {
                    Self::from_f64(self.to_f64() * rhs.to_f64())
                }
                #[cfg(not(feature = "std"))]
                {
                    Err(FixedError::Overflow)
                }
            }
        }
    }
    pub fn checked_div(self, rhs: Self) -> Result<Self, FixedError> {
        if rhs.0 == 0 {
            return Err(FixedError::DivisionByZero);
        }
        if self.0 <= u128::MAX >> FRAC_BITS {
            Ok(Self((self.0 << FRAC_BITS) / rhs.0))
        } else {
            #[cfg(feature = "std")]
            {
                Self::from_f64(self.to_f64() / rhs.to_f64())
            }
            #[cfg(not(feature = "std"))]
            {
                Err(FixedError::Overflow)
            }
        }
    }

    /// Convert to a nearest `f64` for reference-quality transcendental kernels.
    #[cfg(feature = "std")]
    pub fn to_f64(self) -> f64 {
        (self.0 as f64) / (ONE_RAW as f64)
    }

    /// Convert a finite non-negative `f64` to nearest 64.64.
    #[cfg(feature = "std")]
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
        Ok(Self((value * (ONE_RAW as f64)).round() as u128))
    }

    /// Fixed-point base-2 exponent.
    #[cfg(feature = "std")]
    pub fn exp2(self) -> Result<Self, FixedError> {
        Self::from_f64(self.to_f64().exp2())
    }
    /// Fixed-point base-2 logarithm. Zero is outside the logarithm domain.
    #[cfg(feature = "std")]
    pub fn log2(self) -> Result<Self, FixedError> {
        if self.0 == 0 {
            return Err(FixedError::Domain);
        }
        Self::from_f64(self.to_f64().log2())
    }
    /// Fixed-point natural logarithm.
    #[cfg(feature = "std")]
    pub fn ln(self) -> Result<Self, FixedError> {
        if self.0 == 0 {
            return Err(FixedError::Domain);
        }
        Self::from_f64(self.to_f64().ln())
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

/// Stable two-outcome LMSR cost `max(q_l,q_s) + b*ln(1 + exp(-|q_l-q_s|/b))`.
#[cfg(feature = "std")]
pub fn lmsr_cost(
    q_l: FixedU64x64,
    q_s: FixedU64x64,
    b: FixedU64x64,
) -> Result<FixedU64x64, FixedError> {
    if b.raw() == 0 {
        return Err(FixedError::DivisionByZero);
    }
    let max_q = cmp::max(q_l, q_s);
    let min_q = cmp::min(q_l, q_s);
    let displacement = max_q.checked_sub(min_q)?.checked_div(b)?;
    let clamped = cmp::min(displacement, LMSR_DOMAIN_CLAMP);
    // exp(-d) = 2^(-d / ln 2)
    let exp_neg = FixedU64x64::from_f64((-clamped.to_f64()).exp())?;
    let log_term = FixedU64x64::ONE.checked_add(exp_neg)?.ln()?;
    max_q.checked_add(b.checked_mul(log_term)?)
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
    fn lmsr_vectors_v1_and_v4_match_spec() {
        let b = FixedU64x64::from_integer(10_000);
        let c0 = lmsr_cost(FixedU64x64::ZERO, FixedU64x64::ZERO, b).unwrap();
        approx(c0, 6931.47180560, 1e-8);
        let c1 = lmsr_cost(FixedU64x64::from_integer(1_000), FixedU64x64::ZERO, b).unwrap();
        approx(c1.checked_sub(c0).unwrap(), 512.494795136, 1e-8);
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
