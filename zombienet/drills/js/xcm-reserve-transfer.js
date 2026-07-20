// 15 §4.7; 02 §8; 09 §6.1 — correlated v5 AH↔Bleavit XCM drill.
const fs = require("fs");
const path = require("path");

const STATE_FILE = path.join(process.cwd(), "target", "env", "xcm-drill-state.json");
const USDC_AMOUNT = 1_000_000n;

function json(codec) {
  return codec && typeof codec.toJSON === "function" ? codec.toJSON() : codec;
}

function text(value) {
  return JSON.stringify(json(value));
}

function scalarOccurs(value, expected) {
  const actualText = String(value).toLowerCase().replace(/^0x/, "");
  const expectedText = String(expected).toLowerCase().replace(/^0x/, "");
  if (value === expected || actualText === expectedText) return true;
  if (Array.isArray(value)) return value.some((entry) => scalarOccurs(entry, expected));
  return value && typeof value === "object"
    ? Object.values(value).some((entry) => scalarOccurs(entry, expected))
    : false;
}

function namedValues(value, wanted) {
  const found = [];
  if (Array.isArray(value)) {
    for (const entry of value) found.push(...namedValues(entry, wanted));
  } else if (value && typeof value === "object") {
    for (const [key, entry] of Object.entries(value)) {
      if (key.toLowerCase() === wanted.toLowerCase()) found.push(entry);
      found.push(...namedValues(entry, wanted));
    }
  }
  return found;
}

function assertV5(value, label) {
  const object = json(value);
  if (!object || typeof object !== "object" || !Object.keys(object).some((key) => key.toLowerCase() === "v5")) {
    throw new Error(`${label} is not an XCM v5 value: ${text(object)}`);
  }
}

function assertAhOrigin(value) {
  const object = json(value);
  if (!namedValues(object, "sibling").some((entry) => Number(entry) === 1000)) {
    throw new Error(`destination event origin is not Asset Hub sibling 1000: ${text(object)}`);
  }
}

function fungibleAmount(value) {
  const amounts = namedValues(json(value), "fungible");
  if (!amounts.length) throw new Error(`event has no fungible asset amount: ${text(value)}`);
  return BigInt(amounts[0]);
}

function eventOf(events, section, method) {
  return events.find(({ event }) => event.section === section && event.method === method)?.event;
}

function submit(call, signer, errorPrefix = "dispatch failed") {
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
        finish(() => reject(new Error(`${errorPrefix}: ${dispatchError.toString()}`)));
      } else if (status.isInBlock) {
        finish(() => resolve(events));
      }
    }).then((unsub) => {
      unsubscribe = unsub;
      if (settled) unsubscribe();
    }).catch((error) => finish(() => reject(error)));
  });
}

function readState() {
  return fs.existsSync(STATE_FILE) ? JSON.parse(fs.readFileSync(STATE_FILE, "utf8")) : {};
}

function writeState(update) {
  fs.mkdirSync(path.dirname(STATE_FILE), { recursive: true });
  fs.writeFileSync(
    STATE_FILE,
    `${JSON.stringify({ ...readState(), ...update }, null, 2)}\n`,
    { encoding: "utf8", mode: 0o600 },
  );
}

async function connect(networkInfo, nodeName) {
  const node = networkInfo.nodesByName[nodeName];
  if (!node) throw new Error(`topology has no node '${nodeName}'`);
  return zombie.connect(node.wsUri, node.userDefinedTypes);
}

async function assertB4DestinationReady(networkInfo) {
  const api = await connect(networkInfo, "bleavit-collator-1");
  // Readiness is a genuine capability probe, not a version sentinel. The B10
  // production XCM composition (AssetTransactor over Location-keyed ForeignAssets,
  // BleavitBarrier, IsReserve/IsTeleporter, OriginConverter, sender) is present
  // exactly when the canonical XCM v5 Location-keyed USDC is registered — it is
  // seeded in the Bleavit genesis (SQ-101). The former `spec_version === 1` gate
  // predated that wiring and refused the still-unbumped release runtime even
  // after B10 landed; it is retired in favour of this capability check.
  if (!api.query.foreignAssets?.asset) {
    throw new Error(
      "NOTE(B10): Bleavit runtime exposes no foreignAssets pallet — the XCM composition is absent",
    );
  }
  const canonical = {
    parents: 1,
    interior: { X3: [{ Parachain: 1000 }, { PalletInstance: 50 }, { GeneralIndex: 1337 }] },
  };
  try {
    const asset = await api.query.foreignAssets.asset(canonical);
    if (asset.isNone) {
      throw new Error("canonical Location-keyed USDC is unregistered (SQ-101 genesis wiring absent)");
    }
  } catch (error) {
    throw new Error(`NOTE(B10): Bleavit canonical Location-keyed USDC is unavailable: ${error}`);
  }
  return api;
}

async function blockEvents(api, number) {
  const hash = await api.rpc.chain.getBlockHash(number);
  const at = await api.at(hash);
  return at.query.system.events();
}

async function waitForProcessed(api, messageId, start) {
  const deadline = Date.now() + 300_000;
  let next = start;
  while (Date.now() < deadline) {
    const head = (await api.rpc.chain.getHeader()).number.toNumber();
    while (next <= head) {
      const events = await blockEvents(api, next);
      const processed = events
        .map(({ event }) => event)
        .find((event) => event.section === "messageQueue" && event.method === "Processed"
          && event.data[0].toHex() === messageId);
      if (processed) return { block: next, events, processed };
      next += 1;
    }
    await new Promise((resolve) => setTimeout(resolve, 6_000));
  }
  throw new Error(`Bleavit emitted no correlated MessageQueue.Processed for ${messageId}`);
}

function assertSent(events, alice, expectedBeneficiary, amount) {
  const sent = eventOf(events, "polkadotXcm", "Sent");
  if (!sent) throw new Error("source in-block receipt has no polkadotXcm.Sent");
  const [origin, destination, message, messageId] = sent.data;
  assertV5(destination, "Sent.destination");
  assertV5(message, "Sent.message");
  if (!scalarOccurs(json(origin), alice.address)
      && !scalarOccurs(json(origin), Buffer.from(alice.publicKey).toString("hex"))) {
    throw new Error(`Sent.origin does not identify Alice: ${text(origin)}`);
  }
  const decodedDestination = json(destination);
  if (!scalarOccurs(decodedDestination, 4242)) {
    throw new Error(`Sent.destination is not para 4242: ${text(decodedDestination)}`);
  }
  const decodedMessage = json(message);
  for (const required of [50, 1337, amount.toString(), expectedBeneficiary]) {
    if (!scalarOccurs(decodedMessage, required)) {
      throw new Error(`Sent.message omits correlated field ${required}: ${text(decodedMessage)}`);
    }
  }
  return messageId.toHex();
}

function transferInputs(alice, failure) {
  const destination = { V5: { parents: 1, interior: { X1: [{ Parachain: 4242 }] } } };
  const beneficiary = failure
    ? { V5: { parents: 0, interior: { X1: [{ PalletInstance: 255 }] } } }
    : {
        V5: {
          parents: 0,
          interior: { X1: [{ AccountId32: { network: null, id: alice.publicKey } }] },
        },
      };
  const asset = {
    id: { parents: 0, interior: { X2: [{ PalletInstance: 50 }, { GeneralIndex: 1337 }] } },
    fun: { Fungible: USDC_AMOUNT },
  };
  return { destination, beneficiary, assets: { V5: [asset] } };
}

async function setup(api, alice, networkInfo) {
  await assertB4DestinationReady(networkInfo); // fail before any AH debit/mint mutation
  const assets = api.tx.assets;
  const sudo = api.tx.sudo?.sudo;
  if (!assets?.forceCreate || !assets?.mint || !sudo) {
    throw new Error("Asset Hub local assets/sudo setup surface is absent");
  }
  if ((await api.query.assets.asset(1337)).isNone) {
    await submit(sudo(assets.forceCreate(1337, alice.address, true, 10_000)), alice);
  }
  const account = await api.query.assets.account(1337, alice.address);
  const balance = account.isSome ? account.unwrap().balance.toBigInt() : 0n;
  if (balance < 10_000_000n) {
    await submit(assets.mint(1337, alice.address, 10_000_000n - balance), alice);
  }
  return (await api.query.assets.account(1337, alice.address)).unwrap().balance.toString();
}

async function transfer(api, alice, networkInfo, failure) {
  await assertB4DestinationReady(networkInfo);
  const call = api.tx.polkadotXcm?.limitedReserveTransferAssets;
  if (!call) throw new Error("Asset Hub reserve-transfer call is absent");
  const bleavit = await connect(networkInfo, "bleavit-collator-1");
  const start = (await bleavit.rpc.chain.getHeader()).number.toNumber();
  const inputs = transferInputs(alice, failure);
  const receipt = await submit(
    call(inputs.destination, inputs.beneficiary, inputs.assets, 0, "Unlimited"),
    alice,
  );
  const beneficiary = failure ? 255 : Buffer.from(alice.publicKey).toString("hex");
  const messageId = assertSent(receipt, alice, beneficiary, USDC_AMOUNT);
  const destination = await waitForProcessed(bleavit, messageId, start);
  const succeeded = destination.processed.data[destination.processed.data.length - 1].isTrue;
  assertAhOrigin(destination.processed.data[1]);

  if (failure) {
    if (succeeded) throw new Error("unconvertible-beneficiary transfer unexpectedly succeeded");
    writeState({ failure: { messageId, block: destination.block, amount: USDC_AMOUNT.toString() } });
    return destination.block;
  }
  if (!succeeded) throw new Error("valid AH→Bleavit USDC transfer was processed as failure");
  const deposited = eventOf(destination.events, "foreignAssets", "Deposited");
  if (!deposited) throw new Error("successful destination block has no ForeignAssets.Deposited");
  const [assetId, who, depositedAmount] = deposited.data;
  const canonical = json(assetId);
  for (const required of [1, 1000, 50, 1337]) {
    if (!scalarOccurs(canonical, required)) {
      throw new Error(`Deposited.assetId omits canonical location field ${required}`);
    }
  }
  if (!scalarOccurs(json(who), alice.address)) throw new Error("Deposited.who is not Alice");
  const amount = BigInt(depositedAmount.toString());
  if (amount <= 0n || amount > USDC_AMOUNT) throw new Error(`invalid deposited amount ${amount}`);
  return destination.block;
}

async function trap(networkInfo) {
  const state = readState();
  if (!state.failure) throw new Error("failure leg has not recorded a correlated message");
  const api = await connect(networkInfo, "bleavit-collator-1");
  const events = await blockEvents(api, state.failure.block);
  const trapped = eventOf(events, "polkadotXcm", "AssetsTrapped");
  if (!trapped) throw new Error("failed inbound block has no polkadotXcm.AssetsTrapped");
  const [hash, origin, assets] = trapped.data;
  assertAhOrigin(origin);
  assertV5(assets, "AssetsTrapped.assets");
  const amount = fungibleAmount(assets);
  if (amount <= 0n || amount > BigInt(state.failure.amount)) {
    throw new Error(`trapped amount ${amount} is not correlated to sent amount ${state.failure.amount}`);
  }
  writeState({
    trap: { hash: hash.toHex(), origin: json(origin), assets: json(assets), amount: amount.toString() },
  });
  return hash.toHex();
}

function unwrapV5(versioned) {
  const key = Object.keys(versioned).find((entry) => entry.toLowerCase() === "v5");
  if (!key) throw new Error("stored trap assets are not v5");
  return versioned[key];
}

function isHereOrigin(value) {
  const object = json(value);
  return scalarOccurs(object, "Here") && namedValues(object, "parents").some((entry) => Number(entry) === 0);
}

async function recovery(api, alice, networkInfo) {
  const state = readState();
  if (!state.trap) throw new Error("trap leg has not recorded exact trapped assets");
  const send = api.tx.polkadotXcm?.send;
  const sudo = api.tx.sudo?.sudo;
  if (!send || !sudo) {
    throw new Error(
      "NOTE(B7): Asset Hub v5 ClaimAsset dispatch needs polkadotXcm.send behind the local preset's sudo " +
        "(a signed send would descend to Alice's account origin and can never match the AH-chain-keyed trap)",
    );
  }
  const bleavit = await connect(networkInfo, "bleavit-collator-1");
  const start = (await bleavit.rpc.chain.getHeader()).number.toNumber();
  const assets = unwrapV5(state.trap.assets);
  const firstAsset = assets[0];
  const destination = { V5: { parents: 1, interior: { X1: [{ Parachain: 4242 }] } } };
  const beneficiary = {
    parents: 0,
    interior: { X1: [{ AccountId32: { network: null, id: alice.publicKey } }] },
  };
  const message = {
    V5: [
      { ClaimAsset: { assets, ticket: { parents: 0, interior: "Here" } } },
      { BuyExecution: { fees: firstAsset, weightLimit: "Unlimited" } },
      { DepositAsset: { assets: { Wild: { AllCounted: 1 } }, beneficiary } },
    ],
  };
  let receipt;
  try {
    // Root-dispatched send: pallet_xcm converts Root to the bare chain origin
    // (no DescendOrigin is prepended), so the message reaches Bleavit as the
    // sibling-AH origin the trap is keyed under (amended 09 §6.1 recovery row).
    receipt = await submit(
      sudo(send(destination, message)),
      alice,
      "NOTE(B7): Asset Hub sudo v5 ClaimAsset dispatch is not available/authorized in the generated paseo-local runtime",
    );
  } catch (error) {
    throw new Error(String(error));
  }
  const sent = eventOf(receipt, "polkadotXcm", "Sent");
  if (!sent || !isHereOrigin(sent.data[0])) {
    throw new Error(
      "NOTE(B7): Asset Hub sudo send did not produce the bare chain origin required to recover an AH-keyed trap",
    );
  }
  assertV5(sent.data[1], "recovery Sent.destination");
  assertV5(sent.data[2], "recovery Sent.message");
  const messageId = sent.data[3].toHex();
  const destinationResult = await waitForProcessed(bleavit, messageId, start);
  if (!destinationResult.processed.data[destinationResult.processed.data.length - 1].isTrue) {
    throw new Error("NOTE(B7): inbound AH chain-origin ClaimAsset was not accepted by Bleavit");
  }
  const claimed = eventOf(destinationResult.events, "polkadotXcm", "AssetsClaimed");
  const deposited = eventOf(destinationResult.events, "foreignAssets", "Deposited");
  if (!claimed || !deposited) {
    throw new Error("NOTE(B7): recovery block lacks correlated AssetsClaimed/Deposited events");
  }
  if (claimed.data[0].toHex() !== state.trap.hash
      || text(claimed.data[1]) !== JSON.stringify(state.trap.origin)
      || text(claimed.data[2]) !== JSON.stringify(state.trap.assets)) {
    throw new Error("AssetsClaimed does not match the exact trap hash/origin/assets");
  }
  if (!scalarOccurs(json(deposited.data[1]), alice.address)) {
    throw new Error("recovered ForeignAssets.Deposited beneficiary is not Alice");
  }
  return messageId;
}

async function run(nodeName, networkInfo, args) {
  const api = await connect(networkInfo, nodeName);
  await zombie.util.cryptoWaitReady();
  const keyring = new zombie.Keyring({ type: "sr25519", ss58Format: api.registry.chainSS58 });
  const alice = keyring.addFromUri("//Alice");
  const leg = args[0];
  if (leg === "setup") return setup(api, alice, networkInfo);
  if (leg === "success") return transfer(api, alice, networkInfo, false);
  if (leg === "failure") return transfer(api, alice, networkInfo, true);
  if (leg === "trap") return trap(networkInfo);
  if (leg === "recovery") return recovery(api, alice, networkInfo);
  throw new Error(`unknown XCM drill leg '${leg}'`);
}

module.exports = { run };
