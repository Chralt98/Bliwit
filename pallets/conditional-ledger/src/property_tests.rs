//! Systemic ledger properties required by 03 §11 and 15 §§4.2–4.3.
//!
//! Sequence properties run against the frame-free core (the fast differential
//! oracle).  Deposit, prefix-order and reap obligations run against the FRAME
//! mock because those behaviours deliberately live only in the pallet shell.

use crate::{
    mock::*, DepositsHeld, Error as PalletError, PositionCount, PositionTotals, Positions, Vaults,
};
use conditional_ledger_core::{
    baseline, position, Error as CoreError, Event as CoreEvent, LedgerOrigin, LedgerState,
};
use frame_support::traits::{fungibles::Inspect, GetCallName};
use futarchy_primitives::{
    kernel, Balance, Branch, FixedU64, GateType, PositionId, PositionKind, ScalarSide,
};
use proptest::{
    prelude::*,
    test_runner::{Config as ProptestConfig, RngSeed, TestCaseError, TestCaseResult},
};

const PID: u64 = 1;
const BASELINE_EPOCH: u32 = 1;
const HOLDERS: [u8; 4] = [10, 11, 12, 13];

fn property_config(seed: u64) -> ProptestConfig {
    // `Config::default()` reads PROPTEST_CASES (default 256).  Pin the seed
    // unless the caller explicitly supplies PROPTEST_RNG_SEED, so normal and
    // CI runs have no clock/OS-entropy dependency.
    let mut config = ProptestConfig::default();
    if std::env::var_os("PROPTEST_RNG_SEED").is_none() {
        config.rng_seed = RngSeed::Fixed(seed);
    }
    config.failure_persistence = None;
    config
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OpTag {
    Split,
    Merge,
    SplitScalar,
    MergeScalar,
    SplitGate,
    MergeGate,
    SplitBaseline,
    MergeBaseline,
    Transfer,
    Resolve,
    Void,
    SettleScalar,
    SettleGate,
    SettleBaseline,
    Redeem,
    RedeemVoid,
    RedeemScalar,
    RedeemScalarPair,
    RedeemGate,
    RedeemBaseline,
    RedeemBaselinePair,
    SweepDust,
    SweepDustBaseline,
}

impl OpTag {
    const ALL: [Self; 23] = [
        Self::Split,
        Self::Merge,
        Self::SplitScalar,
        Self::MergeScalar,
        Self::SplitGate,
        Self::MergeGate,
        Self::SplitBaseline,
        Self::MergeBaseline,
        Self::Transfer,
        Self::Resolve,
        Self::Void,
        Self::SettleScalar,
        Self::SettleGate,
        Self::SettleBaseline,
        Self::Redeem,
        Self::RedeemVoid,
        Self::RedeemScalar,
        Self::RedeemScalarPair,
        Self::RedeemGate,
        Self::RedeemBaseline,
        Self::RedeemBaselinePair,
        Self::SweepDust,
        Self::SweepDustBaseline,
    ];
}

#[derive(Clone, Debug)]
enum Op {
    Split {
        who: u8,
        amount: Balance,
    },
    Merge {
        who: u8,
        amount: Balance,
    },
    SplitScalar {
        who: u8,
        branch: Branch,
        amount: Balance,
    },
    MergeScalar {
        who: u8,
        branch: Branch,
        amount: Balance,
    },
    SplitGate {
        who: u8,
        branch: Branch,
        gate: GateType,
        amount: Balance,
    },
    MergeGate {
        who: u8,
        branch: Branch,
        gate: GateType,
        amount: Balance,
    },
    SplitBaseline {
        who: u8,
        amount: Balance,
    },
    MergeBaseline {
        who: u8,
        amount: Balance,
    },
    Transfer {
        from: u8,
        to: u8,
        position: PositionId,
        amount: Balance,
    },
    Resolve {
        winner: Branch,
    },
    Void,
    SettleScalar {
        score: FixedU64,
    },
    SettleGate {
        gate: GateType,
        outcome: bool,
    },
    SettleBaseline {
        score: FixedU64,
    },
    Redeem {
        who: u8,
        amount: Balance,
    },
    RedeemVoid {
        who: u8,
        branch: Branch,
        kind: PositionKind,
        amount: Balance,
    },
    RedeemScalar {
        who: u8,
        side: ScalarSide,
        amount: Balance,
    },
    RedeemScalarPair {
        who: u8,
        amount: Balance,
    },
    RedeemGate {
        who: u8,
        gate: GateType,
        amount: Balance,
    },
    RedeemBaseline {
        who: u8,
        side: ScalarSide,
        amount: Balance,
    },
    RedeemBaselinePair {
        who: u8,
        amount: Balance,
    },
    SweepDust,
    SweepDustBaseline,
}

impl Op {
    fn tag(&self) -> OpTag {
        match self {
            Self::Split { .. } => OpTag::Split,
            Self::Merge { .. } => OpTag::Merge,
            Self::SplitScalar { .. } => OpTag::SplitScalar,
            Self::MergeScalar { .. } => OpTag::MergeScalar,
            Self::SplitGate { .. } => OpTag::SplitGate,
            Self::MergeGate { .. } => OpTag::MergeGate,
            Self::SplitBaseline { .. } => OpTag::SplitBaseline,
            Self::MergeBaseline { .. } => OpTag::MergeBaseline,
            Self::Transfer { .. } => OpTag::Transfer,
            Self::Resolve { .. } => OpTag::Resolve,
            Self::Void => OpTag::Void,
            Self::SettleScalar { .. } => OpTag::SettleScalar,
            Self::SettleGate { .. } => OpTag::SettleGate,
            Self::SettleBaseline { .. } => OpTag::SettleBaseline,
            Self::Redeem { .. } => OpTag::Redeem,
            Self::RedeemVoid { .. } => OpTag::RedeemVoid,
            Self::RedeemScalar { .. } => OpTag::RedeemScalar,
            Self::RedeemScalarPair { .. } => OpTag::RedeemScalarPair,
            Self::RedeemGate { .. } => OpTag::RedeemGate,
            Self::RedeemBaseline { .. } => OpTag::RedeemBaseline,
            Self::RedeemBaselinePair { .. } => OpTag::RedeemBaselinePair,
            Self::SweepDust => OpTag::SweepDust,
            Self::SweepDustBaseline => OpTag::SweepDustBaseline,
        }
    }
}

fn branch_strategy() -> impl Strategy<Value = Branch> {
    prop_oneof![Just(Branch::Accept), Just(Branch::Reject)]
}

fn gate_strategy() -> impl Strategy<Value = GateType> {
    prop_oneof![Just(GateType::Survival), Just(GateType::Security)]
}

fn side_strategy() -> impl Strategy<Value = ScalarSide> {
    prop_oneof![Just(ScalarSide::Long), Just(ScalarSide::Short)]
}

fn kind_strategy() -> impl Strategy<Value = PositionKind> {
    prop_oneof![
        Just(PositionKind::BranchUsdc),
        Just(PositionKind::Long),
        Just(PositionKind::Short),
        Just(PositionKind::GateYes(GateType::Survival)),
        Just(PositionKind::GateNo(GateType::Survival)),
        Just(PositionKind::GateYes(GateType::Security)),
        Just(PositionKind::GateNo(GateType::Security)),
    ]
}

fn position_strategy() -> impl Strategy<Value = PositionId> {
    prop_oneof![
        (branch_strategy(), kind_strategy()).prop_map(|(branch, kind)| position(PID, branch, kind)),
        side_strategy().prop_map(|side| baseline(BASELINE_EPOCH, side)),
    ]
}

fn amount_strategy() -> impl Strategy<Value = Balance> {
    0u128..=2_000_000u128
}

#[derive(Clone, Copy, Debug)]
struct WrapperBuy {
    branch: Branch,
    side: ScalarSide,
    amount: Balance,
    cost: Balance,
}

impl WrapperBuy {
    fn fee(self) -> Balance {
        // 04 §6.1 / 13 §1: buy fees are ceil(30 bps * cost).  This
        // property deliberately has no market-core dependency, so it spells
        // out the wrapper's plain-ledger inputs at the current governed value.
        (self.cost * 30).div_ceil(10_000)
    }
}

fn wrapper_buy_for(branch: Branch, side: ScalarSide) -> impl Strategy<Value = WrapperBuy> {
    (
        kernel::MIN_TRADE_USDC..=(2 * kernel::MIN_TRADE_USDC),
        100_000u128..=900_000u128,
    )
        .prop_map(move |(amount, average_price_1e6)| {
            // An LMSR buy has an average execution price strictly inside
            // (0,1).  Keep well off the domain endpoints and round cost up as
            // the maker-adverse wrapper does (04 §4/§6.1).
            let cost = (amount * average_price_1e6).div_ceil(1_000_000);
            WrapperBuy {
                branch,
                side,
                amount,
                cost,
            }
        })
}

fn wrapper_buy_strategy() -> impl Strategy<Value = WrapperBuy> {
    (
        branch_strategy(),
        side_strategy(),
        kernel::MIN_TRADE_USDC..=(2 * kernel::MIN_TRADE_USDC),
        100_000u128..=900_000u128,
    )
        .prop_map(|(branch, side, amount, average_price_1e6)| {
            let cost = (amount * average_price_1e6).div_ceil(1_000_000);
            WrapperBuy {
                branch,
                side,
                amount,
                cost,
            }
        })
}

fn wrapper_buys_strategy() -> impl Strategy<Value = Vec<WrapperBuy>> {
    (
        wrapper_buy_for(Branch::Accept, ScalarSide::Long),
        wrapper_buy_for(Branch::Accept, ScalarSide::Short),
        wrapper_buy_for(Branch::Reject, ScalarSide::Long),
        wrapper_buy_for(Branch::Reject, ScalarSide::Short),
        prop::collection::vec(wrapper_buy_strategy(), 0..5),
    )
        .prop_map(
            |(accept_long, accept_short, reject_long, reject_short, mut extras)| {
                let mut buys = vec![accept_long, accept_short, reject_long, reject_short];
                buys.append(&mut extras);
                buys
            },
        )
}

/// Scores deliberately hit the two endpoints and k±1 fixed-grid boundaries;
/// a uniform range alone makes those cases effectively unreachable (03 §11).
fn score_strategy() -> impl Strategy<Value = u64> {
    prop_oneof![
        4 => Just(0),
        4 => Just(1_000_000_000),
        2 => Just(1),
        2 => Just(999_999_999),
        4 => (0u64..=1_000u64, 0u8..3).prop_map(|(k, offset)| {
            let boundary = k * 1_000_000;
            match offset {
                0 => boundary.saturating_sub(1),
                1 => boundary,
                _ => boundary.saturating_add(1).min(1_000_000_000),
            }
        }),
        1 => 0u64..=1_000_000_000u64,
    ]
}

fn score_or_invalid_strategy() -> impl Strategy<Value = u64> {
    prop_oneof![8 => score_strategy(), 1 => Just(1_000_000_001)]
}

/// A single exhaustive tag→strategy registry. `call_tag` below is an
/// exhaustive FRAME Call→tag projection, so adding a pallet call without a
/// real generator arm is either a compile error or a completeness-test error
/// (15 §4.2).
fn generator_arm(tag: OpTag) -> BoxedStrategy<Op> {
    match tag {
        OpTag::Split => (0u8..6, amount_strategy())
            .prop_map(|(who, amount)| Op::Split { who, amount })
            .boxed(),
        OpTag::Merge => (0u8..6, amount_strategy())
            .prop_map(|(who, amount)| Op::Merge { who, amount })
            .boxed(),
        OpTag::SplitScalar => (0u8..6, branch_strategy(), amount_strategy())
            .prop_map(|(who, branch, amount)| Op::SplitScalar {
                who,
                branch,
                amount,
            })
            .boxed(),
        OpTag::MergeScalar => (0u8..6, branch_strategy(), amount_strategy())
            .prop_map(|(who, branch, amount)| Op::MergeScalar {
                who,
                branch,
                amount,
            })
            .boxed(),
        OpTag::SplitGate => (
            0u8..6,
            branch_strategy(),
            gate_strategy(),
            amount_strategy(),
        )
            .prop_map(|(who, branch, gate, amount)| Op::SplitGate {
                who,
                branch,
                gate,
                amount,
            })
            .boxed(),
        OpTag::MergeGate => (
            0u8..6,
            branch_strategy(),
            gate_strategy(),
            amount_strategy(),
        )
            .prop_map(|(who, branch, gate, amount)| Op::MergeGate {
                who,
                branch,
                gate,
                amount,
            })
            .boxed(),
        OpTag::SplitBaseline => (0u8..6, amount_strategy())
            .prop_map(|(who, amount)| Op::SplitBaseline { who, amount })
            .boxed(),
        OpTag::MergeBaseline => (0u8..6, amount_strategy())
            .prop_map(|(who, amount)| Op::MergeBaseline { who, amount })
            .boxed(),
        OpTag::Transfer => (0u8..6, 0u8..6, position_strategy(), amount_strategy())
            .prop_map(|(from, to, position, amount)| Op::Transfer {
                from,
                to,
                position,
                amount,
            })
            .boxed(),
        OpTag::Resolve => branch_strategy()
            .prop_map(|winner| Op::Resolve { winner })
            .boxed(),
        OpTag::Void => Just(Op::Void).boxed(),
        OpTag::SettleScalar => score_or_invalid_strategy()
            .prop_map(|score| Op::SettleScalar {
                score: FixedU64(score),
            })
            .boxed(),
        OpTag::SettleGate => (gate_strategy(), any::<bool>())
            .prop_map(|(gate, outcome)| Op::SettleGate { gate, outcome })
            .boxed(),
        OpTag::SettleBaseline => score_or_invalid_strategy()
            .prop_map(|score| Op::SettleBaseline {
                score: FixedU64(score),
            })
            .boxed(),
        OpTag::Redeem => (0u8..6, amount_strategy())
            .prop_map(|(who, amount)| Op::Redeem { who, amount })
            .boxed(),
        OpTag::RedeemVoid => (
            0u8..6,
            branch_strategy(),
            kind_strategy(),
            amount_strategy(),
        )
            .prop_map(|(who, branch, kind, amount)| Op::RedeemVoid {
                who,
                branch,
                kind,
                amount,
            })
            .boxed(),
        OpTag::RedeemScalar => (0u8..6, side_strategy(), amount_strategy())
            .prop_map(|(who, side, amount)| Op::RedeemScalar { who, side, amount })
            .boxed(),
        OpTag::RedeemScalarPair => (0u8..6, amount_strategy())
            .prop_map(|(who, amount)| Op::RedeemScalarPair { who, amount })
            .boxed(),
        OpTag::RedeemGate => (0u8..6, gate_strategy(), amount_strategy())
            .prop_map(|(who, gate, amount)| Op::RedeemGate { who, gate, amount })
            .boxed(),
        OpTag::RedeemBaseline => (0u8..6, side_strategy(), amount_strategy())
            .prop_map(|(who, side, amount)| Op::RedeemBaseline { who, side, amount })
            .boxed(),
        OpTag::RedeemBaselinePair => (0u8..6, amount_strategy())
            .prop_map(|(who, amount)| Op::RedeemBaselinePair { who, amount })
            .boxed(),
        OpTag::SweepDust => Just(Op::SweepDust).boxed(),
        OpTag::SweepDustBaseline => Just(Op::SweepDustBaseline).boxed(),
    }
}

fn op_strategy() -> impl Strategy<Value = Op> {
    // The frame-free core has no archive clock. Its sequence alphabet uses
    // the 21 core operations; the two remaining registered arms are routed
    // to real FRAME dispatches by PT-5b below.
    prop::sample::select(OpTag::ALL[..21].to_vec()).prop_flat_map(generator_arm)
}

fn initial_core() -> LedgerState<u8> {
    let mut state = LedgerState::new();
    state.create_vault(PID, 0).expect("fresh proposal vault");
    state
        .create_baseline_vault(BASELINE_EPOCH)
        .expect("fresh Baseline vault");
    state.add_protocol_account(5);
    state
}

fn apply_op(state: &mut LedgerState<u8>, op: &Op) -> Result<(), CoreError> {
    match *op {
        Op::Split { who, amount } => state.split(LedgerOrigin::Signed, PID, &who, amount),
        Op::Merge { who, amount } => state.merge(LedgerOrigin::Signed, PID, &who, amount),
        Op::SplitScalar {
            who,
            branch,
            amount,
        } => state.split_scalar(LedgerOrigin::Signed, PID, branch, &who, amount),
        Op::MergeScalar {
            who,
            branch,
            amount,
        } => state.merge_scalar(LedgerOrigin::Signed, PID, branch, &who, amount),
        Op::SplitGate {
            who,
            branch,
            gate,
            amount,
        } => state.split_gate(LedgerOrigin::Signed, PID, branch, gate, &who, amount),
        Op::MergeGate {
            who,
            branch,
            gate,
            amount,
        } => state.merge_gate(LedgerOrigin::Signed, PID, branch, gate, &who, amount),
        Op::SplitBaseline { who, amount } => {
            state.split_baseline(LedgerOrigin::Signed, BASELINE_EPOCH, &who, amount)
        }
        Op::MergeBaseline { who, amount } => {
            state.merge_baseline(LedgerOrigin::Signed, BASELINE_EPOCH, &who, amount)
        }
        Op::Transfer {
            from,
            to,
            position,
            amount,
        } => state.transfer(LedgerOrigin::Signed, position, &from, &to, amount),
        Op::Resolve { winner } => state.resolve(LedgerOrigin::ResolveAuthority, PID, winner),
        Op::Void => state.void(LedgerOrigin::ResolveAuthority, PID),
        Op::SettleScalar { score } => {
            state.settle_scalar(LedgerOrigin::SettleAuthority, PID, score)
        }
        Op::SettleGate { gate, outcome } => {
            state.settle_gate(LedgerOrigin::SettleAuthority, PID, gate, outcome)
        }
        Op::SettleBaseline { score } => {
            state.settle_baseline(LedgerOrigin::SettleAuthority, BASELINE_EPOCH, score)
        }
        Op::Redeem { who, amount } => state.redeem(PID, &who, amount),
        Op::RedeemVoid {
            who,
            branch,
            kind,
            amount,
        } => state.redeem_void(PID, branch, kind, &who, amount),
        Op::RedeemScalar { who, side, amount } => state.redeem_scalar(PID, side, &who, amount),
        Op::RedeemScalarPair { who, amount } => state.redeem_scalar_pair(PID, &who, amount),
        Op::RedeemGate { who, gate, amount } => state.redeem_gate(PID, gate, &who, amount),
        Op::RedeemBaseline { who, side, amount } => {
            state.redeem_baseline(BASELINE_EPOCH, side, &who, amount)
        }
        Op::RedeemBaselinePair { who, amount } => {
            state.redeem_baseline_pair(BASELINE_EPOCH, &who, amount)
        }
        Op::SweepDust | Op::SweepDustBaseline => {
            unreachable!("shell-only generator arms are routed through PT-5b")
        }
    }
}

fn total_escrow(state: &LedgerState<u8>) -> Balance {
    state
        .vaults
        .iter()
        .map(|vault| vault.info.escrowed)
        .chain(
            state
                .baseline_vaults
                .iter()
                .map(|vault| vault.info.escrowed),
        )
        .sum()
}

fn assert_escrow_equation(before: &LedgerState<u8>, after: &LedgerState<u8>) -> TestCaseResult {
    let event = after
        .events
        .last()
        .expect("successful core op emits one event");
    let before_escrow = total_escrow(before);
    let after_escrow = total_escrow(after);
    let expected = match *event {
        CoreEvent::Split(_, amount) | CoreEvent::BaselineSplit(_, amount) => {
            before_escrow.checked_add(amount)
        }
        CoreEvent::Merged(_, amount)
        | CoreEvent::BaselineMerged(_, amount)
        | CoreEvent::Redeemed(_, amount)
        | CoreEvent::ScalarPairRedeemed(_, amount)
        | CoreEvent::GateRedeemed(_, _, amount) => before_escrow.checked_sub(amount),
        CoreEvent::ScalarRedeemed(_, _, payout) | CoreEvent::BaselineRedeemed(_, _, payout) => {
            before_escrow.checked_sub(payout)
        }
        CoreEvent::VoidRedeemed(_, _, _, payout) => before_escrow.checked_sub(payout),
        _ => Some(before_escrow),
    };
    prop_assert_eq!(Some(after_escrow), expected, "03 §6 escrow state equation");
    Ok(())
}

fn total_for(state: &LedgerState<u8>, id: PositionId) -> Balance {
    state
        .position_totals
        .iter()
        .find(|total| total.id == id)
        .map_or(0, |total| total.total)
}

fn core_balance(state: &LedgerState<u8>, id: PositionId, owner: u8) -> Balance {
    state
        .positions
        .iter()
        .find(|record| record.id == id && record.owner == owner)
        .map_or(0, |record| record.balance)
}

fn wrapper_branch_index(branch: Branch) -> usize {
    match branch {
        Branch::Accept => 0,
        Branch::Reject => 1,
    }
}

fn wrapper_mirror(branch: Branch) -> Branch {
    match branch {
        Branch::Accept => Branch::Reject,
        Branch::Reject => Branch::Accept,
    }
}

fn wrapper_scalar_kind(side: ScalarSide) -> PositionKind {
    match side {
        ScalarSide::Long => PositionKind::Long,
        ScalarSide::Short => PositionKind::Short,
    }
}

fn wrapper_book(branch: Branch) -> u8 {
    match branch {
        Branch::Accept => 5,
        Branch::Reject => 6,
    }
}

fn seed_wrapper_books(state: &mut LedgerState<u8>, pid: u64, treasury: u8, inventory: Balance) {
    for branch in [Branch::Accept, Branch::Reject] {
        let book = wrapper_book(branch);
        // This is the decision-book POL seed from 04 §6.3: treasury splits
        // collateral, transfers the book's branch-USDC, and the book converts
        // it to complete scalar inventory.  Each branch has its own book.
        state
            .split(LedgerOrigin::MarketAuthority, pid, &treasury, inventory)
            .expect("POL seed split is legal");
        state
            .transfer(
                LedgerOrigin::MarketAuthority,
                position(pid, branch, PositionKind::BranchUsdc),
                &treasury,
                &book,
                inventory,
            )
            .expect("POL seed reaches the branch book");
        state
            .split_scalar(LedgerOrigin::MarketAuthority, pid, branch, &book, inventory)
            .expect("POL seed becomes complete-set inventory");
    }
}

fn apply_wrapper_buy(state: &mut LedgerState<u8>, pid: u64, buyer: u8, fees: u8, buy: WrapperBuy) {
    let book = wrapper_book(buy.branch);
    let fee = buy.fee();
    let total = buy.cost + fee;

    // Replicate market-core::buy_branch using only the ledger operations in
    // 04 §6.1: split cost+fee, route target cost and the complete fee pair,
    // leave mirror cost with the buyer, recycle book revenue into complete
    // sets, and deliver the purchased target leg from seeded inventory.
    state
        .split(LedgerOrigin::MarketAuthority, pid, &buyer, total)
        .expect("wrapper split is legal");
    state
        .transfer(
            LedgerOrigin::MarketAuthority,
            position(pid, buy.branch, PositionKind::BranchUsdc),
            &buyer,
            &book,
            buy.cost,
        )
        .expect("target-branch cost reaches book");
    state
        .transfer(
            LedgerOrigin::MarketAuthority,
            position(pid, buy.branch, PositionKind::BranchUsdc),
            &buyer,
            &fees,
            fee,
        )
        .expect("target-branch fee reaches fee account");
    state
        .transfer(
            LedgerOrigin::MarketAuthority,
            position(pid, wrapper_mirror(buy.branch), PositionKind::BranchUsdc),
            &buyer,
            &fees,
            fee,
        )
        .expect("mirror-branch fee reaches fee account");
    state
        .split_scalar(
            LedgerOrigin::MarketAuthority,
            pid,
            buy.branch,
            &book,
            buy.cost,
        )
        .expect("book recycles cost into complete sets");
    state
        .transfer(
            LedgerOrigin::MarketAuthority,
            position(pid, buy.branch, wrapper_scalar_kind(buy.side)),
            &book,
            &buyer,
            buy.amount,
        )
        .expect("book inventory delivers the purchased leg");
}

fn expected_wrapper_void_recovery(buys: &[WrapperBuy]) -> Balance {
    let mut branch_usdc = [0u128; 2];
    let mut long = [0u128; 2];
    let mut short = [0u128; 2];

    for buy in buys {
        branch_usdc[wrapper_branch_index(wrapper_mirror(buy.branch))] += buy.cost;
        let index = wrapper_branch_index(buy.branch);
        match buy.side {
            ScalarSide::Long => long[index] += buy.amount,
            ScalarSide::Short => short[index] += buy.amount,
        }
    }

    // 03 §6.4: pair-first recovery is exact at each layer.  Scalar pairs
    // merge into branch-USDC; branch pairs then merge at par; only residual
    // branch/scalar claims take the claimant-adverse half/quarter floors.
    for index in 0..2 {
        let scalar_pairs = long[index].min(short[index]);
        long[index] -= scalar_pairs;
        short[index] -= scalar_pairs;
        branch_usdc[index] += scalar_pairs;
    }
    let branch_pairs = branch_usdc[0].min(branch_usdc[1]);
    branch_usdc[0] -= branch_pairs;
    branch_usdc[1] -= branch_pairs;

    branch_pairs
        + branch_usdc[0] / 2
        + branch_usdc[1] / 2
        + long[0] / 4
        + long[1] / 4
        + short[0] / 4
        + short[1] / 4
}

fn recover_wrapper_buyer(
    state: &mut LedgerState<u8>,
    pid: u64,
    buyer: u8,
) -> Result<Balance, CoreError> {
    let before = proposal_escrow(state, pid);

    for branch in [Branch::Accept, Branch::Reject] {
        let pairs = core_balance(state, position(pid, branch, PositionKind::Long), buyer).min(
            core_balance(state, position(pid, branch, PositionKind::Short), buyer),
        );
        if pairs > 0 {
            state.merge_scalar(LedgerOrigin::Signed, pid, branch, &buyer, pairs)?;
        }
    }

    let branch_pairs = core_balance(
        state,
        position(pid, Branch::Accept, PositionKind::BranchUsdc),
        buyer,
    )
    .min(core_balance(
        state,
        position(pid, Branch::Reject, PositionKind::BranchUsdc),
        buyer,
    ));
    if branch_pairs > 0 {
        state.merge(LedgerOrigin::Signed, pid, &buyer, branch_pairs)?;
    }

    for branch in [Branch::Accept, Branch::Reject] {
        for kind in [
            PositionKind::BranchUsdc,
            PositionKind::Long,
            PositionKind::Short,
        ] {
            let balance = core_balance(state, position(pid, branch, kind), buyer);
            if balance > 0 {
                state.redeem_void(pid, branch, kind, &buyer, balance)?;
            }
        }
    }

    Ok(before - proposal_escrow(state, pid))
}

fn assert_no_undeclared_mint(
    before: &LedgerState<u8>,
    after: &LedgerState<u8>,
    op: &Op,
) -> TestCaseResult {
    for total in &after.position_totals {
        let old = total_for(before, total.id);
        if total.total > old {
            prop_assert!(
                matches!(
                    op,
                    Op::Split { .. }
                        | Op::SplitScalar { .. }
                        | Op::SplitGate { .. }
                        | Op::SplitBaseline { .. }
                        | Op::MergeScalar { .. }
                        | Op::MergeGate { .. }
                ),
                "PT-4: {:?} minted {:?}",
                op.tag(),
                total.id
            );
        }
    }
    let increase = |id| total_for(after, id).saturating_sub(total_for(before, id));
    match *op {
        Op::Split { amount, .. } => {
            prop_assert_eq!(
                increase(position(PID, Branch::Accept, PositionKind::BranchUsdc)),
                amount
            );
            prop_assert_eq!(
                increase(position(PID, Branch::Reject, PositionKind::BranchUsdc)),
                amount
            );
        }
        Op::SplitScalar { branch, amount, .. } => {
            prop_assert_eq!(increase(position(PID, branch, PositionKind::Long)), amount);
            prop_assert_eq!(increase(position(PID, branch, PositionKind::Short)), amount);
        }
        Op::SplitGate {
            branch,
            gate,
            amount,
            ..
        } => {
            prop_assert_eq!(
                increase(position(PID, branch, PositionKind::GateYes(gate))),
                amount
            );
            prop_assert_eq!(
                increase(position(PID, branch, PositionKind::GateNo(gate))),
                amount
            );
        }
        Op::SplitBaseline { amount, .. } => {
            prop_assert_eq!(increase(baseline(BASELINE_EPOCH, ScalarSide::Long)), amount);
            prop_assert_eq!(
                increase(baseline(BASELINE_EPOCH, ScalarSide::Short)),
                amount
            );
        }
        _ => {}
    }
    Ok(())
}

fn exercise_sequence(ops: &[Op], check_mint_paths: bool) -> TestCaseResult {
    let mut state = initial_core();
    for op in ops {
        let before = state.clone();
        let result = apply_op(&mut state, op);
        if result.is_err() {
            prop_assert_eq!(&state, &before, "illegal {:?} changed state", op.tag());
        } else {
            prop_assert_eq!(state.events.len(), before.events.len() + 1);
            assert_escrow_equation(&before, &state)?;
            if check_mint_paths {
                assert_no_undeclared_mint(&before, &state, op)?;
            }
        }
        prop_assert_eq!(state.try_state(), Ok(()), "after {:?}", op.tag());
    }
    Ok(())
}

fn proposal_escrow(state: &LedgerState<u8>, pid: u64) -> Balance {
    state
        .vaults
        .iter()
        .find(|vault| vault.proposal == pid)
        .expect("vault exists")
        .info
        .escrowed
}

fn baseline_escrow(state: &LedgerState<u8>, epoch: u32) -> Balance {
    state
        .baseline_vaults
        .iter()
        .find(|vault| vault.epoch == epoch)
        .expect("Baseline vault exists")
        .info
        .escrowed
}

fn split_fragments(total: Balance, weights: &[u16]) -> Vec<Balance> {
    let floor = kernel::MIN_TRANSFER_USDC;
    let reserved = floor * weights.len() as u128;
    let remainder = total - reserved;
    let weight_sum: u128 = weights.iter().map(|weight| u128::from(*weight) + 1).sum();
    let mut fragments = Vec::with_capacity(weights.len());
    let mut assigned = 0;
    for (index, weight) in weights.iter().enumerate() {
        let fragment = if index + 1 == weights.len() {
            total - assigned
        } else {
            floor + remainder * (u128::from(*weight) + 1) / weight_sum
        };
        assigned += fragment;
        fragments.push(fragment);
    }
    fragments
}

fn distribute(state: &mut LedgerState<u8>, id: PositionId, owner: u8, fragments: &[Balance]) {
    for (holder, amount) in HOLDERS.iter().zip(fragments) {
        state
            .transfer(LedgerOrigin::Signed, id, &owner, holder, *amount)
            .expect("minimum-sized distribution");
    }
}

fn full_redemption_band(total: Balance, remaining: Balance, holders: usize) -> TestCaseResult {
    let payout = total - remaining;
    prop_assert!(payout <= total);
    prop_assert!(
        payout >= total.saturating_sub(holders as u128),
        "PT-3 payout {payout}, E {total}, holders {holders}"
    );
    Ok(())
}

fn permutation_strategy(len: usize) -> impl Strategy<Value = Vec<usize>> {
    prop::collection::vec(any::<u64>(), len).prop_map(move |keys| {
        let mut keyed: Vec<_> = keys.into_iter().enumerate().collect();
        keyed.sort_by_key(|(index, key)| (*key, *index));
        keyed.into_iter().map(|(index, _)| index).collect()
    })
}

#[derive(Clone, Copy, Debug)]
enum SweepRaceAction {
    TransferProposal,
    RedeemProposalVoid,
    SweepProposal,
    RedeemBaselineShort,
    RedeemBaselinePair,
    SweepBaseline,
}

const SWEEP_RACE_ACTIONS: [SweepRaceAction; 6] = [
    SweepRaceAction::TransferProposal,
    SweepRaceAction::RedeemProposalVoid,
    SweepRaceAction::SweepProposal,
    SweepRaceAction::RedeemBaselineShort,
    SweepRaceAction::RedeemBaselinePair,
    SweepRaceAction::SweepBaseline,
];

#[derive(Clone, Copy, Debug)]
enum VoidAction {
    MergeBranches,
    MergeScalar,
    MergeGate,
    TransferBranch,
    RedeemAcceptAtPairHolder,
    RedeemRejectAtPairHolder,
    RedeemLong,
    RedeemShort,
    RedeemGateYes,
    RedeemGateNo,
    RedeemAcceptAtTransferSource,
}

const VOID_ACTIONS: [VoidAction; 11] = [
    VoidAction::MergeBranches,
    VoidAction::MergeScalar,
    VoidAction::MergeGate,
    VoidAction::TransferBranch,
    VoidAction::RedeemAcceptAtPairHolder,
    VoidAction::RedeemRejectAtPairHolder,
    VoidAction::RedeemLong,
    VoidAction::RedeemShort,
    VoidAction::RedeemGateYes,
    VoidAction::RedeemGateNo,
    VoidAction::RedeemAcceptAtTransferSource,
];

fn apply_void_action(
    state: &mut LedgerState<u8>,
    action: VoidAction,
    tranche: Balance,
    gate: GateType,
) -> Result<(), CoreError> {
    match action {
        VoidAction::MergeBranches => state.merge(LedgerOrigin::Signed, 62, &10, tranche),
        VoidAction::MergeScalar => {
            state.merge_scalar(LedgerOrigin::Signed, 62, Branch::Accept, &11, tranche)
        }
        VoidAction::MergeGate => {
            state.merge_gate(LedgerOrigin::Signed, 62, Branch::Accept, gate, &12, tranche)
        }
        VoidAction::TransferBranch => state.transfer(
            LedgerOrigin::Signed,
            position(62, Branch::Accept, PositionKind::BranchUsdc),
            &13,
            &10,
            tranche,
        ),
        VoidAction::RedeemAcceptAtPairHolder => {
            state.redeem_void(62, Branch::Accept, PositionKind::BranchUsdc, &10, tranche)
        }
        VoidAction::RedeemRejectAtPairHolder => {
            state.redeem_void(62, Branch::Reject, PositionKind::BranchUsdc, &10, tranche)
        }
        VoidAction::RedeemLong => {
            state.redeem_void(62, Branch::Accept, PositionKind::Long, &11, tranche)
        }
        VoidAction::RedeemShort => {
            state.redeem_void(62, Branch::Accept, PositionKind::Short, &11, tranche)
        }
        VoidAction::RedeemGateYes => state.redeem_void(
            62,
            Branch::Accept,
            PositionKind::GateYes(gate),
            &12,
            tranche,
        ),
        VoidAction::RedeemGateNo => {
            state.redeem_void(62, Branch::Accept, PositionKind::GateNo(gate), &12, tranche)
        }
        VoidAction::RedeemAcceptAtTransferSource => {
            state.redeem_void(62, Branch::Accept, PositionKind::BranchUsdc, &13, tranche)
        }
    }
}

fn run_void_permutation(
    initial: &LedgerState<u8>,
    order: &[usize],
    tranche: Balance,
    gate: GateType,
) -> Result<Balance, TestCaseError> {
    let mut state = initial.clone();
    for index in order {
        let before = state.clone();
        if apply_void_action(&mut state, VOID_ACTIONS[*index], tranche, gate).is_err() {
            prop_assert_eq!(&state, &before, "failed VOID action mutated state");
        }
        prop_assert_eq!(state.try_state(), Ok(()));
    }

    // Complete the recovery using unpaired D-1 redemptions. Tranches are
    // multiples of four, so this canonical tail has no rounding ambiguity.
    let claims: Vec<_> = state
        .positions
        .iter()
        .filter_map(|record| match record.id {
            PositionId::Proposal {
                proposal: 62,
                branch,
                kind,
            } => Some((record.owner, branch, kind, record.balance)),
            _ => None,
        })
        .collect();
    for (owner, branch, kind, balance) in claims {
        state
            .redeem_void(62, branch, kind, &owner, balance)
            .expect("snapshot claim remains redeemable");
    }
    prop_assert!(
        state
            .positions
            .iter()
            .all(|record| !matches!(record.id, PositionId::Proposal { proposal: 62, .. })),
        "PT-6 recovery left a live proposal claim"
    );
    prop_assert_eq!(state.try_state(), Ok(()));
    Ok(proposal_escrow(&state, 62))
}

proptest! {
    #![proptest_config(property_config(0x1ed6_0001))]

    /// PT-1 + 15 §4.3 state-machine legality: L-1…L-4 and the 03 §6 equations
    /// hold after every legal op; every illegal op is a strict no-op.
    #[test]
    fn pt1_conservation_and_illegal_ops_are_atomic(
        ops in prop::collection::vec(op_strategy(), 1..32)
    ) {
        exercise_sequence(&ops, false)?;
    }

    /// PT-2: the three D-1/D-3 holder profiles use the exact half/quarter/par
    /// schedule, including the real 04 §6.1 buy-wrapper portfolio.
    #[test]
    fn pt2_annulment_profiles_follow_d1_exactly(
        amount in (4 * kernel::MIN_SPLIT_USDC)..=2_000_000u128,
        wrapper_buys in wrapper_buys_strategy(),
    ) {
        // (a) A complete Accept+Reject holder recovers par.
        let mut paired = LedgerState::new();
        paired.create_vault(1, 0).unwrap();
        paired.split(LedgerOrigin::Signed, 1, &0, amount).unwrap();
        paired.void(LedgerOrigin::ResolveAuthority, 1).unwrap();
        let before = proposal_escrow(&paired, 1);
        paired.merge(LedgerOrigin::Signed, 1, &0, amount).unwrap();
        prop_assert_eq!(before - proposal_escrow(&paired, 1), amount);

        // (b) Drive several buys on both sides of both branch books through
        // the exact plain-ledger accounting of 04 §6.1 / market-core's
        // buy_branch.  No post-VOID transfer supplies a free complement.
        const BUYER: u8 = 0;
        const TREASURY: u8 = 1;
        const FEES: u8 = 7;
        let mut wrapper = LedgerState::new();
        wrapper.create_vault(2, 0).unwrap();
        wrapper.add_protocol_account(wrapper_book(Branch::Accept));
        wrapper.add_protocol_account(wrapper_book(Branch::Reject));
        wrapper.add_protocol_account(FEES);
        let inventory = wrapper_buys.iter().map(|buy| buy.amount).sum();
        seed_wrapper_books(&mut wrapper, 2, TREASURY, inventory);
        for buy in &wrapper_buys {
            prop_assert!(buy.cost < buy.amount, "LMSR buy cost must be below amount");
            apply_wrapper_buy(&mut wrapper, 2, BUYER, FEES, *buy);
        }
        prop_assert_eq!(wrapper.try_state(), Ok(()));

        wrapper.void(LedgerOrigin::ResolveAuthority, 2).unwrap();
        let recovery = recover_wrapper_buyer(&mut wrapper, 2, BUYER)
            .expect("the buyer's own VOID claims have a terminating recovery path");
        let expected_recovery = expected_wrapper_void_recovery(&wrapper_buys);
        prop_assert_eq!(recovery, expected_recovery);
        prop_assert_eq!(wrapper.try_state(), Ok(()));

        let costs: Balance = wrapper_buys.iter().map(|buy| buy.cost).sum();
        let fees: Balance = wrapper_buys.iter().map(|buy| buy.fee()).sum();
        let debited = costs + fees;
        let neutral_premium_delta = expected_recovery as i128 - costs as i128;
        prop_assert_eq!(
            recovery as i128 - debited as i128,
            neutral_premium_delta - fees as i128,
            "04 §6.2/03 §6.4 guarantee neutral-prior recovery, net of the chosen market premium and fees",
        );

        // (c) Deliberately unpaired branch claims pay half; scalar/gate legs
        // pay quarter, all with claimant-adverse floors.
        let mut unpaired = LedgerState::new();
        unpaired.create_vault(3, 0).unwrap();
        unpaired.split(LedgerOrigin::Signed, 3, &0, amount).unwrap();
        unpaired.split_scalar(LedgerOrigin::Signed, 3, Branch::Accept, &0, amount).unwrap();
        unpaired.transfer(
            LedgerOrigin::Signed,
            position(3, Branch::Accept, PositionKind::Short),
            &0,
            &1,
            amount,
        ).unwrap();
        unpaired.void(LedgerOrigin::ResolveAuthority, 3).unwrap();
        let mut last = proposal_escrow(&unpaired, 3);
        unpaired.redeem_void(3, Branch::Reject, PositionKind::BranchUsdc, &0, amount).unwrap();
        prop_assert_eq!(last - proposal_escrow(&unpaired, 3), amount / 2);
        last = proposal_escrow(&unpaired, 3);
        unpaired.redeem_void(3, Branch::Accept, PositionKind::Long, &0, amount).unwrap();
        prop_assert_eq!(last - proposal_escrow(&unpaired, 3), amount / 4);
        last = proposal_escrow(&unpaired, 3);
        unpaired.redeem_void(3, Branch::Accept, PositionKind::Short, &1, amount).unwrap();
        prop_assert_eq!(last - proposal_escrow(&unpaired, 3), amount / 4);
        prop_assert_eq!(unpaired.try_state(), Ok(()));

        let mut gate_unpaired = LedgerState::new();
        gate_unpaired.create_vault(4, 0).unwrap();
        gate_unpaired.split(LedgerOrigin::Signed, 4, &0, amount).unwrap();
        gate_unpaired.split_gate(
            LedgerOrigin::Signed, 4, Branch::Accept, GateType::Security, &0, amount
        ).unwrap();
        gate_unpaired.void(LedgerOrigin::ResolveAuthority, 4).unwrap();
        let mut last = proposal_escrow(&gate_unpaired, 4);
        gate_unpaired.redeem_void(
            4, Branch::Accept, PositionKind::GateYes(GateType::Security), &0, amount
        ).unwrap();
        prop_assert_eq!(last - proposal_escrow(&gate_unpaired, 4), amount / 4);
        last = proposal_escrow(&gate_unpaired, 4);
        gate_unpaired.redeem_void(
            4, Branch::Accept, PositionKind::GateNo(GateType::Security), &0, amount
        ).unwrap();
        prop_assert_eq!(last - proposal_escrow(&gate_unpaired, 4), amount / 4);
    }

    /// PT-3: a mixed proposal portfolio puts branch, scalar, and gate claims
    /// on the same holders while fragmenting every instrument independently.
    /// Pair-first redemption leaves at most one rounded scalar claim per
    /// holder, preserving the exact [E-holders,E] bound (15 §4.3).
    #[test]
    fn pt3_mixed_portfolios_round_against_claimants(
        score in score_strategy(),
        branch_extra in 0u128..=300_000u128,
        scalar_extra in 0u128..=300_000u128,
        gate_extra in 0u128..=300_000u128,
        branch_weights in prop::array::uniform4(0u16..1000),
        long_weights in prop::array::uniform4(0u16..1000),
        short_weights in prop::array::uniform4(0u16..1000),
        gate_yes_weights in prop::array::uniform4(0u16..1000),
        gate_no_weights in prop::array::uniform4(0u16..1000),
        baseline_long_weights in prop::array::uniform4(0u16..1000),
        baseline_short_weights in prop::array::uniform4(0u16..1000),
        void_accept_weights in prop::array::uniform4(0u16..1000),
        void_reject_weights in prop::array::uniform4(0u16..1000),
        gate in gate_strategy(),
        outcome in any::<bool>(),
    ) {
        let holder_floor = kernel::MIN_TRANSFER_USDC * HOLDERS.len() as u128;
        let branch_total = holder_floor + branch_extra;
        let scalar_total = holder_floor + scalar_extra;
        let gate_total = holder_floor + gate_extra;
        let total = branch_total + scalar_total + gate_total;
        let branch_fragments = split_fragments(branch_total, &branch_weights);
        let long_fragments = split_fragments(scalar_total, &long_weights);
        let short_fragments = split_fragments(scalar_total, &short_weights);
        let gate_yes_fragments = split_fragments(gate_total, &gate_yes_weights);
        let gate_no_fragments = split_fragments(gate_total, &gate_no_weights);

        let mut scalar = LedgerState::new();
        scalar.create_vault(10, 0).unwrap();
        scalar.split(LedgerOrigin::Signed, 10, &0, total).unwrap();
        scalar.split_scalar(
            LedgerOrigin::Signed, 10, Branch::Accept, &0, scalar_total
        ).unwrap();
        scalar.split_gate(
            LedgerOrigin::Signed, 10, Branch::Accept, gate, &0, gate_total
        ).unwrap();
        distribute(
            &mut scalar,
            position(10, Branch::Accept, PositionKind::BranchUsdc),
            0,
            &branch_fragments,
        );
        distribute(
            &mut scalar,
            position(10, Branch::Accept, PositionKind::Long),
            0,
            &long_fragments,
        );
        distribute(
            &mut scalar,
            position(10, Branch::Accept, PositionKind::Short),
            0,
            &short_fragments,
        );
        distribute(
            &mut scalar,
            position(10, Branch::Accept, PositionKind::GateYes(gate)),
            0,
            &gate_yes_fragments,
        );
        distribute(
            &mut scalar,
            position(10, Branch::Accept, PositionKind::GateNo(gate)),
            0,
            &gate_no_fragments,
        );
        scalar.resolve(LedgerOrigin::ResolveAuthority, 10, Branch::Accept).unwrap();
        scalar.settle_gate(LedgerOrigin::SettleAuthority, 10, gate, outcome).unwrap();
        scalar.settle_scalar(LedgerOrigin::SettleAuthority, 10, FixedU64(score)).unwrap();
        let winning_gate = if outcome {
            PositionKind::GateYes(gate)
        } else {
            PositionKind::GateNo(gate)
        };
        for (holder, branch_amount) in HOLDERS.iter().zip(&branch_fragments) {
            scalar.redeem(10, holder, *branch_amount).unwrap();
            let holder_long = core_balance(
                &scalar,
                position(10, Branch::Accept, PositionKind::Long),
                *holder,
            );
            let holder_short = core_balance(
                &scalar,
                position(10, Branch::Accept, PositionKind::Short),
                *holder,
            );
            let paired = holder_long.min(holder_short);
            if paired > 0 {
                scalar.redeem_scalar_pair(10, holder, paired).unwrap();
            }
            let long_left = holder_long - paired;
            let short_left = holder_short - paired;
            if long_left > 0 {
                scalar.redeem_scalar(10, ScalarSide::Long, holder, long_left).unwrap();
            }
            if short_left > 0 {
                scalar.redeem_scalar(10, ScalarSide::Short, holder, short_left).unwrap();
            }
            let gate_amount = core_balance(
                &scalar,
                position(10, Branch::Accept, winning_gate),
                *holder,
            );
            scalar.redeem_gate(10, gate, holder, gate_amount).unwrap();
        }
        full_redemption_band(total, proposal_escrow(&scalar, 10), HOLDERS.len())?;

        let mut base = LedgerState::new();
        base.create_baseline_vault(10).unwrap();
        base.split_baseline(LedgerOrigin::Signed, 10, &0, scalar_total).unwrap();
        let base_long = split_fragments(scalar_total, &baseline_long_weights);
        let base_short = split_fragments(scalar_total, &baseline_short_weights);
        distribute(&mut base, baseline(10, ScalarSide::Long), 0, &base_long);
        distribute(&mut base, baseline(10, ScalarSide::Short), 0, &base_short);
        base.settle_baseline(LedgerOrigin::SettleAuthority, 10, FixedU64(score)).unwrap();
        for holder in HOLDERS {
            let long = core_balance(&base, baseline(10, ScalarSide::Long), holder);
            let short = core_balance(&base, baseline(10, ScalarSide::Short), holder);
            let paired = long.min(short);
            if paired > 0 {
                base.redeem_baseline_pair(10, &holder, paired).unwrap();
            }
            if long > paired {
                base.redeem_baseline(10, ScalarSide::Long, &holder, long - paired).unwrap();
            }
            if short > paired {
                base.redeem_baseline(10, ScalarSide::Short, &holder, short - paired).unwrap();
            }
        }
        full_redemption_band(
            scalar_total,
            baseline_escrow(&base, 10),
            HOLDERS.len(),
        )?;

        let mut voided = LedgerState::new();
        voided.create_vault(11, 0).unwrap();
        voided.split(LedgerOrigin::Signed, 11, &0, branch_total).unwrap();
        let void_accept = split_fragments(branch_total, &void_accept_weights);
        let void_reject = split_fragments(branch_total, &void_reject_weights);
        distribute(
            &mut voided,
            position(11, Branch::Accept, PositionKind::BranchUsdc),
            0,
            &void_accept,
        );
        distribute(
            &mut voided,
            position(11, Branch::Reject, PositionKind::BranchUsdc),
            0,
            &void_reject,
        );
        voided.void(LedgerOrigin::ResolveAuthority, 11).unwrap();
        for holder in HOLDERS {
            let accept = core_balance(
                &voided,
                position(11, Branch::Accept, PositionKind::BranchUsdc),
                holder,
            );
            let reject = core_balance(
                &voided,
                position(11, Branch::Reject, PositionKind::BranchUsdc),
                holder,
            );
            let paired = accept.min(reject);
            if paired > 0 {
                voided.merge(LedgerOrigin::Signed, 11, &holder, paired).unwrap();
            }
            if accept > paired {
                voided.redeem_void(
                    11,
                    Branch::Accept,
                    PositionKind::BranchUsdc,
                    &holder,
                    accept - paired,
                ).unwrap();
            }
            if reject > paired {
                voided.redeem_void(
                    11,
                    Branch::Reject,
                    PositionKind::BranchUsdc,
                    &holder,
                    reject - paired,
                ).unwrap();
            }
        }
        full_redemption_band(
            branch_total,
            proposal_escrow(&voided, 11),
            HOLDERS.len(),
        )?;
    }

    /// PT-4: total supply can increase only in split families.  All other
    /// generated operations can only preserve or burn supply.
    #[test]
    fn pt4_no_mint_outside_declared_split_families(
        ops in prop::collection::vec(op_strategy(), 1..32)
    ) {
        exercise_sequence(&ops, true)?;
    }

    /// PT-5: the pallet refuses early reap, then refunds all deposits and
    /// sweeps exactly the archived terminal escrow after the delay.
    #[test]
    fn pt5_reap_safety_and_deposit_refunds(
        amount in kernel::MIN_SPLIT_USDC..=2_000_000u128,
        score in score_strategy(),
        forfeit in any::<bool>(),
    ) {
        new_test_ext().execute_with(|| -> TestCaseResult {
            PalletMinSplitGuard::reset();
            prop_assert!(Ledger::create_vault(RuntimeOrigin::signed(MARKET), 50, 0).is_ok());
            prop_assert!(Ledger::split(RuntimeOrigin::signed(ALICE), 50, amount).is_ok());
            prop_assert!(Ledger::split_scalar(
                RuntimeOrigin::signed(ALICE), 50, Branch::Accept, amount
            ).is_ok());
            prop_assert!(Ledger::resolve(RuntimeOrigin::signed(RESOLVER), 50, Branch::Accept).is_ok());
            prop_assert!(Ledger::settle_scalar(
                RuntimeOrigin::signed(SETTLER), 50, FixedU64(score)
            ).is_ok());

            let early = (
                Vaults::<Test>::get(50),
                Positions::<Test>::iter().collect::<Vec<_>>(),
                PositionTotals::<Test>::iter().collect::<Vec<_>>(),
                PositionCount::<Test>::iter().collect::<Vec<_>>(),
                DepositsHeld::<Test>::get(),
            );
            prop_assert_eq!(
                Ledger::sweep_dust(RuntimeOrigin::signed(ALICE), 50),
                Err(PalletError::<Test>::ReapNotDue.into())
            );
            prop_assert_eq!(Vaults::<Test>::get(50), early.0);
            prop_assert_eq!(Positions::<Test>::iter().collect::<Vec<_>>(), early.1);
            prop_assert_eq!(PositionTotals::<Test>::iter().collect::<Vec<_>>(), early.2);
            prop_assert_eq!(PositionCount::<Test>::iter().collect::<Vec<_>>(), early.3);
            prop_assert_eq!(DepositsHeld::<Test>::get(), early.4);

            let escrow = Vaults::<Test>::get(50).unwrap().escrowed;
            prop_assert_eq!(
                Positions::<Test>::get(
                    position(50, Branch::Accept, PositionKind::Long), ALICE
                ).min(Positions::<Test>::get(
                    position(50, Branch::Accept, PositionKind::Short), ALICE
                )),
                escrow,
                "PT-5 archived complete-set claims must value to the swept escrow"
            );
            let forfeited_deposits = if forfeit {
                // R-7: model an owner account that disappeared while its
                // archived position entries remained; its deposits route to
                // INSURANCE rather than resurrecting the owner.
                frame_system::Account::<Test>::remove(ALICE);
                early.4
            } else {
                0
            };
            let insurance_before = <Assets as Inspect<AccountId>>::balance(USDC, &INSURANCE);
            System::set_block_number(1 + ArchiveDelay::get() + 1);
            prop_assert!(Ledger::sweep_dust(RuntimeOrigin::signed(ALICE), 50).is_ok());
            prop_assert!(!Vaults::<Test>::contains_key(50));
            prop_assert_eq!(DepositsHeld::<Test>::get(), 0);
            prop_assert_eq!(PositionCount::<Test>::get(ALICE), 0);
            let insurance_after = <Assets as Inspect<AccountId>>::balance(USDC, &INSURANCE);
            prop_assert_eq!(
                insurance_after - insurance_before,
                escrow + forfeited_deposits
            );
            prop_assert!(Ledger::do_try_state().is_ok());
            Ok(())
        })?;
    }

    /// PT-5b / 03 §12: the real pallet sweep cranks race late transfers and
    /// redemptions at the archive boundary. Whichever dispatch wins first,
    /// both proposal and Baseline paths remain solvent and eventually reap.
    #[test]
    fn pt5b_mock_sweeps_interleave_with_late_claims(
        amount in (4 * kernel::MIN_SPLIT_USDC)..=2_000_000u128,
        order in permutation_strategy(SWEEP_RACE_ACTIONS.len()),
        proposal_sweep in generator_arm(OpTag::SweepDust),
        baseline_sweep in generator_arm(OpTag::SweepDustBaseline),
    ) {
        new_test_ext().execute_with(|| -> TestCaseResult {
            const RACE_PID: u64 = 51;
            const RACE_EPOCH: u32 = 51;
            PalletMinSplitGuard::reset();
            ReapBatch::set(1);
            let chunk = amount / 4;

            prop_assert!(Ledger::create_vault(RuntimeOrigin::signed(MARKET), RACE_PID, 0).is_ok());
            prop_assert!(Ledger::split(RuntimeOrigin::signed(ALICE), RACE_PID, amount).is_ok());
            prop_assert!(Ledger::split_scalar(
                RuntimeOrigin::signed(ALICE), RACE_PID, Branch::Accept, amount
            ).is_ok());
            prop_assert!(Ledger::void(RuntimeOrigin::signed(RESOLVER), RACE_PID).is_ok());

            prop_assert!(Ledger::create_baseline_vault(
                RuntimeOrigin::signed(MARKET),
                RACE_EPOCH
            ).is_ok());
            prop_assert!(Ledger::split_baseline(
                RuntimeOrigin::signed(ALICE), RACE_EPOCH, amount
            ).is_ok());
            prop_assert!(Ledger::transfer(
                RuntimeOrigin::signed(ALICE),
                baseline(RACE_EPOCH, ScalarSide::Long),
                BOB,
                chunk,
            ).is_ok());
            prop_assert!(Ledger::settle_baseline(
                RuntimeOrigin::signed(SETTLER), RACE_EPOCH, FixedU64(500_000_000)
            ).is_ok());

            // Live claims continue to move and redeem before ArchiveDelay.
            prop_assert!(Ledger::transfer(
                RuntimeOrigin::signed(ALICE),
                position(RACE_PID, Branch::Accept, PositionKind::Long),
                BOB,
                chunk,
            ).is_ok());
            prop_assert!(Ledger::redeem_void(
                RuntimeOrigin::signed(ALICE),
                RACE_PID,
                Branch::Accept,
                PositionKind::Long,
                chunk,
            ).is_ok());
            prop_assert!(Ledger::redeem_baseline(
                RuntimeOrigin::signed(BOB),
                RACE_EPOCH,
                ScalarSide::Long,
                chunk,
            ).is_ok());
            prop_assert!(Ledger::redeem_baseline_pair(
                RuntimeOrigin::signed(ALICE), RACE_EPOCH, chunk
            ).is_ok());

            let terminal = System::block_number();
            System::set_block_number(terminal + ArchiveDelay::get() - 1);
            prop_assert_eq!(
                Ledger::sweep_dust(RuntimeOrigin::signed(ALICE), RACE_PID),
                Err(PalletError::<Test>::ReapNotDue.into())
            );
            prop_assert_eq!(
                Ledger::sweep_dust_baseline(RuntimeOrigin::signed(ALICE), RACE_EPOCH),
                Err(PalletError::<Test>::ReapNotDue.into())
            );
            System::set_block_number(terminal + ArchiveDelay::get());

            let pair_remainder = amount - 2 * chunk;
            for index in order {
                let result = match SWEEP_RACE_ACTIONS[index] {
                    SweepRaceAction::TransferProposal => Ledger::transfer(
                        RuntimeOrigin::signed(ALICE),
                        position(RACE_PID, Branch::Accept, PositionKind::Short),
                        CHARLIE,
                        chunk,
                    ),
                    SweepRaceAction::RedeemProposalVoid => Ledger::redeem_void(
                        RuntimeOrigin::signed(ALICE),
                        RACE_PID,
                        Branch::Accept,
                        PositionKind::Long,
                        pair_remainder,
                    ),
                    SweepRaceAction::SweepProposal => match &proposal_sweep {
                        Op::SweepDust => Ledger::sweep_dust(
                            RuntimeOrigin::signed(BOB),
                            RACE_PID,
                        ),
                        _ => unreachable!("SweepDust generator returned the wrong arm"),
                    },
                    SweepRaceAction::RedeemBaselineShort => Ledger::redeem_baseline(
                        RuntimeOrigin::signed(ALICE),
                        RACE_EPOCH,
                        ScalarSide::Short,
                        chunk,
                    ),
                    SweepRaceAction::RedeemBaselinePair => Ledger::redeem_baseline_pair(
                        RuntimeOrigin::signed(ALICE),
                        RACE_EPOCH,
                        pair_remainder,
                    ),
                    SweepRaceAction::SweepBaseline => match &baseline_sweep {
                        Op::SweepDustBaseline => Ledger::sweep_dust_baseline(
                            RuntimeOrigin::signed(BOB),
                            RACE_EPOCH,
                        ),
                        _ => unreachable!("SweepDustBaseline generator returned the wrong arm"),
                    },
                };
                // A late claimant may lose the race to archival reaping, but
                // either success or a status-quo error must preserve try-state.
                let _status_quo_or_progress = result;
                prop_assert!(Ledger::do_try_state().is_ok());
            }

            // Drain every remaining bounded batch and prove that both distinct
            // generator arms actually reach their pallet calls.
            for _ in 0..20 {
                if Vaults::<Test>::contains_key(RACE_PID) {
                    prop_assert!(Ledger::sweep_dust(
                        RuntimeOrigin::signed(BOB), RACE_PID
                    ).is_ok());
                }
                if crate::BaselineVaults::<Test>::contains_key(RACE_EPOCH) {
                    prop_assert!(Ledger::sweep_dust_baseline(
                        RuntimeOrigin::signed(BOB), RACE_EPOCH
                    ).is_ok());
                }
            }
            prop_assert!(!Vaults::<Test>::contains_key(RACE_PID));
            prop_assert!(!crate::BaselineVaults::<Test>::contains_key(RACE_EPOCH));
            prop_assert!(Ledger::do_try_state().is_ok());
            Ok(())
        })?;
    }

    /// PT-6: void is reachable from both eligible non-terminal states, barred
    /// after ScalarSettled, and random permutations of the complete I-27
    /// surface (merge families, transfer, redeem_void) have the same total.
    #[test]
    fn pt6_void_reachability_recovery_and_order_independence(
        tranche_raw in kernel::MIN_SPLIT_USDC..=500_000u128,
        gate in gate_strategy(),
        order in permutation_strategy(VOID_ACTIONS.len()),
    ) {
        let tranche = tranche_raw - tranche_raw % 4;
        let amount = 4 * tranche;
        let mut open = LedgerState::new();
        open.create_vault(60, 0).unwrap();
        open.split(LedgerOrigin::Signed, 60, &0, amount).unwrap();
        let mut resolved = open.clone();
        resolved.resolve(LedgerOrigin::ResolveAuthority, 60, Branch::Accept).unwrap();
        prop_assert_eq!(open.void(LedgerOrigin::ResolveAuthority, 60), Ok(()));
        prop_assert_eq!(resolved.void(LedgerOrigin::ResolveAuthority, 60), Ok(()));

        let mut settled = LedgerState::new();
        settled.create_vault(61, 0).unwrap();
        settled.split(LedgerOrigin::Signed, 61, &0, amount).unwrap();
        settled.resolve(LedgerOrigin::ResolveAuthority, 61, Branch::Accept).unwrap();
        settled.settle_scalar(LedgerOrigin::SettleAuthority, 61, FixedU64(500_000_000)).unwrap();
        let snapshot = settled.clone();
        prop_assert_eq!(
            settled.void(LedgerOrigin::ResolveAuthority, 61),
            Err(CoreError::WrongVaultState)
        );
        prop_assert_eq!(settled, snapshot);

        let mut recover = LedgerState::new();
        recover.create_vault(62, 0).unwrap();
        recover.split(LedgerOrigin::Signed, 62, &0, amount).unwrap();
        // Mixed holder inventory: branch pair, scalar pair, gate pair, and an
        // independently transferable branch claim all coexist.
        for branch in [Branch::Accept, Branch::Reject] {
            recover.transfer(
                LedgerOrigin::Signed,
                position(62, branch, PositionKind::BranchUsdc),
                &0,
                &10,
                tranche,
            ).unwrap();
        }
        recover.split_scalar(
            LedgerOrigin::Signed, 62, Branch::Accept, &0, tranche
        ).unwrap();
        for kind in [PositionKind::Long, PositionKind::Short] {
            recover.transfer(
                LedgerOrigin::Signed,
                position(62, Branch::Accept, kind),
                &0,
                &11,
                tranche,
            ).unwrap();
        }
        recover.split_gate(
            LedgerOrigin::Signed, 62, Branch::Accept, gate, &0, tranche
        ).unwrap();
        for kind in [PositionKind::GateYes(gate), PositionKind::GateNo(gate)] {
            recover.transfer(
                LedgerOrigin::Signed,
                position(62, Branch::Accept, kind),
                &0,
                &12,
                tranche,
            ).unwrap();
        }
        recover.transfer(
            LedgerOrigin::Signed,
            position(62, Branch::Accept, PositionKind::BranchUsdc),
            &0,
            &13,
            tranche,
        ).unwrap();
        recover.void(LedgerOrigin::ResolveAuthority, 62).unwrap();

        let canonical: Vec<_> = (0..VOID_ACTIONS.len()).collect();
        let canonical_remaining = run_void_permutation(&recover, &canonical, tranche, gate)?;
        let permuted_remaining = run_void_permutation(&recover, &order, tranche, gate)?;
        let canonical_payout = amount - canonical_remaining;
        let permuted_payout = amount - permuted_remaining;
        prop_assert!(canonical_payout <= amount);
        prop_assert!(permuted_payout <= amount);
        prop_assert_eq!(permuted_payout, canonical_payout, "first-redeemer advantage");
        prop_assert_eq!(canonical_payout, amount, "multiple-of-four portfolio is exact");
    }

    /// PT-7: atomic scalar/Baseline pair redemption is exactly `a`; separate
    /// legs never exceed it.  A settled gate pair likewise pays exactly `a`
    /// through its one winning side.
    #[test]
    fn pt7_pair_exactness_and_leg_flooring(
        amount in kernel::MIN_SPLIT_USDC..=2_000_000u128,
        score in score_strategy(),
        gate in gate_strategy(),
        outcome in any::<bool>(),
    ) {
        let mut scalar = LedgerState::new();
        scalar.create_vault(70, 0).unwrap();
        scalar.split(LedgerOrigin::Signed, 70, &0, amount).unwrap();
        scalar.split_scalar(LedgerOrigin::Signed, 70, Branch::Accept, &0, amount).unwrap();
        scalar.resolve(LedgerOrigin::ResolveAuthority, 70, Branch::Accept).unwrap();
        scalar.settle_scalar(LedgerOrigin::SettleAuthority, 70, FixedU64(score)).unwrap();
        let mut atomic = scalar.clone();
        let before = proposal_escrow(&atomic, 70);
        atomic.redeem_scalar_pair(70, &0, amount).unwrap();
        prop_assert_eq!(before - proposal_escrow(&atomic, 70), amount);
        let mut legs = scalar;
        let before = proposal_escrow(&legs, 70);
        legs.redeem_scalar(70, ScalarSide::Long, &0, amount).unwrap();
        legs.redeem_scalar(70, ScalarSide::Short, &0, amount).unwrap();
        prop_assert!(before - proposal_escrow(&legs, 70) <= amount);

        let mut base = LedgerState::new();
        base.create_baseline_vault(70).unwrap();
        base.split_baseline(LedgerOrigin::Signed, 70, &0, amount).unwrap();
        base.settle_baseline(LedgerOrigin::SettleAuthority, 70, FixedU64(score)).unwrap();
        let mut atomic = base.clone();
        let before = baseline_escrow(&atomic, 70);
        atomic.redeem_baseline_pair(70, &0, amount).unwrap();
        prop_assert_eq!(before - baseline_escrow(&atomic, 70), amount);
        let mut legs = base;
        let before = baseline_escrow(&legs, 70);
        legs.redeem_baseline(70, ScalarSide::Long, &0, amount).unwrap();
        legs.redeem_baseline(70, ScalarSide::Short, &0, amount).unwrap();
        prop_assert!(before - baseline_escrow(&legs, 70) <= amount);

        let mut gated = LedgerState::new();
        gated.create_vault(71, 0).unwrap();
        gated.split(LedgerOrigin::Signed, 71, &0, amount).unwrap();
        gated.split_gate(LedgerOrigin::Signed, 71, Branch::Accept, gate, &0, amount).unwrap();
        let losing_kind = if outcome {
            PositionKind::GateNo(gate)
        } else {
            PositionKind::GateYes(gate)
        };
        gated.transfer(
            LedgerOrigin::Signed,
            position(71, Branch::Accept, losing_kind),
            &0,
            &1,
            amount,
        ).unwrap();
        gated.resolve(LedgerOrigin::ResolveAuthority, 71, Branch::Accept).unwrap();
        gated.settle_scalar(LedgerOrigin::SettleAuthority, 71, FixedU64(score)).unwrap();
        let before_gate_outcome = gated.clone();
        prop_assert_eq!(
            gated.redeem_gate(71, gate, &0, amount),
            Err(CoreError::GateNotSettled)
        );
        prop_assert_eq!(&gated, &before_gate_outcome);
        gated.settle_gate(LedgerOrigin::SettleAuthority, 71, gate, outcome).unwrap();

        // 03 §5.3: the losing side is zero-valued and reap-only. The public
        // redemption call selects the winning side, so a losing-only holder
        // must fail atomically rather than receiving any payout.
        let before_loser = gated.clone();
        let escrow_before_loser = proposal_escrow(&gated, 71);
        prop_assert_eq!(
            gated.redeem_gate(71, gate, &1, amount),
            Err(CoreError::InsufficientPosition)
        );
        prop_assert_eq!(&gated, &before_loser);
        prop_assert_eq!(proposal_escrow(&gated, 71), escrow_before_loser);
        prop_assert_eq!(
            core_balance(&gated, position(71, Branch::Accept, losing_kind), 1),
            amount
        );
        let before = proposal_escrow(&gated, 71);
        gated.redeem_gate(71, gate, &0, amount).unwrap();
        prop_assert_eq!(before - proposal_escrow(&gated, 71), amount);
        prop_assert_eq!(
            core_balance(&gated, position(71, Branch::Accept, losing_kind), 1),
            amount,
            "losing gate side must pay exactly zero and remain reap-only"
        );
    }

    /// PT-8 (prefix/churn half): reaping one vault leaves every other prefix
    /// byte-for-byte unchanged and PositionCount remains exact through churn.
    #[test]
    fn pt8_prefix_isolation_and_position_count_churn(
        moved in kernel::MIN_TRANSFER_USDC..=500_000u128,
    ) {
        new_test_ext().execute_with(|| -> TestCaseResult {
            let total = 1_000_000u128;
            prop_assert!(Ledger::create_vault(RuntimeOrigin::signed(MARKET), 80, 0).is_ok());
            prop_assert!(Ledger::create_vault(RuntimeOrigin::signed(MARKET), 81, 0).is_ok());
            prop_assert!(Ledger::split(RuntimeOrigin::signed(ALICE), 80, total).is_ok());
            prop_assert!(Ledger::split(RuntimeOrigin::signed(ALICE), 81, total).is_ok());
            let id = position(80, Branch::Accept, PositionKind::BranchUsdc);
            prop_assert!(Ledger::transfer(RuntimeOrigin::signed(ALICE), id, BOB, moved).is_ok());
            prop_assert!(Ledger::transfer(
                RuntimeOrigin::signed(BOB), id, ALICE, kernel::MIN_TRANSFER_USDC
            ).is_ok());
            assert_position_counts_exact()?;
            prop_assert!(Ledger::void(RuntimeOrigin::signed(RESOLVER), 80).is_ok());
            prop_assert!(Ledger::void(RuntimeOrigin::signed(RESOLVER), 81).is_ok());

            let untouched_positions: Vec<_> = Positions::<Test>::iter()
                .filter(|(id, _, _)| matches!(id, PositionId::Proposal { proposal: 81, .. }))
                .collect();
            let untouched_totals: Vec<_> = PositionTotals::<Test>::iter()
                .filter(|(id, _)| matches!(id, PositionId::Proposal { proposal: 81, .. }))
                .collect();
            System::set_block_number(1 + ArchiveDelay::get() + 1);
            prop_assert!(Ledger::sweep_dust(RuntimeOrigin::signed(ALICE), 80).is_ok());
            prop_assert!(!Vaults::<Test>::contains_key(80));
            prop_assert!(Vaults::<Test>::contains_key(81));
            prop_assert!(Positions::<Test>::iter().all(|(id, _, _)| !matches!(
                id, PositionId::Proposal { proposal: 80, .. }
            )), "PT-8 reap left an entry in the target vault prefix");
            prop_assert_eq!(
                Positions::<Test>::iter()
                    .filter(|(id, _, _)| matches!(id, PositionId::Proposal { proposal: 81, .. }))
                    .collect::<Vec<_>>(),
                untouched_positions
            );
            prop_assert_eq!(
                PositionTotals::<Test>::iter()
                    .filter(|(id, _)| matches!(id, PositionId::Proposal { proposal: 81, .. }))
                    .collect::<Vec<_>>(),
                untouched_totals
            );
            assert_position_counts_exact()?;
            prop_assert!(Ledger::do_try_state().is_ok());
            Ok(())
        })?;
    }
}

fn assert_position_counts_exact() -> TestCaseResult {
    for who in [ALICE, BOB, CHARLIE] {
        let actual = Positions::<Test>::iter()
            .filter(|(_, owner, balance)| *owner == who && *balance > 0)
            .count() as u32;
        prop_assert_eq!(PositionCount::<Test>::get(who), actual);
    }
    Ok(())
}

/// Reset guard kept as a named helper so a future mutable MinSplit test cannot
/// leak its parameter-thread-local value into these property cases.
struct PalletMinSplitGuard;

impl PalletMinSplitGuard {
    fn reset() {
        MinSplit::set(kernel::MIN_SPLIT_USDC);
    }
}

#[test]
fn pt3_named_scalar_070005_vector_is_exact() {
    // Existing pallet regression oracle (03 §6.3): 14_001 + 2_999 + 2_999,
    // leaving exactly one unit in escrow from E=20_000.
    let mut state = LedgerState::new();
    state.create_vault(90, 0).unwrap();
    state.split(LedgerOrigin::Signed, 90, &0, 20_000).unwrap();
    state
        .split_scalar(LedgerOrigin::Signed, 90, Branch::Accept, &0, 20_000)
        .unwrap();
    state
        .transfer(
            LedgerOrigin::Signed,
            position(90, Branch::Accept, PositionKind::Short),
            &0,
            &1,
            10_000,
        )
        .unwrap();
    state
        .transfer(
            LedgerOrigin::Signed,
            position(90, Branch::Accept, PositionKind::Short),
            &0,
            &2,
            10_000,
        )
        .unwrap();
    state
        .resolve(LedgerOrigin::ResolveAuthority, 90, Branch::Accept)
        .unwrap();
    state
        .settle_scalar(LedgerOrigin::SettleAuthority, 90, FixedU64(700_050_000))
        .unwrap();
    state
        .redeem_scalar(90, ScalarSide::Long, &0, 20_000)
        .unwrap();
    state
        .redeem_scalar(90, ScalarSide::Short, &1, 10_000)
        .unwrap();
    state
        .redeem_scalar(90, ScalarSide::Short, &2, 10_000)
        .unwrap();
    let payouts: Vec<_> = state
        .events
        .iter()
        .filter_map(|event| match event {
            CoreEvent::ScalarRedeemed(_, _, payout) => Some(*payout),
            _ => None,
        })
        .collect();
    assert_eq!(payouts, vec![14_001, 2_999, 2_999]);
    assert_eq!(proposal_escrow(&state, 90), 1);
}

#[test]
fn pt3_named_split_100_then_void_vector_is_exact() {
    // Cross-checked against tests.rs: its base-unit fixture is 100_000.  Pair
    // merge pays exactly 100_000 (never the retired 200_000 rule).
    let mut state = LedgerState::new();
    state.create_vault(91, 0).unwrap();
    state.split(LedgerOrigin::Signed, 91, &0, 100_000).unwrap();
    state.void(LedgerOrigin::ResolveAuthority, 91).unwrap();
    let before = proposal_escrow(&state, 91);
    state.merge(LedgerOrigin::Signed, 91, &0, 100_000).unwrap();
    assert_eq!(before - proposal_escrow(&state, 91), 100_000);
    assert_eq!(proposal_escrow(&state, 91), 0);
}

#[test]
fn pt8_non_protocol_cap_and_protocol_exemption() {
    new_test_ext().execute_with(|| {
        for pid in 100..132u64 {
            Ledger::create_vault(RuntimeOrigin::signed(MARKET), pid, 0).unwrap();
            Ledger::split(RuntimeOrigin::signed(ALICE), pid, UNIT).unwrap();
        }
        assert_eq!(PositionCount::<Test>::get(ALICE), 64);
        Ledger::create_vault(RuntimeOrigin::signed(MARKET), 132, 0).unwrap();
        let snapshot = (
            Vaults::<Test>::get(132),
            PositionCount::<Test>::get(ALICE),
            DepositsHeld::<Test>::get(),
        );
        assert_eq!(
            Ledger::split(RuntimeOrigin::signed(ALICE), 132, UNIT),
            Err(PalletError::<Test>::TooManyPositions.into())
        );
        assert_eq!(Vaults::<Test>::get(132), snapshot.0);
        assert_eq!(PositionCount::<Test>::get(ALICE), snapshot.1);
        assert_eq!(DepositsHeld::<Test>::get(), snapshot.2);

        let held = DepositsHeld::<Test>::get();
        for pid in 200..240u64 {
            Ledger::create_vault(RuntimeOrigin::signed(MARKET), pid, 0).unwrap();
            Ledger::do_split(RuntimeOrigin::signed(MARKET), pid, BOOK, UNIT).unwrap();
        }
        assert_eq!(PositionCount::<Test>::get(BOOK), 0);
        assert_eq!(DepositsHeld::<Test>::get(), held);
        Ledger::do_try_state().unwrap();
    });
}

#[test]
fn operation_generator_is_complete_against_call_metadata() {
    type LedgerCall = crate::pallet::Call<Test>;

    // Construct every call once. `call_tag`'s no-wildcard match below makes a
    // newly-added FRAME call a compile error until it is assigned a tag.
    let registry = vec![
        LedgerCall::split {
            pid: PID,
            amount: kernel::MIN_SPLIT_USDC,
        },
        LedgerCall::merge {
            pid: PID,
            amount: kernel::MIN_SPLIT_USDC,
        },
        LedgerCall::split_scalar {
            pid: PID,
            branch: Branch::Accept,
            amount: kernel::MIN_SPLIT_USDC,
        },
        LedgerCall::merge_scalar {
            pid: PID,
            branch: Branch::Accept,
            amount: kernel::MIN_SPLIT_USDC,
        },
        LedgerCall::split_gate {
            pid: PID,
            branch: Branch::Accept,
            gate: GateType::Survival,
            amount: kernel::MIN_SPLIT_USDC,
        },
        LedgerCall::merge_gate {
            pid: PID,
            branch: Branch::Accept,
            gate: GateType::Survival,
            amount: kernel::MIN_SPLIT_USDC,
        },
        LedgerCall::transfer {
            position: position(PID, Branch::Accept, PositionKind::BranchUsdc),
            to: ALICE,
            amount: kernel::MIN_TRANSFER_USDC,
        },
        LedgerCall::split_baseline {
            epoch: BASELINE_EPOCH,
            amount: kernel::MIN_SPLIT_USDC,
        },
        LedgerCall::merge_baseline {
            epoch: BASELINE_EPOCH,
            amount: kernel::MIN_SPLIT_USDC,
        },
        LedgerCall::resolve {
            pid: PID,
            winner: Branch::Accept,
        },
        LedgerCall::void { pid: PID },
        LedgerCall::settle_scalar {
            pid: PID,
            s: FixedU64(0),
        },
        LedgerCall::settle_gate {
            pid: PID,
            gate: GateType::Survival,
            outcome: false,
        },
        LedgerCall::settle_baseline {
            epoch: BASELINE_EPOCH,
            s: FixedU64(0),
        },
        LedgerCall::redeem {
            pid: PID,
            amount: 1,
        },
        LedgerCall::redeem_scalar {
            pid: PID,
            side: ScalarSide::Long,
            amount: 1,
        },
        LedgerCall::redeem_scalar_pair {
            pid: PID,
            amount: 1,
        },
        LedgerCall::redeem_gate {
            pid: PID,
            gate: GateType::Survival,
            amount: 1,
        },
        LedgerCall::redeem_void {
            pid: PID,
            branch: Branch::Accept,
            kind: PositionKind::BranchUsdc,
            amount: 1,
        },
        LedgerCall::redeem_baseline {
            epoch: BASELINE_EPOCH,
            side: ScalarSide::Long,
            amount: 1,
        },
        LedgerCall::redeem_baseline_pair {
            epoch: BASELINE_EPOCH,
            amount: 1,
        },
        LedgerCall::sweep_dust { pid: PID },
        LedgerCall::sweep_dust_baseline {
            epoch: BASELINE_EPOCH,
        },
    ];

    let metadata = <LedgerCall as GetCallName>::get_call_names();
    assert_eq!(registry.len(), metadata.len(), "15 §4.2 call count drift");
    let mut seen = Vec::new();
    let mut runner = proptest::test_runner::TestRunner::default();
    for call in &registry {
        let name = call.get_call_name();
        assert!(metadata.contains(&name), "stale call registry row `{name}`");
        assert!(
            !seen.contains(&name),
            "duplicate call registry row `{name}`"
        );
        seen.push(name);

        let expected = call_tag(call);
        let generated = generator_arm(expected)
            .new_tree(&mut runner)
            .expect("generator arm constructs")
            .current();
        assert_eq!(
            generated.tag(),
            expected,
            "15 §4.2 call `{name}` maps to a non-existent/wrong generator arm"
        );
    }
}

fn call_tag(call: &crate::pallet::Call<Test>) -> OpTag {
    match call {
        crate::pallet::Call::split { .. } => OpTag::Split,
        crate::pallet::Call::merge { .. } => OpTag::Merge,
        crate::pallet::Call::split_scalar { .. } => OpTag::SplitScalar,
        crate::pallet::Call::merge_scalar { .. } => OpTag::MergeScalar,
        crate::pallet::Call::split_gate { .. } => OpTag::SplitGate,
        crate::pallet::Call::merge_gate { .. } => OpTag::MergeGate,
        crate::pallet::Call::transfer { .. } => OpTag::Transfer,
        crate::pallet::Call::split_baseline { .. } => OpTag::SplitBaseline,
        crate::pallet::Call::merge_baseline { .. } => OpTag::MergeBaseline,
        crate::pallet::Call::resolve { .. } => OpTag::Resolve,
        crate::pallet::Call::void { .. } => OpTag::Void,
        crate::pallet::Call::settle_scalar { .. } => OpTag::SettleScalar,
        crate::pallet::Call::settle_gate { .. } => OpTag::SettleGate,
        crate::pallet::Call::settle_baseline { .. } => OpTag::SettleBaseline,
        crate::pallet::Call::redeem { .. } => OpTag::Redeem,
        crate::pallet::Call::redeem_scalar { .. } => OpTag::RedeemScalar,
        crate::pallet::Call::redeem_scalar_pair { .. } => OpTag::RedeemScalarPair,
        crate::pallet::Call::redeem_gate { .. } => OpTag::RedeemGate,
        crate::pallet::Call::redeem_void { .. } => OpTag::RedeemVoid,
        crate::pallet::Call::redeem_baseline { .. } => OpTag::RedeemBaseline,
        crate::pallet::Call::redeem_baseline_pair { .. } => OpTag::RedeemBaselinePair,
        crate::pallet::Call::sweep_dust { .. } => OpTag::SweepDust,
        crate::pallet::Call::sweep_dust_baseline { .. } => OpTag::SweepDustBaseline,
        crate::pallet::Call::__Ignore(_, _) => unreachable!("FRAME ignore variant is uninhabited"),
    }
}
