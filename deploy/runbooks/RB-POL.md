---
id: RB-POL
title: Protocol-owned liquidity floor disturbance
owner_role: Monitoring coordinator
funding_line: ops.monitoring
page_immediately: false
alerts:
  - domain: Liquidity floors
    trigger: "POL disturbed"
spec_refs:
  - docs/architecture/04-markets-and-pricing.md
  - docs/architecture/05-welfare-and-decision-engine.md
  - docs/architecture/08-treasury-and-economics.md
  - docs/architecture/12-release-and-operations.md
  - docs/architecture/13-parameters.md
---

## Purpose

Restore the treasury/POL funding and seeding conditions required for decision-grade books when
effective protocol-owned liquidity falls below its class floor or is disturbed during a decision
window, without manually changing an open book.

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| Liquidity floors | effective POL vs floor | POL disturbed |

The alert means a book's effective POL no longer satisfies the applicable on-chain class floor or
the undisturbed-window condition. This is decision safety, not merely market quality: the decision
engine must treat the affected book as not decision-grade.

## Diagnosis

1. Pin the affected market, proposal class, branch, gate/decision kind, decision window, and
   finalized hash. Resolve class floors and current `pol.b*`/budget Params from chain metadata and
   storage; values and bounds are owned by [13](../../docs/architecture/13-parameters.md).
2. Read `Market.Markets`, `Market.SeededMarkets`, `Market.RerunSeededMarkets`, and the owning
   ledger book-account positions. Correlate `MarketCreated` and `Seeded` to prove that lifecycle
   seeding occurred exactly once and produced the expected complete-set inventory.
3. Read `FutarchyTreasury.State` line balances and `pol_commitments`, plus `FutarchyApi::nav()` obligations
   and spendable NAV. A missing/underfunded POL or POL_BASELINE line, failed seed, reserve haircut,
   or stale commitment mirror are distinct incidents ([08 §1 and §3](../../docs/architecture/08-treasury-and-economics.md)).
4. Determine whether disturbance is pre-creation or mid-window. Pre-creation insufficiency should
   shrink/defer the funded slate or fail the loud NAV gate. Mid-window withdrawal, inventory drift,
   or an unexpected `b`/seed change invalidates grade for that window.
5. Check `FutarchyApi::decision_stats(pid)`, `Market.DecisionWindows`, coverage/staleness, and the
   book's grade inputs. The current pallet grade proves `SeededMarkets` and `book.b >= pol_floor`;
   the §6.3 “effective POL vs floor” monitoring series and time-averaged disturbance evidence are
   planned with O5, so use chain/event reconstruction rather than inventing a storage name.
6. Check the hard-gate coupling in [05 §5.1–§5.2](../../docs/architecture/05-welfare-and-decision-engine.md).
   For gated proposals, all gate books must be valid before ruin-veto evaluation; welfare books
   also require POL floor/undisturbed conditions. No welfare margin can override a gate veto, and
   no disturbed book can be treated as decision-grade.
7. Inspect reserve-health state and `NavHaircutFlagged`. Under the reserve flag, spendable NAV is
   zero and new POL seeding is intentionally fail-static. Do not diagnose this as an isolated seed
   bug.
8. Look for the specified `SlotsShrunk`/`NavFloorUnmet` evidence when capacity is insufficient.
   `NavFloorUnmet` is shipped by Treasury; `SlotsShrunk` is specified in [08 §4.4](../../docs/architecture/08-treasury-and-economics.md)
   but is not present in the current epoch event enum, so its absence must be reported as a
   monitoring/implementation gap rather than proof no shrink occurred.

## Remediation

### Safe / permissionless

1. Restore due keeper progress and observation coverage if the apparent disturbance is stale
   monitoring. A keeper may run only the existing permissionless lifecycle cranks; it cannot seed
   or resize POL by discretion.
2. Preserve the book and ledger state. Do not transfer positions into the book account, re-run
   `seed`, or trade treasury funds to imitate the floor. `AlreadySeeded` is an idempotence guard,
   not an instruction to bypass it.
3. Allow affected decisions to extend or reject as not decision-grade. This is the safe status-quo
   result while POL evidence is insufficient.
4. If the issue is only alert reconstruction, repair the O5 exporter/rule from finalized storage
   and events; never silence a real decision-window disturbance by widening the threshold.

### Privileged

1. Replenish POL/POL_BASELINE through the treasury budget-line and commitment paths owned by
   [08 §1, §3, and §8](../../docs/architecture/08-treasury-and-economics.md). On-chain funding must
   be a passed TREASURY action dispatched with `FutarchyTreasury`; there is no operator or Root
   shortcut ([06 §3.2](../../docs/architecture/06-governance-and-guardians.md)).
2. Funding enables the ordinary next lifecycle seed; it does not retroactively make a disturbed
   decision window valid. Do not privilegedly reopen or recapitalize an already-closed book.
3. If reserve impairment caused the halt, use RB-TREASURY/RB-ORACLE and route to PB-RESERVE only
   while its verified trigger is live. If ledger drift caused it, hand off to RB-LEDGER and its
   stricter freeze playbook. The current runtime's downstream playbook effects fail closed
   ([runtime dispatcher](../../runtime/bleavit-runtime/src/configs.rs)), so the specified playbook
   is an escalation/repair requirement today, not a containment state operators may assume.

## Escalation

Page the Monitoring coordinator first, the Keeper coordinator for stale lifecycle/observation
work, and the Oracle operations coordinator for reserve-health evidence. A funding deficit needs
the published TREASURY governance path; a code/event-surface defect also pages the Release
operations lead. Any guardian playbook activation carries the mandatory retrospective review and
postmortem obligations of [06 §5.4–§6](../../docs/architecture/06-governance-and-guardians.md).

## References

- [04 §3, §6.3, and §10 — subsidy bound and POL seeding](../../docs/architecture/04-markets-and-pricing.md)
- [05 §5.1–§5.2 — hard-gate ordering and POL decision grade](../../docs/architecture/05-welfare-and-decision-engine.md)
- [08 §1, §3–§5, and §8 — treasury, floors, and replenishment flow](../../docs/architecture/08-treasury-and-economics.md)
- [12 §6.3 — Liquidity floors alert](../../docs/architecture/12-release-and-operations.md)
- [13 §1 and §4 — parameter registry and bounds](../../docs/architecture/13-parameters.md)
