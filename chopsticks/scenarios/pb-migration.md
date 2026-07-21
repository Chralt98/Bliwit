<!-- 15 §1/§4.7; 06 §6.2; 09 §3.2. -->
# PB-MIGRATION steps

> **Stale since 2026-07-20 (SQ-127/SQ-144 ruled).** This card still images the
> retired paired `PendingUpgrade` + two-hash checkpoint form. The anchor is now
> captured at code *application*, is `(block_number, block_hash)`, lives in its
> own guard item, and try-state is a one-way implication (09 §3.2(2)). Re-image
> when the implementation lands (PLAN.md SQ-127/SQ-144, batch X).


1. Start `pb-migration.yml`; verify the real migration halt, active playbook,
   and paired pending/checkpoint/authorization-history cells.
2. Resource-bounded branch: invoke the bounded continuation control no more
   than twice, assert a successful retry clears the halt only after try-state.
3. Rollback branch: manufacture a logic/storage-shape fault or a second failed
   retry, assert the default within 72 hours is a forward remediation upgrade
   through the expedited CODE lane with full attestation and ratification.
4. Assert ordinary affected-surface transactions and the execution queue stay
   halted throughout, then run try-state after recovery.

NOTE(B7): 09 §3.2 retains `[VERIFY pallet-migrations control surface]`; B6
must bind the retry and forward-rollback calls. The checkpoint-lifetime-during-
halt question is logged as a `PLAN.md` spec question; until resolved, this card
uses the try-state-valid paired PendingUpgrade+Checkpoint form. The values are
SCALE `[u8;32] ++ u32(0) ++ u32(43_200) ++ u32(2)`, a two-hash checkpoint,
`u32(0)` last authorization, and Compact(1)+`(u32(0),u32(0))` expedited
history, verified against the real types.

NOTE(B7): the ExecutionGuard cells are inert until A11 occupies runtime slot
62 (B1a follow-up).

The Guardian side is live now: its real members, arithmetic bonds, activation
review, expiry, allowance struct, and cursors are StorageValues keyed by
`Twox128("Guardian") ++ Twox128(item)` and SCALE-checked against the Rust types,
so block-number expiry maintenance runs. The pallet has no per-playbook
allowance, registered-playbook/active-hold, migration-resolved lift, or
downstream effect-revert storage. The runtime review scheduler remains a stub
and guardian `CurrentEpoch` stays zero until A8; no 06 §6.2 state is invented.
