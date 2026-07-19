from __future__ import annotations
from dataclasses import dataclass
from decimal import Decimal
from enum import Enum
from typing import Mapping

from .treasury import (
    attack_cost_hat,
    decision_pair_contest_capital,
    in_cap_prize,
    l_hat,
    security_sizing_ok,
)


class Outcome(Enum):
    ADOPT = "Adopt"
    EXTEND = "Extend"
    REJECT = "Reject"


class RejectReason(Enum):
    NOT_DECISION_GRADE = "NotDecisionGrade"
    GATE_VETO_SURVIVAL = "GateVetoSurvival"
    GATE_VETO_SECURITY = "GateVetoSecurity"
    HURDLE_NOT_MET = "HurdleNotMet"
    CONVERGENCE_FAILED = "ConvergenceFailed"
    SECOND_EXTENSION_FAILED = "SecondExtensionFailed"
    PROCESS_HOLD = "ProcessHold"
    CONSTITUTION_VIOLATION = "ConstitutionViolation"
    RESOURCE_CONFLICT = "ResourceConflict"
    RATE_LIMITED = "RateLimited"
    VETO_UPHELD_BY_REVIEW = "VetoUpheldByReview"  # Guardian review producer, T24.
    STALE_QUEUE = "StaleQueue"  # Execution-guard producer, T16.
    PAYLOAD_REVERTED = "PayloadReverted"  # Execution recording, T18/T22.
    NOT_RATIFIED = "NotRatified"  # Execution-guard producer, T16.
    SECURITY_SIZING = "SecuritySizing"
    ATTESTATION_MISSING = "AttestationMissing"


class Grade(Enum):
    OK = "Ok"
    INSUFFICIENT = "Insufficient"
    INVALID = "Invalid"


class Gate(Enum):
    SURVIVAL = "Survival"
    SECURITY = "Security"


class ProposalClass(Enum):
    PARAM = "Param"
    TREASURY = "Treasury"
    CODE = "Code"
    META = "Meta"


@dataclass(frozen=True)
class Decision:
    outcome: Outcome
    reason: RejectReason | None = None


DEFAULT_P_MAX = Decimal("0.05")  # 13 §1 gate.p_max.
DEFAULT_EPS = Decimal("0.02")  # 13 §1 gate.eps.
SANITY_LO = Decimal("0.02")  # 05 §5.2 sanity band (welfare books only).
SANITY_HI = Decimal("0.98")
DELTA_MAX = Decimal("0.05")  # 05 §5.2 |spot_close - TWAP| convergence bound.
COVERAGE_MIN = Decimal("0.95")  # 13 §1 dec.coverage.
GATE_NB_COVERAGE = Decimal("0.98")  # 13 §1 gate.nb_coverage.
GATE_NB_CONV = Decimal("0.01")  # 13 §1 gate.nb_conv.
GATE_V_MIN_FACTOR = Decimal("0.1")  # 05 §5.2: gate.v_min = 0.1*dec.v_min(class).


def grade_welfare_book(
    *,
    twap: Decimal,
    spot_close: Decimal,
    coverage: Decimal,
    stale_events: int,
    pol_floor_met: bool,
    pol_undisturbed: bool,
    contest_capital: Decimal,
    v_min: Decimal,
    coverage_min: Decimal = COVERAGE_MIN,
    delta_max: Decimal = DELTA_MAX,
    sanity_lo: Decimal = SANITY_LO,
    sanity_hi: Decimal = SANITY_HI,
) -> Grade:
    """05 §5.2 welfare-book decision grade, per book.

    The contest measure is the 04 §7a time-averaged at-risk contest capital:
    balanced complete sets and gross traded notional are NOT the measure. The
    remediable-by-time shortfalls (contest capital below `dec.v_min(class)`,
    coverage below `dec.coverage`, a first stale event — 04 §7 grants exactly
    one staleness extension) grade Insufficient, feeding step 5's single
    shared extension; every other failure (sanity band, POL floor/undisturbed,
    a second stale event, non-convergence) grades Invalid — status-quo
    default, fail-closed.
    """
    twap = Decimal(twap)
    spot_close = Decimal(spot_close)
    if not Decimal(0) <= twap <= Decimal(1):
        raise ValueError("TWAP must be in [0, 1]")
    if stale_events < 0:
        raise ValueError("stale_events must be non-negative")
    if not sanity_lo <= twap <= sanity_hi:
        return Grade.INVALID
    if not (pol_floor_met and pol_undisturbed):
        return Grade.INVALID
    if stale_events >= 2:
        return Grade.INVALID
    if abs(spot_close - twap) > delta_max:
        return Grade.INVALID
    if (
        Decimal(contest_capital) < Decimal(v_min)
        or Decimal(coverage) < coverage_min
        or stale_events == 1
    ):
        return Grade.INSUFFICIENT
    return Grade.OK


def gate_decision_grade(
    *,
    twap: Decimal,
    spot_close: Decimal,
    coverage: Decimal,
    stale_events: int,
    pol_floor_met: bool,
    pol_undisturbed: bool,
    contest_capital: Decimal,
    gate_v_min: Decimal,
    nb_coverage: Decimal = GATE_NB_COVERAGE,
    nb_conv: Decimal = GATE_NB_CONV,
) -> bool:
    """05 §5.2 gate-book validity (step 3 input): band-exempt, GB-NB outside.

    Inside [0.02, 0.98] the welfare-book validity checks apply; outside, the
    near-boundary rule (coverage >= gate.nb_coverage, zero stale events,
    |spot_close - TWAP| <= gate.nb_conv). The `gate.v_min` contest floor —
    graded over the same 04 §7a contest-capital measure — applies in both
    regimes.
    """
    twap = Decimal(twap)
    spot_close = Decimal(spot_close)
    if not Decimal(0) <= twap <= Decimal(1):
        raise ValueError("TWAP must be in [0, 1]")
    if stale_events < 0:
        raise ValueError("stale_events must be non-negative")
    if Decimal(contest_capital) < Decimal(gate_v_min):
        return False
    if SANITY_LO <= twap <= SANITY_HI:
        return (
            grade_welfare_book(
                twap=twap,
                spot_close=spot_close,
                coverage=coverage,
                stale_events=stale_events,
                pol_floor_met=pol_floor_met,
                pol_undisturbed=pol_undisturbed,
                contest_capital=contest_capital,
                v_min=gate_v_min,
            )
            is Grade.OK
        )
    return (
        Decimal(coverage) >= nb_coverage
        and stale_events == 0
        and abs(spot_close - twap) <= nb_conv
    )


def _decimal_map(
    values: Mapping[Gate | str, Decimal] | None, default: Decimal
) -> dict[Gate, Decimal]:
    result = {gate: default for gate in Gate}
    if values is None:
        return result
    for gate in Gate:
        for key in (gate, gate.value, gate.name, gate.name.lower()):
            if key in values:
                result[gate] = Decimal(values[key])
                break
    return result


def _bool_map(
    values: Mapping[Gate | str, bool] | None, default: bool
) -> dict[Gate, bool]:
    result = {gate: default for gate in Gate}
    if values is None:
        return result
    for gate in Gate:
        for key in (gate, gate.value, gate.name, gate.name.lower()):
            if key in values:
                result[gate] = bool(values[key])
                break
    return result


def _grade(value: Grade | str) -> Grade:
    if isinstance(value, Grade):
        return value
    for grade in Grade:
        if value in (grade.value, grade.name, grade.name.lower()):
            return grade
    raise ValueError("unknown grade")


def _proposal_class(value: ProposalClass | str) -> ProposalClass:
    if isinstance(value, ProposalClass):
        return value
    for proposal_class in ProposalClass:
        if value in (
            proposal_class.value,
            proposal_class.name,
            proposal_class.name.lower(),
        ):
            return proposal_class
    raise ValueError("unknown proposal class")


def decide(
    accept_full: Decimal,
    reject_full_effective: Decimal,
    delta: Decimal,
    accept_trailing: Decimal | None = None,
    reject_trailing_effective: Decimal | None = None,
    converged: bool = True,
    extended: bool = False,
    preimage_ok: bool = True,
    resource_locks_held: bool = True,
    process_hold: bool = False,
    requires_gate_markets: bool = False,
    gate_book_valid: bool = True,
    gate_valid: Mapping[Gate | str, bool] | None = None,
    p_adopt: Mapping[Gate | str, Decimal] | None = None,
    p_reject: Mapping[Gate | str, Decimal] | None = None,
    p_max: Mapping[Gate | str, Decimal] | None = None,
    eps: Mapping[Gate | str, Decimal] | None = None,
    welfare_grade: Grade | str = Grade.OK,
    grade: bool | None = None,
    proposal_class: ProposalClass | str = ProposalClass.PARAM,
    ask: Decimal = Decimal(0),
    envelope_value: Decimal = Decimal(0),
    spendable_nav: Decimal = Decimal(0),
    measured_liquidity: Decimal = Decimal(0),
    pol_depth: Decimal | None = None,
    contest_accept: Decimal | None = None,
    contest_reject: Decimal | None = None,
    flow_cap: Decimal | None = None,
    b_accept: Decimal | None = None,
    b_reject: Decimal | None = None,
    published_flow_per_day: Decimal | None = None,
    decision_window: int = 43_200,
    attestation_ok: bool = True,
    queue_time_ok: bool = True,
) -> Decision:
    """The ordered 11-step decision rule of 05 §5.4.

    `grade` is a compatibility alias for the former welfare-only boolean; gate
    validity remains a separate step and is never inferred from it.

    Step 9's L-hat may be supplied either pre-composed (`measured_liquidity`)
    or decomposed per 08 §5.2 (`pol_depth` + the binding
    `min(contest_accept, contest_reject)` bounded by
    `flow_cap * (b_accept + b_reject)` — the SQ-231 contest-capital form);
    supplying any of the six decomposed inputs requires all six.
    """
    a_f = Decimal(accept_full)
    r_f = Decimal(reject_full_effective)
    d = Decimal(delta)
    a_t = a_f if accept_trailing is None else Decimal(accept_trailing)
    r_t = (
        r_f
        if reject_trailing_effective is None
        else Decimal(reject_trailing_effective)
    )
    proposal_class = _proposal_class(proposal_class)
    if grade is not None:
        welfare_grade = Grade.OK if grade else Grade.INVALID
    welfare_grade = _grade(welfare_grade)

    # Step 1: payload/preimage, then resource locks.
    if not preimage_ok:
        return Decision(Outcome.REJECT, RejectReason.CONSTITUTION_VIOLATION)
    if not resource_locks_held:
        return Decision(Outcome.REJECT, RejectReason.RESOURCE_CONFLICT)

    # Step 2: disputes, guardian hold, and dead-man state.
    if process_hold:
        return Decision(Outcome.REJECT, RejectReason.PROCESS_HOLD)

    # Steps 3-4 apply only to gate-bearing classes. Per 05 §5.4 the loop asserts
    # each gate's validity *then* its veto, so a Survival veto is reported before
    # Security's validity is inspected (per-gate `gate_valid` overrides the scalar
    # `gate_book_valid` fallback).
    if requires_gate_markets:
        adopt = _decimal_map(p_adopt, Decimal(0))
        reject = _decimal_map(p_reject, Decimal(0))
        maxima = _decimal_map(p_max, DEFAULT_P_MAX)
        margins = _decimal_map(eps, DEFAULT_EPS)
        valid = _bool_map(gate_valid, gate_book_valid)
        for gate in (Gate.SURVIVAL, Gate.SECURITY):
            if not valid[gate]:
                return Decision(Outcome.REJECT, RejectReason.NOT_DECISION_GRADE)
            if (
                adopt[gate] > maxima[gate]
                or adopt[gate] > reject[gate] + margins[gate]
            ):
                reason = (
                    RejectReason.GATE_VETO_SURVIVAL
                    if gate is Gate.SURVIVAL
                    else RejectReason.GATE_VETO_SECURITY
                )
                return Decision(Outcome.REJECT, reason)

    # Step 5: welfare grade, with one shared extension budget.
    if welfare_grade is Grade.INSUFFICIENT and not extended:
        return Decision(Outcome.EXTEND)
    if welfare_grade is not Grade.OK:
        return Decision(Outcome.REJECT, RejectReason.NOT_DECISION_GRADE)

    # Steps 6-8: full/trailing hurdle and convergence match.
    full_pass = a_f >= r_f + d
    tail_pass = a_t >= r_t + d
    if not (full_pass and tail_pass and converged):
        if full_pass != tail_pass:
            if extended:
                return Decision(
                    Outcome.REJECT, RejectReason.SECOND_EXTENSION_FAILED
                )
            return Decision(Outcome.EXTEND)
        reason = (
            RejectReason.HURDLE_NOT_MET
            if converged
            else RejectReason.CONVERGENCE_FAILED
        )
        return Decision(Outcome.REJECT, reason)

    # Step 9: 3·InCapPrize <= AttackCost-hat, with treasury rounding.
    decomposed = (
        pol_depth,
        contest_accept,
        contest_reject,
        flow_cap,
        b_accept,
        b_reject,
    )
    if any(part is not None for part in decomposed):
        if any(part is None for part in decomposed):
            raise ValueError(
                "decomposed L-hat needs pol_depth, contest_accept, "
                "contest_reject, flow_cap, b_accept and b_reject together"
            )
        pair_contest = decision_pair_contest_capital(
            contest_accept, contest_reject
        )
        measured_liquidity = l_hat(
            pol_depth, pair_contest, flow_cap, b_accept, b_reject
        )
    prize = in_cap_prize(
        proposal_class,
        ask=ask,
        envelope=envelope_value,
        spendable_nav=spendable_nav,
    )
    attack_cost = attack_cost_hat(
        measured_liquidity,
        published_flow_per_day=published_flow_per_day,
        decision_window=decision_window,
    )
    if not security_sizing_ok(prize, attack_cost):
        return Decision(Outcome.REJECT, RejectReason.SECURITY_SIZING)

    # Step 10: attestation precedes constitutional meters/spacing.
    if proposal_class in (ProposalClass.CODE, ProposalClass.META):
        if not attestation_ok:
            return Decision(Outcome.REJECT, RejectReason.ATTESTATION_MISSING)
    if not queue_time_ok:
        return Decision(Outcome.REJECT, RejectReason.RATE_LIMITED)

    # Step 11.
    return Decision(Outcome.ADOPT)
