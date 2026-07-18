---
id: RB-KEEPER
title: Keeper fleet and decision-critical crank response
owner_role: Keeper coordinator
funding_line: ops.keepers
page_immediately: false
alerts:
  - domain: Epoch progress
    trigger: "tick lag > 600 blocks"
  - domain: TWAP
    trigger: "coverage < 96% mid-window"
  - domain: Keepers
    trigger: "no crank 1 h"
  - domain: Keeper budget
    trigger: "> 80% of 12,000 USDC/epoch"
spec_refs:
  - docs/architecture/01-system-overview.md
  - docs/architecture/04-markets-and-pricing.md
  - docs/architecture/05-welfare-and-decision-engine.md
  - docs/architecture/08-treasury-and-economics.md
  - docs/architecture/12-release-and-operations.md
---

## Purpose

Restore permissionless protocol progress when epoch work, decision-window observations, or
other keeper cranks fall behind, and preserve decision-grade coverage through metered-budget
exhaustion. A keeper outage makes the protocol safe but stagnant: it must never be worked around
by fabricating observations or forcing a decision.

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| Epoch progress | phase, blocks-to-boundary, tick lag | tick lag > 600 blocks |
| TWAP | coverage %, stale events, spot-vs-TWAP dispersion | coverage < 96% mid-window |
| Keepers | rebate claims per role, inactivity | no crank 1 h |
| Keeper budget | metered-budget utilization | > 80% of 12,000 USDC/epoch |

Tick lag means finalized chain time has passed work which `epoch.tick` should have advanced.
Low mid-window coverage is decision-critical because gaps can make a book fail grade. Inactivity
means no successful sanctioned crank is visible for the affected role, not merely that one daemon
lost a race. Budget utilization warns that the on-chain rebate meter is nearing its payment latch;
it does not disable any crank.

## Diagnosis

1. Establish an independent finalized head from a second provider. Compare it with
   `bleavit_keeper_current_block` and `bleavit_keeper_connected`; a flat chain head is a chain or
   collator incident, while an advancing head with a flat keeper gauge is a fleet/RPC incident.
2. Read `FutarchyApi::epoch_status()` and direct `Epoch.EpochOf`/`Epoch.Schedule` storage at the
   same finalized hash. Record the phase, next boundary, dead-man state, and the last successful
   `tick` role timestamp before submitting anything ([05 §3](../../docs/architecture/05-welfare-and-decision-engine.md)).
3. Compare every deployed instance and role using the shipped series
   `bleavit_keeper_planned_total`, `bleavit_keeper_submitted_total`,
   `bleavit_keeper_succeeded_total`, `bleavit_keeper_failed_total`, and
   `bleavit_keeper_last_successful_crank_timestamp_seconds`. Metric labels are the shipped roles
   `tick`, `observe`, `decide`, `settle`, `execute`, `oracle-close`, `registry-close`, `cleanup`,
   `renewal`, and `welfare` ([keeper metrics](../../keeper/bleavit-keeper/src/metrics.rs)).
4. For tick lag, inspect capability startup logs and live metadata for `Epoch.tick`. If plans rise
   but submissions do not, inspect signer funding, nonce/finality timeouts, and RPC failover. If
   submissions rise but successes do not, decode the finalized dispatch error; ordinary
   race-losses are benign only when another keeper advanced the same state.
5. For TWAP loss, identify every decision-window book from live proposals and inspect
   `Market.Markets`, `Market.DecisionWindows`, `Market.WindowCheckpoints`, `Observed` events, and
   `bleavit_keeper_stale_decision_window_books{role="observe"}`. Prioritize stale decision-window
   observations ahead of general observations; the shipped planner does this explicitly
   ([04 §7](../../docs/architecture/04-markets-and-pricing.md)).
6. For the inactivity page, evaluate timestamps per required role. A cleanup success must not
   mask an idle `observe`, `tick`, or `decide` role. Exclude intentionally disabled roles, and
   confirm whether no work was due before declaring a daemon unhealthy
   ([keeper operator guide](../../keeper/README.md)).
7. For the budget page, read `FutarchyTreasury.State.keeper_meter` and the live `keeper.budget` Param,
   then correlate `KeeperBudgetLow { remaining }` and
   `KeeperBudgetExhausted { epoch, spent }`. Both events are latch-once per epoch; effective
   exhaustion emits Low before Exhausted, and Exhausted latches all further metered payments for
   that epoch even if a later parameter change would create headroom
   ([treasury core](../../crates/futarchy-treasury-core/src/lib.rs)).
8. Confirm the failure is not only rebate custody: payout or line-funding failure returns zero
   without charging the meter and cannot roll back the useful crank. Distinguish that from an
   exhausted meter by the on-chain events and meter flags.

## Remediation

### Safe / permissionless

1. Restore at least two independent live keeper instances on distinct RPC providers and failure
   domains. Live mode requires an explicit funded signer; `--dry-run` diagnoses but cannot restore
   progress ([01 §4.2](../../docs/architecture/01-system-overview.md)).
2. Enable the missing live-metadata capability and let the planner submit its bounded,
   idempotent calls. During a decision window, restore `observe`, `tick`, `decide`, `settle`, and
   required `welfare` work before general observation or reaping. Do not hand-construct guessed
   arguments when the planner has disabled a capability.
3. Keep cranking after `KeeperBudgetExhausted`. Every sanctioned crank remains Signed,
   permissionless, and idempotent; only the rebate stops. This is the status-quo-safe continuity
   rule in [08 §6.3](../../docs/architecture/08-treasury-and-economics.md).
4. If the chain itself is stalled or dead-man is engaged, do not replay stale work against an
   unfinalized head. Restore finality first; the epoch clock and decision windows apply their
   specified pause/recovery behavior.

### Privileged

1. The Keeper coordinator may use the funded `ops.keepers` commitment to keep operator accounts
   and infrastructure running beyond meter exhaustion. Any on-chain line funding or spend must
   arrive through a passed TREASURY action and the `FutarchyTreasury` origin; there is no admin
   top-up path ([06 §3.2](../../docs/architecture/06-governance-and-guardians.md),
   [08 §1.1](../../docs/architecture/08-treasury-and-economics.md)).
2. Funding `ops.keepers` does not reopen an exhausted per-epoch keeper meter. Treat it as the
   specified operator-subsidy continuity path, not as permission to alter meter state or retry a
   claimant-favoring payout.
3. Do not use a guardian action merely to cure keeper inactivity or low coverage. Guardian
   playbooks require their own verified trigger; missing information must extend or reject to the
   status quo under the decision rules.

## Escalation

Page the Keeper coordinator first. Page the Infrastructure coordinator when finalized-head or
RPC/archive evidence points outside the fleet, the Oracle operations coordinator for stalled
oracle/registry roles, and the Monitoring coordinator when alert/event ingestion disagrees with
direct finalized storage ([12 §6.1](../../docs/architecture/12-release-and-operations.md)). If a
verified dead-man, reserve, migration, or ledger-drift trigger is present, hand off to its named
guardian playbook runbook; keeper budget exhaustion alone activates none. Record decision-window
coverage loss, affected proposals/books, unpaid continuity duration, and funding used in the
postmortem.

## References

- [01 §4.2 — node roles and off-chain services](../../docs/architecture/01-system-overview.md)
- [04 §7 — TWAP accumulator and stale-event behavior](../../docs/architecture/04-markets-and-pricing.md)
- [05 §3 and §5 — epoch schedule and decision grade](../../docs/architecture/05-welfare-and-decision-engine.md)
- [06 §3 — runtime origins and permissionless crank calls](../../docs/architecture/06-governance-and-guardians.md)
- [08 §1.1 and §6 — keeper funding, meter, and exhaustion](../../docs/architecture/08-treasury-and-economics.md)
- [12 §6 — owned operations and alert rows](../../docs/architecture/12-release-and-operations.md)
- [Shipped keeper operator guide](../../keeper/README.md)
