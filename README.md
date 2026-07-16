<p align="center">
  <img src="assets/Bleavit-logo.png" alt="Bleavit logo" width="160">
</p>

# Bleavit — A self-governing system

Futarchy was invented by Prof. Robin Hanson — thank you for your work; this project
exists to build one.

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

**Specification complete (2026-07-12) · foundations through M3 implemented (2026-07-13) · Track-A core relocation/shell scaffolds in progress (2026-07-14).**

- The authoritative spec is [`docs/architecture/`](docs/architecture/README.md) —
  16 component documents + decision record, produced by resolving all 101 findings
  of an adversarial design review. Treat changes to it as rare and deliberate — the
  implementation follows the spec — but it is editable, not guarded; see rule R-1 in
  [AGENTS.md](AGENTS.md).
- Implementation progress, milestones, and the session log live in [`PLAN.md`](PLAN.md).

## Repository map

| Path | What it is |
|---|---|
| [`docs/architecture/`](docs/architecture/README.md) | The specification (00–15). Start with its README; reading order 01 → 02 → 03 → 04 → 05 |
| [`docs/design/`](docs/design/claude-design-kit/00-START-HERE.md) | Derived, non-normative design assets: `claude-design-kit/` packs the spec into ≤10 files + a ready prompt for generating frontend design prototypes with Claude Design |
| [`PLAN.md`](PLAN.md) | Implementation roadmap, milestone status, session log — the living source of "where are we" |
| [`AGENTS.md`](AGENTS.md) | Operating manual + rules for all coding agents (and useful for humans) |
| [`CLAUDE.md`](CLAUDE.md) | Claude Code wiring: skills, subagents, hooks |
| `.claude/` | Automation: session-context injection, skills, subagents, path-scoped rules, and Stop-hook guards for PLAN.md freshness and README's pinned lines |
| `.codex/` | Codex CLI session playbooks mirroring the skills |
| [`Cargo.toml`](Cargo.toml), [`rust-toolchain.toml`](rust-toolchain.toml), [`.github/workflows/ci.yml`](.github/workflows/ci.yml), [`tools/ci/rust-workspace-gates.sh`](tools/ci/rust-workspace-gates.sh), [`tools/ci/check-doc-links.py`](tools/ci/check-doc-links.py) | M0 bootstrap: Rust workspace manifest, pinned toolchain components, CI skeleton, and local gate scripts |
| `crates/futarchy-primitives/` | M1 shared primitive crate: `no_std` contract/view types, version constant, and kernel/chain/currency bounds |
| `crates/futarchy-fixed/` | M2 deterministic 64.64 fixed-point LMSR/transcendental crate with generated regression fixtures |
| [`reference-model/`](reference-model/pyproject.toml), [`tools/reference-model/generate-vectors.py`](tools/reference-model/generate-vectors.py) | M3 independent Python executable spec and CI-regenerated JSON vector corpus |
| `pallets/`, `crates/*-core/` | Track A (re-scoped to production FRAME): each `crates/<name>-core/` contains the frame-free functional core and each `pallets/<name>/` currently contains a compiling shell scaffold (lib.rs · mock · tests · benchmarking · weights). The remaining Track-A work is replacing those scaffolds with real `#[frame_support::pallet]` wrappers |
| `runtime/bleavit-runtime/` | B1 runtime assembly crate: pallet composition model, SafetyFilter BaseCallFilter adapter, USDC/fee/origin/genesis filtering wiring — a frame-free model, not yet `construct_runtime!`/`impl_runtime_apis!` (that is B1a) |
| `runtime-api/` | B2 `futarchy-runtime-api` crate: the `sp_api::decl_runtime_apis!` declaration of the frozen 11-method `FutarchyApi` (02 §3) over the view types in `futarchy-primitives`; the runtime implements it in B1a |
| `node/`, `frontend/` | Implementation roots created for future milestones; currently placeholders until their tracks begin |

## How this gets built

The project is implemented **incrementally across many agent sessions**, one PLAN.md
milestone at a time, under three standing constraints:

1. every observable behavior traces to a spec section; the spec is the source of
   truth and changes to it are rare and deliberate (rule R-1);
2. parameter values come only from doc 13 / the contract surface in doc 02;
3. the living documents (PLAN/README/AGENTS/CLAUDE) are updated in the same session
   as any change — enforced by a Stop hook.

Humans and agents alike: read [AGENTS.md](AGENTS.md), then [PLAN.md](PLAN.md), then work.

## Toolchain (pinned)

- **Runtime:** Rust / Polkadot SDK, release line `polkadot-stable2603` (umbrella crate
  `polkadot-sdk = "2603.0.0"`), FRAME + Cumulus; Zombienet, Chopsticks, try-runtime,
  TLA⁺/Quint, cargo-fuzz, frame-benchmarking (01 §9, 15 §4).
- **Frontend:** TypeScript, polkadot-api 2.x, smoldot 3.x, Vite 8, Dexie 4; Arweave
  via permaweb-deploy/Turbo; Playwright + Lighthouse CI (01 §9, 10, 12).
- **Reference model:** Python high-precision reference math, CI-regenerated vector corpus (MPFR-256 release target) (04 §5, 15 §4.4).

M0 re-verified the initial platform pins on 2026-07-13; the detailed result is tracked as V-1 in [PLAN.md](PLAN.md).

## License

[GPL-3.0](LICENSE)

---

You theorized it, we are cooking it. Bon appétit, Prof. Hanson.
