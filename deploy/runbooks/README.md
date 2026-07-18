# Bleavit operational runbooks

Runbooks-as-code means the on-call response procedures named by
[12 §6.3](../../docs/architecture/12-release-and-operations.md#63-monitoring-and-alerting)
live in this reviewed, versioned directory beside the deployment artifacts. They
are derived operational guidance: architecture documents remain normative, and a
conflict is resolved in favor of the specification.

## Index

| ID | Title | owner_role | page_immediately |
|---|---|---|---|
| RB-KEEPER | Keeper fleet and decision-critical crank response | Keeper coordinator | false |
| RB-INTAKE | Proposal intake saturation and containment | Monitoring coordinator | false |
| RB-MARKET | Market solvency-bound and numerical anomaly response | Monitoring coordinator | false |
| RB-POL | Protocol-owned liquidity floor disturbance | Monitoring coordinator | false |
| RB-ORACLE | Deep oracle dispute response | Oracle operations coordinator | false |
| RB-LEDGER | Ledger collateralization breach response | Monitoring coordinator | true |
| RB-TREASURY | Treasury NAV meter and stream anomaly response | Monitoring coordinator | false |
| RB-XCM | Trapped XCM asset response | Monitoring coordinator | false |
| RB-GUARDIAN | Guardian action accountability and review | Monitoring coordinator | false |
| RB-UPGRADE | Stalled migration recovery | Release operations lead | false |
| RB-STORAGE | Storage bound pressure | Monitoring coordinator | false |
| RB-BOOTNODE | Bootnode and served-state availability | Bootnode program coordinator | false |
| RB-RELEASE | Release integrity and channel response | Release operations lead | true |

## Frozen frontmatter

Every `RB-*.md` file uses this YAML subset. Values are single-line plain or
double-quoted scalars (quotes are stripped; escape sequences are not decoded);
indentation is exactly two spaces for list entries; flow syntax, tabs, YAML
comments/tags/anchors/aliases, multiline scalars, and unknown keys are invalid.
`page_immediately` must be the literal unquoted token `true` or `false`.

```yaml
---
id: RB-KEEPER
title: Short one-line title
owner_role: Keeper coordinator
funding_line: ops.keepers
page_immediately: false
alerts:
  - domain: Epoch progress
    trigger: tick lag > 600 blocks
spec_refs:
  - docs/architecture/12-release-and-operations.md
---
```

The `alerts` list repeats every §6.3 row bound to the ID. Comparisons are exact
after cell-level Markdown decoration is stripped and Markdown table escapes are
decoded. Each body then contains, in order, `Purpose`, `Alerts`, `Diagnosis`,
`Remediation`, `Escalation`, and `References` level-two sections.

## Ownership and funding

| Runbook | owner_role | funding_line |
|---|---|---|
| RB-KEEPER | Keeper coordinator | ops.keepers |
| RB-INTAKE, RB-MARKET, RB-POL, RB-TREASURY, RB-LEDGER, RB-XCM, RB-GUARDIAN, RB-STORAGE | Monitoring coordinator | ops.monitoring |
| RB-ORACLE | Oracle operations coordinator | ops.oracle_evidence |
| RB-UPGRADE | Release operations lead | ops.arweave / ops.monitoring |
| RB-BOOTNODE | Bootnode program coordinator | ops.bootnodes |
| RB-RELEASE | Release operations lead | ops.arweave / ops.monitoring |

§6.1's Monitoring row owns the §6.3 stack, so protocol-domain alerts default
to the monitoring coordinator as first responder, with domain escalation in each
runbook's Escalation section; runbooks with a dedicated §6.1 program row bind to
that row's owner.

## Structural gate

[The O4 checker](../../tools/deploy/check-runbooks.py) treats
[12 §6.3](../../docs/architecture/12-release-and-operations.md#63-monitoring-and-alerting)
as the single source of the alert-to-runbook binding and §6.1 as the source of
valid owner/funding row pairs. It requires exactly two §6.3 alert tables and the
§6.4 incident-response heading; checks each alert row has one runbook owner and
one frontmatter occurrence; compares frontmatter and body `Alerts` tables (Domain,
Key series, Trigger) after the normalization stated above; enforces paging markers,
strict scalar/frontmatter syntax, body section order, case-sensitive filenames,
local files and Markdown heading fragments; pins doc 12 and this index to the
frozen 13-ID O4 set; and binds the index bidirectionally to every runbook's title,
owner role, and paging flag.

Run it from the repository root:

```sh
python3 tools/deploy/check-runbooks.py
```

CI runs the same command in the `docs` job (`Documentation links`) under the
`Check runbooks` step.
