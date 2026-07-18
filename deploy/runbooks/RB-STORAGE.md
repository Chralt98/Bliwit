---
id: RB-STORAGE
title: Storage bound pressure
owner_role: Monitoring coordinator
funding_line: ops.monitoring
page_immediately: false
alerts:
  - domain: Storage
    trigger: "> 80% of any bound"
spec_refs:
  - docs/architecture/12-release-and-operations.md
  - docs/architecture/13-parameters.md
  - docs/architecture/15-invariants-and-testing.md
  - docs/architecture/06-governance-and-guardians.md
  - tools/limit-coverage/registry.toml
---

## Purpose

Use this runbook when a bounded map, ring, queue, cursor, or measured PoV surface
approaches its normative ceiling. The response protects bounded-state invariant
I-21 and treats the generated limit-coverage inventory as the audit surface; it
does not turn an operational alert into authority to rewrite storage.

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| Storage | per-map counts vs bounds, PoV sizes | > 80% of any bound |

The trigger is an early-warning occupancy or proof-size condition. It is not by
itself proof that a bound was exceeded or that a machine/state invariant failed;
the owning pallet's state, dispatch behavior, and `try-state` result decide that.

## Diagnosis

1. Identify the exact storage item or PoV measurement named by the alert. Resolve
   its normative ceiling and scope from
   [13 §4](../../docs/architecture/13-parameters.md#4-reconciled-storage-bounds-d-10--one-table-all-budgets-derive-from-it);
   do not copy a dashboard threshold into a new source of truth.
2. Compare like scopes. In particular, `Epoch::IntakeQueue` is not the same
   population as the counted `Epoch::Proposals` pipeline. Inspect
   [`IntakeQueue`, `Proposals`, `RecentCohortSummaries`, and `ResourceLocks`](../../pallets/epoch/src/lib.rs)
   separately rather than adding unrelated occupancies.
3. For market or ledger pressure, inspect `Market::Markets` and the ledger's
   `Vaults`, `BaselineVaults`, `Positions`, `PositionCount`, and `PositionTotals`.
   Confirm whether the alert is a count ceiling, an economic storage bound, or a
   per-value encoded-size/PoV concern before choosing a cleanup path.
4. Check the owning pallet's recent lifecycle events and terminal-state age.
   Determine whether normal bounded cleanup is progressing, stalled, or blocked
   by an archive-delay or settlement precondition. Do not invent a pruning call
   for a surface that has none.
5. Run or inspect the relevant `try-state` result. Every pallet's machine/state
   invariant coverage and the release-blocking consequence of failure are defined
   by
   [15 §1](../../docs/architecture/15-invariants-and-testing.md#1-protocol-invariants-backend-i-1i-27).
6. For a PoV alert, compare the measured call at its worst bounded arguments with
   the committed benchmark/weight regression evidence specified in
   [15 §4.5](../../docs/architecture/15-invariants-and-testing.md#45-fuzzing-benchmarks-weights).
   Keep state occupancy and per-call proof size as separate diagnoses.
7. Check the key's entry in
   [`tools/limit-coverage/registry.toml`](../../tools/limit-coverage/registry.toml)
   and run the generated coverage checker. The classification records whether the
   surface is a dispatch limit, parameter bound, storage-layout value, diagnostic,
   or explicitly unwired; it must agree with [15 §4.6](../../docs/architecture/15-invariants-and-testing.md#46-generated-limit-coverage-suite-i-22-enforcement).

## Remediation

### Safe / permissionless

1. Invoke only an existing permissionless lifecycle crank whose owning pallet
   documents the affected state transition, such as epoch progression,
   settlement, stale-queue rejection, failed-execution expiry, or an exposed
   archive-delay reap. Submit bounded batches and remeasure after each dispatch.
2. If the required precondition is not met, leave the item in place. Never bypass
   archive delay, settlement, or conservation checks merely to reduce occupancy.
3. Keep the alert active until both occupancy/PoV evidence and the owning
   `try-state` check are healthy. A successful cleanup transaction does not prove
   a different map or invariant is healthy.

### Privileged / governance-controlled

1. `system.set_storage`, `kill_storage`, and `kill_prefix` are in the nobody row
   from genesis under
   [06 §3.2](../../docs/architecture/06-governance-and-guardians.md#32-authority-matrix-call-level-capability-table-normative).
   Do not propose or execute them as an incident shortcut.
2. A proposed parameter or code change follows its normal classified governance
   path. Storage-bound changes reopen the owning architecture and derived-budget
   analysis; an alert never authorizes raising a ceiling in place.
3. Block release or upgrade progression on a red `try-state`, limit-coverage, or
   PoV gate. Preserve the incumbent runtime as the status-quo default until the
   bounded remediation has passed the normative verification suite.

## Escalation

The Monitoring coordinator is first responder. Page the owner of the pallet or
runtime surface once the exact bound is identified, and page the Release
operations lead if a release, migration, benchmark, or `try-state` gate is red.

Storage pressure alone activates no guardian playbook. An independently verified
I-4 drift flag routes to `RB-LEDGER` and may admit `PB-LEDGER-FREEZE`; a migrations
halt-and-alarm flag routes to `RB-UPGRADE` and may admit `PB-MIGRATION`. Those
playbooks remain trigger-gated under
[06 §6.2](../../docs/architecture/06-governance-and-guardians.md#62-playbook-registry-and-enumerated-capability-table-resolves-the-emergencyplaybook-enumeration-medium).
Record the root cause, the exact registry/bound row, and the evidence that the
condition cleared in the incident postmortem.

## References

- [12 §6.3 — storage alert binding](../../docs/architecture/12-release-and-operations.md#63-monitoring-and-alerting)
- [13 §4 — reconciled storage-bound registry](../../docs/architecture/13-parameters.md#4-reconciled-storage-bounds-d-10--one-table-all-budgets-derive-from-it)
- [15 §1 — bounded-state invariants and `try-state`](../../docs/architecture/15-invariants-and-testing.md#1-protocol-invariants-backend-i-1i-27)
- [15 §4.5–§4.6 — PoV and generated limit coverage](../../docs/architecture/15-invariants-and-testing.md#45-fuzzing-benchmarks-weights)
- [06 §3.2 and §6.2 — authority and trigger-gated playbooks](../../docs/architecture/06-governance-and-guardians.md#32-authority-matrix-call-level-capability-table-normative)
- [Generated limit-coverage audit surface](../../tools/limit-coverage/registry.toml)
