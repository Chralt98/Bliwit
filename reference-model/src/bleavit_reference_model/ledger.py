from __future__ import annotations
from dataclasses import dataclass, field
from decimal import Decimal, ROUND_FLOOR, localcontext
from enum import Enum

WORK_PREC = 100
BASE_UNIT = Decimal("0.000001")  # 02 §8: one USDC base unit.


class VaultState(Enum):
    OPEN = "Open"
    RESOLVED = "Resolved"
    SCALAR_SETTLED = "ScalarSettled"
    VOIDED = "Voided"


class BaselineState(Enum):
    OPEN = "Open"
    SETTLED = "Settled"


class Branch(Enum):
    ACCEPT = "Accept"
    REJECT = "Reject"


class GateType(Enum):
    SURVIVAL = "Survival"
    SECURITY = "Security"


class ScalarSide(Enum):
    LONG = "Long"
    SHORT = "Short"


class GateSide(Enum):
    YES = "Yes"
    NO = "No"


class PositionKind(Enum):
    BRANCH_USDC = "BranchUsdc"
    LONG = "Long"
    SHORT = "Short"
    GATE_YES = "GateYes"
    GATE_NO = "GateNo"


def to_base_units(amount) -> int:
    with localcontext() as ctx:
        ctx.prec = WORK_PREC
        scaled = Decimal(str(amount)) / BASE_UNIT
        integral = scaled.to_integral_value(rounding=ROUND_FLOOR)
        if scaled != integral:
            raise ValueError("amount is below the USDC base-unit grid")
        return int(integral)


def from_base_units(amount: int) -> Decimal:
    return Decimal(amount) * BASE_UNIT


def _amount(amount: int) -> int:
    if not isinstance(amount, int) or isinstance(amount, bool) or amount < 0:
        raise ValueError("amount must be a non-negative integer base-unit balance")
    return amount


def _floor_product(amount: int, factor: Decimal) -> int:
    with localcontext() as ctx:
        ctx.prec = WORK_PREC
        return int(
            (Decimal(amount) * Decimal(factor)).to_integral_value(
                rounding=ROUND_FLOOR
            )
        )


@dataclass
class BranchSupply:
    usdc: int = 0
    scalar_sets: int = 0
    gate_sets: dict[GateType, int] = field(
        default_factory=lambda: {gate: 0 for gate in GateType}
    )
    long: int = 0
    short: int = 0
    gate_yes: dict[GateType, int] = field(
        default_factory=lambda: {gate: 0 for gate in GateType}
    )
    gate_no: dict[GateType, int] = field(
        default_factory=lambda: {gate: 0 for gate in GateType}
    )

    def identity(self) -> int:
        return self.usdc + self.scalar_sets + sum(self.gate_sets.values())


@dataclass
class Vault:
    escrowed: int = 0
    state: VaultState = VaultState.OPEN
    winner: Branch | None = None
    s: Decimal | None = None
    branches: dict[Branch, BranchSupply] = field(
        default_factory=lambda: {branch: BranchSupply() for branch in Branch}
    )
    gate_outcomes: dict[GateType, bool | None] = field(
        default_factory=lambda: {gate: None for gate in GateType}
    )
    collateral_in: int = 0
    total_payouts: int = 0
    terminal_redemptions: int = 0

    def split(self, amount: int) -> None:
        self._need(VaultState.OPEN)
        amount = _amount(amount)
        self.escrowed += amount
        self.collateral_in += amount
        for branch in Branch:
            self.branches[branch].usdc += amount
        self.check_conservation()

    def merge(self, amount: int) -> int:
        if self.state not in (
            VaultState.OPEN,
            VaultState.RESOLVED,
            VaultState.VOIDED,
        ):
            raise ValueError("wrong state")
        amount = _amount(amount)
        for branch in Branch:
            self._take(self.branches[branch], "usdc", amount)
        self._pay(amount)
        self.check_conservation()
        return amount

    def split_scalar(self, branch: Branch, amount: int) -> None:
        self._need(VaultState.OPEN)
        amount = _amount(amount)
        supply = self.branches[branch]
        self._take(supply, "usdc", amount)
        supply.scalar_sets += amount
        supply.long += amount
        supply.short += amount
        self.check_conservation()

    def merge_scalar(self, branch: Branch, amount: int) -> None:
        if self.state not in (
            VaultState.OPEN,
            VaultState.RESOLVED,
            VaultState.VOIDED,
        ):
            raise ValueError("wrong state")
        amount = _amount(amount)
        supply = self.branches[branch]
        self._take(supply, "long", amount)
        self._take(supply, "short", amount)
        self._take(supply, "scalar_sets", amount)
        supply.usdc += amount
        self.check_conservation()

    def split_gate(self, branch: Branch, gate: GateType, amount: int) -> None:
        self._need(VaultState.OPEN)
        amount = _amount(amount)
        supply = self.branches[branch]
        self._take(supply, "usdc", amount)
        supply.gate_sets[gate] += amount
        supply.gate_yes[gate] += amount
        supply.gate_no[gate] += amount
        self.check_conservation()

    def merge_gate(self, branch: Branch, gate: GateType, amount: int) -> None:
        if self.state not in (
            VaultState.OPEN,
            VaultState.RESOLVED,
            VaultState.VOIDED,
        ):
            raise ValueError("wrong state")
        amount = _amount(amount)
        supply = self.branches[branch]
        self._take_map(supply.gate_yes, gate, amount)
        self._take_map(supply.gate_no, gate, amount)
        self._take_map(supply.gate_sets, gate, amount)
        supply.usdc += amount
        self.check_conservation()

    def transfer(self, amount: int) -> int:
        if self.state not in (
            VaultState.OPEN,
            VaultState.RESOLVED,
            VaultState.VOIDED,
        ):
            raise ValueError("wrong state")
        return _amount(amount)

    def resolve(self, winner: Branch) -> None:
        self._need(VaultState.OPEN)
        self.state = VaultState.RESOLVED
        self.winner = winner
        self.check_conservation()

    def void(self) -> None:
        if self.state not in (VaultState.OPEN, VaultState.RESOLVED):
            raise ValueError("wrong state")
        self.state = VaultState.VOIDED
        self.check_conservation()

    def settle_scalar(self, s: Decimal) -> None:
        self._need(VaultState.RESOLVED)
        s = Decimal(s)
        if not Decimal(0) <= s <= Decimal(1):
            raise ValueError("s must be in [0, 1]")
        self.state = VaultState.SCALAR_SETTLED
        self.s = s
        self.check_conservation()

    def settle_gate(self, gate: GateType, outcome: bool) -> None:
        if self.state not in (
            VaultState.RESOLVED,
            VaultState.SCALAR_SETTLED,
        ):
            raise ValueError("wrong state")
        if self.gate_outcomes[gate] is not None:
            raise ValueError("gate already settled")
        self.gate_outcomes[gate] = bool(outcome)
        self.check_conservation()

    def redeem(self, branch: Branch, amount: int) -> int:
        self._need(VaultState.SCALAR_SETTLED)
        self._winning(branch)
        amount = _amount(amount)
        self._take(self.branches[branch], "usdc", amount)
        return self._terminal_pay(amount)

    def redeem_scalar(
        self, branch: Branch, side: ScalarSide, amount: int
    ) -> int:
        self._need(VaultState.SCALAR_SETTLED)
        self._winning(branch)
        amount = _amount(amount)
        supply = self.branches[branch]
        if side is ScalarSide.LONG:
            self._take(supply, "long", amount)
            payout = _floor_product(amount, self.s)
        else:
            self._take(supply, "short", amount)
            payout = _floor_product(amount, Decimal(1) - self.s)
        return self._terminal_pay(payout)

    def redeem_scalar_pair(self, branch: Branch, amount: int) -> int:
        self._need(VaultState.SCALAR_SETTLED)
        self._winning(branch)
        amount = _amount(amount)
        supply = self.branches[branch]
        self._take(supply, "long", amount)
        self._take(supply, "short", amount)
        self._take(supply, "scalar_sets", amount)
        return self._terminal_pay(amount)

    def redeem_gate(
        self,
        branch: Branch,
        gate: GateType,
        side: GateSide,
        amount: int,
    ) -> int:
        self._need(VaultState.SCALAR_SETTLED)
        self._winning(branch)
        outcome = self.gate_outcomes[gate]
        if outcome is None:
            raise ValueError("gate is not settled")
        amount = _amount(amount)
        balances = (
            self.branches[branch].gate_yes
            if side is GateSide.YES
            else self.branches[branch].gate_no
        )
        self._take_map(balances, gate, amount)
        winning = (side is GateSide.YES) == outcome
        return self._terminal_pay(amount if winning else 0)

    def redeem_void(
        self,
        branch: Branch,
        kind: PositionKind,
        amount: int,
        gate: GateType | None = None,
    ) -> int:
        self._need(VaultState.VOIDED)
        amount = _amount(amount)
        supply = self.branches[branch]
        if kind is PositionKind.BRANCH_USDC:
            self._take(supply, "usdc", amount)
            payout = amount // 2
        elif kind is PositionKind.LONG:
            self._take(supply, "long", amount)
            payout = amount // 4
        elif kind is PositionKind.SHORT:
            self._take(supply, "short", amount)
            payout = amount // 4
        elif kind in (PositionKind.GATE_YES, PositionKind.GATE_NO):
            if gate is None:
                raise ValueError("gate is required")
            balances = supply.gate_yes if kind is PositionKind.GATE_YES else supply.gate_no
            self._take_map(balances, gate, amount)
            payout = amount // 4
        else:
            raise ValueError("unsupported position kind")
        return self._terminal_pay(payout)

    def sweep_dust(self) -> int:
        if self.state not in (VaultState.SCALAR_SETTLED, VaultState.VOIDED):
            raise ValueError("vault is not terminal")
        residue = self.escrowed
        self._pay(residue)
        self.terminal_redemptions += 1
        for branch in Branch:
            self.branches[branch] = BranchSupply()
        self.check_conservation()
        return residue

    def check_conservation(self) -> None:
        """Assert 03 §6.1 identities and the terminal payout bound."""
        if self.escrowed < 0 or self.total_payouts > self.collateral_in:
            raise AssertionError("payout exceeds collateral")
        if self.escrowed + self.total_payouts != self.collateral_in:
            raise AssertionError("escrow flow mismatch")
        if self.terminal_redemptions == 0:
            for supply in self.branches.values():
                if supply.identity() != self.escrowed:
                    raise AssertionError("per-branch conservation identity failed")
                if supply.long != supply.short or supply.long != supply.scalar_sets:
                    raise AssertionError("scalar pair supply mismatch")
                for gate in GateType:
                    if (
                        supply.gate_yes[gate] != supply.gate_no[gate]
                        or supply.gate_yes[gate] != supply.gate_sets[gate]
                    ):
                        raise AssertionError("gate pair supply mismatch")
        if self._claim_bound() > self.escrowed:
            raise AssertionError("remaining claims exceed escrow")

    def _claim_bound(self) -> int:
        if self.state is VaultState.OPEN:
            return max(supply.identity() for supply in self.branches.values())
        if self.state is VaultState.RESOLVED:
            return self.branches[self.winner].identity()
        if self.state is VaultState.VOIDED:
            # 03 §6.4/§6.5: value the maximal dispatchable recovery path,
            # not each claim in isolation. Complete scalar/gate sets can first
            # merge into branch-USDC, and effective Accept+Reject branch-USDC
            # can then merge at par. Only unmatched remainders take the
            # claimant-adverse half/quarter VOID floors.
            effective = {}
            leftovers = 0
            for branch, supply in self.branches.items():
                scalar_pairs = min(supply.long, supply.short)
                branch_usdc = supply.usdc + scalar_pairs
                leftovers += (supply.long - scalar_pairs) // 4
                leftovers += (supply.short - scalar_pairs) // 4
                for gate in GateType:
                    gate_pairs = min(
                        supply.gate_yes[gate], supply.gate_no[gate]
                    )
                    branch_usdc += gate_pairs
                    leftovers += (
                        supply.gate_yes[gate] - gate_pairs
                    ) // 4
                    leftovers += (
                        supply.gate_no[gate] - gate_pairs
                    ) // 4
                effective[branch] = branch_usdc
            cross_pairs = min(
                effective[Branch.ACCEPT], effective[Branch.REJECT]
            )
            return (
                cross_pairs
                + (effective[Branch.ACCEPT] - cross_pairs) // 2
                + (effective[Branch.REJECT] - cross_pairs) // 2
                + leftovers
            )
        supply = self.branches[self.winner]
        pairs = min(supply.long, supply.short)
        scalar = pairs
        scalar += _floor_product(supply.long - pairs, self.s)
        scalar += _floor_product(
            supply.short - pairs, Decimal(1) - self.s
        )
        gates = 0
        for gate in GateType:
            outcome = self.gate_outcomes[gate]
            if outcome is None:
                gates += max(supply.gate_yes[gate], supply.gate_no[gate])
            elif outcome:
                gates += supply.gate_yes[gate]
            else:
                gates += supply.gate_no[gate]
        return supply.usdc + scalar + gates

    def _terminal_pay(self, amount: int) -> int:
        self.terminal_redemptions += 1
        self._pay(amount)
        self.check_conservation()
        return amount

    def _pay(self, amount: int) -> None:
        amount = _amount(amount)
        if amount > self.escrowed:
            raise ValueError("insufficient escrow")
        self.escrowed -= amount
        self.total_payouts += amount

    def _winning(self, branch: Branch) -> None:
        if branch is not self.winner:
            raise ValueError("losing branch")

    def _need(self, state: VaultState) -> None:
        if self.state is not state:
            raise ValueError("wrong state")

    @staticmethod
    def _take(target, name: str, amount: int) -> None:
        current = getattr(target, name)
        if amount > current:
            raise ValueError("insufficient supply")
        setattr(target, name, current - amount)

    @staticmethod
    def _take_map(target: dict, key, amount: int) -> None:
        if amount > target[key]:
            raise ValueError("insufficient supply")
        target[key] -= amount


@dataclass
class BaselineVault:
    epoch: int
    escrowed: int = 0
    sets: int = 0
    long: int = 0
    short: int = 0
    state: BaselineState = BaselineState.OPEN
    s: Decimal | None = None
    collateral_in: int = 0
    total_payouts: int = 0

    def split_baseline(self, amount: int) -> None:
        self._need(BaselineState.OPEN)
        amount = _amount(amount)
        self.escrowed += amount
        self.collateral_in += amount
        self.sets += amount
        self.long += amount
        self.short += amount
        self.check_conservation()

    def merge_baseline(self, amount: int) -> int:
        self._need(BaselineState.OPEN)
        amount = _amount(amount)
        self._take_pair(amount)
        self._pay(amount)
        self.check_conservation()
        return amount

    def transfer(self, amount: int) -> int:
        return _amount(amount)

    def settle_baseline(self, s: Decimal) -> None:
        self._need(BaselineState.OPEN)
        s = Decimal(s)
        if not Decimal(0) <= s <= Decimal(1):
            raise ValueError("s must be in [0, 1]")
        self.state = BaselineState.SETTLED
        self.s = s
        self.check_conservation()

    def redeem_baseline(self, side: ScalarSide, amount: int) -> int:
        self._need(BaselineState.SETTLED)
        amount = _amount(amount)
        if side is ScalarSide.LONG:
            self._take("long", amount)
            payout = _floor_product(amount, self.s)
        else:
            self._take("short", amount)
            payout = _floor_product(amount, Decimal(1) - self.s)
        self._pay(payout)
        self.check_conservation()
        return payout

    def redeem_baseline_pair(self, amount: int) -> int:
        self._need(BaselineState.SETTLED)
        amount = _amount(amount)
        self._take_pair(amount)
        self._pay(amount)
        self.check_conservation()
        return amount

    def sweep_dust(self) -> int:
        self._need(BaselineState.SETTLED)
        residue = self.escrowed
        self._pay(residue)
        self.long = self.short = self.sets = 0
        self.check_conservation()
        return residue

    def check_conservation(self) -> None:
        if self.escrowed < 0 or self.total_payouts > self.collateral_in:
            raise AssertionError("baseline payout exceeds collateral")
        if self.escrowed + self.total_payouts != self.collateral_in:
            raise AssertionError("baseline escrow flow mismatch")
        if self.state is BaselineState.OPEN:
            if not self.escrowed == self.sets == self.long == self.short:
                raise AssertionError("baseline set identity failed")
        if self._claim_bound() > self.escrowed:
            raise AssertionError("baseline claims exceed escrow")

    def _claim_bound(self) -> int:
        if self.state is BaselineState.OPEN:
            return self.sets
        pairs = min(self.long, self.short)
        return (
            pairs
            + _floor_product(self.long - pairs, self.s)
            + _floor_product(self.short - pairs, Decimal(1) - self.s)
        )

    def _pay(self, amount: int) -> None:
        if amount > self.escrowed:
            raise ValueError("insufficient escrow")
        self.escrowed -= amount
        self.total_payouts += amount

    def _take(self, name: str, amount: int) -> None:
        current = getattr(self, name)
        if amount > current:
            raise ValueError("insufficient supply")
        setattr(self, name, current - amount)

    def _take_pair(self, amount: int) -> None:
        self._take("long", amount)
        self._take("short", amount)
        self._take("sets", amount)

    def _need(self, state: BaselineState) -> None:
        if self.state is not state:
            raise ValueError("wrong state")
