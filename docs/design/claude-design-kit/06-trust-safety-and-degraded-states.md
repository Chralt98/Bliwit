# Trust, safety & degraded-state UX obligations

> **DERIVED, NON-NORMATIVE.** Distilled 2026-07-12 (commit `9f250be`) from the frozen spec —
> doc 00 (decision record), doc 14 (threat model), doc 15 (invariants & testing) — for upload
> to Claude Design. Where this file and the spec disagree, the spec wins. IDs (D-n, TH-n, I-n,
> INV-FE-n, E-n) are canonical and must be kept verbatim in any design annotations.

These are **binding UX constraints**, not suggestions: doc 15's INV-FE invariants are frozen
texts, and doc 14's accepted residual risks "MUST be carried into user-facing honesty surfaces".

## 1. The fifteen frontend invariants (doc 15 §2) — design consequences

Canonical provenance labels (exact spellings, INV-FE-9): `verified-finalized`, `verified-best`,
`derived-local`, `provider`, `stale-cache`. Canonical fail-safe modes (INV-FE-12): `restricted`,
`read-only-incompatible`.

| ID | Rule | What the design must show |
|---|---|---|
| INV-FE-1 | The in-browser light client is the only authoritative read path; RPC/indexer/cache data is never promoted to verified | Transaction surfaces render exclusively verified-finalized values |
| INV-FE-2 | Every precondition is re-read at one finalized block immediately before signing; failure blocks with a diff | A pre-sign "refresh & gate" step; an expected-vs-actual diff view on failure |
| INV-FE-3 | Provider data never satisfies a precondition, never renders "passed/settled/mature/final/safe" without a chain read | Outcome badges only from chain reads |
| INV-FE-4 | Every protocol workflow works with no indexer, no RPC, no provider, cleared storage | No screen may depend on third-party data to function |
| INV-FE-5 | The app never holds/derives/stores private keys; signing only in the user's wallet | No in-app key UX; design wallet round-trips (extension + raw-payload/QR) |
| INV-FE-6 | Serverless static bundle; server-requiring features are out of scope, not centralized | No login, no accounts, no server-backed features |
| INV-FE-7 | Local-storage loss is a performance event only | Cache loss must never look like an error needing rescue |
| INV-FE-8 | Single-operator tampering is detected and **surfaced, never silently repaired** | Divergence warning states (red banner, `FE-REL-002`) |
| INV-FE-9 | Every displayed value carries a typed verification status; no unlabeled rendering path | The provenance badge system is a core design-system component (certified partly by visual-regression tests) |
| INV-FE-10 | Reproducible releases: users can verify which app they run, commit → served bytes | Feeds the verification panel |
| INV-FE-11 | Pinned identity (release TXID, commit, hashes, genesis) displayed in an **always-available verification panel**; genesis mismatch is terminal | Permanent verification panel; a designed terminal "wrong chain" screen |
| INV-FE-12 | Unknown runtimes ⇒ explicit `restricted` / `read-only-incompatible` modes; undecodable data renders as raw SCALE with a warning; the app never guesses | Mode banners; a designed raw-SCALE fallback rendering |
| INV-FE-13 | No telemetry, no analytics, no remote config; settings live on-device | Nothing that implies tracking; privacy honesty copy |
| INV-FE-14 | Everything signed is inspectable: human summary + decoded tree + raw SCALE bytes/hash, **all derived from the bytes to be signed, never form state**; expert mode exposes raw storage keys | Three-level payload review in the confirm screen; expert mode |
| INV-FE-15 | Acceleration providers are optional, labelled "to the pixel", auto-disabled on mismatch; history gaps are first-class and visible, never spliced | Origin labels on every provider row; gap rendering in charts/tables |

Backend invariants with direct UI consequences (doc 15 §1): **I-25** — no protocol workflow may
depend on any off-chain service beyond chain P2P + static hosting. **I-26/I-27** — under a
`Voided` vault, exactly `merge`, `merge_scalar`, gate-set merge, `transfer`, `redeem_void` are
allowed; the VOID screen may offer only these. **I-2(c)** — the honest annulment statement
(unpaired holder recovers 0.5 — "not a loss of principal on any protocol path") is the copy
baseline for VOID messaging.

## 2. Threat-model rows that are UI obligations (doc 14)

**Fake frontends & phishing.** TH-38: self-check mismatch ⇒ red banner, signing disabled
pending expert acknowledgment. TH-39/TH-46: in-app release-provenance panel, canonical-name
education, verification panel (TXID + genesis), identicon habits. TH-45: the UI must NOT claim
to detect a hostile service worker (unsound) — recovery guidance instead. TH-56: exact payload
bytes + hash shown; the wallet is the independent second display channel. TH-55: chain-sourced
strings render as text only, never markup.

**Light-client states.** TH-47: peer isolation = liveness failure only — show stale-but-real
state *as old, never as fresh* (finalized-head age > 60 s ⇒ `FE-CHAIN-003`). TH-48: RPC
fallback off by default, quarantined, persistent "UNVERIFIED RPC MODE" banner, normal-mode
signing disabled. TH-51: STALE badges + timestamps mandatory pre-sync. TH-52/53: restricted /
read-only-incompatible modes; signing disabled globally when incompatible. TH-54: transaction
outcome is decoded from finalized events — truthful outcome reporting, never assumed success.

**Untrusted data honesty.** TH-49 (accepted residual): provider-fed charts can influence
trading judgment — mitigation is mandatory, non-suppressible provenance labelling + structural
firewall, never a verification claim. TH-50: snapshots are content-hash-pinned and sampled;
the chain-served 32-cohort layer is the always-verified core. TH-14: when the USDC
reserve-health flag is set, the NAV renders with a haircut and banner — never full backing.
TH-29: Phases 0–3 show the persistent sudo banner; Phase 3 is honestly "a capped-exposure
trust phase".

**Privacy.** TH-60/61: no telemetry ever; per-provider disclosure ("this operator sees the
addresses you look up") and per-use consent for address-history queries.

**Accepted residuals as copy obligations (14 §4).** Twelve accepted residual risks must appear
in user-facing honesty surfaces where applicable — e.g. deep-history forgery limits ("sampling
does not detect a self-consistent forgery at depth"), perfect-clone phishing, reserve-freeze
and depeg risk borne by holders.

## 3. Degraded modes to design (every one has a specified state)

- **Boot machine** (doc 10 §3): `ShellLoaded → StorageOpen → WorkerSpawn → ChainStarting →
  RelaySyncing → ParaSyncing → IdentityCheck → CompatCheck → Ready`; plus `MemoryOnly`
  (IndexedDB failed — non-terminal), `WorkerFailed`, `WasmFailed`, `SyncDegraded` (no peers),
  `ReadyRestricted`, `ReadOnlyIncompatible`, and terminal `WrongChain`. First verified render
  can take 30–90 s (desktop) / 90–240 s (mobile) — sync progress is a designed experience.
- **Session flags** (combinable): `Degraded` (peer/finality health) × `MemoryOnly` × `RpcOnly`,
  on top of compat mode `full`/`restricted`/`read-only-incompatible`. Every combination renders.
- **Protocol freezes**: PB-LEDGER-FREEZE (guardian playbook freezes all ledger+market calls,
  ≤ 14 days) — whole-app frozen-trading state; PB-RESERVE (split inflows halted + NAV haircut).
  Disputes hold *settlement, never decisions*. Thin markets refuse to decide
  (`NotDecisionGrade`) — "refuse to decide rather than decide badly" is a designed outcome, not an error.
- **Chain halted**: block production stalls; dead-man switch freezes execution; recovery epoch
  has no propose phase.
- **Local-data failure**: IndexedDB corruption auto-rebuilds (`FE-IDX-001`); retention
  auto-tunes to storage budget — degrades chart depth/resolution, never correctness.
- **The degradation matrix**: rows E1–E14 (doc 15 §3.3): first visit desktop/mobile,
  returning visit with gaps, after runtime upgrade, deep link to old proposal, active market,
  ten-month chart, address history, IndexedDB corruption, providers down, gateway down, slow
  peer discovery, signing while chain advances, obsolete release. Doc 11 §11.12 (the owner of
  the matrix) adds E15–E23: referendum voting, VOID redemption, AH deposit, AH withdraw,
  upgrade crank, guardian approval, sudo era, evidence unretrievable, ratification-deadline
  risk. Each row specifies: Visible state · Loading · Available verified data · Unavailable
  convenience data · Failure message · Recovery.
  ⚠ Numbering caution: doc 15 §3.3 numbers its five added scenarios E15–E19 with DIFFERENT
  referents than doc 11's E15–E23 (e.g. VOID redemption is E15 in doc 15 but E16 in doc 11).
  **Follow doc 11's numbering** (it owns the matrix); the scenario *content* of the two lists
  is equivalent. (Logged as an open spec question in PLAN.md.)

## 4. Design principles stated by the spec itself (doc 15 §3.4)

- **DB-1**: advanced historical analytics degrade gracefully rather than preventing protocol
  use — degradation is labeled, explained in place, recoverable; never silent, never structural.
- **DB-2**: "A 'decentralized' app nobody can load is centralization by another name" —
  load-time and resource budgets are release gates (initial JS ≤ 350 KB gz).
- **DB-5**: "Convenience is never load-bearing" — every convenience path is quarantined,
  labeled, removable.
- **UX-D7**: long-horizon history must be *achievable through some labeled path* — it may
  degrade, but must not be structurally impossible for everyone forever.
