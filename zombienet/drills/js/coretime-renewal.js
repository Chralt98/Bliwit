// 15 §4.7; 09 §4/§7.1 — permissionless renewal under every freeze.
//
// The local preset makes Alice the authenticated quote authority and renewal
// account. The `ops.coretime` line and its backing DOT custody are injected by
// the drill chain-spec generator: both `fund_budget_line` and
// `set_coretime_authority` require the exact FutarchyTreasury custom origin,
// which sudo's Root dispatch cannot manufacture. The stage branch proves those
// prerequisites are present and notes a fresh quote through the real signed
// call. The execute branch then proves the staged-renewal Phase-1 exit form.
// The probe branch retains the pre-staging exemption-reachability assertion.
const QUOTE_PRICE_PLANCK = 1n;

function findEvent(events, section, method) {
  return events.find(({ event }) => event.section === section && event.method === method)?.event;
}

function decodedDispatchError(api, dispatchError) {
  if (dispatchError.isModule) {
    const decoded = api.registry.findMetaError(dispatchError.asModule);
    return `${decoded.section}.${decoded.name}`;
  }
  return dispatchError.toString();
}

function submit(api, call, signer, label) {
  return new Promise((resolve, reject) => {
    let unsubscribe;
    let settled = false;
    const finish = (callback) => {
      settled = true;
      if (unsubscribe) unsubscribe();
      callback();
    };
    call
      .signAndSend(signer, ({ dispatchError, events, status }) => {
        if (dispatchError) {
          finish(() => resolve({ error: decodedDispatchError(api, dispatchError), events }));
        } else if (status.isInBlock) {
          finish(() => resolve({ events }));
        }
      })
      .then((unsub) => {
        unsubscribe = unsub;
        if (settled) unsubscribe();
      })
      .catch((error) => finish(() => reject(new Error(`${label}: ${error}`))));
  });
}

function decodedLine(line) {
  return line === "OpsCoretime" || line === "opsCoretime" || line?.opsCoretime !== undefined;
}

function quotePeriod(quote) {
  if (Array.isArray(quote)) return quote[0];
  return quote.periodIndex ?? quote.period_index;
}

function quotePrice(quote) {
  if (Array.isArray(quote)) return quote[1];
  return quote.price;
}

function readStagingState(state, periodIndex) {
  const json = state.toJSON() ?? {};
  const quotes = json.coretimeQuotes ?? [];
  const lines = json.lines ?? [];
  const quote = quotes.find((candidate) => Number(quotePeriod(candidate)) === periodIndex);
  const opsCoretime = lines.find(([line]) => decodedLine(line));
  const funded = (json.fundedCoretimePeriods ?? []).some(
    (period) => Number(period) === periodIndex,
  );
  return { funded, opsCoretime, quote };
}

function assertEvents(events, expected, label) {
  const missing = expected.filter(([section, method]) => !findEvent(events, section, method));
  if (missing.length) {
    throw new Error(
      `${label}: in-block receipt missing ${missing.map((entry) => entry.join(".")).join(", ")}`,
    );
  }
}

async function assertGenesisStaging(api, alice, periodIndex) {
  if (!api.query.futarchyTreasury?.coretimeQuoteAuthority) {
    throw new Error("futarchyTreasury.CoretimeQuoteAuthority storage is absent from metadata");
  }
  if (!api.query.futarchyTreasury?.coretimeRenewalAccount) {
    throw new Error("futarchyTreasury.CoretimeRenewalAccount storage is absent from metadata");
  }
  const authority = await api.query.futarchyTreasury.coretimeQuoteAuthority();
  if (authority.isNone) {
    throw new Error("genesis staging omitted futarchyTreasury.CoretimeQuoteAuthority");
  }
  const authorityKey = api.createType("AccountId32", authority.unwrap().toU8a()).toHex();
  const aliceKey = api.createType("AccountId32", alice.publicKey).toHex();
  if (authorityKey !== aliceKey) {
    throw new Error(`genesis quote authority is ${authorityKey}, expected drill Alice ${aliceKey}`);
  }
  const renewalAccount = await api.query.futarchyTreasury.coretimeRenewalAccount();
  if (renewalAccount.isNone) {
    throw new Error("genesis staging omitted futarchyTreasury.CoretimeRenewalAccount");
  }
  const { funded, opsCoretime } = readStagingState(
    await api.query.futarchyTreasury.state(),
    periodIndex,
  );
  if (funded) {
    throw new Error(`coretime period ${periodIndex} is already marked funded at drill start`);
  }
  if (!opsCoretime || BigInt(opsCoretime[1]) === 0n) {
    throw new Error(
      "genesis staging omitted a nonzero futarchyTreasury.State ops.coretime line",
    );
  }
}

async function stage(api, alice, periodIndex) {
  if (!api.tx.futarchyTreasury?.noteCoretimeQuote) {
    throw new Error("futarchyTreasury.note_coretime_quote is absent from runtime metadata");
  }
  await assertGenesisStaging(api, alice, periodIndex);
  const outcome = await submit(
    api,
    api.tx.futarchyTreasury.noteCoretimeQuote(periodIndex, QUOTE_PRICE_PLANCK),
    alice,
    "stage coretime quote",
  );
  if (outcome.error) {
    throw new Error(`stage coretime quote: dispatch failed: ${outcome.error}`);
  }
  assertEvents(
    outcome.events,
    [
      ["system", "ExtrinsicSuccess"],
      ["futarchyTreasury", "CoretimeQuoteNoted"],
    ],
    "stage coretime quote",
  );
  const { quote } = readStagingState(await api.query.futarchyTreasury.state(), periodIndex);
  if (!quote || BigInt(quotePrice(quote)) !== QUOTE_PRICE_PLANCK) {
    throw new Error(`fresh quote for coretime period ${periodIndex} was not persisted`);
  }
  return { periodIndex, price: QUOTE_PRICE_PLANCK.toString() };
}

async function execute(api, alice, periodIndex) {
  if (!api.tx.futarchyTreasury?.executeCoretimeRenewal) {
    throw new Error("futarchyTreasury.execute_coretime_renewal is absent from runtime metadata");
  }
  const before = readStagingState(await api.query.futarchyTreasury.state(), periodIndex);
  if (!before.quote || !before.opsCoretime || BigInt(before.opsCoretime[1]) === 0n) {
    throw new Error(`coretime period ${periodIndex} is not staged with a quote and funded line`);
  }
  const outcome = await submit(
    api,
    api.tx.futarchyTreasury.executeCoretimeRenewal(periodIndex),
    alice,
    "execute staged coretime renewal",
  );
  if (outcome.error) {
    throw new Error(`staged coretime renewal dispatch failed: ${outcome.error}`);
  }
  assertEvents(
    outcome.events,
    [
      ["system", "ExtrinsicSuccess"],
      ["futarchyTreasury", "CoretimeRenewalCalled"],
    ],
    "execute staged coretime renewal",
  );
  const after = readStagingState(await api.query.futarchyTreasury.state(), periodIndex);
  if (!after.funded || after.quote) {
    throw new Error(
      `successful renewal left period ${periodIndex} without its funded marker or retained its quote`,
    );
  }
  return { periodIndex, form: "staged-renewal" };
}

async function probe(api, alice, periodIndex) {
  // Freeze-exemption reachability proof: with no quote staged for `periodIndex`
  // (genesis seeds the authority + renewal account but no quote), the
  // permissionless renewal must REACH its treasury business logic
  // (RenewalWindowClosed) rather than be blocked by the engaged dead-man freeze
  // (09 §4 D-9). It dispatches directly and deliberately does NOT pre-read
  // `futarchyTreasury.state`: that raw-storage read hits a portable-metadata
  // SCALE round-trip quirk on the rolling-meter TreasuryState (SQ-284) — the
  // runtime round-trips its own state and the 02 contract's consumers read
  // FutarchyApi views, not raw storage, so the reachability proof needs neither.
  const outcome = await submit(
    api,
    api.tx.futarchyTreasury.executeCoretimeRenewal(periodIndex),
    alice,
    "probe freeze-exempt coretime renewal",
  );
  if (outcome.error === "futarchyTreasury.RenewalWindowClosed") {
    return { periodIndex, form: "exemption-reachability" };
  }
  throw new Error(
    `coretime renewal did not prove freeze-exempt treasury reachability: ${
      outcome.error ?? "unexpected ExtrinsicSuccess"
    }`,
  );
}

async function run(nodeName, networkInfo, args) {
  const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
  const api = await zombie.connect(wsUri, userDefinedTypes);
  await zombie.util.cryptoWaitReady();
  const keyring = new zombie.Keyring({ type: "sr25519", ss58Format: api.registry.chainSS58 });
  const alice = keyring.addFromUri("//Alice");
  const branch = args[0];
  const periodIndex = Number(args[1]);
  if (!Number.isSafeInteger(periodIndex) || periodIndex < 0 || periodIndex > 0xffffffff) {
    throw new Error(`invalid coretime period index '${args[1]}'`);
  }
  if (!api.query.futarchyTreasury?.state) {
    throw new Error("futarchyTreasury.State storage is absent from runtime metadata");
  }
  if (branch === "stage") return stage(api, alice, periodIndex);
  if (branch === "execute") return execute(api, alice, periodIndex);
  if (branch === "probe") return probe(api, alice, periodIndex);
  throw new Error(`unknown coretime-renewal branch '${branch}'`);
}

module.exports = { run };
