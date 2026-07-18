---
id: RB-XCM
title: Trapped XCM asset response
owner_role: Monitoring coordinator
funding_line: ops.monitoring
page_immediately: false
alerts:
  - domain: XCM
    trigger: any trap
spec_refs:
  - docs/architecture/02-integration-contract.md
  - docs/architecture/06-governance-and-guardians.md
  - docs/architecture/07-oracle-and-disputes.md
  - docs/architecture/09-execution-upgrades-and-rollout.md
  - docs/architecture/12-release-and-operations.md
  - docs/architecture/15-invariants-and-testing.md
---

## Purpose

This runbook covers any XCM asset trap. Operators identify the trap's origin and
ownership, distinguish transport health from reserve health, and use only the
recovery paths that are both ownership-safe and dispatchable in the shipped
runtime. No XCM outcome may improve a decision or settlement input (I-24).

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| XCM | send/fail/timeout counters, trapped assets | any trap |

The trigger means `PolkadotXcm::AssetsTrapped` or the corresponding trapped-asset
state exists, even if balances appear small or a later send succeeds. Every trap
requires an ownership-safe disposition and an auditable recovery record
([09 §6.1](../../docs/architecture/09-execution-upgrades-and-rollout.md)).

## Diagnosis

1. Record a finalized block hash, message hash/topic, trap hash, asset set,
   beneficiary, XCM origin, destination, and the complete `AssetsTrapped` event.
   Preserve the source and destination execution traces when available.
2. Inspect `Welfare::XcmTraffic(epoch, day)` for its real `Accepted`,
   `SendFailed`, and `ProbeTimeout` counters. `HealthTrackingRouter` records only
   local validation/delivery; an accepted send is not evidence of remote
   execution success ([09 §6.4](../../docs/architecture/09-execution-upgrades-and-rollout.md)).
3. Inspect `Oracle::ReserveHealth` and the `ReserveProbeSent`,
   `ReserveProbeResult`, `ReserveUnhealthy`, and `ReserveRecovered` events. Confirm
   whether `XcmProbeDispatcher` sent the Asset Hub program and whether
   `ProbeAwareResponseHandler` authenticated a matching response.
4. Treat an error, timeout, or absent probe as failure. A failing R sets the daily
   C breach flag fail-static; never infer reserve health from ordinary traffic or
   from the absence of a user complaint ([07 §8](../../docs/architecture/07-oracle-and-disputes.md),
   [15 I-24](../../docs/architecture/15-invariants-and-testing.md)).
5. Classify the trap key: local signed origin, protocol origin, or remote origin.
   `claim_assets` requires the claim origin to equal the location under which the
   trap was keyed; changing the beneficiary does not change ownership.
6. Check relay and Asset Hub HRMP channel state, `XcmpQueue`/`MessageQueue`
   backlog and suspension state, message delivery events, fee purchase, asset
   identity, and version negotiation. The channel-establishment procedure remains
   a spec `[VERIFY]`; do not improvise force calls to create or reopen a channel.
7. Compare the canonical USDC and DOT locations against the frozen chain identity
   in [02](../../docs/architecture/02-integration-contract.md). Unknown assets,
   `Transact`, unpaid execution, and non-allowlisted origins are supposed to fail.
8. Deployment-readiness check: the XCM library provides
   `HealthTrackingRouter`, `XcmProbeDispatcher`, `ProbeAwareResponseHandler`, and
   the call classifier, but the assembled runtime's production asset transactor
   and canonical user exit are currently fail-closed. Record an unavailable
   recovery route honestly; do not claim a transfer succeeded because a library
   component exists.

## Remediation

### Safe / permissionless

1. For a trap keyed to the caller's local signed origin, the ownership-valid route
   specified by [09 §6.1](../../docs/architecture/09-execution-upgrades-and-rollout.md)
   remains self-scoped `PolkadotXcm.claim_assets`; verify the origin, versioned
   asset set, and trap hash against finalized state. Do not submit it in the
   shipped runtime: the base-call classifier treats every `claim_assets` call as
   TREASURY-domain and the empty `ExecuteXcmOrigin` converter also cannot convert
   Signed, so the call fails closed. Record the self-keyed trap under SQ-235.
2. After SQ-235 is wired, use only that Signed, self-scoped route for locally keyed
   user traps. It cannot reclaim a protocol- or remote-keyed trap, and changing the
   beneficiary cannot widen the claim origin.
3. For a remote-origin trap, coordinate an inbound `ClaimAsset` program from that
   same remote origin. A local treasury call cannot impersonate Asset Hub or a
   remote account.
4. Continue permissionless reserve probes and collect channel evidence. Do not
   retry an idempotent protocol transfer until its prior state and idempotency key
   have been proved terminal; ordinary user transfers follow standard XCM error
   semantics.

### Privileged

1. Do not attempt any `claim_assets` recovery in the shipped runtime. The
   TREASURY-class `claim_assets` path is mandated by
   [09 §6.1](../../docs/architecture/09-execution-upgrades-and-rollout.md) and
   admitted by the runtime classifier, but `pallet_xcm::ExecuteXcmOrigin` is wired
   with the empty origin converter, so a `FutarchyTreasury`-origin dispatch fails
   closed with `BadOrigin`. Record the trap and exact claim inputs against the
   logged SQ-235 wiring gap. The Signed self-scoped route above remains the
   spec-mandated route only for self-keyed traps, but is likewise unavailable
   until the classifier and origin converter are wired consistently.
2. When `Oracle::ReserveHealth` proves reserve impairment, page guardians for
   `PB-RESERVE`. That playbook halts split inflows only and preserves merge,
   redemption, and exit behavior; cite [06 §6.2](../../docs/architecture/06-governance-and-guardians.md)
   and [07 §8](../../docs/architecture/07-oracle-and-disputes.md) rather than
   restating its parameters.
3. Verify the machine trigger, successful `GuardianAction`/`PlaybookActivated`,
   and reserve-health phase flag before announcing containment. The current
   runtime reports unavailable downstream guardian playbook effects as an error;
   an approval without the effect is not a halt.
4. Never use `pallet_xcm.send`, `force_*`, `Transact`, storage mutation, or a
   superuser-origin conversion as recovery. Those surfaces are default-denied by
   [09 §6.1](../../docs/architecture/09-execution-upgrades-and-rollout.md).

## Escalation

Page the Infrastructure coordinator for relay/Asset Hub/HRMP or queue failures,
the Oracle operations coordinator when R probe authenticity or timeout folding is
in doubt, and the treasury domain when a protocol-keyed claim is required. Page
guardians only for a verified reserve-health trigger and `PB-RESERVE`. Any
activation follows the retrospective ratification obligations in
[06 §5.4](../../docs/architecture/06-governance-and-guardians.md). Keep the
incident open until every trapped asset has been recovered, returned remotely, or
recorded as blocked on an exact unavailable authority path.

## References

- [09 §6.1–§6.4 — XCM rules, claims, channels, X and R](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [02 §8 — frozen chain and asset identity](../../docs/architecture/02-integration-contract.md)
- [07 §8 — reserve probe and PB-RESERVE trigger](../../docs/architecture/07-oracle-and-disputes.md)
- [06 §5.4/§6.2 — playbook authority and review](../../docs/architecture/06-governance-and-guardians.md)
- [12 §6.3 — XCM alert row](../../docs/architecture/12-release-and-operations.md)
- [15 I-24 — fail-static XCM invariant](../../docs/architecture/15-invariants-and-testing.md)
