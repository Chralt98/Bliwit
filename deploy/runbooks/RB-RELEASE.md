---
id: RB-RELEASE
title: Release integrity and channel response
owner_role: Release operations lead
funding_line: ops.arweave / ops.monitoring
page_immediately: true
alerts:
  - domain: Release integrity
    trigger: any byte mismatch; 2-of-3 resolver divergence
  - domain: Descriptor lead time
    trigger: no covering release at 50% of DescriptorLeadTime
  - domain: ReleaseChannel
    trigger: update missing 600 blocks after repoint; any SECURITY flip
spec_refs:
  - docs/architecture/02-integration-contract.md
  - docs/architecture/09-execution-upgrades-and-rollout.md
  - docs/architecture/12-release-and-operations.md
  - docs/architecture/15-invariants-and-testing.md
---

## Purpose

This runbook covers distribution integrity, descriptor lead-time consumption, and
the metadata-independent `ReleaseChannel`. Any served-byte mismatch or resolver
divergence pages immediately and is treated as a hostile release until proven
otherwise; the other legs protect upgrade compatibility and stranded users
([12 §1–§6.4](../../docs/architecture/12-release-and-operations.md)).

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| Release integrity | out-of-band bundle-vs-manifest comparison (§5.2), ArNS resolution consistency across gateways, ANT record history | any byte mismatch; 2-of-3 resolver divergence |
| Descriptor lead time | `UpgradeAuthorized` age vs covering-release liveness | no covering release at 50% of `DescriptorLeadTime` |
| ReleaseChannel | staleness vs latest repoint, flag changes | update missing 600 blocks after repoint; any `SECURITY` flip |

The integrity row indicates potentially hostile distribution, not an ordinary CDN
fault. The descriptor row means the release train has consumed half of the
on-chain application window without live compatible descriptors. The channel row
means pinned/stranded clients may have stale or security-critical recovery data.

## Diagnosis

1. For an integrity alert, page immediately. Record the finalized block hash,
   raw `ReleaseChannel` bytes, ANT record/history, resolved TXID from every probe,
   gateway response headers, and served bytes. Use a headless, cacheless fetcher;
   never trust a service worker in the incident path ([12 §5.2](../../docs/architecture/12-release-and-operations.md)).
2. Resolve and fetch by both canonical name and immutable TXID. Byte-compare every
   file with the signed `release.json` map, verify minisign signatures and
   independent attestations against the current keyring generation and on-chain
   revocation bits, and cross-check the manifest TXID with `ReleaseChannel`.
3. Determine whether divergence is gateway-local, name-resolution divergence, an
   ANT record change, a signed-manifest mismatch, or altered bytes. Until all
   comparisons agree, keep the hostile-release classification.
4. Inspect the shipped backend artifact evidence without confusing it for the
   frontend release: `release-manifest.json`, `readiness-report.md`,
   `build-info.json`, `runtime-info.json`, `fixtures-report.json`, environment
   `run-evidence.json`, `supply-chain-summary.json`, and the content-addressed
   `dist/` inventory produced by [tools/release](../../tools/release/README.md).
   Any strict-assembly readiness gap blocks publication.
5. Note tooling reality: the repository ships backend artifact assembly and
   verification inputs, but the normative frontend `release.json`,
   `tools/verify-release`, `repoint.sh`, signing registry, and key-bearing operator
   process are later release milestones. Never substitute `release-manifest.json`
   for the signed frontend manifest or claim an unshipped command was run.
6. For descriptor lead time, locate `UpgradeAuthorized`,
   `ExecutionGuard::PendingUpgrade`, its committed artifact/metadata hashes,
   `applicable_at`, and `ReleaseChannel.pending_authorized_at`. Verify the live
   production release covers the target `spec_version` and includes both the
   parachain and required Asset Hub descriptor sets.
7. For `ReleaseChannel`, compare `version`, `manifest_txid`,
   `release_json_hash`, `updated_at`, `spec_version`, pending authorization,
   minimum supported version, keyring generation, revocation bits, and flags with
   the latest repoint and approved release record. Decode from the frozen raw key,
   not current metadata ([02 §12](../../docs/architecture/02-integration-contract.md)).
8. Audit signer disjointness and ceremony records if a key or ANT action is
   implicated. CI has neither release signing keys nor controller shares; a CI
   artifact alone cannot prove an authorized repoint.

## Remediation

### Safe / permissionless

1. Preserve all divergent bytes and resolution proofs. Publish the incident on
   the static status page and community channels without linking users through a
   suspect canonical origin. Provide the last-good immutable TXID.
2. Re-run independent build and clean-container verification. Keep signing and
   repoint operations separate from diagnosis; no single operator should hold
   both release and ANT authority ([12 §2.2](../../docs/architecture/12-release-and-operations.md)).
3. For the descriptor alert, escalate the release train immediately. Build from
   the queue-time artifact commitment, run descriptor drift and reproducibility
   gates, and choose the standard lane unless the delta satisfies the mechanically
   checked descriptor-only lane in [12 §1.5](../../docs/architecture/12-release-and-operations.md).
4. For a missing channel update without evidence of compromise, prepare the exact
   `ReleaseChannel` record from the approved release and verify its frozen layout,
   target version, hashes, and flags before the authorized write. Never clear a
   `SECURITY` flag merely to silence the alert.

The four incident paths below contain privileged release/channel actions. They
require the quorum and authority defined in [12 §1–§4](../../docs/architecture/12-release-and-operations.md);
CI and monitoring operators do not sign or repoint.

### Hostile release

1. Use the ANT controller quorum to repoint the canonical name to the last-good
   immutable TXID; the normative quorum is in [12 §4.2](../../docs/architecture/12-release-and-operations.md).
2. Update `ReleaseChannel` with the `SECURITY` flag and a
   `min_supported_version` bump so pinned and stranded clients receive the chain
   warning ([12 §3](../../docs/architecture/12-release-and-operations.md)).
3. If a release signing key is implicated, revoke its index on-chain, advance the
   keyring generation as specified, and re-sign or roll back the production
   release through the applicable release lane ([12 §2.3](../../docs/architecture/12-release-and-operations.md)).
4. Publish the status-page and community announcement, including clean immutable
   recovery links, then publish a content-addressed postmortem TX. Do not declare
   recovery until the out-of-band monitor sees the last-good bytes and consistent
   resolution.

### Wrong-chain-spec

1. Stop repointing the affected candidate and publish a patch release through the
   lane its actual delta requires. Re-run genesis/chain identity checks,
   descriptor generation, independent attestations, and the standard release
   gates ([12 §1](../../docs/architecture/12-release-and-operations.md)).
2. The bundle's pinned chain-spec and genesis identities make cross-environment
   confusion non-weaponizable: the client must fail closed rather than connect to
   the wrong chain. Do not weaken those pins to make the bad release boot.

### ArNS-key loss

1. Do not treat the name as expiring: `futarchy` is permabuy, so there is no lease
   lapse. Keep serving and announcing immutable per-release TXIDs
   ([12 §4.1](../../docs/architecture/12-release-and-operations.md)).
2. If the controller quorum can be restored, run the reviewed FROST/ceremony
   re-issuance path and re-attest signer/controller disjointness.
3. If fewer than 3 shares are recoverable, establish a new name and announce it
   through `ReleaseChannel`. The chain channel—not the lost ArNS name—is the
   recovery root ([12 §6.4](../../docs/architecture/12-release-and-operations.md)).

### Distribution mismatch

1. Treat the mismatch as a hostile release until proven otherwise. Preserve both
   byte trees, gateway responses, ANT history, signed manifest, and monitor
   transcript; do not downgrade based on a single healthy gateway.
2. If clean independent fetches prove a gateway-local fault and all signed bytes,
   name resolution, and channel state agree, remove or quarantine that gateway
   through the reviewed release configuration. Otherwise execute the Hostile
   release path in full ([12 §5.2/§6.4](../../docs/architecture/12-release-and-operations.md)).

## Escalation

Page the Release operations lead and Monitoring coordinator for all three alert
legs; page ANT controllers and release signers only through their disjoint
ceremony channels. Descriptor coverage escalates to the release train at the first
alert and blocks application if no verified covering release is live before the
on-chain gate. Any `SECURITY` flip is public incident state: coordinate the status
page, community announcement, key review, immutable postmortem, and follow-up
release. A hostile-release or ArNS recovery that needs `ReleaseChannel` uses its
`ConstitutionalValues` authority and applicable accountability trail, never sudo
or a raw storage write.

## References

- [12 §1 — release train, descriptor lane, and rollback](../../docs/architecture/12-release-and-operations.md)
- [02 §12 — frozen ReleaseChannel raw layout](../../docs/architecture/02-integration-contract.md)
- [12 §2–§4 — keys, ReleaseChannel, ArNS/ANT](../../docs/architecture/12-release-and-operations.md)
- [12 §5.2/§6.3–§6.4 — out-of-band monitor, alerts, incidents](../../docs/architecture/12-release-and-operations.md)
- [09 §2.1–§2.3 — upgrade authorization and lead-time coupling](../../docs/architecture/09-execution-upgrades-and-rollout.md)
- [15 §4.8 — frontend release verification gates](../../docs/architecture/15-invariants-and-testing.md)
- [Backend release artifact tooling](../../tools/release/README.md)
