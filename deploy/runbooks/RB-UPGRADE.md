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

The trigger means the active `Migrations::Cursor` has not advanced under the
machine stall rule. It does not authorize a force-set of the cursor, storage
rewrite, or rollback to old bytes ([12 §6.3](../../docs/architecture/12-release-and-operations.md)).

## Diagnosis

1. Record a finalized block hash, current and target `spec_version`, authorized
   code hash, migration identifier, cursor bytes, failed-step index, and the first
   block at which progress stopped. Preserve raw SCALE as well as decoded state.
2. Read `Migrations::Cursor` and the runtime-internal
   `BleavitRuntimeMigration::{HaltSources, FailedStep, ProgressMarker}` values.
   Confirm repeated finalized samples carry the same active cursor marker, or
   that the SDK cursor is `Stuck`; rule out a stale monitor or slow but advancing
   migration.
3. Read `ExecutionGuard::MigrationHalt`, `PendingUpgrade`,
   `PendingUpgradeCheckpoint`, `LastUpgradeAuthorized`, and the relevant
   `ExecutionGuard::Queue` entry. Capture its `pre_upgrade_checkpoint` parent
   block hash and state root as the immutable audit anchor
   ([09 §2.1/§3.2](../../docs/architecture/09-execution-upgrades-and-rollout.md)).
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
   halted-state writes with the checkpoint; do not infer the class from an error
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
   the cursor, failed step, checkpoint, and impact boundary for operators.
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
   forward CODE upgrade restoring checkpoint semantics for the affected keys.
   Build, attest, ratify, and descriptor-cover it through the expedited lane that
   the active migration halt makes admissible.

### Privileged

1. While the verified migration trigger is active, the guardian 5-of-7 may
   activate registered `PB-MIGRATION` ([06 §6.2](../../docs/architecture/06-governance-and-guardians.md)).
   Verify the effect, not merely approvals. The current runtime returns an error
   when downstream guardian playbook effects are unavailable; in that state the
   chain is not operationally contained.
2. Activation and every recovery step must produce the spec trail:
   `GuardianAction`, `MigrationHalted {cursor, failed_step}`, `PlaybookActivated`,
   and `ReviewScheduled`, with the justification hash tied to the checkpoint and
   diagnosis. Preserve equivalent raw proofs if an implementation gap prevents an
   event, and escalate the gap.
3. Decide retry versus rollback within the deadline in [09 §3.2](../../docs/architecture/09-execution-upgrades-and-rollout.md);
   inaction defaults to rollback initiation. Guardians may freeze and use bounded
   continuation controls, but cannot install code.
4. A rollback is a forward upgrade through the normal execution guard, using the
   migration-halt-gated expedited CODE lane. Full attestation, ratification,
   payload checks, and `DescriptorLeadTime` still apply; no privileged shortcut
   exists ([09 §3.1](../../docs/architecture/09-execution-upgrades-and-rollout.md)).
5. Clear the halt only after migration completion and green `try-state`. Do not
   unpause because the cursor was manually removed or because a release was merely
   published.

## Escalation

Page the Release operations lead, Infrastructure coordinator, guardian council,
and the owners of the affected pallet immediately after confirmation. The
release team owns the covering descriptors and repair train; guardians own only
the scoped `PB-MIGRATION` activation. Every activation auto-schedules a
retrospective `ratify` review, and an unratified activation carries the guardian
bond consequence specified by [06 §5.4](../../docs/architecture/06-governance-and-guardians.md).
If descriptor coverage is consuming the lead-time margin, invoke RB-RELEASE's
descriptor-release leg; never apply code before the on-chain lead-time gate.

## References

- [09 §2 — two-phase upgrade and checkpoint path](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [09 §3.1–§3.2 — expedited lane and PB-MIGRATION](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [06 §5.4/§6.2 — guardian review and playbook scope](../../docs/architecture/06-governance-and-guardians.md)
- [12 §1/§6.3 — descriptor release coupling and alert](../../docs/architecture/12-release-and-operations.md)
- [15 §4.7 — upgrade verification](../../docs/architecture/15-invariants-and-testing.md)
