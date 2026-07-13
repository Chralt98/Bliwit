# Claude Design prompt

> **How to use:** In Claude Design (claude.ai/design), attach the seven kit files
> (`00-START-HERE.md` … `06-trust-safety-and-degraded-states.md`), then paste everything
> below the horizontal rule as your prompt. Iterate in the same conversation so the design
> system stays consistent (follow-up prompts are suggested at the bottom of this file).

---

Design the canonical web application for **Bliwit** — a futarchy-governed blockchain where
prediction markets, not token votes, make the operational decisions. This is financial
governance infrastructure with real money and adversarial users, and its specification
mandates radical honesty in the interface. The product is fully specified; the visual
identity does not exist yet. You are creating it.

## Your sources — attached, in reading order

1. `00-START-HERE.md` — how to read this pack and the ground rules.
2. `01-product-brief.md` — what Bliwit is, the nine personas, product principles.
3. `02-domain-model-and-lifecycles.md` — every entity, state machine and payout rule.
4. `03-frontend-architecture-VERBATIM.md` — frozen spec: the provenance/trust model, boot
   state machine, degraded modes, history layers.
5. `04-frontend-workflows-and-screens-VERBATIM.md` — frozen spec: the screen inventory
   S1–S20, signing-safety rules, precondition tables, VOID redemption flow, degradation rows.
6. `05-data-naming-and-parameters.md` — canonical names (your entire label/copy vocabulary)
   and real parameter values (your mock-data numbers).
7. `06-trust-safety-and-degraded-states.md` — the 15 frontend invariants and threat-driven
   UX obligations.

**Rules of engagement.** Protocol facts — states, flows, names, numbers, mandated copy —
come only from these files; where they conflict, files 03/04 win. If a fact you need is
missing, mark it `[ASSUMPTION]` in an annotation and choose the conservative reading (this
system's bias is status-quo-default, honesty over polish). Canonical terms are law: ACCEPT /
REJECT branches, LONG/SHORT, Baseline, `verified-finalized` / `verified-best` /
`derived-local` / `provider` / `stale-cache`, `Voided`, the RejectReason names, screen IDs
S1–S20. Never rename, never synonymize, never soften. Everything the spec does NOT govern —
typography, palette, layout, motion, personality — is yours, and I want a real point of view.

## Deliverable — iteration 1: the core loop, high fidelity

Design a coherent, interactive multi-screen prototype covering:

1. **App shell + epoch header (S1)** — global navigation for: Proposals, Markets, Portfolio,
   Governance, Treasury, History, Advanced; the epoch/phase clock (epoch number, current
   phase of `Intake → Qualify → Seed → Trade → Decide → Review → Execute → Housekeeping`,
   countdown to `next_boundary` in blocks + human time); connection/provenance status; the
   **non-dismissable** "Bootstrap governance: sudo active" banner; verification-panel entry.
2. **Proposal list + detail (S2)** — lifecycle state badges; on detail: the decision
   dashboard ("will it pass?"): ACCEPT vs REJECT vs Baseline TWAPs, uplift vs required
   hurdle δ, the four gate books vs their 0.05 red line, coverage % vs 95%, volume vs
   `v_min`, convergence check, `SecuritySizing` status; the ratification panel with its
   execute-time deadline; committed payload with hash.
3. **Market trading (S3)** — trade ticket with exact quote preview (`cost`, 30 bps `fee`,
   post-trade price `p_after`, mandatory `max_cost` slippage bound, `within_domain` guard);
   the mirror-credit explanation ("you also receive N REJECT-NUM — your principal is
   protected if this branch is annulled"); spot vs TWAP chart with the sanity band [0.02,
   0.98] shaded, visible data gaps, and provenance labeling on the series.
4. **Portfolio (S4)** — positions grouped by proposal and branch; vault-state badges
   (`Open` / `Resolved` / `ScalarSettled` / `Voided`); redeem flows including the full
   **VOID layout**: merge-pairs-first as the visually primary action ("100% recovery"),
   `redeem_void` secondary with the honest 0.5 / 0.25 rates, mixed-holdings decomposition
   with total-recovery figure — the spec forbids describing those rates as a penalty.
5. **Recent settlements (S8)** — the chain-served 32-cohort history (~22 months): per cohort
   the settlement score `s`, Baseline TWAP, proposal outcomes with RejectReason chips.
6. **One complete transaction confirm flow** — the pre-sign ritual: refresh at a fresh
   finalized block, named precondition rows with expected vs actual (and the blocking diff
   state), the three-level payload review (human summary / decoded tree / raw SCALE bytes +
   hash, all decoded from the exact bytes to be signed), fee-currency selector (GOV ⇄ NUM at
   the live `fee.gov_num_rate`), then Broadcast → InBestBlock → **Finalized** with the
   outcome decoded from finalized events.
7. **Degraded-state variants** (as screen states, not an afterthought): first-load
   light-client sync (first verified render takes 30–90 s — make the wait legible: relay →
   parachain → identity → compatibility progression); `SyncDegraded` peer loss with
   per-bootnode diagnostics; pre-sync `stale-cache` rendering with badges; the persistent
   "UNVERIFIED RPC MODE" banner; the PB-LEDGER-FREEZE frozen state; `read-only-incompatible`
   with the newer-release pointer.

Use realistic mock data from file 05 throughout: the worked decision example (ACCEPT 0.560 /
REJECT 0.520, TWAPs 0.5585 / 0.5210 / Baseline 0.5230, uplift 0.0375 vs δ 0.025), real bonds
(PARAM 1,000 NUM … META 50,000 NUM), 21-day epoch with real phase days, NUM with 6 decimals,
GOV with 12, SS58 addresses (prefix 7777) with identicons, block-denominated deadlines shown
in both blocks and human time. No lorem ipsum anywhere.

## Design mandates from the spec (non-negotiable)

- **Provenance is the core visual language.** Every displayed value carries one of the five
  verification statuses. Design this as a first-class system — instantly legible, not
  color-alone (a11y), with text equivalents, consistent from dashboard tiles to table cells
  to chart series. Provider data is visibly second-class *forever* (hatching, badging —
  never promoted).
- **Time pressure is ambient**: countdowns and phase boundaries everywhere, blocks + human
  time together.
- **Charts never lie**: holes render as visible gaps with explainers; downsampled ranges are
  labeled; TWAP vs spot is a first-class pair; no interpolation, no smoothing.
- **Rejection is the system working.** `Rejected(HurdleNotMet)` reads as a calm, informative
  outcome, not an error. Reserve alarm styling for genuine safety states (gate breach,
  ledger freeze, reserve haircut).
- **Dense but calm**: professional users, information-rich tables, tabular numerals; the
  Advanced area is denser still, with no simplified summaries.
- Every screen designed with loading / empty / error / degraded states.

## Aesthetic direction

The subject's own materials: probability (prices that ARE probabilities of welfare, 0–1),
time (a 21-day constitutional clock), and proof (light-client verification). I want
**"precision instrument for collective judgment"** — the confidence of a scientific
instrument and the gravitas of a civic institution, not a crypto-casino and not a generic
SaaS dashboard. Numbers are the protagonists: choose a body/data face with excellent tabular
figures and let the type system carry the personality. Color is semantic first: ACCEPT vs
REJECT need a stable, colorblind-safe pair used with total consistency; provenance statuses
and safety states claim their own reserved hues; decoration must never compete with meaning.
Light and dark themes, dark not being merely inverted.

Before building, write a short **design plan**: 4–6 named hex values, the typefaces and
their roles, the layout concept, and the ONE signature element that makes this app
unmistakably Bliwit (candidates: the epoch phase-clock, the provenance badge language, the
ACCEPT/REJECT dual-world motif — or something better). Critique your own plan against the
generic-AI-design defaults — warm cream + serif + terracotta; near-black + single acid
accent; broadsheet hairlines everywhere — and if any part of your plan would look at home in
a template gallery, revise it and say what you changed. Take one deliberate aesthetic risk
and name it. Then build to the plan, and finish with a self-critique pass: verify every
canonical term against file 05, every mandate above, WCAG AA contrast, visible focus states,
reduced-motion behavior — and remove one decorative element that isn't earning its place.

**Copy voice**: plain verbs, sentence case, specific numbers over adjectives; controls say
exactly what they do (`Split 1,000 NUM`, not `Submit`); errors state what happened and how
to recover, without apologizing; the mandated honesty texts (sudo banner, VOID rates, NAV
haircut, unverified-mode warnings) appear verbatim in intent. If the files contradict each
other or leave something ambiguous, list it under "Spec questions" in your response rather
than guessing.

---

## Suggested follow-up prompts (same conversation, one per iteration)

- "Iteration 2: the governance surface (S9–S11) — six-track referenda list/detail with
  approval/support curves, conviction voting with lock consequences above the fold,
  delegation, the ratification panel states from §11.7.4, and the OracleResolution ballot
  with the pre-cohort snapshot rule (show effective power = 0 warning). Same design system."
- "Iteration 3: funding + balances (S12, S13, S20) — the guided Asset Hub deposit with the
  two-leg tracker (each leg with its own provenance badge), Phase-3 cap notices,
  fee-viability note, withdraw flow with XCM-health line."
- "Iteration 4: the Advanced area (S14–S19) — reporter console with evidence display and
  round timeline (72 h windows, bond doubling, watchtower quorum), guardian 5-of-7 console
  with decoded call batches and allowance meters, treasury NAV view with haircut state and
  outflow-meter gauges, the upgrade crank with hash-verification steps and DescriptorLeadTime
  countdown, welfare snapshot crank, registry filing/challenge."
- "Iteration 5: the welfare & constitution dashboard (S7) and execution queue (S6) — pillar
  composition, gate gauges vs floors, daily breach-flag bitmap calendar, MetricSpec registry;
  the 14-row execute precondition checklist as expected/actual."
