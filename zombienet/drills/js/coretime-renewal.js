// 15 §4.7; 09 §4/§7.1 — permissionless renewal under every freeze.
//
// Two-form assertion (PR #63 Codex P1): quotes are noted only by the
// runtime-internal B4 Coretime re-read seam and `ops.coretime` is funded only
// through the FutarchyTreasury class origin — both deliberately unreachable
// from a chain spec or a signed extrinsic, so nothing can pre-stage a quote in
// this topology until the B1a seam wiring lands.
// - staged (quote + funded line present): the renewal must fully succeed and
//   emit `CoretimeRenewalCalled` — the Phase-1 exit form (09 §7.1).
// - unstaged (today's runtime): the dispatch must still REACH the pallet
//   through the active dead-man freeze and fail with the treasury-internal
//   `RenewalWindowClosed` — which proves the 09 §4 freeze exemption (a
//   SafetyFilter/freeze rejection such as `CallFiltered` fails this drill).
async function run(nodeName, networkInfo, args) {
  const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
  const api = await zombie.connect(wsUri, userDefinedTypes);
  await zombie.util.cryptoWaitReady();
  const keyring = new zombie.Keyring({ type: "sr25519", ss58Format: api.registry.chainSS58 });
  const alice = keyring.addFromUri("//Alice");
  const periodIndex = Number(args[0]);

  if (!api.tx.futarchyTreasury?.executeCoretimeRenewal) {
    throw new Error("execute_coretime_renewal is absent from runtime metadata");
  }
  if (!api.query.futarchyTreasury?.state) {
    throw new Error("futarchyTreasury.State storage is absent from runtime metadata");
  }
  const state = (await api.query.futarchyTreasury.state()).toJSON() ?? {};
  const quotes = state.coretimeQuotes ?? [];
  const quote = quotes.find(([period]) => Number(period) === periodIndex);
  const lines = state.lines ?? [];
  const opsCoretime = lines.find(
    ([line]) =>
      line === "OpsCoretime" || line === "opsCoretime" || line?.opsCoretime !== undefined,
  );
  const staged = Boolean(quote) && Boolean(opsCoretime) && BigInt(opsCoretime[1]) >= BigInt(quote[1]);

  const outcome = await new Promise((resolve, reject) => {
    let unsubscribe;
    let settled = false;
    const finish = (callback) => {
      settled = true;
      if (unsubscribe) unsubscribe();
      callback();
    };
    api.tx.futarchyTreasury
      .executeCoretimeRenewal(periodIndex)
      .signAndSend(alice, ({ dispatchError, events, status }) => {
        if (dispatchError) {
          if (dispatchError.isModule) {
            const decoded = api.registry.findMetaError(dispatchError.asModule);
            finish(() => resolve({ error: `${decoded.section}.${decoded.name}` }));
          } else {
            finish(() => resolve({ error: dispatchError.toString() }));
          }
        } else if (status.isInBlock) {
          const success = events.some(
            ({ event }) => event.section === "system" && event.method === "ExtrinsicSuccess",
          );
          const renewed = events.some(
            ({ event }) =>
              event.section === "futarchyTreasury" && event.method === "CoretimeRenewalCalled",
          );
          if (success) finish(() => resolve({ renewed }));
        }
      })
      .then((unsub) => {
        unsubscribe = unsub;
        if (settled) unsubscribe();
      })
      .catch((error) => finish(() => reject(error)));
  });

  if (staged) {
    if (outcome.error) {
      throw new Error(`staged renewal dispatch failed: ${outcome.error}`);
    }
    if (!outcome.renewed) {
      throw new Error("staged renewal succeeded without CoretimeRenewalCalled");
    }
    return { periodIndex, form: "staged-renewal" };
  }
  if (outcome.error === "futarchyTreasury.RenewalWindowClosed") {
    // NOTE(B7): the pre-staging form — the call traversed the dead-man freeze
    // and every filter into treasury logic (09 §4 exemption proven); the full
    // staged-renewal form activates once B1a wires the quote-noting seam and
    // the ops.coretime funding path (09 §7.1 Phase-1 exit runs that form).
    return { periodIndex, form: "exemption-reachability" };
  }
  throw new Error(
    `coretime renewal neither succeeded nor proved freeze-exempt reachability: ${
      outcome.error ?? "ExtrinsicSuccess without CoretimeRenewalCalled"
    }`,
  );
}

module.exports = { run };
