use execution_guard_core::{hash_payload, CallDomain as GuardDomain, DispatchCall, Payload};
use futarchy_primitives::currency::USDC;
use origins_core::{BoxedCall, CallDomain, Origin, RuntimeCall};
use parity_scale_codec::Encode;
use std::{fs, path::Path};

fn boxed(call: RuntimeCall) -> BoxedCall {
    BoxedCall::new(call)
}

fn dispatch(domain: GuardDomain, call: RuntimeCall, encoded_len: u32) -> DispatchCall {
    DispatchCall {
        domain,
        encoded_len,
        declared_weight: 1,
        call,
        succeeds: true,
        error: [0; 4],
        upgrade_hash: None,
        target_spec_version: None,
    }
}

fn payload(calls: Vec<DispatchCall>) -> Payload {
    Payload {
        hash: hash_payload(&calls),
        calls,
    }
}

fn write(path: impl AsRef<Path>, bytes: &[u8]) {
    fs::write(path, bytes).expect("write committed fuzz seed");
}

fn selector(b_usdc: u64, amount_usdc: u64) -> u16 {
    let b = u128::from(b_usdc) * USDC;
    let amount = u128::from(amount_usdc) * USDC;
    let value =
        (amount - market_core::MIN_TRADE) * u128::from(u16::MAX) / (b / 4 - market_core::MIN_TRADE);
    u16::try_from(value).expect("selector fits")
}

fn trade_seed(b_usdc: u64, round_trip_usdc: u64, ops: &[(bool, bool, u16)]) -> Vec<u8> {
    let mut bytes = (b_usdc - 100).to_le_bytes().to_vec();
    bytes.extend(selector(b_usdc, round_trip_usdc).to_le_bytes());
    for (buy, long, amount_selector) in ops {
        bytes.push(u8::from(!buy));
        bytes.push(u8::from(!long));
        bytes.extend(amount_selector.to_le_bytes());
    }
    bytes
}

/// The harness derives the book kind from the high bits of the first trade
/// record's buy-flag byte (`TradeCase::kind_from_seed`: `sel = data[10] >> 1`),
/// so OR the chosen `sel` in there without disturbing the buy/sell bit. `sel`
/// values: `0` Decision/Accept, `2` Gate/Accept/Survival, `14`
/// Gate/Reject/Security, `3` Baseline.
fn trade_seed_kind(
    sel: u8,
    b_usdc: u64,
    round_trip_usdc: u64,
    ops: &[(bool, bool, u16)],
) -> Vec<u8> {
    let mut bytes = trade_seed(b_usdc, round_trip_usdc, ops);
    if bytes.len() > 10 {
        bytes[10] |= sel << 1;
    }
    bytes
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus");
    for target in [
        "payload_scale_decode",
        "nested_wrapper_filter",
        "lmsr_trade_paths",
    ] {
        fs::create_dir_all(root.join(target)).expect("create corpus directory");
    }

    let valid = payload(vec![dispatch(
        GuardDomain::Param,
        RuntimeCall::Leaf(CallDomain::Param),
        8,
    )]);
    write(
        root.join("payload_scale_decode/valid_param.scale"),
        &valid.encode(),
    );

    let max_calls = payload(
        (0..16)
            .map(|_| {
                dispatch(
                    GuardDomain::Param,
                    RuntimeCall::Leaf(CallDomain::Param),
                    4096,
                )
            })
            .collect(),
    );
    write(
        root.join("payload_scale_decode/max_16_calls_64k.scale"),
        &max_calls.encode(),
    );

    let too_many = payload(
        (0..17)
            .map(|_| {
                dispatch(
                    GuardDomain::Public,
                    RuntimeCall::Leaf(CallDomain::Public),
                    1,
                )
            })
            .collect(),
    );
    write(
        root.join("payload_scale_decode/reject_17_calls.scale"),
        &too_many.encode(),
    );

    let overflow = payload(vec![
        dispatch(
            GuardDomain::Public,
            RuntimeCall::Leaf(CallDomain::Public),
            u32::MAX,
        ),
        dispatch(
            GuardDomain::Public,
            RuntimeCall::Leaf(CallDomain::Public),
            1,
        ),
    ]);
    write(
        root.join("payload_scale_decode/reject_length_overflow.scale"),
        &overflow.encode(),
    );

    let proxy_announced =
        RuntimeCall::ProxyAnnounced(boxed(RuntimeCall::UtilityBatch(vec![RuntimeCall::Leaf(
            CallDomain::Param,
        )])));
    write(
        root.join("nested_wrapper_filter/proxy_announced_param.scale"),
        &proxy_announced.encode(),
    );

    let threshold_one = RuntimeCall::MultisigAsMultiThreshold1(boxed(
        RuntimeCall::UtilityWithWeight(boxed(RuntimeCall::Leaf(CallDomain::Code))),
    ));
    write(
        root.join("nested_wrapper_filter/as_multi_threshold_1_code.scale"),
        &threshold_one.encode(),
    );

    let shared_budget = RuntimeCall::UtilityBatch(vec![
        RuntimeCall::UtilityBatch(
            (0..9)
                .map(|_| RuntimeCall::Leaf(CallDomain::Public))
                .collect(),
        ),
        RuntimeCall::UtilityBatchAll(
            (0..9)
                .map(|_| RuntimeCall::Leaf(CallDomain::Public))
                .collect(),
        ),
    ]);
    write(
        root.join("nested_wrapper_filter/shared_budget_20_calls.scale"),
        &shared_budget.encode(),
    );

    let scheduler = RuntimeCall::Scheduler {
        origin: Origin::OracleResolution,
        call: boxed(RuntimeCall::Leaf(CallDomain::OracleResolution)),
    };
    write(
        root.join("nested_wrapper_filter/scheduler_oracle.scale"),
        &scheduler.encode(),
    );

    let mut too_deep = RuntimeCall::Leaf(CallDomain::Public);
    for _ in 0..5 {
        too_deep = RuntimeCall::Sudo(boxed(too_deep));
    }
    write(
        root.join("nested_wrapper_filter/reject_depth_5.scale"),
        &too_deep.encode(),
    );

    let v1 = selector(10_000, 1_000);
    write(
        root.join("lmsr_trade_paths/v1_v2_v5_round_trip.seed"),
        &trade_seed(10_000, 1_000, &[(true, true, v1), (false, true, u16::MAX)]),
    );

    let half_v3 = selector(10_000, 2_027);
    write(
        root.join("lmsr_trade_paths/v3_displacement.seed"),
        &trade_seed(
            10_000,
            1_000,
            &[(true, true, half_v3), (true, true, half_v3)],
        ),
    );

    let v4_walk = vec![(true, true, u16::MAX); 24];
    write(
        root.join("lmsr_trade_paths/v4_loss_walk.seed"),
        &trade_seed(10_000, 1_000, &v4_walk),
    );

    // Finding D: committed Gate and Baseline sequences (both sides, buy then
    // sell) so the corpus — not only the `Arbitrary` path — drives
    // `buy_gate`/`sell_gate`/`buy_baseline`/`sell_baseline`.
    let two_sided = [
        (true, true, v1),
        (true, false, v1),
        (false, true, u16::MAX),
        (false, false, u16::MAX),
    ];
    write(
        root.join("lmsr_trade_paths/gate_reject_security.seed"),
        &trade_seed_kind(14, 10_000, 1_000, &two_sided),
    );
    write(
        root.join("lmsr_trade_paths/baseline_two_sided.seed"),
        &trade_seed_kind(3, 10_000, 1_000, &two_sided),
    );
}
