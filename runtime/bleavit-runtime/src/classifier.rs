use alloc::{boxed::Box, vec, vec::Vec};
use frame_support::{
    traits::{ConstU32, Contains, IsSubType, UnfilteredDispatchable},
    BoundedVec,
};
use futarchy_primitives::{kernel, ProposalClass, ResourceId, RuntimeVersionConstraint, H256};
use origins_core::{BoxedCall, CallDomain, Origin as ClassOrigin, RuntimeCall as FilterCall};
use pallet_origins::{SafetyClassifier, SafetyFilter};
use parity_scale_codec::{Decode, Encode};
use sp_runtime::{traits::Dispatchable, DispatchError};

use crate::{Runtime, RuntimeCall, RuntimeOrigin, System, VERSION};

/// Execution-guard-owned upgrade availability seam (09 §2.2).
pub trait PendingUpgradeProvider {
    fn applicable_at() -> Option<futarchy_primitives::BlockNumber>;
}

/// The base filter trusts only the real guard-owned pending-upgrade record.
pub struct PendingExecutionGuard;

impl PendingUpgradeProvider for PendingExecutionGuard {
    fn applicable_at() -> Option<futarchy_primitives::BlockNumber> {
        pallet_execution_guard::pallet::PendingUpgrade::<crate::Runtime>::get()
            .map(|pending| pending.applicable_at)
    }
}

#[derive(Clone, Copy)]
struct ProjectionBudget {
    depth: u32,
    calls: u32,
}

impl ProjectionBudget {
    const fn root() -> Self {
        Self { depth: 0, calls: 0 }
    }

    fn count(&mut self) -> bool {
        match self.calls.checked_add(1) {
            Some(next) if next <= kernel::MAX_NESTED_CALLS => {
                self.calls = next;
                true
            }
            _ => false,
        }
    }

    fn enter(&mut self) -> bool {
        match self.depth.checked_add(1) {
            Some(next) if next <= kernel::MAX_NESTED_LEVELS => {
                self.depth = next;
                true
            }
            _ => false,
        }
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}

/// Why a proposal payload has no canonical 05 §1.4 resource footprint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FootprintError {
    Unclassifiable,
    TooManyResources,
}

fn keyed_resource(tag: u8, discriminator: &[u8]) -> ResourceId {
    let digest = sp_io::hashing::blake2_256(discriminator);
    let mut resource = [0_u8; 8];
    resource[0] = tag;
    resource[1..].copy_from_slice(&digest[..7]);
    resource
}

fn singleton_resource(tag: u8) -> ResourceId {
    let mut resource = [0_u8; 8];
    resource[0] = tag;
    resource
}

fn insert_resource(
    footprint: &mut BoundedVec<ResourceId, ConstU32<8>>,
    resource: ResourceId,
) -> Result<(), FootprintError> {
    if !footprint.contains(&resource) {
        footprint
            .try_push(resource)
            .map_err(|_| FootprintError::TooManyResources)?;
    }
    Ok(())
}

fn derive_resource_inner(
    call: &RuntimeCall,
    budget: &mut ProjectionBudget,
    footprint: &mut BoundedVec<ResourceId, ConstU32<8>>,
) -> Result<(), FootprintError> {
    if !budget.count() {
        return Err(FootprintError::Unclassifiable);
    }

    let resource = match call {
        RuntimeCall::Constitution(pallet_constitution::Call::set_param { key, .. }) => {
            if !matches!(
                pallet_constitution::Params::<Runtime>::get(key).map(|record| record.class),
                Some(
                    pallet_constitution::ParamClass::Param
                        | pallet_constitution::ParamClass::Treasury
                        | pallet_constitution::ParamClass::Meta
                        | pallet_constitution::ParamClass::MetaAndValues
                )
            ) {
                return Err(FootprintError::Unclassifiable);
            }
            keyed_resource(0x01, key)
        }
        RuntimeCall::Constitution(pallet_constitution::Call::amend_registry { key, .. }) => {
            keyed_resource(0x01, key)
        }
        RuntimeCall::Constitution(pallet_constitution::Call::set_capability { record }) => {
            let mut discriminator = record.class.encode();
            discriminator.extend(record.capability.encode());
            keyed_resource(0x02, &discriminator)
        }
        RuntimeCall::System(frame_system::Call::authorize_upgrade { .. }) => {
            singleton_resource(0x03)
        }
        RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::spend { dest, .. }) => {
            keyed_resource(0x07, &dest.encode())
        }
        RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::open_stream {
            recipient,
            ..
        }) => keyed_resource(0x07, &recipient.encode()),
        RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::cancel_stream { id }) => {
            keyed_resource(0x08, &id.encode())
        }
        RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::fund_budget_line {
            line,
            ..
        }) => keyed_resource(0x09, &line.encode()),
        RuntimeCall::FutarchyTreasury(pallet_futarchy_treasury::Call::set_coretime_authority {
            ..
        }) => keyed_resource(
            0x09,
            &pallet_futarchy_treasury::BudgetLine::OpsCoretime.encode(),
        ),
        RuntimeCall::Utility(pallet_utility::Call::batch_all { calls }) => {
            if !budget.enter() {
                return Err(FootprintError::Unclassifiable);
            }
            for nested in calls {
                if let Err(error) = derive_resource_inner(nested, budget, footprint) {
                    budget.leave();
                    return Err(error);
                }
            }
            budget.leave();
            return Ok(());
        }
        _ => return Err(FootprintError::Unclassifiable),
    };

    insert_resource(footprint, resource)
}

/// Derive the canonical, deduplicated resource-domain footprint from a decoded
/// execution payload (05 §1.4). Only `utility.batch_all` is a valid wrapper.
pub(crate) fn derive_resource_footprint(
    calls: &[RuntimeCall],
) -> Result<BoundedVec<ResourceId, ConstU32<8>>, FootprintError> {
    let mut budget = ProjectionBudget::root();
    let mut footprint = BoundedVec::default();
    for call in calls {
        derive_resource_inner(call, &mut budget, &mut footprint)?;
    }
    Ok(footprint)
}

fn denied() -> FilterCall {
    FilterCall::Leaf(CallDomain::Nobody)
}

fn boxed(call: FilterCall) -> BoxedCall {
    BoxedCall(Box::new(call))
}

fn leaf(domain: CallDomain) -> FilterCall {
    FilterCall::Leaf(domain)
}

fn projected_or_denied<T>(projected: Option<T>, wrap: impl FnOnce(T) -> FilterCall) -> FilterCall {
    match projected {
        Some(call) => wrap(call),
        None => denied(),
    }
}

fn project_many(calls: &[RuntimeCall], budget: &mut ProjectionBudget) -> Option<Vec<FilterCall>> {
    if !budget.enter() {
        return None;
    }
    let mut projected = Vec::new();
    for call in calls {
        if budget.calls >= kernel::MAX_NESTED_CALLS {
            budget.leave();
            return None;
        }
        projected.push(project_inner(call, budget));
        if matches!(projected.last(), Some(FilterCall::Leaf(CallDomain::Nobody)))
            && budget.calls >= kernel::MAX_NESTED_CALLS
        {
            budget.leave();
            return None;
        }
    }
    budget.leave();
    Some(projected)
}

fn project_wrapped(call: &RuntimeCall, budget: &mut ProjectionBudget) -> Option<BoxedCall> {
    if !budget.enter() {
        return None;
    }
    let projected = project_inner(call, budget);
    budget.leave();
    Some(boxed(projected))
}

#[allow(clippy::too_many_lines)]
fn project_inner(call: &RuntimeCall, budget: &mut ProjectionBudget) -> FilterCall {
    if !budget.count() {
        return denied();
    }

    match call {
        RuntimeCall::System(call) => match call {
            frame_system::Call::remark { .. } | frame_system::Call::remark_with_event { .. } => {
                leaf(CallDomain::Public)
            }
            frame_system::Call::authorize_upgrade { .. } => leaf(CallDomain::InternalRoot),
            frame_system::Call::apply_authorized_upgrade { code } => {
                if pending_upgrade_is_applicable(code) {
                    leaf(CallDomain::Public)
                } else {
                    denied()
                }
            }
            frame_system::Call::set_heap_pages { .. }
            | frame_system::Call::set_code { .. }
            | frame_system::Call::set_code_without_checks { .. }
            | frame_system::Call::set_storage { .. }
            | frame_system::Call::kill_storage { .. }
            | frame_system::Call::kill_prefix { .. }
            | frame_system::Call::authorize_upgrade_without_checks { .. }
            | frame_system::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::ParachainSystem(call) => match call {
            cumulus_pallet_parachain_system::Call::set_validation_data { .. } => {
                leaf(CallDomain::Public)
            }
            // Arbitrary UMP send is the upward analog of `pallet_xcm.send`
            // (06 §3.2 nobody row; 09 §6.1 "no Transact governance either
            // direction") — denied for every origin including sudo.
            cumulus_pallet_parachain_system::Call::sudo_send_upward_message { .. }
            | cumulus_pallet_parachain_system::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Timestamp(call) => match call {
            pallet_timestamp::Call::set { .. } => leaf(CallDomain::Public),
            pallet_timestamp::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::ParachainInfo(call) => match call {
            staging_parachain_info::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Balances(call) => match call {
            pallet_balances::Call::transfer_allow_death { .. }
            | pallet_balances::Call::transfer_keep_alive { .. }
            | pallet_balances::Call::transfer_all { .. }
            | pallet_balances::Call::upgrade_accounts { .. }
            | pallet_balances::Call::burn { .. } => leaf(CallDomain::Public),
            // VIT is conviction-voting power and bond collateral: forced
            // transfers/minting/unreserves are the native-balance analog of the
            // D-13 storage-rewrite row — denied for every origin including
            // sudo (06 §3.2 nobody row's "asset force_*", applied to the
            // native asset).
            pallet_balances::Call::force_transfer { .. }
            | pallet_balances::Call::force_unreserve { .. }
            | pallet_balances::Call::force_set_balance { .. }
            | pallet_balances::Call::force_adjust_total_issuance { .. }
            | pallet_balances::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Vesting(call) => match call {
            pallet_vesting::Call::vest { .. }
            | pallet_vesting::Call::vest_other { .. }
            | pallet_vesting::Call::vested_transfer { .. }
            | pallet_vesting::Call::merge_schedules { .. } => leaf(CallDomain::Public),
            // D-13 nobody row: these rewrite third-party VIT lock state, the
            // vesting analog of balances.force_*, so even sudo may not reach them.
            pallet_vesting::Call::force_vested_transfer { .. }
            | pallet_vesting::Call::force_remove_vesting_schedule { .. }
            | pallet_vesting::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::ForeignAssets(call) => match call {
            pallet_assets::Call::create { .. } => leaf(CallDomain::ConstitutionalValues),
            pallet_assets::Call::transfer { .. }
            | pallet_assets::Call::transfer_keep_alive { .. }
            | pallet_assets::Call::approve_transfer { .. }
            | pallet_assets::Call::cancel_approval { .. }
            | pallet_assets::Call::transfer_approved { .. }
            | pallet_assets::Call::touch { .. }
            | pallet_assets::Call::refund { .. }
            | pallet_assets::Call::touch_other { .. }
            | pallet_assets::Call::refund_other { .. }
            | pallet_assets::Call::transfer_all { .. } => leaf(CallDomain::Public),
            pallet_assets::Call::force_create { .. }
            | pallet_assets::Call::start_destroy { .. }
            | pallet_assets::Call::destroy_accounts { .. }
            | pallet_assets::Call::destroy_approvals { .. }
            | pallet_assets::Call::finish_destroy { .. }
            | pallet_assets::Call::mint { .. }
            | pallet_assets::Call::burn { .. }
            | pallet_assets::Call::force_transfer { .. }
            | pallet_assets::Call::freeze { .. }
            | pallet_assets::Call::thaw { .. }
            | pallet_assets::Call::freeze_asset { .. }
            | pallet_assets::Call::thaw_asset { .. }
            | pallet_assets::Call::transfer_ownership { .. }
            | pallet_assets::Call::set_team { .. }
            | pallet_assets::Call::set_metadata { .. }
            | pallet_assets::Call::clear_metadata { .. }
            | pallet_assets::Call::force_set_metadata { .. }
            | pallet_assets::Call::force_clear_metadata { .. }
            | pallet_assets::Call::force_asset_status { .. }
            | pallet_assets::Call::force_cancel_approval { .. }
            | pallet_assets::Call::set_min_balance { .. }
            | pallet_assets::Call::block { .. }
            | pallet_assets::Call::set_reserves { .. }
            | pallet_assets::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Referenda(call) => match call {
            pallet_referenda::Call::cancel { .. } | pallet_referenda::Call::kill { .. } => {
                leaf(CallDomain::ConstitutionalValues)
            }
            pallet_referenda::Call::submit { .. }
            | pallet_referenda::Call::place_decision_deposit { .. }
            | pallet_referenda::Call::refund_decision_deposit { .. }
            | pallet_referenda::Call::nudge_referendum { .. }
            | pallet_referenda::Call::one_fewer_deciding { .. }
            | pallet_referenda::Call::refund_submission_deposit { .. }
            | pallet_referenda::Call::set_metadata { .. } => leaf(CallDomain::Public),
            pallet_referenda::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::ConvictionVoting(call) => match call {
            pallet_conviction_voting::Call::vote { .. }
            | pallet_conviction_voting::Call::delegate { .. }
            | pallet_conviction_voting::Call::undelegate { .. }
            | pallet_conviction_voting::Call::unlock { .. }
            | pallet_conviction_voting::Call::remove_vote { .. }
            | pallet_conviction_voting::Call::remove_other_vote { .. } => leaf(CallDomain::Public),
            pallet_conviction_voting::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Preimage(call) => match call {
            pallet_preimage::Call::note_preimage { .. }
            | pallet_preimage::Call::unnote_preimage { .. }
            | pallet_preimage::Call::request_preimage { .. }
            | pallet_preimage::Call::unrequest_preimage { .. }
            | pallet_preimage::Call::ensure_updated { .. } => leaf(CallDomain::Public),
            pallet_preimage::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Scheduler(call) => match call {
            pallet_scheduler::Call::schedule { .. }
            | pallet_scheduler::Call::cancel { .. }
            | pallet_scheduler::Call::schedule_named { .. }
            | pallet_scheduler::Call::cancel_named { .. }
            | pallet_scheduler::Call::schedule_after { .. }
            | pallet_scheduler::Call::schedule_named_after { .. }
            | pallet_scheduler::Call::set_retry { .. }
            | pallet_scheduler::Call::set_retry_named { .. }
            | pallet_scheduler::Call::cancel_retry { .. }
            | pallet_scheduler::Call::cancel_retry_named { .. }
            | pallet_scheduler::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Utility(call) => match call {
            pallet_utility::Call::batch { calls } => {
                projected_or_denied(project_many(calls, budget), FilterCall::UtilityBatch)
            }
            pallet_utility::Call::batch_all { calls } => {
                projected_or_denied(project_many(calls, budget), FilterCall::UtilityBatchAll)
            }
            pallet_utility::Call::force_batch { calls } => {
                projected_or_denied(project_many(calls, budget), FilterCall::UtilityForceBatch)
            }
            pallet_utility::Call::as_derivative { call, .. } => projected_or_denied(
                project_wrapped(call, budget),
                FilterCall::UtilityAsDerivative,
            ),
            pallet_utility::Call::dispatch_as { call, .. } => {
                projected_or_denied(project_wrapped(call, budget), FilterCall::UtilityDispatchAs)
            }
            pallet_utility::Call::with_weight { call, .. } => {
                projected_or_denied(project_wrapped(call, budget), FilterCall::UtilityWithWeight)
            }
            // Stable2603 added two call-carrying utility variants after the
            // frozen 06 §3.3 wrapper table. They fail closed until that table is amended.
            pallet_utility::Call::if_else { .. }
            | pallet_utility::Call::dispatch_as_fallible { .. }
            | pallet_utility::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Proxy(call) => match call {
            pallet_proxy::Call::proxy { call, .. } => {
                projected_or_denied(project_wrapped(call, budget), FilterCall::Proxy)
            }
            pallet_proxy::Call::proxy_announced { call, .. } => {
                projected_or_denied(project_wrapped(call, budget), FilterCall::ProxyAnnounced)
            }
            pallet_proxy::Call::add_proxy { .. }
            | pallet_proxy::Call::remove_proxy { .. }
            | pallet_proxy::Call::remove_proxies { .. }
            | pallet_proxy::Call::create_pure { .. }
            | pallet_proxy::Call::kill_pure { .. }
            | pallet_proxy::Call::announce { .. }
            | pallet_proxy::Call::remove_announcement { .. }
            | pallet_proxy::Call::reject_announcement { .. }
            | pallet_proxy::Call::poke_deposit { .. } => leaf(CallDomain::Public),
            pallet_proxy::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Multisig(call) => match call {
            pallet_multisig::Call::as_multi_threshold_1 { call, .. } => projected_or_denied(
                project_wrapped(call, budget),
                FilterCall::MultisigAsMultiThreshold1,
            ),
            pallet_multisig::Call::as_multi { call, .. } => {
                projected_or_denied(project_wrapped(call, budget), FilterCall::MultisigAsMulti)
            }
            pallet_multisig::Call::approve_as_multi { .. } => FilterCall::MultisigApproveAsMulti,
            pallet_multisig::Call::cancel_as_multi { .. }
            | pallet_multisig::Call::poke_deposit { .. } => leaf(CallDomain::Public),
            pallet_multisig::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Migrations(call) => match call {
            pallet_migrations::Call::force_set_cursor { .. }
            | pallet_migrations::Call::force_set_active_cursor { .. }
            | pallet_migrations::Call::force_onboard_mbms { .. }
            | pallet_migrations::Call::clear_historic { .. }
            | pallet_migrations::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Sudo(call) => match call {
            // `sudo`/`sudo_unchecked_weight` dispatch the inner call as Root.
            // Root satisfies no custom `EnsureOrigin`, no protocol-account
            // `Signed(_)` authority, and — because the inner projection is
            // recursed — cannot reach the nobody row (09 §5.1, D-13).
            pallet_sudo::Call::sudo { call }
            | pallet_sudo::Call::sudo_unchecked_weight { call, .. } => {
                projected_or_denied(project_wrapped(call, budget), FilterCall::Sudo)
            }
            // `sudo_as` dispatches the inner call as `Signed(who)` for a
            // caller-CHOSEN `who`, bypassing the base filter. That fabricates an
            // arbitrary signed origin — including a victim account (fund theft
            // via `balances.transfer`) or a protocol sovereign such as the
            // welfare settlement account, which would forge the SettleAuthority
            // and drive ledger settlement directly. 06 §3.1 makes SettleAuthority
            // "reachable through exactly one path" (welfare→ledger) and D-13
            // bounds the founding multisig's worst case; impersonation defeats
            // both. `sudo_as` has no bootstrap use the Root-dispatching `sudo`
            // does not already cover (09 §5.3), so it is denied outright. R-1
            // spec correction (06 §3.3): the wrapper table's "recursed" entry
            // for `sudo_as` is narrowed to "denied" (PLAN Decision log; SQ-99).
            pallet_sudo::Call::sudo_as { .. } => denied(),
            pallet_sudo::Call::set_key { .. } | pallet_sudo::Call::remove_key { .. } => {
                leaf(CallDomain::Public)
            }
            pallet_sudo::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::XcmpQueue(call) => match call {
            cumulus_pallet_xcmp_queue::Call::suspend_xcm_execution { .. }
            | cumulus_pallet_xcmp_queue::Call::resume_xcm_execution { .. }
            | cumulus_pallet_xcmp_queue::Call::update_suspend_threshold { .. }
            | cumulus_pallet_xcmp_queue::Call::update_drop_threshold { .. }
            | cumulus_pallet_xcmp_queue::Call::update_resume_threshold { .. } => {
                leaf(CallDomain::Public)
            }
            cumulus_pallet_xcmp_queue::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::MessageQueue(call) => match call {
            pallet_message_queue::Call::reap_page { .. }
            | pallet_message_queue::Call::execute_overweight { .. } => leaf(CallDomain::Public),
            pallet_message_queue::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::CumulusXcm(call) => match call {
            cumulus_pallet_xcm::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::PolkadotXcm(call) => match call {
            // Self-scoped by pallet-xcm's `(origin, assets)` trap key: a Signed
            // account can recover only its own local-origin trap, while remote
            // traps require an inbound ClaimAsset from that exact origin.
            pallet_xcm::Call::claim_assets { .. } => leaf(CallDomain::Public),
            pallet_xcm::Call::limited_reserve_transfer_assets { .. } => {
                match bleavit_xcm::filter::classify_pallet_xcm_call(call) {
                    bleavit_xcm::filter::XcmCallDisposition::SignedAllowed => {
                        leaf(CallDomain::Public)
                    }
                    bleavit_xcm::filter::XcmCallDisposition::DeniedAllOrigins
                    | bleavit_xcm::filter::XcmCallDisposition::TreasuryOnly => denied(),
                }
            }
            // Every remaining pallet-xcm call stays nobody: arbitrary XCM,
            // teleport and generic transfer escape hatches remain closed.
            pallet_xcm::Call::send { .. }
            | pallet_xcm::Call::teleport_assets { .. }
            | pallet_xcm::Call::reserve_transfer_assets { .. }
            | pallet_xcm::Call::execute { .. }
            | pallet_xcm::Call::force_xcm_version { .. }
            | pallet_xcm::Call::force_default_xcm_version { .. }
            | pallet_xcm::Call::force_subscribe_version_notify { .. }
            | pallet_xcm::Call::force_unsubscribe_version_notify { .. }
            | pallet_xcm::Call::limited_teleport_assets { .. }
            | pallet_xcm::Call::force_suspension { .. }
            | pallet_xcm::Call::transfer_assets { .. }
            | pallet_xcm::Call::transfer_assets_using_type_and_then { .. }
            | pallet_xcm::Call::add_authorized_alias { .. }
            | pallet_xcm::Call::remove_authorized_alias { .. }
            | pallet_xcm::Call::remove_all_authorized_aliases { .. }
            | pallet_xcm::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::CollatorSelection(call) => match call {
            pallet_collator_selection::Call::set_invulnerables { .. }
            | pallet_collator_selection::Call::set_desired_candidates { .. }
            | pallet_collator_selection::Call::set_candidacy_bond { .. }
            | pallet_collator_selection::Call::register_as_candidate { .. }
            | pallet_collator_selection::Call::leave_intent { .. }
            | pallet_collator_selection::Call::add_invulnerable { .. }
            | pallet_collator_selection::Call::remove_invulnerable { .. }
            | pallet_collator_selection::Call::update_bond { .. }
            | pallet_collator_selection::Call::take_candidate_slot { .. } => {
                leaf(CallDomain::Public)
            }
            pallet_collator_selection::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Session(call) => match call {
            pallet_session::Call::set_keys { .. } | pallet_session::Call::purge_keys { .. } => {
                leaf(CallDomain::Public)
            }
            pallet_session::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Constitution(call) => match call {
            // 06 §3.2 routes `set_param` by the key's class: PARAM keys via
            // FutarchyParam, TREASURY keys (`pol.b*`, `ops.*`) via
            // FutarchyTreasury, META/META+values keys via FutarchyMeta,
            // CONST/entrenched keys via ConstitutionalValues. The classifier
            // reads the live registry record; an unknown key fails closed
            // (`set_param` on it would fail `UnknownParam` in-pallet anyway).
            pallet_constitution::Call::set_param { key, .. } => {
                match pallet_constitution::Params::<crate::Runtime>::get(key)
                    .map(|record| record.class)
                {
                    Some(pallet_constitution::ParamClass::Param) => leaf(CallDomain::Param),
                    Some(pallet_constitution::ParamClass::Treasury) => leaf(CallDomain::Treasury),
                    Some(
                        pallet_constitution::ParamClass::Meta
                        | pallet_constitution::ParamClass::MetaAndValues,
                    ) => leaf(CallDomain::Meta),
                    Some(
                        pallet_constitution::ParamClass::Const
                        | pallet_constitution::ParamClass::Entrenched,
                    ) => leaf(CallDomain::ConstitutionalValues),
                    None => denied(),
                }
            }
            pallet_constitution::Call::set_capability { .. } => leaf(CallDomain::Meta),
            pallet_constitution::Call::set_phase_flag { .. } => leaf(CallDomain::Public),
            pallet_constitution::Call::set_release_channel { .. } => {
                leaf(CallDomain::ConstitutionalValues)
            }
            pallet_constitution::Call::amend_registry { .. } => leaf(CallDomain::Meta),
            pallet_constitution::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::ConditionalLedger(call) => match call {
            pallet_conditional_ledger::Call::split { .. }
            | pallet_conditional_ledger::Call::merge { .. }
            | pallet_conditional_ledger::Call::split_scalar { .. }
            | pallet_conditional_ledger::Call::merge_scalar { .. }
            | pallet_conditional_ledger::Call::split_gate { .. }
            | pallet_conditional_ledger::Call::merge_gate { .. }
            | pallet_conditional_ledger::Call::transfer { .. }
            | pallet_conditional_ledger::Call::split_baseline { .. }
            | pallet_conditional_ledger::Call::merge_baseline { .. }
            | pallet_conditional_ledger::Call::resolve { .. }
            | pallet_conditional_ledger::Call::void { .. }
            | pallet_conditional_ledger::Call::settle_scalar { .. }
            | pallet_conditional_ledger::Call::settle_gate { .. }
            | pallet_conditional_ledger::Call::settle_baseline { .. }
            | pallet_conditional_ledger::Call::redeem { .. }
            | pallet_conditional_ledger::Call::redeem_scalar { .. }
            | pallet_conditional_ledger::Call::redeem_scalar_pair { .. }
            | pallet_conditional_ledger::Call::redeem_gate { .. }
            | pallet_conditional_ledger::Call::redeem_void { .. }
            | pallet_conditional_ledger::Call::redeem_baseline { .. }
            | pallet_conditional_ledger::Call::redeem_baseline_pair { .. }
            | pallet_conditional_ledger::Call::sweep_dust { .. }
            | pallet_conditional_ledger::Call::sweep_dust_baseline { .. } => {
                leaf(CallDomain::Public)
            }
            pallet_conditional_ledger::Call::set_split_paused { .. }
            | pallet_conditional_ledger::Call::set_frozen { .. } => {
                leaf(CallDomain::EmergencyPlaybook)
            }
            pallet_conditional_ledger::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Market(call) => match call {
            pallet_market::Call::buy { .. }
            | pallet_market::Call::sell { .. }
            | pallet_market::Call::crank_observe { .. }
            | pallet_market::Call::reap { .. } => leaf(CallDomain::Public),
            pallet_market::Call::freeze_creation { .. }
            | pallet_market::Call::set_frozen { .. } => leaf(CallDomain::EmergencyPlaybook),
            pallet_market::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Welfare(call) => match call {
            pallet_welfare::Call::register_spec { .. } => leaf(CallDomain::ConstitutionalValues),
            pallet_welfare::Call::record_snapshot { .. }
            | pallet_welfare::Call::record_daily_gate { .. } => leaf(CallDomain::Public),
            pallet_welfare::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Oracle(call) => match call {
            pallet_oracle::Call::adjudicate { .. } => leaf(CallDomain::OracleResolution),
            pallet_oracle::Call::register_reporter { .. }
            | pallet_oracle::Call::deregister_reporter { .. }
            | pallet_oracle::Call::report { .. }
            | pallet_oracle::Call::challenge { .. }
            | pallet_oracle::Call::recompute_proof { .. }
            | pallet_oracle::Call::register_watchtower { .. }
            | pallet_oracle::Call::ack_observed { .. }
            | pallet_oracle::Call::crank_round_close { .. }
            | pallet_oracle::Call::crank_reserve_probe { .. } => leaf(CallDomain::Public),
            pallet_oracle::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::IncidentRegistry(call) => match call {
            pallet_registry::Call::file { .. }
            | pallet_registry::Call::challenge_filing { .. }
            | pallet_registry::Call::ack_observed { .. }
            | pallet_registry::Call::crank_close { .. }
            | pallet_registry::Call::resolve_challenge { .. }
            | pallet_registry::Call::close_epoch { .. }
            | pallet_registry::Call::reap_epoch { .. } => leaf(CallDomain::Public),
            pallet_registry::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::MilestoneRegistry(call) => match call {
            pallet_registry::Call::file { .. }
            | pallet_registry::Call::challenge_filing { .. }
            | pallet_registry::Call::ack_observed { .. }
            | pallet_registry::Call::crank_close { .. }
            | pallet_registry::Call::resolve_challenge { .. }
            | pallet_registry::Call::close_epoch { .. }
            | pallet_registry::Call::reap_epoch { .. } => leaf(CallDomain::Public),
            pallet_registry::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::FutarchyTreasury(call) => match call {
            pallet_futarchy_treasury::Call::fund_budget_line { .. }
            | pallet_futarchy_treasury::Call::spend { .. }
            | pallet_futarchy_treasury::Call::open_stream { .. }
            | pallet_futarchy_treasury::Call::cancel_stream { .. }
            | pallet_futarchy_treasury::Call::issue_vit { .. }
            | pallet_futarchy_treasury::Call::recover_foreign { .. }
            | pallet_futarchy_treasury::Call::set_coretime_authority { .. } => {
                leaf(CallDomain::Treasury)
            }
            pallet_futarchy_treasury::Call::claim_stream { .. }
            | pallet_futarchy_treasury::Call::execute_coretime_renewal { .. }
            | pallet_futarchy_treasury::Call::note_coretime_quote { .. }
            | pallet_futarchy_treasury::Call::prune_coretime_quote { .. } => {
                leaf(CallDomain::Public)
            }
            pallet_futarchy_treasury::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Guardian(call) => match call {
            pallet_guardian::Call::set_members { .. }
            | pallet_guardian::Call::ratify_action { .. }
            | pallet_guardian::Call::renew_playbook { .. }
            | pallet_guardian::Call::uphold_veto { .. }
            | pallet_guardian::Call::recall { .. }
            | pallet_guardian::Call::set_playbook_registered { .. } => {
                leaf(CallDomain::ConstitutionalValues)
            }
            pallet_guardian::Call::propose_action { .. }
            | pallet_guardian::Call::approve_action { .. } => leaf(CallDomain::Public),
            pallet_guardian::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Attestor(call) => match call {
            pallet_attestor::Call::set_members { .. }
            | pallet_attestor::Call::resolve_challenge { .. } => {
                leaf(CallDomain::ConstitutionalValues)
            }
            pallet_attestor::Call::attest { .. }
            | pallet_attestor::Call::challenge_attestation { .. } => leaf(CallDomain::Public),
            pallet_attestor::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::Epoch(call) => match call {
            pallet_epoch::Call::submit { .. }
            | pallet_epoch::Call::withdraw { .. }
            | pallet_epoch::Call::tick { .. }
            | pallet_epoch::Call::decide { .. }
            | pallet_epoch::Call::settle_cohort { .. } => leaf(CallDomain::Public),
            pallet_epoch::Call::set_next_epoch_length { .. } => {
                leaf(CallDomain::ConstitutionalValues)
            }
            pallet_epoch::Call::delay_once { .. }
            | pallet_epoch::Call::force_reject_process_hold { .. } => {
                leaf(CallDomain::GuardianHold)
            }
            // These callbacks accept only the execution-guard sovereign
            // Signed account. They are origin-gated in-pallet and have no
            // externally constructible custom origin.
            pallet_epoch::Call::mark_executed { .. }
            | pallet_epoch::Call::mark_failed_executed { .. }
            | pallet_epoch::Call::retry_exhausted_to_measurement { .. }
            | pallet_epoch::Call::expire_or_stale_queue { .. } => leaf(CallDomain::Public),
            pallet_epoch::Call::void_cohort { .. }
            | pallet_epoch::Call::set_intake_paused { .. } => leaf(CallDomain::EmergencyPlaybook),
            pallet_epoch::Call::__Ignore(_, _) => denied(),
        },
        RuntimeCall::ExecutionGuard(call) => match call {
            pallet_execution_guard::Call::execute { .. }
            | pallet_execution_guard::Call::apply_authorized_upgrade { .. }
            | pallet_execution_guard::Call::expire_failed_execution { .. }
            | pallet_execution_guard::Call::reject_stale { .. } => leaf(CallDomain::Public),
            pallet_execution_guard::Call::ratify { .. } => leaf(CallDomain::ConstitutionalValues),
            pallet_execution_guard::Call::__Ignore(_, _) => denied(),
        },
    }
}

pub struct BleavitSafetyClassifier;

impl SafetyClassifier for BleavitSafetyClassifier {
    type Call = RuntimeCall;

    fn project(call: &Self::Call) -> FilterCall {
        project_inner(call, &mut ProjectionBudget::root())
    }
}

fn pending_upgrade_is_applicable(code: &[u8]) -> bool {
    PendingExecutionGuard::applicable_at().is_some_and(|at| System::block_number() >= at)
        && crate::configs::direct_system_upgrade_allowed(code)
}

fn guard_domain(domain: CallDomain) -> Option<pallet_execution_guard::CallDomain> {
    match domain {
        CallDomain::Public => Some(pallet_execution_guard::CallDomain::Public),
        CallDomain::Param => Some(pallet_execution_guard::CallDomain::Param),
        CallDomain::Treasury => Some(pallet_execution_guard::CallDomain::Treasury),
        CallDomain::Code => Some(pallet_execution_guard::CallDomain::Code),
        CallDomain::Meta => Some(pallet_execution_guard::CallDomain::Meta),
        CallDomain::InternalRoot => {
            Some(pallet_execution_guard::CallDomain::InternalRootApplyUpgrade)
        }
        _ => None,
    }
}

fn collect_guard_domains(
    call: &FilterCall,
    domains: &mut pallet_execution_guard::ReDerivedDomains,
    nested_calls: &mut u32,
) -> Result<(), DispatchError> {
    *nested_calls = nested_calls
        .checked_add(1)
        .ok_or(DispatchError::Other("guard nested-call count overflow"))?;
    if *nested_calls > kernel::MAX_NESTED_CALLS {
        return Err(DispatchError::Other("guard nested-call bound exceeded"));
    }
    match call {
        FilterCall::Leaf(domain) => {
            let domain = guard_domain(*domain)
                .ok_or(DispatchError::Other("guard call has forbidden domain"))?;
            if !domains.contains(&domain) {
                domains
                    .try_push(domain)
                    .map_err(|_| DispatchError::Other("guard domain bound exceeded"))?;
            }
        }
        // SQ-96 / 09 section 1.2(12): best-effort wrappers cannot provide
        // all-or-nothing execution, including when nested in batch_all.
        FilterCall::UtilityBatch(_) | FilterCall::UtilityForceBatch(_) => {
            return Err(DispatchError::Other("guard rejects best-effort wrapper"));
        }
        FilterCall::UtilityBatchAll(calls) => {
            for call in calls {
                collect_guard_domains(call, domains, nested_calls)?;
            }
        }
        FilterCall::UtilityDispatchAs(call)
        | FilterCall::UtilityAsDerivative(call)
        | FilterCall::UtilityWithWeight(call)
        | FilterCall::Proxy(call)
        | FilterCall::ProxyAnnounced(call)
        | FilterCall::MultisigAsMulti(call)
        | FilterCall::MultisigAsMultiThreshold1(call)
        | FilterCall::Sudo(call) => collect_guard_domains(&call.0, domains, nested_calls)?,
        FilterCall::Scheduler { call, .. } => {
            collect_guard_domains(&call.0, domains, nested_calls)?
        }
        FilterCall::MultisigApproveAsMulti => {}
    }
    Ok(())
}

/// Concrete execution-guard dispatcher. It reuses the same closed classifier
/// as the runtime base filter and bypasses the origin-less base filter only
/// after an origin-aware recheck has succeeded.
pub struct RuntimeDispatcher;

/// A gate breach freezes execution only while the CURRENT epoch's record shows
/// it (06 §5 auto-release; PR #66 Codex P1): a breached record retained from a
/// prior epoch (welfare keeps a rolling window and pruning is keeper-driven)
/// must not latch the freeze after recovery.
pub(crate) fn current_epoch_gate_breach() -> bool {
    let current_epoch = pallet_epoch::EpochOf::<Runtime>::get().index;
    pallet_welfare::GateBreachFlags::<Runtime>::get(current_epoch)
        .map(|flags| flags.s_breached || flags.c_breached)
        .unwrap_or(false)
}

fn live_execution_freeze() -> bool {
    let gate_breach = current_epoch_gate_breach();
    let dead_man = pallet_constitution::PhaseFlags::<Runtime>::get()
        & pallet_constitution::PhaseFlagsValue::DEAD_MAN_ENGAGED
        != 0;
    gate_breach || dead_man
}

pub(crate) fn capability_enabled_for_call(class: ProposalClass, call: &RuntimeCall) -> bool {
    <crate::configs::RuntimeCapabilities as pallet_execution_guard::Capabilities<RuntimeCall>>::call_enabled(
        class, call,
    )
}

impl pallet_execution_guard::BatchDispatcher<RuntimeCall> for RuntimeDispatcher {
    fn rederive_call(
        call: &RuntimeCall,
    ) -> Result<pallet_execution_guard::ReDerivedCall, DispatchError> {
        if Self::authorize_upgrade_hash(call).is_some() {
            // Capability enforcement deliberately does NOT live here: the guard
            // composes `rederive_call` with its own ordered `Capabilities` check
            // (09 §1.2), so folding it in would surface a disabled capability as
            // the wrong list item (`BadDomainDeclaration` instead of
            // `CapabilityDenied`).
            let domains = pallet_execution_guard::ReDerivedDomains::try_from(vec![
                pallet_execution_guard::CallDomain::InternalRootAuthorizeUpgrade,
            ])
            .map_err(|_| DispatchError::Other("guard domain bound exceeded"))?;
            return Ok(pallet_execution_guard::ReDerivedCall {
                domains,
                nested_calls: 1,
            });
        }
        let projected = BleavitSafetyClassifier::project(call);
        let mut domains = pallet_execution_guard::ReDerivedDomains::default();
        let mut nested_calls = 0;
        collect_guard_domains(&projected, &mut domains, &mut nested_calls)?;
        Ok(pallet_execution_guard::ReDerivedCall {
            domains,
            nested_calls,
        })
    }

    fn safety_filter(class: ProposalClass, call: &RuntimeCall) -> bool {
        // The guard groups this filter with `Capabilities::call_enabled` under
        // one `CapabilityDenied` check item (09 §1.2), so folding capability in
        // here keeps the reported error right; only `rederive_call` must stay
        // capability-agnostic (it maps to `BadDomainDeclaration`).
        !live_execution_freeze()
            && Self::rederive_call(call).is_ok()
            && capability_enabled_for_call(class, call)
            && ClassOrigin::from_proposal_class(class).is_some_and(|origin| {
                SafetyFilter::<BleavitSafetyClassifier>::contains_for(origin, call)
            })
    }

    fn authorize_upgrade_hash(call: &RuntimeCall) -> Option<H256> {
        let system: Option<&frame_system::Call<Runtime>> = call.is_sub_type();
        match system {
            Some(frame_system::Call::authorize_upgrade { code_hash }) => Some(code_hash.0),
            _ => None,
        }
    }

    fn dispatch_with_class_origin(
        call: RuntimeCall,
        class: ProposalClass,
    ) -> frame_support::dispatch::DispatchResult {
        if !Self::safety_filter(class, &call) {
            return Err(DispatchError::Other("guard dispatch-time safety filter"));
        }
        match call {
            RuntimeCall::Utility(pallet_utility::Call::batch_all { calls }) => {
                // `pallet-utility` normally dispatches inner calls through the
                // origin-less base filter, which deliberately rejects privileged
                // leaves. Execute the already-classified atomic wrapper here so
                // each leaf gets the same class origin and failures roll back the
                // complete nested batch (09 section 1.2(12), SQ-96).
                frame_support::storage::with_storage_layer(|| {
                    for call in calls {
                        Self::dispatch_with_class_origin(call, class)?;
                    }
                    Ok(())
                })
            }
            call => {
                let origin = pallet_origins::Origin::from_proposal_class(class)
                    .ok_or(DispatchError::BadOrigin)?;
                call.dispatch_bypass_filter(RuntimeOrigin::from(origin))
                    .map(|_| ())
                    .map_err(|error| error.error)
            }
        }
    }

    fn dispatch_authorize_upgrade(code_hash: H256) -> frame_support::dispatch::DispatchResult {
        RuntimeCall::System(frame_system::Call::authorize_upgrade {
            code_hash: code_hash.into(),
        })
        .dispatch_bypass_filter(RuntimeOrigin::root())
        .map(|_| ())
        .map_err(|error| error.error)
    }

    fn dispatch_apply_authorized_upgrade(code: Vec<u8>) -> frame_support::dispatch::DispatchResult {
        if live_execution_freeze() {
            return Err(DispatchError::Other("live execution freeze active"));
        }
        frame_support::storage::with_storage_layer(|| {
            crate::configs::parachain_upgrade_preflight(&code)?;

            // PB-MIGRATION's rollback is a forward remediation upgrade (09 §7).
            // The stock frame-system preflight rejects every non-empty MBM
            // cursor, so only an actually stuck cursor, or a still-live active
            // stall backed by the migration failure/stall sources, is retired
            // before scheduling. Unrelated alarms and resumed/healthy active
            // work do not make a cursor disposable.
            if crate::configs::retire_stuck_migration_cursor_for_remediation() {
                // retired inside the helper
            } else {
                #[cfg(not(feature = "runtime-benchmarks"))]
                System::can_set_code(&code, true).into_result()?;
            }

            RuntimeCall::System(frame_system::Call::apply_authorized_upgrade { code })
                .dispatch(RuntimeOrigin::none())
                .map(|_| ())
                .map_err(|error| error.error)
        })
    }

    fn observed_runtime_version(code: &[u8]) -> Option<RuntimeVersionConstraint> {
        let encoded = sp_io::misc::runtime_version(code)?;
        let version = sp_version::RuntimeVersion::decode(&mut &encoded[..]).ok()?;
        Some(RuntimeVersionConstraint {
            spec_name: version.spec_name.as_bytes().to_vec().try_into().ok()?,
            spec_version: version.spec_version,
        })
    }

    fn checkpoint() -> (H256, H256) {
        let parent_hash = System::parent_hash().0;
        let state_root = sp_io::storage::root(VERSION.state_version());
        #[allow(clippy::manual_unwrap_or, clippy::manual_unwrap_or_default)]
        let state_root = match <[u8; 32]>::try_from(state_root.as_slice()) {
            Ok(root) => root,
            Err(_) => [0; 32],
        };
        (parent_hash, state_root)
    }
}

/// Closed bare-leaf admission set required because the stable2606 scheduler
/// uses filtered dispatch (SQ-32 branch (i); re-verified on pallet-scheduler
/// 50.0.0 at the D-19 line move). Every call here produces a values
/// origin (ConstitutionalValues or OracleResolution) at enactment and still
/// passes its own exact `EnsureOrigin` — the second of the two independent
/// checks. Bare leaves only: the origin-blind [`Contains`] impl never admits a
/// wrapper carrying one of these.
pub fn is_values_enactment_leaf(call: &RuntimeCall) -> bool {
    // `set_param` on a CONST/entrenched key produces ConstitutionalValues (the
    // `constitution`/`entrenched` tracks, 06 §2.1/§3.2); its class-conditioned
    // domain in `project_inner` maps it to `ConstitutionalValues`, so a passed
    // values referendum enacting it must be admitted here too — otherwise the
    // origin-blind base filter rejects the scheduled dispatch before the
    // pallet's `GovernanceOrigin` check runs. PARAM/TREASURY/META keys produce
    // Futarchy* (belief-side, execution guard) origins and are deliberately
    // NOT admitted here.
    if let RuntimeCall::Constitution(pallet_constitution::Call::set_param { key, .. }) = call {
        return matches!(
            pallet_constitution::Params::<crate::Runtime>::get(key).map(|record| record.class),
            Some(
                pallet_constitution::ParamClass::Const
                    | pallet_constitution::ParamClass::Entrenched
            )
        );
    }
    matches!(
        call,
        RuntimeCall::Welfare(pallet_welfare::Call::register_spec { .. })
            // SQ-151: B1a wires ForeignAssets::CreateOrigin to
            // EnsureConstitutionalAssetCreate (EnsureConstitutionalValues).
            // Admit the bare scheduler leaf here; the pallet CreateOrigin
            // remains the independent second check required by 06 §3.3(b).
            | RuntimeCall::ForeignAssets(pallet_assets::Call::create { .. })
            // Same shape as SQ-151, surfaced by the S5 exhaustiveness suite on
            // the A8 merge: `epoch.set_next_epoch_length` is
            // ConstitutionalValues-domain and in-pallet gated by
            // `ConstitutionalValuesOrigin` (the independent second check), so
            // the bare scheduler leaf must clear the origin-blind base filter
            // or the configured values path is unreachable.
            | RuntimeCall::Epoch(pallet_epoch::Call::set_next_epoch_length { .. })
            | RuntimeCall::Constitution(pallet_constitution::Call::amend_registry { .. })
            | RuntimeCall::Constitution(pallet_constitution::Call::set_release_channel { .. })
            // `referenda.cancel`/`kill` are ConstitutionalValues-domain (the
            // runtime's `CancelOrigin`/`KillOrigin`), so a values referendum
            // enacting them must clear the origin-blind base filter — otherwise
            // the scheduler's filtered dispatch rejects `CallFiltered` before the
            // configured origin check runs, leaving both governance controls
            // unreachable (PR #57 Codex-bot P2).
            | RuntimeCall::Referenda(pallet_referenda::Call::cancel { .. })
            | RuntimeCall::Referenda(pallet_referenda::Call::kill { .. })
            | RuntimeCall::Guardian(pallet_guardian::Call::set_members { .. })
            | RuntimeCall::Guardian(pallet_guardian::Call::ratify_action { .. })
            | RuntimeCall::Guardian(pallet_guardian::Call::renew_playbook { .. })
            | RuntimeCall::Guardian(pallet_guardian::Call::uphold_veto { .. })
            | RuntimeCall::Guardian(pallet_guardian::Call::recall { .. })
            | RuntimeCall::Guardian(pallet_guardian::Call::set_playbook_registered { .. })
            | RuntimeCall::Attestor(pallet_attestor::Call::set_members { .. })
            | RuntimeCall::Attestor(pallet_attestor::Call::resolve_challenge { .. })
            | RuntimeCall::Oracle(pallet_oracle::Call::adjudicate { .. })
            | RuntimeCall::ExecutionGuard(pallet_execution_guard::Call::ratify { .. })
    )
}

/// Runtime base filter. Values enactment has two independent checks: track
/// admission in referenda and the target pallet's `EnsureOrigin`. Belief-side
/// execution retains base-filter denial and A11's origin-aware recheck.
pub struct RuntimeBaseCallFilter;

impl Contains<RuntimeCall> for RuntimeBaseCallFilter {
    fn contains(call: &RuntimeCall) -> bool {
        SafetyFilter::<BleavitSafetyClassifier>::contains(call) || is_values_enactment_leaf(call)
    }
}

impl RuntimeBaseCallFilter {
    /// Origin-aware authority-matrix check (guard step 5 / I-11; 06 §3.4
    /// scheduler re-entry). Deliberately the RAW `SafetyFilter` — the
    /// values-enactment leaf admission exists only in the origin-blind
    /// [`Contains`] impl (the SQ-32 stock-scheduler accommodation); adding it
    /// here would let `ConstitutionalValues` pass the matrix check for leaves
    /// owned by other origins (e.g. `oracle.adjudicate`, OracleResolution-only).
    pub fn contains_for(origin: ClassOrigin, call: &RuntimeCall) -> bool {
        SafetyFilter::<BleavitSafetyClassifier>::contains_for(origin, call)
    }
}
