---
id: RB-GUARDIAN
title: Guardian action accountability and review
owner_role: Monitoring coordinator
funding_line: ops.monitoring
page_immediately: false
alerts:
  - domain: Guardian
    trigger: "any action"
spec_refs:
  - docs/architecture/02-integration-contract.md
  - docs/architecture/06-governance-and-guardians.md
  - docs/architecture/09-execution-upgrades-and-rollout.md
  - docs/architecture/12-release-and-operations.md
  - docs/architecture/13-parameters.md
  - docs/architecture/15-invariants-and-testing.md
---

## Purpose

Review every dispatched guardian action immediately, verify its enumerated scope and allowance,
and drive the mandatory retrospective ratification or the specified slash-and-recall consequence.
The page is accountability by design; it does not imply the action was invalid.

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| Guardian | actions, allowance consumption, pending reviews | any action |

Every finalized `GuardianAction` pages because each action exercises exceptional, bounded power,
consumes an allowance, and opens a retrospective review obligation. Do not deduplicate distinct
action ids or suppress a page because the same playbook was used before.

## Diagnosis

1. Pin the finalized `GuardianAction { action_id, power, target, justification_hash }` and its
   block/extrinsic. Verify it is the frozen 02 §6 event, not merely `ActionProposed` or
   `ActionApproved` ([02 §6](../../docs/architecture/02-integration-contract.md)).
2. Join the action with `Guardian.Members`, `PendingActions`, `Approvals`, and the fifth approval.
   Verify the approvers were current members, distinct, and that the dispatched power/target match
   the proposal and justification hash ([06 §5.1](../../docs/architecture/06-governance-and-guardians.md)).
3. Match the power to the exhaustive table in [06 §5.2](../../docs/architecture/06-governance-and-guardians.md).
   Check proposal state for delay/rerun, verified trigger and registered call scope for a playbook,
   and the corresponding `ForceRerun` or `PlaybookActivated` event where applicable.
4. Read `Guardian.Allowances`, `RerunUsed`, `ActivePlaybooks`, and current phase. Confirm the action
   consumed the correct allowance and that rollout sunset has not removed that power. Values are
   read from chain/metadata and [13](../../docs/architecture/13-parameters.md), never copied into
   the runbook.
5. Confirm `ReviewScheduled { action, referendum }`, `Guardian.ReviewDeadlines`,
   `Guardian.ReviewReferenda`, and the stock `Referenda.ReferendumInfoFor` record all name the same
   action. Missing scheduling after a finalized action is an accountability incident.
6. Compute pending-review latency from the action epoch and the live review deadline Param. Page
   on missing referendum, stalled referendum, a deadline near expiry, or a deadline/state join
   that cannot be reproduced. Do not wait for the deadline alarm to begin review.
7. Follow terminal review evidence. A pass executes `guardian.ratify_action` via
   `ConstitutionalValues`, emits `ActionRatified`, and refunds the fronted submission deposit. A
   missed deadline emits `ReviewFailed { action, slashed_each }` and must slash every approving
   member according to [06 §5.4](../../docs/architecture/06-governance-and-guardians.md).
8. Verify bond/accountability state after failure and look for `RecallScheduled`. The shipped
   `MemberBonds` is a seat-indexed arithmetic ledger rather than a fungible hold, so the mandated
   economic slash is not production-complete and a departed approver can evade this implementation
   path ([guardian core](../../crates/guardian-core/src/lib.rs)). Preserve the protocol rule as the
   required outcome and escalate the implementation gap.
9. The shipped pallet persists its arithmetic slash and `ReviewFailed` even when recall scheduling
   fails; the current runtime recall scheduler returns an error because recall bond fronting is not
   wired ([runtime scheduler](../../runtime/bleavit-runtime/src/configs.rs)). Absence of
   `RecallScheduled` is therefore a known but still spec-noncompliant escalation, not a reason to
   roll back the recorded slash.
10. Check shipped effect availability. Delay and force-rerun effects are wired; pause-intake,
   playbook, and gate-suspend downstream effects currently fail closed and must not emit a
   successful `GuardianAction` in production. An event claiming one of those effects succeeded
   demands immediate code/state investigation.
11. Preserve the justification preimage/content, trigger proof, exact allowance before/after,
    referendum timeline, approver set, and affected protocol objects for the mandatory review and
    postmortem.

## Remediation

### Safe / permissionless

1. Publish the action packet and keep finalized event/storage monitoring live. Anyone may inspect
   or participate in the ordinary referenda/voting surface, but no Signed call may forge
   `GuardianHold`, `EmergencyPlaybook`, or `ConstitutionalValues`.
2. Keep unrelated keeper and exit paths running. Do not broaden a scoped pause, rerun, or
   playbook in the name of incident response; subtractive scope and expiry are safety properties.
3. If a review join is missing or the shipped effect failed, preserve fail-closed state and open a
   normal code/release incident. Do not use `set_storage`, sudo, or a second guardian action to
   manufacture ratification evidence.
4. Let the bounded maintenance hook enforce overdue review consequences. Never delete or rewrite
   `ReviewDeadlines` to avoid slashing.

### Privileged

1. Retrospective approval must pass the `ratify` track and dispatch
   `guardian.ratify_action(action_id)` with `ConstitutionalValues`. Guardians cannot ratify their
   own action, extend their scope, move funds, install code, or alter market outcomes
   ([06 §3 and §5](../../docs/architecture/06-governance-and-guardians.md)).
2. If review does not pass by the live deadline, accept the specified approver slash and schedule
   recall on the `guardian` track. Do not compensate approvers from treasury or suppress the
   failure because the original action appeared useful.
3. Playbook renewal is limited to the enumerated values path for the playbook that permits it.
   Any longer repair uses the normal proposal/release path; guardian authority cannot be extended
   ad hoc.

## Escalation

The Monitoring coordinator owns the page and immediately contacts the named guardian signers and
values-governance facilitators. Page the Release operations lead for code/migration actions,
missing review or recall machinery, or an event/effect mismatch; the Keeper coordinator for an
action affecting epoch progress; and the Oracle operations coordinator for an oracle/reserve
trigger ([12 §6.1](../../docs/architecture/12-release-and-operations.md)). Use the specifically
activated guardian playbook runbook when one exists. Every action requires retrospective review;
every unratified action requires the specified slash and recall response, plus a published
postmortem covering trigger validity, scope, allowance, effect, and review latency.

## References

- [02 §6–§7 — frozen guardian events and readable storage](../../docs/architecture/02-integration-contract.md)
- [06 §3 — origin and authority matrix](../../docs/architecture/06-governance-and-guardians.md)
- [06 §5 — guardian powers, allowances, review, slash, and recall](../../docs/architecture/06-governance-and-guardians.md)
- [06 §6 — emergency playbook accountability](../../docs/architecture/06-governance-and-guardians.md)
- [09 §3.2 — migration guardian review example](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [12 §6.3 — Guardian alert](../../docs/architecture/12-release-and-operations.md)
- [13 §1 — live guardian review parameter](../../docs/architecture/13-parameters.md)
- [15 I-23 — enumerated scope and review invariant](../../docs/architecture/15-invariants-and-testing.md)
