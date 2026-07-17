# Bleavit fuzzing workspace

This is a separate Cargo workspace because libFuzzer requires nightly Rust and
brings a fuzz-only dependency graph. As with `keeper/`, keeping it outside the
root workspace prevents those dependencies from perturbing the runtime's
`=`-exact `polkadot-stable2606` pins.

## Targets and assertions

- `payload_scale_decode` runs two paths. (a) Raw-bytes: decode an untrusted
  SCALE `Payload` behind codec recursion and allocation limits, round-trip it,
  and drive the execution-guard boundary. (b) Structured: an `Arbitrary`
  generator builds plausible payloads (call trees, mixed small/boundary/overflow
  declared lengths, matched or mismatched committed hash) so the guard's admit
  and bound-rejection paths are actually reached — random bytes almost never
  SCALE-decode into a valid `Payload`. Both assert the 16-call/64-KiB limits,
  checked length sums, committed-field hash binding, outcome-field hash
  independence, verdict-independence from the queue `hash` field, and guard
  `try_state` after every admitted batch (15 §4.5; I-10/I-11). NOTE: this target
  fuzzes the frame-free MODEL type (`execution_guard_core::Payload`). The
  production preimage-decode boundary is `pallet_execution_guard::RuntimeBatch`
  over the aggregate runtime call, a frame-dependent type this nightly workspace
  cannot link; its unbounded-recursion decode was hardened separately
  (`kernel::MAX_PAYLOAD_DECODE_DEPTH`, `decode_all_with_depth_limit`) with the
  runtime-crate regression `deep_preimage_batch_decode_fails_closed_at_the_depth_limit`
  (SQ-135).
- `nested_wrapper_filter` generates bounded but limit-crossing call trees and
  checks `SafetyFilter::validate`/`validate_batch` three ways: against an ordered
  structural oracle (agreeing on accept/deny AND the exact error variant), and
  against `independent_admits` — an INDEPENDENT property predicate (a separate
  node-count / container-depth / structural-denial / per-leaf-privilege
  computation, not a walk clone) applied to BOTH the origin-less and the
  `Some(class_origin)` guard paths. The `wrapper_admission_truth_table` unit test
  pins a hand-computed enumeration of the 06 §3.3 rows so a shared logic error
  cannot hide behind mutual agreement. Covers `proxy_announced`,
  `as_multi_threshold_1`, shared batch budgets, dispatch-as denial, scheduler
  origin capture (which OVERRIDES the outer origin), nobody calls, and transitive
  proxyish privilege denial (I-8/I-10/I-11). It also depth- and memory-limits raw
  SCALE call decoding. The caller-origin monotonicity assertion excludes the
  subtree below a valid scheduler boundary (revalidated under the captured values
  origin, not the caller's).
- `lmsr_trade_paths` generates trade sequences over sensible `b`, side,
  buy/sell, bounded amounts, and a **seed-selected book kind** — Decision (either
  branch), Gate (either branch, either `GateType`), or the unbranched Baseline.
  It drives the real `market-core` `buy_book`/`sell_book` (hence
  `buy_gate`/`sell_gate`/`buy_baseline`/`sell_baseline`) through an atomic mock
  `LedgerOps`, reading the kind-correct position set (Decision `Long`/`Short`,
  Gate `GateYes`/`GateNo`, Baseline `PositionId::Baseline{epoch, side}`). It
  checks `try_state`, the exact per-kind inventory/conservation identity (the
  fee-retaining Baseline wrapper tracks a distinct inventory accumulator from the
  gross cash flow), the I-12 ceiling `maker drain <= ceil(b*ln(2))` (the exact
  headroom, no per-trade slack), strict cost monotonicity on buys, and
  fee-and-rounding-protected round trips. Exercising Baseline matters because
  `sell_baseline` was historically solvency-buggy. A fail-closed trade (the book
  cannot collateralize the delivered side — e.g. the `depletion_sell_rejection`
  corpus seed) is a legitimate no-op, rolled back and skipped; the invariants
  still run on every realized state. It also probes the fixed kernel at and across
  the `48*b` domain edge (04 §4-§6, especially §6.3; I-12). Every literal USDC
  base-unit scale comes from `futarchy_primitives::currency::USDC` (rule 4).

The independent wrapper oracle has ordinary unit tests and runs with:

```sh
cd fuzz
cargo test --workspace --locked
```

Run one target against its committed corpus and then fuzz it locally:

```sh
cd fuzz
cargo fuzz run nested_wrapper_filter corpus/nested_wrapper_filter -- -runs=0
cargo fuzz run nested_wrapper_filter corpus/nested_wrapper_filter
```

Run all CI-format, build, corpus-regression, and smoke gates from the repository
root with `tools/ci/fuzz-gates.sh`. CI smoke runs are intentionally short and
catch regressions in the committed corpus plus shallow random exploration.
Long campaigns, corpus distillation, and sanitizer-matrix campaigns belong to
the B8 release pipeline.

On a finding, preserve the original artifact under `artifacts/<target>/`, run
`cargo fuzz tmin <target> <artifact>`, and add the minimized reproducer to
`corpus/<target>/` before fixing it. A crash, timeout, invariant assertion, or
out-of-memory termination is a real finding; never delete it merely to make the
smoke gate green. Targets have no network dependency.
