---
id: RB-LEDGER
title: Ledger collateralization breach response
owner_role: Monitoring coordinator
funding_line: ops.monitoring
page_immediately: true
alerts:
  - domain: Collateralization
    trigger: any drift ≠ 0
spec_refs:
  - docs/architecture/03-conditional-ledger.md
  - docs/architecture/06-governance-and-guardians.md
  - docs/architecture/09-execution-upgrades-and-rollout.md
  - docs/architecture/12-release-and-operations.md
  - docs/architecture/15-invariants-and-testing.md
---

## Purpose

This page-immediately runbook covers any unexplained difference between ledger
escrow liabilities and the ledger sovereign USDC balance. A confirmed difference
is an I-3/I-4-class solvency incident: containment and evidence preservation take
precedence over liveness, and no action may mint or release claims while the
ledger is drifted ([03 §6/§9](../../docs/architecture/03-conditional-ledger.md),
[15 I-1/I-3/I-4](../../docs/architecture/15-invariants-and-testing.md)).

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| Collateralization | Σ escrow vs sovereign balance drift | any drift ≠ 0 |

The trigger means the reconciliation gauge observed a non-zero unexplained
difference. Expected genesis endowment, swept dust, and position deposits must be
accounted exactly as [03 L-2](../../docs/architecture/03-conditional-ledger.md)
defines; they are not permission to dismiss a residual difference.

## Diagnosis

1. Page immediately. Record the finalized block hash, runtime `spec_version`,
   monitor query and result, ledger sovereign account, canonical USDC `Location`,
   and the first block at which the gauge diverged.
2. Before dispatching containment, repeat the reconciliation against the same
   finalized block on an independent archive node. A best-head/reorg sample,
   stale metadata decoder, wrong asset key, or omission of deposit accounting is
   a monitor failure, not a chain solvency breach.
3. Sum `ConditionalLedger::Vaults[*].escrowed` and
   `ConditionalLedger::BaselineVaults[*].escrowed`; separately read
   `ConditionalLedger::DepositsHeld` and the sovereign balance in
   `ForeignAssets`. Apply the L-2 accounting treatment from
   [03 §9](../../docs/architecture/03-conditional-ledger.md) exactly.
4. Cross-check `ConditionalLedger::PositionTotals` against live
   `ConditionalLedger::Positions`, then verify each vault's per-branch L-1
   equation. Capture the first proposal, baseline vault, or instrument whose
   supply projection diverges.
5. Run the same read-only `try-state` logic against an archive snapshot when the
   environment supports it. A failure confirms an invariant breach; a green
   result alongside a non-zero monitor gauge is evidence of monitor arithmetic
   or input drift and must be fixed before clearing the alert.
6. Audit events from the last known-good block: `Split`, `Merged`, scalar/gate
   split and merge events, redemption events, settlement/VOID events, and dust
   sweeps. Identify the first state transition after which the equation fails;
   do not replay any value-moving call on production.
7. Check `Constitution::PhaseFlags` for the `LEDGER_FROZEN` effect and
   `Guardian::ActivePlaybooks` for `LedgerFreeze`. Do not confuse a freeze effect
   with the machine-checked I-4 trigger that makes activation admissible.
8. Deployment-readiness check: the current runtime must expose the live
   `ledger_drift` trigger to `Guardian`. If it does not, activation cannot be
   replaced by operator judgment; record the missing binding as a blocking
   implementation incident and escalate immediately.

## Remediation

### Safe / permissionless

1. Stop automated split, merge, trade, observation, redemption, settlement, and
   sweep submissions while diagnosis is in progress. Preserve signed extrinsics,
   events, storage proofs, and archive snapshots from the first bad block.
2. Keep reconciliation, evidence collection, oracle/dispute calls, and governance
   reads available. Do not submit any action that could mint a position, transfer
   a claim, pay a redemption, or release escrow while drift remains non-zero.
3. If both archive-node calculations prove the monitor wrong, repair the monitor,
   replay its calculation over the incident interval, and require a finalized
   zero result before resolving the page. A monitor correction never writes chain
   state.

### Privileged

1. On a confirmed I-4 drift flag, use the [06 §6.3](../../docs/architecture/06-governance-and-guardians.md)
   guardian 5-of-7 path to activate `PB-LEDGER-FREEZE`. Cite that playbook; do not
   restate or improvise its call batch, expiry, or renewal parameters.
2. Verify `GuardianAction`, `PlaybookActivated`, `ReviewScheduled`, the active
   playbook record, and the `LEDGER_FROZEN` phase bit before declaring containment.
   The shipped runtime currently treats unavailable downstream playbook effects
   as an error; an event proposal or approval without a successful effect is not
   containment.
3. Keep the status-quo default: no guardian, treasury, sudo, or governance action
   may manually edit storage, manufacture collateral, mint/release claims, or
   bypass the freeze. A code repair uses the normal execution guard and, only
   while the verified freeze is active, the expedited CODE lane in
   [09 §3.1](../../docs/architecture/09-execution-upgrades-and-rollout.md).
4. Lift containment only through the playbook's specified reconciliation-clear
   path or expiry/recovery process after `try-state` and archive reconciliation
   are green. Never clear the visible effect merely to restore liveness.

## Escalation

Page the Infrastructure coordinator for independent archive proofs and the
Release operations lead if a guarded repair upgrade is required. Page the guardian
council only after the archive confirmation, with the I-4 trigger evidence and the
exact last-good/bad block pair. Every activation must enter the mandatory
retrospective ratification trail; unratified action consequences are owned by
[06 §5.4](../../docs/architecture/06-governance-and-guardians.md). If the freeze
cannot activate because the trigger/effect binding is absent, announce that the
chain is not contained and keep all operator-submitted value-moving work stopped.

## References

- [03 §6/§9 — conservation and try-state reconciliation](../../docs/architecture/03-conditional-ledger.md)
- [06 §5.4/§6.2–§6.3 — guardian accountability and PB-LEDGER-FREEZE](../../docs/architecture/06-governance-and-guardians.md)
- [09 §3.1 — guarded expedited repair lane](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [12 §6.3 — collateralization alert](../../docs/architecture/12-release-and-operations.md)
- [15 I-1/I-3/I-4 — ledger invariant classification](../../docs/architecture/15-invariants-and-testing.md)
