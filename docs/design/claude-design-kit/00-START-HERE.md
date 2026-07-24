# Bleavit design-context pack — START HERE

> Generated 2026-07-12 from the frozen specification in `docs/architecture/` at commit
> `9f250be`; refreshed through integration contract v12 on 2026-07-23 (A12 signed oracle escalation custody). This pack exists
> because design tools cap attachments (Claude Design: 10 files)
> and the full spec is ~677 KB across 17 documents. It compresses the spec into 7 files
> without losing anything a frontend designer needs. **The spec always wins over this pack.**

## What you (the design model) are looking at

Bleavit is a futarchy-governed blockchain — prediction markets make its decisions — with a
fully decentralized canonical web app (in-browser light client, Arweave-distributed, no
backend). The complete product behavior is already specified; these files are that
specification, curated for design work.

## The files, in reading order

| File | What it is | Trust level |
|---|---|---|
| `00-START-HERE.md` | This manifest | — |
| `01-product-brief.md` | What Bleavit is, who uses it, product principles, look-and-feel drivers | Derived synthesis |
| `02-domain-model-and-lifecycles.md` | Every entity, state machine, payout rule, role — what each screen's objects ARE | Derived distillation |
| `03-frontend-architecture-VERBATIM.md` | **Frozen spec doc 10, verbatim**: provenance/trust model, boot state machine, compat modes, three-layer history, resource budgets | Normative (verbatim copy) |
| `04-frontend-workflows-and-screens-VERBATIM.md` | **Frozen spec doc 11, verbatim**: screen inventory S1–S20, transaction/signing rules, per-call precondition tables, VOID flow, governance/operator/funding surfaces, degradation rows E15–E23 | Normative (verbatim copy) |
| `05-data-naming-and-parameters.md` | The readable data surface, canonical names (extrinsics, events, states, errors — your label/copy vocabulary) and every UI-visible parameter value (your mock-data numbers) | Derived distillation |
| `06-trust-safety-and-degraded-states.md` | The 15 frontend invariants (INV-FE), threat-model UI obligations, degraded modes to design | Derived distillation |

## Ground rules

1. **Protocol truth comes only from these files.** States, flows, names, numbers, mandated
   copy — never invent, extrapolate or "improve" them. If a needed fact is missing, mark it
   `[ASSUMPTION]` visibly and pick the conservative reading (this system's bias is
   status-quo-default and honesty-over-polish).
2. **Canonical spelling is law.** ACCEPT/REJECT, LONG/SHORT, Baseline, `verified-finalized`,
   `provider`, `stale-cache`, `Voided`, RejectReason variants, screen IDs S1–S20, E-row IDs —
   use them exactly. (E-row numbering follows file 04; see the caution in file 06.)
3. **`[VERIFY]` tags mark genuinely unresolved facts** in the spec. Design around them
   conservatively; never resolve them by assumption.
4. **Visual identity is yours.** The spec mandates behavior and honesty, not aesthetics.
   Typography, palette, layout, motion, density, personality — open territory, and the reason
   this pack exists.

## Regeneration

Files 03/04 are byte-copies of `docs/architecture/10-…` and `11-…` plus a header; re-copy to
refresh. Files 01/02/05/06 are distillations; regenerate them against the spec if the
architecture is ever amended (it is frozen — amendments are rare and logged in PLAN.md).
