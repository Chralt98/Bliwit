// 15 §4.7; 09 §7.1; 05 §4.8; 13 §2; SQ-282 — wait for the relay BEST to advance
// N blocks while the Bleavit parachain is stalled (all collators paused).
//
// The on-chain dead-man observes `PersistedValidationData.relay_parent_number`
// (the relay best/anchor — the only relay signal on stable2606; the GRANDPA
// finalized head is not runtime-observable, SQ-282). It engages when the
// relay parent jumps by >= DEAD_MAN_RELAY_BLOCKS between two consecutive
// parachain blocks. Pausing every collator stalls the parachain while the relay
// keeps producing, so once the relay best has advanced N (> the kernel
// threshold) blocks past the pause baseline, resuming the collators forces the
// first catch-up block to carry a relay_parent_gap >= threshold. This drives the
// block-production/inclusion-stall trigger the runtime can actually see — not a
// relay finality stall (which is off-chain per 05 §4.8 / 14 TH-37).
async function run(_nodeName, networkInfo, args) {
  const relay = networkInfo.nodesByName["relay-alice"];
  if (!relay) throw new Error("relay-alice is absent from the drill topology");
  const api = await zombie.connect(relay.wsUri, relay.userDefinedTypes);
  const requiredAdvance = Number(args[0] || 48);
  // Baseline is captured after the collators are paused (this script runs after
  // the pause steps), so the parachain's last anchored relay parent is <= this.
  const baseline = (await api.rpc.chain.getHeader()).number.toNumber();
  const deadline = Date.now() + 1_500_000;

  while (Date.now() < deadline) {
    const best = (await api.rpc.chain.getHeader()).number.toNumber();
    if (best - baseline >= requiredAdvance) {
      return { baseline, best, advanced: best - baseline };
    }
    await new Promise((resolve) => setTimeout(resolve, 6_000));
  }
  throw new Error(`relay best advanced fewer than ${requiredAdvance} blocks from ${baseline}`);
}

module.exports = { run };
