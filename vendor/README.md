# Vendored crates

## `core2-0.4.0`

Byte-for-byte extraction of the published crates.io package
`core2-0.4.0.crate` (sha256
`b49ba7ef1ad6107f8824dbe97de947cbaac53c44e7f9756a1fba0d37c1eec505`),
minus cargo's packaging markers (`.cargo-ok`, `.cargo_vcs_info.json`).
Licensed MIT OR Apache-2.0 (both license texts included in the directory).

**Why vendored.** Every published version of `core2` is yanked on
crates.io, and the `polkadot-stable2603` node stack hard-requires it
(`sc-network 0.56.x → litep2p 0.13.x → multihash 0.17 → core2 ^0.4`).
Fresh dependency resolution can never select a yanked version, so the
collator node (`node/bleavit-node`) would be unresolvable from the
registry alone. The workspace `[patch.crates-io]` entry points `core2`
at this vendored copy, which keeps resolution deterministic, `--locked`
CI honest, and the source auditable in-tree — consistent with the
supply-chain pinning regime (01 §9; 15 §4.5).

Runtime/pallet crates do not depend on `core2`; the patch only feeds the
node's networking dependency closure.
