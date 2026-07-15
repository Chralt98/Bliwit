# 09 — Execution, Upgrades, XCM and Rollout

**Status: normative component specification. Supersedes the corresponding sections of BACKEND_PLAN.md/FRONTEND_PLAN.md** (BE §18, §19, §26 delta, §27, §29, §30.4; the §5.2.6 pallet-execution-guard entry; the ADR-16 rollout row). Implements [00-decision-record.md](./00-decision-record.md) decisions **D-9** (expedited CODE lane, coretime exemption), **D-13** (Phase-3 insider containment), **D-14** (`DescriptorLeadTime`, `ReleaseChannel` key), the D-12 backend XCM surface, and the disposition rows for X-7, X-9, B-13 (pinning), B-16, B-17 (lane side), PB-MIGRATION, SafetyFilter (execution side) and kernel attestation (execution side). RFC 2119 language throughout.

**Boundary.** This document owns: the execution guard (queue, `execute()`, dispatch-time revalidation, execution records), the runtime-upgrade path, the emergency execution lanes (expedited CODE lane, PB-MIGRATION), coretime continuity, the Phase-3 insider-containment mechanics on the execution side, the XCM architecture, the rollout plan with phase gates, and the backend WBS delta. It references, and does not restate: the frozen chain↔frontend contract and history model ([02](./02-integration-contract.md)); the decision engine and `RejectReason` producers ([05](./05-welfare-and-decision-engine.md)); guardian playbooks, PB-LEDGER-FREEZE, the SafetyFilter recursion rule, the attestor-registry governance and the ratify track ([06](./06-governance-and-guardians.md)); watchtowers and the dispute game ([07](./07-oracle-and-disputes.md)); treasury budget lines, min-viable NAV and bond-slash economics ([08](./08-treasury-and-economics.md)); the FE precondition tables and sudo-era banner ([11](./11-frontend-workflows.md)); the release train, descriptor pipeline and expedited descriptor-only release ([12](./12-release-and-operations.md)); parameter values ([13](./13-parameters.md)); threat rows ([14](./14-threat-model.md)); test regime ([15](./15-invariants-and-testing.md)).

---

## 1. Execution guard (`pallet-execution-guard`)

The guard is the single belief-side execution path. `pallet-scheduler` is deliberately absent from it: scheduler agendas capture origin at scheduling and would not repeat the checks of §1.2; the guard is the revalidation point. Belief-side execution never touches the scheduler; the scheduler remains solely `pallet-referenda`'s enactment engine, whose dispatches re-enter `BaseCallFilter` + origin checks at dispatch time (carried from BE §6.4; the phantom "§18.6" references in both source documents are void — the attestation regime is §2.4 of this document).

### 1.1 Queue semantics and queue-time preconditions

`enqueue` is callable only by `pallet-epoch`'s decision path (I-9). Storage:

```
QueuedExecution {
  pid, payload_hash, payload_len, class, maturity, grace_end,
  version_constraint, meters_declared, ratify_ref: Option<ReferendumIndex>,
  attestation_id: Option<AttestationId>,       // CODE/META only
  pre_upgrade_checkpoint: Option<(BlockHash, StateRoot)>, // written at execute() for CODE/META
  cancelled: bool
}
```

Queue bound = `MaxLiveProposals = 32` *(normative value: [13](./13-parameters.md))*.

Queue-time preconditions (enforced by the decision path before `enqueue` succeeds):

1. `decide()` returned Pass with all gate/welfare checks per [05](./05-welfare-and-decision-engine.md) — including the decide-time `SecuritySizing` outflow cap (D-4).
2. **Preimage pinned**: the payload preimage was pinned at qualification via `request_preimage` (§7.3) and is still noted. A missing preimage at queue time cancels per T4 with the 10% bond slash owned by [06](./06-governance-and-guardians.md)/[08](./08-treasury-and-economics.md) (B-13).
3. **CODE/META only — kernel attestation complete (D-18 upgrade of BE §18.4)**: a valid `AttestationRecord` for the committed artifact hash exists with **≥ 2-of-3 attestor signatures** from the bonded attestor registry and its **72h challenge window** *(normative value: [13](./13-parameters.md))* has elapsed with no upheld challenge (§2.4). Attestation is achievable well before queue time because the artifact hash is committed at proposal submission and the trading window is 13 days.
4. **CODE/META only** — the `ratify` referendum reference is recorded (`ratify_ref`). Per D-5 there is **one** ratification deadline and it is checked at `execute()` dispatch time, not here; the referendum runs during the timelock. Recording the reference at queue time merely binds the proposal to a specific referendum.

### 1.2 `execute(pid)` — permissionless, atomic; the complete dispatch-time check list

Checks in order, **all at dispatch time**. This list is normative and canonical: the FE `execute` precondition row in [11](./11-frontend-workflows.md) MUST mirror it item-for-item (X-11i — the source FE row omitted items 6–10 below).

1. **Queue state**: queued, not cancelled, `maturity ≤ now ≤ grace_end`.
2. **Preimage**: fetched; hash and length match the trading-time commitment (pinned since qualification, §7.3).
3. **Version constraint**: `RuntimeVersionConstraint` matches live `spec_name`/`spec_version` — mismatch (an intervening upgrade) transitions to `Rejected(StaleQueue)`, requiring resubmission. **Layout (normative here; the type appears inside the frozen [02 §4](./02-integration-contract.md) `QueuedExecutionView` and the doc-05 `Proposal`):** `RuntimeVersionConstraint { spec_name: BoundedVec<u8, 32> /* UTF-8, = RuntimeVersion.spec_name */, spec_version: u32 }`.
4. **Ratification (D-5)**: for CODE/META (and rule-altering META per the [06](./06-governance-and-guardians.md) capability table), the referendum at `ratify_ref` is in state Passed → else `Rejected(NotRatified)`. This is the single ratification deadline; there is no queue-time ratification check.
5. **Attestation presence**: for CODE/META, the `AttestationRecord` referenced by `attestation_id` still exists, is unrevoked and unchallenged → else `Rejected(AttestationMissing)`. (Content validity was established at queue time, §1.1(3); this check catches post-queue revocation by an upheld late challenge.)
6. **Capability rules**: every call domain in the batch is admissible for the class origin per the constitution capability table.
7. **Rate meters**: treasury-outflow, issuance and `code.spacing` meters admit the batch (I-7, I-17). Meter contention ⇒ execution stays queued and retries within grace.
8. **Resource locks**: all declared resource-domain locks are still held by `pid`.
9. **No guardian hold**: no active `delay_once` on `pid`, no guardian suspension touching the queue.
10. **Gate flags / freezes**: no active hard-gate daily breach flag, no dead-man freeze, no active PB-LEDGER-FREEZE and no PB-MIGRATION halt-at-fault (§4.2). The enumerated coretime-renewal call is the sole exemption and does not pass through the queue at all (§5).
11. **Payload bounds**: decode batch (≤ 16 calls, ≤ 64 KiB, declared weight ≤ 25% of block limit, dispatch class Normal); re-derive call domains and assert ⊆ declared domains (I-11); recursively apply the **closed** SafetyFilter (§1.4) to nested wrappers.
12. **Dispatch**: each call with the class origin (`FutarchyParam`/`FutarchyTreasury`/`FutarchyCode`/`FutarchyMeta`); the batch is atomic — any item error rolls back the batch's state and transitions per T18.
13. **Record**: write `ExecutionRecord`, consume meters, release locks, unpin the preimage, emit `Executed`.

### 1.3 Origin discipline

Every class origin is produced by exactly one pallet through exactly one code path; none is obtainable from a signed extrinsic, XCM origin conversion, or wrapper call. There is no path to unrestricted Root: `EnsureRoot` succeeds only for the internal dispatch the guard performs for the two allowlisted `frame_system` upgrade calls, constructed inside the runtime (I-10).

### 1.4 SafetyFilter closure — execution-side enforcement

The SafetyFilter recursion rule is owned by [06](./06-governance-and-guardians.md). Normative consequence enforced here: the wrapper set the filter recurses into is **closed** — it MUST include `proxy_announced` and `as_multi_threshold_1` in addition to `batch`/`batch_all`/`force_batch`/`proxy`/`as_multi` (the review's completeness finding: these were the only two unhandled call-wrappers; had they fallen to allow-without-recursion, the filter was bypassable). Guard step 11 applies the closed filter to every nested level (`MAX_NESTED = 4`, ≤ 16 calls total); [15](./15-invariants-and-testing.md) carries the negative-test obligation (I-10/I-11 fuzz over *all* wrapper types, including the two added ones, and over `sudo.sudo(call)` during Phases 0–3 per §6.1).

### 1.5 Execution records and history serving

`ExecutionRecords` is a ring buffer (≤ 256 entries). The source's "pruned to indexer" language is **replaced** (D-2/D-6): canonical execution history is **event-derived and chain-served within the committed window** — `Executed`/`Rejected(*)`/`UpgradeAuthorized`/`UpgradeApplied` events plus the ring buffer are servable via the [02](./02-integration-contract.md) three-layer history model (chain-served recent window; 30-day committed operator window). The indexer is an optional dashboard convenience, **never load-bearing** for the canonical frontend.

---

## 2. Runtime-upgrade path (CODE or META class)

### 2.1 Two-phase authorize/apply flow

1. Proposer commits the artifact set at submission: candidate Wasm **hash**, target `RuntimeVersion` (spec_version N+1), **metadata hash** (via `frame-metadata-hash-extension` tooling — verified crate), migration plan with benchmarked migration weight, audit-report hash, and kernel **attestation reference** (§2.4). The payload is exactly `system.authorize_upgrade(code_hash)` (verified dispatchable) plus the commitment record write, plus the `ReleaseChannel` update (§2.3).
2. Markets trade the committed artifact; gate markets mandatory; the `ratify` referendum is submitted any time after the artifact-hash commitment and runs during the timelock (D-5).
3. After the timelock, `execute(pid)` runs the full §1.2 list and dispatches `authorize_upgrade(code_hash)` with internally constructed Root restricted to this single call and hash (I-10, I-19). The guard records `PendingUpgrade { hash, authorized_at: now, applicable_at: now + DescriptorLeadTime }`, writes the pre-upgrade checkpoint (§4.2), updates `ReleaseChannel` (§2.3) and emits `UpgradeAuthorized { hash, authorized_at, applicable_at }`. `authorize_upgrade_without_checks` and `set_code*` are filtered for every origin, always, from genesis (§6.1).
4. **Application is lead-time-gated (D-14, §2.2).** From `applicable_at`, **anyone** submits the matching artifact via `system.apply_authorized_upgrade(code)` (verified); `cumulus-pallet-parachain-system` performs the parachain code-upgrade signaling to the relay **[VERIFY current go-ahead mechanics on stable2603]**.
5. Migrations run under `pallet-migrations` (verified crate): multi-block, cursor-bounded, benchmarked; `try-runtime` + `try-state` gate the release in CI.
6. Post-upgrade: the guard records the observed spec_version and clears `PendingUpgrade`; failed post-upgrade checks on testnet block the release; on mainnet, migration failure follows `pallet-migrations` halt-and-alarm semantics **[VERIFY exact failure semantics]** with the now fully specified playbook **PB-MIGRATION** (§4.2) as recovery.

Edge handling (carried): queued-under-N-executed-after-upgrade ⇒ rejected by check §1.2(3); storage-version mismatch ⇒ migration framework refuses ⇒ PB-MIGRATION; excessive migration weight ⇒ multi-block spreading within declared bound, else abort ⇒ PB-MIGRATION; metadata incompatibility ⇒ wallet-detectable via metadata hash; emergency rollback: **none on-chain** — a rollback is a forward upgrade through the same process (or the expedited lane, §4.1); collator binary compatibility handled by the ops release train ([12](./12-release-and-operations.md)).

### 2.2 `DescriptorLeadTime` (D-14, resolves X-7)

`system.apply_authorized_upgrade` for a CODE/META artifact MUST NOT be dispatchable before

```
now ≥ authorized_at + DescriptorLeadTime        // 43,200 blocks = 72 h (normative value: 13)
```

where `authorized_at` is the block of the `UpgradeAuthorized` event (guard step §2.1(3)). Enforcement is deterministic and origin-independent: the SafetyFilter consults `PendingUpgrade` and denies `apply_authorized_upgrade` while `now < applicable_at`; an early submission fails statelessly. This closes the X-7 defect — application could previously follow authorization by one block, opening a ≥ 72h global `ReadOnlyIncompatible` signing outage that outlasted 48h obligations (oracle challenge windows now 72h with watchtower quorum, [07](./07-oracle-and-disputes.md), interlock preserved: the challenge window never fits inside an un-warned descriptor gap).

The frontend obligation is the mirror half and is **release-gating, not a convention**: v(N+1) descriptors MUST be generated from the queue-time artifact commitment and live on the release channel **before `applicable_at`** ([12](./12-release-and-operations.md)); the expedited descriptor-only release (2 attestations, no 72h soak, 3-of-5 repoint) exists for exactly this window. Because the artifact hash and metadata hash are committed at *submission* (≥ timelock + trading window before `applicable_at`), the 72h lead time is a floor on the last mile, not the whole descriptor budget.

### 2.3 `ReleaseChannel` — fixed-layout raw storage key (D-14; replaces the FE §24.5 `system.remark` pointer)

`pallet-constitution` exposes a `StorageValue` at the **well-known raw key**

```
twox_128("Constitution") ++ twox_128("ReleaseChannel")
```

whose fixed-width byte layout is **frozen forever** — it MUST never change across any runtime upgrade; any future extension appends fields beyond offset 168 with a schema bump, never a change to existing offsets or a rename to a new key. This makes the value readable by stranded apps (pinned releases in `ReadOnlyIncompatible`) that cannot decode current metadata: static key, fixed offsets, no metadata required. **The byte layout is owned and frozen by [02 §12](./02-integration-contract.md)** (168 bytes; quoted for readability — *normative layout: 02 §12*):

```
ReleaseChannelRecord {                 // normative layout: 02 §12 (offsets there)
  schema:                u8,          // constant 1
  version:               [u8; 32],    // UTF-8 semver, zero-padded — current canonical release
  manifest_txid:         [u8; 43],    // Arweave base64url TXID, zero-padded
  release_json_hash:     [u8; 32],    // SHA-256
  updated_at:            u32,         // block of last update
  spec_version:          u32,         // current runtime spec_version
  pending_authorized_at: u32,         // block of a pending UpgradeAuthorized; 0 if none
  min_supported_version: [u8; 32],    // UTF-8 semver, zero-padded (12 §3.2)
  keyring_generation:    u32,         // 12 §2.1
  revoked_key_bits:      u64,         // revocation bitmask, 12 §2.3
  flags:                 u32,         // bit 0 SECURITY, bit 1 EXPEDITED, bit 2 URGENT_UPGRADE
}
```

Writers, exhaustively: (a) the execution guard at `UpgradeAuthorized` (`spec_version` target, `pending_authorized_at`, the `URGENT_UPGRADE` flag) and at applied-upgrade detection (clears pending fields); (b) `ConstitutionalValues` via `constitution.set_release_channel` for manifest-pointer, `min_supported_version` and keyring-generation/revocation updates, including expedited descriptor-only releases ([12](./12-release-and-operations.md)). No other origin can write it. The FE in-app upgrade warning for pinned-release users binds to this key ([12](./12-release-and-operations.md)).

### 2.4 Kernel attestation — bonded attestor registry (upgrade of BE §18.4; no longer presence-only)

Deterministic Wasm builds (`srtool`-class **[VERIFY current recommended tooling]**) are carried. The presence-only attestation of the source ("any 32-byte hash passes") is replaced:

- **Attestor registry**: ≥ 3 values-elected attestors, each posting a bond; election, recall, bond size and slash routing are owned by [06](./06-governance-and-guardians.md) (registry governance) and [13](./13-parameters.md) (values).
- **AttestationRecord** per artifact: `{ artifact_hash, source_commit, build_env_digest, signatures: BoundedVec<(AttestorId, Signature), 8>, submitted_at, challenge_end }`. Valid iff ≥ **2-of-3** registered attestors sign that they independently reproduced `artifact_hash` from `source_commit`.
- **Challenge window**: 72h *(normative value: [13](./13-parameters.md))* from record submission. A bonded challenge alleging non-reproducibility routes through the [07](./07-oracle-and-disputes.md) dispute game (deterministic recomputation is the natural terminal step: rebuild and compare hashes); an upheld challenge slashes the signing attestors, revokes the record, and any queued proposal referencing it fails §1.2(5) at execute.
- **Gating**: record validity + elapsed window is a **queue-time precondition** (§1.1(3)); record continued existence is a dispatch-time check (§1.2(5)).

Every release still publishes source commit, build-environment digest, Wasm hash, metadata hash. Verifying attestation *content* beyond hash reproduction remains social/off-chain (A-4) and is stated as such — the registry converts "presence-only" into "bonded, plural, challengeable", not into a mathematical proof.

### 2.5 Worked example (BE §30.4, updated)

META proposal commits (wasm_hash H, spec 102, metadata hash M, migration weight 40% of one block spread over 3 blocks, audit hash, attestation reference A). Attestors 1 and 3 sign A during the first trading week; the 72h challenge window closes unchallenged; queue-time preconditions hold. The values `ratify` referendum passes day 12 (during the timelock). Markets: uplift 0.071 ≥ 0.060; gates clean. Queued, timelock 14 d. `execute` passes checks 1–11 (ratified ✔, attestation present ✔, meters ✔) and dispatches `authorize_upgrade(H)` via restricted internal Root; `UpgradeAuthorized{H, authorized_at, applicable_at = authorized_at + 43,200}`; `ReleaseChannel` updated; checkpoint recorded. The FE ships v(N+1) descriptors (generated from the queue-time commitment) inside the 72h; the release-gating check in [12](./12-release-and-operations.md) confirms them live. At `applicable_at + 10`, a keeper submits `apply_authorized_upgrade(code)`; parachain-system schedules the swap; migrations run over 3 blocks; guard records spec 102; `try-state` clean. A stale PARAM item queued under spec 101 later hits §1.2(3) → `Rejected(StaleQueue)` and is resubmitted.

---

## 3. Emergency lanes

### 3.1 Expedited CODE lane (D-9, resolves B-17 lane side)

A repair upgrade MUST NOT need a new privileged origin, and it does not get one. The expedited lane is the normal CODE process with compressed windows, admissible **only while a machine-checked emergency condition is active**:

- **PB-LEDGER-FREEZE** is active (guardian 5-of-7 playbook keyed to the I-4 drift flag; contents, expiry ≤ 14 days + one renewal, and retro-ratification owned by [06](./06-governance-and-guardians.md)); **or**
- a **PB-MIGRATION halt-at-fault** is active (§4.2) — without this arm the designated migration fallback could not terminate, since its rollback path is a forward upgrade and the normal lane is O(25 days).

Lane mechanics:

| Aspect | Expedited rule | Normal-lane rule |
|---|---|---|
| Admissibility | one of the two freeze conditions active, machine-checked at submission *and* at decide | always |
| Gate markets | mandatory, **72h** decision window | full trading window |
| Values ratification | fast-track `ratify` referendum, **3-day** decision period ([06](./06-governance-and-guardians.md) track table); checked at execute per D-5 | 7-day ratify |
| Attestation | full §2.4 regime (2-of-3 + 72h window, runnable concurrently with the gate market) | same |
| Execution | the **normal execution guard**, full §1.2 list; check §1.2(10) treats the *triggering* freeze as satisfied for the expedited proposal itself (the freeze exists to contain the fault the proposal repairs) | full list |
| `DescriptorLeadTime` | **applies in full** — the freeze contains the exploit, so the 72h descriptor floor is affordable; the expedited descriptor-only FE release ([12](./12-release-and-operations.md)) covers the last mile | applies |
| `code.spacing` meter | **waived for the expedited proposal** (a 14–30 d spacing floor inside a ≤ 14-day freeze would nullify the lane); the meter still *records* the upgrade, and the next normal-lane CODE measures spacing from it | enforced |
| Privileged origin | none — dispatches as `FutarchyCode` | none |

Worst-case repair latency under an active freeze: 72h gate + 3d ratify (concurrent where windows overlap) + timelock floor 24h + 72h lead time ≈ **9–10 days**, inside the 14-day freeze envelope — versus the ~25–46-day normal path B-17 measured.

### 3.2 PB-MIGRATION — contents (resolves the empty-designated-fallback finding)

PB-MIGRATION is a registered guardian playbook (preimage-committed, values-ratified in advance, scoped, expiring). The review flagged it as an empty name; its contents are now normative:

**Trigger (machine-checked, any of):** `pallet-migrations` reports a failed migration step; migration cursor stalled > 900 blocks; post-upgrade `try-state` assertion failure signal. Activation requires guardian 5-of-7 while a trigger is active.

**1. Halt-at-fault semantics.** On trigger, the chain fail-stops the *affected surface*, not block production: `pallet-migrations` keeps ordinary transactions paused while a multi-block migration is incomplete (its native behavior **[VERIFY exact pausing surface on stable2603]**); the playbook additionally freezes the execution queue and ledger/market inflows (the same enumerated call set PB-LEDGER-FREEZE uses, [06](./06-governance-and-guardians.md)) so that no state predicated on the half-migrated layout is created. No partial-migration state is ever exposed to user calls.

**2. State-root checkpoint.** At every CODE/META `execute()`, the guard records `pre_upgrade_checkpoint = (parent block hash, state root)` in the `QueuedExecution` record and the `UpgradeAuthorized` event stream (§2.1(3)). PB-MIGRATION references this checkpoint as the audit anchor: the divergence set between checkpoint and halted state is exactly the migration's writes to the cursor position, which is what the retry-or-rollback decision is made over.

**3. Retry-or-rollback decision rule.**
- **Retry** (up to 2 attempts) iff the failure class is resource-bounded — weight/PoV overrun or a transient host error — using the migration framework's bounded continuation controls with re-benchmarked weight **[VERIFY pallet-migrations control surface]**. A retry that completes clears the halt; `try-state` must pass before the freeze lifts.
- **Rollback** otherwise (logic fault, storage-shape mismatch, second failed retry): rollback is a **forward upgrade** carrying a remediation artifact whose migration restores the checkpoint semantics for the affected keys. It rides the **expedited CODE lane** (§3.1, admissible because the halt is active) with full attestation and fast-track ratification. Guardians cannot install code (kernel prohibition, carried); their playbook power ends at freezing, retrying within the framework's bounds, and *initiating* the lane.
- The decision MUST be taken within 72h of activation; inaction defaults to rollback initiation (status-quo-default discipline).

**4. Guardian notification and review.** Activation and every step emit `GuardianAction { power, target, justification_hash }` plus `MigrationHalted { cursor, failed_step }`; the RB-UPGRADE runbook pages operators immediately; a mandatory retrospective `ratify` review is auto-scheduled; an unratified activation slashes 50% of approving members' bonds per the [06](./06-governance-and-guardians.md) rule.

---

## 4. Coretime continuity (D-9, resolves B-16)

The B-16 wedge — the dead-man freeze suppressing the TREASURY pipeline that funds the coretime renewal that would end the degradation — is closed by one enumerated, narrowly-scoped call:

`futarchy_treasury.execute_coretime_renewal(period_index)` — permissionless (Signed), keeper-rebated. Semantics:

- Spends **only** from the pre-authorized `ops.coretime` budget line in `pallet-futarchy-treasury` ([08](./08-treasury-and-economics.md)); the line is replenished by ordinary TREASURY-class decisions (Phase ≥ 4) or the ops multisig path (Phase ≤ 3), so the standing authorization is decided in calm weather, not during degradation.
- Dispatches the XCM reserve transfer (DOT) from the treasury to the chain's renewal account on the Coretime chain, per the §6 XCM rules **[VERIFY current renewal-price mechanics and channel procedure]**; idempotency key = `period_index`, bounded retry, keeper-monitored — an XCM failure never enters any decision or settlement path (I-24).
- **Exempt from every freeze**: dispatchable during the dead-man freeze, hard-gate suspension, PB-LEDGER-FREEZE and PB-MIGRATION halt. It does not pass through the execution queue (§1.2(10) does not apply — it is not a queued proposal), cannot be blocked by guardian holds, and is the *only* treasury outflow with this property.
- Engaging it during degraded mode **does not consume a recovery epoch**: renewal is maintenance, not a governance decision, and the proposal-free recovery-epoch rule keys only on dead-man engagement itself, never on this call.
- Bounds: per-period spend ≤ the budget-line balance; the line's size is a [13](./13-parameters.md) value; the call is a no-op (error, no state change) when the renewal window is not open or the period is already funded.

---

## 5. Phase-3 insider containment (D-13, resolves X-9)

### 5.1 Genesis-filtered calls — all origins, including sudo

The following `frame_system` calls are filtered **from genesis**, for **all** origins including the sudo key (the source's "post-bootstrap" qualifier is deleted):

```
system.set_storage
system.kill_storage
system.kill_prefix
system.set_code_without_checks
system.authorize_upgrade_without_checks
```

(`system.set_code` remains filtered as before; `authorize_upgrade`/`apply_authorized_upgrade` are the sole upgrade path, §2.) Enforcement: the SafetyFilter denies these unconditionally, **and recurses through `sudo.sudo` / `sudo.sudo_unchecked_weight` / `sudo.sudo_as`** during Phases 0–3 so the multisig cannot reach them through the sudo wrapper; [15](./15-invariants-and-testing.md) carries negative tests for every wrapper × every filtered call. Consequence: even a fully malicious founding multisig cannot silently rewrite storage or install unchecked code — its worst case is bounded to the sudo-admissible surface, which is what the [14](./14-threat-model.md) founding-multisig threat row (new) prices.

### 5.2 Phase-3 real-USDC exposure caps

While real USDC trades under bootstrap authority, exposure is capped by two constitution keys (values in [13](./13-parameters.md)):

- `phase3.tvl_cap` — global cap on total USDC held by the ledger + treasury sovereign accounts; enforced at every `split`/inflow-crediting path and at the XCM inflow leg (excess inbound reserve transfers fail politely, funds remain on Asset Hub).
- `phase3.deposit_cap` — per-account cumulative USDC deposit cap over the phase; enforced at the same points.

Both keys are **raised only by phase gates** (§7): they are not PARAM/META-adjustable during Phases ≤ 3; each phase-advancement upgrade carries the scheduled raise, and Phase 5+ sets them to unbounded sentinels. The FE surfaces the caps and the persistent **"bootstrap governance (sudo active)"** banner chain-read from `PhaseFlags` ([02](./02-integration-contract.md) chain identity; FE side [11](./11-frontend-workflows.md)) — sudo-era state is never presented as trust-equivalent to post-sudo state.

### 5.3 Bootstrap authority (carried, restated)

`pallet-sudo` held by a 4-of-6 founding multisig, powers used only for: incident response in Phases 0–2, arming phase flags on evidence, and authorizing the Phase-3→4 upgrade. Every sudo use MUST be announced with a justification hash. The founding multisig is a first-class adversary in [14](./14-threat-model.md). Sunset: the removal migration of §7.2.

---

## 6. XCM architecture (BE §19 carried + D-12 backend surface)

### 6.1 Rule table (normative)

| Aspect | v1 rule |
|---|---|
| XCM version | latest stable on `stable2603` **[VERIFY exact version const]**; version negotiation enabled |
| Accepted origins | Asset Hub, relay chain, Coretime chain; all others barred |
| Barrier | paid-execution allowlist + known query responses; **`Transact` refused from all locations**; no superuser origin conversion; `UnpaidExecution` refused |
| Asset mappings | USDC (Asset Hub location ↔ local `ForeignAssets` id, pinned per D-17: `{parents: 1, X3(Parachain(1000), PalletInstance(50), GeneralIndex(1337))}` **[VERIFY asset index 1337]**) and DOT (parent) only; unknown assets refused |
| Reserve model | Asset Hub is reserve for USDC; relay/AH for DOT; **teleports disabled** |
| Fees/weight | `WeightTrader` selling execution for DOT or USDC at governed rates; USDC is a sufficient asset, so a USDC-only account can pay inbound execution fees ([08](./08-treasury-and-economics.md) owns `fee.vit_usdc_rate` for the tx-payment side) |
| Failure handling | protocol-initiated transfers (coretime funding §4, treasury recovery) are keeper-monitored with bounded retry and idempotency keys; user transfers follow standard XCM error semantics; **no XCM outcome participates in any decision or settlement path** (I-24) — an XCM failure can therefore never default to adoption |
| Trapped assets | recovery via a TREASURY-class `claim_assets` call only |
| Disabled instructions | `Transact`, HRMP channel-request handling beyond system defaults, any instruction not needed for reserve transfer + fee payment — default-deny posture |
| Governance restriction | `pallet_xcm::{send, force_*}` filtered for all origins; cross-chain governance execution deferred |

External oracle-parachain feeds via XCM remain analyzed and excluded as settlement sources in v1.

### 6.2 Withdrawal (exit) path — user-callable

`pallet_xcm.limited_reserve_transfer_assets` (USDC → Asset Hub, DOT for fees) is user-callable on this chain and is the canonical exit. It is an ordinary signed call subject to `phase3.tvl_cap` accounting (withdrawals always allowed — caps bind inflows only) and gets a normal FE screen with a precondition row ([11](./11-frontend-workflows.md), D-12). It is **not** frozen by PB-LEDGER-FREEZE's inflow arm; whether the freeze halts outflows too is owned by [06](./06-governance-and-guardians.md) (D-9 freezes both directions — the FE precondition row reflects the flag).

### 6.3 Asset Hub on-ramp — what the backend provides (D-12)

The guided deposit flow ([11](./11-frontend-workflows.md)) runs a second light-client connection to Asset Hub. This chain's obligations, so that flow can exist:

1. **Pinned identity**: the USDC `Location`, local `ForeignAssets` id, paraId and ss58 prefix frozen in [02](./02-integration-contract.md) (D-17) — the FE `ChainIdentity` pins them; nothing is discovered at runtime.
2. **Sufficiency + fee viability**: USDC registered sufficient at genesis; inbound reserve transfers deliver to accounts holding zero VIT and the delivered USDC can pay subsequent local fees.
3. **HRMP channels**: opened during Phase 2 on Paseo, Phase 3 on Polkadot **[VERIFY current channel-establishment procedure]**; channel liveness is a rollout gate (§7).
4. **Asset Hub descriptor set**: added to the descriptor pipeline and shipped in the same release train ([12](./12-release-and-operations.md)); pipeline liveness incl. Asset Hub descriptors is a Phase-3 gate (§7).
5. **Test artifacts**: the [02](./02-integration-contract.md) published test-artifact feed includes a Chopsticks/Zombienet Asset Hub ⇄ parachain reserve-transfer environment (E21, §8) so the FE can regression-test the funding flow per release.

### 6.4 XCM health and reserve health

- **X sub-metric (XCM health)**: send/fail/timeout counters and channel-liveness enter `C_onchain` ([05](./05-welfare-and-decision-engine.md)/[07](./07-oracle-and-disputes.md) own the pillar split). **[VERIFY XCM-health data availability on stable2603 — which queue/error surfaces are runtime-readable]** — this open item is retained, and it now has a fallback rather than a hole:
- **R probe (reserve health)**: a deterministic, same-block-computable probe of the sovereign-account/reserve state (inbound-transfer success within the observation window, sovereign balance reconciliation). R enters `C_onchain`; a failing R sets the daily **C** breach flag **fail-static** (R and X both live in `C_onchain` — pillar placement owned by [05](./05-welfare-and-decision-engine.md) §4.3) — if XCM-health data proves unavailable or ambiguous, R alone drives the flag, so a frozen/censored reserve channel degrades to status-quo defaults, never to silent full-backing claims. PB-RESERVE (halt split inflows) and the FE NAV-haircut surfacing are owned by [07](./07-oracle-and-disputes.md)/[08](./08-treasury-and-economics.md)/[10](./10-frontend-architecture.md).

---

## 7. Rollout plan and phase gates (BE §29 reworked)

### 7.1 Phase table

Advancement at every step = published evidence + META decision + values ratification; delays always allowed, acceleration never. New gates versus the source are **bold**.

| Phase | Enabled classes / authority | Guardian powers | Entry criteria | Exit criteria | Rollback/halt | Audits |
|---|---|---|---|---|---|---|
| 0 Reference & simulation | none | n/a | E1–E8 code-complete | reference model ≡ pallets on shared vectors; sim false-pass < 1%; δ/POL calibration published | n/a | internal review |
| 1 Local nets | none | n/a | Phase 0 exit | Zombienet: 3 unattended epochs incl. collator/keeper loss + dead-man drill; **coretime-renewal-call drill under active dead-man**; **PB-MIGRATION drill (forced failed migration → retry and rollback branches)** | n/a | — |
| 2 Public testnet (Paseo) + bounties | all classes, testnet-binding | full powers, penalty-free | Phase 1 exit; bounty program funded; **ss58 prefix 7777 registry submission accepted**; **testnet WSS bootnode set live (≥ 8 browser-reachable across ≥ 4 operators, ≥ 2 on :443)**; **integration-contract implementation (E15) deployed** | ≥ 6 epochs; zero invariant breaches; ≥ 1 full upgrade e2e **incl. `DescriptorLeadTime` + attestation + ratify path**; unclaimed core bounties or all findings fixed; **contract freeze signed by both teams ([02](./02-integration-contract.md))**; **descriptor pipeline exercised e2e on testnet**; **expedited-CODE-lane drill under a staged freeze** | redeploy | audit A begins |
| 3 Mainnet shadow futarchy | markets real (under **`phase3.tvl_cap` / `phase3.deposit_cap`**, §5.2), decisions advisory (guard disconnected); sudo present, **dangerous calls filtered from genesis (§5.1)**; **FE sudo banner live** | full | audits A+B passed; genesis ceremony; **mainnet WSS bootnode set live (≥ 8/≥ 4 ops/≥ 2 on :443) + 30-day operator served-state commitment in force**; **descriptor pipeline live incl. Asset Hub descriptor set**; **≥ 3 registered oracle reporters with full stakes**; **HRMP channels to Asset Hub open; funding flow (deposit + withdraw) passing the published XCM test suite** | ≥ 6 epochs; standing-Baseline calibration error within band; F̂ measured ≥ L/2; zero oracle deadlocks; **attestor registry elected and ≥ 1 mainnet-shadow upgrade attested 2-of-3** | sudo pause | reproducible-build attest |
| 4 Binding PARAM | PARAM only; **sudo removed** (§7.2); exposure caps raised per schedule | pause/delay/rerun/playbooks | Phase 3 exit + values ratification of the arming upgrade; **NAV ≥ min-viable NAV(PARAM) ([08](./08-treasury-and-economics.md)) — loud gate: arming refused with a published shortfall figure, never silently** | ≥ 12 binding PARAM decisions; zero constitutional slashes of the engine; zero reversal-consensus incidents | PB-HALT-INTAKE; values-voted retreat to shadow (guard disconnect is itself a playbook) | — |
| 5 + TREASURY | + TREASURY (streams mandatory > 1% NAV); caps raised | as P4 | Phase 4 exit; V_min consistently met; **treasury funding ≥ 25M USDC and NAV ≥ min-viable NAV(TREASURY)** | 2 epochs at full treasury cadence; zero unratified guardian actions | as P4 | — |
| 6 + CODE/META | + CODE, META (values ratification mandatory); caps → unbounded sentinels | delay-once + playbooks only | Phase 5 exit; scope-A re-audit; **NAV ≥ min-viable NAV(CODE/META) (≈ 14M USDC floor for one CODE at floor liquidity — normative value: [08](./08-treasury-and-economics.md))** | 1 CODE upgrade shipped and stable ≥ 60 d **through the full D-14 lead-time path**; dispute game exercised without deadlock | as P4 | scope-A re-audit |
| 7 Mature | all; guardian reduced to playbooks only; renewal by entrenched track | playbooks only | Phase 6 exit; entrenched-track confirmation | steady-state; guardian sunset vote scheduled | as P4 | periodic |

### 7.2 Sudo removal (Phase 3→4; carried, normative)

The Phase-3→4 runtime upgrade (a) removes `pallet-sudo` from the runtime, (b) runs a migration **asserting the sudo key storage is purged** and that no origin maps to Root outside the guard's internal path — the migration fails the upgrade if the assertion fails, (c) flips the execution guard from shadow to binding for PARAM, (d) applies the scheduled exposure-cap raise. From that block, the authority matrix is complete and closed.

### 7.3 Preimage discipline (resolves B-13, pinning side)

At **qualification** (screening pass, slot won), `pallet-epoch` dispatches `preimage.request_preimage(payload_hash)`, pinning the committed payload for the proposal's whole lifecycle; the pin is released (`unrequest_preimage`) at every terminal state (`Executed`, `Rejected(*)`, expiry, withdrawal). Consequences: the unnote-after-adoption sabotage is dead (a requested preimage cannot be unnoted out from under the queue; any keeper can re-note the public bytes in the residual race), and the preimage-missing cancellation path now costs its proposer the 10% bond slash owned by [06](./06-governance-and-guardians.md)/[08](./08-treasury-and-economics.md). Playbook preimages are pinned the same way at playbook registration.

---

## 8. Backend WBS delta (extends BE §26; existing rows E1–E14 carried unchanged)

| Epic | Objective / scope | Depends on | Outputs | Acceptance criteria | Specialty |
|---|---|---|---|---|---|
| E15 (2) | **Integration-contract implementation** ([02](./02-integration-contract.md)): 11-method `FutarchyApi` + view types in `futarchy-primitives`, `Traded`/`Observed` events, `RecentCohortSummaries` ring, `BaselineMarketOf`, oracle names, T20 `Voided` event, constants API, published test-artifact feed | E5, E9 | runtime API + events + fixtures feed | contract conformance suite green; **mirrors FE-R1 and is release-gating for the backend exactly as FE-12 is for the frontend** | FRAME + contract |
| E16 (3) | **Ledger delta**: `VaultState::Voided` + `redeem_void`, gate instruments (`PositionKind::GateYes/GateNo`, per-branch supplies, `settle_gate`), `redeem_scalar_pair`, per-branch conservation identity | E2 | reworked ledger + PT suite + TLA⁺ delta | I-1…I-5 re-proven over the enlarged operation set | FRAME + formal |
| E17 (2) | **Baseline market home**: epoch-keyed `BaselineVaults`, `PositionId::Baseline{epoch, ..}`, `pol.b_baseline`, settlement via SettleAuthority | E3, E16 | Baseline book lifecycle | Baseline TWAP consumable by `decide()` every epoch; settlement e2e | FRAME |
| E18 (3) | **`pallet-registry`** (Incident/Milestone): bonded filings, challenge windows, slashing, bounds, weights; feeds `C_attested` | E4, E6 | new pallet + tests | bounded storage; dispute interaction per [07](./07-oracle-and-disputes.md) | FRAME |
| E19 (2) | **Watchtower registry** + challenge-window acknowledgment quorum (2-of-N co-sign "observed", else one 48h extension) | E6 | registry + window logic | censorship drill: unacknowledged window extends exactly once | FRAME |
| E20 (2) | **Attestor registry + attestation records** (§2.4) + queue-time gating + `ReleaseChannel` key + `DescriptorLeadTime` filter | E4, E7 | registry, records, filter wiring | negative tests: early `apply_authorized_upgrade` fails; unattested CODE cannot queue; `ReleaseChannel` readable via raw key with no metadata | FRAME + security |
| E21 (2) | **Funding-flow backend surface** (§6.3): sufficiency config, HRMP procedures, reserve-health R probe, Asset Hub descriptor + Chopsticks/Zombienet reserve-transfer artifacts in the E15 feed | E9, E15 | XCM config + artifacts | XCM suite green incl. failure/trap/recovery; R probe drives the daily C flag fail-static in drill | XCM |
| E22 (1) | **Emergency-lane wiring**: expedited-lane admissibility predicate, PB-MIGRATION playbook preimages + checkpoint plumbing, coretime-renewal call + freeze exemptions, genesis call filter + exposure caps | E4, E7, E8 | lane + playbook + call | Phase-1/2 drills of §7.1 pass; §5.1 negative tests over all wrappers incl. sudo | FRAME + security |

---

## 9. Resolves

| Finding | Resolution in this document |
|---|---|
| X-7 | §2.2: `apply_authorized_upgrade` gated on `now ≥ authorized_at + DescriptorLeadTime` (43,200 blocks = 72h), SafetyFilter-enforced; FE descriptor obligation made release-gating ([12](./12-release-and-operations.md)); §2.3 `ReleaseChannel` warns pinned users without metadata (D-14) |
| X-9 | §5: five dangerous `frame_system` calls filtered from genesis for all origins incl. sudo (with sudo-wrapper recursion); `phase3.tvl_cap`/`phase3.deposit_cap` raised only by phase gates *(key names normative in [13](./13-parameters.md))*; founding multisig in the [14](./14-threat-model.md) adversary model; FE sudo banner ([11](./11-frontend-workflows.md)) (D-13) |
| B-16 | §4: enumerated `execute_coretime_renewal` against the pre-authorized budget line — exempt from dead-man and all freezes, keeper-executable in degraded mode, consumes no recovery epoch (D-9) |
| B-17 (lane) | §3.1: expedited CODE lane — admissible only under an active machine-checked freeze (PB-LEDGER-FREEZE or PB-MIGRATION halt), 72h gate market + 3-day fast-track ratification, normal execution guard, no new privileged origin, `code.spacing` waived for the repair itself (D-9; playbook side in [06](./06-governance-and-guardians.md)) |
| B-13 (pinning) | §7.3: `request_preimage` pinning at qualification, released at terminal states; kills unnote sabotage; slash side owned by [06](./06-governance-and-guardians.md)/[08](./08-treasury-and-economics.md) |
| PB-MIGRATION (open [VERIFY] fallback) | §3.2: contents specified — halt-at-fault semantics, state-root checkpoint at every CODE/META execute, retry(≤2)-or-rollback decision rule with 72h default-to-rollback, rollback as forward upgrade via the expedited lane, guardian notification + mandatory retro review |
| SafetyFilter (execution side) | §1.4: wrapper recursion closed over `proxy_announced` and `as_multi_threshold_1`; guard step 11 applies the closed filter; negative-test obligation in [15](./15-invariants-and-testing.md) (rule owned by [06](./06-governance-and-guardians.md)) |
| Attestation (presence-only) | §2.4: bonded values-elected attestor registry (≥ 3), 2-of-3 signatures + 72h challenge window as a queue-time precondition; dispatch-time presence re-check `RejectReason::AttestationMissing` (registry governance in [06](./06-governance-and-guardians.md)) |
| X-11i (backend side) | §1.2: the complete, canonical dispatch-time check list — including ratification (D-5, `NotRatified`), attestation presence, meters, resource locks, gate flags/freezes — which the FE `execute` precondition row in [11](./11-frontend-workflows.md) MUST mirror item-for-item |
| X-8 (backend surface) | §6.2–6.3: user-callable reserve-transfer withdrawal; pinned identity, sufficiency, HRMP, Asset Hub descriptors and test artifacts for the on-ramp (D-12; FE flow owned by [11](./11-frontend-workflows.md)) |
| B-med (USDC freeze, backend probe) | §6.4: reserve-health R probe in `C_onchain` with a fail-static daily C flag as the fallback for the retained XCM-health **[VERIFY]** (pillar ownership in [07](./07-oracle-and-disputes.md)) |
| X-11j (partial) | §1: the phantom "§18.6" references are removed; the attestation regime is §2.4 of this document |
