#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::{
    kernel, Balance, Branch, EpochId, FixedU64, GateType, MetricSpecVersion, PositionId,
    PositionKind, ProposalId, ScalarSide, VaultState,
};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub const MAX_POSITIONS_PER_ACCOUNT: u32 = 64;
pub const SCALE_1E9: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Decode, Default, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct BranchSupply {
    pub usdc: Balance,
    pub scalar_sets: Balance,
    pub gate_sets: [Balance; 2],
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct VaultInfo {
    pub escrowed: Balance,
    pub branches: [BranchSupply; 2],
    pub state: VaultState,
    pub gate_outcomes: [Option<bool>; 2],
    pub spec: MetricSpecVersion,
}

impl VaultInfo {
    pub const fn open(spec: MetricSpecVersion) -> Self {
        Self {
            escrowed: 0,
            branches: [
                BranchSupply {
                    usdc: 0,
                    scalar_sets: 0,
                    gate_sets: [0; 2],
                },
                BranchSupply {
                    usdc: 0,
                    scalar_sets: 0,
                    gate_sets: [0; 2],
                },
            ],
            state: VaultState::Open,
            gate_outcomes: [None, None],
            spec,
        }
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum BaselineState {
    Open,
    Settled(FixedU64),
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct BaselineVaultInfo {
    pub escrowed: Balance,
    pub sets: Balance,
    pub state: BaselineState,
}
impl BaselineVaultInfo {
    pub const fn open() -> Self {
        Self {
            escrowed: 0,
            sets: 0,
            state: BaselineState::Open,
        }
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum LedgerOrigin {
    Signed,
    MarketAuthority,
    ResolveAuthority,
    SettleAuthority,
    Root,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Event {
    Split(ProposalId, Balance),
    Merged(ProposalId, Balance),
    ScalarSplit(ProposalId, Branch, Balance),
    ScalarMerged(ProposalId, Branch, Balance),
    GateSplit(ProposalId, Branch, GateType, Balance),
    GateMerged(ProposalId, Branch, GateType, Balance),
    PositionTransferred(PositionId, Balance),
    BaselineSplit(EpochId, Balance),
    BaselineMerged(EpochId, Balance),
    VaultResolved(ProposalId, Branch),
    VaultVoided(ProposalId),
    ScalarSettlementSet(ProposalId, Branch, FixedU64),
    GateSettled(ProposalId, Branch, GateType, bool),
    BaselineSettled(EpochId, FixedU64),
    Redeemed(ProposalId, Balance),
    ScalarRedeemed(ProposalId, ScalarSide, Balance),
    ScalarPairRedeemed(ProposalId, Balance),
    GateRedeemed(ProposalId, GateType, Balance),
    VoidRedeemed(ProposalId, PositionKind, Balance, Balance),
    BaselineRedeemed(EpochId, ScalarSide, Balance),
    VaultReaped(ProposalId, Balance),
    BaselineVaultReaped(EpochId, Balance),
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Error {
    BadOrigin,
    UnknownVault,
    UnknownBaselineVault,
    WrongVaultState,
    AmountTooSmall,
    ArithmeticOverflow,
    InsufficientPosition,
    PositionCapExceeded,
    InvalidScore,
    GateAlreadySettled,
    GateNotSettled,
    WrongBranch,
    TryStateViolation,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct PositionRecord<AccountId> {
    pub id: PositionId,
    pub owner: AccountId,
    pub balance: Balance,
    pub deposit: Balance,
}
#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct PositionCount<AccountId> {
    pub owner: AccountId,
    pub count: u32,
}
#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct PositionTotal {
    pub id: PositionId,
    pub total: Balance,
}
#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct VaultRecord {
    pub proposal: ProposalId,
    pub info: VaultInfo,
}
#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct BaselineVaultRecord {
    pub epoch: EpochId,
    pub info: BaselineVaultInfo,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct LedgerState<AccountId> {
    pub vaults: Vec<VaultRecord>,
    pub baseline_vaults: Vec<BaselineVaultRecord>,
    pub positions: Vec<PositionRecord<AccountId>>,
    pub position_counts: Vec<PositionCount<AccountId>>,
    pub position_totals: Vec<PositionTotal>,
    pub deposits_held: Balance,
    pub events: Vec<Event>,
    pub protocol_accounts: Vec<AccountId>,
}

impl<AccountId: Clone + Eq> LedgerState<AccountId> {
    pub const fn new() -> Self {
        Self {
            vaults: Vec::new(),
            baseline_vaults: Vec::new(),
            positions: Vec::new(),
            position_counts: Vec::new(),
            position_totals: Vec::new(),
            deposits_held: 0,
            events: Vec::new(),
            protocol_accounts: Vec::new(),
        }
    }
    pub fn create_vault(&mut self, pid: ProposalId, spec: MetricSpecVersion) -> Result<(), Error> {
        ensure!(
            self.vaults.iter().all(|v| v.proposal != pid),
            Error::TryStateViolation
        );
        self.vaults.push(VaultRecord {
            proposal: pid,
            info: VaultInfo::open(spec),
        });
        Ok(())
    }
    pub fn create_baseline_vault(&mut self, epoch: EpochId) -> Result<(), Error> {
        ensure!(
            self.baseline_vaults.iter().all(|v| v.epoch != epoch),
            Error::TryStateViolation
        );
        self.baseline_vaults.push(BaselineVaultRecord {
            epoch,
            info: BaselineVaultInfo::open(),
        });
        Ok(())
    }
    pub fn add_protocol_account(&mut self, who: AccountId) {
        if !self.protocol_accounts.contains(&who) {
            self.protocol_accounts.push(who);
        }
    }

    // Every mutating operation is atomic: a failure at any step (e.g. a
    // position-cap rejection on the second leg of a split, or a capped
    // recipient on a transfer) restores the pre-call state, so no partial
    // vault/position mutation can ever become observable (G-1, I-1).
    fn atomically<R>(
        &mut self,
        op: impl FnOnce(&mut Self) -> Result<R, Error>,
    ) -> Result<R, Error> {
        let snapshot = self.clone();
        let result = op(self);
        if result.is_err() {
            *self = snapshot;
        }
        result
    }

    pub fn split(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.split_impl(origin, pid, who, a))
    }

    pub fn merge(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.merge_impl(origin, pid, who, a))
    }

    pub fn split_scalar(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        b: Branch,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.split_scalar_impl(origin, pid, b, who, a))
    }

    pub fn merge_scalar(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        b: Branch,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.merge_scalar_impl(origin, pid, b, who, a))
    }

    pub fn split_gate(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        b: Branch,
        g: GateType,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.split_gate_impl(origin, pid, b, g, who, a))
    }

    pub fn merge_gate(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        b: Branch,
        g: GateType,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.merge_gate_impl(origin, pid, b, g, who, a))
    }

    pub fn transfer(
        &mut self,
        origin: LedgerOrigin,
        id: PositionId,
        from: &AccountId,
        to: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.transfer_impl(origin, id, from, to, a))
    }

    pub fn redeem(&mut self, pid: ProposalId, who: &AccountId, a: Balance) -> Result<(), Error> {
        self.atomically(|led| led.redeem_impl(pid, who, a))
    }

    pub fn redeem_scalar(
        &mut self,
        pid: ProposalId,
        side: ScalarSide,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.redeem_scalar_impl(pid, side, who, a))
    }

    pub fn redeem_scalar_pair(
        &mut self,
        pid: ProposalId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.redeem_scalar_pair_impl(pid, who, a))
    }

    pub fn redeem_gate(
        &mut self,
        pid: ProposalId,
        g: GateType,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.redeem_gate_impl(pid, g, who, a))
    }

    pub fn redeem_void(
        &mut self,
        pid: ProposalId,
        b: Branch,
        kind: PositionKind,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.redeem_void_impl(pid, b, kind, who, a))
    }

    pub fn split_baseline(
        &mut self,
        origin: LedgerOrigin,
        epoch: EpochId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.split_baseline_impl(origin, epoch, who, a))
    }

    pub fn merge_baseline(
        &mut self,
        origin: LedgerOrigin,
        epoch: EpochId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.merge_baseline_impl(origin, epoch, who, a))
    }

    pub fn redeem_baseline(
        &mut self,
        epoch: EpochId,
        side: ScalarSide,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.redeem_baseline_impl(epoch, side, who, a))
    }

    pub fn redeem_baseline_pair(
        &mut self,
        epoch: EpochId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.atomically(|led| led.redeem_baseline_pair_impl(epoch, who, a))
    }

    fn split_impl(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        ensure_signed_or_market(origin)?;
        ensure!(a >= kernel::MIN_SPLIT_USDC, Error::AmountTooSmall);
        self.with_vault_mut(pid, |v| {
            ensure!(matches!(v.state, VaultState::Open), Error::WrongVaultState);
            v.escrowed = add(v.escrowed, a)?;
            v.branches[0].usdc = add(v.branches[0].usdc, a)?;
            v.branches[1].usdc = add(v.branches[1].usdc, a)?;
            Ok(())
        })?;
        self.mint(
            position(pid, Branch::Accept, PositionKind::BranchUsdc),
            who,
            a,
        )?;
        self.mint(
            position(pid, Branch::Reject, PositionKind::BranchUsdc),
            who,
            a,
        )?;
        self.events.push(Event::Split(pid, a));
        Ok(())
    }
    fn merge_impl(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        ensure_signed_or_market(origin)?;
        self.with_vault(pid, |v| {
            ensure!(
                matches!(
                    v.state,
                    VaultState::Open | VaultState::Resolved(_) | VaultState::Voided
                ),
                Error::WrongVaultState
            );
            Ok(())
        })??;
        self.ensure_holds(
            position(pid, Branch::Accept, PositionKind::BranchUsdc),
            who,
            a,
        )?;
        self.ensure_holds(
            position(pid, Branch::Reject, PositionKind::BranchUsdc),
            who,
            a,
        )?;
        self.burn(
            position(pid, Branch::Accept, PositionKind::BranchUsdc),
            who,
            a,
        )?;
        self.burn(
            position(pid, Branch::Reject, PositionKind::BranchUsdc),
            who,
            a,
        )?;
        self.with_vault_mut(pid, |v| {
            v.escrowed = sub(v.escrowed, a)?;
            v.branches[0].usdc = sub(v.branches[0].usdc, a)?;
            v.branches[1].usdc = sub(v.branches[1].usdc, a)?;
            Ok(())
        })?;
        self.events.push(Event::Merged(pid, a));
        Ok(())
    }
    fn split_scalar_impl(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        b: Branch,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        ensure_signed_or_market(origin)?;
        self.with_vault(pid, |v| {
            ensure!(matches!(v.state, VaultState::Open), Error::WrongVaultState);
            Ok(())
        })??;
        self.burn(position(pid, b, PositionKind::BranchUsdc), who, a)?;
        self.with_vault_mut(pid, |v| {
            let bs = &mut v.branches[bix(b)];
            bs.usdc = sub(bs.usdc, a)?;
            bs.scalar_sets = add(bs.scalar_sets, a)?;
            Ok(())
        })?;
        self.mint(position(pid, b, PositionKind::Long), who, a)?;
        self.mint(position(pid, b, PositionKind::Short), who, a)?;
        self.events.push(Event::ScalarSplit(pid, b, a));
        Ok(())
    }
    fn merge_scalar_impl(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        b: Branch,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        ensure_signed_or_market(origin)?;
        self.with_vault(pid, |v| {
            ensure!(
                matches!(
                    v.state,
                    VaultState::Open | VaultState::Resolved(_) | VaultState::Voided
                ),
                Error::WrongVaultState
            );
            Ok(())
        })??;
        self.ensure_holds(position(pid, b, PositionKind::Long), who, a)?;
        self.ensure_holds(position(pid, b, PositionKind::Short), who, a)?;
        self.burn(position(pid, b, PositionKind::Long), who, a)?;
        self.burn(position(pid, b, PositionKind::Short), who, a)?;
        self.with_vault_mut(pid, |v| {
            let bs = &mut v.branches[bix(b)];
            bs.usdc = add(bs.usdc, a)?;
            bs.scalar_sets = sub(bs.scalar_sets, a)?;
            Ok(())
        })?;
        self.mint(position(pid, b, PositionKind::BranchUsdc), who, a)?;
        self.events.push(Event::ScalarMerged(pid, b, a));
        Ok(())
    }
    fn split_gate_impl(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        b: Branch,
        g: GateType,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        ensure_signed_or_market(origin)?;
        self.with_vault(pid, |v| {
            ensure!(matches!(v.state, VaultState::Open), Error::WrongVaultState);
            Ok(())
        })??;
        self.burn(position(pid, b, PositionKind::BranchUsdc), who, a)?;
        self.with_vault_mut(pid, |v| {
            let bs = &mut v.branches[bix(b)];
            bs.usdc = sub(bs.usdc, a)?;
            bs.gate_sets[gix(g)] = add(bs.gate_sets[gix(g)], a)?;
            Ok(())
        })?;
        self.mint(position(pid, b, PositionKind::GateYes(g)), who, a)?;
        self.mint(position(pid, b, PositionKind::GateNo(g)), who, a)?;
        self.events.push(Event::GateSplit(pid, b, g, a));
        Ok(())
    }
    fn merge_gate_impl(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        b: Branch,
        g: GateType,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        ensure_signed_or_market(origin)?;
        self.with_vault(pid, |v| {
            ensure!(
                matches!(
                    v.state,
                    VaultState::Open | VaultState::Resolved(_) | VaultState::Voided
                ),
                Error::WrongVaultState
            );
            Ok(())
        })??;
        self.ensure_holds(position(pid, b, PositionKind::GateYes(g)), who, a)?;
        self.ensure_holds(position(pid, b, PositionKind::GateNo(g)), who, a)?;
        self.burn(position(pid, b, PositionKind::GateYes(g)), who, a)?;
        self.burn(position(pid, b, PositionKind::GateNo(g)), who, a)?;
        self.with_vault_mut(pid, |v| {
            ensure!(
                matches!(
                    v.state,
                    VaultState::Open | VaultState::Resolved(_) | VaultState::Voided
                ),
                Error::WrongVaultState
            );
            let bs = &mut v.branches[bix(b)];
            bs.usdc = add(bs.usdc, a)?;
            bs.gate_sets[gix(g)] = sub(bs.gate_sets[gix(g)], a)?;
            Ok(())
        })?;
        self.mint(position(pid, b, PositionKind::BranchUsdc), who, a)?;
        self.events.push(Event::GateMerged(pid, b, g, a));
        Ok(())
    }

    pub fn do_split(&mut self, pid: ProposalId, who: &AccountId, a: Balance) -> Result<(), Error> {
        self.split(LedgerOrigin::MarketAuthority, pid, who, a)
    }
    pub fn do_transfer(
        &mut self,
        id: PositionId,
        from: &AccountId,
        to: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.transfer(LedgerOrigin::MarketAuthority, id, from, to, a)
    }
    pub fn do_split_scalar(
        &mut self,
        pid: ProposalId,
        b: Branch,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.split_scalar(LedgerOrigin::MarketAuthority, pid, b, who, a)
    }
    pub fn do_split_gate(
        &mut self,
        pid: ProposalId,
        b: Branch,
        g: GateType,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.split_gate(LedgerOrigin::MarketAuthority, pid, b, g, who, a)
    }
    pub fn do_split_baseline(
        &mut self,
        epoch: EpochId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.split_baseline(LedgerOrigin::MarketAuthority, epoch, who, a)
    }
    pub fn do_merge(&mut self, pid: ProposalId, who: &AccountId, a: Balance) -> Result<(), Error> {
        self.merge(LedgerOrigin::MarketAuthority, pid, who, a)
    }
    pub fn do_merge_scalar(
        &mut self,
        pid: ProposalId,
        b: Branch,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.merge_scalar(LedgerOrigin::MarketAuthority, pid, b, who, a)
    }
    pub fn do_merge_gate(
        &mut self,
        pid: ProposalId,
        b: Branch,
        g: GateType,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.merge_gate(LedgerOrigin::MarketAuthority, pid, b, g, who, a)
    }
    pub fn do_merge_baseline(
        &mut self,
        epoch: EpochId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.merge_baseline(LedgerOrigin::MarketAuthority, epoch, who, a)
    }

    fn transfer_impl(
        &mut self,
        origin: LedgerOrigin,
        id: PositionId,
        from: &AccountId,
        to: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        ensure_signed_or_market(origin)?;
        ensure!(a > 0, Error::AmountTooSmall);
        // 03 §R-2 is two rules about deposit-backed position hygiene, not a
        // blanket amount minimum: (a) positions cannot be created below
        // MinTransfer, (b) a Signed transfer leaving a sub-MinTransfer
        // remainder moves the whole balance. The creation floor binds every
        // deposit-backed (non-protocol) destination regardless of origin -
        // market-wrapper payouts included (Codex review, PR #34) - while
        // protocol book/fee destinations hold no deposits and are exempt,
        // which is what lets the sub-MinTransfer fee legs of 03 §R-3 route
        // through these same ordinary ops. The remainder sweep applies to
        // Signed calls only: MarketAuthority moves are exact by construction
        // (04 §6.1).
        let mut a = a;
        let dest_exists = self.positions.iter().any(|p| p.id == id && &p.owner == to);
        if !dest_exists && !self.protocol_accounts.contains(to) {
            ensure!(a >= kernel::MIN_TRANSFER_USDC, Error::AmountTooSmall);
        }
        if matches!(origin, LedgerOrigin::Signed) && !self.protocol_accounts.contains(from) {
            let balance = self
                .positions
                .iter()
                .find(|p| p.id == id && &p.owner == from)
                .map_or(0, |p| p.balance);
            let remainder = balance.saturating_sub(a);
            if remainder > 0 && remainder < kernel::MIN_TRANSFER_USDC {
                a = balance;
            }
        }
        self.ensure_position_live(id)?;
        self.burn(id, from, a)?;
        self.mint(id, to, a)?;
        self.events.push(Event::PositionTransferred(id, a));
        Ok(())
    }

    pub fn resolve(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        w: Branch,
    ) -> Result<(), Error> {
        ensure!(
            matches!(origin, LedgerOrigin::ResolveAuthority | LedgerOrigin::Root),
            Error::BadOrigin
        );
        self.with_vault_mut(pid, |v| {
            ensure!(matches!(v.state, VaultState::Open), Error::WrongVaultState);
            v.state = VaultState::Resolved(w);
            Ok(())
        })?;
        self.events.push(Event::VaultResolved(pid, w));
        Ok(())
    }
    pub fn void(&mut self, origin: LedgerOrigin, pid: ProposalId) -> Result<(), Error> {
        ensure!(
            matches!(origin, LedgerOrigin::ResolveAuthority | LedgerOrigin::Root),
            Error::BadOrigin
        );
        self.with_vault_mut(pid, |v| {
            ensure!(
                matches!(v.state, VaultState::Open | VaultState::Resolved(_)),
                Error::WrongVaultState
            );
            v.state = VaultState::Voided;
            Ok(())
        })?;
        self.events.push(Event::VaultVoided(pid));
        Ok(())
    }
    pub fn settle_scalar(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        s: FixedU64,
    ) -> Result<(), Error> {
        ensure_settle(origin)?;
        ensure_score(s)?;
        let winner = self.with_vault_mut(pid, |v| {
            let VaultState::Resolved(w) = v.state else {
                return Err(Error::WrongVaultState);
            };
            v.state = VaultState::ScalarSettled { winner: w, s };
            Ok(w)
        })?;
        self.events.push(Event::ScalarSettlementSet(pid, winner, s));
        Ok(())
    }
    pub fn settle_gate(
        &mut self,
        origin: LedgerOrigin,
        pid: ProposalId,
        g: GateType,
        outcome: bool,
    ) -> Result<(), Error> {
        ensure_settle(origin)?;
        let winner = self.with_vault_mut(pid, |v| {
            let w = match v.state {
                VaultState::Resolved(w) | VaultState::ScalarSettled { winner: w, .. } => w,
                _ => return Err(Error::WrongVaultState),
            };
            let slot = &mut v.gate_outcomes[gix(g)];
            ensure!(slot.is_none(), Error::GateAlreadySettled);
            *slot = Some(outcome);
            Ok(w)
        })?;
        self.events
            .push(Event::GateSettled(pid, winner, g, outcome));
        Ok(())
    }

    fn redeem_impl(&mut self, pid: ProposalId, who: &AccountId, a: Balance) -> Result<(), Error> {
        let w = self.settled_winner(pid)?;
        self.burn(position(pid, w, PositionKind::BranchUsdc), who, a)?;
        self.pay_proposal(pid, a)?;
        self.events.push(Event::Redeemed(pid, a));
        Ok(())
    }
    fn redeem_scalar_impl(
        &mut self,
        pid: ProposalId,
        side: ScalarSide,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        let (w, s) = self.settled(pid)?;
        self.burn(
            position(
                pid,
                w,
                match side {
                    ScalarSide::Long => PositionKind::Long,
                    ScalarSide::Short => PositionKind::Short,
                },
            ),
            who,
            a,
        )?;
        let pay = mul_score(
            a,
            if matches!(side, ScalarSide::Long) {
                s.0 as u128
            } else {
                SCALE_1E9 - s.0 as u128
            },
        )?;
        self.pay_proposal(pid, pay)?;
        self.events.push(Event::ScalarRedeemed(pid, side, pay));
        Ok(())
    }
    fn redeem_scalar_pair_impl(
        &mut self,
        pid: ProposalId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        let w = self.settled_winner(pid)?;
        self.ensure_holds(position(pid, w, PositionKind::Long), who, a)?;
        self.ensure_holds(position(pid, w, PositionKind::Short), who, a)?;
        self.burn(position(pid, w, PositionKind::Long), who, a)?;
        self.burn(position(pid, w, PositionKind::Short), who, a)?;
        self.pay_proposal(pid, a)?;
        self.events.push(Event::ScalarPairRedeemed(pid, a));
        Ok(())
    }
    fn redeem_gate_impl(
        &mut self,
        pid: ProposalId,
        g: GateType,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        let w = self.settled_winner(pid)?;
        let outcome = self.with_vault(pid, |v| {
            v.gate_outcomes[gix(g)].ok_or(Error::GateNotSettled)
        })??;
        self.burn(
            position(
                pid,
                w,
                if outcome {
                    PositionKind::GateYes(g)
                } else {
                    PositionKind::GateNo(g)
                },
            ),
            who,
            a,
        )?;
        self.pay_proposal(pid, a)?;
        self.events.push(Event::GateRedeemed(pid, g, a));
        Ok(())
    }
    fn redeem_void_impl(
        &mut self,
        pid: ProposalId,
        b: Branch,
        kind: PositionKind,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.with_vault(pid, |v| {
            ensure!(
                matches!(v.state, VaultState::Voided),
                Error::WrongVaultState
            );
            Ok(())
        })??;
        self.burn(position(pid, b, kind), who, a)?;
        let pay = match kind {
            PositionKind::BranchUsdc => a / 2,
            _ => a / 4,
        };
        self.pay_proposal(pid, pay)?;
        self.events.push(Event::VoidRedeemed(pid, kind, a, pay));
        Ok(())
    }

    fn split_baseline_impl(
        &mut self,
        origin: LedgerOrigin,
        epoch: EpochId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        ensure_signed_or_market(origin)?;
        ensure!(a >= kernel::MIN_SPLIT_USDC, Error::AmountTooSmall);
        self.with_base_mut(epoch, |v| {
            ensure!(
                matches!(v.state, BaselineState::Open),
                Error::WrongVaultState
            );
            v.escrowed = add(v.escrowed, a)?;
            v.sets = add(v.sets, a)?;
            Ok(())
        })?;
        self.mint(baseline(epoch, ScalarSide::Long), who, a)?;
        self.mint(baseline(epoch, ScalarSide::Short), who, a)?;
        self.events.push(Event::BaselineSplit(epoch, a));
        Ok(())
    }
    fn merge_baseline_impl(
        &mut self,
        origin: LedgerOrigin,
        epoch: EpochId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        ensure_signed_or_market(origin)?;
        self.with_base(epoch, |v| {
            ensure!(
                matches!(v.state, BaselineState::Open),
                Error::WrongVaultState
            );
            Ok(())
        })??;
        self.ensure_holds(baseline(epoch, ScalarSide::Long), who, a)?;
        self.ensure_holds(baseline(epoch, ScalarSide::Short), who, a)?;
        self.burn(baseline(epoch, ScalarSide::Long), who, a)?;
        self.burn(baseline(epoch, ScalarSide::Short), who, a)?;
        self.with_base_mut(epoch, |v| {
            ensure!(
                matches!(v.state, BaselineState::Open),
                Error::WrongVaultState
            );
            v.escrowed = sub(v.escrowed, a)?;
            v.sets = sub(v.sets, a)?;
            Ok(())
        })?;
        self.events.push(Event::BaselineMerged(epoch, a));
        Ok(())
    }
    pub fn settle_baseline(
        &mut self,
        origin: LedgerOrigin,
        epoch: EpochId,
        s: FixedU64,
    ) -> Result<(), Error> {
        ensure_settle(origin)?;
        ensure_score(s)?;
        self.with_base_mut(epoch, |v| {
            ensure!(
                matches!(v.state, BaselineState::Open),
                Error::WrongVaultState
            );
            v.state = BaselineState::Settled(s);
            Ok(())
        })?;
        self.events.push(Event::BaselineSettled(epoch, s));
        Ok(())
    }
    fn redeem_baseline_impl(
        &mut self,
        epoch: EpochId,
        side: ScalarSide,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        let s = self.with_base(epoch, |v| match v.state {
            BaselineState::Settled(s) => Ok(s),
            _ => Err(Error::WrongVaultState),
        })??;
        self.burn(baseline(epoch, side), who, a)?;
        let pay = mul_score(
            a,
            if matches!(side, ScalarSide::Long) {
                s.0 as u128
            } else {
                SCALE_1E9 - s.0 as u128
            },
        )?;
        self.pay_baseline(epoch, pay)?;
        self.events.push(Event::BaselineRedeemed(epoch, side, pay));
        Ok(())
    }
    fn redeem_baseline_pair_impl(
        &mut self,
        epoch: EpochId,
        who: &AccountId,
        a: Balance,
    ) -> Result<(), Error> {
        self.with_base(epoch, |v| match v.state {
            BaselineState::Settled(_) => Ok(()),
            _ => Err(Error::WrongVaultState),
        })??;
        self.ensure_holds(baseline(epoch, ScalarSide::Long), who, a)?;
        self.ensure_holds(baseline(epoch, ScalarSide::Short), who, a)?;
        self.burn(baseline(epoch, ScalarSide::Long), who, a)?;
        self.burn(baseline(epoch, ScalarSide::Short), who, a)?;
        self.pay_baseline(epoch, a)?;
        self.events
            .push(Event::BaselineRedeemed(epoch, ScalarSide::Long, a));
        Ok(())
    }

    pub fn try_state(&self) -> Result<(), Error> {
        for p in &self.positions {
            ensure!(p.balance > 0, Error::TryStateViolation);
            ensure!(
                p.deposit == 0 || p.deposit == kernel::POSITION_DEPOSIT_USDC,
                Error::TryStateViolation
            );
        }
        for c in &self.position_counts {
            ensure!(
                self.protocol_accounts.contains(&c.owner) || c.count <= MAX_POSITIONS_PER_ACCOUNT,
                Error::PositionCapExceeded
            );
            let actual = self.positions.iter().filter(|p| p.owner == c.owner).count() as u32;
            ensure!(actual == c.count, Error::TryStateViolation);
        }
        for t in &self.position_totals {
            let actual: Balance = self
                .positions
                .iter()
                .filter(|p| p.id == t.id)
                .try_fold(0u128, |acc, p| {
                    acc.checked_add(p.balance).ok_or(Error::ArithmeticOverflow)
                })?;
            ensure!(actual == t.total, Error::TryStateViolation);
        }
        self.try_state_vault_conservation()?;
        Ok(())
    }

    /// Vault conservation (03 §6, I-1..I-4): while a vault is Open or
    /// Resolved, every branch's outstanding claims equal escrow exactly and
    /// every paired supply matches its vault-side set counter; in terminal
    /// states (redemptions decrement escrow only), escrow must still cover
    /// the worst-case remaining liability computed from live supplies.
    fn try_state_vault_conservation(&self) -> Result<(), Error> {
        for v in &self.vaults {
            let pid = v.proposal;
            let info = &v.info;
            match info.state {
                VaultState::Open | VaultState::Resolved(_) => {
                    for b in [Branch::Accept, Branch::Reject] {
                        let bs = &info.branches[bix(b)];
                        let claims = add(
                            add(bs.usdc, bs.scalar_sets)?,
                            add(bs.gate_sets[0], bs.gate_sets[1])?,
                        )?;
                        ensure!(claims == info.escrowed, Error::TryStateViolation);
                        ensure!(
                            self.total(position(pid, b, PositionKind::BranchUsdc)) == bs.usdc,
                            Error::TryStateViolation
                        );
                        let long = self.total(position(pid, b, PositionKind::Long));
                        let short = self.total(position(pid, b, PositionKind::Short));
                        ensure!(
                            long == short && long == bs.scalar_sets,
                            Error::TryStateViolation
                        );
                        for g in [GateType::Survival, GateType::Security] {
                            let yes = self.total(position(pid, b, PositionKind::GateYes(g)));
                            let no = self.total(position(pid, b, PositionKind::GateNo(g)));
                            ensure!(
                                yes == no && yes == bs.gate_sets[gix(g)],
                                Error::TryStateViolation
                            );
                        }
                    }
                }
                VaultState::ScalarSettled { winner, s } => {
                    let w = winner;
                    let mut liability = self.total(position(pid, w, PositionKind::BranchUsdc));
                    // redeem_scalar_pair pays complete pairs at par regardless
                    // of s, so pairs must be counted exactly - flooring both
                    // single legs undercounts by up to one unit per pair
                    // (Codex review, PR #33).
                    let long = self.total(position(pid, w, PositionKind::Long));
                    let short = self.total(position(pid, w, PositionKind::Short));
                    let pairs = long.min(short);
                    liability = add(liability, pairs)?;
                    liability = add(liability, mul_score(sub(long, pairs)?, s.0 as u128)?)?;
                    liability = add(
                        liability,
                        mul_score(sub(short, pairs)?, SCALE_1E9 - s.0 as u128)?,
                    )?;
                    for g in [GateType::Survival, GateType::Security] {
                        let yes = self.total(position(pid, w, PositionKind::GateYes(g)));
                        let no = self.total(position(pid, w, PositionKind::GateNo(g)));
                        let gate_liability = match info.gate_outcomes[gix(g)] {
                            Some(true) => yes,
                            Some(false) => no,
                            None => yes.max(no),
                        };
                        liability = add(liability, gate_liability)?;
                    }
                    ensure!(info.escrowed >= liability, Error::TryStateViolation);
                }
                VaultState::Voided => {
                    // transfer/merge/merge_scalar/merge_gate stay available in
                    // Voided (03: the D-1 par-recovery path), so holders can
                    // assemble complete sets into branch-USDC and complete
                    // Accept+Reject pairs into par. Worst-case liability is
                    // therefore pair-first (Codex review, PR #33): per branch,
                    // scalar/gate pairs merge into effective branch-USDC;
                    // cross-branch pairs redeem at par; only unmatched
                    // remainders take the floor(a/2) / floor(a/4) VOID rates.
                    let mut effective = [0u128; 2];
                    let mut leftovers = 0u128;
                    for b in [Branch::Accept, Branch::Reject] {
                        let mut eff = self.total(position(pid, b, PositionKind::BranchUsdc));
                        let long = self.total(position(pid, b, PositionKind::Long));
                        let short = self.total(position(pid, b, PositionKind::Short));
                        let scalar_pairs = long.min(short);
                        eff = add(eff, scalar_pairs)?;
                        leftovers = add(
                            leftovers,
                            add(sub(long, scalar_pairs)? / 4, sub(short, scalar_pairs)? / 4)?,
                        )?;
                        for g in [GateType::Survival, GateType::Security] {
                            let yes = self.total(position(pid, b, PositionKind::GateYes(g)));
                            let no = self.total(position(pid, b, PositionKind::GateNo(g)));
                            let gate_pairs = yes.min(no);
                            eff = add(eff, gate_pairs)?;
                            leftovers = add(
                                leftovers,
                                add(sub(yes, gate_pairs)? / 4, sub(no, gate_pairs)? / 4)?,
                            )?;
                        }
                        effective[bix(b)] = eff;
                    }
                    let cross_pairs = effective[0].min(effective[1]);
                    let mut liability = cross_pairs;
                    liability = add(liability, sub(effective[0], cross_pairs)? / 2)?;
                    liability = add(liability, sub(effective[1], cross_pairs)? / 2)?;
                    liability = add(liability, leftovers)?;
                    ensure!(info.escrowed >= liability, Error::TryStateViolation);
                }
            }
        }
        for v in &self.baseline_vaults {
            let epoch = v.epoch;
            let info = &v.info;
            let long = self.total(baseline(epoch, ScalarSide::Long));
            let short = self.total(baseline(epoch, ScalarSide::Short));
            match info.state {
                BaselineState::Open => {
                    ensure!(info.escrowed == info.sets, Error::TryStateViolation);
                    ensure!(long == short && long == info.sets, Error::TryStateViolation);
                }
                BaselineState::Settled(s) => {
                    // redeem_baseline_pair pays complete pairs at par.
                    let pairs = long.min(short);
                    let liability = add(
                        pairs,
                        add(
                            mul_score(sub(long, pairs)?, s.0 as u128)?,
                            mul_score(sub(short, pairs)?, SCALE_1E9 - s.0 as u128)?,
                        )?,
                    )?;
                    ensure!(info.escrowed >= liability, Error::TryStateViolation);
                }
            }
        }
        Ok(())
    }

    fn total(&self, id: PositionId) -> Balance {
        self.position_totals
            .iter()
            .find(|t| t.id == id)
            .map_or(0, |t| t.total)
    }

    fn ensure_holds(&self, id: PositionId, owner: &AccountId, a: Balance) -> Result<(), Error> {
        let balance = self
            .positions
            .iter()
            .find(|p| p.id == id && &p.owner == owner)
            .map_or(0, |p| p.balance);
        ensure!(balance >= a, Error::InsufficientPosition);
        Ok(())
    }

    fn mint(&mut self, id: PositionId, owner: &AccountId, a: Balance) -> Result<(), Error> {
        if a == 0 {
            return Ok(());
        }
        if let Some(p) = self
            .positions
            .iter_mut()
            .find(|p| p.id == id && &p.owner == owner)
        {
            p.balance = add(p.balance, a)?;
        } else {
            let protocol = self.protocol_accounts.contains(owner);
            if !protocol {
                let count = self.count_mut(owner);
                ensure!(
                    *count < MAX_POSITIONS_PER_ACCOUNT,
                    Error::PositionCapExceeded
                );
                *count += 1;
                self.deposits_held = add(self.deposits_held, kernel::POSITION_DEPOSIT_USDC)?;
            }
            self.positions.push(PositionRecord {
                id,
                owner: owner.clone(),
                balance: a,
                deposit: if protocol {
                    0
                } else {
                    kernel::POSITION_DEPOSIT_USDC
                },
            });
        }
        self.add_total(id, a)
    }
    fn burn(&mut self, id: PositionId, owner: &AccountId, a: Balance) -> Result<(), Error> {
        if a == 0 {
            return Ok(());
        }
        let idx = self
            .positions
            .iter()
            .position(|p| p.id == id && &p.owner == owner)
            .ok_or(Error::InsufficientPosition)?;
        ensure!(
            self.positions[idx].balance >= a,
            Error::InsufficientPosition
        );
        self.positions[idx].balance -= a;
        self.sub_total(id, a)?;
        if self.positions[idx].balance == 0 {
            let dep = self.positions[idx].deposit;
            self.positions.remove(idx);
            if dep > 0 {
                self.deposits_held = sub(self.deposits_held, dep)?;
                *self.count_mut(owner) -= 1;
            }
        }
        Ok(())
    }
    fn count_mut(&mut self, owner: &AccountId) -> &mut u32 {
        if let Some(i) = self.position_counts.iter().position(|c| &c.owner == owner) {
            &mut self.position_counts[i].count
        } else {
            self.position_counts.push(PositionCount {
                owner: owner.clone(),
                count: 0,
            });
            let idx = self.position_counts.len() - 1;
            &mut self.position_counts[idx].count
        }
    }
    fn add_total(&mut self, id: PositionId, a: Balance) -> Result<(), Error> {
        if let Some(t) = self.position_totals.iter_mut().find(|t| t.id == id) {
            t.total = add(t.total, a)?;
        } else {
            self.position_totals.push(PositionTotal { id, total: a });
        }
        Ok(())
    }
    fn sub_total(&mut self, id: PositionId, a: Balance) -> Result<(), Error> {
        let i = self
            .position_totals
            .iter()
            .position(|t| t.id == id)
            .ok_or(Error::TryStateViolation)?;
        self.position_totals[i].total = sub(self.position_totals[i].total, a)?;
        if self.position_totals[i].total == 0 {
            self.position_totals.remove(i);
        }
        Ok(())
    }
    fn with_vault<R>(&self, pid: ProposalId, f: impl FnOnce(&VaultInfo) -> R) -> Result<R, Error> {
        self.vaults
            .iter()
            .find(|v| v.proposal == pid)
            .map(|v| f(&v.info))
            .ok_or(Error::UnknownVault)
    }
    fn with_vault_mut<R>(
        &mut self,
        pid: ProposalId,
        f: impl FnOnce(&mut VaultInfo) -> Result<R, Error>,
    ) -> Result<R, Error> {
        let v = self
            .vaults
            .iter_mut()
            .find(|v| v.proposal == pid)
            .ok_or(Error::UnknownVault)?;
        f(&mut v.info)
    }
    fn with_base<R>(
        &self,
        e: EpochId,
        f: impl FnOnce(&BaselineVaultInfo) -> R,
    ) -> Result<R, Error> {
        self.baseline_vaults
            .iter()
            .find(|v| v.epoch == e)
            .map(|v| f(&v.info))
            .ok_or(Error::UnknownBaselineVault)
    }
    fn with_base_mut<R>(
        &mut self,
        e: EpochId,
        f: impl FnOnce(&mut BaselineVaultInfo) -> Result<R, Error>,
    ) -> Result<R, Error> {
        let v = self
            .baseline_vaults
            .iter_mut()
            .find(|v| v.epoch == e)
            .ok_or(Error::UnknownBaselineVault)?;
        f(&mut v.info)
    }
    fn ensure_position_live(&self, id: PositionId) -> Result<(), Error> {
        match id {
            PositionId::Proposal { proposal, .. } => self.with_vault(proposal, |v| {
                ensure!(
                    matches!(
                        v.state,
                        VaultState::Open | VaultState::Resolved(_) | VaultState::Voided
                    ),
                    Error::WrongVaultState
                );
                Ok(())
            })?,
            PositionId::Baseline { epoch, .. } => self.with_base(epoch, |v| {
                ensure!(
                    matches!(v.state, BaselineState::Open),
                    Error::WrongVaultState
                );
                Ok(())
            })?,
        }
    }
    fn settled(&self, pid: ProposalId) -> Result<(Branch, FixedU64), Error> {
        self.with_vault(pid, |v| match v.state {
            VaultState::ScalarSettled { winner, s } => Ok((winner, s)),
            _ => Err(Error::WrongVaultState),
        })?
    }
    fn settled_winner(&self, pid: ProposalId) -> Result<Branch, Error> {
        Ok(self.settled(pid)?.0)
    }
    fn pay_proposal(&mut self, pid: ProposalId, a: Balance) -> Result<(), Error> {
        self.with_vault_mut(pid, |v| {
            v.escrowed = sub(v.escrowed, a)?;
            Ok(())
        })
    }
    fn pay_baseline(&mut self, e: EpochId, a: Balance) -> Result<(), Error> {
        self.with_base_mut(e, |v| {
            v.escrowed = sub(v.escrowed, a)?;
            Ok(())
        })
    }
}

impl<AccountId: Clone + Eq> Default for LedgerState<AccountId> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn position(proposal: ProposalId, branch: Branch, kind: PositionKind) -> PositionId {
    PositionId::Proposal {
        proposal,
        branch,
        kind,
    }
}
pub fn baseline(epoch: EpochId, side: ScalarSide) -> PositionId {
    PositionId::Baseline { epoch, side }
}
fn bix(b: Branch) -> usize {
    match b {
        Branch::Accept => 0,
        Branch::Reject => 1,
    }
}
fn gix(g: GateType) -> usize {
    match g {
        GateType::Survival => 0,
        GateType::Security => 1,
    }
}
fn add(a: Balance, b: Balance) -> Result<Balance, Error> {
    a.checked_add(b).ok_or(Error::ArithmeticOverflow)
}
fn sub(a: Balance, b: Balance) -> Result<Balance, Error> {
    a.checked_sub(b).ok_or(Error::ArithmeticOverflow)
}
fn mul_score(a: Balance, s: u128) -> Result<Balance, Error> {
    a.checked_mul(s)
        .ok_or(Error::ArithmeticOverflow)
        .map(|v| v / SCALE_1E9)
}
fn ensure_score(s: FixedU64) -> Result<(), Error> {
    ensure!((s.0 as u128) <= SCALE_1E9, Error::InvalidScore);
    Ok(())
}
fn ensure_signed_or_market(o: LedgerOrigin) -> Result<(), Error> {
    ensure!(
        matches!(
            o,
            LedgerOrigin::Signed | LedgerOrigin::MarketAuthority | LedgerOrigin::Root
        ),
        Error::BadOrigin
    );
    Ok(())
}
fn ensure_settle(o: LedgerOrigin) -> Result<(), Error> {
    ensure!(
        matches!(o, LedgerOrigin::SettleAuthority | LedgerOrigin::Root),
        Error::BadOrigin
    );
    Ok(())
}

macro_rules! ensure {
    ($cond:expr, $err:expr) => {
        if !$cond {
            return Err($err);
        }
    };
}
use ensure;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking {
    use super::*;
    pub fn benchmark_split() -> Result<(), Error> {
        let mut s = LedgerState::<u64>::new();
        s.create_vault(1, 0)?;
        s.split(LedgerOrigin::Signed, 1, &7, kernel::MIN_SPLIT_USDC)
    }
    pub fn benchmark_redeem_void() -> Result<(), Error> {
        let mut s = LedgerState::<u64>::new();
        s.create_vault(1, 0)?;
        s.split(LedgerOrigin::Signed, 1, &7, kernel::MIN_SPLIT_USDC)?;
        s.void(LedgerOrigin::ResolveAuthority, 1)?;
        s.redeem_void(
            1,
            Branch::Accept,
            PositionKind::BranchUsdc,
            &7,
            kernel::MIN_SPLIT_USDC,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn acct(n: u8) -> [u8; 32] {
        [n; 32]
    }
    fn fill_to_cap(s: &mut LedgerState<[u8; 32]>, who: &[u8; 32], splits: u32) {
        // Occupy 2·splits position entries via real splits so counts,
        // deposits, totals, and vault state all stay consistent for try_state.
        for i in 0..splits {
            let pid = 1_000 + u64::from(i);
            s.create_vault(pid, 0).unwrap();
            s.split(LedgerOrigin::Signed, pid, who, kernel::MIN_SPLIT_USDC)
                .unwrap();
        }
    }

    #[test]
    fn split_at_position_cap_is_atomic() {
        // Codex review, PR #16 (P1): with 63 live positions, the first split
        // mint succeeds (64 = cap) and the second hits the cap - the vault
        // escrow, branch supplies, and the first leg must all roll back.
        let mut s = LedgerState::new();
        let a = acct(1);
        let other = acct(9);
        fill_to_cap(&mut s, &a, 32); // 64 entries
                                     // Move one full Reject leg away so the account holds exactly 63.
        s.transfer(
            LedgerOrigin::Signed,
            position(1_000, Branch::Reject, PositionKind::BranchUsdc),
            &a,
            &other,
            kernel::MIN_SPLIT_USDC,
        )
        .unwrap();
        s.create_vault(1, 0).unwrap();
        let deposits_before = s.deposits_held;
        let positions_before = s.positions.len();
        assert_eq!(
            s.split(LedgerOrigin::Signed, 1, &a, 1_000_000).unwrap_err(),
            Error::PositionCapExceeded
        );
        // Fully rolled back: no vault escrow, no orphan Accept leg, no count
        // or deposit drift.
        assert_eq!(s.vaults.last().unwrap().info.escrowed, 0);
        assert_eq!(s.vaults.last().unwrap().info.branches[0].usdc, 0);
        assert!(s
            .positions
            .iter()
            .all(|p| !matches!(p.id, PositionId::Proposal { proposal: 1, .. })));
        assert_eq!(s.positions.len(), positions_before);
        assert_eq!(s.deposits_held, deposits_before);
        s.try_state().unwrap();
    }

    #[test]
    fn transfer_to_capped_recipient_is_atomic() {
        // Codex review, PR #16 (P1): a failed transfer must not burn the
        // sender's balance.
        let mut s = LedgerState::new();
        let sender = acct(1);
        let capped = acct(2);
        fill_to_cap(&mut s, &capped, 32); // exactly 64 entries
        s.create_vault(1, 0).unwrap();
        s.split(LedgerOrigin::Signed, 1, &sender, 1_000_000)
            .unwrap();
        let id = position(1, Branch::Accept, PositionKind::BranchUsdc);
        assert_eq!(
            s.transfer(LedgerOrigin::Signed, id, &sender, &capped, 500_000)
                .unwrap_err(),
            Error::PositionCapExceeded
        );
        let balance = s
            .positions
            .iter()
            .find(|p| p.id == id && p.owner == sender)
            .map(|p| p.balance)
            .unwrap();
        assert_eq!(balance, 1_000_000);
        s.try_state().unwrap();
    }

    #[test]
    fn try_state_alarms_on_vault_conservation_breaks() {
        // Codex review, PR #16 (P1): corrupting escrow or branch supplies
        // must trip try_state.
        let mut s = LedgerState::new();
        s.create_vault(1, 0).unwrap();
        let a = acct(1);
        s.split(LedgerOrigin::Signed, 1, &a, 1_000_000).unwrap();
        s.try_state().unwrap();
        s.vaults[0].info.escrowed += 1;
        assert_eq!(s.try_state().unwrap_err(), Error::TryStateViolation);
        s.vaults[0].info.escrowed -= 1;
        s.vaults[0].info.branches[0].usdc -= 1;
        assert_eq!(s.try_state().unwrap_err(), Error::TryStateViolation);
        s.vaults[0].info.branches[0].usdc += 1;
        s.try_state().unwrap();
        // Terminal-state solvency: a settled vault whose escrow falls below
        // the outstanding winner-side liability alarms.
        s.resolve(LedgerOrigin::ResolveAuthority, 1, Branch::Accept)
            .unwrap();
        s.settle_scalar(LedgerOrigin::SettleAuthority, 1, FixedU64(500_000_000))
            .unwrap();
        s.try_state().unwrap();
        s.vaults[0].info.escrowed = 0;
        assert_eq!(s.try_state().unwrap_err(), Error::TryStateViolation);
    }

    #[test]
    fn settled_liability_counts_complete_pairs_at_par() {
        // Codex review, PR #33: with an odd supply and s = 0.5, flooring both
        // single legs undercounts the pair-redemption liability by one unit.
        let mut s = LedgerState::new();
        s.create_vault(1, 0).unwrap();
        let a = acct(1);
        s.split(LedgerOrigin::Signed, 1, &a, 10_001).unwrap();
        s.split_scalar(LedgerOrigin::Signed, 1, Branch::Accept, &a, 10_001)
            .unwrap();
        s.resolve(LedgerOrigin::ResolveAuthority, 1, Branch::Accept)
            .unwrap();
        s.settle_scalar(LedgerOrigin::SettleAuthority, 1, FixedU64(500_000_000))
            .unwrap();
        s.try_state().unwrap();
        // Escrow one unit below the pair liability must alarm - the old
        // floored-legs formula (2 x floor(10_001/2) = 10_000) accepted it.
        s.vaults[0].info.escrowed -= 1;
        assert_eq!(s.try_state().unwrap_err(), Error::TryStateViolation);
    }

    #[test]
    fn void_liability_counts_par_pair_assembly() {
        // Codex review, PR #33: merge stays available in Voided, so live
        // Accept+Reject pairs recover par - not 2 x floor(a/2).
        let mut s = LedgerState::new();
        s.create_vault(1, 0).unwrap();
        let a = acct(1);
        s.split(LedgerOrigin::Signed, 1, &a, 10_001).unwrap();
        s.void(LedgerOrigin::ResolveAuthority, 1).unwrap();
        s.try_state().unwrap();
        s.vaults[0].info.escrowed -= 1;
        assert_eq!(s.try_state().unwrap_err(), Error::TryStateViolation);
    }

    #[test]
    fn settlement_events_carry_the_winning_branch() {
        // Codex review, PR #16 (P2): 02 §5 freezes
        // ScalarSettlementSet { pid, branch, s } and
        // GateSettled { pid, branch, gate, outcome }.
        let mut s = LedgerState::new();
        s.create_vault(1, 0).unwrap();
        let a = acct(1);
        s.split(LedgerOrigin::Signed, 1, &a, 1_000_000).unwrap();
        s.resolve(LedgerOrigin::ResolveAuthority, 1, Branch::Accept)
            .unwrap();
        s.settle_gate(LedgerOrigin::SettleAuthority, 1, GateType::Survival, false)
            .unwrap();
        s.settle_scalar(LedgerOrigin::SettleAuthority, 1, FixedU64(700_000_000))
            .unwrap();
        assert!(s.events.iter().any(|e| matches!(
            e,
            Event::GateSettled(1, Branch::Accept, GateType::Survival, false)
        )));
        assert!(s.events.iter().any(|e| matches!(
            e,
            Event::ScalarSettlementSet(1, Branch::Accept, FixedU64(700_000_000))
        )));
    }

    #[test]
    fn split_merge_and_deposits_conserve() {
        let mut s = LedgerState::new();
        s.create_vault(1, 2).unwrap();
        let a = acct(1);
        s.split(LedgerOrigin::Signed, 1, &a, 1_000_000).unwrap();
        assert_eq!(s.vaults[0].info.escrowed, 1_000_000);
        assert_eq!(s.positions.len(), 2);
        assert_eq!(s.deposits_held, 2 * kernel::POSITION_DEPOSIT_USDC);
        s.merge(LedgerOrigin::Signed, 1, &a, 1_000_000).unwrap();
        assert_eq!(s.vaults[0].info.escrowed, 0);
        assert_eq!(s.positions.len(), 0);
        assert_eq!(s.deposits_held, 0);
        s.try_state().unwrap();
    }
    #[test]
    fn scalar_and_gate_families_update_per_branch_supply() {
        let mut s = LedgerState::new();
        s.create_vault(1, 0).unwrap();
        let a = acct(1);
        s.split(LedgerOrigin::Signed, 1, &a, 2_000_000).unwrap();
        s.split_scalar(LedgerOrigin::Signed, 1, Branch::Accept, &a, 500_000)
            .unwrap();
        s.split_gate(
            LedgerOrigin::Signed,
            1,
            Branch::Reject,
            GateType::Security,
            &a,
            700_000,
        )
        .unwrap();
        let v = s.vaults[0].info;
        assert_eq!(v.branches[0].usdc, 1_500_000);
        assert_eq!(v.branches[0].scalar_sets, 500_000);
        assert_eq!(v.branches[1].gate_sets[1], 700_000);
        s.merge_scalar(LedgerOrigin::Signed, 1, Branch::Accept, &a, 500_000)
            .unwrap();
        s.merge_gate(
            LedgerOrigin::Signed,
            1,
            Branch::Reject,
            GateType::Security,
            &a,
            700_000,
        )
        .unwrap();
        s.try_state().unwrap();
    }
    #[test]
    fn authority_state_machine_and_origin_misuse() {
        let mut s = LedgerState::<[u8; 32]>::new();
        s.create_vault(1, 0).unwrap();
        assert_eq!(
            s.resolve(LedgerOrigin::Signed, 1, Branch::Accept),
            Err(Error::BadOrigin)
        );
        s.resolve(LedgerOrigin::ResolveAuthority, 1, Branch::Accept)
            .unwrap();
        assert_eq!(
            s.split(LedgerOrigin::Signed, 1, &acct(1), 1_000_000),
            Err(Error::WrongVaultState)
        );
        assert_eq!(s.void(LedgerOrigin::ResolveAuthority, 1), Ok(()));
        assert_eq!(
            s.settle_scalar(LedgerOrigin::SettleAuthority, 1, FixedU64(500_000_000)),
            Err(Error::WrongVaultState)
        );
    }
    #[test]
    fn scalar_settlement_rounds_against_redeemer_and_pair_exact() {
        let mut s = LedgerState::new();
        s.create_vault(1, 0).unwrap();
        let a = acct(1);
        s.split(LedgerOrigin::Signed, 1, &a, 1_000_001).unwrap();
        s.split_scalar(LedgerOrigin::Signed, 1, Branch::Accept, &a, 1_000_001)
            .unwrap();
        s.resolve(LedgerOrigin::ResolveAuthority, 1, Branch::Accept)
            .unwrap();
        s.settle_scalar(LedgerOrigin::SettleAuthority, 1, FixedU64(333_333_333))
            .unwrap();
        s.redeem_scalar(1, ScalarSide::Long, &a, 1_000_001).unwrap();
        assert_eq!(s.vaults[0].info.escrowed, 666_668);
        let b = acct(2);
        s.split(LedgerOrigin::MarketAuthority, 1, &b, 1_000_000)
            .unwrap_err();
    }
    #[test]
    fn gate_and_void_redemption_follow_spec_schedule() {
        let mut s = LedgerState::new();
        s.create_vault(1, 0).unwrap();
        let a = acct(1);
        s.split(LedgerOrigin::Signed, 1, &a, 4_000_000).unwrap();
        s.split_gate(
            LedgerOrigin::Signed,
            1,
            Branch::Accept,
            GateType::Survival,
            &a,
            4_000_000,
        )
        .unwrap();
        s.void(LedgerOrigin::ResolveAuthority, 1).unwrap();
        s.redeem_void(
            1,
            Branch::Accept,
            PositionKind::GateYes(GateType::Survival),
            &a,
            4_000_000,
        )
        .unwrap();
        assert_eq!(s.vaults[0].info.escrowed, 3_000_000);
    }
    #[test]
    fn settled_gate_pays_winning_side_only() {
        let mut s = LedgerState::new();
        s.create_vault(1, 0).unwrap();
        let a = acct(1);
        s.split(LedgerOrigin::Signed, 1, &a, 1_000_000).unwrap();
        s.split_gate(
            LedgerOrigin::Signed,
            1,
            Branch::Accept,
            GateType::Security,
            &a,
            1_000_000,
        )
        .unwrap();
        s.resolve(LedgerOrigin::ResolveAuthority, 1, Branch::Accept)
            .unwrap();
        s.settle_scalar(LedgerOrigin::SettleAuthority, 1, FixedU64(500_000_000))
            .unwrap();
        assert_eq!(
            s.redeem_gate(1, GateType::Security, &a, 1_000_000),
            Err(Error::GateNotSettled)
        );
        s.settle_gate(LedgerOrigin::SettleAuthority, 1, GateType::Security, true)
            .unwrap();
        s.redeem_gate(1, GateType::Security, &a, 1_000_000).unwrap();
    }
    #[test]
    fn baseline_split_settle_redeem_pair_exact() {
        let mut s = LedgerState::new();
        s.create_baseline_vault(9).unwrap();
        let a = acct(1);
        s.split_baseline(LedgerOrigin::Signed, 9, &a, 1_000_001)
            .unwrap();
        s.settle_baseline(LedgerOrigin::SettleAuthority, 9, FixedU64(500_000_000))
            .unwrap();
        s.redeem_baseline_pair(9, &a, 1_000_001).unwrap();
        assert_eq!(s.baseline_vaults[0].info.escrowed, 0);
    }
    #[test]
    fn cap_applies_to_non_protocol_recipients() {
        let mut s = LedgerState::new();
        let a = acct(1);
        for i in 0..MAX_POSITIONS_PER_ACCOUNT {
            s.mint(baseline(i, ScalarSide::Long), &a, 1).unwrap();
        }
        assert_eq!(
            s.mint(baseline(99, ScalarSide::Long), &a, 1),
            Err(Error::PositionCapExceeded)
        );
        let p = acct(2);
        s.add_protocol_account(p);
        for i in 0..(MAX_POSITIONS_PER_ACCOUNT + 1) {
            s.mint(baseline(i, ScalarSide::Long), &p, 1).unwrap();
        }
    }
}
