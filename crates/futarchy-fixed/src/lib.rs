#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! Unsigned 64.64 fixed-point primitives for Bleavit LMSR math.
//!
//! The market specification requires an internal 64.64 representation for LMSR
//! `exp2`, `log2`, `ln`, and cost calculations, with deterministic integer
//! range reduction/iteration and maker-adverse rounding at currency boundaries.
//! This crate keeps the fixed-point surface small and deterministic; broader
//! protocol types (and the kernel domain/error bounds this crate imports) live
//! in `futarchy-primitives`.
//!
//! ## Guarded internal precision (04 §4, SQ-14/SQ-15)
//!
//! The `exp2`/`log2` kernels are evaluated in an internal **Q96** representation
//! (96 fractional bits = the 64 result bits plus [`GUARD_BITS`] = 32 guard bits),
//! rounded **once** to 64.64 at the boundary. 64-bit-wide intermediates provably
//! cannot meet the bounds; the guard bits make the worst case hold with margin.
//!
//! Worst-case error analysis (part of crate conformance, 04 §4):
//! * `exp2` frac kernel is a product of at most 64 table factors
//!   `2^{2^-(i+1)}`, each stored to Q96 (representation error ≤ 2⁻⁹⁷ relative)
//!   and each Q96 multiply rounded (≤ 2⁻⁹⁷ relative). The accumulated relative
//!   error over ≤ 64 factors is ≤ 128·2⁻⁹⁷ = 2⁻⁹⁰; the single Q96→Q64 rounding
//!   adds ≤ 0.5 ulp. Total ≪ 2 ulp of the `[1,2)` kernel ⇔ relative ≤ 2⁻⁶³.
//! * `log2` extracts 64 fractional bits by repeated Q96 squaring; each squaring
//!   error is ≤ 2⁻⁹⁷ relative, so the accumulated output error is ≤ ~2⁻⁹⁰,
//!   below the 64th-bit rounding of ≤ 0.5 ulp. Total ≤ 2 ulp absolute.
//! * `ln x = log2 x · ln 2` uses a Q96 `ln 2` ([`LN2_Q96`]); the product's error
//!   from the constant is ≤ 63·2⁻⁹⁷ absolute, so `ln` stays ≤ 2 ulp absolute on
//!   the full domain (a 64.64 `ln 2` would lose ~5 ulp at `log2 ≈ 63`).
//!
//! The empirical proof is the per-commit ≥ 10³-point dense-bit adversarial corpus
//! generated from the reference model (04 §4/§5) and asserted in the tests.

use core::cmp;
use core::fmt;
use core::ops::{Add, Div, Mul, Sub};

use futarchy_primitives::kernel::LMSR_DOMAIN_BOUND;
pub use futarchy_primitives::kernel::{COMPOSED_COST_MAX_ULP, PRIMITIVE_MAX_ULP};

/// Number of fractional bits in [`FixedU64x64`].
pub const FRAC_BITS: u32 = 64;
/// One unit in raw 64.64 representation.
pub const ONE_RAW: u128 = 1u128 << FRAC_BITS;
/// Natural logarithm of two as an unsigned 64.64 constant (for consumers that
/// need `b·ln 2` in currency units; the `ln` kernel uses [`LN2_Q96`]).
pub const LN_2: FixedU64x64 = FixedU64x64(12_786_308_645_202_655_660);

/// Guard bits carried beyond the 64 fractional result bits (04 §4 reference
/// configuration: ≥ 32, single final rounding).
const GUARD_BITS: u32 = 32;
/// Internal fractional bits for the guarded `exp2`/`log2` kernels.
const INTERNAL_FRAC_BITS: u32 = FRAC_BITS + GUARD_BITS; // 96
/// `1.0` in the internal Q96 representation.
const ONE_Q96: u128 = 1u128 << INTERNAL_FRAC_BITS;
/// `ln 2` in Q96 (`round(ln 2 · 2^96)`), used to keep `ln` ≤ 2 ulp on wide inputs.
const LN2_Q96: u128 = 54_916_777_467_707_473_351_141_471_128;

/// `EXP2_FRAC_FACTORS_Q96[i] = round(2^{2^-(i+1)} · 2^96)`; the `exp2` fractional
/// kernel multiplies these for each set bit of the 64-bit fraction. Self-checked
/// in tests via `f[i]² ≈ f[i-1]` and `f[0]² ≈ 2` (needs no external oracle).
const EXP2_FRAC_FACTORS_Q96: [u128; 64] = [
    112045541949572279837463876455,
    94218694570555024373110687280,
    86398923866664962375130072335,
    82735892943544661968409507608,
    80963033366431698391403892407,
    80090900639232773493759855183,
    79658363602065864296602447411,
    79442971854562650309779985388,
    79335494482037593178543608469,
    79281810334857995344526032841,
    79254981885274639038225885637,
    79241571065139888961741854322,
    79234866506068713947867698354,
    79231514439261175779748764725,
    79229838459036794155212045658,
    79229000482219122111440011082,
    79228581497133874770443338218,
    79228372005422143143813826733,
    79228267259773999700675265240,
    79228214887001858491551355114,
    79228188700628770505088420373,
    79228175607445472165130168567,
    79228169060854634408312904164,
    79228165787559418383175182041,
    79228164150911861083921604088,
    79228163332588095112623330337,
    79228162923426215296556284074,
    79228162718845276180918278822,
    79228162616554806821198155068,
    79228162565409572190862812835,
    79228162539836954888076321620,
    79228162527050646239778370987,
    79228162520657491916403219414,
    79228162517460914754909099563,
    79228162515862626174210403621,
    79228162515063481883873146647,
    79228162514663909738707540908,
    79228162514464123666125493726,
    79228162514364230629834659057,
    79228162514314284111689288953,
    79228162514289310852616615709,
    79228162514276824223080282038,
    79228162514270580908312115941,
    79228162514267459250928033077,
    79228162514265898422235991691,
    79228162514265118007889971010,
    79228162514264727800716960672,
    79228162514264532697130455504,
    79228162514264435145337202920,
    79228162514264386369440576628,
    79228162514264361981492263482,
    79228162514264349787518106909,
    79228162514264343690531028622,
    79228162514264340642037489479,
    79228162514264339117790719908,
    79228162514264338355667335122,
    79228162514264337974605642729,
    79228162514264337784074796532,
    79228162514264337688809373434,
    79228162514264337641176661885,
    79228162514264337617360306111,
    79228162514264337605452128223,
    79228162514264337599498039280,
    79228162514264337596520994808,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct U256 {
    hi: u128,
    lo: u128,
}

impl U256 {
    const ZERO: Self = Self { hi: 0, lo: 0 };

    const fn from_u128(value: u128) -> Self {
        Self { hi: 0, lo: value }
    }

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

    /// `floor(self / 2^shift)` as `u128`; errors if the result does not fit.
    fn shr_to_u128(self, shift: u32) -> Result<u128, FixedError> {
        if shift == 0 {
            if self.hi != 0 {
                return Err(FixedError::Overflow);
            }
            return Ok(self.lo);
        }
        if shift < 128 {
            if (self.hi >> shift) != 0 {
                return Err(FixedError::Overflow);
            }
            Ok((self.lo >> shift) | (self.hi << (128 - shift)))
        } else if shift < 256 {
            Ok(self.hi >> (shift - 128))
        } else {
            Ok(0)
        }
    }

    /// `round(self / 2^shift)` (round half up) as `u128`; errors on overflow.
    /// `shift` must be ≥ 1 so the discarded most-significant bit exists.
    fn shr_round_to_u128(self, shift: u32) -> Result<u128, FixedError> {
        let truncated = self.shr_to_u128(shift)?;
        if self.bit(shift - 1) {
            truncated.checked_add(1).ok_or(FixedError::Overflow)
        } else {
            Ok(truncated)
        }
    }

    fn bit(self, index: u32) -> bool {
        if index < 128 {
            ((self.lo >> index) & 1) == 1
        } else if index < 256 {
            ((self.hi >> (index - 128)) & 1) == 1
        } else {
            false
        }
    }

    fn gt(self, rhs: Self) -> bool {
        self.hi > rhs.hi || (self.hi == rhs.hi && self.lo > rhs.lo)
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

/// Multiply two Q96 values, rounding the 256-bit product back to Q96.
fn mul_q96_round(lhs: u128, rhs: u128) -> Result<u128, FixedError> {
    U256::mul_u128(lhs, rhs).shr_round_to_u128(INTERNAL_FRAC_BITS)
}

/// Round a Q96 value in `[2^96, 2^97)` down to Q64 (`round(v / 2^32)`).
fn round_q96_to_q64(value_q96: u128) -> u128 {
    (value_q96 >> GUARD_BITS) + ((value_q96 >> (GUARD_BITS - 1)) & 1)
}

/// `2^(frac / 2^64)` in Q96 for `frac ∈ [0, 2^64)`; result in `[2^96, 2^97)`.
///
/// The 64-bit fraction's bit `63-i` carries weight `2^-(i+1)`, so a set bit
/// multiplies in `EXP2_FRAC_FACTORS_Q96[i] = 2^{2^-(i+1)}`.
fn exp2_frac_q96(frac: u128) -> Result<u128, FixedError> {
    let mut acc = ONE_Q96;
    for (i, factor) in EXP2_FRAC_FACTORS_Q96.iter().enumerate() {
        if ((frac >> (63 - i as u32)) & 1) == 1 {
            acc = mul_q96_round(acc, *factor)?;
        }
    }
    Ok(acc)
}

/// `2^(raw / 2^64)` for non-negative `raw`, as a 64.64 value.
fn exp2_positive_raw(raw: u128) -> Result<u128, FixedError> {
    let whole = raw >> FRAC_BITS;
    if whole >= 64 {
        return Err(FixedError::Overflow);
    }
    let frac = raw & (ONE_RAW - 1);
    let kernel_q64 = round_q96_to_q64(exp2_frac_q96(frac)?);
    // `checked_shl` only guards the shift *amount*, not lost significant bits, so
    // it would silently wrap `2^{whole}` past `whole ≈ 63`; multiply-and-check instead.
    kernel_q64
        .checked_mul(1u128 << whole)
        .ok_or(FixedError::Overflow)
}

/// `2^(-raw / 2^64)` for non-negative `raw`, as a 64.64 value in `(0, 1]`.
///
/// Uses the complement identity `2^{-(w+f)} = 2^{-(w+1)} · 2^{1-f}` so the same
/// `[1,2)` frac kernel serves the negative path — no reciprocal division.
fn exp2_negative_raw(raw: u128) -> Result<u128, FixedError> {
    let whole = raw >> FRAC_BITS;
    let frac = raw & (ONE_RAW - 1);
    let (kernel_q96, exp_shift) = if frac == 0 {
        (ONE_Q96, whole)
    } else {
        // 2^{1-f} with (1-f) ∈ (0,1) computed from the complement fraction.
        (exp2_frac_q96(ONE_RAW - frac)?, whole + 1)
    };
    let total_shift = exp_shift + u128::from(GUARD_BITS);
    if total_shift >= 256 {
        return Ok(0);
    }
    U256::from_u128(kernel_q96).shr_round_to_u128(total_shift as u32)
}

/// Fractional-then-integer `log2` for `raw` encoding a value `≥ 1`.
fn log2_raw(raw: u128) -> Result<u128, FixedError> {
    if raw == 0 {
        return Err(FixedError::Domain);
    }

    let bit_index = 127 - raw.leading_zeros();
    let integer = i64::from(bit_index) - i64::from(FRAC_BITS);
    if integer < 0 {
        // Values below 1 have a negative log2, outside the unsigned range.
        return Err(FixedError::Domain);
    }
    let integer = integer as u32;

    // Normalize the mantissa into Q96 in [2^96, 2^97): value / 2^integer · 2^96
    // = raw · 2^(32 - integer), computed without dropping bits.
    let mut norm = if integer <= GUARD_BITS {
        raw.checked_shl(GUARD_BITS - integer)
            .ok_or(FixedError::Overflow)?
    } else {
        let s = integer - GUARD_BITS;
        (raw >> s) + ((raw >> (s - 1)) & 1)
    };

    let mut fraction = 0u128;
    for i in 0..64u32 {
        norm = mul_q96_round(norm, norm)?;
        if norm >= (ONE_Q96 << 1) {
            norm = (norm + 1) >> 1;
            fraction |= 1u128 << (63 - i);
        }
    }
    // Round the 64-bit fraction to nearest by inspecting the 65th bit.
    norm = mul_q96_round(norm, norm)?;
    if norm >= (ONE_Q96 << 1) {
        fraction = fraction.checked_add(1).ok_or(FixedError::Overflow)?;
    }

    (u128::from(integer) << FRAC_BITS)
        .checked_add(fraction)
        .ok_or(FixedError::Overflow)
}

fn ln_one_plus(x: FixedU64x64) -> Result<FixedU64x64, FixedError> {
    FixedU64x64::ONE.checked_add(x)?.ln()
}

#[cfg(feature = "std")]
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

    /// Convert to a nearest `f64` for diagnostics and tests (std only — the
    /// deterministic `no_std` surface is float-free per 01 §5.2 / rule 2).
    #[cfg(feature = "std")]
    pub fn to_f64(self) -> f64 {
        (self.0 as f64) / (ONE_RAW as f64)
    }

    /// Convert a finite non-negative `f64` to nearest 64.64 (std only).
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

    /// Fixed-point base-2 exponent (04 §4: relative error ≤ 2⁻⁶³).
    pub fn exp2(self) -> Result<Self, FixedError> {
        exp2_positive_raw(self.0).map(Self)
    }
    /// Fixed-point base-2 logarithm (04 §4: ≤ 2 ulp absolute). Zero is outside
    /// the logarithm domain; values below 1 have a negative (unrepresentable) log2.
    pub fn log2(self) -> Result<Self, FixedError> {
        if self.0 == 0 {
            return Err(FixedError::Domain);
        }
        log2_raw(self.0).map(Self)
    }
    /// Fixed-point natural logarithm (04 §4: ≤ 2 ulp absolute on the full domain).
    pub fn ln(self) -> Result<Self, FixedError> {
        if self.0 == 0 {
            return Err(FixedError::Domain);
        }
        let log2 = log2_raw(self.0)?;
        U256::mul_u128(log2, LN2_Q96)
            .shr_round_to_u128(INTERNAL_FRAC_BITS)
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

/// Reject a two-outcome LMSR state outside the protocol domain `|q_L−q_S|/b ≤ 48`.
///
/// The comparison is exact in `u256` (`diff > 48·b`) rather than dividing first,
/// so no truncation sliver lets a state just past the clamp through (04 §4).
fn ensure_lmsr_domain(
    q_l: FixedU64x64,
    q_s: FixedU64x64,
    b: FixedU64x64,
) -> Result<(), FixedError> {
    if b.raw() == 0 {
        return Err(FixedError::DivisionByZero);
    }
    let diff = cmp::max(q_l, q_s)
        .raw()
        .saturating_sub(cmp::min(q_l, q_s).raw());
    let bound = U256::mul_u128(b.raw(), u128::from(LMSR_DOMAIN_BOUND));
    if U256::from_u128(diff).gt(bound) {
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

    // Nearest 64.64 to 0.5 and 0.6 (exactly representable on-chain inputs used by
    // the V3 displacement vector).
    const CORPUS_RAW_HALF: u128 = 1 << 63;
    const CORPUS_RAW_SIX_TENTHS: u128 = 11_068_046_444_225_730_970;

    mod reference_model_vectors {
        use super::*;

        const REFERENCE_VECTORS: &str =
            include_str!("../../../reference-model/fixtures/vectors.json");
        const CORPUS_B: u128 = 10_000;
        const USDC_BASE_UNIT_RAW_CEIL: u128 = ONE_RAW.div_ceil(1_000_000);
        const COMPOSED_RAW_TOLERANCE: u128 =
            (COMPOSED_COST_MAX_ULP as u128) * CORPUS_B + USDC_BASE_UNIT_RAW_CEIL;

        fn json_value<'a>(text: &'a str, key: &str) -> &'a str {
            let needle = format!("\"{key}\":");
            let start = text
                .find(&needle)
                .unwrap_or_else(|| panic!("missing JSON key: {key}"));
            text[start + needle.len()..].trim_start()
        }

        fn container_from_start(text: &str, opening: u8, closing: u8) -> &str {
            assert_eq!(text.as_bytes().first(), Some(&opening));
            let mut depth = 0u32;
            let mut in_string = false;
            let mut escaped = false;
            for (index, byte) in text.bytes().enumerate() {
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        in_string = false;
                    }
                    continue;
                }
                if byte == b'"' {
                    in_string = true;
                } else if byte == opening {
                    depth += 1;
                } else if byte == closing {
                    depth = depth.checked_sub(1).unwrap();
                    if depth == 0 {
                        return &text[..=index];
                    }
                }
            }
            panic!("unterminated JSON container")
        }

        fn json_container<'a>(text: &'a str, key: &str, opening: u8, closing: u8) -> &'a str {
            container_from_start(json_value(text, key), opening, closing)
        }

        fn json_string<'a>(text: &'a str, key: &str) -> &'a str {
            let value = json_value(text, key);
            assert_eq!(value.as_bytes().first(), Some(&b'"'));
            let mut escaped = false;
            for (index, byte) in value.bytes().enumerate().skip(1) {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    return &value[1..index];
                }
            }
            panic!("unterminated JSON string for key: {key}")
        }

        fn json_u128(text: &str, key: &str) -> u128 {
            let value = json_value(text, key);
            let end = value
                .bytes()
                .position(|byte| !byte.is_ascii_digit())
                .unwrap_or(value.len());
            assert!(end > 0, "JSON value for {key} is not an unsigned integer");
            value[..end].parse().unwrap()
        }

        fn fixed_integer(text: &str, key: &str) -> FixedU64x64 {
            FixedU64x64::from_integer(json_string(text, key).parse().unwrap())
        }

        fn fixed_decimal(text: &str, key: &str) -> FixedU64x64 {
            FixedU64x64::from_f64(json_string(text, key).parse().unwrap()).unwrap()
        }

        fn assert_composed_within(actual: FixedU64x64, expected_raw: u128) {
            assert_raw_within(actual, expected_raw, COMPOSED_RAW_TOLERANCE);
        }

        fn assert_price_within(actual: FixedU64x64, expected_raw: u128) {
            assert_raw_within(actual, expected_raw, u128::from(COMPOSED_COST_MAX_ULP));
        }

        #[test]
        fn normative_lmsr_vectors_match_reference_model_artifact() {
            assert_eq!(
                json_string(REFERENCE_VECTORS, "schema"),
                "bleavit.reference-model.v4"
            );
            let vectors = json_container(REFERENCE_VECTORS, "lmsr_vectors", b'{', b'}');
            let b = FixedU64x64::from_integer(CORPUS_B as u64);
            let zero = FixedU64x64::ZERO;

            let v1 = json_container(vectors, "V1", b'{', b'}');
            let buy = lmsr_buy_cost(
                zero,
                zero,
                b,
                LmsrSide::Long,
                FixedU64x64::from_integer(1_000),
            )
            .unwrap();
            assert_composed_within(buy, json_u128(v1, "raw_64x64_nearest"));

            let v2 = json_container(vectors, "V2", b'{', b'}');
            let price = lmsr_price_long(FixedU64x64::from_integer(1_000), zero, b).unwrap();
            assert_price_within(price, json_u128(v2, "raw_64x64_nearest"));

            let v3 = json_container(vectors, "V3", b'{', b'}');
            let displacement = lmsr_displacement_between_prices(
                b,
                FixedU64x64::from_raw(CORPUS_RAW_HALF),
                FixedU64x64::from_raw(CORPUS_RAW_SIX_TENTHS),
            )
            .unwrap();
            assert_composed_within(displacement, fixed_decimal(v3, "delta").raw());
            let displacement_cost =
                lmsr_buy_cost(zero, zero, b, LmsrSide::Long, displacement).unwrap();
            assert_composed_within(displacement_cost, fixed_decimal(v3, "cost").raw());

            let v4 = json_container(vectors, "V4", b'{', b'}');
            let worst_case_loss = b.checked_mul(LN_2).unwrap();
            assert_composed_within(worst_case_loss, json_u128(v4, "raw_64x64_nearest"));

            let v5 = json_container(vectors, "V5", b'{', b'}');
            let proceeds = lmsr_sell_proceeds(
                FixedU64x64::from_integer(1_000),
                zero,
                b,
                LmsrSide::Long,
                FixedU64x64::from_integer(1_000),
            )
            .unwrap();
            assert_composed_within(proceeds, fixed_decimal(v5, "proceeds_before_fees").raw());

            let v6 = json_container(vectors, "V6", b'{', b'}');
            assert_eq!(json_string(v6, "error"), "PriceBoundExceeded");
            assert_eq!(json_string(v6, "side"), "long");
            assert_eq!(fixed_integer(v6, "b"), b);
            assert_eq!(
                lmsr_buy_cost(
                    fixed_integer(v6, "q_long"),
                    fixed_integer(v6, "q_short"),
                    fixed_integer(v6, "b"),
                    LmsrSide::Long,
                    fixed_integer(v6, "amount"),
                ),
                Err(FixedError::Domain)
            );
        }

        #[test]
        fn high_precision_lmsr_corpus_matches_reference_model_artifact() {
            let corpus = json_container(REFERENCE_VECTORS, "high_precision_corpus", b'{', b'}');
            let b = fixed_integer(corpus, "b");
            let samples = json_container(corpus, "samples", b'[', b']');
            let mut remaining = &samples[1..samples.len() - 1];
            let mut checked = 0u32;
            while let Some(start) = remaining.find('{') {
                remaining = &remaining[start..];
                let sample = container_from_start(remaining, b'{', b'}');
                let q_l = fixed_integer(sample, "q_long");
                let q_s = fixed_integer(sample, "q_short");
                assert_composed_within(
                    lmsr_cost(q_l, q_s, b).unwrap(),
                    json_u128(sample, "cost_raw_64x64_nearest"),
                );
                assert_price_within(
                    lmsr_price_long(q_l, q_s, b).unwrap(),
                    json_u128(sample, "price_raw_64x64_nearest"),
                );
                checked += 1;
                remaining = &remaining[sample.len()..];
            }
            assert_eq!(checked, 9);
        }

        // 04 §4/§5 per-commit adversarial gate: the ≥10³-point dense-bit corpus
        // (generated from the single reference model) certifies exp2/log2/ln to
        // the normative bounds. This is the test the old few-set-bit corpus could
        // not perform — dense fractions are where the unguarded kernel drifted to
        // 10 ulp.
        #[test]
        fn adversarial_transcendental_corpus_matches_reference_model() {
            let corpus = json_container(REFERENCE_VECTORS, "transcendental_corpus", b'{', b'}');
            let declared = json_u128(corpus, "count");
            assert!(
                declared >= 1_000,
                "04 §4 requires ≥10³ adversarial points; found {declared}"
            );
            let rows = json_container(corpus, "rows", b'[', b']');
            let mut remaining = &rows[1..rows.len() - 1];
            let mut checked = 0u128;
            let mut worst_exp2_ulp = 0u128;
            let mut worst_log2_ulp = 0u128;
            let mut popcount_sum = 0u32;
            let mut exp2_frac_rows = 0u32;
            while let Some(start) = remaining.find('{') {
                remaining = &remaining[start..];
                let row = container_from_start(remaining, b'{', b'}');
                let function = json_string(row, "f");
                let input_raw = json_u128(row, "in");
                let expected_raw = json_u128(row, "out");
                match function {
                    "exp2" => {
                        let actual = FixedU64x64::from_raw(input_raw).exp2().unwrap().raw();
                        // Normative exp2 bound: relative ≤ 2⁻⁶³ ⇔ ≤ 2 ulp of the
                        // [1,2) kernel. For frac-only inputs (expected ∈ [2^64,2^65))
                        // this is exactly ≤ 2 ulp absolute.
                        let tolerance = expected_raw >> 63;
                        let ulp = actual.abs_diff(expected_raw);
                        assert!(
                            ulp <= tolerance,
                            "exp2({input_raw}) rel error {ulp} > {tolerance} (2⁻⁶³)"
                        );
                        if input_raw < ONE_RAW {
                            worst_exp2_ulp = worst_exp2_ulp.max(ulp);
                            popcount_sum += input_raw.count_ones();
                            exp2_frac_rows += 1;
                        }
                    }
                    "log2" => {
                        let actual = FixedU64x64::from_raw(input_raw).log2().unwrap().raw();
                        let ulp = actual.abs_diff(expected_raw);
                        assert!(
                            ulp <= u128::from(PRIMITIVE_MAX_ULP),
                            "log2({input_raw}) abs error {ulp} > {PRIMITIVE_MAX_ULP} ulp"
                        );
                        worst_log2_ulp = worst_log2_ulp.max(ulp);
                    }
                    "ln" => {
                        let actual = FixedU64x64::from_raw(input_raw).ln().unwrap().raw();
                        let ulp = actual.abs_diff(expected_raw);
                        assert!(
                            ulp <= u128::from(PRIMITIVE_MAX_ULP),
                            "ln({input_raw}) abs error {ulp} > {PRIMITIVE_MAX_ULP} ulp"
                        );
                    }
                    other => panic!("unknown corpus function: {other}"),
                }
                checked += 1;
                remaining = &remaining[row.len()..];
            }
            assert_eq!(checked, declared, "row count disagrees with declared count");
            // The fractions must be genuinely dense (04 §4: "not only few-set-bit
            // fracs"): a uniform 64-bit draw averages ~32 set bits.
            assert!(exp2_frac_rows > 100);
            let mean_popcount = popcount_sum / exp2_frac_rows;
            assert!(
                mean_popcount >= 24,
                "corpus fractions are not dense (mean popcount {mean_popcount})"
            );
            assert!(worst_exp2_ulp <= u128::from(PRIMITIVE_MAX_ULP));
            assert!(worst_log2_ulp <= u128::from(PRIMITIVE_MAX_ULP));
        }
    }

    #[test]
    fn factor_table_is_self_consistent() {
        // f[i] = 2^{2^-(i+1)} ⇒ f[i]² = f[i-1], and f[0]² = 2. This pins the
        // embedded Q96 table with no external oracle (a transcription error in a
        // magic constant cannot survive both identities).
        for i in 1..64usize {
            let squared =
                mul_q96_round(EXP2_FRAC_FACTORS_Q96[i], EXP2_FRAC_FACTORS_Q96[i]).unwrap();
            assert!(
                squared.abs_diff(EXP2_FRAC_FACTORS_Q96[i - 1]) <= 2,
                "factor[{i}]² diverges from factor[{}]",
                i - 1
            );
        }
        let f0_squared = mul_q96_round(EXP2_FRAC_FACTORS_Q96[0], EXP2_FRAC_FACTORS_Q96[0]).unwrap();
        assert!(f0_squared.abs_diff(ONE_Q96 << 1) <= 2, "factor[0]² ≠ 2");
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
    fn constants_match_reference_values() {
        approx(LN_2, core::f64::consts::LN_2, 1e-18);
        assert_eq!(PRIMITIVE_MAX_ULP, 2);
        assert_eq!(COMPOSED_COST_MAX_ULP, 8);
        // LN2_Q96 agrees with the 64.64 constant to the shared 64 fractional bits
        // (they differ by at most the rounding of the 64th bit: floor vs nearest).
        assert!((LN2_Q96 >> GUARD_BITS).abs_diff(LN_2.raw()) <= 1);
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
    fn exp2_round_trips_through_log2() {
        // On [1,2) the round trip is tight: log2 lands in [0,1) (kernel only) and
        // exp2 inverts it to within a few ulp. (Wide operands amplify log2's
        // ≤2-ulp absolute error by their magnitude; the corpus checks those
        // against the oracle directly rather than through a round trip.)
        for seed in 0u64..64 {
            let frac = u128::from(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)); // dense
            let x = FixedU64x64::from_raw(ONE_RAW | frac);
            let round_trip = x.log2().unwrap().exp2().unwrap();
            assert_raw_within(round_trip, x.raw(), 8);
        }
    }

    #[test]
    fn log_zero_is_rejected() {
        assert_eq!(FixedU64x64::ZERO.ln(), Err(FixedError::Domain));
        assert_eq!(FixedU64x64::ZERO.log2(), Err(FixedError::Domain));
    }

    #[test]
    fn log2_below_one_is_out_of_range() {
        assert_eq!(
            FixedU64x64::from_raw(ONE_RAW - 1).log2(),
            Err(FixedError::Domain)
        );
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
        // The clamp edge is derived from the single-homed bound, never a literal.
        let edge = u64::from(LMSR_DOMAIN_BOUND) * 10_000;
        assert_eq!(
            lmsr_cost(FixedU64x64::from_integer(edge + 1), FixedU64x64::ZERO, b),
            Err(FixedError::Domain)
        );
        assert_eq!(
            lmsr_buy_cost(
                FixedU64x64::from_integer(edge),
                FixedU64x64::ZERO,
                b,
                LmsrSide::Long,
                FixedU64x64::from_integer(1),
            ),
            Err(FixedError::Domain)
        );
        assert!(lmsr_buy_cost(
            FixedU64x64::from_integer(edge - 1),
            FixedU64x64::ZERO,
            b,
            LmsrSide::Long,
            FixedU64x64::from_integer(1),
        )
        .is_ok());
    }

    #[test]
    fn lmsr_domain_edge_is_exact_at_the_sub_unit_sliver() {
        // A displacement one raw ulp past 48·b must reject; exactly 48·b must not.
        // The old divide-first check truncated this sliver and wrongly accepted it.
        let b = FixedU64x64::from_integer(10_000);
        let clamp = u128::from(LMSR_DOMAIN_BOUND);
        let exact = FixedU64x64::from_raw(clamp * b.raw());
        assert!(lmsr_cost(exact, FixedU64x64::ZERO, b).is_ok());
        let one_past = FixedU64x64::from_raw(clamp * b.raw() + 1);
        assert_eq!(
            lmsr_cost(one_past, FixedU64x64::ZERO, b),
            Err(FixedError::Domain)
        );
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
