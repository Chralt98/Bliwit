//! I-12/I-13 systemic market properties (04 §§4, 6.3, 7; 15 §4.3).

use conditional_ledger_core::{position, LedgerState};
use futarchy_primitives::{
    Balance, Branch, FixedU64, GateType, PositionId, PositionKind, ScalarSide,
};
use market_core::{
    buy_book, observe_book, seed_book, sell_book, BookKind, Error as MarketError,
    Event as MarketEvent, LedgerOps, MarketBook, MarketParams, FEE_BPS, MIN_TRADE, PRICE_ONE_1E9,
};
use proptest::{
    prelude::*,
    test_runner::{Config as ProptestConfig, RngSeed},
};

const USDC: Balance = 1_000_000;
const PROPOSAL: u64 = 1;
const BOOK: u8 = 9;
const FEES: u8 = 8;
const TREASURY: u8 = 7;
const TRADER: u8 = 1;

fn property_config(seed: u64) -> ProptestConfig {
    let mut config = ProptestConfig::default();
    if std::env::var_os("PROPTEST_RNG_SEED").is_none() {
        config.rng_seed = RngSeed::Fixed(seed);
    }
    config.failure_persistence = None;
    config
}

fn side_kind(side: ScalarSide) -> PositionKind {
    match side {
        ScalarSide::Long => PositionKind::Long,
        ScalarSide::Short => PositionKind::Short,
    }
}

fn balance(ledger: &TestLedger, owner: u8, branch: Branch, kind: PositionKind) -> Balance {
    ledger.position_balance(position(PROPOSAL, branch, kind), &owner)
}

/// Test adapter over the real in-memory core ledger. It forwards every call
/// unmodified — including zero amounts, which the core rejects — so the seam
/// behaves exactly like production (the wrapper itself skips zero-sized legs
/// per 04 §6.1; S1 re-pass finding).
#[derive(Clone, Debug)]
struct TestLedger(LedgerState<u8>);

impl LedgerOps<u8> for TestLedger {
    fn do_split(&mut self, pid: u64, who: &u8, amount: Balance) -> Result<(), ()> {
        self.0.do_split(pid, who, amount).map_err(|_| ())
    }

    fn do_transfer(
        &mut self,
        id: PositionId,
        from: &u8,
        to: &u8,
        amount: Balance,
    ) -> Result<(), ()> {
        self.0.do_transfer(id, from, to, amount).map_err(|_| ())
    }

    fn do_split_scalar(
        &mut self,
        pid: u64,
        branch: Branch,
        who: &u8,
        amount: Balance,
    ) -> Result<(), ()> {
        self.0
            .do_split_scalar(pid, branch, who, amount)
            .map_err(|_| ())
    }

    fn do_split_gate(
        &mut self,
        pid: u64,
        branch: Branch,
        gate: GateType,
        who: &u8,
        amount: Balance,
    ) -> Result<(), ()> {
        self.0
            .do_split_gate(pid, branch, gate, who, amount)
            .map_err(|_| ())
    }

    fn do_split_baseline(&mut self, epoch: u32, who: &u8, amount: Balance) -> Result<(), ()> {
        self.0.do_split_baseline(epoch, who, amount).map_err(|_| ())
    }

    fn do_merge(&mut self, pid: u64, who: &u8, amount: Balance) -> Result<(), ()> {
        self.0.do_merge(pid, who, amount).map_err(|_| ())
    }

    fn do_merge_scalar(
        &mut self,
        pid: u64,
        branch: Branch,
        who: &u8,
        amount: Balance,
    ) -> Result<(), ()> {
        self.0
            .do_merge_scalar(pid, branch, who, amount)
            .map_err(|_| ())
    }

    fn do_merge_gate(
        &mut self,
        pid: u64,
        branch: Branch,
        gate: GateType,
        who: &u8,
        amount: Balance,
    ) -> Result<(), ()> {
        self.0
            .do_merge_gate(pid, branch, gate, who, amount)
            .map_err(|_| ())
    }

    fn do_merge_baseline(&mut self, epoch: u32, who: &u8, amount: Balance) -> Result<(), ()> {
        self.0.do_merge_baseline(epoch, who, amount).map_err(|_| ())
    }

    fn note_protocol_account(&mut self, who: u8) {
        self.0.note_protocol_account(who);
    }

    fn position_balance(&self, id: PositionId, who: &u8) -> Balance {
        self.0.position_balance(id, who)
    }
}

/// 04 §4's composed-cost approximation term, rounded up to USDC base units.
fn approximation_bound_per_trade(b: Balance) -> Balance {
    let numerator = 8u128.checked_mul(b).expect("bounded legal b");
    let denominator = 1u128 << 64;
    numerator / denominator + u128::from(numerator % denominator != 0)
}

/// Independent upper approximation to ln(2), with 27 decimal places.  This is
/// deliberately not `futarchy_fixed::LN_2` nor `seed_book`'s returned value:
/// I-12's oracle must not share the implementation under test (15 §4.3).
const LN_2_CEIL_1E27: u128 = 693_147_180_559_945_309_417_232_122;
const DECIMAL_1E27: u128 = 1_000_000_000_000_000_000_000_000_000;
/// One raw unit for the single final 1e9-grid realization required by 04 §7.
/// Unlike the retired `intervals + 1` term, this does not excuse intermediate
/// rounding accumulated by an implementation-specific exponentiation scheme.
const I13_FINAL_GRID_UNITS: u64 = 1;

fn independent_b_ln_2_ceiling(b: Balance) -> Balance {
    let numerator = b
        .checked_mul(LN_2_CEIL_1E27)
        .expect("13 §1 legal b fits the 27-decimal oracle");
    numerator / DECIMAL_1E27 + u128::from(numerator % DECIMAL_1E27 != 0)
}

fn assert_loss_bound(
    book: &MarketBook<u8>,
    buy_cash: Balance,
    sell_cash: Balance,
    fee_cash: Balance,
    trades: u32,
) {
    // Independent marked-to-worst-world P&L (04 §3/§6.3): q_long/q_short are
    // the outstanding obligations, while executed trade events independently
    // account for cash received and paid. Fees are unconditional cash in the
    // separate fee account, so including them can only improve solvency.
    let obligation = book.q_long.max(book.q_short);
    let loss_before_fees = obligation
        .checked_add(sell_cash)
        .expect("bounded trade sequence")
        .saturating_sub(buy_cash);
    let realized_loss = loss_before_fees.saturating_sub(fee_cash);

    // 04 §4 permits the composed 8-ulp term plus exactly one base unit of
    // maker-benefiting final-grid rounding per executed trade.
    let per_trade = 1 + approximation_bound_per_trade(book.b);
    let rounding_bound = u128::from(trades)
        .checked_mul(per_trade)
        .expect("bounded sequence");
    let analytic_bound = independent_b_ln_2_ceiling(book.b)
        .checked_add(rounding_bound)
        .expect("bounded legal b and sequence");
    assert!(
        realized_loss <= analytic_bound,
        "I-12: loss {realized_loss}, obligations {obligation}, buys {buy_cash}, sells {sell_cash}, fees {fee_cash}, independent ceil(b ln2) {}, rounding {rounding_bound}",
        independent_b_ln_2_ceiling(book.b),
    );
    // The fee-exclusive book is the stricter oracle. A fee-routing regression
    // must not be able to hide an LMSR loss-bound violation.
    assert!(
        loss_before_fees <= analytic_bound,
        "I-12 before fees: loss {loss_before_fees} exceeds {analytic_bound}"
    );
}

fn trade_cash(events: &[MarketEvent<u8>]) -> Balance {
    events
        .iter()
        .find_map(|event| match event {
            MarketEvent::Traded { cost, .. } => Some(*cost),
            _ => None,
        })
        .expect("a successful trade emits Traded")
}

fn legal_b_strategy() -> impl Strategy<Value = Balance> {
    // 13 §1: gate books use the fixed 7,500-USDC value; the bounded
    // baseline/decision range spans 10,000..=100,000 USDC. Sample every
    // base-unit value in that range, not merely its named launch points.
    prop_oneof![Just(7_500 * USDC), (10_000 * USDC..=100_000 * USDC),]
}

fn trade_amount(b: Balance, raw: u16) -> Balance {
    let span = b / 4 - MIN_TRADE;
    MIN_TRADE + span * u128::from(raw) / u128::from(u16::MAX)
}

proptest! {
    #![proptest_config(property_config(0x1ed6_0012))]

    /// I-12: arbitrary legal buy/sell walks on both outcomes stay within the
    /// pre-collateralized b·ln2 loss envelope, with fees disabled or at the
    /// normative 30 bps.  The assertion runs after every step and at close.
    #[test]
    fn i12_lmsr_realized_loss_never_exceeds_seed_plus_rounding(
        b in legal_b_strategy(),
        fees_on in any::<bool>(),
        actions in prop::collection::vec((any::<bool>(), any::<bool>(), any::<u16>()), 1..40),
    ) {
        let params = MarketParams {
            fee_bps: if fees_on { FEE_BPS } else { 0 },
            ..MarketParams::default()
        };
        let mut ledger = TestLedger(LedgerState::new());
        ledger.0.create_vault(PROPOSAL, 0).unwrap();
        let mut book = MarketBook::open(
            1,
            BookKind::Decision { proposal: PROPOSAL, branch: Branch::Accept },
            BOOK,
            FEES,
            b,
        );
        seed_book(&book, &mut ledger, &TREASURY).unwrap();
        assert_loss_bound(&book, 0, 0, 0, 0);

        let mut successful = 0u32;
        let mut buy_cash = 0u128;
        let mut sell_cash = 0u128;
        let mut block = 0u64;
        for (prefer_sell, choose_short, raw) in actions {
            block += params.obs_interval;
            let side = if choose_short { ScalarSide::Short } else { ScalarSide::Long };
            let requested = trade_amount(b, raw);
            let held = balance(&ledger, TRADER, Branch::Accept, side_kind(side));
            let book_before = book;
            let ledger_before = ledger.clone();
            let is_sell = prefer_sell && held >= MIN_TRADE;
            let result = if is_sell {
                let amount = requested.min(held);
                sell_book(
                    &mut book,
                    &mut ledger,
                    &params,
                    &TRADER,
                    side,
                    amount,
                    0,
                    block,
                )
            } else {
                buy_book(
                    &mut book,
                    &mut ledger,
                    &params,
                    &TRADER,
                    side,
                    requested,
                    Balance::MAX,
                    block,
                )
            };
            let events = match result {
                Ok(events) => events,
                // Explicit caller-domain clamps are lawful status-quo results.
                // This generator supplies permissive slippage and >=MinTrade,
                // so reaching either arm is still retained for shrinkability.
                Err(MarketError::SlippageExceeded | MarketError::AmountTooSmall) => {
                    book = book_before;
                    ledger = ledger_before;
                    assert_loss_bound(
                        &book,
                        buy_cash,
                        sell_cash,
                        book.fees_accrued,
                        successful,
                    );
                    continue;
                }
                Err(error) => {
                    prop_assert!(
                        false,
                        "I-12 legal-domain trade failed with {error:?}; inventory/ledger failures are forbidden"
                    );
                    unreachable!("prop_assert! returns on failure")
                }
            };
            let cash = trade_cash(&events);
            if is_sell {
                sell_cash = sell_cash.checked_add(cash).expect("bounded sequence");
            } else {
                buy_cash = buy_cash.checked_add(cash).expect("bounded sequence");
            }
            successful += 1;
            ledger.0.try_state().unwrap();
            assert_loss_bound(
                &book,
                buy_cash,
                sell_cash,
                book.fees_accrued,
                successful,
            );
        }

        book.phase = market_core::MarketPhase::Closed;
        assert_loss_bound(
            &book,
            buy_cash,
            sell_cash,
            book.fees_accrued,
            successful,
        );
    }

    /// I-13: arbitrary legal κ/observation-grid parameters and arbitrary quote
    /// gaps respect (1±κ)^k.  The reference factor is iterative, independent
    /// of market-core's exponentiation-by-squaring implementation.
    #[test]
    fn i13_twap_gap_drift_and_single_interval_slew_are_bounded(
        obs_interval in 5u64..=50,
        kappa in 1_000_000u64..=20_000_000u64,
        samples in prop::collection::vec((1u64..=250, 0u64..=PRICE_ONE_1E9), 1..64),
    ) {
        let params = MarketParams {
            obs_interval,
            kappa_1e9: kappa,
            stale_gap_blocks: 50,
            ..MarketParams::default()
        };
        let mut book = MarketBook::open(
            2,
            BookKind::Decision { proposal: PROPOSAL, branch: Branch::Accept },
            BOOK,
            FEES,
            10_000 * USDC,
        );
        let mut block = 0u64;

        for (gap, quote) in samples {
            block = block.saturating_add(gap);
            book.last_quote_1e9 = FixedU64(quote);
            let before = book;
            let observed = observe_book(&mut book, &params, block).unwrap();
            let elapsed = block.saturating_sub(before.last_observed_block);
            if elapsed < obs_interval {
                prop_assert!(observed.is_none());
                prop_assert_eq!(book, before);
                continue;
            }

            prop_assert!(observed.is_some());
            let intervals = (elapsed / obs_interval).max(1);
            // Exact rational interval: old·(1±κ)^k.  Only the final 1e9-grid
            // conversion rounds; no per-multiply residue is granted (04 §7).
            let low = exact_rational_grid_bound(
                before.last_observation_1e9.0,
                PRICE_ONE_1E9 - kappa,
                intervals,
                false,
            )
            .saturating_sub(I13_FINAL_GRID_UNITS);
            let high = exact_rational_grid_bound(
                before.last_observation_1e9.0,
                PRICE_ONE_1E9 + kappa,
                intervals,
                true,
            )
            .saturating_add(I13_FINAL_GRID_UNITS);
            let recorded = book.last_observation_1e9.0;
            prop_assert!(
                recorded >= low && recorded <= high,
                "I-13: {recorded} outside [{low},{high}] over {intervals} intervals"
            );
            prop_assert_eq!(
                book.cumulative_price_blocks,
                before.cumulative_price_blocks + u128::from(recorded) * u128::from(elapsed)
            );
            if intervals == 1 {
                let one_low = reference_mul_1e9(
                    before.last_observation_1e9.0,
                    PRICE_ONE_1E9 - kappa,
                );
                let one_high = reference_mul_up_1e9(
                    before.last_observation_1e9.0,
                    PRICE_ONE_1E9 + kappa,
                );
                prop_assert!(recorded >= one_low && recorded <= one_high, "single-grid slew cap");
            }
        }
    }
}

fn reference_mul_1e9(a: u64, b: u64) -> u64 {
    ((u128::from(a) * u128::from(b)) / u128::from(PRICE_ONE_1E9)).min(u128::from(u64::MAX)) as u64
}

fn reference_mul_up_1e9(a: u64, b: u64) -> u64 {
    let product = u128::from(a) * u128::from(b);
    let denominator = u128::from(PRICE_ONE_1E9);
    (product / denominator + u128::from(product % denominator != 0)).min(u128::from(u64::MAX))
        as u64
}

/// Exact positive integer in base 1e9. The denominator of
/// `old * factor^k / 1e9^k` is therefore a digit shift, so this tiny oracle
/// needs no bigint dependency and performs no intermediate rounding.
#[derive(Clone, Debug)]
struct Base1e9(Vec<u32>);

impl Base1e9 {
    fn one() -> Self {
        Self(vec![1])
    }

    fn mul_u64(&mut self, factor: u64) {
        const BASE: u128 = 1_000_000_000;
        let mut carry = 0u128;
        for digit in &mut self.0 {
            let product = u128::from(*digit) * u128::from(factor) + carry;
            *digit = (product % BASE) as u32;
            carry = product / BASE;
        }
        while carry > 0 {
            self.0.push((carry % BASE) as u32);
            carry /= BASE;
        }
    }

    fn div_base_pow(&self, exponent: usize, round_up: bool) -> u64 {
        let mut quotient = 0u128;
        for digit in self.0.iter().skip(exponent).rev() {
            quotient = quotient
                .checked_mul(u128::from(PRICE_ONE_1E9))
                .and_then(|value| value.checked_add(u128::from(*digit)))
                .unwrap_or(u128::MAX);
        }
        if round_up && self.0.iter().take(exponent).any(|digit| *digit != 0) {
            quotient = quotient.saturating_add(1);
        }
        quotient.min(u128::from(u64::MAX)) as u64
    }
}

fn exact_rational_grid_bound(old: u64, factor: u64, exponent: u64, round_up: bool) -> u64 {
    let mut numerator = Base1e9::one();
    for _ in 0..exponent {
        numerator.mul_u64(factor);
    }
    numerator.mul_u64(old);
    numerator.div_base_pow(exponent as usize, round_up)
}

/// S1 re-pass regression: a spec-legal sell whose LMSR proceeds floor to zero
/// base units (04 §4 claimant-adverse rounding at an extreme price) must
/// SUCCEED paying zero — the wrapper skips its zero-sized ledger legs instead
/// of tripping the ledger's zero-transfer rejection (04 §6.1).
#[test]
fn zero_payout_sell_at_extreme_price_succeeds() {
    let b: Balance = 7_500_000_000; // 7,500 USDC — the 13 §1 gate-book floor
    let params = MarketParams {
        fee_bps: FEE_BPS,
        ..MarketParams::default()
    };
    let mut ledger = TestLedger(LedgerState::new());
    ledger.0.create_vault(PROPOSAL, 0).unwrap();
    let mut book = MarketBook::open(
        1,
        BookKind::Decision {
            proposal: PROPOSAL,
            branch: Branch::Accept,
        },
        BOOK,
        FEES,
        b,
    );
    seed_book(&book, &mut ledger, &TREASURY).unwrap();
    // Displace the quote to the LONG-worthless extreme, just inside the ±48
    // logit domain: (q_short - q_long)/b ≈ 47.998, so a minimum legal sell of
    // MIN_TRADE (1 USDC) yields proceeds ≈ 1e6·e^{-47.998} ≈ 1.4e-15 USDC →
    // floor 0 base units.
    book.q_long = MIN_TRADE;
    book.q_short = MIN_TRADE + (b / 1_000) * 47_998;

    // Hand the trader MIN_TRADE LONG through the real ledger.
    ledger.0.do_split(PROPOSAL, &TRADER, MIN_TRADE).unwrap();
    ledger
        .0
        .do_split_scalar(PROPOSAL, Branch::Accept, &TRADER, MIN_TRADE)
        .unwrap();
    let held_before = balance(&ledger, TRADER, Branch::Accept, PositionKind::Long);
    assert_eq!(held_before, MIN_TRADE);

    let events = sell_book(
        &mut book,
        &mut ledger,
        &params,
        &TRADER,
        ScalarSide::Long,
        MIN_TRADE,
        0,
        params.obs_interval,
    )
    .expect("zero-payout sell inside the legal domain must succeed");
    assert!(!events.is_empty());
    assert_eq!(
        balance(&ledger, TRADER, Branch::Accept, PositionKind::Long),
        0,
        "the sold leg left the seller"
    );
    assert_eq!(book.q_long, 0, "inventory state advanced");
}
