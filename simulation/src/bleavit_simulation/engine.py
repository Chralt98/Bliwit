from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal, localcontext
import random

from bleavit_reference_model.decision import (
    Decision,
    Gate,
    Grade,
    Outcome,
    ProposalClass,
    RejectReason,
    decide,
)
from bleavit_reference_model.lmsr import displacement_cost
from bleavit_reference_model.treasury import (
    BASELINE_B,
    GATE_B,
    LN2,
    attack_cost_hat,
    dec_v_min,
    decision_delta,
    in_cap_prize,
    pol_b,
    round_down,
)

from .config import (
    GATE_EPS,
    GATE_P_MAX,
    GATE_V_MIN_FRACTION,
    SIGMA_FLOORS,
    SimulationConfig,
)
from .market import (
    BookSummary,
    ExecutedBook,
    execute_toward,
    execute_turnover,
    summarize_executed_book,
)
from .proposals import Proposal, persistent_belief_error, proposal_rng


def _clamp(value: Decimal, lo: Decimal = Decimal("0.001"), hi: Decimal = Decimal("0.999")) -> Decimal:
    return min(max(Decimal(value), lo), hi)


def _class_enum(name: str) -> ProposalClass:
    return {
        "param": ProposalClass.PARAM,
        "treasury": ProposalClass.TREASURY,
        "code": ProposalClass.CODE,
        "meta": ProposalClass.META,
    }[name]


def _requires_gate(proposal: Proposal) -> bool:
    return proposal.gate_exposure == "gate"


def _strategy_for(proposal_id: int, config: SimulationConfig) -> str:
    epoch = proposal_id // config.epoch_slate_size
    rng = proposal_rng(0x5354524154454759, epoch, 0x4D4958)
    draw = Decimal(str(rng.random()))
    cumulative = Decimal(0)
    for strategy, weight in config.attack_strategy_mix:
        cumulative += Decimal(weight)
        if draw < cumulative:
            return strategy
    return config.attack_strategy_mix[-1][0]


def _formation_ratio(proposal: Proposal, seed: int, config: SimulationConfig) -> Decimal:
    row = next(row for row in config.formation_strata if row[0] == proposal.formation_regime)
    rng = proposal_rng(seed, proposal.proposal_id, 0x464F524D)
    lo, hi = Decimal(row[1]), Decimal(row[2])
    return lo + (hi - lo) * Decimal(str(rng.random()))


def _segment_starts(config: SimulationConfig) -> tuple[int, ...]:
    late = config.decision_window - config.late_spike_blocks
    starts = (1, 7_201, 14_401, 21_601, config.decision_window - config.trailing_window + 1, late + 1)
    if tuple(sorted(set(starts))) != starts:
        raise ValueError("Phase-0 segment schedule requires doc-13 windows")
    return starts


@dataclass(frozen=True)
class GateBookEvidence:
    gate: str
    branch: str
    contest: Decimal
    valid: bool
    summary: BookSummary

    def evidence(self) -> dict:
        return {
            "branch": self.branch,
            "contest": str(self.contest),
            "full": str(self.summary.full),
            "gate": self.gate,
            "observed_close": str(self.summary.observed_close),
            "spot": str(self.summary.spot),
            "stale_events": self.summary.stale_events,
            "valid": self.valid,
        }


@dataclass(frozen=True)
class SimulationResult:
    proposal: Proposal
    outcome: str
    reason: str | None
    initial_outcome: str
    extended: bool
    strategy: str
    budget_multiple: Decimal
    delta: Decimal
    b: Decimal
    baseline_b: Decimal
    prize: Decimal | None
    v_min: Decimal
    contest_accept: Decimal
    contest_reject: Decimal
    initial_contest_accept: Decimal
    initial_contest_reject: Decimal
    baseline_contest: Decimal
    accept: BookSummary
    reject: BookSummary
    baseline: BookSummary
    gate_books: tuple[GateBookEvidence, ...]
    baseline_valid: bool
    baseline_carried: bool
    attack_cost: Decimal
    measured_liquidity: Decimal
    manip_floor: Decimal | None
    manip_c_disp: tuple[Decimal, Decimal] | None
    realized_manipulation_spend: Decimal
    manipulator_gross_deployed: Decimal
    manipulator_displacement: Decimal
    informed_flow: Decimal
    noise_flow: Decimal
    arbitrage_flow: Decimal
    manipulator_flow: Decimal
    welfare_grade: str
    ledger_conservation_error: Decimal

    @property
    def decidable_harm(self) -> bool:
        return self.proposal.harmful and abs(self.proposal.true_effect) >= self.delta

    def evidence(self) -> dict:
        return {
            "arbitrage_corrective_flow": str(self.arbitrage_flow),
            "attack_cost_hat": str(self.attack_cost),
            "b": str(self.b),
            "baseline_b": str(self.baseline_b),
            "baseline_carried": self.baseline_carried,
            "baseline_contest": str(self.baseline_contest),
            "baseline_full": str(self.baseline.full),
            "baseline_observed_close": str(self.baseline.observed_close),
            "baseline_raw_spot": str(self.baseline.spot),
            "baseline_valid": self.baseline_valid,
            "budget_multiple": str(self.budget_multiple),
            "contest_accept": str(self.contest_accept),
            "contest_reject": str(self.contest_reject),
            "decidable_harm": self.decidable_harm,
            "delta": str(self.delta),
            "extended": self.extended,
            "full_accept": str(self.accept.full),
            "full_reject": str(self.reject.full),
            "gate_books": [row.evidence() for row in self.gate_books],
            "informed_flow": str(self.informed_flow),
            "initial_contest_accept": str(self.initial_contest_accept),
            "initial_contest_reject": str(self.initial_contest_reject),
            "initial_outcome": self.initial_outcome,
            "ledger_conservation_error": str(self.ledger_conservation_error),
            "manip_c_disp": None if self.manip_c_disp is None else [str(value) for value in self.manip_c_disp],
            "manip_floor_hat": None if self.manip_floor is None else str(self.manip_floor),
            "measured_liquidity": str(self.measured_liquidity),
            "manipulator_displacement": str(self.manipulator_displacement),
            "manipulator_flow": str(self.manipulator_flow),
            "manipulator_gross_deployed": str(self.manipulator_gross_deployed),
            "noise_flow": str(self.noise_flow),
            "observed_close_accept": str(self.accept.observed_close),
            "observed_close_reject": str(self.reject.observed_close),
            "outcome": self.outcome,
            "prize": None if self.prize is None else str(self.prize),
            "proposal": self.proposal.evidence(),
            "raw_spot_accept": str(self.accept.spot),
            "raw_spot_reject": str(self.reject.spot),
            "realized_manipulation_spend": str(self.realized_manipulation_spend),
            "reason": self.reason,
            "strategy": self.strategy,
            "trailing_accept": str(self.accept.trailing),
            "trailing_reject": str(self.reject.trailing),
            "v_min": str(self.v_min),
            "welfare_grade": self.welfare_grade,
        }


def _price_targets(proposal: Proposal, seed: int, prize: Decimal) -> tuple[Decimal, Decimal, Decimal]:
    rng = proposal_rng(seed, proposal.proposal_id, 0x5452555448)
    baseline = Decimal("0.48") + Decimal(str(rng.random())) * Decimal("0.08")
    error = persistent_belief_error(seed, proposal)
    signal = proposal.true_effect + error
    return (
        _clamp(baseline + signal / Decimal(2), Decimal("0.021"), Decimal("0.979")),
        _clamp(baseline - signal / Decimal(2), Decimal("0.021"), Decimal("0.979")),
        baseline,
    )


def _fund(book: ExecutedBook, participant: str, gross: Decimal) -> None:
    # Cost is at most amount at the quoting clamp; the explicit 2% buffer pays
    # 30 bps fees and adverse path movement without granting infinite balance.
    book.account(participant, Decimal(gross) * Decimal("1.02") + Decimal(1))


def _execute_organic_window(
    book: ExecutedBook,
    *,
    truth: Decimal,
    desired_contest: Decimal,
    seed: int,
    proposal_id: int,
    salt: int,
    config: SimulationConfig,
    extension: bool,
) -> None:
    starts = _segment_starts(config)
    noise_share = Decimal(config.noise_flow_share)
    informed_total = Decimal(desired_contest) * (Decimal(1) - noise_share)
    noise_total = Decimal(desired_contest) * noise_share
    informed_name = f"informed:{book.name}"
    noise_name = f"noise:{book.name}"
    _fund(book, informed_name, informed_total)
    _fund(book, noise_name, noise_total)
    rng = proposal_rng(seed, proposal_id, salt + (0x4558 if extension else 0))
    arrival = (Decimal("0.45"), Decimal("0.70"), Decimal("0.88"), Decimal("1"), Decimal("1"), Decimal("1"))
    for index, block in enumerate(starts):
        informed_budget = informed_total / Decimal(len(starts))
        informed_target = _clamp(Decimal("0.5") + (truth - Decimal("0.5")) * arrival[index])
        used = execute_toward(
            book,
            informed_name,
            target=informed_target,
            gross_notional=informed_budget,
            block=block,
            role="informed",
        )
        execute_turnover(
            book,
            informed_name,
            gross_notional=max(Decimal(0), informed_budget - used),
            block=block,
            role="informed",
            first_side="long" if truth >= Decimal("0.5") else "short",
        )
        noise_budget = noise_total / Decimal(len(starts))
        amplitude = Decimal(config.noise_price_amplitude_delta)
        noise_target = _clamp(
            truth + (Decimal(str(rng.random())) - Decimal("0.5")) * amplitude
        )
        noise_used = execute_toward(
            book,
            noise_name,
            target=noise_target,
            gross_notional=noise_budget,
            block=block,
            role="noise",
        )
        execute_turnover(
            book,
            noise_name,
            gross_notional=max(Decimal(0), noise_budget - noise_used),
            block=block,
            role="noise",
            first_side="long" if rng.randrange(2) else "short",
        )


def _attack_targets(harmful: bool) -> tuple[Decimal, Decimal]:
    return (
        (Decimal("0.95"), Decimal("0.05"))
        if harmful
        else (Decimal("0.05"), Decimal("0.95"))
    )


def _apply_pair_attack(
    accept: ExecutedBook,
    reject: ExecutedBook,
    *,
    proposal: Proposal,
    config: SimulationConfig,
    strategy: str,
    budget: Decimal,
    accept_truth: Decimal,
    reject_truth: Decimal,
    organic_pair_contest: Decimal,
    reuse_attacker_balance: bool = False,
) -> None:
    if budget <= 0 or strategy == "th7_baseline_suppression":
        return
    target_accept, target_reject = _attack_targets(proposal.harmful)
    first_index = 5 if strategy == "th2_late_spike" else 0
    starts = _segment_starts(config)
    affected = len(starts) - first_index
    days = (
        Decimal(config.late_spike_blocks) / Decimal(14_400)
        if strategy == "th2_late_spike"
        else Decimal(config.decision_window) / Decimal(14_400)
    )
    base_liquidity = Decimal(2) * accept.b * LN2 + organic_pair_contest
    pair_correction = (
        base_liquidity / Decimal(2) * days * Decimal(config.arbitrage_elasticity)
    )
    for book in (accept, reject):
        if not reuse_attacker_balance:
            book.account("manipulator", budget / Decimal(2))
        _fund(book, "arbitrage", pair_correction / Decimal(2))
    per_attack = budget / Decimal(2) / Decimal(affected)
    per_correction = pair_correction / Decimal(2) / Decimal(affected)
    for index in range(first_index, len(starts)):
        block = starts[index]
        for book, attack_target, truth in (
            (accept, target_accept, accept_truth),
            (reject, target_reject, reject_truth),
        ):
            attacked = execute_toward(
                book,
                "manipulator",
                target=attack_target,
                gross_notional=per_attack,
                block=block,
                role="manipulator",
            )
            if attacked <= 0:
                continue
            corrected = execute_toward(
                book,
                "arbitrage",
                target=truth,
                gross_notional=min(per_correction, attacked),
                block=block,
                role="arbitrage",
            )
            # The attacker can counter only the correction actually executed;
            # its explicit cash balance enforces the original budget.
            execute_toward(
                book,
                "manipulator",
                target=attack_target,
                gross_notional=corrected,
                block=block,
                role="manipulator",
            )


def _epoch_baseline_truth(seed: int, proposal_id: int, config: SimulationConfig) -> tuple[Decimal, Decimal, bool]:
    epoch = proposal_id // config.epoch_slate_size
    rng = proposal_rng(seed, epoch, 0x42415345)
    truth = Decimal("0.48") + Decimal(str(rng.random())) * Decimal("0.08")
    formation = Decimal(config.baseline_flow_min_floor) + Decimal(config.baseline_flow_range_floor) * Decimal(str(rng.random()))
    previous_rng = proposal_rng(seed, epoch - 1, 0x42415345)
    previous = Decimal("0.48") + Decimal(str(previous_rng.random())) * Decimal("0.08")
    previous_rng.random()
    prior_formation = Decimal(config.baseline_flow_min_floor) + Decimal(config.baseline_flow_range_floor) * Decimal(str(previous_rng.random()))
    return truth, previous, prior_formation < Decimal(1)


def _apply_baseline_attack(
    book: ExecutedBook,
    *,
    config: SimulationConfig,
    strategy: str,
    budget: Decimal,
    truth: Decimal,
    organic_contest: Decimal,
) -> None:
    if budget <= 0 or strategy != "th7_baseline_suppression":
        return
    starts = _segment_starts(config)
    correction = (book.b * LN2 + organic_contest) / Decimal(2) * Decimal(3) * Decimal(config.arbitrage_elasticity)
    book.account("manipulator", budget)
    _fund(book, "arbitrage", correction)
    for block in starts:
        attacked = execute_toward(book, "manipulator", target=Decimal("0.05"), gross_notional=budget / Decimal(len(starts)), block=block, role="manipulator")
        if attacked <= 0:
            continue
        corrected = execute_toward(book, "arbitrage", target=truth, gross_notional=min(correction / Decimal(len(starts)), attacked), block=block, role="arbitrage")
        execute_toward(book, "manipulator", target=Decimal("0.05"), gross_notional=corrected, block=block, role="manipulator")


def _summary(book: ExecutedBook, config: SimulationConfig, *, initial: Decimal = Decimal("0.5"), initial_quote: Decimal | None = None) -> BookSummary:
    return summarize_executed_book(
        book,
        initial=initial,
        initial_quote=initial_quote,
        kappa=Decimal(config.kappa),
        interval=config.observation_interval,
        decision_window=config.decision_window,
        trailing_window=config.trailing_window,
    )


def _book_valid(summary: BookSummary, contest: Decimal, floor: Decimal, config: SimulationConfig, *, gate: bool) -> bool:
    if contest < floor or summary.stale_events:
        return False
    in_band = Decimal("0.02") <= summary.full <= Decimal("0.98")
    convergence = Decimal(config.delta_max) if in_band or not gate else Decimal("0.01")
    return abs(summary.spot - summary.full) <= convergence


def _stale_decision(stale_events: int, extended: bool) -> Decision | None:
    """04 §7: first pair event consumes the shared extension; second rejects."""
    if stale_events <= 0:
        return None
    if stale_events == 1 and not extended:
        return Decision(Outcome.EXTEND)
    return Decision(Outcome.REJECT, RejectReason.NOT_DECISION_GRADE)


def _signed_manip_floor(
    *,
    b: Decimal,
    accept_price: Decimal,
    reject_price: Decimal,
    delta: Decimal,
    contest_notional: Decimal,
    flow_cap: Decimal,
) -> tuple[Decimal, tuple[Decimal, Decimal]]:
    ceiling = Decimal("0.999")
    a0 = _clamp(accept_price, Decimal("0.001"), ceiling - delta)
    r0 = _clamp(reject_price, Decimal("0.001") + delta, ceiling)
    accept_cost = displacement_cost(b, a0, a0 + delta)
    reject_cost = displacement_cost(b, Decimal(1) - r0, Decimal(1) - (r0 - delta))
    held = min(Decimal(contest_notional), Decimal(flow_cap) * Decimal(2) * b) * delta
    return round_down(accept_cost + reject_cost + held), (round_down(accept_cost), round_down(reject_cost))


def _evaluate(
    *,
    proposal: Proposal,
    config: SimulationConfig,
    prize: Decimal | None,
    v_min: Decimal,
    accept: BookSummary,
    reject: BookSummary,
    contest_accept: Decimal,
    contest_reject: Decimal,
    baseline_full: Decimal,
    baseline_trailing: Decimal,
    gate_books: tuple[GateBookEvidence, ...],
    delta: Decimal,
    extended: bool,
    b: Decimal,
) -> tuple[Decision, Grade, Decimal]:
    stale = _stale_decision(accept.stale_events + reject.stale_events, extended)
    grade = Grade.OK if (
        _book_valid(accept, contest_accept, v_min, config, gate=False)
        and _book_valid(reject, contest_reject, v_min, config, gate=False)
    ) else Grade.INSUFFICIENT
    liquidity = Decimal(2) * b * LN2 + min(contest_accept, contest_reject)
    if prize is None:
        return Decision(Outcome.REJECT, RejectReason.SECURITY_SIZING), Grade.INVALID, liquidity
    if stale is not None:
        return stale, Grade.INVALID, liquidity
    sigma = SIGMA_FLOORS[proposal.proposal_class]
    converged = abs(accept.spot - accept.full) <= Decimal(config.delta_max) and abs(reject.spot - reject.full) <= Decimal(config.delta_max)
    p_adopt: dict[str, Decimal] = {}
    p_reject: dict[str, Decimal] = {}
    gate_valid: dict[str, bool] = {}
    for gate in ("survival", "security"):
        rows = [row for row in gate_books if row.gate == gate]
        if rows:
            p_adopt[gate] = next(row.summary.full for row in rows if row.branch == "adopt")
            p_reject[gate] = next(row.summary.full for row in rows if row.branch == "reject")
            gate_valid[gate] = all(row.valid for row in rows)
    decision = decide(
        accept_full=accept.full,
        reject_full_effective=max(reject.full, baseline_full - sigma),
        delta=delta,
        accept_trailing=accept.trailing,
        reject_trailing_effective=max(reject.trailing, baseline_trailing - sigma),
        converged=converged,
        extended=extended,
        requires_gate_markets=_requires_gate(proposal),
        gate_valid=gate_valid,
        p_adopt=p_adopt,
        p_reject=p_reject,
        p_max={gate: GATE_P_MAX for gate in (Gate.SURVIVAL, Gate.SECURITY)},
        eps={gate: GATE_EPS for gate in (Gate.SURVIVAL, Gate.SECURITY)},
        welfare_grade=grade,
        proposal_class=_class_enum(proposal.proposal_class),
        ask=proposal.ask,
        envelope_value=proposal.envelope or Decimal(0),
        spendable_nav=(
            proposal.nav
            if proposal.upgrade_payload or proposal.proposal_class not in ("code", "meta")
            else Decimal(0)
        ),
        measured_liquidity=liquidity,
        published_flow_per_day=None,
        decision_window=config.decision_window,
        attestation_ok=True,
        queue_time_ok=True,
    )
    return decision, grade, liquidity


def _gate_books(
    proposal: Proposal,
    *,
    seed: int,
    config: SimulationConfig,
    v_min: Decimal,
    extension: bool,
) -> tuple[tuple[ExecutedBook, ...], tuple[GateBookEvidence, ...]]:
    if not _requires_gate(proposal):
        return (), ()
    truths = {
        ("survival", "adopt"): proposal.survival_risk_adopt,
        ("survival", "reject"): proposal.survival_risk_reject,
        ("security", "adopt"): proposal.security_risk_adopt,
        ("security", "reject"): proposal.security_risk_reject,
    }
    books = []
    evidence = []
    floor = GATE_V_MIN_FRACTION * v_min
    formation = _formation_ratio(proposal, seed ^ 0x47415445, config)
    for index, ((gate, branch), truth) in enumerate(truths.items()):
        book = ExecutedBook(f"gate:{gate}:{branch}", GATE_B)
        _execute_organic_window(
            book,
            truth=truth,
            desired_contest=floor * formation,
            seed=seed,
            proposal_id=proposal.proposal_id,
            salt=0x47415445 + index,
            config=config,
            extension=extension,
        )
        summary = _summary(book, config, initial=Decimal("0.02"))
        contest = book.contest_notional()
        valid = _book_valid(summary, contest, floor, config, gate=True)
        books.append(book)
        evidence.append(GateBookEvidence(gate, branch, contest, valid, summary))
    return tuple(books), tuple(evidence)


def _extend_gate_books(
    proposal: Proposal,
    books: tuple[ExecutedBook, ...],
    prior: tuple[GateBookEvidence, ...],
    *,
    seed: int,
    config: SimulationConfig,
    v_min: Decimal,
) -> tuple[tuple[ExecutedBook, ...], tuple[GateBookEvidence, ...]]:
    """Run the shared extension without resetting gate q or participants."""
    if not books:
        return (), ()
    truths = {
        ("survival", "adopt"): proposal.survival_risk_adopt,
        ("survival", "reject"): proposal.survival_risk_reject,
        ("security", "adopt"): proposal.security_risk_adopt,
        ("security", "reject"): proposal.security_risk_reject,
    }
    floor = GATE_V_MIN_FRACTION * v_min
    formation = _formation_ratio(proposal, seed ^ 0x47415445, config)
    evidence = []
    for index, (book, previous) in enumerate(zip(books, prior)):
        initial = previous.summary.observed_close
        initial_quote = previous.summary.spot
        book.events.clear()
        _execute_organic_window(
            book,
            truth=truths[(previous.gate, previous.branch)],
            desired_contest=(
                floor * formation * Decimal(config.extension_flow_multiplier)
            ),
            seed=seed,
            proposal_id=proposal.proposal_id,
            salt=0x47415445 + index,
            config=config,
            extension=True,
        )
        summary = _summary(
            book,
            config,
            initial=initial,
            initial_quote=initial_quote,
        )
        contest = book.contest_notional()
        evidence.append(
            GateBookEvidence(
                previous.gate,
                previous.branch,
                contest,
                _book_valid(summary, contest, floor, config, gate=True),
                summary,
            )
        )
    return books, tuple(evidence)


def _role_notional(books: tuple[ExecutedBook, ...], role: str) -> Decimal:
    return sum((book.contest_notional({role}) for book in books), Decimal(0))


def _attacker_loss(books: tuple[ExecutedBook, ...]) -> Decimal:
    initial = Decimal(0)
    liquidation = Decimal(0)
    for book in books:
        if "manipulator" in book.participants:
            participant = book.participants["manipulator"]
            initial += participant.initial_cash
            liquidation += book.liquidation_value("manipulator")
    return max(Decimal(0), initial - liquidation)


def simulate_proposal(
    proposal: Proposal,
    *,
    seed: int,
    config: SimulationConfig,
    budget_multiple: Decimal | None = None,
    absolute_direct_budget: Decimal | None = None,
    absolute_baseline_budget: Decimal | None = None,
    delta_multiplier: Decimal = Decimal(1),
    delta_override: Decimal | None = None,
    b_multiplier: Decimal = Decimal(1),
    baseline_b_multiplier: Decimal = Decimal(1),
    flow_cap: Decimal | None = None,
) -> SimulationResult:
    budget_multiple = Decimal(config.primary_manipulator_budget_multiple if budget_multiple is None else budget_multiple)
    flow_cap = Decimal(config.diagnostic_probe_flow_cap if flow_cap is None else flow_cap)
    prize = None if proposal.envelope is None and proposal.proposal_class in ("param", "code", "meta") else in_cap_prize(
        proposal.proposal_class,
        ask=proposal.ask,
        envelope=proposal.envelope or Decimal(0),
        spendable_nav=proposal.nav,
        upgrade_payload=proposal.upgrade_payload,
    )
    sizing_prize = prize or Decimal(0)
    b = pol_b(proposal.proposal_class, sizing_prize) * Decimal(b_multiplier)
    baseline_b = BASELINE_B * Decimal(baseline_b_multiplier)
    v_min = dec_v_min(proposal.proposal_class, sizing_prize)
    delta = Decimal(delta_override) if delta_override is not None else decision_delta(proposal.proposal_class, sizing_prize) * Decimal(delta_multiplier)
    delta = min(delta, Decimal("0.10"))
    strategy = _strategy_for(proposal.proposal_id, config)
    accept_truth, reject_truth, _ = _price_targets(proposal, seed, sizing_prize)
    formation = _formation_ratio(proposal, seed, config) * Decimal(b_multiplier).sqrt()
    if strategy == "th4_thin_capture":
        formation = min(formation, Decimal("0.97"))
    rng = proposal_rng(seed, proposal.proposal_id, 0x494D42414C)
    imbalance = Decimal("0.94") + Decimal("0.12") * Decimal(str(rng.random()))
    desired_accept = v_min * formation * imbalance
    desired_reject = v_min * formation * (Decimal(2) - imbalance)
    accept_book = ExecutedBook("accept", b)
    reject_book = ExecutedBook("reject", b)
    _execute_organic_window(accept_book, truth=accept_truth, desired_contest=desired_accept, seed=seed, proposal_id=proposal.proposal_id, salt=0x414343, config=config, extension=False)
    _execute_organic_window(reject_book, truth=reject_truth, desired_contest=desired_reject, seed=seed, proposal_id=proposal.proposal_id, salt=0x52454A, config=config, extension=False)
    initial_contest_accept = accept_book.contest_notional()
    initial_contest_reject = reject_book.contest_notional()
    default_budget = budget_multiple * Decimal(3) * sizing_prize
    direct_budget = default_budget if absolute_direct_budget is None else Decimal(absolute_direct_budget)
    baseline_budget = default_budget if absolute_baseline_budget is None else Decimal(absolute_baseline_budget)
    _apply_pair_attack(
        accept_book,
        reject_book,
        proposal=proposal,
        config=config,
        strategy=strategy,
        budget=direct_budget,
        accept_truth=accept_truth,
        reject_truth=reject_truth,
        organic_pair_contest=min(initial_contest_accept, initial_contest_reject),
    )
    baseline_truth, previous_baseline, prior_carried = _epoch_baseline_truth(seed, proposal.proposal_id, config)
    baseline_book = ExecutedBook("baseline", baseline_b)
    baseline_floor = Decimal(config.baseline_contest_floor)
    baseline_rng = proposal_rng(seed, proposal.proposal_id // config.epoch_slate_size, 0x42464C4F57)
    baseline_formation = Decimal(config.baseline_flow_min_floor) + Decimal(config.baseline_flow_range_floor) * Decimal(str(baseline_rng.random()))
    _execute_organic_window(baseline_book, truth=baseline_truth, desired_contest=baseline_floor * baseline_formation, seed=seed, proposal_id=proposal.proposal_id // config.epoch_slate_size, salt=0x424153, config=config, extension=False)
    _apply_baseline_attack(baseline_book, config=config, strategy=strategy, budget=baseline_budget, truth=baseline_truth, organic_contest=baseline_book.contest_notional())
    gate_ledgers, gate_evidence = _gate_books(proposal, seed=seed, config=config, v_min=v_min, extension=False)
    accept = _summary(accept_book, config)
    reject = _summary(reject_book, config)
    baseline = _summary(baseline_book, config)
    baseline_contest = baseline_book.contest_notional()
    baseline_valid = _book_valid(baseline, baseline_contest, baseline_floor, config, gate=False)
    baseline_full = baseline.full if baseline_valid else previous_baseline
    baseline_trailing = baseline.trailing if baseline_valid else previous_baseline
    baseline_carried = not baseline_valid
    initial, grade, liquidity = _evaluate(
        proposal=proposal,
        config=config,
        prize=prize,
        v_min=v_min,
        accept=accept,
        reject=reject,
        contest_accept=accept_book.contest_notional(),
        contest_reject=reject_book.contest_notional(),
        baseline_full=baseline_full,
        baseline_trailing=baseline_trailing,
        gate_books=gate_evidence,
        delta=delta,
        extended=False,
        b=b,
    )
    final = initial
    extended = initial.outcome is Outcome.EXTEND
    if extended:
        # The shifted decision window keeps q/balances/positions, but contest
        # telemetry and TWAP cover only the actual three-day extension window.
        starts = (accept.observed_close, reject.observed_close, baseline.observed_close)
        raw_starts = (accept.spot, reject.spot, baseline.spot)
        for book in (accept_book, reject_book, baseline_book):
            book.events.clear()
        multiplier = Decimal(config.extension_flow_multiplier)
        _execute_organic_window(accept_book, truth=accept_truth, desired_contest=desired_accept * multiplier, seed=seed, proposal_id=proposal.proposal_id, salt=0x414343, config=config, extension=True)
        _execute_organic_window(reject_book, truth=reject_truth, desired_contest=desired_reject * multiplier, seed=seed, proposal_id=proposal.proposal_id, salt=0x52454A, config=config, extension=True)
        _execute_organic_window(baseline_book, truth=baseline_truth, desired_contest=baseline_floor * baseline_formation * multiplier, seed=seed, proposal_id=proposal.proposal_id // config.epoch_slate_size, salt=0x424153, config=config, extension=True)
        remaining_direct = sum(
            (
                book.participants.get("manipulator").cash
                if book.participants.get("manipulator") is not None
                else Decimal(0)
                for book in (accept_book, reject_book)
            ),
            Decimal(0),
        )
        _apply_pair_attack(
            accept_book,
            reject_book,
            proposal=proposal,
            config=config,
            strategy=strategy,
            budget=remaining_direct,
            accept_truth=accept_truth,
            reject_truth=reject_truth,
            organic_pair_contest=min(
                accept_book.contest_notional(), reject_book.contest_notional()
            ),
            reuse_attacker_balance=True,
        )
        accept = _summary(accept_book, config, initial=starts[0], initial_quote=raw_starts[0])
        reject = _summary(reject_book, config, initial=starts[1], initial_quote=raw_starts[1])
        baseline = _summary(baseline_book, config, initial=starts[2], initial_quote=raw_starts[2])
        baseline_contest = baseline_book.contest_notional()
        baseline_valid = _book_valid(baseline, baseline_contest, baseline_floor, config, gate=False)
        baseline_full = baseline.full if baseline_valid else previous_baseline
        baseline_trailing = baseline.trailing if baseline_valid else previous_baseline
        baseline_carried = not baseline_valid
        gate_ledgers, gate_evidence = _extend_gate_books(
            proposal,
            gate_ledgers,
            gate_evidence,
            seed=seed,
            config=config,
            v_min=v_min,
        )
        final, grade, liquidity = _evaluate(
            proposal=proposal,
            config=config,
            prize=prize,
            v_min=v_min,
            accept=accept,
            reject=reject,
            contest_accept=accept_book.contest_notional(),
            contest_reject=reject_book.contest_notional(),
            baseline_full=baseline_full,
            baseline_trailing=baseline_trailing,
            gate_books=gate_evidence,
            delta=delta,
            extended=True,
            b=b,
        )
    contest_accept = accept_book.contest_notional()
    contest_reject = reject_book.contest_notional()
    attack_cost = attack_cost_hat(liquidity, decision_window=config.decision_window)
    manip_floor, c_disp = _signed_manip_floor(
        b=b,
        accept_price=accept.full,
        reject_price=reject.full,
        delta=delta,
        contest_notional=contest_accept + contest_reject,
        flow_cap=flow_cap,
    )
    all_books = (accept_book, reject_book, baseline_book) + gate_ledgers
    direct_books = (accept_book, reject_book)
    manipulator_flow = _role_notional(all_books, "manipulator")
    manipulator_deployed = sum(
        (event.cost + event.fee for book in all_books for event in book.events if event.role == "manipulator" and event.direction == "buy"),
        Decimal(0),
    )
    displacement = abs(accept.spot - accept_truth) + abs(reject.spot - reject_truth)
    return SimulationResult(
        proposal=proposal,
        outcome=final.outcome.value,
        reason=None if final.reason is None else final.reason.value,
        initial_outcome=initial.outcome.value,
        extended=extended,
        strategy=strategy,
        budget_multiple=budget_multiple,
        delta=delta,
        b=b,
        baseline_b=baseline_b,
        prize=prize,
        v_min=v_min,
        contest_accept=contest_accept,
        contest_reject=contest_reject,
        initial_contest_accept=initial_contest_accept,
        initial_contest_reject=initial_contest_reject,
        baseline_contest=baseline_contest,
        accept=accept,
        reject=reject,
        baseline=baseline,
        gate_books=gate_evidence,
        baseline_valid=baseline_valid,
        baseline_carried=baseline_carried,
        attack_cost=attack_cost,
        measured_liquidity=liquidity,
        manip_floor=manip_floor,
        manip_c_disp=c_disp,
        realized_manipulation_spend=_attacker_loss(all_books),
        manipulator_gross_deployed=manipulator_deployed,
        manipulator_displacement=displacement,
        informed_flow=_role_notional(all_books, "informed"),
        noise_flow=_role_notional(all_books, "noise"),
        arbitrage_flow=_role_notional(all_books, "arbitrage"),
        manipulator_flow=manipulator_flow,
        welfare_grade=grade.value,
        ledger_conservation_error=max(
            (
                abs(book.settlement_conservation_error(side))
                for book in all_books
                for side in ("long", "short")
            ),
            default=Decimal(0),
        ),
    )
