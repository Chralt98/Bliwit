from __future__ import annotations
from dataclasses import dataclass
from decimal import Decimal, ROUND_FLOOR, localcontext

WORK_PREC = 100
STALE_GAP_BLOCKS = 50  # 04 §7.
BASE_UNIT = Decimal("0.000001")  # 02 §8: USDC has 6 decimals (noi_t grid, 04 §7a).


@dataclass(frozen=True)
class Observation:
    block: int
    value: Decimal
    cumulative: Decimal


class TwapAccumulator:
    """Backward-weighted slew-capped accumulator from 04 §7."""

    def __init__(
        self,
        initial: Decimal,
        kappa: Decimal = Decimal("0.005"),
        obs_interval: int = 10,
    ):
        if obs_interval <= 0:
            raise ValueError("obs_interval must be positive")
        self.last = Decimal(initial)
        self.kappa = Decimal(kappa)
        self.obs_interval = obs_interval
        self.stale_events = 0
        self.points = [Observation(0, self.last, Decimal(0))]

    def observe(self, block: int, previous_quote: Decimal) -> Decimal:
        previous = self.points[-1]
        if block <= previous.block:
            raise ValueError("block must increase")
        elapsed = block - previous.block
        if elapsed > STALE_GAP_BLOCKS:
            self.stale_events += 1
        with localcontext() as ctx:
            ctx.prec = WORK_PREC
            k = max(1, elapsed // self.obs_interval)
            lo = self.last * (Decimal(1) - self.kappa) ** k
            hi = self.last * (Decimal(1) + self.kappa) ** k
            self.last = min(max(Decimal(previous_quote), lo), hi)
            cumulative = previous.cumulative + self.last * elapsed
            self.points.append(Observation(block, self.last, cumulative))
            return self.last

    def cumulative_at(self, block: int) -> Decimal:
        """A(block), where o_i weights the backward interval (t_{i-1}, t_i]."""
        if block < self.points[0].block or block > self.points[-1].block:
            raise ValueError("block is outside the recorded accumulator")
        if block == self.points[0].block:
            return self.points[0].cumulative
        with localcontext() as ctx:
            ctx.prec = WORK_PREC
            for left, right in zip(self.points, self.points[1:]):
                if block <= right.block:
                    return left.cumulative + right.value * (block - left.block)
        raise AssertionError("unreachable")

    def mean(self, start: int, end: int) -> Decimal:
        """TWAP = (A(end)-A(start))/(end-start), per 04 §7."""
        if end <= start:
            raise ValueError("bad window")
        with localcontext() as ctx:
            ctx.prec = WORK_PREC
            return (self.cumulative_at(end) - self.cumulative_at(start)) / Decimal(
                end - start
            )


def _d(value) -> Decimal:
    return value if isinstance(value, Decimal) else Decimal(str(value))


def marked_open_interest(
    q_long,
    q_short,
    price_long,
    q_pol_long=Decimal(0),
    q_pol_short=Decimal(0),
) -> Decimal:
    """04 §7a at-risk capital: mark only the unmatched outcome leg.

    `q_side` is the maker's net sold quantity per side (the LMSR cost-function
    state); `q_pol_side` is the recorded protocol-seeded position (04 §10 — the
    storage the "POL undisturbed" check reads). After the POL exclusion, equal
    LONG/SHORT quantities are a complete set that pays par for every settlement
    and therefore carries no settlement risk. Strip that matched quantity first,
    then mark only the excess LONG at `price_long` or excess SHORT at its
    complement. Rounds DOWN on the USDC base-unit grid: the measure feeds
    validity floors and the step-9 certificate, so under-counting is the
    conservative direction.
    """
    with localcontext() as ctx:
        ctx.prec = WORK_PREC
        price = _d(price_long)
        if not Decimal(0) <= price <= Decimal(1):
            raise ValueError("price must be in [0, 1]")
        long = max(_d(q_long) - _d(q_pol_long), Decimal(0))
        short = max(_d(q_short) - _d(q_pol_short), Decimal(0))
        noi = (
            (long - short) * price
            if long >= short
            else (short - long) * (Decimal(1) - price)
        )
        return noi.quantize(BASE_UNIT, rounding=ROUND_FLOOR)


class ContestCapitalAccumulator:
    """04 §7a at-risk contest-capital accumulator: N += noi_t*Δblocks.

    Mirrors `TwapAccumulator`'s discipline exactly: the caller supplies the
    PREVIOUS block's stored q and quote for each recorded observation (a trade
    in block n can never contribute its own state to the observation recorded
    in block n); the newly recorded `noi_t` is weighted backward over the
    interval that ends at its record block; `ContestCapital(w) = (N(end) -
    N(start)) / blocks` via the same checkpoint grid. There is no slew clamp —
    κ is a price-series property; wash flow nets out by LMSR path independence
    and balanced complete-set inventory nets out because it has no settlement
    risk. N is monotone non-decreasing (noi_t >= 0).
    """

    def __init__(self, q_pol_long=Decimal(0), q_pol_short=Decimal(0)):
        self.q_pol_long = _d(q_pol_long)
        self.q_pol_short = _d(q_pol_short)
        self.points = [Observation(0, Decimal(0), Decimal(0))]

    def observe(self, block: int, q_long, q_short, price_long) -> Decimal:
        previous = self.points[-1]
        if block <= previous.block:
            raise ValueError("block must increase")
        elapsed = block - previous.block
        with localcontext() as ctx:
            ctx.prec = WORK_PREC
            noi = marked_open_interest(
                q_long, q_short, price_long, self.q_pol_long, self.q_pol_short
            )
            cumulative = previous.cumulative + noi * elapsed
            self.points.append(Observation(block, noi, cumulative))
            return noi

    def cumulative_at(self, block: int) -> Decimal:
        """N(block), where noi_i weights the backward interval (t_{i-1}, t_i]."""
        if block < self.points[0].block or block > self.points[-1].block:
            raise ValueError("block is outside the recorded accumulator")
        if block == self.points[0].block:
            return self.points[0].cumulative
        with localcontext() as ctx:
            ctx.prec = WORK_PREC
            for left, right in zip(self.points, self.points[1:]):
                if block <= right.block:
                    return left.cumulative + right.value * (block - left.block)
        raise AssertionError("unreachable")

    def mean(self, start: int, end: int) -> Decimal:
        """ContestCapital(w) = (N(end)-N(start))/blocks, per 04 §7a."""
        if end <= start:
            raise ValueError("bad window")
        with localcontext() as ctx:
            ctx.prec = WORK_PREC
            return (self.cumulative_at(end) - self.cumulative_at(start)) / Decimal(
                end - start
            )
