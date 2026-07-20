// NOTE(SQ-274; R-7): no production migrations.force_failure/retry surface
// exists or should. A real stuck FRAME cursor would trigger the MBM lockdown
// that pauses the guardian workflow, so this drill stages MigrationHalt=true
// at genesis instead (see bleavit-migration.toml / generate-relay-specs.sh).
// 15 §4.7; 06 §5.1/§6.2; 09 §3.2/§7.1 — PB-MIGRATION driver.
const MIGRATION_HALT_STORAGE_KEY = "0x0fa4af4f19b810e797f335f5b2f479282405b4b29c977f7ec63d38c3ae2db231";

function findEvent(events, section, method) {
  return events.find(({ event }) => event.section === section && event.method === method)?.event;
}

function submit(call, signer, expected = [], label = "") {
  const context = label || `${call.method.section}.${call.method.method}`;
  return new Promise((resolve, reject) => {
    let unsubscribe;
    let settled = false;
    const finish = (callback) => {
      settled = true;
      if (unsubscribe) unsubscribe();
      callback();
    };
    const fail = (error) => reject(new Error(`${context}: ${error}`));
    call.signAndSend(signer, ({ dispatchError, events, status }) => {
      if (dispatchError) {
        finish(() => fail(dispatchError.toString()));
      } else if (status.isInBlock) {
        const missing = expected.filter(([section, method]) => !findEvent(events, section, method));
        if (missing.length) {
          finish(() => fail(
            `in-block receipt missing ${missing.map((entry) => entry.join(".")).join(", ")}`,
          ));
        } else {
          finish(() => resolve(events));
        }
      }
    }).then((unsub) => {
      unsubscribe = unsub;
      if (settled) unsubscribe();
    }).catch((error) => finish(() => fail(error)));
  });
}

async function ensureMembershipAndFunding(api, keyring) {
  const membersValue = await api.query.guardian.members();
  // Guardian `Members` is `[Option<AccountId>; 7]` — a vacancy-aware seat array
  // (B1b recall semantics), not a plain `[u8;32]` array. The query may surface
  // as an Option-wrapped array or the array directly; normalise, then unwrap
  // each seated `Option<AccountId>` to its raw account key. Compare raw keys on
  // both sides (never ss58) so the chain's display prefix is irrelevant.
  const seatArray =
    membersValue && typeof membersValue.unwrapOr === "function"
      ? membersValue.unwrapOr(null)
      : membersValue;
  if (!seatArray) {
    throw new Error("NOTE(B7): injected seven-seat guardian membership is absent");
  }
  const members = [...seatArray]
    .map((seat) => (seat && typeof seat.unwrapOr === "function" ? seat.unwrapOr(null) : seat))
    .filter((seat) => seat && !seat.isEmpty)
    .map((account) => account.toHex());
  const signers = ["//Alice", "//Bob", "//Charlie", "//Dave", "//Eve"]
    .map((uri) => keyring.addFromUri(uri));
  const missing = signers.filter(
    (signer) => !members.includes(api.createType("AccountId32", signer.publicKey).toHex()),
  );
  if (missing.length) {
    throw new Error(`injected guardian membership omits ${missing.map((s) => s.address).join(", ")}`);
  }

  const alice = signers[0];
  const transfer = api.tx.balances?.transferAllowDeath;
  if (!transfer) throw new Error("balances.transfer_allow_death is absent");
  for (const signer of signers.slice(1)) {
    const account = await api.query.system.account(signer.address);
    // Fees ride the fungible adapter, which cannot touch frozen (vesting-locked)
    // funds — the genesis-vested guardians have large `free` but zero usable
    // balance, so gate the top-up on free minus frozen, not free.
    const usable = account.data.free.toBigInt() - account.data.frozen.toBigInt();
    if (usable < 1_000_000_000_000n) {
      await submit(transfer(signer.address, 2_000_000_000_000n), alice, [], `fund ${signer.address}`);
    }
  }
  return signers;
}

async function guardianRollbackWorkflow(api, keyring) {
  const guardian = api.tx.guardian;
  if (!guardian?.proposeAction || !guardian?.approveAction) {
    throw new Error("guardian propose_action/approve_action workflow is absent");
  }
  const signers = await ensureMembershipAndFunding(api, keyring);
  const height = (await api.rpc.chain.getHeader()).number.toNumber();
  const power = {
    activatePlaybook: {
      id: "Migration",
      trigger: "MigrationHalt",
      expiry: height + 201_600,
    },
  };
  const justification = `0x${"b7".repeat(32)}`;
  const proposed = await submit(
    guardian.proposeAction(power, justification),
    signers[0],
    [["guardian", "ActionProposed"]],
  );
  const action = findEvent(proposed, "guardian", "ActionProposed");
  const actionId = action.data[0].toNumber();
  for (const signer of signers.slice(1, 4)) {
    await submit(guardian.approveAction(actionId), signer, [["guardian", "ActionApproved"]], `guardian.approveAction by ${signer.address}`);
  }
  // The Migration playbook is the one 06 §6.2 power with no EmergencyPlaybook-safe
  // runtime effect: retrying/rolling back a stuck migration needs Root-only
  // pallet-migrations cursor controls, and fabricating Root inside an
  // EmergencyPlaybook dispatch would widen that origin beyond the pre-ratified
  // 06 §6.2 surface (R-7 — runtime `playbook_calls(Migration)` returns
  // `Other("PB-MIGRATION cursor retry has no EmergencyPlaybook-safe runtime
  // call")`). The freeze arm is automatic — the halt-source bridge engaged
  // `MigrationHalt`, which the `assert-halt` leg proves — and the retry/rollback
  // is the ratified expedited-CODE remediation lane; neither is a guardian call.
  // So with the trigger ACTIVE (staged at genesis) the dispatching 5th approval
  // must fail CLOSED with `DispatchError::Other`, NOT `guardian.TriggerInactive`
  // (which would mean the staged trigger never engaged) and NOT a successful
  // `PlaybookActivated`. That precise fail-closed refusal is this drill's
  // PB-MIGRATION recovery assertion.
  let refusal = null;
  try {
    await submit(guardian.approveAction(actionId), signers[4], [], "dispatching approval");
  } catch (error) {
    refusal = String(error);
  }
  if (refusal === null) {
    throw new Error(
      "the Migration playbook unexpectedly activated — it must have no EmergencyPlaybook-safe effect (R-7)",
    );
  }
  if (/TriggerInactive/i.test(refusal)) {
    throw new Error(
      `dispatching approval refused with TriggerInactive — the staged MigrationHalt trigger never engaged: ${refusal}`,
    );
  }
  if (!/\bOther\b/.test(refusal)) {
    throw new Error(
      `dispatching approval failed with an unexpected error (expected DispatchError::Other, the no-EmergencyPlaybook-safe-call refusal): ${refusal}`,
    );
  }
  return actionId;
}

async function run(nodeName, networkInfo, args) {
  const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
  const api = await zombie.connect(wsUri, userDefinedTypes);
  await zombie.util.cryptoWaitReady();
  const keyring = new zombie.Keyring({ type: "sr25519", ss58Format: api.registry.chainSS58 });
  const branch = args[0];

  if (branch === "assert-halt") {
    const migrationHalt = api.query.executionGuard?.migrationHalt;
    if (!migrationHalt) {
      throw new Error("executionGuard.migrationHalt storage query is absent from runtime metadata");
    }
    const halt = await migrationHalt();
    // `.key()` already returns the 0x-prefixed hex storage key (a string).
    const actualKey = migrationHalt.key();
    if (!halt.isTrue) {
      throw new Error(`staged executionGuard.migrationHalt decoded to ${halt.toString()}, expected true`);
    }
    if (actualKey !== MIGRATION_HALT_STORAGE_KEY) {
      throw new Error(
        `executionGuard.migrationHalt storage key is ${actualKey}, expected ${MIGRATION_HALT_STORAGE_KEY}`,
      );
    }
    return;
  }
  if (branch === "rollback") return guardianRollbackWorkflow(api, keyring);
  throw new Error(`unknown PB-MIGRATION branch '${branch}'`);
}

module.exports = { run };
