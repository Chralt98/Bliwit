from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal

from bleavit_reference_model.lmsr import displacement_cost


@dataclass(frozen=True)
class PriceMove:
    price: Decimal
    spend: Decimal


def arbitrage_flow(
    liquidity: Decimal,
    days: Decimal = Decimal(1),
    elasticity: Decimal = Decimal(1),
) -> Decimal:
    """A-2 sensitivity: corrective flow L/2 per day at elasticity 1."""
    liquidity = Decimal(liquidity)
    days = Decimal(days)
    elasticity = Decimal(elasticity)
    if liquidity < 0 or days < 0 or elasticity < 0:
        raise ValueError("arbitrage inputs must be non-negative")
    return liquidity / Decimal(2) * days * elasticity


def _move_cost(
    b: Decimal, current_price: Decimal, target_price: Decimal
) -> Decimal:
    if target_price >= current_price:
        return displacement_cost(b, current_price, target_price)
    return displacement_cost(
        b,
        Decimal(1) - current_price,
        Decimal(1) - target_price,
    )


def budgeted_move_price(
    *,
    b: Decimal,
    current_price: Decimal,
    target_price: Decimal,
    budget: Decimal,
    liquidity: Decimal | None = None,
    days: Decimal = Decimal(0),
    elasticity: Decimal = Decimal(1),
) -> PriceMove:
    """Move toward a target using normative LMSR displacement cost.

    Optional liquidity reserves A-2 corrective capital before the displacement;
    this is used for sustained manipulators, while informed-trader sanity tests
    exercise the pure LMSR move.
    """
    b = Decimal(b)
    current = Decimal(current_price)
    target = Decimal(target_price)
    budget = Decimal(budget)
    if b <= 0 or budget < 0:
        raise ValueError("b must be positive and budget non-negative")
    if not Decimal("0.001") <= current <= Decimal("0.999"):
        raise ValueError("current price outside quoting clamp")
    target = min(max(target, Decimal("0.001")), Decimal("0.999"))
    reserve = (
        Decimal(0)
        if liquidity is None
        else arbitrage_flow(Decimal(liquidity), Decimal(days), Decimal(elasticity))
    )
    if current == target or budget <= reserve:
        return PriceMove(current, Decimal(0))
    available = budget - reserve
    full_cost = _move_cost(b, current, target)
    if full_cost <= available:
        return PriceMove(target, reserve + full_cost)

    lo, hi = (
        (current, target) if target > current else (target, current)
    )
    rising = target > current
    for _ in range(96):
        mid = (lo + hi) / Decimal(2)
        cost = _move_cost(b, current, mid)
        if rising:
            if cost <= available:
                lo = mid
            else:
                hi = mid
        else:
            if cost <= available:
                hi = mid
            else:
                lo = mid
    price = lo if rising else hi
    spend = reserve + _move_cost(b, current, price)
    return PriceMove(price, spend)
