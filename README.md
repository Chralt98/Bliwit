<p align="center">
  <img src="assets/Bleavit-logo.png" alt="Bleavit logo" width="160">
</p>

# Bleavit — A self-governing system

Futarchy was invented by Prof. Robin Hanson — thank you for your work; this
project exists to build one.

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

**Specification complete (2026-07-12) · Track M (M0–M3), Track A, B1a, B3, B4, B5, B6, B7, B9, S1, S3 and S5 implemented · B1b Epoch runtime wiring is gate-green with compliance questions still open (2026-07-17).**

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
| [`Cargo.toml`](Cargo.toml), [`rust-toolchain.toml`](rust-toolchain.toml), [`.github/workflows/ci.yml`](.github/workflows/ci.yml), [`tools/ci/rust-workspace-gates.sh`](tools/ci/rust-workspace-gates.sh), [`tools/ci/check-doc-links.py`](tools/ci/check-doc-links.py) | M0 bootstrap: Rust workspace manifest, pinned toolchain components, CI skeleton, and local gate scripts; B8 added [`tools/ci/supply-chain-gates.sh`](tools/ci/supply-chain-gates.sh) (pinned cargo-audit + lockfile gates, annotated exceptions in `.cargo/audit.toml`) and the kernel-change full-sweep workflow [`sweep.yml`](.github/workflows/sweep.yml) |
| [`tools/release/`](tools/release/README.md), [`.github/workflows/release.yml`](.github/workflows/release.yml) | B8: the tag-triggered release-artifact publication pipeline (02 §11; 15 §5) — reproducibly-built wasm + metadata + hashes, chain specs, environment archives (gated on B7 run evidence), deterministic chainHead fixtures over the frozen critical surface, the ≥10⁷-point reference-corpus sweep, all content-addressed with a readiness report; publishes as a prerelease until an operator attaches Arweave mirror evidence |
| `crates/futarchy-primitives/` | M1 shared primitive crate: `no_std` contract/view types, version constant, and kernel/chain/currency bounds |
| `crates/futarchy-fixed/` | M2 deterministic 64.64 fixed-point LMSR/transcendental crate with generated regression fixtures |
| [`reference-model/`](reference-model/pyproject.toml), [`tools/reference-model/generate-vectors.py`](tools/reference-model/generate-vectors.py) | M3 independent Python executable spec and CI-regenerated JSON vector corpus |
| [`simulation/`](simulation/README.md), [`tools/simulation/run-calibration.py`](tools/simulation/run-calibration.py) | S4 agent-based Phase-0 economic simulation (15 §4.9): executed-trade LMSR ledger with adversarial agents, committed deterministic calibration evidence; sim-gated parameter publication parked pending SQ-231 |
| `pallets/`, `crates/*-core/` | Track A (complete): each Track-A `crates/<name>-core/` is the frame-free functional core (differential oracle) and each matching `pallets/<name>/` its production `#[frame_support::pallet]` shell (origin-checked calls, bounded 02-frozen storage, try-state, benchmarks, doc-15 suites) |
| `pallets/inflow-caps/` | B4 residual state-only pallet: the shared Phase-3 cumulative per-account USDC inflow meter and total-local-issuance mint admission check (09 §5.2); no dispatchables or standalone weights, because callers include it in their transaction/weight envelopes |
| `runtime/bleavit-runtime/` | B1a + A8/A11 + B4 residual + B5 + B6: the real Cumulus parachain runtime — `construct_runtime!` over the Track-A + standard/system pallets (`Epoch` index 61, `ExecutionGuard` index 62, `InflowCaps` index 63), `BaseCallFilter = SafetyFilter`, live constitution-backed DOT/USDC XCM trader rates, the XCM-health→welfare chain (X only; R pending SQ-195), disjoint epoch-bond escrow and custody-synced treasury pots, the full 09 §2 upgrade path, and generated weights/PoV budgets. B1b remains 🔨 for SQ-172, SQ-176 and SQ-178. |
| `runtime/bleavit-xcm/` | B4 XCM layer (runtime-independent library the runtime wires): default-deny barrier (`Transact`/unpaid refused; Asset Hub/relay/Coretime origins only), pinned USDC/DOT matchers + reserve model (no teleports locally), constitution-governed weight trader, reserve-probe program + authenticated response router, coretime-renewal DOT funding leg (relay-teleport route), Phase-3 inflow-cap adapters, `pallet_xcm` call classifier |
| `runtime-api/` | B2 `futarchy-runtime-api` crate: the `sp_api::decl_runtime_apis!` declaration of the frozen 11-method `FutarchyApi` (02 §3) over the view types in `futarchy-primitives`; wiring it into the runtime's `impl_runtime_apis!` is the follow-up |
| `node/bleavit-node/` | B3: the collator node — a thin branding of the pinned `polkadot-omni-node` stack; the runtime ships in the chain spec, not in the node |
| `deploy/`, `tools/deploy/` | B3: chain-spec pipeline (pinned `staging-chain-spec-builder`), WSS bootnode operator manifests + the 02 §10 threshold validator, production genesis-allocation template, prepared ss58-7777 registry submission |
| [`keeper/`](keeper/README.md) | B9: the off-chain keeper reference implementation (`bleavit-keeper`) — a subxt-based service any operator can run to crank the chain's permissionless extrinsics (phase ticks, TWAP observations, decide, execute, settle, oracle/registry closes, cleanup), with Prometheus metrics per 12 §6.3. A separate cargo workspace so its dependency tree cannot disturb the runtime's exact pins; the on-chain rebate meter (08 §6.3) lives in the treasury pallet |
| `zombienet/`, `chopsticks/`, `tools/env/` | B7: test-environment definitions as release artifacts (15 §4.7) — multi-node topologies + the 09 §7.1 drill suite, forked-state upgrade/playbook scenarios, and the pinned-tooling fetch/generate/validate scripts (`tools/env/pins.env` is the single pin home) |
| [`models/`](models/README.md), `tools/verify/` | S1: TLA⁺ formal models of the conditional ledger and the T1–T24 proposal machine (15 §4.1) plus the pinned-TLC runner — main configs prove the invariants above anti-vacuity floors, witness configs must *violate* (reachability), mutation configs prove the invariants can fail |
| [`fuzz/`](fuzz/README.md) | S2: `bleavit-fuzz` — cargo-fuzz (libFuzzer) targets for the three 15 §4.5 areas (SCALE payload decode, nested-wrapper filtering, LMSR trade paths), each asserting invariants (I-10/I-11/I-12) rather than mere no-panic. A separate nightly-pinned cargo workspace (like `keeper/`) so libFuzzer + nightly cannot disturb the runtime's exact pins; curated seed corpora + the `fuzz` CI job (`tools/ci/fuzz-gates.sh`) |
| `frontend/` | Implementation root for Track F; currently a placeholder until the track begins |

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

- **Runtime:** Rust / Polkadot SDK, release line `polkadot-stable2606` (umbrella crate
  `polkadot-sdk = "2606.0.0"`; D-19), FRAME + Cumulus; Zombienet, Chopsticks, try-runtime,
  TLA⁺/Quint, cargo-fuzz, frame-benchmarking (01 §9, 15 §4).
- **Frontend:** TypeScript, polkadot-api 2.x, smoldot 3.x, Vite 8, Dexie 4; Arweave
  via permaweb-deploy/Turbo; Playwright + Lighthouse CI (01 §9, 10, 12).
- **Reference model:** Python high-precision reference math, CI-regenerated vector corpus, and the ≥10⁷-point release-gated differential sweep (04 §5, 15 §4.4; B8).

M0 re-verified the initial platform pins on 2026-07-13; the detailed result is tracked as V-1 in [PLAN.md](PLAN.md).

## License

[GPL-3.0](LICENSE)

---

You theorized it, we are cooking it. Bon appétit, Prof. Hanson.
