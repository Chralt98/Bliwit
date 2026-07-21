# 12 — Release Engineering and Operations

**Status: normative component specification. Supersedes the corresponding sections of BACKEND_PLAN.md/FRONTEND_PLAN.md** — specifically FRONTEND_PLAN §9.5–§9.8 (release-relevant parts), §22–§25, §27–§28 and BACKEND_PLAN §28 — implementing [D-6](00-decision-record.md) (layer-2 operator commitments), [D-14](00-decision-record.md) (descriptor gating, expedited release, stranded-app channel), and [D-16](00-decision-record.md) (funding lines, ArNS permabuy, signer disjointness).

**Boundary.** This document owns: the release train (build, deploy, attestation, soak, repoint, rollback), the release/ArNS key architecture and its organizational controls, the bundle-level distribution security controls (CSP, service worker, SRI at release scope), the release-process obligations around the `ReleaseChannel` stranded-app channel, and the complete operational layer (node programs, monitoring, incident response, funding, phase-gate wiring). It **references**: the frozen chain↔frontend contract ([02](02-integration-contract.md)); treasury budget-line amounts ([08](08-treasury-and-economics.md)); the upgrade path, `DescriptorLeadTime` enforcement, `ReleaseChannel` storage definition and phase gates ([09](09-execution-upgrades-and-rollout.md)); the frontend data/verification architecture ([10](10-frontend-architecture.md)); constants ([13](13-parameters.md)); and threat rows ([14](14-threat-model.md)). Test/release gates that this document's checklists invoke are specified in [15](15-invariants-and-testing.md).

RFC 2119 language throughout.

---

## 1. Release train

### 1.1 Deterministic build

Pinned Node version via container digest; `npm ci` from the committed lockfile; `SOURCE_DATE_EPOCH` fixed; Vite configured for content-hash-only filenames and stable chunking; a post-build normalizer strips nondeterminism (plain tree, no archive metadata). Output: `dist/` + `release.json` (version, source commit, build-recipe digest, per-file SHA-256 map, Arweave manifest TXID, chain-identity block per [02](02-integration-contract.md), supported `spec_version` range, descriptor metadata hashes — **including the Asset Hub descriptor set**, §1.6 — SBOM hash, signing key IDs, keyring generation) + `sbom.cdx.json` (CycloneDX from the lockfile). Two independent CI environments MUST produce an identical tree hash; third-party rebuild instructions are published and re-tested quarterly ([15](15-invariants-and-testing.md)).

### 1.2 Two-pass Arweave deploy

Pass 1: upload `dist/` + path manifest via permaweb-deploy/Turbo SDK → manifest TXID `M`. Pass 2: patch `release.json.arweaveManifestTxId = M`, upload `release.json` as a tagged sibling TX (`App-Release: <version>`, `App-Manifest: M`), re-upload the path manifest including it. Because the manifest references `release.json`, the final manifest TXID `M′` differs from `M`; `release.json` records the asset-tree manifest and the app resolves its own base TXID at runtime from `location`; the verification CLI checks both. **[VERIFY the exact two-pass flow against live gateway behavior — prototype gate FE-P7.]** Every release additionally creates an immutable per-release undername `vX-Y-Z_futarchy`.

### 1.3 Independent verification

Anyone MUST be able to reproduce the verdict with no project infrastructure:

```bash
git clone https://github.com/<org>/futarchy-app && cd futarchy-app && git checkout <commit>
docker run --rm -v $PWD:/src futarchy/build@sha256:<digest> ./tools/release/build.sh
./tools/verify-release compare --local dist/ --arweave <manifest-txid> \
    --release-json ar://<release-json-txid> --require-attestations 2
# fetches via ≥2 gateways with hash verification, byte-compares every file,
# verifies minisign signatures against the published keyring, checks the
# on-chain revocation set (§3.1) when a node is reachable, prints VERDICT
```

`tools/verify-release` is also published standalone. It MUST fail on any signature from a key marked revoked in the on-chain `ReleaseChannel` record (§2.3) and MUST include the signer-registry disjointness check (§2.2) in its `signers audit` subcommand.

### 1.4 Standard release checklist (normative gates)

A repoint of the production name `futarchy` to a new release MUST NOT occur until **all** of:

1. **Green release gates** per [15](15-invariants-and-testing.md) (reproducible-build gate, descriptor drift gate, wallet/browser matrices, degradation-matrix suite, attestation verification from a clean container).
2. **≥ 2 independent attestations**: builders in different organizations/infrastructure reproduce the tree hash and publish minisign-signed attestation TXs on Arweave.
3. **72 h staging soak** on `staging_futarchy` against the live target network.
4. **Changelog TX** published; release notes list the immutable TXID, attestation TXIDs, and the multi-gateway URL set.
5. **3-of-5 ArNS repoint** (§4.2) executed by the controller quorum via one ANT `setRecord`.
6. **`ReleaseChannel` update** submitted per §3 within 600 blocks of the repoint.

CI can neither sign nor repoint: CI holds **no** minisign keys and **no** ANT controller shares (verification-protocol check 8 carried forward — CI compromise can block releases but cannot ship one alone).

**Release-signature floor (normative; added 2026-07-20).** Gate 2 fixes a floor for *attestations*; the checklist previously fixed none for the release-key signatures over `release.json` itself, leaving §1.1's plural "signing key IDs" unquantified. The floor is **≥ 2 valid release-key signatures from distinct active keys of the current keyring generation** (§2.1), counted after excluding every key marked revoked in `ReleaseChannel` (§2.3) — the same floor as attestations, for the same reason: a canonical release must survive the loss or compromise of any single release key without becoming unverifiable, and a one-signature release would make a single key a unilateral shipping authority, which is precisely the concentration §2.2 exists to prevent. `verify-release` (§1.3) and the out-of-band monitor (§5.2) MUST treat fewer than two valid signatures as a verification failure. A deployment MAY require more and MUST state its minimum explicitly rather than inherit one silently; it MUST NOT configure fewer.

### 1.5 Expedited descriptor-only release (D-14)

A second, faster lane exists. It is **admissible only** for releases whose delta against the incumbent production release is confined to: `packages/descriptors/**` (including the Asset Hub descriptor set), descriptor metadata hashes and the supported `spec_version` range in `release.json`, and release metadata files (changelog, release history data). **Zero app-code delta**: every other file in the built tree MUST be byte-identical to the incumbent release.

Requirements: the same reproducible build and **2 attestations** — where each attestor additionally attests to the **delta scope** (mechanically checked by `verify-release diff-scope --against <incumbent-txid>`, which byte-compares the trees and fails if any out-of-scope file differs) — then a **3-of-5 repoint**. **No staging soak is required.** The `ReleaseChannel` update sets the `EXPEDITED` flag; every expedited release MUST be followed by a retrospective entry in the release log stating why the lane was used.

Rationale (honest scope): the soak exists to catch app-behavior regressions; a descriptor-only delta has no app-code surface to regress, while descriptor lateness is itself a live risk (§1.6). Any change touching app code — however small — MUST use the standard lane.

### 1.6 Descriptor lead-time gating (D-14, D-12)

This is a **release-gating rule, not a convention**:

- v(N+1) descriptors MUST be generated from the **queue-time artifact commitment** (the wasm/metadata hashes committed when the CODE/META proposal is queued — [09](09-execution-upgrades-and-rollout.md)), not from the post-enactment chain. The timelock is the build window.
- A release covering `spec_version` N+1 MUST be live on the production name **before application maturity** — `applicable_at = authorized_at + DescriptorLeadTime` (43,200 blocks = 72 h *(normative value: [13](13-parameters.md))*), where `authorized_at` is recorded when `execute()` dispatches `authorize_upgrade` and the SafetyFilter denies `apply_authorized_upgrade` until `applicable_at` ([09](09-execution-upgrades-and-rollout.md) §2.2). Because the backend enforces the lead time on-chain and the descriptors are buildable from queue time, the frontend has ≥ the full timelock plus 72 h; the standard lane (with soak) is the expected path, and the expedited lane is the repair path when the margin has been consumed (late descriptor bug, re-generation after an artifact-commitment correction).
- CI enforcement: the descriptor pipeline regenerates descriptors from each supported runtime artifact and diffs against committed ones on every PR and on every runtime release ([15](15-invariants-and-testing.md)). Monitoring (§6.3) alerts when an `UpgradeAuthorized` event exists and no covering release is live at 50% of the lead time.
- **Asset Hub set (D-12)**: the funding flow's second light-client connection pins an Asset Hub descriptor set. Asset Hub upgrades ride the Fellowship's schedule, outside this protocol's governance; the pipeline therefore monitors Asset Hub `spec_version` on both networks and ships Asset Hub descriptor refreshes through the **expedited lane** (they satisfy its admissibility by construction). A stale Asset Hub set degrades only the funding flow (labeled, per [11](11-frontend-workflows.md)), never core trading.

### 1.7 Rollback

Every release is a permanent Arweave transaction; per-release undernames plus in-app release history keep every prior version reachable by immutable TXID. Rollback = `./tools/release/repoint.sh futarchy <previous-manifest-txid>` collecting 3-of-5 signatures; nothing is deleted, so rollback is **O(minutes) and reversible**. Carried forward and still correct: for a vulnerability *introduced by the current release*, rollback ends exposure in minutes. For a vulnerability present in all prior releases, or when rollback is compat-blocked (the previous release does not cover the current `spec_version`), the fix rides the standard or expedited lane as its delta scope dictates; the `ReleaseChannel` `SECURITY` flag (§3) warns users on the affected releases in the interim.

---

## 2. Keys and signing

### 2.1 Key architecture (carried forward)

Two disjoint key populations:

- **Release minisign keys**: sign `release.json`'s hash. Keyring published in-repo, in-app, and on Arweave. Attestor keys are minisign keys of the independent builders.
- **ArNS controller keys**: control the ANT that resolves `futarchy` (§4.2).

Both: hardware-backed, geographically distributed, documented ceremony, annual rotation with overlap; old keyrings are retained in-app for verifying historical releases, tagged by **keyring generation** (a monotonically increasing `u32` carried in `release.json` and in `ReleaseChannel`). CI holds neither population (§1.4).

### 2.2 Signer disjointness (D-16) — hard organizational control

**Requirement:** the set of ArNS controller key holders and the set of minisign release-key holders MUST be disjoint: `ArNS controllers ∩ release signers = ∅`, evaluated over *natural persons*, not key IDs. Additionally, out-of-band attestation-monitor operators (§5.2) MUST NOT be ArNS controllers. Rationale: without disjointness, ~3 insiders holding 2 ArNS shares + 2 release keys ship a fully self-verifying malicious release; with it, a malicious release requires colluding across two organizationally separated groups.

**Verification procedure (normative):**

1. A **signer registry** (`SIGNERS.md` in-repo + mirrored on Arweave) lists every active minisign key ID, every ANT controller address, and every attestor key, each mapped to a stable operator identifier (a named person or a named organization role held by a named person).
2. CI mechanically checks that no operator identifier appears in both populations; a violation is release-blocking.
3. At every key ceremony (issuance, rotation, revocation) the disjointness predicate over the updated registry MUST be re-attested by ≥ 2 attestors in a signed attestation TX, and reviewed quarterly thereafter.
4. `verify-release signers audit` recomputes checks 2–3 from public data.

**Residual (declared):** the mechanical check operates on *declared* identities; a false declaration (one person under two identities) defeats it. This is an organizational-honesty limit, recorded as a threat row in [14](14-threat-model.md) alongside the founding-multisig insider row; the mitigations are the ceremony's multi-party witness requirement and the attestor sign-off.

### 2.3 Compromised-release-key revocation

Old bundles ship their keyring immutably, so revocation MUST NOT depend on shipping new code to the affected users. The path:

1. The compromised key ID is added to the **on-chain revocation set** in `ReleaseChannel` (§3.1: `keyring_generation` is bumped and the key's index is set in `revoked_key_bits`), and the `SECURITY` flag is set if any live release was signed by the revoked key.
2. All apps — including pinned and stranded ones, which read `ReleaseChannel` without current metadata — MUST treat a revoked key as invalid for every verification (self-check, update verification, attestation counting) from the moment the revocation is observed at a finalized head.
3. A new keyring generation is published via the next release and the signer registry (§2.2); `verify-release` fetches the revocation set when a node is reachable and warns loudly when it cannot.
4. If the revoked key co-signed the *current production* release, the release is re-signed by remaining valid keys (delta scope: release metadata only → expedited lane) or rolled back (§1.7).

Write authority, retro-ratification, and abuse bounds for `ReleaseChannel` are defined in [09](09-execution-upgrades-and-rollout.md) (layout: [02](02-integration-contract.md) §12); the griefing surface (a malicious writer can at most force warnings/signing-friction in old releases, recoverable by opening a newer release) is a [14](14-threat-model.md) row.

---

## 3. Stranded-app release channel (D-14)

### 3.1 The `ReleaseChannel` raw storage key

The §24.5 `system.remark` release pointer of FRONTEND_PLAN is **deleted**: a stranded app in `ReadOnlyIncompatible` lacks current-runtime metadata exactly when it needs the pointer, and remark decoding requires it. Its replacement is a **fixed-layout raw storage value** `ReleaseChannel` in `pallet-constitution` — byte layout owned and frozen by [02](02-integration-contract.md) §12; write authority and upgrade-path wiring in [09](09-execution-upgrades-and-rollout.md); quoted here for readability *(normative layout: [02](02-integration-contract.md) §12)*:

fixed-width, **frozen forever** (any future need is met by appending fields beyond offset 168 with a schema bump, never by changing existing offsets), read at the well-known raw key `twox128("Constitution") ++ twox128("ReleaseChannel")` — 168 bytes:

`schema u8` · `version [u8;32]` (UTF-8 semver, zero-padded) · `manifest_txid [u8;43]` (base64url TXID) · `release_json_hash [u8;32]` · `updated_at u32` · `spec_version u32` · `pending_authorized_at u32` · `min_supported_version [u8;32]` (UTF-8 semver) · `keyring_generation u32` · `revoked_key_bits u64` (bitmask over key indices within the generation's published keyring) · `flags u32` (bit 0 `SECURITY`, bit 1 `EXPEDITED`, bit 2 `URGENT_UPGRADE`, others reserved zero).

Because the key and layout are metadata-independent, **any** shipped release — past, pinned, stranded — can read and decode it via a plain light-client storage read forever.

**Reader discipline for reserved bits and future schemas (normative; added 2026-07-20).** [02](02-integration-contract.md) §12 fixes `flags` bits 0–2, requires bits 3–31 to be zero, and confines every extension to fields **appended beyond offset 168** under a schema bump. Read together those rules settle a question the layout alone leaves open: the reserved bits are frozen zero for the life of the layout — a future flag is a new appended field, never a redefinition of bits 3–31 — so no schema bump can retroactively give them meaning. Therefore:

- A reader that observes a non-zero reserved bit is looking at a record no conforming writer could have produced (corruption, or a writer outside the two authorized origins of [02](02-integration-contract.md) §12). It MUST **reject the record rather than mask** the unknown bits, because masking would silently accept an unauthenticated writer's record as canonical.
- Rejection MUST surface as an explicit *degraded/unknown* state and MUST NOT be rendered as a healthy one. For §3.2's in-app obligations that means neither "no newer release exists" nor "no security flag is set"; for the §6.3 monitoring stack it means the affected series go **absent** rather than to a healthy-looking zero — a rejected record must never be indistinguishable from a clean one — with the absence itself caught by the operator's exporter-health monitoring, which is a §6.1 obligation of the monitoring service rather than a §6.3 row.
- Readers MUST accept a value **longer** than 168 bytes and parse the frozen prefix, discarding the remainder: that is exactly [02](02-integration-contract.md) §12's append path, and a reader that requires an exact length would break on the first schema bump it was designed to survive. A value **shorter** than 168 bytes is malformed and is rejected as above.

### 3.2 Old-app obligations (in-app upgrade warning UX — required)

Every release MUST poll `ReleaseChannel` at each finalized head (one raw read) and:

- `version >` own and `manifest_txid ≠` own → non-blocking "newer canonical release exists" banner with the immutable-TXID link (carries forward §18.6/E14 UX); pinned users MAY dismiss persistently.
- `min_supported_version >` own version → **prominent blocking warning**; signing is gated behind an explicit per-session acknowledgment; the warning names the newer release's undername and TXID.
- `SECURITY` flag set while own release's signing keys intersect `revoked_key_bits`, or own version `< min_supported_version` with `SECURITY` set → red security banner, **signing disabled** (reads continue), direct link to the newer release. This is the review's missing "warn pinned users" path: pinning protects against hostile *repoints*, and the chain channel — verified state, not a mutable name — is what reaches pinned users anyway.

The channel is advisory *display and gating* data with a bounded write authority ([09](09-execution-upgrades-and-rollout.md)); it is never a code-delivery mechanism and never triggers automatic updates (the release-scoped service worker's no-auto-`skipWaiting` rule, §5.2, is unchanged).

---

## 4. ArNS and distribution control (F-5, X-13, D-16)

### 4.1 Tenure: permabuy

The canonical name `futarchy` MUST be held under ArNS **permabuy** (permanent registration), funded from the `ops.arweave` line (§6.1). This **closes the lease-lapse takeover**: under the previously undecided lease tenure, an expired lease let *anyone* re-register the canonical name — a full T-2-class distribution compromise requiring no key compromise at all (DESIGN_REVIEW X-13/F-mediums). With permabuy there is no renewal event to miss and no expiry failure mode; the threat row in [14](14-threat-model.md) records the closure. Staging/dev/per-release undernames ride the same permanently held name. (The deferred Bulletin mirror, if ever activated, has retention/renewal semantics and keeps its own expiry-alert runbook — FRONTEND_PLAN §31 D-Bulletin, unchanged.)

### 4.2 ANT control quorum

- **Primary:** 3-of-5 native ANT n-of-m control **if** n-of-m controller capability is confirmed by the prototype gate **[VERIFY ANT n-of-m controller capability in @ar.io/sdk 4.x — prototype gate FE-P7 ([10](10-frontend-architecture.md) §12; D-16, which notes an earlier draft mislabeled it FE-P8)]**.
- **Fallback:** if native n-of-m does not materialize, a **FROST-ed25519 threshold ceremony** producing a single on-chain controller key whose signatures require a 3-of-5 threshold of share-holders **[VERIFY FROST output compatibility with the ANT controller signature scheme — same prototype gate]**.
- **Prohibited:** single-key custody of the production ANT, under any circumstance, including "temporarily" during bootstrap. **If neither the native quorum nor the FROST ceremony materializes, launch blocks on this line** — the production name does not go live (phase-gate wiring §6.5).

Controller shares: hardware-backed, geographically distributed, holders disjoint from release signers (§2.2). Per-release immutable undernames and the boot-time ArNS resolution cross-check are carried forward ([10](10-frontend-architecture.md)); the residual — a hostile-but-*consistent* repoint by a controller quorum is policy-bound, not protocol-bound — remains disclosed in [14](14-threat-model.md), now mitigated by the out-of-band attestation monitor (§5.2) whose operators are controller-disjoint.

---

## 5. Bundle-level security controls

### 5.1 CSP: `connect-src` allowlist (F-medium)

The FRONTEND_PLAN §9.8 policy is amended in exactly one directive: **`connect-src *` is replaced by an enumerated allowlist**, rebuilt per release, containing precisely:

1. the committed WSS bootnode endpoints (the §6.2 program set, plus relay bootnodes from the bundled specs);
2. the baked static ar.io gateway fallback list (independent operators; Wayfinder's runtime gateway selection is restricted to the intersection of its network-sourced set with this allowlist);
3. the hosts of release-registry providers that ship *in this release's* vetted provider registry (empty at launch — opt-in posture unchanged), plus opted-in RPC fallback endpoints that are release-listed.

All other directives are unchanged (`default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; …`), still delivered as meta-CSP (gateways own real headers; documented limitation: meta-CSP cannot set `frame-ancestors`).

**What it does (honestly):** bounds network egress to an enumerated operator set — an XSS or compromised dependency can no longer exfiltrate to an arbitrary attacker host; exfiltration now requires an allowlisted operator's cooperation or a public channel. **What it does not do:** it is not the injection defense (that is `script-src`, unchanged); it does not stop exfiltration *to* allowlisted gateways via query strings, to chain peers via extrinsic content, or via timing channels; and a malicious gateway serving tampered HTML can strip the meta tag entirely — but that is already the T-1 altered-bundle case, caught by the T-1 controls, not by CSP. **Cost (accepted):** user-supplied custom bootnodes/RPC/provider endpoints outside the allowlist cannot be reached by the shipped bundle, because meta-CSP is fixed at build time. Expert escape hatches: request allowlisting in a release, or run a self-served build (documented in the expert panel). This trades a rarely used expert affordance for exfiltration containment for every user; user-supplied *allowlisted* endpoints still work.

### 5.2 Service worker: rules, declared residual, out-of-band attestation monitor (F-medium)

Carried forward unchanged: release-scoped SW; cache name = manifest TXID; assets fetched only by exact hashed filename from its own release; no automatic `skipWaiting` — activation only on explicit user action; old cache deleted on activation; hash-mismatch responses refused; "pin this release" toggle; SW file itself hash-listed in `release.json`.

**Removed claim:** FRONTEND_PLAN T-13's detection story — the §18.6-style in-app self-check and app-vs-ArNS version comparison — is **withdrawn as a hostile-SW control**. A malicious service worker interposes on *all* same-origin fetches, including the self-check's fetch of `release.json`, the attestation fetches, and the ArNS resolution probes: it answers them with consistent lies. In-app self-checks remain useful against *accidental* corruption and against threats that do not control the SW (T-1/T-3), but against a hostile SW they verify nothing.

**Declared residual:** one accepted malicious release achieves durable per-origin persistence — the hostile SW can keep serving itself and suppress in-app update/warning UI on that origin until the user unregisters it (browser UI), clears site data, or loads via a different origin/immutable-TXID path. This is recorded as a residual-risk row in [14](14-threat-model.md).

**Compensating control — out-of-band attestation monitor (required):** ≥ 2 independent monitor operators, **disjoint from ArNS controllers** (§2.2), run a headless fetcher (no service worker, no browser cache) that, on every finalized `ReleaseChannel` change and at least hourly:

1. resolves `futarchy` via ≥ 3 independent gateways and fetches the full served bundle by resolved TXID and by name;
2. byte-compares every file against the per-file SHA-256 map of the *signed* `release.json`, verifies the minisign signatures and ≥ 2 attestations against the current keyring generation, and cross-checks `manifest_txid` against `ReleaseChannel`;
3. on any mismatch, fires the **release-integrity alerting channel**: the monitoring stack's paging route (§6.3, `RB-RELEASE`), the public static status page, community channels, and — on confirmed compromise — the incident path of §6.4 (repoint/rollback + `ReleaseChannel` `SECURITY` flag, which reaches even users whose SW suppresses in-app banners only if they load a clean context; the status page and wallet-community channels are the honest remainder).

The monitor detects hostile *distribution* (wrong bytes served for the canonical name) from outside any SW's reach; it cannot reach into a browser profile already running a hostile SW — that residual stands as declared.

### 5.3 SRI

Unchanged from FRONTEND_PLAN §9.8: build-generated SHA-384 `integrity` attributes on every `<script>`/`<link>`; protects sub-asset integrity when honest HTML is served with tampered assets; complements, never replaces, content addressing.

---

## 6. Operational layer (X-13, X-4, D-6, D-16)

There is no application backend; "operations" is the set of **owned and funded** commitments below. Every row names an owner role (an accountable person holds each role; assignments are published in the ops handbook) and a treasury budget line whose amount is normative in [08](08-treasury-and-economics.md).

### 6.1 Owned-and-funded ops table (normative)

| Service | Commitment (MUST) | Owner role | Funding line ([08](08-treasury-and-economics.md)) |
|---|---|---|---|
| **WSS bootnodes** | ≥ 8 browser-reachable WSS bootnodes across ≥ 4 operators, ≥ 2 on port 443 *(normative values: [13](13-parameters.md))*; listed in the chain spec per the [02](02-integration-contract.md) node-roles row; liveness-monitored; set updates ride releases + on-chain discovery | Bootnode program coordinator | `ops.bootnodes` |
| **Served-state window** | The protocol-funded bootnode/RPC operators jointly serve **30 days** of state and bodies *(normative value: [13](13-parameters.md))* — the [D-6](00-decision-record.md) layer-2 commitment backing FE backfill (`provider`-labeled unless smoldot re-read, per [10](10-frontend-architecture.md)) | Infrastructure coordinator | `ops.rpc_archive` |
| **Collators** | 5 invulnerables → 8–12 bonded permissionless (Phase 4+); geographic/organizational diversity feeds the Security pillar; compensation 2,000 USDC/collator/epoch *(normative value: [08](08-treasury-and-economics.md))* | Collator program coordinator | `ops.collators` |
| **RPC / archive nodes** | ≥ 4 load-balanced public RPC (rate-limited, no signing) + ≥ 2 archive nodes (oracle recomputation, dispute evidence, and the 30-day window's tail) | Infrastructure coordinator | `ops.rpc_archive` |
| **Monitoring & alerting** | The full §6.3 stack, including the release-integrity monitor (§5.2) and browser-reachability probes | Monitoring coordinator | `ops.monitoring` |
| **Keeper operations** | ≥ 2 independent operator-run keepers + the permissionless public; subsidies beyond the metered budget of 12,000 USDC/epoch *(normative value: [08](08-treasury-and-economics.md))*, with the exhaustion alarm wired to §6.3 | Keeper coordinator | `ops.keepers` |
| **Oracle evidence hosting** | Evidence artifacts and archived MetricSpec documents persisted to Arweave and mirrored on the archive nodes; retrievable for the full dispute-latency horizon ([07](07-oracle-and-disputes.md)) | Oracle operations coordinator | `ops.oracle_evidence` |
| **Watchtowers** | ≥ 2 bonded registered watchtowers for the challenge-window acknowledgment quorum ([07](07-oracle-and-disputes.md)) | Oracle operations coordinator | `ops.watchtowers` |
| **Arweave / ArNS** | Permabuy of `futarchy` (§4.1); per-release upload credits (Turbo); undername operations; status page hosting | Release operations lead | `ops.arweave` |
| **Release operations** | §1 ceremonies, key ceremonies (§2), signer registry upkeep, incident playbooks (§6.4) | Release operations lead | `ops.arweave` / `ops.monitoring` |

Bootnode operator diversity counts *organizations*, not machines; an operator MAY serve multiple roles (bootnode + RPC + archive) but the ≥ 4-operator floor applies to distinct organizations. Browser-reachability (valid TLS, WSS, port policy) is part of the commitment, not best-effort — corporate/mobile networks blocking non-443 WSS is the documented reason for the :443 floor.

**Keeper parameter sourcing (normative; added 2026-07-20).** Neither this table nor [01](01-system-overview.md) §4.2 previously stated where an off-chain keeper obtains the tunables that decide *when* its cranks are due, leaving clients free to compile in [13](13-parameters.md)'s genesis figures and drift silently after the first amendment. The obligation: for every [13](13-parameters.md) row governing a crank's due-time or admissibility, a keeper MUST resolve the value from live `Constitution.Params` at the head it plans against, under the precedence **operator override > live chain value > documented genesis default**. A compiled-in default is a last-resort fallback, never the primary source, and MUST equal the [13](13-parameters.md) genesis value it mirrors. A row for which no default is safe — one whose stale value would make the keeper submit an invalid or economically wrong extrinsic rather than merely a mistimed one — is **live-or-nothing**: the keeper skips the crank instead of guessing. This binds clients, not consensus: every crank is idempotent and the on-chain guards remain authoritative, so a stale keeper costs liveness and never safety. Because a keeper cannot generally distinguish "the row is absent" from "the read failed", both fall back identically, and operators detect the resulting staleness through the keeper's own §6.3 liveness series rather than through the chain.

### 6.2 Bootnode program

The program (FE §28(1), now backed by the [02](02-integration-contract.md) chain-spec requirement resolving X-4's ops side): operators run relay-connected futarchy nodes exposing WSS multiaddrs with valid certificates; the committed set ships in the bundled chain spec (hash-pinned) and updates ride releases plus on-chain discovery; operators self-monitor (their Prometheus) **and** the §6.3 stack probes dialability *from a browser context* (headless browser dial test per endpoint) — a bootnode that only native peers can dial fails its commitment. Served-state window: the 30-day retention is a commitment of this program's node fleet, phase-gated below; it is an *honest ops line*, not a protocol guarantee — the FE labels data accordingly ([10](10-frontend-architecture.md)).

### 6.3 Monitoring and alerting

Carried forward from BACKEND_PLAN §28 (Prometheus + on-chain-event alerting, runbooks-as-code in `deploy/runbooks/`), unchanged rows first:

| Domain | Key series | Alert (example) | Runbook |
|---|---|---|---|
| Epoch progress | phase, blocks-to-boundary, tick lag | tick lag > 600 blocks | RB-KEEPER |
| Proposal state | per-state counts, queue depth | intake queue at bound | RB-INTAKE |
| Markets | price, depth, spread proxy, book P&L | book loss > 0.9·b·ln2 | RB-MARKET |
| TWAP | coverage %, stale events, spot-vs-TWAP dispersion | coverage < 96% mid-window | RB-KEEPER |
| Liquidity floors | effective POL vs floor | POL disturbed | RB-POL |
| Oracle | report timeliness, open disputes, round depth | round 3 opened | RB-ORACLE |
| Collateralization | Σ escrow vs sovereign balance drift | any drift ≠ 0 | RB-LEDGER (page immediately) |
| Treasury | NAV, meter utilization, stream schedule | meter > 80% | RB-TREASURY |
| XCM | send/fail/timeout counters, trapped assets | any trap | RB-XCM |
| Keepers | rebate claims per role, inactivity | no crank 1 h | RB-KEEPER |
| Guardian | actions, allowance consumption, pending reviews | any action | RB-GUARDIAN |
| Upgrades | authorized hash, applied version, migration cursor | cursor stalled | RB-UPGRADE (PB-MIGRATION, page immediately) |
| Storage | per-map counts vs bounds, PoV sizes | > 80% of any bound | RB-STORAGE |
| Numerics | LMSR domain-rejection count, rounding-dust accumulation | anomaly spike | RB-MARKET |

New rows owned by this document:

| Domain | Key series | Alert | Runbook |
|---|---|---|---|
| Bootnodes | per-endpoint browser-dial success, WSS cert expiry, operator count | < 8 dialable, or < 2 on :443, or cert < 14 d | RB-BOOTNODE |
| Served-state window | per-operator retention depth | window < 30 d on the joint fleet | RB-BOOTNODE |
| Release integrity | out-of-band bundle-vs-manifest comparison (§5.2), ArNS resolution consistency across gateways, ANT record history | any byte mismatch; 2-of-3 resolver divergence | RB-RELEASE (page immediately) |
| Descriptor lead time | `UpgradeAuthorized` age vs covering-release liveness | no covering release at 50% of `DescriptorLeadTime` | RB-RELEASE |
| ReleaseChannel | staleness vs latest repoint, flag changes | update missing 600 blocks after repoint; any `SECURITY` flip | RB-RELEASE |
| Keeper budget | metered-budget utilization | > 80% of `keeper.budget` | RB-KEEPER |

**Row semantics (normative; added 2026-07-20).** The rows above are terse by design. The readings below were otherwise left to the implementer, and each is now fixed:

- **Values in trigger cells.** A trigger cell that *restates* a governance-amendable figure goes stale the moment governance amends it, and every runbook and rule byte-bound to that cell goes stale with it. Such a cell therefore names the owning [13](13-parameters.md) key, and the alert is evaluated **against the live value**: the Keeper-budget row fires at 80 % of the live `keeper.budget` — 12,000 USDC/epoch at genesis, *normative value: [13](13-parameters.md)* — never of a figure frozen here. Two cases are deliberately outside that rule. A literal is admissible where **this document owns the value**: the ReleaseChannel row's 600 blocks is §1.4(6)'s repoint deadline, not a [13](13-parameters.md) row, and does not move under governance. And an **early-warning margin** — a threshold set deliberately inside a protocol requirement so the alert fires *before* the requirement is breached, as the TWAP row's 96 % sits above `dec.coverage` — is a monitoring choice owned here rather than a restatement; it MUST be re-derived whenever the requirement it guards is amended.
- **Proposal state.** "Intake queue at bound" is the **intake** queue (bound 64, *normative value: [13](13-parameters.md)*) — the queue RB-INTAKE addresses and the one [05](05-welfare-and-decision-engine.md) §T1 and [14](14-threat-model.md) TH-16 mean by queue pressure. The execution-guard queue (bound 32) is a different lifecycle stage; its occupancy is covered by the Storage row's 80 %-of-bound rule, not here.
- **Keeper inactivity.** "No crank 1 h" is a **daemon-side** signal, evaluated per keeper role against that role's last-successful-crank timestamp. It is deliberately not inferred from chain activity, which cannot distinguish an idle keeper from a role that legitimately had no due work. A role that has never succeeded since process start is indistinguishable from a disabled role and is excluded rather than alerted on, and the signal is scoped per role so activity in a cleanup role cannot mask silence in a decision-critical one. Excluding those cases is what makes the signal usable, and it creates exactly two declared blind spots: **a role stuck since start-up**, which carries the same zero timestamp as a disabled one, and **a keeper whose chain connection is down**, which the liveness conjunct silences even though its cranks are equally not happening. Both are real gaps in this row and MUST be covered by the keeper's own liveness series and by finalized-head lag monitored independently of the keeper process — never assumed away because this row is quiet.
- **Ownership.** This table assigns runbooks, not owners: an alert's owner is the owner of its §6.1 row. Protocol-domain alerts have no dedicated program row and fall to the **Monitoring coordinator** as first responder, under §6.1's Monitoring row, with domain escalation named in the runbook. A runbook spanning §6.3 rows that map to *different* §6.1 rows takes its primary row's owner and MUST name the other row's owner in its escalation path — the Bootnodes / Served-state-window pair is the live instance of this.
- **Two observables this table left implicit.** "Update missing 600 blocks after repoint" has no on-chain marker for the repoint block, so the lag is anchored at the first finalized head at which the gateway-majority ArNS target and `ReleaseChannel.manifest_txid` **disagree**. That clock also starts when the gateways reach no majority at all — deliberately, since a name that cannot be resolved consistently is not a healthy state. "Window < 30 d on the joint fleet" aggregates per-operator retention as the **maximum** reported depth: every operator's window ends at now, so the deepest window is the joint window. Per-operator shortfalls above that floor belong to the bootnode program (§6.2), not to this row.
- **"Cursor stalled" (Upgrades row)** *(added 2026-07-20, SQ-265)* is the **union** of three conditions, any one of which raises the alert: the latched no-progress stall halt, the live no-progress detector over an active migration cursor, and a cursor in the `Stuck` state. A permanently failed migration is strictly more alarming than a slow one and RB-UPGRADE (PB-MIGRATION) owns both dispositions, so they share one signal rather than competing for an operator's attention. The union is deliberately **not** "any migration halt is latched" — halt sources other than the stall latch carry their own dispositions and are not folded in here. The no-progress leg is a conservative trigger by construction: a lawful migration step may mutate storage while returning identical cursor bytes, so the signal admits false positives and the runbook, not the detector, adjudicates. A tighter *normative* definition of "stalled" for PB-MIGRATION would have to come from [05 §4.3](05-welfare-and-decision-engine.md)'s cursor semantics and is not supplied by this row.
- **"Effective POL vs floor" (Liquidity-floors row)** *(added 2026-07-20, SQ-266)* is compared **per component and never summed**. The series carry a component label separating the proposal-subsidy line — compared against the sum of live proposal-book commitments — from the Baseline line, compared against its own live commitments plus the standing seed headroom for the next Baseline book. A summed comparison would let overfunding on one line mask underfunding on the other, which is exactly the competition [08 §4.3](08-treasury-and-economics.md) forbids; the alert rule MUST therefore match the two series on the component label rather than aggregating over it. Per-**book** granularity is *not* required of this row: the per-book "POL undisturbed" condition of [05 §5.2](05-welfare-and-decision-engine.md) is a decision-grading predicate enforced on chain, not a monitoring threshold, and this row's purpose is to detect competition *between* the two funding lines.

**Runtime telemetry source (added 2026-07-18, B13).** The audited sources for the runtime-side series above that no frozen [02](02-integration-contract.md) view exposes (book P&L and its b·ln2 bound, live mid-window TWAP coverage, effective POL vs floor, collateral drift, migration-cursor stall, numerics anomaly counters, and the storage-bound remainder invisible to portable metadata) are the methods of the runtime's **`TelemetryApi`** — a monitoring-only `decl_runtime_apis!` trait that is **explicitly outside the 02 integration contract**: the frontend never consumes it, it carries no contract version, and its shape may change additively or otherwise without a 02 §13 bump or joint sign-off (this document owns it). The §6.3 exporter consumes it via `state_call` under the same per-family fail-closed degradation as every other family (failures degrade to absent series, never healthy zeros) — with one boundary drawn deliberately (amended 2026-07-18, B13 review): fail-closed covers *collection* failures only; a **computed alert condition** (e.g. a book loss exceeding its b·ln2 bound — an I-12 breach) MUST be carried in the series so its alert fires, never suppressed as a family degradation. One sourcing exception: the LMSR **domain-rejection** component of the Numerics row cannot be a runtime API or storage counter — a rejected extrinsic rolls back all storage — so its audited source is the finalized `System.ExtrinsicFailed` event stream (error identity resolved from metadata, never hardcoded), exported as a cumulative counter; the rounding-dust component rides `TelemetryApi`. Solvency-relevant methods (collateral drift) MUST be computed from the same quantities the owning pallet's try-state checks compare — the API is a window onto audited state, never a second bookkeeping.

The frontend itself still has **no telemetry of any kind**; its only diagnostic channel is user-initiated copy-to-clipboard reports (unchanged). Everything above monitors *infrastructure*, not users.

### 6.4 Incident response

Carried forward, amended: **Hostile release** — 3-of-5 repoint to the last good TXID (O(minutes)); `ReleaseChannel` update (`SECURITY` flag + `min_supported_version` bump, reaching pinned/stranded users per §3.2); key revocation if a signing key is implicated (§2.3); announcement via status page + community channels; postmortem TX. **Wrong-chain-spec** — patch release (bundle pins make cross-environment confusion non-weaponizable). **ArNS-key loss** — with permabuy the name cannot lapse; loss of quorum shares below threshold triggers the FROST/ceremony re-issuance runbook while per-release undernames and immutable TXIDs keep every release reachable; a permanently lost quorum (< 3 recoverable shares) is the declared worst case: a new name is established and announced through `ReleaseChannel` — the chain channel, not the compromised/lost name, is the recovery root. **Distribution mismatch** (monitor alert, §5.2) — treat as hostile release until proven otherwise.

### 6.5 Phase-gate wiring ([09](09-execution-upgrades-and-rollout.md))

The following are **entry-blocking** additions to the phase gates; amounts and phase definitions are normative in [09](09-execution-upgrades-and-rollout.md)/[08](08-treasury-and-economics.md):

- **Phase 2 (public testnet):** bootnode program live on Paseo at full D-6 numbers; monitoring stack incl. browser-dial probes; staging/dev ArNS names; release train exercised end-to-end (incl. one expedited dry run and one rollback drill).
- **Phase 3 (mainnet shadow, production name launches):** permabuy completed; ANT quorum ceremony completed — **launch blocks here if neither native n-of-m nor FROST materializes (§4.2)**; signer-registry disjointness attested (§2.2); out-of-band attestation monitor live with ≥ 2 disjoint operators; mainnet bootnode set + 30-day served-state window operational; `ReleaseChannel` written and read back by a shipped release; coretime quote authority and renewal account seated from the ops ceremony and proven by one end-to-end quote note + prune ([09](09-execution-upgrades-and-rollout.md) §4).
- **Phase 4 (binding decisions):** keeper funding line active with exhaustion alarm; watchtower quorum registered and funded; oracle evidence hosting live; descriptor lead-time monitoring proven against ≥ 1 full CODE/META upgrade e2e (the Phase-2 upgrade rehearsal counts only if the release-gating check fired correctly).

Advancement discipline is unchanged: published evidence + META decision + values ratification; delays always allowed, acceleration never.

---

## Resolves

| Finding | Resolution in this document |
|---|---|
| F-5 | §4: ArNS permabuy eliminates lease-lapse takeover; ANT control is 3-of-5 native n-of-m or FROST-ed25519 fallback; single-key custody prohibited; launch blocks on this line; the capability [VERIFY] is retained with its prototype gate (§4.2). |
| X-13 | §6.1–§6.3: every operational service has a named owner role and a treasury budget-line reference to [08](08-treasury-and-economics.md); ArNS tenure decided (permabuy); costs funded; phase-gated in §6.5. |
| X-4 (ops side) | §6.1–§6.2, §6.5: the WSS bootnode program (≥8/≥4 operators/≥2 on :443) is an owned, funded, monitored commitment wired into the chain spec ([02](02-integration-contract.md)) and the phase gates, with browser-context dial probes; the D-6 30-day served-state window rides the same fleet. |
| F-med: expedited release | §1.5–§1.7, §3: expedited descriptor-only lane (2 attestations, no soak, 3-of-5 repoint, mechanically verified zero app-code delta); descriptor release-gating before execute maturity; rollback semantics restated honestly; pinned-user warning (§3.2) and shipped-keyring revocation (§2.3) close the review's two named gaps. |
| F-med: CSP | §5.1: `connect-src *` replaced by a per-release host allowlist (bootnodes + baked gateways + release-listed opted-in providers), with an honest statement of what it contains (exfiltration bounding) and does not (injection is script-src's job; allowlisted-operator and public-channel exfiltration remain), and the accepted expert-endpoint cost. |
| F-med: hostile SW | §5.2: the §18.6-style self-check detection claim is removed (a hostile SW intercepts the checking fetches); durable per-origin persistence is a declared residual ([14](14-threat-model.md)); compensating control is the required out-of-band attestation monitor with controller-disjoint operators and a named alerting channel. |
| F-med: ArNS lease | §4.1: moot by permabuy; the old lapse threat and its closure are recorded here and in [14](14-threat-model.md). |
| F-med: system.remark (FE side) | §3: the remark pointer is deleted; stranded/pinned apps read the fixed-layout `ReleaseChannel` raw storage key ([09](09-execution-upgrades-and-rollout.md)) without current metadata; in-app warning/blocking/security UX obligations specified per release forever. |
| F-med: signer disjointness | §2.2 (+§5.2 monitor disjointness): ArNS controllers ∩ minisign release keys = ∅ as a hard organizational control with a mechanical registry check, ceremony re-attestation, quarterly audit, a `verify-release` subcommand, and a declared residual with its threat row in [14](14-threat-model.md). |
