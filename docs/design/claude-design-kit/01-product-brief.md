# Bleavit — Product brief for frontend design

> **DERIVED, NON-NORMATIVE.** Synthesized 2026-07-12 (commit `9f250be`) and reviewed
> through the reserve-probe/I-24 amendments of 2026-07-22 against the frozen specification
> in `docs/architecture/` (mainly 00, 01, 10, 11) for upload to Claude Design.
> Where this brief and the spec disagree, the spec wins. Citations like "(01 §4)" point into
> the architecture set.

## 1. What Bleavit is

Bleavit is a **futarchy-governed blockchain**: a Polkadot parachain whose decisions are made
by prediction markets instead of token voting. The slogan-level idea (R. Hanson): *vote on
values, bet on beliefs.*

- A **values layer** (token voting with conviction locks, six narrow referenda tracks) decides
  only what "good" means: welfare-metric definitions, constitutional amendments, guardian
  elections, and ratification of high-risk outcomes. It can never enact operational changes. (01 §1)
- A **beliefs layer** of conditional prediction markets decides everything operational. For each
  proposal, traders bet on the chain's future **welfare score `s ∈ [0,1]`** in two hypothetical
  worlds — "world where the proposal is ACCEPTED" vs. "world where it is REJECTED" — plus an
  unconditional per-epoch **Baseline market**. If the market prices the ACCEPT-world welfare
  clearly above the REJECT-world (and every safety gate passes), the proposal executes
  automatically. (01 §1, 04, 05)
- Proposals come in **five classes**: PARAM (parameter change), TREASURY (spending), CODE
  (runtime upgrade), META (changes to the mechanism itself), CONSTITUTIONAL (values-side).
  There is deliberately no "Emergency" class — emergencies are handled by an elected, bonded
  7-seat **guardian council** executing pre-committed playbooks. (01 §1, 06)
- Everything runs on a recurring, pipelined **21-day epoch machine**: proposals are submitted,
  screened, traded for ~13 days, decided by a gate-first decision rule, executed, then measured
  against reality for weeks, and finally settled — with several epoch cohorts in flight at once. (05)

Money: **USDC** (a USDC-backed stable asset, 6 decimals) is the sole market collateral, bond
currency and settlement unit. **VIT** (native token, 12 decimals) is for values voting,
guardian/collator bonds, and fees (fees payable in either currency). Blocks every 6 seconds. (01 §4, 02)

## 2. What this frontend is

The **canonical client is itself decentralized** (10, 11, 12):

- A static React SPA distributed on **Arweave** (permanent storage), named via ArNS,
  hash-verified. No backend, no server, no telemetry, no required RPC or indexer.
- It talks to the chain through an **in-browser light client** (smoldot in a Web Worker) that
  verifies finality proofs itself. The app *proves* what it shows rather than trusting a server.
- Every displayed value carries a **provenance status** — this is the design's defining
  material. The five statuses (10 §2.1): `verified-finalized` (light-client proven),
  `verified-best` (proven but not yet final; display-only), `derived-local` (computed from the
  user's own local index, with coverage/holes), `provider` (untrusted third-party data,
  permanently labelled, never promoted), `stale-cache` (old local data shown before sync
  completes). UI components literally cannot render a value without a status.
- **Transaction safety is a visible ritual**: before any signature, the app re-checks every
  precondition against a freshly finalized block and shows expected-vs-actual; the confirm
  screen is decoded from the exact bytes being signed, never from form state. (11 §11.3–§11.4)
- The app must be honest when degraded: light-client sync states, peer loss, incompatible
  runtime, missing history ("holes" render as visible gaps, never interpolated), unverified
  RPC fallback mode with a persistent warning banner. (10 §3, §6; 11 §11.12)

## 3. Who uses it (all served by ONE app — 11 §11.2)

| Persona | What they do | Main surfaces |
|---|---|---|
| **Observer / citizen** | Watches governance: epoch clock, proposals, welfare dashboard, settlements | S1, S2, S7, S8 |
| **Trader** | Bets on ACCEPT/REJECT welfare outcomes; splits USDC into conditional positions, trades LMSR books, redeems after settlement | S3, S4, S20 |
| **Proposer** | Submits a proposal batch with bond + preimage; tracks it through the lifecycle | S5, S2 |
| **VIT voter / delegate** | Votes on six referenda tracks with conviction locks; delegates; ratifies CODE/META outcomes; votes terminal oracle disputes | S9–S11 |
| **Funder** | Moves USDC from Polkadot Asset Hub in/out of the chain (guided two-leg XCM flow) | S12, S13, S20 |
| **Oracle reporter / watchtower** | Bonded professionals: report metric values, challenge lies, escalate disputes, file evidence | S14, S19 |
| **Guardian** (7 elected) | 5-of-7 approval console for emergency playbooks; every action retro-ratified | S15 |
| **Keeper / operator** | Permissionless cranks: ticks, decision finalization, `execute()`, snapshots, the runtime-upgrade crank | S6, S17, S18 |
| **Treasury recipient** | Claims vested streams; watches NAV and outflow meters | S16 |

Operator surfaces (S14–S19) live under an explicit **"Advanced"** area: same trust rules,
denser information, no simplified summaries (11 §11.2).

## 4. Product principles the design must embody

1. **Provenance is the brand.** The verified/provider/stale distinction is not a footnote — it
   is the core visual language, non-suppressible, readable at a glance and by screen readers. (10 §2)
2. **The chain is the only authority.** No number is ever invented, extrapolated or smoothed.
   Charts show holes. Countdowns tick in blocks and human time. "The runtime is the final
   arbiter" — the UI predicts nothing it cannot re-derive. (11 §11.1)
3. **Status-quo default is a feature.** Rejection, timeout and dispute all resolve to "nothing
   happens". Copy and visuals treat a rejected proposal as the system working, not failing. (01 §2.1 G-1)
4. **Honest degradation.** Every failure state has specified visuals, copy and recovery
   (E-rows E1–E23). Loading is dominated by light-client sync — a first verified render can
   take ~30–90 s; the design must make that wait legible and trustworthy, not hide it. (10 §9.4, 11 §11.12)
5. **No dark patterns, mandated honesty copy.** Examples the spec literally requires: VOID
   redemption must lead with the 100%-recovery merge path and may not describe the 0.5/0.25
   fallback rates as a penalty (11 §11.6); the NAV screen must never show full backing while the
   reserve-health flag is set (11 §11.8.3); a non-dismissable "Bootstrap governance: sudo
   active" banner on every route during early phases (11 §11.10).
6. **Dense, professional, self-serve.** Users range from citizens to professional reporters and
   guardians. The Advanced area is intentionally dense. Everything is explorable without any
   account, server or API key — expert mode exposes raw storage keys and SCALE bytes. (11 §11.4)

## 5. Ambient facts that shape look & feel

- Time is physical: 6-second blocks; 21-day epochs; phase windows (trading ≈ days 5–18);
  72-hour challenge windows; 72-hour upgrade descriptor lead time. Countdowns, phase dials and
  deadline pressure are everywhere. (05, 13)
- Prices are probabilities-of-welfare: LMSR books quote `s ∈ [0,1]` per branch; the decision
  compares time-weighted averages (TWAP), not spot. Spot vs. TWAP is a first-class display pair. (04)
- Positions are *conditional*: "1,000 USDC in the ACCEPT branch of proposal #42" — the UI must
  make branch-world thinking effortless (mirror-branch holdings, complete pairs, merge/redeem). (03)
- Scale bounds (max sustained): ≤ 64 intake-family records, ≤ 32 post-qualification
  non-terminal proposals, ≤ 196 active/POL-live books, ≤ 2,240 retained book rows,
  64 positions per account, 21-day cycles settling for weeks — a busy but bounded system;
  no infinite feeds. (13, 02)
- Addresses render as checksummed SS58 (prefix 7777) with identicons. USDC has 6 decimals,
  VIT 12. (02, 11 §11.3)

## 6. Current status

Specification is complete and frozen; implementation has not started. This design prototype is
a **precursor artifact for Track F** (frontend) of PLAN.md — it explores what the specified
product should look like. It must therefore *conform to* the spec's UX obligations, but visual
style, layout, typography and identity are open design territory.
