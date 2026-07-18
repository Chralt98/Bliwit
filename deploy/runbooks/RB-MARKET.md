---
id: RB-MARKET
title: Market solvency-bound and numerical anomaly response
owner_role: Monitoring coordinator
funding_line: ops.monitoring
page_immediately: false
alerts:
  - domain: Markets
    trigger: "book loss > 0.9·b·ln2"
  - domain: Numerics
    trigger: "anomaly spike"
spec_refs:
  - docs/architecture/03-conditional-ledger.md
  - docs/architecture/04-markets-and-pricing.md
  - docs/architecture/05-welfare-and-decision-engine.md
  - docs/architecture/12-release-and-operations.md
  - docs/architecture/15-invariants-and-testing.md
---

## Purpose

Investigate books approaching the LMSR worst-case subsidy envelope and spikes in fixed-point domain
rejections or rounding dust, while preserving the I-12 solvency boundary and refusing any invented
privileged market intervention.

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| Markets | price, depth, spread proxy, book P&L | book loss > 0.9·b·ln2 |
| Numerics | LMSR domain-rejection count, rounding-dust accumulation | anomaly spike |

The market trigger is an early warning below the per-book I-12 bound, not evidence by itself of
insolvency: a legitimate persistent one-sided flow can consume most seeded subsidy. The numerics
trigger means rejection or dust behavior has departed from its established baseline and must be
classified against the fixed-point guard rails.

## Diagnosis

1. Pin the alert to a finalized block, market id, book kind, proposal/epoch, and live `b`. Read
   `Market.Markets`, `Market.SeededMarkets`, the owning ledger vault, and `Seeded { market,
   headroom }`; never substitute a configured or dashboard `b` for chain state.
2. Reconstruct book cash flow and inventory from `Traded` events plus the book account's complete
   sets. The book-P&L series is planned under [12 §6.3](../../docs/architecture/12-release-and-operations.md);
   until it lands, the event/storage reconstruction is the auditable source. Compare the loss to
   the exact I-12 expression and rounding allowance in
   [04 §3 and §6.3](../../docs/architecture/04-markets-and-pricing.md).
3. Classify benign one-sided flow: fills and `q` move consistently toward one side, fees and
   revenue recycling continue, the book retains sufficient complete sets, the owning vault is
   live, and no ledger conservation/try-state alarm accompanies the loss. The bound is approached
   from below under finite legal-domain flow.
4. Classify a possible fault: P&L jumps without matching successful fills, book inventory differs
   from ledger positions, `b` or seed markers changed unexpectedly, a legal-domain quote fails,
   or the loss calculation exceeds the specified bound plus rounding. Any ledger balance drift is
   RB-LEDGER, not a market-only incident.
5. For numerics, count finalized `PriceBoundExceeded` and `ArithmeticOverflow` dispatch errors by
   market and input. A `PriceBoundExceeded` at the LMSR edge is a correct guard-rail refusal;
   repeated failures well inside the live domain or state mutation on refusal are faults
   ([04 §4](../../docs/architecture/04-markets-and-pricing.md)).
6. Inspect charges, payouts, and fee rounding. Charges must round up, payouts/proceeds down, and
   cumulative maker benefit/dust stays within the per-trade rule before ledger sweeping. Separate
   many legitimate minimum-size dust entries from a discontinuity or claimant-favoring sign.
7. Run or inspect the latest market/ledger `try-state` evidence. Market try-state checks the live
   LMSR domain, vault backing, and TWAP accumulator sanity; the release differential/property
   suites own the numerical loss proof ([15 §1 and §4](../../docs/architecture/15-invariants-and-testing.md)).
8. Check decision impact through `FutarchyApi::decision_stats(pid)` and the book's coverage,
   staleness, POL, contest, and convergence data. Failed grade must extend/reject under [05 §5](../../docs/architecture/05-welfare-and-decision-engine.md),
   never be manually promoted to an adoption.

## Remediation

### Safe / permissionless

1. Preserve the finalized state, event range, quote inputs, and independent P&L recomputation.
   Continue due `crank_observe` calls only while the book is trading and no verified freeze is
   active; observations cannot repair inventory or override close.
2. Stop operator-authored trades or test traffic against the affected book. Users retain their
   ordinary signed rights, but operations must not attempt to “balance” a book with treasury or
   guardian funds.
3. If the behavior is a legitimate one-sided walk and all I-12 checks hold, observe through close
   and settlement. The pre-collateralized subsidy exists for this case.
4. If a numerical fault is suspected, reproduce it against the committed reference vectors and
   release sweep artifacts. A rejected trade must remain a no-op; do not loosen the domain or
   rounding checks in production.

### Privileged

1. There is no privileged market-price, inventory, P&L, or settlement intervention. Market
   `buy`/`sell` and observation are Signed; lifecycle transitions come from the epoch/settlement
   authorities specified in [06 §3](../../docs/architecture/06-governance-and-guardians.md).
2. The specified guardian playbooks apply only to their verified triggers: PB-LEDGER-FREEZE
   requires an I-4 drift flag, and PB-DEPEG freezes creation rather than rewriting an open book.
   Neither is activated solely because a book approaches its expected subsidy loss. Their current
   downstream effects are not represented on chain and fail closed
   ([runtime dispatcher](../../runtime/bleavit-runtime/src/configs.rs)); escalate rather than
   claiming containment succeeded.
3. A code repair follows the normal or trigger-qualified expedited CODE path and execution guard.
   Until then, status-quo decision rejection and existing collateralized settlement paths are the
   safe outcome.

## Escalation

Page the Monitoring coordinator and the Keeper coordinator if observation coverage is also
degrading. Page the Release operations lead with the minimal reproducer for any inner-domain
numerics fault, and hand any conservation drift immediately to RB-LEDGER/PB-LEDGER-FREEZE. Page the
Oracle operations coordinator only when settlement inputs or disputes, rather than book math, are
implicated. Record whether the case was benign one-sided subsidy consumption, a monitoring
calculation error, or a protocol fault; never close on “price looked extreme” alone.

## References

- [03 §6–§9 — ledger conservation, rounding, and errors](../../docs/architecture/03-conditional-ledger.md)
- [04 §3–§7 — LMSR bound, fixed-point guard rails, solvency, and TWAP](../../docs/architecture/04-markets-and-pricing.md)
- [05 §5 — decision-grade status-quo behavior](../../docs/architecture/05-welfare-and-decision-engine.md)
- [06 §3 — authority matrix](../../docs/architecture/06-governance-and-guardians.md)
- [12 §6.3 — Markets and Numerics alerts](../../docs/architecture/12-release-and-operations.md)
- [15 I-12/I-13 and §4 — invariant verification](../../docs/architecture/15-invariants-and-testing.md)
