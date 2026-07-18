from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal, localcontext
import random

from bleavit_reference_model.treasury import (
    decision_delta,
    in_cap_prize,
    p_ref,
)

from .config import CLASSES


MASK64 = (1 << 64) - 1
NAV_FLOORS = {
    "param": Decimal("1848400"),
    "treasury": Decimal("7393600"),
    "code": Decimal("13862944"),
    "meta": Decimal("21256533"),
}
TREASURY_SMALL_NAV_FLOOR = Decimal("4620981")
NAV_REGIMES = (
    ("floor", Decimal("1.00")),
    ("growth", Decimal("1.75")),
    ("mature", Decimal("3.50")),
    ("large", Decimal("7.50")),
)


def mix64(value: int) -> int:
    value = (value + 0x9E3779B97F4A7C15) & MASK64
    value = ((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9) & MASK64
    value = ((value ^ (value >> 27)) * 0x94D049BB133111EB) & MASK64
    return (value ^ (value >> 31)) & MASK64


def proposal_rng(seed: int, proposal_id: int, salt: int = 0) -> random.Random:
    combined = seed ^ mix64(proposal_id + 1) ^ mix64(salt + 0x53494D)
    return random.Random(mix64(combined))


def _class_for(seed: int, proposal_id: int) -> str:
    block, offset = divmod(proposal_id, len(CLASSES))
    values = list(CLASSES)
    proposal_rng(seed, block, 0xC1A55).shuffle(values)
    return values[offset]


def _centered_irwin_hall(rng: random.Random, terms: int = 6) -> Decimal:
    total = sum((Decimal(str(rng.random())) for _ in range(terms)), Decimal(0))
    return total - Decimal(terms) / Decimal(2)


@dataclass(frozen=True)
class Proposal:
    proposal_id: int
    proposal_class: str
    regime: str
    nav: Decimal
    ask: Decimal
    envelope: Decimal | None
    true_effect: Decimal
    harmful: bool
    upgrade_payload: bool
    effect_stratum: str
    formation_regime: str
    gate_exposure: str
    survival_risk_adopt: Decimal
    survival_risk_reject: Decimal
    security_risk_adopt: Decimal
    security_risk_reject: Decimal

    def evidence(self) -> dict:
        return {
            "ask": str(self.ask),
            "class": self.proposal_class,
            "envelope": None if self.envelope is None else str(self.envelope),
            "harmful": self.harmful,
            "nav": str(self.nav),
            "proposal_id": self.proposal_id,
            "regime": self.regime,
            "true_effect": str(self.true_effect),
            "upgrade_payload": self.upgrade_payload,
            "effect_stratum": self.effect_stratum,
            "formation_regime": self.formation_regime,
            "gate_exposure": self.gate_exposure,
            "gate_truth": {
                "security_adopt": str(self.security_risk_adopt),
                "security_reject": str(self.security_risk_reject),
                "survival_adopt": str(self.survival_risk_adopt),
                "survival_reject": str(self.survival_risk_reject),
            },
        }


def _weighted_row(rng: random.Random, rows, weight_index: int):
    draw = Decimal(str(rng.random()))
    cumulative = Decimal(0)
    for row in rows:
        cumulative += Decimal(row[weight_index])
        if draw < cumulative:
            return row
    return rows[-1]


def generate_proposal(seed: int, proposal_id: int) -> Proposal:
    return generate_proposal_with_config(seed, proposal_id, None)


def generate_proposal_with_config(
    seed: int,
    proposal_id: int,
    config,
) -> Proposal:
    proposal_class = _class_for(seed, proposal_id)
    rng = proposal_rng(seed, proposal_id, 0x50524F50)
    regime, multiplier = NAV_REGIMES[rng.randrange(len(NAV_REGIMES))]
    jitter_min = Decimal("1.00") if config is None else Decimal(config.nav_jitter_min)
    jitter_max = Decimal("1.05") if config is None else Decimal(config.nav_jitter_max)
    jitter = jitter_min + Decimal(str(rng.random())) * (jitter_max - jitter_min)
    with localcontext() as ctx:
        ctx.prec = 50
        base_floor = (
            TREASURY_SMALL_NAV_FLOOR
            if proposal_class == "treasury" and regime == "floor"
            else NAV_FLOORS[proposal_class]
        )
        nav = min(
            Decimal("100000000"),
            (base_floor * multiplier * jitter).quantize(
                Decimal("0.01")
            ),
        )

    harmful = rng.random() < 0.5
    reference_prize = p_ref(proposal_class)
    if proposal_class == "param":
        ask = Decimal(0)
        envelope = reference_prize * (
            Decimal("0.35") + Decimal("0.60") * Decimal(str(rng.random()))
        )
    elif proposal_class == "treasury":
        max_fraction = Decimal("0.01") if regime == "floor" else Decimal("0.05")
        ask = nav * (
            Decimal("0.0025")
            + (max_fraction - Decimal("0.0025"))
            * Decimal(str(rng.random())) ** 2
        )
        envelope = Decimal(0)
    else:
        ask = nav * Decimal("0.01") * Decimal(str(rng.random()))
        envelope = reference_prize * (
            Decimal("0.35") + Decimal("0.70") * Decimal(str(rng.random()))
        )

    upgrade_fraction = Decimal("0.50") if config is None else Decimal(
        config.upgrade_payload_fraction
    )
    upgrade_payload = proposal_class in ("code", "meta") and (
        Decimal(str(rng.random())) < upgrade_fraction
    )
    prize = in_cap_prize(
        proposal_class,
        ask=ask,
        envelope=envelope,
        spendable_nav=nav,
        upgrade_payload=upgrade_payload,
    )
    effective_delta = decision_delta(proposal_class, prize)
    effect_rows = (
        (("sub_half_delta", "0.00", "0.50", "0.15"),
         ("half_to_one_delta", "0.50", "1.00", "0.15"),
         ("one_to_two_delta", "1.00", "2.00", "0.40"),
         ("two_to_three_delta", "2.00", "3.00", "0.30"))
        if config is None else config.effect_strata
    )
    effect_row = _weighted_row(rng, effect_rows, 3)
    lo, hi = Decimal(effect_row[1]), Decimal(effect_row[2])
    magnitude = effective_delta * (
        lo + (hi - lo) * Decimal(str(rng.random()))
    )
    true_effect = -magnitude if harmful else magnitude

    formation_rows = (
        (("thin", "0.70", "0.97", "0.25"),
         ("marginal", "0.97", "1.08", "0.35"),
         ("deep", "1.08", "1.44", "0.40"))
        if config is None else config.formation_strata
    )
    formation_row = _weighted_row(rng, formation_rows, 3)

    gate_exposure = "gate" if proposal_class in ("code", "meta") or (
        proposal_class == "treasury" and ask > nav * Decimal("0.01")
    ) else "no_gate"
    correlation = Decimal("0.75") if config is None else Decimal(config.gate_harm_correlation)
    latent = Decimal(str(rng.random()))
    elevated = harmful and latent < correlation
    reject_base = Decimal("0.010") + Decimal("0.020") * Decimal(str(rng.random()))
    survival_adopt = reject_base + (
        Decimal("0.055") + Decimal("0.035") * Decimal(str(rng.random()))
        if elevated else Decimal("0.004") * Decimal(str(rng.random()))
    )
    security_reject = Decimal("0.010") + Decimal("0.020") * Decimal(str(rng.random()))
    security_elevated = harmful and Decimal(str(rng.random())) < correlation
    security_adopt = security_reject + (
        Decimal("0.035") + Decimal("0.035") * Decimal(str(rng.random()))
        if security_elevated else Decimal("0.004") * Decimal(str(rng.random()))
    )

    return Proposal(
        proposal_id=proposal_id,
        proposal_class=proposal_class,
        regime=regime,
        nav=nav,
        ask=ask.quantize(Decimal("0.000001")),
        envelope=envelope.quantize(Decimal("0.000001")),
        true_effect=true_effect,
        harmful=harmful,
        upgrade_payload=upgrade_payload,
        effect_stratum=effect_row[0],
        formation_regime=formation_row[0],
        gate_exposure=gate_exposure,
        survival_risk_adopt=min(survival_adopt, Decimal("0.20")),
        survival_risk_reject=reject_base,
        security_risk_adopt=min(security_adopt, Decimal("0.20")),
        security_risk_reject=security_reject,
    )


def persistent_belief_error(
    seed: int, proposal: Proposal, scale: Decimal = Decimal("0.45")
) -> Decimal:
    rng = proposal_rng(seed, proposal.proposal_id, 0xB3113F)
    prize = in_cap_prize(
        proposal.proposal_class,
        ask=proposal.ask,
        envelope=proposal.envelope or Decimal(0),
        spendable_nav=proposal.nav,
        upgrade_payload=proposal.upgrade_payload,
    )
    return (
        _centered_irwin_hall(rng)
        * decision_delta(proposal.proposal_class, prize)
        * Decimal(scale)
    )
