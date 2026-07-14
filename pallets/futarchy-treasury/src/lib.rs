#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::{AccountId, Balance, BlockNumber, EpochId, ProposalClass, H256};
use pallet_origins::Origin;
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub const USDC: Balance = 1_000_000;
pub const VIT: Balance = 1_000_000_000_000;
pub const MAX_BUDGET_LINES: usize = 32;
pub const MAX_STREAMS: usize = 128;
pub const MAX_PENDING_OUTFLOWS: usize = 64;
pub const MAX_POL_COMMITMENTS: usize = 196;
pub const TRS_CAP_PROPOSAL_BPS: u32 = 500;
pub const TRS_CAP_30D_BPS: u32 = 1_000;
pub const TRS_CAP_180D_BPS: u32 = 3_000;
pub const TRS_STREAM_THRESHOLD_BPS: u32 = 100;
pub const ISS_INFLATION_CAP_BPS: u32 = 200;
pub const DAY_BLOCKS: BlockNumber = 14_400;
pub const DAYS_30_BLOCKS: BlockNumber = 432_000;
pub const DAYS_180_BLOCKS: BlockNumber = 2_592_000;
pub const DAYS_365_BLOCKS: BlockNumber = 5_256_000;
pub const KEEPER_BUDGET_EPOCH: Balance = 12_000 * USDC;
pub const COLLATOR_COMP_EPOCH: Balance = 2_000 * USDC;
pub const MAX_FUNDED_CORETIME_PERIODS: usize = 8;

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum TreasuryAccount {
    Main,
    Pol,
    PolBaseline,
    Insurance,
    Keeper,
    Oracle,
    Rewards,
    Ops,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum BudgetLine {
    Pol,
    PolBaseline,
    Keeper,
    Oracle,
    Rewards,
    OpsBootnodes,
    OpsRpcArchive,
    OpsCollators,
    OpsKeepers,
    OpsOracleEvidence,
    OpsWatchtowers,
    OpsMonitoring,
    OpsArweave,
    OpsCoretime,
}

impl BudgetLine {
    pub const fn account(self) -> TreasuryAccount {
        match self {
            Self::Pol => TreasuryAccount::Pol,
            Self::PolBaseline => TreasuryAccount::PolBaseline,
            Self::Keeper => TreasuryAccount::Keeper,
            Self::Oracle => TreasuryAccount::Oracle,
            Self::Rewards => TreasuryAccount::Rewards,
            Self::OpsBootnodes
            | Self::OpsRpcArchive
            | Self::OpsCollators
            | Self::OpsKeepers
            | Self::OpsOracleEvidence
            | Self::OpsWatchtowers
            | Self::OpsMonitoring
            | Self::OpsArweave
            | Self::OpsCoretime => TreasuryAccount::Ops,
        }
    }
    pub const fn vit_issuance_allowed(self) -> bool {
        matches!(
            self,
            Self::Rewards
                | Self::OpsBootnodes
                | Self::OpsRpcArchive
                | Self::OpsCollators
                | Self::OpsKeepers
                | Self::OpsOracleEvidence
                | Self::OpsWatchtowers
                | Self::OpsMonitoring
                | Self::OpsArweave
                | Self::OpsCoretime
        )
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum AssetKind {
    Usdc,
    Vit,
    Foreign(H256),
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct NavView {
    pub nav: Balance,
    pub spendable_nav: Balance,
    pub reserve_impaired: bool,
    pub meter_utilization_bps: u32,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct StreamInput {
    pub line: BudgetLine,
    pub recipient: AccountId,
    pub total: Balance,
    pub start: BlockNumber,
    pub duration: BlockNumber,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct Stream {
    pub id: u64,
    pub recipient: AccountId,
    pub line: BudgetLine,
    pub total: Balance,
    pub claimed: Balance,
    pub start: BlockNumber,
    pub duration: BlockNumber,
    pub cancelled: bool,
}

/// Rolling outflow meter over a trailing window (08 §1.3, I-7).
///
/// Charges accumulate in per-day buckets; `BUCKETS` is the window length in
/// days **plus one**, so a bucket is evicted only once every spend in it is
/// strictly older than the window (a spend late on day d stays counted
/// through day d + BUCKETS − 1). Coverage is therefore always at least the
/// window — the conservative direction. A fixed reset-at-boundary window
/// would admit up to twice the cap across the seam.
#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct RollingMeter<const BUCKETS: usize> {
    pub limit_bps: u32,
    pub buckets: [Balance; BUCKETS],
    pub last_day: BlockNumber,
}

impl<const BUCKETS: usize> RollingMeter<BUCKETS> {
    pub const fn new(limit_bps: u32) -> Self {
        Self {
            limit_bps,
            buckets: [0; BUCKETS],
            last_day: 0,
        }
    }
    /// Evict buckets that fell out of the trailing window.
    pub fn roll(&mut self, now: BlockNumber) {
        let day = now / DAY_BLOCKS;
        if day <= self.last_day {
            return;
        }
        let gap = day - self.last_day;
        if gap as usize >= BUCKETS {
            self.buckets = [0; BUCKETS];
        } else {
            for offset in 1..=gap {
                let index = (self.last_day.saturating_add(offset)) as usize % BUCKETS;
                self.buckets[index] = 0;
            }
        }
        self.last_day = day;
    }
    pub fn spent(&self) -> Balance {
        self.buckets
            .iter()
            .copied()
            .fold(0, Balance::saturating_add)
    }
    /// Whether a charge would fit; call [`Self::roll`] first.
    pub fn can_charge(&self, nav: Balance, amount: Balance) -> Result<(), Error> {
        let limit = bps(nav, self.limit_bps)?;
        let next = self.spent().checked_add(amount).ok_or(Error::Overflow)?;
        ensure!(next <= limit, Error::MeterExhausted);
        Ok(())
    }
    /// Record a charge that [`Self::can_charge`] already admitted.
    pub fn add(&mut self, now: BlockNumber, amount: Balance) -> Result<(), Error> {
        let index = (now / DAY_BLOCKS) as usize % BUCKETS;
        self.buckets[index] = self.buckets[index]
            .checked_add(amount)
            .ok_or(Error::Overflow)?;
        Ok(())
    }
    /// Roll, check, and record in one step.
    pub fn charge(&mut self, now: BlockNumber, nav: Balance, amount: Balance) -> Result<(), Error> {
        self.roll(now);
        self.can_charge(nav, amount)?;
        self.add(now, amount)
    }
    pub fn utilization_bps(&self, nav: Balance) -> u32 {
        let limit = bps(nav, self.limit_bps).unwrap_or(0);
        if limit == 0 {
            0
        } else {
            self.spent()
                .saturating_mul(10_000)
                .saturating_div(limit)
                .min(10_000) as u32
        }
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct IssuanceMeter {
    pub supply_at_window_start: Balance,
    pub minted: Balance,
    pub window_start: BlockNumber,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub enum Event {
    Spent {
        line: BudgetLine,
        dest: AccountId,
        amount: Balance,
    },
    StreamOpened {
        id: u64,
        recipient: AccountId,
        total: Balance,
    },
    StreamClaimed {
        id: u64,
        recipient: AccountId,
        amount: Balance,
    },
    StreamCancelled {
        id: u64,
        reverted: Balance,
    },
    BudgetLineFunded {
        line: BudgetLine,
        amount: Balance,
    },
    VitIssued {
        amount: Balance,
        line: BudgetLine,
        meter_after: Balance,
    },
    NavHaircutFlagged {
        epoch: EpochId,
        flag: bool,
    },
    ForeignRecovered {
        asset: AssetKind,
        dest: AccountId,
        amount: Balance,
    },
    CoretimeRenewalCalled {
        line: BudgetLine,
        amount: Balance,
    },
    NavFloorUnmet {
        class: ProposalClass,
        nav: Balance,
        floor: Balance,
    },
    KeeperBudgetLow {
        remaining: Balance,
    },
    KeeperBudgetExhausted,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Error {
    BadOrigin,
    UnknownBudgetLine,
    InsufficientFunds,
    ReserveImpaired,
    ProposalCapExceeded,
    StreamRequired,
    MeterExhausted,
    StreamNotFound,
    StreamNotClaimable,
    NotRecipient,
    AlreadyCancelled,
    BadDuration,
    RenewalWindowClosed,
    PeriodAlreadyFunded,
    TooManyStreams,
    TooManyBudgetLines,
    TooManyObligations,
    IssuanceLineNotAllowed,
    IssuanceCapExceeded,
    UnknownForeignAsset,
    NavFloorUnmet,
    Overflow,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct Treasury {
    pub main_usdc: Balance,
    pub vit_supply: Balance,
    pub reserve_impaired: bool,
    pub lines: Vec<(BudgetLine, Balance)>,
    pub streams: Vec<Stream>,
    pub pending_outflows: Vec<Balance>,
    pub pol_commitments: Vec<Balance>,
    pub meter_30d: RollingMeter<31>,
    pub meter_180d: RollingMeter<181>,
    pub issuance: IssuanceMeter,
    pub events: Vec<Event>,
    pub next_stream_id: u64,
    /// VIT balances credited to budget lines by `issue_vit` (08 §2.3) —
    /// tracked apart from the USDC line balances so NAV (USDC at par, VIT
    /// marked 0 per 08 §1.2) never counts minted VIT.
    pub vit_lines: Vec<(BudgetLine, Balance)>,
    /// Coretime periods already funded via `execute_coretime_renewal`
    /// (09 §4 idempotency key), most recent last.
    pub funded_coretime_periods: Vec<u32>,
    /// Open renewal quotes `(period_index, price)` noted by the runtime from
    /// Coretime-chain state (09 §4/§6). A present quote is what makes the
    /// renewal window open; the price never comes from the caller.
    pub coretime_quotes: Vec<(u32, Balance)>,
}

impl Default for Treasury {
    fn default() -> Self {
        Self {
            main_usdc: 0,
            vit_supply: 1_000_000_000 * VIT,
            reserve_impaired: false,
            lines: Vec::new(),
            streams: Vec::new(),
            pending_outflows: Vec::new(),
            pol_commitments: Vec::new(),
            meter_30d: RollingMeter::new(TRS_CAP_30D_BPS),
            meter_180d: RollingMeter::new(TRS_CAP_180D_BPS),
            issuance: IssuanceMeter {
                supply_at_window_start: 1_000_000_000 * VIT,
                minted: 0,
                window_start: 0,
            },
            events: Vec::new(),
            next_stream_id: 0,
            vit_lines: Vec::new(),
            funded_coretime_periods: Vec::new(),
            coretime_quotes: Vec::new(),
        }
    }
}

impl Treasury {
    pub fn fund_budget_line(
        &mut self,
        origin: Origin,
        line: BudgetLine,
        amount: Balance,
    ) -> Result<(), Error> {
        ensure!(origin == Origin::FutarchyTreasury, Error::BadOrigin);
        self.debit_main(amount)?;
        self.credit_line(line, amount)?;
        self.events.push(Event::BudgetLineFunded { line, amount });
        Ok(())
    }
    pub fn spend(
        &mut self,
        origin: Origin,
        now: BlockNumber,
        line: BudgetLine,
        dest: AccountId,
        amount: Balance,
    ) -> Result<(), Error> {
        ensure!(origin == Origin::FutarchyTreasury, Error::BadOrigin);
        self.ensure_spendable()?;
        // 08 §1.3: grants above trs.stream_threshold = 1% NAV MUST stream —
        // a direct spend must not bypass vesting and later cancellability.
        ensure!(
            amount <= bps(self.nav().nav, TRS_STREAM_THRESHOLD_BPS)?,
            Error::StreamRequired
        );
        self.enforce_outflow(now, amount)?;
        self.debit_line(line, amount)?;
        self.events.push(Event::Spent { line, dest, amount });
        Ok(())
    }
    pub fn open_stream(
        &mut self,
        origin: Origin,
        now: BlockNumber,
        input: StreamInput,
    ) -> Result<u64, Error> {
        ensure!(origin == Origin::FutarchyTreasury, Error::BadOrigin);
        self.ensure_spendable()?;
        ensure!(input.duration > 0, Error::BadDuration);
        ensure!(self.streams.len() < MAX_STREAMS, Error::TooManyStreams);
        let nav = self.nav().nav;
        ensure!(
            input.total > bps(nav, TRS_STREAM_THRESHOLD_BPS)?,
            Error::StreamRequired
        );
        self.enforce_outflow(now, input.total)?;
        self.debit_line(input.line, input.total)?;
        let id = self.next_stream_id;
        self.next_stream_id = self.next_stream_id.checked_add(1).ok_or(Error::Overflow)?;
        self.streams.push(Stream {
            id,
            recipient: input.recipient,
            line: input.line,
            total: input.total,
            claimed: 0,
            start: input.start,
            duration: input.duration,
            cancelled: false,
        });
        self.events.push(Event::StreamOpened {
            id,
            recipient: input.recipient,
            total: input.total,
        });
        Ok(id)
    }
    pub fn claim_stream(
        &mut self,
        who: AccountId,
        now: BlockNumber,
        id: u64,
    ) -> Result<Balance, Error> {
        let idx = self
            .streams
            .iter()
            .position(|s| s.id == id)
            .ok_or(Error::StreamNotFound)?;
        let s = self.streams[idx];
        ensure!(s.recipient == who, Error::NotRecipient);
        ensure!(!s.cancelled, Error::AlreadyCancelled);
        let vested = vested_amount(s.total, s.start, s.duration, now);
        let claimable = vested.checked_sub(s.claimed).ok_or(Error::Overflow)?;
        ensure!(claimable > 0, Error::StreamNotClaimable);
        self.streams[idx].claimed = vested;
        self.events.push(Event::StreamClaimed {
            id,
            recipient: who,
            amount: claimable,
        });
        Ok(claimable)
    }
    pub fn cancel_stream(&mut self, origin: Origin, id: u64) -> Result<Balance, Error> {
        ensure!(origin == Origin::FutarchyTreasury, Error::BadOrigin);
        let idx = self
            .streams
            .iter()
            .position(|s| s.id == id)
            .ok_or(Error::StreamNotFound)?;
        ensure!(!self.streams[idx].cancelled, Error::AlreadyCancelled);
        let remainder = self.streams[idx]
            .total
            .checked_sub(self.streams[idx].claimed)
            .ok_or(Error::Overflow)?;
        self.streams[idx].cancelled = true;
        self.main_usdc = self
            .main_usdc
            .checked_add(remainder)
            .ok_or(Error::Overflow)?;
        self.events.push(Event::StreamCancelled {
            id,
            reverted: remainder,
        });
        Ok(remainder)
    }
    pub fn issue_vit(
        &mut self,
        origin: Origin,
        now: BlockNumber,
        amount: Balance,
        line: BudgetLine,
    ) -> Result<(), Error> {
        ensure!(origin == Origin::FutarchyTreasury, Error::BadOrigin);
        ensure!(line.vit_issuance_allowed(), Error::IssuanceLineNotAllowed);
        if now >= self.issuance.window_start.saturating_add(DAYS_365_BLOCKS) {
            self.issuance.window_start = now;
            self.issuance.minted = 0;
            self.issuance.supply_at_window_start = self.vit_supply;
        }
        let cap = bps(self.issuance.supply_at_window_start, ISS_INFLATION_CAP_BPS)?;
        let next = self
            .issuance
            .minted
            .checked_add(amount)
            .ok_or(Error::Overflow)?;
        ensure!(next <= cap, Error::IssuanceCapExceeded);
        self.issuance.minted = next;
        self.vit_supply = self.vit_supply.checked_add(amount).ok_or(Error::Overflow)?;
        // 08 §2.3: the minted VIT is credited to the requested REWARDS/ops
        // line so the line can actually disburse it.
        if let Some((_, bal)) = self.vit_lines.iter_mut().find(|(l, _)| *l == line) {
            *bal = bal.checked_add(amount).ok_or(Error::Overflow)?;
        } else {
            ensure!(
                self.vit_lines.len() < MAX_BUDGET_LINES,
                Error::TooManyBudgetLines
            );
            self.vit_lines.push((line, amount));
        }
        self.events.push(Event::VitIssued {
            amount,
            line,
            meter_after: next,
        });
        Ok(())
    }

    pub fn line_balance(&self, line: BudgetLine) -> Balance {
        self.lines
            .iter()
            .find(|(l, _)| *l == line)
            .map(|(_, b)| *b)
            .unwrap_or(0)
    }

    pub fn vit_line_balance(&self, line: BudgetLine) -> Balance {
        self.vit_lines
            .iter()
            .find(|(l, _)| *l == line)
            .map(|(_, b)| *b)
            .unwrap_or(0)
    }
    pub fn recover_foreign(
        &mut self,
        origin: Origin,
        asset: AssetKind,
        dest: AccountId,
        amount: Balance,
    ) -> Result<(), Error> {
        ensure!(origin == Origin::FutarchyTreasury, Error::BadOrigin);
        ensure!(
            !matches!(asset, AssetKind::Usdc | AssetKind::Vit),
            Error::UnknownForeignAsset
        );
        self.events.push(Event::ForeignRecovered {
            asset,
            dest,
            amount,
        });
        Ok(())
    }
    /// Note the current renewal quote for a coretime period. Runtime context
    /// read from Coretime-chain state (09 §4/§6) — never a user-facing call;
    /// a present quote is what makes the renewal window open.
    pub fn note_coretime_renewal_quote(
        &mut self,
        period_index: u32,
        price: Balance,
    ) -> Result<(), Error> {
        ensure!(
            !self.funded_coretime_periods.contains(&period_index),
            Error::PeriodAlreadyFunded
        );
        if let Some((_, quoted)) = self
            .coretime_quotes
            .iter_mut()
            .find(|(p, _)| *p == period_index)
        {
            *quoted = price;
            return Ok(());
        }
        ensure!(
            self.coretime_quotes.len() < MAX_FUNDED_CORETIME_PERIODS,
            Error::TooManyObligations
        );
        self.coretime_quotes.push((period_index, price));
        Ok(())
    }

    /// 09 §4 `execute_coretime_renewal(period_index)`: permissionless
    /// (Signed, keeper-rebated), spends only from the pre-authorized
    /// `ops.coretime` line (the line balance is the bound — no NAV meters),
    /// exempt from every freeze including the reserve-health flag, idempotent
    /// per coretime period, and a no-op error while the renewal window is
    /// closed. The paid amount is the runtime-noted quote — a permissionless
    /// caller can neither mark a period funded for free nor drain the line.
    pub fn execute_coretime_renewal(
        &mut self,
        _keeper: AccountId,
        period_index: u32,
    ) -> Result<(), Error> {
        ensure!(
            !self.funded_coretime_periods.contains(&period_index),
            Error::PeriodAlreadyFunded
        );
        let quote_index = self
            .coretime_quotes
            .iter()
            .position(|(p, _)| *p == period_index)
            .ok_or(Error::RenewalWindowClosed)?;
        let (_, price) = self.coretime_quotes[quote_index];
        self.debit_line(BudgetLine::OpsCoretime, price)?;
        self.coretime_quotes.remove(quote_index);
        if self.funded_coretime_periods.len() >= MAX_FUNDED_CORETIME_PERIODS {
            self.funded_coretime_periods.remove(0);
        }
        self.funded_coretime_periods.push(period_index);
        self.events.push(Event::CoretimeRenewalCalled {
            line: BudgetLine::OpsCoretime,
            amount: price,
        });
        Ok(())
    }
    pub fn set_reserve_impaired(&mut self, epoch: EpochId, flag: bool) {
        if self.reserve_impaired != flag {
            self.reserve_impaired = flag;
            self.events.push(Event::NavHaircutFlagged { epoch, flag });
        }
    }
    pub fn nav(&self) -> NavView {
        let obligations = self
            .open_stream_remainders()
            .saturating_add(sum(&self.pending_outflows))
            .saturating_add(sum(&self.pol_commitments));
        let nav = self
            .main_usdc
            .saturating_add(sum_lines(&self.lines))
            .saturating_sub(obligations);
        let spendable_nav = if self.reserve_impaired { 0 } else { nav };
        NavView {
            nav,
            spendable_nav,
            reserve_impaired: self.reserve_impaired,
            meter_utilization_bps: self
                .meter_30d
                .utilization_bps(nav)
                .max(self.meter_180d.utilization_bps(nav)),
        }
    }
    pub fn floor(class: ProposalClass) -> Balance {
        match class {
            ProposalClass::Param => 1_848_400 * USDC,
            ProposalClass::Treasury => 7_393_600 * USDC,
            ProposalClass::Code => 13_862_944 * USDC,
            ProposalClass::Meta => 21_256_533 * USDC,
            ProposalClass::Constitutional => 21_256_533 * USDC,
        }
    }
    pub fn ensure_nav_floor(&mut self, class: ProposalClass) -> Result<(), Error> {
        let nav = self.nav().spendable_nav;
        let floor = Self::floor(class);
        if nav < floor {
            self.events.push(Event::NavFloorUnmet { class, nav, floor });
            return Err(Error::NavFloorUnmet);
        }
        Ok(())
    }
    pub fn try_state(&self) -> Result<(), Error> {
        ensure!(
            self.lines.len() <= MAX_BUDGET_LINES,
            Error::TooManyBudgetLines
        );
        ensure!(self.streams.len() <= MAX_STREAMS, Error::TooManyStreams);
        ensure!(
            self.pending_outflows.len() <= MAX_PENDING_OUTFLOWS,
            Error::TooManyObligations
        );
        ensure!(
            self.pol_commitments.len() <= MAX_POL_COMMITMENTS,
            Error::TooManyObligations
        );
        for s in &self.streams {
            ensure!(s.claimed <= s.total, Error::Overflow);
            ensure!(s.duration > 0, Error::BadDuration);
        }
        ensure!(
            self.vit_lines.len() <= MAX_BUDGET_LINES,
            Error::TooManyBudgetLines
        );
        let vit_in_lines = self
            .vit_lines
            .iter()
            .map(|(_, b)| *b)
            .fold(0, Balance::saturating_add);
        ensure!(vit_in_lines <= self.vit_supply, Error::Overflow);
        ensure!(
            self.funded_coretime_periods.len() <= MAX_FUNDED_CORETIME_PERIODS,
            Error::TooManyObligations
        );
        ensure!(
            self.coretime_quotes.len() <= MAX_FUNDED_CORETIME_PERIODS,
            Error::TooManyObligations
        );
        Ok(())
    }
    fn ensure_spendable(&self) -> Result<(), Error> {
        ensure!(!self.reserve_impaired, Error::ReserveImpaired);
        Ok(())
    }
    fn enforce_outflow(&mut self, now: BlockNumber, amount: Balance) -> Result<(), Error> {
        let nav = self.nav().spendable_nav;
        ensure!(
            amount <= bps(nav, TRS_CAP_PROPOSAL_BPS)?,
            Error::ProposalCapExceeded
        );
        // Check both windows before recording in either: a rejected outflow
        // must not consume meter capacity.
        self.meter_30d.roll(now);
        self.meter_180d.roll(now);
        self.meter_30d.can_charge(nav, amount)?;
        self.meter_180d.can_charge(nav, amount)?;
        self.meter_30d.add(now, amount)?;
        self.meter_180d.add(now, amount)
    }
    fn debit_main(&mut self, amount: Balance) -> Result<(), Error> {
        ensure!(self.main_usdc >= amount, Error::InsufficientFunds);
        self.main_usdc -= amount;
        Ok(())
    }
    fn credit_line(&mut self, line: BudgetLine, amount: Balance) -> Result<(), Error> {
        if let Some((_, bal)) = self.lines.iter_mut().find(|(l, _)| *l == line) {
            *bal = bal.checked_add(amount).ok_or(Error::Overflow)?;
        } else {
            ensure!(
                self.lines.len() < MAX_BUDGET_LINES,
                Error::TooManyBudgetLines
            );
            self.lines.push((line, amount));
        }
        Ok(())
    }
    fn debit_line(&mut self, line: BudgetLine, amount: Balance) -> Result<(), Error> {
        let (_, bal) = self
            .lines
            .iter_mut()
            .find(|(l, _)| *l == line)
            .ok_or(Error::UnknownBudgetLine)?;
        ensure!(*bal >= amount, Error::InsufficientFunds);
        *bal -= amount;
        Ok(())
    }
    fn open_stream_remainders(&self) -> Balance {
        self.streams
            .iter()
            .filter(|s| !s.cancelled)
            .fold(0, |acc, s| {
                acc.saturating_add(s.total.saturating_sub(s.claimed))
            })
    }
}

pub fn bps(amount: Balance, bps: u32) -> Result<Balance, Error> {
    amount
        .checked_mul(bps as Balance)
        .ok_or(Error::Overflow)?
        .checked_div(10_000)
        .ok_or(Error::Overflow)
}
pub fn vested_amount(
    total: Balance,
    start: BlockNumber,
    duration: BlockNumber,
    now: BlockNumber,
) -> Balance {
    if now <= start {
        0
    } else {
        let elapsed = now.saturating_sub(start).min(duration);
        total
            .saturating_mul(elapsed as Balance)
            .saturating_div(duration as Balance)
    }
}
fn sum(v: &[Balance]) -> Balance {
    v.iter().copied().fold(0, Balance::saturating_add)
}
fn sum_lines(v: &[(BudgetLine, Balance)]) -> Balance {
    v.iter().map(|(_, b)| *b).fold(0, Balance::saturating_add)
}

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking {
    pub fn benchmark_stub() -> bool {
        true
    }
}

macro_rules! ensure {
    ($cond:expr, $err:expr $(,)?) => {
        if !$cond {
            return Err($err);
        }
    };
}
use ensure;

#[cfg(test)]
mod tests {
    use super::*;
    const TREASURY: Origin = Origin::FutarchyTreasury;
    fn acct(n: u8) -> AccountId {
        [n; 32]
    }
    fn funded() -> Treasury {
        let mut t = Treasury {
            main_usdc: 25_000_000 * USDC,
            ..Default::default()
        };
        t.fund_budget_line(TREASURY, BudgetLine::OpsCollators, 1_500_000 * USDC)
            .unwrap();
        t.fund_budget_line(TREASURY, BudgetLine::Rewards, 1_000_000 * USDC)
            .unwrap();
        t.fund_budget_line(TREASURY, BudgetLine::OpsCoretime, 500_000 * USDC)
            .unwrap();
        t
    }
    #[test]
    fn spend_requires_treasury_origin_and_caps() {
        let mut t = funded();
        assert_eq!(
            t.spend(
                Origin::FutarchyParam,
                0,
                BudgetLine::OpsCollators,
                acct(1),
                1
            )
            .unwrap_err(),
            Error::BadOrigin
        );
        // 08 §1.3: a direct spend above trs.stream_threshold = 1% NAV must
        // stream instead (Codex review, PR #21) - it must not pay out
        // immediately merely because it is below the 5% proposal cap.
        assert_eq!(
            t.spend(
                TREASURY,
                0,
                BudgetLine::OpsCollators,
                acct(1),
                300_000 * USDC
            )
            .unwrap_err(),
            Error::StreamRequired
        );
        // The 5% proposal cap still binds stream openings.
        assert_eq!(
            t.open_stream(
                TREASURY,
                0,
                StreamInput {
                    line: BudgetLine::OpsCollators,
                    recipient: acct(2),
                    total: 2_000_000 * USDC,
                    start: 0,
                    duration: 100,
                }
            )
            .unwrap_err(),
            Error::ProposalCapExceeded
        );
        t.spend(
            TREASURY,
            0,
            BudgetLine::OpsCollators,
            acct(1),
            100_000 * USDC,
        )
        .unwrap();
    }
    #[test]
    fn reserve_haircut_sets_spendable_zero_and_blocks_new_commitments() {
        let mut t = funded();
        t.set_reserve_impaired(7, true);
        let nav = t.nav();
        assert!(nav.reserve_impaired);
        assert_eq!(nav.spendable_nav, 0);
        assert_eq!(
            t.spend(TREASURY, 0, BudgetLine::OpsCollators, acct(1), 1)
                .unwrap_err(),
            Error::ReserveImpaired
        );
        assert_eq!(
            t.ensure_nav_floor(ProposalClass::Param).unwrap_err(),
            Error::NavFloorUnmet
        );
    }
    #[test]
    fn streams_are_mandatory_above_threshold_claimable_and_cancellable() {
        let mut t = funded();
        assert_eq!(
            t.open_stream(
                TREASURY,
                0,
                StreamInput {
                    line: BudgetLine::OpsCollators,
                    recipient: acct(2),
                    total: 10_000 * USDC,
                    start: 0,
                    duration: 100
                }
            )
            .unwrap_err(),
            Error::StreamRequired
        );
        let id = t
            .open_stream(
                TREASURY,
                0,
                StreamInput {
                    line: BudgetLine::OpsCollators,
                    recipient: acct(2),
                    total: 300_000 * USDC,
                    start: 10,
                    duration: 100,
                },
            )
            .unwrap();
        assert_eq!(
            t.claim_stream(acct(9), 60, id).unwrap_err(),
            Error::NotRecipient
        );
        assert_eq!(t.claim_stream(acct(2), 60, id).unwrap(), 150_000 * USDC);
        assert_eq!(t.cancel_stream(TREASURY, id).unwrap(), 150_000 * USDC);
    }
    #[test]
    fn issuance_meter_allows_only_rewards_or_ops_and_caps_two_percent() {
        let mut t = funded();
        assert_eq!(
            t.issue_vit(TREASURY, 0, 1, BudgetLine::Pol).unwrap_err(),
            Error::IssuanceLineNotAllowed
        );
        let cap = 20_000_000 * VIT;
        t.issue_vit(TREASURY, 0, cap, BudgetLine::Rewards).unwrap();
        // Codex review, PR #21: the minted VIT must land on the requested
        // line, not only in the supply/meter accounting.
        assert_eq!(t.vit_line_balance(BudgetLine::Rewards), cap);
        assert_eq!(
            t.issue_vit(TREASURY, 0, 1, BudgetLine::Rewards)
                .unwrap_err(),
            Error::IssuanceCapExceeded
        );
        t.issue_vit(TREASURY, DAYS_365_BLOCKS, 1, BudgetLine::OpsArweave)
            .unwrap();
        assert_eq!(t.vit_line_balance(BudgetLine::OpsArweave), 1);
        t.try_state().unwrap();
    }
    #[test]
    fn coretime_renewal_is_permissionless_line_bounded_and_idempotent() {
        // 09 §4 (Codex review, PR #21): the renewal is a Signed keeper path -
        // requiring the FutarchyTreasury origin would keep the B-16 wedge
        // closed on paper only, since no decision can execute under a freeze.
        let mut t = funded();
        t.set_reserve_impaired(1, true);
        // No runtime-noted quote: the renewal window is closed.
        assert_eq!(
            t.execute_coretime_renewal(acct(7), 42).unwrap_err(),
            Error::RenewalWindowClosed
        );
        // The paid amount is the noted quote (Codex review, PR #32): a
        // permissionless keeper can neither fund a period for free nor pick
        // the amount.
        t.note_coretime_renewal_quote(42, 100_000 * USDC).unwrap();
        let line_before = t.line_balance(BudgetLine::OpsCoretime);
        t.execute_coretime_renewal(acct(7), 42).unwrap();
        assert_eq!(
            t.line_balance(BudgetLine::OpsCoretime),
            line_before - 100_000 * USDC
        );
        assert!(matches!(
            t.events.last(),
            Some(Event::CoretimeRenewalCalled {
                amount,
                ..
            }) if *amount == 100_000 * USDC
        ));
        // Idempotent per period_index - even against a re-noted quote.
        assert_eq!(
            t.note_coretime_renewal_quote(42, 1).unwrap_err(),
            Error::PeriodAlreadyFunded
        );
        assert_eq!(
            t.execute_coretime_renewal(acct(8), 42).unwrap_err(),
            Error::PeriodAlreadyFunded
        );
        // Bounded solely by the pre-authorized line balance.
        t.note_coretime_renewal_quote(43, 500_000 * USDC).unwrap();
        assert_eq!(
            t.execute_coretime_renewal(acct(8), 43).unwrap_err(),
            Error::InsufficientFunds
        );
        t.note_coretime_renewal_quote(43, 400_000 * USDC).unwrap();
        t.execute_coretime_renewal(acct(8), 43).unwrap();
        t.try_state().unwrap();
    }

    #[test]
    fn outflow_meters_roll_across_the_window_boundary() {
        // Codex review, PR #21: with a fixed window, 10% NAV at the last block
        // of the window plus 10% at the first block of the next passed while
        // the trailing 30 days held 20%. The bucketed meter rejects it.
        let nav = 1_000_000 * USDC;
        let tenth = 100_000 * USDC;
        let mut m: RollingMeter<31> = RollingMeter::new(TRS_CAP_30D_BPS);
        let day29 = DAYS_30_BLOCKS - 1;
        m.charge(day29, nav, tenth).unwrap();
        assert_eq!(
            m.charge(DAYS_30_BLOCKS, nav, tenth).unwrap_err(),
            Error::MeterExhausted
        );
        // Once the loaded day falls out of the trailing window, capacity
        // returns.
        m.charge(day29 + DAYS_30_BLOCKS + DAY_BLOCKS, nav, tenth)
            .unwrap();
    }

    #[test]
    fn boundary_day_spend_is_retained_for_the_full_window() {
        // Codex review, PR #32: a spend on the last block of day 0 is only
        // ~29 days old at day 30's first block; the eviction ring must keep
        // it counted (window + 1 buckets), and release it a day later.
        let nav = 1_000_000 * USDC;
        let tenth = 100_000 * USDC;
        let mut m: RollingMeter<31> = RollingMeter::new(TRS_CAP_30D_BPS);
        m.charge(DAY_BLOCKS - 1, nav, tenth).unwrap(); // last block of day 0
        assert_eq!(
            m.charge(DAYS_30_BLOCKS, nav, tenth).unwrap_err(), // first block of day 30
            Error::MeterExhausted
        );
        // At day 31 the day-0 spend is strictly older than 30 days.
        m.charge(DAYS_30_BLOCKS + DAY_BLOCKS, nav, tenth).unwrap();
    }

    #[test]
    fn rejected_outflows_do_not_consume_meter_capacity() {
        // Codex review, PR #21: a spend that passes the 30d meter but fails
        // the 180d meter must leave both meters unchanged.
        let mut t = funded();
        let nav = t.nav().spendable_nav;
        // Pre-load the 180d meter to its cap so any further outflow fails it.
        let cap_180 = bps(nav, TRS_CAP_180D_BPS).unwrap();
        t.meter_180d.buckets[0] = cap_180;
        let before_30 = t.meter_30d.spent();
        assert_eq!(
            t.spend(TREASURY, 0, BudgetLine::OpsCollators, acct(1), USDC)
                .unwrap_err(),
            Error::MeterExhausted
        );
        assert_eq!(t.meter_30d.spent(), before_30);
        assert_eq!(t.meter_180d.spent(), cap_180);
        // The capacity is still there for a spend that fits both windows.
        t.meter_180d.buckets[0] = 0;
        t.spend(TREASURY, 0, BudgetLine::OpsCollators, acct(1), USDC)
            .unwrap();
    }
    #[test]
    fn try_state_checks_bounds() {
        let mut t = funded();
        t.streams.push(Stream {
            id: 0,
            recipient: acct(1),
            line: BudgetLine::Rewards,
            total: 1,
            claimed: 2,
            start: 0,
            duration: 1,
            cancelled: false,
        });
        assert_eq!(t.try_state().unwrap_err(), Error::Overflow);
    }
}
