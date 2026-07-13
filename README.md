# Bliwit

A **futarchy-governed Polkadot parachain**: token holders vote on *values* (what the
chain should optimize), while conditional prediction markets decide *beliefs* (which
proposals actually get executed). All consensus-critical logic is native Rust FRAME
pallets — no smart-contract environment in the trusted computing base. The canonical
client is a fully decentralized frontend: an Arweave-distributed static app running an
in-browser light client (smoldot), with no backend, no indexer dependency, and no
telemetry.

Core mechanics (see the architecture set for the normative detail): scalar Mode B
futarchy over a welfare score, LMSR market maker in verified 64.64 fixed point, a
purpose-built conditional ledger with machine-checked solvency invariants, a bonded
optimistic oracle with escalating disputes, an execution guard with narrow
class-specific origins (no unrestricted Root), and an eight-phase evidence-gated
rollout that removes `sudo` at Phase 4.

## Status

**Specification complete and frozen (2026-07-12) · implementation not started.**

- The authoritative spec is [`docs/architecture/`](docs/architecture/README.md) —
  16 component documents + decision record, produced by resolving all 101 findings
  of an adversarial design review. It is **immutable** for day-to-day work
  (change-controlled; see [AGENTS.md](AGENTS.md)).
- Implementation progress, milestones, and the session log live in [`PLAN.md`](PLAN.md).

## Repository map

| Path | What it is |
|---|---|
| [`docs/architecture/`](docs/architecture/README.md) | The frozen specification (00–15). Start with its README; reading order 01 → 02 → 03 → 04 → 05 |
| [`docs/design/`](docs/design/claude-design-kit/00-START-HERE.md) | Derived, non-normative design assets: `claude-design-kit/` packs the spec into ≤10 files + a ready prompt for generating frontend design prototypes with Claude Design |
| [`PLAN.md`](PLAN.md) | Implementation roadmap, milestone status, session log — the living source of "where are we" |
| [`AGENTS.md`](AGENTS.md) | Operating manual + rules for all coding agents (and useful for humans) |
| [`CLAUDE.md`](CLAUDE.md) | Claude Code wiring: skills, subagents, hooks |
| `.claude/` | Enforcement + automation: architecture write-guard hooks, session-context injection, skills, subagents, path-scoped rules |
| `.codex/` | Codex CLI session playbooks mirroring the skills |
| `crates/`, `pallets/`, `runtime/`, `node/`, `reference-model/`, `frontend/` | Planned — created milestone by milestone per PLAN.md |

## How this gets built

The project is implemented **incrementally across many agent sessions**, one PLAN.md
milestone at a time, under three standing constraints:

1. `docs/architecture/` is never modified (hook- and permission-enforced);
2. every observable behavior traces to a spec section; parameter values come only
   from doc 13 / the frozen contract in doc 02;
3. the living documents (PLAN/README/AGENTS/CLAUDE) are updated in the same session
   as any change — enforced by a Stop hook.

Humans and agents alike: read [AGENTS.md](AGENTS.md), then [PLAN.md](PLAN.md), then work.

## Toolchain (pinned)

- **Runtime:** Rust / Polkadot SDK, release line `polkadot-stable2603` (umbrella crate
  `polkadot-sdk = "2603.0.0"`), FRAME + Cumulus; Zombienet, Chopsticks, try-runtime,
  TLA⁺/Quint, cargo-fuzz, frame-benchmarking (01 §9, 15 §4).
- **Frontend:** TypeScript, polkadot-api 2.x, smoldot 3.x, Vite 8, Dexie 4; Arweave
  via permaweb-deploy/Turbo; Playwright + Lighthouse CI (01 §9, 10, 12).
- **Reference model:** Python + MPFR-256, CI-regenerated vector corpus (04 §5, 15 §4.4).

Pins must be re-verified before implementation begins — tracked as V-1 in
[PLAN.md](PLAN.md).

## License

[GPL-3.0](LICENSE)
