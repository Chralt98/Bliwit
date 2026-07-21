---
id: RB-UPGRADE
title: Stalled migration recovery
owner_role: Release operations lead
funding_line: ops.arweave / ops.monitoring
page_immediately: false
alerts:
  - domain: Upgrades
    trigger: cursor stalled
spec_refs:
  - docs/architecture/06-governance-and-guardians.md
  - docs/architecture/09-execution-upgrades-and-rollout.md
  - docs/architecture/12-release-and-operations.md
  - docs/architecture/15-invariants-and-testing.md
---

## Purpose

This runbook is the operational face of `PB-MIGRATION`. It covers a multi-block
migration cursor that has stopped making progress: the affected surface must
fail-stop while block production and unaffected work continue, and recovery is a
bounded retry or a rollback implemented as a forward upgrade
([09 §2/§3.2](../../docs/architecture/09-execution-upgrades-and-rollout.md)).

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| Upgrades | authorized hash, applied version, migration cursor | cursor stalled |

The trigger *should* mean an active `Migrations::Cursor` has been running longer
than its budget — `now − cursor.started_at > 900` blocks (09 §3.2(d)); it is a
**time budget, not a progress test**, because a lawful migration may return
byte-identical cursors for hours while doing real work. **The deployed runtime
does not implement that yet**: it still raises on `(index, inner_cursor)`
byte-equality held for > 900 blocks, so a healthy `Cursor = ()` migration can
false-raise this alert. Treat a raise as *unconfirmed* until you have checked
`started_at` by hand (Diagnosis 2). Implementation owed — PLAN.md SQ-132 in
batch X.

The alert does not authorize a force-set of the cursor, a storage
rewrite, or a rollback to old bytes ([12 §6.3](../../docs/architecture/12-release-and-operations.md)).

## Diagnosis

1. Record a finalized block hash, current and target `spec_version`, authorized
   code hash, migration identifier, cursor bytes, failed-step index, and the first
   block at which progress stopped. Preserve raw SCALE as well as decoded state.
2. Read `Migrations::Cursor` and the runtime-internal
   `BleavitRuntimeMigration::{HaltSources, FailedStep, ProgressMarker}` values —
   `ProgressMarker` is still what the deployed detector keys on, so you need it to
   explain *why* the halt raised. Then compute `now − cursor.started_at` and
   compare it against 900: that is the normative predicate (09 §3.2(d)) and the
   one that decides whether this is a real stall. **A halt raised while
   `now − started_at ≤ 900` is a false raise** — the migration is progressing and
   returning identical cursor bytes; record it as such and do not begin a repair.
   Confirm `Stuck` separately. Rule out a stale monitor.
3. Read `ExecutionGuard::MigrationHalt`, `PendingUpgrade`, `LastUpgradeAuthorized`
   and the relevant `ExecutionGuard::Queue` entry.
   **The 09 §3.2(2) pre-migration anchor is not yet implemented — do not wait for
   it and do not read `PendingUpgradeCheckpoint` expecting to find it.** That item
   is killed at the relay go-ahead callback, one block before any migration can
   step; the queue row's `pre_upgrade_checkpoint` is written at execute but the
   row itself is deleted on success (09 §1.2(13)). Both cells are therefore empty
   at diagnosis time (SQ-127/SQ-144, ruled 2026-07-20 — capture moves to
   code-application time and single-homes in its own storage item; implementation
   owed, PLAN.md SQ-127/SQ-144 in batch X). Until it lands, reconstruct the anchor off-chain: **the block in
   which `UpgradeApplied` was emitted is itself the last pre-migration block** —
   the go-ahead callback runs in the final old-code block, and the new code takes
   effect in its successor. Take that block's own header hash and `state_root`
   (equivalently: the parent of the first block carrying the new `spec_version`).
   Record both in the incident log as the audit anchor and note that they were
   reconstructed, not read from chain
   ([09 §3.2](../../docs/architecture/09-execution-upgrades-and-rollout.md)).
4. Reconstruct `UpgradeAuthorized`, `UpgradeApplied`, `UpgradeAborted`, execution,
   migration, and `GuardianAction` events. The spec also mandates
   `MigrationHalted {cursor, failed_step}`. If the deployed runtime does not emit
   it, preserve the raw cursor/failed-step proof and record the missing event as a
   compliance gap; absence of the event is not evidence that no halt exists.
5. Confirm halt-at-fault behavior: blocks finalize and unaffected calls remain
   usable, while the affected transaction surface, execution queue, and new
   ledger/market inflows fail-stop. Any half-migrated layout exposed to user calls
   is a separate invariant breach.
6. Classify the fault from evidence: resource-bounded overrun/transient host
   failure versus logic fault, storage-shape mismatch, or failed retry. Compare
   halted-state writes with the reconstructed anchor; do not infer the class from an error
   string alone.
7. Check release coupling. From `UpgradeAuthorized`, compare `applicable_at`, the
   covering production descriptor release, `ReleaseChannel`, and the committed
   metadata/artifact hashes. A repair or rollback-forward artifact still follows
   the descriptor pipeline and release train
   ([09 §2.2](../../docs/architecture/09-execution-upgrades-and-rollout.md),
   [12 §1](../../docs/architecture/12-release-and-operations.md)).

## Remediation

### Safe / permissionless

1. Preserve block production and unaffected services; stop automated execution,
   inflow, and migration-control submissions against the affected surface. Publish
   the cursor, failed step, reconstructed anchor, and impact boundary for operators.
2. Keep ordinary finalized reads, alerting, and release preparation live. Do not
   call `Migrations.force_set_cursor`, `force_set_active_cursor`,
   `force_onboard_mbms`, or `clear_historic`; the runtime classifier denies these
   calls and [09 §3.2](../../docs/architecture/09-execution-upgrades-and-rollout.md)
   forbids set-storage-class recovery.
3. Prepare a retry only for the resource-bounded classes and within the attempt
   rule owned by [09 §3.2](../../docs/architecture/09-execution-upgrades-and-rollout.md).
   Re-benchmark the step and run `try-runtime`/`try-state` against the halted
   snapshot before proposing continuation. The exact upstream control surface is
   still a spec `[VERIFY]`; never improvise a force call.
4. For every other class, or an exhausted retry path, prepare rollback as a
   forward CODE upgrade restoring the pre-migration-anchor semantics for the affected keys.
   Build, attest, ratify, and descriptor-cover it through the expedited lane that
   the active migration halt makes admissible.

### Privileged

1. While the verified migration trigger is active, the guardian 5-of-7 may
   activate registered `PB-MIGRATION` ([06 §6.2](../../docs/architecture/06-governance-and-guardians.md)).
   Verify the effect, not merely approvals. The current runtime returns an error
   when downstream guardian playbook effects are unavailable; in that state the
   chain is not operationally contained.
2. **`PB-MIGRATION` has no dispatchable activation** and therefore produces no
   guardian trail: its admissible call set is empty, so a fifth approval fails
   closed and the whole extrinsic reverts, recording no `PlaybookActivated`, no
   allowance consumption and no review record
   ([06 §6.2](../../docs/architecture/06-governance-and-guardians.md)). Do not
   wait for those events and do not treat their absence as an implementation
   gap. The accountability trail is the automatic `MigrationHalt` halt-source
   bridge and its own event stream; capture the halt-source bits and the
   diagnosis as the raw proof. (`MigrationHalted {cursor, failed_step}` is
   likewise not emitted today — tracked in PLAN.md, do not block on it.)
3. Decide retry versus rollback within the deadline in [09 §3.2](../../docs/architecture/09-execution-upgrades-and-rollout.md);
   inaction defaults to rollback initiation. Guardians cannot install code, and
   on stable2606 they have **no in-framework retry either**: `pallet-migrations`'
   continuation controls are Root-only and filtered to the D-13 "nobody" row, so
   both retry and rollback ride the expedited-CODE lane (SQ-274).
   **⚠ Know before you plan the repair: while any `Migrations::Cursor` exists the
   chain is inherent-only.** `MultiStepMigrator::ongoing()` is `Cursor::exists()`
   and `frame-executive` then rejects every non-inherent extrinsic, so the
   expedited-CODE lane named above — and `execute(pid)`, guardian calls, and sudo
   with it — **cannot be included in a block** until the cursor is gone. This is
   an open spec question (PLAN.md SQ-309), not a settled procedure: escalate to
   the guardian council and the release lead immediately rather than attempting
   an on-chain repair that cannot be submitted. A `Stuck` cursor with
   `FailedMigrationHandling::KeepStuck` has no on-chain exit at present.
4. A rollback is a forward upgrade through the normal execution guard, using the
   migration-halt-gated expedited CODE lane. Full attestation, ratification,
   payload checks, and `DescriptorLeadTime` still apply; no privileged shortcut
   exists ([09 §3.1](../../docs/architecture/09-execution-upgrades-and-rollout.md)).
5. Lift the **guardian playbook freeze** only after migration completion and a
   green `try-state` run — that precondition binds the operator freeze, not the
   on-chain flag, because `try-state` never runs in production block execution
   ([09 §3.2](../../docs/architecture/09-execution-upgrades-and-rollout.md),
   amendment (c)). The on-chain `MigrationHalt` clears **mechanically** on
   migration completion or successful recovery-image application. Do not declare
   the incident closed because the cursor was manually removed or because a
   release was merely published.

## Escalation

Page the Release operations lead, Infrastructure coordinator, guardian council,
and the owners of the affected pallet immediately after confirmation. The
release team owns the covering descriptors and repair train. Guardians own no
`PB-MIGRATION` activation at all on stable2606 (step 2): their role here is to
initiate the expedited-CODE repair lane, which the halt makes admissible. The
retrospective-`ratify` review and the unratified-activation bond consequence of
[06 §5.4](../../docs/architecture/06-governance-and-guardians.md) attach to
guardian actions that actually dispatch — so they attach to that lane's guardian
steps, not to a `PB-MIGRATION` activation that cannot occur.
If descriptor coverage is consuming the lead-time margin, invoke RB-RELEASE's
descriptor-release leg; never apply code before the on-chain lead-time gate.

## References

- [09 §2 — two-phase upgrade path](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [09 §3.2(2) — the pre-migration anchor](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [09 §3.1–§3.2 — expedited lane and PB-MIGRATION](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [06 §5.4/§6.2 — guardian review and playbook scope](../../docs/architecture/06-governance-and-guardians.md)
- [12 §1/§6.3 — descriptor release coupling and alert](../../docs/architecture/12-release-and-operations.md)
- [15 §4.7 — upgrade verification](../../docs/architecture/15-invariants-and-testing.md)
