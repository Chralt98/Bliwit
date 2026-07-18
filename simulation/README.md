# Bleavit Phase-0 economic simulation

This tree implements the doc-15 §4.9 Phase-0 agent-based calibration tier. It
generates planted-truth proposals across all four market classes and executes
chronological informed, noise, arbitrage, and manipulator orders against real
two-outcome LMSR book state. Every fill is MaxTrade-capped, balance-limited,
charged the doc-13 `mkt.fee` of 30 bps, and retained in an event ledger. Step-5
contest notional and step-9 measured depth are sums of emitted `Traded.cost`
magnitudes; attacker and arbitrage fills count exactly like any other fill.

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

The synthetic Baseline contest floor of 250,000 USDC is a named assumption,
using the TREASURY-tier scale as an analogy to doc 08 §4.3; it is pending a
specification question. Phase-0 also names its always-clean scheduled-coverage
and undisturbed-POL legs as assumptions rather than measured defenses.

## Preserved thin-market finding

The artifact always records the confirmed seam prominently: attacker and
arbitrage gross contest flow can promote an initially below-`v_min` book to
decision grade and simultaneously increase step-9 `L_hat`/`AttackCost_hat`,
which can license the same flip. Doc 05 §5.6 caps wash flow only inside
`ManipFloor_hat.C_hold`; it does not cap step-5 grading or step-9 measured
depth. This finding is retained whether the remediated headline rates pass or
fail.

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
