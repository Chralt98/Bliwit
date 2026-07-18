#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]
#![allow(clippy::too_many_arguments)]

extern crate alloc;

use alloc::vec::Vec;
use conditional_ledger_core::{baseline, position, LedgerOrigin, LedgerState};
use futarchy_fixed::{
    lmsr_buy_cost, lmsr_cost, lmsr_price_long, lmsr_sell_proceeds, round_charge_up,
    round_payout_down, FixedError, FixedU64x64, LmsrSide, LN_2,
};
use futarchy_primitives::{
    kernel, Balance, BlockNumber, Branch, EpochId, FixedU64, GateType, MarketId, PositionId,
    PositionKind, ProposalId, QuoteView, ScalarSide, TradeSide,
};
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub const FEE_BPS: u128 = 30;
pub const BPS_DENOM: u128 = 10_000;
pub const MIN_TRADE: Balance = futarchy_primitives::kernel::MIN_TRADE_USDC;
pub const OBS_INTERVAL: u64 = 10;
pub const STALE_GAP_BLOCKS: u64 = futarchy_primitives::kernel::MKT_STALE_GAP_BLOCKS;
pub const KAPPA_1E9: u64 = 5_000_000;
pub const PRICE_ONE_1E9: u64 = 1_000_000_000;

/// Live market tunables the FRAME pallet injects from pallet-constitution::Params.
/// Defaults are the reference-model / differential-oracle values (13 §1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MarketParams {
    pub fee_bps: u128,
    pub obs_interval: u64,
    pub kappa_1e9: u64,
    pub stale_gap_blocks: u64,
}

impl Default for MarketParams {
    fn default() -> Self {
        Self {
            fee_bps: FEE_BPS,
            obs_interval: OBS_INTERVAL,
            kappa_1e9: KAPPA_1E9,
            stale_gap_blocks: STALE_GAP_BLOCKS,
        }
    }
}

/// The exact ledger operations the D-3 trade wrapper (04 §6) consumes.
///
/// The in-memory [`LedgerState`] implements this for the differential oracle;
/// the production FRAME pallet supplies a shim over `pallet-conditional-ledger`.
/// Ledger errors collapse to [`Error::Ledger`] so every failure is status-quo.
#[allow(clippy::result_unit_err)]
pub trait LedgerOps<AccountId> {
    fn do_split(&mut self, pid: ProposalId, who: &AccountId, a: Balance) -> Result<(), ()>;
    fn do_transfer(
        &mut self,
        id: PositionId,
        from: &AccountId,
        to: &AccountId,
        a: Balance,
    ) -> Result<(), ()>;
    fn do_split_scalar(
        &mut self,
        pid: ProposalId,
        b: Branch,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), ()>;
    fn do_split_gate(
        &mut self,
        pid: ProposalId,
        b: Branch,
        g: GateType,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), ()>;
    fn do_split_baseline(&mut self, epoch: EpochId, who: &AccountId, a: Balance) -> Result<(), ()>;
    fn do_merge(&mut self, pid: ProposalId, who: &AccountId, a: Balance) -> Result<(), ()>;
    fn do_merge_scalar(
        &mut self,
        pid: ProposalId,
        b: Branch,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), ()>;
    fn do_merge_gate(
        &mut self,
        pid: ProposalId,
        b: Branch,
        g: GateType,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), ()>;
    fn do_merge_baseline(&mut self, epoch: EpochId, who: &AccountId, a: Balance) -> Result<(), ()>;
    fn note_protocol_account(&mut self, who: AccountId);
    fn position_balance(&self, id: PositionId, who: &AccountId) -> Balance;
}

impl<A: Clone + Eq> LedgerOps<A> for LedgerState<A> {
    fn do_split(&mut self, pid: ProposalId, who: &A, a: Balance) -> Result<(), ()> {
        self.split(LedgerOrigin::MarketAuthority, pid, who, a)
            .map_err(|_| ())
    }

    fn do_transfer(&mut self, id: PositionId, from: &A, to: &A, a: Balance) -> Result<(), ()> {
        self.transfer(LedgerOrigin::MarketAuthority, id, from, to, a)
            .map_err(|_| ())
    }

    fn do_split_scalar(
        &mut self,
        pid: ProposalId,
        b: Branch,
        who: &A,
        a: Balance,
    ) -> Result<(), ()> {
        self.split_scalar(LedgerOrigin::MarketAuthority, pid, b, who, a)
            .map_err(|_| ())
    }

    fn do_split_gate(
        &mut self,
        pid: ProposalId,
        b: Branch,
        g: GateType,
        who: &A,
        a: Balance,
    ) -> Result<(), ()> {
        self.split_gate(LedgerOrigin::MarketAuthority, pid, b, g, who, a)
            .map_err(|_| ())
    }

    fn do_split_baseline(&mut self, epoch: EpochId, who: &A, a: Balance) -> Result<(), ()> {
        self.split_baseline(LedgerOrigin::MarketAuthority, epoch, who, a)
            .map_err(|_| ())
    }

    fn do_merge(&mut self, pid: ProposalId, who: &A, a: Balance) -> Result<(), ()> {
        self.merge(LedgerOrigin::MarketAuthority, pid, who, a)
            .map_err(|_| ())
    }

    fn do_merge_scalar(
        &mut self,
        pid: ProposalId,
        b: Branch,
        who: &A,
        a: Balance,
    ) -> Result<(), ()> {
        self.merge_scalar(LedgerOrigin::MarketAuthority, pid, b, who, a)
            .map_err(|_| ())
    }

    fn do_merge_gate(
        &mut self,
        pid: ProposalId,
        b: Branch,
        g: GateType,
        who: &A,
        a: Balance,
    ) -> Result<(), ()> {
        self.merge_gate(LedgerOrigin::MarketAuthority, pid, b, g, who, a)
            .map_err(|_| ())
    }

    fn do_merge_baseline(&mut self, epoch: EpochId, who: &A, a: Balance) -> Result<(), ()> {
        self.merge_baseline(LedgerOrigin::MarketAuthority, epoch, who, a)
            .map_err(|_| ())
    }

    fn note_protocol_account(&mut self, who: A) {
        self.add_protocol_account(who);
    }

    fn position_balance(&self, id: PositionId, who: &A) -> Balance {
        self.positions
            .iter()
            .find(|p| p.id == id && &p.owner == who)
            .map_or(0, |p| p.balance)
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
pub enum BookKind {
    Decision {
        proposal: ProposalId,
        branch: Branch,
    },
    Gate {
        proposal: ProposalId,
        branch: Branch,
        gate: GateType,
    },
    Baseline {
        epoch: EpochId,
    },
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
pub enum MarketPhase {
    Trading,
    Extended,
    Closed,
    Settled,
}

/// One decision-window registration. The FRAME shell keeps at most eight of
/// these per book, matching the bounded checkpoint contract in 04 §7.
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
pub struct TwapWindow {
    pub start: BlockNumber,
    pub trailing_start: BlockNumber,
    pub end: BlockNumber,
    pub observations: u32,
    pub stale_events: u8,
    /// Time integral of non-POL contest notional over this full window.
    pub contest_notional_blocks: u128,
    /// Last block through which contest notional has been integrated.
    pub contest_accrued_until: BlockNumber,
    /// Cleared on any accumulator overflow; an invalid window never grades.
    pub contest_valid: bool,
    /// Quote after all trades in the exact close block. A later observation
    /// may never synthesize this value from post-close information.
    pub close_spot: Option<FixedU64>,
    /// Set only by the epoch decision-boundary read (or market close). Once
    /// true, no observation, trade or contest accrual may mutate this window.
    pub sealed: bool,
}

/// Two-limb unsigned accumulator for the 04 §7 price×block integral.
///
/// Realistic chain values occupy `lo`; the explicit high limb keeps the
/// consensus representation and overflow behavior correct for the full u256
/// domain without adding a big-integer dependency.
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Default,
    Encode,
    Eq,
    MaxEncodedLen,
    Ord,
    PartialEq,
    PartialOrd,
    TypeInfo,
)]
pub struct TwapCumulative {
    pub hi: u128,
    pub lo: u128,
}

impl TwapCumulative {
    pub const ZERO: Self = Self { hi: 0, lo: 0 };

    pub const fn from_low(lo: u128) -> Self {
        Self { hi: 0, lo }
    }

    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        let (lo, carry) = self.lo.overflowing_add(rhs.lo);
        let hi = self
            .hi
            .checked_add(rhs.hi)?
            .checked_add(u128::from(carry))?;
        Some(Self { hi, lo })
    }

    pub fn checked_add_product(self, observation: u64, blocks: u64) -> Option<Self> {
        let product = u128::from(observation).checked_mul(u128::from(blocks))?;
        self.checked_add(Self::from_low(product))
    }

    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        let (lo, borrow) = self.lo.overflowing_sub(rhs.lo);
        let hi = self
            .hi
            .checked_sub(rhs.hi)?
            .checked_sub(u128::from(borrow))?;
        Some(Self { hi, lo })
    }

    /// Divide the full two-limb value by a block count using base-2⁶⁴ long
    /// division. The quotient remains a two-limb value; callers decide whether
    /// their narrower result type can represent it.
    pub fn checked_div_by_blocks(self, blocks: u64) -> Option<Self> {
        if blocks == 0 {
            return None;
        }

        let hi_hi = u64::try_from(self.hi >> 64).ok()?;
        let hi_lo = u64::try_from(self.hi & u128::from(u64::MAX)).ok()?;
        let lo_hi = u64::try_from(self.lo >> 64).ok()?;
        let lo_lo = u64::try_from(self.lo & u128::from(u64::MAX)).ok()?;

        let (q_hi_hi, remainder) = Self::divide_word(0, hi_hi, blocks)?;
        let (q_hi_lo, remainder) = Self::divide_word(remainder, hi_lo, blocks)?;
        let (q_lo_hi, remainder) = Self::divide_word(remainder, lo_hi, blocks)?;
        let (q_lo_lo, _) = Self::divide_word(remainder, lo_lo, blocks)?;

        Some(Self {
            hi: (u128::from(q_hi_hi) << 64) | u128::from(q_hi_lo),
            lo: (u128::from(q_lo_hi) << 64) | u128::from(q_lo_lo),
        })
    }

    pub fn try_into_u64(self) -> Option<u64> {
        if self.hi != 0 {
            return None;
        }
        u64::try_from(self.lo).ok()
    }

    fn divide_word(remainder: u64, word: u64, divisor: u64) -> Option<(u64, u64)> {
        let numerator = (u128::from(remainder) << 64) | u128::from(word);
        let quotient = u64::try_from(numerator.checked_div(u128::from(divisor))?).ok()?;
        let remainder = u64::try_from(numerator.checked_rem(u128::from(divisor))?).ok()?;
        Some((quotient, remainder))
    }
}

impl From<u128> for TwapCumulative {
    fn from(value: u128) -> Self {
        Self::from_low(value)
    }
}

/// Accumulator value at a boundary crossed by a newly recorded observation.
/// Observations are weighted backward over the interval ending at their record
/// block (04 §7).
pub fn accumulator_at_boundary(
    previous_block: BlockNumber,
    previous_cumulative: TwapCumulative,
    observation: FixedU64,
    boundary: BlockNumber,
) -> Option<TwapCumulative> {
    let elapsed = boundary.checked_sub(previous_block)?;
    previous_cumulative.checked_add_product(observation.0, u64::from(elapsed))
}

/// Exact fixed-grid mean between two cumulative checkpoints.
pub fn twap_between(
    start: TwapCumulative,
    end: TwapCumulative,
    blocks: BlockNumber,
) -> Option<FixedU64> {
    let value = end
        .checked_sub(start)?
        .checked_div_by_blocks(u64::from(blocks))?
        .try_into_u64()?;
    Some(FixedU64(value))
}

/// Scheduled-interval coverage check (05 §5). Division is avoided so the
/// comparison has no rounding ambiguity.
pub fn coverage_at_least(
    observations: u32,
    window: BlockNumber,
    interval: BlockNumber,
    required_pct: u8,
) -> bool {
    if interval == 0 || required_pct > 100 {
        return false;
    }
    let expected = window / interval;
    expected > 0
        && observations.saturating_mul(100) >= expected.saturating_mul(u32::from(required_pct))
}

/// Maker-loss depth for one seeded book, rounded down so security sizing never
/// overstates the capital available to absorb manipulation flow.
pub fn maker_loss_floor(b: Balance) -> Option<Balance> {
    fixed_to_base_units_down(fx(b).ok()?.checked_mul(LN_2).ok()?).ok()
}

/// Exact per-book POL obligation created by seeding, rounded up in the same
/// maker-solvent direction as the actual ledger split (04 §10; 08 §1.2/§3).
/// Treasury NAV mirroring and epoch budget prediction call this function too,
/// so no parallel `b·ln 2` formula can drift from the cash seed path.
pub fn seed_headroom(b: Balance) -> Result<Balance, Error> {
    fixed_to_base_units_up(fx(b)?.checked_mul(LN_2).map_err(map_fixed)?)
}

/// Reconstruct the maker's worst-world loss at an LMSR quantity state.
///
/// Path independence makes net premium held at `(q_long, q_short)` equal to
/// `C(q_long, q_short) - C(0, 0)`. The worst settlement pays the larger
/// outstanding quantity, hence loss is
/// `max(q_long, q_short) - premium`. Premium is rounded up exactly like a buy
/// charge; sell proceeds round down, so the real book inventory can only retain
/// at least this much premium. The result is therefore a conservative upper
/// bound on realized custody loss, and never exceeds the seeded `b·ln 2`
/// headroom (04 §3–§6.3).
pub fn maker_loss_at_state(
    b: Balance,
    q_long: Balance,
    q_short: Balance,
) -> Result<Balance, Error> {
    let initial = lmsr_cost(FixedU64x64::ZERO, FixedU64x64::ZERO, fx(b)?).map_err(map_fixed)?;
    let current = lmsr_cost(fx(q_long)?, fx(q_short)?, fx(b)?).map_err(map_fixed)?;
    let premium = current.checked_sub(initial).map_err(map_fixed)?;
    let premium_held = fixed_to_base_units_up(premium)?;
    let loss = q_long
        .max(q_short)
        .checked_sub(premium_held)
        .unwrap_or_default();
    if loss > seed_headroom(b)? {
        return Err(Error::TryStateViolation);
    }
    Ok(loss)
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
pub struct MarketBook<AccountId> {
    pub id: MarketId,
    pub kind: BookKind,
    pub phase: MarketPhase,
    pub account: AccountId,
    pub fees_account: AccountId,
    pub b: Balance,
    pub q_long: Balance,
    pub q_short: Balance,
    pub fees_accrued: Balance,
    pub last_quote_1e9: FixedU64,
    pub last_observation_1e9: FixedU64,
    pub last_observed_block: u64,
    pub cumulative_price_blocks: TwapCumulative,
    pub stale_events: u8,
}

impl<AccountId> MarketBook<AccountId> {
    /// Construct a fresh trading book with the canonical neutral quote (04 §2).
    pub fn open(
        id: MarketId,
        kind: BookKind,
        account: AccountId,
        fees_account: AccountId,
        b: Balance,
    ) -> Self {
        Self {
            id,
            kind,
            phase: MarketPhase::Trading,
            account,
            fees_account,
            b,
            q_long: 0,
            q_short: 0,
            fees_accrued: 0,
            last_quote_1e9: FixedU64(500_000_000),
            last_observation_1e9: FixedU64(500_000_000),
            last_observed_block: 0,
            cumulative_price_blocks: TwapCumulative::ZERO,
            stale_events: 0,
        }
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
pub enum Event<AccountId> {
    MarketCreated(MarketId),
    BaselineMarketMapped(EpochId, MarketId),
    Seeded(MarketId, Balance),
    Traded {
        market: MarketId,
        who: AccountId,
        side: TradeSide,
        amount: Balance,
        cost: Balance,
        p_after: FixedU64,
    },
    Observed {
        market: MarketId,
        o_t: FixedU64,
    },
    Closed(MarketId),
    Reaped(MarketId),
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
pub enum Error {
    UnknownMarket,
    DuplicateMarket,
    DuplicateBaselineMarket,
    BadOrigin,
    NotTrading,
    AmountTooSmall,
    AmountTooLarge,
    SlippageExceeded,
    PriceBoundExceeded,
    ArithmeticOverflow,
    Ledger,
    TryStateViolation,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct MarketState<AccountId> {
    pub markets: Vec<MarketBook<AccountId>>,
    pub baseline_market_of: Vec<(EpochId, MarketId)>,
    pub events: Vec<Event<AccountId>>,
}

impl<AccountId: Clone + Eq> MarketState<AccountId> {
    pub const fn new() -> Self {
        Self {
            markets: Vec::new(),
            baseline_market_of: Vec::new(),
            events: Vec::new(),
        }
    }

    pub fn create_market(
        &mut self,
        id: MarketId,
        kind: BookKind,
        account: AccountId,
        fees_account: AccountId,
        b: Balance,
    ) -> Result<(), Error> {
        ensure!(
            self.markets.iter().all(|m| m.id != id),
            Error::DuplicateMarket
        );
        if let BookKind::Baseline { epoch } = kind {
            ensure!(
                self.baseline_market_of.iter().all(|(e, _)| *e != epoch),
                Error::DuplicateBaselineMarket
            );
            self.baseline_market_of.push((epoch, id));
            self.events.push(Event::BaselineMarketMapped(epoch, id));
        }
        self.markets
            .push(MarketBook::open(id, kind, account, fees_account, b));
        self.events.push(Event::MarketCreated(id));
        Ok(())
    }

    pub fn seed(
        &mut self,
        ledger: &mut LedgerState<AccountId>,
        id: MarketId,
        treasury: &AccountId,
    ) -> Result<Balance, Error> {
        let idx = self
            .markets
            .iter()
            .position(|m| m.id == id)
            .ok_or(Error::UnknownMarket)?;
        let headroom = seed_book(&self.markets[idx], ledger, treasury)?;
        self.events.push(Event::Seeded(id, headroom));
        Ok(headroom)
    }

    pub fn buy(
        &mut self,
        ledger: &mut LedgerState<AccountId>,
        id: MarketId,
        who: &AccountId,
        side: ScalarSide,
        amount: Balance,
        max_cost: Balance,
        block: u64,
    ) -> Result<(), Error> {
        // 04 §6.4: buy/sell are atomic with all ledger moves - a failure at
        // any wrapper step restores both the book and the ledger
        // (Codex review, PR #34).
        let market_snapshot = self.clone();
        let ledger_snapshot = ledger.clone();
        let result = self.buy_inner(ledger, id, who, side, amount, max_cost, block);
        if result.is_err() {
            *self = market_snapshot;
            *ledger = ledger_snapshot;
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn buy_inner(
        &mut self,
        ledger: &mut LedgerState<AccountId>,
        id: MarketId,
        who: &AccountId,
        side: ScalarSide,
        amount: Balance,
        max_cost: Balance,
        block: u64,
    ) -> Result<(), Error> {
        let idx = self
            .markets
            .iter()
            .position(|m| m.id == id)
            .ok_or(Error::UnknownMarket)?;
        let events = buy_book(
            &mut self.markets[idx],
            ledger,
            &MarketParams::default(),
            who,
            side,
            amount,
            max_cost,
            block,
        )?;
        self.events.extend(events);
        Ok(())
    }

    pub fn sell(
        &mut self,
        ledger: &mut LedgerState<AccountId>,
        id: MarketId,
        who: &AccountId,
        side: ScalarSide,
        amount: Balance,
        min_proceeds: Balance,
        block: u64,
    ) -> Result<(), Error> {
        // See buy(): the sell wrapper is equally atomic across its ledger
        // moves, so e.g. a net Baseline payout below the split floor cannot
        // strand the seller's already-merged leg (Codex review, PR #34).
        let market_snapshot = self.clone();
        let ledger_snapshot = ledger.clone();
        let result = self.sell_inner(ledger, id, who, side, amount, min_proceeds, block);
        if result.is_err() {
            *self = market_snapshot;
            *ledger = ledger_snapshot;
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn sell_inner(
        &mut self,
        ledger: &mut LedgerState<AccountId>,
        id: MarketId,
        who: &AccountId,
        side: ScalarSide,
        amount: Balance,
        min_proceeds: Balance,
        block: u64,
    ) -> Result<(), Error> {
        let idx = self
            .markets
            .iter()
            .position(|m| m.id == id)
            .ok_or(Error::UnknownMarket)?;
        let events = sell_book(
            &mut self.markets[idx],
            ledger,
            &MarketParams::default(),
            who,
            side,
            amount,
            min_proceeds,
            block,
        )?;
        self.events.extend(events);
        Ok(())
    }

    pub fn close(&mut self, id: MarketId) -> Result<(), Error> {
        let m = self.market_mut(id)?;
        m.phase = MarketPhase::Closed;
        self.events.push(Event::Closed(id));
        Ok(())
    }
    pub fn baseline_market(&self, epoch: EpochId) -> Option<MarketId> {
        self.baseline_market_of
            .iter()
            .find(|(e, _)| *e == epoch)
            .map(|(_, m)| *m)
    }
    pub fn try_state(&self) -> Result<(), Error> {
        for m in &self.markets {
            ensure!(m.b > 0, Error::TryStateViolation);
            ensure!(
                m.q_long <= 48 * m.b && m.q_short <= 48 * m.b
                    || m.q_long.abs_diff(m.q_short) <= 48 * m.b,
                Error::PriceBoundExceeded
            );
        }
        Ok(())
    }

    fn market_mut(&mut self, id: MarketId) -> Result<&mut MarketBook<AccountId>, Error> {
        self.markets
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or(Error::UnknownMarket)
    }
}

impl<AccountId: Clone + Eq> Default for MarketState<AccountId> {
    fn default() -> Self {
        Self::new()
    }
}

/// Mutation-free 02 §4 / 04 §6 quote using the exact fixed-point and
/// maker-adverse currency-rounding paths used by [`buy_book`] and
/// [`sell_book`]. `cost` is the gross buy charge or gross sell proceeds before
/// fee. Per 02 §4, `within_domain` reports only the post-trade LMSR domain;
/// 11 §11.5 P-1 makes the independent per-trade bound a separate frontend
/// precondition, exposed here by `max_trade`. A closed book or an unevaluable
/// post-state rejects.
pub fn quote<A>(
    m: &MarketBook<A>,
    side: TradeSide,
    amount: Balance,
    fee_bps: u128,
) -> Result<QuoteView, Error> {
    ensure_trade_phase(m.phase)?;
    let (post_long, post_short, cost) = match side {
        TradeSide::BuyLong => {
            let post_long = add(m.q_long, amount)?;
            let cost = lmsr_buy_cost(
                fx(m.q_long)?,
                fx(m.q_short)?,
                fx(m.b)?,
                LmsrSide::Long,
                fx(amount)?,
            )
            .map_err(map_fixed)?;
            (post_long, m.q_short, fixed_to_base_units_up(cost)?)
        }
        TradeSide::BuyShort => {
            let post_short = add(m.q_short, amount)?;
            let cost = lmsr_buy_cost(
                fx(m.q_long)?,
                fx(m.q_short)?,
                fx(m.b)?,
                LmsrSide::Short,
                fx(amount)?,
            )
            .map_err(map_fixed)?;
            (m.q_long, post_short, fixed_to_base_units_up(cost)?)
        }
        TradeSide::SellLong => {
            let post_long = sub(m.q_long, amount)?;
            let proceeds = lmsr_sell_proceeds(
                fx(m.q_long)?,
                fx(m.q_short)?,
                fx(m.b)?,
                LmsrSide::Long,
                fx(amount)?,
            )
            .map_err(map_fixed)?;
            (post_long, m.q_short, fixed_to_base_units_down(proceeds)?)
        }
        TradeSide::SellShort => {
            let post_short = sub(m.q_short, amount)?;
            let proceeds = lmsr_sell_proceeds(
                fx(m.q_long)?,
                fx(m.q_short)?,
                fx(m.b)?,
                LmsrSide::Short,
                fx(amount)?,
            )
            .map_err(map_fixed)?;
            (m.q_long, post_short, fixed_to_base_units_down(proceeds)?)
        }
    };
    let p_after_1e9 = price_1e9_quantities(post_long, post_short, m.b)?;
    let max_trade = max_trade_amount(m.b);
    // 02 §4 freezes this field as the post-trade |q_L-q_S|/b predicate only;
    // the 11 §11.5 P-1 min/max check remains independently visible through
    // `max_trade` and the MinTrade metadata constant.
    let within_domain = quantities_within_domain(post_long, post_short, m.b);
    Ok(QuoteView {
        cost,
        fee: fee_up(cost, fee_bps)?,
        p_after_1e9,
        max_trade,
        within_domain,
    })
}

/// Execute one buy against a single book using the supplied ledger adapter.
#[allow(clippy::too_many_arguments)]
pub fn buy_book<A: Clone + Eq, L: LedgerOps<A>>(
    m: &mut MarketBook<A>,
    ledger: &mut L,
    params: &MarketParams,
    who: &A,
    side: ScalarSide,
    amount: Balance,
    max_cost: Balance,
    block: u64,
) -> Result<Vec<Event<A>>, Error> {
    ensure_trade_phase(m.phase)?;
    ensure_trade_bounds(m.b, amount)?;
    let cost_fx = lmsr_buy_cost(
        fx(m.q_long)?,
        fx(m.q_short)?,
        fx(m.b)?,
        lside(side),
        fx(amount)?,
    )
    .map_err(map_fixed)?;
    let cost = fixed_to_base_units_up(cost_fx)?;
    let fee = fee_up(cost, params.fee_bps)?;
    ensure!(
        cost.checked_add(fee).ok_or(Error::ArithmeticOverflow)? <= max_cost,
        Error::SlippageExceeded
    );
    match m.kind {
        BookKind::Decision { proposal, branch } => buy_branch(
            ledger,
            proposal,
            branch,
            side,
            who,
            &m.account,
            &m.fees_account,
            amount,
            cost,
            fee,
        )?,
        BookKind::Gate {
            proposal,
            branch,
            gate,
        } => buy_gate(
            ledger,
            proposal,
            branch,
            gate,
            side,
            who,
            &m.account,
            &m.fees_account,
            amount,
            cost,
            fee,
        )?,
        BookKind::Baseline { epoch } => buy_baseline(
            ledger,
            epoch,
            side,
            who,
            &m.account,
            amount,
            cost.checked_add(fee).ok_or(Error::ArithmeticOverflow)?,
        )?,
    }
    match side {
        ScalarSide::Long => m.q_long = add(m.q_long, amount)?,
        ScalarSide::Short => m.q_short = add(m.q_short, amount)?,
    }
    m.fees_accrued = add(m.fees_accrued, fee)?;
    let observed = observe_book(m, params, block)?;
    let p = price_1e9(m)?;
    m.last_quote_1e9 = p;
    let mut events = Vec::new();
    if let Some(event) = observed {
        events.push(event);
    }
    events.push(Event::Traded {
        market: m.id,
        who: who.clone(),
        side: if matches!(side, ScalarSide::Long) {
            TradeSide::BuyLong
        } else {
            TradeSide::BuyShort
        },
        amount,
        cost,
        p_after: p,
    });
    Ok(events)
}

/// Execute one sell against a single book using the supplied ledger adapter.
#[allow(clippy::too_many_arguments)]
pub fn sell_book<A: Clone + Eq, L: LedgerOps<A>>(
    m: &mut MarketBook<A>,
    ledger: &mut L,
    params: &MarketParams,
    who: &A,
    side: ScalarSide,
    amount: Balance,
    min_proceeds: Balance,
    block: u64,
) -> Result<Vec<Event<A>>, Error> {
    ensure_trade_phase(m.phase)?;
    ensure_trade_bounds(m.b, amount)?;
    let proceeds_fx = lmsr_sell_proceeds(
        fx(m.q_long)?,
        fx(m.q_short)?,
        fx(m.b)?,
        lside(side),
        fx(amount)?,
    )
    .map_err(map_fixed)?;
    let proceeds = fixed_to_base_units_down(proceeds_fx)?;
    let fee = fee_up(proceeds, params.fee_bps)?;
    let net = sub(proceeds, fee)?;
    ensure!(net >= min_proceeds, Error::SlippageExceeded);
    match m.kind {
        BookKind::Decision { proposal, branch } => sell_branch(
            ledger,
            proposal,
            branch,
            side,
            who,
            &m.account,
            &m.fees_account,
            amount,
            net,
            fee,
        )?,
        BookKind::Gate {
            proposal,
            branch,
            gate,
        } => sell_gate(
            ledger,
            proposal,
            branch,
            gate,
            side,
            who,
            &m.account,
            &m.fees_account,
            amount,
            net,
            fee,
        )?,
        BookKind::Baseline { epoch } => {
            sell_baseline(ledger, epoch, side, who, &m.account, amount, net, fee)?
        }
    }
    match side {
        ScalarSide::Long => m.q_long = sub(m.q_long, amount)?,
        ScalarSide::Short => m.q_short = sub(m.q_short, amount)?,
    }
    m.fees_accrued = add(m.fees_accrued, fee)?;
    let observed = observe_book(m, params, block)?;
    let p = price_1e9(m)?;
    m.last_quote_1e9 = p;
    let mut events = Vec::new();
    if let Some(event) = observed {
        events.push(event);
    }
    events.push(Event::Traded {
        market: m.id,
        who: who.clone(),
        side: if matches!(side, ScalarSide::Long) {
            TradeSide::SellLong
        } else {
            TradeSide::SellShort
        },
        amount,
        cost: proceeds,
        p_after: p,
    });
    Ok(events)
}

/// Seed one book with its LMSR worst-case-loss headroom (04 §10).
pub fn seed_book<A: Clone + Eq, L: LedgerOps<A>>(
    m: &MarketBook<A>,
    ledger: &mut L,
    treasury: &A,
) -> Result<Balance, Error> {
    let headroom = seed_headroom(m.b)?;
    ledger.note_protocol_account(m.account.clone());
    ledger.note_protocol_account(m.fees_account.clone());
    match m.kind {
        BookKind::Decision { proposal, branch } => {
            ledger
                .do_split(proposal, treasury, headroom)
                .map_err(|_| Error::Ledger)?;
            ledger
                .do_transfer(
                    position(proposal, branch, PositionKind::BranchUsdc),
                    treasury,
                    &m.account,
                    headroom,
                )
                .map_err(|_| Error::Ledger)?;
            ledger
                .do_split_scalar(proposal, branch, &m.account, headroom)
                .map_err(|_| Error::Ledger)?;
        }
        BookKind::Gate {
            proposal,
            branch,
            gate,
        } => {
            ledger
                .do_split(proposal, treasury, headroom)
                .map_err(|_| Error::Ledger)?;
            ledger
                .do_transfer(
                    position(proposal, branch, PositionKind::BranchUsdc),
                    treasury,
                    &m.account,
                    headroom,
                )
                .map_err(|_| Error::Ledger)?;
            ledger
                .do_split_gate(proposal, branch, gate, &m.account, headroom)
                .map_err(|_| Error::Ledger)?;
        }
        BookKind::Baseline { epoch } => {
            // POL_BASELINE (the `treasury` arg) funds the seed (04 §8.3/§10), NOT the
            // book account: the FRAME ledger charges the split payer's USDC, and the
            // deterministic book account is not the POL_BASELINE treasury line, so
            // splitting to `m.account` would fail (book unfunded) or drain the wrong
            // account. Treasury mints the headroom set, then both legs move to the
            // book — the unbranched analogue of the decision/gate seed paths.
            ledger
                .do_split_baseline(epoch, treasury, headroom)
                .map_err(|_| Error::Ledger)?;
            ledger
                .do_transfer(
                    baseline(epoch, ScalarSide::Long),
                    treasury,
                    &m.account,
                    headroom,
                )
                .map_err(|_| Error::Ledger)?;
            ledger
                .do_transfer(
                    baseline(epoch, ScalarSide::Short),
                    treasury,
                    &m.account,
                    headroom,
                )
                .map_err(|_| Error::Ledger)?;
        }
    }
    Ok(headroom)
}

/// Seed the Accept/Reject pair from one collateral split. A proposal split
/// mints both branch legs; consuming one split per book would strand mirror
/// legs in POL and double the specified budget (04 §10; 08 §8.4).
pub fn seed_branch_pair<A: Clone + Eq, L: LedgerOps<A>>(
    accept: &MarketBook<A>,
    reject: &MarketBook<A>,
    ledger: &mut L,
    treasury: &A,
) -> Result<Balance, Error> {
    ensure!(
        accept.id != reject.id && accept.b == reject.b,
        Error::TryStateViolation
    );
    let (proposal, gate) = match (accept.kind, reject.kind) {
        (
            BookKind::Decision {
                proposal: left,
                branch: Branch::Accept,
            },
            BookKind::Decision {
                proposal: right,
                branch: Branch::Reject,
            },
        ) if left == right => (left, None),
        (
            BookKind::Gate {
                proposal: left,
                branch: Branch::Accept,
                gate: left_gate,
            },
            BookKind::Gate {
                proposal: right,
                branch: Branch::Reject,
                gate: right_gate,
            },
        ) if left == right && left_gate == right_gate => (left, Some(left_gate)),
        _ => return Err(Error::TryStateViolation),
    };
    let headroom = seed_headroom(accept.b)?;
    for book in [accept, reject] {
        ledger.note_protocol_account(book.account.clone());
        ledger.note_protocol_account(book.fees_account.clone());
    }
    ledger
        .do_split(proposal, treasury, headroom)
        .map_err(|_| Error::Ledger)?;
    for (book, branch) in [(accept, Branch::Accept), (reject, Branch::Reject)] {
        ledger
            .do_transfer(
                position(proposal, branch, PositionKind::BranchUsdc),
                treasury,
                &book.account,
                headroom,
            )
            .map_err(|_| Error::Ledger)?;
        match gate {
            Some(gate) => ledger
                .do_split_gate(proposal, branch, gate, &book.account, headroom)
                .map_err(|_| Error::Ledger)?,
            None => ledger
                .do_split_scalar(proposal, branch, &book.account, headroom)
                .map_err(|_| Error::Ledger)?,
        }
    }
    Ok(headroom)
}

fn buy_branch<A: Clone + Eq, L: LedgerOps<A>>(
    ledger: &mut L,
    pid: ProposalId,
    branch: Branch,
    side: ScalarSide,
    who: &A,
    book: &A,
    fees: &A,
    amount: Balance,
    cost: Balance,
    fee: Balance,
) -> Result<(), Error> {
    let total = add(cost, fee)?;
    ledger
        .do_split(pid, who, total)
        .map_err(|_| Error::Ledger)?;
    let mirror = other(branch);
    ledger
        .do_transfer(
            position(pid, branch, PositionKind::BranchUsdc),
            who,
            book,
            cost,
        )
        .map_err(|_| Error::Ledger)?;
    // Zero-sized fee legs are skipped (a governed fee of 0 bps is legal and
    // the ledger rejects zero transfers — S1 re-pass finding).
    if fee > 0 {
        ledger
            .do_transfer(
                position(pid, branch, PositionKind::BranchUsdc),
                who,
                fees,
                fee,
            )
            .map_err(|_| Error::Ledger)?;
        ledger
            .do_transfer(
                position(pid, mirror, PositionKind::BranchUsdc),
                who,
                fees,
                fee,
            )
            .map_err(|_| Error::Ledger)?;
    }
    ledger
        .do_split_scalar(pid, branch, book, cost)
        .map_err(|_| Error::Ledger)?;
    ledger
        .do_transfer(position(pid, branch, scalar_kind(side)), book, who, amount)
        .map_err(|_| Error::Ledger)?;
    Ok(())
}
fn buy_gate<A: Clone + Eq, L: LedgerOps<A>>(
    ledger: &mut L,
    pid: ProposalId,
    branch: Branch,
    gate: GateType,
    side: ScalarSide,
    who: &A,
    book: &A,
    fees: &A,
    amount: Balance,
    cost: Balance,
    fee: Balance,
) -> Result<(), Error> {
    let total = add(cost, fee)?;
    ledger
        .do_split(pid, who, total)
        .map_err(|_| Error::Ledger)?;
    let mirror = other(branch);
    ledger
        .do_transfer(
            position(pid, branch, PositionKind::BranchUsdc),
            who,
            book,
            cost,
        )
        .map_err(|_| Error::Ledger)?;
    // Zero-sized fee legs are skipped (a governed fee of 0 bps is legal and
    // the ledger rejects zero transfers — S1 re-pass finding).
    if fee > 0 {
        ledger
            .do_transfer(
                position(pid, branch, PositionKind::BranchUsdc),
                who,
                fees,
                fee,
            )
            .map_err(|_| Error::Ledger)?;
        ledger
            .do_transfer(
                position(pid, mirror, PositionKind::BranchUsdc),
                who,
                fees,
                fee,
            )
            .map_err(|_| Error::Ledger)?;
    }
    ledger
        .do_split_gate(pid, branch, gate, book, cost)
        .map_err(|_| Error::Ledger)?;
    ledger
        .do_transfer(
            position(pid, branch, gate_kind(gate, side)),
            book,
            who,
            amount,
        )
        .map_err(|_| Error::Ledger)?;
    Ok(())
}
fn buy_baseline<A: Clone + Eq, L: LedgerOps<A>>(
    ledger: &mut L,
    epoch: EpochId,
    side: ScalarSide,
    who: &A,
    book: &A,
    amount: Balance,
    total: Balance,
) -> Result<(), Error> {
    // 04 §6.1 Baseline degenerate wrapper: cost + fee pays in directly and
    // there is no mirror credit - the buyer must not retain a fee-sized
    // set pair, so both full legs move to the book.
    ledger
        .do_split_baseline(epoch, who, total)
        .map_err(|_| Error::Ledger)?;
    ledger
        .do_transfer(baseline(epoch, ScalarSide::Long), who, book, total)
        .map_err(|_| Error::Ledger)?;
    ledger
        .do_transfer(baseline(epoch, ScalarSide::Short), who, book, total)
        .map_err(|_| Error::Ledger)?;
    ledger
        .do_transfer(baseline(epoch, side), book, who, amount)
        .map_err(|_| Error::Ledger)?;
    Ok(())
}
fn sell_branch<A: Clone + Eq, L: LedgerOps<A>>(
    ledger: &mut L,
    pid: ProposalId,
    branch: Branch,
    side: ScalarSide,
    who: &A,
    book: &A,
    fees: &A,
    amount: Balance,
    net: Balance,
    fee: Balance,
) -> Result<(), Error> {
    ledger
        .do_transfer(position(pid, branch, scalar_kind(side)), who, book, amount)
        .map_err(|_| Error::Ledger)?;
    // 04 §6.1: the book "raises the payout" by merging complete sets — exactly
    // the net+fee bUSDC the trade needs, not the full received `amount`. Merging
    // `amount` required mirror-side inventory >= amount and made legal sells fail
    // with InsufficientPosition after asymmetric buys depleted the mirror leg
    // (S1 I-12 property suite); the payout-sized merge is the spec reading and
    // leaves the surplus legs as book inventory (a complete set == 1 bUSDC).
    // Zero-sized legs are skipped: a spec-legal sell whose proceeds floor to 0
    // (04 §4 claimant-adverse rounding at extreme prices) must succeed paying 0,
    // and the ledger rejects zero transfers (S1 re-pass finding).
    let payout = add(net, fee)?;
    if payout > 0 {
        ledger
            .do_merge_scalar(pid, branch, book, payout)
            .map_err(|_| Error::Ledger)?;
    }
    if fee > 0 {
        ledger
            .do_transfer(
                position(pid, branch, PositionKind::BranchUsdc),
                book,
                fees,
                fee,
            )
            .map_err(|_| Error::Ledger)?;
    }
    if net > 0 {
        ledger
            .do_transfer(
                position(pid, branch, PositionKind::BranchUsdc),
                book,
                who,
                net,
            )
            .map_err(|_| Error::Ledger)?;
    }
    merge_net_with_mirror(ledger, pid, branch, who, net)?;
    Ok(())
}
fn sell_gate<A: Clone + Eq, L: LedgerOps<A>>(
    ledger: &mut L,
    pid: ProposalId,
    branch: Branch,
    gate: GateType,
    side: ScalarSide,
    who: &A,
    book: &A,
    fees: &A,
    amount: Balance,
    net: Balance,
    fee: Balance,
) -> Result<(), Error> {
    ledger
        .do_transfer(
            position(pid, branch, gate_kind(gate, side)),
            who,
            book,
            amount,
        )
        .map_err(|_| Error::Ledger)?;
    // Payout-sized merge with zero-legs skipped, same 04 §6.1 reading as
    // `sell_branch`.
    let payout = add(net, fee)?;
    if payout > 0 {
        ledger
            .do_merge_gate(pid, branch, gate, book, payout)
            .map_err(|_| Error::Ledger)?;
    }
    if fee > 0 {
        ledger
            .do_transfer(
                position(pid, branch, PositionKind::BranchUsdc),
                book,
                fees,
                fee,
            )
            .map_err(|_| Error::Ledger)?;
    }
    if net > 0 {
        ledger
            .do_transfer(
                position(pid, branch, PositionKind::BranchUsdc),
                book,
                who,
                net,
            )
            .map_err(|_| Error::Ledger)?;
    }
    merge_net_with_mirror(ledger, pid, branch, who, net)?;
    Ok(())
}

/// 04 §6.1 sell wrapper, final step: automatically merge the net
/// target-branch proceeds with the seller's mirror-branch branch-USDC
/// balance - up to min(net, mirror balance) - into USDC; any unmatched
/// remainder stays with the seller as target-branch branch-USDC.
fn merge_net_with_mirror<A: Clone + Eq, L: LedgerOps<A>>(
    ledger: &mut L,
    pid: ProposalId,
    branch: Branch,
    who: &A,
    net: Balance,
) -> Result<(), Error> {
    let mirror = other(branch);
    let mirror_balance =
        ledger.position_balance(position(pid, mirror, PositionKind::BranchUsdc), who);
    let merge_amount = net.min(mirror_balance);
    if merge_amount > 0 {
        ledger
            .do_merge(pid, who, merge_amount)
            .map_err(|_| Error::Ledger)?;
    }
    Ok(())
}
fn sell_baseline<A: Clone + Eq, L: LedgerOps<A>>(
    ledger: &mut L,
    epoch: EpochId,
    side: ScalarSide,
    who: &A,
    book: &A,
    amount: Balance,
    net: Balance,
    fee: Balance,
) -> Result<(), Error> {
    // The 30 bps fee is withheld from the payout (04 §6.1): the seller receives
    // net-of-fee value; the withheld remainder stays with the book. The seller's
    // `amount` leg is merged by the book into USDC, then the BOOK funds the `net`
    // payout complete set and hands both legs to the seller. A seller must never pay
    // for their own proceeds — the unbranched Baseline book has no mirror leg to
    // merge against (cf. `merge_net_with_mirror` for decision/gate), so the payout is
    // delivered in-kind as a par-redeemable set the book, not the seller, funds.
    // (`do_split_baseline(who, net)` here would debit the seller ~net USDC against the
    // real ledger — the in-memory oracle models no custody and hid it; Codex A3 review.)
    ledger
        .do_transfer(baseline(epoch, side), who, book, amount)
        .map_err(|_| Error::Ledger)?;
    // Payout-sized merge (04 §6.1, same reading as `sell_branch`): net+fee pairs
    // fund the seller's net set plus the fee custody in USDC; merging the full
    // `amount` needed mirror inventory >= amount and could fail a legal sell.
    let payout = add(net, fee)?;
    if payout > 0 {
        ledger
            .do_merge_baseline(epoch, book, payout)
            .map_err(|_| Error::Ledger)?;
    }
    if net > 0 {
        ledger
            .do_split_baseline(epoch, book, net)
            .map_err(|_| Error::Ledger)?;
        ledger
            .do_transfer(baseline(epoch, ScalarSide::Long), book, who, net)
            .map_err(|_| Error::Ledger)?;
        ledger
            .do_transfer(baseline(epoch, ScalarSide::Short), book, who, net)
            .map_err(|_| Error::Ledger)?;
    }
    Ok(())
}

pub fn observe_book<A: Clone + Eq>(
    m: &mut MarketBook<A>,
    params: &MarketParams,
    block: u64,
) -> Result<Option<Event<A>>, Error> {
    ensure!(params.obs_interval > 0, Error::ArithmeticOverflow);
    ensure!(params.kappa_1e9 <= PRICE_ONE_1E9, Error::ArithmeticOverflow);
    if block < m.last_observed_block.saturating_add(params.obs_interval) {
        return Ok(None);
    }
    let elapsed = block.saturating_sub(m.last_observed_block);
    if elapsed > params.stale_gap_blocks {
        m.stale_events = m.stale_events.saturating_add(1);
    }
    let prev = m.last_quote_1e9.0;
    let old = m.last_observation_1e9.0;
    // 04 §7: over k missed observation intervals the slew clamp widens to
    // (1±kappa)^k; flooring the interval count is the conservative reading.
    // Rounding is directional so the computed clamp always sits INSIDE the
    // exact real-arithmetic envelope (I-13): the lower bound rounds UP
    // (per-step flooring drifted it below exact (1-kappa)^k, admitting extra
    // downward drift — S1 I-13 property finding), the upper bound rounds down.
    let intervals = (elapsed / params.obs_interval).max(1);
    let low = mul_1e9_up(old, pow_1e9_up(PRICE_ONE_1E9 - params.kappa_1e9, intervals));
    let high = mul_1e9(
        old,
        pow_1e9(
            PRICE_ONE_1E9
                .checked_add(params.kappa_1e9)
                .ok_or(Error::ArithmeticOverflow)?,
            intervals,
        ),
    );
    let capped = prev.clamp(low, high);
    m.cumulative_price_blocks = m
        .cumulative_price_blocks
        .checked_add_product(capped, elapsed)
        .ok_or(Error::ArithmeticOverflow)?;
    m.last_observation_1e9 = FixedU64(capped);
    m.last_observed_block = block;
    Ok(Some(Event::Observed {
        market: m.id,
        o_t: FixedU64(capped),
    }))
}
/// Saturating 1e9-scale multiply; results are only consumed clamped to the
/// price range, so capping intermediates at 2e9 keeps every product exact
/// where it matters and overflow-free.
pub fn mul_1e9(a: u64, b: u64) -> u64 {
    ((u128::from(a) * u128::from(b)) / u128::from(PRICE_ONE_1E9)).min(u128::from(u64::MAX)) as u64
}
/// 1e9-scale integer power by squaring. Saturates at 1e18 so that even the
/// smallest representable observation (one raw unit) widens to the full
/// [0, 1] price band under a long enough gap - a 2x cap would under-widen
/// low observations (Codex review, PR #34).
pub fn pow_1e9(base: u64, mut exp: u64) -> u64 {
    const CAP: u64 = PRICE_ONE_1E9 * PRICE_ONE_1E9;
    let mut result = PRICE_ONE_1E9;
    let mut factor = base.min(CAP);
    while exp > 0 {
        if exp & 1 == 1 {
            result = mul_1e9(result, factor).min(CAP);
        }
        exp >>= 1;
        if exp > 0 {
            factor = mul_1e9(factor, factor).min(CAP);
        }
    }
    result
}

/// 1e9-scale multiply rounding UP. Used for the lower slew clamp so the
/// computed bound never falls below the exact real-arithmetic value (I-13).
pub fn mul_1e9_up(a: u64, b: u64) -> u64 {
    ((u128::from(a) * u128::from(b)).div_ceil(u128::from(PRICE_ONE_1E9))).min(u128::from(u64::MAX))
        as u64
}

/// 1e9-scale integer power by squaring, rounding every step UP: the result is
/// at least the exact base^exp, making it the sound companion of `mul_1e9_up`
/// for the LOWER clamp bound (a lower bound may be tightened, never loosened).
pub fn pow_1e9_up(base: u64, mut exp: u64) -> u64 {
    const CAP: u64 = PRICE_ONE_1E9 * PRICE_ONE_1E9;
    let mut result = PRICE_ONE_1E9;
    let mut factor = base.min(CAP);
    while exp > 0 {
        if exp & 1 == 1 {
            result = mul_1e9_up(result, factor).min(CAP);
        }
        exp >>= 1;
        if exp > 0 {
            factor = mul_1e9_up(factor, factor).min(CAP);
        }
    }
    result
}
fn price_1e9<A>(m: &MarketBook<A>) -> Result<FixedU64, Error> {
    price_1e9_quantities(m.q_long, m.q_short, m.b)
}
fn price_1e9_quantities(q_long: Balance, q_short: Balance, b: Balance) -> Result<FixedU64, Error> {
    let p = lmsr_price_long(fx(q_long)?, fx(q_short)?, fx(b)?).map_err(map_fixed)?;
    Ok(FixedU64(
        (p.raw()
            .checked_mul(PRICE_ONE_1E9 as u128)
            .ok_or(Error::ArithmeticOverflow)?
            >> 64) as u64,
    ))
}
fn fixed_to_base_units_up(v: FixedU64x64) -> Result<Balance, Error> {
    let scaled = v
        .checked_mul(FixedU64x64::from_integer(1_000_000))
        .map_err(map_fixed)?;
    Ok(round_charge_up(scaled))
}
fn fixed_to_base_units_down(v: FixedU64x64) -> Result<Balance, Error> {
    let scaled = v
        .checked_mul(FixedU64x64::from_integer(1_000_000))
        .map_err(map_fixed)?;
    Ok(round_payout_down(scaled))
}
fn fx(v: Balance) -> Result<FixedU64x64, Error> {
    let units = v / 1_000_000;
    FixedU64x64::from_integer(u64::try_from(units).map_err(|_| Error::ArithmeticOverflow)?)
        .checked_add(FixedU64x64::from_raw(((v % 1_000_000) << 64) / 1_000_000))
        .map_err(map_fixed)
}
pub fn fee_up(cost: Balance, fee_bps: u128) -> Result<Balance, Error> {
    let v = cost.checked_mul(fee_bps).ok_or(Error::ArithmeticOverflow)?;
    Ok(v / BPS_DENOM + u128::from(v % BPS_DENOM != 0))
}
fn quantities_within_domain(q_long: Balance, q_short: Balance, b: Balance) -> bool {
    if b == 0 {
        return false;
    }
    let diff = q_long.abs_diff(q_short);
    let bound = Balance::from(kernel::LMSR_DOMAIN_BOUND);
    let quotient = diff / b;
    quotient < bound || (quotient == bound && diff % b == 0)
}
/// Read-only phase half of trade admission (04 §6.4). FRAME wrappers and
/// runtime views reuse this predicate so a non-trading book can never quote as
/// executable while `buy`/`sell` reject it.
pub fn ensure_trade_phase(p: MarketPhase) -> Result<(), Error> {
    ensure!(
        matches!(p, MarketPhase::Trading | MarketPhase::Extended),
        Error::NotTrading
    );
    Ok(())
}
/// Maximum amount admitted by one trade for a book with liquidity parameter
/// `b` (04 §6.2 / 13 §2). An invalid future kernel ratio fails closed to zero.
pub fn max_trade_amount(b: Balance) -> Balance {
    let (numerator, denominator) = kernel::MAX_TRADE_RATIO;
    let Some(scaled) = b.checked_mul(Balance::from(numerator)) else {
        return 0;
    };
    let Some(value) = scaled.checked_div(Balance::from(denominator)) else {
        return 0;
    };
    value
}
fn ensure_trade_bounds(b: Balance, a: Balance) -> Result<(), Error> {
    ensure!(a >= MIN_TRADE, Error::AmountTooSmall);
    ensure!(a <= max_trade_amount(b), Error::AmountTooLarge);
    Ok(())
}
fn lside(s: ScalarSide) -> LmsrSide {
    if matches!(s, ScalarSide::Long) {
        LmsrSide::Long
    } else {
        LmsrSide::Short
    }
}
fn scalar_kind(s: ScalarSide) -> PositionKind {
    if matches!(s, ScalarSide::Long) {
        PositionKind::Long
    } else {
        PositionKind::Short
    }
}
fn gate_kind(g: GateType, s: ScalarSide) -> PositionKind {
    if matches!(s, ScalarSide::Long) {
        PositionKind::GateYes(g)
    } else {
        PositionKind::GateNo(g)
    }
}
fn other(b: Branch) -> Branch {
    if matches!(b, Branch::Accept) {
        Branch::Reject
    } else {
        Branch::Accept
    }
}
fn add<T: CheckedAdd>(a: T, b: T) -> Result<T, Error> {
    a.cadd(b)
}
fn sub(a: Balance, b: Balance) -> Result<Balance, Error> {
    a.checked_sub(b).ok_or(Error::ArithmeticOverflow)
}
trait CheckedAdd: Sized {
    fn cadd(self, rhs: Self) -> Result<Self, Error>;
}
impl CheckedAdd for u128 {
    fn cadd(self, rhs: Self) -> Result<Self, Error> {
        self.checked_add(rhs).ok_or(Error::ArithmeticOverflow)
    }
}
fn map_fixed(e: FixedError) -> Error {
    match e {
        FixedError::Domain => Error::PriceBoundExceeded,
        FixedError::Overflow | FixedError::DivisionByZero | FixedError::NonFinite => {
            Error::ArithmeticOverflow
        }
    }
}
macro_rules! ensure {
    ($cond:expr, $err:expr) => {
        if !$cond {
            return Err($err);
        }
    };
}
use ensure;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking {
    use super::*;

    pub fn benchmark_buy() -> Result<(), Error> {
        let mut ledger = LedgerState::<u64>::new();
        ledger.create_vault(1, 0).map_err(|_| Error::Ledger)?;
        let mut markets = MarketState::new();
        markets.create_market(
            1,
            BookKind::Decision {
                proposal: 1,
                branch: Branch::Accept,
            },
            900,
            901,
            10_000_000_000,
        )?;
        markets.seed(&mut ledger, 1, &100)?;
        markets.buy(
            &mut ledger,
            1,
            &200,
            ScalarSide::Long,
            MIN_TRADE,
            Balance::MAX,
            OBS_INTERVAL,
        )
    }

    pub fn benchmark_crank_observe() -> Result<(), Error> {
        let mut markets = MarketState::new();
        markets.create_market(1, BookKind::Baseline { epoch: 1 }, 900, 901, 10_000_000_000)?;
        observe_book(
            &mut markets.markets[0],
            &MarketParams::default(),
            OBS_INTERVAL,
        )
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conditional_ledger_core::LedgerOrigin;
    fn a(n: u8) -> [u8; 32] {
        [n; 32]
    }
    const B: Balance = 10_000_000_000;

    #[test]
    fn twap_cumulative_checked_math_spans_both_limbs() {
        let carried = TwapCumulative::from(u128::MAX).checked_add(TwapCumulative::from(1));
        assert_eq!(carried, Some(TwapCumulative { hi: 1, lo: 0 }));
        assert_eq!(
            carried.and_then(|value| value.checked_sub(TwapCumulative::from(1))),
            Some(TwapCumulative::from(u128::MAX)),
        );
        assert_eq!(
            TwapCumulative { hi: 1, lo: 0 }.checked_div_by_blocks(2),
            Some(TwapCumulative {
                hi: 0,
                lo: 1u128 << 127,
            }),
        );
        assert_eq!(TwapCumulative::default(), TwapCumulative::ZERO);
        assert_eq!(TwapCumulative::max_encoded_len(), 32);
    }

    #[test]
    fn twap_cumulative_failures_are_checked_and_low_limb_results_are_identical() {
        let maximum = TwapCumulative {
            hi: u128::MAX,
            lo: u128::MAX,
        };
        assert_eq!(maximum.checked_add(TwapCumulative::from(1)), None);
        assert_eq!(
            TwapCumulative::ZERO.checked_sub(TwapCumulative::from(1)),
            None
        );
        assert_eq!(TwapCumulative::from(1).checked_div_by_blocks(0), None);

        let start = TwapCumulative {
            hi: 0,
            lo: u128::MAX - 4_999_999_999,
        };
        let end = TwapCumulative { hi: 1, lo: 0 };
        assert_eq!(
            accumulator_at_boundary(0, start, FixedU64(500_000_000), 10),
            Some(end),
        );
        assert_eq!(twap_between(start, end, 10), Some(FixedU64(500_000_000)),);
        assert_eq!(twap_between(start, end, 0), None);
        assert_eq!(twap_between(end, start, 10), None);
    }

    #[test]
    fn quote_matches_buy_and_sell_execution_paths_without_mutating() {
        let mut ledger = LedgerState::new();
        ledger.create_vault(1, 0).unwrap();
        let mut markets = MarketState::new();
        markets
            .create_market(
                7,
                BookKind::Decision {
                    proposal: 1,
                    branch: Branch::Accept,
                },
                a(9),
                a(8),
                B,
            )
            .unwrap();
        markets.seed(&mut ledger, 7, &a(1)).unwrap();

        let before = markets.markets[0];
        let buy_quote = quote(&before, TradeSide::BuyLong, 1_000_000_000, FEE_BPS).unwrap();
        let exact_buy = lmsr_buy_cost(
            fx(before.q_long).unwrap(),
            fx(before.q_short).unwrap(),
            fx(before.b).unwrap(),
            LmsrSide::Long,
            fx(1_000_000_000).unwrap(),
        )
        .unwrap();
        assert_eq!(buy_quote.cost, fixed_to_base_units_up(exact_buy).unwrap());
        assert_eq!(markets.markets[0], before, "quote must be mutation-free");
        markets
            .buy(
                &mut ledger,
                7,
                &a(2),
                ScalarSide::Long,
                1_000_000_000,
                Balance::MAX,
                10,
            )
            .unwrap();
        assert_eq!(markets.markets[0].fees_accrued, buy_quote.fee);
        assert_eq!(markets.markets[0].last_quote_1e9, buy_quote.p_after_1e9);
        assert!(matches!(
            markets.events.last(),
            Some(Event::Traded { cost, .. }) if *cost == buy_quote.cost
        ));

        let before_sell = markets.markets[0];
        let sell_quote = quote(&before_sell, TradeSide::SellLong, 1_000_000_000, FEE_BPS).unwrap();
        let exact_sell = lmsr_sell_proceeds(
            fx(before_sell.q_long).unwrap(),
            fx(before_sell.q_short).unwrap(),
            fx(before_sell.b).unwrap(),
            LmsrSide::Long,
            fx(1_000_000_000).unwrap(),
        )
        .unwrap();
        assert_eq!(
            sell_quote.cost,
            fixed_to_base_units_down(exact_sell).unwrap()
        );
        markets
            .sell(
                &mut ledger,
                7,
                &a(2),
                ScalarSide::Long,
                1_000_000_000,
                0,
                20,
            )
            .unwrap();
        assert_eq!(
            markets.markets[0].fees_accrued.checked_sub(buy_quote.fee),
            Some(sell_quote.fee)
        );
        assert_eq!(markets.markets[0].last_quote_1e9, sell_quote.p_after_1e9);
        assert!(matches!(
            markets.events.last(),
            Some(Event::Traded { cost, .. }) if *cost == sell_quote.cost
        ));
    }

    #[test]
    fn maker_loss_reconstructs_normative_v1_v3_and_v4_states() -> Result<(), Error> {
        assert_eq!(maker_loss_at_state(B, 0, 0), Ok(0));

        // 04 §5 V1: 1,000 LONG costs 512.494795136 USDC. Maker-favourable
        // charge rounding leaves 487.505204 USDC of realized loss.
        let v1 = maker_loss_at_state(B, 1_000_000_000, 0)?;
        assert!(v1.abs_diff(487_505_204) <= 1, "V1 loss {v1}");

        // V3: q = 10,000·ln(1.5), cost = 2,231.43551314 USDC.
        let v3 = maker_loss_at_state(B, 4_054_651_081, 0)?;
        assert!(v3.abs_diff(1_823_215_567) <= 1, "V3 loss {v3}");

        // V4: the finite domain edge approaches, but cannot exceed, b·ln 2.
        let edge = B
            .checked_mul(Balance::from(kernel::LMSR_DOMAIN_BOUND))
            .ok_or(Error::ArithmeticOverflow)?;
        let v4 = maker_loss_at_state(B, edge, 0)?;
        let bound = seed_headroom(B)?;
        assert!(v4 <= bound);
        assert!(
            bound.saturating_sub(v4) <= 1,
            "edge loss {v4}, bound {bound}"
        );
        Ok(())
    }

    #[test]
    fn quote_reports_domain_independently_from_trade_bounds_and_rejects_bad_math() {
        let book = MarketBook::open(1, BookKind::Baseline { epoch: 1 }, a(1), a(2), B);
        let over_limit =
            quote(&book, TradeSide::BuyLong, max_trade_amount(B) + 1, FEE_BPS).unwrap();
        // 02 §4 freezes `within_domain` to the post-trade LMSR predicate;
        // 11 §11.5 P-1 makes the per-trade maximum a separate FE check.
        assert!(over_limit.within_domain);
        assert!(over_limit.cost > 0);
        assert_eq!(over_limit.max_trade, max_trade_amount(B));
        assert!(max_trade_amount(B) + 1 > over_limit.max_trade);
        assert_eq!(
            quote(&book, TradeSide::SellLong, 1, FEE_BPS),
            Err(Error::ArithmeticOverflow)
        );
    }

    #[test]
    fn buy_wrapper_collects_complete_pair_fee_and_records_twap() {
        let mut ledger = LedgerState::new();
        ledger.create_vault(1, 0).unwrap();
        let mut m = MarketState::new();
        m.create_market(
            7,
            BookKind::Decision {
                proposal: 1,
                branch: Branch::Accept,
            },
            a(9),
            a(8),
            B,
        )
        .unwrap();
        m.seed(&mut ledger, 7, &a(1)).unwrap();
        m.buy(
            &mut ledger,
            7,
            &a(2),
            ScalarSide::Long,
            1_000_000_000,
            600_000_000,
            10,
        )
        .unwrap();
        assert!(ledger.positions.iter().any(|p| p.owner == a(2)
            && p.id == position(1, Branch::Accept, PositionKind::Long)
            && p.balance == 1_000_000_000));
        assert_eq!(m.markets[0].q_long, 1_000_000_000);
        assert!(matches!(
            m.events.last().unwrap(),
            Event::Traded {
                side: TradeSide::BuyLong,
                ..
            }
        ));
        ledger.try_state().unwrap();
        m.try_state().unwrap();
    }
    #[test]
    fn traded_events_carry_the_trader() {
        // Codex review, PR #17 (P1): 02 §6 freezes
        // Traded { market, who, side, amount, cost, p_after }.
        let mut ledger = LedgerState::new();
        ledger.create_vault(1, 0).unwrap();
        let mut m = MarketState::new();
        m.create_market(
            7,
            BookKind::Decision {
                proposal: 1,
                branch: Branch::Accept,
            },
            a(9),
            a(8),
            B,
        )
        .unwrap();
        m.seed(&mut ledger, 7, &a(1)).unwrap();
        m.buy(
            &mut ledger,
            7,
            &a(2),
            ScalarSide::Long,
            1_000_000_000,
            600_000_000,
            10,
        )
        .unwrap();
        assert!(matches!(
            m.events.last().unwrap(),
            Event::Traded { who, side: TradeSide::BuyLong, .. } if *who == a(2)
        ));
    }

    #[test]
    fn decision_sell_round_trip_releases_usdc_via_mirror_merge() {
        // Codex review, PR #17 (P1): after a buy-then-sell round trip the
        // wrapper must merge the net target-branch proceeds with the seller's
        // mirror-branch balance into USDC, not strand them as branch-USDC.
        let mut ledger = LedgerState::new();
        ledger.create_vault(1, 0).unwrap();
        let mut m = MarketState::new();
        m.create_market(
            7,
            BookKind::Decision {
                proposal: 1,
                branch: Branch::Accept,
            },
            a(9),
            a(8),
            B,
        )
        .unwrap();
        m.seed(&mut ledger, 7, &a(1)).unwrap();
        let trader = a(2);
        m.buy(
            &mut ledger,
            7,
            &trader,
            ScalarSide::Long,
            1_000_000_000,
            600_000_000,
            10,
        )
        .unwrap();
        let escrow_before_sell = ledger.vaults[0].info.escrowed;
        let mirror_before = balance_of(
            &ledger,
            position(1, Branch::Reject, PositionKind::BranchUsdc),
            &trader,
        );
        assert!(mirror_before > 0);
        m.sell(
            &mut ledger,
            7,
            &trader,
            ScalarSide::Long,
            1_000_000_000,
            1,
            20,
        )
        .unwrap();
        // All net target-branch proceeds were merged against the mirror leg:
        // nothing strands as Accept-branch branch-USDC...
        assert_eq!(
            balance_of(
                &ledger,
                position(1, Branch::Accept, PositionKind::BranchUsdc),
                &trader
            ),
            0
        );
        // ...the mirror balance shrank by the merged net, and the vault
        // escrow released that USDC to the seller.
        let mirror_after = balance_of(
            &ledger,
            position(1, Branch::Reject, PositionKind::BranchUsdc),
            &trader,
        );
        let released = escrow_before_sell - ledger.vaults[0].info.escrowed;
        assert!(released > 0);
        assert_eq!(mirror_before - mirror_after, released);
        ledger.try_state().unwrap();
        m.try_state().unwrap();
    }

    #[test]
    fn baseline_fees_are_withheld_on_both_sides() {
        // Codex review, PR #17 (P2): the buyer must not retain a fee-sized
        // complete pair, and sells must pay out net of the 30 bps fee.
        let mut ledger = LedgerState::new();
        ledger.create_baseline_vault(3).unwrap();
        let mut m = MarketState::new();
        m.create_market(11, BookKind::Baseline { epoch: 3 }, a(9), a(8), B)
            .unwrap();
        m.seed(&mut ledger, 11, &a(1)).unwrap();
        let trader = a(2);
        m.buy(
            &mut ledger,
            11,
            &trader,
            ScalarSide::Long,
            1_000_000_000,
            600_000_000,
            10,
        )
        .unwrap();
        // Exactly the bought LONG leg - no residual SHORT (fee pair) with the
        // buyer.
        assert_eq!(
            balance_of(&ledger, baseline(3, ScalarSide::Long), &trader),
            1_000_000_000
        );
        assert_eq!(
            balance_of(&ledger, baseline(3, ScalarSide::Short), &trader),
            0
        );
        let buy_fee = m.markets[0].fees_accrued;
        assert!(buy_fee > 0);
        m.sell(
            &mut ledger,
            11,
            &trader,
            ScalarSide::Long,
            1_000_000_000,
            1,
            20,
        )
        .unwrap();
        // The seller's payout pair equals proceeds net of fee: total fees
        // accrued grew by the sell fee and the payout reflects it.
        assert!(m.markets[0].fees_accrued > buy_fee);
        let payout_pair = balance_of(&ledger, baseline(3, ScalarSide::Long), &trader);
        assert_eq!(
            payout_pair,
            balance_of(&ledger, baseline(3, ScalarSide::Short), &trader)
        );
        assert!(payout_pair > 0);
        ledger.try_state().unwrap();
        m.try_state().unwrap();
    }

    #[test]
    fn minimum_trades_with_dust_fees_are_admissible() {
        // Codex review, PR #17 (P2): a valid 1 USDC minimum trade near p=0.5
        // carries a 30 bps fee below MinTransfer; the wrapper's exact
        // MarketAuthority moves must still route it (03 R-2/R-3).
        let mut ledger = LedgerState::new();
        ledger.create_vault(1, 0).unwrap();
        let mut m = MarketState::new();
        m.create_market(
            7,
            BookKind::Decision {
                proposal: 1,
                branch: Branch::Accept,
            },
            a(9),
            a(8),
            B,
        )
        .unwrap();
        m.seed(&mut ledger, 7, &a(1)).unwrap();
        m.buy(
            &mut ledger,
            7,
            &a(2),
            ScalarSide::Long,
            1_000_000,
            600_000,
            10,
        )
        .unwrap();
        let fee = m.markets[0].fees_accrued;
        assert!(fee > 0 && fee < 10_000, "fee {fee} should be dust-sized");
        // The fee pair actually reached the fees account.
        assert_eq!(
            balance_of(
                &ledger,
                position(1, Branch::Accept, PositionKind::BranchUsdc),
                &a(8)
            ),
            fee
        );
        assert_eq!(
            balance_of(
                &ledger,
                position(1, Branch::Reject, PositionKind::BranchUsdc),
                &a(8)
            ),
            fee
        );
        ledger.try_state().unwrap();
    }

    #[test]
    fn twap_clamp_widens_over_missed_intervals() {
        // Codex review, PR #17 (P2): 04 §7 widens the clamp to (1±kappa)^k
        // over k missed observation intervals.
        assert_eq!(pow_1e9(PRICE_ONE_1E9, 100), PRICE_ONE_1E9);
        let up = pow_1e9(PRICE_ONE_1E9 + KAPPA_1E9, 10);
        assert!((1_051_100_000..1_051_200_000).contains(&up), "up {up}");
        let down = pow_1e9(PRICE_ONE_1E9 - KAPPA_1E9, 10);
        assert!((951_100_000..951_200_000).contains(&down), "down {down}");
        // End to end: a quote jump after ten missed intervals records at the
        // widened bound rather than one kappa step.
        let mut ledger = LedgerState::new();
        ledger.create_vault(1, 0).unwrap();
        let mut m = MarketState::new();
        m.create_market(
            7,
            BookKind::Decision {
                proposal: 1,
                branch: Branch::Accept,
            },
            a(9),
            a(8),
            B,
        )
        .unwrap();
        m.seed(&mut ledger, 7, &a(1)).unwrap();
        // Move the quote sharply at block 10 (the observation there records
        // the pre-trade 0.5 quote per 04 §7), then trade again after ten
        // missed intervals so the stored post-move quote is observed.
        m.buy(
            &mut ledger,
            7,
            &a(2),
            ScalarSide::Long,
            2_500_000_000,
            2_000_000_000,
            10,
        )
        .unwrap();
        m.buy(
            &mut ledger,
            7,
            &a(2),
            ScalarSide::Long,
            1_000_000,
            600_000,
            110,
        )
        .unwrap();
        let observed = m.markets[0].last_observation_1e9.0;
        let single_step = mul_1e9(500_000_000, PRICE_ONE_1E9 + KAPPA_1E9);
        let widened = mul_1e9(500_000_000, pow_1e9(PRICE_ONE_1E9 + KAPPA_1E9, 10));
        assert!(
            observed > single_step,
            "observed {observed} vs single-step {single_step}"
        );
        assert!(
            observed <= widened,
            "observed {observed} vs widened {widened}"
        );
    }

    #[test]
    fn wrapper_payouts_to_users_respect_the_creation_floor_atomically() {
        // Codex review, PR #34: MarketAuthority moves are floor-exempt only
        // toward protocol destinations - a payout that would create a
        // sub-MinTransfer deposit-backed user position is rejected at the
        // ledger.
        let mut ledger: LedgerState<[u8; 32]> = LedgerState::new();
        ledger.create_vault(1, 0).unwrap();
        ledger
            .split(LedgerOrigin::Signed, 1, &a(4), 1_000_000)
            .unwrap();
        assert_eq!(
            ledger.transfer(
                LedgerOrigin::MarketAuthority,
                position(1, Branch::Accept, PositionKind::BranchUsdc),
                &a(4),
                &a(5),
                9_999,
            ),
            Err(conditional_ledger_core::Error::AmountTooSmall)
        );

        // And a wrapper step failing mid-sell rolls the whole trade back
        // instead of stranding the seller's already-moved leg: the seller is
        // at the position cap and sells only part of the LONG, so the net
        // payout needs a fresh target-branch entry and hits the cap after the
        // leg transfer and merge already ran.
        let mut ledger = LedgerState::new();
        ledger.create_vault(1, 0).unwrap();
        let mut m = MarketState::new();
        m.create_market(
            7,
            BookKind::Decision {
                proposal: 1,
                branch: Branch::Accept,
            },
            a(9),
            a(8),
            B,
        )
        .unwrap();
        m.seed(&mut ledger, 7, &a(1)).unwrap();
        let trader = a(2);
        m.buy(
            &mut ledger,
            7,
            &trader,
            ScalarSide::Long,
            2_000_000_000,
            1_500_000_000,
            10,
        )
        .unwrap();
        // Fill the remaining 62 slots (the trader holds LONG + mirror bUSDC).
        for i in 0..31u64 {
            let pid = 1_000 + i;
            ledger.create_vault(pid, 0).unwrap();
            ledger
                .split(LedgerOrigin::Signed, pid, &trader, 1_000_000)
                .unwrap();
        }
        let held = balance_of(
            &ledger,
            position(1, Branch::Accept, PositionKind::Long),
            &trader,
        );
        assert_eq!(held, 2_000_000_000);
        let q_long_before = m.markets[0].q_long;
        let escrow_before = ledger.vaults[0].info.escrowed;
        assert_eq!(
            m.sell(
                &mut ledger,
                7,
                &trader,
                ScalarSide::Long,
                1_000_000_000,
                1,
                20
            )
            .unwrap_err(),
            Error::Ledger
        );
        assert_eq!(
            balance_of(
                &ledger,
                position(1, Branch::Accept, PositionKind::Long),
                &trader
            ),
            held
        );
        assert_eq!(m.markets[0].q_long, q_long_before);
        assert_eq!(ledger.vaults[0].info.escrowed, escrow_before);
        ledger.try_state().unwrap();
        m.try_state().unwrap();
    }

    #[test]
    fn twap_widening_reaches_the_full_band_from_low_observations() {
        // Codex review, PR #34: a 2x pow cap under-widened low observations
        // (old = 0.10 with (1.005)^k > 10 must admit 1.0).
        let widened = mul_1e9(100_000_000, pow_1e9(PRICE_ONE_1E9 + KAPPA_1E9, 462));
        assert!(widened >= PRICE_ONE_1E9, "widened {widened}");
        let from_dust = mul_1e9(1, pow_1e9(PRICE_ONE_1E9 + KAPPA_1E9, 10_000));
        assert!(from_dust >= PRICE_ONE_1E9, "from_dust {from_dust}");
    }

    fn balance_of(
        ledger: &LedgerState<[u8; 32]>,
        id: futarchy_primitives::PositionId,
        who: &[u8; 32],
    ) -> Balance {
        ledger
            .positions
            .iter()
            .find(|p| p.id == id && &p.owner == who)
            .map_or(0, |p| p.balance)
    }

    #[test]
    fn slippage_phase_and_trade_bounds_are_enforced() {
        let mut ledger = LedgerState::new();
        ledger.create_vault(1, 0).unwrap();
        let mut m = MarketState::new();
        m.create_market(
            7,
            BookKind::Decision {
                proposal: 1,
                branch: Branch::Accept,
            },
            a(9),
            a(8),
            B,
        )
        .unwrap();
        m.seed(&mut ledger, 7, &a(1)).unwrap();
        assert_eq!(
            m.buy(
                &mut ledger,
                7,
                &a(2),
                ScalarSide::Long,
                1_000_000_000,
                1,
                10
            ),
            Err(Error::SlippageExceeded)
        );
        assert_eq!(
            m.buy(
                &mut ledger,
                7,
                &a(2),
                ScalarSide::Long,
                max_trade_amount(B).saturating_add(1),
                Balance::MAX,
                10
            ),
            Err(Error::AmountTooLarge)
        );
        m.close(7).unwrap();
        assert_eq!(
            m.buy(
                &mut ledger,
                7,
                &a(2),
                ScalarSide::Long,
                1_000_000,
                Balance::MAX,
                20
            ),
            Err(Error::NotTrading)
        );
    }
    #[test]
    fn baseline_market_mapping_is_written() {
        let mut m: MarketState<[u8; 32]> = MarketState::new();
        m.create_market(3, BookKind::Baseline { epoch: 42 }, a(9), a(8), B)
            .unwrap();
        assert_eq!(m.baseline_market(42), Some(3));
        assert_eq!(
            m.create_market(4, BookKind::Baseline { epoch: 42 }, a(7), a(6), B),
            Err(Error::DuplicateBaselineMarket)
        );
    }
}
