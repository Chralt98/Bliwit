---
id: RB-ORACLE
title: Deep oracle dispute response
owner_role: Oracle operations coordinator
funding_line: ops.oracle_evidence
page_immediately: false
alerts:
  - domain: Oracle
    trigger: round 3 opened
spec_refs:
  - docs/architecture/06-governance-and-guardians.md
  - docs/architecture/07-oracle-and-disputes.md
  - docs/architecture/12-release-and-operations.md
  - docs/architecture/13-parameters.md
  - docs/architecture/15-invariants-and-testing.md
---

## Purpose

This runbook covers an oracle dispute that has reached round 3. At that depth the
ordinary reporting game is near its terminal `OracleResolution` handoff; operators
preserve evidence and observability while the protocol continues to default to a
challenge-closed or neutral result, never an unverified forward settlement
([07 §4–§11](../../docs/architecture/07-oracle-and-disputes.md)).

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| Oracle | report timeliness, open disputes, round depth | round 3 opened |

The trigger means a live `Oracle::Rounds` entry has escalated to the deepest
ordinary dispute round. It is not itself a verdict, proof of reporter fault, or
permission to replace the reporting game with an operator judgment
([12 §6.3](../../docs/architecture/12-release-and-operations.md),
[07 §5](../../docs/architecture/07-oracle-and-disputes.md)).

## Diagnosis

1. Acknowledge the alert and record a finalized block hash, runtime
   `spec_version`, `(component, epoch, spec_version)` round key, report hash, both
   evidence hashes, and the `RoundEscalated` event. Work from finalized state.
2. Read `Oracle::Rounds` on an archive node. Confirm the stored round is 3 and
   reconstruct the event sequence from `Reported`, `Challenged`, and
   `RoundEscalated`; distinguish a genuine escalation from a stale monitor sample.
3. Check report timeliness against the frozen MetricSpec and the schedule in
   [07 §5/§11](../../docs/architecture/07-oracle-and-disputes.md). Also inspect the
   money deadline: a late terminal verdict can settle bonds and reputation but
   cannot reopen money that already took the neutral path (I-18).
4. Inspect `Oracle::AckRecords` together with `WindowAcknowledged`,
   `WindowExtended`, and `QuorumFailed`. Confirm the [07 §4](../../docs/architecture/07-oracle-and-disputes.md)
   2-of-N "observed" acknowledgment state and whether the lifecycle's single
   extension has already been consumed; take its duration from
   [13](../../docs/architecture/13-parameters.md), not an operator constant.
5. Fetch each content-addressed artifact named by the round from Arweave and the
   archive-node mirror funded by `ops.oracle_evidence`. Hash the bytes, verify the
   frozen `formula_ref` and MetricSpec version, and record any unavailable or
   mismatched object. Retrievability is a validity condition, not a convenience
   ([07 §5.1/§9](../../docs/architecture/07-oracle-and-disputes.md)).
6. On an independent archive node, recompute from the frozen snapshot block and
   committed raw data. Check `Oracle::Recomputable` before treating a mechanical
   proof as admissible; do not manufacture a proof format for a component absent
   from that set.
7. Inspect `AdjudicationRequested`. If present, capture its referendum index and
   verify the proposal dispatches the shipped
   `oracle.adjudicate(component, epoch, spec_version, value, reporter_wrong)` on
   the `OracleResolution` track. The first three arguments identify the round;
   the §5.4 verdict is decomposed into the settled `value` and the
   `reporter_wrong` stake-discipline decision. The spec's
   `oracle.adjudicate(round_id, verdict)` shorthand diverges from that shipped
   signature and is logged as SQ-236
   ([07 §5.4](../../docs/architecture/07-oracle-and-disputes.md)).

## Remediation

### Safe / permissionless

1. Pin missing evidence to its content-addressed destination and mirror the exact
   bytes on the funded archive nodes. Preserve the original object and hashes;
   never silently replace evidence under a new interpretation
   ([07 §9](../../docs/architecture/07-oracle-and-disputes.md),
   [12 §6.1](../../docs/architecture/12-release-and-operations.md)).
2. Page every registered watchtower that has not acknowledged observability.
   Acknowledgment asserts only finalized visibility and a reachable challenge
   surface; it must not attest that either value is true ([07 §4](../../docs/architecture/07-oracle-and-disputes.md)).
3. If the frozen spec permits deterministic recomputation, support any keeper in
   submitting `oracle.recompute_proof`. Reproduce the proof independently before
   announcing it, and retain the archive-node inputs with the incident record.
4. Keep permissionless round-close and deadline cranks available. If no
   challenge-closed value exists by the money deadline, allow the specified
   neutral settlement; never inject a guessed value to avoid the status-quo path.

### Privileged

1. When round 3 cannot resolve mechanically, follow the existing
   `AdjudicationRequested` handoff into `OracleResolution`; neither the operations
   coordinator nor guardians may adjudicate the value directly
   ([07 §5.4](../../docs/architecture/07-oracle-and-disputes.md)).
2. The `OracleResolution` enactment may call only the shipped
   `oracle.adjudicate(component, epoch, spec_version, value, reporter_wrong)` with
   the committed round-key triple and the verdict's settled value and
   reporter-fault boolean; retain SQ-236 on the §5.4 shorthand divergence.
   Re-check the snapshot electorate and referendum status before enactment; a
   result arriving after the money deadline remains bond-and-reputation-only
   ([07 §5.4–§5.6](../../docs/architecture/07-oracle-and-disputes.md)).
3. Use `PB-ORACLE-VOID` only for the separate verified oracle-deadlock or
   gate-input-failure trigger defined by [06 §6.2](../../docs/architecture/06-governance-and-guardians.md).
   Round depth alone does not authorize it.

## Escalation

Page the Monitoring coordinator for alert-path or archive-observability failures,
the Infrastructure coordinator for archive-node recomputation support, and the
Oracle operations coordinator for evidence hosting and watchtower coordination
([12 §6.1](../../docs/architecture/12-release-and-operations.md)). Escalate the
substantive terminal verdict only through `OracleResolution`. If evidence was
unavailable, watchtower quorum failed, or the adjudication path missed the money
deadline, publish those facts with the post-incident record; do not describe a
neutral settlement as a successful oracle verdict.

## References

- [07 §4–§6 — watchtowers and the reporting game](../../docs/architecture/07-oracle-and-disputes.md)
- [07 §9–§11 — evidence, neutral settlement, and latency](../../docs/architecture/07-oracle-and-disputes.md)
- [06 §2.3/§6.2 — OracleResolution and oracle playbook scope](../../docs/architecture/06-governance-and-guardians.md)
- [12 §6.1/§6.3 — owned evidence hosting and alert row](../../docs/architecture/12-release-and-operations.md)
- [13 — oracle and watchtower parameter value home](../../docs/architecture/13-parameters.md)
- [15 I-18 — challenge-closed money settlement](../../docs/architecture/15-invariants-and-testing.md)
