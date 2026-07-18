from __future__ import annotations

from dataclasses import asdict, dataclass
from decimal import Decimal
import hashlib
import json
from pathlib import Path
import sys

from bleavit_reference_model.treasury import (
    BASELINE_B,
    B_FLOORS,
    DELTA_FLOORS,
    V_MIN_FLOORS,
)

DEFAULT_SEED = 0xB1EA_1504_0000_0001
CLASSES = ("param", "treasury", "code", "meta")
DECISION_WINDOW = 43_200
TRAILING_WINDOW = 14_400
OBS_INTERVAL = 10
KAPPA = Decimal("0.005")
DELTA_MAX = Decimal("0.05")
CAP_PROPOSAL = Decimal("0.05")
SIGMA_FLOORS = {
    "param": Decimal("0.003"),
    "treasury": Decimal("0.005"),
    "code": Decimal("0.008"),
    "meta": Decimal("0.010"),
}
GATE_V_MIN_FRACTION = Decimal("0.1")
GATE_P_MAX = Decimal("0.05")
GATE_EPS = Decimal("0.02")
TRADE_FEE = Decimal("0.003")

EFFECT_STRATA = (
    ("sub_half_delta", "0.00", "0.50", "0.15"),
    ("half_to_one_delta", "0.50", "1.00", "0.15"),
    ("one_to_two_delta", "1.00", "2.00", "0.40"),
    ("two_to_three_delta", "2.00", "3.00", "0.30"),
)
FORMATION_STRATA = (
    ("thin", "0.70", "0.97", "0.25"),
    ("marginal", "0.97", "1.08", "0.35"),
    ("deep", "1.08", "1.44", "0.40"),
)
ATTACK_STRATEGY_MIX = (
    ("th1_displace_hold", "0.25"),
    ("th2_late_spike", "0.15"),
    ("th4_thin_capture", "0.25"),
    ("th6_belief_capture", "0.20"),
    ("th7_baseline_suppression", "0.15"),
)


@dataclass(frozen=True)
class SimulationConfig:
    """Phase-0 hypotheses and doc-13 inputs used by the synthetic tier."""

    proposal_count: int = 10_000
    decision_window: int = DECISION_WINDOW
    trailing_window: int = TRAILING_WINDOW
    observation_interval: int = OBS_INTERVAL
    kappa: str = str(KAPPA)
    delta_max: str = str(DELTA_MAX)
    arbitrage_elasticity: str = "1.0"
    primary_manipulator_budget_multiple: str = "0.90"
    manipulator_budget_multiples: tuple[str, ...] = (
        "0.00",
        "0.50",
        "0.90",
        "1.00",
        "1.50",
        "3.00",
    )
    delta_multipliers: tuple[str, ...] = (
        "0.00",
        "0.10",
        "0.20",
        "0.40",
        "0.60",
        "0.80",
        "1.00",
        "1.20",
    )
    pol_b_multipliers: tuple[str, ...] = (
        "0.75",
        "1.00",
        "1.25",
        "1.50",
    )
    flow_cap_quantile: str = "0.995"
    diagnostic_probe_flow_cap: str = "17"
    publication_margin: str = "0.10"
    publication_round_usdc: int = 10_000
    subsample_size: int = 24
    sensitivity_sample_per_class: int = 64
    threshold_sample_per_class: int = 8
    pol_sample_per_class: int = 125
    baseline_sensitivity_epoch_count: int = 50
    threshold_relative_tolerance: str = "0.05"
    threshold_max_budget_multiple: str = "3.00"
    class_mix_block: int = 4
    epoch_slate_size: int = 5
    nav_jitter_min: str = "1.00"
    nav_jitter_max: str = "1.05"
    belief_error_scale: str = "0.45"
    noise_flow_share: str = "0.35"
    noise_price_amplitude_delta: str = "0.12"
    thin_market_flow_multiplier: str = "0.72"
    extension_flow_multiplier: str = "1.25"
    late_spike_blocks: int = 10
    baseline_contest_floor: str = "250000"
    baseline_flow_min_floor: str = "0.82"
    baseline_flow_range_floor: str = "0.75"
    gate_harm_correlation: str = "0.75"
    upgrade_payload_fraction: str = "0.50"
    effect_strata: tuple[tuple[str, str, str, str], ...] = EFFECT_STRATA
    formation_strata: tuple[tuple[str, str, str, str], ...] = FORMATION_STRATA
    attack_strategy_mix: tuple[tuple[str, str], ...] = ATTACK_STRATEGY_MIX

    def validate(self) -> None:
        if self.proposal_count < 1:
            raise ValueError("proposal_count must be positive")
        if self.decision_window <= 0 or self.trailing_window <= 0:
            raise ValueError("decision windows must be positive")
        if self.trailing_window >= self.decision_window:
            raise ValueError("trailing window must be shorter than full window")
        if self.observation_interval <= 0:
            raise ValueError("observation interval must be positive")
        if self.decision_window % self.observation_interval:
            raise ValueError("decision window must align to observations")
        if self.trailing_window % self.observation_interval:
            raise ValueError("trailing window must align to observations")
        if self.late_spike_blocks <= 0 or self.late_spike_blocks > 50:
            raise ValueError("TH-2 spike must stay within the 50-block fresh gap")
        if self.epoch_slate_size <= 0:
            raise ValueError("epoch slate size must be positive")
        if self.sensitivity_sample_per_class <= 0:
            raise ValueError("sensitivity sample per class must be positive")
        if self.pol_sample_per_class <= 0:
            raise ValueError("POL sample per class must be positive")
        if self.baseline_sensitivity_epoch_count <= 0:
            raise ValueError("baseline sensitivity epoch count must be positive")
        for label, rows in (
            ("effect", self.effect_strata),
            ("formation", self.formation_strata),
        ):
            if sum((Decimal(row[3]) for row in rows), Decimal(0)) != 1:
                raise ValueError(f"{label} stratum weights must sum to one")
        if sum((Decimal(row[1]) for row in self.attack_strategy_mix), Decimal(0)) != 1:
            raise ValueError("attack strategy weights must sum to one")
        if not Decimal(0) <= Decimal(self.noise_flow_share) <= Decimal(1):
            raise ValueError("noise flow share must be in [0, 1]")
        if not Decimal(0) < Decimal(self.threshold_relative_tolerance) <= Decimal("0.25"):
            raise ValueError("threshold tolerance must be in (0, 0.25]")

    def canonical(self) -> dict:
        payload = asdict(self)
        for key in (
            "manipulator_budget_multiples",
            "delta_multipliers",
            "pol_b_multipliers",
            "effect_strata",
            "formation_strata",
            "attack_strategy_mix",
        ):
            payload[key] = [
                list(row) if isinstance(row, tuple) else row
                for row in payload[key]
            ]
        payload.update(
            {
                "b_floors": {key: str(B_FLOORS[key]) for key in CLASSES},
                "baseline_b": str(BASELINE_B),
                "delta_floors": {
                    key: str(DELTA_FLOORS[key]) for key in CLASSES
                },
                "sigma_floors": {
                    key: str(SIGMA_FLOORS[key]) for key in CLASSES
                },
                "v_min_floors": {
                    key: str(V_MIN_FLOORS[key]) for key in CLASSES
                },
                "gate_v_min_fraction": str(GATE_V_MIN_FRACTION),
                "gate_p_max": str(GATE_P_MAX),
                "gate_eps": str(GATE_EPS),
                "mkt_fee": str(TRADE_FEE),
                "cap_proposal": str(CAP_PROPOSAL),
                "source_model_sha256": source_model_digest(),
            }
        )
        return payload

    def digest(self) -> str:
        encoded = json.dumps(
            self.canonical(), sort_keys=True, separators=(",", ":")
        ).encode("utf-8")
        return hashlib.sha256(encoded).hexdigest()


def source_model_digest() -> str:
    """Bind evidence to simulation and every reference-model package source."""
    root = Path(__file__).resolve().parent
    digest = hashlib.sha256()
    reference = root.parents[2] / "reference-model" / "src" / "bleavit_reference_model"
    paths = [("simulation", path) for path in sorted(root.glob("*.py"))]
    paths.extend(("reference-model", path) for path in sorted(reference.glob("*.py")))
    for package, path in paths:
        digest.update(package.encode("utf-8"))
        digest.update(b"/")
        digest.update(path.name.encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def python_version_tuple() -> tuple[int, int, int, str, int]:
    info = sys.version_info
    return (info.major, info.minor, info.micro, info.releaselevel, info.serial)
