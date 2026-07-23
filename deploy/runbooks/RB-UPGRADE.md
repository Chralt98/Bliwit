---
id: RB-UPGRADE
title: Stalled migration recovery
owner_role: Release operations lead
funding_line: ops.arweave / ops.monitoring
page_immediately: true
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
migration cursor that has exceeded its declared time budget or entered `Stuck`:
the affected surface must fail-stop while block production continues. Recovery
uses the immutable terminal image committed and qualified with the primary
([09 §2/§3.2](../../docs/architecture/09-execution-upgrades-and-rollout.md)).

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| Upgrades | authorized hash, applied version, migration cursor | cursor stalled |

The live trigger means an active `Migrations::Cursor` has been running longer
than its budget — `now − cursor.started_at > 900` blocks (09 §3.2(d)) — or the
cursor is `Stuck`. It is a **time budget, not a progress test**, because a lawful
migration may return byte-identical cursors for hours while doing real work.
The runtime reads the SDK cursor's own `started_at`; the retired
`BleavitRuntimeMigration::ProgressMarker` is not an evidence source.

The alert does not authorize a force-set of the cursor, a storage
rewrite, or a rollback to old bytes ([12 §6.3](../../docs/architecture/12-release-and-operations.md)).

## Diagnosis

1. Record a finalized block hash, current and target `spec_version`, authorized
   code hash, migration identifier, cursor bytes, failed-step index, and the first
   block at which the cursor became active (`cursor.started_at`). Preserve raw
   SCALE as well as decoded state.
2. Read `Migrations::Cursor` and the runtime-internal
   `BleavitRuntimeMigration::{HaltSources, FailedStep}` values. Compute
   `now − cursor.started_at` and compare it against 900: that is the normative
   predicate (09 §3.2(d)). Confirm `Stuck` separately. If the stall halt is set
   while `now − started_at ≤ 900` and the cursor is not `Stuck`, preserve the raw
   state and treat the detector/signal disagreement as an invariant breach; do
   not infer progress or rewrite the cursor. Rule out a stale monitor.
3. Read `ExecutionGuard::MigrationHalt`, `PendingUpgrade`, `LastUpgradeAuthorized`
   and the relevant `ExecutionGuard::Queue` entry.
   Read `ExecutionGuard.PreMigrationAnchor`. Per 09 §3.2(2), it is captured at
   code application only when the applied image registers a multi-block migration
   cursor, and stores `(anchor_block, anchor_hash)` for the last pre-migration
   block. It is absent during authorization, absent for a zero-MBM image, and
   cleared by the migration-completion callback. If a live or halted migration
   has no anchor, preserve the raw cursor and surrounding headers and treat the
   missing cell as an invariant breach; do not reconstruct a healthy on-chain
   value by assumption
   ([09 §3.2](../../docs/architecture/09-execution-upgrades-and-rollout.md)).
4. Reconstruct `UpgradeAuthorized`, `UpgradeApplied`, `UpgradeAborted`, execution,
   migration, and `GuardianAction` events. Locate the first
   `MigrationHalted {cursor, failed_step}` for this halt-source activation. Treat
   those fields as the machine trigger's snapshot and compare them with a state
   proof at the event block/phase, not merely with current storage. On a failed
   migration step the handler emits while the SDK cursor is still `Active`, after
   which the SDK writes `Stuck`; on a stall, later execution in the same block may
   also advance or replace the live cursor. Those expected transitions do not make
   a different current cursor an evidence mismatch. The event is emitted once on
   the first machine-trigger halt; a later halt source does not emit it again. If
   that first event is absent, preserve the raw cursor/failed-step proof and record
   a compliance gap; absence of the event is not evidence that no halt exists.
5. Confirm halt-at-fault behavior. With a genuine live/retired migration
   cursor, blocks and inherents continue and finalized reads/monitoring remain
   available, but `OnlyInherents` pauses **every ordinary extrinsic** until
   terminal recovery; no user call is "unaffected." A source-less staged halt
   has no half-migrated layout and freezes only the execution queue. Any user
   call admitted against a genuine half-migrated layout is an invariant breach.
6. Read the committed recovery descriptor, qualification cache/queue transfer,
   preimage request, paired build records, and both boot-extracted runtime
   identities. Require one source commit/base/spec name and recovery version
   exactly primary + 1. After primary application, attestor roster or challenge
   changes do not alter this frozen terminal eligibility.
7. During recovery, verify the exact cursor moved to
   `RetiredMigrationCursor`, lockdown stayed active, and Cumulus scheduled only
   the committed recovery hash. On relay Abort the same cursor must be restored
   byte-for-byte; on GoAhead the installed `:code` identity must match before
   the terminal transition runs.

## Remediation

### Safe / permissionless

1. Preserve block production, inherents, finalized reads, monitoring and release
   preparation. Under a genuine cursor, stop all ordinary transaction
   submission—the runtime rejects it globally—and publish the cursor, failed
   step, reconstructed anchor and impact boundary for operators. Under a
   source-less staged halt, stop automated execution submissions against the
   gated surface.
2. Keep alerting and artifact verification live. Do not
   call `Migrations.force_set_cursor`, `force_set_active_cursor`,
   `force_onboard_mbms`, or `clear_historic`; the runtime classifier denies these
   calls and [09 §3.2](../../docs/architecture/09-execution-upgrades-and-rollout.md)
   forbids set-storage-class recovery.
3. Do not select or submit new recovery bytes after lockdown. The mandatory
   inherent owns scheduling of the precommitted terminal image; ordinary,
   guardian, sudo and expedited-CODE extrinsics cannot execute in
   `OnlyInherents`.
4. If the paired image is absent, mismatched, relay-aborted, or its
   release-specific repair fails, preserve the locks and escalate. Never clear
   the cursor or substitute a generic repair. Current production profiles
   prohibit ordinary MBMs; a cursor therefore proves a release-gate bypass
   unless its release carried exhaustive cutpoint-repair evidence.

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
   bridge, the first `MigrationHalted {cursor, failed_step}` event, and the raw
   state proof; capture all three.
3. Guardians cannot install, replace, accelerate, or cancel the terminal image.
   Confirm that the inherent attempt is advancing and that the paired recovery
   artifact is the release-published one. Relay `force_set_current_code` remains
   break-glass only and is not a protocol recovery step.
4. Any follow-up CODE proposal is possible only after successful terminal
   recovery has restored ordinary inclusion. It follows the normal guard,
   attestation, ratification and descriptor rules.
5. Lift the **guardian playbook freeze** only after migration completion and a
   green `try-state` run — that precondition binds the operator freeze, not the
   on-chain flag, because `try-state` never runs in production block execution
   ([09 §3.2](../../docs/architecture/09-execution-upgrades-and-rollout.md),
   amendment (c)). The on-chain `MigrationHalt` clears **mechanically** on
   migration completion or successful recovery-image application. Do not declare
   the incident closed because the cursor was manually removed or because a
   release was merely published.

## Escalation

This alert pages on arrival, not after triage: a stalled cursor already holds
ordinary transactions under the multi-block-migration lockdown, so the on-call
responder is paged by the alert itself
([12 §6.3](../../docs/architecture/12-release-and-operations.md);
[09 §3.2](../../docs/architecture/09-execution-upgrades-and-rollout.md)). Widen
to the Release operations lead, Infrastructure coordinator, guardian council,
and the owners of the affected pallet once the halt is confirmed. The
release team owns the paired artifact evidence and recovery audit. Guardians own
no `PB-MIGRATION` activation at all on stable2606 (step 2) and cannot initiate a
code path while the chain is inherent-only. The
retrospective-`ratify` review and the unratified-activation bond consequence of
[06 §5.4](../../docs/architecture/06-governance-and-guardians.md) attach to
guardian actions that actually dispatch, not to a `PB-MIGRATION` activation that
cannot occur.
If descriptor coverage is consuming the lead-time margin, invoke RB-RELEASE's
descriptor-release leg; never apply code before the on-chain lead-time gate.

## References

- [09 §2 — two-phase upgrade path](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [09 §3.2(2) — the pre-migration anchor](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [09 §3.1–§3.2 — expedited lane and PB-MIGRATION](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [06 §5.4/§6.2 — guardian review and playbook scope](../../docs/architecture/06-governance-and-guardians.md)
- [12 §1/§6.3 — descriptor release coupling and alert](../../docs/architecture/12-release-and-operations.md)
- [15 §4.7 — upgrade verification](../../docs/architecture/15-invariants-and-testing.md)
