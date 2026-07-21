from __future__ import annotations
from dataclasses import dataclass
from decimal import (
    Decimal,
    ROUND_CEILING,
    ROUND_FLOOR,
    ROUND_HALF_UP,
    localcontext,
)

WORK_PREC = 100
BASE_UNIT = Decimal("0.000001")
BLOCKS_PER_DAY = Decimal("14400")
CAP_PROPOSAL = Decimal("0.05")  # 13 §1 trs.cap_proposal.
POL_BUDGET = Decimal("0.0075")  # 13 §1 pol.budget_epoch.
LN2 = Decimal(
    "0.6931471805599453094172321214581765680755001343602552541206800094933936219696947156058633269964186875"
)
B_FLOORS = {
    "param": Decimal("10000"),
    "treasury": Decimal("25000"),
    "code": Decimal("60000"),
    "meta": Decimal("100000"),
}
V_MIN_FLOORS = {
    "param": Decimal("100000"),
    "treasury": Decimal("250000"),
    "code": Decimal("600000"),
    "meta": Decimal("1200000"),
}
# Phase-0 calibrated dec.delta floors (13 §1; V-12, 2026-07-19), raised from the
# pre-calibration 0.015/0.025/0.040/0.060 so that every class's decidable-harm
# false-pass rate is < 1 % and no PROFITABLE manipulation flips a proposal below
# 3·InCapPrize (15 §4.9). TREASURY/CODE/META took a uniform 1.5x; PARAM took 2.5x
# — its small floor + unbacked capability-envelope prizes (SQ-173) make marginal
# near-boundary flips cheap, so it needs the extra margin to reach 0 profitable
# exploits. The 0.005 kernel floor (DECISION_DELTA_FLOOR) is unchanged.
DELTA_FLOORS = {
    "param": Decimal("0.0375"),
    "treasury": Decimal("0.0375"),
    "code": Decimal("0.060"),
    "meta": Decimal("0.090"),
}
GATE_B = Decimal("7500")
BASELINE_B = Decimal("25000")
# 13 §1 sec.flow_cap: value is Phase-0 sim-gated ([VERIFY]); 7 is the kernel
# hard minimum (08 §5.3 — below ×7 the ceiling could reject honest
# exactly-grade proposals). The model therefore takes flow_cap as an input
# from its consumers and enforces only the bound.
FLOW_CAP_MIN = Decimal(7)


@dataclass(frozen=True)
class NavView:
    nav: Decimal
    spendable_nav: Decimal
    reserve_impaired: bool


def _d(value) -> Decimal:
    return value if isinstance(value, Decimal) else Decimal(str(value))


def _class_name(value) -> str:
    name = str(getattr(value, "value", value)).lower()
    if name not in B_FLOORS:
        raise ValueError("unknown proposal class")
    return name


def round_up(value) -> Decimal:
    with localcontext() as ctx:
        ctx.prec = WORK_PREC
        return _d(value).quantize(BASE_UNIT, rounding=ROUND_CEILING)


def round_down(value) -> Decimal:
    with localcontext() as ctx:
        ctx.prec = WORK_PREC
        return _d(value).quantize(BASE_UNIT, rounding=ROUND_FLOOR)


def display_integer(value) -> int:
    return int(_d(value).to_integral_value(rounding=ROUND_HALF_UP))


def nav(
    liquid_usdc,
    undisbursed_reversions=Decimal(0),
    obligations=Decimal(0),
    reserve_impaired: bool = False,
    liabilities=None,
    vit_holdings=Decimal(0),
    in_flight_xcm=Decimal(0),
) -> NavView:
    """08 §1.2 NAV; VIT and in-flight XCM are deliberately marked zero."""
    del vit_holdings, in_flight_xcm
    if isinstance(liquid_usdc, (list, tuple)):
        liquid = sum((_d(value) for value in liquid_usdc), Decimal(0))
    else:
        liquid = _d(liquid_usdc)
    if liabilities is not None:
        obligations = liabilities
    with localcontext() as ctx:
        ctx.prec = WORK_PREC
        value = max(
            Decimal(0),
            liquid + _d(undisbursed_reversions) - _d(obligations),
        )
        return NavView(
            nav=value,
            spendable_nav=Decimal(0) if reserve_impaired else value,
            reserve_impaired=reserve_impaired,
        )


def in_cap_prize(
    proposal_class,
    *,
    ask=Decimal(0),
    envelope=Decimal(0),
    spendable_nav=Decimal(0),
    cap_proposal: Decimal = CAP_PROPOSAL,
    upgrade_payload: bool = True,
) -> Decimal:
    """08 §5.2 prize proxy, rounded up as required by 05 §5.4 step 9."""
    name = _class_name(proposal_class)
    ask = _d(ask)
    envelope = _d(envelope)
    cap = _d(cap_proposal) * _d(spendable_nav)
    if name == "treasury":
        prize = ask
    elif name == "param":
        prize = envelope
    else:
        prize = (
            max(ask, envelope, cap)
            if upgrade_payload
            else max(ask, envelope)
        )
    return round_up(prize)


def _flow_cap(value) -> Decimal:
    cap = _d(value)
    if cap < FLOW_CAP_MIN:
        raise ValueError("sec.flow_cap below its hard minimum of 7 (08 §5.3)")
    return cap


def decision_pair_contest_capital(contest_accept, contest_reject) -> Decimal:
    """04 §7a / 08 §5.2: binding contest capital of a decision pair."""
    accept = _d(contest_accept)
    reject = _d(contest_reject)
    if accept < 0 or reject < 0:
        raise ValueError("negative decision-book contest capital")
    return min(accept, reject)


def l_hat(pol_depth, contest_capital, flow_cap, b_accept, b_reject) -> Decimal:
    """08 §5.2 L-hat: POL pair depth + the capped non-POL contest term.

    The non-POL term is min(contest capital of the decision pair over the
    decision window (04 §7a — SQ-231: gross traded notional no longer feeds
    the certificate), sec.flow_cap * (b_acc + b_rej)).
    """
    with localcontext() as ctx:
        ctx.prec = WORK_PREC
        pol_depth = _d(pol_depth)
        contest = _d(contest_capital)
        pair_b = _d(b_accept) + _d(b_reject)
        if pol_depth < 0 or contest < 0 or _d(b_accept) < 0 or _d(b_reject) < 0:
            raise ValueError("negative security-sizing input")
        return pol_depth + min(contest, _flow_cap(flow_cap) * pair_b)


def attack_cost_hat(
    liquidity,
    *,
    published_flow_per_day: Decimal | None = None,
    decision_window: int = 43_200,
) -> Decimal:
    """08 §5.2 F-hat·T_dec, rounded down."""
    with localcontext() as ctx:
        ctx.prec = WORK_PREC
        liquidity = _d(liquidity)
        if liquidity < 0 or decision_window < 0:
            raise ValueError("negative security-sizing input")
        half = liquidity / Decimal(2)
        flow = (
            half
            if published_flow_per_day is None
            else min(half, _d(published_flow_per_day))
        )
        days = Decimal(decision_window) / BLOCKS_PER_DAY
        return round_down(flow * days)


def security_sizing_ok(prize, attack_cost) -> bool:
    """05 §5.4 step 9: never divide the conservatively rounded cost."""
    return Decimal(3) * _d(prize) <= _d(attack_cost)


def dec_v_min(proposal_class, prize) -> Decimal:
    name = _class_name(proposal_class)
    return max(V_MIN_FLOORS[name], Decimal(2) * _d(prize))


def p_ref(proposal_class) -> Decimal:
    name = _class_name(proposal_class)
    with localcontext() as ctx:
        ctx.prec = WORK_PREC
        depth = Decimal(2) * B_FLOORS[name] * LN2
        # 08 §5.4 P_ref: every class uses the decision-pair depth 2·b·ln2
        # (the former PARAM 27,726/63,863 cells were a doubling slip,
        # reconciled 2026-07-15 — SQ-29).
        return (depth + V_MIN_FLOORS[name]) / Decimal(2)


def pol_b(proposal_class, prize) -> Decimal:
    name = _class_name(proposal_class)
    ratio = max(Decimal(1), _d(prize) / p_ref(name))
    return B_FLOORS[name] * ratio


def decision_delta(proposal_class, prize) -> Decimal:
    name = _class_name(proposal_class)
    ratio = max(Decimal(1), _d(prize) / p_ref(name))
    return min(DELTA_FLOORS[name] * ratio, Decimal("0.10"))


def pol_commitment(proposal_class) -> Decimal:
    name = _class_name(proposal_class)
    books_b = Decimal(2) * B_FLOORS[name]
    if name in ("param", "treasury", "code", "meta"):
        books_b += Decimal(4) * GATE_B
    with localcontext() as ctx:
        ctx.prec = WORK_PREC
        return books_b * LN2


def baseline_commitment() -> Decimal:
    with localcontext() as ctx:
        ctx.prec = WORK_PREC
        return BASELINE_B * LN2


def nav_floor(
    proposal_class,
    *,
    slots: int = 1,
) -> Decimal:
    if slots <= 0:
        raise ValueError("slots must be positive")
    name = _class_name(proposal_class)
    exact = pol_commitment(name) * slots
    # SPEC-NOTE(08 §4.1 rounding convention): displayed rows inconsistently
    # divide exact and whole-USDC commitments. These branches reproduce every
    # displayed row within the mandated 10-USDC tolerance without changing 08.
    if name == "meta":
        charged = (
            exact.to_integral_value(rounding=ROUND_HALF_UP)
            if slots == 1
            else exact.to_integral_value(rounding=ROUND_FLOOR)
        )
    elif name == "treasury":
        charged = exact.to_integral_value(rounding=ROUND_HALF_UP)
    else:
        charged = exact
    return charged / POL_BUDGET


def manip_floor_hat(
    books,
    delta,
    contest_capital,
    flow_cap,
) -> Decimal:
    """05 §5.6 C_disp+C_hold diagnostic; it never gates.

    `contest_capital` is V_win, the 04 §7a time-averaged marked value of net
    outstanding trader positions over the window (SQ-231 amendment: gross
    traded notional is not the measure). Churn and wash flow net out of it by
    construction, so sec.flow_cap is a secondary ceiling here, not the control
    that bounds wash-trade inflation.
    """
    delta = _d(delta)
    with localcontext() as ctx:
        ctx.prec = WORK_PREC
        c_disp = Decimal(0)
        total_b = Decimal(0)
        for b, price in books:
            b = _d(b)
            price = _d(price)
            if not Decimal(0) < price < Decimal(1) - delta:
                raise ValueError("displacement leaves probability domain")
            ratio = (
                (price + delta) * (Decimal(1) - price)
                / ((Decimal(1) - price - delta) * price)
            )
            c_disp += b * ratio.ln()
            total_b += b
        held_flow = min(_d(contest_capital), _flow_cap(flow_cap) * total_b)
        c_hold = held_flow * delta
        return round_down(c_disp + c_hold)
