# Bleavit Phase-0 economic simulation

This tree implements the doc-15 §4.9 Phase-0 agent-based calibration tier. It
generates planted-truth proposals across all four market classes and executes
chronological informed, noise, holder, arbitrage, and manipulator orders
against real two-outcome LMSR book state. Every fill is MaxTrade-capped,
balance-limited, charged the doc-13 `mkt.fee` of 30 bps, and retained in an
event ledger. Per the SQ-231 amendment (04 §7a), step-5 contest grading and
the step-9 certificate consume **time-averaged at-risk contest capital** — the
marked unmatched directional trader position, replayed through the reference
model's `ContestCapitalAccumulator` with previous-block semantics — bounded in
step 9 by the `sec.flow_cap` ceiling. Wash churn and balanced complete-set
inventory both net out; gross `Traded.cost` sums and the balanced organic-holder
leg remain recorded in the executed ledger but do not certify security depth.
An attacker or arbitrageur's unmatched directional exposure counts exactly
like any other real at-risk exposure.

Run the deterministic full calibration (10,000 proposals):

```sh
python3 tools/simulation/run-calibration.py --full
```

Validate the committed artifact, its source/Python binding, Merkle-style
outcome root, and its byte-exact pinned replay:

```sh
python3 tools/simulation/run-calibration.py --check
```

`--check` distinguishes structure from economics. It prints
`calibration structure OK` after the evidence reproduces; it still exits 1 if
the structurally valid artifact records normative violations. A red calibration
therefore blocks publication without being mistaken for corrupt evidence.

Run the fast suites:

```sh
PYTHONPATH=reference-model/src:simulation/src \
  python3 -m unittest discover -s simulation/tests
PYTHONPATH=reference-model/src \
  python3 -m unittest discover -s reference-model/tests
python3 tools/reference-model/generate-vectors.py --check
```

## Pre-registered reporting

The executable config pre-registers four `|true_effect| / effective_delta`
bands (`[0,.5)`, `[.5,1)`, `[1,2)`, `[2,3]`), the attack-strategy mix, gate
exposure, and thin/marginal/deep formation regimes. The artifact reports every
axis conditionally and also reports a distribution-weighted aggregate. The
decidable-harm weighted rate renormalizes weights over only the registered
`|effect| >= delta` bands, so sub-hurdle strata cannot dilute that diagnostic.

The doc-15 §4.9 false-pass publication gate is evaluated per class on
*decidable harm*, `|true_effect| >= delta`, because harm smaller than the
decision hurdle is not what that hurdle purports to defend against. The full
distribution aggregate remains visible and is evaluated as a diagnostic; it
is intentionally sensitive to the pre-registered stratum weights. Publication
also requires clean sub-`3P` causal threshold brackets. Until those gates pass,
the `published` block is explicitly `candidates-only`.

Threshold search covers every observed attacked wrong-PASS candidate in the
full population, plus a pre-registered beneficial sample for the separately
reported griefing diagnostic. It uses state-identical zero-budget
counterfactuals and 5% relative-width binary brackets. Envelope evidence is
fail-closed when outcome or realized-loss monotonicity is not demonstrated;
endpoint losses are never presented as a loss bracket by assumption.

The Baseline contest floor of 250,000 USDC is the TREASURY-tier `dec.v_min.trs`
that 05 §5.2 mandates for the Baseline book (SQ-232 resolution, 2026-07-18).
Phase-0 also names its always-clean scheduled-coverage and undisturbed-POL legs
as assumptions rather than measured defenses.

## Thin-market promotion under the contest-capital measure

The pre-amendment seam — attack-generated *gross* flow promoting a below-`v_min`
book while self-funding the step-9 certificate — is closed by SQ-231: churn
nets out of the 04 §7a measure, so promotion requires genuinely held net
exposure across the window, and the `sec.flow_cap · (b_acc + b_rej)` ceiling
now bounds the step-9 `L_hat` contribution. The artifact keeps measuring and
recording promotion (`thin_market_capture`): attacker plus arbitrage *held*
capital can still lift a thin book to decision grade, but at genuine
capital-at-risk the threshold brackets price against `3·InCapPrize`.

`AttackCost_hat` remains the doc-08 upper estimate. The signed, book-specific
`ManipFloor_hat` envelope is applied only to causal wrong-PASS full+trailing
decision-book displacement brackets. Wrong-REJECT and convergence-DoS flips
are separately labeled griefing-cost diagnostics. Close-time attacker unwind
is a conservative counterfactual liquidation valuation (including fees), not
an invisible emitted trade, so it does not alter contest flow or the decision
TWAP.

This tier does not replace public testnet contests, mechanism-design review,
the Phase-3/4 measurement of `F_hat_pub`, or the independent audits required by
doc 15.
