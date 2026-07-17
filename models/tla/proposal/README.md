# Proposal-machine TLA⁺ model

`Proposal.tla` is the bounded exhaustive model required by 15 §4.1 for the
single-proposal lifecycle in 05 §2.1. It checks I-9, I-14, I-15 and I-18 over
keeper, guardian, values-ratification, attestation, version invalidation,
oracle-dispute, VOID, retry and T20 interleavings. `Small.cfg` uses PARAM/CODE
and the compact welfare outcome set; `Full.cfg` adds TREASURY/META and every
abstracted 05 §5.5 failure. `ForceRerun.cfg` enables the separately labeled
06 §5.3 `force_rerun` edges and rechecks the full main invariant/property set.
`Constitutional` is deliberately absent: 05 §1.1 and §5.6 route those subjects
to the values/referenda track with no markets, so they do not enter this market
proposal machine.

The force-rerun scope is conditional because of open specification question
PLAN.md SQ-161: 06 §5.3 requires `Trading/Extended/Queued → Extended`, while
05 §2.1 calls its T1–T24 table exhaustive and contains no such row. The model
constant `ForceRerunModeled` is `FALSE` in every pre-existing main, witness and
mutation config, preserving the strict 05 §2.1 interpretation and its state
spaces. Only the explicitly named force-rerun configs set it to `TRUE`; they
prove safety under the 06 §5.3 interpretation pending SQ-161's resolution.

Run any main scope directly from this directory:

```bash
java -cp ../../../tools/verify/bin/tla2tools.jar tlc2.TLC \
  -workers 8 -config Small.cfg Proposal.tla
java -cp ../../../tools/verify/bin/tla2tools.jar tlc2.TLC \
  -workers 8 -config Full.cfg Proposal.tla
java -cp ../../../tools/verify/bin/tla2tools.jar tlc2.TLC \
  -workers 8 -config ForceRerun.cfg Proposal.tla
```

The repository-wide harness reads `manifest.env`, including the
`WITNESS_CONFIGS` reachability legs described below. With eight workers Small
currently reaches 66,709 distinct states in 29 seconds and Full reaches 264,241
in 1 minute 49 seconds. The full force-rerun scope reaches 568,235 distinct
states. `MIN_DISTINCT_STATES=33350` is deliberately about half the observed
Small count, so an accidentally disabled action graph fails even when every
invariant is vacuously true.

## Configurations

| Config(s) | `ForceRerunModeled` | Harness role |
|---|---:|---|
| `Small.cfg`, `Full.cfg` | `FALSE` | Main strict-05 §2.1 safety scopes |
| `ForceRerun.cfg` | `TRUE` | Full main invariant/property set under 06 §5.3 |
| Pre-existing `Witness*.cfg`, `MutationI14.cfg`, `MutationT16.cfg` | `FALSE` | Original reachability/falsifiability scopes, unchanged |
| `WitnessForceRerun.cfg` | `TRUE` | Expected violation proves an `FR` edge is reachable |
| `WitnessForceQueuedCancel.cfg` | `TRUE` | Expected violation proves a queued mandate is atomically cancelled and recorded |
| `MutationForceCancelExecute.cfg` | `TRUE` | Expected I-15 violation if a force-cancelled mandate is dispatched |

## Transition correspondence

The history record is the coverage witness: every proposal-state edge appends
the unique matching T-row, action kind, reason and mandate id. Automatic T17
and T21 append adjacent records in the same TLC step. This makes their
dispatch/same-block coupling uninterruptible while retaining both normative
rows in counterexample traces.

| Model action | 05 §2.1 row(s) | `epoch-core` cross-check |
|---|---:|---|
| `Submit` | T1 | `submit` |
| `Withdraw` | T2 | `withdraw` |
| `StartScreening` | T3 | `tick` → `qualify` |
| `CancelScreening` | T4 | `qualify` / `cancel` |
| `Qualify` | T5 | `qualify` |
| `Rollover` | T6 | `qualify` → `rollover_or_refund` |
| `OpenMarkets` | T7 | `open_markets` |
| `DecideEvaluation` + shared `DecideOutcome` (`Extend`) | T8 | `decide_with` (`DecisionOutcome::Extend`) |
| `DecideEvaluation` + shared `DecideOutcome` (`Adopt`) | T9 | `decide_with` (`DecisionOutcome::Adopt`) |
| `DecideEvaluation` + shared `DecideOutcome` (`Reject`) | T10 + automatic T21 | `decide_with`, `decide_engine`, `reject_to_measurement` |
| `GuardianDelay` | T11 | `delay_once` |
| `ScheduleRerun` | T12 | `schedule_rerun` |
| `OpenRerun` | T13 | `open_rerun` |
| `Execute` | T14 + automatic T17 | `mark_executed` (guard-side dispatch is outside the core) |
| `Expire` | T15 + automatic T21 | `expire_or_stale_queue` |
| `GuardReject` | T16 + automatic T21 | `expire_or_stale_queue` |
| accept resolution coupled to `Execute`/`RetrySucceeds` | T17 | `mark_executed` / `start_measurement` |
| `PayloadReverts` | T18 | `mark_failed_executed` |
| `SettleCohort` | T19 | `settle_cohort` |
| `ForceReject` | T20 | `force_reject_process_hold`, `tick` |
| reject resolution coupled to T10/T15/T16/T24 | T21 | `reject_to_measurement`, `expire_or_stale_queue` |
| `RetryExhausted` | T22 | `retry_exhausted_to_measurement` |
| `RetrySucceeds` | T23 + automatic T17 | `mark_executed` |
| `ReviewUpholdsVeto` | T24 + automatic T21 | `veto_upheld` |
| `GuardianForceRerun` | **FR — 06 §5.3, conditional per SQ-161; not a T-row** | guardian `force_rerun` hook into the epoch machine |

Environment-only actions do not change the proposal state and therefore do
not claim T-rows: `ValuesRatify`, `RatificationFails`, `RevokeAttestation`,
`InvalidateVersion`, `AdvanceGrace`, `AdvanceRetryDeadline`, `CloseChallenge`,
`ContestValue`, `NeutralizeContest`, and `VoidContest`. They expose the
interleavings consumed by guard and cohort transitions. `VoidContest` changes
only the cohort/vault shadow to absorbing VOID; it deliberately does not invent
a proposal edge absent from the exhaustive 05 §2.1 table.

| Environment action | T-row | Implementation correspondence |
|---|---:|---|
| `ValuesRatify` | — | execution-guard ratification seam consumed by `mark_executed` / `expire_or_stale_queue` |
| `RatificationFails` | — | 02 §2 `RatificationStatus::Failed`; permits pre-grace T16 and disables a T23 retry |
| `RevokeAttestation` | — | 09 §1.2(5) live guard seam in Queued or FailedExecuted; rejection is T16 while Queued and retry re-validation fails after T18 |
| `InvalidateVersion` | — | 09 §1.2(3) version-constraint mismatch required by `StaleQueue`; also re-checked by T23 |
| `AdvanceGrace` | — | clock input consumed by `expire_or_stale_queue` for T15/T16 |
| `AdvanceRetryDeadline` | — | clock input consumed by `retry_exhausted_to_measurement` for T22 |
| `CloseChallenge`, `ContestValue` | — | welfare/oracle input seam consumed by `settle_cohort` |
| `NeutralizeContest` | — | 07 §10–11 force-neutralization before `settle_cohort` |
| `VoidContest` | — | cohort/vault shadow of `void_cohort`; no 05 §2.1 proposal row is claimed |

## Property correspondence

- `TransitionTableExhaustive` gives every logged edge exactly one T-row
  interpretation and requires an unbroken `Absent`-to-current-state history.
  Reachability is a separate obligation discharged by witness configs, not
  inferred from this row validator.
- `I15NoRejectedMandateExecutes` requires T9/full-pass provenance for every
  T14/T23 mandate, rejects a same-mandate T10/T15/T16/T20/T24 predecessor,
  and treats a same-mandate `FR` as an execution-cancelling predecessor while
  still permitting the fresh T9 required after the rerun. The persistent
  `forceCancelledMandates` set supplies independent provenance: an id in that
  set can never become `executedMandate`. Mandate ids distinguish each
  cancelled queue entry from the fresh T9 entry after T13 or FR.
- `I9OnlyDecideQueuesOnlyGuardExecutes` checks action provenance directly over
  the T-row history (15 §1 I-9).
- `I14GateVetoBeforeWelfare` checks every reachable decision record with a gate
  veto and requires the matching T10 rejection. Each `DecideEvaluation` draws
  both gate and welfare observations, then the one shared `DecideOutcome`
  definition applies the 05 §5.4 ordering. A witness reaches gate-veto plus
  welfare-says-Adopt, and `MutationI14.cfg` proves that allowing welfare to
  override the veto violates I-14.
- `RerunRaisedHurdleApplied` requires the post-T13
  `RaisedHurdleMiss` case—base δ passes, δ+1 pp fails—to reject with
  `HurdleNotMet` whenever earlier checks did not already decide the result
  (05 §2.1 T13, §5.4 steps 6–8).
- `NoRetryAfterDeadline` records expired mandate ids and forbids a later T23;
  T23 itself checks `retryExpired`, version validity, ratification Passed and
  live attestation again (05 §2.1 T23; 09 §1.2).
- `I18ChallengeClosedSettlement` permits T19 only after `Closed`; an input
  that was `Contested` can only be neutralized before T19 or become VOID, and
  never records a money-moving settlement (05 §7; 07 §10–11; 15 §1 I-18).
- `BudgetsAndRerunFinality`, `SameBlockT21ExactlyOnce`,
  `VaultCouplingSanity`, `TerminalStatesAbsorb`, and
  `TerminalAttemptsRejected` check the ordinary extension budget, the shared
  one-ever T11/FR guardian-rerun budget, finality after either T13 or FR,
  mandatory retrospective-review scheduling, automatic rejection resolution,
  authorized branch resolution, T20/no-measurement coupling, terminal-state
  absorption, and explicit rejected dispatch attempts whose real action name
  is recorded in the separate audit variable while all protocol state remains
  unchanged.

## Reachability and mutation legs

The manifest lists witness configs that assert the negation of an interesting
condition. The repository runner requires TLC to violate every witness
invariant. The witnesses cover T2; T6+T24; T13+T18+pre-deadline T23; T20 with an
open vault; a live FailedExecuted state after retry expiry; T22; early
ratification-Failed T16; gate-veto plus welfare-says-Adopt; the rerun raised
hurdle; an explicit rejected terminal attempt; an FR edge; and specifically a
queued FR whose mandate id is recorded in `forceCancelledMandates`. This is
the reachability counterpart to the main configs' safety invariants.

`MutationI14.cfg` sets `MUTATE_I14=TRUE`, making shared `DecideOutcome` permit
welfare-Adopt to override a gate veto; TLC must violate
`I14GateVetoBeforeWelfare`. `MutationT16.cfg` sets
`MUTATE_T16_ENQUEUE=TRUE`, making a T16 path manufacture a fresh T9; TLC must
violate `I15NoRejectedMandateExecutes`. `MutationForceCancelExecute.cfg` sets
`ForceRerunModeled=TRUE` and `MUTATE_FORCE_CANCEL_EXECUTE=TRUE`, deliberately
dispatching the force-cancelled mandate directly from Extended; I-15 must catch
that trace. All mutation constants are FALSE in every main and ordinary witness
config and exist only to keep these regression mutations reproducible.

Run the three expected-failure mutation legs directly:

```bash
java -cp ../../../tools/verify/bin/tla2tools.jar tlc2.TLC \
  -workers 8 -config MutationI14.cfg Proposal.tla
java -cp ../../../tools/verify/bin/tla2tools.jar tlc2.TLC \
  -workers 8 -config MutationT16.cfg Proposal.tla
java -cp ../../../tools/verify/bin/tla2tools.jar tlc2.TLC \
  -workers 8 -config MutationForceCancelExecute.cfg Proposal.tla
```

## Abstractions and preservation arguments

| Abstraction | Why the checked invariants survive it |
|---|---|
| One proposal, at most two mandate ids | I-9/I-14/I-15 are per-proposal. Two ids are sufficient for either requeue family: initial T9, then T11–T13 or queued FR, followed by a fresh T9. |
| Blocks compressed to ordering flags | Exact durations do not affect the safety properties. Separate grace/retry actions preserve every before/after interleaving, including the 72 h T18 window. |
| Gate and welfare arithmetic drawn together | Exploring both observations over-approximates numeric TWAP results; shared `DecideOutcome` alone imposes the §5.4 order. Earlier failures are recorded as `NotEvaluated`, while a veto record retains the independently drawn welfare result so I-14 is testable. |
| Gate applicability is class-scoped | CODE/META always carry gates, PARAM does not, and TREASURY nondeterministically represents either side of the 1% NAV threshold (05 §5.1). Constitutional subjects are on the separate no-market values track (05 §1.1/§5.6). |
| Welfare arithmetic is abstract result families | `Pass`, `RaisedHurdleMiss`, `Insufficient`, `Disagree`, plus named Full failures retain every control-flow/reason family while dropping prices, balances and fixed-point arithmetic irrelevant to these machine invariants. |
| T13/FR market economics | The δ+1 pp rerun control-flow effect is modeled explicitly. Reopening/resetting books and TWAP accumulators, the FR 3-day window, and T13's 2× POL are market-state/economic obligations, not proposal-machine state. Positions and the vault shadow are untouched by FR, matching 06 §5.3; numeric effects remain covered by market/runtime tests. |
| Guardian approvals, bonds, slashes, and per-epoch allowance arithmetic omitted | `GuardianForceRerun` represents the already-authorized 5-of-7 dispatch. The one-per-proposal shared rerun budget is explicit, as is mandatory retrospective-review scheduling. Membership aggregation, the one-per-epoch meter, review deposits, and later slash/recall bookkeeping cannot authorize enqueue/execute or replenish that consumed proposal budget. |
| Proposal ratification and attestation survive reruns | FR changes neither field, matching 06 §5.3's requirement that the records survive; the independent environment actions remain free to change them later where the ordinary guard rules permit. |
| T17/T21 are atomic with their parent row | 05 §2.1 says T17 is automatic and T21 is same-block. Atomic append removes impossible interleavings without removing a legal one. |
| T22 tags its ACCEPT resolution as `T17Accept` | T22 normatively resolves Accept itself. The tag factors the same authorized accept-resolution primitive used by T17; history still records T22, never a fictitious T17 edge. |
| Cohort aggregation and settlement cursor collapsed | A single proposal/input is the minimal witness for an open/closed/contested violation. Batching cannot turn a contested value into a closed one. |
| Oracle VOID leaves proposal state unchanged | It changes the independent cohort/vault shadow to absorbing `Void`, so no absent 05 §2.1 proposal transition is invented and no later T19 is possible. |
| T4 static failures use `ConstitutionViolation` as representative | All T4 reasons have the same Cancelled/no-vault control flow and cannot affect the four checked invariants. |

### T15/T16 precedence decision

The table permits T15 and T16 failure conditions to become observable at the
same deadline without stating a reason-priority rule. This model gives specific
T16 causes precedence: version invalidation, ratification Failed, Pending at
grace end, or missing attestation disable generic T15; T15 represents only a
healthy-but-unexecuted mandate whose grace elapsed. A Failed ratification can
fire T16 before grace end, while Pending remains retryable until grace end, in
line with 05 §2.1 and 06 §2.2. This conservative disambiguation is a modeling
decision pending the repository's logged specification question; no normative
spec text is changed here.

TLC exhausts only these declared finite scopes. The model proves the listed
safety properties for this abstraction; it does not prove liveness, economic
sizing, numeric welfare correctness, or multi-proposal cohort bounds.
