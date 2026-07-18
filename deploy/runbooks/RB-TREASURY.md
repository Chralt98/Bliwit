---
id: RB-TREASURY
title: Treasury NAV meter and stream anomaly response
owner_role: Monitoring coordinator
funding_line: ops.monitoring
page_immediately: false
alerts:
  - domain: Treasury
    trigger: "meter > 80%"
spec_refs:
  - docs/architecture/02-integration-contract.md
  - docs/architecture/06-governance-and-guardians.md
  - docs/architecture/08-treasury-and-economics.md
  - docs/architecture/12-release-and-operations.md
  - docs/architecture/15-invariants-and-testing.md
---

## Purpose

Investigate high treasury-meter utilization together with NAV, reserve-health, obligation, and
stream-schedule anomalies, and keep all remediation inside the metered `FutarchyTreasury` path.

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| Treasury | NAV, meter utilization, stream schedule | meter > 80% |

The trigger means at least one monitored treasury meter has crossed its early-warning threshold.
It is a capacity warning, not authority to bypass a cap; new commitments that do not fit must wait
or fail while existing valid claims remain payable under their own rules.

## Diagnosis

1. Pin `FutarchyApi::nav()` and direct `FutarchyTreasury.State` reads to the same finalized hash. Record
   `total`, `spendable_nav`, account decomposition, stream remainders, obligations,
   `meter_utilization_bps`, class floors, and `haircut_flag` ([02 §3–§4](../../docs/architecture/02-integration-contract.md)).
2. Recompute NAV from the components in [08 §1.2](../../docs/architecture/08-treasury-and-economics.md):
   liquid USDC and stream assets less open-stream, queued-outflow, and live-POL obligations. VIT
   and in-flight XCM are not positive NAV. A dashboard total that cannot reproduce is an incident.
3. Inspect `FutarchyTreasury.State.meter_30d` and `FutarchyTreasury.State.meter_180d`; the public NAV utilization is the maximum of
   those rolling outflow meters. Also inspect `FutarchyTreasury.State.issuance` and its live Param because issuance
   is a separate I-17 meter not decomposed in `NavView`
   ([treasury core](../../crates/futarchy-treasury-core/src/lib.rs)).
4. Inspect `FutarchyTreasury.State.keeper_meter` and the finalized `KeeperBudgetLow`/
   `KeeperBudgetExhausted` events. The §6.3 keeper low alarm is the keeper-meter complement to this
   treasury-wide alert and routes operational continuity to RB-KEEPER; do not infer it from
   `NavView.meter_utilization_bps`.
5. Correlate outflow failures with `MeterExhausted`, queued executions, grace, and later retries.
   Meter contention is specified to wait/retry; repeated failures must not be “fixed” by deleting
   buckets or splitting one obligation outside its declared proposal.
6. For every live stream in `FutarchyTreasury.State.streams`, validate `claimed <= total`, nonzero duration,
   recipient, line, start, cancellation state, and vested-but-unclaimed amount at the finalized
   block. Join `StreamOpened`, `StreamClaimed`, and `StreamCancelled` exactly once per transition.
7. Classify schedule anomalies: claim before vesting, claim by a non-recipient, claim beyond total,
   cancellation without a later TREASURY decision, wrong line reversion, or an event/storage
   mismatch. `StreamNotClaimable` on a zero-vested remainder is normal.
8. Check `NavHaircutFlagged` and oracle reserve-health events. A live haircut makes spendable NAV
   zero, blocks new outflows/POL/streams, and makes every minimum-viable-NAV arming gate fail while
   preserving existing books and stream claims.
9. Compare spendable NAV with the per-class floor array from `nav()`. Minimum-viable NAV is a loud
   rollout/arming condition owned by [08 §4](../../docs/architecture/08-treasury-and-economics.md);
   cite its table rather than copying a floor into alert configuration. Correlate failed arming
   with `NavFloorUnmet { class, nav, floor }`.
10. Check the last green treasury `try-state`: rolling meters must be monotone in-window, every
    collection bounded, streams internally valid, and dedicated keeper/oracle line credit no
    greater than its real-USDC pot ([15 I-7/I-17](../../docs/architecture/15-invariants-and-testing.md)).

## Remediation

### Safe / permissionless

1. Preserve finalized NAV, State, Params, events, queued payloads, and custody balances before any
   governance action. Repair an exporter mismatch from these sources; do not change chain state to
   make a dashboard green.
2. A stream recipient may call the ordinary Signed `claim_stream` only for vested value. Keepers
   may continue permissionless queue retries within grace. Neither actor may open, cancel, or
   re-line an obligation.
3. Let meters refuse new work and let reserve impairment set spendable NAV to zero. This is the
   required fail-static outcome; never reorder claimant payments or create an unbacked line credit.
4. Route keeper-meter pressure to RB-KEEPER and continue cranks unpaid after its exhaustion. Route
   reserve-health diagnosis to RB-ORACLE before proposing any new treasury commitment.

### Privileged

1. `spend`, `open_stream`, `cancel_stream`, and `fund_budget_line` require a passed TREASURY
   proposal and `FutarchyTreasury`; `claim_stream` alone is the recipient's Signed path
   ([06 §3.2](../../docs/architecture/06-governance-and-guardians.md)).
2. Replenish a legitimate depleted line only through `fund_budget_line`; dedicated KEEPER/ORACLE
   funding is custody-synced from MAIN. Do not fund a line merely to evade rolling-meter refusal.
3. Cancel an anomalous or no-longer-authorized stream only through a later TREASURY decision; the
   undisbursed remainder returns as specified. Preserve already vested claimant rights.
4. The specified PB-RESERVE path is admissible only under its deterministic reserve-health trigger
   and halts split inflows, not claimant recovery. Its downstream effect is not represented in the
   current runtime and fails closed ([runtime dispatcher](../../runtime/bleavit-runtime/src/configs.rs));
   page for a reviewed repair rather than reporting a freeze. A ledger drift instead uses the
   stricter RB-LEDGER path.

## Escalation

The Monitoring coordinator owns the initial incident. Page the Keeper coordinator for keeper-meter
pressure, the Oracle operations coordinator for reserve-health/haircut evidence, and the Release
operations lead for a NAV, meter, or stream-state implementation defect. A privileged treasury
correction must be published as a TREASURY proposal; a guardian playbook, if independently
triggered, carries mandatory retrospective ratification and postmortem review. Record the exact
meter, affected obligations, NAV/floor impact, and whether any valid claim was delayed.

## References

- [02 §3–§4 — `FutarchyApi::nav` and `NavView`](../../docs/architecture/02-integration-contract.md)
- [06 §3 — treasury and guardian origins](../../docs/architecture/06-governance-and-guardians.md)
- [08 §1 — NAV, streams, meters, calls, and reserve haircut](../../docs/architecture/08-treasury-and-economics.md)
- [08 §4 — minimum-viable NAV](../../docs/architecture/08-treasury-and-economics.md)
- [08 §6.3 — keeper-meter complement](../../docs/architecture/08-treasury-and-economics.md)
- [12 §6.3 — Treasury alert](../../docs/architecture/12-release-and-operations.md)
- [15 I-7/I-17 — metering invariants](../../docs/architecture/15-invariants-and-testing.md)
