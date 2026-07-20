# 06 — Governance and Guardians

**Status: normative component specification. Supersedes the corresponding sections of BACKEND_PLAN.md/FRONTEND_PLAN.md** (BE §6, §16, §18.3 governance side, §20; the guardian and values rows of §5.1/§5.2.8, §21, §22). Normative language: RFC 2119. Decisions implemented here: D-5, D-7, D-9 (guardian side), D-13 (filter placement), D-18 (adjudication track, attestor registry), plus the disposition rows for B-11, B-13, B-17 (guardian side), B-19, force_rerun, EmergencyPlaybook enumeration, SafetyFilter, and slot monopolization (see [00-decision-record.md](./00-decision-record.md)).

**Boundary.** This document owns: the values layer (tracks, scope discipline, entrenchment path, ratification, oracle adjudication), runtime origins and call filters, the authority matrix, intake economics, `pallet-guardian` (membership, powers, playbooks), and the kernel-attestor registry. It references, and does not restate: decision-engine pseudocode and state-machine transition numbers ([05](./05-welfare-and-decision-engine.md)), execution-guard dispatch mechanics, the upgrade path and the expedited CODE lane ([09](./09-execution-upgrades-and-rollout.md)), bond/slash economic sizing ([08](./08-treasury-and-economics.md)), the frozen chain↔frontend contract ([02](./02-integration-contract.md)), the governance/operator screens ([11](./11-frontend-workflows.md)), and all parameter values ([13](./13-parameters.md)). Values quoted here for readability are marked *(normative value: [13](./13-parameters.md))*.

---

## 1. The two-layer constitution, restated

- The **values layer** (`pallet-referenda` + `pallet-conviction-voting` over VIT, six narrowly scoped tracks) controls only: welfare-metric definitions and weights, entrenched-floor tightening, constitutional-registry amendments within immutable meta-bounds, guardian **and attestor** election/recall, playbook-registry ratification and renewal, ratification of META/upgrade adoptions, guardian retrospective review, and terminal oracle adjudication. It can never enact operational proposals.
- The **beliefs layer** decides **five** proposal classes — PARAM, TREASURY, CODE, META, CONSTITUTIONAL (values-side routing) — through the epoch machine of [05](./05-welfare-and-decision-engine.md). `ProposalClass::Emergency` is **deleted** (D-7, §6.1 below).
- **Guardians** are a bonded, values-elected, subtractive safety council: they can pause, delay, force a re-run, activate pre-ratified playbooks, and suspend execution under a hard-gate breach — and nothing else (§5).

The scope split is structural (I-8): each track origin passes a per-track `Contains<RuntimeCall>` filter enumerating admissible calls; `ConstitutionalValues` cannot invoke treasury spends, PARAM/META parameter keys, market calls or execution-guard calls other than `ratify` — such a referendum fails at dispatch. Conversely, belief-class proposals whose resource domains include values-scope domains are cancelled at screening. Neither layer can reach the other's scope.

---

## 2. Values layer

### 2.1 Tracks (reconciled six-track table)

All curve/deposit/duration values are simulation hypotheses *(normative values: [13](./13-parameters.md))*; changes from the superseded BE §16.1 are **bold**.

| Track          | Origin produced      | Scope (admissible calls, exhaustive)                                                                                                          | Deposit    | Prepare/decision/confirm | Approval / support curves          | Enactment delay                                        |
| -------------- | -------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- | ---------- | ------------------------ | ---------------------------------- | ------------------------------------------------------ |
| `metric`       | ConstitutionalValues | `welfare.register_spec`, `welfare.activate_spec`, metric weights within kernel bounds                                                          | 10,000 VIT | 2 d / 14 d / 2 d         | linear 60%→50% / reciprocal 10%→2% | 14 d (+ activation ≥ 2 epochs)                         |
| `constitution` | ConstitutionalValues | `constitution.amend_registry` within meta-bounds; floor tightening; `constitution.set_release_channel` (canonical repoint / `min_supported_version` bump / key revocation — [02](./02-integration-contract.md) §12) | 25,000 VIT | 2 d / 21 d / 3 d         | 67% / 15%→5%                       | 28 d                                                   |
| `entrenched`   | ConstitutionalValues | floor loosening; guardian-scope change; kernel-adjacent registry entries                                                                       | 50,000 VIT | 7 d / 28 d / 7 d         | **80% / 20%→10%**                  | 4 epochs                                               |
| `guardian`     | ConstitutionalValues | `guardian.set_members`, recall (`guardian.recall(action_id)`, §5.4), playbook-registry ratification (`guardian.set_playbook_registered`, §6.2), **`attestor.set_members` / attestor recall (§7), `guardian.renew_playbook` (§6.3)** | 5,000 VIT  | 1 d / 7 d / 1 d          | 55% / 5%                           | 2 d                                                    |
| `ratify`       | ConstitutionalValues | **`execution_guard.ratify(proposal_id, referendum_index)`** (§2.4); **`guardian.ratify_action(action_id)`**, **`guardian.uphold_veto(action_id)`** (delay reviews only, §5.4)                             | 1,000 VIT  | 1 d / 7 d / 1 d          | 50% / 5%                           | **immediate — the only deadline is at `execute()` (§2.2)** |
| `oracle`       | OracleResolution     | `oracle.adjudicate(round_id, verdict)` only                                                                                                    | 5,000 VIT  | **0 / 7 d / 1 d**        | **60% / 10%→3%**                   | immediate                                              |

Conviction locking via `pallet-conviction-voting` (1×–6×, locks up to 32 weeks) remains the native ve-analogue. `pallet-preimage` stores referendum calls; `pallet-scheduler` enacts with the track origin and re-enters all filters at dispatch (§3.4).

**Expedited ratify schedule (D-9 hook).** While `PB-LEDGER-FREEZE` is active (§6.3), the `ratify` track admits the expedited schedule (prepare 0 / decision 3 d / confirm 12 h) **only** for the ratification referendum of a proposal in the expedited CODE lane. Lane admissibility, gate-market compression and guard mechanics are owned by [09](./09-execution-upgrades-and-rollout.md); this table is the track-side hook.

### 2.2 Ratification of CODE/META adoptions (D-5; resolves B-11)

The superseded §18.3(2) named two deadlines ("checked at queue time and again at execute"). There is now exactly **one**:

> **R-1 (normative).** A proposal that requires values ratification MUST have a Passed ratification referendum recorded on-chain **at the moment `execute(pid)` dispatches**. No ratification check exists at queue time; queue admission never blocks on the referendum.

**Which proposals require ratification.** (a) Every proposal whose committed payload contains `system.authorize_upgrade` (all runtime upgrades, CODE or META class); (b) every META proposal flagged *rule-altering* at classification (payload touches `constitution.set_capability`, `constitution.amend_registry`, `constitution.set_param` on a META+values-class key — the values half of the [13](./13-parameters.md) §1 dual-consent rows — `market.set_template`, or `oracle.set_config`). The flag is machine-derived from the call-domain classification at screening, never proposer-declared.

**Flow.**

1. The artifact hash (`payload_hash`, plus the Wasm/metadata/attestation commitment set for upgrades) is committed at proposal submission and immutable thereafter. The ratification referendum MAY therefore be submitted on the `ratify` track **any time after the artifact-hash commitment** — during Intake, Trading, or (typically) the timelock — and runs concurrently with the market process. Its preimage is exactly `execution_guard.ratify(proposal_id, referendum_index)`.
2. When the referendum passes, the scheduler dispatches `ratify` with `ConstitutionalValues` origin (the `ratify` track's `Contains` filter admits only this call, `guardian.ratify_action`, and `guardian.uphold_veto` — §5.4). The execution guard records `Ratifications[pid] = RatificationRecord { referendum_index, payload_hash, ratified_at }`. Dispatch against a pid whose `payload_hash` no longer exists, or a duplicate ratification, errors benignly.
3. `execute(pid)` performs the ratification check as part of its dispatch-time re-validation (guard step 4; mechanics owned by [09](./09-execution-upgrades-and-rollout.md)): if the proposal requires ratification and no `RatificationRecord` exists, the extrinsic fails and the proposal **remains Queued** — keepers MAY retry until `grace_end`.
4. If `grace_end` passes without ratification, the next `tick` transitions the proposal to `Rejected(NotRatified)` with bond refunded. `RejectReason::NotRatified` is a new variant of the canonical enum (type owned by [02](./02-integration-contract.md)/[05](./05-welfare-and-decision-engine.md)).

**Decision-table coordination.** [05](./05-welfare-and-decision-engine.md) owns the `decide()` pseudocode and the §14.2-equivalent reason-code table; it adds (i) the attestation-presence check (§7) to the pseudocode and (ii) this row, stated here normatively:

| Scenario                                                                | Check point                | Outcome                                                        |
| ----------------------------------------------------------------------- | -------------------------- | --------------------------------------------------------------- |
| Ratification-requiring proposal, referendum Passed before dispatch      | guard step 4 (execute)     | dispatch proceeds                                               |
| Referendum not yet Passed at an `execute` attempt                       | guard step 4 (execute)     | extrinsic errors `NotYetRatified`; stays Queued; retry in grace |
| `grace_end` reached, no ratification recorded                           | `tick`                     | `Rejected(NotRatified)`; bond refunded                          |

A `Suspended → Rerun` cycle (delay-once) or a `force_rerun` (§5.3) does **not** invalidate an existing ratification: the record binds `(pid, payload_hash)` and both are unchanged across reruns. Ratification records are reaped with the proposal.

### 2.3 Oracle adjudication track hardening (D-18; resolves B-19)

Terminal oracle adjudication fixes money-settling facts for whole cohorts; it no longer rides the weakest track. Three changes:

1. **Thresholds raised**: 60% approval, 10% support (decaying to 3%), 7-day decision *(normative values: [13](./13-parameters.md))*. The oracle latency-cap arithmetic in [07](./07-oracle-and-disputes.md) is recomputed against the 7-day terminal round.
2. **Pre-cohort conviction snapshot.** The tally for an `OracleResolution` referendum concerning a dispute whose subject is cohort *e* weights each vote by `min(current conviction-locked VIT, conviction-locked VIT as of SnapshotBlock(e))`, where `SnapshotBlock(e)` is the block at which cohort *e* was created (recorded by `pallet-epoch` as `CohortSnapshotBlock: map EpochId → BlockNumber`, bounded by the ≤ 4 live cohorts). **Capital that entered, or locked, after the disputed cohort began therefore has zero incremental weight in its adjudication** — buying VIT after seeing the dispute cannot buy the verdict. Locks reduced or unlocked since the snapshot count at their current (lower) value, so exiting always costs weight and never adds it.
3. The referendum's admissible call remains exactly `oracle.adjudicate(round_id, verdict)`; the round id binds the subject cohort, from which the tally engine derives the snapshot block.

**[VERIFY]** whether the stock `pallet-conviction-voting`/`pallet-referenda` pair on `polkadot-stable2606` can be configured with an external per-poll voting-power provider (custom `Polling`/`Tally` impl) without forking. Fallback (design default if not): a vendored conviction-voting instance for the `oracle` track only, sharing lock storage read-only.

### 2.4 Entrenchment path (carried forward)

Loosening θ⁻ floors, weakening the VOID rule, expanding guardian scope, or amending the entrenchment rule itself requires the `entrenched` track (80% approval, 20% support, 4-epoch enactment during which Baseline markets for post-activation epochs already trade — the priced second opinion) **and** a passing META market. Both layers must independently consent. Tightening the same floors is ordinary CONSTITUTIONAL-class business.

---

## 3. Runtime origins, call filters and authority matrix

### 3.1 Custom origins

```rust
#[pallet::origin] // in the `pallet-origins` shim crate
pub enum Origin {
    /// Produced by pallet-execution-guard when executing a passed proposal of the given class.
    FutarchyParam,
    FutarchyTreasury,
    FutarchyCode,
    FutarchyMeta,
    /// Produced by the values referenda tracks (via pallet-referenda track origins).
    ConstitutionalValues,        // metric registry, constitutional registry, elections, ratification
    OracleResolution,            // terminal oracle adjudication only
    /// Produced by pallet-guardian on 5-of-7 approval, scoped per power.
    GuardianHold,                // pause-intake / delay-once / force-rerun / gate-suspend
    EmergencyPlaybook,           // enumerated pre-ratified playbook dispatch only (§6.2)
}
```

Note: `ProposalClass::Emergency` is deleted (D-7); the `EmergencyPlaybook` **origin** remains — it is produced by playbook activation, not by a proposal class, and there is no fifth `Futarchy*` origin. Every origin is produced by exactly one pallet through exactly one code path; none is obtainable from a signed extrinsic, XCM origin conversion, or wrapper call. There is **no path to unrestricted Root**: `EnsureRoot` succeeds only for the internal dispatch the execution guard performs for the single allowlisted `system.authorize_upgrade(committed_hash)` call, constructed inside the runtime (I-10).

**Internal `EnsureOrigin`s** (pallet-to-pallet, not `RuntimeOrigin` variants): `MarketAuthority` = pallet-market only; `ResolveAuthority` = pallet-epoch only (drives `ledger.resolve` and `ledger.void`); `SettleAuthority` = **pallet-welfare only**, reachable through exactly one path — `pallet-epoch::settle_cohort → pallet-welfare::compute_settlement → ledger` (the superseded §6.1's welfare/oracle ambiguity is resolved per [05](./05-welfare-and-decision-engine.md); the oracle pallet feeds welfare, it never touches the ledger).

### 3.2 Authority matrix (call-level capability table, normative)

| Call domain (examples)                                                                                                                                                                                                              | FutarchyParam         | FutarchyTreasury | FutarchyCode | FutarchyMeta                | ConstitutionalValues | GuardianHold | EmergencyPlaybook | OracleResolution | Signed |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------- | ---------------- | ------------ | --------------------------- | -------------------- | ------------ | ----------------- | ---------------- | ------ |
| `constitution.set_param` (in-bounds keys of class PARAM)                                                                                                                                                                            | ✔                     | —                | —            | —                           | —                    | —            | —                 | —                | —      |
| `futarchy_treasury.{spend, open_stream, cancel_stream, fund_budget_line}`, `constitution.set_param` (TREASURY-class keys: `pol.b*`, `ops.*`)                                                                                         | —                     | ✔                | —            | —                           | —                    | —            | —                 | —                | —      |
| `system.authorize_upgrade(hash)` (committed CODE artifact)                                                                                                                                                                          | —                     | —                | ✔            | —                           | ratify at execute (§2.2) | —        | —                 | —                | —      |
| `constitution.set_param` (META and META+values keys), `constitution.set_capability`, `constitution.amend_registry` (non-kernel rows, within meta-bounds — [13](./13-parameters.md) rule 7), `market.set_template`, `oracle.set_config`, `registry.set_config` ([07](./07-oracle-and-disputes.md)) | —                     | —                | —            | ✔                           | ratify where rule-altering (§2.2) | — | —                 | —                | —      |
| `welfare.register_spec/activate_spec`, `constitution.amend_registry` (within kernel bounds), `constitution.set_release_channel` ([02](./02-integration-contract.md) §12), `guardian.set_members`, `attestor.set_members` (§7), entrenched-floor tighten | —                     | —                | —            | —                           | ✔                    | —            | —                 | —                | —      |
| `execution_guard.ratify`, `guardian.ratify_action`, `guardian.uphold_veto`, `guardian.recall`, `guardian.renew_playbook`, `guardian.set_playbook_registered`                                                                        | —                     | —                | —            | —                           | ✔ (ratify/guardian tracks only) | — | —                 | —                | —      |
| `guardian.{pause_intake, delay_once, force_rerun, suspend_on_gate}`                                                                                                                                                                 | —                     | —                | —            | —                           | —                    | ✔            | —                 | —                | —      |
| `guardian.activate_playbook(id)` → enumerated preimage dispatch per §6.2                                                                                                                                                            | —                     | —                | —            | —                           | —                    | —            | ✔                 | —                | —      |
| `oracle.adjudicate(round, verdict)`                                                                                                                                                                                                 | —                     | —                | —            | —                           | —                    | —            | —                 | ✔                | —      |
| `registry.{file_incident, file_milestone, challenge}` (bonded; [07](./07-oracle-and-disputes.md))                                                                                                                                   | —                     | —                | —            | —                           | —                    | —            | —                 | —                | ✔      |
| `attestor.{attest, challenge_attestation}` (bonded; §7)                                                                                                                                                                             | —                     | —                | —            | —                           | —                    | —            | —                 | —                | ✔      |
| `system.apply_authorized_upgrade(code)` (permissionless; `DescriptorLeadTime` filter-gated per [09](./09-execution-upgrades-and-rollout.md) §2.2), `execution_guard.apply_authorized_upgrade(code)` (same guarded application path) | —                     | —                | —            | —                           | —                    | —            | —                 | —                | ✔      |
| `execution_guard.expire_failed_execution(pid)` (permissionless T22 keeper crank, [09](./09-execution-upgrades-and-rollout.md) §1.2)                                                                                                | —                     | —                | —            | —                           | —                    | —            | —                 | —                | ✔      |
| `execution_guard.reject_stale(pid)` (permissionless deterministic queue-cleanup crank, [09](./09-execution-upgrades-and-rollout.md) §1.2)                                                                                           | —                     | —                | —            | —                           | —                    | —            | —                 | —                | ✔      |
| `epoch.submit/withdraw`, `market.buy/sell/crank_observe`, `ledger.split/…/redeem/redeem_void`, `epoch.tick/decide/settle_cohort`, `welfare.record_snapshot/record_daily_gate`, `execution_guard.execute`, `oracle.report/challenge`, `referenda.*`, `conviction_voting.*`        | —                     | —                | —            | —                           | —                    | —            | —                 | —                | ✔      |
| `system.set_code`, `set_code_without_checks`, `set_storage`, `kill_storage`, `kill_prefix`, `authorize_upgrade_without_checks`, `pallet_xcm.{force_*, send}`, asset `force_*`                                                       | **nobody** (filtered **from genesis**, all origins **including sudo** — D-13) |  |  |  |  |  |  |  |  |

Changes from the superseded §6.2: the frame-system "nobody" row is enforced **from genesis**, not "post-bootstrap" (D-13; the Phase 0–3 sudo key cannot reach these calls either — enforcement mechanics in §3.3); `registry.*` rows added for the new `pallet-registry` ([07](./07-oracle-and-disputes.md)); `attestor.*` rows added (§7); the ratification calls row added (§2.2); `ledger.redeem_void` added to the user row (D-1, [03](./03-conditional-ledger.md)); the SettleAuthority path pinned per §3.1; the permissionless `welfare.record_snapshot/record_daily_gate` keeper cranks added to the Signed row (A7: snapshot/daily-gate recording is keeper-driven from deterministic on-chain component reads, [05](./05-welfare-and-decision-engine.md) §4.6/§4.7 — the component values are runtime-sourced, so a keeper only triggers the recording, exactly like `epoch.tick`; `welfare.compute_settlement` stays the internal SettleAuthority path of §3.1, never an extrinsic); all `Emergency`-class references removed (D-7).

`epoch.submit` for CONSTITUTIONAL-class subjects routes to the values track (it is a referendum, not a market); the epoch pallet rejects values-scope resource domains from belief-class submissions and vice versa (§1, I-8).

**`PhaseFlags` writer map (bit assignments owned by [02](./02-integration-contract.md) §7.3).** Bits 0–4 (shadow/arming/sudo-present) are written by bootstrap sudo via `constitution.set_phase_flag` (Root-only, restricted to exactly these bits) while sudo exists per [09](./09-execution-upgrades-and-rollout.md) §5.4, and by phase-advancement upgrades thereafter ([09](./09-execution-upgrades-and-rollout.md) §5.2). Bits 5–7 are machinery state, written only through the constitution's bit-specific runtime-internal setters by their owning wiring: bit 5 by the PB-LEDGER-FREEZE playbook path (§6.3), bit 6 by the dead-man switch ([13](./13-parameters.md) §2), bit 7 by the oracle reserve probe ([07](./07-oracle-and-disputes.md) §8). No origin — including Root — can dispatch a machinery bit.

### 3.3 `BaseCallFilter`, the closed wrapper set, and G-5 (resolves SafetyFilter medium)

`BaseCallFilter = SafetyFilter`, a custom `Contains<RuntimeCall>` that (a) denies the "nobody" row unconditionally, (b) denies governance-privileged calls unless dispatched with their matching custom origin — enforced again by each pallet's `EnsureOrigin`, giving two independent checks — and (c) recursively inspects **every** wrapper call. The wrapper set is now **closed**: the table below enumerates every `RuntimeCall` variant in the runtime that can carry another call, and its treatment. Adding a call-carrying variant to the runtime without a row here MUST fail the filter-exhaustiveness CI test (I-8/I-10 test row).

| Wrapper                                                                 | Treatment                                                                                                                       |
| ------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------- |
| `utility.batch`, `utility.batch_all`, `utility.force_batch`             | recurse into every inner call; ≤ `MAX_NESTED = 4` levels, ≤ 16 calls total                                                       |
| `utility.dispatch_as`, `utility.as_derivative`                          | **denied entirely** for external origins                                                                                         |
| `utility.with_weight`                                                   | Root-only upstream (unreachable externally); recursed anyway, defense in depth                                                   |
| `proxy.proxy`                                                           | denied if inner call is privileged-domain; else recurse                                                                          |
| `proxy.proxy_announced`                                                 | **now recursed identically to `proxy.proxy`** (previously unhandled — the filter was bypassable)                                 |
| `multisig.as_multi`                                                     | denied if privileged-domain; else recurse                                                                                        |
| `multisig.as_multi_threshold_1`                                         | **now recursed identically to `as_multi`** (previously unhandled)                                                                |
| `multisig.approve_as_multi`                                             | carries only a call hash, dispatches nothing; the terminal `as_multi` that dispatches is recursed ⇒ closed                       |
| `scheduler.*`                                                           | admissible only for the values-enactment set with track origin captured at scheduling (§3.4); all other scheduling denied        |
| `sudo.sudo`, `sudo.sudo_unchecked_weight`, `sudo.sudo_as` (Phases 0–3)  | recursed: an inner call in the "nobody" row is denied **at the outer extrinsic**, because sudo's inner dispatch bypasses the filter (D-13); the sudo pallet itself is removed at Phase 3→4 |

```rust
impl Contains<RuntimeCall> for SafetyFilter {
    fn contains(c: &RuntimeCall) -> bool {
        match c {
            RuntimeCall::Utility(pallet_utility::Call::batch { calls })
            | RuntimeCall::Utility(pallet_utility::Call::batch_all { calls })
            | RuntimeCall::Utility(pallet_utility::Call::force_batch { calls })
                => calls.len() <= MAX_NESTED_TOTAL && calls.iter().all(Self::contains),
            RuntimeCall::Proxy(pallet_proxy::Call::proxy { call, .. })
            | RuntimeCall::Proxy(pallet_proxy::Call::proxy_announced { call, .. })
            | RuntimeCall::Multisig(pallet_multisig::Call::as_multi { call, .. })
            | RuntimeCall::Multisig(pallet_multisig::Call::as_multi_threshold_1 { call, .. })
                => !is_privileged_domain(call) && Self::contains(call),
            RuntimeCall::Utility(pallet_utility::Call::dispatch_as { .. })
            | RuntimeCall::Utility(pallet_utility::Call::as_derivative { .. }) => false,
            #[cfg(feature = "bootstrap")] // compiled out at the Phase-3→4 upgrade
            RuntimeCall::Sudo(sudo_call) => sudo_inner(sudo_call).map_or(true, Self::contains),
            RuntimeCall::Scheduler(..) => scheduled_inner_allowed(c), // values-enactment set only
            _ => static_domain_allowed(c),
        }
    }
}
```

Nesting depth is bounded (4 levels, ≤ 16 calls total, matching the payload bounds), so filter evaluation weight is bounded.

**G-5 restated (no-escalation guarantee).** Every privileged effect flows through an enumerated custom origin produced by an enumerated pallet; **no composition of `utility` batch variants, `proxy` variants (`proxy`, `proxy_announced`), `multisig` variants (`as_multi`, `as_multi_threshold_1`, `approve_as_multi`), scheduler agendas, sudo wrappers (Phases 0–3), or XCM messages can produce a custom governance origin or Root, or reach a call in the "nobody" row.** Escalation through `utility.dispatch_as` is prevented by filtering that call entirely; through XCM `Transact` by the barrier refusing `Transact` from all locations ([09](./09-execution-upgrades-and-rollout.md)); through the scheduler because scheduled dispatch re-enters origin checks with the origin captured at scheduling. Enforced by the negative-test suite over all wrapper compositions (I-10/I-11).

### 3.4 Why the scheduler cannot bypass revalidation (carried forward)

The scheduler is used solely as `pallet-referenda`'s enactment engine. A scheduled values call dispatches with `ConstitutionalValues`/`OracleResolution` origin and passes the same `SafetyFilter` + `EnsureOrigin` + in-pallet precondition checks at *dispatch* time as a direct call would. Belief-side execution never touches the scheduler: `pallet-execution-guard::execute` is the only path, precisely so that maturity, preimage, version, capability, rate-limit, ratification (§2.2) and attestation (§7) checks are repeated at dispatch ([09](./09-execution-upgrades-and-rollout.md)).

---

## 4. Intake economics (resolves B-13, slot monopolization)

These rules live with the intake flow; the sizing analysis showing they price the griefing levers is owned by [08](./08-treasury-and-economics.md). All fractions and counts below: *(normative values: [13](./13-parameters.md))*.

1. **Preimage pinning at qualification.** At `Screening → Qualified` (T5), `pallet-epoch` calls `request_preimage(payload_hash)` (via the `QueryPreimage`/`StorePreimage` consumer API), pinning the committed payload against unnoting for the proposal's whole lifetime. The request is released on every terminal state (Cancelled, Rejected, Expired, Settled reap). Any keeper MAY re-note a missing public preimage; pinning makes the sabotage window structurally empty rather than merely recoverable.
2. **Preimage-missing cancellation slashes 10%.** T4's disposition for a missing/oversized preimage changes from full refund to **90% refund, 10% of the class bond slashed** to the INSURANCE account. Filling the intake queue with preimage-less proposals now costs `0.10 × Σ bonds`, not just fees — TM-15's "bonds price entries" becomes true for this path.
3. **Non-decision-grade outcomes slash 10%.** A proposal terminating `Rejected(NotDecisionGrade)` (including after an exhausted extension) forfeits 10% of its bond to INSURANCE. Decision-grade rejections (`HurdleNotMet`, `GateVeto*`, `ConvergenceFailed`, …) continue to refund 100% — rejection on information remains free; consuming a slot without ever producing a decision-grade signal does not.
4. **Per-account intake rate limit.** ≤ **4** intake entries per epoch per account, enforced at `epoch.submit` (`IntakeEntriesPerAccount: map (EpochId, AccountId) → u8`; excess fails `IntakeRateLimited`; map reaped at epoch close). Sybil-splitting across accounts still multiplies bond capital at risk under rules 2–3; the residual is a threat-model row in [14](./14-threat-model.md).
5. Unchanged: `IntakeQueue = 64` (pre-qualification only, per D-10), class bonds per [13](./13-parameters.md), 100% slash on constitution violation or false resource declaration, full refund on withdrawal before qualification.

Events: `IntakeSlashed { pid, reason, amount }` accompanies every partial slash.

---

## 5. Guardians (resolves B-17 guardian side, force_rerun medium)

### 5.1 Membership, bonds, approval

Membership: **7 accounts** elected on the `guardian` track; per-member bond **50,000 VIT** *(normative value: [13](./13-parameters.md))* held for the full term plus one epoch; approval threshold **5-of-7**, aggregated inside `pallet-guardian` (not `pallet-collective`, to keep the power surface enumerated). Action flow: any member calls `guardian.propose_action(power, target, justification_hash)`; members call `guardian.approve_action(action_id)`; the fifth approval dispatches with `GuardianHold` (or `EmergencyPlaybook` for activations) atomically. Proposals expire un-dispatched after 3 days.

### 5.2 Powers (exhaustive; kernel-scoped)

| Power                   | Bound                                                                                                                                                                                                                     | Allowance       |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------- |
| `pause_intake`          | ≤ 14 days per activation; affects intake only — live trading windows always run to finalization (subtractive asymmetry)                                                                                                    | 1 per 4 epochs  |
| `delay_once(pid)`       | one queued proposal, once ever; auto-schedules a Rerun at 2× POL and δ+1 pp; rerun outcome final and undelayable                                                                                                            | 2 per epoch     |
| `force_rerun(pid)`      | full definition in §5.3; pre-execution only; one per proposal ever                                                                                                                                                          | 1 per epoch     |
| `activate_playbook(id)` | only while that playbook's verified on-chain trigger is active (§6.2); playbooks are preimage-committed enumerated call batches, values-ratified in advance, each with scope and expiry                                     | per-playbook    |
| `suspend_on_gate`       | freeze the execution queue while a hard-gate daily breach flag is active; auto-releases when the flag clears                                                                                                                | condition-gated |

**Kernel prohibitions (unchanged, entrenched):** guardians cannot enact proposals, move funds, alter market outcomes or settled prices, install code, modify the constitution, or extend their own authority. Guardian scope is a kernel constant; expanding it is an `entrenched`-track decision (§2.4).

### 5.3 `force_rerun(pid)` — full definition (resolves the force_rerun medium)

Previously "one market rerun where telemetry anomaly is flagged" with no state-machine integration. Now:

- **Target.** A proposal, not a single market: the action resets **all** of the proposal's books (decision pair + gate books). The epoch's Baseline book is never a target (its manipulation surface is priced into every decision and guarded by the κ/TWAP package, [04](./04-markets-and-pricing.md)).
- **Admissibility.** 5-of-7, allowance 1/epoch, and **pre-execution only**: the proposal MUST be in `Trading`, `Extended`, or `Queued`. After `execute()` has dispatched — or for a proposal already in a rerun of either kind — the call errors. At most **one guardian rerun of either kind (delay-once rerun or force_rerun) per proposal, ever**, preserving ADR-13's "≈ 1 cycle obstruction" bound.
- **Effects (atomic):**
  1. If `Queued`: the execution-queue entry is cancelled and the timelock voided; the proposal re-enters `Extended`. If `Trading`/`Extended`: it moves to (or stays in) `Extended` with a fresh window.
  2. **TWAP reset**: all accumulators and window checkpoints of the proposal's books are zeroed and restarted; the decision statistic for the re-run is computed **only** from the new window (full window = the Extended window; trailing window = its final 24 h).
  3. **Books reopen** for a 3-day Extended window (`dec.extension` = 43,200 blocks *(normative value: [13](./13-parameters.md))*). POL stays in place; the undisturbed-POL decision-grade condition applies to the new window.
  4. **Positions intact**: no ledger operation of any kind — no mint, burn, void, or transfer. Traders keep exactly their holdings; the vault is untouched.
  5. **One decide re-run**: at the window's close, `decide(pid)` runs once. The outcome is final — no further extension (`p.extended` is set), no second guardian rerun; `delay_once` is also inadmissible against the re-run outcome. An adopt re-queues with a fresh maturity; ratification and attestation records survive (§2.2).
- **State-machine hooks** (transition numbers and pseudocode owned by [05](./05-welfare-and-decision-engine.md)): `Queued → Extended` (guardian force-rerun), `Trading/Extended → Extended` (window reset), then the ordinary decide transitions; [05](./05-welfare-and-decision-engine.md)'s T13-family rule "rerun re-enters Extended (3 days) then decides" covers both rerun kinds.
- **Events**: `ForceRerun { pid, justification_hash, window_end }`.
- Like every guardian action, a force-rerun consumes allowance and schedules mandatory retrospective ratification (§5.4).

### 5.4 Review, ratification, recall, sunset

Every guardian action emits `GuardianAction { action_id, power, target, justification_hash }`, consumes its allowance, and auto-schedules a `ratify`-track retrospective review: `pallet-guardian` submits the referendum (preimage `guardian.ratify_action(action_id)`) with **both** its submission deposit and its decision deposit (the §2.1 `ratify` deposit) fronted pro-rata from the approving members' held bonds and refunded on ratification. The seat bonds are real `fungible` holds on VIT (§5.1), so fronting temporarily releases each approver's slice and ratification restores it. For `delay_once` actions the pallet submits **both verdict referenda at scheduling** — `guardian.ratify_action(action_id)` and `guardian.uphold_veto(action_id)`, deposits for both fronted the same way — so the upheld-veto verdict is on the ballot from the start rather than racing an ad-hoc submission; enacting either verdict atomically cancels the other referendum and refunds its fronted deposits.

The review concludes in exactly one of three ways:

1. **Ratified** — the referendum enacts `guardian.ratify_action(action_id)` (`ratify` track): the action is marked ratified and the fronted deposits return to the approvers' bonds.
2. **Veto upheld** (`delay_once` actions only) — the referendum enacts `guardian.uphold_veto(action_id)` (same `ratify` track): admissible only while the delayed proposal is still `Suspended` (before T12/T13 opens the rerun). The action is simultaneously marked ratified — the veto *is* the review's verdict, the values layer agreeing with the delay — deposits are refunded, and the proposal transitions T24 (`Rejected(VetoUpheldByReview)`, [05](./05-welfare-and-decision-engine.md) §2.1). This is `VetoUpheldByReview`'s single producing site.
3. **Failed** — neither call has enacted within `grd.review_dl` = **2 epochs** of the action *(normative value: [13](./13-parameters.md))*: each approving member is slashed **50% of bond**, with slash proceeds transferred to the treasury MAIN account ([08](./08-treasury-and-economics.md) §1) net of the recall referendum's deposits, which are fronted from them; `ReviewFailed` is emitted; and a recall referendum is auto-scheduled on the `guardian` track with preimage `guardian.recall(action_id)`. Failure of recall *scheduling* never erases the slash or the `ReviewFailed` record.

**Recall semantics.** `guardian.recall(action_id)` (a `guardian`-track values decision) removes every approving member of the failed action from the guardian set: their live approvals are cleared, their seats stay vacant until a subsequent `guardian.set_members` (same track) refills them, and each removed member's remaining bond stays held for one further epoch and is then released (§5.1's "term plus one epoch"). The 5-of-7 approval threshold is anchored at the full seat count, so vacancies only ever reduce guardian capacity — fewer than 5 seated members means no guardian action can dispatch until the values layer refills the set (fail-closed).

**Ordering with the two-epoch deadline.** An uphold-veto is bounded by the rerun opening (the next epoch's Seed phase, [05](./05-welfare-and-decision-engine.md) §2.1 T12/T13), which always precedes the two-epoch ratification deadline; the deadline governs slashing and recall only. The `ratify` track's 1 d / 7 d / 1 d schedule with immediate enactment (§2.1) fits multiple submission attempts inside two 21-day epochs by construction.

Sunset: allowances step down at rollout milestones (Phase 6 → delay-once + playbooks only; Phase 7 → playbooks only), enforced as phase-keyed constants; renewal or further reduction is an `entrenched`-track decision.

---

## 6. Emergency playbooks (implements D-7, D-9)

### 6.1 The Emergency class is deleted (D-7)

`ProposalClass::Emergency` had no bond, markets, state-machine path, or decision rule, and never could: emergencies are handled by guardian playbooks, which is what the design already did in practice. The variant is removed from the canonical enum ([02](./02-integration-contract.md)), the classifier ([05](./05-welfare-and-decision-engine.md)) is exhaustive over five classes and its ADR-3 completeness obligation is now satisfiable, and the per-class rows for Emergency in the parameter table are deleted ([13](./13-parameters.md)). The `Emergency::restricted(class)` hold in the decision pseudocode is replaced by the playbook/hold predicates ([05](./05-welfare-and-decision-engine.md)). **Playbooks are the emergency mechanism**, full stop: pre-audited, preimage-committed, values-ratified in advance, trigger-gated, scoped, and expiring.

### 6.2 Playbook registry and enumerated capability table (resolves the EmergencyPlaybook-enumeration medium)

The `EmergencyPlaybook` origin can dispatch **only** the calls enumerated below — this table is the §3.2-equivalent capability row for that origin, expanded per playbook. A playbook whose preimage contains any call outside its row fails registry ratification (checked mechanically against the call-domain classifier at registration); the SafetyFilter and each pallet's `EnsureOrigin` re-check at dispatch. Registering or amending a playbook is a `guardian`-track values decision.

| Playbook            | Verified on-chain trigger (activation admissible only while set)                    | Admissible call set (exhaustive)                                                                                          | Expiry                                        |
| ------------------- | ------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------- |
| `PB-DEPEG`          | attested 30-day-median depeg trigger (> 2% for 24 h; [07](./07-oracle-and-disputes.md)) | `market.freeze_creation(expiry)` — new-market creation only; open markets unaffected                                        | ≤ 1 epoch, renew by re-activation while triggered |
| `PB-MIGRATION`      | `pallet-migrations` halt-and-alarm flag                                              | **no guardian-dispatchable call** on stable2606 — `pallet-migrations`' cursor controls are Root-only, filtered to the D-13 "nobody" row, so the runtime's `playbook_calls(Migration)` constructs none (SQ-274/280); the freeze is the automatic `MigrationHalt` halt-source bridge (execution-queue) and retry/rollback ride the ratified expedited-CODE lane ([09](./09-execution-upgrades-and-rollout.md) §3.2); never `set_storage`-class calls | until migration resolves                        |
| `PB-ORACLE-VOID`    | oracle deadlock / gate-input failure flag ([07](./07-oracle-and-disputes.md))         | `epoch.void_cohort(epoch_id)` → drives `ledger.void(pid)` for the cohort's vaults via ResolveAuthority ([03](./03-conditional-ledger.md)) | one-shot                                        |
| `PB-HALT-INTAKE`    | gate breach flag, dead-man, or VOID in flight                                        | `epoch.set_intake_paused(true, expiry)`                                                                                     | ≤ 14 days                                       |
| `PB-RESERVE`        | reserve-health trigger R (deterministic C_onchain sub-metric; [07](./07-oracle-and-disputes.md)) | `ledger.set_split_paused(true, expiry)` — halts split **inflows** only; merge/redeem/exit paths stay open                   | ≤ 14 days                                       |
| `PB-LEDGER-FREEZE`  | **I-4 drift flag** (machine-checked solvency anomaly; §6.3)                          | `ledger.set_frozen(true)` + `market.set_frozen(true)` (§6.3)                                                                | ≤ 14 days + one renewal (§6.3)                  |

Every activation emits `PlaybookActivated { id, trigger, expiry }`, consumes the per-playbook allowance, writes a review record, and schedules mandatory retrospective ratification (§5.4). Expiry emits `PlaybookExpired { id }` and reverts the playbook's effects automatically.

**Substrate (B1b).** The six playbooks' effect batches are **kernel-enumerated runtime routines implementing exactly this table** — each activation constructs its row's calls with activation-time arguments (target cohort, `expiry` = activation block + the row's bound) and dispatches them under the `EmergencyPlaybook` origin; a per-playbook conformance test asserts each routine's constructed calls classify into precisely that row's admissible call set (the "checked mechanically against the call-domain classifier" obligation, discharged structurally). The values-governed registry surface is per-playbook **availability**: `guardian.set_playbook_registered(id, enabled)` (`guardian` track, §2.1), with all six registered at genesis — they are ratified by inclusion in this constitution. A *new* playbook, or an amended row, is therefore a CODE/META change to this table plus its routine, values-ratified like any other; the registry toggle alone can only disable or re-enable one of the six. Effect endpoints are expiry-carrying at the *pallet* level too (`epoch.set_intake_paused`, `market.freeze_creation`, `ledger.set_split_paused` carry `expiry`; the §6.3 freezes carry the kernel 14-day window internally), so a stalled guardian crank can delay an explicit revert but can never extend an effect past its kernel bound.

### 6.3 `PB-LEDGER-FREEZE` (D-9; the emergency brake B-17 lacked)

During an active solvency exploit, the previous design had no actor able to pause ledger/market calls — split/redeem/buy/sell kept running for the entire repair cycle. Now:

- **Trigger (hard precondition).** Activation is admissible **only while the I-4 drift flag is set**: the permissionless ledger reconciliation crank has observed `Σ_pid escrowed(pid) ≠ sovereign balance` beyond the accounted dust/endowment tolerance (flag mechanics owned by [03](./03-conditional-ledger.md)). The flag is machine-checked at activation *and* the freeze auto-lifts early if a subsequent reconciliation clears the flag. Guardians cannot invoke this playbook on judgment alone.
- **Effect.** `ledger.set_frozen(true)` and `market.set_frozen(true)`: **all** ledger calls (split, merge, split_scalar, merge_scalar, transfer, redeem, redeem_scalar, redeem_void) and all market calls (buy, sell, crank_observe) error `Frozen` — both inflow and outflow, because during an unattributed drift either direction can be the exploit. The reconciliation crank, guardian calls, values referenda, and oracle/dispute calls remain live. Decision windows overlapping a freeze fail decision-grade (staleness) and resolve to status quo — G-1 is preserved, never inverted.
- **Expiry.** Auto-expires after ≤ **14 days** (201,600 blocks; kernel constant, *(normative value: [13](./13-parameters.md))*). **One renewal only**, for ≤ 14 further days, via a values referendum on the `guardian` track (`guardian.renew_playbook(PB_LEDGER_FREEZE)` — 7-day decision fits inside the first expiry window). No second renewal exists; a longer freeze means the repair has failed and VOID/recovery paths ([03](./03-conditional-ledger.md), [09](./09-execution-upgrades-and-rollout.md)) take over.
- **Accountability.** Mandatory retrospective ratification per §5.4; every activation, renewal and expiry emits events and writes a review record.
- **Expedited CODE lane (unlocked, not owned, here).** While `PB-LEDGER-FREEZE` is active, the expedited CODE lane of [09](./09-execution-upgrades-and-rollout.md) becomes admissible: 72 h gate market + 3-day fast-track `ratify` referendum (§2.1), executing through the **normal execution guard** — no new privileged origin, no guardian code-installation power. The lane exists so the freeze window is long enough to ship the fix it is buying time for.

---

## 7. Kernel attestation: bonded attestor registry (D-18; replaces presence-only checking)

The superseded regime accepted any 32-byte attestation hash — presence-only. The bonded-game discipline now extends to the highest-stakes action:

- **Registry.** `pallet-attestor` (a thin registry whose shipped storage names and SCALE value shapes are frozen in [02 §7.5](./02-integration-contract.md)): members elected on the `guardian` track (`attestor.set_members`), **≥ 3 members** required before any CODE/META proposal can qualify; per-member bond **25,000 VIT** *(normative value: [13](./13-parameters.md))* held; recall via the same track. Attestors MUST be organizationally disjoint from the founding multisig and from release-key holders ([12](./12-release-and-operations.md) signer-disjointness rule).
- **Attestation.** For a CODE/META artifact, an attestor submits `attestor.attest(pid, artifact_hash, statement_hash)` — a signed, bonded assertion that the candidate Wasm preserves the kernel invariant set, where `statement_hash` commits to a published reproducible-build + invariant-diff report. **Quorum: 2 valid attestations from distinct attestors** ("2-of-3" at minimum registry size; 2-of-N generally).
- **Challenge window.** Each attestation is challengeable for **72 h** (43,200 blocks) from submission by anyone posting a bond ≥ 50% of the attestor bond: `attestor.challenge_attestation(attestation_id, evidence_hash)`. A challenged attestation does not count toward quorum while the challenge is open. Resolution: by deterministic recomputation proof where the dispute is reproducibility (any keeper submits it, mechanically resolving), else by a `ratify`-track adjudication referendum. The losing side forfeits 50% of its bond (attestor additionally ejected on a second adjudicated-false attestation). Dispute-game plumbing follows the oracle pattern of [07](./07-oracle-and-disputes.md).
- **Enforcement points.** Attestation quorum (2 unchallenged-or-upheld attestations whose windows have closed or that survive their open windows unchallenged at dispatch) is (i) a queue-time precondition in `decide()` (pseudocode in [05](./05-welfare-and-decision-engine.md); missing quorum ⇒ `Reject(AttestationMissing)`) and (ii) re-checked by the execution guard at `execute()` alongside ratification (§2.2; guard integration owned by [09](./09-execution-upgrades-and-rollout.md)).
- The honesty clause stands: verifying attestation *content* remains social/off-chain in the limit (A-4) — but a false attestation now costs a bond, a seat, and a public adjudication, instead of nothing.

---

## 8. Backend surface for the canonical frontend (X-2, backend side)

The values layer is served by the canonical frontend (FE-14 governance surface, FE-15 operator surface — [11](./11-frontend-workflows.md)). Everything below is light-client readable/submittable, is part of the frozen contract in [02](./02-integration-contract.md) (this list is the governance section of that contract; [02](./02-integration-contract.md) owns final naming), and nothing here may be indexer-dependent:

**Extrinsics** (all Signed): `referenda.{submit, place_decision_deposit, refund_decision_deposit, refund_submission_deposit}`; `conviction_voting.{vote, remove_vote, remove_other_vote, delegate, undelegate, unlock}`; `preimage.note_preimage` (referendum payloads, incl. `execution_guard.ratify` and `guardian.ratify_action` preimages); `guardian.{propose_action, approve_action}` (the 5-of-7 signing workflow); `attestor.{attest, challenge_attestation}`. The `OracleResolution` ballot is an ordinary `conviction_voting.vote` on the `oracle` track — no extra extrinsic.

**Storage / views**: `referenda.{ReferendumInfoFor, ReferendumCount, TrackQueue, DecidingCount}`; `conviction_voting.{VotingFor, ClassLocksFor}`; `execution_guard.{Ratifications, Queue}` (ratification status + maturity/grace countdowns on proposal detail); `guardian.{Members, MemberBonds, PendingActions, Approvals, Allowances, ActiveHolds, Playbooks, ActivePlaybooks, ReviewDeadlines}`; `attestor.{Members, AttestationsFor, OpenChallenges}`; `epoch.CohortSnapshotBlock` (so the FE can display snapshot-tally eligibility for oracle referenda); `constitution.PhaseFlags`. All track parameters, allowances, bonds and deadlines are exposed via the runtime constants API (metadata) — the FE hardcodes nothing (D-2).

**Events**: the standard `referenda`/`conviction_voting` event sets; `Ratified { pid, referendum_index }`; `GuardianAction`; `ForceRerun`; `PlaybookActivated/Renewed/Expired`; `AttestationSubmitted/Challenged/ChallengeResolved`; `IntakeSlashed`.

---

## 9. Parameters introduced or changed by this document

Values are owned by [13](./13-parameters.md); this table is the change list, not the source of truth.

| Key                                  | Default                              | Kind                    |
| ------------------------------------ | ------------------------------------ | ----------------------- |
| oracle track curves                  | 60% / 10%→3% / 7 d + snapshot tally  | track config            |
| `ratify` expedited schedule          | 0 / 3 d / 12 h (freeze-gated)        | track config            |
| `intake.slash_fraction`              | 10% (preimage-missing, non-decision-grade) | PARAM             |
| `intake.max_per_account`             | 4                                    | K                       |
| `pb.ledger_freeze_max`               | 201,600 blocks (14 d)                | K                       |
| `pb.ledger_freeze_renewals`          | 1                                    | K                       |
| `grd.review_deadline`                | 2 epochs                             | META                    |
| `frn.window` (force-rerun Extended)  | = `dec.extension` (43,200 blocks)    | K (shared)              |
| `att.min_members` / `att.quorum`     | 3 / 2                                | K                       |
| `att.bond`                           | 25,000 VIT                           | entrenched              |
| `att.challenge_window`               | 43,200 blocks (72 h)                 | META                    |
| Emergency-class rows                 | **deleted**                          | —                       |

---

## Resolves

| Finding | Resolution in this document |
| ------- | ---------------------------- |
| B-11 (ratification self-contradictory and unwired) | §2.2: single execute-time deadline (D-5); `execution_guard.ratify(pid, ref_index)` admissible on the `ratify` track; `Ratifications` record; guard dispatch check + `Rejected(NotRatified)` at grace end; decision-table row stated here, pseudocode coordinated to [05](./05-welfare-and-decision-engine.md), guard mechanics to [09](./09-execution-upgrades-and-rollout.md) |
| B-13 (costless intake exhaustion; preimage unpinned) | §4: `request_preimage` pinning at qualification with release on terminal states; 10% bond slash on preimage-missing cancellation |
| B-17 (no emergency brake — guardian side) | §6.3: `PB-LEDGER-FREEZE`, 5-of-7, admissible only under the machine-checked I-4 drift flag, freezes all ledger+market calls, ≤ 14 d + one values-referendum renewal, mandatory retro ratification; unlocks the [09](./09-execution-upgrades-and-rollout.md) expedited CODE lane |
| B-19 (terminal adjudication on the weakest track) | §2.3: oracle track raised to 60% / 10% / 7-day with a pre-cohort conviction-snapshot tally excluding capital that entered after the disputed cohort began; six-track table reconciled in §2.1 |
| D-7 / B-med Emergency class | §6.1: `ProposalClass::Emergency` deleted; classifier exhaustive over five classes; playbooks are the emergency mechanism; all class references updated here and in [02](./02-integration-contract.md)/[05](./05-welfare-and-decision-engine.md)/[13](./13-parameters.md) |
| B-med force_rerun (no state-machine integration) | §5.3: pre-execution only; per-proposal target; TWAP reset; books reopen for a 3-day Extended window; positions intact; one final decide re-run; one guardian rerun of either kind per proposal ever; hooks coordinated to [05](./05-welfare-and-decision-engine.md) |
| B-med EmergencyPlaybook call enumeration | §6.2: per-playbook exhaustive admissible call sets in the capability table, mechanically checked at registry ratification and re-checked at dispatch |
| B-med SafetyFilter wrapper gaps | §3.3: recursion extended to `proxy_announced` and `as_multi_threshold_1`; the call-carrying wrapper set enumerated and closed (utility, proxy, multisig, scheduler, bootstrap sudo); G-5 restated; CI exhaustiveness obligation |
| B-med slot monopolization | §4: bond refundable only on decision-grade outcomes (10% slash otherwise); ≤ 4 intake entries/epoch/account; threat row in [14](./14-threat-model.md) |
| B-med attestation presence-only (with [09](./09-execution-upgrades-and-rollout.md)) | §7: bonded, values-elected ≥ 3-member attestor registry; 2-of-N signed attestations with a 72 h challenge window; enforced at queue time and at execute |
| X-2 (backend side) | §8: complete extrinsic/storage/event surface for FE-14/FE-15, frozen via [02](./02-integration-contract.md) |
