// 15 §4.7; 02 §7.3; 09 §1.2(10); 13 §2; 05 §4.8; SQ-282 — dead-man state assertion.
//
// Polls (rather than one-shot reads) because the on-chain dead-man engages only
// once the parachain produces its first catch-up block after the outage — the
// relay_parent_gap (>= DEAD_MAN_RELAY_BLOCKS) is observed at that block, a few
// seconds after `resume`, and the latch then persists until the recovery-epoch
// boundary. A one-shot read immediately after `resume` races ahead of the first
// block. On timeout it reports the live detector state for diagnosis.
async function run(nodeName, networkInfo, args) {
  const { wsUri, userDefinedTypes } = networkInfo.nodesByName[nodeName];
  const api = await zombie.connect(wsUri, userDefinedTypes);
  const values = args.length === 1 ? String(args[0]).split(",") : args;
  const expected = values[0] || "engaged";
  // Optional poll-window seconds (recovery needs a longer window than
  // engagement); kept below the drill step's `within` so this reports its own
  // diagnosis instead of being killed by zombienet.
  const windowSeconds = Number(values[1] || 240);
  if (expected !== "engaged" && expected !== "cleared") {
    throw new Error(`unknown dead-man assertion '${expected}'`);
  }
  // Both engagement and post-recovery freeze-clear checks are A8/A11-gated.
  if (!api.query.epoch || !api.query.executionGuard) {
    throw new Error("NOTE(B7): A8/A11 runtime wiring is required for dead-man freeze assertions");
  }

  // Durable-signal assertion. The acute freeze (`DeadMan.pausedAt` +
  // `executionGuard.deadManFreeze`) is transient in the compressed drill: the
  // rapid post-outage catch-up clears the detector cause within a block or two,
  // lifting the freeze while the chain runs its proposal-free recovery epoch.
  // The DURABLE evidence the dead-man engaged is PhaseFlags bit 6 (02 §7.3
  // "dead-man engaged") plus the detector's latched `incident_active`, both of
  // which persist across the recovery epoch. The acute queue-freeze / clock-pause
  // effects are covered deterministically by the pallet unit test
  // `dead_man_pauses_phase_and_rejects_submission` (a synthetic >= threshold gap).
  const deadline = Date.now() + windowSeconds * 1_000;
  let snapshot = "";
  while (Date.now() < deadline) {
    const flags = (await api.query.constitution.phaseFlags()).toNumber();
    const deadMan = await api.query.epoch.deadMan();
    const queueFrozen = await api.query.executionGuard.deadManFreeze();
    const detector = (await api.query.epoch.deadManDetector()).toJSON() ?? {};
    const engaged = (flags & (1 << 6)) !== 0;
    const incident = detector.incidentActive === true;
    const paused = deadMan.pausedAt.isSome;
    const frozen = queueFrozen.isTrue;
    snapshot = `flags6=${engaged} incidentActive=${incident} pausedAt=${paused} queueFrozen=${frozen}`;
    if (expected === "engaged" && engaged && incident) return flags;
    if (expected === "cleared" && !engaged && !incident && !paused && !frozen) return flags;
    await new Promise((resolve) => setTimeout(resolve, 6_000));
  }

  // Diagnosis: surface the detector inputs so a non-firing catch-up is visible.
  let detail = snapshot;
  try {
    const detector = (await api.query.epoch.deadManDetector()).toJSON();
    const lastRelayParent = (await api.query.epoch.lastRelayParent()).toJSON();
    detail += ` detector=${JSON.stringify(detector)} lastRelayParent=${JSON.stringify(lastRelayParent)}`;
  } catch (error) {
    detail += ` (detector introspection failed: ${error})`;
  }
  throw new Error(`dead-man '${expected}' not observed within poll window (${detail})`);
}

module.exports = { run };
