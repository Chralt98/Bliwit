// 15 §4.7; 09 §7.1; 13 §1 — epoch-progression assertion.
//
// Reads the LIVE epoch length from chain (`epoch.schedule().length`) instead of
// hardcoding a cadence, so it asserts identically against the release runtime
// (302,400-block epochs) and the SQ-128 default-off `fast-timing` test runtime
// (compressed 84-block epochs). The height check is derived from that live
// length; the on-chain `epoch.epochOf().index` is the authoritative counter.
async function run(nodeName, networkInfo, args) {
  const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
  const api = await zombie.connect(wsUri, userDefinedTypes);
  if (!api.query.epoch?.epochOf || !api.query.epoch?.schedule) {
    throw new Error("NOTE(B7): A8 Epoch runtime wiring is required");
  }
  const minimum = Number(args[0]);
  // `EpochSchedule.length` is a struct field literally named `length`, which
  // collides with the Map/Array `.length` accessor on the codec object — read it
  // through toJSON() so we get the field value, not the container size.
  const schedule = (await api.query.epoch.schedule()).toJSON();
  const epochLength = Number(schedule.length);
  if (!Number.isFinite(epochLength) || epochLength <= 0) {
    throw new Error(
      `epoch.schedule().length is not a positive block count: ${JSON.stringify(schedule)}`,
    );
  }
  const expectedBlocks = minimum * epochLength;
  const height = (await api.rpc.chain.getHeader()).number.toNumber();
  if (height < expectedBlocks) {
    throw new Error(
      `epoch soak reached only ${height} blocks; ${minimum} epochs of length ${epochLength} require ${expectedBlocks}`,
    );
  }
  const epoch = await api.query.epoch.epochOf();
  const index = epoch.index.toNumber();
  if (index < minimum) {
    throw new Error(`epoch index ${index} is below required ${minimum}`);
  }
  return index;
}

module.exports = { run };
