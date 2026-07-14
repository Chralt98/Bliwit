from dataclasses import dataclass
from decimal import Decimal

@dataclass(frozen=True)
class Observation:
    block: int
    value: Decimal

class TwapAccumulator:
    """Per-book TWAP accumulator (04 §7).

    Observations land on the `mkt.obs_interval` grid (defaults mirror 13 §1:
    interval 10 blocks, κ = 0.005/interval). The slew clamp applies per elapsed
    observation interval: over k missed intervals it widens to (1±κ)^k.
    Staleness is accounted separately (04 §7) and never alters recorded data.
    """
    def __init__(self, initial: Decimal, kappa: Decimal = Decimal("0.005"), obs_interval: int = 10):
        if obs_interval <= 0: raise ValueError("obs_interval must be positive")
        self.last = initial; self.kappa = kappa; self.obs_interval = obs_interval
        self.points = [Observation(0, initial)]
    def observe(self, block: int, previous_quote: Decimal) -> Decimal:
        if block <= self.points[-1].block: raise ValueError("block must increase")
        # Elapsed observation intervals since the last recorded point; floor is
        # the conservative (tighter-clamp) reading for off-grid inputs.
        k = max(1, (block - self.points[-1].block) // self.obs_interval)
        lo = self.last * (Decimal(1) - self.kappa) ** k
        hi = self.last * (Decimal(1) + self.kappa) ** k
        self.last = min(max(previous_quote, lo), hi)
        self.points.append(Observation(block, self.last)); return self.last
    def mean(self, start: int, end: int) -> Decimal:
        if end <= start: raise ValueError("bad window")
        total = Decimal(0); pts = self.points
        for a, b in zip(pts, pts[1:]):
            s = max(start, a.block); e = min(end, b.block)
            if e > s: total += a.value * (e - s)
        if end > pts[-1].block and pts[-1].block < end: total += pts[-1].value * (end - max(start, pts[-1].block))
        return total / Decimal(end - start)
