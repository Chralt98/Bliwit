---
id: RB-INTAKE
title: Proposal intake saturation and containment
owner_role: Monitoring coordinator
funding_line: ops.monitoring
page_immediately: false
alerts:
  - domain: Proposal state
    trigger: "queue at bound"
spec_refs:
  - docs/architecture/05-welfare-and-decision-engine.md
  - docs/architecture/06-governance-and-guardians.md
  - docs/architecture/09-execution-upgrades-and-rollout.md
  - docs/architecture/12-release-and-operations.md
  - docs/architecture/13-parameters.md
---

## Purpose

Distinguish ordinary bounded-intake backpressure from a stuck epoch or abusive saturation, tell
proposers what is safe to do, and route verified emergency containment through PB-HALT-INTAKE
without disturbing live trading windows.

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| Proposal state | per-state counts, queue depth | queue at bound |

The trigger means the pre-qualification `IntakeQueue` has reached its bound. New submissions are
refused before admission and their bond is not accepted; existing submitted proposals are neither
dropped nor silently displaced ([13 §4](../../docs/architecture/13-parameters.md)).

## Diagnosis

1. Pin all reads to one finalized hash. Read `Epoch.EpochOf` and `FutarchyApi::epoch_status()` to
   confirm the chain is in Intake and that the phase clock is advancing.
2. Read `Epoch.IntakeQueue` and, for operator diagnosis, `Epoch.IntakeProposals`. Verify queue ids
   are unique and correspond to `Submitted` proposals. `IntakeQueue` covers only the
   pre-qualification stage; do not combine it with the separately bounded Screening-to-Settled
   `Epoch.Proposals` population ([02 §7.1](../../docs/architecture/02-integration-contract.md)).
3. Correlate `ProposalSubmitted` and `ProposalWithdrawn` events. A submission rejected with
   `IntakeFull` is expected backpressure at the bound, not loss of a bond or an accepted proposal.
4. Check whether Qualify has begun and whether `epoch.tick` is progressing bounded screening
   batches. A queue still reported as “at bound” after the phase boundary can be stale monitoring
   or RB-KEEPER lag rather than continuing intake pressure.
5. Compare proposer concentration and current-epoch admissions with the per-account intake limit
   and bond lifecycle from [06 §4](../../docs/architecture/06-governance-and-guardians.md). Do not
   label high demand as an attack solely because the queue is full; record repeated missing
   preimages, constitution violations, and `IntakeSlashed` evidence separately.
6. Check dead-man and recovery state. The shipped epoch core refuses new intake while dead-man is
   armed/paused or recovery is in progress and preserves pre-pause submitted work for a recovery
   epoch; that is containment, not queue corruption
   ([epoch core](../../crates/epoch-core/src/lib.rs)).
7. If emergency containment is proposed, verify the PB-HALT-INTAKE trigger is live. Its complete
   registered contents live in [06 §6.2](../../docs/architecture/06-governance-and-guardians.md):
   the playbook may call only `epoch.set_intake_paused(true, expiry)` and affects intake, not open
   trading.
8. Shipped limitation: `epoch.set_intake_paused` is not represented by the current epoch pallet,
   and the runtime deliberately returns an error for pause/playbook downstream effects
   ([runtime dispatcher](../../runtime/bleavit-runtime/src/configs.rs)). Treat PB-HALT-INTAKE as a
   specified but currently unavailable containment surface; never report it active from a failed
   approval.

## Remediation

### Safe / permissionless

1. Publish backpressure status to proposers: an unadmitted submission should wait for a later
   Intake phase. Do not recommend fee escalation or repeated retries; neither can exceed the
   bounded queue.
2. A proposer with an admitted `Submitted` proposal may use `epoch.withdraw` before Qualify for
   the specified refund path. Operators must not withdraw on a proposer's behalf
   ([05 T2](../../docs/architecture/05-welfare-and-decision-engine.md)).
3. Restore `epoch.tick` through RB-KEEPER if the phase has moved to Qualify and screening is not
   advancing. Keeper calls are bounded and permissionless; do not mutate queue storage directly.
4. Preserve preimages and finalized submission evidence. Missing-preimage and non-decision-grade
   paths have specified bond consequences, so “clearing” the queue by removing preimages makes
   the incident worse.

### Privileged

1. PB-HALT-INTAKE is admissible only through the trigger-gated `EmergencyPlaybook` path and its
   pre-ratified call batch; `GuardianHold`/`EmergencyPlaybook` cannot be forged by Signed or Root
   callers ([06 §3.1–§3.2](../../docs/architecture/06-governance-and-guardians.md)).
2. Activate it only for a live gate-breach, dead-man, or VOID-in-flight trigger. Its subtractive
   scope must leave already-open trading windows and their safe finalization paths alone.
3. Because the shipped downstream effect is unavailable, escalation requires a normal reviewed
   runtime repair/release. Do not substitute `set_storage`, sudo, or an unenumerated market/ledger
   freeze; the filter forbids those authority expansions.

## Escalation

Page the Monitoring coordinator, then the Keeper coordinator if qualification work is stalled.
For a live PB-HALT-INTAKE trigger, page the guardian signers named in the ops handbook and the
Release operations lead because the shipped containment call is absent. The rollout phase table
names PB-HALT-INTAKE as the Phase-4-and-later retreat/containment path
([09 §7.1](../../docs/architecture/09-execution-upgrades-and-rollout.md)); its contents remain
owned by 06 §6.2. Every activation must enter mandatory retrospective ratification, and any
unavailable/failed activation belongs in the incident postmortem.

## References

- [02 §7.1 — epoch storage surfaces](../../docs/architecture/02-integration-contract.md)
- [05 §2–§3 — proposal transitions and phase schedule](../../docs/architecture/05-welfare-and-decision-engine.md)
- [06 §3–§6 — intake economics, guardian authority, and PB-HALT-INTAKE](../../docs/architecture/06-governance-and-guardians.md)
- [09 §7.1 — rollout containment table](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [12 §6.3 — proposal-state alert](../../docs/architecture/12-release-and-operations.md)
- [13 §4 — reconciled intake and live-proposal bounds](../../docs/architecture/13-parameters.md)

