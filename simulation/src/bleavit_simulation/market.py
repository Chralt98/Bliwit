from __future__ import annotations

from dataclasses import dataclass, field
from decimal import Decimal, localcontext

from bleavit_reference_model.lmsr import (
    FEE_RATE,
    buy_delta_cost,
    ceil_base,
    displacement_for_price_move,
    floor_base,
    marginal_price_long,
    sell_delta_proceeds,
)
from bleavit_reference_model.twap import STALE_GAP_BLOCKS


WORK_PREC = 100
MIN_TRADE = Decimal(1)


def fast_lmsr_price(b: Decimal, net_q: Decimal) -> Decimal:
    """Delegating shortcut: normative LMSR pricing stays in the reference model."""
    return marginal_price_long(Decimal(b), Decimal(net_q), Decimal(0))


@dataclass(frozen=True)
class _ObservedSegment:
    start: int
    end: int
    value: Decimal
    cumulative_start: Decimal
    cumulative_end: Decimal

    def cumulative_at(self, block: int) -> Decimal:
        with localcontext() as ctx:
            ctx.prec = WORK_PREC
            return self.cumulative_start + self.value * Decimal(
                block - self.start
            )


@dataclass(frozen=True)
class _GeometricSegment:
    start: int
    end: int
    interval: int
    initial: Decimal
    target: Decimal
    factor: Decimal
    clamped_steps: int
    clamped_values: tuple[Decimal, ...]
    steps: int
    cumulative_start: Decimal
    cumulative_end: Decimal

    def _sum(self, count: int) -> Decimal:
        clamped = min(count, self.clamped_steps)
        with localcontext() as ctx:
            ctx.prec = WORK_PREC
            exact_slew = sum(self.clamped_values[:clamped], Decimal(0))
            return exact_slew + self.target * Decimal(count - clamped)

    def _observation(self, index: int) -> Decimal:
        with localcontext() as ctx:
            ctx.prec = WORK_PREC
            if index <= self.clamped_steps:
                return self.clamped_values[index - 1]
            return self.target

    def cumulative_at(self, block: int) -> Decimal:
        with localcontext() as ctx:
            ctx.prec = WORK_PREC
            elapsed = block - self.start
            complete, remainder = divmod(elapsed, self.interval)
            cumulative = (
                self.cumulative_start + self._sum(complete) * self.interval
            )
            if remainder:
                cumulative += self._observation(complete + 1) * remainder
            return cumulative


class FastTwapAccumulator:
    """04 §7 accumulator with closed-form piecewise-constant advancement.

    ``observe`` is the exact single-observation reference behavior. ``advance``
    records every scheduled cadence observation between quote-change events but
    collapses a geometric slew run to one segment.
    """

    def __init__(self, initial: Decimal, kappa: Decimal, interval: int):
        if interval <= 0:
            raise ValueError("interval must be positive")
        self.last = Decimal(initial)
        self.kappa = Decimal(kappa)
        self.interval = interval
        self.block = 0
        self.cumulative = Decimal(0)
        self.stale_events = 0
        self._segments: list[_ObservedSegment | _GeometricSegment] = []

    def observe(self, block: int, previous_quote: Decimal) -> Decimal:
        if block <= self.block:
            raise ValueError("block must increase")
        elapsed = block - self.block
        if elapsed > STALE_GAP_BLOCKS:
            self.stale_events += 1
        with localcontext() as ctx:
            ctx.prec = WORK_PREC
            steps = max(1, elapsed // self.interval)
            lo = self.last * (Decimal(1) - self.kappa) ** steps
            hi = self.last * (Decimal(1) + self.kappa) ** steps
            value = min(max(Decimal(previous_quote), lo), hi)
            end_cumulative = self.cumulative + value * elapsed
        self._segments.append(
            _ObservedSegment(
                self.block,
                block,
                value,
                self.cumulative,
                end_cumulative,
            )
        )
        self.block = block
        self.last = value
        self.cumulative = end_cumulative
        return value

    def _advance_aligned(self, steps: int, quote: Decimal) -> None:
        if steps <= 0:
            return
        quote = Decimal(quote)
        initial = self.last
        if quote == initial:
            factor = Decimal(1)
            clamped = 0
            clamped_values: tuple[Decimal, ...] = ()
            total = quote * steps
            final = quote
        else:
            rising = quote > initial
            factor = Decimal(1) + self.kappa if rising else Decimal(1) - self.kappa
            with localcontext() as ctx:
                ctx.prec = WORK_PREC
                values = []
                current = initial
                for _ in range(steps):
                    lo = current * (Decimal(1) - self.kappa)
                    hi = current * (Decimal(1) + self.kappa)
                    next_value = min(max(quote, lo), hi)
                    if next_value == quote:
                        break
                    values.append(next_value)
                    current = next_value
                clamped_values = tuple(values)
                clamped = len(clamped_values)
                total = sum(clamped_values, Decimal(0)) + quote * Decimal(
                    steps - clamped
                )
                final = clamped_values[-1] if clamped == steps else quote
        start = self.block
        end = start + steps * self.interval
        with localcontext() as ctx:
            ctx.prec = WORK_PREC
            end_cumulative = self.cumulative + total * self.interval
        self._segments.append(
            _GeometricSegment(
                start=start,
                end=end,
                interval=self.interval,
                initial=initial,
                target=quote,
                factor=factor,
                clamped_steps=clamped,
                clamped_values=clamped_values,
                steps=steps,
                cumulative_start=self.cumulative,
                cumulative_end=end_cumulative,
            )
        )
        self.block = end
        self.last = final
        self.cumulative = end_cumulative

    def advance(self, block: int, previous_quote: Decimal) -> Decimal:
        """Advance through every scheduled observation under a held quote."""
        if block <= self.block:
            raise ValueError("block must increase")
        elapsed = block - self.block
        steps, remainder = divmod(elapsed, self.interval)
        self._advance_aligned(steps, Decimal(previous_quote))
        if remainder:
            self.observe(block, Decimal(previous_quote))
        return self.last

    def cumulative_at(self, block: int) -> Decimal:
        if block < 0 or block > self.block:
            raise ValueError("block is outside the recorded accumulator")
        if block == 0:
            return Decimal(0)
        for segment in self._segments:
            if block <= segment.end:
                return segment.cumulative_at(block)
        raise AssertionError("unreachable")

    def mean(self, start: int, end: int) -> Decimal:
        if end <= start:
            raise ValueError("bad window")
        with localcontext() as ctx:
            ctx.prec = WORK_PREC
            return (self.cumulative_at(end) - self.cumulative_at(start)) / Decimal(
                end - start
            )


@dataclass(frozen=True)
class BookSummary:
    full: Decimal
    trailing: Decimal
    spot: Decimal
    observed_close: Decimal
    stale_events: int


@dataclass
class Participant:
    name: str
    initial_cash: Decimal
    cash: Decimal
    long: Decimal = Decimal(0)
    short: Decimal = Decimal(0)


@dataclass(frozen=True)
class TradeEvent:
    block: int
    sequence: int
    participant: str
    role: str
    direction: str
    side: str
    amount: Decimal
    cost: Decimal
    fee: Decimal
    p_after: Decimal
    contest: bool


@dataclass
class ExecutedBook:
    """Event-level LMSR ledger mirroring the 04 §6 trade event surface."""

    name: str
    b: Decimal
    q_long: Decimal = Decimal(0)
    q_short: Decimal = Decimal(0)
    maker_cash: Decimal = Decimal(0)
    fees: Decimal = Decimal(0)
    participants: dict[str, Participant] = field(default_factory=dict)
    events: list[TradeEvent] = field(default_factory=list)
    _price: Decimal = Decimal("0.5")

    @property
    def max_trade(self) -> Decimal:
        return self.b / Decimal(4)

    @property
    def price(self) -> Decimal:
        return self._price

    def account(self, name: str, cash: Decimal = Decimal(0)) -> Participant:
        cash = Decimal(cash)
        if name not in self.participants:
            self.participants[name] = Participant(name, cash, cash)
        elif cash:
            participant = self.participants[name]
            participant.initial_cash += cash
            participant.cash += cash
        return self.participants[name]

    def _event(
        self,
        *,
        block: int,
        participant: Participant,
        role: str,
        direction: str,
        side: str,
        amount: Decimal,
        cost: Decimal,
        fee: Decimal,
        contest: bool,
    ) -> TradeEvent:
        event = TradeEvent(
            block=block,
            sequence=len(self.events),
            participant=participant.name,
            role=role,
            direction=direction,
            side=side,
            amount=amount,
            cost=cost,
            fee=fee,
            p_after=self.price,
            contest=contest,
        )
        self.events.append(event)
        return event

    def buy(
        self,
        participant_name: str,
        side: str,
        amount: Decimal,
        *,
        block: int,
        role: str,
        contest: bool = True,
        quoted: tuple[Decimal, Decimal] | None = None,
    ) -> TradeEvent | None:
        amount = Decimal(amount)
        if amount < MIN_TRADE:
            return None
        if amount > self.max_trade:
            raise ValueError("MaxTrade exceeded")
        participant = self.account(participant_name)
        if quoted is None:
            raw_cost = buy_delta_cost(
                self.b, self.q_long, self.q_short, side, amount
            )
            cost = ceil_base(raw_cost)
            fee = ceil_base(cost * FEE_RATE)
        else:
            cost, fee = quoted
        if cost + fee > participant.cash:
            return None
        participant.cash -= cost + fee
        if side == "long":
            participant.long += amount
            self.q_long += amount
        else:
            participant.short += amount
            self.q_short += amount
        self._price = marginal_price_long(self.b, self.q_long, self.q_short)
        self.maker_cash += cost
        self.fees += fee
        return self._event(
            block=block,
            participant=participant,
            role=role,
            direction="buy",
            side=side,
            amount=amount,
            cost=cost,
            fee=fee,
            contest=contest,
        )

    def sell(
        self,
        participant_name: str,
        side: str,
        amount: Decimal,
        *,
        block: int,
        role: str,
        contest: bool = True,
    ) -> TradeEvent | None:
        amount = Decimal(amount)
        if amount < MIN_TRADE:
            return None
        if amount > self.max_trade:
            raise ValueError("MaxTrade exceeded")
        participant = self.account(participant_name)
        position = participant.long if side == "long" else participant.short
        if amount > position:
            return None
        proceeds = floor_base(
            sell_delta_proceeds(
                self.b, self.q_long, self.q_short, side, amount
            )
        )
        fee = ceil_base(proceeds * FEE_RATE)
        participant.cash += proceeds - fee
        if side == "long":
            participant.long -= amount
            self.q_long -= amount
        else:
            participant.short -= amount
            self.q_short -= amount
        self._price = marginal_price_long(self.b, self.q_long, self.q_short)
        self.maker_cash -= proceeds
        self.fees += fee
        return self._event(
            block=block,
            participant=participant,
            role=role,
            direction="sell",
            side=side,
            amount=amount,
            cost=proceeds,
            fee=fee,
            contest=contest,
        )

    def contest_notional(self, roles: set[str] | None = None) -> Decimal:
        return sum(
            (
                event.cost
                for event in self.events
                if event.contest and (roles is None or event.role in roles)
            ),
            Decimal(0),
        )

    def cash_conservation_error(self) -> Decimal:
        participant_delta = sum(
            (row.cash - row.initial_cash for row in self.participants.values()),
            Decimal(0),
        )
        return participant_delta + self.fees + self.maker_cash

    def settlement_conservation_error(self, winning_side: str) -> Decimal:
        """Economic conservation after a one-dollar winning-share settlement."""
        if winning_side not in ("long", "short"):
            raise ValueError("winning_side must be long or short")
        participant_pnl = sum(
            (
                row.cash
                - row.initial_cash
                + (row.long if winning_side == "long" else row.short)
                for row in self.participants.values()
            ),
            Decimal(0),
        )
        payout = sum(
            (
                row.long if winning_side == "long" else row.short
                for row in self.participants.values()
            ),
            Decimal(0),
        )
        maker_pnl = self.maker_cash - payout
        error = participant_pnl + self.fees + maker_pnl
        # Every cash movement is rounded to the one-micro-USDC chain base unit.
        return Decimal(0) if abs(error) < Decimal("0.000001") else error

    def liquidation_value(self, participant_name: str) -> Decimal:
        """Counterfactual close quote including fees; emits no chain trade."""
        participant = self.account(participant_name)
        q_long, q_short = self.q_long, self.q_short
        value = participant.cash
        for side, held in (("long", participant.long), ("short", participant.short)):
            remaining = held
            while remaining >= MIN_TRADE:
                amount = min(remaining, self.max_trade)
                proceeds = floor_base(
                    sell_delta_proceeds(self.b, q_long, q_short, side, amount)
                )
                fee = ceil_base(proceeds * FEE_RATE)
                value += proceeds - fee
                if side == "long":
                    q_long -= amount
                else:
                    q_short -= amount
                remaining -= amount
            # A final sub-MinTrade remainder is non-liquidatable on chain and
            # therefore conservatively valued at zero.
        return value


def _affordable_amount(
    book: ExecutedBook,
    participant: Participant,
    side: str,
    amount_cap: Decimal,
    notional_cap: Decimal,
) -> tuple[Decimal, Decimal, Decimal]:
    cap = min(book.max_trade, Decimal(amount_cap))
    if cap < MIN_TRADE or notional_cap <= 0 or participant.cash <= 0:
        return Decimal(0), Decimal(0), Decimal(0)
    cost = ceil_base(buy_delta_cost(book.b, book.q_long, book.q_short, side, cap))
    fee = ceil_base(cost * FEE_RATE)
    if cost <= notional_cap and cost + fee <= participant.cash:
        return cap, cost, fee
    with localcontext() as ctx:
        ctx.prec = WORK_PREC
        cash_cap = max(
            Decimal(0),
            (participant.cash - Decimal("0.000002")) / (Decimal(1) + FEE_RATE),
        )
        spend = min(Decimal(notional_cap), cash_cap)
        if spend <= 0:
            return Decimal(0), Decimal(0), Decimal(0)
        p_long = book.price
        p_side = p_long if side == "long" else Decimal(1) - p_long
        target_side = Decimal(1) - (Decimal(1) - p_side) * (-spend / book.b).exp()
        amount = min(
            cap,
            abs(displacement_for_price_move(book.b, p_side, target_side)),
        )
        amount = floor_base(amount)
        # Base-unit rounding can make the last fill one micro-USDC too large.
        # At most three deterministic one-unit trims are needed.
        for _ in range(3):
            if amount < MIN_TRADE:
                return Decimal(0), Decimal(0), Decimal(0)
            actual = ceil_base(
                buy_delta_cost(book.b, book.q_long, book.q_short, side, amount)
            )
            actual_fee = ceil_base(actual * FEE_RATE)
            if actual <= notional_cap and actual + actual_fee <= participant.cash:
                return amount, actual, actual_fee
            amount -= Decimal("0.000001")
    return Decimal(0), Decimal(0), Decimal(0)


def execute_toward(
    book: ExecutedBook,
    participant_name: str,
    *,
    target: Decimal,
    gross_notional: Decimal,
    block: int,
    role: str,
) -> Decimal:
    """Execute chronological MaxTrade-capped buys toward a target quote."""
    target = min(max(Decimal(target), Decimal("0.001")), Decimal("0.999"))
    remaining = Decimal(gross_notional)
    executed = Decimal(0)
    participant = book.account(participant_name)
    while remaining >= Decimal("0.000001"):
        current = book.price
        if abs(current - target) < Decimal("0.000001"):
            break
        side = "long" if current < target else "short"
        needed = abs(displacement_for_price_move(book.b, current, target))
        amount, quoted_cost, quoted_fee = _affordable_amount(
            book, participant, side, needed, remaining
        )
        if amount < MIN_TRADE:
            break
        event = book.buy(
            participant_name,
            side,
            amount,
            block=block,
            role=role,
            quoted=(quoted_cost, quoted_fee),
        )
        if event is None:
            break
        executed += event.cost
        remaining -= event.cost
    return executed


def execute_turnover(
    book: ExecutedBook,
    participant_name: str,
    *,
    gross_notional: Decimal,
    block: int,
    role: str,
    first_side: str,
) -> Decimal:
    """Execute actual paired turnover; every buy and sell is retained."""
    remaining = Decimal(gross_notional)
    executed = Decimal(0)
    participant = book.account(participant_name)
    side = first_side
    while remaining >= Decimal("0.000002") and participant.cash > 0:
        buy_budget = remaining / Decimal(2)
        amount, quoted_cost, quoted_fee = _affordable_amount(
            book, participant, side, book.max_trade, buy_budget
        )
        if amount < MIN_TRADE:
            break
        bought = book.buy(
            participant_name,
            side,
            amount,
            block=block,
            role=role,
            quoted=(quoted_cost, quoted_fee),
        )
        if bought is None:
            break
        sold = book.sell(participant_name, side, amount, block=block, role=role)
        executed += bought.cost
        remaining -= bought.cost
        if sold is not None:
            executed += sold.cost
            remaining -= sold.cost
        side = "short" if side == "long" else "long"
    return executed


def summarize_executed_book(
    book: ExecutedBook,
    *,
    initial: Decimal,
    kappa: Decimal,
    interval: int,
    decision_window: int,
    trailing_window: int,
    initial_quote: Decimal | None = None,
) -> BookSummary:
    """Closed-form TWAP between event blocks with previous-block semantics."""
    accumulator = FastTwapAccumulator(initial, kappa, interval)
    quote = Decimal(initial if initial_quote is None else initial_quote)
    by_block: dict[int, Decimal] = {}
    for event in book.events:
        by_block[event.block] = event.p_after
    for block in sorted(by_block):
        if block > accumulator.block:
            accumulator.advance(block, quote)
        quote = by_block[block]
    if accumulator.block < decision_window:
        accumulator.advance(decision_window, quote)
    return BookSummary(
        full=accumulator.mean(0, decision_window),
        trailing=accumulator.mean(decision_window - trailing_window, decision_window),
        spot=book.price,
        observed_close=accumulator.last,
        stale_events=accumulator.stale_events,
    )


def simulate_book(
    *,
    initial: Decimal,
    kappa: Decimal,
    interval: int,
    decision_window: int,
    trailing_window: int,
    path: list[tuple[int, Decimal]],
) -> BookSummary:
    accumulator = FastTwapAccumulator(initial, kappa, interval)
    for end, quote in path:
        accumulator.advance(end, quote)
    if accumulator.block != decision_window:
        raise ValueError("path must end at the decision window")
    return BookSummary(
        full=accumulator.mean(0, decision_window),
        trailing=accumulator.mean(decision_window - trailing_window, decision_window),
        spot=Decimal(path[-1][1]),
        observed_close=accumulator.last,
        stale_events=accumulator.stale_events,
    )
