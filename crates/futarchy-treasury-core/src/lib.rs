#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::{
    chain_identity::DOT_PLANCKS_PER_DOT, AccountId, Balance, BlockNumber, EpochId, ProposalClass,
    H256,
};
use origins_core::Origin;
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

// Currency units and the genesis VIT supply are chain-identity constants owned
// by `futarchy-primitives` (rule 4 / 13 rule 1); re-exported here under the
// names this core and its FRAME shell consume.
pub const USDC: Balance = futarchy_primitives::currency::USDC;
pub const VIT: Balance = futarchy_primitives::currency::VIT;
/// Genesis VIT supply (08 §2.1 / 02 §8 identity): 1,000,000,000 VIT, fixed at
/// genesis — sourced from [`futarchy_primitives::currency::VIT_TOTAL_SUPPLY`].
pub const DEFAULT_VIT_SUPPLY: Balance = futarchy_primitives::currency::VIT_TOTAL_SUPPLY;
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

/// The two classes governed by the 08 §6.3 keeper meter. Oracle work is paid
/// separately by [`Treasury::oracle_line_rebate`].
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum KeeperMeterClass {
    DecisionCritical,
    General,
}

/// Plain scalar per-epoch keeper meter (08 §6.3).
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Default,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub struct KeeperMeter {
    pub epoch: EpochId,
    pub spent: Balance,
    pub general_spent: Balance,
    pub low_emitted: bool,
    pub exhausted_emitted: bool,
}

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

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
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
    /// Reserve-transferability probe execution + response-delivery fees (07 §8).
    OpsReserveProbe,
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
            | Self::OpsCoretime
            | Self::OpsReserveProbe => TreasuryAccount::Ops,
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
                | Self::OpsReserveProbe
        )
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum AssetKind {
    Usdc,
    Vit,
    Foreign(H256),
}

/// Internal NAV computation (08 §1.2): the treasury's own solvency view. Named
/// distinctly from the frozen 02 §4 `NavView` runtime-API type (in
/// `futarchy-primitives`, a 13-field account decomposition) which the B2
/// `FutarchyApi` builds from these components plus the line balances (rule 5).
#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct NavComponents {
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

/// One authenticated, open Coretime renewal quote (09 §4).
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub struct CoretimeQuote {
    pub period_index: u32,
    pub price: Balance,
    pub noted_at: u64,
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
    ReserveProbeFeeCharged {
        line: BudgetLine,
        amount: Balance,
    },
    CoretimeQuoteNoted {
        period_index: u32,
        price: Balance,
    },
    CoretimeQuotePruned {
        period_index: u32,
    },
    NavFloorUnmet {
        class: ProposalClass,
        nav: Balance,
        floor: Balance,
    },
    KeeperBudgetLow {
        remaining: Balance,
    },
    KeeperBudgetExhausted {
        epoch: EpochId,
        spent: Balance,
    },
    /// 08 §1.2/§1.4: INSURANCE swept into `MAIN` by a TREASURY decision. Not
    /// 02 §6 ingest surface — the treasury owns it per 08 §1.4.
    InsuranceSwept {
        amount: Balance,
    },
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
    ZeroQuote,
    Overflow,
    QuoteExpired,
    QuoteNotExpired,
    RateUnset,
    FeeBudgetUnset,
    QuoteTtlUnset,
    QuoteTimestampInFuture,
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
    /// Rolling 365-day issuance meter (08 §2.3, I-7 monotone): a bucketed
    /// trailing window like the outflow meters, so mints straddling the year
    /// seam cannot exceed `iss.inflation_cap`. `limit_bps` is the live cap
    /// (kernel ceiling 2%, amendable down only), refreshed from Params on load.
    pub issuance: RollingMeter<366>,
    pub events: Vec<Event>,
    pub next_stream_id: u64,
    /// VIT balances credited to budget lines by `issue_vit` (08 §2.3) —
    /// tracked apart from the USDC line balances so NAV (USDC at par, VIT
    /// marked 0 per 08 §1.2) never counts minted VIT.
    pub vit_lines: Vec<(BudgetLine, Balance)>,
    /// Coretime periods already funded via `execute_coretime_renewal`
    /// (09 §4 idempotency key), most recent last.
    pub funded_coretime_periods: Vec<u32>,
    /// Open authenticated renewal quotes (09 §4). A present fresh quote is
    /// what makes the renewal window open; the permissionless renewal caller
    /// can neither choose its price nor refresh its timestamp.
    pub coretime_quotes: Vec<CoretimeQuote>,
    /// Live tunable seams (rule 4). These default to the 13 §1 defaults
    /// (`TRS_CAP_PROPOSAL_BPS`, `TRS_STREAM_THRESHOLD_BPS`), so the frame-free
    /// core and the M3 model behave identically at default parameters. The
    /// production FRAME pallet overwrites them from `pallet-constitution::Params`
    /// (`trs.cap_proposal`, `trs.stream_thr`) on every load — never a hardcode
    /// in runtime code, only the oracle's defaults. The metered caps
    /// (`trs.cap_30d`/`trs.cap_180d` on the outflow meters, `iss.inflation` on
    /// `issuance`) live on the meters' `limit_bps` fields, refreshed the same way.
    pub cap_proposal_bps: u32,
    pub stream_threshold_bps: u32,
    /// Per-epoch 08 §6.3 keeper-rebate meter. The chain is pre-genesis, so the
    /// corresponding aggregate SCALE-shape change requires no migration.
    pub keeper_meter: KeeperMeter,
}

impl Default for Treasury {
    fn default() -> Self {
        Self {
            main_usdc: 0,
            vit_supply: DEFAULT_VIT_SUPPLY,
            reserve_impaired: false,
            lines: Vec::new(),
            streams: Vec::new(),
            pending_outflows: Vec::new(),
            pol_commitments: Vec::new(),
            meter_30d: RollingMeter::new(TRS_CAP_30D_BPS),
            meter_180d: RollingMeter::new(TRS_CAP_180D_BPS),
            issuance: RollingMeter::new(ISS_INFLATION_CAP_BPS),
            events: Vec::new(),
            next_stream_id: 0,
            vit_lines: Vec::new(),
            funded_coretime_periods: Vec::new(),
            coretime_quotes: Vec::new(),
            cap_proposal_bps: TRS_CAP_PROPOSAL_BPS,
            stream_threshold_bps: TRS_STREAM_THRESHOLD_BPS,
            keeper_meter: KeeperMeter::default(),
        }
    }
}

impl Treasury {
    /// Return the amount payable for one 08 §6.3 metered keeper rebate.
    ///
    /// This operation is deliberately infallible and fail-soft: arithmetic,
    /// tranche, budget, or line-funding failures return zero and never affect
    /// the useful crank which requested the rebate.
    pub fn keeper_rebate(
        &mut self,
        now_epoch: EpochId,
        class: KeeperMeterClass,
        rebate: Balance,
        budget: Balance,
    ) -> Balance {
        if self.keeper_meter.epoch != now_epoch {
            self.keeper_meter = KeeperMeter {
                epoch: now_epoch,
                ..KeeperMeter::default()
            };
        }
        // Exhaustion is a per-epoch payment latch, not merely an alarm latch:
        // once it fires, a later parameter decrease must not reopen capacity.
        if self.keeper_meter.exhausted_emitted {
            return 0;
        }
        if rebate == 0 || budget == 0 {
            return 0;
        }

        let next_spent = self.keeper_meter.spent.checked_add(rebate);
        if next_spent.is_none_or(|spent| spent > budget) {
            if !self.keeper_meter.low_emitted {
                self.events.push(Event::KeeperBudgetLow {
                    remaining: budget.saturating_sub(self.keeper_meter.spent),
                });
                self.keeper_meter.low_emitted = true;
            }
            if !self.keeper_meter.exhausted_emitted {
                self.events.push(Event::KeeperBudgetExhausted {
                    epoch: now_epoch,
                    spent: self.keeper_meter.spent,
                });
                self.keeper_meter.exhausted_emitted = true;
            }
            return 0;
        }
        let Some(next_spent) = next_spent else {
            return 0;
        };

        let next_general_spent = if class == KeeperMeterClass::General {
            let general_cap = budget / 5;
            let Some(next_general) = self.keeper_meter.general_spent.checked_add(rebate) else {
                return 0;
            };
            if next_general > general_cap {
                return 0;
            }
            Some(next_general)
        } else {
            None
        };

        // The line check is read-only until every meter check has passed;
        // insufficient funding is a pure no-op with no meter charge/event.
        let Ok(line_index) = self.debitable_line(BudgetLine::Keeper, rebate) else {
            return 0;
        };
        let Some((_, line_balance)) = self.lines.get_mut(line_index) else {
            return 0;
        };
        let Some(next_line_balance) = line_balance.checked_sub(rebate) else {
            return 0;
        };
        *line_balance = next_line_balance;
        self.keeper_meter.spent = next_spent;
        if let Some(next_general) = next_general_spent {
            self.keeper_meter.general_spent = next_general;
        }

        // `budget - floor(budget/5)` is ceil(80% of budget), avoiding a
        // potentially overflowing multiplication and rounding against the
        // claimant at the threshold boundary.
        let low_threshold = budget.saturating_sub(budget / 5);
        if next_spent >= low_threshold && !self.keeper_meter.low_emitted {
            self.events.push(Event::KeeperBudgetLow {
                remaining: budget.saturating_sub(next_spent),
            });
            self.keeper_meter.low_emitted = true;
        }
        if next_spent == budget && !self.keeper_meter.exhausted_emitted {
            self.events.push(Event::KeeperBudgetExhausted {
                epoch: now_epoch,
                spent: next_spent,
            });
            self.keeper_meter.exhausted_emitted = true;
        }
        rebate
    }

    /// Return the amount payable from the separate 07 ORACLE budget line.
    /// Oracle/registry cranks do not consume the 08 §6.3 keeper meter.
    pub fn oracle_line_rebate(&mut self, rebate: Balance) -> Balance {
        if rebate == 0 {
            return 0;
        }
        let Ok(line_index) = self.debitable_line(BudgetLine::Oracle, rebate) else {
            return 0;
        };
        let Some((_, line_balance)) = self.lines.get_mut(line_index) else {
            return 0;
        };
        let Some(next_line_balance) = line_balance.checked_sub(rebate) else {
            return 0;
        };
        *line_balance = next_line_balance;
        rebate
    }

    /// Pay one proposer reward from the dedicated REWARDS line (08 §1.1;
    /// 05 §2.1 T17). Rewards are execution-time obligations, not ordinary
    /// discretionary grants: they bypass the stream and rolling outflow
    /// meters, but still require an already-funded bounded line. The pallet
    /// adapter treats a missing or underfunded line as a strict no-op, so an
    /// execution can never create an unbacked claimant.
    pub fn proposer_reward(&mut self, dest: AccountId, amount: Balance) -> Result<(), Error> {
        if amount == 0 {
            return Ok(());
        }
        let idx = self.debitable_line(BudgetLine::Rewards, amount)?;
        self.lines[idx].1 -= amount;
        self.events.push(Event::Spent {
            line: BudgetLine::Rewards,
            dest,
            amount,
        });
        Ok(())
    }

    /// Debit the dedicated collator-compensation line for one Housekeeping
    /// distribution. Each authored-block share is rounded down against the
    /// claimant, and the line is charged once for the exact sum of payouts.
    /// This is an existing operational obligation, so it intentionally does
    /// not pass through the discretionary outflow meters or reserve haircut.
    pub fn collator_compensation(
        &mut self,
        shares: &[(AccountId, u32)],
        amount_per_collator: Balance,
        registered_collators: u32,
    ) -> Result<Vec<(AccountId, Balance)>, Error> {
        if amount_per_collator == 0 || registered_collators == 0 || shares.is_empty() {
            return Ok(Vec::new());
        }
        let total_blocks = shares.iter().try_fold(0u128, |total, (_, blocks)| {
            total
                .checked_add(u128::from(*blocks))
                .ok_or(Error::Overflow)
        })?;
        if total_blocks == 0 {
            return Ok(Vec::new());
        }
        let total_pool = amount_per_collator
            .checked_mul(Balance::from(registered_collators))
            .ok_or(Error::Overflow)?;
        let mut payouts = Vec::with_capacity(shares.len());
        let mut total_payout = 0u128;
        for (account, blocks) in shares {
            if *blocks == 0 {
                continue;
            }
            let payout = total_pool
                .checked_mul(u128::from(*blocks))
                .ok_or(Error::Overflow)?
                / total_blocks;
            if payout == 0 {
                continue;
            }
            total_payout = total_payout.checked_add(payout).ok_or(Error::Overflow)?;
            payouts.push((*account, payout));
        }
        if total_payout == 0 {
            return Ok(Vec::new());
        }
        let idx = self.debitable_line(BudgetLine::OpsCollators, total_payout)?;
        self.lines[idx].1 -= total_payout;
        for (dest, amount) in &payouts {
            self.events.push(Event::Spent {
                line: BudgetLine::OpsCollators,
                dest: *dest,
                amount: *amount,
            });
        }
        Ok(payouts)
    }

    pub fn fund_budget_line(
        &mut self,
        origin: Origin,
        line: BudgetLine,
        amount: Balance,
    ) -> Result<(), Error> {
        ensure!(origin == Origin::FutarchyTreasury, Error::BadOrigin);
        // Atomicity (G-1): compute the entire credit — table room AND the
        // credited balance's overflow check — BEFORE touching `main_usdc`, so a
        // debit is never followed by a failing credit (Codex review).
        match self.lines.iter().position(|(l, _)| *l == line) {
            Some(i) => {
                let credited = self.lines[i].1.checked_add(amount).ok_or(Error::Overflow)?;
                ensure!(self.main_usdc >= amount, Error::InsufficientFunds);
                self.main_usdc -= amount;
                self.lines[i].1 = credited;
            }
            None => {
                ensure!(
                    self.lines.len() < MAX_BUDGET_LINES,
                    Error::TooManyBudgetLines
                );
                ensure!(self.main_usdc >= amount, Error::InsufficientFunds);
                self.main_usdc -= amount;
                self.lines.push((line, amount));
            }
        }
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
            amount <= bps(self.nav().nav, self.stream_threshold_bps)?,
            Error::StreamRequired
        );
        // 08 §1.3: per-proposal outflow ≤ trs.cap_proposal × spendable NAV.
        // Read-only, checked before the line so its error keeps precedence.
        let nav = self.nav().spendable_nav;
        ensure!(
            amount <= bps(nav, self.cap_proposal_bps)?,
            Error::ProposalCapExceeded
        );
        // Atomicity (G-1): validate the line debit before charging the meters,
        // so a rejected spend leaves both the line and the meters untouched
        // (FRAME rolls back all storage on `Err`; the core must match).
        let idx = self.debitable_line(line, amount)?;
        self.charge_meters(now, nav, amount)?;
        self.lines[idx].1 -= amount;
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
        // The 13 §4 bound is on CONCURRENT open streams (08 §1.3), not the
        // lifetime count. When the table is full, a terminal stream (cancelled,
        // or fully vested-and-claimed) may be reaped to make room. Compute the
        // reapable slot read-only here; the removal is deferred to the commit
        // below so a rejected open stays a strict no-op (G-1).
        let reap_idx = if self.streams.len() < MAX_STREAMS {
            None
        } else {
            Some(
                self.streams
                    .iter()
                    .position(|s| s.cancelled || s.claimed >= s.total)
                    .ok_or(Error::TooManyStreams)?,
            )
        };
        let nav = self.nav().nav;
        ensure!(
            input.total > bps(nav, self.stream_threshold_bps)?,
            Error::StreamRequired
        );
        // 08 §1.3 per-proposal cap (read-only, precedence over the line error).
        let spendable = self.nav().spendable_nav;
        ensure!(
            input.total <= bps(spendable, self.cap_proposal_bps)?,
            Error::ProposalCapExceeded
        );
        // Atomicity (G-1): validate the line debit and the id increment before
        // charging the meters / mutating.
        let idx = self.debitable_line(input.line, input.total)?;
        let next_id = self.next_stream_id.checked_add(1).ok_or(Error::Overflow)?;
        self.charge_meters(now, spendable, input.total)?;
        self.lines[idx].1 -= input.total;
        // Commit: reap the terminal slot (if any) then push — both after every
        // fallible step.
        if let Some(i) = reap_idx {
            self.streams.remove(i);
        }
        let id = self.next_stream_id;
        self.next_stream_id = next_id;
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
        // Compute the reverted MAIN balance before mutating (G-1 atomicity).
        let new_main = self
            .main_usdc
            .checked_add(remainder)
            .ok_or(Error::Overflow)?;
        self.streams[idx].cancelled = true;
        self.main_usdc = new_main;
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
        // 08 §2.3: a ROLLING 365-day issuance meter (I-7 monotone). Roll a COPY
        // so a rejected mint mutates nothing (G-1 atomicity). The cap base is
        // the supply at the trailing window's start, computed as the current
        // supply minus the mints still inside the window (`vit_supply − spent`);
        // because the meter never resets its trailing sum at a boundary, two
        // mints straddling the year seam are summed and cannot together exceed
        // `iss.inflation_cap` — the fixed-window boundary doubling is closed.
        let mut meter = self.issuance;
        meter.roll(now);
        let reference_supply = self.vit_supply.saturating_sub(meter.spent());
        let cap = bps(reference_supply, meter.limit_bps)?;
        let next = meter.spent().checked_add(amount).ok_or(Error::Overflow)?;
        ensure!(next <= cap, Error::IssuanceCapExceeded);
        let new_supply = self.vit_supply.checked_add(amount).ok_or(Error::Overflow)?;
        // Pre-validate the line credit so a full table cannot mint-then-fail.
        let line_pos = self.vit_lines.iter().position(|(l, _)| *l == line);
        ensure!(
            line_pos.is_some() || self.vit_lines.len() < MAX_BUDGET_LINES,
            Error::TooManyBudgetLines
        );
        // All reachable checks passed — commit the meter charge, the mint and
        // the line credit together.
        meter.add(now, amount)?;
        self.issuance = meter;
        self.vit_supply = new_supply;
        // 08 §2.3: the minted VIT is credited to the requested REWARDS/ops
        // line so the line can actually disburse it.
        match line_pos {
            Some(i) => {
                self.vit_lines[i].1 = self.vit_lines[i]
                    .1
                    .checked_add(amount)
                    .ok_or(Error::Overflow)?;
            }
            None => self.vit_lines.push((line, amount)),
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
    /// 08 §1.2/§1.4 `sweep_insurance(amount)` — the one admissible outflow of
    /// the INSURANCE account, and the internal half of it.
    ///
    /// INSURANCE is **outside** NAV (08 §1.2), so crediting `main_usdc` raises
    /// NAV by exactly `amount` — the effect 08 §1.2 specifies. Deliberately
    /// takes **no** budget line: the sweep is an inbound transfer *to* `MAIN`,
    /// not an outflow *from* it, and 08 §1.2 explicitly rejected a dedicated
    /// `BudgetLine::Insurance` because modelling an inflow-fed reserve as a line
    /// inverts the direction of every §1.1 control.
    ///
    /// Origin is `FutarchyTreasury` only — a passed TREASURY decision. No
    /// guardian power, playbook or admin origin can reach it. The custody half
    /// (`Preservation::Preserve` on the real USDC) is the pallet's job.
    pub fn sweep_insurance(&mut self, origin: Origin, amount: Balance) -> Result<(), Error> {
        ensure!(origin == Origin::FutarchyTreasury, Error::BadOrigin);
        self.main_usdc = self.main_usdc.checked_add(amount).ok_or(Error::Overflow)?;
        self.events.push(Event::InsuranceSwept { amount });
        Ok(())
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
    /// Note an authenticated renewal quote. Re-noting an open period replaces
    /// both price and timestamp in place, so the bound counts periods rather
    /// than revisions (09 §4).
    pub fn note_coretime_renewal_quote(
        &mut self,
        period_index: u32,
        price: Balance,
        now: u64,
    ) -> Result<(), Error> {
        // A real renewal costs > 0; a zero quote would let a keeper "renew" for
        // free and permanently mark the period funded, blocking a corrected
        // retry (Codex review). Reject it — absence of a quote (window closed)
        // is distinct from a zero price.
        ensure!(price > 0, Error::ZeroQuote);
        ensure!(
            !self.funded_coretime_periods.contains(&period_index),
            Error::PeriodAlreadyFunded
        );
        if let Some(quote) = self
            .coretime_quotes
            .iter_mut()
            .find(|quote| quote.period_index == period_index)
        {
            quote.price = price;
            quote.noted_at = now;
            self.events.push(Event::CoretimeQuoteNoted {
                period_index,
                price,
            });
            return Ok(());
        }
        ensure!(
            self.coretime_quotes.len() < MAX_FUNDED_CORETIME_PERIODS,
            Error::TooManyObligations
        );
        self.coretime_quotes.push(CoretimeQuote {
            period_index,
            price,
            noted_at: now,
        });
        self.events.push(Event::CoretimeQuoteNoted {
            period_index,
            price,
        });
        Ok(())
    }

    /// Drop an open quote. Its authority may prune at any time; every other
    /// Signed caller may prune only once `age > ttl` (09 §4).
    pub fn prune_coretime_quote(
        &mut self,
        period_index: u32,
        now: u64,
        ttl: u64,
        authority: bool,
    ) -> Result<(), Error> {
        let quote_index = self
            .coretime_quotes
            .iter()
            .position(|quote| quote.period_index == period_index)
            .ok_or(Error::RenewalWindowClosed)?;
        if !authority {
            ensure!(ttl > 0, Error::QuoteTtlUnset);
            let noted_at = self
                .coretime_quotes
                .get(quote_index)
                .map(|quote| quote.noted_at)
                .ok_or(Error::RenewalWindowClosed)?;
            let age = now
                .checked_sub(noted_at)
                .ok_or(Error::QuoteTimestampInFuture)?;
            ensure!(age > ttl, Error::QuoteNotExpired);
        }
        self.coretime_quotes.remove(quote_index);
        self.events
            .push(Event::CoretimeQuotePruned { period_index });
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
        now: u64,
        ttl: u64,
        dot_rate: Balance,
        fee_budget: Balance,
    ) -> Result<Balance, Error> {
        ensure!(
            !self.funded_coretime_periods.contains(&period_index),
            Error::PeriodAlreadyFunded
        );
        let quote_index = self
            .coretime_quotes
            .iter()
            .position(|quote| quote.period_index == period_index)
            .ok_or(Error::RenewalWindowClosed)?;
        ensure!(ttl > 0, Error::QuoteTtlUnset);
        ensure!(dot_rate > 0, Error::RateUnset);
        ensure!(fee_budget > 0, Error::FeeBudgetUnset);
        let quote = self
            .coretime_quotes
            .get(quote_index)
            .copied()
            .ok_or(Error::RenewalWindowClosed)?;
        let age = now
            .checked_sub(quote.noted_at)
            .ok_or(Error::QuoteTimestampInFuture)?;
        ensure!(age <= ttl, Error::QuoteExpired);
        let total_dot = quote.price.checked_add(fee_budget).ok_or(Error::Overflow)?;
        let converted_debit = dot_to_usdc_ceil(total_dot, dot_rate)?;
        self.debit_line(BudgetLine::OpsCoretime, converted_debit)?;
        self.coretime_quotes.remove(quote_index);
        if self.funded_coretime_periods.len() >= MAX_FUNDED_CORETIME_PERIODS {
            self.funded_coretime_periods.remove(0);
        }
        self.funded_coretime_periods.push(period_index);
        self.events.push(Event::CoretimeRenewalCalled {
            line: BudgetLine::OpsCoretime,
            amount: converted_debit,
        });
        Ok(quote.price)
    }

    /// Reserve the maximum DOT fee envelope for one 07 §8 probe against the
    /// dedicated USDC-denominated `ops.reserve_probe` line (SQ-114).
    ///
    /// The conversion uses the dedicated governed reserve-probe DOT rate and
    /// rounds up against the spender. The XCM dispatcher invokes
    /// this inside the same storage layer as local send validation, so a send
    /// rejected before delivery rolls the debit and event back together.
    pub fn charge_reserve_probe_fee(
        &mut self,
        dot_fee: Balance,
        dot_rate: Balance,
    ) -> Result<Balance, Error> {
        let converted_debit = reserve_probe_fee_debit(dot_fee, dot_rate)?;
        self.debit_line(BudgetLine::OpsReserveProbe, converted_debit)?;
        self.events.push(Event::ReserveProbeFeeCharged {
            line: BudgetLine::OpsReserveProbe,
            amount: converted_debit,
        });
        Ok(converted_debit)
    }
    pub fn set_reserve_impaired(&mut self, epoch: EpochId, flag: bool) {
        if self.reserve_impaired != flag {
            self.reserve_impaired = flag;
            self.events.push(Event::NavHaircutFlagged { epoch, flag });
        }
    }
    /// Runtime-internal (08 §1.2/§8.2): sync the live-book POL subsidy
    /// commitments `nav()` nets as obligations. The POL/market lifecycle (A3)
    /// owns the set — this pallet only holds the treasury's view of it so NAV
    /// reflects live commitments; B1a wires the sync. The registration keying
    /// and trigger are a cross-pallet concern (PLAN SQ-47). Bounded (13 §4).
    pub fn set_pol_commitments(&mut self, commitments: &[Balance]) -> Result<(), Error> {
        ensure!(
            commitments.len() <= MAX_POL_COMMITMENTS,
            Error::TooManyObligations
        );
        self.pol_commitments = commitments.to_vec();
        Ok(())
    }
    /// Runtime-internal (08 §1.2/§1.3): sync the queued in-cap proposal outflows
    /// `nav()` nets as obligations. The execution-guard queue (A11) owns them;
    /// B1a wires the sync (PLAN SQ-47). Bounded (13 §4).
    pub fn set_pending_outflows(&mut self, outflows: &[Balance]) -> Result<(), Error> {
        ensure!(
            outflows.len() <= MAX_PENDING_OUTFLOWS,
            Error::TooManyObligations
        );
        self.pending_outflows = outflows.to_vec();
        Ok(())
    }
    pub fn nav(&self) -> NavComponents {
        // 08 §1.2: NAV = liquid USDC at par + reversions − obligations (open
        // stream remainders owed FROM the treasury, queued in-cap outflows, POL
        // commitments). An open stream's remainder is BOTH such an obligation
        // AND escrowed USDC the treasury still holds at par (it was debited from
        // the spendable lines at open, so `main + Σlines` no longer counts it):
        // it enters as an asset and an equal liability that net to zero, i.e.
        // the committed funds are excluded from NAV exactly once — via the
        // open-time line debit. Counting only the obligation would double-
        // subtract them (understating NAV, tightening every cap and floor).
        let escrow = self.open_stream_remainders();
        let obligations = self.obligations();
        let assets = self
            .main_usdc
            .saturating_add(sum_lines(&self.lines))
            .saturating_add(escrow);
        let nav = assets.saturating_sub(obligations);
        let spendable_nav = if self.reserve_impaired { 0 } else { nav };
        NavComponents {
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
            ProposalClass::Param => 4_620_989 * USDC,
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
        // I-7/I-17 (15 §1): the trailing-window minted sum never exceeds the
        // KERNEL ceiling `ISS_INFLATION_CAP_BPS` (2%) of supply. `iss.inflation`
        // is amendable DOWN only, so the live cap ≤ 2%; checking against the
        // fixed kernel ceiling (not the live cap) keeps this a true standing
        // invariant that a mid-window down-amendment cannot retroactively break
        // (a release-blocking `try_state` failure would otherwise be reachable).
        let issuance_ceiling = bps(self.vit_supply, ISS_INFLATION_CAP_BPS)?;
        ensure!(
            self.issuance.spent() <= issuance_ceiling,
            Error::IssuanceCapExceeded
        );
        ensure!(
            self.funded_coretime_periods.len() <= MAX_FUNDED_CORETIME_PERIODS,
            Error::TooManyObligations
        );
        ensure!(
            self.coretime_quotes.len() <= MAX_FUNDED_CORETIME_PERIODS,
            Error::TooManyObligations
        );
        for (index, quote) in self.coretime_quotes.iter().enumerate() {
            ensure!(quote.price > 0, Error::ZeroQuote);
            ensure!(
                !self.funded_coretime_periods.contains(&quote.period_index),
                Error::PeriodAlreadyFunded
            );
            ensure!(
                !self
                    .coretime_quotes
                    .iter()
                    .skip(index.saturating_add(1))
                    .any(|other| other.period_index == quote.period_index),
                Error::TooManyObligations
            );
        }
        for (index, period) in self.funded_coretime_periods.iter().enumerate() {
            ensure!(
                !self
                    .funded_coretime_periods
                    .iter()
                    .skip(index.saturating_add(1))
                    .any(|other| other == period),
                Error::TooManyObligations
            );
        }
        // The live keeper budget may shrink mid-epoch, so `spent <= budget`
        // is intentionally not a standing invariant. Likewise, an over-budget
        // first attempt may set `exhausted_emitted` while `spent == 0`.
        ensure!(
            self.keeper_meter.general_spent <= self.keeper_meter.spent,
            Error::Overflow
        );
        Ok(())
    }

    /// Runtime-aware extension of [`Self::try_state`] for quote timestamps.
    pub fn try_state_at(&self, now: u64) -> Result<(), Error> {
        self.try_state()?;
        ensure!(
            self.coretime_quotes
                .iter()
                .all(|quote| quote.noted_at <= now),
            Error::QuoteTimestampInFuture
        );
        Ok(())
    }
    fn ensure_spendable(&self) -> Result<(), Error> {
        ensure!(!self.reserve_impaired, Error::ReserveImpaired);
        Ok(())
    }
    /// Charge both rolling meters against `nav` for `amount`, atomically: roll
    /// into copies, and commit BOTH only if both windows admit (G-1) — a
    /// rejected outflow must not consume, or even lazily roll, meter state,
    /// since FRAME rolls the whole dispatch back on `Err`. The caller has
    /// already enforced the per-proposal cap against the same `nav`.
    fn charge_meters(
        &mut self,
        now: BlockNumber,
        nav: Balance,
        amount: Balance,
    ) -> Result<(), Error> {
        let mut m30 = self.meter_30d;
        let mut m180 = self.meter_180d;
        m30.roll(now);
        m180.roll(now);
        m30.can_charge(nav, amount)?;
        m180.can_charge(nav, amount)?;
        m30.add(now, amount)?;
        m180.add(now, amount)?;
        self.meter_30d = m30;
        self.meter_180d = m180;
        Ok(())
    }
    /// Index of `line` in `self.lines`, proven to hold at least `amount`
    /// (`UnknownBudgetLine`/`InsufficientFunds` otherwise) — a read-only check so
    /// callers can validate a debit before any mutation.
    fn debitable_line(&self, line: BudgetLine, amount: Balance) -> Result<usize, Error> {
        let (idx, (_, balance)) = self
            .lines
            .iter()
            .enumerate()
            .find(|(_, (candidate, _))| *candidate == line)
            .ok_or(Error::UnknownBudgetLine)?;
        ensure!(*balance >= amount, Error::InsufficientFunds);
        Ok(idx)
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
    /// Sum of every non-cancelled stream's undisbursed remainder (08 §1.2).
    /// Saturating arithmetic is the same conservative path consumed by NAV.
    pub fn open_stream_remainders(&self) -> Balance {
        self.streams
            .iter()
            .filter(|s| !s.cancelled)
            .fold(0, |acc, s| {
                acc.saturating_add(s.total.saturating_sub(s.claimed))
            })
    }

    /// Exact 08 §1.2 obligation term consumed by [`Self::nav`]: open stream
    /// remainders, queued in-cap outflows, and live-book POL commitments.
    pub fn obligations(&self) -> Balance {
        self.open_stream_remainders()
            .saturating_add(sum(&self.pending_outflows))
            .saturating_add(sum(&self.pol_commitments))
    }
}

/// Exact ceil-rounded local line debit for one reserve-probe DOT envelope.
pub fn reserve_probe_fee_debit(dot_fee: Balance, dot_rate: Balance) -> Result<Balance, Error> {
    ensure!(dot_rate > 0, Error::RateUnset);
    ensure!(dot_fee > 0, Error::FeeBudgetUnset);
    dot_to_usdc_ceil(dot_fee, dot_rate)
}

/// Local arming runway: enough exact one-envelope debits to cross both the
/// fail and recovery latches. Zero total attempts is invalid rather than an
/// accidental zero-cost arm.
pub fn reserve_probe_runway_debit(
    dot_fee: Balance,
    dot_rate: Balance,
    fail_threshold: u8,
    recover_threshold: u8,
) -> Result<Balance, Error> {
    let attempts = Balance::from(fail_threshold)
        .checked_add(Balance::from(recover_threshold))
        .ok_or(Error::Overflow)?;
    ensure!(attempts > 0, Error::FeeBudgetUnset);
    reserve_probe_fee_debit(dot_fee, dot_rate)?
        .checked_mul(attempts)
        .ok_or(Error::Overflow)
}

/// Convert a DOT-planck outflow into µUSDC budget debit, rounding up against
/// the spender (08 §1.1; shared by the two operations DOT envelopes).
fn dot_to_usdc_ceil(dot_amount: Balance, dot_rate: Balance) -> Result<Balance, Error> {
    let numerator = dot_amount.checked_mul(dot_rate).ok_or(Error::Overflow)?;
    let quotient = numerator / DOT_PLANCKS_PER_DOT;
    if numerator % DOT_PLANCKS_PER_DOT == 0 {
        Ok(quotient)
    } else {
        quotient.checked_add(1).ok_or(Error::Overflow)
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

    fn rebate_funded(keeper: Balance, oracle: Balance) -> Treasury {
        let mut t = Treasury::default();
        if keeper > 0 {
            t.lines.push((BudgetLine::Keeper, keeper));
        }
        if oracle > 0 {
            t.lines.push((BudgetLine::Oracle, oracle));
        }
        t
    }

    fn probe_funded(balance: Balance) -> Treasury {
        let mut t = Treasury::default();
        t.lines.push((BudgetLine::OpsReserveProbe, balance));
        t.lines.push((BudgetLine::OpsCoretime, 777));
        t.lines.push((BudgetLine::Oracle, 888));
        t
    }

    #[test]
    fn reserve_probe_fee_charge_is_ceil_rounded_and_touches_only_its_line() {
        let mut exact = probe_funded(10_000_000);
        let main = exact.main_usdc;
        assert_eq!(
            exact
                .charge_reserve_probe_fee(DOT_PLANCKS_PER_DOT, 5_000_000)
                .unwrap(),
            5_000_000
        );
        assert_eq!(exact.line_balance(BudgetLine::OpsReserveProbe), 5_000_000);
        assert_eq!(exact.line_balance(BudgetLine::OpsCoretime), 777);
        assert_eq!(exact.line_balance(BudgetLine::Oracle), 888);
        assert_eq!(exact.main_usdc, main);
        assert_eq!(
            exact.events,
            vec![Event::ReserveProbeFeeCharged {
                line: BudgetLine::OpsReserveProbe,
                amount: 5_000_000,
            }]
        );

        let mut ceil = probe_funded(2);
        assert_eq!(ceil.charge_reserve_probe_fee(1, 1).unwrap(), 1);
        assert_eq!(ceil.line_balance(BudgetLine::OpsReserveProbe), 1);
    }

    #[test]
    fn reserve_probe_arm_runway_multiplies_the_exact_ceil_debit() {
        assert_eq!(
            reserve_probe_runway_debit(DOT_PLANCKS_PER_DOT, 5_000_000, 2, 3),
            Ok(25_000_000)
        );
        // Ceil happens per attempted envelope, then the exact number of
        // fail+recover attempts is reserved: ceil(1 planck × 1 / 1e10) × 5.
        assert_eq!(reserve_probe_runway_debit(1, 1, 2, 3), Ok(5));
        assert_eq!(
            reserve_probe_runway_debit(1, 1, 0, 0),
            Err(Error::FeeBudgetUnset)
        );
    }

    #[test]
    fn reserve_probe_fee_rejections_leave_the_full_state_byte_identical() {
        for (fee, rate, expected) in [
            (0, 1, Error::FeeBudgetUnset),
            (1, 0, Error::RateUnset),
            (u128::MAX, 2, Error::Overflow),
            (DOT_PLANCKS_PER_DOT, 2, Error::InsufficientFunds),
        ] {
            let mut t = probe_funded(1);
            let before = t.clone();
            assert_eq!(t.charge_reserve_probe_fee(fee, rate), Err(expected));
            assert_eq!(t, before);
        }
    }

    #[test]
    fn reserve_probe_budget_line_is_append_only_in_scale_encoding() {
        let legacy = [
            BudgetLine::Pol,
            BudgetLine::PolBaseline,
            BudgetLine::Keeper,
            BudgetLine::Oracle,
            BudgetLine::Rewards,
            BudgetLine::OpsBootnodes,
            BudgetLine::OpsRpcArchive,
            BudgetLine::OpsCollators,
            BudgetLine::OpsKeepers,
            BudgetLine::OpsOracleEvidence,
            BudgetLine::OpsWatchtowers,
            BudgetLine::OpsMonitoring,
            BudgetLine::OpsArweave,
            BudgetLine::OpsCoretime,
        ];
        for (index, expected) in legacy.into_iter().enumerate() {
            assert_eq!(expected.encode(), vec![index as u8]);
            assert_eq!(BudgetLine::decode(&mut &[index as u8][..]), Ok(expected));
        }
        assert_eq!(BudgetLine::OpsCoretime.encode(), vec![13]);
        assert_eq!(BudgetLine::OpsReserveProbe.encode(), vec![14]);
    }

    #[test]
    fn keeper_meter_crosses_eighty_and_one_hundred_percent_exactly_once() {
        let mut t = rebate_funded(200, 0);
        for _ in 0..3 {
            assert_eq!(
                t.keeper_rebate(7, KeeperMeterClass::DecisionCritical, 20, 100),
                20
            );
        }
        assert!(t.events.is_empty());
        assert_eq!(
            t.keeper_rebate(7, KeeperMeterClass::DecisionCritical, 20, 100),
            20
        );
        assert_eq!(t.events, vec![Event::KeeperBudgetLow { remaining: 20 }]);
        assert_eq!(
            t.keeper_rebate(7, KeeperMeterClass::DecisionCritical, 20, 100),
            20
        );
        assert_eq!(
            t.events,
            vec![
                Event::KeeperBudgetLow { remaining: 20 },
                Event::KeeperBudgetExhausted {
                    epoch: 7,
                    spent: 100,
                },
            ]
        );
        assert_eq!(
            t.keeper_rebate(7, KeeperMeterClass::DecisionCritical, 1, 100),
            0
        );
        assert_eq!(t.events.len(), 2);
    }

    #[test]
    fn keeper_general_tranche_uses_floor_and_preserves_decision_reservation() {
        let mut t = rebate_funded(200, 0);
        // floor(101 / 5) = 20: the first general rebate fits exactly, the next
        // unit is rejected while decision-critical capacity remains available.
        assert_eq!(t.keeper_rebate(1, KeeperMeterClass::General, 20, 101), 20);
        assert_eq!(t.keeper_rebate(1, KeeperMeterClass::General, 1, 101), 0);
        assert_eq!(t.keeper_meter.general_spent, 20);
        assert_eq!(t.keeper_meter.spent, 20);
        assert_eq!(
            t.keeper_rebate(1, KeeperMeterClass::DecisionCritical, 81, 101),
            81
        );
        assert_eq!(t.keeper_meter.spent, 101);
    }

    #[test]
    fn keeper_meter_zero_and_unfunded_paths_are_pure_noops() {
        let mut t = Treasury::default();
        let before = t.clone();
        assert_eq!(t.keeper_rebate(0, KeeperMeterClass::General, 0, 100), 0);
        assert_eq!(t, before);
        assert_eq!(
            t.keeper_rebate(0, KeeperMeterClass::DecisionCritical, 1, 0),
            0
        );
        assert_eq!(t, before);
        assert_eq!(
            t.keeper_rebate(0, KeeperMeterClass::DecisionCritical, 1, 100),
            0
        );
        assert_eq!(t.keeper_meter.spent, 0);
        assert!(t.events.is_empty());

        assert_eq!(t.oracle_line_rebate(1), 0);
        assert_eq!(t.oracle_line_rebate(0), 0);
        assert!(t.events.is_empty());
    }

    #[test]
    fn oracle_line_rebate_is_unmetered_and_line_bounded() {
        let mut t = rebate_funded(0, 5);
        assert_eq!(t.oracle_line_rebate(3), 3);
        assert_eq!(t.line_balance(BudgetLine::Oracle), 2);
        assert_eq!(t.oracle_line_rebate(3), 0);
        assert_eq!(t.keeper_meter, KeeperMeter::default());
        assert!(t.events.is_empty());
    }

    #[test]
    fn proposer_reward_debits_only_the_funded_rewards_line() {
        let mut t = Treasury::default();
        t.lines.push((BudgetLine::Rewards, 10));
        assert_eq!(t.proposer_reward(acct(9), 6), Ok(()));
        assert_eq!(t.line_balance(BudgetLine::Rewards), 4);
        assert_eq!(
            t.events,
            vec![Event::Spent {
                line: BudgetLine::Rewards,
                dest: acct(9),
                amount: 6,
            }]
        );
        let before = t.clone();
        assert_eq!(t.proposer_reward(acct(9), 5), Err(Error::InsufficientFunds));
        assert_eq!(t, before);
    }

    #[test]
    fn collator_compensation_rounds_each_share_down_and_debits_one_line() {
        let mut t = Treasury::default();
        t.lines.push((BudgetLine::OpsCollators, 4_000 * USDC));
        let payouts = t
            .collator_compensation(&[(acct(1), 2), (acct(2), 1), (acct(3), 0)], 2_000 * USDC, 2)
            .unwrap();
        assert_eq!(
            payouts,
            vec![(acct(1), 2_666_666_666), (acct(2), 1_333_333_333)]
        );
        assert_eq!(
            t.line_balance(BudgetLine::OpsCollators),
            4_000 * USDC - 3_999_999_999
        );
        assert!(t.events.iter().all(|event| matches!(
            event,
            Event::Spent {
                line: BudgetLine::OpsCollators,
                ..
            }
        )));
    }

    #[test]
    fn collator_compensation_is_fail_static_when_the_line_is_unfunded() {
        let mut t = Treasury::default();
        let before = t.clone();
        assert_eq!(
            t.collator_compensation(&[(acct(1), 1)], COLLATOR_COMP_EPOCH, 1),
            Err(Error::UnknownBudgetLine)
        );
        assert_eq!(t, before);
    }

    #[test]
    fn collator_compensation_scales_by_registered_collators_including_zero_authors() {
        let mut t = Treasury::default();
        t.lines.push((BudgetLine::OpsCollators, 6_000 * USDC));
        let payouts = t
            .collator_compensation(&[(acct(1), 1), (acct(2), 0)], 2_000 * USDC, 3)
            .unwrap();
        assert_eq!(payouts, vec![(acct(1), 6_000 * USDC)]);
        assert_eq!(t.line_balance(BudgetLine::OpsCollators), 0);
    }

    #[test]
    fn over_budget_attempt_emits_exhausted_once_without_charging() {
        let mut t = rebate_funded(500, 0);
        assert_eq!(
            t.keeper_rebate(9, KeeperMeterClass::DecisionCritical, 101, 100),
            0
        );
        assert_eq!(t.keeper_meter.spent, 0);
        assert_eq!(t.line_balance(BudgetLine::Keeper), 500);
        assert_eq!(
            t.events,
            vec![
                Event::KeeperBudgetLow { remaining: 100 },
                Event::KeeperBudgetExhausted { epoch: 9, spent: 0 },
            ]
        );
        assert_eq!(
            t.keeper_rebate(9, KeeperMeterClass::DecisionCritical, 101, 100),
            0
        );
        assert_eq!(t.events.len(), 2);
    }

    #[test]
    fn keeper_exhaustion_latches_after_a_rebate_parameter_drop() {
        let mut t = rebate_funded(500, 0);
        assert_eq!(
            t.keeper_rebate(9, KeeperMeterClass::DecisionCritical, 101, 100),
            0
        );

        // Even though a later, smaller rebate would fit from spent=0, the
        // first effective-exhaustion alarm has stopped rebates for this epoch.
        assert_eq!(
            t.keeper_rebate(9, KeeperMeterClass::DecisionCritical, 1, 100),
            0
        );
        assert_eq!(t.keeper_meter.spent, 0);
        assert_eq!(t.line_balance(BudgetLine::Keeper), 500);
        assert_eq!(t.events.len(), 2);
    }

    #[test]
    fn shrunken_budget_emits_low_before_exhausted() {
        let mut t = rebate_funded(500, 0);
        assert_eq!(
            t.keeper_rebate(9, KeeperMeterClass::DecisionCritical, 20, 100),
            20
        );
        assert!(t.events.is_empty());

        assert_eq!(
            t.keeper_rebate(9, KeeperMeterClass::DecisionCritical, 1, 10),
            0
        );
        assert_eq!(
            t.events,
            vec![
                Event::KeeperBudgetLow { remaining: 0 },
                Event::KeeperBudgetExhausted {
                    epoch: 9,
                    spent: 20,
                },
            ]
        );
    }

    #[test]
    fn keeper_meter_resets_by_epoch_and_survives_mid_epoch_budget_shrink() {
        let mut t = rebate_funded(500, 0);
        assert_eq!(
            t.keeper_rebate(3, KeeperMeterClass::DecisionCritical, 80, 100),
            80
        );
        assert!(t.keeper_meter.low_emitted);

        // Governance may shrink the live budget below already-consumed spend.
        // A new attempt pays nothing, emits exhaustion once, and try_state
        // remains valid because it deliberately does not assert spent<=budget.
        assert_eq!(
            t.keeper_rebate(3, KeeperMeterClass::DecisionCritical, 1, 50),
            0
        );
        assert_eq!(t.keeper_meter.spent, 80);
        assert!(t.keeper_meter.exhausted_emitted);
        assert!(t.try_state().is_ok());

        assert_eq!(
            t.keeper_rebate(4, KeeperMeterClass::DecisionCritical, 1, 50),
            1
        );
        assert_eq!(
            t.keeper_meter,
            KeeperMeter {
                epoch: 4,
                spent: 1,
                general_spent: 0,
                low_emitted: false,
                exhausted_emitted: false,
            }
        );
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
        // Rolling window: at the 365-day seam the day-0 mint is STILL counted
        // (it evicts only once strictly older than the window), so a second full
        // cap is refused — the fixed-window boundary doubling is closed.
        assert_eq!(
            t.issue_vit(TREASURY, DAYS_365_BLOCKS, 1, BudgetLine::OpsArweave)
                .unwrap_err(),
            Error::IssuanceCapExceeded
        );
        // One day later the day-0 mint has fully rolled off; capacity returns.
        t.issue_vit(
            TREASURY,
            DAYS_365_BLOCKS + DAY_BLOCKS,
            1,
            BudgetLine::OpsArweave,
        )
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
        let now = 10;
        let ttl = 100;
        let rate = 10_000_000_000;
        let fee = 100;
        // No runtime-noted quote: the renewal window is closed.
        assert_eq!(
            t.execute_coretime_renewal(acct(7), 42, now, ttl, rate, fee),
            Err(Error::RenewalWindowClosed)
        );
        // The dispatcher receives the authority-noted DOT quote, while the
        // USDC line pays ceil((quote + fee) * rate / 1 DOT).
        let quote = 100_000 * USDC;
        assert_eq!(t.note_coretime_renewal_quote(42, quote, now), Ok(()));
        let line_before = t.line_balance(BudgetLine::OpsCoretime);
        assert_eq!(
            t.execute_coretime_renewal(acct(7), 42, now, ttl, rate, fee),
            Ok(quote)
        );
        assert_eq!(
            t.line_balance(BudgetLine::OpsCoretime),
            line_before - quote - fee
        );
        assert!(matches!(
            t.events.last(),
            Some(Event::CoretimeRenewalCalled {
                amount,
                ..
            }) if *amount == quote + fee
        ));
        // Idempotent per period_index - even against a re-noted quote.
        assert_eq!(
            t.note_coretime_renewal_quote(42, 1, now),
            Err(Error::PeriodAlreadyFunded)
        );
        assert_eq!(
            t.execute_coretime_renewal(acct(8), 42, now, ttl, rate, fee),
            Err(Error::PeriodAlreadyFunded)
        );
        // Bounded solely by the pre-authorized line balance.
        assert_eq!(
            t.note_coretime_renewal_quote(43, 500_000 * USDC, now),
            Ok(())
        );
        assert_eq!(
            t.execute_coretime_renewal(acct(8), 43, now, ttl, rate, fee),
            Err(Error::InsufficientFunds)
        );
        assert_eq!(
            t.note_coretime_renewal_quote(43, 399_999 * USDC, now),
            Ok(())
        );
        assert_eq!(
            t.execute_coretime_renewal(acct(8), 43, now, ttl, rate, fee),
            Ok(399_999 * USDC)
        );
        assert_eq!(t.try_state(), Ok(()));
    }

    #[test]
    fn coretime_quote_supersession_and_strict_prune_boundary() {
        let mut t = funded();
        assert_eq!(t.note_coretime_renewal_quote(7, 10, 20), Ok(()));
        assert_eq!(t.note_coretime_renewal_quote(7, 15, 30), Ok(()));
        assert_eq!(
            t.coretime_quotes,
            vec![CoretimeQuote {
                period_index: 7,
                price: 15,
                noted_at: 30,
            }]
        );

        assert_eq!(
            t.prune_coretime_quote(7, 130, 100, false),
            Err(Error::QuoteNotExpired)
        );
        assert_eq!(t.coretime_quotes.len(), 1);
        assert_eq!(t.prune_coretime_quote(7, 131, 100, false), Ok(()));
        assert!(t.coretime_quotes.is_empty());

        assert_eq!(t.note_coretime_renewal_quote(8, 10, 200), Ok(()));
        assert_eq!(t.prune_coretime_quote(8, 200, 0, true), Ok(()));
        assert!(t.coretime_quotes.is_empty());
    }

    #[test]
    fn coretime_conversion_rounds_up_and_consumes_live_inputs() {
        let mut t = funded();
        let before = t.line_balance(BudgetLine::OpsCoretime);
        assert_eq!(t.note_coretime_renewal_quote(9, 1, 10), Ok(()));
        assert_eq!(
            t.execute_coretime_renewal(acct(7), 9, 110, 100, 5_000_000, 100),
            Ok(1)
        );
        // ceil((1 + 100) * 5_000_000 / 10_000_000_000) == 1.
        assert_eq!(t.line_balance(BudgetLine::OpsCoretime), before - 1);
    }

    #[test]
    fn coretime_execute_failure_paths_are_fail_static() {
        let mut t = funded();
        assert_eq!(t.note_coretime_renewal_quote(10, 10, 20), Ok(()));
        let before = t.clone();
        for (ttl, rate, fee, now, expected) in [
            (0, 1, 1, 20, Error::QuoteTtlUnset),
            (100, 0, 1, 20, Error::RateUnset),
            (100, 1, 0, 20, Error::FeeBudgetUnset),
            (100, 1, 1, 121, Error::QuoteExpired),
            (100, 1, 1, 19, Error::QuoteTimestampInFuture),
        ] {
            assert_eq!(
                t.execute_coretime_renewal(acct(7), 10, now, ttl, rate, fee),
                Err(expected)
            );
            assert_eq!(t, before);
        }

        assert_eq!(t.note_coretime_renewal_quote(10, Balance::MAX, 20), Ok(()));
        let before_add_overflow = t.clone();
        assert_eq!(
            t.execute_coretime_renewal(acct(7), 10, 20, 100, 1, 1),
            Err(Error::Overflow)
        );
        assert_eq!(t, before_add_overflow);

        assert_eq!(
            t.note_coretime_renewal_quote(10, Balance::MAX / 2, 20),
            Ok(())
        );
        let before_mul_overflow = t.clone();
        assert_eq!(
            t.execute_coretime_renewal(acct(7), 10, 20, 100, 3, 1),
            Err(Error::Overflow)
        );
        assert_eq!(t, before_mul_overflow);

        assert_eq!(
            t.prune_coretime_quote(10, 19, 100, false),
            Err(Error::QuoteTimestampInFuture)
        );
        assert_eq!(t, before_mul_overflow);
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
    fn nav_reuses_public_stream_remainder_and_obligation_helpers() {
        let mut t = Treasury {
            main_usdc: 1_000,
            ..Treasury::default()
        };
        t.lines = vec![(BudgetLine::Pol, 100)];
        t.streams = vec![
            Stream {
                id: 1,
                recipient: acct(1),
                line: BudgetLine::Rewards,
                total: 90,
                claimed: 30,
                start: 0,
                duration: 10,
                cancelled: false,
            },
            Stream {
                id: 2,
                recipient: acct(2),
                line: BudgetLine::Rewards,
                total: 80,
                claimed: 10,
                start: 0,
                duration: 10,
                cancelled: true,
            },
        ];
        t.pending_outflows = vec![20, 30];
        t.pol_commitments = vec![40];

        assert_eq!(t.open_stream_remainders(), 60);
        assert_eq!(t.obligations(), 150);
        assert_eq!(t.nav().nav, 1_010);
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
