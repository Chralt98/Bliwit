// 15 §4.7; 09 §2.1/§3.1/§7.1 — full D-14 expedited CODE-lane drill.
const fs = require("fs");

function eventMatches(events, section, method) {
  return events.some(({ event }) => event.section === section && event.method === method);
}

function submit(call, signer, expectedEvent) {
  return new Promise((resolve, reject) => {
    let unsubscribe;
    let settled = false;
    const finish = (callback) => {
      settled = true;
      if (unsubscribe) unsubscribe();
      callback();
    };
    call.signAndSend(signer, ({ dispatchError, events, status }) => {
      if (dispatchError) {
        finish(() => reject(new Error(dispatchError.toString())));
      } else if (status.isInBlock) {
        if (!eventMatches(events, "executionGuard", expectedEvent)) {
          finish(() => reject(new Error(`in-block receipt has no executionGuard.${expectedEvent}`)));
        } else {
          finish(() => resolve(events));
        }
      }
    }).then((unsub) => {
      unsubscribe = unsub;
      if (settled) unsubscribe();
    }).catch((error) => finish(() => reject(error)));
  });
}

function expectRejected(call, signer) {
  return new Promise((resolve, reject) => {
    let unsubscribe;
    let settled = false;
    const finish = (callback) => {
      settled = true;
      if (unsubscribe) unsubscribe();
      callback();
    };
    call.signAndSend(signer, ({ dispatchError, events, status }) => {
      if (dispatchError && status.isInBlock) {
        finish(() => resolve(dispatchError.toString()));
      } else if (status.isInBlock) {
        const applied = eventMatches(events, "executionGuard", "UpgradeApplied");
        finish(() => reject(new Error(
          applied ? "early apply emitted UpgradeApplied" : "early apply unexpectedly succeeded",
        )));
      }
    }).then((unsub) => {
      unsubscribe = unsub;
      if (settled) unsubscribe();
    }).catch((error) => finish(() => reject(error)));
  });
}

function asNumber(value) {
  return typeof value.toNumber === "function" ? value.toNumber() : Number(value.toString());
}

async function waitUntilBlock(api, target) {
  while (true) {
    const now = (await api.rpc.chain.getHeader()).number.toNumber();
    if (now >= target) return now;
    await new Promise((resolve) => setTimeout(resolve, 6_000));
  }
}

async function run(nodeName, networkInfo, args) {
  const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
  const api = await zombie.connect(wsUri, userDefinedTypes);
  await zombie.util.cryptoWaitReady();
  const keyring = new zombie.Keyring({ type: "sr25519", ss58Format: api.registry.chainSS58 });
  const alice = keyring.addFromUri("//Alice");
  const values = args.length === 1 ? String(args[0]).split(",") : args;
  const pid = Number(values[0]);
  const code = fs.readFileSync(values[1]);

  if (!api.tx.executionGuard || !api.query.executionGuard) {
    throw new Error("NOTE(B7): A11 ExecutionGuard runtime wiring is required");
  }

  const missing = [];
  const flags = (await api.query.constitution.phaseFlags()).toNumber();
  if ((flags & (1 << 5)) === 0) missing.push("PhaseFlags bit 5");
  const active = await api.query.guardian.activePlaybooks();
  if (!active.some((record) => record.id.toString() === "LedgerFreeze")) {
    missing.push("active LedgerFreeze record");
  }
  const queued = await api.query.executionGuard.queue(pid);
  const expedited = await api.query.executionGuard.expedited(pid);
  if (queued.isNone) missing.push(`queued proposal ${pid}`);
  if (!expedited.isTrue) missing.push(`Expedited[${pid}]=true`);
  if (missing.length) {
    throw new Error(`staged expedited-lane preconditions missing: ${missing.join(", ")}`);
  }

  await submit(api.tx.executionGuard.execute(pid), alice, "UpgradeAuthorized");
  const pending = await api.query.executionGuard.pendingUpgrade();
  if (pending.isNone) throw new Error("authorization emitted but PendingUpgrade is absent");
  const record = pending.unwrap();
  const authorizedAt = asNumber(record.authorizedAt);
  const applicableAt = asNumber(record.applicableAt);
  if (applicableAt - authorizedAt !== 43_200) {
    throw new Error(
      `D-14 lead time is ${applicableAt - authorizedAt} blocks, expected 43200`,
    );
  }

  await expectRejected(api.tx.executionGuard.applyAuthorizedUpgrade(code), alice);
  await waitUntilBlock(api, applicableAt);
  await submit(
    api.tx.executionGuard.applyAuthorizedUpgrade(code),
    alice,
    "UpgradeApplied",
  );
  return applicableAt;
}

module.exports = { run };
