# models/ — formal models (S1, 15 §4.1; 03 §11)

TLA⁺ models checked exhaustively by TLC on small finite instances. These are the
15 §4.1 "model checking" gate: they prove the cited invariants **over all
interleavings** of the abstracted state machines — including guardian,
oracle-dispute, `void` and T20 events — at the declared scopes.

| Model | Module | Proves | Owning spec |
|---|---|---|---|
| `tla/ledger/` | `Ledger` | Per-proposal-vault portion of I-1 (L-1 identities), 03 §6.5's `V(state) <= E` bound, I-3's exactly-once/authority/`Open` structure (post-decision assumed at this interface and discharged by the proposal model's I-9/I-15 T17/T21/T22 paths), I-26, amended I-27 including post-archive Voided sweep, D-8, outflow monotonicity, and rejected-call economic stuttering. Baseline identity, cross-vault summation, and I-4 sovereign custody remain try-state/PT/differential obligations. | 03 §2.3, §5.4, §6, §11; 05 §2.1; 15 §1 |
| `tla/proposal/` | `Proposal` | I-9 (only decide enqueues; only the guard executes), I-14 (gate vetoes before welfare, no override), I-15 (no rejection/timeout/veto/expiry path enqueues or executes), I-18 (only challenge-closed values settle; contested ⇒ neutral/VOID), T1–T24 exhaustiveness (anything absent errors) | 05 §2.1, §5.4; 15 §1 |

## Running

```bash
tools/verify/run-model-checks.sh
```

The runner fetches the pinned `tla2tools.jar` (pin: `tools/env/pins.env`,
digest-verified), then checks every `models/tla/<name>/` per its
`manifest.env` (contract documented in the runner header). Ordinary configs must
pass all invariants/properties and clear `MIN_DISTINCT_STATES`. `WITNESS_CONFIGS`
invert reachability conditions: TLC must report their named invariant violation, so
an accidentally unreachable behavior fails the gate rather than passing vacuously.
CI runs this as the `model-checking` job.

## Abstraction discipline

A model proves what it models — each model directory's README carries a
**correspondence table** mapping every model action/constant to the spec
section and implementation site it abstracts, plus the abstractions taken
(amounts bounded, account sets small, etc.). The models are spec-derived, not
code-derived: on divergence between model and implementation the spec wins
(R-1), and the finding belongs in PLAN.md · Spec questions.

Scope caveat (honesty): TLC exhausts the state space of the *declared finite
instance* (the `.cfg` constants). The invariants are proven for those scopes;
the induction argument extending conservation to all scopes is 03 §6.5.
