//! Runtime-level composition, cross-pallet wiring and safety-filter regression suite.

#![allow(clippy::assertions_on_constants, clippy::manual_unwrap_or_default)]

use alloc::{boxed::Box, vec, vec::Vec};

use bleavit_xcm::{
    caps::InflowCaps as XcmInflowCaps,
    identity::usdc_location,
    trader::{GovernedWeightTrader, TraderRates, WeightRate},
};
use frame_support::{
    assert_noop, assert_ok,
    dispatch::{DispatchClass, GetDispatchInfo},
    traits::{
        fungible::{Inspect as FungibleInspect, InspectHold},
        fungibles::{Inspect as FungiblesInspect, Mutate as FungiblesMutate},
        tokens::ConversionToAssetBalance,
        ConstU32, Contains, EnsureOrigin, Get, Hooks, OriginTrait, PalletInfo, PalletsInfoAccess,
        QueryPreimage, StorePreimage, VestingSchedule,
    },
    weights::Weight,
    BoundedVec,
};
use futarchy_primitives::{
    chain_identity, currency, kernel, DecisionOutcome, MarketSet, Proposal, ProposalClass,
    ProposalState, RatificationStatus, RejectReason, RuntimeVersionConstraint,
};
use origins_core::Origin as ClassOrigin;
use parity_scale_codec::{Compact, Decode, Encode};
use sp_core::H256;
use sp_genesis_builder::PresetId;
use sp_inherents::InherentData;
use sp_keyring::Sr25519Keyring;
use sp_runtime::{
    generic::{Era, SignedPayload},
    traits::{Block as BlockT, Dispatchable, Header as HeaderT},
    transaction_validity::{InvalidTransaction, TransactionValidityError},
    BuildStorage, DispatchError, MultiAddress, MultiSignature,
};
use staging_xcm::latest::{
    Asset as XcmAsset, AssetId as XcmAssetId, Fungibility, Weight as XcmWeight, XcmContext,
};
use staging_xcm::{IdentifyVersion, VersionedAssets, VersionedLocation};
use staging_xcm_executor::{
    test_helpers::mock_asset_to_holding, traits::WeightTrader, AssetsInHolding,
};

use crate::{
    classifier::{RuntimeBaseCallFilter, RuntimeDispatcher},
    AccountId, AllPalletsWithSystem, AssetTxPayment, Attestor, Aura, AuraExt, Authorship, Balance,
    Balances, BlockNumber, CollatorSelection, ConditionalLedger, Constitution, ConvictionVoting,
    CumulusXcm, Epoch, ExecutionGuard, ForeignAssets, FutarchyTreasury, Guardian, IncidentRegistry,
    InflowCaps, Market, MessageQueue, Migrations, MilestoneRegistry, Multisig, Oracle, Origins,
    PalletInfo as RuntimePalletInfo, ParachainInfo, ParachainSystem, PolkadotXcm, Preimage, Proxy,
    Referenda, Runtime, RuntimeCall, RuntimeGenesisConfig, RuntimeOrigin, Scheduler, Session, Sudo,
    System, Timestamp, TrackOrigins, TransactionPayment, TxExtension, UncheckedExtrinsic, Utility,
    Vesting, Welfare, XcmpQueue, FEE_VIT_USDC_RATE_KEY, MILLISECS_PER_BLOCK, SS58_PREFIX,
    USDC_DECIMALS, USDC_LOCATION_ENCODED, VERSION, VIT_DECIMALS,
};

trait SameType<Rhs> {}
impl<T> SameType<T> for T {}

fn assert_same_type<Left, Right>()
where
    Left: SameType<Right>,
{
}

pub(crate) fn account(seed: u8) -> AccountId {
    AccountId::new([seed; 32])
}

fn xcm_holding_amount(holding: &AssetsInHolding, id: &staging_xcm::latest::Location) -> u128 {
    holding
        .fungible_assets_iter()
        .find_map(|asset| match asset {
            XcmAsset {
                id: XcmAssetId(location),
                fun: Fungibility::Fungible(amount),
            } if &location == id => Some(amount),
            _ => None,
        })
        .unwrap_or_default()
}

fn merge_json(base: &mut serde_json::Value, patch: serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(base), serde_json::Value::Object(patch)) => {
            for (key, value) in patch {
                match base.get_mut(&key) {
                    Some(slot) => merge_json(slot, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, patch) => *base = patch,
    }
}

pub(crate) fn development_ext() -> sp_io::TestExternalities {
    let preset =
        match crate::genesis::get_preset(&PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET)) {
            Some(bytes) => bytes,
            None => Vec::new(),
        };
    assert!(!preset.is_empty());
    let mut merged = match serde_json::to_value(RuntimeGenesisConfig::default()) {
        Ok(value) => value,
        Err(error) => {
            assert!(false, "default genesis must encode: {error}");
            serde_json::Value::Null
        }
    };
    let patch = match serde_json::from_slice::<serde_json::Value>(&preset) {
        Ok(value) => value,
        Err(error) => {
            assert!(false, "development preset patch must decode: {error}");
            serde_json::Value::Null
        }
    };
    merge_json(&mut merged, patch);
    let config = match serde_json::from_value::<RuntimeGenesisConfig>(merged) {
        Ok(config) => config,
        Err(error) => {
            assert!(false, "development preset must decode: {error}");
            RuntimeGenesisConfig::default()
        }
    };
    let storage = match config.build_storage() {
        Ok(storage) => storage,
        Err(error) => {
            assert!(false, "development preset must build: {error}");
            Default::default()
        }
    };
    sp_io::TestExternalities::new(storage)
}

struct CandidateRuntimeVersion(Vec<u8>);

impl sp_core::traits::ReadRuntimeVersion for CandidateRuntimeVersion {
    fn read_runtime_version(
        &self,
        _: &[u8],
        _: &mut dyn sp_externalities::Externalities,
    ) -> Result<Vec<u8>, String> {
        Ok(self.0.clone())
    }
}

pub(crate) fn upgrade_ext() -> sp_io::TestExternalities {
    let mut version = VERSION;
    version.spec_version = version.spec_version.saturating_add(1);
    let mut ext = development_ext();
    ext.register_extension(sp_core::traits::ReadRuntimeVersionExt::new(
        CandidateRuntimeVersion(version.encode()),
    ));
    ext
}

fn release_channel_raw() -> Option<Vec<u8>> {
    let mut key = sp_io::hashing::twox_128(b"Constitution").to_vec();
    key.extend_from_slice(&sp_io::hashing::twox_128(b"ReleaseChannel"));
    sp_io::storage::get(&key).map(|bytes| bytes.to_vec())
}

fn raw_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let source = bytes.get(offset..offset.checked_add(4)?)?;
    let mut encoded = [0u8; 4];
    encoded.copy_from_slice(source);
    Some(u32::from_le_bytes(encoded))
}

fn assert_raw_unchanged_outside(before: &[u8], after: &[u8], owned: &[core::ops::Range<usize>]) {
    assert_eq!(before.len(), after.len());
    for (index, (before, after)) in before.iter().zip(after).enumerate() {
        if !owned.iter().any(|range| range.contains(&index)) {
            assert_eq!(before, after, "unexpected ReleaseChannel write at {index}");
        }
    }
}

fn seed_queued_epoch_proposal(
    pid: futarchy_primitives::ProposalId,
    class: ProposalClass,
    payload_hash: H256,
    payload_len: u32,
    maturity: BlockNumber,
    grace_end: BlockNumber,
    version_constraint: RuntimeVersionConstraint,
) -> Result<(), DispatchError> {
    Epoch::tick(RuntimeOrigin::signed(account(69)), Default::default())?;
    let epoch = pallet_epoch::EpochOf::<Runtime>::get().index;
    let epoch_schedule = pallet_epoch::Schedule::<Runtime>::get();
    let first_market = pid.saturating_mul(10);
    let gates = matches!(class, ProposalClass::Code | ProposalClass::Meta).then_some([
        first_market.saturating_add(3),
        first_market.saturating_add(4),
        first_market.saturating_add(5),
        first_market.saturating_add(6),
    ]);
    let proposal = Proposal {
        id: pid,
        proposer: account(70),
        class,
        state: ProposalState::Queued,
        epoch,
        submitted_at: System::block_number(),
        payload_hash: payload_hash.0,
        payload_len,
        ask: 0,
        bond: 0,
        resources: Default::default(),
        metric_spec: 1,
        decide_at: System::block_number(),
        rerun: false,
        extended: false,
        delayed_once: false,
        markets: Some(MarketSet {
            accept: first_market.saturating_add(1),
            reject: first_market.saturating_add(2),
            gates,
            baseline: 9_000u64.saturating_add(epoch.into()),
        }),
        maturity: Some(maturity),
        grace_end: Some(grace_end),
        version_constraint: Some(version_constraint),
        decision: Some(DecisionOutcome::Adopt),
    };
    pallet_epoch::Proposals::<Runtime>::insert(pid, proposal);
    pallet_epoch::ProposalSchedules::<Runtime>::insert(
        pid,
        pallet_epoch::ProposalSchedule {
            epoch,
            epoch_start_block: epoch_schedule.epoch_start_block,
            epoch_length: epoch_schedule.length,
            decide_at: System::block_number(),
            metric_spec: 1,
        },
    );
    pallet_epoch::NextProposalId::<Runtime>::mutate(|next| {
        *next = (*next).max(pid.saturating_add(1));
    });
    pallet_conditional_ledger::Vaults::<Runtime>::insert(
        pid,
        pallet_conditional_ledger::core_ledger::VaultInfo::open(1),
    );
    Ok(())
}

fn seed_decision_grade_market(
    id: futarchy_primitives::MarketId,
    kind: pallet_market::core_market::BookKind,
    quote: futarchy_primitives::FixedU64,
    end: BlockNumber,
    windows: (BlockNumber, BlockNumber),
    b: Balance,
    contest: Balance,
) -> Result<(), DispatchError> {
    let owner = match kind {
        pallet_market::core_market::BookKind::Decision { proposal, .. }
        | pallet_market::core_market::BookKind::Gate { proposal, .. } => proposal,
        pallet_market::core_market::BookKind::Baseline { .. } => {
            pallet_market::DecisionWindowOwners::<Runtime>::iter_values()
                .flat_map(|owners| owners.into_iter())
                .find_map(|record| (record.3 == end).then_some(record.0))
                .ok_or(DispatchError::Other("baseline window owner"))?
        }
    };
    let (window, trailing) = windows;
    let start = end
        .checked_sub(window)
        .ok_or(DispatchError::Other("window"))?;
    let trailing_start = end
        .checked_sub(trailing)
        .ok_or(DispatchError::Other("trailing window"))?;
    let mut book =
        pallet_market::core_market::MarketBook::open(id, kind, account(80), account(81), b);
    book.q_long = contest / 2;
    book.q_short = contest.saturating_sub(book.q_long);
    book.last_quote_1e9 = quote;
    book.last_observation_1e9 = quote;
    book.last_observed_block = u64::from(end);
    book.cumulative_price_blocks = u128::from(quote.0)
        .checked_mul(u128::from(window))
        .ok_or(DispatchError::Other("twap accumulator"))?
        .into();
    pallet_market::Markets::<Runtime>::insert(id, book);
    pallet_market::SeededMarkets::<Runtime>::insert(id, ());
    let interval = u32::try_from(crate::configs::MarketObsInterval::get())
        .map_err(|_| DispatchError::Other("observation interval"))?;
    let observations = window
        .checked_div(interval)
        .ok_or(DispatchError::Other("observation coverage"))?;
    let contest_capital_blocks = contest
        .checked_mul(Balance::from(window))
        .ok_or(DispatchError::Other("contest accumulator"))?;
    let windows =
        frame_support::BoundedVec::<_, frame_support::traits::ConstU32<8>>::try_from(vec![
            pallet_market::core_market::TwapWindow {
                start,
                trailing_start,
                end,
                observations,
                stale_events: 0,
                contest_capital_blocks,
                contest_accrued_until: end,
                contest_valid: true,
                close_spot: Some(quote),
                sealed: true,
            },
        ])
        .map_err(|_| DispatchError::Other("window bound"))?;
    pallet_market::DecisionWindows::<Runtime>::insert(id, windows);
    pallet_market::DecisionWindowOwners::<Runtime>::insert(
        id,
        frame_support::BoundedVec::truncate_from(vec![(owner, start, trailing_start, end)]),
    );
    let cumulative_at = |at: BlockNumber| {
        at.checked_sub(start)
            .and_then(|elapsed| u128::from(quote.0).checked_mul(u128::from(elapsed)))
            .map(pallet_market::core_market::TwapCumulative::from)
    };
    let checkpoints =
        frame_support::BoundedVec::<_, frame_support::traits::ConstU32<8>>::try_from(vec![
            (start, pallet_market::core_market::TwapCumulative::ZERO),
            (
                trailing_start,
                cumulative_at(trailing_start)
                    .ok_or(DispatchError::Other("trailing accumulator"))?,
            ),
            (
                end,
                cumulative_at(end).ok_or(DispatchError::Other("end accumulator"))?,
            ),
        ])
        .map_err(|_| DispatchError::Other("checkpoint bound"))?;
    pallet_market::TwapCheckpoints::<Runtime>::insert(id, checkpoints);
    Ok(())
}

fn seed_code_decision_markets(
    pid: futarchy_primitives::ProposalId,
    end: BlockNumber,
    accept_quote: futarchy_primitives::FixedU64,
    reject_quote: futarchy_primitives::FixedU64,
) -> Result<MarketSet, DispatchError> {
    let proposal = pallet_epoch::Proposals::<Runtime>::get(pid)
        .ok_or(DispatchError::Other("CODE proposal missing"))?;
    let markets = proposal
        .markets
        .ok_or(DispatchError::Other("CODE market set missing"))?;
    let gates = markets
        .gates
        .ok_or(DispatchError::Other("CODE gate set missing"))?;
    let params = <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
    let index = crate::configs::proposal_class_index(ProposalClass::Code);
    let decision_contest = params.v_min[index];
    let gate_contest = params.gate_v_min[index];
    let decision_b = crate::configs::class_pol_floor(ProposalClass::Code);
    let gate_b = crate::configs::balance_param(b"pol.b_gate");
    let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
    let neutral = futarchy_primitives::FixedU64(500_000_000);
    for (id, kind, quote, b, contest) in [
        (
            markets.accept,
            pallet_market::core_market::BookKind::Decision {
                proposal: pid,
                branch: futarchy_primitives::Branch::Accept,
            },
            accept_quote,
            decision_b,
            decision_contest,
        ),
        (
            markets.reject,
            pallet_market::core_market::BookKind::Decision {
                proposal: pid,
                branch: futarchy_primitives::Branch::Reject,
            },
            reject_quote,
            decision_b,
            decision_contest,
        ),
        (
            gates[0],
            pallet_market::core_market::BookKind::Gate {
                proposal: pid,
                branch: futarchy_primitives::Branch::Accept,
                gate: futarchy_primitives::GateType::Survival,
            },
            neutral,
            gate_b,
            gate_contest,
        ),
        (
            gates[1],
            pallet_market::core_market::BookKind::Gate {
                proposal: pid,
                branch: futarchy_primitives::Branch::Reject,
                gate: futarchy_primitives::GateType::Survival,
            },
            neutral,
            gate_b,
            gate_contest,
        ),
        (
            gates[2],
            pallet_market::core_market::BookKind::Gate {
                proposal: pid,
                branch: futarchy_primitives::Branch::Accept,
                gate: futarchy_primitives::GateType::Security,
            },
            neutral,
            gate_b,
            gate_contest,
        ),
        (
            gates[3],
            pallet_market::core_market::BookKind::Gate {
                proposal: pid,
                branch: futarchy_primitives::Branch::Reject,
                gate: futarchy_primitives::GateType::Security,
            },
            neutral,
            gate_b,
            gate_contest,
        ),
        (
            markets.baseline,
            pallet_market::core_market::BookKind::Baseline {
                epoch: proposal.epoch,
            },
            neutral,
            baseline_b,
            decision_contest,
        ),
    ] {
        seed_decision_grade_market(
            id,
            kind,
            quote,
            end,
            (params.decision_window, params.trailing_window),
            b,
            contest,
        )?;
    }
    pallet_market::BaselineMarketOf::<Runtime>::insert(proposal.epoch, markets.baseline);
    Ok(markets)
}

fn assert_guard_ownership_cleared(pid: futarchy_primitives::ProposalId, payload_hash: H256) {
    assert_eq!(
        preimage_request_count(payload_hash),
        0,
        "proposal {pid} retained a preimage request"
    );
    assert!(!pallet_execution_guard::Queue::<Runtime>::contains_key(pid));
    assert!(!pallet_execution_guard::RerunPins::<Runtime>::contains_key(
        pid
    ));
    assert!(!pallet_execution_guard::Ratifications::<Runtime>::contains_key(pid));
    assert!(!pallet_execution_guard::AttestationBindings::<Runtime>::contains_key(pid));
    assert!(ExecutionGuard::do_try_state().is_ok());
    assert!(Epoch::do_try_state().is_ok());
}

#[allow(clippy::too_many_arguments)]
fn seed_two_window_baseline(
    id: futarchy_primitives::MarketId,
    epoch: futarchy_primitives::EpochId,
    early_owner: futarchy_primitives::ProposalId,
    late_owner: futarchy_primitives::ProposalId,
    early_end: BlockNumber,
    late_end: BlockNumber,
    window: BlockNumber,
    trailing: BlockNumber,
    early_quote: futarchy_primitives::FixedU64,
    late_quote: futarchy_primitives::FixedU64,
    b: Balance,
    contest: Balance,
) -> Result<(), DispatchError> {
    let early_start = early_end
        .checked_sub(window)
        .ok_or(DispatchError::Other("early baseline start"))?;
    let late_start = late_end
        .checked_sub(window)
        .ok_or(DispatchError::Other("late baseline start"))?;
    let early_trailing = early_end
        .checked_sub(trailing)
        .ok_or(DispatchError::Other("early baseline trailing"))?;
    let late_trailing = late_end
        .checked_sub(trailing)
        .ok_or(DispatchError::Other("late baseline trailing"))?;
    let early_total = u128::from(early_quote.0)
        .checked_mul(u128::from(window))
        .ok_or(DispatchError::Other("early baseline accumulator"))?;
    let cumulative = |elapsed: BlockNumber, quote: futarchy_primitives::FixedU64| {
        u128::from(quote.0).checked_mul(u128::from(elapsed))
    };
    let late_total = early_total
        .checked_add(
            cumulative(window, late_quote)
                .ok_or(DispatchError::Other("late baseline accumulator"))?,
        )
        .ok_or(DispatchError::Other("baseline accumulator"))?;
    let mut book = pallet_market::core_market::MarketBook::open(
        id,
        pallet_market::core_market::BookKind::Baseline { epoch },
        account(82),
        account(83),
        b,
    );
    book.q_long = contest / 2;
    book.q_short = contest.saturating_sub(book.q_long);
    book.last_quote_1e9 = late_quote;
    book.last_observation_1e9 = late_quote;
    book.last_observed_block = u64::from(late_end);
    book.cumulative_price_blocks = late_total.into();
    pallet_market::Markets::<Runtime>::insert(id, book);
    pallet_market::SeededMarkets::<Runtime>::insert(id, ());

    let interval = u32::try_from(crate::configs::MarketObsInterval::get())
        .map_err(|_| DispatchError::Other("observation interval"))?;
    let observations = window
        .checked_div(interval)
        .ok_or(DispatchError::Other("observation coverage"))?;
    let contest_blocks = contest
        .checked_mul(Balance::from(window))
        .ok_or(DispatchError::Other("contest accumulator"))?;
    let windows =
        frame_support::BoundedVec::<_, frame_support::traits::ConstU32<8>>::try_from(vec![
            pallet_market::core_market::TwapWindow {
                start: early_start,
                trailing_start: early_trailing,
                end: early_end,
                observations,
                stale_events: 0,
                contest_capital_blocks: contest_blocks,
                contest_accrued_until: early_end,
                contest_valid: true,
                close_spot: Some(early_quote),
                sealed: true,
            },
            pallet_market::core_market::TwapWindow {
                start: late_start,
                trailing_start: late_trailing,
                end: late_end,
                observations,
                stale_events: 0,
                contest_capital_blocks: contest_blocks,
                contest_accrued_until: late_end,
                contest_valid: true,
                close_spot: Some(late_quote),
                sealed: true,
            },
        ])
        .map_err(|_| DispatchError::Other("window bound"))?;
    pallet_market::DecisionWindows::<Runtime>::insert(id, windows);
    pallet_market::DecisionWindowOwners::<Runtime>::insert(
        id,
        frame_support::BoundedVec::truncate_from(vec![
            (early_owner, early_start, early_trailing, early_end),
            (late_owner, late_start, late_trailing, late_end),
        ]),
    );

    let checkpoints =
        frame_support::BoundedVec::<_, frame_support::traits::ConstU32<8>>::try_from(vec![
            (
                early_start,
                pallet_market::core_market::TwapCumulative::ZERO,
            ),
            (
                early_trailing,
                cumulative(early_trailing.saturating_sub(early_start), early_quote)
                    .ok_or(DispatchError::Other("early trailing accumulator"))?
                    .into(),
            ),
            (early_end, early_total.into()),
            (
                late_trailing,
                early_total
                    .checked_add(
                        cumulative(late_trailing.saturating_sub(late_start), late_quote)
                            .ok_or(DispatchError::Other("late trailing accumulator"))?,
                    )
                    .ok_or(DispatchError::Other("late trailing accumulator"))?
                    .into(),
            ),
            (late_end, late_total.into()),
        ])
        .map_err(|_| DispatchError::Other("checkpoint bound"))?;
    pallet_market::TwapCheckpoints::<Runtime>::insert(id, checkpoints);
    Ok(())
}

fn enqueue_attested_code_upgrade_pending_ratification(
    pid: futarchy_primitives::ProposalId,
    candidate: &[u8],
) -> Option<(BlockNumber, H256)> {
    let members = [account(90), account(91), account(92)];
    assert_ok!(Attestor::set_members(
        pallet_origins::Origin::ConstitutionalValues.into(),
        members.to_vec(),
    ));
    let artifact = H256::from(sp_io::hashing::blake2_256(candidate));
    for (member, statement) in members.iter().take(2).zip([101u8, 102u8]) {
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(member.clone()),
            pid,
            artifact.0,
            [statement; 32],
        ));
    }
    let first = pallet_attestor::Attestations::<Runtime>::get()
        .into_iter()
        .find(|record| record.pid == pid && record.artifact_hash == artifact.0)?;
    System::set_block_number(first.challenge_deadline.saturating_add(1));
    assert!(Attestor::has_quorum(pid, artifact.0));

    let call = RuntimeCall::System(frame_system::Call::authorize_upgrade {
        code_hash: artifact,
    });
    let batch =
        pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![call]).ok()?;
    let bytes = batch.encode();
    let payload_len = u32::try_from(bytes.len()).ok()?;
    let payload_hash = <Preimage as StorePreimage>::note(bytes.into()).ok()?;
    let now = System::block_number();
    let maturity = now.checked_add(
        <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_timelock(
            ProposalClass::Code,
        ),
    )?;
    let grace_end = maturity.checked_add(
        <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_grace(
            ProposalClass::Code,
        ),
    )?;
    let version_constraint = pallet_execution_guard::CurrentSpecName::<Runtime>::get()?;
    let declared_domains = pallet_execution_guard::pallet::StoredDomains::try_from(vec![
        pallet_execution_guard::CallDomain::InternalRootAuthorizeUpgrade,
    ])
    .ok()?;
    seed_queued_epoch_proposal(
        pid,
        ProposalClass::Code,
        payload_hash,
        payload_len,
        maturity,
        grace_end,
        version_constraint.clone(),
    )
    .ok()?;
    assert_ok!(ExecutionGuard::enqueue(
        RuntimeOrigin::signed(crate::configs::epoch_account()),
        pallet_execution_guard::pallet::StoredQueuedExecution {
            pid,
            payload_hash: payload_hash.0,
            payload_len,
            class: ProposalClass::Code,
            maturity,
            grace_end,
            version_constraint,
            meters_declared: Default::default(),
            // 06 §2.2 R-1: queue admission precedes the values referendum
            // in the ordinary flow. The later `ratify` call binds its index
            // into this already-live queue entry.
            ratify_ref: None,
            ratification_passed: false,
            attestation_id: Some(first.id),
            pre_upgrade_checkpoint: None,
            cancelled: false,
            declared_domains,
            failed_at: None,
        },
        false,
    ));
    Some((maturity, artifact))
}

fn enqueue_attested_code_upgrade(
    pid: futarchy_primitives::ProposalId,
    candidate: &[u8],
    referendum_index: u32,
) -> Option<(BlockNumber, H256)> {
    let setup = enqueue_attested_code_upgrade_pending_ratification(pid, candidate)?;
    assert_ok!(ExecutionGuard::ratify(
        pallet_origins::Origin::ConstitutionalValues.into(),
        pid,
        referendum_index,
    ));
    Some(setup)
}

fn preimage_request_count(hash: impl Into<H256>) -> u32 {
    match pallet_preimage::RequestStatusFor::<Runtime>::get(hash.into()) {
        Some(pallet_preimage::RequestStatus::Requested { count, .. }) => count,
        _ => 0,
    }
}

fn empty_param_proposal(
    id: futarchy_primitives::ProposalId,
    proposer: AccountId,
    payload_hash: H256,
    payload_len: u32,
) -> Proposal<AccountId> {
    Proposal {
        id,
        proposer,
        class: ProposalClass::Param,
        state: ProposalState::Submitted,
        epoch: pallet_epoch::CurrentEpoch::<Runtime>::get(),
        submitted_at: System::block_number(),
        payload_hash: payload_hash.0,
        payload_len,
        ask: 0,
        bond: crate::configs::balance_param(b"prop.bond.param"),
        resources: Default::default(),
        metric_spec: 0,
        decide_at: 0,
        rerun: false,
        extended: false,
        delayed_once: false,
        markets: None,
        maturity: None,
        grace_end: None,
        version_constraint: pallet_execution_guard::CurrentSpecName::<Runtime>::get(),
        decision: None,
    }
}

fn fund_param_market_lifecycles(decision_seed_count: u128) {
    let decision_b = crate::configs::balance_param(b"pol.b.param");
    let gate_b = crate::configs::balance_param(b"pol.b_gate");
    let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
    let decision_headroom =
        pallet_market::core_market::seed_headroom(decision_b).expect("bounded decision b");
    let gate_headroom = pallet_market::core_market::seed_headroom(gate_b).expect("bounded gate b");
    let baseline_headroom =
        pallet_market::core_market::seed_headroom(baseline_b).expect("bounded baseline b");
    assert_ok!(ForeignAssets::mint_into(
        usdc_location(),
        &crate::configs::pol_account(),
        decision_headroom
            .saturating_mul(decision_seed_count)
            .saturating_add(gate_headroom.saturating_mul(decision_seed_count.saturating_mul(2)),)
            .saturating_add(currency::USDC),
    ));
    assert_ok!(ForeignAssets::mint_into(
        usdc_location(),
        &crate::configs::pol_baseline_account(),
        baseline_headroom.saturating_add(currency::USDC),
    ));
    pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
        state.main_usdc = decision_headroom
            .saturating_mul(decision_seed_count.saturating_mul(4))
            .saturating_add(gate_headroom.saturating_mul(decision_seed_count.saturating_mul(8)))
            .saturating_add(baseline_headroom.saturating_mul(4));
    });
}

fn open_runtime_param_proposal(
    pid: futarchy_primitives::ProposalId,
    decide_at: BlockNumber,
) -> Result<Proposal<AccountId>, DispatchError> {
    use pallet_epoch::MarketAccess;

    let mut proposal = empty_param_proposal(pid, account(158), H256::zero(), 0);
    proposal.metric_spec = 1;
    proposal.state = ProposalState::Trading;
    proposal.decide_at = decide_at;
    let plan = <crate::configs::RuntimePolBudget as pallet_epoch::PolBudget<AccountId>>::proposal_seed_plan(&proposal)
        .ok_or(DispatchError::Other("PARAM seed plan"))?;
    let markets = <crate::configs::RuntimeMarketAccess as MarketAccess<AccountId>>::open_markets(
        &proposal,
        false,
        Some(plan),
    )?;
    proposal.markets = Some(markets);
    pallet_epoch::Proposals::<Runtime>::insert(pid, proposal.clone());
    let schedule = pallet_epoch::Schedule::<Runtime>::get();
    pallet_epoch::ProposalSchedules::<Runtime>::insert(
        pid,
        pallet_epoch::ProposalSchedule {
            epoch: proposal.epoch,
            epoch_start_block: schedule.epoch_start_block,
            epoch_length: schedule.length,
            decide_at,
            metric_spec: proposal.metric_spec,
        },
    );
    pallet_epoch::NextProposalId::<Runtime>::mutate(|next| {
        *next = (*next).max(pid.saturating_add(1));
    });
    Ok(proposal)
}

fn create_synthetic_markets_for_void(
    pid: futarchy_primitives::ProposalId,
) -> Result<(), DispatchError> {
    use futarchy_primitives::{Branch, GateType};
    use pallet_market::core_market::BookKind;

    let proposal = pallet_epoch::Proposals::<Runtime>::get(pid)
        .ok_or(DispatchError::Other("void fixture proposal"))?;
    let markets = proposal
        .markets
        .ok_or(DispatchError::Other("void fixture market set"))?;
    let decision_b = crate::configs::class_pol_floor(proposal.class);
    let gate_b = crate::configs::balance_param(b"pol.b_gate");
    let create = |id, kind, b| {
        if pallet_market::Markets::<Runtime>::contains_key(id) {
            return Ok(());
        }
        Market::create_market(
            RuntimeOrigin::signed(crate::configs::epoch_account()),
            id,
            kind,
            account(160),
            account(161),
            b,
        )
    };
    create(
        markets.accept,
        BookKind::Decision {
            proposal: pid,
            branch: Branch::Accept,
        },
        decision_b,
    )?;
    create(
        markets.reject,
        BookKind::Decision {
            proposal: pid,
            branch: Branch::Reject,
        },
        decision_b,
    )?;
    if let Some(gates) = markets.gates {
        for (id, branch, gate) in [
            (gates[0], Branch::Accept, GateType::Survival),
            (gates[1], Branch::Reject, GateType::Survival),
            (gates[2], Branch::Accept, GateType::Security),
            (gates[3], Branch::Reject, GateType::Security),
        ] {
            create(
                id,
                BookKind::Gate {
                    proposal: pid,
                    branch,
                    gate,
                },
                gate_b,
            )?;
        }
    }
    create(
        markets.baseline,
        BookKind::Baseline {
            epoch: proposal.epoch,
        },
        crate::configs::balance_param(b"pol.b_baseline"),
    )
}

fn install_single_active_metric_spec(
    version: futarchy_primitives::MetricSpecVersion,
) -> Option<()> {
    for (stored_version, _) in pallet_welfare::MetricSpecs::<Runtime>::iter() {
        pallet_welfare::MetricSpecs::<Runtime>::remove(stored_version);
    }
    let cadence_blocks = u32::try_from(crate::configs::MarketObsInterval::get()).ok()?;
    let active_spec = pallet_welfare::MetricSpec {
        id: 1,
        version,
        pillar: pallet_welfare::Pillar::S,
        weight: futarchy_primitives::FixedU64(pallet_welfare::ONE),
        epsilon_floor: pallet_welfare::EPSILON_PILLAR,
        activation_epoch: pallet_epoch::CurrentEpoch::<Runtime>::get(),
        source: pallet_welfare::SourceClass::Onchain,
        formula_ref: [1; 32],
        units: [2; 16],
        repr: [3; 16],
        cadence_blocks,
        sanity_min: futarchy_primitives::FixedU64(0),
        sanity_max: futarchy_primitives::FixedU64(pallet_welfare::ONE),
        has_normalization_rule: true,
        has_missing_data_rule: true,
        has_gaming_vectors: true,
        has_challenge_procedure: true,
        prior_bounds: [futarchy_primitives::FixedU64(pallet_welfare::ONE);
            pallet_welfare::HISTORY_PRIORS],
    };
    let active_specs = pallet_welfare::BoundedSpecSet::try_from(vec![active_spec]).ok()?;
    pallet_welfare::MetricSpecs::<Runtime>::insert(version, active_specs);
    Some(())
}

fn install_active_x_snapshot_spec(
    version: futarchy_primitives::MetricSpecVersion,
    activation_epoch: futarchy_primitives::EpochId,
) -> Option<()> {
    for (stored_version, _) in pallet_welfare::MetricSpecs::<Runtime>::iter() {
        pallet_welfare::MetricSpecs::<Runtime>::remove(stored_version);
    }
    pallet_welfare::SnapshotDeadline::<Runtime>::kill();
    let active_spec = pallet_welfare::MetricSpec {
        id: futarchy_primitives::metric_ids::X,
        version,
        pillar: pallet_welfare::Pillar::COnchain,
        weight: futarchy_primitives::FixedU64(pallet_welfare::ONE),
        epsilon_floor: pallet_welfare::EPSILON_PILLAR,
        activation_epoch,
        source: pallet_welfare::SourceClass::Onchain,
        formula_ref: [1; 32],
        units: [2; 16],
        repr: [3; 16],
        cadence_blocks: 1,
        sanity_min: futarchy_primitives::FixedU64(0),
        sanity_max: futarchy_primitives::FixedU64(pallet_welfare::ONE),
        has_normalization_rule: true,
        has_missing_data_rule: true,
        has_gaming_vectors: true,
        has_challenge_procedure: true,
        prior_bounds: [futarchy_primitives::FixedU64(pallet_welfare::ONE);
            pallet_welfare::HISTORY_PRIORS],
    };
    let active_specs = pallet_welfare::BoundedSpecSet::try_from(vec![active_spec]).ok()?;
    pallet_welfare::MetricSpecs::<Runtime>::insert(version, active_specs);
    Some(())
}

fn note_runtime_batch(calls: Vec<RuntimeCall>) -> Option<(H256, u32)> {
    let batch = pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(calls).ok()?;
    let bytes = batch.encode();
    let payload_len = u32::try_from(bytes.len()).ok()?;
    let payload_hash = <Preimage as StorePreimage>::note(bytes.into()).ok()?;
    Some((payload_hash, payload_len))
}

fn expected_resource_key(tag: u8, discriminator: Option<&[u8]>) -> futarchy_primitives::ResourceId {
    let mut key = [0_u8; 8];
    key[0] = tag;
    if let Some(discriminator) = discriminator {
        key[1..].copy_from_slice(&sp_io::hashing::blake2_256(discriminator)[..7]);
    }
    key
}

fn registered_param_call(record: pallet_constitution::ParamRecord) -> RuntimeCall {
    RuntimeCall::Constitution(pallet_constitution::Call::set_param {
        key: record.key,
        value: record.value,
    })
}

fn derived_single_resource(call: RuntimeCall) -> Option<futarchy_primitives::ResourceId> {
    let footprint = crate::classifier::derive_resource_footprint(&[call]).ok()?;
    (footprint.len() == 1).then(|| footprint[0])
}

fn submit_param_payload(
    proposer: AccountId,
    calls: Vec<RuntimeCall>,
    resources: Vec<futarchy_primitives::ResourceId>,
) -> Option<futarchy_primitives::ProposalId> {
    let (payload_hash, payload_len) = note_runtime_batch(calls)?;
    let bond = crate::configs::balance_param(b"prop.bond.param");
    ForeignAssets::mint_into(usdc_location(), &proposer, bond).ok()?;
    let pid = pallet_epoch::NextProposalId::<Runtime>::get();
    let mut proposal = empty_param_proposal(pid, proposer.clone(), payload_hash, payload_len);
    proposal.resources = futarchy_primitives::BoundedVec::try_from(resources).ok()?;
    Epoch::submit(RuntimeOrigin::signed(proposer), proposal).ok()?;
    Some(pid)
}

fn tick_qualification(pids: Vec<futarchy_primitives::ProposalId>) -> Option<()> {
    System::set_block_number(current_qualify_block());
    let batch = pallet_epoch::TickBatch::try_from(pids).ok()?;
    Epoch::tick(RuntimeOrigin::signed(account(229)), batch).ok()?;
    Some(())
}

fn current_qualify_block() -> BlockNumber {
    let schedule = pallet_epoch::Schedule::<Runtime>::get();
    schedule.epoch_start_block.saturating_add(
        schedule
            .length
            .saturating_mul(futarchy_primitives::phase_offsets::QUALIFY_NUM)
            / futarchy_primitives::phase_offsets::DENOMINATOR,
    )
}

fn stored_proposal_state(pid: futarchy_primitives::ProposalId) -> Option<ProposalState> {
    pallet_epoch::Proposals::<Runtime>::get(pid)
        .or_else(|| pallet_epoch::IntakeProposals::<Runtime>::get(pid))
        .map(|proposal| proposal.state)
}

fn seed_submitted_as_qualified(
    pid: futarchy_primitives::ProposalId,
    metric_spec: futarchy_primitives::MetricSpecVersion,
) -> Option<()> {
    let mut proposal = pallet_epoch::IntakeProposals::<Runtime>::take(pid)?;
    proposal.state = ProposalState::Qualified;
    proposal.metric_spec = metric_spec;
    let schedule = pallet_epoch::Schedule::<Runtime>::get();
    proposal.decide_at = schedule.epoch_start_block.saturating_add(
        schedule
            .length
            .saturating_mul(futarchy_primitives::phase_offsets::DECIDE_NUM)
            / futarchy_primitives::phase_offsets::DENOMINATOR,
    );
    pallet_epoch::IntakeQueue::<Runtime>::mutate(|queue| queue.retain(|queued| *queued != pid));
    pallet_epoch::ProposalSchedules::<Runtime>::insert(
        pid,
        pallet_epoch::ProposalSchedule {
            epoch: proposal.epoch,
            epoch_start_block: schedule.epoch_start_block,
            epoch_length: schedule.length,
            decide_at: proposal.decide_at,
            metric_spec,
        },
    );
    <Preimage as QueryPreimage>::request(&proposal.payload_hash.into());
    pallet_epoch::QualificationPreimageRequests::<Runtime>::insert(pid, proposal.payload_hash);
    pallet_epoch::Proposals::<Runtime>::insert(pid, proposal);
    Some(())
}

fn qualification_states_for_order(reverse: bool) -> Option<(Vec<ProposalState>, usize)> {
    let mut ext = development_ext();
    ext.execute_with(|| {
        install_single_active_metric_spec(30)?;
        let slots = usize::from(
            <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get()
                .epoch_slots,
        );
        let candidate_count = slots.checked_add(1)?;
        let (payload_hash, payload_len) = note_runtime_batch(Vec::new())?;
        let floor = crate::configs::balance_param(b"prop.bond.param");
        let mut pids = Vec::new();
        for index in 0..candidate_count {
            let seed = u8::try_from(index).ok()?.checked_add(170)?;
            let proposer = account(seed);
            let premium = Balance::try_from(index).ok()?.checked_add(1)?;
            let held = floor.checked_add(premium)?;
            assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, held,));
            let pid = pallet_epoch::NextProposalId::<Runtime>::get();
            let mut proposal =
                empty_param_proposal(pid, proposer.clone(), payload_hash, payload_len);
            proposal.bond = held;
            assert_ok!(Epoch::submit(RuntimeOrigin::signed(proposer), proposal));
            pids.push(pid);
        }
        let mut crank_order = pids.clone();
        if reverse {
            crank_order.reverse();
        }
        System::set_block_number(current_qualify_block());
        let batch = pallet_epoch::TickBatch::try_from(crank_order).ok()?;
        Epoch::tick(RuntimeOrigin::signed(account(190)), batch).ok()?;
        let states = pids
            .iter()
            .map(|pid| stored_proposal_state(*pid))
            .collect::<Option<Vec<_>>>()?;
        Some((states, slots))
    })
}

fn enqueue_treasury_call(
    pid: futarchy_primitives::ProposalId,
    call: RuntimeCall,
) -> Option<BlockNumber> {
    let batch =
        pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![call]).ok()?;
    let bytes = batch.encode();
    let payload_len = u32::try_from(bytes.len()).ok()?;
    let payload_hash = <Preimage as StorePreimage>::note(bytes.into()).ok()?;
    let now = System::block_number();
    let maturity = now.checked_add(
        <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_timelock(
            ProposalClass::Treasury,
        ),
    )?;
    let grace_end = maturity.checked_add(
        <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_grace(
            ProposalClass::Treasury,
        ),
    )?;
    let version_constraint = pallet_execution_guard::CurrentSpecName::<Runtime>::get()?;
    let declared_domains = pallet_execution_guard::pallet::StoredDomains::try_from(vec![
        pallet_execution_guard::CallDomain::Treasury,
    ])
    .ok()?;
    seed_queued_epoch_proposal(
        pid,
        ProposalClass::Treasury,
        payload_hash,
        payload_len,
        maturity,
        grace_end,
        version_constraint.clone(),
    )
    .ok()?;
    assert_ok!(ExecutionGuard::enqueue(
        RuntimeOrigin::signed(crate::configs::epoch_account()),
        pallet_execution_guard::pallet::StoredQueuedExecution {
            pid,
            payload_hash: payload_hash.0,
            payload_len,
            class: ProposalClass::Treasury,
            maturity,
            grace_end,
            version_constraint,
            meters_declared: Default::default(),
            ratify_ref: None,
            ratification_passed: false,
            attestation_id: None,
            pre_upgrade_checkpoint: None,
            cancelled: false,
            declared_domains,
            failed_at: None,
        },
        false,
    ));
    Some(maturity)
}

/// Enqueue a Treasury execution from PRE-ENCODED preimage bytes (skipping the
/// main-thread `encode()` of `enqueue_treasury_call`, which would recurse for a
/// deeply-nested payload). Treasury needs no ratification/attestation, so
/// `execute` reaches `decode_batch` after only the maturity check.
fn enqueue_treasury_bytes(
    pid: futarchy_primitives::ProposalId,
    bytes: Vec<u8>,
) -> Option<BlockNumber> {
    let payload_len = u32::try_from(bytes.len()).ok()?;
    let payload_hash = <Preimage as StorePreimage>::note(bytes.into()).ok()?;
    let now = System::block_number();
    let maturity = now.checked_add(
        <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_timelock(
            ProposalClass::Treasury,
        ),
    )?;
    let grace_end = maturity.checked_add(
        <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_grace(
            ProposalClass::Treasury,
        ),
    )?;
    let version_constraint = pallet_execution_guard::CurrentSpecName::<Runtime>::get()?;
    let declared_domains = pallet_execution_guard::pallet::StoredDomains::try_from(vec![
        pallet_execution_guard::CallDomain::Treasury,
    ])
    .ok()?;
    seed_queued_epoch_proposal(
        pid,
        ProposalClass::Treasury,
        payload_hash,
        payload_len,
        maturity,
        grace_end,
        version_constraint.clone(),
    )
    .ok()?;
    assert_ok!(ExecutionGuard::enqueue(
        RuntimeOrigin::signed(crate::configs::epoch_account()),
        pallet_execution_guard::pallet::StoredQueuedExecution {
            pid,
            payload_hash: payload_hash.0,
            payload_len,
            class: ProposalClass::Treasury,
            maturity,
            grace_end,
            version_constraint,
            meters_declared: Default::default(),
            ratify_ref: None,
            ratification_passed: false,
            attestation_id: None,
            pre_upgrade_checkpoint: None,
            cancelled: false,
            declared_domains,
            failed_at: None,
        },
        false,
    ));
    Some(maturity)
}

pub(crate) fn seed_parachain_upgrade_boundary(candidate_len: usize) {
    let max_code_size = u32::try_from(candidate_len).map_or(u32::MAX, |len| len.saturating_add(1));
    cumulus_pallet_parachain_system::ValidationData::<Runtime>::put(
        cumulus_primitives_core::PersistedValidationData::default(),
    );
    cumulus_pallet_parachain_system::HostConfiguration::<Runtime>::put(
        cumulus_primitives_core::AbridgedHostConfiguration {
            max_code_size,
            max_head_data_size: 0,
            max_upward_queue_count: 0,
            max_upward_queue_size: 0,
            max_upward_message_size: 0,
            max_upward_message_num_per_candidate: 0,
            hrmp_max_message_num_per_candidate: 0,
            validation_upgrade_cooldown: 0,
            validation_upgrade_delay: 0,
            async_backing_params: cumulus_primitives_core::relay_chain::AsyncBackingParams {
                max_candidate_depth: 0,
                allowed_ancestry_len: 0,
            },
        },
    );
    cumulus_pallet_parachain_system::UpgradeRestrictionSignal::<Runtime>::kill();
}

fn submit_relay_upgrade_go_ahead() {
    submit_relay_upgrade_signal(cumulus_primitives_core::relay_chain::UpgradeGoAhead::GoAhead);
}

fn submit_relay_upgrade_abort() {
    submit_relay_upgrade_signal(cumulus_primitives_core::relay_chain::UpgradeGoAhead::Abort);
}

fn submit_relay_parent(relay_parent_number: u32) {
    let builder = cumulus_test_relay_sproof_builder::RelayStateSproofBuilder {
        para_id: futarchy_primitives::chain_identity::FIXTURE_PARA_ID.into(),
        current_slot: u64::from(relay_parent_number).into(),
        included_para_head: Some(cumulus_primitives_core::relay_chain::HeadData(Vec::new())),
        ..Default::default()
    };
    let (relay_parent_storage_root, relay_chain_state) = builder.into_state_root_and_proof();
    let data = cumulus_pallet_parachain_system::parachain_inherent::BasicParachainInherentData {
        validation_data: cumulus_primitives_core::PersistedValidationData {
            relay_parent_number,
            relay_parent_storage_root,
            ..Default::default()
        },
        relay_chain_state,
        relay_parent_descendants: Default::default(),
        collator_peer_id: None,
    };
    let inbound = cumulus_pallet_parachain_system::parachain_inherent::InboundMessagesData::new(
        Default::default(),
        Default::default(),
    );
    pallet_aura::CurrentSlot::<Runtime>::put(sp_consensus_aura::Slot::from(u64::from(
        relay_parent_number,
    )));
    cumulus_pallet_parachain_system::ValidationData::<Runtime>::kill();
    assert_ok!(ParachainSystem::set_validation_data(
        RuntimeOrigin::none(),
        data,
        inbound,
    ));
}

fn submit_relay_upgrade_signal(signal: cumulus_primitives_core::relay_chain::UpgradeGoAhead) {
    let builder = cumulus_test_relay_sproof_builder::RelayStateSproofBuilder {
        para_id: futarchy_primitives::chain_identity::FIXTURE_PARA_ID.into(),
        upgrade_go_ahead: Some(signal),
        included_para_head: Some(cumulus_primitives_core::relay_chain::HeadData(Vec::new())),
        ..Default::default()
    };
    let (relay_parent_storage_root, relay_chain_state) = builder.into_state_root_and_proof();
    let data = cumulus_pallet_parachain_system::parachain_inherent::BasicParachainInherentData {
        validation_data: cumulus_primitives_core::PersistedValidationData {
            relay_parent_number: 1,
            relay_parent_storage_root,
            ..Default::default()
        },
        relay_chain_state,
        relay_parent_descendants: Default::default(),
        collator_peer_id: None,
    };
    let inbound = cumulus_pallet_parachain_system::parachain_inherent::InboundMessagesData::new(
        Default::default(),
        Default::default(),
    );
    // `seed_parachain_upgrade_boundary` models the scheduling block. The real
    // next-block initialize removes its validation data before this inherent.
    cumulus_pallet_parachain_system::ValidationData::<Runtime>::kill();
    assert_ok!(ParachainSystem::set_validation_data(
        RuntimeOrigin::none(),
        data,
        inbound,
    ));
}

pub(crate) fn remark() -> RuntimeCall {
    RuntimeCall::System(frame_system::Call::remark { remark: vec![1] })
}

pub(crate) fn set_pending_upgrade(applicable_at: Option<BlockNumber>) {
    match applicable_at {
        Some(applicable_at) => {
            pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::put(
                pallet_execution_guard::PendingUpgrade {
                    hash: sp_io::hashing::blake2_256(&[1]),
                    authorized_at: applicable_at
                        .saturating_sub(kernel::DESCRIPTOR_LEAD_TIME_BLOCKS),
                    applicable_at,
                    target_spec_version: VERSION.spec_version.saturating_add(1),
                },
            );
        }
        None => pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::kill(),
    }
}

pub(crate) fn nobody_system_calls() -> Vec<RuntimeCall> {
    vec![
        RuntimeCall::System(frame_system::Call::set_heap_pages { pages: 64 }),
        RuntimeCall::System(frame_system::Call::set_code { code: vec![1] }),
        RuntimeCall::System(frame_system::Call::set_code_without_checks { code: vec![1] }),
        RuntimeCall::System(frame_system::Call::set_storage {
            items: vec![(vec![1], vec![2])],
        }),
        RuntimeCall::System(frame_system::Call::kill_storage {
            keys: vec![vec![1]],
        }),
        RuntimeCall::System(frame_system::Call::kill_prefix {
            prefix: vec![1],
            subkeys: 1,
        }),
        RuntimeCall::System(frame_system::Call::authorize_upgrade {
            code_hash: H256::repeat_byte(8),
        }),
        RuntimeCall::System(frame_system::Call::authorize_upgrade_without_checks {
            code_hash: H256::repeat_byte(9),
        }),
    ]
}

pub(crate) fn closed_wrappers(call: RuntimeCall) -> Vec<RuntimeCall> {
    let who = account(7);
    let signed_origin: <RuntimeOrigin as frame_support::traits::OriginTrait>::PalletsOrigin =
        frame_system::RawOrigin::Signed(who.clone()).into();
    vec![
        RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![call.clone()],
        }),
        RuntimeCall::Utility(pallet_utility::Call::batch_all {
            calls: vec![call.clone()],
        }),
        RuntimeCall::Utility(pallet_utility::Call::force_batch {
            calls: vec![call.clone()],
        }),
        RuntimeCall::Utility(pallet_utility::Call::as_derivative {
            index: 0,
            call: Box::new(call.clone()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::dispatch_as {
            as_origin: Box::new(signed_origin.clone()),
            call: Box::new(call.clone()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::with_weight {
            call: Box::new(call.clone()),
            weight: Weight::zero(),
        }),
        RuntimeCall::Utility(pallet_utility::Call::if_else {
            main: Box::new(call.clone()),
            fallback: Box::new(remark()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::if_else {
            main: Box::new(remark()),
            fallback: Box::new(call.clone()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::dispatch_as_fallible {
            as_origin: Box::new(signed_origin),
            call: Box::new(call.clone()),
        }),
        RuntimeCall::Proxy(pallet_proxy::Call::proxy {
            real: MultiAddress::Id(who.clone()),
            force_proxy_type: None,
            call: Box::new(call.clone()),
        }),
        RuntimeCall::Proxy(pallet_proxy::Call::proxy_announced {
            delegate: MultiAddress::Id(who.clone()),
            real: MultiAddress::Id(account(8)),
            force_proxy_type: None,
            call: Box::new(call.clone()),
        }),
        RuntimeCall::Multisig(pallet_multisig::Call::as_multi {
            threshold: 2,
            other_signatories: vec![who.clone()],
            maybe_timepoint: None,
            call: Box::new(call.clone()),
            max_weight: Weight::zero(),
        }),
        RuntimeCall::Multisig(pallet_multisig::Call::as_multi_threshold_1 {
            other_signatories: vec![who.clone()],
            call: Box::new(call.clone()),
        }),
        RuntimeCall::Sudo(pallet_sudo::Call::sudo {
            call: Box::new(call.clone()),
        }),
        RuntimeCall::Sudo(pallet_sudo::Call::sudo_unchecked_weight {
            call: Box::new(call.clone()),
            weight: Weight::zero(),
        }),
        RuntimeCall::Sudo(pallet_sudo::Call::sudo_as {
            who: MultiAddress::Id(who),
            call: Box::new(call),
        }),
    ]
}

fn signed_vit_transfer(destination: AccountId, amount: crate::Balance) -> UncheckedExtrinsic {
    let call = RuntimeCall::Balances(pallet_balances::Call::transfer_allow_death {
        dest: MultiAddress::Id(destination),
        value: amount,
    });
    let extensions: TxExtension = (
        frame_system::AuthorizeCall::<Runtime>::new(),
        frame_system::CheckNonZeroSender::<Runtime>::new(),
        frame_system::CheckSpecVersion::<Runtime>::new(),
        frame_system::CheckTxVersion::<Runtime>::new(),
        frame_system::CheckGenesis::<Runtime>::new(),
        frame_system::CheckEra::<Runtime>::from(Era::Immortal),
        frame_system::CheckNonce::<Runtime>::from(0),
        frame_system::CheckWeight::<Runtime>::new(),
        pallet_asset_tx_payment::ChargeAssetTxPayment::<Runtime>::from(0, None),
        (
            frame_metadata_hash_extension::CheckMetadataHash::<Runtime>::new(false),
            crate::StorageWeightReclaim::new(),
        ),
    );
    let payload = match SignedPayload::new(call, extensions) {
        Ok(payload) => payload,
        Err(error) => {
            assert!(false, "signed payload must be constructible: {error:?}");
            return UncheckedExtrinsic::new_bare(remark());
        }
    };
    let signature = payload.using_encoded(|bytes| Sr25519Keyring::Alice.sign(bytes));
    let (call, extensions, _) = payload.deconstruct();
    UncheckedExtrinsic::new_signed(
        call,
        MultiAddress::Id(Sr25519Keyring::Alice.to_account_id()),
        MultiSignature::Sr25519(signature),
        extensions,
    )
}

fn build_executive_smoke_block(destination: AccountId) -> crate::Block {
    let builder = cumulus_test_relay_sproof_builder::RelayStateSproofBuilder {
        para_id: futarchy_primitives::chain_identity::FIXTURE_PARA_ID.into(),
        current_slot: 1u64.into(),
        included_para_head: Some(cumulus_primitives_core::relay_chain::HeadData(Vec::new())),
        ..Default::default()
    };
    let (relay_parent_storage_root, relay_chain_state) = builder.into_state_root_and_proof();
    let validation_data = cumulus_primitives_core::PersistedValidationData {
        relay_parent_number: 1,
        relay_parent_storage_root,
        ..Default::default()
    };
    let parachain_data = cumulus_primitives_parachain_inherent::ParachainInherentData {
        validation_data,
        relay_chain_state,
        downward_messages: Default::default(),
        horizontal_messages: Default::default(),
        relay_parent_descendants: Default::default(),
        collator_peer_id: None,
    };
    let mut inherent_data = InherentData::new();
    assert!(inherent_data
        .put_data(*b"timstap0", &kernel::MILLISECS_PER_BLOCK)
        .is_ok());
    assert!(inherent_data
        .put_data(
            cumulus_primitives_parachain_inherent::INHERENT_IDENTIFIER,
            &parachain_data,
        )
        .is_ok());
    let mut extrinsics = crate::InherentDataExt::create_extrinsics(&inherent_data);
    assert_eq!(extrinsics.len(), 2);
    extrinsics.push(signed_vit_transfer(
        destination,
        currency::VIT_EXISTENTIAL_DEPOSIT,
    ));

    let header = crate::Header::new(
        1,
        Default::default(),
        Default::default(),
        System::block_hash(0),
        sp_runtime::Digest {
            logs: vec![sp_runtime::DigestItem::PreRuntime(
                sp_consensus_aura::AURA_ENGINE_ID,
                1u64.encode(),
            )],
        },
    );
    crate::Executive::initialize_block(&header);
    for extrinsic in extrinsics.iter().cloned() {
        assert!(crate::Executive::apply_extrinsic(extrinsic).is_ok());
    }
    let finalized = crate::Executive::finalize_block();
    crate::Block::new(finalized, extrinsics)
}

#[test]
fn composition_contains_all_wired_pallets_at_their_frozen_indices() {
    macro_rules! assert_pallet {
        ($pallet:ty, $index:expr, $name:expr) => {{
            assert_eq!(
                <RuntimePalletInfo as PalletInfo>::index::<$pallet>(),
                Some($index)
            );
            assert_eq!(
                <RuntimePalletInfo as PalletInfo>::name::<$pallet>(),
                Some($name)
            );
        }};
    }

    assert_pallet!(System, 0, "System");
    assert_pallet!(Timestamp, 1, "Timestamp");
    assert_pallet!(ParachainSystem, 2, "ParachainSystem");
    assert_pallet!(ParachainInfo, 3, "ParachainInfo");
    assert_pallet!(Balances, 10, "Balances");
    assert_pallet!(ForeignAssets, 11, "ForeignAssets");
    assert_pallet!(TransactionPayment, 12, "TransactionPayment");
    assert_pallet!(AssetTxPayment, 13, "AssetTxPayment");
    assert_pallet!(Vesting, 14, "Vesting");
    assert_pallet!(Referenda, 20, "Referenda");
    assert_pallet!(ConvictionVoting, 21, "ConvictionVoting");
    assert_pallet!(Preimage, 22, "Preimage");
    assert_pallet!(Scheduler, 23, "Scheduler");
    assert_pallet!(Utility, 24, "Utility");
    assert_pallet!(Proxy, 25, "Proxy");
    assert_pallet!(Multisig, 26, "Multisig");
    assert_pallet!(Migrations, 27, "Migrations");
    assert_pallet!(Sudo, 28, "Sudo");
    assert_pallet!(XcmpQueue, 30, "XcmpQueue");
    assert_pallet!(MessageQueue, 31, "MessageQueue");
    assert_pallet!(CumulusXcm, 32, "CumulusXcm");
    assert_pallet!(PolkadotXcm, 33, "PolkadotXcm");
    assert_pallet!(Authorship, 40, "Authorship");
    assert_pallet!(CollatorSelection, 41, "CollatorSelection");
    assert_pallet!(Session, 42, "Session");
    assert_pallet!(Aura, 43, "Aura");
    assert_pallet!(AuraExt, 44, "AuraExt");
    assert_pallet!(Origins, 50, "Origins");
    assert_pallet!(Constitution, 51, "Constitution");
    assert_pallet!(ConditionalLedger, 52, "ConditionalLedger");
    assert_pallet!(Market, 53, "Market");
    assert_pallet!(Welfare, 54, "Welfare");
    assert_pallet!(Oracle, 55, "Oracle");
    assert_pallet!(IncidentRegistry, 56, "IncidentRegistry");
    assert_pallet!(MilestoneRegistry, 57, "MilestoneRegistry");
    assert_pallet!(FutarchyTreasury, 58, "FutarchyTreasury");
    assert_pallet!(Guardian, 59, "Guardian");
    assert_pallet!(Attestor, 60, "Attestor");
    assert_pallet!(Epoch, 61, "Epoch");
    assert_pallet!(ExecutionGuard, 62, "ExecutionGuard");
    assert_pallet!(InflowCaps, 63, "InflowCaps");
    assert_pallet!(TrackOrigins, 64, "TrackOrigins");
    assert_eq!(
        <AllPalletsWithSystem as PalletsInfoAccess>::infos().len(),
        42
    );
}

#[test]
fn epoch_clock_is_live_across_sibling_configs() {
    use frame_support::traits::Get;

    development_ext().execute_with(|| {
        pallet_epoch::EpochOf::<Runtime>::mutate(|epoch| epoch.index = 7);
        assert_eq!(Epoch::current_epoch(), 7);
        assert_eq!(pallet_epoch::CurrentEpoch::<Runtime>::get(), 7);
        assert_eq!(
            <<Runtime as pallet_welfare::Config>::CurrentEpoch as Get<u32>>::get(),
            7
        );
        assert_eq!(
            <<Runtime as pallet_guardian::Config>::CurrentEpoch as Get<u32>>::get(),
            7
        );
        assert_eq!(
            <<Runtime as pallet_futarchy_treasury::Config>::CurrentEpoch as Get<u32>>::get(),
            7
        );
    });
}

#[test]
fn execution_guard_enqueue_rejects_signed_callers() {
    development_ext().execute_with(|| {
        let version = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => return assert!(false, "guard genesis must seed its runtime version"),
        };
        let item = pallet_execution_guard::StoredQueuedExecution {
            pid: 1,
            payload_hash: [1; 32],
            payload_len: 0,
            class: ProposalClass::Param,
            maturity: 1,
            grace_end: 2,
            version_constraint: version,
            meters_declared: Default::default(),
            ratify_ref: None,
            ratification_passed: false,
            attestation_id: None,
            pre_upgrade_checkpoint: None,
            cancelled: false,
            declared_domains: Default::default(),
            failed_at: None,
        };
        assert_eq!(
            ExecutionGuard::enqueue(RuntimeOrigin::signed(account(77)), item, false),
            Err(DispatchError::BadOrigin)
        );
    });
}

#[test]
fn guard_rejects_best_effort_wrappers_and_admits_atomic_batch_all() {
    // limit-coverage: dead-man-switch
    use pallet_execution_guard::BatchDispatcher;

    development_ext().execute_with(|| {
        let leaf = RuntimeCall::Constitution(pallet_constitution::Call::set_param {
            key: pallet_constitution::key16(b"mkt.obs_interval"),
            value: pallet_constitution::ParamValue::U32(10),
        });
        let batch = RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![leaf.clone()],
        });
        let force_batch = RuntimeCall::Utility(pallet_utility::Call::force_batch {
            calls: vec![leaf.clone()],
        });
        let batch_all = RuntimeCall::Utility(pallet_utility::Call::batch_all { calls: vec![leaf] });
        assert!(!RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &batch
        ));
        assert!(!RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &force_batch
        ));
        assert!(RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &batch_all
        ));
        pallet_epoch::EpochOf::<Runtime>::mutate(|epoch| epoch.index = 10);
        assert!(RuntimeDispatcher::dispatch_with_class_origin(
            batch_all.clone(),
            ProposalClass::Param,
        )
        .is_ok());
        pallet_constitution::Capabilities::<Runtime>::mutate(|rows| {
            if let Some(row) = rows.iter_mut().find(|row| {
                row.class == ProposalClass::Param
                    && row.capability
                        == pallet_constitution::Capability::SetParam(pallet_constitution::key16(
                            b"mkt.obs_interval",
                        ))
            }) {
                row.enabled = false;
            }
        });
        assert!(!RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &batch_all
        ));
        assert!(RuntimeDispatcher::dispatch_with_class_origin(
            batch_all.clone(),
            ProposalClass::Param,
        )
        .is_err());
        pallet_constitution::Capabilities::<Runtime>::mutate(|rows| {
            if let Some(row) = rows.iter_mut().find(|row| {
                row.class == ProposalClass::Param
                    && row.capability
                        == pallet_constitution::Capability::SetParam(pallet_constitution::key16(
                            b"mkt.obs_interval",
                        ))
            }) {
                row.enabled = true;
            }
        });
        let live_epoch = pallet_epoch::EpochOf::<Runtime>::get().index;
        pallet_welfare::GateBreachFlags::<Runtime>::insert(
            live_epoch,
            pallet_welfare::CoreGateBreachFlags {
                s_breached: true,
                c_breached: false,
                day_bitmap: [1, 0],
            },
        );
        assert!(!RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &batch_all
        ));
        // PR #66 Codex P1: only the CURRENT epoch's gate record freezes
        // execution. A breached record retained from a prior epoch (welfare's
        // rolling window; pruning is keeper-driven) must auto-release once the
        // epoch has moved on (06 §5).
        pallet_epoch::EpochOf::<Runtime>::mutate(|epoch| epoch.index = live_epoch + 1);
        assert!(RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &batch_all
        ));
        pallet_epoch::EpochOf::<Runtime>::mutate(|epoch| epoch.index = live_epoch);
        pallet_welfare::GateBreachFlags::<Runtime>::remove(live_epoch);
        pallet_constitution::PhaseFlags::<Runtime>::mutate(|flags| {
            *flags |= pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED;
        });
        assert!(!RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &batch_all
        ));
        let coretime_renewal = RuntimeCall::FutarchyTreasury(
            pallet_futarchy_treasury::Call::execute_coretime_renewal { period_index: 1 },
        );
        assert!(RuntimeBaseCallFilter::contains(&coretime_renewal));
    });
}

#[test]
fn canonical_asset_hub_exit_is_public_and_runtime_filter_rejects_sibling_or_unknown_asset() {
    use staging_xcm::latest::{prelude::*, Asset, AssetId, Assets, Fungibility};

    let beneficiary = local_xcm_account(&account(76));
    let usdc = Asset {
        id: AssetId(usdc_location()),
        fun: Fungibility::Fungible(1),
    };
    let call = |destination: staging_xcm::latest::Location| {
        RuntimeCall::PolkadotXcm(pallet_xcm::Call::limited_reserve_transfer_assets {
            dest: Box::new(VersionedLocation::from(destination)),
            beneficiary: Box::new(VersionedLocation::from(beneficiary.clone())),
            assets: Box::new(VersionedAssets::from(Assets::from(usdc.clone()))),
            fee_asset_item: 0,
            weight_limit: Limited(production_xcm_weight_limit()),
        })
    };
    assert!(RuntimeBaseCallFilter::contains(&call(
        bleavit_xcm::identity::asset_hub_location(),
    )));
    assert!(!RuntimeBaseCallFilter::contains(&call(
        bleavit_xcm::identity::coretime_location(),
    )));

    type ExitFilter = <Runtime as pallet_xcm::Config>::XcmReserveTransferFilter;
    assert_same_type::<ExitFilter, bleavit_xcm::filter::ReserveTransferFilter>();
    let local_origin = local_xcm_account(&account(76));
    assert!(ExitFilter::contains(&(
        local_origin.clone(),
        vec![usdc.clone()],
    )));
    assert!(!ExitFilter::contains(&(
        local_origin,
        vec![Asset {
            id: AssetId(bleavit_xcm::identity::coretime_location()),
            fun: Fungibility::Fungible(1),
        }],
    )));
}

#[test]
fn identity_and_version_pins_match_the_integration_contract() {
    assert_eq!(SS58_PREFIX, 7_777);
    assert_eq!(SS58_PREFIX, chain_identity::SS58_PREFIX);
    assert_eq!(MILLISECS_PER_BLOCK, kernel::MILLISECS_PER_BLOCK);
    assert_eq!(MILLISECS_PER_BLOCK, 6_000);
    assert_eq!(currency::VIT_EXISTENTIAL_DEPOSIT, 10_000_000_000);
    assert_eq!(VIT_DECIMALS, 12);
    assert_eq!(USDC_DECIMALS, 6);
    assert_eq!(FEE_VIT_USDC_RATE_KEY, *b"fee.vit_usdc\0\0\0\0");
    assert_eq!(VERSION.spec_name.as_ref(), "bleavit");
    assert_eq!(VERSION.impl_name.as_ref(), "bleavit-runtime");
    assert_eq!(VERSION.spec_version, 1);
    assert_eq!(
        VERSION.transaction_version,
        futarchy_primitives::INTEGRATION_CONTRACT_VERSION
    );
    // The XCM identity spelling originated in contract v4; v5 retains it while
    // changing Treasury class-floor and gate-market semantics.
    assert_eq!(VERSION.transaction_version, 5);
    assert_eq!(usdc_location().encode(), USDC_LOCATION_ENCODED);
}

#[test]
fn usdc_admin_and_fee_posture_is_fail_closed() {
    let create = RuntimeCall::ForeignAssets(pallet_assets::Call::create {
        id: usdc_location(),
        admin: MultiAddress::Id(account(1)),
        min_balance: currency::USDC_CENT,
    });
    let mint = RuntimeCall::ForeignAssets(pallet_assets::Call::mint {
        id: usdc_location(),
        beneficiary: MultiAddress::Id(account(2)),
        amount: currency::USDC_CENT,
    });
    // SQ-151: the bare scheduler leaf must clear the origin-blind base filter;
    // the pallet's CreateOrigin remains the independent authority check.
    assert!(RuntimeBaseCallFilter::contains(&create));
    assert!(RuntimeBaseCallFilter::contains_for(
        ClassOrigin::ConstitutionalValues,
        &create
    ));
    assert!(!RuntimeBaseCallFilter::contains(&mint));
    assert!(!RuntimeBaseCallFilter::contains_for(
        ClassOrigin::ConstitutionalValues,
        &mint
    ));

    development_ext().execute_with(|| {
        assert!(crate::configs::LiveFeeConversion::to_asset_balance(1, usdc_location()).is_err());
        let other_asset = bleavit_xcm::identity::asset_hub_asset_location(
            chain_identity::USDC_ASSET_INDEX.saturating_add(1),
        );
        assert!(crate::configs::LiveFeeConversion::to_asset_balance(1, other_asset).is_err());
    });
}

#[test]
fn usdc_fee_conversion_scales_decimals_and_rounds_against_the_payer() {
    development_ext().execute_with(|| {
        pallet_constitution::Params::<Runtime>::insert(
            FEE_VIT_USDC_RATE_KEY,
            pallet_constitution::ParamRecord {
                key: FEE_VIT_USDC_RATE_KEY,
                value: pallet_constitution::ParamValue::Fixed(futarchy_primitives::FixedU64(
                    2_000_000_000,
                )),
                min: pallet_constitution::ParamValue::Fixed(futarchy_primitives::FixedU64(1)),
                max: pallet_constitution::ParamValue::Fixed(futarchy_primitives::FixedU64(
                    u64::MAX,
                )),
                max_delta: None,
                cooldown_epochs: 0,
                last_changed_epoch: 0,
                last_change_block: 0,
                class: pallet_constitution::ParamClass::Treasury,
                kernel_bounded: false,
            },
        );
        assert_eq!(
            crate::configs::LiveFeeConversion::to_asset_balance(currency::VIT, usdc_location()),
            Ok(2 * currency::USDC)
        );
        assert_eq!(
            crate::configs::LiveFeeConversion::to_asset_balance(1, usdc_location()),
            Ok(1)
        );
        assert_eq!(
            crate::configs::LiveFeeConversion::to_asset_balance(0, usdc_location()),
            Ok(0)
        );
    });
}

#[test]
fn governed_xcm_rates_are_read_from_genesis_params_and_live_updates() {
    development_ext().execute_with(|| {
        use crate::configs::ConstitutionTraderRates;

        // 13 §1 defaults: xcm.trade_dot_per_sec = 10 DOT/s (1e11 planck),
        // xcm.trade_dot_per_mb = 1 DOT/MiB (1e10); xcm.trade_usdc_per_sec =
        // 50 USDC/s (5e7 µUSDC), xcm.trade_usdc_per_mb = 5 USDC/MiB (5e6).
        assert_eq!(
            ConstitutionTraderRates::dot_rate(),
            WeightRate {
                units_per_second: 100_000_000_000,
                units_per_megabyte: 10_000_000_000,
            }
        );
        assert_eq!(
            ConstitutionTraderRates::usdc_rate(),
            WeightRate {
                units_per_second: 50_000_000,
                units_per_megabyte: 5_000_000,
            }
        );

        let key = pallet_constitution::key16(b"xcm.dot_per_sec");
        // The live clock begins at epoch zero, so exercise the real registry-
        // amendment path to make this non-kernel PARAM row writable before
        // proving `set_param` is observed immediately.
        assert_ok!(Constitution::amend_registry(
            pallet_origins::Origin::FutarchyMeta.into(),
            key,
            pallet_constitution::ParamValue::Balance(1_000_000_000),
            pallet_constitution::ParamValue::Balance(10_000_000_000_000),
            Some(pallet_constitution::MaxDelta::Factor(2)),
            0,
        ));
        assert_ok!(Constitution::set_param(
            pallet_origins::Origin::FutarchyParam.into(),
            key,
            pallet_constitution::ParamValue::Balance(200_000_000_000),
        ));
        assert_eq!(
            ConstitutionTraderRates::dot_rate().units_per_second,
            200_000_000_000
        );
    });
}

#[test]
fn oracle_registration_reads_live_constitution_stake() {
    development_ext().execute_with(|| {
        pallet_epoch::EpochOf::<Runtime>::mutate(|clock| {
            clock.index = clock.index.saturating_add(2)
        });
        assert_eq!(
            <crate::configs::RuntimeOracleParams as pallet_oracle::OracleParamsProvider>::get()
                .bond_bps,
            250,
        );
        assert_ok!(Constitution::set_param(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::key16(b"orc.bond_bps"),
            pallet_constitution::ParamValue::Perbill(30_000_000),
        ));
        assert_eq!(
            <crate::configs::RuntimeOracleParams as pallet_oracle::OracleParamsProvider>::get()
                .bond_bps,
            300,
            "Perbill storage must convert to basis points exactly once",
        );
        let amended_stake = 150_000_000_000_u128;
        assert_ok!(Constitution::set_param(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::key16(b"orc.rep_stake"),
            pallet_constitution::ParamValue::Balance(amended_stake),
        ));

        let reporter = account(48);
        assert_ok!(Oracle::register_reporter(RuntimeOrigin::signed(
            reporter.clone()
        )));
        assert_eq!(
            pallet_oracle::Reporters::<Runtime>::get(reporter).map(|info| info.stake),
            Some(amended_stake),
        );
    });
}

#[test]
fn b10_param_providers_match_defaults_and_exact_genesis_keys() {
    development_ext().execute_with(|| {
        use pallet_attestor::AttestorParamsProvider;
        use pallet_oracle::OracleParamsProvider;

        assert_eq!(
            <crate::configs::RuntimeOracleParams as OracleParamsProvider>::get(),
            pallet_oracle::OracleParams::DEFAULT
        );
        assert_eq!(
            <crate::configs::RuntimeAttestorParams as AttestorParamsProvider>::get(),
            pallet_attestor::AttestorParams::DEFAULT
        );
        assert_eq!(
            crate::configs::GuardianReviewDeadline::get(),
            pallet_guardian::REVIEW_DEADLINE_EPOCHS
        );

        for name in [
            b"orc.bond_floor".as_slice(),
            b"orc.rep_stake".as_slice(),
            b"wt.stake".as_slice(),
            b"res.probe_amount".as_slice(),
            b"att.bond".as_slice(),
        ] {
            assert!(matches!(
                pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(name))
                    .map(|record| record.value),
                Some(pallet_constitution::ParamValue::Balance(_))
            ));
        }
        for name in [
            b"orc.window".as_slice(),
            b"res.probe_int".as_slice(),
            b"res.probe_to".as_slice(),
            b"att.window".as_slice(),
            b"grd.review_dl".as_slice(),
        ] {
            assert!(matches!(
                pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(name))
                    .map(|record| record.value),
                Some(pallet_constitution::ParamValue::U32(_))
            ));
        }
        for name in [
            b"orc.rounds".as_slice(),
            b"wt.quorum".as_slice(),
            b"res.fail_thr".as_slice(),
            b"res.recover_thr".as_slice(),
        ] {
            assert!(matches!(
                pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(name))
                    .map(|record| record.value),
                Some(pallet_constitution::ParamValue::U8(_))
            ));
        }
        assert!(matches!(
            pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(
                b"orc.bond_bps"
            ))
            .map(|record| record.value),
            Some(pallet_constitution::ParamValue::Perbill(_))
        ));
    });
}

#[test]
fn oracle_bond_bps_conversion_rounds_up_non_whole_basis_point() {
    development_ext().execute_with(|| {
        use pallet_oracle::OracleParamsProvider;

        let key = pallet_constitution::key16(b"orc.bond_bps");
        pallet_constitution::Params::<Runtime>::mutate(key, |record| {
            if let Some(record) = record {
                record.value = pallet_constitution::ParamValue::Perbill(24_999_999);
            }
        });
        assert_eq!(
            <crate::configs::RuntimeOracleParams as OracleParamsProvider>::get().bond_bps,
            250
        );
    });
}

#[test]
fn b10_pol_reader_uses_named_defaults_when_live_records_are_missing() {
    development_ext().execute_with(|| {
        use pallet_epoch::PolBudget;

        for name in [
            b"pol.budget_epoch".as_slice(),
            b"pol.b.param".as_slice(),
            b"pol.b.trs".as_slice(),
            b"pol.b.code".as_slice(),
            b"pol.b.meta".as_slice(),
            b"pol.b_gate".as_slice(),
        ] {
            pallet_constitution::Params::<Runtime>::remove(pallet_constitution::key16(name));
        }
        let proposal = empty_param_proposal(99_001, account(91), H256::zero(), 0);
        let plan = <crate::configs::RuntimePolBudget as PolBudget<AccountId>>::proposal_seed_plan(
            &proposal,
        )
        .expect("PARAM proposal retains a default POL seed plan");
        assert_eq!(plan.decision_b, pallet_constitution::POL_B_DEFAULTS[0]);
        assert_eq!(plan.gate_b, Some(pallet_constitution::POL_GATE_B_DEFAULT));
    });
}

#[test]
fn param_seed_plan_opens_six_proposal_books() {
    use pallet_epoch::{EpochParamsProvider, MarketAccess, PolBudget};

    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 99_003;
        let params = <crate::configs::RuntimeEpochParams as EpochParamsProvider>::get();
        let decision_b = crate::configs::balance_param(b"pol.b.param");
        let gate_b = crate::configs::balance_param(b"pol.b_gate");
        let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
        let decision_headroom =
            pallet_market::core_market::seed_headroom(decision_b).expect("bounded decision b");
        let gate_headroom =
            pallet_market::core_market::seed_headroom(gate_b).expect("bounded gate b");
        let baseline_headroom =
            pallet_market::core_market::seed_headroom(baseline_b).expect("bounded baseline b");
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &crate::configs::pol_account(),
            decision_headroom
                .saturating_add(gate_headroom.saturating_mul(2))
                .saturating_add(currency::USDC),
        ));
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &crate::configs::pol_baseline_account(),
            baseline_headroom.saturating_add(currency::USDC),
        ));
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = decision_headroom
                .saturating_mul(2)
                .saturating_add(gate_headroom.saturating_mul(4))
                .saturating_add(baseline_headroom)
                .saturating_mul(100);
        });

        let mut proposal = empty_param_proposal(PID, account(92), H256::zero(), 0);
        proposal.metric_spec = 1;
        proposal.state = ProposalState::Qualified;
        proposal.decide_at = System::block_number().saturating_add(params.decision_window);
        let plan = <crate::configs::RuntimePolBudget as PolBudget<AccountId>>::proposal_seed_plan(
            &proposal,
        )
        .expect("PARAM proposal retains a POL seed plan");
        assert_eq!(plan.gate_b, Some(gate_b));
        let markets =
            <crate::configs::RuntimeMarketAccess as MarketAccess<AccountId>>::open_markets(
                &proposal,
                false,
                Some(plan),
            )
            .expect("PARAM markets open");
        let gates = markets.gates.expect("PARAM proposal has four gate books");
        let proposal_books = [
            markets.accept,
            markets.reject,
            gates[0],
            gates[1],
            gates[2],
            gates[3],
        ];
        assert_eq!(proposal_books.len(), 6);
        assert!(proposal_books
            .iter()
            .all(pallet_market::Markets::<Runtime>::contains_key));
        assert_eq!(pallet_market::Markets::<Runtime>::count(), 7);
    });
}

#[test]
fn low_ask_treasury_seed_plan_opens_six_proposal_books() {
    use pallet_epoch::{EpochParamsProvider, MarketAccess, PolBudget};

    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 99_002;
        let params = <crate::configs::RuntimeEpochParams as EpochParamsProvider>::get();
        let decision_b = crate::configs::balance_param(b"pol.b.trs");
        let gate_b = crate::configs::balance_param(b"pol.b_gate");
        let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
        let decision_headroom =
            pallet_market::core_market::seed_headroom(decision_b).expect("bounded decision b");
        let gate_headroom =
            pallet_market::core_market::seed_headroom(gate_b).expect("bounded gate b");
        let baseline_headroom =
            pallet_market::core_market::seed_headroom(baseline_b).expect("bounded baseline b");
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &crate::configs::pol_account(),
            decision_headroom
                .saturating_add(gate_headroom.saturating_mul(2))
                .saturating_add(currency::USDC),
        ));
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &crate::configs::pol_baseline_account(),
            baseline_headroom.saturating_add(currency::USDC),
        ));
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = decision_headroom
                .saturating_mul(2)
                .saturating_add(gate_headroom.saturating_mul(4))
                .saturating_add(baseline_headroom)
                .saturating_mul(100);
        });

        let mut proposal = empty_param_proposal(PID, account(91), H256::zero(), 0);
        proposal.class = ProposalClass::Treasury;
        proposal.ask = 1;
        proposal.metric_spec = 1;
        proposal.state = ProposalState::Qualified;
        proposal.decide_at = System::block_number().saturating_add(params.decision_window);
        let nav = FutarchyTreasury::nav().spendable_nav;
        assert!(proposal.ask <= nav / 100, "fixture is at most 1% of NAV");

        let plan = <crate::configs::RuntimePolBudget as PolBudget<AccountId>>::proposal_seed_plan(
            &proposal,
        )
        .expect("low-ask Treasury proposal retains a POL seed plan");
        assert_eq!(plan.gate_b, Some(gate_b));
        let markets =
            <crate::configs::RuntimeMarketAccess as MarketAccess<AccountId>>::open_markets(
                &proposal,
                false,
                Some(plan),
            )
            .expect("low-ask Treasury markets open");
        let gates = markets
            .gates
            .expect("low-ask Treasury proposal has four gate books");
        let proposal_books = [
            markets.accept,
            markets.reject,
            gates[0],
            gates[1],
            gates[2],
            gates[3],
        ];
        assert_eq!(proposal_books.len(), 6);
        assert!(proposal_books
            .iter()
            .all(pallet_market::Markets::<Runtime>::contains_key));
        assert_eq!(pallet_market::Markets::<Runtime>::count(), 7);
    });
}

#[test]
fn real_treasury_nav_and_constitution_pol_budget_shrink_epoch_slots() {
    // limit-coverage: pol.budget_epoch
    use pallet_epoch::PolBudget;

    development_ext().execute_with(|| {
        const SPEC: futarchy_primitives::MetricSpecVersion = 43;
        assert!(install_single_active_metric_spec(SPEC).is_some());
        let probe = empty_param_proposal(99_010, account(94), H256::zero(), 0);
        let commitment =
            <crate::configs::RuntimePolBudget as PolBudget<AccountId>>::proposal_seed_plan(&probe)
                .expect("PARAM commitment")
                .commitment;
        let rate = match pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(
            b"pol.budget_epoch",
        ))
        .map(|record| record.value)
        {
            Some(pallet_constitution::ParamValue::Perbill(parts)) if parts > 0 => u128::from(parts),
            _ => {
                assert!(false, "genesis must contain the live POL budget rate");
                return;
            }
        };
        let desired_budget = commitment.saturating_mul(2).saturating_add(commitment / 2);
        let nav = desired_budget
            .saturating_mul(1_000_000_000)
            .saturating_add(rate.saturating_sub(1))
            / rate;
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = nav;
            state.reserve_impaired = false;
            state.lines.clear();
            state.streams.clear();
            state.pending_outflows.clear();
            state.pol_commitments.clear();
        });
        let live_budget =
            <crate::configs::RuntimePolBudget as PolBudget<AccountId>>::epoch_budget();
        assert!(live_budget >= commitment.saturating_mul(2));
        assert!(live_budget < commitment.saturating_mul(3));

        let call = RuntimeCall::Constitution(pallet_constitution::Call::set_param {
            key: pallet_constitution::key16(b"mkt.obs_interval"),
            value: pallet_constitution::ParamValue::U32(10),
        });
        let (payload_hash, payload_len) =
            note_runtime_batch(vec![call]).expect("PARAM payload notes");
        let floor = crate::configs::balance_param(b"prop.bond.param");
        let mut pids = Vec::new();
        for premium in [30_u128, 20, 10] {
            let proposer = account(u8::try_from(100 + pids.len()).unwrap_or(100));
            let held = floor.saturating_add(premium);
            assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, held));
            let pid = pallet_epoch::NextProposalId::<Runtime>::get();
            let mut proposal =
                empty_param_proposal(pid, proposer.clone(), payload_hash, payload_len);
            proposal.bond = held;
            assert_ok!(Epoch::submit(RuntimeOrigin::signed(proposer), proposal));
            assert!(seed_submitted_as_qualified(pid, SPEC).is_some());
            pids.push(pid);
        }

        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        System::set_block_number(
            schedule.epoch_start_block.saturating_add(
                schedule
                    .length
                    .saturating_mul(futarchy_primitives::phase_offsets::SEED_NUM)
                    / futarchy_primitives::phase_offsets::DENOMINATOR,
            ),
        );
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(103)),
            Default::default(),
        ));
        assert!(System::events().iter().any(|record| matches!(
            &record.event,
            crate::RuntimeEvent::Epoch(pallet_epoch::Event::SlotsShrunk {
                requested: 3,
                funded: 2,
                dropped,
                ..
            }) if dropped.as_slice() == [pids[2]]
        )));
        assert_eq!(pallet_epoch::FundedPolSlots::<Runtime>::get().len(), 2);
        assert_eq!(
            pallet_epoch::IntakeProposals::<Runtime>::get(pids[2]).map(|proposal| proposal.state),
            Some(ProposalState::Submitted),
        );
    });
}

#[test]
fn attestation_creation_snapshots_the_live_constitution_window() {
    development_ext().execute_with(|| {
        pallet_epoch::EpochOf::<Runtime>::mutate(|clock| {
            clock.index = clock.index.saturating_add(2)
        });
        let key = pallet_constitution::key16(b"att.window");
        let first_window = 50_000_u32;
        assert_ok!(Constitution::set_param(
            pallet_origins::Origin::FutarchyMeta.into(),
            key,
            pallet_constitution::ParamValue::U32(first_window),
        ));

        let members = [account(51), account(52), account(53)];
        assert_ok!(Attestor::set_members(
            pallet_origins::Origin::ConstitutionalValues.into(),
            members.to_vec(),
        ));
        let submitted_at = System::block_number();
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(members[0].clone()),
            9_101,
            [91; 32],
            [92; 32],
        ));
        let first_deadline = submitted_at.saturating_add(first_window);
        assert_eq!(
            pallet_attestor::Attestations::<Runtime>::get()
                .first()
                .map(|record| record.challenge_deadline),
            Some(first_deadline),
        );

        pallet_epoch::EpochOf::<Runtime>::mutate(|clock| {
            clock.index = clock.index.saturating_add(2)
        });
        let second_window = 60_000_u32;
        assert_ok!(Constitution::set_param(
            pallet_origins::Origin::FutarchyMeta.into(),
            key,
            pallet_constitution::ParamValue::U32(second_window),
        ));
        assert_eq!(
            pallet_attestor::Attestations::<Runtime>::get()
                .first()
                .map(|record| record.challenge_deadline),
            Some(first_deadline),
            "an existing attestation keeps its creation-time deadline",
        );
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(members[1].clone()),
            9_101,
            [91; 32],
            [93; 32],
        ));
        assert_eq!(
            pallet_attestor::Attestations::<Runtime>::get()
                .get(1)
                .map(|record| record.challenge_deadline),
            Some(submitted_at.saturating_add(second_window)),
        );
    });
}

#[test]
fn governed_xcm_trader_rounds_both_weight_dimensions_up_against_the_payer() {
    use crate::configs::ConstitutionTraderRates;
    use frame_support::weights::constants::{WEIGHT_PROOF_SIZE_PER_MB, WEIGHT_REF_TIME_PER_SECOND};

    development_ext().execute_with(|| {
        let context = XcmContext::with_message_id([44; 32]);
        let payment = mock_asset_to_holding(XcmAsset {
            id: XcmAssetId(usdc_location()),
            fun: Fungibility::Fungible(10),
        });
        let mut trader = GovernedWeightTrader::<ConstitutionTraderRates, ()>::new();
        let bought = trader.buy_weight(XcmWeight::from_parts(1, 1), payment, &context);
        assert!(bought.is_ok(), "governed USDC payment must buy weight");

        let rate = ConstitutionTraderRates::usdc_rate();
        let reference_price = rate
            .units_per_second
            .saturating_add(u128::from(WEIGHT_REF_TIME_PER_SECOND).saturating_sub(1))
            / u128::from(WEIGHT_REF_TIME_PER_SECOND);
        let proof_price = rate
            .units_per_megabyte
            .saturating_add(u128::from(WEIGHT_PROOF_SIZE_PER_MB).saturating_sub(1))
            / u128::from(WEIGHT_PROOF_SIZE_PER_MB);
        let charged = reference_price.saturating_add(proof_price);
        assert_eq!(charged, 6);
        if let Ok(surplus) = bought {
            assert_eq!(xcm_holding_amount(&surplus, &usdc_location()), 10 - charged);
        }
    });
}

#[test]
fn phase_inflow_caps_use_real_foreign_asset_issuance_and_live_params() {
    use crate::configs::PhaseInflowCaps;

    development_ext().execute_with(|| {
        // 13 §1 default: phase3.tvl_cap = 2,000,000 USDC (µUSDC, 6 decimals).
        let global_cap = 2_000_000_000_000_u128;
        let issued = global_cap - 100;
        assert!(<ForeignAssets as FungiblesMutate<AccountId>>::mint_into(
            usdc_location(),
            &account(46),
            issued,
        )
        .is_ok());
        assert_eq!(ForeignAssets::total_issuance(usdc_location()), issued);
        assert_ok!(<PhaseInflowCaps as XcmInflowCaps<AccountId>>::usdc_mint_admissible(100));
        assert_eq!(
            <PhaseInflowCaps as XcmInflowCaps<AccountId>>::usdc_mint_admissible(101),
            Err(())
        );

        // The adapter observes the live row on its next read. Phase-gate
        // discipline for cap changes is enforced by the governance layer;
        // its set_param mutability remains the constitution-side SQ follow-up.
        assert_ok!(Constitution::set_param(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::key16(b"phase3.tvl_cap"),
            pallet_constitution::ParamValue::Balance(issued),
        ));
        assert_eq!(
            <PhaseInflowCaps as XcmInflowCaps<AccountId>>::usdc_mint_admissible(1),
            Err(())
        );

        assert_ok!(Constitution::set_param(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::key16(b"phase3.dep_cap"),
            pallet_constitution::ParamValue::Balance(10),
        ));
        let beneficiary = account(47);
        assert_ok!(
            <PhaseInflowCaps as XcmInflowCaps<AccountId>>::note_usdc_inflow(&beneficiary, 10,)
        );
        assert_eq!(
            <PhaseInflowCaps as XcmInflowCaps<AccountId>>::note_usdc_inflow(&beneficiary, 1),
            Err(())
        );
        assert_eq!(
            pallet_inflow_caps::CumulativeDeposits::<Runtime>::get(beneficiary),
            10
        );
    });
}

fn set_balance_param_value(name: &[u8], value: Balance) {
    let key = pallet_constitution::key16(name);
    let Some(mut record) = pallet_constitution::Params::<Runtime>::get(key) else {
        assert!(false, "missing genesis balance param {name:?}");
        return;
    };
    record.value = pallet_constitution::ParamValue::Balance(value);
    pallet_constitution::Params::<Runtime>::insert(key, record);
}

fn local_xcm_account(who: &AccountId) -> staging_xcm::latest::Location {
    staging_xcm::latest::Location::new(
        0,
        [staging_xcm::latest::Junction::AccountId32 {
            network: Some(staging_xcm::latest::NetworkId::Polkadot),
            id: who.clone().into(),
        }],
    )
}

fn production_xcm_weight_limit() -> XcmWeight {
    crate::configs::xcm_config::UnitWeightCost::get().saturating_mul(10)
}

fn execute_production_inbound_usdc(
    amount: Balance,
    beneficiary: &AccountId,
    message_byte: u8,
) -> staging_xcm::latest::Outcome {
    use staging_xcm::latest::prelude::*;

    let incoming = XcmAsset {
        id: XcmAssetId(usdc_location()),
        fun: Fungibility::Fungible(amount),
    };
    let weight_limit = production_xcm_weight_limit();
    let program = Xcm(vec![
        ReserveAssetDeposited(Assets::from(incoming.clone())),
        ClearOrigin,
        BuyExecution {
            fees: incoming,
            weight_limit: Limited(weight_limit),
        },
        DepositAsset {
            assets: Wild(AllCounted(1)),
            beneficiary: local_xcm_account(beneficiary),
        },
    ]);
    let mut message_id = [message_byte; 32];
    <crate::configs::xcm_config::Executor as ExecuteXcm<RuntimeCall>>::prepare_and_execute(
        bleavit_xcm::identity::asset_hub_location(),
        program,
        &mut message_id,
        weight_limit,
        XcmWeight::zero(),
    )
}

fn latest_production_xcm_trap() -> Option<(H256, staging_xcm::latest::Location, VersionedAssets)> {
    System::events().iter().rev().find_map(|record| {
        if let crate::RuntimeEvent::PolkadotXcm(pallet_xcm::Event::AssetsTrapped {
            hash,
            origin,
            assets,
        }) = &record.event
        {
            Some((*hash, origin.clone(), assets.clone()))
        } else {
            None
        }
    })
}

fn create_local_production_xcm_trap(
    origin: &staging_xcm::latest::Location,
    amount: Balance,
    message_byte: u8,
) -> Option<(H256, VersionedAssets)> {
    use staging_xcm_executor::traits::{DropAssets, TransactAsset};

    System::set_block_number(1);
    let asset = XcmAsset {
        id: XcmAssetId(usdc_location()),
        fun: Fungibility::Fungible(amount),
    };
    let context = XcmContext::with_message_id([message_byte; 32]);
    let holding = match <crate::configs::xcm_config::AssetTransactors as TransactAsset>::mint_asset(
        &asset, &context,
    ) {
        Ok(holding) => holding,
        Err(error) => {
            assert!(false, "local trap setup must mint USDC: {error:?}");
            return None;
        }
    };
    <PolkadotXcm as DropAssets>::drop_assets(origin, holding, &context);
    let Some((hash, trapped_origin, assets)) = latest_production_xcm_trap() else {
        assert!(false, "local trap setup must emit AssetsTrapped");
        return None;
    };
    assert_eq!(&trapped_origin, origin);
    Some((hash, assets))
}

#[test]
fn production_xcm_config_binds_capped_assets_reserves_barrier_and_trap_claims() {
    use crate::configs::xcm_config;
    use staging_xcm_executor::{traits::TrapAndClaimAssets, Config as ExecutorConfig};

    assert_same_type::<
        <xcm_config::XcmConfig as ExecutorConfig>::AssetTransactor,
        xcm_config::CappedAssets,
    >();
    assert_same_type::<
        <xcm_config::XcmConfig<xcm_config::TrapRecoveryAssets> as ExecutorConfig>::AssetTransactor,
        xcm_config::TrapRecoveryAssets,
    >();
    assert_same_type::<
        <Runtime as pallet_xcm::Config>::XcmExecutor,
        xcm_config::TrapRecoveryExecutor,
    >();
    assert_same_type::<<xcm_config::XcmConfig as ExecutorConfig>::XcmSender, xcm_config::Router>();
    assert_same_type::<<Runtime as pallet_xcm::Config>::XcmRouter, xcm_config::Router>();
    assert_same_type::<
        <xcm_config::XcmConfig as ExecutorConfig>::IsReserve,
        bleavit_xcm::assets::BleavitReserves,
    >();
    assert_same_type::<<xcm_config::XcmConfig as ExecutorConfig>::IsTeleporter, ()>();
    assert_same_type::<<xcm_config::XcmConfig as ExecutorConfig>::OriginConverter, ()>();
    assert_same_type::<<xcm_config::XcmConfig as ExecutorConfig>::Barrier, xcm_config::Barrier>();
    assert_same_type::<<xcm_config::XcmConfig as ExecutorConfig>::AssetTrap, PolkadotXcm>();
    fn assert_trap_and_claim<T: TrapAndClaimAssets>() {}
    assert_trap_and_claim::<<xcm_config::XcmConfig as ExecutorConfig>::AssetTrap>();
    assert_eq!(
        xcm_config::RelayNetwork::get(),
        Some(staging_xcm::latest::NetworkId::Polkadot)
    );
}

#[test]
fn production_xcm_under_caps_mints_deposits_and_records_the_beneficiary() {
    development_ext().execute_with(|| {
        let beneficiary = account(54);
        let amount = 20 * currency::USDC;
        let issuance_before = ForeignAssets::total_issuance(usdc_location());
        set_balance_param_value(b"phase3.tvl_cap", issuance_before.saturating_add(amount));
        set_balance_param_value(b"phase3.dep_cap", amount);

        assert!(execute_production_inbound_usdc(amount, &beneficiary, 54)
            .ensure_complete()
            .is_ok());
        let credited = ForeignAssets::balance(usdc_location(), &beneficiary);
        assert!(credited > 0);
        assert_eq!(
            pallet_inflow_caps::CumulativeDeposits::<Runtime>::get(&beneficiary),
            credited
        );
        assert!(latest_production_xcm_trap().is_none());
    });
}

#[test]
fn production_xcm_global_cap_refuses_before_minting_or_trapping() {
    // limit-coverage: phase3.tvl_cap
    use staging_xcm::latest::{Error as XcmError, InstructionError, Outcome};

    development_ext().execute_with(|| {
        let beneficiary = account(55);
        let amount = 20 * currency::USDC;
        let issuance_before = ForeignAssets::total_issuance(usdc_location());
        set_balance_param_value(
            b"phase3.tvl_cap",
            issuance_before.saturating_add(amount).saturating_sub(1),
        );
        set_balance_param_value(b"phase3.dep_cap", Balance::MAX);

        let outcome = execute_production_inbound_usdc(amount, &beneficiary, 55);
        assert!(matches!(
            outcome,
            Outcome::Incomplete {
                error: InstructionError {
                    index: 0,
                    error: XcmError::FailedToTransactAsset("USDC global inflow cap exceeded"),
                },
                ..
            }
        ));
        assert_eq!(
            ForeignAssets::total_issuance(usdc_location()),
            issuance_before
        );
        assert_eq!(ForeignAssets::balance(usdc_location(), &beneficiary), 0);
        assert_eq!(
            pallet_inflow_caps::CumulativeDeposits::<Runtime>::get(&beneficiary),
            0
        );
        assert!(latest_production_xcm_trap().is_none());
    });
}

#[test]
fn production_xcm_account_cap_traps_and_asset_hub_origin_can_recover() {
    // limit-coverage: phase3.dep_cap
    use staging_xcm::latest::prelude::*;

    development_ext().execute_with(|| {
        System::set_block_number(1);
        let refused_beneficiary = account(56);
        let recovery_beneficiary = account(57);
        let amount = 20 * currency::USDC;
        let issuance_before = ForeignAssets::total_issuance(usdc_location());
        set_balance_param_value(b"phase3.tvl_cap", issuance_before.saturating_add(amount));
        set_balance_param_value(b"phase3.dep_cap", 1);

        let outcome = execute_production_inbound_usdc(amount, &refused_beneficiary, 56);
        assert!(matches!(
            outcome,
            Outcome::Incomplete {
                error: InstructionError {
                    index: 3,
                    error: XcmError::FailedToTransactAsset("USDC inflow cap exceeded"),
                },
                ..
            }
        ));
        let issuance_after_trap = ForeignAssets::total_issuance(usdc_location());
        assert!(issuance_after_trap > issuance_before);
        assert!(issuance_after_trap <= issuance_before.saturating_add(amount));
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &refused_beneficiary),
            0
        );
        assert_eq!(
            pallet_inflow_caps::CumulativeDeposits::<Runtime>::get(&refused_beneficiary),
            0
        );

        let Some((hash, trap_origin, versioned_assets)) = latest_production_xcm_trap() else {
            assert!(false, "deposit-cap refusal must trap its minted holding");
            return;
        };
        assert_eq!(trap_origin, bleavit_xcm::identity::asset_hub_location());
        assert_eq!(PolkadotXcm::asset_trap(&hash), 1);

        let local_claim = RuntimeCall::PolkadotXcm(pallet_xcm::Call::claim_assets {
            assets: Box::new(versioned_assets.clone()),
            beneficiary: Box::new(VersionedLocation::from(local_xcm_account(
                &recovery_beneficiary,
            ))),
        });
        assert!(RuntimeBaseCallFilter::contains(&local_claim));
        assert!(PolkadotXcm::claim_assets(
            RuntimeOrigin::signed(refused_beneficiary.clone()),
            Box::new(versioned_assets.clone()),
            Box::new(VersionedLocation::from(local_xcm_account(
                &recovery_beneficiary,
            ))),
        )
        .is_err());
        assert_eq!(
            PolkadotXcm::asset_trap(&hash),
            1,
            "a Signed account cannot claim an Asset-Hub-keyed trap"
        );

        let latest_assets: Assets = match versioned_assets.clone().try_into() {
            Ok(assets) => assets,
            Err(()) => {
                assert!(false, "trapped v5 assets must decode as latest");
                return;
            }
        };
        let fee_asset = match latest_assets.inner().first() {
            Some(asset) => asset.clone(),
            None => {
                assert!(false, "the trapped holding must contain USDC");
                return;
            }
        };
        let ticket = Location::new(
            0,
            [GeneralIndex(u128::from(
                versioned_assets.identify_version(),
            ))],
        );
        // Pin the global cap exactly to issuance that already includes the
        // trapped holding. Recovery must not re-apply a prospective mint gate:
        // pallet-xcm's reconstruction is net issuance-neutral.
        set_balance_param_value(b"phase3.tvl_cap", issuance_after_trap);
        set_balance_param_value(b"phase3.dep_cap", amount);
        let weight_limit = production_xcm_weight_limit();
        let recovery = Xcm(vec![
            ClaimAsset {
                assets: latest_assets,
                ticket,
            },
            BuyExecution {
                fees: fee_asset,
                weight_limit: Limited(weight_limit),
            },
            DepositAsset {
                assets: Wild(AllCounted(1)),
                beneficiary: local_xcm_account(&recovery_beneficiary),
            },
        ]);
        let mut message_id = [57; 32];
        let recovery_outcome =
            <crate::configs::xcm_config::Executor as ExecuteXcm<RuntimeCall>>::prepare_and_execute(
                bleavit_xcm::identity::asset_hub_location(),
                recovery,
                &mut message_id,
                weight_limit,
                XcmWeight::zero(),
            );
        assert!(recovery_outcome.ensure_complete().is_ok());
        assert_eq!(PolkadotXcm::asset_trap(&hash), 0);
        let recovered = ForeignAssets::balance(usdc_location(), &recovery_beneficiary);
        assert!(recovered > 0);
        assert_eq!(
            pallet_inflow_caps::CumulativeDeposits::<Runtime>::get(&recovery_beneficiary),
            recovered
        );
        let issuance_after_recovery = ForeignAssets::total_issuance(usdc_location());
        assert!(issuance_after_recovery <= issuance_after_trap);
        assert_eq!(
            issuance_after_recovery,
            issuance_before.saturating_add(recovered),
            "claim reconstruction adds no issuance; only paid recovery fees are removed"
        );
    });
}

#[test]
fn production_xcm_signed_claim_reconstructs_at_the_global_cap_and_records_deposit() {
    development_ext().execute_with(|| {
        let owner = account(58);
        let owner_location = local_xcm_account(&owner);
        let amount = 20 * currency::USDC;
        let issuance_before = ForeignAssets::total_issuance(usdc_location());
        let Some((hash, assets)) = create_local_production_xcm_trap(&owner_location, amount, 58)
        else {
            return;
        };
        let issuance_with_trap = ForeignAssets::total_issuance(usdc_location());
        assert_eq!(issuance_with_trap, issuance_before.saturating_add(amount));
        set_balance_param_value(b"phase3.tvl_cap", issuance_with_trap);
        set_balance_param_value(b"phase3.dep_cap", amount);

        assert_ok!(PolkadotXcm::claim_assets(
            RuntimeOrigin::signed(owner.clone()),
            Box::new(assets),
            Box::new(VersionedLocation::from(owner_location)),
        ));
        assert_eq!(PolkadotXcm::asset_trap(&hash), 0);
        assert_eq!(ForeignAssets::balance(usdc_location(), &owner), amount);
        assert_eq!(
            pallet_inflow_caps::CumulativeDeposits::<Runtime>::get(&owner),
            amount
        );
        assert_eq!(
            ForeignAssets::total_issuance(usdc_location()),
            issuance_with_trap
        );
    });
}

#[test]
fn production_xcm_signed_claim_over_account_cap_retraps_without_recording() {
    development_ext().execute_with(|| {
        let owner = account(59);
        let owner_location = local_xcm_account(&owner);
        let amount = 20 * currency::USDC;
        let Some((hash, assets)) = create_local_production_xcm_trap(&owner_location, amount, 59)
        else {
            return;
        };
        let issuance_with_trap = ForeignAssets::total_issuance(usdc_location());
        set_balance_param_value(b"phase3.tvl_cap", issuance_with_trap);
        set_balance_param_value(b"phase3.dep_cap", amount.saturating_sub(1));

        assert!(PolkadotXcm::claim_assets(
            RuntimeOrigin::signed(owner.clone()),
            Box::new(assets),
            Box::new(VersionedLocation::from(owner_location)),
        )
        .is_err());
        assert_eq!(
            PolkadotXcm::asset_trap(&hash),
            1,
            "a refused local recovery must remain trapped"
        );
        assert_eq!(ForeignAssets::balance(usdc_location(), &owner), 0);
        assert_eq!(
            pallet_inflow_caps::CumulativeDeposits::<Runtime>::get(&owner),
            0
        );
        assert_eq!(
            ForeignAssets::total_issuance(usdc_location()),
            issuance_with_trap
        );
    });
}

#[test]
fn production_xcm_treasury_class_can_recover_only_the_protocol_keyed_trap() {
    use pallet_execution_guard::BatchDispatcher;

    development_ext().execute_with(|| {
        let protocol = crate::configs::treasury_protocol_account();
        let protocol_location = local_xcm_account(&protocol);
        let amount = 20 * currency::USDC;
        let Some((hash, assets)) = create_local_production_xcm_trap(&protocol_location, amount, 60)
        else {
            return;
        };
        let issuance_with_trap = ForeignAssets::total_issuance(usdc_location());
        set_balance_param_value(b"phase3.tvl_cap", issuance_with_trap);
        set_balance_param_value(b"phase3.dep_cap", amount);

        let claim = RuntimeCall::PolkadotXcm(pallet_xcm::Call::claim_assets {
            assets: Box::new(assets),
            beneficiary: Box::new(VersionedLocation::from(protocol_location)),
        });
        assert!(RuntimeBaseCallFilter::contains_for(
            ClassOrigin::FutarchyTreasury,
            &claim
        ));
        assert_ok!(RuntimeDispatcher::dispatch_with_class_origin(
            claim,
            ProposalClass::Treasury,
        ));
        assert_eq!(PolkadotXcm::asset_trap(&hash), 0);
        assert_eq!(ForeignAssets::balance(usdc_location(), &protocol), amount);
        assert_eq!(
            pallet_inflow_caps::CumulativeDeposits::<Runtime>::get(&protocol),
            amount
        );
        assert_eq!(
            ForeignAssets::total_issuance(usdc_location()),
            issuance_with_trap
        );
    });
}

#[test]
fn development_preset_builds_and_pins_usdc_and_para_identity() {
    development_ext().execute_with(|| {
        assert_eq!(
            u32::from(ParachainInfo::parachain_id()),
            chain_identity::FIXTURE_PARA_ID
        );
        assert!(ForeignAssets::asset_exists(usdc_location()));
        assert_eq!(
            ForeignAssets::minimum_balance(usdc_location()),
            currency::USDC_CENT
        );
        let details =
            pallet_assets::Asset::<Runtime, pallet_assets::Instance1>::get(usdc_location());
        assert!(details.is_some_and(|asset| asset.is_sufficient));
        let metadata =
            pallet_assets::Metadata::<Runtime, pallet_assets::Instance1>::get(usdc_location());
        assert_eq!(metadata.decimals, currency::USDC_DECIMALS);
        assert_eq!(
            Balances::minimum_balance(),
            currency::VIT_EXISTENTIAL_DEPOSIT
        );
        assert_eq!(Balances::total_issuance(), currency::VIT_TOTAL_SUPPLY);
    });
}

#[test]
fn usdc_storage_keys_match_the_frozen_surface_manifest() {
    fn storage_key(item: &[u8], encoded_location: &[u8]) -> Vec<u8> {
        let mut key = Vec::with_capacity(64 + 16 + encoded_location.len());
        key.extend_from_slice(&sp_io::hashing::twox_128(b"ForeignAssets"));
        key.extend_from_slice(&sp_io::hashing::twox_128(item));
        key.extend_from_slice(&sp_io::hashing::blake2_128(encoded_location));
        key.extend_from_slice(encoded_location);
        key
    }

    let encoded_location = usdc_location().encode();
    let asset_key = storage_key(b"Asset", &encoded_location);
    let metadata_key = storage_key(b"Metadata", &encoded_location);

    assert_eq!(
        format!("0x{}", sp_core::hexdisplay::HexDisplay::from(&asset_key)),
        "0x30e64a56026f4b5e3c2d196283a9a17dd34371a193a751eea5883e9553457b2e484550ecc01d89e5e7bb33be1915aaef010300a10f043205e514"
    );
    assert_eq!(
        format!("0x{}", sp_core::hexdisplay::HexDisplay::from(&metadata_key)),
        "0x30e64a56026f4b5e3c2d196283a9a17db5f3822e35ca2f31ce3526eab1363fd2484550ecc01d89e5e7bb33be1915aaef010300a10f043205e514"
    );

    development_ext().execute_with(|| {
        assert_eq!(
            pallet_assets::Asset::<Runtime, pallet_assets::Instance1>::hashed_key_for(
                usdc_location()
            ),
            asset_key
        );
        assert_eq!(
            pallet_assets::Metadata::<Runtime, pallet_assets::Instance1>::hashed_key_for(
                usdc_location()
            ),
            metadata_key
        );
        assert!(ForeignAssets::asset_exists(usdc_location()));
    });
}

#[test]
fn development_genesis_builds_epoch_from_valid_live_constitution_params() {
    development_ext().execute_with(|| {
        let params =
            <<Runtime as pallet_epoch::Config>::Params as pallet_epoch::EpochParamsProvider>::get();
        assert!(params.validate().is_ok());
        assert_eq!(
            pallet_epoch::EpochOf::<Runtime>::get(),
            pallet_epoch::EpochInfo {
                index: 1,
                phase: futarchy_primitives::EpochPhase::Intake,
                phase_start_block: 0,
            },
        );
        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        assert_eq!(schedule.epoch_start_block, 0);
        assert_eq!(schedule.length, params.epoch_length);
        assert_eq!(schedule.next_length, params.epoch_length);
        assert_eq!(pallet_epoch::NextProposalId::<Runtime>::get(), 1);
    });
}

#[test]
fn live_epoch_clock_fans_out_to_all_four_sibling_pallets() {
    development_ext().execute_with(|| {
        let initial = pallet_epoch::EpochOf::<Runtime>::get().index;
        let epoch_length = pallet_epoch::Schedule::<Runtime>::get().length;
        System::set_block_number(epoch_length);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(76)),
            Default::default(),
        ));
        let live = pallet_epoch::EpochOf::<Runtime>::get().index;
        assert_eq!(live, initial.saturating_add(1));
        assert_eq!(
            <<Runtime as pallet_constitution::Config>::CurrentEpoch as Get<
                futarchy_primitives::EpochId,
            >>::get(),
            live,
        );
        assert_eq!(
            <<Runtime as pallet_welfare::Config>::CurrentEpoch as Get<
                futarchy_primitives::EpochId,
            >>::get(),
            live,
        );
        assert_eq!(
            <<Runtime as pallet_futarchy_treasury::Config>::CurrentEpoch as Get<
                futarchy_primitives::EpochId,
            >>::get(),
            live,
        );
        assert_eq!(
            <<Runtime as pallet_guardian::Config>::CurrentEpoch as Get<
                futarchy_primitives::EpochId,
            >>::get(),
            live,
        );
    });
}

#[test]
fn relay_gap_4_799_does_not_engage_the_dead_man_switch() {
    development_ext().execute_with(|| {
        System::set_block_number(1);
        submit_relay_parent(1);
        System::set_block_number(2);
        submit_relay_parent(
            1_u32
                .saturating_add(kernel::DEAD_MAN_RELAY_BLOCKS)
                .saturating_sub(1),
        );
        assert_eq!(
            Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED,
            0,
        );
        assert_eq!(
            pallet_epoch::LastRelayParent::<Runtime>::get(),
            Some(kernel::DEAD_MAN_RELAY_BLOCKS)
        );
    });
}

#[test]
fn relay_gap_4_800_latches_until_one_full_proposal_free_recovery_epoch() {
    use pallet_execution_guard::BatchDispatcher;

    development_ext().execute_with(|| {
        System::set_block_number(1);
        submit_relay_parent(1);
        let frozen = pallet_epoch::EpochOf::<Runtime>::get();

        System::set_block_number(2);
        submit_relay_parent(1_u32.saturating_add(kernel::DEAD_MAN_RELAY_BLOCKS));
        let batch_all = RuntimeCall::Utility(pallet_utility::Call::batch_all {
            calls: vec![RuntimeCall::Constitution(
                pallet_constitution::Call::set_param {
                    key: pallet_constitution::key16(b"mkt.obs_interval"),
                    value: pallet_constitution::ParamValue::U32(10),
                },
            )],
        });
        assert_ne!(
            Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED,
            0,
        );
        assert!(RuntimeDispatcher::rederive_call(&batch_all).is_ok());
        assert!(!RuntimeDispatcher::safety_filter(
            ProposalClass::Param,
            &batch_all
        ));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(77)),
            Default::default(),
        ));
        assert_eq!(pallet_epoch::EpochOf::<Runtime>::get(), frozen);
        assert_eq!(pallet_epoch::DeadMan::<Runtime>::get().paused_at, Some(2));

        // A normal next relay parent heals the detector cause, not the latch.
        System::set_block_number(3);
        submit_relay_parent(2_u32.saturating_add(kernel::DEAD_MAN_RELAY_BLOCKS));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(77)),
            Default::default(),
        ));
        assert_ne!(
            Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED,
            0,
        );
        assert_eq!(pallet_epoch::DeadMan::<Runtime>::get().paused_at, None);
        assert_eq!(
            pallet_epoch::DeadMan::<Runtime>::get().recovery_epoch,
            Some(frozen.index.saturating_add(1)),
        );
        let proposer = account(78);
        let bond = crate::configs::balance_param(b"prop.bond.param");
        assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, bond));
        let (payload_hash, payload_len) = match note_runtime_batch(Vec::new()) {
            Some(payload) => payload,
            None => {
                assert!(false, "empty runtime batch must encode");
                return;
            }
        };
        assert_noop!(
            Epoch::submit(
                RuntimeOrigin::signed(proposer.clone()),
                empty_param_proposal(
                    pallet_epoch::NextProposalId::<Runtime>::get(),
                    proposer,
                    payload_hash,
                    payload_len,
                ),
            ),
            pallet_epoch::Error::<Runtime>::BadPhase
        );

        let recovery_start = pallet_epoch::Schedule::<Runtime>::get().epoch_start_block;
        let recovery_length = pallet_epoch::Schedule::<Runtime>::get().length;
        System::set_block_number(recovery_start.saturating_add(recovery_length));
        submit_relay_parent(3_u32.saturating_add(kernel::DEAD_MAN_RELAY_BLOCKS));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(77)),
            Default::default(),
        ));
        assert_eq!(pallet_epoch::DeadMan::<Runtime>::get().recovery_epoch, None);
        assert_eq!(
            Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED,
            0,
        );
        assert!(RuntimeDispatcher::rederive_call(&batch_all).is_ok());
    });
}

#[test]
fn relay_pause_suppresses_blocked_snapshot_overdue_and_recovery_clears_freeze() {
    const SPEC: futarchy_primitives::MetricSpecVersion = 42;

    development_ext().execute_with(|| {
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        assert!(install_active_x_snapshot_spec(SPEC, epoch).is_some());
        let due = Epoch::scheduled_epoch_end(epoch).expect("current epoch is scheduled");
        let overdue = due
            .saturating_add(kernel::DEAD_MAN_SNAPSHOT_OVERDUE_BLOCKS)
            .saturating_add(10);

        System::set_block_number(1);
        submit_relay_parent(1);
        System::set_block_number(2);
        submit_relay_parent(1_u32.saturating_add(kernel::DEAD_MAN_RELAY_BLOCKS));
        assert_eq!(pallet_epoch::DeadMan::<Runtime>::get().paused_at, Some(2));

        // The raw schedule deadline is far overdue, but this epoch cannot
        // legally close or record its snapshot while the clock is paused.
        System::set_block_number(overdue);
        submit_relay_parent(2_u32.saturating_add(kernel::DEAD_MAN_RELAY_BLOCKS));
        assert_eq!(pallet_epoch::DeadManDetector::<Runtime>::get().causes, 0);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(77)),
            Default::default(),
        ));
        assert_eq!(pallet_epoch::DeadMan::<Runtime>::get().paused_at, None);
        assert!(pallet_epoch::DeadMan::<Runtime>::get()
            .recovery_epoch
            .is_some());
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(account(77)),
            epoch,
            SPEC,
        ));

        let recovery = pallet_epoch::Schedule::<Runtime>::get();
        System::set_block_number(recovery.epoch_start_block.saturating_add(recovery.length));
        submit_relay_parent(3_u32.saturating_add(kernel::DEAD_MAN_RELAY_BLOCKS));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(77)),
            Default::default(),
        ));
        assert_eq!(pallet_epoch::DeadManDetector::<Runtime>::get().causes, 0);
        assert_eq!(pallet_epoch::DeadMan::<Runtime>::get().recovery_epoch, None);
        assert_eq!(
            Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED,
            0,
        );
    });
}

#[test]
fn pause_across_decision_boundary_resumes_and_decides_at_shifted_window_end() {
    use pallet_epoch::{EpochParamsProvider, MarketAccess};

    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_018;
        let params = <crate::configs::RuntimeEpochParams as EpochParamsProvider>::get();
        let decision_b = crate::configs::balance_param(b"pol.b.param");
        let gate_b = crate::configs::balance_param(b"pol.b_gate");
        let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
        let decision_headroom =
            pallet_market::core_market::seed_headroom(decision_b).expect("bounded decision b");
        let gate_headroom =
            pallet_market::core_market::seed_headroom(gate_b).expect("bounded gate b");
        let baseline_headroom =
            pallet_market::core_market::seed_headroom(baseline_b).expect("bounded baseline b");
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &crate::configs::pol_account(),
            decision_headroom
                .saturating_add(gate_headroom.saturating_mul(2))
                .saturating_add(currency::USDC),
        ));
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &crate::configs::pol_baseline_account(),
            baseline_headroom.saturating_add(currency::USDC),
        ));
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = decision_headroom
                .saturating_mul(4)
                .saturating_add(gate_headroom.saturating_mul(8))
                .saturating_add(baseline_headroom.saturating_mul(2));
        });

        let old_end = System::block_number().saturating_add(params.decision_window);
        let mut proposal = empty_param_proposal(PID, account(155), H256::zero(), 0);
        proposal.metric_spec = 1;
        proposal.state = ProposalState::Trading;
        proposal.decide_at = old_end;
        let plan = <crate::configs::RuntimePolBudget as pallet_epoch::PolBudget<AccountId>>::proposal_seed_plan(&proposal)
            .expect("PARAM seed plan");
        let markets = <crate::configs::RuntimeMarketAccess as MarketAccess<AccountId>>::open_markets(
            &proposal,
            false,
            Some(plan),
        )
        .expect("market set opens");
        proposal.markets = Some(markets);
        pallet_epoch::Proposals::<Runtime>::insert(PID, proposal.clone());
        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        pallet_epoch::ProposalSchedules::<Runtime>::insert(
            PID,
            pallet_epoch::ProposalSchedule {
                epoch: proposal.epoch,
                epoch_start_block: schedule.epoch_start_block,
                epoch_length: schedule.length,
                decide_at: old_end,
                metric_spec: proposal.metric_spec,
            },
        );
        pallet_epoch::NextProposalId::<Runtime>::mutate(|next| {
            *next = (*next).max(PID.saturating_add(1));
        });

        System::set_block_number(1);
        submit_relay_parent(1);
        System::set_block_number(old_end.saturating_sub(2));
        submit_relay_parent(1_u32.saturating_add(kernel::DEAD_MAN_RELAY_BLOCKS));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(155)),
            Default::default(),
        ));
        assert_eq!(
            pallet_epoch::DeadMan::<Runtime>::get().paused_at,
            Some(old_end.saturating_sub(2)),
        );

        System::set_block_number(old_end.saturating_add(2));
        submit_relay_parent(2_u32.saturating_add(kernel::DEAD_MAN_RELAY_BLOCKS));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(155)),
            Default::default(),
        ));
        let resumed = pallet_epoch::Proposals::<Runtime>::get(PID).expect("proposal resumes");
        let shifted_end = old_end.saturating_add(4);
        assert_eq!(resumed.decide_at, shifted_end);
        for id in [markets.accept, markets.reject, markets.baseline] {
            assert!(pallet_market::DecisionWindowOwners::<Runtime>::get(id)
                .iter()
                .any(|owner| owner.0 == PID && owner.3 == shifted_end));
            assert!(pallet_market::DecisionWindows::<Runtime>::get(id)
                .iter()
                .any(|window| window.end == shifted_end && !window.sealed));
        }

        // Decisions remain status-quo blocked for the mandated proposal-free
        // recovery epoch. Once it completes, the proposal must still seal the
        // exact shifted boundary rather than wedging on the stale old end.
        let recovery = pallet_epoch::Schedule::<Runtime>::get();
        System::set_block_number(recovery.epoch_start_block.saturating_add(recovery.length));
        submit_relay_parent(3_u32.saturating_add(kernel::DEAD_MAN_RELAY_BLOCKS));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(155)),
            Default::default(),
        ));
        assert_eq!(pallet_epoch::DeadMan::<Runtime>::get().recovery_epoch, None);
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(account(155)), PID));
        let decided = pallet_epoch::Proposals::<Runtime>::get(PID).expect("proposal remains live");
        assert_eq!(decided.state, ProposalState::Measuring);
        assert!(matches!(decided.decision, Some(DecisionOutcome::Reject(_))));
        for id in [markets.accept, markets.reject, markets.baseline] {
            assert!(pallet_market::DecisionWindows::<Runtime>::get(id).is_empty());
            assert_eq!(
                pallet_market::Markets::<Runtime>::get(id).map(|book| book.phase),
                Some(pallet_market::core_market::MarketPhase::Closed),
            );
        }
    });
}

#[test]
fn snapshot_overdue_boundary_engages_and_a_due_snapshot_resets_the_marker() {
    const SPEC: futarchy_primitives::MetricSpecVersion = 41;

    development_ext().execute_with(|| {
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        assert!(install_active_x_snapshot_spec(SPEC, epoch).is_some());
        let Some(due) = Epoch::scheduled_epoch_end(epoch) else {
            assert!(false, "current epoch end must be scheduled");
            return;
        };
        let Some(boundary) = due.checked_add(kernel::DEAD_MAN_SNAPSHOT_OVERDUE_BLOCKS) else {
            assert!(false, "snapshot boundary must fit");
            return;
        };

        System::set_block_number(boundary);
        submit_relay_parent(1);
        assert_eq!(
            Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED,
            0,
        );
        System::set_block_number(boundary.saturating_add(1));
        submit_relay_parent(2);
        assert_ne!(
            Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED,
            0,
        );
    });

    development_ext().execute_with(|| {
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        assert!(install_active_x_snapshot_spec(SPEC, epoch).is_some());
        let Some(due) = Epoch::scheduled_epoch_end(epoch) else {
            assert!(false, "current epoch end must be scheduled");
            return;
        };
        System::set_block_number(due);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(79)),
            Default::default(),
        ));
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(account(79)),
            epoch,
            SPEC,
        ));
        let Some(progress) = pallet_welfare::SnapshotDeadline::<Runtime>::get() else {
            assert!(
                false,
                "successful due snapshot must retain the next deadline"
            );
            return;
        };
        assert_eq!(progress.last_snapshot_epoch, Some(epoch));
        assert_eq!(progress.due_epoch, epoch.saturating_add(1));

        System::set_block_number(
            due.saturating_add(kernel::DEAD_MAN_SNAPSHOT_OVERDUE_BLOCKS)
                .saturating_add(1),
        );
        submit_relay_parent(1);
        assert_eq!(
            Constitution::phase_flags() & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED,
            0,
        );
    });
}

#[test]
fn development_allocations_match_the_genesis_economics_exactly() {
    use crate::genesis::{
        community_account, incentives_account, treasury_account, ALICE_PUBLIC, BOB_PUBLIC,
        CHARLIE_PUBLIC, COMMUNITY_DISTRIBUTION, DAVE_PUBLIC, ECOSYSTEM_OPS, ECOSYSTEM_OPS_ACCOUNT,
        FOUNDING_TEAM, FOUNDING_TEAM_ACCOUNT, INCENTIVE_PROGRAMS, TREASURY_RESERVE,
    };

    assert_eq!(
        TREASURY_RESERVE
            + COMMUNITY_DISTRIBUTION
            + FOUNDING_TEAM
            + ECOSYSTEM_OPS
            + INCENTIVE_PROGRAMS,
        currency::VIT_TOTAL_SUPPLY
    );

    development_ext().execute_with(|| {
        assert_eq!(Balances::free_balance(treasury_account()), TREASURY_RESERVE);
        assert_eq!(
            Balances::free_balance(community_account()),
            COMMUNITY_DISTRIBUTION
        );
        assert_eq!(
            Balances::free_balance(incentives_account()),
            INCENTIVE_PROGRAMS
        );
        for public in [CHARLIE_PUBLIC, DAVE_PUBLIC] {
            assert_eq!(
                Balances::free_balance(AccountId::new(public)),
                FOUNDING_TEAM_ACCOUNT
            );
        }
        for public in [ALICE_PUBLIC, BOB_PUBLIC] {
            assert_eq!(
                Balances::free_balance(AccountId::new(public)),
                ECOSYSTEM_OPS_ACCOUNT
            );
        }
        assert_eq!(Balances::total_issuance(), currency::VIT_TOTAL_SUPPLY);
    });
}

#[test]
fn treasury_rebate_payout_moves_real_usdc_from_the_selected_pot() {
    use crate::configs::{treasury_keeper_account, treasury_oracle_account, TreasuryRebatePayout};
    use pallet_futarchy_treasury::{PayoutLine, RebatePayout, TreasuryParams as _};

    development_ext().execute_with(|| {
        // `keeper.rebate` is deliberately unseeded until B5 calibration.
        assert_eq!(crate::configs::TreasuryParams::keeper_rebate(), 0);
        assert_eq!(
            crate::configs::TreasuryParams::keeper_budget_epoch(),
            12_000 * currency::USDC
        );

        let keeper = account(77);
        let keeper_pot = treasury_keeper_account();
        let oracle_pot = treasury_oracle_account();
        let amount = 10 * currency::USDC;
        let retained = currency::USDC_CENT;
        assert!(<ForeignAssets as FungiblesMutate<AccountId>>::mint_into(
            usdc_location(),
            &keeper_pot,
            amount + retained,
        )
        .is_ok());
        assert!(<ForeignAssets as FungiblesMutate<AccountId>>::mint_into(
            usdc_location(),
            &oracle_pot,
            amount + retained,
        )
        .is_ok());
        assert_eq!(
            TreasuryRebatePayout::pot_balance(PayoutLine::Keeper),
            amount + retained
        );
        assert_eq!(
            TreasuryRebatePayout::pot_balance(PayoutLine::Oracle),
            amount + retained
        );

        assert!(<TreasuryRebatePayout as RebatePayout<AccountId>>::pay(
            &keeper,
            amount,
            PayoutLine::Keeper,
        )
        .is_ok());
        assert_eq!(ForeignAssets::balance(usdc_location(), &keeper), amount);
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &keeper_pot),
            retained
        );

        assert!(<TreasuryRebatePayout as RebatePayout<AccountId>>::pay(
            &keeper,
            amount,
            PayoutLine::Oracle,
        )
        .is_ok());
        assert_eq!(ForeignAssets::balance(usdc_location(), &keeper), 2 * amount);
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &oracle_pot),
            retained
        );
    });
}

#[test]
fn treasury_keeper_line_funding_moves_matching_real_usdc_into_the_pot() {
    use crate::{configs::treasury_keeper_account, genesis::treasury_account};
    use pallet_futarchy_treasury::BudgetLine;

    development_ext().execute_with(|| {
        let main = treasury_account();
        let keeper_pot = treasury_keeper_account();
        let amount = 50 * currency::USDC;
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = amount;
        });
        assert!(<ForeignAssets as FungiblesMutate<AccountId>>::mint_into(
            usdc_location(),
            &main,
            amount,
        )
        .is_ok());
        let main_before = ForeignAssets::balance(usdc_location(), &main);

        assert_ok!(FutarchyTreasury::fund_budget_line(
            pallet_origins::Origin::FutarchyTreasury.into(),
            BudgetLine::Keeper,
            amount,
        ));

        assert_eq!(FutarchyTreasury::line_balance(BudgetLine::Keeper), amount);
        assert_eq!(ForeignAssets::balance(usdc_location(), &keeper_pot), amount);
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &main),
            main_before - amount
        );
        assert_eq!(FutarchyTreasury::treasury().main_usdc, 0);
    });
}

#[test]
fn treasury_custody_sync_cannot_sweep_or_double_count_epoch_bond_escrow() {
    use crate::{
        configs::{epoch_account, treasury_keeper_account},
        genesis::treasury_account,
    };
    use pallet_futarchy_treasury::BudgetLine;

    development_ext().execute_with(|| {
        let proposer = account(194);
        let epoch_escrow = epoch_account();
        let treasury_main = treasury_account();
        let keeper_pot = treasury_keeper_account();
        assert_ne!(epoch_escrow, treasury_main);
        assert_ne!(epoch_escrow, keeper_pot);

        let batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(Vec::new()) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "empty bounded payload must encode");
                    return;
                }
            };
        let bytes = batch.encode();
        let payload_len = match u32::try_from(bytes.len()) {
            Ok(len) => len,
            Err(_) => {
                assert!(false, "bounded payload length must fit u32");
                return;
            }
        };
        let payload_hash = match <Preimage as StorePreimage>::note(bytes.into()) {
            Ok(hash) => hash,
            Err(error) => {
                assert!(false, "payload preimage must be noted: {error:?}");
                return;
            }
        };
        let bond = crate::configs::balance_param(b"prop.bond.param");
        let funding = 50 * currency::USDC;
        assert!(bond > 0);
        assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, bond,));
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &treasury_main,
            funding,
        ));
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = funding;
        });

        let pid = pallet_epoch::NextProposalId::<Runtime>::get();
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(proposer.clone()),
            empty_param_proposal(pid, proposer, payload_hash, payload_len),
        ));
        let escrow_before = ForeignAssets::balance(usdc_location(), &epoch_escrow);
        let issuance_before = ForeignAssets::total_issuance(usdc_location());
        let deposits_before =
            pallet_inflow_caps::CumulativeDeposits::<Runtime>::iter().collect::<Vec<_>>();
        assert_eq!(escrow_before, bond);
        assert_eq!(
            pallet_epoch::ProposalBonds::<Runtime>::get(pid).map(|entry| entry.held),
            Some(bond),
        );

        assert_ok!(FutarchyTreasury::fund_budget_line(
            pallet_origins::Origin::FutarchyTreasury.into(),
            BudgetLine::Keeper,
            funding,
        ));

        assert_eq!(
            ForeignAssets::balance(usdc_location(), &epoch_escrow),
            escrow_before,
            "MAIN→KEEPER custody sync must not touch the epoch sovereign escrow",
        );
        assert_eq!(
            pallet_epoch::ProposalBonds::<Runtime>::get(pid).map(|entry| entry.held),
            Some(bond),
            "an internal treasury transfer must not mutate epoch liabilities",
        );
        assert_eq!(
            ForeignAssets::total_issuance(usdc_location()),
            issuance_before,
            "custody sync is a transfer, not a mint or burn",
        );
        assert_eq!(
            pallet_inflow_caps::CumulativeDeposits::<Runtime>::iter().collect::<Vec<_>>(),
            deposits_before,
            "local custody transfers do not traverse the XCM inflow meter",
        );
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &keeper_pot),
            funding
        );
        assert!(Epoch::do_try_state().is_ok());
        assert!(FutarchyTreasury::do_try_state().is_ok());
    });
}

#[test]
fn treasury_pot_funding_failure_rolls_back_internal_and_asset_state() {
    use crate::{configs::treasury_keeper_account, genesis::treasury_account};
    use pallet_futarchy_treasury::BudgetLine;

    development_ext().execute_with(|| {
        let main = treasury_account();
        let keeper_pot = treasury_keeper_account();
        let amount = 50 * currency::USDC;
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = amount;
        });
        let state_before = pallet_futarchy_treasury::State::<Runtime>::get();
        let main_before = ForeignAssets::balance(usdc_location(), &main);
        let pot_before = ForeignAssets::balance(usdc_location(), &keeper_pot);
        let issuance_before = ForeignAssets::total_issuance(usdc_location());

        assert!(FutarchyTreasury::fund_budget_line(
            pallet_origins::Origin::FutarchyTreasury.into(),
            BudgetLine::Keeper,
            amount,
        )
        .is_err());

        assert_eq!(
            pallet_futarchy_treasury::State::<Runtime>::get(),
            state_before
        );
        assert_eq!(ForeignAssets::balance(usdc_location(), &main), main_before);
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &keeper_pot),
            pot_before
        );
        assert_eq!(
            ForeignAssets::total_issuance(usdc_location()),
            issuance_before
        );
    });
}

#[test]
fn treasury_non_pot_line_funding_does_not_move_foreign_assets() {
    use crate::{configs::treasury_keeper_account, genesis::treasury_account};
    use pallet_futarchy_treasury::BudgetLine;

    development_ext().execute_with(|| {
        let main = treasury_account();
        let keeper_pot = treasury_keeper_account();
        let amount = 25 * currency::USDC;
        let retained = currency::USDC;
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = amount;
        });
        assert!(<ForeignAssets as FungiblesMutate<AccountId>>::mint_into(
            usdc_location(),
            &main,
            amount + retained,
        )
        .is_ok());
        let main_before = ForeignAssets::balance(usdc_location(), &main);
        let pot_before = ForeignAssets::balance(usdc_location(), &keeper_pot);
        let issuance_before = ForeignAssets::total_issuance(usdc_location());

        assert_ok!(FutarchyTreasury::fund_budget_line(
            pallet_origins::Origin::FutarchyTreasury.into(),
            BudgetLine::Pol,
            amount,
        ));

        assert_eq!(FutarchyTreasury::line_balance(BudgetLine::Pol), amount);
        assert_eq!(ForeignAssets::balance(usdc_location(), &main), main_before);
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &keeper_pot),
            pot_before
        );
        assert_eq!(
            ForeignAssets::total_issuance(usdc_location()),
            issuance_before
        );
    });
}

#[test]
fn coretime_authority_quote_and_keeper_execution_use_live_conversion() {
    use crate::configs::{balance_param, take_test_coretime_renewals, RuntimeCapabilities};
    use pallet_execution_guard::{BatchDispatcher, Capabilities};
    use pallet_futarchy_treasury::BudgetLine;

    development_ext().execute_with(|| {
        let authority = account(205);
        let renewal_account = [206; 32];
        let line_funding = 10 * currency::USDC;
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = line_funding;
        });
        assert_ok!(FutarchyTreasury::fund_budget_line(
            pallet_origins::Origin::FutarchyTreasury.into(),
            BudgetLine::OpsCoretime,
            line_funding,
        ));

        let set_authority =
            RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::set_coretime_authority {
                quote_authority: authority.clone(),
                renewal_account,
            });
        assert!(RuntimeCapabilities::call_enabled(
            ProposalClass::Treasury,
            &set_authority,
        ));
        assert_ok!(RuntimeDispatcher::dispatch_with_class_origin(
            set_authority,
            ProposalClass::Treasury,
        ));

        let period_index = 77;
        let price = 1_u128;
        assert_ok!(FutarchyTreasury::note_coretime_quote(
            RuntimeOrigin::signed(authority),
            period_index,
            price,
        ));
        let fee = balance_param(b"ops.ct_fee_dot");
        let rate = balance_param(b"ops.ct_dot_rate");
        let numerator = match price
            .checked_add(fee)
            .and_then(|total| total.checked_mul(rate))
        {
            Some(value) => value,
            None => {
                assert!(false, "bounded genesis Coretime conversion must fit");
                return;
            }
        };
        let dot = 10_000_000_000_u128;
        let converted = numerator / dot + u128::from(numerator % dot != 0);
        let before = FutarchyTreasury::line_balance(BudgetLine::OpsCoretime);
        let _ = take_test_coretime_renewals();

        assert_ok!(FutarchyTreasury::execute_coretime_renewal(
            RuntimeOrigin::signed(account(207)),
            period_index,
        ));
        assert_eq!(
            FutarchyTreasury::line_balance(BudgetLine::OpsCoretime),
            before - converted,
        );
        assert_eq!(
            take_test_coretime_renewals(),
            vec![(period_index, price)],
            "the runtime test dispatcher exercises the XCM dispatch seam",
        );
        assert!(FutarchyTreasury::treasury()
            .funded_coretime_periods
            .contains(&period_index));
    });
}

fn assert_coretime_liveness_calls_dispatch_during(
    period_index: u32,
    activate_degraded_state: impl FnOnce(),
) {
    use crate::configs::take_test_coretime_renewals;
    use pallet_futarchy_treasury::BudgetLine;

    let authority = AccountId::new(crate::genesis::ALICE_PUBLIC);
    let line_funding = 10 * currency::USDC;
    pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
        state.main_usdc = line_funding;
    });
    assert_ok!(FutarchyTreasury::fund_budget_line(
        pallet_origins::Origin::FutarchyTreasury.into(),
        BudgetLine::OpsCoretime,
        line_funding,
    ));

    activate_degraded_state();

    let note = RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::note_coretime_quote {
        period_index,
        price: 1,
    });
    let prune =
        RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::prune_coretime_quote {
            period_index,
        });
    let execute =
        RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::execute_coretime_renewal {
            period_index,
        });
    assert!(RuntimeBaseCallFilter::contains(&note));
    assert!(RuntimeBaseCallFilter::contains(&prune));
    assert!(RuntimeBaseCallFilter::contains(&execute));

    assert_ok!(note.dispatch(RuntimeOrigin::signed(authority.clone())));
    assert_ok!(prune.dispatch(RuntimeOrigin::signed(authority.clone())));
    assert_ok!(RuntimeCall::FutarchyTreasury(
        pallet_futarchy_treasury::Call::note_coretime_quote {
            period_index,
            price: 1,
        }
    )
    .dispatch(RuntimeOrigin::signed(authority)));
    let _ = take_test_coretime_renewals();
    assert_ok!(execute.dispatch(RuntimeOrigin::signed(account(208))));
    assert_eq!(take_test_coretime_renewals(), vec![(period_index, 1)]);
}

#[test]
fn coretime_liveness_calls_dispatch_while_dead_man_freeze_is_active() {
    development_ext().execute_with(|| {
        assert_coretime_liveness_calls_dispatch_during(88, || {
            assert_ok!(Constitution::note_dead_man_engaged(true));
            assert_ne!(
                Constitution::phase_flags()
                    & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED,
                0
            );
        });
    });
}

#[test]
fn coretime_liveness_calls_dispatch_while_hard_gate_suspension_is_active() {
    use pallet_execution_guard::GuardianState;
    use pallet_guardian::GuardianEffectDispatcher;

    development_ext().execute_with(|| {
        assert_coretime_liveness_calls_dispatch_during(89, || {
            System::set_block_number(10);
            let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
            pallet_welfare::GateBreachFlags::<Runtime>::insert(
                epoch,
                pallet_welfare::CoreGateBreachFlags {
                    s_breached: true,
                    c_breached: false,
                    day_bitmap: [1, 0],
                },
            );
            assert_ok!(crate::configs::RuntimeGuardianEffects::dispatch(
                pallet_guardian::GuardianPower::SuspendOnGate,
                H256::zero().into(),
            ));
            assert!(crate::configs::RuntimeGuardianState::gate_suspended());
        });
    });
}

#[test]
fn coretime_liveness_calls_dispatch_while_ledger_freeze_playbook_is_active() {
    use pallet_guardian::GuardianEffectDispatcher;

    development_ext().execute_with(|| {
        assert_coretime_liveness_calls_dispatch_during(90, || {
            System::set_block_number(10);
            assert_ok!(crate::configs::RuntimeGuardianEffects::dispatch(
                pallet_guardian::GuardianPower::ActivatePlaybook {
                    id: pallet_guardian::PlaybookId::LedgerFreeze,
                    trigger: pallet_guardian::PlaybookTrigger::LedgerDrift,
                    expiry: 20,
                    target: None,
                },
                H256::repeat_byte(180).into(),
            ));
            pallet_guardian::ActivePlaybooks::<Runtime>::mutate(|active| {
                assert!(active
                    .try_push(pallet_guardian::ActivePlaybook {
                        id: pallet_guardian::PlaybookId::LedgerFreeze,
                        expiry: 20,
                        renewals_used: 0,
                    })
                    .is_ok());
            });
            assert!(Guardian::playbook_active(
                pallet_guardian::PlaybookId::LedgerFreeze
            ));
            assert!(pallet_conditional_ledger::FrozenUntil::<Runtime>::get().is_some());
            assert!(pallet_market::FrozenUntil::<Runtime>::get().is_some());
        });
    });
}

#[test]
fn coretime_liveness_calls_dispatch_while_reserve_health_flag_is_set() {
    development_ext().execute_with(|| {
        assert_coretime_liveness_calls_dispatch_during(91, || {
            FutarchyTreasury::set_reserve_impaired(true);
            assert!(FutarchyTreasury::treasury().reserve_impaired);
            assert_eq!(FutarchyTreasury::nav().spendable_nav, 0);
        });
    });
}

#[test]
fn xcm_traffic_recorder_uses_the_live_epoch_start_for_normal_day_attribution() {
    use crate::configs::XcmTrafficRecorder;
    use bleavit_xcm::health::LocalXcmHealthSink;

    development_ext().execute_with(|| {
        const EPOCH: u32 = 7;
        const START: u32 = 123;
        pallet_epoch::EpochOf::<Runtime>::mutate(|info| info.index = EPOCH);
        pallet_epoch::Schedule::<Runtime>::mutate(|schedule| {
            schedule.epoch_start_block = START;
        });
        System::set_block_number(START + 3 * futarchy_primitives::kernel::BLOCKS_PER_DAY + 17);
        XcmTrafficRecorder::note_sent();
        assert_eq!(Welfare::xcm_traffic(EPOCH, 3).accepted, 1);
    });
}

#[test]
fn xcm_traffic_recorder_attributes_the_first_post_roll_event_to_new_epoch_day_zero() {
    use crate::configs::XcmTrafficRecorder;
    use bleavit_xcm::health::LocalXcmHealthSink;

    development_ext().execute_with(|| {
        const NEW_EPOCH: u32 = 12;
        const ROLL_BLOCK: u32 = 90_001;
        pallet_epoch::EpochOf::<Runtime>::mutate(|info| info.index = NEW_EPOCH);
        pallet_epoch::Schedule::<Runtime>::mutate(|schedule| {
            schedule.epoch_start_block = ROLL_BLOCK;
        });
        System::set_block_number(ROLL_BLOCK);
        XcmTrafficRecorder::note_send_failure();
        assert_eq!(Welfare::xcm_traffic(NEW_EPOCH, 0).failed, 1);
    });
}

#[test]
fn xcm_traffic_recorder_clamps_large_live_epoch_day_to_u8_max() {
    use crate::configs::XcmTrafficRecorder;
    use bleavit_xcm::health::LocalXcmHealthSink;

    development_ext().execute_with(|| {
        const EPOCH: u32 = 19;
        pallet_epoch::EpochOf::<Runtime>::mutate(|info| info.index = EPOCH);
        pallet_epoch::Schedule::<Runtime>::mutate(|schedule| {
            schedule.epoch_start_block = 0;
        });
        System::set_block_number(u32::MAX);
        XcmTrafficRecorder::note_probe_timeout();
        assert_eq!(Welfare::xcm_traffic(EPOCH, u8::MAX).probe_timeouts, 1);
    });
}

#[test]
fn oracle_probe_timeout_sink_records_welfare_xcm_traffic() {
    development_ext().execute_with(|| {
        pallet_epoch::EpochOf::<Runtime>::mutate(|info| info.index = 0);
        pallet_epoch::Schedule::<Runtime>::mutate(|schedule| {
            schedule.epoch_start_block = 0;
        });
        System::set_block_number(pallet_oracle::RES_PROBE_INTERVAL);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(account(
            78
        ))));
        System::set_block_number(pallet_oracle::RES_PROBE_INTERVAL * 2);
        assert_ok!(Oracle::crank_reserve_probe(RuntimeOrigin::signed(account(
            78
        ))));

        assert_eq!(Welfare::xcm_traffic(0, 2).probe_timeouts, 1);
    });
}

#[test]
fn runtime_metric_inputs_do_not_emit_r_even_when_it_is_registered() {
    use pallet_welfare::{
        BoundedSpecSet, ComponentValue, MetricInputs, MetricSpec, Pillar, SourceClass,
        EPSILON_PILLAR, HISTORY_PRIORS, ONE,
    };

    fn spec(id: u16, version: u16) -> MetricSpec {
        MetricSpec {
            id,
            version,
            pillar: Pillar::COnchain,
            weight: futarchy_primitives::FixedU64(ONE / 2),
            epsilon_floor: EPSILON_PILLAR,
            activation_epoch: 0,
            source: SourceClass::Onchain,
            formula_ref: [1; 32],
            units: [2; 16],
            repr: [3; 16],
            cadence_blocks: 1,
            sanity_min: futarchy_primitives::FixedU64(0),
            sanity_max: futarchy_primitives::FixedU64(ONE),
            has_normalization_rule: true,
            has_missing_data_rule: true,
            has_gaming_vectors: true,
            has_challenge_procedure: false,
            prior_bounds: [futarchy_primitives::FixedU64(ONE); HISTORY_PRIORS],
        }
    }

    development_ext().execute_with(|| {
        const VERSION: u16 = 77;
        const EPOCH: u32 = 9;
        const DAY: u8 = 3;
        let specs = BoundedSpecSet::truncate_from(vec![
            spec(futarchy_primitives::metric_ids::X, VERSION),
            spec(futarchy_primitives::metric_ids::R, VERSION),
        ]);
        pallet_welfare::MetricSpecs::<Runtime>::insert(VERSION, specs);

        assert_eq!(
            crate::configs::RuntimeMetricInputs::onchain_components(EPOCH, VERSION),
            vec![ComponentValue {
                id: futarchy_primitives::metric_ids::X,
                value: futarchy_primitives::FixedU64(ONE),
            }]
        );

        Welfare::note_xcm_traffic(EPOCH, DAY, pallet_welfare::XcmTrafficKind::Accepted);
        Welfare::note_xcm_traffic(EPOCH, DAY, pallet_welfare::XcmTrafficKind::SendFailed);
        Welfare::note_xcm_traffic(EPOCH, DAY, pallet_welfare::XcmTrafficKind::SendFailed);
        pallet_oracle::ReserveHealth::<Runtime>::put(pallet_oracle::ReserveHealthValue {
            unhealthy: true,
            ..Default::default()
        });
        let degraded = vec![ComponentValue {
            id: futarchy_primitives::metric_ids::X,
            value: futarchy_primitives::FixedU64(333_333_333),
        }];
        assert_eq!(
            crate::configs::RuntimeMetricInputs::onchain_components(EPOCH, VERSION),
            degraded
        );
        assert_eq!(
            crate::configs::RuntimeMetricInputs::daily_components(EPOCH, DAY, VERSION),
            degraded
        );
        assert_eq!(
            crate::configs::RuntimeMetricInputs::daily_components(EPOCH, DAY + 1, VERSION),
            vec![ComponentValue {
                id: futarchy_primitives::metric_ids::X,
                value: futarchy_primitives::FixedU64(ONE),
            }]
        );
        assert!(
            crate::configs::RuntimeMetricInputs::onchain_components(EPOCH, VERSION + 1).is_empty()
        );
    });
}

#[test]
fn development_key_constants_match_the_well_known_sr25519_keys() {
    assert_eq!(
        crate::genesis::ALICE_PUBLIC,
        Sr25519Keyring::Alice.to_raw_public()
    );
    assert_eq!(
        crate::genesis::BOB_PUBLIC,
        Sr25519Keyring::Bob.to_raw_public()
    );
    assert_eq!(
        crate::genesis::CHARLIE_PUBLIC,
        Sr25519Keyring::Charlie.to_raw_public()
    );
    assert_eq!(
        crate::genesis::DAVE_PUBLIC,
        Sr25519Keyring::Dave.to_raw_public()
    );
}

#[test]
fn team_allocations_are_transfer_locked() {
    development_ext().execute_with(|| {
        let alice = Sr25519Keyring::Alice.to_account_id();
        for team_member in [
            Sr25519Keyring::Charlie.to_account_id(),
            Sr25519Keyring::Dave.to_account_id(),
        ] {
            assert_eq!(Balances::usable_balance(&team_member), 0);
            let transfer = RuntimeCall::Balances(pallet_balances::Call::transfer_allow_death {
                dest: MultiAddress::Id(alice.clone()),
                value: 1,
            });
            assert!(transfer
                .dispatch(RuntimeOrigin::signed(team_member.clone()))
                .is_err());
            assert_eq!(
                Balances::free_balance(&team_member),
                crate::genesis::FOUNDING_TEAM_ACCOUNT
            );
        }
    });
}

#[test]
fn fully_vesting_locked_account_cannot_pay_native_transaction_fees() {
    type NativeFeeCharger = <Runtime as pallet_transaction_payment::Config>::OnChargeTransaction;

    development_ext().execute_with(|| {
        let charlie = Sr25519Keyring::Charlie.to_account_id();
        let fee_call = remark();
        let dispatch_info = fee_call.get_dispatch_info();
        let result = <NativeFeeCharger as pallet_transaction_payment::OnChargeTransaction<
            Runtime,
        >>::withdraw_fee(&charlie, &fee_call, &dispatch_info, 1, 0);
        assert!(matches!(
            result,
            Err(TransactionValidityError::Invalid(
                InvalidTransaction::Payment
            ))
        ));
        assert_eq!(
            Balances::free_balance(&charlie),
            crate::genesis::FOUNDING_TEAM_ACCOUNT
        );
        assert!(Balances::locks(&charlie)
            .iter()
            .any(|lock| lock.id == *b"vesting "));
    });
}

#[test]
fn team_vesting_curve_is_cliffed_and_never_faster_than_the_ideal_curve() {
    let charlie = Sr25519Keyring::Charlie.to_account_id();
    let year = crate::genesis::BLOCKS_PER_YEAR;
    let total = crate::genesis::FOUNDING_TEAM_ACCOUNT;
    let horizon = 4 * year;

    development_ext().execute_with(|| {
        let locked_at = |block| {
            System::set_block_number(block);
            match Vesting::vesting_balance(&charlie) {
                Some(locked) => locked,
                None => {
                    assert!(false, "Charlie must have a genesis vesting schedule");
                    0
                }
            }
        };

        assert_eq!(locked_at(0), total);
        assert_eq!(locked_at(year - 1), total);
        assert_eq!(locked_at(year), total);

        let mut unlocked_samples = Vec::new();
        for block in [year, 2 * year, 3 * year, horizon] {
            let unlocked = total - locked_at(block);
            assert!(
                unlocked * crate::Balance::from(horizon) <= total * crate::Balance::from(block),
                "genesis vesting must never dominate the ideal t/4 unlock curve"
            );
            unlocked_samples.push(unlocked);
        }
        assert!(unlocked_samples.windows(2).all(|pair| pair[0] < pair[1]));

        // pallet-vesting floors `per_block` during genesis construction. The
        // exact 100M allocation is not divisible by the exact three-year block
        // length, so a sub-VIT remainder conservatively clears one block after
        // the nominal four-year horizon rather than one block before it.
        let duration = 3 * year;
        let per_block = total / crate::Balance::from(duration);
        let rounding_tail = total - per_block * crate::Balance::from(duration);
        assert_eq!(locked_at(horizon), rounding_tail);
        assert!(rounding_tail > 0);
        assert_eq!(locked_at(horizon + 1), 0);
    });
}

#[test]
fn vesting_force_calls_are_nobody_and_public_calls_remain_public() {
    let schedule = pallet_vesting::VestingInfo::new(currency::VIT, 1, 0);
    let force_calls = [
        RuntimeCall::Vesting(pallet_vesting::Call::force_vested_transfer {
            source: MultiAddress::Id(account(1)),
            target: MultiAddress::Id(account(2)),
            schedule,
        }),
        RuntimeCall::Vesting(pallet_vesting::Call::force_remove_vesting_schedule {
            target: MultiAddress::Id(account(1)),
            schedule_index: 0,
        }),
    ];
    for call in force_calls {
        assert!(!RuntimeBaseCallFilter::contains(&call));
        for origin in pallet_origins::Origin::ALL {
            assert!(!RuntimeBaseCallFilter::contains_for(
                origin.to_model(),
                &call
            ));
        }
        for wrapped in closed_wrappers(call) {
            assert!(!RuntimeBaseCallFilter::contains(&wrapped));
        }
    }

    assert!(RuntimeBaseCallFilter::contains(&RuntimeCall::Vesting(
        pallet_vesting::Call::vest {}
    )));
    assert!(RuntimeBaseCallFilter::contains(&RuntimeCall::Vesting(
        pallet_vesting::Call::vested_transfer {
            target: MultiAddress::Id(account(2)),
            schedule,
        }
    )));
}

#[test]
fn vesting_schedule_bound_rejects_the_ninth_schedule() {
    // limit-coverage: Vesting schedules per account
    development_ext().execute_with(|| {
        let source = Sr25519Keyring::Alice.to_account_id();
        let target = account(99);
        let schedule = pallet_vesting::VestingInfo::new(currency::VIT, 1, 100);
        for _ in 0..8 {
            assert_ok!(Vesting::vested_transfer(
                RuntimeOrigin::signed(source.clone()),
                MultiAddress::Id(target.clone()),
                schedule,
            ));
        }
        assert_noop!(
            Vesting::vested_transfer(
                RuntimeOrigin::signed(source),
                MultiAddress::Id(target),
                schedule,
            ),
            pallet_vesting::Error::<Runtime>::AtMaxVestingSchedules
        );
    });
}

#[test]
fn oracle_proof_bound_is_enforced_by_real_runtime_extrinsic_admission() {
    // limit-coverage: orc.max_proof_bytes
    let bound = pallet_oracle::MAX_PROOF_BYTES_BOUND;
    let proof = BoundedVec::<u8, ConstU32<{ pallet_oracle::MAX_PROOF_BYTES_BOUND }>>::try_from(
        vec![0; bound as usize],
    )
    .expect("the at-bound proof constructs");
    let call = RuntimeCall::Oracle(pallet_oracle::Call::recompute_proof {
        component: 1,
        epoch: 1,
        spec_version: 1,
        proof,
    });
    let call_bytes = call.encode();
    let call_len = call_bytes.len();
    let encoded_at_bound = UncheckedExtrinsic::new_bare(call).encode();
    let mut at_bound_input = encoded_at_bound.as_slice();
    let decoded_at_bound = UncheckedExtrinsic::decode(&mut at_bound_input);
    assert!(decoded_at_bound.is_ok());
    assert!(at_bound_input.is_empty());

    // Derive the real bare-extrinsic preamble from the valid runtime type, then
    // replace only its call bytes with a proof whose declared/actual length is
    // bound+1. This exercises UncheckedExtrinsic::decode, not just pallet Call.
    let encoded_bound_len = Compact(bound).encode();
    let proof_start = call_bytes.len().saturating_sub(bound as usize);
    let length_start = proof_start.saturating_sub(encoded_bound_len.len());
    assert_eq!(
        &call_bytes[length_start..proof_start],
        encoded_bound_len.as_slice()
    );
    let mut oversized_call = call_bytes;
    oversized_call.splice(
        length_start..proof_start,
        Compact(bound.saturating_add(1)).encode(),
    );
    oversized_call.push(0);

    let mut inner_at_bound = encoded_at_bound.as_slice();
    let declared_inner = Compact::<u32>::decode(&mut inner_at_bound)
        .expect("valid bare extrinsic has a compact length")
        .0 as usize;
    assert_eq!(declared_inner, inner_at_bound.len());
    let preamble_len = inner_at_bound.len().saturating_sub(call_len);
    let mut oversized_inner = inner_at_bound[..preamble_len].to_vec();
    oversized_inner.extend(oversized_call);
    let mut encoded_oversized =
        Compact(u32::try_from(oversized_inner.len()).expect("test extrinsic length fits u32"))
            .encode();
    encoded_oversized.extend(oversized_inner);

    let error = UncheckedExtrinsic::decode(&mut encoded_oversized.as_slice())
        .expect_err("a bound+1 proof must fail real runtime extrinsic admission");
    assert!(error.to_string().contains("BoundedVec exceeds its limit"));
}

#[test]
fn tick_batch_bound_is_enforced_by_real_runtime_extrinsic_admission() {
    // limit-coverage: TickBatch
    let bound = kernel::TICK_BATCH;
    let pids = pallet_epoch::TickBatch::try_from(
        (0..u64::from(bound)).collect::<Vec<futarchy_primitives::ProposalId>>(),
    )
    .expect("the at-bound batch constructs");
    let call = RuntimeCall::Epoch(pallet_epoch::Call::tick { pids });
    let call_bytes = call.encode();
    let call_len = call_bytes.len();
    let encoded_at_bound = UncheckedExtrinsic::new_bare(call).encode();
    let mut at_bound_input = encoded_at_bound.as_slice();
    let decoded_at_bound = UncheckedExtrinsic::decode(&mut at_bound_input);
    assert!(decoded_at_bound.is_ok());
    assert!(at_bound_input.is_empty());

    // Same construction as the oracle-proof admission test above: keep the real
    // bare-extrinsic preamble, replace only the batch's compact length with
    // bound+1 and append one more fixed-width pid, then prove the REAL
    // UncheckedExtrinsic::decode rejects the 11th item at admission.
    let pid_bytes = core::mem::size_of::<futarchy_primitives::ProposalId>();
    let encoded_bound_len = Compact(bound).encode();
    let items_start = call_bytes.len().saturating_sub(bound as usize * pid_bytes);
    let length_start = items_start.saturating_sub(encoded_bound_len.len());
    assert_eq!(
        &call_bytes[length_start..items_start],
        encoded_bound_len.as_slice()
    );
    let mut oversized_call = call_bytes;
    oversized_call.splice(
        length_start..items_start,
        Compact(bound.saturating_add(1)).encode(),
    );
    oversized_call.extend(core::iter::repeat_n(0u8, pid_bytes));

    let mut inner_at_bound = encoded_at_bound.as_slice();
    let declared_inner = Compact::<u32>::decode(&mut inner_at_bound)
        .expect("valid bare extrinsic has a compact length")
        .0 as usize;
    assert_eq!(declared_inner, inner_at_bound.len());
    let preamble_len = inner_at_bound.len().saturating_sub(call_len);
    let mut oversized_inner = inner_at_bound[..preamble_len].to_vec();
    oversized_inner.extend(oversized_call);
    let mut encoded_oversized =
        Compact(u32::try_from(oversized_inner.len()).expect("test extrinsic length fits u32"))
            .encode();
    encoded_oversized.extend(oversized_inner);

    let error = UncheckedExtrinsic::decode(&mut encoded_oversized.as_slice())
        .expect_err("an 11-pid tick batch must fail real runtime extrinsic admission");
    assert!(error.to_string().contains("BoundedVec exceeds its limit"));
}

/// Shared byte-surgery core of the admission tests above: prove the at-bound
/// call decodes as a real bare extrinsic, then splice the trailing
/// `BoundedVec<u8, _>`'s compact length to bound+1 (adding one filler byte)
/// and return the error the REAL `UncheckedExtrinsic::decode` rejects it
/// with (each caller asserts the specific message). `tail_len` is the fixed
/// number of encoded bytes that follow the bounded vec in the call.
fn trailing_byte_vec_admission_error(call: RuntimeCall, bound: u32, tail_len: usize) -> String {
    let call_bytes = call.encode();
    let call_len = call_bytes.len();
    let encoded_at_bound = UncheckedExtrinsic::new_bare(call).encode();
    let mut at_bound_input = encoded_at_bound.as_slice();
    let decoded_at_bound = UncheckedExtrinsic::decode(&mut at_bound_input);
    assert!(decoded_at_bound.is_ok());
    assert!(at_bound_input.is_empty());

    let encoded_bound_len = Compact(bound).encode();
    let items_start = call_bytes
        .len()
        .saturating_sub(tail_len)
        .saturating_sub(bound as usize);
    let length_start = items_start.saturating_sub(encoded_bound_len.len());
    assert_eq!(
        &call_bytes[length_start..items_start],
        encoded_bound_len.as_slice()
    );
    let mut oversized_call = call_bytes;
    oversized_call.splice(
        length_start..items_start,
        Compact(bound.saturating_add(1)).encode(),
    );
    oversized_call.insert(items_start, 0);

    let mut inner_at_bound = encoded_at_bound.as_slice();
    let declared_inner = Compact::<u32>::decode(&mut inner_at_bound)
        .expect("valid bare extrinsic has a compact length")
        .0 as usize;
    assert_eq!(declared_inner, inner_at_bound.len());
    let preamble_len = inner_at_bound.len().saturating_sub(call_len);
    let mut oversized_inner = inner_at_bound[..preamble_len].to_vec();
    oversized_inner.extend(oversized_call);
    let mut encoded_oversized =
        Compact(u32::try_from(oversized_inner.len()).expect("test extrinsic length fits u32"))
            .encode();
    encoded_oversized.extend(oversized_inner);

    let error = UncheckedExtrinsic::decode(&mut encoded_oversized.as_slice())
        .expect_err("a bound+1 vec must fail real runtime extrinsic admission");
    error.to_string()
}

#[test]
fn migration_cursor_bound_is_enforced_by_real_runtime_extrinsic_admission() {
    // limit-coverage: MIGRATION_CURSOR_MAX_LEN
    let bound = futarchy_primitives::bounds::MIGRATION_CURSOR_MAX_LEN;
    let inner_cursor =
        pallet_migrations::RawCursorOf::<Runtime>::try_from(vec![0u8; bound as usize])
            .expect("the at-bound cursor constructs");
    let call = RuntimeCall::Migrations(pallet_migrations::Call::force_set_active_cursor {
        index: 0,
        inner_cursor: Some(inner_cursor),
        started_at: None,
    });
    // The encoded `started_at: None` is the single byte following the cursor.
    let error = trailing_byte_vec_admission_error(call, bound, 1);
    assert!(error.contains("BoundedVec exceeds its limit"));
}

#[test]
fn migration_identifier_bound_is_enforced_by_real_runtime_extrinsic_admission() {
    // limit-coverage: MIGRATION_IDENTIFIER_MAX_LEN
    use pallet_migrations::HistoricCleanupSelector;

    let bound = futarchy_primitives::bounds::MIGRATION_IDENTIFIER_MAX_LEN;
    let identifier =
        pallet_migrations::IdentifierOf::<Runtime>::try_from(vec![0u8; bound as usize])
            .expect("the at-bound identifier constructs");
    let call = RuntimeCall::Migrations(pallet_migrations::Call::clear_historic {
        selector: HistoricCleanupSelector::Specific(vec![identifier]),
    });
    // The single identifier's bytes are the encoding's tail.
    let error = trailing_byte_vec_admission_error(call, bound, 0);
    assert!(error.contains("BoundedVec exceeds its limit"));
}

#[test]
fn metadata_generates_and_runtime_constants_are_visible() {
    development_ext().execute_with(|| {
        let encoded = Runtime::metadata().encode();
        assert!(encoded.len() > 128);
        assert_eq!(
            crate::configs::Ss58Prefix::get(),
            chain_identity::SS58_PREFIX
        );
        assert_eq!(pallet_guardian::GUARDIAN_SEATS, 7);
        assert!(encoded
            .windows(b"PlaybookFreezeWindowBlocks".len())
            .any(|window| window == b"PlaybookFreezeWindowBlocks"));
        assert_eq!(kernel::PLAYBOOK_FREEZE_WINDOW_BLOCKS, 201_600);
    });
}

#[test]
fn metadata_exposes_only_allowed_attestor_and_guardian_constants() {
    use frame_support::__private::metadata::{RuntimeMetadata, RuntimeMetadataPrefixed};

    development_ext().execute_with(|| {
        let version = Runtime::metadata_versions()
            .into_iter()
            .filter(|version| matches!(version, 15 | 16))
            .max()
            .expect("stable2606 exposes V15 or V16 metadata");
        let encoded = Runtime::metadata_at_version(version)
            .expect("a reported runtime metadata version is constructible");
        let prefixed = RuntimeMetadataPrefixed::decode(&mut &encoded[..])
            .expect("runtime-generated metadata decodes");

        macro_rules! assert_constant_surface {
            ($metadata:expr) => {{
                let attestor = $metadata
                    .pallets
                    .iter()
                    .find(|pallet| pallet.name == "Attestor")
                    .expect("Attestor pallet is present");
                let attestor_names = attestor
                    .constants
                    .iter()
                    .map(|constant| constant.name.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(
                    attestor_names,
                    vec!["AttMinMembers", "AttQuorum", "ChallengeWindowBlocks"]
                );
                let window = attestor
                    .constants
                    .iter()
                    .find(|constant| constant.name == "ChallengeWindowBlocks")
                    .expect("att.window kernel floor is exposed");
                assert_eq!(
                    u32::decode(&mut &window.value[..]).expect("window constant is u32"),
                    kernel::ORC_WINDOW_BLOCKS
                );

                let guardian = $metadata
                    .pallets
                    .iter()
                    .find(|pallet| pallet.name == "Guardian")
                    .expect("Guardian pallet is present");
                assert_eq!(
                    guardian
                        .constants
                        .iter()
                        .map(|constant| constant.name.as_str())
                        .collect::<Vec<_>>(),
                    // `PlaybookFreezeWindowBlocks` (B1b) is a genuine kernel
                    // value, the class 02 §9(2) admits; the Params-driven
                    // constants stay removed (B10).
                    vec![
                        "GuardianSeats",
                        "GuardianThreshold",
                        "GuardianBond",
                        "PlaybookFreezeWindowBlocks",
                    ]
                );
            }};
        }

        match prefixed.1 {
            RuntimeMetadata::V15(metadata) => assert_constant_surface!(metadata),
            RuntimeMetadata::V16(metadata) => assert_constant_surface!(metadata),
            metadata => panic!(
                "requested V{version}, but runtime returned V{}",
                metadata.version()
            ),
        }
    });
}

#[test]
fn d13_system_calls_are_denied_bare_and_through_every_closed_wrapper() {
    let calls = nobody_system_calls();
    for call in &calls {
        assert!(!RuntimeBaseCallFilter::contains(call));
        for wrapped in closed_wrappers(call.clone()) {
            assert!(!RuntimeBaseCallFilter::contains(&wrapped));
        }
    }
    development_ext().execute_with(|| {
        for call in calls {
            let result = call.clone().dispatch(RuntimeOrigin::signed(account(70)));
            assert!(matches!(result, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));
            for wrapped in closed_wrappers(call) {
                let result = wrapped.dispatch(RuntimeOrigin::signed(account(70)));
                assert!(matches!(result, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));
            }
        }
    });
    let mut nested = RuntimeCall::System(frame_system::Call::set_code { code: vec![1] });
    for depth in 0..kernel::MAX_NESTED_LEVELS {
        nested = match depth % 3 {
            0 => RuntimeCall::Utility(pallet_utility::Call::batch {
                calls: vec![nested],
            }),
            1 => RuntimeCall::Proxy(pallet_proxy::Call::proxy {
                real: MultiAddress::Id(account(15)),
                force_proxy_type: None,
                call: Box::new(nested),
            }),
            _ => RuntimeCall::Sudo(pallet_sudo::Call::sudo {
                call: Box::new(nested),
            }),
        };
        assert!(!RuntimeBaseCallFilter::contains(&nested));
    }
    assert!(RuntimeBaseCallFilter::contains(&remark()));
}

#[test]
fn nesting_budget_accepts_the_limit_and_fails_closed_beyond_it() {
    // limit-coverage: MAX_NESTED
    let mut at_limit = remark();
    for _ in 0..kernel::MAX_NESTED_LEVELS {
        at_limit = RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![at_limit],
        });
    }
    assert!(RuntimeBaseCallFilter::contains(&at_limit));
    let beyond = RuntimeCall::Utility(pallet_utility::Call::batch {
        calls: vec![at_limit],
    });
    assert!(!RuntimeBaseCallFilter::contains(&beyond));

    let oversized = RuntimeCall::Utility(pallet_utility::Call::batch {
        calls: (0..=kernel::MAX_NESTED_CALLS).map(|_| remark()).collect(),
    });
    assert!(!RuntimeBaseCallFilter::contains(&oversized));

    development_ext().execute_with(|| {
        for call in [beyond, oversized] {
            assert_noop!(
                call.dispatch(RuntimeOrigin::signed(account(70))),
                frame_system::Error::<Runtime>::CallFiltered
            );
        }
    });
}

/// Decode-bomb hardening (15 §4.5, SQ-225): the execution guard decodes
/// preimage-sourced batches (`decode_batch`) whose element type `RuntimeCall`
/// nests recursively. Without a depth limit an adversarial hash-committed
/// preimage of one deeply-nested call (≤ `MAX_BYTES`) would recurse in `Decode`
/// until the wasm stack-height trap / native stack abort — a G-1 violation in
/// audit-scope-A code. `MAX_PAYLOAD_DECODE_DEPTH` bounds the decode so an
/// over-deep batch fails closed to a decode error rather than trapping, while a
/// spec-legal shallow batch still decodes.
#[test]
fn deep_preimage_batch_decode_fails_closed_at_the_depth_limit() {
    // limit-coverage: MAX_PAYLOAD_DECODE_DEPTH
    use parity_scale_codec::DecodeLimit;

    // Construct + encode the over-deep call on a large-stack helper thread:
    // building/encoding it recurses, but the depth-limited decode under test
    // does not (it bails at the limit before recursing that far).
    let deep_bytes = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            let mut nested = remark();
            for _ in 0..(kernel::MAX_PAYLOAD_DECODE_DEPTH as usize + 200) {
                nested = RuntimeCall::Utility(pallet_utility::Call::batch {
                    calls: vec![nested],
                });
            }
            // A `RuntimeBatch` (BoundedVec<RuntimeCall, 16>) SCALE-encodes as a
            // one-element vector carrying the deeply-nested call.
            vec![nested].encode()
        })
        .expect("spawn deep-encode thread")
        .join()
        .expect("encode deep call");

    // (a) The codec mechanism: the real guard type rejects the over-deep batch.
    let over_deep = pallet_execution_guard::RuntimeBatch::<Runtime>::decode_all_with_depth_limit(
        kernel::MAX_PAYLOAD_DECODE_DEPTH,
        &mut &deep_bytes[..],
    );
    assert!(
        over_deep.is_err(),
        "an over-deep preimage batch must fail closed at the depth limit, not trap"
    );

    // A legitimately shallow batch (within the `MAX_NESTED_LEVELS` filter bound)
    // still decodes cleanly through the same depth-limited path.
    let shallow_bytes = vec![RuntimeCall::Utility(pallet_utility::Call::batch {
        calls: vec![remark()],
    })]
    .encode();
    assert!(
        pallet_execution_guard::RuntimeBatch::<Runtime>::decode_all_with_depth_limit(
            kernel::MAX_PAYLOAD_DECODE_DEPTH,
            &mut &shallow_bytes[..],
        )
        .is_ok(),
        "a spec-legal shallow batch must still decode"
    );

    // (b) The PRODUCTION wiring (PR #92 bot P2): drive the same over-deep
    // preimage through the guard's real `execute` → `decode_batch` path and
    // assert it fails closed to `BadPreimage`. Treasury needs no
    // ratification/attestation, so `decode_batch` is the operative gate here —
    // this pins that `decode_batch` USES the depth limit (a revert to unbounded
    // `Decode` would abort this test on the native stack instead of passing).
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 9_256;
        let maturity = enqueue_treasury_bytes(PID, deep_bytes.clone())
            .expect("over-deep treasury payload enqueues (not decoded at enqueue time)");
        System::set_block_number(maturity);
        let execute_error = ExecutionGuard::execute(RuntimeOrigin::signed(account(92)), PID)
            .expect_err("guard execute must reject the over-deep preimage");
        assert_eq!(
            execute_error.error,
            pallet_execution_guard::Error::<Runtime>::BadPreimage.into(),
            "the guard's decode_batch must fail closed on an over-deep preimage"
        );
    });
}

#[test]
fn screening_rejects_overdeep_preimage_with_shared_decode_limit() {
    let spawned = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            let mut nested = remark();
            for _ in 0..(kernel::MAX_PAYLOAD_DECODE_DEPTH as usize + 200) {
                nested = RuntimeCall::Utility(pallet_utility::Call::batch {
                    calls: vec![nested],
                });
            }
            vec![nested].encode()
        });
    assert!(
        spawned.is_ok(),
        "deep screening-preimage encoder must spawn"
    );
    let Ok(handle) = spawned else {
        return;
    };
    let joined = handle.join();
    assert!(
        joined.is_ok(),
        "deep screening-preimage encoder must complete"
    );
    let Ok(deep_bytes) = joined else {
        return;
    };

    development_ext().execute_with(|| {
        let decoded = pallet_execution_guard::Pallet::<Runtime>::decode_batch(&deep_bytes);
        assert!(
            decoded.is_err(),
            "shared decoder must reject an over-deep preimage"
        );
        let Err(decode_error) = decoded else {
            return;
        };
        assert_eq!(
            decode_error,
            pallet_execution_guard::Error::<Runtime>::BadPreimage.into()
        );

        let payload_len = u32::try_from(deep_bytes.len());
        assert!(
            payload_len.is_ok(),
            "bounded screening preimage length must fit u32"
        );
        let Ok(payload_len) = payload_len else {
            return;
        };
        let payload_hash = <Preimage as StorePreimage>::note(deep_bytes.into());
        assert!(
            payload_hash.is_ok(),
            "bounded screening preimage must be noted"
        );
        let Ok(payload_hash) = payload_hash else {
            return;
        };
        let proposal = empty_param_proposal(9_257, account(93), payload_hash, payload_len);
        assert_eq!(
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                AccountId,
            >>::static_check(&proposal),
            pallet_epoch::StaticCheckDisposition::Refund(RejectReason::ProcessHold),
            "screening must fail closed with the typed unverifiable disposition"
        );
    });
}

#[test]
fn bare_system_upgrade_calls_stay_denied_when_guard_descriptor_matures() {
    let authorize = RuntimeCall::System(frame_system::Call::authorize_upgrade {
        code_hash: H256::repeat_byte(1),
    });
    let all_origins = [
        ClassOrigin::FutarchyParam,
        ClassOrigin::FutarchyTreasury,
        ClassOrigin::FutarchyCode,
        ClassOrigin::FutarchyMeta,
        ClassOrigin::ConstitutionalValues,
        ClassOrigin::OracleResolution,
        ClassOrigin::GuardianHold,
        ClassOrigin::EmergencyPlaybook,
    ];
    assert!(!RuntimeBaseCallFilter::contains(&authorize));
    for origin in all_origins {
        assert!(!RuntimeBaseCallFilter::contains_for(origin, &authorize));
    }

    upgrade_ext().execute_with(|| {
        let apply =
            RuntimeCall::System(frame_system::Call::apply_authorized_upgrade { code: vec![1] });
        System::set_block_number(10);
        seed_parachain_upgrade_boundary(1);
        set_pending_upgrade(None);
        assert!(!RuntimeBaseCallFilter::contains(&apply));
        for wrapped in closed_wrappers(apply.clone()) {
            assert!(!RuntimeBaseCallFilter::contains(&wrapped));
        }
        set_pending_upgrade(Some(11));
        assert!(!RuntimeBaseCallFilter::contains(&apply));
        set_pending_upgrade(Some(10));
        assert!(RuntimeBaseCallFilter::contains(&apply));
        assert!(RuntimeBaseCallFilter::contains(&RuntimeCall::Utility(
            pallet_utility::Call::batch {
                calls: vec![apply.clone()],
            }
        )));
        set_pending_upgrade(Some(9));
        assert!(RuntimeBaseCallFilter::contains(&apply));
        set_pending_upgrade(None);
    });
}

#[test]
fn code_queue_admits_before_ratification_and_binds_a_later_pass_before_execute() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 5_991;
        const RATIFY_REF: u32 = 91;
        let candidate = b"a8-r1-late-ratification".to_vec();
        let (maturity, _) =
            match enqueue_attested_code_upgrade_pending_ratification(PID, &candidate) {
                Some(setup) => setup,
                None => {
                    assert!(false, "unratified CODE queue fixture must be constructible");
                    return;
                }
            };

        let queued = match pallet_execution_guard::Queue::<Runtime>::get(PID) {
            Some(queued) => queued,
            None => {
                assert!(false, "CODE must enqueue before its ratification passes");
                return;
            }
        };
        assert_eq!(queued.ratify_ref, None);
        assert!(!queued.ratification_passed);
        assert!(!pallet_execution_guard::Ratifications::<Runtime>::contains_key(PID));

        // The only ratification deadline is execute-time: before the values
        // referendum binds, execution fails without consuming the queue.
        System::set_block_number(maturity);
        assert_eq!(
            ExecutionGuard::execute(RuntimeOrigin::signed(account(78)), PID)
                .map_err(|error| error.error),
            Err(pallet_execution_guard::Error::<Runtime>::NotRatified.into()),
        );
        assert!(pallet_execution_guard::Queue::<Runtime>::contains_key(PID));

        assert_ok!(ExecutionGuard::ratify(
            pallet_origins::Origin::ConstitutionalValues.into(),
            PID,
            RATIFY_REF,
        ));
        let bound = match pallet_execution_guard::Queue::<Runtime>::get(PID) {
            Some(queued) => queued,
            None => {
                assert!(false, "late ratification must retain the queue entry");
                return;
            }
        };
        assert_eq!(bound.ratify_ref, Some(RATIFY_REF));
        assert!(bound.ratification_passed);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(78)),
            PID,
        ));
        assert!(!pallet_execution_guard::Queue::<Runtime>::contains_key(PID));
    });
}

#[test]
fn ratification_views_agree_and_never_understate_an_unratified_code_upgrade() {
    // Regression (B2): `proposal_summaries` mapped an absent `Ratifications`
    // record to `NotRequired` for every class, so a CODE upgrade awaiting its
    // values referendum rendered as "no ratification needed" while
    // `execution_queue` — reading the guard's own projection — rendered the
    // same proposal as unratified. 06 §2.2 R-1 makes a passed referendum an
    // execute precondition for these classes, so G-1 requires the fail-closed
    // spelling, and the two API surfaces must never contradict each other.
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 5_993;
        const RATIFY_REF: u32 = 93;
        let candidate = b"b2-ratification-view-agreement".to_vec();
        if enqueue_attested_code_upgrade_pending_ratification(PID, &candidate).is_none() {
            assert!(false, "unratified CODE queue fixture must be constructible");
            return;
        }
        assert!(!pallet_execution_guard::Ratifications::<Runtime>::contains_key(PID));

        let summary_status = |pid| {
            crate::views::proposal_summaries()
                .into_iter()
                .find(|view| view.id == pid)
                .map(|view| view.ratification)
        };
        let queue_status = |pid| {
            crate::views::execution_queue()
                .into_iter()
                .find(|view| view.pid == pid)
                .map(|view| view.ratification)
        };

        // A CODE class requires ratification (06 §2.2); with no record on chain
        // the summary must not claim otherwise, and must equal the guard view.
        assert_eq!(
            summary_status(PID),
            Some(RatificationStatus::Failed { referendum: 0 }),
        );
        assert_eq!(summary_status(PID), queue_status(PID));

        assert_ok!(ExecutionGuard::ratify(
            pallet_origins::Origin::ConstitutionalValues.into(),
            PID,
            RATIFY_REF,
        ));

        // `Ratifications` is written only by the RatifyOrigin-gated call, so a
        // present record is a passed referendum on both surfaces.
        assert_eq!(
            summary_status(PID),
            Some(RatificationStatus::Passed {
                referendum: RATIFY_REF
            }),
        );
        assert_eq!(summary_status(PID), queue_status(PID));
    });
}

#[test]
fn never_ratified_code_fails_at_execute_not_at_queue_admission() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 5_992;
        let candidate = b"a8-r1-never-ratified".to_vec();
        let (maturity, _) =
            match enqueue_attested_code_upgrade_pending_ratification(PID, &candidate) {
                Some(setup) => setup,
                None => {
                    assert!(false, "unratified CODE queue fixture must be constructible");
                    return;
                }
            };
        assert!(pallet_execution_guard::Queue::<Runtime>::contains_key(PID));
        System::set_block_number(maturity);
        assert_eq!(
            ExecutionGuard::execute(RuntimeOrigin::signed(account(79)), PID)
                .map_err(|error| error.error),
            Err(pallet_execution_guard::Error::<Runtime>::NotRatified.into()),
        );
        assert!(pallet_execution_guard::Queue::<Runtime>::contains_key(PID));
        assert_eq!(
            pallet_epoch::Proposals::<Runtime>::get(PID).map(|proposal| proposal.state),
            Some(ProposalState::Queued),
        );
    });
}

#[test]
fn upgrade_path_authorizes_schedules_and_clears_only_after_validation_code_applies() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_001;
        const RATIFY_REF: u32 = 71;
        let candidate = b"bleavit-b6-candidate-runtime-v2".to_vec();
        let (maturity, artifact) = match enqueue_attested_code_upgrade(PID, &candidate, RATIFY_REF) {
            Some(setup) => setup,
            None => {
                assert!(false, "attested upgrade fixture must be constructible");
                return;
            }
        };

        System::set_block_number(maturity);
        let release_before = release_channel_raw();
        let checkpoint_parent = System::parent_hash();
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(75)),
            PID,
        ));

        let authorization = System::authorized_upgrade();
        assert!(authorization
            .is_some_and(|authorization| authorization.code_hash() == &artifact));
        let pending = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending,
            None => {
                assert!(false, "successful CODE execution must create PendingUpgrade");
                return;
            }
        };
        assert_eq!(pending.hash, artifact.0);
        assert_eq!(pending.authorized_at, maturity);
        assert_eq!(
            pending.applicable_at,
            maturity.saturating_add(kernel::DESCRIPTOR_LEAD_TIME_BLOCKS)
        );
        assert_eq!(
            pending.target_spec_version,
            VERSION.spec_version.saturating_add(1)
        );
        let checkpoint = pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get();
        assert!(checkpoint.is_some_and(|(parent, state_root)| {
            parent == checkpoint_parent.0 && state_root != [0; 32]
        }));
        assert!(System::events().iter().any(|record| matches!(
            &record.event,
            crate::RuntimeEvent::ExecutionGuard(
                pallet_execution_guard::Event::UpgradeAuthorized {
                    code_hash,
                    authorized_at,
                }
            ) if *code_hash == artifact.0 && *authorized_at == maturity
        )));

        let raw = match release_channel_raw() {
            Some(raw) => raw,
            None => {
                assert!(false, "frozen ReleaseChannel raw key must exist");
                return;
            }
        };
        assert_eq!(raw.len(), pallet_constitution::RELEASE_CHANNEL_LEN);
        assert_eq!(raw_u32(&raw, 108), Some(maturity));
        assert_eq!(raw_u32(&raw, 112), Some(pending.target_spec_version));
        assert_eq!(raw_u32(&raw, 116), Some(maturity));
        assert!(raw_u32(&raw, 164).is_some_and(|flags| flags & (1 << 2) != 0));
        if let Some(before) = release_before {
            assert_raw_unchanged_outside(&before, &raw, &[108..120, 164..168]);
            assert_eq!(
                raw_u32(&before, 164).map(|flags| flags & !(1 << 2)),
                raw_u32(&raw, 164).map(|flags| flags & !(1 << 2))
            );
        } else {
            assert!(false, "genesis ReleaseChannel raw key must exist");
        }

        let system_apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: candidate.clone(),
        });
        System::set_block_number(pending.applicable_at.saturating_sub(1));
        assert!(!RuntimeBaseCallFilter::contains(&system_apply));
        let early = system_apply
            .clone()
            .dispatch(RuntimeOrigin::signed(account(76)));
        assert!(matches!(early, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));
        assert!(System::authorized_upgrade().is_some());

        System::set_block_number(pending.applicable_at);
        let wrong_apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: b"wrong-authorized-artifact".to_vec(),
        });
        assert!(!RuntimeBaseCallFilter::contains(&wrong_apply));
        let wrong = wrong_apply.dispatch(RuntimeOrigin::signed(account(76)));
        assert!(matches!(wrong, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));
        assert!(System::authorized_upgrade().is_some());

        seed_parachain_upgrade_boundary(candidate.len());
        assert!(RuntimeBaseCallFilter::contains(&system_apply));
        assert!(system_apply
            .dispatch(RuntimeOrigin::signed(account(76)))
            .is_ok());
        assert_eq!(
            cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::get(),
            candidate
        );
        assert_eq!(
            cumulus_pallet_parachain_system::NewValidationCode::<Runtime>::get(),
            Some(candidate.clone())
        );
        assert!(System::authorized_upgrade().is_none());
        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_some());
        let authorized_raw = raw.clone();

        // The next block's guard initialization observes the successful
        // Cumulus schedule before the relay inherent can consume its signal.
        System::set_block_number(System::block_number().saturating_add(1));
        let _ = ExecutionGuard::on_initialize(System::block_number());
        assert_eq!(
            pallet_execution_guard::ScheduledUpgrade::<Runtime>::get(),
            Some(artifact.0)
        );

        // Exercise the production Cumulus boundary: relay-state proof decode,
        // `GoAhead`, `:code` installation, and the configured OnSystemEvent.
        submit_relay_upgrade_go_ahead();

        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get().is_none());
        let applied_raw = match release_channel_raw() {
            Some(raw) => raw,
            None => {
                assert!(false, "ReleaseChannel must survive applied-upgrade callback");
                return;
            }
        };
        assert_eq!(raw_u32(&applied_raw, 108), Some(System::block_number()));
        assert_eq!(raw_u32(&applied_raw, 116), Some(0));
        assert!(raw_u32(&applied_raw, 164).is_some_and(|flags| flags & (1 << 2) == 0));
        assert_raw_unchanged_outside(
            &authorized_raw,
            &applied_raw,
            &[108..112, 116..120, 164..168],
        );
        assert_eq!(
            raw_u32(&authorized_raw, 164).map(|flags| flags & !(1 << 2)),
            raw_u32(&applied_raw, 164).map(|flags| flags & !(1 << 2))
        );
        assert!(System::events().iter().any(|record| matches!(
            &record.event,
            crate::RuntimeEvent::ExecutionGuard(pallet_execution_guard::Event::UpgradeApplied {
                code_hash,
                spec_version,
            }) if *code_hash == artifact.0 && *spec_version == pending.target_spec_version
        )));
        assert!(!System::events().iter().any(|record| matches!(
            &record.event,
            crate::RuntimeEvent::ExecutionGuard(pallet_execution_guard::Event::UpgradeAborted {
                ..
            })
        )));
    });
}

#[test]
fn relay_abort_clears_pending_state_alarms_and_allows_normal_reproposal() {
    use pallet_guardian::GuardianTriggers;

    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_007;
        const RETRY_PID: futarchy_primitives::ProposalId = 6_008;
        let candidate = b"bleavit-b6-relay-aborted-runtime-v2".to_vec();
        let (maturity, artifact) = match enqueue_attested_code_upgrade(PID, &candidate, 77) {
            Some(setup) => setup,
            None => {
                assert!(false, "abort fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(85)),
            PID,
        ));
        let pending = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending,
            None => {
                assert!(false, "abort fixture must authorize an upgrade");
                return;
            }
        };
        System::set_block_number(pending.applicable_at);
        seed_parachain_upgrade_boundary(candidate.len());
        let apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: candidate.clone(),
        });
        assert!(apply.dispatch(RuntimeOrigin::signed(account(86))).is_ok());
        assert!(System::authorized_upgrade().is_none());
        assert!(cumulus_pallet_parachain_system::PendingValidationCode::<
            Runtime,
        >::exists());

        System::set_block_number(System::block_number().saturating_add(1));
        let _ = ExecutionGuard::on_initialize(System::block_number());
        assert_eq!(
            pallet_execution_guard::ScheduledUpgrade::<Runtime>::get(),
            Some(artifact.0)
        );
        let release_before_abort = match release_channel_raw() {
            Some(raw) => raw,
            None => {
                assert!(false, "abort fixture release channel must exist");
                return;
            }
        };

        submit_relay_upgrade_abort();

        assert!(
            cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::get().is_empty()
        );
        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::ScheduledUpgrade::<Runtime>::get().is_none());
        assert!(System::authorized_upgrade().is_none());
        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert!(!crate::configs::RuntimeGuardianTriggers::current().migration_halt);
        assert!(System::events().iter().any(|record| matches!(
            &record.event,
            crate::RuntimeEvent::ExecutionGuard(pallet_execution_guard::Event::UpgradeAborted {
                code_hash,
            }) if *code_hash == artifact.0
        )));
        let release_after_abort = match release_channel_raw() {
            Some(raw) => raw,
            None => {
                assert!(false, "abort cleanup must preserve ReleaseChannel");
                return;
            }
        };
        assert_eq!(
            raw_u32(&release_after_abort, 108),
            Some(System::block_number())
        );
        assert_eq!(raw_u32(&release_after_abort, 116), Some(0));
        assert!(raw_u32(&release_after_abort, 164).is_some_and(|flags| flags & (1 << 2) == 0));
        assert_raw_unchanged_outside(
            &release_before_abort,
            &release_after_abort,
            &[108..112, 116..120, 164..168],
        );

        // No callback re-arms frame-system. A fresh proposal must perform the
        // full attestation/queue/ratification/execution path again.
        let spacing_end = pallet_execution_guard::LastUpgradeAuthorized::<Runtime>::get()
            .and_then(|last| {
                last.checked_add(
                    <crate::configs::ExecutionParams as pallet_execution_guard::Params>::code_spacing(),
                )
            })
            .unwrap_or_else(System::block_number);
        System::set_block_number(System::block_number().max(spacing_end));
        let (retry_maturity, _) = match enqueue_attested_code_upgrade(RETRY_PID, &candidate, 78) {
            Some(setup) => setup,
            None => {
                assert!(false, "the aborted artifact must be re-proposable");
                return;
            }
        };
        assert!(System::authorized_upgrade().is_none());
        System::set_block_number(retry_maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(87)),
            RETRY_PID,
        ));
        assert!(System::authorized_upgrade()
            .is_some_and(|authorization| authorization.code_hash() == &artifact));
    });
}

#[test]
fn relay_abort_cleanup_survives_a_writer_b_release_channel_rewrite() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_009;
        let candidate = b"bleavit-b6-abort-writer-b-runtime-v2".to_vec();
        let (maturity, artifact) = match enqueue_attested_code_upgrade(PID, &candidate, 81) {
            Some(setup) => setup,
            None => {
                assert!(false, "writer-b abort fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(88)),
            PID,
        ));
        let pending = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending,
            None => {
                assert!(false, "writer-b abort fixture must authorize an upgrade");
                return;
            }
        };
        System::set_block_number(pending.applicable_at);
        seed_parachain_upgrade_boundary(candidate.len());
        let apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: candidate.clone(),
        });
        assert!(apply.dispatch(RuntimeOrigin::signed(account(89))).is_ok());
        System::set_block_number(System::block_number().saturating_add(1));
        let _ = ExecutionGuard::on_initialize(System::block_number());
        assert_eq!(
            pallet_execution_guard::ScheduledUpgrade::<Runtime>::get(),
            Some(artifact.0)
        );

        // Writer (b) lawfully repoints the channel mid-flight, zeroing the
        // guard-owned pending fields (the SQ-134 interaction). The abort
        // cleanup must tolerate this — never wedge `PendingUpgrade` — and
        // must leave writer (b)'s newer value byte-identical.
        let mut rewritten = [0u8; pallet_constitution::RELEASE_CHANNEL_LEN];
        match release_channel_raw() {
            Some(raw) if raw.len() == rewritten.len() => rewritten.copy_from_slice(&raw),
            _ => {
                assert!(false, "writer-b fixture release channel must exist");
                return;
            }
        }
        rewritten[116..120].copy_from_slice(&0u32.to_le_bytes());
        let flags = raw_u32(&rewritten, 164).unwrap_or(0) & !(1 << 2);
        rewritten[164..168].copy_from_slice(&flags.to_le_bytes());
        assert_ok!(Constitution::set_release_channel(
            pallet_origins::Origin::ConstitutionalValues.into(),
            rewritten,
        ));

        submit_relay_upgrade_abort();

        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::ScheduledUpgrade::<Runtime>::get().is_none());
        assert!(System::events().iter().any(|record| matches!(
            &record.event,
            crate::RuntimeEvent::ExecutionGuard(pallet_execution_guard::Event::UpgradeAborted {
                code_hash,
            }) if *code_hash == artifact.0
        )));
        assert_eq!(release_channel_raw().as_deref(), Some(&rewritten[..]));
    });
}

#[test]
fn applied_cleanup_survives_a_writer_b_release_channel_rewrite() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_010;
        let candidate = b"bleavit-b6-applied-writer-b-runtime-v2".to_vec();
        let (maturity, artifact) = match enqueue_attested_code_upgrade(PID, &candidate, 82) {
            Some(setup) => setup,
            None => {
                assert!(false, "applied writer-b fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(93)),
            PID,
        ));
        let pending = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending,
            None => {
                assert!(false, "applied writer-b fixture must authorize an upgrade");
                return;
            }
        };
        System::set_block_number(pending.applicable_at);
        seed_parachain_upgrade_boundary(candidate.len());
        let apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: candidate.clone(),
        });
        assert!(apply.dispatch(RuntimeOrigin::signed(account(94))).is_ok());
        System::set_block_number(System::block_number().saturating_add(1));
        let _ = ExecutionGuard::on_initialize(System::block_number());

        // Writer (b) lawfully repoints the channel between scheduling and the
        // relay GoAhead, zeroing the guard-owned pending fields. An applied
        // upgrade cannot be retried, so the applied cleanup must tolerate the
        // rewrite (PR #65 P1): guard state records the application, writer
        // (b)'s newer channel value stays byte-identical, and no halt source
        // is raised.
        let mut rewritten = [0u8; pallet_constitution::RELEASE_CHANNEL_LEN];
        match release_channel_raw() {
            Some(raw) if raw.len() == rewritten.len() => rewritten.copy_from_slice(&raw),
            _ => {
                assert!(false, "applied writer-b fixture release channel must exist");
                return;
            }
        }
        rewritten[116..120].copy_from_slice(&0u32.to_le_bytes());
        let flags = raw_u32(&rewritten, 164).unwrap_or(0) & !(1 << 2);
        rewritten[164..168].copy_from_slice(&flags.to_le_bytes());
        assert_ok!(Constitution::set_release_channel(
            pallet_origins::Origin::ConstitutionalValues.into(),
            rewritten,
        ));

        submit_relay_upgrade_go_ahead();

        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::ScheduledUpgrade::<Runtime>::get().is_none());
        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert!(System::events().iter().any(|record| matches!(
            &record.event,
            crate::RuntimeEvent::ExecutionGuard(pallet_execution_guard::Event::UpgradeApplied {
                code_hash,
                ..
            }) if *code_hash == artifact.0
        )));
        assert_eq!(release_channel_raw().as_deref(), Some(&rewritten[..]));
    });
}

#[test]
fn upgrade_apply_without_pending_descriptor_is_filter_denied() {
    development_ext().execute_with(|| {
        let apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: b"no-pending-upgrade".to_vec(),
        });
        assert!(!RuntimeBaseCallFilter::contains(&apply));
        let result = apply.dispatch(RuntimeOrigin::signed(account(77)));
        assert!(matches!(result, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));
    });
}

#[test]
fn system_authorization_survives_cumulus_overlap_preflight_rejection() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_006;
        let candidate = b"bleavit-b6-overlap-preflight-runtime-v2".to_vec();
        let (maturity, artifact) = match enqueue_attested_code_upgrade(PID, &candidate, 76) {
            Some(setup) => setup,
            None => {
                assert!(false, "overlap preflight fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(82)),
            PID,
        ));
        let pending_before = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get()
        {
            Some(pending) => pending,
            None => {
                assert!(false, "CODE execution must leave a guard pending upgrade");
                return;
            }
        };
        let checkpoint_before =
            pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get();
        let release_before = release_channel_raw();
        System::set_block_number(pending_before.applicable_at);
        seed_parachain_upgrade_boundary(candidate.len());
        let existing = b"already-scheduled-validation-code".to_vec();
        cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::put(existing.clone());

        let apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: candidate,
        });
        assert!(!RuntimeBaseCallFilter::contains(&apply));
        let result = apply.dispatch(RuntimeOrigin::signed(account(83)));
        assert!(matches!(result, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));

        assert!(System::authorized_upgrade()
            .is_some_and(|authorization| authorization.code_hash() == &artifact));
        assert_eq!(
            pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get(),
            Some(pending_before)
        );
        assert_eq!(
            pallet_execution_guard::PendingUpgradeCheckpoint::<Runtime>::get(),
            checkpoint_before
        );
        assert_eq!(release_channel_raw(), release_before);
        assert_eq!(
            cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::get(),
            existing
        );
    });
}

#[test]
fn migration_halt_keeps_forward_remediation_upgrade_applicable() {
    use frame_support::migrations::FailedMigrationHandler;

    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_003;
        let candidate = b"bleavit-b6-dispatcher-runtime-v2".to_vec();
        let (maturity, _) = match enqueue_attested_code_upgrade(PID, &candidate, 73) {
            Some(setup) => setup,
            None => {
                assert!(false, "dispatcher upgrade fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(80)),
            PID,
        ));
        let applicable_at = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending.applicable_at,
            None => {
                assert!(false, "dispatcher fixture must authorize an upgrade");
                return;
            }
        };
        System::set_block_number(applicable_at);
        seed_parachain_upgrade_boundary(candidate.len());

        pallet_migrations::Cursor::<Runtime>::put(pallet_migrations::MigrationCursor::Stuck);
        assert_eq!(
            crate::configs::MigrationFailureToGuard::failed(Some(3)),
            frame_support::migrations::FailedMigrationHandling::KeepStuck
        );
        assert!(System::authorized_upgrade().is_some());
        let bounded = match pallet_execution_guard::pallet::RuntimeCode::<Runtime>::try_from(
            candidate.clone(),
        ) {
            Ok(code) => code,
            Err(_) => {
                assert!(false, "remediation runtime must fit the code bound");
                return;
            }
        };
        assert_ok!(ExecutionGuard::apply_authorized_upgrade(
            RuntimeOrigin::signed(account(84)),
            bounded,
        ));
        assert_eq!(
            cumulus_pallet_parachain_system::PendingValidationCode::<Runtime>::get(),
            candidate
        );
        assert!(System::authorized_upgrade().is_none());
        assert!(pallet_migrations::Cursor::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_some());
    });
}

#[test]
fn applied_code_alarm_does_not_retire_a_healthy_active_migration_cursor() {
    use cumulus_pallet_parachain_system::OnSystemEvent;

    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_010;
        let candidate = b"bleavit-b6-healthy-active-cursor-runtime-v2".to_vec();
        let (maturity, _) = match enqueue_attested_code_upgrade(PID, &candidate, 79) {
            Some(setup) => setup,
            None => {
                assert!(false, "healthy cursor fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(89)),
            PID,
        ));
        let pending = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending,
            None => {
                assert!(false, "healthy cursor fixture must authorize an upgrade");
                return;
            }
        };
        System::set_block_number(pending.applicable_at);
        seed_parachain_upgrade_boundary(candidate.len());
        let cursor = pallet_migrations::MigrationCursor::Active(pallet_migrations::ActiveCursor {
            index: 0,
            inner_cursor: None,
            started_at: System::block_number(),
        });
        pallet_migrations::Cursor::<Runtime>::put(cursor.clone());
        crate::configs::ExecutionGuardSystemEvent::on_validation_code_applied();
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
        let authorization_hash_before =
            System::authorized_upgrade().map(|authorization| *authorization.code_hash());
        let release_before = release_channel_raw();
        let bounded =
            match pallet_execution_guard::pallet::RuntimeCode::<Runtime>::try_from(candidate) {
                Ok(code) => code,
                Err(_) => {
                    assert!(false, "healthy cursor runtime must fit the code bound");
                    return;
                }
            };

        assert_noop!(
            ExecutionGuard::apply_authorized_upgrade(RuntimeOrigin::signed(account(90)), bounded,),
            frame_system::Error::<Runtime>::MultiBlockMigrationsOngoing
        );
        assert_eq!(pallet_migrations::Cursor::<Runtime>::get(), Some(cursor));
        assert_eq!(
            System::authorized_upgrade().map(|authorization| *authorization.code_hash()),
            authorization_hash_before
        );
        assert_eq!(release_channel_raw(), release_before);
        assert_eq!(
            pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get(),
            Some(pending)
        );
    });
}

#[test]
fn code_queue_rejects_real_under_quorum_attestation_without_storage_changes() {
    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_004;
        let candidate = b"bleavit-b6-under-quorum-candidate".to_vec();
        let members = [account(94), account(95), account(96)];
        assert_ok!(Attestor::set_members(
            pallet_origins::Origin::ConstitutionalValues.into(),
            members.to_vec(),
        ));
        let artifact = sp_io::hashing::blake2_256(&candidate);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(members[0].clone()),
            PID,
            artifact,
            [104; 32],
        ));
        let record = match pallet_attestor::Attestations::<Runtime>::get()
            .into_iter()
            .find(|record| record.pid == PID && record.artifact_hash == artifact)
        {
            Some(record) => record,
            None => {
                assert!(
                    false,
                    "the real attestor adapter fixture must store one record"
                );
                return;
            }
        };
        System::set_block_number(record.challenge_deadline.saturating_add(1));
        assert!(!Attestor::has_quorum(PID, artifact));

        let call = RuntimeCall::System(frame_system::Call::authorize_upgrade {
            code_hash: H256::from(artifact),
        });
        let batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![call]) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "single-call upgrade batch must fit");
                    return;
                }
            };
        let bytes = batch.encode();
        let payload_len = match u32::try_from(bytes.len()) {
            Ok(len) => len,
            Err(_) => {
                assert!(false, "bounded batch length must fit u32");
                return;
            }
        };
        let payload_hash = match <Preimage as StorePreimage>::note(bytes.into()) {
            Ok(hash) => hash,
            Err(_) => {
                assert!(false, "bounded batch preimage must be accepted");
                return;
            }
        };
        <Preimage as QueryPreimage>::request(&payload_hash);
        let now = System::block_number();
        let maturity = now.saturating_add(
            <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_timelock(
                ProposalClass::Code,
            ),
        );
        let grace_end = maturity.saturating_add(
            <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_grace(
                ProposalClass::Code,
            ),
        );
        let version_constraint = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => {
                assert!(
                    false,
                    "guard genesis must store the current runtime version"
                );
                return;
            }
        };
        let declared_domains = match pallet_execution_guard::pallet::StoredDomains::try_from(vec![
            pallet_execution_guard::CallDomain::InternalRootAuthorizeUpgrade,
        ]) {
            Ok(domains) => domains,
            Err(_) => {
                assert!(false, "single upgrade domain must fit");
                return;
            }
        };
        assert_ok!(seed_queued_epoch_proposal(
            PID,
            ProposalClass::Code,
            payload_hash,
            payload_len,
            maturity,
            grace_end,
            version_constraint.clone(),
        ));
        assert_noop!(
            ExecutionGuard::enqueue(
                RuntimeOrigin::signed(crate::configs::epoch_account()),
                pallet_execution_guard::pallet::StoredQueuedExecution {
                    pid: PID,
                    payload_hash: payload_hash.0,
                    payload_len,
                    class: ProposalClass::Code,
                    maturity,
                    grace_end,
                    version_constraint,
                    meters_declared: Default::default(),
                    ratify_ref: Some(74),
                    ratification_passed: false,
                    attestation_id: Some(record.id),
                    pre_upgrade_checkpoint: None,
                    cancelled: false,
                    declared_domains,
                    failed_at: None,
                },
                false,
            ),
            pallet_execution_guard::Error::<Runtime>::AttestationMissing
        );
        assert!(!pallet_execution_guard::pallet::Queue::<Runtime>::contains_key(PID));
        assert!(!pallet_execution_guard::AttestationBindings::<Runtime>::contains_key(PID));
    });
}

#[test]
fn code_execution_losing_live_attestor_quorum_is_a_storage_noop() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_002;
        let candidate = b"bleavit-b6-unattested-runtime-v2".to_vec();
        let (maturity, _) = match enqueue_attested_code_upgrade(PID, &candidate, 72) {
            Some(setup) => setup,
            None => {
                assert!(false, "attested upgrade fixture must be constructible");
                return;
            }
        };
        // Member 91 supplied one of the two attestations. Replacing it makes
        // the still-present record live-below-quorum before execution.
        assert_ok!(Attestor::set_members(
            pallet_origins::Origin::ConstitutionalValues.into(),
            vec![account(90), account(92), account(93)],
        ));
        assert!(!Attestor::has_quorum(
            PID,
            sp_io::hashing::blake2_256(&candidate),
        ));
        System::set_block_number(maturity);
        let queued_before = pallet_execution_guard::pallet::Queue::<Runtime>::get(PID);
        let release_before = release_channel_raw();
        // `execute` refunds via `DispatchResultWithPostInfo` (B5), so the error
        // carries a checks-only post-info; the surrounding asserts pin the
        // storage no-op that `assert_noop!` used to check.
        let execute_error = ExecutionGuard::execute(RuntimeOrigin::signed(account(78)), PID)
            .expect_err("guard execute must reject");
        assert_eq!(
            execute_error.error,
            pallet_execution_guard::Error::<Runtime>::AttestationMissing.into()
        );
        assert_eq!(
            pallet_execution_guard::pallet::Queue::<Runtime>::get(PID),
            queued_before
        );
        assert_eq!(release_channel_raw(), release_before);
        assert!(System::authorized_upgrade().is_none());
        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_none());
    });
}

#[test]
fn live_code_capability_disables_and_reenables_upgrade_authorization() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_005;
        let capability = pallet_constitution::Capability::AuthorizeUpgrade;
        assert_ok!(Constitution::set_capability(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::CapabilityRecord {
                class: ProposalClass::Code,
                capability,
                enabled: false,
            },
        ));
        let candidate = b"bleavit-b6-capability-gated-runtime-v2".to_vec();
        let (maturity, _) = match enqueue_attested_code_upgrade(PID, &candidate, 75) {
            Some(setup) => setup,
            None => {
                assert!(false, "capability fixture must be constructible");
                return;
            }
        };
        assert!(pallet_execution_guard::pallet::Queue::<Runtime>::contains_key(PID));
        System::set_block_number(maturity);

        assert!(!Constitution::capability_enabled(
            ProposalClass::Code,
            capability,
        ));
        // `execute` refunds via `DispatchResultWithPostInfo` (B5), so the error
        // carries a checks-only post-info; the surrounding asserts pin the
        // storage no-op that `assert_noop!` used to check.
        let execute_error = ExecutionGuard::execute(RuntimeOrigin::signed(account(81)), PID)
            .expect_err("guard execute must reject");
        assert_eq!(
            execute_error.error,
            pallet_execution_guard::Error::<Runtime>::CapabilityDenied.into()
        );
        assert!(System::authorized_upgrade().is_none());
        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_none());
        assert!(pallet_execution_guard::pallet::Queue::<Runtime>::contains_key(PID));

        assert_ok!(Constitution::set_capability(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::CapabilityRecord {
                class: ProposalClass::Code,
                capability,
                enabled: true,
            },
        ));
        assert!(Constitution::capability_enabled(
            ProposalClass::Code,
            capability,
        ));
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(81)),
            PID,
        ));
        assert!(System::authorized_upgrade().is_some());
        assert!(pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get().is_some());
    });
}

#[test]
fn live_treasury_capability_disables_queued_call_without_state_change_then_reenables() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_009;
        let capability = pallet_constitution::Capability::TreasurySpend;
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| state.main_usdc = 10);
        let call =
            RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
                line: pallet_futarchy_treasury::BudgetLine::Pol,
                amount: 1,
            });
        let maturity = match enqueue_treasury_call(PID, call) {
            Some(maturity) => maturity,
            None => {
                assert!(false, "treasury capability fixture must be constructible");
                return;
            }
        };
        assert_ok!(Constitution::set_capability(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::CapabilityRecord {
                class: ProposalClass::Treasury,
                capability,
                enabled: false,
            },
        ));
        System::set_block_number(maturity);
        let state_before = pallet_futarchy_treasury::State::<Runtime>::get();
        let queue_before = pallet_execution_guard::pallet::Queue::<Runtime>::get(PID);

        // `execute` refunds via `DispatchResultWithPostInfo` (B5), so the error
        // carries a checks-only post-info; the surrounding asserts pin the
        // storage no-op that `assert_noop!` used to check.
        let execute_error = ExecutionGuard::execute(RuntimeOrigin::signed(account(88)), PID)
            .expect_err("guard execute must reject");
        assert_eq!(
            execute_error.error,
            pallet_execution_guard::Error::<Runtime>::CapabilityDenied.into()
        );
        assert_eq!(
            pallet_futarchy_treasury::State::<Runtime>::get(),
            state_before
        );
        assert_eq!(
            pallet_execution_guard::pallet::Queue::<Runtime>::get(PID),
            queue_before
        );

        assert_ok!(Constitution::set_capability(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::CapabilityRecord {
                class: ProposalClass::Treasury,
                capability,
                enabled: true,
            },
        ));
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(88)),
            PID,
        ));
        assert!(pallet_execution_guard::pallet::Queue::<Runtime>::get(PID).is_none());
        assert_ne!(
            pallet_futarchy_treasury::State::<Runtime>::get(),
            state_before
        );
    });
}

#[test]
fn execute_under_constitution_dead_man_reports_freeze_active_and_preserves_queue() {
    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_010;
        let call =
            RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
                line: pallet_futarchy_treasury::BudgetLine::Pol,
                amount: 1,
            });
        let maturity = enqueue_treasury_call(PID, call).expect("treasury call enqueues");
        System::set_block_number(maturity);
        let queued_before = pallet_execution_guard::Queue::<Runtime>::get(PID);
        pallet_constitution::PhaseFlags::<Runtime>::mutate(|flags| {
            *flags |= pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED;
        });

        let error = ExecutionGuard::execute(RuntimeOrigin::signed(account(82)), PID)
            .expect_err("dead-man engagement freezes execute");
        assert_eq!(
            error.error,
            pallet_execution_guard::Error::<Runtime>::FreezeActive.into(),
        );
        assert_eq!(
            pallet_execution_guard::Queue::<Runtime>::get(PID),
            queued_before
        );
    });
}

#[test]
fn failed_migration_handler_sets_the_guard_machine_signal() {
    use frame_support::migrations::FailedMigrationHandler;
    use pallet_guardian::GuardianTriggers;

    development_ext().execute_with(|| {
        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert_eq!(
            crate::configs::MigrationFailureToGuard::failed(Some(3)),
            frame_support::migrations::FailedMigrationHandling::KeepStuck
        );
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert_eq!(crate::configs::MigrationFailedStep::get(), Some(3));
        assert!(crate::configs::RuntimeGuardianTriggers::current().migration_halt);
    });
}

#[test]
fn migration_completion_clears_a_migration_failure_halt() {
    use frame_support::migrations::{FailedMigrationHandler, MigrationStatusHandler};

    development_ext().execute_with(|| {
        assert_eq!(
            crate::configs::MigrationFailureToGuard::failed(Some(4)),
            frame_support::migrations::FailedMigrationHandling::KeepStuck
        );
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
        crate::configs::MigrationStatusToGuard::completed();
        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert!(crate::configs::MigrationFailedStep::get().is_none());
    });
}

#[test]
fn valid_zero_mbm_recovery_image_clears_migration_failure_and_stall_sources() {
    use cumulus_pallet_parachain_system::OnSystemEvent;
    use frame_support::migrations::FailedMigrationHandler;

    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 6_011;
        let candidate = b"bleavit-b6-zero-mbm-recovery-runtime-v2".to_vec();
        let (maturity, artifact) = match enqueue_attested_code_upgrade(PID, &candidate, 80) {
            Some(setup) => setup,
            None => {
                assert!(false, "zero-MBM recovery fixture must be constructible");
                return;
            }
        };
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(91)),
            PID,
        ));
        let pending = match pallet_execution_guard::pallet::PendingUpgrade::<Runtime>::get() {
            Some(pending) => pending,
            None => {
                assert!(false, "zero-MBM recovery fixture must authorize an upgrade");
                return;
            }
        };
        assert_eq!(
            crate::configs::MigrationFailureToGuard::failed(Some(5)),
            frame_support::migrations::FailedMigrationHandling::KeepStuck
        );
        let observed_at = System::block_number();
        pallet_migrations::Cursor::<Runtime>::put(pallet_migrations::MigrationCursor::Active(
            pallet_migrations::ActiveCursor {
                index: 0,
                inner_cursor: None,
                started_at: observed_at,
            },
        ));
        crate::configs::ExecutionGuardSystemEvent::on_validation_data(
            &cumulus_primitives_core::PersistedValidationData::default(),
        );
        System::set_block_number(
            observed_at
                .saturating_add(kernel::MIGRATION_STALL_BLOCKS)
                .saturating_add(1),
        );
        crate::configs::ExecutionGuardSystemEvent::on_validation_data(
            &cumulus_primitives_core::PersistedValidationData::default(),
        );
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert_eq!(crate::configs::MigrationHaltSources::get() & 0b011, 0b011);
        // The recovery image contains no MBMs; model the abandoned cursor as
        // already retired before its application boundary.
        pallet_migrations::Cursor::<Runtime>::kill();
        System::set_block_number(System::block_number().max(pending.applicable_at));
        seed_parachain_upgrade_boundary(candidate.len());
        let apply =
            RuntimeCall::System(frame_system::Call::apply_authorized_upgrade { code: candidate });
        assert!(apply.dispatch(RuntimeOrigin::signed(account(92))).is_ok());
        System::set_block_number(System::block_number().saturating_add(1));
        let _ = ExecutionGuard::on_initialize(System::block_number());
        assert_eq!(
            pallet_execution_guard::ScheduledUpgrade::<Runtime>::get(),
            Some(artifact.0)
        );

        submit_relay_upgrade_go_ahead();

        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());
        assert_eq!(crate::configs::MigrationHaltSources::get(), 0);
        assert!(crate::configs::MigrationFailedStep::get().is_none());
        assert!(crate::configs::MigrationProgressMarker::get().is_none());
    });
}

#[test]
fn migration_completion_does_not_clear_an_applied_code_mismatch_halt() {
    use cumulus_pallet_parachain_system::OnSystemEvent;
    use frame_support::migrations::MigrationStatusHandler;

    upgrade_ext().execute_with(|| {
        crate::configs::ExecutionGuardSystemEvent::on_validation_code_applied();
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
        crate::configs::MigrationStatusToGuard::completed();
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
    });
}

#[test]
fn active_migration_cursor_halts_only_after_stall_threshold() {
    use cumulus_pallet_parachain_system::OnSystemEvent;

    development_ext().execute_with(|| {
        let first_observed = 10;
        pallet_migrations::Cursor::<Runtime>::put(pallet_migrations::MigrationCursor::Active(
            pallet_migrations::ActiveCursor {
                index: 0,
                inner_cursor: None,
                started_at: first_observed,
            },
        ));
        System::set_block_number(first_observed);
        let mandatory_before = *System::block_weight().get(DispatchClass::Mandatory);
        crate::configs::ExecutionGuardSystemEvent::on_validation_data(
            &cumulus_primitives_core::PersistedValidationData::default(),
        );
        let mandatory_after = *System::block_weight().get(DispatchClass::Mandatory);
        assert!(mandatory_after.ref_time() > mandatory_before.ref_time());
        assert!(mandatory_after.proof_size() > mandatory_before.proof_size());
        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());

        System::set_block_number(first_observed.saturating_add(kernel::MIGRATION_STALL_BLOCKS));
        crate::configs::ExecutionGuardSystemEvent::on_validation_data(
            &cumulus_primitives_core::PersistedValidationData::default(),
        );
        assert!(!pallet_execution_guard::MigrationHalt::<Runtime>::get());

        System::set_block_number(
            first_observed
                .saturating_add(kernel::MIGRATION_STALL_BLOCKS)
                .saturating_add(1),
        );
        crate::configs::ExecutionGuardSystemEvent::on_validation_data(
            &cumulus_primitives_core::PersistedValidationData::default(),
        );
        assert!(pallet_execution_guard::MigrationHalt::<Runtime>::get());
    });
}

#[test]
fn runtime_type_wiring_pins_migration_and_upgrade_event_bridges() {
    assert_same_type::<
        <Runtime as pallet_migrations::Config>::FailedMigrationHandler,
        crate::configs::MigrationFailureToGuard,
    >();
    assert_same_type::<
        <Runtime as pallet_migrations::Config>::MigrationStatusHandler,
        crate::configs::MigrationStatusToGuard,
    >();
    assert_same_type::<
        <Runtime as cumulus_pallet_parachain_system::Config>::OnSystemEvent,
        crate::configs::ExecutionGuardSystemEvent,
    >();
    assert_eq!(
        <<Runtime as pallet_migrations::Config>::CursorMaxLen as Get<u32>>::get(),
        futarchy_primitives::bounds::MIGRATION_CURSOR_MAX_LEN,
    );
    assert_eq!(
        <<Runtime as pallet_migrations::Config>::IdentifierMaxLen as Get<u32>>::get(),
        futarchy_primitives::bounds::MIGRATION_IDENTIFIER_MAX_LEN,
    );
    let expected_service_weight = sp_runtime::Perbill::from_percent(
        futarchy_primitives::bounds::MIGRATION_SERVICE_WEIGHT_PERCENT,
    ) * crate::configs::RuntimeBlockWeights::get().max_block;
    assert_eq!(
        <<Runtime as pallet_migrations::Config>::MaxServiceWeight as Get<Weight>>::get(),
        expected_service_weight,
    );
}

#[test]
fn sq104_migration_admin_calls_are_denied_bare_and_under_sudo() {
    let calls = vec![
        RuntimeCall::Migrations(pallet_migrations::Call::force_set_cursor { cursor: None }),
        RuntimeCall::Migrations(pallet_migrations::Call::force_set_active_cursor {
            index: 0,
            inner_cursor: None,
            started_at: None,
        }),
        RuntimeCall::Migrations(pallet_migrations::Call::force_onboard_mbms {}),
        RuntimeCall::Migrations(pallet_migrations::Call::clear_historic {
            selector: pallet_migrations::HistoricCleanupSelector::Specific(Vec::new()),
        }),
    ];
    development_ext().execute_with(|| {
        for call in calls {
            assert!(!RuntimeBaseCallFilter::contains(&call));
            for wrapped in closed_wrappers(call) {
                assert!(!RuntimeBaseCallFilter::contains(&wrapped));
                let result = wrapped.dispatch(RuntimeOrigin::signed(account(79)));
                assert!(matches!(result, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));
            }
        }
    });
}

#[test]
fn guard_dispatcher_rechecks_the_dynamic_classifier_at_dispatch_time() {
    use pallet_execution_guard::BatchDispatcher;

    development_ext().execute_with(|| {
        let key = pallet_constitution::key16(b"mkt.obs_interval");
        let value = match pallet_constitution::Params::<Runtime>::take(key) {
            Some(record) => record.value,
            None => {
                assert!(false, "Param-class benchmark key must exist");
                return;
            }
        };
        let call = RuntimeCall::Constitution(pallet_constitution::Call::set_param { key, value });
        assert_eq!(
            crate::classifier::RuntimeDispatcher::dispatch_with_class_origin(
                call,
                ProposalClass::Param,
            ),
            Err(DispatchError::Other("guard dispatch-time safety filter"))
        );
    });
}

#[test]
fn proposal_classes_map_to_the_frozen_belief_origins() {
    assert_eq!(
        pallet_origins::Origin::from_proposal_class(ProposalClass::Param),
        Some(pallet_origins::Origin::FutarchyParam)
    );
    assert_eq!(
        pallet_origins::Origin::from_proposal_class(ProposalClass::Treasury),
        Some(pallet_origins::Origin::FutarchyTreasury)
    );
    assert_eq!(
        pallet_origins::Origin::from_proposal_class(ProposalClass::Code),
        Some(pallet_origins::Origin::FutarchyCode)
    );
    assert_eq!(
        pallet_origins::Origin::from_proposal_class(ProposalClass::Meta),
        Some(pallet_origins::Origin::FutarchyMeta)
    );
    assert_eq!(
        pallet_origins::Origin::from_proposal_class(ProposalClass::Constitutional),
        None
    );
}

fn assert_custom_origin_refuses_system_origins<E>()
where
    E: EnsureOrigin<RuntimeOrigin, Success = ()>,
{
    assert!(E::try_origin(RuntimeOrigin::signed(account(1))).is_err());
    assert!(E::try_origin(RuntimeOrigin::root()).is_err());
    assert!(E::try_origin(RuntimeOrigin::none()).is_err());
}

#[test]
fn all_eight_custom_origins_refuse_signed_root_and_none() {
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureFutarchyParam>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureFutarchyTreasury>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureFutarchyCode>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureFutarchyMeta>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureConstitutionalValues>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureOracleResolution>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureGuardianHold>();
    assert_custom_origin_refuses_system_origins::<pallet_origins::EnsureEmergencyPlaybook>();
}

#[test]
fn domain_delegation_and_privileged_laundering_are_pinned() {
    let treasury =
        RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
            line: pallet_futarchy_treasury::BudgetLine::Pol,
            amount: 1,
        });
    assert!(RuntimeBaseCallFilter::contains(&remark()));
    assert!(!RuntimeBaseCallFilter::contains(&treasury));
    assert!(RuntimeBaseCallFilter::contains_for(
        ClassOrigin::FutarchyTreasury,
        &treasury
    ));
    assert!(!RuntimeBaseCallFilter::contains_for(
        ClassOrigin::FutarchyParam,
        &treasury
    ));
    for (index, wrapped) in closed_wrappers(treasury.clone()).into_iter().enumerate() {
        assert!(!RuntimeBaseCallFilter::contains(&wrapped));
        // Proxy and multisig may project the wrapped domain, but they cannot
        // carry a privileged class origin across the delegation boundary.
        if (9..=12).contains(&index) {
            assert!(!RuntimeBaseCallFilter::contains_for(
                ClassOrigin::FutarchyTreasury,
                &wrapped
            ));
        }
    }
    let nested = RuntimeCall::Proxy(pallet_proxy::Call::proxy_announced {
        delegate: MultiAddress::Id(account(11)),
        real: MultiAddress::Id(account(12)),
        force_proxy_type: None,
        call: Box::new(RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![treasury],
        })),
    });
    assert!(!RuntimeBaseCallFilter::contains_for(
        ClassOrigin::FutarchyTreasury,
        &nested
    ));
}

fn epoch_call_samples() -> Vec<RuntimeCall> {
    let proposal = Proposal {
        id: 0,
        proposer: account(30),
        class: ProposalClass::Param,
        state: ProposalState::Submitted,
        epoch: 0,
        submitted_at: 0,
        payload_hash: [0; 32],
        payload_len: 0,
        ask: 0,
        bond: 0,
        resources: Default::default(),
        metric_spec: 0,
        decide_at: 0,
        rerun: false,
        extended: false,
        delayed_once: false,
        markets: None,
        maturity: None,
        grace_end: None,
        version_constraint: None,
        decision: None,
    };
    vec![
        RuntimeCall::Epoch(pallet_epoch::Call::submit { proposal }),
        RuntimeCall::Epoch(pallet_epoch::Call::withdraw { pid: 0 }),
        RuntimeCall::Epoch(pallet_epoch::Call::tick {
            pids: Default::default(),
        }),
        RuntimeCall::Epoch(pallet_epoch::Call::decide { pid: 0 }),
        RuntimeCall::Epoch(pallet_epoch::Call::settle_cohort { epoch: 0, batch: 1 }),
        RuntimeCall::Epoch(pallet_epoch::Call::set_next_epoch_length {}),
        RuntimeCall::Epoch(pallet_epoch::Call::delay_once {
            pid: 0,
            justification_hash: [0; 32],
        }),
        RuntimeCall::Epoch(pallet_epoch::Call::mark_executed { pid: 0 }),
        RuntimeCall::Epoch(pallet_epoch::Call::mark_failed_executed { pid: 0 }),
        RuntimeCall::Epoch(pallet_epoch::Call::retry_exhausted_to_measurement { pid: 0 }),
        RuntimeCall::Epoch(pallet_epoch::Call::expire_or_stale_queue {
            pid: 0,
            reason: None,
        }),
        RuntimeCall::Epoch(pallet_epoch::Call::force_reject_process_hold { pid: 0 }),
        RuntimeCall::Epoch(pallet_epoch::Call::void_cohort { epoch: 0 }),
    ]
}

#[test]
fn epoch_classifier_rows_and_closed_privileged_wrappers_match_the_authority_matrix() {
    let calls = epoch_call_samples();
    assert_eq!(calls.len(), 13);

    for call in &calls[0..5] {
        assert!(RuntimeBaseCallFilter::contains(call));
    }
    for call in &calls[7..11] {
        assert!(RuntimeBaseCallFilter::contains(call));
    }

    let values = &calls[5];
    assert!(crate::classifier::is_values_enactment_leaf(values));
    assert!(RuntimeBaseCallFilter::contains(values));
    assert!(RuntimeBaseCallFilter::contains_for(
        ClassOrigin::ConstitutionalValues,
        values,
    ));

    for guardian in [&calls[6], &calls[11]] {
        assert!(!RuntimeBaseCallFilter::contains(guardian));
        assert!(RuntimeBaseCallFilter::contains_for(
            ClassOrigin::GuardianHold,
            guardian,
        ));
    }
    let void = &calls[12];
    assert!(!RuntimeBaseCallFilter::contains(void));
    assert!(RuntimeBaseCallFilter::contains_for(
        ClassOrigin::EmergencyPlaybook,
        void,
    ));

    for privileged in [&calls[5], &calls[6], &calls[11], &calls[12]] {
        for wrapped in closed_wrappers(privileged.clone()) {
            assert!(
                !RuntimeBaseCallFilter::contains(&wrapped),
                "privileged epoch call must be a bare leaf only: {wrapped:?}",
            );
        }
    }
}

#[test]
fn epoch_privileged_leaves_reject_every_non_authority_origin() {
    development_ext().execute_with(|| {
        let calls = epoch_call_samples();
        for index in [5usize, 6, 7, 8, 9, 10, 11, 12] {
            for bad_origin in [
                RuntimeOrigin::signed(account(71)),
                RuntimeOrigin::root(),
                RuntimeOrigin::none(),
            ] {
                let result = calls[index].clone().dispatch(bad_origin);
                assert!(matches!(
                    result,
                    Err(error)
                        if error.error == DispatchError::BadOrigin
                            || error.error
                                == frame_system::Error::<Runtime>::CallFiltered.into()
                ));
            }
        }

        for index in [5usize, 6, 11, 12] {
            for wrapped in closed_wrappers(calls[index].clone()) {
                assert!(!RuntimeBaseCallFilter::contains(&wrapped));
                let result = wrapped.dispatch(RuntimeOrigin::signed(account(72)));
                assert!(matches!(result, Err(error) if error.error == frame_system::Error::<Runtime>::CallFiltered.into()));
            }
        }
    });
}

fn assert_runtime_gate_veto(class: ProposalClass, expected: RejectReason) {
    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_099;
        let params =
            <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        let end = params.decision_window;
        System::set_block_number(end);
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        let markets = MarketSet {
            accept: 80_991,
            reject: 80_992,
            gates: Some([80_993, 80_994, 80_995, 80_996]),
            baseline: 80_997,
        };
        let gates = markets
            .gates
            .expect("gate-bearing fixture has four physical gate ids");
        let class_index = crate::configs::proposal_class_index(class);
        let contest = params.v_min[class_index];
        let gate_contest = params.gate_v_min[class_index];
        let decision_b = crate::configs::class_pol_floor(class);
        let gate_b = crate::configs::balance_param(b"pol.b_gate");
        let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
        let gate_quotes = match expected {
            RejectReason::GateVetoSurvival => [
                futarchy_primitives::FixedU64(100_000_000),
                futarchy_primitives::FixedU64(100_000_000),
                futarchy_primitives::FixedU64(0),
                futarchy_primitives::FixedU64(0),
            ],
            RejectReason::GateVetoSecurity => [
                futarchy_primitives::FixedU64(0),
                futarchy_primitives::FixedU64(0),
                futarchy_primitives::FixedU64(100_000_000),
                futarchy_primitives::FixedU64(100_000_000),
            ],
            _ => {
                assert!(false, "fixture supports only Survival/Security vetoes");
                return;
            }
        };
        for result in [
            seed_decision_grade_market(
                markets.accept,
                pallet_market::core_market::BookKind::Decision {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Accept,
                },
                futarchy_primitives::FixedU64(700_000_000),
                end,
                (params.decision_window, params.trailing_window),
                decision_b,
                contest,
            ),
            seed_decision_grade_market(
                markets.reject,
                pallet_market::core_market::BookKind::Decision {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Reject,
                },
                futarchy_primitives::FixedU64(500_000_000),
                end,
                (params.decision_window, params.trailing_window),
                decision_b,
                contest,
            ),
            seed_decision_grade_market(
                gates[0],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Accept,
                    gate: futarchy_primitives::GateType::Survival,
                },
                gate_quotes[0],
                end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                gates[1],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Reject,
                    gate: futarchy_primitives::GateType::Survival,
                },
                gate_quotes[1],
                end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                gates[2],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Accept,
                    gate: futarchy_primitives::GateType::Security,
                },
                gate_quotes[2],
                end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                gates[3],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Reject,
                    gate: futarchy_primitives::GateType::Security,
                },
                gate_quotes[3],
                end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                markets.baseline,
                pallet_market::core_market::BookKind::Baseline { epoch },
                futarchy_primitives::FixedU64(500_000_000),
                end,
                (params.decision_window, params.trailing_window),
                baseline_b,
                contest,
            ),
        ] {
            assert_ok!(result);
        }
        pallet_market::BaselineMarketOf::<Runtime>::insert(epoch, markets.baseline);
        assert!(gates
            .iter()
            .all(pallet_market::Markets::<Runtime>::contains_key));

        let batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(Vec::new()) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "empty bounded payload must encode");
                    return;
                }
            };
        let bytes = batch.encode();
        let payload_len = match u32::try_from(bytes.len()) {
            Ok(len) => len,
            Err(_) => {
                assert!(false, "bounded payload length fits u32");
                return;
            }
        };
        let payload_hash = match <Preimage as StorePreimage>::note(bytes.into()) {
            Ok(hash) => hash,
            Err(error) => {
                assert!(false, "payload preimage must be noted: {error:?}");
                return;
            }
        };
        <Preimage as QueryPreimage>::request(&payload_hash);
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = contest.saturating_mul(100);
        });
        let ask = if class == ProposalClass::Treasury {
            1
        } else {
            0
        };
        let spendable_nav = FutarchyTreasury::nav().spendable_nav;
        if class == ProposalClass::Treasury {
            assert!(ask <= spendable_nav / 100, "fixture ask is at most 1% NAV");
        }
        let proposal = Proposal {
            id: PID,
            proposer: account(70),
            class,
            state: ProposalState::Trading,
            epoch,
            submitted_at: 0,
            payload_hash: payload_hash.0,
            payload_len,
            ask,
            bond: Balance::MAX,
            resources: Default::default(),
            metric_spec: 1,
            decide_at: end,
            rerun: false,
            extended: false,
            delayed_once: false,
            markets: Some(markets),
            maturity: None,
            grace_end: None,
            version_constraint: pallet_execution_guard::CurrentSpecName::<Runtime>::get(),
            decision: None,
        };
        pallet_epoch::Proposals::<Runtime>::insert(PID, proposal);
        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        pallet_epoch::ProposalSchedules::<Runtime>::insert(
            PID,
            pallet_epoch::ProposalSchedule {
                epoch,
                epoch_start_block: schedule.epoch_start_block,
                epoch_length: schedule.length,
                decide_at: end,
                metric_spec: 1,
            },
        );
        pallet_epoch::NextProposalId::<Runtime>::mutate(|next| {
            *next = (*next).max(PID.saturating_add(1));
        });
        pallet_conditional_ledger::Vaults::<Runtime>::insert(
            PID,
            pallet_conditional_ledger::core_ledger::VaultInfo::open(1),
        );
        let snapshot = match Epoch::decision_input_snapshot(PID) {
            Some(snapshot) => snapshot,
            None => {
                assert!(false, "assembled runtime must expose decision inputs");
                return;
            }
        };
        assert_eq!(snapshot.inputs.gate_twaps, Some(gate_quotes));

        assert_ok!(Epoch::decide(RuntimeOrigin::signed(account(69)), PID));
        assert_eq!(
            pallet_epoch::Proposals::<Runtime>::get(PID).and_then(|proposal| proposal.decision),
            Some(DecisionOutcome::Reject(expected)),
        );
    });
}

#[test]
fn low_ask_treasury_reaches_survival_veto_through_runtime_epoch() {
    assert_runtime_gate_veto(ProposalClass::Treasury, RejectReason::GateVetoSurvival);
}

#[test]
fn low_ask_treasury_reaches_security_veto_through_runtime_epoch() {
    assert_runtime_gate_veto(ProposalClass::Treasury, RejectReason::GateVetoSecurity);
}

#[test]
fn param_reaches_survival_veto_through_runtime_epoch() {
    assert_runtime_gate_veto(ProposalClass::Param, RejectReason::GateVetoSurvival);
}

#[test]
fn param_reaches_security_veto_through_runtime_epoch() {
    assert_runtime_gate_veto(ProposalClass::Param, RejectReason::GateVetoSecurity);
}

#[test]
fn seeded_trading_decision_revalidates_real_payload_before_guard_enqueue() {
    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_000;
        // The old SQ-172 regression used an empty, unverifiable payload and
        // asserted a fail-closed queue refusal. Use a canonical live Treasury
        // leaf now so the same decision fixture exercises real queue-time
        // footprint verification and the guard handoff.
        let line = pallet_futarchy_treasury::BudgetLine::Pol;
        let resource = expected_resource_key(0x09, Some(&line.encode()));
        let call =
            RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
                line,
                amount: 0,
            });
        let batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![call]) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "one canonical guard call must fit the bound");
                    return;
                }
            };
        let bytes = batch.encode();
        let payload_len = match u32::try_from(bytes.len()) {
            Ok(len) => len,
            Err(_) => {
                assert!(false, "bounded payload length must fit u32");
                return;
            }
        };
        let payload_hash = match <Preimage as StorePreimage>::note(bytes.into()) {
            Ok(hash) => hash,
            Err(error) => {
                assert!(false, "payload preimage must be noted: {error:?}");
                return;
            }
        };
        <Preimage as QueryPreimage>::request(&payload_hash);
        let version_constraint = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => {
                assert!(false, "guard genesis must bind a runtime version");
                return;
            }
        };
        let params =
            <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        let end = params.decision_window;
        System::set_block_number(end);
        let epoch = pallet_epoch::EpochOf::<Runtime>::get().index;
        let ids = MarketSet {
            accept: 81_001,
            reject: 81_002,
            gates: Some([81_003, 81_004, 81_005, 81_006]),
            baseline: 81_007,
        };
        let class_index = crate::configs::proposal_class_index(ProposalClass::Treasury);
        let contest = params.v_min[class_index];
        let gate_contest = params.gate_v_min[class_index];
        let decision_b = crate::configs::class_pol_floor(ProposalClass::Treasury);
        let gate_b = crate::configs::balance_param(b"pol.b_gate");
        let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
        let gates = ids.gates.expect("Treasury fixture has gate books");
        for result in [
            seed_decision_grade_market(
                ids.accept,
                pallet_market::core_market::BookKind::Decision {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Accept,
                },
                futarchy_primitives::FixedU64(700_000_000),
                end,
                (params.decision_window, params.trailing_window),
                decision_b,
                contest,
            ),
            seed_decision_grade_market(
                ids.reject,
                pallet_market::core_market::BookKind::Decision {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Reject,
                },
                futarchy_primitives::FixedU64(500_000_000),
                end,
                (params.decision_window, params.trailing_window),
                decision_b,
                contest,
            ),
            seed_decision_grade_market(
                gates[0],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Accept,
                    gate: futarchy_primitives::GateType::Survival,
                },
                futarchy_primitives::FixedU64(0),
                end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                gates[1],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Reject,
                    gate: futarchy_primitives::GateType::Survival,
                },
                futarchy_primitives::FixedU64(0),
                end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                gates[2],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Accept,
                    gate: futarchy_primitives::GateType::Security,
                },
                futarchy_primitives::FixedU64(0),
                end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                gates[3],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Reject,
                    gate: futarchy_primitives::GateType::Security,
                },
                futarchy_primitives::FixedU64(0),
                end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                ids.baseline,
                pallet_market::core_market::BookKind::Baseline { epoch },
                futarchy_primitives::FixedU64(500_000_000),
                end,
                (params.decision_window, params.trailing_window),
                baseline_b,
                contest,
            ),
        ] {
            assert_ok!(result);
        }
        pallet_market::BaselineMarketOf::<Runtime>::insert(epoch, ids.baseline);
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = contest.saturating_mul(100);
        });
        let proposal = Proposal {
            id: PID,
            proposer: account(70),
            class: ProposalClass::Treasury,
            state: ProposalState::Trading,
            epoch,
            submitted_at: 0,
            payload_hash: payload_hash.0,
            payload_len,
            ask: 0,
            bond: Balance::MAX,
            resources: match futarchy_primitives::BoundedVec::try_from(vec![resource]) {
                Ok(resources) => resources,
                Err(_) => {
                    assert!(false, "one canonical resource must fit");
                    return;
                }
            },
            metric_spec: 1,
            decide_at: end,
            rerun: false,
            extended: false,
            delayed_once: false,
            markets: Some(ids),
            maturity: None,
            grace_end: None,
            version_constraint: Some(version_constraint),
            decision: None,
        };
        pallet_epoch::Proposals::<Runtime>::insert(PID, proposal);
        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        pallet_epoch::ProposalSchedules::<Runtime>::insert(
            PID,
            pallet_epoch::ProposalSchedule {
                epoch,
                epoch_start_block: schedule.epoch_start_block,
                epoch_length: schedule.length,
                decide_at: end,
                metric_spec: 1,
            },
        );
        pallet_epoch::NextProposalId::<Runtime>::mutate(|next| {
            *next = (*next).max(PID.saturating_add(1));
        });
        pallet_epoch::ResourceLocks::<Runtime>::put(pallet_epoch::Locks::truncate_from(vec![(
            resource, PID,
        )]));
        pallet_conditional_ledger::Vaults::<Runtime>::insert(
            PID,
            pallet_conditional_ledger::core_ledger::VaultInfo::open(1),
        );

        // B2 no-drift lock: the public view and the crank consume the same
        // pallet-epoch DecisionInputSnapshot assembly. Pin every observable
        // TWAP before the crank and retain the real decide/enqueue assertion.
        let snapshot = match Epoch::decision_input_snapshot(PID) {
            Some(snapshot) => snapshot,
            None => {
                assert!(false, "complete decision snapshot must be readable");
                return;
            }
        };
        let stats = match crate::views::decision_stats(PID) {
            Some(stats) => stats,
            None => {
                assert!(false, "complete decision statistics must be exposed");
                return;
            }
        };
        assert!(snapshot.backing_complete);
        assert_eq!(stats.twap_accept_1e9, snapshot.inputs.accept_full);
        assert_eq!(stats.twap_reject_1e9, snapshot.inputs.reject_full);
        assert_eq!(stats.trailing_accept_1e9, snapshot.inputs.accept_trailing);
        assert_eq!(stats.trailing_reject_1e9, snapshot.inputs.reject_trailing);
        assert_eq!(stats.twap_baseline_1e9, snapshot.inputs.baseline_full);
        assert_eq!(stats.gate_twaps_1e9, snapshot.inputs.gate_twaps);
        assert_eq!(stats.coverage_pct, 100);
        assert_eq!(stats.traded_volume, contest);
        assert_eq!(stats.v_min_required, contest);
        assert_eq!(stats.in_cap_prize, 0);
        assert!(stats.converged);
        assert_eq!(
            stats.attack_cost_hat,
            pallet_epoch::attack_cost_hat(
                snapshot.inputs.measured_depth,
                snapshot.inputs.published_flow_per_day,
                snapshot.params.decision_window,
            )
            .expect("fixture depth arithmetic is bounded")
        );

        assert_ok!(Epoch::decide(RuntimeOrigin::signed(account(69)), PID));
        assert_eq!(
            pallet_epoch::Proposals::<Runtime>::get(PID).map(|proposal| proposal.state),
            Some(ProposalState::Queued),
        );
        assert!(pallet_execution_guard::Queue::<Runtime>::contains_key(PID));
    });
}

#[test]
fn delayed_decide_uses_own_baseline_window_before_classless_queue_refusal() {
    development_ext().execute_with(|| {
        const EARLY_PID: futarchy_primitives::ProposalId = 8_020;
        const LATE_PID: futarchy_primitives::ProposalId = 8_021;
        let params =
            <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        let early_end = params.decision_window;
        let late_end = early_end.saturating_add(params.decision_window);
        System::set_block_number(late_end);
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        let early_markets = MarketSet {
            accept: 82_001,
            reject: 82_002,
            gates: Some([82_003, 82_004, 82_005, 82_006]),
            baseline: 82_007,
        };
        let late_markets = MarketSet {
            accept: 82_011,
            reject: 82_012,
            gates: Some([82_013, 82_014, 82_015, 82_016]),
            baseline: early_markets.baseline,
        };
        let class_index = crate::configs::proposal_class_index(ProposalClass::Treasury);
        let contest = params.v_min[class_index];
        let gate_contest = params.gate_v_min[class_index];
        let decision_b = crate::configs::class_pol_floor(ProposalClass::Treasury);
        let gate_b = crate::configs::balance_param(b"pol.b_gate");
        let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
        let early_gates = early_markets
            .gates
            .expect("Treasury fixture has gate books");
        for result in [
            seed_decision_grade_market(
                early_markets.accept,
                pallet_market::core_market::BookKind::Decision {
                    proposal: EARLY_PID,
                    branch: futarchy_primitives::Branch::Accept,
                },
                futarchy_primitives::FixedU64(700_000_000),
                early_end,
                (params.decision_window, params.trailing_window),
                decision_b,
                contest,
            ),
            seed_decision_grade_market(
                early_markets.reject,
                pallet_market::core_market::BookKind::Decision {
                    proposal: EARLY_PID,
                    branch: futarchy_primitives::Branch::Reject,
                },
                futarchy_primitives::FixedU64(500_000_000),
                early_end,
                (params.decision_window, params.trailing_window),
                decision_b,
                contest,
            ),
            seed_decision_grade_market(
                early_gates[0],
                pallet_market::core_market::BookKind::Gate {
                    proposal: EARLY_PID,
                    branch: futarchy_primitives::Branch::Accept,
                    gate: futarchy_primitives::GateType::Survival,
                },
                futarchy_primitives::FixedU64(0),
                early_end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                early_gates[1],
                pallet_market::core_market::BookKind::Gate {
                    proposal: EARLY_PID,
                    branch: futarchy_primitives::Branch::Reject,
                    gate: futarchy_primitives::GateType::Survival,
                },
                futarchy_primitives::FixedU64(0),
                early_end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                early_gates[2],
                pallet_market::core_market::BookKind::Gate {
                    proposal: EARLY_PID,
                    branch: futarchy_primitives::Branch::Accept,
                    gate: futarchy_primitives::GateType::Security,
                },
                futarchy_primitives::FixedU64(0),
                early_end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                early_gates[3],
                pallet_market::core_market::BookKind::Gate {
                    proposal: EARLY_PID,
                    branch: futarchy_primitives::Branch::Reject,
                    gate: futarchy_primitives::GateType::Security,
                },
                futarchy_primitives::FixedU64(0),
                early_end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_two_window_baseline(
                early_markets.baseline,
                epoch,
                EARLY_PID,
                LATE_PID,
                early_end,
                late_end,
                params.decision_window,
                params.trailing_window,
                futarchy_primitives::FixedU64(500_000_000),
                // If the delayed early crank incorrectly consumes this later
                // window, the 0.9 Baseline floor defeats its 0.7 Accept book.
                futarchy_primitives::FixedU64(900_000_000),
                baseline_b,
                contest,
            ),
        ] {
            assert_ok!(result);
        }
        pallet_market::BaselineMarketOf::<Runtime>::insert(epoch, early_markets.baseline);
        assert_eq!(
            <crate::configs::RuntimeMarketAccess as pallet_epoch::MarketAccess<AccountId>>::twap_full_at(
                early_markets.baseline,
                early_end,
            ),
            Some(futarchy_primitives::FixedU64(500_000_000)),
        );
        assert_eq!(
            <crate::configs::RuntimeMarketAccess as pallet_epoch::MarketAccess<AccountId>>::twap_full_at(
                early_markets.baseline,
                late_end,
            ),
            Some(futarchy_primitives::FixedU64(900_000_000)),
        );

        let batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(Vec::new()) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "empty guard batch must fit");
                    return;
                }
            };
        let bytes = batch.encode();
        let payload_len = match u32::try_from(bytes.len()) {
            Ok(len) => len,
            Err(_) => {
                assert!(false, "bounded payload length must fit u32");
                return;
            }
        };
        let payload_hash = match <Preimage as StorePreimage>::note(bytes.into()) {
            Ok(hash) => hash,
            Err(error) => {
                assert!(false, "payload preimage must be noted: {error:?}");
                return;
            }
        };
        <Preimage as QueryPreimage>::request(&payload_hash);
        let version = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => {
                assert!(false, "guard genesis must bind a runtime version");
                return;
            }
        };
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = contest.saturating_mul(100);
        });

        let proposal = |id, decide_at, markets| Proposal {
            id,
            proposer: account(149),
            class: ProposalClass::Treasury,
            state: ProposalState::Trading,
            epoch,
            submitted_at: 0,
            payload_hash: payload_hash.0,
            payload_len,
            ask: 0,
            bond: Balance::MAX,
            resources: Default::default(),
            metric_spec: 1,
            decide_at,
            rerun: false,
            extended: false,
            delayed_once: false,
            markets: Some(markets),
            maturity: None,
            grace_end: None,
            version_constraint: Some(version.clone()),
            decision: None,
        };
        pallet_epoch::Proposals::<Runtime>::insert(
            EARLY_PID,
            proposal(EARLY_PID, early_end, early_markets),
        );
        pallet_epoch::Proposals::<Runtime>::insert(
            LATE_PID,
            proposal(LATE_PID, late_end, late_markets),
        );
        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        pallet_epoch::ProposalSchedules::<Runtime>::insert(
            EARLY_PID,
            pallet_epoch::ProposalSchedule {
                epoch,
                epoch_start_block: schedule.epoch_start_block,
                epoch_length: schedule.length,
                decide_at: early_end,
                metric_spec: 1,
            },
        );
        pallet_conditional_ledger::Vaults::<Runtime>::insert(
            EARLY_PID,
            pallet_conditional_ledger::core_ledger::VaultInfo::open(1),
        );
        pallet_epoch::NextProposalId::<Runtime>::mutate(|next| {
            *next = (*next).max(LATE_PID.saturating_add(1));
        });

        assert_ok!(Epoch::decide(
            RuntimeOrigin::signed(account(150)),
            EARLY_PID,
        ));
        assert_eq!(
            pallet_epoch::Proposals::<Runtime>::get(EARLY_PID).map(|proposal| proposal.state),
            Some(ProposalState::Measuring),
            "the earlier window must pass every market check and reach queue-time refusal",
        );
        assert_eq!(
            pallet_epoch::Proposals::<Runtime>::get(EARLY_PID)
                .and_then(|proposal| proposal.decision),
            Some(DecisionOutcome::Reject(RejectReason::RateLimited)),
            "using the later Baseline window would fail before the class-less queue check",
        );
        assert!(!pallet_execution_guard::Queue::<Runtime>::contains_key(EARLY_PID));
    });
}

fn enact_passing_referendum(index: u32) {
    let voter = account(199);
    let voting_balance = Balances::total_issuance().saturating_mul(10);
    assert_ok!(Balances::force_set_balance(
        RuntimeOrigin::root(),
        MultiAddress::Id(voter.clone()),
        voting_balance,
    ));
    assert_ok!(ConvictionVoting::vote(
        RuntimeOrigin::signed(voter),
        index,
        pallet_conviction_voting::AccountVote::Standard {
            vote: pallet_conviction_voting::Vote {
                aye: true,
                conviction: pallet_conviction_voting::Conviction::Locked1x,
            },
            balance: voting_balance,
        },
    ));
    let Some(pallet_referenda::ReferendumInfo::Ongoing(status)) =
        pallet_referenda::ReferendumInfoFor::<Runtime>::get(index)
    else {
        assert!(false, "referendum must be ongoing before preparation ends");
        return;
    };
    let prepare_at = status.submitted.saturating_add(
        crate::configs::TRACKS[usize::from(status.track)]
            .info
            .prepare_period,
    );
    System::set_block_number(prepare_at);
    assert_ok!(Referenda::nudge_referendum(RuntimeOrigin::root(), index));
    let Some(pallet_referenda::ReferendumInfo::Ongoing(status)) =
        pallet_referenda::ReferendumInfoFor::<Runtime>::get(index)
    else {
        assert!(false, "referendum must enter deciding");
        return;
    };
    let Some(confirm_at) = status.deciding.and_then(|deciding| deciding.confirming) else {
        assert!(false, "supermajority vote must enter confirmation");
        return;
    };
    let enactment_delay = crate::configs::TRACKS[usize::from(status.track)]
        .info
        .min_enactment_period;
    System::set_block_number(confirm_at);
    assert_ok!(Referenda::nudge_referendum(RuntimeOrigin::root(), index));
    assert!(matches!(
        pallet_referenda::ReferendumInfoFor::<Runtime>::get(index),
        Some(pallet_referenda::ReferendumInfo::Approved(..))
    ));
    // A zero-minimum track schedules at the approval block, whose scheduler
    // hook has already run in this direct test harness; execute it next block.
    let enact_at = confirm_at.saturating_add(enactment_delay.max(1));
    System::set_block_number(enact_at);
    let _ = Scheduler::on_initialize(enact_at);
}

fn seed_guardian_delay_action(
    pid: futarchy_primitives::ProposalId,
    first_member_seed: u8,
) -> Option<([AccountId; pallet_guardian::GUARDIAN_SEATS], u32, u32, u32)> {
    System::set_block_number(System::block_number().max(1));
    let members = core::array::from_fn(|index| account(first_member_seed + index as u8));
    for member in &members {
        assert_ok!(Balances::force_set_balance(
            RuntimeOrigin::root(),
            MultiAddress::Id(member.clone()),
            pallet_guardian::GUARDIAN_BOND.saturating_add(currency::VIT),
        ));
    }
    assert_ok!(Guardian::set_members(
        pallet_origins::Origin::ConstitutionalValues.into(),
        members.clone(),
    ));
    let version_constraint = pallet_execution_guard::CurrentSpecName::<Runtime>::get()?;
    let payload = pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![
        RuntimeCall::System(frame_system::Call::remark {
            remark: b"guardian-delay-review".to_vec(),
        }),
    ])
    .ok()?;
    let payload_bytes = payload.encode();
    let payload_len = u32::try_from(payload_bytes.len()).ok()?;
    let payload_hash = <Preimage as StorePreimage>::note(payload_bytes.into()).ok()?;
    let maturity = System::block_number().checked_add(
        <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_timelock(
            ProposalClass::Treasury,
        ),
    )?;
    let grace_end = maturity.checked_add(
        <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_grace(
            ProposalClass::Treasury,
        ),
    )?;
    assert_ok!(seed_queued_epoch_proposal(
        pid,
        ProposalClass::Treasury,
        payload_hash,
        payload_len,
        maturity,
        grace_end,
        version_constraint.clone(),
    ));
    assert_ok!(ExecutionGuard::enqueue(
        RuntimeOrigin::signed(crate::configs::epoch_account()),
        pallet_execution_guard::StoredQueuedExecution {
            pid,
            payload_hash: payload_hash.0,
            payload_len,
            class: ProposalClass::Treasury,
            maturity,
            grace_end,
            version_constraint,
            meters_declared: Default::default(),
            ratify_ref: None,
            ratification_passed: false,
            attestation_id: None,
            pre_upgrade_checkpoint: None,
            cancelled: false,
            declared_domains: Default::default(),
            failed_at: None,
        },
        false,
    ));
    assert_ok!(Guardian::propose_action(
        RuntimeOrigin::signed(members[0].clone()),
        pallet_guardian::GuardianPower::DelayOnce { pid },
        H256::repeat_byte(first_member_seed.saturating_add(1)).into(),
    ));
    let action = pallet_guardian::NextActionId::<Runtime>::get().saturating_sub(1);
    for member in members.iter().take(5).skip(1) {
        assert_ok!(Guardian::approve_action(
            RuntimeOrigin::signed(member.clone()),
            action,
        ));
    }
    let referendum = pallet_guardian::ReviewReferenda::<Runtime>::get(action)?;
    let veto_referendum = pallet_guardian::VetoReviewReferenda::<Runtime>::get(action)?;
    Some((members, action, referendum, veto_referendum))
}

fn assert_guardian_review_referendum(index: u32, action: u32, uphold_veto: bool) {
    let Some(pallet_referenda::ReferendumInfo::Ongoing(status)) =
        pallet_referenda::ReferendumInfoFor::<Runtime>::get(index)
    else {
        assert!(false, "guardian review {index} must be ongoing");
        return;
    };
    assert_eq!(status.track, 4);
    assert_eq!(status.submission_deposit.amount, currency::VIT);
    assert_eq!(
        status.decision_deposit.map(|deposit| deposit.amount),
        Some(1_000 * currency::VIT)
    );
    let Ok((call, _)) = <Preimage as QueryPreimage>::peek(&status.proposal) else {
        assert!(false, "guardian review {index} preimage must decode");
        return;
    };
    if uphold_veto {
        assert!(matches!(
            call,
            RuntimeCall::Guardian(pallet_guardian::Call::uphold_veto { action_id })
                if action_id == action
        ));
    } else {
        assert!(matches!(
            call,
            RuntimeCall::Guardian(pallet_guardian::Call::ratify_action { action_id })
                if action_id == action
        ));
    }
}

#[test]
fn guardian_review_votes_and_enacts_on_ratify_track_with_bonds_restored() {
    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_009;
        let Some((members, action, referendum, veto_referendum)) =
            seed_guardian_delay_action(PID, 80)
        else {
            assert!(false, "delay action must schedule both review verdicts");
            return;
        };
        assert_guardian_review_referendum(referendum, action, false);
        assert_guardian_review_referendum(veto_referendum, action, true);
        assert!(pallet_guardian::ReviewFrontingOf::<Runtime>::contains_key(
            action
        ));

        enact_passing_referendum(referendum);

        assert!(pallet_guardian::ReviewDeadlines::<Runtime>::get()
            .iter()
            .any(|review| review.action_id == action && review.ratified));
        assert!(!pallet_guardian::ReviewFrontingOf::<Runtime>::contains_key(
            action
        ));
        assert!(!pallet_guardian::ReviewReferenda::<Runtime>::contains_key(
            action
        ));
        assert!(!pallet_guardian::VetoReviewReferenda::<Runtime>::contains_key(action));
        assert!(matches!(
            pallet_referenda::ReferendumInfoFor::<Runtime>::get(referendum),
            Some(pallet_referenda::ReferendumInfo::Approved(_, None, None))
        ));
        assert!(matches!(
            pallet_referenda::ReferendumInfoFor::<Runtime>::get(veto_referendum),
            Some(pallet_referenda::ReferendumInfo::Cancelled(_, None, None))
        ));
        let reason: crate::RuntimeHoldReason = pallet_guardian::HoldReason::SeatBond.into();
        for member in members.iter().take(5) {
            assert_eq!(
                Balances::balance_on_hold(&reason, member),
                pallet_guardian::GUARDIAN_BOND
            );
        }

        let Some(deadline) = pallet_epoch::GuardianReviewDeadlines::<Runtime>::get(PID) else {
            assert!(false, "ratification keeps the T12 review deadline");
            return;
        };
        pallet_epoch::EpochOf::<Runtime>::mutate(|clock| clock.index = deadline);
        let Ok(batch) = pallet_epoch::TickBatch::try_from(vec![PID]) else {
            assert!(false, "one proposal fits the tick bound");
            return;
        };
        assert_ok!(Epoch::tick(RuntimeOrigin::signed(account(81)), batch));
        assert_eq!(
            pallet_epoch::Proposals::<Runtime>::get(PID).map(|proposal| proposal.state),
            Some(ProposalState::Rerun),
            "ratifying the delay preserves the ordinary T12 rerun path"
        );
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn guardian_uphold_veto_cancels_ongoing_review_and_commits_t24() {
    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_007;
        let Some((members, action, referendum, veto_referendum)) =
            seed_guardian_delay_action(PID, 60)
        else {
            assert!(false, "delay action must schedule both review verdicts");
            return;
        };
        assert!(matches!(
            pallet_referenda::ReferendumInfoFor::<Runtime>::get(referendum),
            Some(pallet_referenda::ReferendumInfo::Ongoing(_))
        ));

        enact_passing_referendum(veto_referendum);

        assert!(matches!(
            pallet_referenda::ReferendumInfoFor::<Runtime>::get(referendum),
            Some(pallet_referenda::ReferendumInfo::Cancelled(_, None, None))
        ));
        assert!(matches!(
            pallet_referenda::ReferendumInfoFor::<Runtime>::get(veto_referendum),
            Some(pallet_referenda::ReferendumInfo::Approved(_, None, None))
        ));

        let proposal = pallet_epoch::Proposals::<Runtime>::get(PID)
            .expect("vetoed proposal enters measurement");
        assert_eq!(proposal.state, ProposalState::Measuring);
        assert_eq!(
            proposal.decision,
            Some(DecisionOutcome::Reject(RejectReason::VetoUpheldByReview))
        );
        assert!(!pallet_epoch::GuardianReviewDeadlines::<Runtime>::contains_key(PID));
        assert!(pallet_guardian::ReviewDeadlines::<Runtime>::get()
            .iter()
            .any(|review| review.action_id == action && review.ratified));
        let reason: crate::RuntimeHoldReason = pallet_guardian::HoldReason::SeatBond.into();
        for member in members.iter().take(5) {
            assert_eq!(
                Balances::balance_on_hold(&reason, member),
                pallet_guardian::GUARDIAN_BOND
            );
        }
        assert!(!pallet_guardian::ReviewFrontingOf::<Runtime>::contains_key(
            action
        ));
        assert!(!pallet_guardian::ReviewReferenda::<Runtime>::contains_key(
            action
        ));
        assert!(!pallet_guardian::VetoReviewReferenda::<Runtime>::contains_key(action));
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn uphold_veto_after_rerun_is_benign_and_plain_ratification_still_works() {
    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_008;
        let Some((_, action, referendum, _)) = seed_guardian_delay_action(PID, 70) else {
            assert!(false, "delay action must schedule both review verdicts");
            return;
        };
        // The epoch T12/T13 path owns reopening and its queue handoff; seed the
        // resulting state here so this test isolates the review-verdict race.
        pallet_epoch::Proposals::<Runtime>::mutate(PID, |proposal| {
            if let Some(proposal) = proposal {
                proposal.state = ProposalState::Extended;
                proposal.rerun = true;
            }
        });
        pallet_epoch::GuardianReviewDeadlines::<Runtime>::remove(PID);
        assert_eq!(
            pallet_epoch::Proposals::<Runtime>::get(PID).map(|proposal| proposal.state),
            Some(ProposalState::Extended)
        );
        assert_noop!(
            Guardian::uphold_veto(crate::track_origins::Origin::Ratify.into(), action),
            pallet_epoch::Error::<Runtime>::BadState
        );
        assert!(pallet_guardian::ReviewDeadlines::<Runtime>::get()
            .iter()
            .any(|review| review.action_id == action && !review.ratified));
        assert_ok!(Referenda::cancel(
            pallet_origins::Origin::ConstitutionalValues.into(),
            referendum,
        ));
        assert_ok!(Guardian::ratify_action(
            crate::track_origins::Origin::Ratify.into(),
            action,
        ));
        assert!(pallet_guardian::ReviewDeadlines::<Runtime>::get()
            .iter()
            .any(|review| review.action_id == action && review.ratified));
    });
}

#[test]
fn guardian_track_scope_is_enforced_and_underfunded_election_is_atomic() {
    development_ext().execute_with(|| {
        assert_noop!(
            Guardian::ratify_action(crate::track_origins::Origin::Metric.into(), 0),
            DispatchError::BadOrigin
        );
        let members = core::array::from_fn(|index| account(210 + index as u8));
        for member in &members {
            assert_ok!(Balances::force_set_balance(
                RuntimeOrigin::root(),
                MultiAddress::Id(member.clone()),
                currency::VIT,
            ));
        }
        for member in members.iter().take(6) {
            assert_ok!(Balances::force_set_balance(
                RuntimeOrigin::root(),
                MultiAddress::Id(member.clone()),
                pallet_guardian::GUARDIAN_BOND.saturating_add(currency::VIT),
            ));
        }
        assert!(Guardian::set_members(
            crate::track_origins::Origin::GuardianTrack.into(),
            members.clone(),
        )
        .is_err());
        assert!(Guardian::members().is_none());
        let reason: crate::RuntimeHoldReason = pallet_guardian::HoldReason::SeatBond.into();
        for member in members.iter().take(6) {
            assert_eq!(Balances::balance_on_hold(&reason, member), 0);
        }

        assert_ok!(Balances::force_set_balance(
            RuntimeOrigin::root(),
            MultiAddress::Id(members[6].clone()),
            pallet_guardian::GUARDIAN_BOND.saturating_add(currency::VIT),
        ));
        assert_ok!(Guardian::set_members(
            crate::track_origins::Origin::GuardianTrack.into(),
            members.clone(),
        ));
        assert_eq!(Guardian::members(), Some(members.clone().map(Some)));
        assert_noop!(
            Guardian::set_members(crate::track_origins::Origin::Ratify.into(), members),
            DispatchError::BadOrigin
        );
    });
}

#[test]
fn fifth_guardian_delay_approval_dispatches_epoch_effect_and_schedules_real_review() {
    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_010;
        System::set_block_number(System::block_number().max(1));
        pallet_epoch::EpochOf::<Runtime>::mutate(|clock| {
            clock.index = clock.index.saturating_add(2)
        });
        let review_start_epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        let amended_review_deadline = 3_u32;
        assert_ok!(Constitution::set_param(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::key16(b"grd.review_dl"),
            pallet_constitution::ParamValue::U32(amended_review_deadline),
        ));
        let members = [
            account(101),
            account(102),
            account(103),
            account(104),
            account(105),
            account(106),
            account(107),
        ];
        for member in &members {
            assert_ok!(Balances::force_set_balance(
                RuntimeOrigin::root(),
                MultiAddress::Id(member.clone()),
                pallet_guardian::GUARDIAN_BOND.saturating_add(currency::VIT),
            ));
        }
        assert_ok!(Guardian::set_members(
            pallet_origins::Origin::ConstitutionalValues.into(),
            members.clone(),
        ));
        let version_constraint = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => {
                assert!(false, "guard genesis must bind a runtime version");
                return;
            }
        };
        let maturity = System::block_number().saturating_add(
            <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_timelock(
                ProposalClass::Treasury,
            ),
        );
        let grace_end = maturity.saturating_add(
            <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_grace(
                ProposalClass::Treasury,
            ),
        );
        assert_ok!(seed_queued_epoch_proposal(
            PID,
            ProposalClass::Treasury,
            H256::repeat_byte(9),
            1,
            maturity,
            grace_end,
            version_constraint,
        ));
        let before_referenda = pallet_referenda::ReferendumCount::<Runtime>::get();
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(members[0].clone()),
            pallet_guardian::GuardianPower::DelayOnce { pid: PID },
            H256::repeat_byte(7).into(),
        ));
        let action = pallet_guardian::NextActionId::<Runtime>::get().saturating_sub(1);
        for member in members.iter().take(5).skip(1) {
            assert_ok!(Guardian::approve_action(
                RuntimeOrigin::signed(member.clone()),
                action,
            ));
        }

        assert_eq!(
            pallet_epoch::Proposals::<Runtime>::get(PID).map(|proposal| proposal.state),
            Some(ProposalState::Suspended),
        );
        assert!(pallet_epoch::GuardianReviewDeadlines::<Runtime>::contains_key(PID));
        assert_eq!(
            pallet_referenda::ReferendumCount::<Runtime>::get(),
            before_referenda.saturating_add(2),
        );
        assert_eq!(
            pallet_guardian::ReviewReferenda::<Runtime>::get(action),
            Some(before_referenda),
        );
        assert_eq!(
            pallet_guardian::ReviewDeadlines::<Runtime>::get()
                .iter()
                .find(|review| review.action_id == action)
                .map(|review| review.deadline_epoch),
            Some(review_start_epoch.saturating_add(amended_review_deadline)),
        );
        assert_eq!(
            pallet_guardian::VetoReviewReferenda::<Runtime>::get(action),
            Some(before_referenda.saturating_add(1)),
        );
        assert_guardian_review_referendum(before_referenda, action, false);
        assert_guardian_review_referendum(before_referenda.saturating_add(1), action, true);
        let deadline = match pallet_epoch::GuardianReviewDeadlines::<Runtime>::get(PID) {
            Some(deadline) => deadline,
            None => {
                assert!(false, "delay-once must persist its review deadline");
                return;
            }
        };
        assert_eq!(
            deadline,
            review_start_epoch.saturating_add(amended_review_deadline),
        );
        assert!(
            !<crate::configs::RuntimeEpochGuardian as pallet_epoch::GuardianAccess>::review_window_closed(PID),
        );
        pallet_epoch::EpochOf::<Runtime>::mutate(|clock| clock.index = deadline);
        assert!(
            <crate::configs::RuntimeEpochGuardian as pallet_epoch::GuardianAccess>::review_window_closed(PID),
        );

        let bonds_before = pallet_guardian::MemberBonds::<Runtime>::get();
        let treasury_before = Balances::balance(&crate::genesis::treasury_account());
        let guardian_before = <Balances as FungibleInspect<AccountId>>::total_balance(
            &crate::configs::guardian_account(),
        );
        pallet_epoch::EpochOf::<Runtime>::mutate(|clock| clock.index = deadline);
        let _ = Guardian::on_initialize(System::block_number());
        let bonds_after = pallet_guardian::MemberBonds::<Runtime>::get();
        let slash = pallet_guardian::GUARDIAN_BOND
            .saturating_mul(Balance::from(pallet_guardian::REVIEW_SLASH_PERCENT))
            / 100;
        for index in 0..5 {
            assert_eq!(
                bonds_after[index],
                bonds_before[index].saturating_sub(slash),
            );
        }
        for index in 5..pallet_guardian::GUARDIAN_SEATS {
            assert_eq!(bonds_after[index], bonds_before[index]);
        }
        let reason: crate::RuntimeHoldReason = pallet_guardian::HoldReason::SeatBond.into();
        for member in members.iter().take(5) {
            assert_eq!(Balances::balance_on_hold(&reason, member), slash);
        }
        for member in members.iter().skip(5) {
            assert_eq!(
                Balances::balance_on_hold(&reason, member),
                pallet_guardian::GUARDIAN_BOND
            );
        }
        assert!(pallet_guardian::ReviewDeadlines::<Runtime>::get().is_empty());
        assert!(!pallet_guardian::ReviewReferenda::<Runtime>::contains_key(action));
        assert!(!pallet_guardian::VetoReviewReferenda::<Runtime>::contains_key(
            action
        ));
        assert!(!pallet_guardian::ReviewFrontingOf::<Runtime>::contains_key(
            action
        ));
        assert!(matches!(
            pallet_referenda::ReferendumInfoFor::<Runtime>::get(before_referenda),
            Some(pallet_referenda::ReferendumInfo::Cancelled(_, None, None))
        ));
        assert!(matches!(
            pallet_referenda::ReferendumInfoFor::<Runtime>::get(
                before_referenda.saturating_add(1)
            ),
            Some(pallet_referenda::ReferendumInfo::Cancelled(_, None, None))
        ));
        let failed = pallet_guardian::FailedActions::<Runtime>::get(action)
            .expect("deadline writes the recall substrate");
        let recall = failed
            .recall_referendum
            .expect("deadline schedules a real recall referendum");
        let recall_info = pallet_referenda::ReferendumInfoFor::<Runtime>::get(recall);
        let Some(pallet_referenda::ReferendumInfo::Ongoing(recall_status)) = recall_info else {
            assert!(false, "recall referendum must be ongoing");
            return;
        };
        assert_eq!(recall_status.track, 3);
        assert_eq!(
            recall_status
                .decision_deposit
                .as_ref()
                .map(|deposit| deposit.amount),
            Some(5_000 * currency::VIT),
        );
        let recall_deposits = 5_001 * currency::VIT;
        assert_eq!(guardian_before, 2_002 * currency::VIT);
        assert_eq!(
            <Balances as FungibleInspect<AccountId>>::total_balance(
                &crate::configs::guardian_account()
            ),
            recall_deposits,
            "only the recall deposits may remain with the guardian sovereign"
        );
        assert_eq!(
            Balances::balance(&crate::genesis::treasury_account()),
            treasury_before
                .saturating_add(slash.saturating_mul(5))
                .saturating_sub(recall_deposits),
        );
        let events = System::events();
        assert!(
            events.iter().any(|record| matches!(
                record.event,
                crate::RuntimeEvent::Guardian(pallet_guardian::Event::ReviewFailed {
                    action: failed,
                    slashed_each,
                }) if failed == action && slashed_each == slash
            )),
            "guardian accountability signal missing from {events:?}",
        );
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Guardian(pallet_guardian::Event::RecallScheduled {
                action: recalled,
                referendum,
            }) if recalled == action && referendum == recall
        )));
        assert_ok!(Guardian::do_try_state());

        enact_passing_referendum(recall);
        let seated = Guardian::members().expect("council remains initialized");
        assert_eq!(seated.iter().filter(|member| member.is_some()).count(), 2);
        assert!(!pallet_guardian::FailedActions::<Runtime>::contains_key(action));
        assert_eq!(
            Balances::balance(&crate::genesis::treasury_account()),
            treasury_before.saturating_add(slash.saturating_mul(5)),
            "recall refunds both deposits into treasury MAIN",
        );

        pallet_epoch::EpochOf::<Runtime>::mutate(|clock| {
            clock.index = deadline.saturating_add(1)
        });
        let _ = Guardian::on_initialize(System::block_number());
        for member in members.iter().take(5) {
            assert_eq!(Balances::balance_on_hold(&reason, member), 0);
        }
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(members[5].clone()),
            pallet_guardian::GuardianPower::DelayOnce { pid: PID },
            H256::repeat_byte(8).into(),
        ));
        let blocked = pallet_guardian::NextActionId::<Runtime>::get().saturating_sub(1);
        assert_ok!(Guardian::approve_action(
            RuntimeOrigin::signed(members[6].clone()),
            blocked,
        ));
        assert!(!pallet_guardian::PendingActions::<Runtime>::get()
            .iter()
            .find(|pending| pending.id == blocked)
            .is_some_and(|pending| pending.dispatched));
    });
}

#[test]
fn completed_guardian_rerun_flag_does_not_permanently_hold_execution() {
    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_011;
        let version_constraint = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => {
                assert!(false, "guard genesis must bind a runtime version");
                return;
            }
        };
        assert_ok!(seed_queued_epoch_proposal(
            PID,
            ProposalClass::Treasury,
            H256::repeat_byte(8),
            1,
            System::block_number(),
            System::block_number(),
            version_constraint,
        ));
        pallet_epoch::Proposals::<Runtime>::mutate(PID, |proposal| {
            if let Some(proposal) = proposal {
                proposal.rerun = true;
                proposal.state = ProposalState::Extended;
            }
        });
        assert!(
            !<crate::configs::RuntimeGuardianState as pallet_execution_guard::GuardianState>::rerun_held(PID),
        );
        pallet_epoch::Proposals::<Runtime>::mutate(PID, |proposal| {
            if let Some(proposal) = proposal {
                proposal.state = ProposalState::Rerun;
            }
        });
        assert!(
            <crate::configs::RuntimeGuardianState as pallet_execution_guard::GuardianState>::rerun_held(PID),
        );
    });
}

#[test]
fn suspended_code_rerun_preserves_ratification_attestation_and_one_continuous_pin() {
    use pallet_epoch::ExecutionGuardAccess;

    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_012;
        const RATIFY_REF: u32 = 93;
        let candidate = b"a8-r1-suspended-rerun".to_vec();
        let (_, _) = match enqueue_attested_code_upgrade(PID, &candidate, RATIFY_REF) {
            Some(setup) => setup,
            None => {
                assert!(false, "ratified queued CODE fixture must be constructible");
                return;
            }
        };
        let queued_before = match pallet_execution_guard::Queue::<Runtime>::get(PID) {
            Some(queued) => queued,
            None => {
                assert!(false, "CODE fixture must be queued");
                return;
            }
        };
        <Preimage as StorePreimage>::unnote(&H256::from(queued_before.payload_hash));
        let pin_before = preimage_request_count(queued_before.payload_hash);
        assert_eq!(pin_before, 1, "the execution guard must own one live pin");
        let attestation_before = pallet_execution_guard::AttestationBindings::<Runtime>::get(PID);
        let ratification_before = pallet_execution_guard::Ratifications::<Runtime>::get(PID);
        assert!(attestation_before.is_some());
        assert!(ratification_before.is_some());

        assert_ok!(Epoch::delay_once(
            pallet_origins::Origin::GuardianHold.into(),
            PID,
            H256::repeat_byte(61).into(),
        ));
        let current_epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        pallet_epoch::GuardianReviewDeadlines::<Runtime>::insert(PID, current_epoch);
        let batch = match pallet_epoch::TickBatch::try_from(vec![PID]) {
            Ok(batch) => batch,
            Err(_) => {
                assert!(false, "single suspended-rerun crank must fit");
                return;
            }
        };
        assert_ok!(Epoch::tick(RuntimeOrigin::signed(account(142)), batch));
        assert_eq!(
            pallet_epoch::Proposals::<Runtime>::get(PID).map(|proposal| proposal.state),
            Some(ProposalState::Rerun),
        );
        assert!(!pallet_execution_guard::Queue::<Runtime>::contains_key(PID));
        assert_eq!(
            pallet_execution_guard::Ratifications::<Runtime>::get(PID),
            ratification_before,
        );
        assert_eq!(
            pallet_execution_guard::AttestationBindings::<Runtime>::get(PID),
            attestation_before,
        );
        assert_eq!(
            preimage_request_count(queued_before.payload_hash),
            pin_before
        );

        // Model the successful rerun decision's T9 persistence edge. The
        // runtime adapter must be able to re-enqueue with the surviving
        // ratification, and must not take a second request on the same pin.
        let now = System::block_number();
        let maturity = now.saturating_add(
            <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_timelock(
                ProposalClass::Code,
            ),
        );
        let grace = <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_grace(
            ProposalClass::Code,
        );
        pallet_epoch::Proposals::<Runtime>::mutate(PID, |proposal| {
            if let Some(proposal) = proposal {
                proposal.state = ProposalState::Queued;
                proposal.decision = Some(DecisionOutcome::Adopt);
                proposal.maturity = Some(maturity);
                proposal.grace_end = Some(maturity.saturating_add(grace));
            }
        });
        assert_ok!(
            <crate::configs::RuntimeEpochExecutionGuard as ExecutionGuardAccess>::enqueue(
                PID,
                queued_before.payload_hash,
                Some(queued_before.version_constraint.clone()),
                maturity,
                grace,
                true,
            ),
        );
        let requeued = match pallet_execution_guard::Queue::<Runtime>::get(PID) {
            Some(queued) => queued,
            None => {
                assert!(false, "rerun adoption must re-enqueue");
                return;
            }
        };
        assert_eq!(requeued.ratify_ref, Some(RATIFY_REF));
        assert!(requeued.ratification_passed);
        assert_eq!(preimage_request_count(requeued.payload_hash), pin_before);

        assert_ok!(
            <crate::configs::RuntimeEpochExecutionGuard as ExecutionGuardAccess>::dequeue_terminal(
                PID,
            ),
        );
        assert_eq!(preimage_request_count(requeued.payload_hash), 0,);
        assert!(!pallet_execution_guard::Ratifications::<Runtime>::contains_key(PID));
        assert!(!pallet_execution_guard::AttestationBindings::<Runtime>::contains_key(PID));
    });
}

#[test]
fn queued_code_force_rerun_preserves_guard_records_pin_and_uses_only_guardian_event() {
    use pallet_epoch::ExecutionGuardAccess;

    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_014;
        const RATIFY_REF: u32 = 94;
        let candidate = b"a8-r1-force-rerun".to_vec();
        if enqueue_attested_code_upgrade(PID, &candidate, RATIFY_REF).is_none() {
            assert!(false, "ratified queued CODE fixture must be constructible");
            return;
        }
        let proposal = match pallet_epoch::Proposals::<Runtime>::get(PID) {
            Some(proposal) => proposal,
            None => {
                assert!(false, "queued CODE proposal must exist");
                return;
            }
        };
        let markets = match proposal.markets {
            Some(markets) => markets,
            None => {
                assert!(false, "queued CODE proposal must retain its markets");
                return;
            }
        };
        let params =
            <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        let now = System::block_number();
        let decision_b = crate::configs::class_pol_floor(ProposalClass::Code);
        let gate_b = crate::configs::balance_param(b"pol.b_gate");
        let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
        let contest = params.v_min[crate::configs::proposal_class_index(ProposalClass::Code)];
        let gates = match markets.gates {
            Some(gates) => gates,
            None => {
                assert!(false, "CODE fixture must carry gate markets");
                return;
            }
        };
        let books = [
            (
                markets.accept,
                pallet_market::core_market::BookKind::Decision {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Accept,
                },
                decision_b,
            ),
            (
                markets.reject,
                pallet_market::core_market::BookKind::Decision {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Reject,
                },
                decision_b,
            ),
            (
                gates[0],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Accept,
                    gate: futarchy_primitives::GateType::Survival,
                },
                gate_b,
            ),
            (
                gates[1],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Reject,
                    gate: futarchy_primitives::GateType::Survival,
                },
                gate_b,
            ),
            (
                gates[2],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Accept,
                    gate: futarchy_primitives::GateType::Security,
                },
                gate_b,
            ),
            (
                gates[3],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Reject,
                    gate: futarchy_primitives::GateType::Security,
                },
                gate_b,
            ),
            (
                markets.baseline,
                pallet_market::core_market::BookKind::Baseline {
                    epoch: proposal.epoch,
                },
                baseline_b,
            ),
        ];
        for (id, kind, b) in books {
            assert_ok!(seed_decision_grade_market(
                id,
                kind,
                futarchy_primitives::FixedU64(500_000_000),
                now,
                (params.decision_window, params.trailing_window),
                b,
                contest,
            ));
        }
        pallet_market::BaselineMarketOf::<Runtime>::insert(proposal.epoch, markets.baseline);

        let queued = match pallet_execution_guard::Queue::<Runtime>::get(PID) {
            Some(queued) => queued,
            None => {
                assert!(false, "CODE fixture must be queued");
                return;
            }
        };
        <Preimage as StorePreimage>::unnote(&H256::from(queued.payload_hash));
        let pin_before = preimage_request_count(queued.payload_hash);
        assert_eq!(pin_before, 1, "the execution guard must own one live pin");
        let ratification_before = pallet_execution_guard::Ratifications::<Runtime>::get(PID);
        let attestation_before = pallet_execution_guard::AttestationBindings::<Runtime>::get(PID);
        let members = [
            account(151),
            account(152),
            account(153),
            account(154),
            account(155),
            account(156),
            account(157),
        ];
        for member in &members {
            assert_ok!(Balances::force_set_balance(
                RuntimeOrigin::root(),
                MultiAddress::Id(member.clone()),
                pallet_guardian::GUARDIAN_BOND.saturating_add(currency::VIT),
            ));
        }
        assert_ok!(Guardian::set_members(
            pallet_origins::Origin::ConstitutionalValues.into(),
            members.clone(),
        ));
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(members[0].clone()),
            pallet_guardian::GuardianPower::ForceRerun { pid: PID },
            H256::repeat_byte(62).into(),
        ));
        let action = pallet_guardian::NextActionId::<Runtime>::get().saturating_sub(1);
        for member in members.iter().take(5).skip(1) {
            assert_ok!(Guardian::approve_action(
                RuntimeOrigin::signed(member.clone()),
                action,
            ));
        }

        assert_eq!(
            pallet_epoch::Proposals::<Runtime>::get(PID).map(|proposal| proposal.state),
            Some(ProposalState::Extended),
        );
        assert!(!pallet_execution_guard::Queue::<Runtime>::contains_key(PID));
        assert_eq!(
            pallet_execution_guard::Ratifications::<Runtime>::get(PID),
            ratification_before,
        );
        assert_eq!(
            pallet_execution_guard::AttestationBindings::<Runtime>::get(PID),
            attestation_before,
        );
        assert_eq!(preimage_request_count(queued.payload_hash), pin_before);
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Guardian(pallet_guardian::Event::ForceRerun {
                pid: rerun_pid,
                ..
            }) if rerun_pid == PID
        )));
        assert!(!System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Epoch(pallet_epoch::Event::RerunOpened(rerun_pid))
                if rerun_pid == PID
        )));

        assert_ok!(
            <crate::configs::RuntimeEpochExecutionGuard as ExecutionGuardAccess>::dequeue_terminal(
                PID,
            ),
        );
        assert_eq!(preimage_request_count(queued.payload_hash), 0,);
        assert!(!pallet_execution_guard::Ratifications::<Runtime>::contains_key(PID));
        assert!(!pallet_execution_guard::AttestationBindings::<Runtime>::contains_key(PID));
    });
}

#[test]
fn rerun_reject_releases_retained_pin_ratification_and_attestation() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_020;
        const RATIFY_REF: u32 = 101;
        if enqueue_attested_code_upgrade(PID, b"a8-r2-rerun-reject", RATIFY_REF).is_none() {
            assert!(false, "ratified queued CODE fixture must be constructible");
            return;
        }
        let queued = match pallet_execution_guard::Queue::<Runtime>::get(PID) {
            Some(queued) => queued,
            None => {
                assert!(false, "CODE fixture must be queued");
                return;
            }
        };
        <Preimage as StorePreimage>::unnote(&H256::from(queued.payload_hash));
        assert_eq!(preimage_request_count(queued.payload_hash), 1);
        assert_ok!(seed_code_decision_markets(
            PID,
            System::block_number(),
            futarchy_primitives::FixedU64(500_000_000),
            futarchy_primitives::FixedU64(500_000_000),
        ));
        assert_ok!(Epoch::force_rerun_from_guardian(PID));
        let rerun = match pallet_epoch::Proposals::<Runtime>::get(PID) {
            Some(proposal) => proposal,
            None => {
                assert!(false, "force-rerun proposal must remain live");
                return;
            }
        };
        assert_eq!(rerun.state, ProposalState::Extended);
        assert!(pallet_execution_guard::RerunPins::<Runtime>::contains_key(
            PID
        ));
        assert_ok!(seed_code_decision_markets(
            PID,
            rerun.decide_at,
            futarchy_primitives::FixedU64(500_000_000),
            futarchy_primitives::FixedU64(500_000_000),
        ));
        System::set_block_number(rerun.decide_at);
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(account(214)), PID));
        let decided = match pallet_epoch::Proposals::<Runtime>::get(PID) {
            Some(proposal) => proposal,
            None => {
                assert!(false, "rejected proposal must enter measurement");
                return;
            }
        };
        assert_eq!(decided.state, ProposalState::Measuring);
        assert!(matches!(decided.decision, Some(DecisionOutcome::Reject(_))));
        assert_guard_ownership_cleared(PID, H256::from(queued.payload_hash));
    });
}

#[test]
fn tick_t20_from_rerun_releases_every_retained_guard_record() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_021;
        if enqueue_attested_code_upgrade(PID, b"a8-r2-rerun-t20", 102).is_none() {
            assert!(false, "ratified queued CODE fixture must be constructible");
            return;
        }
        let queued = match pallet_execution_guard::Queue::<Runtime>::get(PID) {
            Some(queued) => queued,
            None => {
                assert!(false, "CODE fixture must be queued");
                return;
            }
        };
        <Preimage as StorePreimage>::unnote(&H256::from(queued.payload_hash));
        assert_ok!(Epoch::delay_once(
            pallet_origins::Origin::GuardianHold.into(),
            PID,
            H256::repeat_byte(63).into(),
        ));
        let current_epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        pallet_epoch::GuardianReviewDeadlines::<Runtime>::insert(PID, current_epoch);
        let batch = match pallet_epoch::TickBatch::try_from(vec![PID]) {
            Ok(batch) => batch,
            Err(_) => {
                assert!(false, "single rerun scheduling tick must fit");
                return;
            }
        };
        assert_ok!(Epoch::tick(RuntimeOrigin::signed(account(215)), batch));
        assert_eq!(stored_proposal_state(PID), Some(ProposalState::Rerun));
        assert_eq!(preimage_request_count(queued.payload_hash), 1);
        assert!(pallet_execution_guard::RerunPins::<Runtime>::contains_key(
            PID
        ));
        assert_ok!(create_synthetic_markets_for_void(PID));

        pallet_constitution::PhaseFlags::<Runtime>::mutate(|flags| {
            *flags |= pallet_constitution::PhaseFlagsValue::LEDGER_FROZEN;
        });
        let batch = match pallet_epoch::TickBatch::try_from(vec![PID]) {
            Ok(batch) => batch,
            Err(_) => {
                assert!(false, "single stale-rerun T20 tick must fit");
                return;
            }
        };
        assert_ok!(Epoch::tick(RuntimeOrigin::signed(account(215)), batch));
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Epoch(pallet_epoch::Event::ProposalForceRejected {
                pid,
                reason: RejectReason::ProcessHold,
            }) if pid == PID
        )));
        assert_guard_ownership_cleared(PID, H256::from(queued.payload_hash));
    });
}

#[test]
fn void_cohort_releases_a_retained_rerun_pin_and_guard_records() {
    upgrade_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_022;
        const QUEUED_PID: futarchy_primitives::ProposalId = 8_023;
        if enqueue_attested_code_upgrade(PID, b"a8-r2-void-retained", 103).is_none() {
            assert!(false, "ratified queued CODE fixture must be constructible");
            return;
        }
        let queued = match pallet_execution_guard::Queue::<Runtime>::get(PID) {
            Some(queued) => queued,
            None => {
                assert!(false, "CODE fixture must be queued");
                return;
            }
        };
        <Preimage as StorePreimage>::unnote(&H256::from(queued.payload_hash));
        assert_ok!(ExecutionGuard::dequeue_for_rerun(PID));
        assert_eq!(preimage_request_count(queued.payload_hash), 1);
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        pallet_epoch::Proposals::<Runtime>::mutate(PID, |proposal| {
            if let Some(proposal) = proposal {
                proposal.state = ProposalState::Measuring;
            }
        });
        let proposals = match frame_support::BoundedVec::try_from(vec![PID]) {
            Ok(proposals) => proposals,
            Err(_) => {
                assert!(false, "one proposal must fit a cohort");
                return;
            }
        };
        pallet_epoch::Cohorts::<Runtime>::insert(
            epoch,
            pallet_epoch::CohortInfo {
                epoch,
                proposals,
                status: pallet_epoch::CohortStatus::Measuring {
                    until_epoch: epoch.saturating_add(2),
                },
            },
        );
        let call =
            RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
                line: pallet_futarchy_treasury::BudgetLine::Pol,
                amount: 1,
            });
        if enqueue_treasury_call(QUEUED_PID, call).is_none() {
            assert!(false, "same-epoch queued fixture must be constructible");
            return;
        }
        let same_epoch_queued = match pallet_execution_guard::Queue::<Runtime>::get(QUEUED_PID) {
            Some(queued) => queued,
            None => {
                assert!(false, "same-epoch proposal must be queued");
                return;
            }
        };
        <Preimage as StorePreimage>::unnote(&H256::from(same_epoch_queued.payload_hash));
        pallet_epoch::Proposals::<Runtime>::mutate(QUEUED_PID, |proposal| {
            if let Some(proposal) = proposal {
                proposal.epoch = epoch;
            }
        });
        pallet_epoch::ProposalSchedules::<Runtime>::mutate(QUEUED_PID, |schedule| {
            if let Some(schedule) = schedule {
                schedule.epoch = epoch;
            }
        });
        assert_ok!(create_synthetic_markets_for_void(PID));
        assert_ok!(create_synthetic_markets_for_void(QUEUED_PID));

        assert_ok!(Epoch::void_cohort(
            pallet_origins::Origin::EmergencyPlaybook.into(),
            epoch,
        ));
        assert!(!pallet_epoch::Cohorts::<Runtime>::contains_key(epoch));
        assert!(!pallet_epoch::Proposals::<Runtime>::contains_key(PID));
        assert!(!pallet_epoch::Proposals::<Runtime>::contains_key(
            QUEUED_PID
        ));
        let summary = pallet_epoch::RecentCohortSummaries::<Runtime>::get()
            .into_iter()
            .find(|summary| summary.epoch == epoch);
        assert!(summary.as_ref().is_some_and(|summary| summary.voided));
        assert!(summary.is_some_and(|summary| {
            summary.proposals.len() == 2
                && summary.proposals.iter().all(|(_, _, decision)| {
                    *decision == DecisionOutcome::Reject(RejectReason::ProcessHold)
                })
        }));
        assert_eq!(
            crate::views::recent_cohorts().as_slice(),
            pallet_epoch::RecentCohortSummaries::<Runtime>::get().as_slice(),
            "02 §4/§7.1 stored cohort form is the runtime API view form"
        );
        assert_guard_ownership_cleared(PID, H256::from(queued.payload_hash));
        assert_guard_ownership_cleared(QUEUED_PID, H256::from(same_epoch_queued.payload_hash));
    });
}

/// 03 §2.3/§5.2 · 05 §7(5): an epoch VOID settles the epoch's **Baseline**
/// vault at the neutral `s = 0.5` in the same transaction that voids the
/// cohort. Both Baseline redemption calls require `Settled` (03 §5.3), so
/// omitting the settlement leaves the vault `Open` forever and permanently
/// strands every single-sided Baseline holder of the voided epoch — while
/// full-pair holders still exit at par via `merge_baseline`, which is why the
/// omission was invisible to every solvency invariant (SQ-92).
///
/// This is the end-to-end regression over the real
/// epoch → welfare (SettleAuthority) → ledger wiring; the pallet suites cover
/// the seams individually.
#[test]
fn sq92_epoch_void_settles_the_baseline_and_unstrands_a_single_sided_holder() {
    use futarchy_primitives::{PositionId, ScalarSide};
    use pallet_conditional_ledger::core_ledger::BaselineState;
    use pallet_market::core_market::BookKind;

    development_ext().execute_with(|| {
        const BASELINE_MARKET: futarchy_primitives::MarketId = 92_000;
        // frame_system drops events deposited at block 0.
        System::set_block_number(1);
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        let holder = account(190);
        let counterparty = account(191);
        let short = PositionId::Baseline {
            epoch,
            side: ScalarSide::Short,
        };

        // Seeding the epoch's Baseline book is what creates its Baseline vault
        // (03 §2.2; pallet-market holds ledger MarketAuthority).
        assert_ok!(Market::create_market(
            RuntimeOrigin::signed(crate::configs::epoch_account()),
            BASELINE_MARKET,
            BookKind::Baseline { epoch },
            account(192),
            account(193),
            crate::configs::balance_param(b"pol.b_baseline"),
        ));
        assert_eq!(
            pallet_conditional_ledger::BaselineVaults::<Runtime>::get(epoch)
                .map(|vault| vault.state),
            Some(BaselineState::Open)
        );

        let stake = 10 * currency::USDC;
        let deposit = crate::configs::LedgerPositionDeposit::get();
        let funding = stake.saturating_mul(4);
        assert_ok!(ForeignAssets::mint_into(usdc_location(), &holder, funding));
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &counterparty,
            funding
        ));
        // 03 §7 R-4: the ledger sovereign is genesis-endowed and can never be
        // reaped. The fixture models that endowment (the asset's own
        // `min_balance`, never a literal) so the *last* redeemer of the vault
        // is not blocked by the collateral pallet's `Preservation::Preserve`.
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &ConditionalLedger::account_id(),
            <ForeignAssets as FungiblesInspect<AccountId>>::minimum_balance(usdc_location()),
        ));

        // Split, then dispose of the SHORT leg: `holder` is now single-sided
        // and cannot reach par through `merge_baseline` any more.
        assert_ok!(ConditionalLedger::split_baseline(
            RuntimeOrigin::signed(holder.clone()),
            epoch,
            stake,
        ));
        assert_ok!(ConditionalLedger::transfer(
            RuntimeOrigin::signed(holder.clone()),
            short,
            counterparty.clone(),
            stake,
        ));
        assert_noop!(
            ConditionalLedger::redeem_baseline(
                RuntimeOrigin::signed(holder.clone()),
                epoch,
                ScalarSide::Long,
                stake,
            ),
            pallet_conditional_ledger::Error::<Runtime>::WrongVaultState
        );

        // Normal lifecycle: the epoch's Baseline book is closed at the decision
        // boundary (`RuntimeMarketAccess::close_markets`) long before its cohort
        // can be voided out of `Measuring`.
        assert_ok!(Market::close(
            RuntimeOrigin::signed(crate::configs::epoch_account()),
            BASELINE_MARKET,
        ));

        // The epoch VOID (05 §7(5) cohort void, T20).
        let proposals = match BoundedVec::try_from(Vec::new()) {
            Ok(proposals) => proposals,
            Err(_) => {
                assert!(false, "empty cohort must fit");
                return;
            }
        };
        pallet_epoch::Cohorts::<Runtime>::insert(
            epoch,
            pallet_epoch::CohortInfo {
                epoch,
                proposals,
                status: pallet_epoch::CohortStatus::Measuring {
                    until_epoch: epoch.saturating_add(2),
                },
            },
        );
        assert_ok!(Epoch::void_cohort(
            pallet_origins::Origin::EmergencyPlaybook.into(),
            epoch,
        ));

        // 03 §2.3 `Baseline Open → Settled(s)`, at the kernel constant.
        assert_eq!(
            pallet_conditional_ledger::BaselineVaults::<Runtime>::get(epoch)
                .map(|vault| vault.state),
            Some(BaselineState::Settled(kernel::VOID_BASELINE_SCORE))
        );
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::ConditionalLedger(
                pallet_conditional_ledger::Event::BaselineSettled { epoch: settled, s },
            ) if settled == epoch && s == kernel::VOID_BASELINE_SCORE
        )));

        // The stranded holder can now redeem. Payouts are derived from the
        // kernel constant, never hand-computed: LONG floor(a·s), SHORT
        // floor(a·(1−s)) (03 §5.3/§6.3).
        let scale = u128::from(kernel::SCORE_SCALE);
        let s = u128::from(kernel::VOID_BASELINE_SCORE.0);
        let long_payout = stake.saturating_mul(s) / scale;
        let short_payout = stake.saturating_mul(scale.saturating_sub(s)) / scale;
        let escrow_before = pallet_conditional_ledger::BaselineVaults::<Runtime>::get(epoch)
            .map(|vault| vault.escrowed)
            .unwrap_or_default();

        let holder_before = ForeignAssets::balance(usdc_location(), &holder);
        assert_ok!(ConditionalLedger::redeem_baseline(
            RuntimeOrigin::signed(holder.clone()),
            epoch,
            ScalarSide::Long,
            stake,
        ));
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &holder).saturating_sub(holder_before),
            long_payout.saturating_add(deposit),
        );

        let counterparty_before = ForeignAssets::balance(usdc_location(), &counterparty);
        assert_ok!(ConditionalLedger::redeem_baseline(
            RuntimeOrigin::signed(counterparty.clone()),
            epoch,
            ScalarSide::Short,
            stake,
        ));
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &counterparty)
                .saturating_sub(counterparty_before),
            short_payout.saturating_add(deposit),
        );

        // R-1/L-2: the two floors never over-draw the vault's escrow.
        assert!(long_payout.saturating_add(short_payout) <= escrow_before);
        assert!(Epoch::do_try_state().is_ok());
        assert!(ConditionalLedger::do_try_state().is_ok());
        assert!(Market::do_try_state().is_ok());
    });
}

/// Companion to the regression above, over the window in which the epoch's
/// Baseline **book** is still live when its cohort is voided.
///
/// Reachability (all legal transitions): `start_measurement` (05 §2.1 T13/T21)
/// opens the cohort for epoch `e` as soon as the *first* epoch-`e` proposal
/// enters `Measuring`, while `RuntimeMarketAccess::close_markets` deliberately
/// keeps the epoch's Baseline book open while any sibling epoch-`e` proposal is
/// still `Trading`/`Extended` (its `baseline_still_live` guard — the T18
/// extended-decision window). A `void_cohort(e)` inside that window is exactly
/// the PB-ORACLE-VOID/T20 path of 05 §7(5), and 03 §5.2 makes its Baseline
/// settlement mandatory and unconditional there.
///
/// Obligation asserted: a legal dispatch may never leave the chain in a
/// try-state-violating state (15 §1; 03 §9: drift ⇒ I-4 flag ⇒
/// PB-LEDGER-FREEZE eligibility per D-9; G-1).
#[test]
fn sq92_epoch_void_with_a_live_baseline_book_keeps_market_try_state_green() {
    use futarchy_primitives::ScalarSide;
    use pallet_conditional_ledger::core_ledger::BaselineState;
    use pallet_market::core_market::BookKind;

    development_ext().execute_with(|| {
        const BASELINE_MARKET: futarchy_primitives::MarketId = 92_001;
        System::set_block_number(1);
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        let holder = account(194);

        assert_ok!(Market::create_market(
            RuntimeOrigin::signed(crate::configs::epoch_account()),
            BASELINE_MARKET,
            BookKind::Baseline { epoch },
            account(195),
            account(196),
            crate::configs::balance_param(b"pol.b_baseline"),
        ));
        let stake = 10 * currency::USDC;
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &holder,
            stake.saturating_mul(4)
        ));
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &ConditionalLedger::account_id(),
            <ForeignAssets as FungiblesInspect<AccountId>>::minimum_balance(usdc_location()),
        ));
        assert_ok!(ConditionalLedger::split_baseline(
            RuntimeOrigin::signed(holder.clone()),
            epoch,
            stake,
        ));

        let proposals = match BoundedVec::try_from(Vec::new()) {
            Ok(proposals) => proposals,
            Err(_) => {
                assert!(false, "empty cohort must fit");
                return;
            }
        };
        pallet_epoch::Cohorts::<Runtime>::insert(
            epoch,
            pallet_epoch::CohortInfo {
                epoch,
                proposals,
                status: pallet_epoch::CohortStatus::Measuring {
                    until_epoch: epoch.saturating_add(2),
                },
            },
        );

        // G-1 / 03 §5.2: the VOID must not fail on the settlement leg …
        assert_ok!(Epoch::void_cohort(
            pallet_origins::Origin::EmergencyPlaybook.into(),
            epoch,
        ));
        assert_eq!(
            pallet_conditional_ledger::BaselineVaults::<Runtime>::get(epoch)
                .map(|vault| vault.state),
            Some(BaselineState::Settled(kernel::VOID_BASELINE_SCORE))
        );
        assert_ok!(ConditionalLedger::redeem_baseline(
            RuntimeOrigin::signed(holder),
            epoch,
            ScalarSide::Long,
            stake,
        ));

        // … and it must not leave a flagged state behind. This is the
        // regression for the `pallet-market` defect the SQ-92 fix exposed:
        // `observe_baseline_terminal` latched `SettlementObservedAt` without
        // closing the book, unlike its proposal counterpart
        // `observe_proposal_terminal`, while `do_try_state` requires every
        // observed entry to carry `MarketPhase::Closed` + `ClosedAt`. It was
        // unreachable before, because on the ordinary path `close_markets`
        // closes the Baseline book once the epoch's last proposal leaves
        // Trading/Extended — an epoch VOID never passes through `decide`.
        assert!(matches!(
            pallet_market::Markets::<Runtime>::get(BASELINE_MARKET).map(|book| book.phase),
            Some(pallet_market::core_market::MarketPhase::Closed)
        ));
        assert!(Epoch::do_try_state().is_ok());
        assert!(ConditionalLedger::do_try_state().is_ok());
        assert!(Market::do_try_state().is_ok());
    });
}

#[test]
fn never_queued_ratification_is_reaped_on_withdraw_and_after_intake_reap() {
    upgrade_ext().execute_with(|| {
        let proposer = account(216);
        let bond = crate::configs::balance_param(b"prop.bond.code");
        let (payload_hash, payload_len) = match note_runtime_batch(Vec::new()) {
            Some(payload) => payload,
            None => {
                assert!(false, "prequeue CODE payload must encode");
                return;
            }
        };
        assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, bond));
        let pid = pallet_epoch::NextProposalId::<Runtime>::get();
        let mut proposal = empty_param_proposal(pid, proposer.clone(), payload_hash, payload_len);
        proposal.class = ProposalClass::Code;
        proposal.bond = bond;
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(proposer.clone()),
            proposal,
        ));
        assert_ok!(ExecutionGuard::ratify(
            pallet_origins::Origin::ConstitutionalValues.into(),
            pid,
            104,
        ));
        assert!(pallet_execution_guard::Ratifications::<Runtime>::contains_key(pid));
        <Preimage as StorePreimage>::unnote(&payload_hash);
        assert_eq!(preimage_request_count(payload_hash), 0);

        assert_ok!(Epoch::withdraw(RuntimeOrigin::signed(proposer), pid));
        assert_guard_ownership_cleared(pid, payload_hash);
        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        System::set_block_number(schedule.epoch_start_block.saturating_add(schedule.length));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(217)),
            Default::default(),
        ));
        assert!(!pallet_epoch::IntakeProposals::<Runtime>::contains_key(pid));
        assert!(Epoch::do_try_state().is_ok());
        assert!(ExecutionGuard::do_try_state().is_ok());
    });
}

#[test]
fn ratified_prequeue_trading_reject_releases_epoch_pin_and_ratification() {
    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_023;
        let (payload_hash, payload_len) = match note_runtime_batch(Vec::new()) {
            Some(payload) => payload,
            None => {
                assert!(false, "prequeue Trading payload must encode");
                return;
            }
        };
        let params =
            <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        let end = System::block_number()
            .saturating_add(params.decision_window)
            .saturating_add(1);
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        let first_market = PID.saturating_mul(10);
        let markets = MarketSet {
            accept: first_market.saturating_add(1),
            reject: first_market.saturating_add(2),
            gates: Some([
                first_market.saturating_add(3),
                first_market.saturating_add(4),
                first_market.saturating_add(5),
                first_market.saturating_add(6),
            ]),
            baseline: first_market.saturating_add(7),
        };
        let mut proposal = empty_param_proposal(PID, account(218), payload_hash, payload_len);
        proposal.class = ProposalClass::Code;
        proposal.state = ProposalState::Trading;
        proposal.epoch = epoch;
        proposal.metric_spec = 1;
        proposal.decide_at = end;
        proposal.markets = Some(markets);
        pallet_epoch::Proposals::<Runtime>::insert(PID, proposal);
        pallet_epoch::ProposalSchedules::<Runtime>::insert(
            PID,
            pallet_epoch::ProposalSchedule {
                epoch,
                epoch_start_block: pallet_epoch::Schedule::<Runtime>::get().epoch_start_block,
                epoch_length: pallet_epoch::Schedule::<Runtime>::get().length,
                decide_at: end,
                metric_spec: 1,
            },
        );
        pallet_epoch::NextProposalId::<Runtime>::mutate(|next| {
            *next = (*next).max(PID.saturating_add(1));
        });
        pallet_conditional_ledger::Vaults::<Runtime>::insert(
            PID,
            pallet_conditional_ledger::core_ledger::VaultInfo::open(1),
        );
        <Preimage as QueryPreimage>::request(&payload_hash);
        pallet_epoch::QualificationPreimageRequests::<Runtime>::insert(PID, payload_hash.0);
        <Preimage as StorePreimage>::unnote(&payload_hash);
        assert_eq!(preimage_request_count(payload_hash), 1);
        assert_ok!(ExecutionGuard::ratify(
            pallet_origins::Origin::ConstitutionalValues.into(),
            PID,
            105,
        ));
        assert!(!pallet_execution_guard::AttestationBindings::<Runtime>::contains_key(PID));
        assert_ok!(seed_code_decision_markets(
            PID,
            end,
            futarchy_primitives::FixedU64(500_000_000),
            futarchy_primitives::FixedU64(500_000_000),
        ));
        System::set_block_number(end);
        assert_ok!(Epoch::decide(RuntimeOrigin::signed(account(219)), PID));
        assert!(matches!(
            pallet_epoch::Proposals::<Runtime>::get(PID).map(|proposal| proposal.state),
            Some(ProposalState::Measuring)
        ));
        assert_guard_ownership_cleared(PID, payload_hash);
    });
}

#[test]
fn i9_epoch_enqueue_guard_execute_and_epoch_callback_are_real_and_origin_narrow() {
    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_001;
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| state.main_usdc = 10);
        assert_ok!(Constitution::set_capability(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::CapabilityRecord {
                class: ProposalClass::Treasury,
                capability: pallet_constitution::Capability::TreasurySpend,
                enabled: true,
            },
        ));
        let call = RuntimeCall::FutarchyTreasury(
            pallet_futarchy_treasury::Call::fund_budget_line {
                line: pallet_futarchy_treasury::BudgetLine::Pol,
                amount: 1,
            },
        );
        let batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![call]) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "single treasury call must fit the guard batch");
                    return;
                }
            };
        let bytes = batch.encode();
        let payload_len = match u32::try_from(bytes.len()) {
            Ok(len) => len,
            Err(_) => {
                assert!(false, "bounded guard batch length must fit u32");
                return;
            }
        };
        let payload_hash = match <Preimage as StorePreimage>::note(bytes.into()) {
            Ok(hash) => hash,
            Err(_) => {
                assert!(false, "bounded guard batch preimage must be accepted");
                return;
            }
        };
        let version_constraint = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => {
                assert!(false, "guard genesis must store the current runtime version");
                return;
            }
        };
        let now = System::block_number();
        let maturity = now.saturating_add(
            <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_timelock(
                ProposalClass::Treasury,
            ),
        );
        let grace =
            <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_grace(
                ProposalClass::Treasury,
            );
        let grace_end = maturity.saturating_add(grace);
        assert_ok!(seed_queued_epoch_proposal(
            PID,
            ProposalClass::Treasury,
            payload_hash,
            payload_len,
            maturity,
            grace_end,
            version_constraint.clone(),
        ));

        assert_ok!(<crate::configs::RuntimeEpochExecutionGuard as pallet_epoch::ExecutionGuardAccess>::enqueue(
            PID,
            payload_hash.0,
            Some(version_constraint),
            maturity,
            grace,
            false,
        ));
        let queued = match pallet_execution_guard::Queue::<Runtime>::get(PID) {
            Some(queued) => queued,
            None => {
                assert!(false, "the epoch adapter must create a real guard queue entry");
                return;
            }
        };

        for bad_origin in [
            RuntimeOrigin::signed(account(71)),
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
        ] {
            assert_noop!(
                ExecutionGuard::enqueue(bad_origin, queued.clone(), false),
                DispatchError::BadOrigin,
            );
        }

        let callbacks = [
            RuntimeCall::Epoch(pallet_epoch::Call::mark_executed { pid: PID }),
            RuntimeCall::Epoch(pallet_epoch::Call::mark_failed_executed { pid: PID }),
            RuntimeCall::Epoch(pallet_epoch::Call::retry_exhausted_to_measurement { pid: PID }),
            RuntimeCall::Epoch(pallet_epoch::Call::expire_or_stale_queue {
                pid: PID,
                reason: None,
            }),
        ];
        for callback in &callbacks {
            for bad_origin in [
                RuntimeOrigin::signed(account(72)),
                RuntimeOrigin::root(),
                RuntimeOrigin::none(),
            ] {
                let result = callback.clone().dispatch(bad_origin);
                assert!(matches!(result, Err(error) if error.error == DispatchError::BadOrigin));
            }
            for wrapped in closed_wrappers(callback.clone()) {
                if RuntimeBaseCallFilter::contains(&wrapped) {
                    let _ = wrapped.dispatch(RuntimeOrigin::signed(account(73)));
                }
                assert_eq!(
                    pallet_epoch::Proposals::<Runtime>::get(PID).map(|p| p.state),
                    Some(ProposalState::Queued),
                );
                assert!(pallet_execution_guard::Queue::<Runtime>::contains_key(PID));
            }
        }

        System::set_block_number(maturity);
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        pallet_welfare::GateBreachFlags::<Runtime>::insert(
            epoch,
            pallet_welfare::CoreGateBreachFlags {
                s_breached: true,
                c_breached: false,
                day_bitmap: [1, 0],
            },
        );
        pallet_execution_guard::GateSuspension::<Runtime>::put(epoch);
        assert!(<crate::configs::RuntimeGuardianState as pallet_execution_guard::GuardianState>::gate_suspended());
        let queue_before_suspension = pallet_execution_guard::Queue::<Runtime>::get(PID);
        let suspended = ExecutionGuard::execute(RuntimeOrigin::signed(account(74)), PID);
        let suspended_error = match suspended {
            Ok(_) => {
                assert!(false, "gate-suspended execution must fail");
                return;
            }
            Err(error) => error.error,
        };
        assert_eq!(
            suspended_error,
            pallet_execution_guard::Error::<Runtime>::GateSuspended.into()
        );
        assert_eq!(
            pallet_execution_guard::Queue::<Runtime>::get(PID),
            queue_before_suspension
        );
        pallet_welfare::GateBreachFlags::<Runtime>::remove(epoch);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(74)),
            PID,
        ));
        assert_eq!(
            pallet_epoch::Proposals::<Runtime>::get(PID).map(|p| p.state),
            Some(ProposalState::Measuring),
        );
        assert_eq!(
            pallet_conditional_ledger::Vaults::<Runtime>::get(PID).map(|vault| vault.state),
            Some(futarchy_primitives::VaultState::Resolved(
                futarchy_primitives::Branch::Accept,
            )),
        );
        assert!(!pallet_execution_guard::Queue::<Runtime>::contains_key(PID));
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Epoch(pallet_epoch::Event::MeasurementStarted { cohort: 1 })
        )));
        #[cfg(feature = "try-runtime")]
        assert!(
            <crate::AllPalletsWithSystem as frame_support::traits::TryState<
                crate::BlockNumber,
            >>::try_state(
                System::block_number(),
                frame_try_runtime::TryStateSelect::All,
            )
            .is_ok(),
        );
    });
}

#[test]
fn queue_meters_are_rederived_from_the_batch_not_copied_from_resource_locks() {
    use pallet_epoch::ExecutionGuardAccess;

    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_013;
        assert_ok!(Constitution::set_capability(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::CapabilityRecord {
                class: ProposalClass::Treasury,
                capability: pallet_constitution::Capability::TreasurySpend,
                enabled: true,
            },
        ));
        let call = RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::spend {
            line: pallet_futarchy_treasury::BudgetLine::Pol,
            dest: account(147),
            amount: 1,
        });
        let batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![call]) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "single treasury spend must fit the guard batch");
                    return;
                }
            };
        let bytes = batch.encode();
        let payload_len = match u32::try_from(bytes.len()) {
            Ok(len) => len,
            Err(_) => {
                assert!(false, "bounded payload length must fit u32");
                return;
            }
        };
        let payload_hash = match <Preimage as StorePreimage>::note(bytes.into()) {
            Ok(hash) => hash,
            Err(error) => {
                assert!(false, "treasury payload preimage must be noted: {error:?}");
                return;
            }
        };
        let version = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => {
                assert!(false, "guard genesis must bind a runtime version");
                return;
            }
        };
        let now = System::block_number();
        let maturity = now.saturating_add(
            <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_timelock(
                ProposalClass::Treasury,
            ),
        );
        let grace = <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_grace(
            ProposalClass::Treasury,
        );
        assert_ok!(seed_queued_epoch_proposal(
            PID,
            ProposalClass::Treasury,
            payload_hash,
            payload_len,
            maturity,
            maturity.saturating_add(grace),
            version.clone(),
        ));
        pallet_epoch::Proposals::<Runtime>::mutate(PID, |proposal| {
            if let Some(proposal) = proposal {
                proposal.ask = 1;
            }
        });
        assert!(pallet_epoch::Proposals::<Runtime>::get(PID)
            .is_some_and(|proposal| proposal.resources.is_empty()));

        assert_ok!(
            <crate::configs::RuntimeEpochExecutionGuard as ExecutionGuardAccess>::enqueue(
                PID,
                payload_hash.0,
                Some(version),
                maturity,
                grace,
                false,
            ),
        );
        let queued = match pallet_execution_guard::Queue::<Runtime>::get(PID) {
            Some(queued) => queued,
            None => {
                assert!(false, "epoch adapter must enqueue the treasury batch");
                return;
            }
        };
        assert!(
            !queued.meters_declared.is_empty(),
            "a real treasury outflow batch must declare its re-derived live meters",
        );
        assert!(queued.meters_declared.iter().all(|meter| {
            pallet_execution_guard::HeldResources::<Runtime>::get().contains(&(PID, *meter))
        }));

        let blocked = match pallet_execution_guard::pallet::StoredBlockedMeters::try_from(
            queued.meters_declared.to_vec(),
        ) {
            Ok(blocked) => blocked,
            Err(_) => {
                assert!(false, "derived queue meters must fit the live-meter bound");
                return;
            }
        };
        pallet_execution_guard::BlockedMeters::<Runtime>::put(blocked);
        System::set_block_number(maturity);
        assert_eq!(
            ExecutionGuard::execute(RuntimeOrigin::signed(account(148)), PID)
                .map_err(|error| error.error),
            Err(pallet_execution_guard::Error::<Runtime>::MetersBlocked.into()),
        );
        assert!(pallet_execution_guard::Queue::<Runtime>::contains_key(PID));
    });
}

#[test]
fn queued_treasury_outflows_mirror_enqueue_execute_and_terminal_dequeue() {
    use pallet_epoch::ExecutionGuardAccess;

    development_ext().execute_with(|| {
        use pallet_futarchy_treasury::BudgetLine;

        assert_ok!(Constitution::set_capability(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::CapabilityRecord {
                class: ProposalClass::Treasury,
                capability: pallet_constitution::Capability::TreasurySpend,
                enabled: true,
            },
        ));
        let amount = currency::USDC;
        let main = amount
            .saturating_mul(kernel::BASIS_POINTS_DENOMINATOR)
            .checked_div(Balance::from(
                pallet_futarchy_treasury::TRS_STREAM_THRESHOLD_BPS,
            ))
            .map_or(Balance::MAX, |minimum| minimum.saturating_mul(2));
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = main;
        });
        assert_ok!(FutarchyTreasury::fund_budget_line(
            pallet_origins::Origin::FutarchyTreasury.into(),
            BudgetLine::Pol,
            amount.saturating_mul(2),
        ));
        let nav_before = FutarchyTreasury::nav().nav;

        let enqueue = |pid: futarchy_primitives::ProposalId| -> Option<BlockNumber> {
            let call = RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::spend {
                line: BudgetLine::Pol,
                dest: account(150),
                amount,
            });
            let batch =
                pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![call])
                    .ok()?;
            let bytes = batch.encode();
            let payload_len = u32::try_from(bytes.len()).ok()?;
            let payload_hash = <Preimage as StorePreimage>::note(bytes.into()).ok()?;
            let version = pallet_execution_guard::CurrentSpecName::<Runtime>::get()?;
            let maturity = System::block_number().checked_add(
                <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_timelock(
                    ProposalClass::Treasury,
                ),
            )?;
            let grace =
                <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_grace(
                    ProposalClass::Treasury,
                );
            seed_queued_epoch_proposal(
                pid,
                ProposalClass::Treasury,
                payload_hash,
                payload_len,
                maturity,
                maturity.checked_add(grace)?,
                version.clone(),
            )
            .ok()?;
            pallet_epoch::Proposals::<Runtime>::mutate(pid, |proposal| {
                if let Some(proposal) = proposal {
                    proposal.ask = amount;
                }
            });
            <crate::configs::RuntimeEpochExecutionGuard as ExecutionGuardAccess>::enqueue(
                pid,
                payload_hash.0,
                Some(version),
                maturity,
                grace,
                false,
            )
            .ok()?;
            Some(maturity)
        };

        let execute_pid = 8_014;
        let maturity = match enqueue(execute_pid) {
            Some(maturity) => maturity,
            None => {
                assert!(false, "treasury execution must enqueue");
                return;
            }
        };
        assert_eq!(
            FutarchyTreasury::treasury().pending_outflows.as_slice(),
            &[amount]
        );
        assert_eq!(
            FutarchyTreasury::nav().nav,
            nav_before.saturating_sub(amount)
        );
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(151)),
            execute_pid,
        ));
        assert!(FutarchyTreasury::treasury().pending_outflows.is_empty());
        assert_eq!(
            FutarchyTreasury::nav().nav,
            nav_before.saturating_sub(amount)
        );

        let dequeue_pid = 8_015;
        if enqueue(dequeue_pid).is_none() {
            assert!(false, "treasury cancellation must enqueue");
            return;
        }
        let nav_with_pending = FutarchyTreasury::nav().nav;
        assert_eq!(
            FutarchyTreasury::treasury().pending_outflows.as_slice(),
            &[amount]
        );
        assert_ok!(ExecutionGuard::dequeue_terminal(dequeue_pid));
        assert!(FutarchyTreasury::treasury().pending_outflows.is_empty());
        assert_eq!(
            FutarchyTreasury::nav().nav,
            nav_with_pending.saturating_add(amount)
        );
    });
}

#[test]
fn wired_pending_outflow_sync_rejects_a_corrupt_sixty_fifth_entry() {
    use pallet_execution_guard::{PendingOutflowSync, Preimages};

    development_ext().execute_with(|| {
        // limit-coverage: Treasury pending outflows
        let amount = 1;
        let call = RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::spend {
            line: pallet_futarchy_treasury::BudgetLine::Pol,
            dest: account(154),
            amount,
        });
        let batch = match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![
            call,
        ]) {
            Ok(batch) => batch,
            Err(_) => {
                assert!(false, "one pending-outflow call must fit");
                return;
            }
        };
        let meters = match crate::configs::derived_execution_meters(&batch) {
            Some(meters) => meters,
            None => {
                assert!(false, "treasury spend must derive its outflow meter");
                return;
            }
        };
        let bytes = batch.encode();
        let payload_len = match u32::try_from(bytes.len()) {
            Ok(len) => len,
            Err(_) => {
                assert!(false, "bounded batch length must fit");
                return;
            }
        };
        let payload_hash = match <Preimage as StorePreimage>::note(bytes.into()) {
            Ok(hash) => hash,
            Err(error) => {
                assert!(false, "pending-outflow preimage must note: {error:?}");
                return;
            }
        };
        assert!(crate::configs::RuntimePreimages::fetch(payload_hash.0, payload_len).is_some());
        let version = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => {
                assert!(false, "guard runtime version must exist");
                return;
            }
        };
        for index in 0..=pallet_futarchy_treasury::MAX_PENDING_OUTFLOWS {
            let pid = 20_000_u64.saturating_add(index as u64);
            let mut proposal = empty_param_proposal(pid, account(155), payload_hash, payload_len);
            proposal.class = ProposalClass::Treasury;
            proposal.state = ProposalState::Queued;
            proposal.ask = amount;
            pallet_epoch::Proposals::<Runtime>::insert(pid, proposal);
            pallet_execution_guard::Queue::<Runtime>::insert(
                pid,
                pallet_execution_guard::pallet::StoredQueuedExecution {
                    pid,
                    payload_hash: payload_hash.0,
                    payload_len,
                    class: ProposalClass::Treasury,
                    maturity: 0,
                    grace_end: 0,
                    version_constraint: version.clone(),
                    meters_declared: meters.clone(),
                    ratify_ref: None,
                    ratification_passed: false,
                    attestation_id: None,
                    pre_upgrade_checkpoint: None,
                    cancelled: false,
                    declared_domains: Default::default(),
                    failed_at: None,
                },
            );
        }

        assert_eq!(
            <crate::configs::RuntimePendingOutflowSync as PendingOutflowSync>::sync_pending_outflows(
            ),
            Err(pallet_futarchy_treasury::Error::<Runtime>::TooManyObligations.into())
        );
        assert!(FutarchyTreasury::treasury().pending_outflows.is_empty());
    });
}

#[test]
fn live_book_pol_commitments_include_baseline_and_release_only_at_settlement() {
    use pallet_epoch::{EpochParamsProvider, MarketAccess};
    use pallet_welfare::LedgerSettlement;

    development_ext().execute_with(|| {
        let pid = 8_016;
        let params = <crate::configs::RuntimeEpochParams as EpochParamsProvider>::get();
        let decision_b = crate::configs::balance_param(b"pol.b.param");
        let gate_b = crate::configs::balance_param(b"pol.b_gate");
        let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
        let decision_headroom = match pallet_market::core_market::seed_headroom(decision_b) {
            Ok(amount) => amount,
            Err(error) => {
                assert!(false, "decision headroom must be computable: {error:?}");
                return;
            }
        };
        let baseline_headroom = match pallet_market::core_market::seed_headroom(baseline_b) {
            Ok(amount) => amount,
            Err(error) => {
                assert!(false, "baseline headroom must be computable: {error:?}");
                return;
            }
        };
        let gate_headroom = match pallet_market::core_market::seed_headroom(gate_b) {
            Ok(amount) => amount,
            Err(error) => {
                assert!(false, "gate headroom must be computable: {error:?}");
                return;
            }
        };
        let total = decision_headroom
            .saturating_mul(2)
            .saturating_add(gate_headroom.saturating_mul(4))
            .saturating_add(baseline_headroom);
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &crate::configs::pol_account(),
            decision_headroom
                .saturating_add(gate_headroom.saturating_mul(2))
                .saturating_add(currency::USDC),
        ));
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &crate::configs::pol_baseline_account(),
            baseline_headroom.saturating_add(currency::USDC),
        ));
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = total.saturating_mul(2);
        });
        let mut proposal = empty_param_proposal(pid, account(152), H256::zero(), 0);
        proposal.metric_spec = 1;
        proposal.state = ProposalState::Qualified;
        proposal.decide_at = System::block_number().saturating_add(params.decision_window);
        let nav_before = FutarchyTreasury::nav().nav;

        let seed_plan = match <crate::configs::RuntimePolBudget as pallet_epoch::PolBudget<
            AccountId,
        >>::proposal_seed_plan(&proposal)
        {
            Some(plan) => plan,
            None => {
                assert!(false, "PARAM proposal must have a live POL seed plan");
                return;
            }
        };
        let markets =
            match <crate::configs::RuntimeMarketAccess as MarketAccess<AccountId>>::open_markets(
                &proposal,
                false,
                Some(seed_plan),
            ) {
                Ok(markets) => markets,
                Err(error) => {
                    assert!(false, "market set must open: {error:?}");
                    return;
                }
            };
        proposal.markets = Some(markets);
        let commitments = FutarchyTreasury::treasury().pol_commitments;
        assert_eq!(
            commitments.as_slice(),
            &[
                decision_headroom,
                decision_headroom,
                gate_headroom,
                gate_headroom,
                gate_headroom,
                gate_headroom,
                baseline_headroom,
            ],
            "Baseline is a live-book NAV obligation even though its budget line is separate",
        );
        assert_eq!(
            FutarchyTreasury::nav().nav,
            nav_before.saturating_sub(total)
        );

        System::set_block_number(proposal.decide_at);
        assert_ok!(<crate::configs::RuntimeMarketAccess as MarketAccess<
            AccountId,
        >>::seal_decision_window(&proposal));
        assert_ok!(<crate::configs::RuntimeMarketAccess as MarketAccess<
            AccountId,
        >>::close_markets(&proposal,));
        assert_eq!(FutarchyTreasury::treasury().pol_commitments, commitments);

        assert_ok!(ConditionalLedger::resolve(
            RuntimeOrigin::signed(crate::configs::epoch_account()),
            pid,
            futarchy_primitives::Branch::Accept,
        ));
        assert_ok!(
            <crate::configs::WelfareLedger as LedgerSettlement>::settle_scalar(
                pid,
                futarchy_primitives::FixedU64(500_000_000),
            )
        );
        assert_eq!(
            FutarchyTreasury::treasury().pol_commitments.as_slice(),
            &[baseline_headroom]
        );
        assert_eq!(
            FutarchyTreasury::nav().nav,
            nav_before.saturating_sub(baseline_headroom)
        );
        assert_ok!(
            <crate::configs::WelfareLedger as LedgerSettlement>::settle_baseline(
                proposal.epoch,
                futarchy_primitives::FixedU64(500_000_000),
            )
        );
        assert!(FutarchyTreasury::treasury().pol_commitments.is_empty());
        assert_eq!(FutarchyTreasury::nav().nav, nav_before);

        System::set_block_number(
            proposal
                .decide_at
                .saturating_add(crate::configs::LedgerArchiveDelay::get()),
        );
        let gates = markets.gates.expect("PARAM proposal has gate books");
        for market in [
            markets.accept,
            markets.reject,
            gates[0],
            gates[1],
            gates[2],
            gates[3],
            markets.baseline,
        ] {
            assert_ok!(Market::reap(RuntimeOrigin::signed(account(153)), market));
        }
        assert!(FutarchyTreasury::treasury().pol_commitments.is_empty());
        assert_eq!(pallet_market::Markets::<Runtime>::count(), 0);
    });
}

#[test]
fn seeded_force_reject_void_closes_and_reaps_all_proposal_books() {
    use pallet_epoch::{EpochParamsProvider, MarketAccess};

    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_017;
        let params = <crate::configs::RuntimeEpochParams as EpochParamsProvider>::get();
        let decision_b = crate::configs::balance_param(b"pol.b.code");
        let gate_b = crate::configs::balance_param(b"pol.b_gate");
        let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
        let decision_headroom =
            pallet_market::core_market::seed_headroom(decision_b).expect("bounded decision b");
        let gate_headroom =
            pallet_market::core_market::seed_headroom(gate_b).expect("bounded gate b");
        let baseline_headroom =
            pallet_market::core_market::seed_headroom(baseline_b).expect("bounded baseline b");
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &crate::configs::pol_account(),
            decision_headroom
                .saturating_add(gate_headroom.saturating_mul(2))
                .saturating_add(currency::USDC),
        ));
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &crate::configs::pol_baseline_account(),
            baseline_headroom.saturating_add(currency::USDC),
        ));
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = decision_headroom
                .saturating_mul(2)
                .saturating_add(gate_headroom.saturating_mul(4))
                .saturating_add(baseline_headroom)
                .saturating_mul(2);
        });

        let mut proposal = empty_param_proposal(PID, account(154), H256::zero(), 0);
        proposal.class = ProposalClass::Code;
        proposal.metric_spec = 1;
        proposal.state = ProposalState::Trading;
        proposal.decide_at = System::block_number().saturating_add(params.decision_window);
        let seed_plan = <crate::configs::RuntimePolBudget as pallet_epoch::PolBudget<
            AccountId,
        >>::proposal_seed_plan(&proposal)
        .expect("PARAM seed plan");
        let markets =
            <crate::configs::RuntimeMarketAccess as MarketAccess<AccountId>>::open_markets(
                &proposal,
                false,
                Some(seed_plan),
            )
            .expect("seeded market set");
        proposal.markets = Some(markets);
        pallet_epoch::Proposals::<Runtime>::insert(PID, proposal.clone());
        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        pallet_epoch::ProposalSchedules::<Runtime>::insert(
            PID,
            pallet_epoch::ProposalSchedule {
                epoch: proposal.epoch,
                epoch_start_block: schedule.epoch_start_block,
                epoch_length: schedule.length,
                decide_at: proposal.decide_at,
                metric_spec: proposal.metric_spec,
            },
        );
        pallet_epoch::NextProposalId::<Runtime>::mutate(|next| {
            *next = (*next).max(PID.saturating_add(1));
        });
        let mut proposal_books = Vec::from([markets.accept, markets.reject]);
        proposal_books.extend(markets.gates.expect("CODE market set has gate books"));
        let protocol_accounts_before = pallet_market::MarketProtocolAccounts::<Runtime>::count();
        assert_eq!(FutarchyTreasury::treasury().pol_commitments.len(), 7);
        let void_block = System::block_number();

        assert_ok!(Epoch::force_reject_process_hold(
            pallet_origins::Origin::GuardianHold.into(),
            PID,
        ));
        assert_eq!(
            pallet_conditional_ledger::Vaults::<Runtime>::get(PID).map(|vault| vault.state),
            Some(futarchy_primitives::VaultState::Voided),
        );
        assert_eq!(
            FutarchyTreasury::treasury().pol_commitments.as_slice(),
            &[baseline_headroom],
        );
        assert_eq!(
            pallet_conditional_ledger::VaultTerminalAt::<Runtime>::get(PID),
            Some(void_block),
        );
        for id in &proposal_books {
            let book = pallet_market::Markets::<Runtime>::get(id).expect("voided book remains");
            assert_eq!(book.phase, pallet_market::core_market::MarketPhase::Closed);
            assert_eq!(
                pallet_market::ClosedAt::<Runtime>::get(id),
                Some(void_block)
            );
            assert_eq!(
                pallet_market::SettlementObservedAt::<Runtime>::get(id),
                Some(void_block),
                "the durable terminal latch is the reap-delay anchor",
            );
            assert!(!pallet_market::Pallet::<Runtime>::pol_obligation_live(
                *id, &book
            ));
        }
        assert!(Market::do_try_state().is_ok());
        assert!(FutarchyTreasury::do_try_state().is_ok());

        System::set_block_number(
            void_block.saturating_add(crate::configs::LedgerArchiveDelay::get()),
        );
        for id in &proposal_books {
            assert_ok!(Market::reap(RuntimeOrigin::signed(account(153)), *id));
        }
        assert!(!pallet_market::ProposalMarketIds::<Runtime>::contains_key(
            PID
        ));
        let removed_accounts = u32::try_from(proposal_books.len())
            .unwrap_or_default()
            .saturating_mul(2);
        assert_eq!(
            pallet_market::MarketProtocolAccounts::<Runtime>::count(),
            protocol_accounts_before.saturating_sub(removed_accounts),
        );
        assert_eq!(pallet_market::Markets::<Runtime>::count(), 1);
        assert!(Market::do_try_state().is_ok());
        assert!(FutarchyTreasury::do_try_state().is_ok());
    });
}

#[test]
fn two_sequential_guardian_reruns_prune_terminal_baseline_windows() {
    use pallet_epoch::MarketAccess;

    development_ext().execute_with(|| {
        fund_param_market_lifecycles(6);
        let params =
            <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        let mut baseline = None;

        for (offset, pid) in [8_030_u64, 8_031].into_iter().enumerate() {
            let initial_end = params
                .decision_window
                .saturating_add(u32::try_from(offset).unwrap_or_default().saturating_mul(10));
            let proposal = open_runtime_param_proposal(pid, initial_end)
                .expect("proposal market lifecycle opens");
            let markets = proposal.markets.expect("market set");
            if let Some(existing) = baseline {
                assert_eq!(markets.baseline, existing);
            } else {
                baseline = Some(markets.baseline);
            }

            System::set_block_number(initial_end);
            assert_ok!(<crate::configs::RuntimeMarketAccess as MarketAccess<
                AccountId,
            >>::seal_decision_window(&proposal));
            System::set_block_number(initial_end.saturating_add(1));
            assert_ok!(Epoch::force_rerun_from_guardian(pid));
            let rerun = pallet_epoch::Proposals::<Runtime>::get(pid).expect("rerun persists");
            assert!(rerun.rerun);

            System::set_block_number(rerun.decide_at);
            assert_ok!(<crate::configs::RuntimeMarketAccess as MarketAccess<
                AccountId,
            >>::seal_decision_window(&rerun));
            pallet_epoch::Proposals::<Runtime>::mutate(pid, |stored| {
                if let Some(stored) = stored {
                    stored.state = ProposalState::Measuring;
                }
            });
            assert_ok!(<crate::configs::RuntimeMarketAccess as MarketAccess<
                AccountId,
            >>::close_markets(&rerun));
            assert!(pallet_market::DecisionWindows::<Runtime>::get(markets.baseline).is_empty());
            assert!(
                pallet_market::DecisionWindowOwners::<Runtime>::get(markets.baseline).is_empty()
            );
        }
    });
}

#[test]
fn live_baseline_windows_reject_ninth_boundary_through_guardian_rerun_lifecycle() {
    // limit-coverage: TwapCheckpoints
    development_ext().execute_with(|| {
        fund_param_market_lifecycles(5);
        let params =
            <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        let first = open_runtime_param_proposal(8_032, params.decision_window)
            .expect("first live window opens");
        let second = open_runtime_param_proposal(8_033, params.decision_window.saturating_add(1))
            .expect("second live window opens");
        let first_markets = first.markets.expect("first market set");
        assert_eq!(
            second.markets.map(|markets| markets.baseline),
            Some(first_markets.baseline),
        );
        let windows_before = pallet_market::DecisionWindows::<Runtime>::get(first_markets.baseline);
        let owners_before =
            pallet_market::DecisionWindowOwners::<Runtime>::get(first_markets.baseline);
        assert_eq!(windows_before.len(), 2);

        System::set_block_number(params.decision_window.saturating_add(100));
        assert_eq!(
            Epoch::force_rerun_from_guardian(first.id),
            Err(pallet_market::Error::<Runtime>::TryStateViolation.into()),
        );
        assert_eq!(
            pallet_market::DecisionWindows::<Runtime>::get(first_markets.baseline),
            windows_before,
        );
        assert_eq!(
            pallet_market::DecisionWindowOwners::<Runtime>::get(first_markets.baseline),
            owners_before,
        );
        assert!(
            !pallet_epoch::Proposals::<Runtime>::get(first.id)
                .expect("failed rerun rolls back proposal")
                .rerun
        );
    });
}

#[test]
fn pol_commitment_bound_rejects_directly_constructed_197th_obligation() {
    development_ext().execute_with(|| {
        // limit-coverage: Treasury POL commitments
        assert_noop!(
            FutarchyTreasury::set_pol_commitments(vec![
                1;
                futarchy_primitives::bounds::MAX_LIVE_MARKETS
                    as usize
                    + 1
            ]),
            pallet_futarchy_treasury::Error::<Runtime>::TooManyObligations
        );
        assert!(FutarchyTreasury::treasury().pol_commitments.is_empty());
    });
}

#[test]
fn queue_time_meter_preview_is_live_recursive_and_read_only() {
    development_ext().execute_with(|| {
        use pallet_futarchy_treasury::BudgetLine;

        let funded_lines = match frame_support::BoundedVec::try_from(vec![(
            BudgetLine::Pol,
            currency::USDC.saturating_mul(10),
        )]) {
            Ok(lines) => lines,
            Err(_) => {
                assert!(false, "one treasury budget line must fit its storage bound");
                return;
            }
        };
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = 0;
            state.lines = funded_lines;
        });

        let spend = RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::spend {
            line: BudgetLine::Pol,
            dest: account(149),
            amount: 1,
        });
        let nested_spend = RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![spend.clone()],
        });
        let spend_batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![
                nested_spend.clone(),
            ]) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "one nested spend must fit the guard batch");
                    return;
                }
            };

        let treasury = FutarchyTreasury::treasury();
        let outflow_ceiling = match pallet_futarchy_treasury::bps(
            treasury.nav().spendable_nav,
            treasury.meter_30d.limit_bps,
        ) {
            Ok(ceiling) => ceiling,
            Err(error) => {
                assert!(
                    false,
                    "the live outflow ceiling must be computable: {error:?}"
                );
                return;
            }
        };
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.meter_30d.buckets[0] = outflow_ceiling;
        });
        let exhausted_outflow = pallet_futarchy_treasury::State::<Runtime>::get();
        assert!(!crate::configs::preview_batch_admission(&spend_batch));
        assert_eq!(
            pallet_futarchy_treasury::State::<Runtime>::get(),
            exhausted_outflow,
            "the decision-time preview must not persist its simulated treasury transition",
        );

        let issue = RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::issue_vit {
            amount: 1,
            line: BudgetLine::Rewards,
        });
        let issue_batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![
                issue.clone()
            ]) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "one issuance call must fit the guard batch");
                    return;
                }
            };
        let treasury = FutarchyTreasury::treasury();
        let issuance_ceiling =
            match pallet_futarchy_treasury::bps(treasury.vit_supply, treasury.issuance.limit_bps) {
                Ok(ceiling) => ceiling,
                Err(error) => {
                    assert!(
                        false,
                        "the live issuance ceiling must be computable: {error:?}"
                    );
                    return;
                }
            };
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.issuance.buckets[0] = issuance_ceiling;
        });
        let exhausted_issuance = pallet_futarchy_treasury::State::<Runtime>::get();
        assert!(!crate::configs::preview_batch_admission(&issue_batch));
        assert_eq!(
            pallet_futarchy_treasury::State::<Runtime>::get(),
            exhausted_issuance,
            "issuance preview must be read-only",
        );

        let authorize = RuntimeCall::System(frame_system::Call::authorize_upgrade {
            code_hash: H256::repeat_byte(150),
        });
        let authorize_batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![
                authorize.clone()
            ]) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "one authorization must fit the guard batch");
                    return;
                }
            };
        pallet_execution_guard::LastUpgradeAuthorized::<Runtime>::kill();
        assert!(crate::configs::preview_batch_admission(&authorize_batch));
        assert_eq!(
            pallet_execution_guard::LastUpgradeAuthorized::<Runtime>::get(),
            None,
            "spacing preview must not authorize or persist a timestamp",
        );
        let now = System::block_number();
        pallet_execution_guard::LastUpgradeAuthorized::<Runtime>::put(now);
        assert!(!crate::configs::preview_batch_admission(&authorize_batch));
        assert_eq!(
            pallet_execution_guard::LastUpgradeAuthorized::<Runtime>::get(),
            Some(now),
        );

        let all_metered =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(vec![
                RuntimeCall::Utility(pallet_utility::Call::batch {
                    calls: vec![spend, nested_spend],
                }),
                issue,
                authorize,
            ]) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(
                        false,
                        "the recursive metering fixture must fit the guard batch"
                    );
                    return;
                }
            };
        let meters = match crate::configs::derived_execution_meters(&all_metered) {
            Some(meters) => meters,
            None => {
                assert!(false, "runtime metering must derive a bounded declaration");
                return;
            }
        };
        assert_eq!(
            meters.len(),
            3,
            "nested duplicate spends plus issuance and code must derive three distinct live meters",
        );
    });
}

#[test]
fn epoch_length_change_is_a_values_track_leaf_with_an_independent_pallet_origin_check() {
    let call = RuntimeCall::Epoch(pallet_epoch::Call::set_next_epoch_length {});
    assert!(crate::classifier::is_values_enactment_leaf(&call));
    assert!(RuntimeBaseCallFilter::contains(&call));
    assert!(RuntimeBaseCallFilter::contains_for(
        ClassOrigin::ConstitutionalValues,
        &call,
    ));
    for wrapped in closed_wrappers(call.clone()) {
        assert!(!RuntimeBaseCallFilter::contains(&wrapped));
    }

    development_ext().execute_with(|| {
        let signed = Epoch::set_next_epoch_length(RuntimeOrigin::signed(account(75)));
        assert!(matches!(signed, Err(DispatchError::BadOrigin)));
        assert_ok!(Epoch::set_next_epoch_length(
            pallet_origins::Origin::ConstitutionalValues.into(),
        ));
    });
}

#[test]
fn classifier_sweeps_every_callable_pallet_and_every_closed_wrapper_shape() {
    let who = account(31);
    let mut calls = vec![
        remark(),
        RuntimeCall::Timestamp(pallet_timestamp::Call::set { now: 6_000 }),
        RuntimeCall::ParachainSystem(
            cumulus_pallet_parachain_system::Call::sudo_send_upward_message { message: vec![1] },
        ),
        RuntimeCall::Balances(pallet_balances::Call::transfer_keep_alive {
            dest: MultiAddress::Id(who.clone()),
            value: 1,
        }),
        RuntimeCall::Vesting(pallet_vesting::Call::vest {}),
        RuntimeCall::ForeignAssets(pallet_assets::Call::transfer {
            id: usdc_location(),
            target: MultiAddress::Id(who.clone()),
            amount: 1,
        }),
        RuntimeCall::Referenda(pallet_referenda::Call::cancel { index: 0 }),
        RuntimeCall::ConvictionVoting(pallet_conviction_voting::Call::remove_vote {
            class: None,
            index: 0,
        }),
        RuntimeCall::Preimage(pallet_preimage::Call::unnote_preimage { hash: H256::zero() }),
        RuntimeCall::Scheduler(pallet_scheduler::Call::cancel { when: 1, index: 0 }),
        RuntimeCall::Utility(pallet_utility::Call::batch { calls: Vec::new() }),
        RuntimeCall::Proxy(pallet_proxy::Call::remove_proxies {}),
        RuntimeCall::Multisig(pallet_multisig::Call::poke_deposit {
            threshold: 2,
            other_signatories: vec![who.clone()],
            call_hash: [0; 32],
        }),
        RuntimeCall::Migrations(pallet_migrations::Call::clear_historic {
            selector: pallet_migrations::HistoricCleanupSelector::Specific(Vec::new()),
        }),
        RuntimeCall::Sudo(pallet_sudo::Call::remove_key {}),
        RuntimeCall::XcmpQueue(cumulus_pallet_xcmp_queue::Call::suspend_xcm_execution {}),
        RuntimeCall::MessageQueue(pallet_message_queue::Call::reap_page {
            message_origin: cumulus_primitives_core::AggregateMessageOrigin::Parent,
            page_index: 0,
        }),
        RuntimeCall::PolkadotXcm(pallet_xcm::Call::force_suspension { suspended: false }),
        RuntimeCall::CollatorSelection(pallet_collator_selection::Call::register_as_candidate {}),
        RuntimeCall::Session(pallet_session::Call::purge_keys {}),
        RuntimeCall::Constitution(pallet_constitution::Call::set_phase_flag {
            flag: 1,
            enabled: false,
        }),
        RuntimeCall::ConditionalLedger(pallet_conditional_ledger::Call::transfer {
            position: futarchy_primitives::PositionId::Proposal {
                proposal: 0,
                branch: futarchy_primitives::Branch::Accept,
                kind: futarchy_primitives::PositionKind::BranchUsdc,
            },
            to: who.clone(),
            amount: 1,
        }),
        RuntimeCall::Market(pallet_market::Call::crank_observe { market: 0 }),
        RuntimeCall::Welfare(pallet_welfare::Call::record_snapshot {
            epoch: 0,
            spec_version: 0,
        }),
        RuntimeCall::Oracle(pallet_oracle::Call::crank_round_close { batch: 1 }),
        RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
            line: pallet_futarchy_treasury::BudgetLine::Pol,
            amount: 1,
        }),
        RuntimeCall::Guardian(pallet_guardian::Call::propose_action {
            power: pallet_guardian::GuardianPower::SuspendOnGate,
            justification_hash: H256::zero().into(),
        }),
        RuntimeCall::Attestor(pallet_attestor::Call::attest {
            pid: 0,
            artifact_hash: H256::zero().into(),
            statement_hash: H256::zero().into(),
        }),
        RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::execute { pid: 0 }),
    ];
    calls.extend(epoch_call_samples());
    calls.extend(
        registry_calls::<()>()
            .into_iter()
            .take(1)
            .map(RuntimeCall::IncidentRegistry),
    );
    calls.extend(
        registry_calls::<pallet_registry::Instance1>()
            .into_iter()
            .take(1)
            .map(RuntimeCall::MilestoneRegistry),
    );
    calls.extend(closed_wrappers(remark()));
    let signed_caller: <RuntimeOrigin as frame_support::traits::OriginTrait>::PalletsOrigin =
        frame_system::RawOrigin::Signed(who.clone()).into();
    calls.extend([
        RuntimeCall::Utility(pallet_utility::Call::as_derivative {
            index: 0,
            call: Box::new(remark()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::dispatch_as {
            as_origin: Box::new(signed_caller.clone()),
            call: Box::new(remark()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::with_weight {
            call: Box::new(remark()),
            weight: Weight::zero(),
        }),
        RuntimeCall::Utility(pallet_utility::Call::if_else {
            main: Box::new(remark()),
            fallback: Box::new(remark()),
        }),
        RuntimeCall::Utility(pallet_utility::Call::dispatch_as_fallible {
            as_origin: Box::new(signed_caller),
            call: Box::new(remark()),
        }),
        RuntimeCall::Multisig(pallet_multisig::Call::approve_as_multi {
            threshold: 2,
            other_signatories: vec![who],
            maybe_timepoint: None,
            call_hash: [0; 32],
            max_weight: Weight::zero(),
        }),
        RuntimeCall::Scheduler(pallet_scheduler::Call::schedule {
            when: 1,
            maybe_periodic: None,
            priority: 0,
            call: Box::new(remark()),
        }),
    ]);
    assert!(calls.len() >= 34);
    for call in calls {
        let _ = RuntimeBaseCallFilter::contains(&call);
        let _ = RuntimeBaseCallFilter::contains_for(ClassOrigin::ConstitutionalValues, &call);
    }
}

fn registry_calls<I: 'static>() -> Vec<pallet_registry::Call<Runtime, I>>
where
    Runtime: pallet_registry::Config<I>,
{
    vec![
        pallet_registry::Call::file {
            epoch: 1,
            class: registry_core::FilingClass::S1,
            points: 1,
            evidence_hash: H256::repeat_byte(1).into(),
            spec_version: 1,
        },
        pallet_registry::Call::challenge_filing {
            epoch: 1,
            filing_id: 0,
            evidence_hash: H256::repeat_byte(2).into(),
        },
        pallet_registry::Call::ack_observed {
            epoch: 1,
            filing_id: 0,
        },
        pallet_registry::Call::crank_close { epoch: 1, batch: 1 },
        pallet_registry::Call::resolve_challenge {
            epoch: 1,
            filing_id: 0,
            uphold: false,
        },
        pallet_registry::Call::close_epoch { epoch: 1 },
        pallet_registry::Call::reap_epoch { epoch: 1 },
    ]
}

#[test]
fn sq75_both_registry_instances_are_base_filter_public_and_resolve_is_origin_gated() {
    let incident: Vec<RuntimeCall> = registry_calls::<()>()
        .into_iter()
        .map(RuntimeCall::IncidentRegistry)
        .collect();
    let milestone: Vec<RuntimeCall> = registry_calls::<pallet_registry::Instance1>()
        .into_iter()
        .map(RuntimeCall::MilestoneRegistry)
        .collect();
    for call in incident.iter().chain(milestone.iter()) {
        assert!(RuntimeBaseCallFilter::contains(call));
        let wrapped = RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![call.clone()],
        });
        assert!(RuntimeBaseCallFilter::contains(&wrapped));
    }

    development_ext().execute_with(|| {
        let result = incident[4]
            .clone()
            .dispatch(RuntimeOrigin::signed(account(9)));
        assert!(matches!(result, Err(error) if error.error == DispatchError::BadOrigin));
        let result = milestone[4]
            .clone()
            .dispatch(RuntimeOrigin::signed(account(9)));
        assert!(matches!(result, Err(error) if error.error == DispatchError::BadOrigin));
    });
}

#[test]
fn signed_custom_pallet_row_is_admitted_by_the_base_filter() {
    let calls = vec![
        RuntimeCall::ConditionalLedger(pallet_conditional_ledger::Call::transfer {
            position: futarchy_primitives::PositionId::Proposal {
                proposal: 0,
                branch: futarchy_primitives::Branch::Accept,
                kind: futarchy_primitives::PositionKind::BranchUsdc,
            },
            to: account(2),
            amount: 1,
        }),
        RuntimeCall::Market(pallet_market::Call::crank_observe { market: 0 }),
        RuntimeCall::Welfare(pallet_welfare::Call::record_snapshot {
            epoch: 0,
            spec_version: 0,
        }),
        RuntimeCall::Oracle(pallet_oracle::Call::crank_round_close { batch: 1 }),
        RuntimeCall::Guardian(pallet_guardian::Call::propose_action {
            power: pallet_guardian::GuardianPower::SuspendOnGate,
            justification_hash: H256::zero().into(),
        }),
        RuntimeCall::Attestor(pallet_attestor::Call::attest {
            pid: 0,
            artifact_hash: H256::zero().into(),
            statement_hash: H256::zero().into(),
        }),
        RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::apply_authorized_upgrade {
            code: Default::default(),
        }),
        RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::expire_failed_execution {
            pid: 0,
        }),
        RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::reject_stale { pid: 0 }),
    ];
    for call in calls {
        assert!(RuntimeBaseCallFilter::contains(&call));
        assert!(call.get_dispatch_info().call_weight.ref_time() > 0);
    }
}

#[test]
fn values_leaf_dispatches_with_values_origin_and_signed_dies_in_pallet() {
    let members = [
        account(1),
        account(2),
        account(3),
        account(4),
        account(5),
        account(6),
        account(7),
    ];
    let call = RuntimeCall::Guardian(pallet_guardian::Call::set_members {
        members: members.clone(),
    });
    assert!(RuntimeBaseCallFilter::contains(&call));
    development_ext().execute_with(|| {
        let signed = call.clone().dispatch(RuntimeOrigin::signed(account(1)));
        assert!(matches!(signed, Err(error) if error.error == DispatchError::BadOrigin));
        for member in &members {
            assert_ok!(Balances::force_set_balance(
                RuntimeOrigin::root(),
                MultiAddress::Id(member.clone()),
                pallet_guardian::GUARDIAN_BOND.saturating_add(currency::VIT),
            ));
        }
        let values = call
            .clone()
            .dispatch(pallet_origins::Origin::ConstitutionalValues.into());
        assert!(values.is_ok());

        let nobody = RuntimeCall::System(frame_system::Call::set_storage { items: vec![] });
        assert!(!RuntimeBaseCallFilter::contains_for(
            ClassOrigin::ConstitutionalValues,
            &nobody
        ));
    });
}

#[test]
fn guardian_pending_empty_membership_on_initialize_is_a_no_op() {
    development_ext().execute_with(|| {
        System::set_block_number(1);
        let before = System::events().len();
        let weight = <Guardian as frame_support::traits::Hooks<BlockNumber>>::on_initialize(1);
        assert_eq!(
            weight,
            <<Runtime as pallet_guardian::Config>::WeightInfo as pallet_guardian::WeightInfo>::on_initialize()
        );
        assert_eq!(System::events().len(), before);
    });
}

#[test]
fn active_metric_spec_adapter_and_seeded_qualification_freeze_the_exact_version() {
    use frame_support::traits::tokens::{Fortitude, Preservation};

    development_ext().execute_with(|| {
        let proposer = account(140);
        let batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(Vec::new()) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "empty bounded payload must encode");
                    return;
                }
            };
        let bytes = batch.encode();
        let payload_len = match u32::try_from(bytes.len()) {
            Ok(len) => len,
            Err(_) => {
                assert!(false, "bounded payload length must fit u32");
                return;
            }
        };
        let payload_hash = match <Preimage as StorePreimage>::note(bytes.into()) {
            Ok(hash) => hash,
            Err(error) => {
                assert!(false, "empty payload preimage must be noted: {error:?}");
                return;
            }
        };
        let bond = crate::configs::balance_param(b"prop.bond.param");
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &proposer,
            bond.saturating_mul(2),
        ));

        let all_specs = pallet_welfare::MetricSpecs::<Runtime>::iter().collect::<Vec<_>>();
        for (version, _) in &all_specs {
            pallet_welfare::MetricSpecs::<Runtime>::remove(version);
        }
        assert_eq!(
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                AccountId,
            >>::active_metric_spec_version(),
            None,
        );
        const ACTIVE_VERSION: futarchy_primitives::MetricSpecVersion = 17;
        let cadence_blocks = match u32::try_from(crate::configs::MarketObsInterval::get()) {
            Ok(value) => value,
            Err(_) => {
                assert!(false, "live observation cadence must fit MetricSpec");
                return;
            }
        };
        let active_spec = pallet_welfare::MetricSpec {
            id: 1,
            version: ACTIVE_VERSION,
            pillar: pallet_welfare::Pillar::S,
            weight: futarchy_primitives::FixedU64(pallet_welfare::ONE),
            epsilon_floor: pallet_welfare::EPSILON_PILLAR,
            activation_epoch: pallet_epoch::CurrentEpoch::<Runtime>::get(),
            source: pallet_welfare::SourceClass::Onchain,
            formula_ref: [1; 32],
            units: [2; 16],
            repr: [3; 16],
            cadence_blocks,
            sanity_min: futarchy_primitives::FixedU64(0),
            sanity_max: futarchy_primitives::FixedU64(pallet_welfare::ONE),
            has_normalization_rule: true,
            has_missing_data_rule: true,
            has_gaming_vectors: true,
            has_challenge_procedure: true,
            prior_bounds: [futarchy_primitives::FixedU64(pallet_welfare::ONE);
                pallet_welfare::HISTORY_PRIORS],
        };
        let active_specs = match pallet_welfare::BoundedSpecSet::try_from(vec![active_spec]) {
            Ok(specs) => specs,
            Err(_) => {
                assert!(false, "one MetricSpec must fit the bounded set");
                return;
            }
        };

        let missing_pid = pallet_epoch::NextProposalId::<Runtime>::get();
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(proposer.clone()),
            empty_param_proposal(missing_pid, proposer.clone(), payload_hash, payload_len),
        ));
        let active_pid = pallet_epoch::NextProposalId::<Runtime>::get();
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(proposer.clone()),
            empty_param_proposal(active_pid, proposer.clone(), payload_hash, payload_len),
        ));

        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        let qualify_at = schedule.epoch_start_block.saturating_add(
            schedule
                .length
                .saturating_mul(futarchy_primitives::phase_offsets::QUALIFY_NUM)
                / futarchy_primitives::phase_offsets::DENOMINATOR,
        );
        System::set_block_number(qualify_at);
        let missing_batch = match pallet_epoch::TickBatch::try_from(vec![missing_pid]) {
            Ok(batch) => batch,
            Err(_) => {
                assert!(false, "single qualification crank must fit");
                return;
            }
        };
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(141)),
            missing_batch,
        ));
        let missing = match pallet_epoch::IntakeProposals::<Runtime>::get(missing_pid) {
            Some(proposal) => proposal,
            None => {
                assert!(
                    false,
                    "missing-spec cancellation must remain in current intake"
                );
                return;
            }
        };
        assert_eq!(missing.state, ProposalState::Cancelled);
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Epoch(pallet_epoch::Event::ProposalCancelled {
                pid,
                reason: RejectReason::ProcessHold,
            }) if pid == missing_pid
        )));
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            bond,
            "missing system MetricSpec is not proposer fraud and must refund its bond",
        );

        pallet_welfare::MetricSpecs::<Runtime>::insert(ACTIVE_VERSION, active_specs);
        assert_eq!(
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                AccountId,
            >>::active_metric_spec_version(),
            Some(ACTIVE_VERSION),
        );
        assert!(seed_submitted_as_qualified(active_pid, ACTIVE_VERSION).is_some());
        let qualified = match pallet_epoch::Proposals::<Runtime>::get(active_pid) {
            Some(proposal) => proposal,
            None => {
                assert!(false, "qualified proposal must remain live");
                return;
            }
        };
        assert_eq!(qualified.state, ProposalState::Qualified);
        assert_eq!(qualified.metric_spec, ACTIVE_VERSION);
    });
}

#[test]
fn classless_payloads_cancel_before_qualification_or_market_creation() {
    use frame_support::traits::tokens::{Fortitude, Preservation};

    development_ext().execute_with(|| {
        assert!(install_single_active_metric_spec(32).is_some());
        let bond = crate::configs::balance_param(b"prop.bond.param");
        let market_count_before = pallet_market::Markets::<Runtime>::count();
        let payloads = vec![
            Vec::new(),
            vec![RuntimeCall::Utility(pallet_utility::Call::batch {
                calls: Vec::new(),
            })],
        ];
        let mut submitted = Vec::new();
        for (index, calls) in payloads.into_iter().enumerate() {
            let seed = match u8::try_from(index)
                .ok()
                .and_then(|value| value.checked_add(214))
            {
                Some(seed) => seed,
                None => {
                    assert!(false, "class-less proposer seed must fit");
                    return;
                }
            };
            let proposer = account(seed);
            let (payload_hash, payload_len) = match note_runtime_batch(calls) {
                Some(payload) => payload,
                None => {
                    assert!(false, "class-less batch must encode");
                    return;
                }
            };
            assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, bond));
            let pid = pallet_epoch::NextProposalId::<Runtime>::get();
            assert_ok!(Epoch::submit(
                RuntimeOrigin::signed(proposer.clone()),
                empty_param_proposal(pid, proposer.clone(), payload_hash, payload_len),
            ));
            submitted.push((pid, proposer));
        }

        System::set_block_number(current_qualify_block());
        let tick = match pallet_epoch::TickBatch::try_from(
            submitted.iter().map(|(pid, _)| *pid).collect::<Vec<_>>(),
        ) {
            Ok(tick) => tick,
            Err(_) => {
                assert!(
                    false,
                    "two class-less proposals must fit one screening tick"
                );
                return;
            }
        };
        assert_ok!(Epoch::tick(RuntimeOrigin::signed(account(216)), tick));

        for (pid, proposer) in submitted {
            let cancelled = match pallet_epoch::IntakeProposals::<Runtime>::get(pid) {
                Some(proposal) => proposal,
                None => {
                    assert!(
                        false,
                        "class-less cancellation must remain in intake history"
                    );
                    return;
                }
            };
            assert_eq!(cancelled.state, ProposalState::Cancelled);
            assert!(cancelled.markets.is_none());
            assert!(!pallet_epoch::Proposals::<Runtime>::contains_key(pid));
            assert!(!pallet_epoch::ProposalBonds::<Runtime>::contains_key(pid));
            assert_eq!(
                ForeignAssets::reducible_balance(
                    usdc_location(),
                    &proposer,
                    Preservation::Expendable,
                    Fortitude::Polite,
                ),
                bond,
                "unclassifiable no-op cancellation is refunded under 06 §4",
            );
            assert!(System::events().iter().any(|record| matches!(
                record.event,
                crate::RuntimeEvent::Epoch(pallet_epoch::Event::ProposalCancelled {
                    pid: cancelled_pid,
                    reason: RejectReason::ProcessHold,
                }) if cancelled_pid == pid
            )));
            assert!(!System::events().iter().any(|record| matches!(
                record.event,
                crate::RuntimeEvent::Epoch(pallet_epoch::Event::ProposalQualified(qualified_pid))
                    if qualified_pid == pid
            )));
            assert!(!System::events().iter().any(|record| matches!(
                record.event,
                crate::RuntimeEvent::Epoch(pallet_epoch::Event::MarketsOpened(opened_pid))
                    if opened_pid == pid
            )));
        }
        assert_eq!(
            pallet_market::Markets::<Runtime>::count(),
            market_count_before
        );
    });
}

#[test]
fn proposal_bond_custody_rejects_unfunded_and_second_intake_then_refunds_withdrawal() {
    use frame_support::traits::tokens::{Fortitude, Preservation};

    development_ext().execute_with(|| {
        let proposer = account(143);
        let unfunded = account(144);
        let batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(Vec::new()) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "empty bounded payload must encode");
                    return;
                }
            };
        let bytes = batch.encode();
        let payload_len = match u32::try_from(bytes.len()) {
            Ok(len) => len,
            Err(_) => {
                assert!(false, "bounded payload length must fit u32");
                return;
            }
        };
        let payload_hash = match <Preimage as StorePreimage>::note(bytes.into()) {
            Ok(hash) => hash,
            Err(error) => {
                assert!(false, "empty payload preimage must be noted: {error:?}");
                return;
            }
        };
        let bond = crate::configs::balance_param(b"prop.bond.param");
        assert!(bond > 0);
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &unfunded,
            bond.saturating_sub(1),
        ));
        let unfunded_pid = pallet_epoch::NextProposalId::<Runtime>::get();
        assert!(Epoch::submit(
            RuntimeOrigin::signed(unfunded.clone()),
            empty_param_proposal(unfunded_pid, unfunded, payload_hash, payload_len,),
        )
        .is_err());
        assert!(!pallet_epoch::Proposals::<Runtime>::contains_key(
            unfunded_pid
        ));
        assert!(!pallet_epoch::ProposalBonds::<Runtime>::contains_key(
            unfunded_pid
        ));

        assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, bond));
        let pid = pallet_epoch::NextProposalId::<Runtime>::get();
        let proposal = empty_param_proposal(pid, proposer.clone(), payload_hash, payload_len);
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(proposer.clone()),
            proposal.clone(),
        ));
        assert!(pallet_epoch::ProposalBonds::<Runtime>::contains_key(pid));
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            0,
            "the class bond must be real held USDC, not proposer-supplied ranking metadata",
        );

        assert!(Epoch::submit(RuntimeOrigin::signed(proposer.clone()), proposal).is_err());
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            0,
            "a rejected duplicate must not mutate the one existing hold",
        );

        assert_ok!(Epoch::withdraw(
            RuntimeOrigin::signed(proposer.clone()),
            pid
        ));
        assert!(!pallet_epoch::ProposalBonds::<Runtime>::contains_key(pid));
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            bond,
            "T2 withdrawal refunds the complete held class bond",
        );
    });
}

#[test]
fn proposal_bond_custody_blocks_late_withdrawal_refunds_terminal_reject_and_slashes_t18_once() {
    use frame_support::traits::tokens::{Fortitude, Preservation};

    let qualify = |pid, proposer: AccountId| -> Option<(H256, Balance)> {
        install_single_active_metric_spec(18)?;
        let batch =
            pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(Vec::new()).ok()?;
        let bytes = batch.encode();
        let payload_len = u32::try_from(bytes.len()).ok()?;
        let payload_hash = <Preimage as StorePreimage>::note(bytes.into()).ok()?;
        let bond = crate::configs::balance_param(b"prop.bond.param");
        assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, bond));
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(proposer.clone()),
            empty_param_proposal(pid, proposer, payload_hash, payload_len),
        ));
        seed_submitted_as_qualified(pid, 18)?;
        assert_eq!(
            pallet_epoch::Proposals::<Runtime>::get(pid).map(|proposal| proposal.state),
            Some(ProposalState::Qualified),
        );
        Some((payload_hash, bond))
    };

    development_ext().execute_with(|| {
        let proposer = account(148);
        let pid = pallet_epoch::NextProposalId::<Runtime>::get();
        let Some((_, bond)) = qualify(pid, proposer.clone()) else {
            assert!(false, "qualification fixture must be constructible");
            return;
        };

        assert!(Epoch::withdraw(RuntimeOrigin::signed(proposer.clone()), pid).is_err());
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            0,
            "a qualified proposal cannot withdraw its held class bond",
        );
        assert_eq!(
            pallet_epoch::ProposalBonds::<Runtime>::get(pid).map(|held| held.held),
            Some(bond),
        );

        assert_ok!(Epoch::force_reject_process_hold(
            pallet_origins::Origin::GuardianHold.into(),
            pid,
        ));
        assert!(!pallet_epoch::ProposalBonds::<Runtime>::contains_key(pid));
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            bond,
            "a non-slashing terminal T20 returns the entire held bond",
        );
    });

    development_ext().execute_with(|| {
        let proposer = account(149);
        let pid = pallet_epoch::NextProposalId::<Runtime>::get();
        let Some((_, bond)) = qualify(pid, proposer.clone()) else {
            assert!(false, "qualification fixture must be constructible");
            return;
        };
        let insurance = crate::configs::insurance_account();
        let insurance_before = ForeignAssets::balance(usdc_location(), &insurance);
        pallet_epoch::Proposals::<Runtime>::mutate(pid, |proposal| {
            if let Some(proposal) = proposal {
                proposal.state = ProposalState::Queued;
                proposal.markets = Some(MarketSet {
                    accept: pid.saturating_mul(10).saturating_add(1),
                    reject: pid.saturating_mul(10).saturating_add(2),
                    gates: None,
                    baseline: pid.saturating_mul(10).saturating_add(3),
                });
                proposal.maturity = Some(System::block_number());
                proposal.grace_end = Some(System::block_number().saturating_add(1));
                proposal.decision = Some(DecisionOutcome::Adopt);
            }
        });
        pallet_conditional_ledger::Vaults::<Runtime>::insert(
            pid,
            pallet_conditional_ledger::core_ledger::VaultInfo::open(1),
        );

        assert_ok!(Epoch::mark_failed_executed(
            RuntimeOrigin::signed(crate::configs::execution_guard_account()),
            pid,
        ));
        let slash = bond / 2 + bond % 2;
        let retained = bond.saturating_sub(slash);
        assert_eq!(
            pallet_epoch::ProposalBonds::<Runtime>::get(pid).map(|held| held.held),
            Some(retained),
        );
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &insurance),
            insurance_before.saturating_add(slash),
            "T18 slashes exactly one claimant-adverse half into insurance",
        );

        assert!(Epoch::mark_failed_executed(
            RuntimeOrigin::signed(crate::configs::execution_guard_account()),
            pid,
        )
        .is_err());
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &insurance),
            insurance_before.saturating_add(slash),
            "a repeated T18 callback cannot slash twice",
        );

        assert_ok!(Epoch::retry_exhausted_to_measurement(
            RuntimeOrigin::signed(crate::configs::execution_guard_account()),
            pid,
        ));
        assert!(!pallet_epoch::ProposalBonds::<Runtime>::contains_key(pid));
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            retained,
            "T22 releases the retained half after the one T18 slash",
        );
    });
}

#[test]
fn missing_preimage_terminal_path_slashes_the_live_param_fraction_to_insurance() {
    use frame_support::traits::tokens::{Fortitude, Preservation};

    development_ext().execute_with(|| {
        let proposer = account(145);
        let bond = crate::configs::balance_param(b"prop.bond.param");
        let slash_pct = match pallet_constitution::Params::<Runtime>::get(
            pallet_constitution::key16(b"intake.slash_pct"),
        ) {
            Some(record) => match record.value {
                pallet_constitution::ParamValue::Percent(value) => value,
                _ => {
                    assert!(false, "intake.slash_pct must remain Percent-typed");
                    return;
                }
            },
            None => {
                assert!(false, "intake.slash_pct must exist in live Params");
                return;
            }
        };
        let slash = bond.saturating_mul(Balance::from(slash_pct)) / 100;
        let insurance = crate::configs::insurance_account();
        let insurance_before = ForeignAssets::balance(usdc_location(), &insurance);
        assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, bond));
        let missing_hash = H256::repeat_byte(151);
        let pid = pallet_epoch::NextProposalId::<Runtime>::get();
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(proposer.clone()),
            empty_param_proposal(pid, proposer.clone(), missing_hash, 1),
        ));

        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        System::set_block_number(
            schedule.epoch_start_block.saturating_add(
                schedule
                    .length
                    .saturating_mul(futarchy_primitives::phase_offsets::QUALIFY_NUM)
                    / futarchy_primitives::phase_offsets::DENOMINATOR,
            ),
        );
        let batch = match pallet_epoch::TickBatch::try_from(vec![pid]) {
            Ok(batch) => batch,
            Err(_) => {
                assert!(false, "single qualification crank must fit");
                return;
            }
        };
        assert_ok!(Epoch::tick(RuntimeOrigin::signed(account(146)), batch));
        let cancelled = match pallet_epoch::IntakeProposals::<Runtime>::get(pid) {
            Some(proposal) => proposal,
            None => {
                assert!(
                    false,
                    "preimage-missing cancellation must remain in current intake"
                );
                return;
            }
        };
        assert_eq!(cancelled.state, ProposalState::Cancelled);
        let cancellation_reason = RejectReason::NotDecisionGrade;
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Epoch(pallet_epoch::Event::ProposalCancelled {
                pid: cancelled_pid,
                reason: RejectReason::NotDecisionGrade,
            }) if cancelled_pid == pid
        )));
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            bond.saturating_sub(slash),
        );
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &insurance),
            insurance_before.saturating_add(slash),
        );
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Epoch(pallet_epoch::Event::IntakeSlashed {
                pid: slashed_pid,
                reason,
                amount,
                ..
            }) if slashed_pid == pid && reason == cancellation_reason && amount == slash
        )));
    });
}

#[test]
fn real_proposal_bond_custody_covers_full_static_slash_and_not_decision_grade_partial_slash() {
    use frame_support::traits::tokens::{Fortitude, Preservation};

    development_ext().execute_with(|| {
        assert!(install_single_active_metric_spec(19).is_some());
        let proposer = account(151);
        let bond = crate::configs::balance_param(b"prop.bond.param");
        let insurance = crate::configs::insurance_account();
        let insurance_before = ForeignAssets::balance(usdc_location(), &insurance);
        let batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(Vec::new()) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "empty payload batch must fit");
                    return;
                }
            };
        let bytes = batch.encode();
        let payload_len = match u32::try_from(bytes.len()) {
            Ok(len) => len,
            Err(_) => {
                assert!(false, "bounded payload length must fit");
                return;
            }
        };
        let payload_hash = match <Preimage as StorePreimage>::note(bytes.into()) {
            Ok(hash) => hash,
            Err(error) => {
                assert!(false, "payload preimage must be noted: {error:?}");
                return;
            }
        };
        assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, bond));
        let pid = pallet_epoch::NextProposalId::<Runtime>::get();
        let mut proposal = empty_param_proposal(pid, proposer.clone(), payload_hash, payload_len);
        proposal.resources = match futarchy_primitives::BoundedVec::try_from(vec![[151; 8]]) {
            Ok(resources) => resources,
            Err(_) => {
                assert!(false, "one false resource declaration must fit");
                return;
            }
        };
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(proposer.clone()),
            proposal
        ));

        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        System::set_block_number(
            schedule.epoch_start_block.saturating_add(
                schedule
                    .length
                    .saturating_mul(futarchy_primitives::phase_offsets::QUALIFY_NUM)
                    / futarchy_primitives::phase_offsets::DENOMINATOR,
            ),
        );
        let tick = match pallet_epoch::TickBatch::try_from(vec![pid]) {
            Ok(tick) => tick,
            Err(_) => {
                assert!(false, "single qualification tick must fit");
                return;
            }
        };
        assert_ok!(Epoch::tick(RuntimeOrigin::signed(account(152)), tick));
        let cancelled = match pallet_epoch::IntakeProposals::<Runtime>::get(pid) {
            Some(proposal) => proposal,
            None => {
                assert!(
                    false,
                    "false-resource cancellation must remain in current intake"
                );
                return;
            }
        };
        assert_eq!(cancelled.state, ProposalState::Cancelled);
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Epoch(pallet_epoch::Event::ProposalCancelled {
                pid: cancelled_pid,
                reason: RejectReason::ConstitutionViolation,
            }) if cancelled_pid == pid
        )));
        assert!(!pallet_epoch::ProposalBonds::<Runtime>::contains_key(pid));
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            0,
            "a false resource declaration loses the complete real bond",
        );
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &insurance),
            insurance_before.saturating_add(bond),
        );
    });

    development_ext().execute_with(|| {
        assert!(install_single_active_metric_spec(20).is_some());
        let proposer = account(153);
        let bond = crate::configs::balance_param(b"prop.bond.param");
        let insurance = crate::configs::insurance_account();
        let insurance_before = ForeignAssets::balance(usdc_location(), &insurance);
        let batch =
            match pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::try_from(Vec::new()) {
                Ok(batch) => batch,
                Err(_) => {
                    assert!(false, "empty payload batch must fit");
                    return;
                }
            };
        let bytes = batch.encode();
        let payload_len = match u32::try_from(bytes.len()) {
            Ok(len) => len,
            Err(_) => {
                assert!(false, "bounded payload length must fit");
                return;
            }
        };
        let payload_hash = match <Preimage as StorePreimage>::note(bytes.into()) {
            Ok(hash) => hash,
            Err(error) => {
                assert!(false, "payload preimage must be noted: {error:?}");
                return;
            }
        };
        assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, bond));
        let pid = pallet_epoch::NextProposalId::<Runtime>::get();
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(proposer.clone()),
            empty_param_proposal(pid, proposer.clone(), payload_hash, payload_len),
        ));
        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        assert!(seed_submitted_as_qualified(pid, 20).is_some());
        let qualified = match pallet_epoch::Proposals::<Runtime>::get(pid) {
            Some(proposal) => proposal,
            None => {
                assert!(
                    false,
                    "proposal must qualify before decision-grade rejection"
                );
                return;
            }
        };
        let end = qualified.decide_at;
        let epoch = qualified.epoch;
        // This regression targets partial decision-grade slashing, not POL
        // capacity. Keep the newly enforced live budget from deferring its
        // deliberately hand-built market fixture.
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = Balance::MAX;
        });
        let seed_at = schedule.epoch_start_block.saturating_add(
            schedule
                .length
                .saturating_mul(futarchy_primitives::phase_offsets::SEED_NUM)
                / futarchy_primitives::phase_offsets::DENOMINATOR,
        );
        System::set_block_number(seed_at);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(154)),
            Default::default(),
        ));
        let trade_at = schedule.epoch_start_block.saturating_add(
            schedule
                .length
                .saturating_mul(futarchy_primitives::phase_offsets::TRADE_NUM)
                / futarchy_primitives::phase_offsets::DENOMINATOR,
        );
        System::set_block_number(trade_at);
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(154)),
            Default::default(),
        ));
        let ids = MarketSet {
            accept: 91_001,
            reject: 91_002,
            gates: None,
            baseline: 91_003,
        };
        let params =
            <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        let class = ProposalClass::Param;
        let contest = params.v_min[crate::configs::proposal_class_index(class)];
        let decision_b = crate::configs::class_pol_floor(class);
        let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
        for result in [
            seed_decision_grade_market(
                ids.accept,
                pallet_market::core_market::BookKind::Decision {
                    proposal: pid,
                    branch: futarchy_primitives::Branch::Accept,
                },
                futarchy_primitives::FixedU64(500_000_000),
                end,
                (params.decision_window, params.trailing_window),
                decision_b,
                0,
            ),
            seed_decision_grade_market(
                ids.reject,
                pallet_market::core_market::BookKind::Decision {
                    proposal: pid,
                    branch: futarchy_primitives::Branch::Reject,
                },
                futarchy_primitives::FixedU64(500_000_000),
                end,
                (params.decision_window, params.trailing_window),
                decision_b,
                contest,
            ),
            seed_decision_grade_market(
                ids.baseline,
                pallet_market::core_market::BookKind::Baseline { epoch },
                futarchy_primitives::FixedU64(500_000_000),
                end,
                (params.decision_window, params.trailing_window),
                baseline_b,
                contest,
            ),
        ] {
            assert_ok!(result);
        }
        pallet_market::BaselineMarketOf::<Runtime>::insert(epoch, ids.baseline);
        pallet_epoch::Proposals::<Runtime>::mutate(pid, |proposal| {
            if let Some(proposal) = proposal {
                proposal.state = ProposalState::Extended;
                proposal.extended = true;
                proposal.markets = Some(ids);
            }
        });
        pallet_conditional_ledger::Vaults::<Runtime>::insert(
            pid,
            pallet_conditional_ledger::core_ledger::VaultInfo::open(20),
        );
        System::set_block_number(end);

        assert_ok!(Epoch::decide(RuntimeOrigin::signed(account(155)), pid));
        assert_eq!(
            pallet_epoch::Proposals::<Runtime>::get(pid)
                .map(|proposal| (proposal.state, proposal.decision)),
            Some((
                ProposalState::Measuring,
                Some(DecisionOutcome::Reject(RejectReason::NotDecisionGrade)),
            )),
        );
        let slash_pct = match pallet_constitution::Params::<Runtime>::get(
            pallet_constitution::key16(b"intake.slash_pct"),
        ) {
            Some(record) => match record.value {
                pallet_constitution::ParamValue::Percent(value) => value,
                _ => {
                    assert!(false, "intake.slash_pct must remain Percent-typed");
                    return;
                }
            },
            None => {
                assert!(false, "intake.slash_pct must exist in live Params");
                return;
            }
        };
        let slash = bond
            .saturating_mul(Balance::from(slash_pct))
            .saturating_add(99)
            / 100;
        assert!(!pallet_epoch::ProposalBonds::<Runtime>::contains_key(pid));
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            bond.saturating_sub(slash),
        );
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &insurance),
            insurance_before.saturating_add(slash),
        );
    });
}

#[test]
fn verified_batch_all_payload_qualifies_and_later_bond_floor_drift_refunds() {
    use frame_support::traits::tokens::{Fortitude, Preservation};

    development_ext().execute_with(|| {
        assert!(install_single_active_metric_spec(21).is_some());
        let proposer = account(156);
        let insurance = crate::configs::insurance_account();
        let insurance_before = ForeignAssets::balance(usdc_location(), &insurance);
        let record = match pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(
            b"mkt.obs_interval",
        )) {
            Some(record) => record,
            None => {
                assert!(false, "registered PARAM record must exist");
                return;
            }
        };
        let resource = expected_resource_key(0x01, Some(&record.key));
        let call = RuntimeCall::Utility(pallet_utility::Call::batch_all {
            calls: vec![registered_param_call(record)],
        });
        let pid = match submit_param_payload(proposer.clone(), vec![call], vec![resource]) {
            Some(pid) => pid,
            None => {
                assert!(false, "canonical PARAM proposal must submit");
                return;
            }
        };
        assert!(tick_qualification(vec![pid]).is_some());
        let qualified = match pallet_epoch::Proposals::<Runtime>::get(pid) {
            Some(proposal) => proposal,
            None => {
                assert!(false, "verified payload must enter the live proposal set");
                return;
            }
        };
        assert_eq!(qualified.state, ProposalState::Qualified);
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Epoch(pallet_epoch::Event::ProposalQualified(qualified_pid))
                if qualified_pid == pid
        )));
        assert_eq!(
            pallet_epoch::ResourceLocks::<Runtime>::get().into_inner(),
            vec![(resource, pid)],
        );
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            0,
            "a qualified proposal keeps its real bond in escrow",
        );
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &insurance),
            insurance_before,
        );
        assert!(pallet_epoch::ProposalBonds::<Runtime>::contains_key(pid));
    });

    development_ext().execute_with(|| {
        let initial_schedule = pallet_epoch::Schedule::<Runtime>::get();
        System::set_block_number(
            initial_schedule
                .epoch_start_block
                .saturating_add(initial_schedule.length.saturating_mul(2)),
        );
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(158)),
            Default::default(),
        ));
        assert!(install_single_active_metric_spec(22).is_some());

        let proposer = account(159);
        let bond = crate::configs::balance_param(b"prop.bond.param");
        let (payload_hash, payload_len) = match note_runtime_batch(Vec::new()) {
            Some(payload) => payload,
            None => {
                assert!(false, "empty runtime batch must be encodable");
                return;
            }
        };
        assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, bond));
        let pid = pallet_epoch::NextProposalId::<Runtime>::get();
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(proposer.clone()),
            empty_param_proposal(pid, proposer.clone(), payload_hash, payload_len),
        ));

        let raised_floor = match bond.checked_mul(2) {
            Some(value) => value,
            None => {
                assert!(false, "bounded proposal-bond floor must not overflow");
                return;
            }
        };
        assert_ok!(Constitution::set_param(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::key16(b"prop.bond.param"),
            pallet_constitution::ParamValue::Balance(raised_floor),
        ));
        System::set_block_number(current_qualify_block());
        let batch = match pallet_epoch::TickBatch::try_from(vec![pid]) {
            Ok(batch) => batch,
            Err(_) => {
                assert!(false, "single floor-drift qualification tick must fit");
                return;
            }
        };
        assert_ok!(Epoch::tick(RuntimeOrigin::signed(account(160)), batch));
        let cancelled = match pallet_epoch::IntakeProposals::<Runtime>::get(pid) {
            Some(proposal) => proposal,
            None => {
                assert!(
                    false,
                    "floor-drift cancellation must remain in current intake"
                );
                return;
            }
        };
        assert_eq!(cancelled.state, ProposalState::Cancelled);
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Epoch(pallet_epoch::Event::ProposalCancelled {
                pid: cancelled_pid,
                reason: RejectReason::ProcessHold,
            }) if cancelled_pid == pid
        )));
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            bond,
            "a governance floor change after submission is not proposer fraud",
        );
        assert!(!pallet_epoch::ProposalBonds::<Runtime>::contains_key(pid));
    });
}

#[test]
fn false_resource_declarations_under_over_and_wrong_fully_slash() {
    use frame_support::traits::tokens::{Fortitude, Preservation};

    development_ext().execute_with(|| {
        assert!(install_single_active_metric_spec(23).is_some());
        let record = match pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(
            b"mkt.obs_interval",
        )) {
            Some(record) => record,
            None => {
                assert!(false, "registered PARAM record must exist");
                return;
            }
        };
        let correct = expected_resource_key(0x01, Some(&record.key));
        let wrong = expected_resource_key(0x01, Some(&pallet_constitution::key16(b"mkt.fee")));
        let declarations = [Vec::new(), vec![correct, wrong], vec![wrong]];
        let bond = crate::configs::balance_param(b"prop.bond.param");
        let insurance = crate::configs::insurance_account();
        let insurance_before = ForeignAssets::balance(usdc_location(), &insurance);
        let mut submitted = Vec::new();

        for (index, resources) in declarations.into_iter().enumerate() {
            let seed = match u8::try_from(index)
                .ok()
                .and_then(|index| index.checked_add(230))
            {
                Some(seed) => seed,
                None => {
                    assert!(false, "mismatch proposer seed must fit");
                    return;
                }
            };
            let proposer = account(seed);
            let pid = match submit_param_payload(
                proposer.clone(),
                vec![registered_param_call(record)],
                resources,
            ) {
                Some(pid) => pid,
                None => {
                    assert!(false, "false declaration fixture must submit");
                    return;
                }
            };
            submitted.push((pid, proposer));
        }

        assert!(tick_qualification(submitted.iter().map(|(pid, _)| *pid).collect()).is_some());
        for (pid, proposer) in &submitted {
            assert_eq!(
                pallet_epoch::IntakeProposals::<Runtime>::get(pid).map(|p| p.state),
                Some(ProposalState::Cancelled),
            );
            assert!(System::events().iter().any(|record| matches!(
                record.event,
                crate::RuntimeEvent::Epoch(pallet_epoch::Event::ProposalCancelled {
                    pid: cancelled_pid,
                    reason: RejectReason::ConstitutionViolation,
                }) if cancelled_pid == *pid
            )));
            assert!(!pallet_epoch::ProposalBonds::<Runtime>::contains_key(pid));
            assert_eq!(
                ForeignAssets::reducible_balance(
                    usdc_location(),
                    proposer,
                    Preservation::Expendable,
                    Fortitude::Polite,
                ),
                0,
                "a verified false resource declaration loses its whole bond",
            );
        }
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &insurance),
            insurance_before.saturating_add(bond.saturating_mul(3)),
        );
        assert!(pallet_epoch::ResourceLocks::<Runtime>::get().is_empty());
    });
}

#[test]
fn mixed_valid_and_values_scope_leaves_use_unclassifiable_refund_slash_taxonomy() {
    use frame_support::traits::tokens::{Fortitude, Preservation};

    development_ext().execute_with(|| {
        assert!(install_single_active_metric_spec(29).is_some());
        let valid = match pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(
            b"mkt.obs_interval",
        )) {
            Some(record) => record,
            None => {
                assert!(false, "registered PARAM record must exist");
                return;
            }
        };
        let values_only = match pallet_constitution::genesis_params()
            .into_iter()
            .find(|record| {
                matches!(
                    record.class,
                    pallet_constitution::ParamClass::Const
                        | pallet_constitution::ParamClass::Entrenched
                )
            }) {
            Some(record) => record,
            None => {
                assert!(false, "registry must contain a values-scope parameter");
                return;
            }
        };
        let calls = vec![
            registered_param_call(valid),
            registered_param_call(values_only),
        ];
        assert!(crate::classifier::derive_resource_footprint(&calls).is_err());
        let resource = expected_resource_key(0x01, Some(&valid.key));
        let refunded = account(243);
        let slashed = account(244);
        let refund_pid = match submit_param_payload(refunded.clone(), calls.clone(), Vec::new()) {
            Some(pid) => pid,
            None => {
                assert!(false, "mixed empty-declaration fixture must submit");
                return;
            }
        };
        let slash_pid = match submit_param_payload(slashed.clone(), calls, vec![resource]) {
            Some(pid) => pid,
            None => {
                assert!(false, "mixed non-empty-declaration fixture must submit");
                return;
            }
        };
        let bond = crate::configs::balance_param(b"prop.bond.param");
        let insurance = crate::configs::insurance_account();
        let insurance_before = ForeignAssets::balance(usdc_location(), &insurance);
        assert!(tick_qualification(vec![refund_pid, slash_pid]).is_some());
        for (pid, reason) in [
            (refund_pid, RejectReason::ProcessHold),
            (slash_pid, RejectReason::ConstitutionViolation),
        ] {
            assert!(System::events().iter().any(|record| matches!(
                record.event,
                crate::RuntimeEvent::Epoch(pallet_epoch::Event::ProposalCancelled {
                    pid: cancelled_pid,
                    reason: cancelled_reason,
                }) if cancelled_pid == pid && cancelled_reason == reason
            )));
        }
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &refunded,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            bond,
        );
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &slashed,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            0,
        );
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &insurance),
            insurance_before.saturating_add(bond),
        );
    });
}

#[test]
fn empty_payload_with_empty_resources_refunds_process_hold() {
    use frame_support::traits::tokens::{Fortitude, Preservation};

    development_ext().execute_with(|| {
        assert!(install_single_active_metric_spec(24).is_some());
        let proposer = account(233);
        let bond = crate::configs::balance_param(b"prop.bond.param");
        let pid = match submit_param_payload(proposer.clone(), Vec::new(), Vec::new()) {
            Some(pid) => pid,
            None => {
                assert!(false, "empty class-less proposal fixture must submit");
                return;
            }
        };
        assert!(tick_qualification(vec![pid]).is_some());
        assert_eq!(
            pallet_epoch::IntakeProposals::<Runtime>::get(pid).map(|p| p.state),
            Some(ProposalState::Cancelled),
        );
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Epoch(pallet_epoch::Event::ProposalCancelled {
                pid: cancelled_pid,
                reason: RejectReason::ProcessHold,
            }) if cancelled_pid == pid
        )));
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            bond,
        );
        assert!(!pallet_epoch::ProposalBonds::<Runtime>::contains_key(pid));
    });
}

#[test]
fn resource_lock_conflict_rolls_second_proposal_and_distinct_keys_both_qualify() {
    development_ext().execute_with(|| {
        assert!(install_single_active_metric_spec(25).is_some());
        let record = match pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(
            b"mkt.obs_interval",
        )) {
            Some(record) => record,
            None => {
                assert!(false, "registered PARAM record must exist");
                return;
            }
        };
        let resource = expected_resource_key(0x01, Some(&record.key));
        let first = match submit_param_payload(
            account(234),
            vec![registered_param_call(record)],
            vec![resource],
        ) {
            Some(pid) => pid,
            None => {
                assert!(false, "first conflict proposal must submit");
                return;
            }
        };
        let second = match submit_param_payload(
            account(235),
            vec![registered_param_call(record)],
            vec![resource],
        ) {
            Some(pid) => pid,
            None => {
                assert!(false, "second conflict proposal must submit");
                return;
            }
        };
        assert!(tick_qualification(vec![first, second]).is_some());
        assert_eq!(stored_proposal_state(first), Some(ProposalState::Qualified));
        assert_eq!(
            stored_proposal_state(second),
            Some(ProposalState::Submitted)
        );
        assert_eq!(
            pallet_epoch::IntakeProposals::<Runtime>::get(second).map(|p| p.epoch),
            Some(pallet_epoch::CurrentEpoch::<Runtime>::get().saturating_add(1)),
        );
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Epoch(pallet_epoch::Event::ProposalDeferred(pid))
                if pid == second
        )));
        assert_eq!(
            pallet_epoch::ResourceLocks::<Runtime>::get().into_inner(),
            vec![(resource, first)],
        );
    });

    development_ext().execute_with(|| {
        assert!(install_single_active_metric_spec(26).is_some());
        let first_record = match pallet_constitution::Params::<Runtime>::get(
            pallet_constitution::key16(b"mkt.obs_interval"),
        ) {
            Some(record) => record,
            None => {
                assert!(false, "first registered PARAM record must exist");
                return;
            }
        };
        let second_record = match pallet_constitution::Params::<Runtime>::get(
            pallet_constitution::key16(b"mkt.fee"),
        ) {
            Some(record) => record,
            None => {
                assert!(false, "second registered PARAM record must exist");
                return;
            }
        };
        assert_ok!(Constitution::set_capability(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::CapabilityRecord {
                class: ProposalClass::Param,
                capability: pallet_constitution::Capability::SetParam(second_record.key),
                enabled: true,
            },
        ));
        let first_resource = expected_resource_key(0x01, Some(&first_record.key));
        let second_resource = expected_resource_key(0x01, Some(&second_record.key));
        let first = match submit_param_payload(
            account(236),
            vec![registered_param_call(first_record)],
            vec![first_resource],
        ) {
            Some(pid) => pid,
            None => {
                assert!(false, "first distinct proposal must submit");
                return;
            }
        };
        let second = match submit_param_payload(
            account(237),
            vec![registered_param_call(second_record)],
            vec![second_resource],
        ) {
            Some(pid) => pid,
            None => {
                assert!(false, "second distinct proposal must submit");
                return;
            }
        };
        assert!(tick_qualification(vec![first, second]).is_some());
        assert_eq!(stored_proposal_state(first), Some(ProposalState::Qualified));
        assert_eq!(
            stored_proposal_state(second),
            Some(ProposalState::Qualified)
        );
        let locks = pallet_epoch::ResourceLocks::<Runtime>::get().into_inner();
        assert_eq!(locks.len(), 2);
        assert!(locks.contains(&(first_resource, first)));
        assert!(locks.contains(&(second_resource, second)));
    });
}

#[test]
fn duplicate_resource_declaration_acquires_one_lock_and_does_not_wedge_qualification() {
    development_ext().execute_with(|| {
        assert!(install_single_active_metric_spec(33).is_some());
        let first_record = pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(
            b"mkt.obs_interval",
        ));
        assert!(first_record.is_some(), "first PARAM record must exist");
        let Some(first_record) = first_record else {
            return;
        };
        let second_record =
            pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(b"mkt.fee"));
        assert!(second_record.is_some(), "second PARAM record must exist");
        let Some(second_record) = second_record else {
            return;
        };
        assert_ok!(Constitution::set_capability(
            pallet_origins::Origin::FutarchyMeta.into(),
            pallet_constitution::CapabilityRecord {
                class: ProposalClass::Param,
                capability: pallet_constitution::Capability::SetParam(second_record.key),
                enabled: true,
            },
        ));

        let first_resource = expected_resource_key(0x01, Some(&first_record.key));
        let second_resource = expected_resource_key(0x01, Some(&second_record.key));
        let first = submit_param_payload(
            account(246),
            vec![registered_param_call(first_record)],
            vec![first_resource, first_resource],
        );
        assert!(
            first.is_some(),
            "duplicate-insensitive proposal must submit"
        );
        let Some(first) = first else {
            return;
        };
        let second = submit_param_payload(
            account(247),
            vec![registered_param_call(second_record)],
            vec![second_resource],
        );
        assert!(second.is_some(), "distinct-key proposal must submit");
        let Some(second) = second else {
            return;
        };

        assert!(tick_qualification(vec![first, second]).is_some());
        assert_eq!(stored_proposal_state(first), Some(ProposalState::Qualified));
        assert_eq!(
            stored_proposal_state(second),
            Some(ProposalState::Qualified)
        );
        assert_eq!(
            pallet_epoch::ResourceLocks::<Runtime>::get().into_inner(),
            vec![(first_resource, first), (second_resource, second)],
            "qualification must persist exactly one lock per distinct resource"
        );
        assert_ok!(Epoch::do_try_state());
    });
}

#[test]
fn overbound_footprint_slashes_empty_declaration_but_unknown_wrapper_refunds() {
    use frame_support::traits::tokens::{Fortitude, Preservation};

    development_ext().execute_with(|| {
        assert!(install_single_active_metric_spec(27).is_some());
        let records = pallet_constitution::genesis_params()
            .into_iter()
            .filter(|record| record.class == pallet_constitution::ParamClass::Param)
            .take(9)
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 9, "the registry must expose nine PARAM keys");
        for record in &records {
            assert_ok!(Constitution::set_capability(
                pallet_origins::Origin::FutarchyMeta.into(),
                pallet_constitution::CapabilityRecord {
                    class: ProposalClass::Param,
                    capability: pallet_constitution::Capability::SetParam(record.key),
                    enabled: true,
                },
            ));
        }
        let calls = records
            .iter()
            .copied()
            .map(registered_param_call)
            .collect::<Vec<_>>();
        assert!(crate::classifier::derive_resource_footprint(&calls).is_err());

        let empty_overbound = account(238);
        let nonempty_overbound = account(239);
        let unknown_wrapper = account(248);
        let empty_overbound_pid =
            submit_param_payload(empty_overbound.clone(), calls.clone(), Vec::new());
        assert!(
            empty_overbound_pid.is_some(),
            "over-bound empty-declaration fixture must submit"
        );
        let Some(empty_overbound_pid) = empty_overbound_pid else {
            return;
        };
        let nonempty_overbound_pid =
            submit_param_payload(nonempty_overbound.clone(), calls, vec![[0xee; 8]]);
        assert!(
            nonempty_overbound_pid.is_some(),
            "over-bound non-empty-declaration fixture must submit"
        );
        let Some(nonempty_overbound_pid) = nonempty_overbound_pid else {
            return;
        };
        let unknown_wrapper_pid = submit_param_payload(
            unknown_wrapper.clone(),
            vec![RuntimeCall::Utility(pallet_utility::Call::batch {
                calls: vec![registered_param_call(records[0])],
            })],
            Vec::new(),
        );
        assert!(
            unknown_wrapper_pid.is_some(),
            "unknown-wrapper empty-declaration fixture must submit"
        );
        let Some(unknown_wrapper_pid) = unknown_wrapper_pid else {
            return;
        };
        let bond = crate::configs::balance_param(b"prop.bond.param");
        let insurance = crate::configs::insurance_account();
        let insurance_before = ForeignAssets::balance(usdc_location(), &insurance);
        assert!(tick_qualification(vec![
            empty_overbound_pid,
            nonempty_overbound_pid,
            unknown_wrapper_pid,
        ])
        .is_some());

        for (pid, reason) in [
            (empty_overbound_pid, RejectReason::ConstitutionViolation),
            (nonempty_overbound_pid, RejectReason::ConstitutionViolation),
            (unknown_wrapper_pid, RejectReason::ProcessHold),
        ] {
            assert_eq!(
                pallet_epoch::IntakeProposals::<Runtime>::get(pid).map(|p| p.state),
                Some(ProposalState::Cancelled),
            );
            assert!(System::events().iter().any(|record| matches!(
                record.event,
                crate::RuntimeEvent::Epoch(pallet_epoch::Event::ProposalCancelled {
                    pid: cancelled_pid,
                    reason: cancelled_reason,
                }) if cancelled_pid == pid && cancelled_reason == reason
            )));
        }
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &empty_overbound,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            0,
        );
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &nonempty_overbound,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            0,
        );
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &unknown_wrapper,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            bond,
        );
        assert_eq!(
            ForeignAssets::balance(usdc_location(), &insurance),
            insurance_before.saturating_add(bond.saturating_mul(2)),
        );
    });
}

#[test]
fn treasury_spend_resource_key_is_0x07_plus_beneficiary_digest() {
    development_ext().execute_with(|| {
        let destination = account(240);
        let call = RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::spend {
            line: pallet_futarchy_treasury::BudgetLine::Pol,
            dest: destination.clone(),
            amount: 1,
        });
        let resource = match derived_single_resource(call) {
            Some(resource) => resource,
            None => {
                assert!(false, "treasury spend must derive one resource");
                return;
            }
        };
        let digest = sp_io::hashing::blake2_256(&destination.encode());
        let expected = [
            0x07, digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6],
        ];
        assert_eq!(resource, expected);
    });
}

#[test]
fn canonical_resource_footprint_enforces_call_and_nesting_bounds() {
    development_ext().execute_with(|| {
        let record = match pallet_constitution::Params::<Runtime>::get(pallet_constitution::key16(
            b"mkt.obs_interval",
        )) {
            Some(record) => record,
            None => {
                assert!(false, "registered PARAM record must exist");
                return;
            }
        };
        let leaf = registered_param_call(record);
        let at_call_limit = (0..kernel::MAX_NESTED_CALLS)
            .map(|_| leaf.clone())
            .collect::<Vec<_>>();
        let footprint = match crate::classifier::derive_resource_footprint(&at_call_limit) {
            Ok(footprint) => footprint,
            Err(_) => {
                assert!(false, "the exact total-call limit must classify");
                return;
            }
        };
        assert_eq!(footprint.len(), 1, "duplicate leaf keys are deduplicated");
        let beyond_call_limit = (0..=kernel::MAX_NESTED_CALLS)
            .map(|_| leaf.clone())
            .collect::<Vec<_>>();
        assert!(crate::classifier::derive_resource_footprint(&beyond_call_limit).is_err());

        let mut at_depth_limit = leaf.clone();
        for _ in 0..kernel::MAX_NESTED_LEVELS {
            at_depth_limit = RuntimeCall::Utility(pallet_utility::Call::batch_all {
                calls: vec![at_depth_limit],
            });
        }
        assert!(crate::classifier::derive_resource_footprint(&[at_depth_limit.clone()]).is_ok());
        let beyond_depth_limit = RuntimeCall::Utility(pallet_utility::Call::batch_all {
            calls: vec![at_depth_limit],
        });
        assert!(crate::classifier::derive_resource_footprint(&[beyond_depth_limit]).is_err());
    });
}

#[test]
fn canonical_resource_key_universe_has_no_semantic_collisions() {
    fn insert_distinct(
        keys: &mut Vec<futarchy_primitives::ResourceId>,
        key: futarchy_primitives::ResourceId,
    ) {
        assert!(
            !keys.contains(&key),
            "semantically distinct resource keys must not collide: {key:?}",
        );
        keys.push(key);
    }

    development_ext().execute_with(|| {
        let params = pallet_constitution::genesis_params();
        let mut keys = Vec::new();

        // Both 0x01 leaves deliberately identify the same parameter record;
        // insert that semantic resource once after asserting byte equality.
        for record in &params {
            let set_param = expected_resource_key(0x01, Some(&record.key));
            let amend_registry = expected_resource_key(0x01, Some(&record.key));
            assert_eq!(set_param, amend_registry);
            insert_distinct(&mut keys, set_param);
        }

        let classes = [
            ProposalClass::Param,
            ProposalClass::Treasury,
            ProposalClass::Code,
            ProposalClass::Meta,
            ProposalClass::Constitutional,
        ];
        let fixed_capabilities = [
            pallet_constitution::Capability::SetCapability,
            pallet_constitution::Capability::AmendRegistry,
            pallet_constitution::Capability::SetReleaseChannel,
            pallet_constitution::Capability::AuthorizeUpgrade,
            pallet_constitution::Capability::TreasurySpend,
            pallet_constitution::Capability::OracleConfig,
            pallet_constitution::Capability::MarketTemplate,
        ];
        for class in classes {
            for capability in fixed_capabilities {
                let call = RuntimeCall::Constitution(pallet_constitution::Call::set_capability {
                    record: pallet_constitution::CapabilityRecord {
                        class,
                        capability,
                        enabled: true,
                    },
                });
                let key = match derived_single_resource(call) {
                    Some(key) => key,
                    None => {
                        assert!(false, "enumerable capability must derive a key");
                        return;
                    }
                };
                insert_distinct(&mut keys, key);
            }
            for record in &params {
                let call = RuntimeCall::Constitution(pallet_constitution::Call::set_capability {
                    record: pallet_constitution::CapabilityRecord {
                        class,
                        capability: pallet_constitution::Capability::SetParam(record.key),
                        enabled: true,
                    },
                });
                let key = match derived_single_resource(call) {
                    Some(key) => key,
                    None => {
                        assert!(false, "keyed capability must derive a key");
                        return;
                    }
                };
                insert_distinct(&mut keys, key);
            }
        }

        for singleton in [0x03, 0x04, 0x05] {
            insert_distinct(&mut keys, expected_resource_key(singleton, None));
        }
        for instance in [0_u8, 1_u8] {
            insert_distinct(
                &mut keys,
                expected_resource_key(0x06, Some(&instance.encode())),
            );
        }
        for seed in 0_u8..16 {
            insert_distinct(
                &mut keys,
                expected_resource_key(0x07, Some(&account(seed).encode())),
            );
        }
        for id in [0_u64, 1, u64::MAX] {
            insert_distinct(&mut keys, expected_resource_key(0x08, Some(&id.encode())));
        }
        for line in [
            pallet_futarchy_treasury::BudgetLine::Pol,
            pallet_futarchy_treasury::BudgetLine::PolBaseline,
            pallet_futarchy_treasury::BudgetLine::Keeper,
            pallet_futarchy_treasury::BudgetLine::Oracle,
            pallet_futarchy_treasury::BudgetLine::Rewards,
            pallet_futarchy_treasury::BudgetLine::OpsBootnodes,
            pallet_futarchy_treasury::BudgetLine::OpsRpcArchive,
            pallet_futarchy_treasury::BudgetLine::OpsCollators,
            pallet_futarchy_treasury::BudgetLine::OpsKeepers,
            pallet_futarchy_treasury::BudgetLine::OpsOracleEvidence,
            pallet_futarchy_treasury::BudgetLine::OpsWatchtowers,
            pallet_futarchy_treasury::BudgetLine::OpsMonitoring,
            pallet_futarchy_treasury::BudgetLine::OpsArweave,
            pallet_futarchy_treasury::BudgetLine::OpsCoretime,
        ] {
            insert_distinct(&mut keys, expected_resource_key(0x09, Some(&line.encode())));
        }
        assert!(keys.len() > params.len());
    });
}

#[test]
fn qualified_real_payload_passes_guard_domain_rederivation_and_executes() {
    use pallet_epoch::ExecutionGuardAccess;

    development_ext().execute_with(|| {
        assert!(install_single_active_metric_spec(28).is_some());
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| state.main_usdc = 10);
        let line = pallet_futarchy_treasury::BudgetLine::Pol;
        let resource = expected_resource_key(0x09, Some(&line.encode()));
        let call =
            RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
                line,
                amount: 1,
            });
        let (payload_hash, payload_len) = match note_runtime_batch(vec![call]) {
            Some(payload) => payload,
            None => {
                assert!(false, "real Treasury execution fixture must encode");
                return;
            }
        };
        let proposer = account(241);
        let pid = pallet_epoch::NextProposalId::<Runtime>::get();
        let mut submitted = empty_param_proposal(pid, proposer.clone(), payload_hash, payload_len);
        submitted.class = ProposalClass::Treasury;
        submitted.bond = crate::configs::balance_param(b"prop.bond.trs");
        submitted.resources = match futarchy_primitives::BoundedVec::try_from(vec![resource]) {
            Ok(resources) => resources,
            Err(_) => {
                assert!(false, "one Treasury resource must fit");
                return;
            }
        };
        assert_eq!(
            crate::configs::required_proposal_bond(&submitted),
            Some(submitted.bond),
        );
        assert_eq!(
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                AccountId,
            >>::in_cap_prize(&submitted),
            Some(0),
        );
        let disposition =
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                AccountId,
            >>::static_check(&submitted);
        assert!(
            matches!(disposition, pallet_epoch::StaticCheckDisposition::Eligible),
            "canonical Treasury fixture must pass static screening: {disposition:?}",
        );
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &proposer,
            submitted.bond,
        ));
        assert_ok!(Epoch::submit(RuntimeOrigin::signed(proposer), submitted));
        assert!(tick_qualification(vec![pid]).is_some());
        assert_eq!(stored_proposal_state(pid), Some(ProposalState::Qualified));
        assert_eq!(
            pallet_epoch::ResourceLocks::<Runtime>::get().into_inner(),
            vec![(resource, pid)],
        );

        let proposal = match pallet_epoch::Proposals::<Runtime>::get(pid) {
            Some(proposal) => proposal,
            None => {
                assert!(false, "qualified proposal must be live");
                return;
            }
        };
        let maturity = System::block_number().saturating_add(
            <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_timelock(
                ProposalClass::Treasury,
            ),
        );
        let grace = <crate::configs::ExecutionParams as pallet_execution_guard::Params>::exec_grace(
            ProposalClass::Treasury,
        );
        assert_ok!(
            <crate::configs::RuntimeEpochExecutionGuard as ExecutionGuardAccess>::enqueue(
                pid,
                proposal.payload_hash,
                proposal.version_constraint.clone(),
                maturity,
                grace,
                false,
            )
        );
        pallet_epoch::Proposals::<Runtime>::mutate(pid, |stored| {
            if let Some(stored) = stored {
                stored.state = ProposalState::Queued;
                stored.maturity = Some(maturity);
                stored.grace_end = Some(maturity.saturating_add(grace));
                stored.decision = Some(DecisionOutcome::Adopt);
                stored.markets = Some(MarketSet {
                    accept: pid.saturating_mul(10).saturating_add(1),
                    reject: pid.saturating_mul(10).saturating_add(2),
                    gates: None,
                    baseline: pid.saturating_mul(10).saturating_add(3),
                });
            }
        });
        pallet_conditional_ledger::Vaults::<Runtime>::insert(
            pid,
            pallet_conditional_ledger::core_ledger::VaultInfo::open(1),
        );
        System::set_block_number(maturity);
        assert_ok!(ExecutionGuard::execute(
            RuntimeOrigin::signed(account(242)),
            pid,
        ));
        assert_eq!(stored_proposal_state(pid), Some(ProposalState::Measuring));
        assert!(!pallet_execution_guard::Queue::<Runtime>::contains_key(pid));
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::FutarchyTreasury(
                pallet_futarchy_treasury::Event::BudgetLineFunded {
                    line: pallet_futarchy_treasury::BudgetLine::Pol,
                    amount: 1,
                }
            )
        )));
    });
}

#[test]
fn classless_screening_outcome_is_independent_of_keeper_permutation() {
    let forward = match qualification_states_for_order(false) {
        Some(outcome) => outcome,
        None => {
            assert!(false, "forward qualification permutation must execute");
            return;
        }
    };
    let reverse = match qualification_states_for_order(true) {
        Some(outcome) => outcome,
        None => {
            assert!(false, "reverse qualification permutation must execute");
            return;
        }
    };
    assert_eq!(forward, reverse, "keeper order cannot decide scarce slots");
    let (states, slots) = forward;
    assert!(slots > 0);
    assert!(states
        .iter()
        .all(|state| *state == ProposalState::Cancelled));
    assert_eq!(
        states
            .iter()
            .filter(|state| **state == ProposalState::Qualified)
            .count(),
        0,
        "class-less candidates cannot consume any scarce qualification slot",
    );
}

#[test]
fn classless_and_unverifiable_high_bonds_cannot_consume_qualification_slots() {
    use frame_support::traits::tokens::{Fortitude, Preservation};

    development_ext().execute_with(|| {
        assert!(install_single_active_metric_spec(31).is_some());
        let slots = usize::from(
            <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get()
                .epoch_slots,
        );
        let floor = crate::configs::balance_param(b"prop.bond.param");
        let (classless_hash, classless_len) = match note_runtime_batch(Vec::new()) {
            Some(payload) => payload,
            None => {
                assert!(false, "class-less empty batch must encode");
                return;
            }
        };
        let (ineligible_hash, ineligible_len) = match note_runtime_batch(vec![remark()]) {
            Some(payload) => payload,
            None => {
                assert!(false, "ineligible non-empty batch must encode");
                return;
            }
        };
        let mut classless = Vec::new();
        for index in 0..slots {
            let seed = match u8::try_from(index)
                .ok()
                .and_then(|value| value.checked_add(191))
            {
                Some(seed) => seed,
                None => {
                    assert!(false, "class-less proposer seed must fit");
                    return;
                }
            };
            let proposer = account(seed);
            let premium = match Balance::try_from(index)
                .ok()
                .and_then(|value| value.checked_add(1))
            {
                Some(premium) => premium,
                None => {
                    assert!(false, "class-less bond premium must fit");
                    return;
                }
            };
            let held = floor.saturating_add(premium);
            assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, held));
            let pid = pallet_epoch::NextProposalId::<Runtime>::get();
            let mut proposal =
                empty_param_proposal(pid, proposer.clone(), classless_hash, classless_len);
            proposal.bond = held;
            assert_ok!(Epoch::submit(RuntimeOrigin::signed(proposer), proposal));
            classless.push(pid);
        }

        let ineligible_proposer = account(210);
        let high_bond = floor.saturating_mul(2);
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &ineligible_proposer,
            high_bond,
        ));
        let ineligible_pid = pallet_epoch::NextProposalId::<Runtime>::get();
        let mut proposal = empty_param_proposal(
            ineligible_pid,
            ineligible_proposer.clone(),
            ineligible_hash,
            ineligible_len,
        );
        proposal.bond = high_bond;
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(ineligible_proposer.clone()),
            proposal,
        ));

        // Both the class-less and resource-unverifiable candidates are
        // refundable, but neither may reach slot allocation regardless of
        // bond size or caller-controlled crank order.
        let mut order = classless.clone();
        order.push(ineligible_pid);
        System::set_block_number(current_qualify_block());
        let batch = match pallet_epoch::TickBatch::try_from(order) {
            Ok(batch) => batch,
            Err(_) => {
                assert!(false, "bounded intake fixture must fit one tick");
                return;
            }
        };
        assert_ok!(Epoch::tick(RuntimeOrigin::signed(account(211)), batch));
        for pid in classless {
            assert_eq!(stored_proposal_state(pid), Some(ProposalState::Cancelled));
        }
        assert_eq!(
            stored_proposal_state(ineligible_pid),
            Some(ProposalState::Cancelled),
        );
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &ineligible_proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            high_bond,
            "the unverifiable high-bond entry is refunded but never consumes a slot",
        );
    });
}

#[test]
fn stale_submitted_proposal_cannot_withdraw_after_rolling_into_a_later_epoch() {
    use frame_support::traits::tokens::{Fortitude, Preservation};

    development_ext().execute_with(|| {
        let proposer = account(212);
        let bond = crate::configs::balance_param(b"prop.bond.param");
        let (payload_hash, payload_len) = match note_runtime_batch(Vec::new()) {
            Some(payload) => payload,
            None => {
                assert!(false, "empty batch must encode");
                return;
            }
        };
        assert_ok!(ForeignAssets::mint_into(usdc_location(), &proposer, bond));
        let pid = pallet_epoch::NextProposalId::<Runtime>::get();
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(proposer.clone()),
            empty_param_proposal(pid, proposer.clone(), payload_hash, payload_len),
        ));
        let submitted_epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        let schedule = pallet_epoch::Schedule::<Runtime>::get();
        System::set_block_number(schedule.epoch_start_block.saturating_add(schedule.length));
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(213)),
            Default::default(),
        ));
        assert!(pallet_epoch::CurrentEpoch::<Runtime>::get() > submitted_epoch);

        assert!(Epoch::withdraw(RuntimeOrigin::signed(proposer.clone()), pid).is_err());
        assert_eq!(stored_proposal_state(pid), Some(ProposalState::Submitted));
        assert_eq!(
            pallet_epoch::ProposalBonds::<Runtime>::get(pid).map(|bond| bond.held),
            Some(bond),
        );
        assert_eq!(
            ForeignAssets::reducible_balance(
                usdc_location(),
                &proposer,
                Preservation::Expendable,
                Fortitude::Polite,
            ),
            0,
            "a stale monopolizer cannot time a full-refund withdrawal in a later Intake phase",
        );
    });
}

#[test]
fn unavailable_welfare_metric_inputs_reject_without_locking_empty_snapshots() {
    development_ext().execute_with(|| {
        System::set_block_number(1);
        let result = RuntimeCall::Welfare(pallet_welfare::Call::record_snapshot {
            epoch: 0,
            spec_version: 0,
        })
        .dispatch(RuntimeOrigin::signed(Sr25519Keyring::Alice.to_account_id()));
        assert!(result.is_err());
        assert!(pallet_welfare::Snapshots::<Runtime>::iter()
            .next()
            .is_none());
    });
}

#[test]
fn invalid_frozen_oracle_schedule_holds_the_decision_fail_closed() {
    use pallet_epoch::OracleAccess;

    development_ext().execute_with(|| {
        const COMPONENT: futarchy_primitives::MetricId = 41;
        const SPEC: futarchy_primitives::MetricSpecVersion = 17;
        let round = pallet_oracle::RoundState {
            component: COMPONENT,
            epoch: pallet_epoch::CurrentEpoch::<Runtime>::get(),
            round: 1,
            spec_version: SPEC,
            reporter: [31; 32],
            value: futarchy_primitives::FixedU64(500_000_000),
            evidence_hash: [32; 32],
            bond: 0,
            challenge_deadline: System::block_number().saturating_add(1),
            extended: false,
            challenger: Some([33; 32]),
            counter_value: Some(futarchy_primitives::FixedU64(400_000_000)),
            acks: 0,
            report_hash: [34; 32],
            stake_at_risk: Balance::MAX,
            cumulative_reporter_bond: 0,
            cumulative_challenger_bond: 0,
        };
        pallet_oracle::Rounds::<Runtime>::insert((COMPONENT, round.epoch, SPEC), round);
        pallet_oracle::RoundSchedules::<Runtime>::insert(
            (COMPONENT, round.epoch, SPEC),
            pallet_oracle::StoredRoundSchedule {
                round_one_bond: 0,
                // Outside the supported envelope, so frozen-schedule validation
                // fails and G-1 conservatively holds the decision.
                round_cap: kernel::ORC_ROUNDS_MAX.saturating_add(1),
            },
        );

        assert!(
            crate::configs::RuntimeEpochOracle::any_open_dispute_touching(SPEC),
            "invalid frozen schedule must conservatively hold the decision",
        );
    });
}

#[test]
fn open_oracle_dispute_merit_floor_uses_its_frozen_game_bond() {
    use pallet_epoch::OracleAccess;

    development_ext().execute_with(|| {
        const COMPONENT: futarchy_primitives::MetricId = 42;
        const SPEC: futarchy_primitives::MetricSpecVersion = 18;
        const FROZEN_BOND: Balance = 10_000_000_000;
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        pallet_oracle::Rounds::<Runtime>::insert(
            (COMPONENT, epoch, SPEC),
            pallet_oracle::RoundState {
                component: COMPONENT,
                epoch,
                round: 1,
                spec_version: SPEC,
                reporter: [41; 32],
                value: futarchy_primitives::FixedU64(500_000_000),
                evidence_hash: [42; 32],
                bond: FROZEN_BOND,
                challenge_deadline: System::block_number().saturating_add(1),
                extended: false,
                challenger: Some([43; 32]),
                counter_value: Some(futarchy_primitives::FixedU64(400_000_000)),
                acks: 0,
                report_hash: [44; 32],
                stake_at_risk: 400_000_000_000,
                cumulative_reporter_bond: FROZEN_BOND,
                cumulative_challenger_bond: FROZEN_BOND,
            },
        );
        pallet_oracle::RoundSchedules::<Runtime>::insert(
            (COMPONENT, epoch, SPEC),
            pallet_oracle::StoredRoundSchedule {
                round_one_bond: FROZEN_BOND,
                round_cap: pallet_oracle::ORC_ROUNDS,
            },
        );

        set_balance_param_value(b"orc.bond_floor", FROZEN_BOND.saturating_mul(10));
        let bps_key = pallet_constitution::key16(b"orc.bond_bps");
        pallet_constitution::Params::<Runtime>::mutate(bps_key, |record| {
            if let Some(record) = record {
                record.value = pallet_constitution::ParamValue::Perbill(100_000_000);
            }
        });

        assert!(
            crate::configs::RuntimeEpochOracle::any_open_dispute_touching(SPEC),
            "live bond amendments cannot reprice an already-open game's merit floor",
        );
    });
}

#[test]
fn executive_builds_and_executes_inherents_and_a_fee_paying_vit_transfer() {
    let destination = account(42);
    let block = development_ext().execute_with(|| build_executive_smoke_block(destination.clone()));
    development_ext().execute_with(|| {
        let alice = Sr25519Keyring::Alice.to_account_id();
        let before = Balances::free_balance(&alice);
        crate::Executive::execute_block(block.into());
        assert_eq!(Timestamp::get(), kernel::MILLISECS_PER_BLOCK);
        assert_eq!(
            Balances::free_balance(&destination),
            currency::VIT_EXISTENTIAL_DEPOSIT
        );
        assert!(Balances::free_balance(&alice) < before - currency::VIT_EXISTENTIAL_DEPOSIT);
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::Balances(pallet_balances::Event::Transfer { .. })
        )));
        assert!(System::events().iter().any(|record| matches!(
            record.event,
            crate::RuntimeEvent::TransactionPayment(
                pallet_transaction_payment::Event::TransactionFeePaid { .. }
            )
        )));
    });
}

#[cfg(feature = "try-runtime")]
#[test]
fn executive_smoke_state_passes_all_try_state_checks() {
    use frame_support::traits::TryState;

    let destination = account(43);
    let block = development_ext().execute_with(|| build_executive_smoke_block(destination));
    development_ext().execute_with(|| {
        crate::Executive::execute_block(block.into());
        assert!(
            <crate::AllPalletsWithSystem as TryState<crate::BlockNumber>>::try_state(
                System::block_number(),
                frame_try_runtime::TryStateSelect::All,
            )
            .is_ok()
        );
    });
}

#[cfg(feature = "try-runtime")]
#[test]
fn try_runtime_api_executes_genesis_upgrade_and_try_state_checks() {
    development_ext().execute_with(|| {
        let input = frame_try_runtime::UpgradeCheckSelect::All.encode();
        let Some(output) = crate::apis::api::dispatch("TryRuntime_on_runtime_upgrade", &input)
        else {
            assert!(false, "TryRuntime runtime API method must be generated");
            return;
        };
        let decoded = <(Weight, Weight) as parity_scale_codec::Decode>::decode(&mut &output[..]);
        match decoded {
            Ok((used, maximum)) => assert!(used.all_lte(maximum)),
            Err(error) => assert!(false, "TryRuntime result must decode: {error}"),
        }
    });
}

// --- Post-authoring review regressions (session fixes over the Codex draft) ---

#[test]
fn ump_send_and_balances_force_calls_are_nobody_even_under_sudo() {
    let mut calls = vec![
        RuntimeCall::ParachainSystem(
            cumulus_pallet_parachain_system::Call::sudo_send_upward_message { message: vec![1] },
        ),
        RuntimeCall::Balances(pallet_balances::Call::force_transfer {
            source: MultiAddress::Id(account(1)),
            dest: MultiAddress::Id(account(2)),
            value: 1,
        }),
        RuntimeCall::Balances(pallet_balances::Call::force_unreserve {
            who: MultiAddress::Id(account(1)),
            amount: 1,
        }),
        RuntimeCall::Balances(pallet_balances::Call::force_set_balance {
            who: MultiAddress::Id(account(1)),
            new_free: 1,
        }),
        RuntimeCall::Balances(pallet_balances::Call::force_adjust_total_issuance {
            direction: pallet_balances::AdjustmentDirection::Increase,
            delta: 1,
        }),
    ];
    for call in calls.drain(..) {
        assert!(
            !RuntimeBaseCallFilter::contains(&call),
            "bare force/UMP call must be nobody: {call:?}"
        );
        for origin in pallet_origins::Origin::ALL {
            assert!(
                !RuntimeBaseCallFilter::contains_for(origin.to_model(), &call),
                "no custom origin may reach the nobody row: {call:?}"
            );
        }
        let sudo_wrapped = RuntimeCall::Sudo(pallet_sudo::Call::sudo {
            call: Box::new(call.clone()),
        });
        assert!(
            !RuntimeBaseCallFilter::contains(&sudo_wrapped),
            "sudo wrapper must not launder the nobody row: {call:?}"
        );
    }
    assert!(RuntimeBaseCallFilter::contains(&RuntimeCall::Balances(
        pallet_balances::Call::transfer_keep_alive {
            dest: MultiAddress::Id(account(2)),
            value: 1,
        }
    )));
}

#[test]
fn origin_aware_matrix_is_not_widened_by_the_values_leaf_admission() {
    let adjudicate = RuntimeCall::Oracle(pallet_oracle::Call::adjudicate {
        component: 1,
        epoch: 1,
        spec_version: 1,
        value: futarchy_primitives::FixedU64(0),
        reporter_wrong: false,
    });
    // The stock-scheduler accommodation admits the bare leaf origin-blind …
    assert!(RuntimeBaseCallFilter::contains(&adjudicate));
    // … but the origin-aware matrix check stays exact: only OracleResolution.
    assert!(RuntimeBaseCallFilter::contains_for(
        ClassOrigin::OracleResolution,
        &adjudicate
    ));
    assert!(!RuntimeBaseCallFilter::contains_for(
        ClassOrigin::ConstitutionalValues,
        &adjudicate
    ));
    // And a values leaf is admitted as a BARE leaf only — wrappers still deny.
    let resolve = RuntimeCall::Attestor(pallet_attestor::Call::resolve_challenge {
        attestation_id: 0,
        attestation_upheld: false,
    });
    assert!(RuntimeBaseCallFilter::contains(&resolve));
    assert!(!RuntimeBaseCallFilter::contains(&RuntimeCall::Utility(
        pallet_utility::Call::batch {
            calls: vec![resolve.clone()],
        }
    )));
    assert!(!RuntimeBaseCallFilter::contains(&RuntimeCall::Proxy(
        pallet_proxy::Call::proxy {
            real: MultiAddress::Id(account(1)),
            force_proxy_type: None,
            call: Box::new(resolve),
        }
    )));
}

#[test]
fn set_param_domain_follows_the_registry_key_class() {
    development_ext().execute_with(|| {
        let set = |name: &[u8]| {
            RuntimeCall::Constitution(pallet_constitution::Call::set_param {
                key: pallet_constitution::key16(name),
                value: pallet_constitution::ParamValue::Balance(1),
            })
        };
        // PARAM-class key (mkt.fee) — FutarchyParam only (06 §3.2 row 1).
        assert!(RuntimeBaseCallFilter::contains_for(
            ClassOrigin::FutarchyParam,
            &set(b"mkt.fee")
        ));
        assert!(!RuntimeBaseCallFilter::contains_for(
            ClassOrigin::FutarchyTreasury,
            &set(b"mkt.fee")
        ));
        // TREASURY-class key (pol.b_gate) — FutarchyTreasury only (row 2).
        assert!(RuntimeBaseCallFilter::contains_for(
            ClassOrigin::FutarchyTreasury,
            &set(b"pol.b_gate")
        ));
        assert!(!RuntimeBaseCallFilter::contains_for(
            ClassOrigin::FutarchyParam,
            &set(b"pol.b_gate")
        ));
        // Unknown key fails closed for every origin and origin-less.
        assert!(!RuntimeBaseCallFilter::contains(&set(b"no.such_key")));
        for origin in pallet_origins::Origin::ALL {
            assert!(!RuntimeBaseCallFilter::contains_for(
                origin.to_model(),
                &set(b"no.such_key")
            ));
        }
        // Origin-less submission of any real set_param stays denied (privileged).
        assert!(!RuntimeBaseCallFilter::contains(&set(b"mkt.fee")));
    });
}

#[test]
fn live_param_adapters_resolve_their_registry_keys() {
    use frame_support::traits::Get;
    development_ext().execute_with(|| {
        // A typo'd key name would silently fall through to 0 — pin every
        // adapter to its 13 §1 genesis value (rule 4).
        assert_eq!(
            crate::configs::LedgerMinSplit::get(),
            kernel::MIN_SPLIT_USDC
        );
        assert_eq!(
            crate::configs::LedgerPositionDeposit::get(),
            kernel::POSITION_DEPOSIT_USDC
        );
        assert_eq!(crate::configs::MarketFee::get(), 30);
        assert_eq!(crate::configs::MarketObsInterval::get(), 10);
        assert_eq!(crate::configs::MarketKappa::get(), 5_000_000);
        assert!(crate::configs::LedgerArchiveDelay::get() > 0);
    });
}

#[test]
fn gate_v_min_is_a_live_bounded_param_not_a_hardwired_decision_floor_ratio() {
    use pallet_epoch::EpochParamsProvider;

    development_ext().execute_with(|| {
        let class = ProposalClass::Treasury;
        let index = crate::configs::proposal_class_index(class);
        let key = pallet_constitution::key16(b"gate.v_min.trs");
        let current = match pallet_constitution::Params::<Runtime>::get(key) {
            Some(record) => match record.value {
                pallet_constitution::ParamValue::Balance(value) => value,
                _ => {
                    assert!(false, "gate.v_min.trs must remain Balance-typed");
                    return;
                }
            },
            None => {
                assert!(false, "13 §1 gate.v_min.trs must exist in Params");
                return;
            }
        };
        let before = crate::configs::RuntimeEpochParams::get();
        assert_eq!(before.gate_v_min[index], current);

        let next = match current.checked_mul(2) {
            Some(value) => value,
            None => {
                assert!(false, "bounded gate floor fixture must not overflow");
                return;
            }
        };
        // Satisfy the row's live cooldown before exercising its max-Δ engine.
        pallet_epoch::EpochOf::<Runtime>::mutate(|clock| {
            clock.index = clock.index.saturating_add(2)
        });
        assert_ok!(Constitution::set_param(
            pallet_origins::Origin::FutarchyMeta.into(),
            key,
            pallet_constitution::ParamValue::Balance(next),
        ));
        assert_eq!(
            crate::configs::RuntimeEpochParams::get().gate_v_min[index],
            next,
        );

        // 13 §1 caps gate.v_min at 0.5× dec.v_min. A later decision
        // outside that coupling must be rejected and leave the live value put.
        pallet_epoch::EpochOf::<Runtime>::mutate(|clock| {
            clock.index = clock.index.saturating_add(2)
        });
        let above_coupling = crate::configs::RuntimeEpochParams::get().v_min[index];
        assert!(Constitution::set_param(
            pallet_origins::Origin::FutarchyMeta.into(),
            key,
            pallet_constitution::ParamValue::Balance(above_coupling),
        )
        .is_err());
        assert_eq!(
            crate::configs::RuntimeEpochParams::get().gate_v_min[index],
            next,
        );
    });
}

#[test]
fn deferred_metric_input_incident_multiplier_uses_the_neutral_identity() {
    use pallet_welfare::MetricInputs;
    development_ext().execute_with(|| {
        // No closed registry epoch ⇒ the neutral 1.0 multiplier (a zero would
        // erase C_attested outright — fail-destructive, not fail-safe).
        assert_eq!(
            crate::configs::RuntimeMetricInputs::incident_multiplier(5),
            futarchy_primitives::FixedU64(1_000_000_000)
        );
    });
}

#[test]
fn sudo_as_is_denied_so_the_founding_multisig_cannot_impersonate_accounts() {
    // P1 (Codex adversarial review): `sudo_as(who, call)` dispatches as
    // `Signed(who)` for a CHOSEN `who`, so recursing it would let the founding
    // multisig forge any signed origin — steal VIT (`transfer`) or, worse,
    // impersonate the welfare settlement account to drive ledger settlement,
    // defeating 06 §3.1's "SettleAuthority reachable through exactly one path".
    // `sudo_as` is denied outright; `sudo`/`sudo_unchecked_weight` (Root
    // dispatch) stay recursed.
    let victim_transfer = RuntimeCall::Balances(pallet_balances::Call::transfer_keep_alive {
        dest: MultiAddress::Id(account(99)),
        value: 1,
    });
    let forge_settlement =
        RuntimeCall::ConditionalLedger(pallet_conditional_ledger::Call::settle_scalar {
            pid: 0,
            s: futarchy_primitives::FixedU64(0),
        });
    for inner in [victim_transfer, forge_settlement, remark()] {
        let sudo_as = RuntimeCall::Sudo(pallet_sudo::Call::sudo_as {
            who: MultiAddress::Id(crate::configs::welfare_settlement_account()),
            call: Box::new(inner.clone()),
        });
        assert!(
            !RuntimeBaseCallFilter::contains(&sudo_as),
            "sudo_as must be denied for every inner call: {inner:?}"
        );
        // …and it must not become reachable by wrapping it further.
        assert!(!RuntimeBaseCallFilter::contains(&RuntimeCall::Utility(
            pallet_utility::Call::batch {
                calls: vec![sudo_as.clone()],
            }
        )));
        // The Root-dispatching variants still recurse a benign public inner.
        let sudo_root = RuntimeCall::Sudo(pallet_sudo::Call::sudo {
            call: Box::new(remark()),
        });
        assert!(RuntimeBaseCallFilter::contains(&sudo_root));
    }
}

#[test]
fn const_and_entrenched_set_param_are_enactable_by_constitutional_values() {
    // P2#5: a passed `constitution`/`entrenched` values referendum enacting
    // `set_param` on a CONST/entrenched key must survive the origin-blind base
    // filter (stock scheduler dispatches filtered, SQ-32) — its produced origin
    // is ConstitutionalValues and its `GovernanceOrigin` check is the second
    // gate. PARAM/TREASURY/META keys must NOT get this bare-leaf admission.
    development_ext().execute_with(|| {
        let set = |name: &[u8]| {
            RuntimeCall::Constitution(pallet_constitution::Call::set_param {
                key: pallet_constitution::key16(name),
                value: pallet_constitution::ParamValue::Fixed(futarchy_primitives::FixedU64(
                    950_000_000,
                )),
            })
        };
        // CONST key + Entrenched key: admitted origin-blind (values-enactment leaf).
        for key in [b"welfare.thS_lo".as_slice(), b"att.bond".as_slice()] {
            assert!(
                RuntimeBaseCallFilter::contains(&set(key)),
                "CONST/entrenched set_param must be enactable: {key:?}"
            );
            assert!(crate::classifier::is_values_enactment_leaf(&set(key)));
            // Still bare-leaf only — a wrapper carrying it is denied.
            assert!(!RuntimeBaseCallFilter::contains(&RuntimeCall::Utility(
                pallet_utility::Call::batch {
                    calls: vec![set(key)],
                }
            )));
        }
        // PARAM key (mkt.fee) is NOT a values-enactment leaf — belief side.
        assert!(!crate::classifier::is_values_enactment_leaf(&set(
            b"mkt.fee"
        )));
        assert!(!RuntimeBaseCallFilter::contains(&set(b"mkt.fee")));
    });
}

#[test]
fn genesis_phase_flags_advertise_sudo_present_alongside_the_sudo_key() {
    // P2#7: the preset installs a sudo key, so bit 4 (SUDO_PRESENT) MUST be set
    // — the FE binds its bootstrap-governance banner to it (09 §5.2).
    development_ext().execute_with(|| {
        let flags = pallet_constitution::PhaseFlags::<Runtime>::get();
        assert_eq!(
            flags,
            pallet_constitution::PhaseFlagsValue::SHADOW_MODE
                | pallet_constitution::PhaseFlagsValue::SUDO_PRESENT
        );
        assert!(
            pallet_sudo::Key::<Runtime>::get().is_some(),
            "preset installs a sudo key"
        );
        assert_ne!(
            flags & pallet_constitution::PhaseFlagsValue::SUDO_PRESENT,
            0,
            "SUDO_PRESENT must be set whenever a sudo key exists"
        );
    });
}

#[test]
fn referenda_support_curves_decay_high_to_low_without_underflow() {
    // A floor/ceil-swapped curve underflows inside `Curve::threshold`. Exercise
    // every distinct six-track support curve at 0/½/1 turnout and pin its
    // normative endpoints.
    use sp_runtime::Perbill;
    let eval = |curve: &pallet_referenda::Curve, x: Perbill| curve.threshold(x);
    let cases = [
        (
            &crate::configs::METRIC_SUPPORT,
            Perbill::from_percent(10),
            Perbill::from_percent(2),
        ),
        (
            &crate::configs::CONSTITUTION_SUPPORT,
            Perbill::from_percent(15),
            Perbill::from_percent(5),
        ),
        (
            &crate::configs::ENTRENCHED_SUPPORT,
            Perbill::from_percent(20),
            Perbill::from_percent(10),
        ),
        (
            &crate::configs::GUARDIAN_SUPPORT,
            Perbill::from_percent(5),
            Perbill::from_percent(5),
        ),
        (
            &crate::configs::RATIFY_SUPPORT,
            Perbill::from_percent(5),
            Perbill::from_percent(5),
        ),
        (
            &crate::configs::ORACLE_SUPPORT,
            Perbill::from_percent(10),
            Perbill::from_percent(3),
        ),
    ];
    for (curve, at_zero, at_one) in cases {
        let lo = eval(curve, Perbill::zero());
        let mid = eval(curve, Perbill::from_rational(1u32, 2u32));
        let hi = eval(curve, Perbill::one());
        assert_eq!(lo, at_zero, "support requirement at turnout 0 is the ceil");
        assert_eq!(hi, at_one, "support requirement at turnout 1 is the floor");
        // Monotone high→low with the exact endpoints proves the curve is not
        // floor/ceil-swapped: a swapped curve wraps (mid far above the ceil) or
        // panics under overflow-checks before reaching here.
        assert!(
            lo >= mid && mid >= hi,
            "support requirement must decay monotonically"
        );
        if at_zero != at_one {
            assert!(
                mid < at_zero && mid > at_one,
                "midpoint strictly between unequal endpoints"
            );
        }
    }
    assert_eq!(
        crate::configs::METRIC_APPROVAL.threshold(Perbill::zero()),
        Perbill::from_percent(60)
    );
    assert_eq!(
        crate::configs::METRIC_APPROVAL.threshold(Perbill::one()),
        Perbill::from_percent(50)
    );
    assert_eq!(
        crate::configs::ENTRENCHED_APPROVAL.threshold(Perbill::from_rational(1u32, 3u32)),
        Perbill::from_percent(80)
    );
    assert_eq!(
        crate::configs::ORACLE_APPROVAL.threshold(Perbill::from_rational(3u32, 4u32)),
        Perbill::from_percent(60)
    );
}

#[test]
fn six_referenda_tracks_have_normative_schedules_and_origins() {
    use pallet_referenda::TracksInfo;

    let expected = [
        (0, 2, 14, 2, 14, 10_000),
        (1, 2, 21, 3, 28, 25_000),
        (2, 7, 28, 7, 84, 50_000),
        (3, 1, 7, 1, 2, 5_000),
        (4, 1, 7, 1, 0, 1_000),
        (5, 0, 7, 1, 0, 5_000),
    ];
    assert_eq!(crate::configs::TRACKS.len(), expected.len());
    for (track, (id, prepare, decision, confirm, enactment, deposit)) in
        crate::configs::TRACKS.iter().zip(expected)
    {
        assert_eq!(track.id, id);
        assert_eq!(track.info.prepare_period, prepare * kernel::BLOCKS_PER_DAY);
        assert_eq!(
            track.info.decision_period,
            decision * kernel::BLOCKS_PER_DAY
        );
        assert_eq!(track.info.confirm_period, confirm * kernel::BLOCKS_PER_DAY);
        assert_eq!(
            track.info.min_enactment_period,
            enactment * kernel::BLOCKS_PER_DAY
        );
        assert_eq!(track.info.decision_deposit, deposit * currency::VIT);
    }

    for (origin, id) in [
        (crate::track_origins::Origin::Metric, 0),
        (crate::track_origins::Origin::Constitution, 1),
        (crate::track_origins::Origin::Entrenched, 2),
        (crate::track_origins::Origin::GuardianTrack, 3),
        (crate::track_origins::Origin::Ratify, 4),
    ] {
        let runtime_origin: RuntimeOrigin = origin.into();
        assert_eq!(
            crate::configs::BleavitTracks::track_for(runtime_origin.caller()),
            Ok(id)
        );
    }
    let legacy: RuntimeOrigin = pallet_origins::Origin::ConstitutionalValues.into();
    assert_eq!(
        crate::configs::BleavitTracks::track_for(legacy.caller()),
        Ok(2)
    );
}

#[test]
fn constitution_track_cannot_amend_entrenched_class_but_entrenched_track_can() {
    development_ext().execute_with(|| {
        let key = pallet_constitution::key16(b"att.bond");
        let record = pallet_constitution::Params::<Runtime>::get(key);
        assert!(record.is_some(), "att.bond must be seeded");
        let Some(record) = record else {
            return;
        };
        assert_eq!(record.class, pallet_constitution::ParamClass::Entrenched);
        pallet_epoch::EpochOf::<Runtime>::mutate(|clock| {
            clock.index = clock.index.saturating_add(record.cooldown_epochs)
        });

        let constitution_origin: RuntimeOrigin = crate::track_origins::Origin::Constitution.into();
        let entrenched_origin: RuntimeOrigin = crate::track_origins::Origin::Entrenched.into();
        let next = pallet_constitution::ParamValue::Balance(
            record.value.as_u128().saturating_add(currency::VIT),
        );
        assert_noop!(
            Constitution::set_param(constitution_origin.clone(), key, next),
            DispatchError::BadOrigin
        );
        assert_ok!(Constitution::set_param(
            entrenched_origin.clone(),
            key,
            next
        ));

        assert_noop!(
            Constitution::amend_registry(
                constitution_origin,
                key,
                record.min,
                record.max,
                record.max_delta,
                record.cooldown_epochs,
            ),
            DispatchError::BadOrigin
        );
        assert_ok!(Constitution::amend_registry(
            entrenched_origin,
            key,
            record.min,
            record.max,
            record.max_delta,
            record.cooldown_epochs,
        ));
    });
}

#[test]
fn referenda_cancel_and_kill_are_enactable_by_constitutional_values() {
    // PR #57 Codex-bot P2: `referenda.cancel`/`kill` are ConstitutionalValues-
    // domain (the runtime's Cancel/Kill origins), so a values referendum
    // enacting them must clear the origin-blind base filter (bare-leaf values
    // enactment); otherwise the scheduler's filtered dispatch rejects
    // `CallFiltered` before the origin check, leaving both controls unreachable.
    for call in [
        RuntimeCall::Referenda(pallet_referenda::Call::cancel { index: 0 }),
        RuntimeCall::Referenda(pallet_referenda::Call::kill { index: 0 }),
    ] {
        assert!(crate::classifier::is_values_enactment_leaf(&call));
        assert!(
            RuntimeBaseCallFilter::contains(&call),
            "cancel/kill must pass the base filter as a bare values-enactment leaf: {call:?}"
        );
        // Bare leaf only — a wrapper carrying it stays denied.
        assert!(!RuntimeBaseCallFilter::contains(&RuntimeCall::Utility(
            pallet_utility::Call::batch {
                calls: vec![call.clone()]
            }
        )));
        // Signed origin still dies at the pallet's Cancel/KillOrigin (BadOrigin),
        // not at the filter — the base filter admits, the EnsureOrigin rejects.
        assert!(RuntimeBaseCallFilter::contains_for(
            ClassOrigin::ConstitutionalValues,
            &call
        ));
    }
}

// ------------------------------------------------------- B2 runtime views --

#[test]
fn view_quote_matches_core_rounding_and_fails_closed() {
    use futarchy_primitives::{Branch, FixedU64, TradeSide};
    use pallet_market::core_market::{BookKind, MarketBook};

    development_ext().execute_with(|| {
        const MARKET_ID: u64 = 41;
        const B: u128 = 10_000_000_000;
        let book = MarketBook::open(
            MARKET_ID,
            BookKind::Decision {
                proposal: 7,
                branch: Branch::Accept,
            },
            account(31),
            account(32),
            B,
        );
        pallet_market::Markets::<Runtime>::insert(MARKET_ID, book.clone());

        let amount = 1_000_000;
        let expected = pallet_market::core_market::quote(
            &book,
            TradeSide::BuyLong,
            amount,
            <Runtime as pallet_market::Config>::Fee::get(),
        )
        .expect("well inside the executable LMSR domain");
        let actual = crate::views::quote(MARKET_ID, TradeSide::BuyLong, amount);
        assert_eq!(actual, expected);
        assert!(actual.within_domain);
        assert!(actual.cost > 0);
        assert!(actual.fee > 0);

        let max_trade = pallet_market::core_market::max_trade_amount(B);
        let over_limit = max_trade.saturating_add(1);
        let expected_over = pallet_market::core_market::quote(
            &book,
            TradeSide::BuyLong,
            over_limit,
            <Runtime as pallet_market::Config>::Fee::get(),
        )
        .expect("the numerical domain extends beyond the per-trade bound");
        let actual_over = crate::views::quote(MARKET_ID, TradeSide::BuyLong, over_limit);
        assert_eq!(actual_over, expected_over);
        // 02 §4 makes this flag only the post-trade LMSR domain predicate;
        // 11 §11.5 P-1 binds the FE to detect the separate trade-size row.
        assert!(actual_over.within_domain);
        assert_eq!(actual_over.max_trade, max_trade);
        assert!(over_limit > actual_over.max_trade);

        assert_eq!(
            crate::views::quote(999, TradeSide::BuyLong, amount),
            futarchy_primitives::QuoteView {
                cost: 0,
                fee: 0,
                p_after_1e9: FixedU64(0),
                max_trade: 0,
                within_domain: false,
            }
        );
        assert_eq!(
            crate::views::quote(MARKET_ID, TradeSide::SellLong, amount),
            futarchy_primitives::QuoteView {
                cost: 0,
                fee: 0,
                p_after_1e9: FixedU64(0),
                max_trade,
                within_domain: false,
            }
        );
    });
}

#[test]
fn view_quote_and_buy_share_closed_registered_window_preflight() {
    use frame_support::{traits::ConstU32, BoundedVec};
    use futarchy_primitives::{Branch, FixedU64, ScalarSide, TradeSide};
    use pallet_market::core_market::{BookKind, MarketBook, TwapWindow};

    development_ext().execute_with(|| {
        const MARKET_ID: u64 = 42;
        const B: u128 = 10_000_000_000;
        const WINDOW_END: BlockNumber = 30;
        let book = MarketBook::open(
            MARKET_ID,
            BookKind::Decision {
                proposal: 7,
                branch: Branch::Accept,
            },
            account(31),
            account(32),
            B,
        );
        pallet_market::Markets::<Runtime>::insert(MARKET_ID, book);
        pallet_market::DecisionWindows::<Runtime>::insert(
            MARKET_ID,
            BoundedVec::<_, ConstU32<8>>::truncate_from(vec![TwapWindow {
                start: 10,
                trailing_start: 20,
                end: WINDOW_END,
                observations: 0,
                stale_events: 0,
                contest_capital_blocks: 0,
                contest_accrued_until: WINDOW_END,
                contest_valid: true,
                close_spot: None,
                sealed: false,
            }]),
        );
        System::set_block_number(WINDOW_END.saturating_add(1));

        let max_trade = pallet_market::core_market::max_trade_amount(B);
        assert_eq!(
            crate::views::quote(MARKET_ID, TradeSide::BuyLong, kernel::MIN_TRADE_USDC),
            futarchy_primitives::QuoteView {
                cost: 0,
                fee: 0,
                p_after_1e9: FixedU64(0),
                max_trade,
                within_domain: false,
            }
        );
        assert_noop!(
            Market::buy(
                RuntimeOrigin::signed(account(33)),
                MARKET_ID,
                ScalarSide::Long,
                kernel::MIN_TRADE_USDC,
                Balance::MAX,
            ),
            pallet_market::Error::<Runtime>::NotTrading
        );
    });
}

#[test]
fn view_account_positions_uses_vault_order_and_truncates_protocol_accounts() {
    use pallet_conditional_ledger::core_ledger::VaultInfo;

    development_ext().execute_with(|| {
        let who = crate::configs::insurance_account();
        let who_raw: [u8; 32] = who.clone().into();
        for proposal in (1..=5).rev() {
            pallet_conditional_ledger::Vaults::<Runtime>::insert(proposal, VaultInfo::open(1));
            for (index, position) in
                pallet_conditional_ledger::core_ledger::proposal_positions(proposal)
                    .into_iter()
                    .enumerate()
            {
                pallet_conditional_ledger::Positions::<Runtime>::insert(
                    position,
                    &who,
                    u128::from(proposal) * 100 + index as u128 + 1,
                );
            }
        }

        let positions = crate::views::account_positions(who_raw);
        assert_eq!(positions.len(), 64);
        for (index, view) in positions.iter().enumerate() {
            let proposal = (index / 14 + 1) as u64;
            let instrument = index % 14;
            assert_eq!(
                view.position,
                pallet_conditional_ledger::core_ledger::proposal_positions(proposal)[instrument]
            );
            assert_eq!(
                view.balance,
                u128::from(proposal) * 100 + instrument as u128 + 1
            );
            assert_eq!(view.vault_state, futarchy_primitives::VaultState::Open);
        }
    });
}

#[test]
fn view_account_positions_includes_baseline_instruments_and_terminal_state() {
    use futarchy_primitives::{Branch, FixedU64, ScalarSide, VaultState};
    use pallet_conditional_ledger::core_ledger::{BaselineState, BaselineVaultInfo};

    development_ext().execute_with(|| {
        let who_raw = [78; 32];
        let who = AccountId::new(who_raw);
        let mut baseline = BaselineVaultInfo::open();
        baseline.state = BaselineState::Settled(FixedU64(700_000_000));
        pallet_conditional_ledger::BaselineVaults::<Runtime>::insert(8, baseline);
        for (position, balance) in pallet_conditional_ledger::core_ledger::baseline_positions(8)
            .into_iter()
            .zip([11, 12])
        {
            pallet_conditional_ledger::Positions::<Runtime>::insert(position, &who, balance);
        }

        let positions = crate::views::account_positions(who_raw);
        assert_eq!(positions.len(), 2);
        assert_eq!(
            positions
                .iter()
                .map(|view| view.position)
                .collect::<Vec<_>>(),
            vec![
                futarchy_primitives::PositionId::Baseline {
                    epoch: 8,
                    side: ScalarSide::Long,
                },
                futarchy_primitives::PositionId::Baseline {
                    epoch: 8,
                    side: ScalarSide::Short,
                },
            ]
        );
        assert!(positions.iter().all(|view| view.vault_state
            == VaultState::ScalarSettled {
                winner: Branch::Accept,
                s: FixedU64(700_000_000),
            }));
    });
}

#[test]
fn view_execution_queue_reuses_guard_projection_and_fails_closed() {
    use pallet_execution_guard::pallet::{StoredBlockedMeters, StoredMeters};

    development_ext().execute_with(|| {
        let version = pallet_execution_guard::CurrentSpecName::<Runtime>::get()
            .expect("guard genesis records the active runtime version");
        let meter = [9; 8];
        for pid in (1..=33).rev() {
            pallet_execution_guard::Queue::<Runtime>::insert(
                pid,
                pallet_execution_guard::pallet::StoredQueuedExecution {
                    pid,
                    payload_hash: [pid as u8; 32],
                    payload_len: 1,
                    class: ProposalClass::Param,
                    maturity: 10,
                    grace_end: 20,
                    version_constraint: version.clone(),
                    meters_declared: StoredMeters::try_from(vec![meter])
                        .expect("one declared meter fits"),
                    ratify_ref: None,
                    ratification_passed: false,
                    attestation_id: None,
                    pre_upgrade_checkpoint: None,
                    cancelled: false,
                    declared_domains: Default::default(),
                    failed_at: None,
                },
            );
        }
        pallet_execution_guard::BlockedMeters::<Runtime>::put(
            StoredBlockedMeters::try_from(vec![meter]).expect("one blocked meter fits"),
        );

        let view = crate::views::execution_queue();
        assert_eq!(
            view.iter().map(|entry| entry.pid).collect::<Vec<_>>(),
            (1..=32).collect::<Vec<_>>()
        );
        assert_eq!(view.len(), 32);
        assert!(view.iter().all(|entry| !entry.meters_clear));
        assert!(view.iter().all(|entry| matches!(
            entry.ratification,
            futarchy_primitives::RatificationStatus::NotRequired
        )));

        pallet_execution_guard::CurrentSpecName::<Runtime>::kill();
        assert!(crate::views::execution_queue().is_empty());
    });
}

#[test]
fn unavailable_prize_keeps_the_base_contest_floor_and_never_slashes_the_proposer() {
    // Codex PR #100 P1 regression. B2 unified the grade adapter and the
    // `decision_stats` view onto one contest-floor helper, but made it void the
    // floor when `in_cap_prize` is unavailable. SQ-173 leaves the prize
    // unbacked for EVERY non-TREASURY class, so every PARAM/CODE/META proposal
    // would have graded non-decision-grade, extended once, then landed on
    // `Reject(NotDecisionGrade)` — slashing 10% of the proposer's intake bond
    // for an input the chain, not the proposer, is missing.
    //
    // A missing prize is a security-sizing input gap: `decide` already fails it
    // at the sizing step (`in_cap_prize.ok_or(BadDecisionInput)`), an error that
    // leaves the proposal in place and retryable. Grading must stay meaningful
    // and keep the base `dec.v_min` floor (05 §5.2; 08 §5.3).
    development_ext().execute_with(|| {
        let params =
            <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        let proposal = empty_param_proposal(9_310, account(31), H256::repeat_byte(9), 1);

        // Precondition: this is exactly the SQ-173 state the bug tripped on.
        assert_eq!(
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                AccountId,
            >>::in_cap_prize(&proposal),
            None,
            "SQ-173: a PARAM prize proxy is unbacked — the premise of this regression",
        );

        let param_index = crate::configs::proposal_class_index(ProposalClass::Param);
        assert_eq!(
            crate::configs::effective_decision_contest_floor(&proposal, &params),
            params.v_min[param_index],
            "an unbacked prize must keep the base dec.v_min floor, not void the grade",
        );
        assert_ne!(
            params.v_min[param_index], 0,
            "the base floor must remain a real, enforceable contest requirement",
        );
    });
}

#[test]
fn view_welfare_current_returns_latest_finalized_breached_snapshot() {
    use futarchy_primitives::FixedU64;
    use pallet_welfare::{MetricSpec, Pillar, SourceClass};

    fn spec(version: u16, activation_epoch: u32) -> MetricSpec {
        MetricSpec {
            id: version,
            version,
            pillar: Pillar::S,
            weight: FixedU64(1_000_000_000),
            epsilon_floor: FixedU64(1),
            activation_epoch,
            source: SourceClass::Onchain,
            formula_ref: [1; 32],
            units: [2; 16],
            repr: [3; 16],
            cadence_blocks: 1,
            sanity_min: FixedU64(0),
            sanity_max: FixedU64(1_000_000_000),
            has_normalization_rule: true,
            has_missing_data_rule: true,
            has_gaming_vectors: true,
            has_challenge_procedure: true,
            prior_bounds: [FixedU64(0); pallet_welfare::HISTORY_PRIORS],
        }
    }

    development_ext().execute_with(|| {
        const CURRENT_EPOCH: u32 = 2;
        const LATEST_FINALIZED_EPOCH: u32 = CURRENT_EPOCH.saturating_sub(1);
        pallet_epoch::EpochOf::<Runtime>::mutate(|info| info.index = CURRENT_EPOCH);
        pallet_oracle::ReserveHealth::<Runtime>::mutate(|health| health.unhealthy = true);
        let sentinel = crate::views::welfare_current();
        assert_eq!(sentinel.epoch, CURRENT_EPOCH);
        assert_eq!(sentinel.spec_version, 0);
        assert_eq!(sentinel.w_current_1e9, FixedU64(0));
        assert!(sentinel.reserve_flag);

        pallet_welfare::MetricSpecs::<Runtime>::insert(
            2,
            pallet_welfare::pallet::BoundedSpecSet::try_from(vec![spec(2, 0)])
                .expect("one metric spec fits"),
        );
        pallet_welfare::MetricSpecs::<Runtime>::insert(
            3,
            pallet_welfare::pallet::BoundedSpecSet::try_from(vec![spec(3, 3)])
                .expect("one future metric spec fits"),
        );
        // Production can only record closed epochs (05 §4.6). Keep an older
        // snapshot to prove the view deterministically selects the greatest
        // finalized epoch for the canonical active spec.
        pallet_welfare::Snapshots::<Runtime>::insert(
            (0, 2),
            pallet_welfare::pallet::StoredSnapshot {
                epoch: 0,
                spec_version: 2,
                s_pillar: FixedU64(1),
                c_onchain: FixedU64(2),
                c_attested: FixedU64(3),
                p_pillar: FixedU64(4),
                a_pillar: FixedU64(5),
                gate_s: FixedU64(6),
                gate_c: FixedU64(7),
                welfare: FixedU64(8),
                components: Default::default(),
            },
        );
        pallet_welfare::Snapshots::<Runtime>::insert(
            (LATEST_FINALIZED_EPOCH, 2),
            pallet_welfare::pallet::StoredSnapshot {
                epoch: LATEST_FINALIZED_EPOCH,
                spec_version: 2,
                s_pillar: FixedU64(101),
                c_onchain: FixedU64(102),
                c_attested: FixedU64(103),
                p_pillar: FixedU64(104),
                a_pillar: FixedU64(105),
                gate_s: FixedU64(106),
                gate_c: FixedU64(107),
                welfare: FixedU64(108),
                components: Default::default(),
            },
        );
        pallet_welfare::GateBreachFlags::<Runtime>::insert(
            LATEST_FINALIZED_EPOCH,
            pallet_welfare::CoreGateBreachFlags {
                s_breached: true,
                c_breached: true,
                day_bitmap: [1, 1],
            },
        );
        assert!(!pallet_welfare::Snapshots::<Runtime>::contains_key((
            CURRENT_EPOCH,
            2
        )));

        let view = crate::views::welfare_current();
        assert_eq!(view.epoch, LATEST_FINALIZED_EPOCH);
        assert_eq!(view.spec_version, 2);
        assert_eq!(view.s_pillar_1e9, FixedU64(101));
        assert_eq!(view.c_onchain_1e9, FixedU64(102));
        assert_eq!(view.c_attested_1e9, FixedU64(103));
        assert_eq!(view.p_pillar_1e9, FixedU64(104));
        assert_eq!(view.a_pillar_1e9, FixedU64(105));
        assert_eq!(view.gate_s_1e9, FixedU64(106));
        assert_eq!(view.gate_c_1e9, FixedU64(107));
        assert_eq!(view.w_current_1e9, FixedU64(108));
        assert!(view.s_breached);
        assert!(view.c_breached);
        assert!(view.reserve_flag);

        pallet_welfare::MetricSpecs::<Runtime>::insert(
            4,
            pallet_welfare::pallet::BoundedSpecSet::try_from(vec![spec(4, 0)])
                .expect("one tied metric spec fits"),
        );
        assert_eq!(
            <crate::configs::RuntimeConstitutionAccess as pallet_epoch::ConstitutionAccess<
                AccountId,
            >>::active_metric_spec_version(),
            None,
            "05 §4.6 / I-16 qualification must fail closed on the latest activation tie"
        );
        // 02 §3 and 05 §4.6 require the runtime view to use that same
        // canonical selector. Until the open encoding question is resolved,
        // sentinel spec_version 0 means "no active spec".
        let ambiguous = crate::views::welfare_current();
        assert_eq!(ambiguous.spec_version, 0);
        assert_eq!(ambiguous.w_current_1e9, FixedU64(0));
        assert_eq!(ambiguous.s_pillar_1e9, FixedU64(0));
        assert!(!ambiguous.s_breached);
        assert!(!ambiguous.c_breached);
        assert!(ambiguous.reserve_flag);
    });
}

#[test]
fn view_params_converts_live_records_in_request_order() {
    use pallet_constitution::{key16, MaxDelta, ParamValue};

    development_ext().execute_with(|| {
        let keeper_key = key16(b"keeper.budget");
        pallet_constitution::Params::<Runtime>::mutate(keeper_key, |record| {
            let record = record
                .as_mut()
                .expect("keeper budget is a genesis parameter");
            record.value = ParamValue::Balance(5);
            record.min = ParamValue::Balance(0);
            record.max = ParamValue::Balance(u128::MAX);
            record.max_delta = Some(MaxDelta::Factor(2));
            record.last_change_block = 99;
        });
        let keys = futarchy_primitives::BoundedVec::try_from(vec![
            key16(b"epoch.length"),
            keeper_key,
            key16(b"epoch.slots"),
            key16(b"iss.inflation"),
            key16(b"pol.b.param"),
            key16(b"epoch.horizon_k"),
            key16(b"att.bond"),
            key16(b"unknown"),
            keeper_key,
        ])
        .expect("fixture stays below the request bound");
        let view = crate::views::params(keys);
        let rows = view.as_slice();

        assert_eq!(view.len(), 8);
        assert_eq!(
            view.iter().map(|row| row.key).collect::<Vec<_>>(),
            vec![
                key16(b"epoch.length"),
                keeper_key,
                key16(b"epoch.slots"),
                key16(b"iss.inflation"),
                key16(b"pol.b.param"),
                key16(b"epoch.horizon_k"),
                key16(b"att.bond"),
                keeper_key,
            ]
        );
        assert_eq!(rows[0].max_delta, 30_240);
        assert_eq!(rows[0].cooldown_blocks, 604_800);
        assert_eq!(rows[0].class, ProposalClass::Meta);
        assert_eq!(rows[1].value, 5);
        assert_eq!(rows[1].max_delta, 2);
        assert_eq!(rows[1].cooldown_blocks, 302_400);
        assert_eq!(rows[1].last_change, 99);
        assert_eq!(rows[1].class, ProposalClass::Param);
        assert_eq!(rows[2].max_delta, 2);
        assert_eq!(rows[2].class, ProposalClass::Meta);
        assert_eq!(rows[3].max_delta, 0);
        assert_eq!(rows[3].cooldown_blocks, 0);
        assert_eq!(rows[3].class, ProposalClass::Constitutional);
        assert_eq!(rows[4].class, ProposalClass::Treasury);
        assert_eq!(rows[5].class, ProposalClass::Meta);
        assert_eq!(rows[6].class, ProposalClass::Constitutional);
        assert_eq!(rows[7], rows[1]);

        pallet_constitution::Params::<Runtime>::remove(key16(b"epoch.length"));
        let one = futarchy_primitives::BoundedVec::try_from(vec![keeper_key])
            .expect("one requested key fits");
        assert_eq!(
            crate::views::params(one).as_slice()[0].cooldown_blocks,
            u32::MAX
        );

        pallet_constitution::Params::<Runtime>::mutate(keeper_key, |record| {
            record
                .as_mut()
                .expect("keeper budget remains present")
                .max_delta = Some(MaxDelta::Factor(0));
        });
        let malformed = futarchy_primitives::BoundedVec::try_from(vec![keeper_key])
            .expect("one requested key fits");
        assert!(crate::views::params(malformed).is_empty());
    });
}

#[test]
fn view_params_projects_factor_delta_conservatively() {
    use pallet_constitution::{key16, MaxDelta};

    development_ext().execute_with(|| {
        // 02 §4 exposes one max_delta scalar for 13 §1's asymmetric
        // exec.lock.* factor rule. Under R-7 it must be no larger than either
        // admitted direction; which side the scalar denotes is still an open
        // contract question, so derive the expectation from the live record.
        let key = key16(b"exec.lock.code");
        let record = pallet_constitution::Params::<Runtime>::get(key)
            .expect("the canonical exec.lock.code record exists");
        let value = record.value.as_u128();
        assert!(matches!(record.max_delta, Some(MaxDelta::Factor(_))));
        let factor = match record.max_delta {
            Some(MaxDelta::Factor(factor)) => u128::from(factor),
            _ => 1,
        };
        assert!(factor >= 1);
        let lower = value / factor + u128::from(value % factor != 0);
        let downward = value.saturating_sub(lower);
        let upward = value.saturating_mul(factor).saturating_sub(value);
        let keys =
            futarchy_primitives::BoundedVec::try_from(vec![key]).expect("one requested key fits");
        let view = crate::views::params(keys);

        assert_eq!(view.len(), 1);
        assert_eq!(view.as_slice()[0].value, value);
        assert_eq!(view.as_slice()[0].max_delta, downward.min(upward));
        assert_eq!(view.as_slice()[0].max_delta, downward);
        assert!(view.as_slice()[0].max_delta < upward);
    });
}

#[test]
fn view_nav_maps_every_contract_field_from_hand_built_state() {
    use pallet_futarchy_treasury::{BudgetLine, Stream};

    development_ext().execute_with(|| {
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = 1_000;
            state.reserve_impaired = true;
            state.lines = frame_support::BoundedVec::truncate_from(vec![
                (BudgetLine::Pol, 10),
                (BudgetLine::PolBaseline, 20),
                (BudgetLine::Keeper, 30),
                (BudgetLine::Oracle, 40),
                (BudgetLine::Rewards, 50),
                (BudgetLine::OpsBootnodes, 60),
            ]);
            state.streams = frame_support::BoundedVec::truncate_from(vec![
                Stream {
                    id: 1,
                    recipient: [1; 32],
                    line: BudgetLine::Rewards,
                    total: 100,
                    claimed: 25,
                    start: 1,
                    duration: 10,
                    cancelled: false,
                },
                Stream {
                    id: 2,
                    recipient: [2; 32],
                    line: BudgetLine::Rewards,
                    total: 70,
                    claimed: 10,
                    start: 1,
                    duration: 10,
                    cancelled: true,
                },
            ]);
            state.pending_outflows = frame_support::BoundedVec::truncate_from(vec![7, 8]);
            state.pol_commitments = frame_support::BoundedVec::truncate_from(vec![9]);
        });
        assert_ok!(<ForeignAssets as FungiblesMutate<AccountId>>::mint_into(
            usdc_location(),
            &crate::configs::insurance_account(),
            55_000_000,
        ));

        let view = crate::views::nav();
        // Assets = main 1,000 + all lines 210 + stream escrow 75;
        // obligations = stream 75 + pending 15 + POL commitment 9.
        assert_eq!(view.total, 1_186);
        assert_eq!(view.main, 1_000);
        assert_eq!(view.pol, 30);
        assert_eq!(view.insurance, 55_000_000);
        assert_eq!(view.keeper, 30);
        assert_eq!(view.oracle, 40);
        assert_eq!(view.rewards, 50);
        assert_eq!(view.stream_remainders, 75);
        assert_eq!(view.obligations, 99);
        assert!(view.haircut_flag);
        assert_eq!(view.spendable_nav, 0);
        assert_eq!(view.meter_utilization_bps, 0);
        assert_eq!(
            view.class_floors,
            [
                FutarchyTreasury::floor(ProposalClass::Param),
                FutarchyTreasury::floor(ProposalClass::Treasury),
                FutarchyTreasury::floor(ProposalClass::Code),
                FutarchyTreasury::floor(ProposalClass::Meta),
            ]
        );
    });
}

#[test]
fn view_open_oracle_rounds_sorts_triple_keys_and_marks_prior_escalation() {
    use futarchy_primitives::FixedU64;

    fn round(
        component: u16,
        epoch: u32,
        version: u16,
        round: u8,
        challenger: Option<[u8; 32]>,
    ) -> pallet_oracle::RoundState {
        pallet_oracle::RoundState {
            component,
            epoch,
            round,
            spec_version: version,
            reporter: [component as u8; 32],
            value: FixedU64(u64::from(component) * 100),
            evidence_hash: [version as u8; 32],
            bond: 1_000 + u128::from(component),
            challenge_deadline: 50 + u32::from(component),
            extended: false,
            challenger,
            counter_value: challenger.map(|_| FixedU64(7)),
            acks: round,
            report_hash: [round; 32],
            stake_at_risk: 10,
            cumulative_reporter_bond: 11,
            cumulative_challenger_bond: 12,
        }
    }

    development_ext().execute_with(|| {
        for state in [
            round(3, 2, 1, 2, None),
            round(1, 9, 2, 1, Some([8; 32])),
            round(1, 8, 3, 1, None),
        ] {
            pallet_oracle::Rounds::<Runtime>::insert(
                (state.component, state.epoch, state.spec_version),
                state,
            );
            pallet_oracle::RoundSchedules::<Runtime>::insert(
                (state.component, state.epoch, state.spec_version),
                pallet_oracle::StoredRoundSchedule {
                    round_one_bond: 1_000 + u128::from(state.component),
                    round_cap: pallet_oracle::ORC_ROUNDS,
                },
            );
        }
        let view = crate::views::open_oracle_rounds();
        let rows = view.as_slice();
        assert_eq!(
            view.iter()
                .map(|entry| (entry.component, entry.epoch, entry.spec_version))
                .collect::<Vec<_>>(),
            vec![(1, 8, 3), (1, 9, 2), (3, 2, 1)]
        );
        assert!(!rows[0].escalated);
        // A live challenger in round one is not an escalation yet.
        assert!(!rows[1].escalated);
        // Round two exists only because the prior round escalated under 07 §5.
        assert!(rows[2].escalated);
        assert_eq!(rows[2].value_1e9, FixedU64(300));
        assert_eq!(rows[2].acked_by_watchtowers, 2);
        assert_eq!(rows[2].evidence_hash, [1; 32]);
    });
}

#[test]
fn view_epoch_status_uses_loaded_clock_and_live_b1b_providers() {
    development_ext().execute_with(|| {
        assert_eq!(
            crate::views::epoch_status(),
            Epoch::epoch_state().status_view()
        );

        assert_ok!(Constitution::note_dead_man_engaged(true));
        assert_ok!(Constitution::note_ledger_frozen(true));
        let armed = crate::views::epoch_status();
        assert!(armed.dead_man_armed);
        assert!(armed.ledger_frozen);
        assert_eq!(armed.phase_flags, Constitution::phase_flags());
        assert_ne!(
            armed.phase_flags & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED,
            0
        );
        assert_ne!(
            armed.phase_flags & pallet_constitution::PhaseFlagsValue::LEDGER_FROZEN,
            0
        );

        assert_ok!(Constitution::note_dead_man_engaged(false));
        assert_ok!(Constitution::note_ledger_frozen(false));
        System::set_block_number(current_qualify_block());
        assert_ok!(Epoch::tick(
            RuntimeOrigin::signed(account(69)),
            Default::default()
        ));
        let advanced = crate::views::epoch_status();
        assert_eq!(advanced, Epoch::epoch_state().status_view());
        assert_eq!(advanced.phase, futarchy_primitives::EpochPhase::Qualify);
        assert_eq!(advanced.phase_start_block, current_qualify_block());
        assert!(advanced.next_boundary > advanced.phase_start_block);
        assert!(!advanced.dead_man_armed);
        assert!(!advanced.ledger_frozen);
    });
}

#[test]
fn view_proposal_summaries_sorts_and_joins_passed_ratification() {
    development_ext().execute_with(|| {
        assert!(crate::views::proposal_summaries().is_empty());
        let version = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => {
                assert!(false, "guard genesis must bind a runtime version");
                return;
            }
        };
        for (pid, class) in [
            (4, ProposalClass::Code),
            (3, ProposalClass::Treasury),
            (2, ProposalClass::Param),
            (1, ProposalClass::Code),
        ] {
            assert_ok!(seed_queued_epoch_proposal(
                pid,
                class,
                H256::repeat_byte(pid as u8),
                1,
                50 + pid as u32,
                80 + pid as u32,
                version.clone(),
            ));
        }
        let ratified = match pallet_epoch::Proposals::<Runtime>::get(1) {
            Some(proposal) => proposal,
            None => {
                assert!(false, "seeded CODE proposal must exist");
                return;
            }
        };
        assert_ok!(ExecutionGuard::ratify(
            pallet_origins::Origin::ConstitutionalValues.into(),
            1,
            77,
        ));
        pallet_epoch::Proposals::<Runtime>::mutate(2, |proposal| {
            if let Some(proposal) = proposal {
                proposal.markets = None;
                proposal.maturity = None;
            }
        });

        let view = crate::views::proposal_summaries();
        assert_eq!(
            view.iter().map(|proposal| proposal.id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        let code = &view.as_slice()[0];
        assert_eq!(code.class, ProposalClass::Code);
        assert_eq!(code.state, ProposalState::Queued);
        assert_eq!(code.proposer, [70; 32]);
        assert_eq!(code.epoch, ratified.epoch);
        assert_eq!(code.payload_hash, ratified.payload_hash);
        assert_eq!(code.ask, ratified.ask);
        assert_eq!(code.decision_market, Some((11, 12)));
        assert_eq!(code.gate_markets, Some([13, 14, 15, 16]));
        assert_eq!(code.decide_at, ratified.decide_at);
        assert_eq!(code.maturity, ratified.maturity);
        assert_eq!(
            code.ratification,
            futarchy_primitives::RatificationStatus::Passed { referendum: 77 }
        );
        assert_eq!(view.as_slice()[1].decision_market, None);
        assert_eq!(view.as_slice()[1].gate_markets, None);
        assert_eq!(view.as_slice()[1].maturity, None);
        // Ratification is class-discriminated, never a blanket `NotRequired`:
        // PARAM/TREASURY need no values referendum (06 §2.2), but the seeded
        // CODE proposal at id 4 has no `Ratifications` record, so it carries
        // the guard's fail-closed spelling — the same value `execution_queue`
        // reports for it.
        assert_eq!(
            view.as_slice()[1].ratification,
            RatificationStatus::NotRequired
        );
        assert_eq!(
            view.as_slice()[2].ratification,
            RatificationStatus::NotRequired
        );
        assert_eq!(view.as_slice()[3].class, ProposalClass::Code);
        assert_eq!(
            view.as_slice()[3].ratification,
            RatificationStatus::Failed { referendum: 0 }
        );
    });
}

#[test]
fn view_decision_stats_pins_effective_floor_pair_minima_gates_and_convergence() {
    development_ext().execute_with(|| {
        const PID: futarchy_primitives::ProposalId = 8_090;
        let params =
            <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        let index = crate::configs::proposal_class_index(ProposalClass::Treasury);
        let prize = params.v_min[index];
        let effective_floor = prize.saturating_mul(2);
        let end = params.decision_window;
        let epoch = pallet_epoch::EpochOf::<Runtime>::get()
            .index
            .saturating_add(1);
        let carried_baseline = futarchy_primitives::FixedU64(620_000_000);
        let markets = MarketSet {
            accept: 89_001,
            reject: 89_002,
            gates: Some([89_003, 89_004, 89_005, 89_006]),
            baseline: 89_007,
        };
        let gates = match markets.gates {
            Some(gates) => gates,
            None => {
                assert!(false, "Treasury fixture must carry gate books");
                return;
            }
        };
        // Use depth above the POL floor so the ×7 ceiling does not mask the
        // asymmetric-book MIN regression below.
        let decision_b = crate::configs::class_pol_floor(ProposalClass::Treasury).saturating_mul(4);
        let gate_b = crate::configs::balance_param(b"pol.b_gate");
        let baseline_b = crate::configs::balance_param(b"pol.b_baseline");
        let gate_contest = params.gate_v_min[index];
        let gate_quotes = [
            futarchy_primitives::FixedU64(410_000_000),
            futarchy_primitives::FixedU64(420_000_000),
            futarchy_primitives::FixedU64(430_000_000),
            futarchy_primitives::FixedU64(440_000_000),
        ];
        for result in [
            seed_decision_grade_market(
                markets.accept,
                pallet_market::core_market::BookKind::Decision {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Accept,
                },
                futarchy_primitives::FixedU64(700_000_000),
                end,
                (params.decision_window, params.trailing_window),
                decision_b,
                effective_floor,
            ),
            seed_decision_grade_market(
                markets.reject,
                pallet_market::core_market::BookKind::Decision {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Reject,
                },
                futarchy_primitives::FixedU64(400_000_000),
                end,
                (params.decision_window, params.trailing_window),
                decision_b,
                effective_floor,
            ),
            seed_decision_grade_market(
                gates[0],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Accept,
                    gate: futarchy_primitives::GateType::Survival,
                },
                gate_quotes[0],
                end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                gates[1],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Reject,
                    gate: futarchy_primitives::GateType::Survival,
                },
                gate_quotes[1],
                end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                gates[2],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Accept,
                    gate: futarchy_primitives::GateType::Security,
                },
                gate_quotes[2],
                end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                gates[3],
                pallet_market::core_market::BookKind::Gate {
                    proposal: PID,
                    branch: futarchy_primitives::Branch::Reject,
                    gate: futarchy_primitives::GateType::Security,
                },
                gate_quotes[3],
                end,
                (params.decision_window, params.trailing_window),
                gate_b,
                gate_contest,
            ),
            seed_decision_grade_market(
                markets.baseline,
                pallet_market::core_market::BookKind::Baseline { epoch },
                futarchy_primitives::FixedU64(650_000_000),
                end,
                (params.decision_window, params.trailing_window),
                baseline_b,
                effective_floor,
            ),
        ] {
            assert_ok!(result);
        }
        // Leave the live Baseline unregistered so the shared decision helper
        // must use the previous settled cohort's 05 §5.3 carry value.
        pallet_epoch::RecentCohortSummaries::<Runtime>::mutate(|recent| {
            assert!(recent
                .try_push(futarchy_primitives::CohortSummary {
                    epoch: epoch.saturating_sub(1),
                    s_1e9: futarchy_primitives::FixedU64(0),
                    baseline_twap_1e9: carried_baseline,
                    proposals: futarchy_primitives::BoundedVec::new(),
                    voided: false,
                    settled_at: 0,
                })
                .is_ok());
        });

        let spend = RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::spend {
            line: pallet_futarchy_treasury::BudgetLine::Pol,
            dest: account(149),
            amount: prize,
        });
        let (payload_hash, payload_len) = match note_runtime_batch(vec![spend]) {
            Some(payload) => payload,
            None => {
                assert!(false, "bounded Treasury payload must be noted");
                return;
            }
        };
        <Preimage as QueryPreimage>::request(&payload_hash);
        let version_constraint = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => {
                assert!(false, "guard genesis must bind a runtime version");
                return;
            }
        };
        // Every Treasury proposal requires the 05 §5.1 gate quartet.
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.main_usdc = prize.saturating_mul(25);
        });
        let proposal = Proposal {
            id: PID,
            proposer: account(70),
            class: ProposalClass::Treasury,
            state: ProposalState::Trading,
            epoch,
            submitted_at: 0,
            payload_hash: payload_hash.0,
            payload_len,
            ask: prize,
            bond: Balance::MAX,
            resources: Default::default(),
            metric_spec: 1,
            decide_at: end,
            rerun: false,
            extended: false,
            delayed_once: false,
            markets: Some(markets),
            maturity: None,
            grace_end: None,
            version_constraint: Some(version_constraint),
            decision: None,
        };
        pallet_epoch::Proposals::<Runtime>::insert(PID, proposal);
        pallet_epoch::NextProposalId::<Runtime>::mutate(|next| {
            *next = (*next).max(PID.saturating_add(1));
        });

        let interval = u32::try_from(crate::configs::MarketObsInterval::get()).unwrap_or_default();
        assert_ne!(interval, 0);
        let expected_observations = params.decision_window / interval;
        let reject_observations = expected_observations.saturating_sub(1);
        let accept_volume = effective_floor.saturating_add(prize);
        let reject_volume = effective_floor;
        let tune_window = |market: futarchy_primitives::MarketId,
                           observations: u32,
                           volume: Balance,
                           close_spot: futarchy_primitives::FixedU64| {
            pallet_market::DecisionWindows::<Runtime>::mutate(market, |windows| {
                if let Some(record) = windows.iter_mut().find(|record| record.end == end) {
                    record.observations = observations;
                    record.contest_capital_blocks =
                        volume.saturating_mul(Balance::from(params.decision_window));
                    record.close_spot = Some(close_spot);
                    true
                } else {
                    false
                }
            })
        };
        assert!(tune_window(
            markets.accept,
            expected_observations,
            accept_volume,
            futarchy_primitives::FixedU64(900_000_000),
        ));
        assert!(tune_window(
            markets.reject,
            reject_observations,
            reject_volume,
            futarchy_primitives::FixedU64(400_000_000),
        ));

        let stats = match crate::views::decision_stats(PID) {
            Some(stats) => stats,
            None => {
                assert!(false, "complete in-cap Treasury statistics must be exposed");
                return;
            }
        };
        let snapshot = match Epoch::decision_input_snapshot(PID) {
            Some(snapshot) => snapshot,
            None => {
                assert!(false, "complete decision snapshot must be readable");
                return;
            }
        };
        let pol_per_book = match pallet_market::core_market::maker_loss_floor(decision_b) {
            Some(depth) => depth,
            None => {
                assert!(false, "bounded POL depth must compute");
                return;
            }
        };
        let pol_depth = match pol_per_book.checked_mul(2) {
            Some(depth) => depth,
            None => {
                assert!(false, "pair POL depth must fit");
                return;
            }
        };
        let b_sum = match decision_b.checked_mul(2) {
            Some(sum) => sum,
            None => {
                assert!(false, "pair b must fit");
                return;
            }
        };
        let expected_min_depth = match pallet_market::core_market::liquidity_hat(
            pol_depth,
            reject_volume,
            crate::configs::sec_flow_cap_1e9(),
            b_sum,
        ) {
            Some(depth) => depth,
            None => {
                assert!(false, "MIN-composed depth must fit");
                return;
            }
        };
        let summed_contest = match accept_volume.checked_add(reject_volume) {
            Some(sum) => sum,
            None => {
                assert!(false, "summed mutant contest must fit");
                return;
            }
        };
        let sum_mutant_depth = match pallet_market::core_market::liquidity_hat(
            pol_depth,
            summed_contest,
            crate::configs::sec_flow_cap_1e9(),
            b_sum,
        ) {
            Some(depth) => depth,
            None => {
                assert!(false, "SUM-mutant depth must fit");
                return;
            }
        };
        assert_eq!(snapshot.inputs.measured_depth, expected_min_depth);
        assert_ne!(expected_min_depth, sum_mutant_depth);
        assert_eq!(stats.gate_twaps_1e9, Some(gate_quotes));
        assert_eq!(stats.twap_baseline_1e9, carried_baseline);
        assert_ne!(stats.twap_baseline_1e9, snapshot.inputs.baseline_full);
        assert_eq!(stats.traded_volume, reject_volume);
        assert_eq!(stats.v_min_required, effective_floor);
        assert_eq!(stats.in_cap_prize, prize);
        let expected_coverage =
            u8::try_from(reject_observations.saturating_mul(100) / expected_observations)
                .unwrap_or_default();
        assert_eq!(stats.coverage_pct, expected_coverage);
        assert!(!stats.converged);
        assert_eq!(
            stats.r_eff_1e9.0,
            pallet_epoch::effective_reject_1e9(
                snapshot.inputs.reject_full,
                carried_baseline,
                snapshot.params.class_sigma(ProposalClass::Treasury),
            )
        );
        assert!(stats.r_eff_1e9.0 > stats.twap_reject_1e9.0);
    });
}

#[test]
fn view_decision_stats_returns_none_for_unknown_or_incomplete_backing() {
    development_ext().execute_with(|| {
        assert_eq!(crate::views::decision_stats(999_999), None);
        let version = match pallet_execution_guard::CurrentSpecName::<Runtime>::get() {
            Some(version) => version,
            None => {
                assert!(false, "guard genesis must bind a runtime version");
                return;
            }
        };
        assert_ok!(seed_queued_epoch_proposal(
            91,
            ProposalClass::Treasury,
            H256::repeat_byte(91),
            1,
            10,
            20,
            version.clone(),
        ));
        // Market ids exist in the Proposal but no exact registered books,
        // windows, spots, or measured depth do: never fabricate a view.
        assert_eq!(crate::views::decision_stats(91), None);
        pallet_epoch::Proposals::<Runtime>::mutate(91, |proposal| {
            if let Some(proposal) = proposal {
                proposal.markets = None;
            }
        });
        assert_eq!(crate::views::decision_stats(91), None);
        pallet_epoch::Proposals::<Runtime>::remove(91);
        pallet_epoch::ProposalSchedules::<Runtime>::remove(91);
        pallet_conditional_ledger::Vaults::<Runtime>::remove(91);

        // Isolate the values/prize seam: every decision and gate book read is
        // complete, but SQ-141 leaves CODE InCapPrize unavailable. G-1 returns
        // None instead of exposing an otherwise plausible partial statistic.
        let params =
            <crate::configs::RuntimeEpochParams as pallet_epoch::EpochParamsProvider>::get();
        System::set_block_number(params.decision_window);
        assert_ok!(seed_queued_epoch_proposal(
            92,
            ProposalClass::Code,
            H256::repeat_byte(92),
            1,
            params.decision_window.saturating_add(10),
            params.decision_window.saturating_add(20),
            version,
        ));
        assert_ok!(seed_code_decision_markets(
            92,
            params.decision_window,
            futarchy_primitives::FixedU64(700_000_000),
            futarchy_primitives::FixedU64(500_000_000),
        ));
        let snapshot = match Epoch::decision_input_snapshot(92) {
            Some(snapshot) => snapshot,
            None => {
                assert!(false, "fully seeded CODE snapshot must be assembled");
                return;
            }
        };
        assert!(snapshot.inputs.measured_depth > 0);
        assert!(snapshot.inputs.gate_twaps.is_some());
        assert_eq!(snapshot.inputs.in_cap_prize, None);
        assert!(!snapshot.backing_complete);
        assert_eq!(crate::views::decision_stats(92), None);
    });
}

#[test]
fn futarchy_api_trait_delegates_all_eleven_runtime_views() {
    use futarchy_runtime_api::runtime_decl_for_futarchy_api::FutarchyApi as RuntimeFutarchyApi;

    development_ext().execute_with(|| {
        type ApiRuntime = Runtime;
        let side = futarchy_primitives::TradeSide::BuyLong;
        let account = [0; 32];
        let keys = futarchy_primitives::BoundedVec::new();

        assert_eq!(
            <ApiRuntime as RuntimeFutarchyApi<crate::Block>>::epoch_status(),
            crate::views::epoch_status()
        );
        assert_eq!(
            <ApiRuntime as RuntimeFutarchyApi<crate::Block>>::proposal_summaries(),
            crate::views::proposal_summaries()
        );
        assert_eq!(
            <ApiRuntime as RuntimeFutarchyApi<crate::Block>>::quote(0, side, 1),
            crate::views::quote(0, side, 1)
        );
        assert_eq!(
            <ApiRuntime as RuntimeFutarchyApi<crate::Block>>::decision_stats(0),
            crate::views::decision_stats(0)
        );
        assert_eq!(
            <ApiRuntime as RuntimeFutarchyApi<crate::Block>>::account_positions(account),
            crate::views::account_positions(account)
        );
        assert_eq!(
            <ApiRuntime as RuntimeFutarchyApi<crate::Block>>::execution_queue(),
            crate::views::execution_queue()
        );
        assert_eq!(
            <ApiRuntime as RuntimeFutarchyApi<crate::Block>>::welfare_current(),
            crate::views::welfare_current()
        );
        assert_eq!(
            <ApiRuntime as RuntimeFutarchyApi<crate::Block>>::params(keys.clone()),
            crate::views::params(keys)
        );
        assert_eq!(
            <ApiRuntime as RuntimeFutarchyApi<crate::Block>>::nav(),
            crate::views::nav()
        );
        assert_eq!(
            <ApiRuntime as RuntimeFutarchyApi<crate::Block>>::recent_cohorts(),
            crate::views::recent_cohorts()
        );
        assert_eq!(
            <ApiRuntime as RuntimeFutarchyApi<crate::Block>>::open_oracle_rounds(),
            crate::views::open_oracle_rounds()
        );
    });
}

#[test]
fn guardian_playbook_routines_construct_exact_emergency_call_sets() {
    development_ext().execute_with(|| {
        use pallet_guardian::PlaybookId;

        System::set_block_number(10);
        let cases = [
            (PlaybookId::Depeg, None, vec!["Market.freeze_creation"]),
            (PlaybookId::OracleVoid, Some(7), vec!["Epoch.void_cohort"]),
            (
                PlaybookId::HaltIntake,
                None,
                vec!["Epoch.set_intake_paused"],
            ),
            (
                PlaybookId::Reserve,
                None,
                vec!["ConditionalLedger.set_split_paused"],
            ),
            (
                PlaybookId::LedgerFreeze,
                None,
                vec!["ConditionalLedger.set_frozen", "Market.set_frozen"],
            ),
        ];
        for (id, target, expected) in cases {
            let calls = crate::configs::RuntimeGuardianEffects::playbook_calls(id, 20, target)
                .unwrap_or_default();
            let names = calls
                .iter()
                .filter_map(|call| match call {
                    RuntimeCall::Market(pallet_market::Call::freeze_creation { .. }) => {
                        Some("Market.freeze_creation")
                    }
                    RuntimeCall::Market(pallet_market::Call::set_frozen { .. }) => {
                        Some("Market.set_frozen")
                    }
                    RuntimeCall::Epoch(pallet_epoch::Call::void_cohort { .. }) => {
                        Some("Epoch.void_cohort")
                    }
                    RuntimeCall::Epoch(pallet_epoch::Call::set_intake_paused { .. }) => {
                        Some("Epoch.set_intake_paused")
                    }
                    RuntimeCall::ConditionalLedger(
                        pallet_conditional_ledger::Call::set_split_paused { .. },
                    ) => Some("ConditionalLedger.set_split_paused"),
                    RuntimeCall::ConditionalLedger(
                        pallet_conditional_ledger::Call::set_frozen { .. },
                    ) => Some("ConditionalLedger.set_frozen"),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                names.len(),
                calls.len(),
                "unexpected call in {id:?} routine"
            );
            assert_eq!(names, expected, "06 §6.2 call-set drift for {id:?}");
            for call in calls {
                assert!(RuntimeBaseCallFilter::contains_for(
                    ClassOrigin::EmergencyPlaybook,
                    &call,
                ));
                for wrong in [
                    ClassOrigin::FutarchyParam,
                    ClassOrigin::ConstitutionalValues,
                    ClassOrigin::GuardianHold,
                ] {
                    assert!(!RuntimeBaseCallFilter::contains_for(wrong, &call));
                }
            }
        }
        assert!(crate::configs::RuntimeGuardianEffects::playbook_calls(
            PlaybookId::Migration,
            20,
            None,
        )
        .is_err());
        assert!(crate::configs::RuntimeGuardianEffects::playbook_calls(
            PlaybookId::OracleVoid,
            20,
            None,
        )
        .is_err());
        assert!(crate::configs::RuntimeGuardianEffects::playbook_calls(
            PlaybookId::Depeg,
            20,
            Some(7),
        )
        .is_err());
    });
}

#[test]
fn emergency_endpoints_accept_only_the_production_playbook_origin() {
    development_ext().execute_with(|| {
        System::set_block_number(10);
        let emergency = || pallet_origins::Origin::EmergencyPlaybook.into();
        assert_ok!(Epoch::set_intake_paused(emergency(), true, 20));
        assert_ok!(Epoch::set_intake_paused(emergency(), false, 0));
        assert_ok!(Market::freeze_creation(emergency(), 20));
        assert_ok!(Market::set_frozen(emergency(), true));
        assert_ok!(Market::set_frozen(emergency(), false));
        assert_ok!(ConditionalLedger::set_split_paused(emergency(), true, 20,));
        assert_ok!(ConditionalLedger::set_split_paused(emergency(), false, 0,));
        assert_ok!(ConditionalLedger::set_frozen(emergency(), true));
        assert_ok!(ConditionalLedger::set_frozen(emergency(), false));

        for origin in [
            RuntimeOrigin::signed(account(250)),
            pallet_origins::Origin::ConstitutionalValues.into(),
            pallet_origins::Origin::GuardianHold.into(),
        ] {
            assert_noop!(
                Epoch::set_intake_paused(origin.clone(), false, 0),
                DispatchError::BadOrigin
            );
            assert_noop!(
                Market::freeze_creation(origin.clone(), 20),
                DispatchError::BadOrigin
            );
            assert_noop!(
                Market::set_frozen(origin.clone(), false),
                DispatchError::BadOrigin
            );
            assert_noop!(
                ConditionalLedger::set_split_paused(origin.clone(), false, 0),
                DispatchError::BadOrigin
            );
            assert_noop!(
                ConditionalLedger::set_frozen(origin, false),
                DispatchError::BadOrigin
            );
        }
    });
}

#[test]
fn gate_suspension_requires_a_live_breach_and_auto_releases() {
    use pallet_execution_guard::GuardianState;
    use pallet_guardian::GuardianEffectDispatcher;

    development_ext().execute_with(|| {
        System::set_block_number(10);
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        assert!(crate::configs::RuntimeGuardianEffects::dispatch(
            pallet_guardian::GuardianPower::SuspendOnGate,
            H256::zero().into(),
        )
        .is_err());
        pallet_welfare::GateBreachFlags::<Runtime>::insert(
            epoch,
            pallet_welfare::CoreGateBreachFlags {
                s_breached: true,
                c_breached: false,
                day_bitmap: [1, 0],
            },
        );
        assert_ok!(crate::configs::RuntimeGuardianEffects::dispatch(
            pallet_guardian::GuardianPower::SuspendOnGate,
            H256::zero().into(),
        ));
        assert!(crate::configs::RuntimeGuardianState::gate_suspended());

        pallet_welfare::GateBreachFlags::<Runtime>::remove(epoch);
        assert!(!crate::configs::RuntimeGuardianState::gate_suspended());
        pallet_welfare::GateBreachFlags::<Runtime>::insert(
            epoch,
            pallet_welfare::CoreGateBreachFlags {
                s_breached: true,
                c_breached: false,
                day_bitmap: [1, 0],
            },
        );
        pallet_epoch::EpochOf::<Runtime>::mutate(|info| info.index = epoch.saturating_add(1));
        assert!(!crate::configs::RuntimeGuardianState::gate_suspended());
    });
}

fn seat_runtime_guardians(first_member_seed: u8) -> [AccountId; pallet_guardian::GUARDIAN_SEATS] {
    let members = core::array::from_fn(|index| account(first_member_seed + index as u8));
    for member in &members {
        assert_ok!(Balances::force_set_balance(
            RuntimeOrigin::root(),
            MultiAddress::Id(member.clone()),
            pallet_guardian::GUARDIAN_BOND.saturating_add(currency::VIT),
        ));
    }
    assert_ok!(Guardian::set_members(
        crate::track_origins::Origin::GuardianTrack.into(),
        members.clone(),
    ));
    members
}

fn dispatch_runtime_guardian_power(
    power: pallet_guardian::GuardianPower,
    first_member_seed: u8,
) -> u32 {
    let members = seat_runtime_guardians(first_member_seed);
    assert_ok!(Guardian::propose_action(
        RuntimeOrigin::signed(members[0].clone()),
        power,
        H256::repeat_byte(first_member_seed).into(),
    ));
    let action = pallet_guardian::NextActionId::<Runtime>::get().saturating_sub(1);
    for member in members.iter().take(5).skip(1) {
        assert_ok!(Guardian::approve_action(
            RuntimeOrigin::signed(member.clone()),
            action,
        ));
    }
    action
}

#[test]
fn five_guardians_pause_intake_until_the_lazy_expiry_boundary() {
    development_ext().execute_with(|| {
        System::set_block_number(10);
        let before_referenda = pallet_referenda::ReferendumCount::<Runtime>::get();
        let action = dispatch_runtime_guardian_power(
            pallet_guardian::GuardianPower::PauseIntake { until: 20 },
            120,
        );
        assert_eq!(
            pallet_epoch::GuardianIntakePausedUntil::<Runtime>::get(),
            Some(20)
        );
        assert!(pallet_guardian::ReviewReferenda::<Runtime>::contains_key(
            action
        ));
        assert!(!pallet_guardian::VetoReviewReferenda::<Runtime>::contains_key(action));
        assert_eq!(
            pallet_referenda::ReferendumCount::<Runtime>::get(),
            before_referenda.saturating_add(1),
            "non-delay guardian actions keep exactly one review referendum"
        );
        assert_guardian_review_referendum(before_referenda, action, false);

        // 06 §5.2: PauseIntake is limited to one successful dispatch per
        // rolling four-epoch window. The rejected fifth approval is atomic.
        let members: [AccountId; pallet_guardian::GUARDIAN_SEATS] =
            core::array::from_fn(|index| account(120 + index as u8));
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(members[0].clone()),
            pallet_guardian::GuardianPower::PauseIntake { until: 21 },
            H256::repeat_byte(121).into(),
        ));
        let blocked_action = pallet_guardian::NextActionId::<Runtime>::get().saturating_sub(1);
        for member in members.iter().take(4).skip(1) {
            assert_ok!(Guardian::approve_action(
                RuntimeOrigin::signed(member.clone()),
                blocked_action,
            ));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(members[4].clone()), blocked_action),
            pallet_guardian::Error::<Runtime>::AllowanceExhausted
        );
        assert_eq!(
            pallet_guardian::Approvals::<Runtime>::get()
                .iter()
                .filter(|(id, _)| *id == blocked_action)
                .count(),
            4
        );

        let proposer = account(130);
        let batch = pallet_execution_guard::pallet::RuntimeBatch::<Runtime>::default();
        let bytes = batch.encode();
        let payload_len = u32::try_from(bytes.len()).unwrap_or_default();
        let payload_hash = <Preimage as StorePreimage>::note(bytes.into()).unwrap_or_default();
        assert_ok!(ForeignAssets::mint_into(
            usdc_location(),
            &proposer,
            crate::configs::balance_param(b"prop.bond.param").saturating_add(currency::USDC),
        ));
        let pid = pallet_epoch::NextProposalId::<Runtime>::get();
        assert_noop!(
            Epoch::submit(
                RuntimeOrigin::signed(proposer.clone()),
                empty_param_proposal(pid, proposer.clone(), payload_hash, payload_len),
            ),
            pallet_epoch::Error::<Runtime>::IntakePaused
        );

        System::set_block_number(20);
        assert_ok!(Epoch::submit(
            RuntimeOrigin::signed(proposer.clone()),
            empty_param_proposal(pid, proposer, payload_hash, payload_len),
        ));
    });
}

#[test]
fn halt_intake_playbook_activates_from_gate_breach_and_expiry_reverts() {
    use frame_support::traits::Hooks;

    development_ext().execute_with(|| {
        System::set_block_number(10);
        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        pallet_welfare::GateBreachFlags::<Runtime>::insert(
            epoch,
            pallet_welfare::CoreGateBreachFlags {
                s_breached: true,
                c_breached: false,
                day_bitmap: [1, 0],
            },
        );
        let action = dispatch_runtime_guardian_power(
            pallet_guardian::GuardianPower::ActivatePlaybook {
                id: pallet_guardian::PlaybookId::HaltIntake,
                trigger: pallet_guardian::PlaybookTrigger::GateBreach,
                expiry: 20,
                target: None,
            },
            140,
        );
        assert!(Guardian::playbook_active(
            pallet_guardian::PlaybookId::HaltIntake
        ));
        assert_eq!(pallet_epoch::IntakePausedUntil::<Runtime>::get(), Some(20));
        assert!(pallet_guardian::ReviewReferenda::<Runtime>::contains_key(
            action
        ));

        System::set_block_number(20);
        let _ = Guardian::on_initialize(20);
        assert!(!Guardian::playbook_active(
            pallet_guardian::PlaybookId::HaltIntake
        ));
        assert_eq!(pallet_epoch::IntakePausedUntil::<Runtime>::get(), None);
    });
}

#[test]
fn halt_intake_expiry_preserves_a_longer_direct_guardian_pause() {
    use frame_support::traits::Hooks;

    development_ext().execute_with(|| {
        System::set_block_number(10);
        let direct_until =
            10_u32.saturating_add(futarchy_primitives::kernel::PLAYBOOK_FREEZE_WINDOW_BLOCKS);
        let playbook_until = 10_u32.saturating_add(14_400);
        let _direct_action = dispatch_runtime_guardian_power(
            pallet_guardian::GuardianPower::PauseIntake {
                until: direct_until,
            },
            150,
        );

        let epoch = pallet_epoch::CurrentEpoch::<Runtime>::get();
        pallet_welfare::GateBreachFlags::<Runtime>::insert(
            epoch,
            pallet_welfare::CoreGateBreachFlags {
                s_breached: true,
                c_breached: false,
                day_bitmap: [1, 0],
            },
        );
        let _playbook_action = dispatch_runtime_guardian_power(
            pallet_guardian::GuardianPower::ActivatePlaybook {
                id: pallet_guardian::PlaybookId::HaltIntake,
                trigger: pallet_guardian::PlaybookTrigger::GateBreach,
                expiry: playbook_until,
                target: None,
            },
            160,
        );
        assert_eq!(
            pallet_epoch::GuardianIntakePausedUntil::<Runtime>::get(),
            Some(direct_until)
        );
        assert_eq!(
            pallet_epoch::IntakePausedUntil::<Runtime>::get(),
            Some(playbook_until)
        );
        assert_eq!(Epoch::intake_paused_until(), Some(direct_until));

        System::set_block_number(playbook_until);
        let _ = Guardian::on_initialize(playbook_until);
        assert!(!Guardian::playbook_active(
            pallet_guardian::PlaybookId::HaltIntake
        ));
        assert_eq!(pallet_epoch::IntakePausedUntil::<Runtime>::get(), None);
        assert_eq!(
            pallet_epoch::GuardianIntakePausedUntil::<Runtime>::get(),
            Some(direct_until)
        );
        assert!(Epoch::intake_paused(playbook_until));

        System::set_block_number(direct_until);
        assert!(!Epoch::intake_paused(direct_until));
    });
}

#[test]
fn guardian_track_admin_controls_registration_and_oracle_void_trigger_stays_fail_closed() {
    use pallet_guardian::GuardianTriggers;

    development_ext().execute_with(|| {
        assert_ok!(Guardian::set_playbook_registered(
            crate::track_origins::Origin::GuardianTrack.into(),
            pallet_guardian::PlaybookId::Depeg,
            false,
        ));
        assert!(!pallet_guardian::PlaybookRegistered::<Runtime>::get(
            pallet_guardian::PlaybookId::Depeg
        ));
        assert_noop!(
            Guardian::set_playbook_registered(
                crate::track_origins::Origin::Ratify.into(),
                pallet_guardian::PlaybookId::Depeg,
                true,
            ),
            DispatchError::BadOrigin
        );

        let triggers = crate::configs::RuntimeGuardianTriggers::current();
        assert!(!triggers.oracle_deadlock);
        assert!(!triggers.void_in_flight);
        let oracle_expiry = System::block_number().saturating_add(10);
        pallet_guardian::ActivePlaybooks::<Runtime>::mutate(|active| {
            let _ = active.try_push(pallet_guardian::ActivePlaybook {
                id: pallet_guardian::PlaybookId::OracleVoid,
                expiry: oracle_expiry,
                renewals_used: 0,
            });
        });
        assert!(crate::configs::RuntimeGuardianTriggers::current().void_in_flight);
        System::set_block_number(oracle_expiry);
        assert!(!Guardian::playbook_active(
            pallet_guardian::PlaybookId::OracleVoid
        ));
        assert!(!crate::configs::RuntimeGuardianTriggers::current().void_in_flight);
    });
}

#[test]
fn oracle_void_unfed_trigger_and_migration_gap_roll_back_fifth_approval() {
    use frame_support::migrations::FailedMigrationHandler;

    development_ext().execute_with(|| {
        System::set_block_number(10);
        let members = seat_runtime_guardians(170);

        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(members[0].clone()),
            pallet_guardian::GuardianPower::ActivatePlaybook {
                id: pallet_guardian::PlaybookId::OracleVoid,
                trigger: pallet_guardian::PlaybookTrigger::OracleDeadlock,
                expiry: 20,
                target: Some(pallet_epoch::CurrentEpoch::<Runtime>::get()),
            },
            H256::repeat_byte(170).into(),
        ));
        let oracle_action = pallet_guardian::NextActionId::<Runtime>::get().saturating_sub(1);
        for member in members.iter().take(4).skip(1) {
            assert_ok!(Guardian::approve_action(
                RuntimeOrigin::signed(member.clone()),
                oracle_action,
            ));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(members[4].clone()), oracle_action),
            pallet_guardian::Error::<Runtime>::TriggerInactive
        );
        assert!(!Guardian::playbook_active(
            pallet_guardian::PlaybookId::OracleVoid
        ));

        assert_eq!(
            crate::configs::MigrationFailureToGuard::failed(Some(9)),
            frame_support::migrations::FailedMigrationHandling::KeepStuck
        );
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(members[0].clone()),
            pallet_guardian::GuardianPower::ActivatePlaybook {
                id: pallet_guardian::PlaybookId::Migration,
                trigger: pallet_guardian::PlaybookTrigger::MigrationHalt,
                expiry: 20,
                target: None,
            },
            H256::repeat_byte(171).into(),
        ));
        let migration_action = pallet_guardian::NextActionId::<Runtime>::get().saturating_sub(1);
        for member in members.iter().take(4).skip(1) {
            assert_ok!(Guardian::approve_action(
                RuntimeOrigin::signed(member.clone()),
                migration_action,
            ));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(members[4].clone()), migration_action),
            DispatchError::Other(
                "PB-MIGRATION cursor retry has no EmergencyPlaybook-safe runtime call"
            )
        );
        assert!(!Guardian::playbook_active(
            pallet_guardian::PlaybookId::Migration
        ));
        assert_eq!(
            pallet_guardian::Approvals::<Runtime>::get()
                .iter()
                .filter(|(id, _)| *id == migration_action)
                .count(),
            4
        );
    });
}

#[test]
fn ledger_freeze_runtime_effect_renews_both_pallets_once_and_reverts_both() {
    use pallet_guardian::GuardianEffectDispatcher;

    development_ext().execute_with(|| {
        System::set_block_number(10);
        assert_ok!(crate::configs::RuntimeGuardianEffects::dispatch(
            pallet_guardian::GuardianPower::ActivatePlaybook {
                id: pallet_guardian::PlaybookId::LedgerFreeze,
                trigger: pallet_guardian::PlaybookTrigger::LedgerDrift,
                expiry: 20,
                target: None,
            },
            H256::repeat_byte(180).into(),
        ));
        assert!(pallet_conditional_ledger::FrozenUntil::<Runtime>::get().is_some());
        assert!(pallet_market::FrozenUntil::<Runtime>::get().is_some());

        System::set_block_number(11);
        assert_ok!(crate::configs::RuntimeGuardianEffects::renew_playbook(
            pallet_guardian::PlaybookId::LedgerFreeze
        ));
        let expected = 11u32.saturating_add(kernel::PLAYBOOK_FREEZE_WINDOW_BLOCKS);
        assert_eq!(
            pallet_conditional_ledger::FrozenUntil::<Runtime>::get(),
            Some(expected)
        );
        assert_eq!(pallet_market::FrozenUntil::<Runtime>::get(), Some(expected));
        assert!(crate::configs::RuntimeGuardianEffects::renew_playbook(
            pallet_guardian::PlaybookId::LedgerFreeze
        )
        .is_err());

        assert_ok!(crate::configs::RuntimeGuardianEffects::revert_playbook(
            pallet_guardian::PlaybookId::LedgerFreeze
        ));
        assert_eq!(
            pallet_conditional_ledger::FrozenUntil::<Runtime>::get(),
            None
        );
        assert_eq!(pallet_market::FrozenUntil::<Runtime>::get(), None);
    });
}

/// 07 §7 *Milestone normalization* + SQ-175: the production runtime has no
/// MetricSpec `target`, so the Milestone registry MUST stay fail-closed — a
/// zero target refuses `file`/`close_epoch` rather than fabricating a 0.0
/// aggregate. Under `runtime-benchmarks` the same seam MUST instead be
/// positive, or every `MilestoneRegistry` benchmark setup aborts with
/// `MilestoneTargetUnset` before anything is measured and weight generation
/// for the instance dies silently (Codex review, PR #118).
#[test]
fn milestone_target_seam_is_fail_closed_in_production_and_admissible_under_benchmarks() {
    use pallet_registry::EpochContext as _;
    let target = crate::configs::RuntimeRegistryEpoch::milestone_target(1);
    if cfg!(feature = "runtime-benchmarks") {
        assert!(
            target > 0,
            "benchmark builds need a positive milestone target or MilestoneRegistry benchmarks abort"
        );
    } else {
        assert_eq!(
            target, 0,
            "production must stay fail-closed until SQ-175 wires a real MetricSpec target"
        );
    }
}
