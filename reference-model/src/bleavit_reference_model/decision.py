from dataclasses import dataclass
from decimal import Decimal
from enum import Enum
class Outcome(Enum): ADOPT="Adopt"; EXTEND="Extend"; REJECT="Reject"
class RejectReason(Enum): NOT_DECISION_GRADE="NotDecisionGrade"; HURDLE_NOT_MET="HurdleNotMet"; CONVERGENCE_FAILED="ConvergenceFailed"; SECOND_EXTENSION_FAILED="SecondExtensionFailed"; SECURITY_SIZING="SecuritySizing"; GATE_VETO_SURVIVAL="GateVetoSurvival"; GATE_VETO_SECURITY="GateVetoSecurity"
@dataclass(frozen=True)
class Decision: outcome: Outcome; reason: RejectReason | None = None

def decide(
    accept_full: Decimal,
    reject_full_effective: Decimal,
    delta: Decimal,
    accept_trailing: Decimal | None = None,
    reject_trailing_effective: Decimal | None = None,
    converged: bool = True,
    grade: bool = True,
    extended: bool = False,
    security_ok: bool = True,
    survival_gate: bool = False,
    security_gate: bool = False,
) -> Decision:
    """Steps 3-9 of the 11-step decide() rule (05 §5.4), window checks exact.

    `reject_*_effective` are the r_eff legs of 05 §5.3 (max of the reject TWAP
    and the σ-shaded Baseline floor), computed by the caller. `grade` conflates
    the step-3/5 decision-grade checks into one boolean; step 2 process holds,
    step 5's one-time insufficiency extension, and steps 10-11 are out of the
    model's scope. The step 6-8 match is normative: Extend fires only on
    full/trailing disagreement (once — recurrence while extended is
    SecondExtensionFailed); a joint failure rejects with HurdleNotMet when
    converged and ConvergenceFailed otherwise.
    """
    a_f = Decimal(accept_full); r_f = Decimal(reject_full_effective); d = Decimal(delta)
    a_t = a_f if accept_trailing is None else Decimal(accept_trailing)
    r_t = r_f if reject_trailing_effective is None else Decimal(reject_trailing_effective)
    if not grade: return Decision(Outcome.REJECT, RejectReason.NOT_DECISION_GRADE)
    if survival_gate: return Decision(Outcome.REJECT, RejectReason.GATE_VETO_SURVIVAL)
    if security_gate: return Decision(Outcome.REJECT, RejectReason.GATE_VETO_SECURITY)
    full_pass = a_f >= r_f + d                                    # step 6
    tail_pass = a_t >= r_t + d                                    # step 7
    if not (full_pass and tail_pass and converged):               # step 8 match
        if full_pass != tail_pass:
            if extended: return Decision(Outcome.REJECT, RejectReason.SECOND_EXTENSION_FAILED)
            return Decision(Outcome.EXTEND)
        return Decision(Outcome.REJECT, RejectReason.HURDLE_NOT_MET if converged else RejectReason.CONVERGENCE_FAILED)
    if not security_ok: return Decision(Outcome.REJECT, RejectReason.SECURITY_SIZING)
    return Decision(Outcome.ADOPT)
