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
runtime. Ordinary XCM outcomes never improve a decision or settlement input;
I-24's sole upward exception is a timely, exact, authenticated success for the
protocol-owned reserve probe `R`.

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
4. For probe-readiness or runway incidents, calculate one full local debit from
   the live `ops.probe_fee` and `ops.probe_rate` records, then compare the
   `ops.reserve_probe` balance against `res.fail_thr + res.recover_thr` such
   debits. Independently query Bleavit's Asset Hub sovereign account: USDC must
   cover `res.probe_amount`, and DOT must cover the same number of full
   `ops.probe_fee` envelopes plus the operations refill margin. Never infer a
   remote balance from the local accounting line.
5. Treat an error, timeout, or absent probe as failure. A failing R sets the daily
   C breach flag fail-static; never infer reserve health from ordinary traffic or
   from the absence of a user complaint ([07 §8](../../docs/architecture/07-oracle-and-disputes.md),
   [15 I-24](../../docs/architecture/15-invariants-and-testing.md)).
6. Classify the trap key: local signed origin, protocol origin, or remote origin.
   `claim_assets` requires the claim origin to equal the location under which the
   trap was keyed; changing the beneficiary does not change ownership.
7. Check relay and Asset Hub HRMP channel state, `XcmpQueue`/`MessageQueue`
   backlog and suspension state, message delivery events, fee purchase, asset
   identity, and version negotiation. The channel-establishment procedure remains
   a spec `[VERIFY]`; do not improvise force calls to create or reopen a channel.
8. Compare the canonical USDC and DOT locations against the frozen chain identity
   in [02](../../docs/architecture/02-integration-contract.md). Unknown assets,
   `Transact`, unpaid execution, and non-allowlisted origins are supposed to fail.
9. Confirm the assembled runtime exposes the live `claim_assets` routes before
   submitting: the base-call classifier admits the call, and
   `LocalOriginToLocation` maps a Signed caller to its own `AccountId32` location
   or `FutarchyTreasury` to the canonical protocol-custody location. A successful
   dispatch still proves only the exact keyed claim; it is not evidence that a
   remote-origin trap was locally recoverable.

## Remediation

### Safe / permissionless

1. For a trap keyed to the caller's local signed origin, submit self-scoped
   `PolkadotXcm.claim_assets` from that exact account after verifying the finalized
   origin, versioned asset set and trap hash. The runtime converts the signer only
   to its own `AccountId32` location; it cannot reclaim a protocol- or remote-keyed
   trap, and changing the beneficiary cannot widen the claim origin.
2. Verify the finalized `AssetsClaimed`/balance effect and that the trap count fell
   before retrying any transfer. A failed claim leaves the trap in place; record
   the exact module/XCM error rather than broadening the origin.
3. For a remote-origin trap, coordinate an inbound `ClaimAsset` program from that
   same remote origin. A local treasury call cannot impersonate Asset Hub or a
   remote account.
4. Continue permissionless reserve probes and collect channel evidence. Do not
   retry an idempotent protocol transfer until its prior state and idempotency key
   have been proved terminal; ordinary user transfers follow standard XCM error
   semantics.

### Privileged

1. While `BootstrapOpsFundingClosed` is false, the stored operations multisig may
   top up only `OpsReserveProbe`, and only so the resulting balance is at or below
   the live `res.fail_thr + res.recover_thr` runway ceiling. Never use that key to
   fund another line or treat the ceiling as discretionary allocation authority.
   TREASURY arming does not close this bridge. The first successful positive
   `FutarchyTreasury` funding of `OpsReserveProbe` closes it atomically; verify the
   resulting line balance and `BootstrapOpsFundingClosed = true` before Phase-4
   sudo removal. Zero/failed funding or another line is not a handover.
2. For a trap keyed to the canonical protocol-custody location, prepare the exact
   `PolkadotXcm.claim_assets` leaf as a TREASURY-class proposal. The runtime maps
   only `FutarchyTreasury` to that location; the proposal must pass the singleton
   XCM-recovery resource lock and zero-outflow Treasury-ask screening before the
   execution guard dispatches it. Do not use this route for user- or remote-keyed
   traps: origin conversion will refuse or leave the unmatched trap unchanged.
3. When `Oracle::ReserveHealth` proves reserve impairment, page guardians for
   `PB-RESERVE`. That playbook halts split inflows only and preserves merge,
   redemption, and exit behavior; cite [06 §6.2](../../docs/architecture/06-governance-and-guardians.md)
   and [07 §8](../../docs/architecture/07-oracle-and-disputes.md) rather than
   restating its parameters.
4. Verify the machine trigger, successful `GuardianAction`/`PlaybookActivated`,
   and reserve-health phase flag before announcing containment. The current
   runtime reports unavailable downstream guardian playbook effects as an error;
   an approval without the effect is not a halt.
5. Never use `pallet_xcm.send`, `force_*`, `Transact`, storage mutation, or a
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
recorded as blocked on an exact unavailable authority path. A lost or compromised
operations key has no direct Signed or Root rotation: if necessary, use the
NAV-gated bootstrap path to arm TREASURY, then complete a positive TREASURY-class
reserve-line funding and rotate through the existing TREASURY origin. Before
persistent launch, correct genesis/redeploy if that path is unavailable; never
widen the key's authority.

## References

- [09 §6.1–§6.4 — XCM rules, claims, channels, X and R](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [02 §8 — frozen chain and asset identity](../../docs/architecture/02-integration-contract.md)
- [07 §8 — reserve probe and PB-RESERVE trigger](../../docs/architecture/07-oracle-and-disputes.md)
- [06 §5.4/§6.2 — playbook authority and review](../../docs/architecture/06-governance-and-guardians.md)
- [12 §6.3 — XCM alert row](../../docs/architecture/12-release-and-operations.md)
- [15 I-24 — fail-static XCM invariant](../../docs/architecture/15-invariants-and-testing.md)
