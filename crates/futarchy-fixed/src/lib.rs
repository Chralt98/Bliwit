#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! Unsigned 64.64 fixed-point primitives for Bleavit LMSR math.
//!
//! The market specification requires an internal 64.64 representation for LMSR
//! `exp2`, `log2`, `ln`, and cost calculations, with deterministic integer
//! range reduction/iteration and maker-adverse rounding at currency boundaries.
//! This crate keeps the fixed-point surface small and deterministic; broader
//! protocol types live in `futarchy-primitives`.

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

const EXP2_FRAC_FACTORS: [u128; 64] = [
    26087635650665564425,
    21936999301089678047,
    20116317054877281742,
    19263451207323153962,
    18850675170876015534,
    18647615946650685159,
    18546908069882975960,
    18496758270674070881,
    18471734244850835106,
    18459234930309000272,
    18452988445124272033,
    18449865995240371898,
    18448304968436414829,
    18447524504564044946,
    18447134285009651015,
    18446939178327825412,
    18446841625760745902,
    18446792849670663277,
    18446768461673986097,
    18446756267687738522,
    18446750170697637486,
    18446747122203342655,
    18446745597956384162,
    18446744835832952145,
    18446744454771247945,
    18446744264240398796,
    18446744168974974960,
    18446744121342263227,
    18446744097525907406,
    18446744085617729507,
    18446744079663640561,
    18446744076686596088,
    18446744075198073852,
    18446744074453812734,
    18446744074081682175,
    18446744073895616895,
    18446744073802584256,
    18446744073756067936,
    18446744073732809776,
    18446744073721180696,
    18446744073715366156,
    18446744073712458886,
    18446744073711005251,
    18446744073710278433,
    18446744073709915025,
    18446744073709733320,
    18446744073709642468,
    18446744073709597042,
    18446744073709574329,
    18446744073709562973,
    18446744073709557294,
    18446744073709554455,
    18446744073709553036,
    18446744073709552326,
    18446744073709551971,
    18446744073709551793,
    18446744073709551705,
    18446744073709551660,
    18446744073709551638,
    18446744073709551627,
    18446744073709551622,
    18446744073709551619,
    18446744073709551617,
    18446744073709551617,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct U256 {
    hi: u128,
    lo: u128,
}

impl U256 {
    const ZERO: Self = Self { hi: 0, lo: 0 };

    fn shl64(value: u128) -> Self {
        Self {
            hi: value >> FRAC_BITS,
            lo: value << FRAC_BITS,
        }
    }

    fn mul_u128(lhs: u128, rhs: u128) -> Self {
        const MASK: u128 = (1u128 << 64) - 1;
        let a0 = lhs & MASK;
        let a1 = lhs >> 64;
        let b0 = rhs & MASK;
        let b1 = rhs >> 64;

        let p0 = a0 * b0;
        let p1 = a0 * b1;
        let p2 = a1 * b0;
        let p3 = a1 * b1;

        let carry = (p0 >> 64) + (p1 & MASK) + (p2 & MASK);
        let lo = (p0 & MASK) | (carry << 64);
        let hi = p3 + (p1 >> 64) + (p2 >> 64) + (carry >> 64);
        Self { hi, lo }
    }

    fn shr64_to_u128(self) -> Result<u128, FixedError> {
        if self.hi >> 64 != 0 {
            return Err(FixedError::Overflow);
        }
        Ok((self.hi << 64) | (self.lo >> 64))
    }

    fn shr64_round_to_u128(self) -> Result<u128, FixedError> {
        let truncated = self.shr64_to_u128()?;
        let round_bit = (self.lo >> 63) & 1;
        if round_bit == 0 {
            return Ok(truncated);
        }
        truncated.checked_add(1).ok_or(FixedError::Overflow)
    }

    fn bit(self, index: u32) -> bool {
        if index < 128 {
            ((self.lo >> index) & 1) == 1
        } else {
            ((self.hi >> (index - 128)) & 1) == 1
        }
    }

    fn div_u128(self, divisor: u128) -> Result<u128, FixedError> {
        if divisor == 0 {
            return Err(FixedError::DivisionByZero);
        }
        let mut rem = Self::ZERO;
        let mut quotient = 0u128;
        for bit in (0..256u32).rev() {
            rem = rem.shl1()?;
            if self.bit(bit) {
                rem.lo |= 1;
            }
            if rem.ge_u128(divisor) {
                rem = rem.sub_u128(divisor);
                if bit >= 128 {
                    return Err(FixedError::Overflow);
                }
                quotient |= 1u128 << bit;
            }
        }
        Ok(quotient)
    }

    fn shl1(self) -> Result<Self, FixedError> {
        if (self.hi >> 127) != 0 {
            return Err(FixedError::Overflow);
        }
        Ok(Self {
            hi: (self.hi << 1) | (self.lo >> 127),
            lo: self.lo << 1,
        })
    }

    fn ge_u128(self, rhs: u128) -> bool {
        self.hi != 0 || self.lo >= rhs
    }

    fn sub_u128(self, rhs: u128) -> Self {
        let (lo, borrow) = self.lo.overflowing_sub(rhs);
        Self {
            hi: self.hi - u128::from(borrow),
            lo,
        }
    }
}

fn mul_q64_round(lhs: u128, rhs: u128) -> Result<u128, FixedError> {
    U256::mul_u128(lhs, rhs).shr64_round_to_u128()
}

fn exp2_positive_raw(raw: u128) -> Result<u128, FixedError> {
    let whole = raw >> FRAC_BITS;
    if whole >= 64 {
        return Err(FixedError::Overflow);
    }
    let mut result = ONE_RAW;
    let frac = raw & (ONE_RAW - 1);
    for (index, factor) in EXP2_FRAC_FACTORS.iter().enumerate() {
        if ((frac >> (63 - index)) & 1) == 1 {
            result = mul_q64_round(result, *factor)?;
        }
    }
    result.checked_shl(whole as u32).ok_or(FixedError::Overflow)
}

fn exp2_negative_raw(raw: u128) -> Result<u128, FixedError> {
    let whole = raw >> FRAC_BITS;
    if whole >= 128 {
        return Ok(0);
    }
    let frac = raw & (ONE_RAW - 1);
    let positive_frac = exp2_positive_raw(frac)?;
    let inverse_frac = U256::shl64(ONE_RAW).div_u128(positive_frac)?;
    if whole == 0 {
        return Ok(inverse_frac);
    }
    let half = 1u128 << ((whole - 1) as u32);
    Ok((inverse_frac + half) >> (whole as u32))
}

fn log2_raw(raw: u128) -> Result<u128, FixedError> {
    if raw == 0 {
        return Err(FixedError::Domain);
    }

    let bit_index = 127 - raw.leading_zeros();
    let integer = i32::try_from(bit_index).map_err(|_| FixedError::Overflow)? - 64;
    if integer < 0 {
        return Err(FixedError::Domain);
    }

    let mut normalized = if integer >= 0 {
        raw >> (integer as u32)
    } else {
        raw << ((-integer) as u32)
    };
    let mut fraction = 0u128;
    for index in 0..64u32 {
        normalized = mul_q64_round(normalized, normalized)?;
        if normalized >= (2 * ONE_RAW) {
            normalized = (normalized + 1) >> 1;
            fraction |= 1u128 << (63 - index);
        }
    }

    ((integer as u128) << FRAC_BITS)
        .checked_add(fraction)
        .ok_or(FixedError::Overflow)
}

fn ln_one_plus(x: FixedU64x64) -> Result<FixedU64x64, FixedError> {
    FixedU64x64::ONE.checked_add(x)?.ln()
}

fn floor_f64(value: f64) -> f64 {
    let truncated = value as i64;
    if value < 0.0 && (truncated as f64) != value {
        (truncated - 1) as f64
    } else {
        truncated as f64
    }
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
        U256::mul_u128(self.0, rhs.0).shr64_to_u128().map(Self)
    }
    pub fn checked_div(self, rhs: Self) -> Result<Self, FixedError> {
        U256::shl64(self.0).div_u128(rhs.0).map(Self)
    }

    /// Convert to a nearest `f64` for diagnostics and tests.
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
    pub fn exp2(self) -> Result<Self, FixedError> {
        exp2_positive_raw(self.0).map(Self)
    }
    /// Fixed-point base-2 logarithm. Zero is outside the logarithm domain.
    pub fn log2(self) -> Result<Self, FixedError> {
        if self.0 == 0 {
            return Err(FixedError::Domain);
        }
        log2_raw(self.0).map(Self)
    }
    /// Fixed-point natural logarithm.
    pub fn ln(self) -> Result<Self, FixedError> {
        if self.0 == 0 {
            return Err(FixedError::Domain);
        }
        log2_raw(self.0)
            .and_then(|log2| mul_q64_round(log2, LN_2.raw()))
            .map(Self)
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
    let exp_neg = displacement
        .checked_div(LN_2)
        .and_then(|x| exp2_negative_raw(x.raw()).map(FixedU64x64))?;
    let log_term = ln_one_plus(exp_neg)?;
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
    let exp_neg = displacement
        .checked_div(LN_2)
        .and_then(|x| exp2_negative_raw(x.raw()).map(FixedU64x64))?;
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
    fn logit_signed_magnitude(price: FixedU64x64) -> Result<(bool, FixedU64x64), FixedError> {
        if price.raw() == 0 || price >= FixedU64x64::ONE {
            return Err(FixedError::Domain);
        }
        let complement = FixedU64x64::ONE.checked_sub(price)?;
        if price >= complement {
            Ok((true, price.checked_div(complement)?.ln()?))
        } else {
            Ok((false, complement.checked_div(price)?.ln()?))
        }
    }

    let (from_positive, from_magnitude) = logit_signed_magnitude(p_from)?;
    let (to_positive, to_magnitude) = logit_signed_magnitude(p_to)?;
    let displacement = if from_positive == to_positive {
        cmp::max(from_magnitude, to_magnitude)
            .checked_sub(cmp::min(from_magnitude, to_magnitude))?
    } else {
        from_magnitude.checked_add(to_magnitude)?
    };
    displacement.checked_mul(b)
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

    fn primitive_value(function: &str, input: FixedU64x64) -> FixedU64x64 {
        match function {
            "exp2" => input.exp2().unwrap(),
            "log2" => input.log2().unwrap(),
            "ln" => input.ln().unwrap(),
            other => panic!("unknown primitive function: {other}"),
        }
    }

    // Corpus rows are evaluated at exactly representable 64.64 inputs, mirroring
    // tools/fixed/generate-lmsr-corpus.py's q64() snapping, so the committed
    // expectations measure kernel error only (never f64 input-representation error).
    const CORPUS_RAW_HALF: u128 = 1 << 63;
    // Nearest 64.64 to 0.6: raw_64x64(Decimal("0.6")) in the generator.
    const CORPUS_RAW_SIX_TENTHS: u128 = 11_068_046_444_225_730_970;
    // The generator evaluates v3_cost at the snapped V3 displacement, i.e. the
    // committed raw of the v3_displace_0_5_to_0_6 row (cross-checked in the test).
    const CORPUS_RAW_V3_DELTA: u128 = 74_795_110_800_902_839_805_191;

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
                FixedU64x64::from_raw(CORPUS_RAW_HALF),
                FixedU64x64::from_raw(CORPUS_RAW_SIX_TENTHS),
            )
            .unwrap(),
            "v3_cost_0_5_to_0_6" => lmsr_buy_cost(
                FixedU64x64::ZERO,
                FixedU64x64::ZERO,
                FixedU64x64::from_integer(10_000),
                LmsrSide::Long,
                FixedU64x64::from_raw(CORPUS_RAW_V3_DELTA),
            )
            .unwrap(),
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
    fn checked_arithmetic_uses_two_limb_intermediates() {
        let large = FixedU64x64::from_integer(1_000_000_000);
        let half = FixedU64x64::from_f64(0.5).unwrap();
        assert_eq!(
            large.checked_mul(half).unwrap(),
            FixedU64x64::from_integer(500_000_000)
        );

        let max_scaled = FixedU64x64::from_raw(u128::MAX >> 1);
        assert_eq!(
            max_scaled
                .checked_div(FixedU64x64::from_integer(2))
                .unwrap()
                .raw(),
            max_scaled.raw() / 2
        );

        assert_eq!(
            FixedU64x64::from_raw(u128::MAX).checked_mul(FixedU64x64::from_integer(2)),
            Err(FixedError::Overflow)
        );
    }

    #[test]
    fn generated_lmsr_corpus_matches_committed_values() {
        // 04 §4 (normative): composed cost-function error ≤ COMPOSED_COST_MAX_ULP,
        // i.e. b-scaled evaluations (costs, displacements) admit at most
        // 8·b·2⁻⁶⁴ USDC — 8·b raw ulps at the corpus's b = 10,000 (04 §5) —
        // while dimensionless price rows admit 8 raw ulps outright.
        let corpus_b = 10_000u128;
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
            if name == "v3_displace_0_5_to_0_6" {
                // Keeps the chained v3_cost input in lockstep with the generator.
                assert_eq!(expected_raw, CORPUS_RAW_V3_DELTA);
            }
            let max_raw_error = if name.contains("price") {
                u128::from(COMPOSED_COST_MAX_ULP)
            } else {
                u128::from(COMPOSED_COST_MAX_ULP) * corpus_b
            };
            let actual = corpus_value(name);
            approx(actual, expected, 1e-8);
            assert_raw_within(actual, expected_raw, max_raw_error);
            checked += 1;
        }
        assert_eq!(checked, 19);
    }

    #[test]
    fn generated_transcendental_corpus_matches_committed_values() {
        let corpus = include_str!("../fixtures/transcendental_corpus.csv");
        let mut checked = 0u32;
        for line in corpus.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields = line.split(',');
            let _name = fields.next().unwrap();
            let function = fields.next().unwrap();
            let input =
                FixedU64x64::from_f64(fields.next().unwrap().parse::<f64>().unwrap()).unwrap();
            let expected = fields.next().unwrap().parse::<f64>().unwrap();
            let expected_raw = fields.next().unwrap().parse::<u128>().unwrap();
            let actual = primitive_value(function, input);
            approx(actual, expected, 1e-9);
            assert_raw_within(actual, expected_raw, u128::from(PRIMITIVE_MAX_ULP));
            checked += 1;
        }
        assert_eq!(checked, 17);
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

        let cross_midpoint_displacement = lmsr_displacement_between_prices(
            b,
            FixedU64x64::from_f64(0.4).unwrap(),
            FixedU64x64::from_f64(0.6).unwrap(),
        )
        .unwrap();
        approx(cross_midpoint_displacement, 8109.30216216, 1e-8);
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
