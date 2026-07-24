# 03 — Conditional Ledger (`pallet-conditional-ledger`)

**Status: normative component specification. Supersedes the corresponding sections of BACKEND_PLAN.md/FRONTEND_PLAN.md** (BE §5.2.1, §7 ledger types, §10; the ledger rows of §21, §23, §24).

**Boundary.** This document owns: the conditional-position instrument set, vault state machines (proposal vaults and Baseline vaults), escrow accounting, the conservation identities and their proof obligations, all ledger calls/events/errors, the Positions storage model, VOID semantics, and the ledger-side call surface consumed by the trade wrapper. It references: [`04-markets-and-pricing.md`](./04-markets-and-pricing.md) for LMSR mathematics, wrapper sequencing, headroom sizing and book lifecycle; [`05-welfare-and-decision-engine.md`](./05-welfare-and-decision-engine.md) for who invokes the Resolve/Settle authorities and when; [`02-integration-contract.md`](./02-integration-contract.md) for the frozen SCALE types and event names; [`13-parameters.md`](./13-parameters.md) for every numeric parameter value; [`15-invariants-and-testing.md`](./15-invariants-and-testing.md) for the consolidated invariant and test registry. Decisions **D-1**, **D-3** (ledger side), **D-8** and the disposition rows for B-1…B-5, X-6, the Positions-map medium and the ScalarSettled low of [`00-decision-record.md`](./00-decision-record.md) are implemented here.

RFC 2119 keywords (MUST/SHOULD/MAY) are normative.

---

## 1. Purpose and trust boundary

Sole custodian of market collateral and **sole mint/burn authority for every conditional instrument in the system** — decision-scalar, gate-binary and Baseline instruments alike (no other pallet may hold, mint or synthesize such claims; this closes the B-2 gap without weakening the authority rule). Everything else in the system may fail without loss of trader principal so long as this pallet's invariants hold. It exposes *no* admin calls, *no* general asset-management surface, and no configuration that can violate conservation.

The pallet is small, frozen early, and heavily verified (audit scope A per BE §24.14). All escrow lives as USDC balance on the pallet's derived sovereign account in `pallet-assets` (`ForeignAssets` instance) — plain balance, no holds required **[VERIFY holds support on pallet-assets in stable2606; fallback: transfer-to-pallet-account escrow, which is the design default — this fallback is also the default for position storage deposits, §4.3]** — with existential state maintained by the §7 R-4 genesis endowment of exactly `min_balance` (a 1-unit endowment, the superseded wording here, is below `min_balance` and would provide no existential protection at all — `pallet-assets` reserves the whole `min_balance`, not one unit).

---

## 2. Instrument model

### 2.1 Position identity

Position identity is a pure function of its coordinates. The canonical SCALE definitions are frozen in [`02-integration-contract.md`](./02-integration-contract.md); this is the owning specification. Relative to BE §7 this is a pre-implementation type change (greenfield — no storage migration exists to run).

```rust
pub enum Branch { Accept, Reject }
pub enum GateType { Survival, Security }              // gate ∈ {S, C}
pub enum ScalarSide { Long, Short }                   // canonical name per doc 02

/// 7 kinds per branch → 14 instrument kinds per proposal vault.
pub enum PositionKind {
    BranchUsdc,                 // branch-conditional USDC
    Long, Short,               // decision-scalar pair on settlement score s
    GateYes(GateType),         // binary gate pair, per branch, per gate  (B-2)
    GateNo(GateType),
}

pub enum PositionId {
    Proposal { proposal: ProposalId, branch: Branch, kind: PositionKind },
    Baseline { epoch: EpochId, side: ScalarSide },     // Baseline market home (B-3)
}
```

- **Proposal instruments**: `2 branches × (1 BranchUsdc + 2 scalar + 2×2 gate) = 14` kinds per vault. Gate kinds exist for every vault and carry non-zero supply for every market-bearing class (PARAM, TREASURY, CODE, META).
- **Baseline instruments**: 2 kinds per epoch — LONG/SHORT on the epoch settlement score `s_e`, collateralized in USDC **directly, with no branch layer** (the Baseline market is unconditional).

`PositionId` max encoded length ≤ 16 B; append-only `#[codec(index)]` discipline per BE §7 applies to all enums above.

### 2.2 Vault records

Per-branch supply fields replace the defective single `branch_pairs` counter (B-4):

```rust
pub struct BranchSupply {
    pub usdc: Balance,              // outstanding branch-USDC supply
    pub scalar_sets: Balance,      // Q_b: complete LONG/SHORT sets outstanding
    pub gate_sets: [Balance; 2],   // G_{b,S}, G_{b,C}: complete YES/NO sets per gate
}

pub struct VaultInfo {
    pub escrowed: Balance,                 // E
    pub branches: [BranchSupply; 2],       // [Accept, Reject]
    pub state: VaultState,
    pub gate_outcomes: [Option<bool>; 2],  // winning-branch breach outcomes [S, C], set by settle_gate
    pub spec: MetricSpecVersion,
}

pub struct BaselineVaultInfo {
    pub escrowed: Balance,                 // E_base
    pub sets: Balance,                     // complete Baseline LONG/SHORT sets outstanding
    pub state: BaselineState,
}
```

Max encoded lengths: `VaultInfo ≤ 224 B`, `BaselineVaultInfo ≤ 64 B` (PoV budget inputs, rolled up in [`13-parameters.md`](./13-parameters.md)).

In `Open` state the ledger MUST maintain `supply(Long_b) = supply(Short_b) = scalar_sets_b` and `supply(GateYes_{b,g}) = supply(GateNo_{b,g}) = gate_sets_b[g]` — scalar and gate legs are minted and burned only in pairs before terminal states. In terminal states (§2.3) redemption burns legs asymmetrically and the invariant switches to the valuation inequalities of §9.

### 2.3 Vault state machines (D-8: no reopening)

```rust
pub enum VaultState {
    Open,
    Resolved(Branch),                              // winning branch; losing-branch claims frozen
    ScalarSettled { winner: Branch, s: FixedU64 }, // carries the winner redeem() needs (B-low)
    Voided,                                        // D-1 / X-6
    BaselineSettled { s: FixedU64 },               // Baseline position-view projection only
}

pub enum BaselineState { Open, Settled(FixedU64) }
```

`BaselineSettled { s }` is the shared [`PositionView`](./02-integration-contract.md) projection
spelling for a settled Baseline position; proposal-vault storage MUST never enter that variant.
The storage representation makes Baseline state separate: `BaselineVaults` stores
`BaselineVaultInfo { state: BaselineState }`, and the runtime maps
`BaselineState::Settled(s)` to `VaultState::BaselineSettled { s }` only when constructing the
shared view. The exclusion is not made unrepresentable inside proposal `VaultInfo`, whose
`state` field uses the shared `VaultState`: instead, the exhaustive proposal transition methods
below never assign `BaselineSettled`, and ledger `try-state` rejects any proposal-vault record
that contains it.

Transitions (exhaustive; anything absent is impossible and MUST error):

| From | To | Trigger | Notes |
|---|---|---|---|
| `Open` | `Resolved(w)` | `resolve` (ResolveAuthority) | exactly once (I-3) |
| `Open` | `Voided` | `void` (ResolveAuthority) | pre-decision VOID (constitutional emergency, PB-ORACLE-VOID before decide) |
| `Resolved(w)` | `ScalarSettled{w,s}` | `settle_scalar` (SettleAuthority) | at cohort settlement e+3 |
| `Resolved(w)` | `Voided` | `void` (ResolveAuthority) | measurement-window VOID (disputed gate input, BE §15.2(7)) |
| `Baseline Open` | `Settled(s)` | `settle_baseline` (SettleAuthority) | measured settlement at epoch e+3; the cohort-VOID and orphan-epoch paths instead settle when their owning transitions fire, both at the neutral `s = 0.5` (§5.2; §6.4) |

`ScalarSettled` and `Voided` are terminal (redemption-only) and mutually exclusive. There is **no transition out of any terminal state and no transition back to `Open`**: per **D-8**, forecast trading is cut from v1, books close at branch resolution, and no vault state readmits minting — `split`, `split_scalar` and `split_gate` require `Open` strictly, which removes the reopened-book / `split_scalar`-requires-`Open` deadlock of BE §11.5 by removing the reopened book (owning text: [`04-markets-and-pricing.md`](./04-markets-and-pricing.md)).

**Outflow monotonicity (new, load-bearing for §6.4):** escrow outflows are admitted only in `Open` (via `merge`), `Resolved` (via `merge` only), and the terminal states. `Resolved` admits **no unpaired redemption** — winning branch-USDC redeems only from `ScalarSettled` (this is a deliberate tightening of BE §5.2.1, which allowed `redeem` at `Resolved`; rationale in §6.4: VOID is reachable *from* `Resolved`, and unpaired par redemptions before the `Resolved → Voided` fork would break VOID conservation exactly as in B-1).

---

## 3. Config

```rust
pub trait Config: frame_system::Config {
    type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
    type Collateral: fungibles::Mutate<Self::AccountId, AssetId = AssetId, Balance = Balance>; // USDC
    type UsdcAssetId: Get<AssetId>;
    /// Internal: pallet-market only — the D-3 wrapper's ledger operations (§6).
    type MarketAuthority: EnsureOrigin<Self::RuntimeOrigin>;
    /// Internal: pallet-epoch only — resolve() and void().
    type ResolveAuthority: EnsureOrigin<Self::RuntimeOrigin>;
    /// Internal: the welfare-owned ledger settlement boundary — measured and
    /// neutral Baseline paths both terminate here (§5.2; doc 05 §6).
    type SettleAuthority: EnsureOrigin<Self::RuntimeOrigin>;
    type MaxPositionsPerAccount: Get<u32>;   // 64 (normative value: §13)
    type PositionDeposit: Get<Balance>;      // 0.1 USDC per Positions entry (normative value: §13)
    /// POL, book, treasury sub-accounts, INSURANCE: exempt from the position cap (D-disposition)
    /// and from the storage deposit (specified here; these accounts are protocol-owned and bounded).
    type ProtocolAccounts: Contains<Self::AccountId>;
    type PalletId: Get<PalletId>;
    type WeightInfo: WeightInfo;
}
```

---

## 4. Storage (structural or economic bounds; max-size arguments)

| Item | Type | Max size argument |
|---|---|---|
| `Vaults: map ProposalId → VaultInfo` | §2.2 | ≤ `MaxLiveProposals(=32)` live + ≤ `MaxSettlingCohorts(=4)·N_active(=5)` settling. Terminal vaults become permissionlessly prefix-drainable after `RedemptionArchiveDelay` (hard maximum one year) and their residue is swept per §7.2, but eligibility is not a structural retention deadline: claimant rows are economically bounded and may be replenished by legal `Voided` transfers until the vault is actually reaped |
| `BaselineVaults: map EpochIndex → BaselineVaultInfo` | §2.2 | Open/market-backed entries consume the 196 active-market envelope. Terminal entries become permissionlessly prefix-drainable after the one-year-bounded archive delay; their cleanup duration is economically, not structurally, bounded by claimant positions. Only a `Settled` vault is reapable (§5.4), so every opened Baseline vault MUST reach `Settled` — §5.2's two neutral paths close the cases (voided cohort, and an epoch whose cohort never formed) that the measured e+3 settlement does not reach |
| `Positions: double_map (PositionId, AccountId) → Balance` | — | key order `(PositionId, AccountId)` so **per-vault reaping drains a prefix** (14 prefixes per proposal vault, 2 per Baseline vault) — fixes the un-reapable `(AccountId, PositionId)` order of BE §5.2.1; per-account bound via `PositionCount`; global growth priced by `PositionDeposit` (economic bound) — the map has no structural global bound, and this is stated honestly: the deposit is the bound |
| `PositionCount: map AccountId → u32` | — | ≤ `MaxPositionsPerAccount(=64)` enforced on every entry creation for non-`ProtocolAccounts`; incremented/decremented with entry lifecycle |
| `PositionTotals: map PositionId → Balance` | supply per instrument | 14 per live proposal vault + 2 per live Baseline vault |

**Storage deposit.** Creating a `Positions` entry for a non-protocol account takes `PositionDeposit = 0.1 USDC` from the entry owner (the *recipient* on `transfer`), held by the pallet (transfer-to-sovereign accounting per the §1 fallback **[VERIFY pallet-assets holds]**), refunded when the entry is deleted (balance → 0, including via reap). Deposits are accounted outside `escrowed` and can never be netted against escrow (conservation is deposit-blind). Together with R-2 (below-minimum transfers move the whole balance) this prevents dust-entry litter and prices the position-cap-dusting threat ([`14-threat-model.md`](./14-threat-model.md)).

---

## 5. Calls

All calls transactional/atomic; permissionless Signed unless noted. Weight drivers are benchmark inputs, not weights.

### 5.1 Minting and transfer (state: `Open` only for all `split*`)

| Call | Origin | Preconditions | Effect | Event | Weight drivers |
|---|---|---|---|---|---|
| `split(pid, a)` | Signed | vault `Open`; `a ≥ MinSplit`; USDC transfer succeeds | `E += a`; mint `a` AcceptUsdc + `a` RejectUsdc to caller | `Split` | 1 asset transfer + 3 map writes |
| `merge(pid, a)` | Signed | vault `Open`, `Resolved`, or `Voided`; caller holds `a` of both branch-USDC | burn both; `E −= a`; transfer `a` USDC out | `Merged` | as split |
| `split_scalar(pid, b, a)` | Signed | vault `Open`; caller holds `a` `b`-USDC | burn `a` `b`-USDC; mint `a` LONG_b + `a` SHORT_b; `usdc_b −= a; Q_b += a` | `ScalarSplit` | 3–5 writes |
| `merge_scalar(pid, b, a)` | Signed | vault `Open`, `Resolved`, or `Voided`; caller holds `a` of both legs | inverse | `ScalarMerged` | — |
| `split_gate(pid, b, g, a)` | Signed | vault `Open`; caller holds `a` `b`-USDC | burn `a` `b`-USDC; mint `a` GateYes(g)_b + `a` GateNo(g)_b; `usdc_b −= a; G_{b,g} += a` | `GateSplit` | 3–5 writes |
| `merge_gate(pid, b, g, a)` | Signed | vault `Open`, `Resolved`, or `Voided`; caller holds `a` of both legs | inverse | `GateMerged` | — |
| `transfer(position_id, to, a)` | Signed | vault `Open`, `Resolved`, or `Voided`; whole units ≥ `MinTransfer`; recipient is **not** a `ProtocolAccount`; recipient under cap; recipient deposit taken | move balance | `PositionTransferred` | 2–4 writes + deposit transfer |
| `split_baseline(epoch, a)` | Signed | Baseline vault `Open`; `a ≥ MinSplit` | `E_base += a`; mint `a` B-LONG + `a` B-SHORT | `BaselineSplit` | as split |
| `merge_baseline(epoch, a)` | Signed | Baseline vault `Open`; caller holds both legs | inverse | `BaselineMerged` | — |

Merge availability in `Resolved` and `Voided` is deliberate: **a complete Accept+Reject pair recovers par (1 USDC per pair) in every non-`ScalarSettled` state** — this is the D-1 primary recovery path. `merge_scalar`/`merge_gate` are value-neutral in every state (a complete set is worth exactly one branch-USDC under every valuation of §6.4) and stay available in `Voided` so set holders can climb back to branch-USDC and then to par. `transfer` stays available in `Voided` so counterparties can assemble pairs. In `ScalarSettled` the redemption calls (§5.3) subsume all of these.

Gate splitting is economically meaningful only for gate-book classes; the ledger does not restrict it by class (a gate set is fully collateralized regardless) — class policy lives in [`04-markets-and-pricing.md`](./04-markets-and-pricing.md).

### 5.2 Authority calls

| Call | Origin | Preconditions | Effect | Event | Weight drivers |
|---|---|---|---|---|---|
| `resolve(pid, w)` | `ResolveAuthority` | vault `Open`; exactly once (I-3) | state → `Resolved(w)`; losing-branch positions frozen (not burned) | `VaultResolved` | O(1) |
| `void(pid)` | `ResolveAuthority` | vault `Open` or `Resolved`; **not** `ScalarSettled`; once | state → `Voided`; all positions (both branches) unfrozen for merge/`redeem_void` | `VaultVoided` | O(1) |
| `settle_scalar(pid, s)` | `SettleAuthority` | vault `Resolved(w)`; `s ∈ [0,1]` (FixedU64, 1e9) | state → `ScalarSettled{winner: w, s}` | `ScalarSettlementSet` | O(1) |
| `settle_gate(pid, g, outcome)` | `SettleAuthority` | vault `Resolved` or `ScalarSettled`; `gate_outcomes[g]` unset | record winning-branch breach outcome for gate `g` | `GateSettled` | O(1) |
| `settle_baseline(epoch, s)` | `SettleAuthority` | Baseline vault `Open`; once | state → `Settled(s)` | `BaselineSettled` | O(1) |

- `void` is entered on the fail-static paths of BE §15.2(7)/PB-ORACLE-VOID and D-9 outcomes: pre-decision (from `Open`) or during measurement (from `Resolved`, when a disputed gate-input component voids the cohort). `void` from `ScalarSettled` MUST error (`WrongVaultState`): redemptions at `s` may already have paid out, and a retroactive VOID would break conservation. The `VaultVoided` event is the `Voided`/T20 event frozen in [`02-integration-contract.md`](./02-integration-contract.md) (X-11f).
- `settle_gate` records the outcome of the deterministic `C_onchain`/S daily breach-flag question for the **winning branch only** (losing-branch gate instruments died at `resolve`). Both `settle_scalar` and the two `settle_gate` calls ride the single settlement path `pallet-epoch::settle_cohort → pallet-welfare::compute_settlement → ledger` at cohort settlement **e+3** (sequencing owned by [`05-welfare-and-decision-engine.md`](./05-welfare-and-decision-engine.md)).
- On the measured path, `settle_baseline(epoch, s_e)` settles the epoch's Baseline vault at epoch settlement **e+3**. Where no measured `s_e` exists, the SettleAuthority settles the Baseline vault at the neutral `s = 0.5` — for a branch-free scalar vault this is *identical in payout* to D-1's neutral valuation, so the Baseline vault needs no `Voided` state (§6.4). **Owning transitions (normative).** The neutral settlement has **exactly two** owning transitions, both specified in [`05-welfare-and-decision-engine.md`](./05-welfare-and-decision-engine.md) §7 and neither restated here: **(a)** the **epoch-VOID path** — the cohort void of §7(5) there, in the same transaction that sets `CohortInfo.status = Void`; and **(b)** the **orphan-epoch finalization path** — the permissionless `pallet-epoch::finalize_epoch_baseline(epoch)` of §7(6) there, for a strictly past epoch whose cohort never formed, admissible only under the three-part precondition that document states normatively. Nothing else settles a Baseline vault neutrally. In particular, per-proposal `void(pid)` (T20 on a single vault) is **not** one of them and settles **no** Baseline, because the Baseline vault is keyed per *epoch*, not per proposal — an arbitrary individual T20 may leave sibling proposals live and the cohort reachable, so only the separate finalizer proves the whole epoch terminal. The settlement is mandatory and unconditional on path (a) and permissionless-on-demand on path (b); because both redemption calls of §5.3 require `Settled`, an unsettled Baseline vault permanently strands every single-sided holder while pair holders still exit at par through `merge_baseline`, so the omission is invisible to §6.4/§6.5's conservation invariants — which is precisely why path (b) exists rather than being left to a later repair (R-7: prefer the reading that cannot strand a claim). Implementations MUST treat "no Baseline vault for the epoch" and "already `Settled`" as no-ops rather than failures on **both** paths — neither a VOID nor a finalization may fail on this leg (G-1).

### 5.3 Redemption calls (terminal states only)

| Call | Origin | Preconditions | Payout | Event |
|---|---|---|---|---|
| `redeem(pid, a)` | Signed | `ScalarSettled{winner: w, ..}`; caller holds `a` winning (`w`) branch-USDC | `a` (1:1) | `Redeemed` |
| `redeem_scalar(pid, kind, a)` | Signed | `ScalarSettled{winner: w, s}`; `kind ∈ {Long, Short}` of branch `w` | LONG: `floor(a·s)`; SHORT: `floor(a·(1−s))` (B-5) | `ScalarRedeemed` |
| `redeem_scalar_pair(pid, a)` | Signed | `ScalarSettled{winner: w, ..}`; caller holds `a` LONG_w **and** `a` SHORT_w | exactly `a` (atomic; no double flooring) | `ScalarPairRedeemed` |
| `redeem_gate(pid, g, a)` | Signed | `ScalarSettled`; `gate_outcomes[g]` recorded; caller holds `a` of the *winning side* (`GateYes` if breach, `GateNo` if not) of the winning branch | `a` (1:1); losing side pays 0 and is reap-only | `GateRedeemed` |
| `redeem_void(pid, kind_coords, a)` | Signed | vault `Voided`; caller holds `a` of the instrument | branch-USDC: `floor(a/2)`; LONG/SHORT/GateYes/GateNo: `floor(a/4)` (D-1; §6.4) | `VoidRedeemed` |
| `redeem_baseline(epoch, kind, a)` | Signed | Baseline `Settled(s)` | LONG: `floor(a·s)`; SHORT: `floor(a·(1−s))` | `BaselineRedeemed` |
| `redeem_baseline_pair(epoch, a)` | Signed | Baseline `Settled`; caller holds `a` of both legs | exactly `a` | `BaselineRedeemed` |

All payouts decrement `escrowed` and burn the redeemed instruments atomically; all divisions round **against the redeemer and in favor of escrow** (R-1); `s` multiplication uses u256 intermediates at 1e9 scale.

The D-1 quarter-value rule is stated for LONG/SHORT; its application to `GateYes`/`GateNo` is a consistent extension recorded here (each gate leg is one side of a binary claim on a branch worth ½ under VOID, hence ¼ — identical in structure to the scalar legs at neutral `s = 0.5`). It does not alter any frozen constant; the conservation argument covering it is §6.4.

### 5.4 Housekeeping

| Call | Origin | Preconditions | Effect | Event |
|---|---|---|---|---|
| `sweep_dust(pid)` / `sweep_dust_baseline(epoch)` | Signed (keeper) | vault terminal + `RedemptionArchiveDelay` elapsed (hard maximum one year) | drain ≤ `ReapBatch(=100)` claimant `Positions` entries per call across the vault's 14 (resp. 2) `PositionId` prefixes; refund deposits to entry owners; residual escrow → INSURANCE; storage and terminal marker reaped when drained. This cleanup is independent of the owning market-book reap | `VaultReaped { pid, residue }` (proposal crank) / `BaselineVaultReaped { epoch, residue }` (Baseline crank) — each identifies its vault; only the name `VaultReaped` is frozen in [`02-integration-contract.md`](./02-integration-contract.md) §6 (fields open) |
| `reconcile()` | Signed (keeper) | checked `TotalEscrowed + DepositsHeld` succeeds | compare the O(1) maintained liability with the sovereign's actual USDC custody; set the persistent I-4 drift latch iff `liability > custody`; record the exact sample. Emit only on a latch edge | `LedgerDriftDetected { liability, custody }` / `LedgerDriftCleared { liability, custody }` |

**Reconciliation accounting is exact and bounded (normative).** `TotalEscrowed` is a
checked maintained total over every proposal and Baseline vault's `escrowed` field. Every
escrow delta updates it in the same storage transaction as the vault post-image and real
USDC transfer; terminal reap subtracts the residue in that same transaction. `reconcile`
therefore performs O(1) storage work and never scans claimant-retained vaults, whose count is
not structurally bounded after their owning market is reaped. The full `try-state` audit still
re-sums both vault maps and requires equality with `TotalEscrowed`, so the bounded crank is
not a second or weaker accounting definition. Its liability is
`TotalEscrowed + DepositsHeld`, using checked arithmetic. The R-4 genesis endowment and any
swept/direct-transfer dust are lawful surplus: they are neither subtracted nor used to demand
equality. The exact I-4 anomaly is only `liability > custody`.

**Legacy backfill is multi-block (normative).** The runtime upgrade that first introduces
`TotalEscrowed` MUST NOT scan either unbounded vault map in `on_runtime_upgrade`. It runs as a
weight-metered `pallet-migrations` cursor over proposal vaults and then Baseline vaults, carrying
only the last key and checked partial sum. The chain remains in the migration framework's
`OnlyInherents` posture until the final total and storage-version write commit atomically; no v1
ledger call can observe the default zero mirror. Every row step pre-charges a conservative
ref-time and proof-size bound, and insufficient weight makes no progress rather than overrunning
the block. The release declares a finite step ceiling strictly below
`MIGRATION_STALL_BLOCKS`; pre-upgrade verification rejects a legacy row population that would
exceed that ceiling.

The paired terminal-recovery profile registers zero SDK migrations. Because this particular
backfill is read-only before its terminal write, its release-specific repair validates that the
retired cursor names this sole segment, discards the untrusted partial sum, and restarts the same
bounded scan from the beginning under a runtime-local recovery cursor. `OnlyInherents` remains
set across every repair block. The recovery image clears its locks and commitment only in the
same transaction that writes the exact mirror and storage version; malformed rows, overflow,
wrong segment identity or bound exhaustion leave the chain locked. The release gate exercises
the start, every proposal/Baseline cutpoint, the phase boundary, the terminal boundary and the
framework's `Stuck` form.

**Terminal markers are swept state (normative).** A vault's terminal markers exist to gate this housekeeping and are removed by it; they are therefore **not** a durable signal any other pallet may key on for long-lived POL accounting. Every transition that records a terminal block — `void`, `settle_scalar`, `settle_baseline` — MUST, in the same atomic storage layer, latch that block into the owning market for each of the vault's books, release the active-market/POL slot and delete that terminal book's auxiliary checkpoint/window state, so the durable market latch survives this sweep. Latching MUST be idempotent, and marker, latch, active-slot release and POL release MUST commit or roll back together — a latch failure rolls the terminal transition back (status-quo default, G-1) rather than leaving a marker this sweep would later delete. The inverse identity is also machine-checked while both sides remain: every retained owning book named by a ledger terminal marker MUST carry the same latch. The obligation binds the **runtime composition** that wires ledger to market, not this pallet's dispatchables in isolation, which write only the marker.

**Protocol inventory at market reap (normative).** Every per-market book and fee address belongs to a canonical, domain-separated `AccountId32` namespace reserved permanently — before creation, throughout the book lifetime, and after reap. `MarketProtocolAccounts` is only the bounded ownership/refcount index; inserting or removing it MUST NOT change deposit classification. Market creation MUST reject a non-canonical pair before creating a vault or index entry. Signed `transfer` MUST reject every `ProtocolAccount` destination (`ProtocolDestination`), including a predictable future book/fee address; the origin-gated `MarketAuthority` wrapper is the sole position ingress, so pre-creation squatting cannot poison an address, reclassify a deposit-backed claimant row, or wedge market creation. Immediately before one archived market row unregisters its two accounts, `MarketAuthority` MUST atomically discard positions owned by exactly those accounts across exactly that book's owning vault universe: 14 fixed proposal instruments (≤ 28 storage cells) or two fixed Baseline instruments (≤ 4 cells). It MUST decrement `PositionTotals` by the discarded balances, move no collateral or held deposit, and touch no claimant-owned row. If any later step of market reap fails, this discard rolls back with it (G-1). The vault and its claimant rows remain independently redeemable/sweepable, so ledger-first and market-first interleavings are both safe.

The BE §5.2.1 note on SGF §9.3 settlement perpetuity carries forward unchanged: after reaping, unredeemed claims remain redeemable through a Merkle-archived claims procedure executed by a TREASURY-class proposal (deliberate v1 compromise, recorded in BE §31).

### 5.5 Internal API for the D-3 trade wrapper (no extrinsic surface)

The buy/sell auto-split wrapper lives in `pallet-market`; its *mechanics and sequencing* are owned by [`04-markets-and-pricing.md`](./04-markets-and-pricing.md). The ledger owns the internal Rust call surface `pallet-market` consumes, all of it gated by `MarketAuthority` and none of it carrying an extrinsic surface. Two families exist: the **vault-construction** pair, which runs atomically during book deployment *before* POL seeding (a vault must exist before anything can be split into it), and the **D-3 `do_*` family**, which runs atomically inside the trade extrinsic:

```rust
// All perform the same state transitions, checks and events as the corresponding extrinsics,
// on behalf of `who`; no origin other than MarketAuthority can reach them.
fn create_vault(pid, spec);          // one Open proposal vault, created at its first book's
                                     // deployment; the proposal's books share it (SQ-33)
fn create_baseline_vault(epoch);     // one Open Baseline vault per epoch, at Baseline-book seed
fn do_split(pid, who, a);            // buy leg: split cost c USDC → c AcceptUsdc + c RejectUsdc to buyer
fn do_transfer(position_id, from, to, a);   // pay target-branch c branch-USDC into the book;
                                            // the mirror-branch c branch-USDC REMAINS WITH THE BUYER (D-3)
fn do_split_scalar(pid, b, who, a);  // book revenue immediately scalar-split into complete
                                     // LONG+SHORT sets held by the book (solvent at any s)
fn do_split_gate(pid, b, g, who, a); // same recycling for gate-book revenue (YES+NO sets)
fn do_split_baseline(epoch, who, a); // Baseline wrapper leg and Baseline revenue recycling
fn do_merge(...); fn do_merge_scalar(...); fn do_merge_gate(...); fn do_merge_baseline(...); // sell path
```

Consequences the ledger guarantees (and the property tests assert): a wrapper buyer always ends the trade holding the purchased target leg plus mirror-branch branch-USDC equal to their paid cost, so on normal losing-branch resolution the mirror redeems at par and under VOID the package receives its D-1 neutral valuation after exact pairs are merged first (§6.4; **not** par in general — SQ-171); book revenue never sits as bare branch-USDC across a block boundary — it is recycled into complete sets in the same extrinsic, so every LMSR obligation stays pre-collateralized in the ledger (I-12).

---

## 6. Accounting semantics

State equations (all checked arithmetic; overflow/underflow aborts the extrinsic — with per-branch supplies no legal flow can underflow, §6.2):

```
split(a):              E += a;  usdc_Acc += a;  usdc_Rej += a
merge(a):              E −= a;  usdc_Acc −= a;  usdc_Rej −= a          // payout a
split_scalar(b,a):     usdc_b −= a;  Q_b += a
merge_scalar(b,a):     usdc_b += a;  Q_b −= a
split_gate(b,g,a):     usdc_b −= a;  G_{b,g} += a
merge_gate(b,g,a):     usdc_b += a;  G_{b,g} −= a
resolve(w):            freeze branch ≠ w
void():                unfreeze all; enter Voided
settle_scalar(s):      record s;    settle_gate(g,o): record o
redeem(a):             E −= a;  usdc_w −= a                            // payout a
redeem_scalar(L,a):    E −= floor(a·s);      burn a LONG_w            // supplies diverge from Q_w
redeem_scalar(S,a):    E −= floor(a·(1−s));  burn a SHORT_w
redeem_scalar_pair(a): E −= a;  Q_w −= a;  burn a LONG_w + a SHORT_w
redeem_gate(g,a):      E −= a;  burn a winning-side leg
redeem_void(bUSDC,a):   E −= floor(a/2);  burn a branch-USDC
redeem_void(leg,a):    E −= floor(a/4);  burn a scalar/gate leg
split_baseline(a):     E_base += a;  sets += a
merge_baseline(a):     E_base −= a;  sets −= a                        // payout a
redeem_baseline*:      as redeem_scalar / redeem_scalar_pair against E_base
```

### 6.1 Conservation identity (per branch — B-4)

For every proposal vault in `Open` (and unchanged through `Resolved` and into `Voided`, since those states admit only balanced pair operations):

```
E = usdc_b + Q_b + G_{b,S} + G_{b,C}        for EACH b ∈ {Accept, Reject}   (L-1)
```

equivalently `E = supply(AcceptUsdc) + Q_Acc + G_{Acc,S} + G_{Acc,C} = supply(RejectUsdc) + Q_Rej + G_{Rej,S} + G_{Rej,C}` — the BE §10.2 identity extended over the scalar **and gate** sets and stored per branch. Dual minting keeps the two right-hand sides equal; every intra-branch op moves value between `usdc_b`, `Q_b`, `G_{b,g}` of the *same branch* and each such op checks its own decremented field ≥ a. **There is no cross-branch counter to underflow.** For Baseline vaults: `E_base = sets = supply(B-LONG) = supply(B-SHORT)` while `Open`.

### 6.2 The POL seeding flow, re-walked (B-4)

TREASURY example (6 books for every ask size). Per branch the seed needs `D = pol.b·ln 2 + headroom_dec` for the decision book (`pol.b = 25,000` ⇒ `D ≈ 17,328.7 + h`) and `G = pol.b_gate·ln 2 + headroom_gate` per gate book (`pol.b_gate = 7,500` ⇒ `G ≈ 5,198.6 + h_g`); values normative in [`13-parameters.md`](./13-parameters.md), headroom sizing in [`04-markets-and-pricing.md`](./04-markets-and-pricing.md). Let `T = D + 2G`.

| Step | Op | E | usdc_Acc | Q_Acc | G_Acc,S | G_Acc,C | usdc_Rej | Q_Rej | G_Rej,S | G_Rej,C | L-1 (Acc / Rej) |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | `split(T)` | T | T | 0 | 0 | 0 | T | 0 | 0 | 0 | T=T / T=T ✓ |
| 2 | `split_scalar(Acc, D)` | T | 2G | D | 0 | 0 | T | 0 | 0 | 0 | 2G+D=T ✓ / T ✓ |
| 3 | `split_scalar(Rej, D)` | T | 2G | D | 0 | 0 | 2G | D | 0 | 0 | ✓ / ✓ |
| 4–5 | `split_gate(Acc, S, G)`, `(Acc, C, G)` | T | 0 | D | G | G | 2G | D | 0 | 0 | 0+D+G+G=T ✓ / ✓ |
| 6–7 | `split_gate(Rej, S, G)`, `(Rej, C, G)` | T | 0 | D | G | G | 0 | D | G | G | ✓ / ✓ |

No decrement ever exceeds its own field. Under the superseded single-counter rule (`escrowed == branch_pairs + Σ scalar_sets`, BE §5.2.1) the same flow drove `branch_pairs: T → T−D → T−2D`, underflowing at step 3 whenever `D > T/2` (always true here, since `D > 2G`) — every market seeding aborted with `ArithmeticOverflow`, or try-state fired a spurious S1 incident on a healthy vault. That defect is structurally impossible in the per-branch form.

Both branches' seeds are dual-minted, so POL is decision-neutral by construction (BE §17.3 carries forward); POL positions MUST remain undisturbed through the decision window (decision-grade condition, owned by [`04-markets-and-pricing.md`](./04-markets-and-pricing.md)). Baseline seeding is the degenerate case: `split_baseline(pol.b_baseline·ln 2 + headroom)` — `pol.b_baseline` is a new parameter whose value lives in [`13-parameters.md`](./13-parameters.md).

### 6.3 Scalar redemption rounding (B-5)

Superseded rule: unpaired SHORT paid `a − floor(a·s)` — rounding *against escrow*. Counterexample (from the review, now a mandatory regression vector): `s = 0.70005`, `E = Q_w = 20,000`; one holder of 20,000 LONG, two holders of 10,000 SHORT each:

| Rule | LONG pays | each SHORT pays | Σ payouts | vs E |
|---|---|---|---|---|
| old: SHORT `a − floor(a·s)` | 14,001 | 10,000 − 7,000 = 3,000 | **20,001** | insolvent (+1) |
| new: SHORT `floor(a·(1−s))` | 14,001 | `floor(2,999.5)` = 2,999 | **19,999** | conserving (residue 1 swept) |

Normative rules: unpaired LONG pays `floor(a·s)`; unpaired SHORT pays `floor(a·(1−s))`; since `floor(a·s) + floor(a·(1−s)) ≤ a` for all `a, s`, fragmentation can never over-draw escrow. A holder of complete pairs uses the atomic `redeem_scalar_pair`, which pays **exactly `a`** — no complete-set holder loses a unit to double flooring (the R-1 guarantee, now achieved without the old rule's compensating over-payment). Identical rules apply to Baseline redemption.

### 6.4 VOID semantics and conservation (B-1, X-6, D-1)

Under `Voided` (normative values: [`00-decision-record.md`](./00-decision-record.md) Part 3 / [`13-parameters.md`](./13-parameters.md)):

- `merge`, `merge_scalar`, `merge_gate` remain enabled, but **only completeness through *both* ledger layers recovers par**, and the layers must not be conflated. `merge` of an **Accept+Reject branch-USDC** pair pays **1 USDC**. `merge_scalar`/`merge_gate` of a **same-branch** LONG+SHORT or YES+NO set pay no USDC at all: they mint one *same-branch* branch-USDC, which is worth `0.5` under VOID unless it is then paired with its opposite-branch counterpart. A holder of only same-branch complete sets therefore recovers 0.5 per unit, not par.
- `redeem_void`: unpaired branch-USDC pays `floor(a/2)`; unpaired LONG/SHORT/GateYes/GateNo pays `floor(a/4)`. Rounding against the redeemer; residue swept per R-5.

**Conservation argument.** Assign the VOID valuation `v(branch-USDC) = ½`, `v(any scalar or gate leg) = ¼`. Total claim value at entry to `Voided`:

```
V = Σ_b [ ½·usdc_b + ¼·(2·Q_b) + ¼·(2·G_{b,S}) + ¼·(2·G_{b,C}) ]
  = ½ · Σ_b [ usdc_b + Q_b + G_{b,S} + G_{b,C} ]
  = ½ · (E + E)          by L-1, which holds at entry (§2.3 outflow monotonicity)
  = E.
```

Every operation available under `Voided` pays at most its burned value: `merge` burns value ½+½ and pays 1; `merge_scalar`/`merge_gate` burn ¼+¼ and mint ½ (no payout); `redeem_void` pays `floor(a/2) ≤ a·v` resp. `floor(a/4) ≤ a·v`. Hence **total payout ≤ E on every path**, with equality only when every instrument is redeemed pair-complete and no flooring loss occurs.

**The B-1 counterexample, re-run.** Split 100 USDC → 100 AcceptUsdc + 100 RejectUsdc; vault voided; `E = 100`:

| Path | Payout | Superseded §10.5 rule ("both kinds redeem 1:1") |
|---|---|---|
| `merge(100)` | **100** (par) | 200 — first redeemers drain the vault: insolvent by 2× |
| `redeem_void` both sides unpaired | 50 + 50 = **100** | — |
| any mix (merge k pairs, redeem rest) | k + floor((100−k)/2)·2/… ≤ **100** | — |

**Why `void` is barred from `ScalarSettled`, and `redeem` from `Resolved` (§2.3).** If unpaired winning-branch redemptions at par were allowed in `Resolved`, a later `Resolved → Voided` transition would find `E` reduced by the redeemed amount `a` while the *losing* branch's claim mass is undiminished — total VOID value `E + a/2 > E_remaining`, reproducing B-1's first-come drain at smaller scale. Deferring unpaired redemption to `ScalarSettled` (which is mutually exclusive with `Voided`) makes every terminal valuation exact. Complete pairs are unaffected: `merge` pays par in `Resolved` and is VOID-safe (it burns claim mass symmetrically in both branches).

**Annulment (PT-2), restated honestly (SQ-171).** Holders complete through **both** layers — i.e. holding the Accept and Reject branch-USDC of a pair, directly or after `merge_scalar`/`merge_gate` — recover full principal under VOID via `merge` (100 %). Same-branch completeness alone does not (see the payout bullets above). A D-3 wrapper buyer's package is the purchased **target scalar leg** (`amount` units) plus **mirror branch-USDC** equal to `cost`. Pair-first recovery values that package under the D-1 schedule above: for an isolated buy with no pairable offsets, recovery is `R = floor(amount/4) + floor(cost/2)`, so net delta against the `cost + fee` debit is `R − cost − fee` — **not `−fee`**. Ignoring dust the non-fee term is `(amount/2 − cost)/2`, which vanishes **iff the average execution price `cost/amount` is 0.5**. That is a property of the realized trade, not of the pre-trade quote: LMSR charges the integral of a rising curve, so a buy opening at a quote of 0.5 executes above 0.5 on average and recovers strictly less than `cost`. Across a portfolio, exact pairs merge at par first and only residual claims are floored. A deliberately unpaired single-branch holder — one who transferred away or sold the mirror — receives the same neutral valuation, **0.5 per branch-USDC unit and 0.25 per scalar leg, not par**: that is the correct price of a voided binary claim, not a haircut. What the protocol guarantees is that **no claim is valued below the D-1 schedule**, not that a market premium is refunded. The superseded wording ("the protocol-path buyer recovers par") over-claimed by implicitly assuming the neutral prior. The older "both branches redeem 1:1" statement of PT-2/BE §10.5 remains retired: it was the B-1 insolvency, not a guarantee.

### 6.5 Induction sketch (§10.3-style, over the full operation set)

Claim: in every reachable state, maximal remaining payouts ≤ escrow (per vault; summing over vaults gives I-4 against the sovereign balance).

Define the state-dependent claim bound `V(state)`:
- `Open`: `V = max_world payout = E` (by L-1: in world *b*, branch-USDC pays 1, a scalar set pays `s+(1−s)=1`, a gate set pays `1+0=1`, so world-b payout `= usdc_b + Q_b + ΣG_{b,g} = E`).
- `Resolved(w)`: only branch-w claims live; `V = usdc_w + Q_w + Σ_g G_{w,g} = E`. No outflow op except `merge` (pays 1 for cross-branch value 1+0 = 1, decrementing both sides of L-1).
- `ScalarSettled{w,s}`: `V = usdc_w·1 + [floor-bounded scalar claims ≤ Q_w-mass] + [gate winning side ≤ G-mass]`; each redemption op pays ≤ the exact value it burns (LONG `floor(a·s) ≤ a·s`, SHORT `floor(a·(1−s)) ≤ a·(1−s)`, pair exactly `a`, gate winning side `a`, losing side 0).
- `Voided`: `V = E` by §6.4, every op pays ≤ burned value.

Base: empty vault, `E = 0`, no claims. Inductive step: every operation in {split, merge, split_scalar, merge_scalar, split_gate, merge_gate, split_baseline, merge_baseline, transfer, resolve, void, settle_scalar, settle_gate, settle_baseline, redeem, redeem_scalar, redeem_scalar_pair, redeem_gate, redeem_void, redeem_baseline, redeem_baseline_pair, sweep_dust} either (i) changes `E` and `V` by equal amounts (split/merge families), (ii) leaves `E` fixed and `V` non-increasing (transfer: fixed; resolve: deletes losing claims; void: `V` maps from `E` to `E` by §6.4; settle_*: fixes a payout parameter within the already-counted bound), or (iii) decrements `E` by a payout ≤ the claim value it burns (all redemption ops, with flooring strictly escrow-favoring). `sweep_dust` moves residual `E` to INSURANCE after all claims are reaped. Hence `V − E` never increases from 0. ∎ (EFP §5.1 adapted; the enlarged instrument set changes the bookkeeping, not the argument — every new instrument enters and leaves `V` through balanced mint/burn pairs.)

---

## 7. Rounding, dust, fees, ED

Carried forward from BE §10.4 with amendments:

- **R-1.** All divisions round **against the claimant and in favor of escrow**. Complete-set exactness is provided by the atomic pair calls (`redeem_scalar_pair`, `redeem_baseline_pair`), *never* by rounding one leg up (the retired SHORT rule).
- **R-2.** `MinSplit = MinTransfer = ledger.min_split = 0.01 USDC (10^4 base units)` *(normative value: [`13-parameters.md`](./13-parameters.md))*; positions cannot be created below it; transfers leaving a remainder below it MUST move the whole balance. Both rules guard deposit-backed position hygiene and therefore bind Signed calls: market-wrapper moves (`MarketAuthority`, [`04-markets-and-pricing.md`](./04-markets-and-pricing.md) §6.1) are exact by construction and MAY carry the sub-`MinTransfer` fee legs of R-3, and destinations that are protocol accounts (books, fee accounts — deposit-exempt) are outside the creation floor.
- **R-3.** Fees (30 bps) are charged by `pallet-market` per the trade-path rule of [`04-markets-and-pricing.md`](./04-markets-and-pricing.md) §6.1: on `buy` the fee is collected as a **complete branch-USDC pair** (both legs to the market's `fees_accrued` account — worth exactly the fee in USDC at any settlement); on `sell` it is withheld **single-sided** in target-branch branch-USDC. To the ledger these are ordinary, fully collateralized positions created by the same split/transfer ops as any other — no special fee path exists, so fee handling cannot break conservation. Realized fee value routes 50% INSURANCE / 50% POL-offset at settlement (economics: doc 08). Fees are non-refundable on all paths, including VOID (buy-side fee pairs merge at par to the protocol; sell-side withheld legs follow their branch).
- **R-4.** USDC is a sufficient asset, `min_balance = 10^4`. Under `Preservation::Preserve` an account's reducible balance is `balance − min_balance`, so an endowment of exactly `min_balance` is a permanent floor no legal protocol flow can cross — that, and not a special case anywhere in the pallet, is the whole mechanism behind "can never be reaped". It follows that an endowment only binds where **every** custody path out of the account preserves; an `Expendable` outflow ignores the floor and reaps the account regardless. Concretely (amended 2026-07-21, milestone B14 — the endowment was previously specified but implemented nowhere):
  - **Statically derived protocol accounts are genesis-endowed with exactly `min_balance`**: the ledger sovereign and its `INSURANCE`, `POL`, `POL_BASELINE`, `FEES`, `BOOK` and `TREASURY` sub-accounts, plus the treasury's `MAIN`, `KEEPER`, `ORACLE` and `REWARDS` sub-accounts. Every **protocol-internal** custody path out of these MUST use `Preservation::Preserve`. One seam is outside that MUST and is stated rather than wished away: the ledger `TREASURY` sub-account is deliberately XCM-addressable (it is the `FutarchyTreasury` origin's account and the coretime beneficiary), and the SDK's `FungiblesAdapter` withdraws with `Preservation::Expendable` unconditionally, so a passed TREASURY-class payload can take that one account below its floor. The exposure is the floor itself — `min_balance`, re-creatable on the next inflow — and no invariant depends on that account's continuity; it is called out here so a later reader does not mistake the gap for an oversight, and so that no *new* Expendable seam is added to any of the other ten. The endowment is deliberately the minimum — genesis-minted USDC carries no Asset Hub reserve behind it — and the resulting slack above escrow is exactly the "genesis endowment" term §9's L-2 already allows for. Without it the **last full redeemer of the last open vault** cannot exit: the payout would reduce the sovereign below `min_balance` and fails `Token(NotExpendable)`, stranding the final claimant on a redemption §5.3 makes legal.
  - **Per-market accounts cannot be genesis-endowed** — their addresses embed a `MarketId` allocated at runtime, so they are not enumerable at genesis. They divide by whether they custody plain USDC at all. Per-market **fee** accounts and **decision/gate book** accounts custody conditional-ledger *positions* only: a scalar or gate merge leaves the vault's `escrowed` unchanged, so no custody moves and `min_balance` never binds them. A per-market **Baseline book** does hold plain USDC — it retains the sell-side fee ([04](./04-markets-and-pricing.md) §6.1) — and is therefore endowed with exactly `min_balance` **at Seed**, funded from the seeding `POL_BASELINE` account, that being the earliest point at which the account exists. Without that floor a Baseline sell whose fee is below `min_balance` fails under the very `Preserve` rule that protects the account, and since the failing band starts at `MinTrade` it is ordinary traffic, not an edge case. Two consequences are normative. First, `POL_BASELINE`'s standing requirement is `min_balance + seed_headroom(pol.b_baseline) + min_balance` per Baseline book, and the POL floor a deployment reports MUST include that per-book allowance — a floor computed without it names exactly the balance at which the endowment becomes unaffordable. Second, the endowment is **best-effort and MUST NOT be a hard precondition of opening the book**: if `POL_BASELINE` cannot afford it the book opens unendowed, degrading to the small-sell rejection above, because the alternative — failing the seed — propagates out of the epoch tick and reverts every proposal in the batch, converting a bounded trading limitation into a chain-wide liveness failure (G-1: the status quo is the *narrow* failure, never the broad one). The endowment MUST also be idempotent, so reruns and re-seeds cannot double-fund the book.
  - Accounts deliberately **not** endowed, and which MUST NOT be: the two registry sovereigns and the epoch bond-escrow account, whose payout paths use `Preservation::Expendable` by design precisely so that draining the last bond stays admissible. Endowing them would be inert, and flipping their preservation to match the pattern above would strand the last claimant — the mirror of the ledger failure this rule exists to prevent.
  - The Baseline book's floor, and any fee USDC accrued on top of it, remain in the account after the market is reaped — `Market::reap` unregisters the account without sweeping its balance. That residue is **currently unrecoverable**, and this rule deliberately does not claim otherwise: R-6 is scoped to USDC that arrived *outside* protocol flows, which this did not, and `recover_foreign` in any case refuses `Usdc` outright and moves no custody. The standing exposure is `min_balance` plus retained fees per reaped Baseline book — bounded, non-solvency-affecting (it is escrow-favouring dust, never an unbacked claim), and left as an acknowledged residual rather than papered over with a recovery path that does not exist. Making it recoverable requires either a reap-time sweep or a working `recover_foreign` custody seam; neither is specified here.
  - Redemptions below `min_balance` are routed to the caller's existing balance or rejected with `BelowMinimum`.
- **R-5.** Swept residue (rounding dust + unredeemed after `RedemptionArchiveDelay = 1 yr`) goes to INSURANCE, event-logged per vault (`VaultReaped { residue }`).
- **R-6.** USDC sent directly to pallet accounts outside protocol flows is recoverable only by TREASURY-class proposal (`recover_foreign`), never by any admin.
- **R-7 (new).** Position storage deposits (§4) are refunded on entry deletion, are excluded from `escrowed` and NAV-escrow reconciliation, and are forfeited to INSURANCE only if the owning account no longer exists at reap time.

---

## 8. Errors

`VaultNotOpen`, `WrongVaultState`, `AlreadyResolved`, `AlreadyVoided`, `NotResolved`, `NotSettled`, `GateNotSettled`, `GateAlreadySettled`, `InsufficientPosition`, `BelowMinimum`, `TooManyPositions`, `DepositFailed`, `ProtocolDestination`, `ArithmeticOverflow` (all conservation math is checked; overflow aborts the extrinsic — and per §6.1, no *legal* flow can underflow a per-branch supply field).

**No wrong-branch failure path is reachable (SQ-170).** The superseded list carried `NotWinningPosition`; it is **unreachable by construction** and has been struck: the settled-redemption calls of §5.3 take no branch argument — the ledger derives the winning branch itself from the vault's `Resolved(w)`/`ScalarSettled{w,s}` record before it builds the position key — so a caller cannot name the losing branch and there is no state in which such an error could be raised. A redemption against a position the caller does not hold surfaces as `InsufficientPosition`, and one against a vault in the wrong state as `WrongVaultState`. Implementations MUST NOT expose an unreachable wrong-branch error variant: an error an implementation declares but no path can produce is dead metadata the frontend must still decode, and is a defect rather than a specification to honor.

---

## 9. Try-state invariants (machine-checked every block in test/try-runtime; drift ⇒ I-4 flag ⇒ PB-LEDGER-FREEZE eligibility per D-9)

| ID | Invariant |
|---|---|
| L-1 | Per-branch conservation: ∀ vault in `Open`/`Resolved`/`Voided`-pre-redemption bookkeeping: `escrowed == usdc_b + Q_b + G_{b,S} + G_{b,C}` for **both** branches; Baseline: `E_base == sets` while `Open`. Supply fields ≡ `PositionTotals` ≡ Σ `Positions` per instrument. |
| L-2 | `TotalEscrowed == Σ_pid escrowed + Σ_e E_base` and `TotalEscrowed + held_deposits ≤ balance(sovereign)` (equality is not required: the R-4 genesis endowment and swept/direct-transfer dust are lawful surplus). The permissionless reconciliation latch equals the comparison recorded by its last exact sample; the live I-4 drift condition is `TotalEscrowed + held_deposits > balance(sovereign)`. |
| L-3 | Terminal valuation bound (integer forms): `ScalarSettled`: `E·10^9 ≥ supply(usdc_w)·10^9 + supply(L_w)·s + supply(S_w)·(10^9−s) + Σ_g supply(winning gate leg g)·10^9`. `Voided`: `4·E ≥ Σ_b [ 2·usdc_b + supply(L_b) + supply(S_b) + Σ_g (supply(Yes_{b,g}) + supply(No_{b,g})) ]`. |
| L-4 | Paired-supply equality in `Open`: `supply(L_b) == supply(S_b) == Q_b`, `supply(Yes_{b,g}) == supply(No_{b,g}) == G_{b,g}`. |
| L-5 | State legality: no vault in a state outside §2.3's transition table; terminal states admit no mint ops (D-8); `resolve`/`void`/`settle_*` each at most once per target. |
| L-6 | `PositionCount(who) ==` number of live `Positions` entries for `who`; `≤ MaxPositionsPerAccount` unless `ProtocolAccounts`; held deposits `== PositionDeposit ×` Σ non-exempt entries. |

---

## 10. Hooks and weights

**Hooks: none.** The pallet does no automatic work (I-20 trivially satisfied); all cleanup and reconciliation are keeper-cranked (`sweep_dust*`, `reconcile`, and the `MarketAuthority` inventory discard inside `market.reap`). Weight functions are benchmarked per call; drivers listed in §5. `sweep_dust` weight is linear in drained entries, bounded by `ReapBatch = 100`; market reap's ledger work is bounded by 28 proposal or four Baseline cells. `reconcile` is O(1): two maintained counters, one collateral-balance read, the prior sample/latch, and at most the edge writes/event/rebate. No vault scan or cursor is permitted on its dispatch path.

---

## 11. Tests (property-test obligations over the FULL operation set)

Consolidated registry: [`15-invariants-and-testing.md`](./15-invariants-and-testing.md). Op alphabet for all sequence-based tests: `{split, merge, split_scalar, merge_scalar, split_gate, merge_gate, split_baseline, merge_baseline, transfer, resolve, void, settle_scalar, settle_gate, settle_baseline, redeem, redeem_void, redeem_scalar, redeem_scalar_pair, redeem_gate, redeem_baseline, redeem_baseline_pair, sweep_dust}` — random interleavings, random legal and illegal states, ≥ 10^6 cases.

- **PT-1 (conservation):** random op sequences maintain L-1…L-4 and the §6 state equations at every step; illegal ops error without state change.
- **PT-2 (annulment, restated):** for random strategies, after `void`: (a) any account holding **cross-branch** complete pairs recovers exactly par via `merge` (same-branch scalar/gate completeness alone does not — §6.4); (b) any **real** wrapper-buyer portfolio (target leg + mirror branch-USDC held) recovers its exact D-1 neutral value after pair-first netting, with net principal delta `neutral recovery − cost − fees`; (c) unpaired single-branch holdings recover `floor(a/2)` / legs `floor(a/4)`. Net principal delta is `−fees only` on path (a); on path (b) it reaches `−fees` only when the realized average execution price is 0.5 — see §6.4 (SQ-171). These are obligations the suite MUST satisfy: path (b) MUST be driven through real `market-core` buys, not reconstructed bookkeeping.
- **PT-3 (rounding):** under a **pair-first** redemption schedule, Σ payouts over all holders after full redemption ∈ `[E − r, E]` in every terminal state, including `Voided` and gate/Baseline settlements, where `r` counts **non-zero residual redemptions applying an independent floor, per residual `PositionId`** — not per distinct account, since one account holding several independently-rounded instrument classes loses dust once per class (SQ-168). Exact pair redemptions contribute 0 to `r`. The §6.3 counterexample (`s = 0.70005, E = 20,000`) and the §6.4 counterexample (`split 100 → void`) are mandatory named regression vectors.
- **PT-4 (no-mint-outside-split):** model-based test that supply changes occur only in the six minting/burning op families; no op ever mints an unpaired leg; the ledger is the only mint path (negative tests via market wrapper, XCM, wrappers).
- **PT-5 (reap safety):** claimant reap never executes while any position balance > 0 unless `RedemptionArchiveDelay` elapsed; archived residue equals Σ outstanding claims valued at settlement; all claimant deposits are refunded or forfeited per R-7. Market reap at the same boundary discards only its two protocol owners' bounded inventory, decrements totals exactly, preserves claimant rows/vault collateral and is safe under both market-first and ledger-first interleavings. Signed transfer into any protocol destination rejects atomically before, during and after registration — including a predicted future book/fee address — while the MarketAuthority ingress remains admissible.
- **PT-6 (VOID reachability and conservation, X-6):** from `Open` and `Resolved` — the two states §2.3 admits `void` from — `void` succeeds; `void` from `ScalarSettled` and repeat `void` in `Voided` always error without state change (both are terminal, §2.3; the superseded "from every non-`ScalarSettled` state" quantifier wrongly included `Voided` — SQ-165). In `Voided`, every holder class has a terminating recovery path (merge or `redeem_void`), any interleaving of the I-27 call surface pays out ≤ `E`, and first-redeemer strategies gain nothing beyond claimant-adverse rounding residue. End-to-end: FE precondition rows exist for both recovery actions (owned by [`11-frontend-workflows.md`](./11-frontend-workflows.md)).
- **PT-7 (pair and gate exactness):** `redeem_scalar_pair`/`redeem_baseline_pair` pay exactly `a`; unpaired leg-by-leg redemption of the same holdings pays ≤ the pair payout. Per branch, gate-set mint/merge/settle preserves the §6.1 identity for every gate, and `settle_gate` outcomes pay 1:0 exactly.
- **PT-8 (key order / bounds):** per-vault reap drains exactly the vault's prefixes and nothing else; `PositionCount` accounting exact under transfer/churn; cap enforced for non-protocol accounts, never for protocol accounts.
- **I-4 reconciliation regression:** every escrow-changing operation and both terminal reaps keep `TotalEscrowed` equal to a full map re-sum; reconciliation sets the latch only for `liability > custody`, treats endowment/dust surplus as healthy, emits/rebates only on edges, and records a sample whose comparison equals the latch. A clean sample after a deficit clears the latch; while the same guardian authorization record remains live, the bounded guardian maintenance tests MUST prove that ledger/market effects and `PhaseFlags` bit 5 lift on that clear and re-engage on a later deficit.
- **Differential tests** vs the Python reference model (BE §24.4) extended to gate, Baseline, VOID and pair-redemption paths; **fuzz** on rounding at `MinSplit` boundaries and `s` near 0, 1, and `k/10^9 ± 1`; **TLA⁺ ledger model** (BE §24.5) re-run with the enlarged state machine proving I-3, L-5 and the §6.5 induction over all interleavings including guardian, oracle-dispute and VOID events.

---

## 12. Audit concerns

Rounding direction on every redemption path (LONG/SHORT floors, VOID halves/quarters — must never round toward the claimant); the `Resolved`-state outflow lockout (no code path may pay unpaired claims before `ScalarSettled`/`Voided`); `void`-after-`resolve` unfreeze correctness; gate-outcome ordering vs scalar settlement in the e+3 settlement transaction; escrow-account ED edge cases; deposit/escrow segregation; reap-vs-late-redeemer race; wrapper internal-API reachability (MarketAuthority only).

---

## Resolves

| Finding | Resolution in this document |
|---|---|
| **B-1** (VOID pays 2× escrow) | §6.4: `Voided` pays pairs par via merge, unpaired branch-USDC `floor(a/2)`, legs `floor(a/4)`; valuation argument shows total payout ≤ E on every path; the 100-USDC counterexample is a named regression vector; PT-2 restated honestly. |
| **B-2** (gate instruments unrepresentable) | §2.1/§2.2/§5: `PositionKind::GateYes/GateNo(gate)` per branch; per-branch `gate_sets` supplies; `split_gate`/`merge_gate`/`settle_gate(pid, gate, outcome)`/`redeem_gate`; conservation identity extended over the gate set (§6.1); ledger remains sole mint/burn authority (§1). |
| **B-3** (Baseline market has no ledger home) — ledger side | §2.1/§2.2/§5: `BaselineVaults: map EpochIndex → BaselineVaultInfo`, `PositionId::Baseline{epoch, Long/Short}`, USDC-direct collateral, `split/merge/settle/redeem_baseline(_pair)`, measured settlement via SettleAuthority at e+3 plus the two neutral paths of 05 §7(5)–(6); `pol.b_baseline` referenced (value: doc 13; market side: doc 04). |
| **B-4** (`branch_pairs` underflow on POL seeding) | §2.2/§6.1: single counter replaced by per-branch supply fields; stored invariant is the per-branch identity over the enlarged set; §6.2 walks the full seeding flow (split + scalar-split both branches + gate splits) showing no decrement can underflow. |
| **B-5** (unpaired SHORT over-pays) | §6.3: SHORT pays `floor(a·(1−s))`; atomic `redeem_scalar_pair` pays exactly `a`; the `s = 0.70005 / E = 20,000` counterexample now conserves (19,999 ≤ 20,000). |
| **X-6** (VOID unreachable end-to-end) | §2.3/§5.2: `VaultState::Voided` exists; `void(pid)` by ResolveAuthority from `Open` or `Resolved`, emitting `VaultVoided` (the frozen T20/`Voided` event, doc 02); explicit redemption semantics and events; PT-6 tests reachability; FE surface in doc 11. |
| **Positions map** (medium: unbounded, unpriceable, un-reapable) | §4: key order `(PositionId, AccountId)` — per-vault reap drains prefixes; per-account bound via `PositionCount` counter map; storage deposit 0.1 USDC/entry; protocol accounts exempt from `MaxPositionsPerAccount = 64`. |
| **ScalarSettled** (low: drops the winning branch) | §2.3: `ScalarSettled { winner, s }` carries the winning branch that `redeem`/`redeem_scalar`/`redeem_gate` need. |
