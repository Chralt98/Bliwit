---
id: RB-BOOTNODE
title: Bootnode and served-state availability
owner_role: Bootnode program coordinator
funding_line: ops.bootnodes
page_immediately: false
alerts:
  - domain: Bootnodes
    trigger: < 8 dialable, or < 2 on :443, or cert < 14 d
  - domain: Served-state window
    trigger: window < 30 d on the joint fleet
spec_refs:
  - docs/architecture/12-release-and-operations.md
  - docs/architecture/01-system-overview.md
  - docs/architecture/02-integration-contract.md
  - docs/architecture/13-parameters.md
  - deploy/chain-specs/README.md
---

## Purpose

Use this runbook when the browser-reachable bootnode set or the jointly served
state window falls below the operational commitments owned by the bootnode
program in
[12 §6](../../docs/architecture/12-release-and-operations.md#6-operational-layer-x-13-x-4-d-6-d-16).
The service is operational infrastructure, not a protocol guarantee.

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| Bootnodes | per-endpoint browser-dial success, WSS cert expiry, operator count | < 8 dialable, or < 2 on :443, or cert < 14 d |
| Served-state window | per-operator retention depth | window < 30 d on the joint fleet |

The dial, operator, port-policy, and retention floors have their normative values
in [13 §3.5](../../docs/architecture/13-parameters.md#35-chain-identity-and-supply-owned-by-02-values-frozen-d-17).
The `cert < 14 d` TLS renewal margin belongs to the
[12 §6.3 Bootnodes alert row](../../docs/architecture/12-release-and-operations.md#63-monitoring-and-alerting),
not §13. A native peer dial is not a passing browser-context probe.

The Served-state window trigger means the operator fleet cannot jointly provide
the retention depth promised by
[12 §6.2](../../docs/architecture/12-release-and-operations.md#62-bootnode-program).
Frontend data obtained through that layer remains provider-labeled.

## Diagnosis

1. Identify the affected network and compare the released chain spec's
   `bootNodes` array with the matching operator manifest under
   [`deploy/chain-specs/`](../chain-specs/README.md). Confirm that each endpoint
   is attributed to one organization and to the expected peer identity.
2. Run the existing chain-spec validator against the exact candidate or released
   artifact with its `paseo` or `polkadot` profile. It checks the frozen browser-WSS,
   operator-diversity, distinct-peer, and port-policy floors from
   [02 §10](../../docs/architecture/02-integration-contract.md#10-wss-bootnode-chain-spec-requirement-d-6-x-4).
3. Inspect the §6.3 per-endpoint browser-dial probe result and certificate-expiry
   observation. The repository does not yet define a Prometheus series name for
   this probe; do not substitute a native node-health check or invent a metric.
4. Separate an endpoint failure from an operator failure: compare TLS validity,
   WSS listener reachability, DNS resolution, peer identity, and the set of
   independent organizations represented by the still-passing endpoints.
5. For a served-state alert, inspect each funded operator's retained state and
   block-body evidence, then calculate the joint fleet window. The repository
   specifies the commitment and alert surface but no canonical metric name.
6. Check relay connectivity and finalized-head progress before attributing a
   fleet-wide dial failure to endpoint configuration. Node roles and the lack of
   any canonical-frontend RPC dependency are defined in
   [01 §4.2](../../docs/architecture/01-system-overview.md#42-node-roles-and-off-chain-services).

## Remediation

### Safe / permissionless

1. Re-run the browser-context dial from an independent network and preserve the
   endpoint, certificate, and finalized-head evidence. A passing native dial does
   not clear the alert under [12 §6.2](../../docs/architecture/12-release-and-operations.md#62-bootnode-program).
2. Ask the affected operator to restore its existing WSS listener, certificate,
   DNS record, peer process, or retention configuration without changing the
   published peer identity. Recheck the whole set after recovery because the
   commitments apply to the fleet, not one machine.
3. Keep the alert open while either documented condition remains true. Do not
   relabel RPC fallback or native-only connectivity as browser-reachable service.

### Privileged / release-controlled

1. If an endpoint or peer must be replaced, the bootnode program coordinator
   prepares the operator-manifest and chain-spec update. Set changes ride the
   release process plus on-chain discovery under
   [12 §6.2](../../docs/architecture/12-release-and-operations.md#62-bootnode-program);
   they are not an ad hoc monitoring edit.
2. Validate the finished artifact with
   [`tools/deploy/validate-chain-spec.py`](../../tools/deploy/validate-chain-spec.py)
   before release. A candidate that fails a floor stays unreleased; preserving
   the incumbent spec is the status-quo default.
3. Do not advance a rollout phase while its bootnode or served-state evidence is
   below the entry requirement. Phase evidence and values ratification follow
   [12 §6.5](../../docs/architecture/12-release-and-operations.md#65-phase-gate-wiring-09).

## Escalation

Page the Monitoring coordinator for probe integrity and the Bootnode program
coordinator for endpoint/operator recovery. Page the Infrastructure coordinator
when the served-state commitment or archive/RPC capacity is involved. Escalate a
required chain-spec replacement to the Release operations lead.

No guardian playbook is activated by bootnode or retention loss alone. If a
separate verified on-chain trigger appears, follow that trigger's domain runbook;
do not infer emergency authority from infrastructure unavailability. Record
phase-gate evidence and the incident postmortem through the release-operations
process in [12 §6](../../docs/architecture/12-release-and-operations.md#6-operational-layer-x-13-x-4-d-6-d-16).

## References

- [12 §6.1–§6.5 — owned programs, alerts, and rollout gates](../../docs/architecture/12-release-and-operations.md#6-operational-layer-x-13-x-4-d-6-d-16)
- [01 §4.2 — node roles and off-chain services](../../docs/architecture/01-system-overview.md#42-node-roles-and-off-chain-services)
- [02 §10 — WSS bootnode chain-spec requirement](../../docs/architecture/02-integration-contract.md#10-wss-bootnode-chain-spec-requirement-d-6-x-4)
- [13 §3.5 — normative bootnode and served-state floors](../../docs/architecture/13-parameters.md#35-chain-identity-and-supply-owned-by-02-values-frozen-d-17)
- [Chain-spec pipeline and operator manifests](../chain-specs/README.md)
