# Futarchy System Architecture — Component Documents

**Status: authoritative.** This document set is the single source of truth for the product design and architecture of the futarchy parachain and its canonical frontend. It supersedes `BACKEND_PLAN.md` and `FRONTEND_PLAN.md` (retained at the repository root as historical references) and implements every resolution recorded in [00-decision-record.md](00-decision-record.md), which in turn disposes of all 101 findings of `DESIGN_REVIEW.md` v2.0 (2026-07-12).

## How this set is organized

The architecture is divided along **trust-domain and lifecycle boundaries** (the same cohesion rule the runtime uses for pallets), not by backend/frontend team. Cross-team facts live in exactly one place: the integration contract (02) and the parameter table (13). Every component document is normative for its own boundary, links to its dependencies, and ends with a *Resolves* table mapping design-review finding IDs to the text that fixes them.

## Reading order

Newcomers: 01 → 02 → 03 → 04 → 05, then as needed. Implementers of a single component: your doc + 02 + 13 + 15.

| Doc | Component | Owns |
|---|---|---|
| [00-decision-record.md](00-decision-record.md) | Decision record | All decisions resolving DESIGN_REVIEW.md; frozen shared constants; editorial standard |
| [01-system-overview.md](01-system-overview.md) | System overview | Goals, guarantees, ADRs, deployment topology, pallet map, rollout summary |
| [02-integration-contract.md](02-integration-contract.md) | **Integration contract (frozen)** | Runtime API, events, storage the FE binds to, chain identity, test artifacts. Change-controlled by both teams |
| [03-conditional-ledger.md](03-conditional-ledger.md) | Conditional ledger | Custody, solvency invariants, VOID, gate instruments, Baseline vaults |
| [04-markets-and-pricing.md](04-markets-and-pricing.md) | Markets & pricing | LMSR, trade path, TWAP, gate + Baseline books, test vectors |
| [05-welfare-and-decision-engine.md](05-welfare-and-decision-engine.md) | Welfare & decision engine | State machines, welfare function, decision rule, reason codes |
| [06-governance-and-guardians.md](06-governance-and-guardians.md) | Governance & guardians | Values layer, tracks, ratification, origins/filters, guardians, playbooks |
| [07-oracle-and-disputes.md](07-oracle-and-disputes.md) | Oracle & disputes | Reporting game, challenge windows, watchtowers, registries |
| [08-treasury-and-economics.md](08-treasury-and-economics.md) | Treasury & economics | Genesis, POL, NAV floors, security sizing, fees, keeper economics |
| [09-execution-upgrades-and-rollout.md](09-execution-upgrades-and-rollout.md) | Execution, upgrades & rollout | Execution guard, upgrade path, XCM, emergency lanes, phase gates |
| [10-frontend-architecture.md](10-frontend-architecture.md) | Frontend architecture | Boot, light client, data layer, verification, history model, budgets |
| [11-frontend-workflows.md](11-frontend-workflows.md) | Frontend workflows | Screens, precondition tables, governance/operator/funding surfaces |
| [12-release-and-operations.md](12-release-and-operations.md) | Release & operations | Release train, keys, ArNS, bootnodes, ops owners and funding |
| [13-parameters.md](13-parameters.md) | Parameters | The single reconciled parameter/bounds/constants table |
| [14-threat-model.md](14-threat-model.md) | Threat model | Combined adversary model for chain + frontend |
| [15-invariants-and-testing.md](15-invariants-and-testing.md) | Invariants & testing | Protocol invariants, INV-FE-1…15 verbatim, test regime, published artifacts |

## Provenance

- `BACKEND_PLAN.md` v1.0 draft and `FRONTEND_PLAN.md` — the source specifications, reorganized here with the review's repairs applied.
- `DESIGN_REVIEW.md` v2.0 — the canonical review; its finding IDs (X-n, B-n, F-n) are cited throughout.
- Everything the review verified as correct ("What the design gets right") is carried forward deliberately; `[VERIFY]` tags are retained wherever genuine uncertainty remains.
