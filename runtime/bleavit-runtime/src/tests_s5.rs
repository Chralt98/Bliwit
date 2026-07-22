//! Milestone S5 metadata-driven filter exhaustiveness and wrapper-negative suites.

use alloc::{collections::BTreeSet, format, string::String, vec, vec::Vec};

use frame_support::__private::metadata::{RuntimeMetadata, RuntimeMetadataPrefixed};
use origins_core::CallDomain;
use parity_scale_codec::{Compact, Decode, Encode};
use scale_info::{form::PortableForm, PortableRegistry, TypeDef, TypeDefPrimitive, Variant};

use crate::{Runtime, RuntimeCall};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConditionalKind {
    ParamKeyClass,
    PendingUpgrade,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum WrapperShape {
    UtilityBatch,
    UtilityAsDerivative,
    UtilityBatchAll,
    UtilityDispatchAs,
    UtilityForceBatch,
    UtilityWithWeight,
    UtilityIfElse,
    UtilityDispatchAsFallible,
    Proxy,
    ProxyAnnounced,
    MultisigAsMultiThreshold1,
    MultisigAsMulti,
    MultisigApproveAsMulti,
    SchedulerSchedule,
    SchedulerScheduleNamed,
    SchedulerScheduleAfter,
    SchedulerScheduleNamedAfter,
    Sudo,
    SudoUncheckedWeight,
    SudoAs,
}

impl WrapperShape {
    /// `approve_as_multi` is the closed-table hash-only negative control.
    pub(crate) const fn carries_call(self) -> bool {
        !matches!(self, Self::MultisigApproveAsMulti)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExpectedTreatment {
    Leaf(CallDomain),
    Conditional(ConditionalKind),
    Wrapper(WrapperShape),
    Denied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InventoryRow {
    pub(crate) pallet: &'static str,
    pub(crate) call: &'static str,
    pub(crate) expected: ExpectedTreatment,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SemanticCarrierRow {
    pub(crate) pallet: &'static str,
    pub(crate) call: &'static str,
    pub(crate) expected: ExpectedTreatment,
}

macro_rules! treatment {
    (leaf public) => {
        ExpectedTreatment::Leaf(CallDomain::Public)
    };
    (leaf param) => {
        ExpectedTreatment::Leaf(CallDomain::Param)
    };
    (leaf treasury) => {
        ExpectedTreatment::Leaf(CallDomain::Treasury)
    };
    (leaf meta) => {
        ExpectedTreatment::Leaf(CallDomain::Meta)
    };
    (leaf values) => {
        ExpectedTreatment::Leaf(CallDomain::ConstitutionalValues)
    };
    (leaf oracle_resolution) => {
        ExpectedTreatment::Leaf(CallDomain::OracleResolution)
    };
    (leaf guardian_hold) => {
        ExpectedTreatment::Leaf(CallDomain::GuardianHold)
    };
    (leaf emergency_playbook) => {
        ExpectedTreatment::Leaf(CallDomain::EmergencyPlaybook)
    };
    (leaf internal_root) => {
        ExpectedTreatment::Leaf(CallDomain::InternalRoot)
    };
    (leaf denied) => {
        ExpectedTreatment::Denied
    };
    (conditional $kind:ident) => {
        ExpectedTreatment::Conditional(ConditionalKind::$kind)
    };
    (wrapper $shape:ident) => {
        ExpectedTreatment::Wrapper(WrapperShape::$shape)
    };
}

macro_rules! inventory {
    ($( $pallet:literal { $( $kind:ident $detail:ident => [$($call:literal),+ $(,)?]; )+ } )+) => {
        pub(crate) const INVENTORY: &[InventoryRow] = &[
            $(
                $(
                    $(InventoryRow {
                        pallet: $pallet,
                        call: $call,
                        expected: treatment!($kind $detail),
                    },)+
                )+
            )+
        ];
    };
}

// This is intentionally a complete, hand-reviewed inventory rather than a
// classifier-derived list: either side changing independently trips CI.
inventory! {
    "System" {
        leaf public => ["remark", "remark_with_event"];
        leaf internal_root => ["authorize_upgrade"];
        conditional PendingUpgrade => ["apply_authorized_upgrade"];
        leaf denied => ["set_heap_pages", "set_code", "set_code_without_checks", "set_storage", "kill_storage", "kill_prefix", "authorize_upgrade_without_checks"];
    }
    "Timestamp" { leaf public => ["set"]; }
    "ParachainSystem" {
        leaf public => ["set_validation_data"];
        leaf denied => ["sudo_send_upward_message"];
    }
    "Balances" {
        leaf public => ["transfer_allow_death", "transfer_keep_alive", "transfer_all", "upgrade_accounts", "burn"];
        leaf denied => ["force_transfer", "force_unreserve", "force_set_balance", "force_adjust_total_issuance"];
    }
    "ForeignAssets" {
        leaf values => ["create"];
        leaf public => ["transfer", "transfer_keep_alive", "approve_transfer", "cancel_approval", "transfer_approved", "touch", "refund", "touch_other", "refund_other", "transfer_all"];
        leaf denied => ["force_create", "start_destroy", "destroy_accounts", "destroy_approvals", "finish_destroy", "mint", "burn", "force_transfer", "freeze", "thaw", "freeze_asset", "thaw_asset", "transfer_ownership", "set_team", "set_metadata", "clear_metadata", "force_set_metadata", "force_clear_metadata", "force_asset_status", "force_cancel_approval", "set_min_balance", "block", "set_reserves"];
    }
    "Vesting" {
        leaf public => ["vest", "vest_other", "vested_transfer", "merge_schedules"];
        leaf denied => ["force_vested_transfer", "force_remove_vesting_schedule"];
    }
    "Referenda" {
        leaf values => ["cancel", "kill"];
        leaf public => ["submit", "place_decision_deposit", "refund_decision_deposit", "nudge_referendum", "one_fewer_deciding", "refund_submission_deposit", "set_metadata"];
    }
    "ConvictionVoting" { leaf public => ["vote", "delegate", "undelegate", "unlock", "remove_vote", "remove_other_vote"]; }
    "Preimage" { leaf public => ["note_preimage", "unnote_preimage", "request_preimage", "unrequest_preimage", "ensure_updated"]; }
    "Scheduler" {
        wrapper SchedulerSchedule => ["schedule"];
        wrapper SchedulerScheduleNamed => ["schedule_named"];
        wrapper SchedulerScheduleAfter => ["schedule_after"];
        wrapper SchedulerScheduleNamedAfter => ["schedule_named_after"];
        leaf denied => ["cancel", "cancel_named", "set_retry", "set_retry_named", "cancel_retry", "cancel_retry_named"];
    }
    "Utility" {
        wrapper UtilityBatch => ["batch"];
        wrapper UtilityAsDerivative => ["as_derivative"];
        wrapper UtilityBatchAll => ["batch_all"];
        wrapper UtilityDispatchAs => ["dispatch_as"];
        wrapper UtilityForceBatch => ["force_batch"];
        wrapper UtilityWithWeight => ["with_weight"];
        wrapper UtilityIfElse => ["if_else"];
        wrapper UtilityDispatchAsFallible => ["dispatch_as_fallible"];
    }
    "Proxy" {
        wrapper Proxy => ["proxy"];
        wrapper ProxyAnnounced => ["proxy_announced"];
        leaf public => ["add_proxy", "remove_proxy", "remove_proxies", "create_pure", "kill_pure", "announce", "remove_announcement", "reject_announcement", "poke_deposit"];
    }
    "Multisig" {
        wrapper MultisigAsMultiThreshold1 => ["as_multi_threshold_1"];
        wrapper MultisigAsMulti => ["as_multi"];
        wrapper MultisigApproveAsMulti => ["approve_as_multi"];
        leaf public => ["cancel_as_multi", "poke_deposit"];
    }
    "Migrations" { leaf denied => ["force_set_cursor", "force_set_active_cursor", "force_onboard_mbms", "clear_historic"]; }
    "Sudo" {
        wrapper Sudo => ["sudo"];
        wrapper SudoUncheckedWeight => ["sudo_unchecked_weight"];
        wrapper SudoAs => ["sudo_as"];
        leaf public => ["set_key", "remove_key"];
    }
    "XcmpQueue" { leaf public => ["suspend_xcm_execution", "resume_xcm_execution", "update_suspend_threshold", "update_drop_threshold", "update_resume_threshold"]; }
    "MessageQueue" { leaf public => ["reap_page", "execute_overweight"]; }
    "PolkadotXcm" {
        leaf public => ["claim_assets"];
        leaf denied => ["send", "teleport_assets", "reserve_transfer_assets", "execute", "force_xcm_version", "force_default_xcm_version", "force_subscribe_version_notify", "force_unsubscribe_version_notify", "limited_reserve_transfer_assets", "limited_teleport_assets", "force_suspension", "transfer_assets", "transfer_assets_using_type_and_then", "add_authorized_alias", "remove_authorized_alias", "remove_all_authorized_aliases"];
    }
    "CollatorSelection" { leaf public => ["set_invulnerables", "set_desired_candidates", "set_candidacy_bond", "register_as_candidate", "leave_intent", "add_invulnerable", "remove_invulnerable", "update_bond", "take_candidate_slot"]; }
    "Session" { leaf public => ["set_keys", "purge_keys"]; }
    "Constitution" {
        conditional ParamKeyClass => ["set_param"];
        // SQ-150 (ruled 2026-07-21): amend_registry is a plain FutarchyMeta
        // leaf — non-kernel rows META-only, kernel rows immutable at dispatch —
        // no longer a contested scope conditional.
        leaf meta => ["set_capability", "amend_registry"];
        leaf public => ["set_phase_flag"];
        leaf values => ["set_release_channel"];
    }
    "ConditionalLedger" {
        leaf public => ["split", "merge", "split_scalar", "merge_scalar", "split_gate", "merge_gate", "transfer", "split_baseline", "merge_baseline", "resolve", "void", "settle_scalar", "settle_gate", "settle_baseline", "redeem", "redeem_scalar", "redeem_scalar_pair", "redeem_gate", "redeem_void", "redeem_baseline", "redeem_baseline_pair", "sweep_dust", "sweep_dust_baseline"];
        leaf emergency_playbook => ["set_split_paused", "set_frozen"];
    }
    "Market" {
        leaf public => ["buy", "sell", "crank_observe", "reap"];
        leaf emergency_playbook => ["freeze_creation", "set_frozen"];
    }
    "Welfare" {
        leaf values => ["register_spec"];
        leaf public => ["record_snapshot", "record_daily_gate"];
    }
    "Oracle" {
        leaf oracle_resolution => ["adjudicate"];
        leaf public => ["register_reporter", "deregister_reporter", "report", "challenge", "recompute_proof", "register_watchtower", "ack_observed", "crank_round_close", "crank_reserve_probe"];
    }
    "IncidentRegistry" { leaf public => ["file", "challenge_filing", "ack_observed", "crank_close", "resolve_challenge", "close_epoch", "reap_epoch"]; }
    "MilestoneRegistry" { leaf public => ["file", "challenge_filing", "ack_observed", "crank_close", "resolve_challenge", "close_epoch", "reap_epoch"]; }
    "FutarchyTreasury" {
        leaf treasury => ["fund_budget_line", "spend", "open_stream", "cancel_stream", "issue_vit", "recover_foreign", "set_coretime_authority"];
        leaf public => ["claim_stream", "execute_coretime_renewal", "note_coretime_quote", "prune_coretime_quote"];
    }
    "Guardian" {
        leaf values => ["set_members", "ratify_action", "renew_playbook", "uphold_veto", "recall", "set_playbook_registered"];
        leaf public => ["propose_action", "approve_action"];
    }
    "Attestor" {
        leaf values => ["set_members", "resolve_challenge"];
        leaf public => ["attest", "challenge_attestation"];
    }
    "Epoch" {
        leaf public => ["submit", "withdraw", "tick", "decide", "settle_cohort", "mark_executed", "mark_failed_executed", "retry_exhausted_to_measurement", "expire_or_stale_queue"];
        leaf values => ["set_next_epoch_length"];
        leaf guardian_hold => ["delay_once", "force_reject_process_hold"];
        leaf emergency_playbook => ["void_cohort", "set_intake_paused"];
    }
    "ExecutionGuard" {
        leaf values => ["ratify"];
        leaf public => ["execute", "apply_authorized_upgrade", "expire_failed_execution", "reject_stale"];
    }
}

// Metadata can expose a `RuntimeCall`-carrying type even when the call is not
// a wrapper that SafetyFilter recursively dispatches. Keep those semantic
// carriers independently pinned so a new proposal/execution surface cannot be
// mistaken for an ordinary leaf merely because it is not in the wrapper table.
pub(crate) const SEMANTIC_CARRIERS: &[SemanticCarrierRow] = &[
    // Public BY DESIGN: the bounded proposal is not dispatched by `submit`.
    // Enactment later runs through the scheduler with the track origin and
    // re-enters the filter (06 §2.1 / §3.4).
    SemanticCarrierRow {
        pallet: "Referenda",
        call: "submit",
        expected: ExpectedTreatment::Leaf(CallDomain::Public),
    },
    // The locally executable XCM program is an opaque call-bearing carrier in
    // metadata and remains the pinned Nobody treatment in this runtime.
    SemanticCarrierRow {
        pallet: "PolkadotXcm",
        call: "execute",
        expected: ExpectedTreatment::Denied,
    },
];

// These generic carriers deliberately omit or obscure their call parameter in
// some scale-info graphs. Name detection is a second tripwire in addition to
// ordinary type reachability; additions require an explicit reviewed pin.
const OPAQUE_SEMANTIC_CARRIER_TYPE_NAMES: &[&str] = &["DoubleEncoded", "VersionedXcm"];
const EXPLICIT_UNIT_OPAQUE_TYPE_ARGUMENTS: &[&str] = &["DoubleEncoded<()>", "VersionedXcm<()>"];

#[derive(Debug)]
struct PalletCalls {
    name: String,
    index: u8,
    call_ty: u32,
}

#[derive(Debug)]
pub(crate) struct RuntimeMetadataModel {
    registry: PortableRegistry,
    pallets: Vec<PalletCalls>,
    runtime_call_ty: u32,
}

impl RuntimeMetadataModel {
    pub(crate) fn load() -> Self {
        let version = Runtime::metadata_versions()
            .into_iter()
            .filter(|version| matches!(version, 15 | 16))
            .max()
            .expect("stable2606 must expose V15 or V16 metadata");
        let encoded = Runtime::metadata_at_version(version)
            .expect("a reported metadata version must be constructible");
        let prefixed = RuntimeMetadataPrefixed::decode(&mut &encoded[..])
            .expect("runtime-generated metadata must decode");
        match prefixed.1 {
            RuntimeMetadata::V15(metadata) => Self {
                runtime_call_ty: metadata.outer_enums.call_enum_ty.id,
                pallets: metadata
                    .pallets
                    .into_iter()
                    .filter_map(|pallet| {
                        pallet.calls.map(|calls| PalletCalls {
                            name: pallet.name,
                            index: pallet.index,
                            call_ty: calls.ty.id,
                        })
                    })
                    .collect(),
                registry: metadata.types,
            },
            RuntimeMetadata::V16(metadata) => Self {
                runtime_call_ty: metadata.outer_enums.call_enum_ty.id,
                pallets: metadata
                    .pallets
                    .into_iter()
                    .filter_map(|pallet| {
                        pallet.calls.map(|calls| PalletCalls {
                            name: pallet.name,
                            index: pallet.index,
                            call_ty: calls.ty.id,
                        })
                    })
                    .collect(),
                registry: metadata.types,
            },
            metadata => panic!(
                "requested V{version}, but runtime returned V{}",
                metadata.version()
            ),
        }
    }

    fn call_variant<'a>(
        &'a self,
        row: &InventoryRow,
    ) -> (&'a PalletCalls, &'a Variant<PortableForm>) {
        let pallet = self
            .pallets
            .iter()
            .find(|pallet| pallet.name == row.pallet)
            .unwrap_or_else(|| panic!("inventory pallet is absent from metadata: {}", row.pallet));
        let call_ty = self.registry.resolve(pallet.call_ty).unwrap_or_else(|| {
            panic!(
                "metadata type {} for pallet {} is absent from the registry",
                pallet.call_ty, pallet.name
            )
        });
        let TypeDef::Variant(calls) = &call_ty.type_def else {
            panic!("metadata call type for {} is not an enum", pallet.name);
        };
        let variant = calls
            .variants
            .iter()
            .find(|variant| variant.name == row.call)
            .unwrap_or_else(|| {
                panic!(
                    "inventory call is absent from metadata: {}.{}",
                    row.pallet, row.call
                )
            });
        (pallet, variant)
    }

    pub(crate) fn materialize(&self, row: &InventoryRow) -> RuntimeCall {
        let (pallet, variant) = self.call_variant(row);
        let mut encoded = vec![pallet.index, variant.index];
        for field in &variant.fields {
            encoded.extend(
                self.synthesize_type(field.ty.id, &mut BTreeSet::new())
                    .unwrap_or_else(|error| {
                        panic!(
                            "cannot synthesize {}.{} field type {}: {error}",
                            row.pallet, row.call, field.ty.id
                        )
                    }),
            );
        }
        let mut input = &encoded[..];
        let call = RuntimeCall::decode(&mut input).unwrap_or_else(|error| {
            panic!(
                "synthesized {}.{} does not decode as RuntimeCall: {error}; bytes={encoded:?}",
                row.pallet, row.call
            )
        });
        assert!(
            input.is_empty(),
            "synthesized {}.{} left {} undecoded bytes",
            row.pallet,
            row.call,
            input.len()
        );
        call
    }

    pub(crate) fn call_name(&self, call: &RuntimeCall) -> (String, String) {
        let encoded = call.encode();
        let pallet_index = *encoded
            .first()
            .expect("a RuntimeCall encoding contains its pallet index");
        let call_index = *encoded
            .get(1)
            .expect("a RuntimeCall encoding contains its variant index");
        let pallet = self
            .pallets
            .iter()
            .find(|pallet| pallet.index == pallet_index)
            .unwrap_or_else(|| panic!("metadata has no call pallet at index {pallet_index}"));
        let call_ty = self
            .registry
            .resolve(pallet.call_ty)
            .unwrap_or_else(|| panic!("metadata call type for {} is absent", pallet.name));
        let TypeDef::Variant(calls) = &call_ty.type_def else {
            panic!("metadata call type for {} is not an enum", pallet.name);
        };
        let variant = calls
            .variants
            .iter()
            .find(|variant| variant.index == call_index)
            .unwrap_or_else(|| {
                panic!(
                    "metadata pallet {} has no call at index {call_index}",
                    pallet.name
                )
            });
        (pallet.name.clone(), variant.name.clone())
    }

    fn synthesize_type(&self, id: u32, visiting: &mut BTreeSet<u32>) -> Result<Vec<u8>, String> {
        if id == self.runtime_call_ty {
            return Ok(
                RuntimeCall::System(frame_system::Call::remark { remark: Vec::new() }).encode(),
            );
        }
        if !visiting.insert(id) {
            return Err(format!(
                "recursive metadata type {id} has no finite base case"
            ));
        }
        let ty = self
            .registry
            .resolve(id)
            .ok_or_else(|| format!("metadata registry has no type {id}"))?;
        let result = match &ty.type_def {
            TypeDef::Composite(composite) => {
                let mut encoded = Vec::new();
                for field in &composite.fields {
                    encoded.extend(self.synthesize_type(field.ty.id, visiting)?);
                }
                Ok(encoded)
            }
            TypeDef::Variant(variants) => {
                let mut last_error = None;
                let mut encoded = None;
                for variant in &variants.variants {
                    let mut branch_visiting = visiting.clone();
                    let mut candidate = vec![variant.index];
                    let fields = variant.fields.iter().try_for_each(|field| {
                        self.synthesize_type(field.ty.id, &mut branch_visiting)
                            .map(|field| candidate.extend(field))
                    });
                    match fields {
                        Ok(()) => {
                            encoded = Some(candidate);
                            break;
                        }
                        Err(error) => last_error = Some(error),
                    }
                }
                encoded.ok_or_else(|| {
                    last_error.unwrap_or_else(|| format!("variant type {id} has no variants"))
                })
            }
            TypeDef::Sequence(sequence) => {
                if self.type_reaches_runtime_call(sequence.type_param.id) {
                    let mut encoded = Compact(1u32).encode();
                    encoded.extend(self.synthesize_type(sequence.type_param.id, visiting)?);
                    Ok(encoded)
                } else {
                    Ok(Compact(0u32).encode())
                }
            }
            TypeDef::Array(array) => {
                let mut encoded = Vec::new();
                for _ in 0..array.len {
                    encoded.extend(self.synthesize_type(array.type_param.id, visiting)?);
                }
                Ok(encoded)
            }
            TypeDef::Tuple(tuple) => {
                let mut encoded = Vec::new();
                for field in &tuple.fields {
                    encoded.extend(self.synthesize_type(field.id, visiting)?);
                }
                Ok(encoded)
            }
            TypeDef::Primitive(primitive) => Ok(encode_primitive(primitive.clone())),
            TypeDef::Compact(_) | TypeDef::BitSequence(_) => Ok(vec![0]),
        };
        visiting.remove(&id);
        result
    }

    pub(crate) fn call_carrying_variants(&self) -> BTreeSet<(String, String)> {
        let mut carrying = BTreeSet::new();
        for pallet in &self.pallets {
            let Some(call_ty) = self.registry.resolve(pallet.call_ty) else {
                panic!("metadata call type for {} is absent", pallet.name);
            };
            let TypeDef::Variant(calls) = &call_ty.type_def else {
                panic!("metadata call type for {} is not an enum", pallet.name);
            };
            for variant in &calls.variants {
                if self.variant_reaches_runtime_call(variant)
                    || self.variant_reaches_opaque_runtime_call_carrier(variant)
                {
                    carrying.insert((pallet.name.clone(), variant.name.clone()));
                }
            }
        }
        carrying
    }

    fn variant_reaches_runtime_call(&self, variant: &Variant<PortableForm>) -> bool {
        variant
            .fields
            .iter()
            .any(|field| self.type_reaches_runtime_call(field.ty.id))
    }

    fn variant_reaches_opaque_runtime_call_carrier(&self, variant: &Variant<PortableForm>) -> bool {
        variant.fields.iter().any(|field| {
            // `VersionedXcm<()>` also appears on remote-send surfaces. The
            // originating metadata field retains that explicit unit generic,
            // even when the portable graph drops it. Exclude only that pinned
            // non-local specialization; aliases and every other opaque use
            // remain fail-loud until they receive a semantic-carrier row.
            !field.type_name.as_deref().is_some_and(|name| {
                EXPLICIT_UNIT_OPAQUE_TYPE_ARGUMENTS
                    .iter()
                    .any(|unit| name.contains(unit))
            }) && self.type_reaches_opaque_semantic_carrier(field.ty.id)
        })
    }

    fn type_reaches_runtime_call(&self, root: u32) -> bool {
        fn visit(model: &RuntimeMetadataModel, id: u32, visited: &mut BTreeSet<u32>) -> bool {
            if id == model.runtime_call_ty {
                return true;
            }
            if !visited.insert(id) {
                return false;
            }
            let Some(ty) = model.registry.resolve(id) else {
                return false;
            };
            if ty
                .type_params
                .iter()
                .filter_map(|param| param.ty)
                .any(|param| visit(model, param.id, visited))
            {
                return true;
            }
            match &ty.type_def {
                TypeDef::Composite(composite) => composite
                    .fields
                    .iter()
                    .any(|field| visit(model, field.ty.id, visited)),
                TypeDef::Variant(variants) => variants.variants.iter().any(|variant| {
                    variant
                        .fields
                        .iter()
                        .any(|field| visit(model, field.ty.id, visited))
                }),
                TypeDef::Sequence(sequence) => visit(model, sequence.type_param.id, visited),
                TypeDef::Array(array) => visit(model, array.type_param.id, visited),
                TypeDef::Tuple(tuple) => tuple
                    .fields
                    .iter()
                    .any(|field| visit(model, field.id, visited)),
                TypeDef::Compact(compact) => visit(model, compact.type_param.id, visited),
                TypeDef::BitSequence(bits) => {
                    visit(model, bits.bit_store_type.id, visited)
                        || visit(model, bits.bit_order_type.id, visited)
                }
                TypeDef::Primitive(_) => false,
            }
        }
        visit(self, root, &mut BTreeSet::new())
    }

    fn type_reaches_opaque_semantic_carrier(&self, root: u32) -> bool {
        fn visit(model: &RuntimeMetadataModel, id: u32, visited: &mut BTreeSet<u32>) -> bool {
            if !visited.insert(id) {
                return false;
            }
            let Some(ty) = model.registry.resolve(id) else {
                return false;
            };
            if ty
                .path
                .segments
                .last()
                .is_some_and(|name| OPAQUE_SEMANTIC_CARRIER_TYPE_NAMES.contains(&name.as_str()))
            {
                return true;
            }
            if ty
                .type_params
                .iter()
                .filter_map(|param| param.ty)
                .any(|param| visit(model, param.id, visited))
            {
                return true;
            }
            match &ty.type_def {
                TypeDef::Composite(composite) => composite
                    .fields
                    .iter()
                    .any(|field| visit(model, field.ty.id, visited)),
                TypeDef::Variant(variants) => variants.variants.iter().any(|variant| {
                    variant
                        .fields
                        .iter()
                        .any(|field| visit(model, field.ty.id, visited))
                }),
                TypeDef::Sequence(sequence) => visit(model, sequence.type_param.id, visited),
                TypeDef::Array(array) => visit(model, array.type_param.id, visited),
                TypeDef::Tuple(tuple) => tuple
                    .fields
                    .iter()
                    .any(|field| visit(model, field.id, visited)),
                TypeDef::Compact(compact) => visit(model, compact.type_param.id, visited),
                TypeDef::BitSequence(bits) => {
                    visit(model, bits.bit_store_type.id, visited)
                        || visit(model, bits.bit_order_type.id, visited)
                }
                TypeDef::Primitive(_) => false,
            }
        }
        visit(self, root, &mut BTreeSet::new())
    }
}

fn encode_primitive(primitive: TypeDefPrimitive) -> Vec<u8> {
    match primitive {
        TypeDefPrimitive::Bool | TypeDefPrimitive::U8 | TypeDefPrimitive::I8 => vec![0],
        TypeDefPrimitive::U16 | TypeDefPrimitive::I16 => vec![0; 2],
        TypeDefPrimitive::Char | TypeDefPrimitive::U32 | TypeDefPrimitive::I32 => vec![0; 4],
        TypeDefPrimitive::U64 | TypeDefPrimitive::I64 => vec![0; 8],
        TypeDefPrimitive::U128 | TypeDefPrimitive::I128 => vec![0; 16],
        TypeDefPrimitive::U256 | TypeDefPrimitive::I256 => vec![0; 32],
        TypeDefPrimitive::Str => vec![0],
    }
}

fn with_metadata(test: impl FnOnce(&RuntimeMetadataModel)) {
    sp_io::TestExternalities::default().execute_with(|| test(&RuntimeMetadataModel::load()));
}

#[test]
fn track_origins_is_pinned_as_origin_only_pallet_at_index_64() {
    sp_io::TestExternalities::default().execute_with(|| {
        let version = Runtime::metadata_versions()
            .into_iter()
            .filter(|version| matches!(version, 15 | 16))
            .max()
            .expect("stable2603 must expose V15 or V16 metadata");
        let encoded = Runtime::metadata_at_version(version)
            .expect("a reported metadata version must be constructible");
        let prefixed = RuntimeMetadataPrefixed::decode(&mut &encoded[..])
            .expect("runtime-generated metadata must decode");
        let pin = match prefixed.1 {
            RuntimeMetadata::V15(metadata) => metadata
                .pallets
                .iter()
                .find(|pallet| pallet.name == "TrackOrigins")
                .map(|pallet| (pallet.index, pallet.calls.is_none())),
            RuntimeMetadata::V16(metadata) => metadata
                .pallets
                .iter()
                .find(|pallet| pallet.name == "TrackOrigins")
                .map(|pallet| (pallet.index, pallet.calls.is_none())),
            metadata => panic!(
                "requested V{version}, but runtime returned V{}",
                metadata.version()
            ),
        };
        assert_eq!(pin, Some((64, true)));
    });
}

#[test]
fn metadata_call_inventory_is_bidirectionally_exhaustive() {
    with_metadata(|metadata| {
        let mut actual = BTreeSet::new();
        for pallet in &metadata.pallets {
            let call_ty = metadata
                .registry
                .resolve(pallet.call_ty)
                .unwrap_or_else(|| panic!("metadata call type for {} is absent", pallet.name));
            let TypeDef::Variant(calls) = &call_ty.type_def else {
                panic!("metadata call type for {} is not an enum", pallet.name);
            };
            for variant in &calls.variants {
                actual.insert((pallet.name.clone(), variant.name.clone()));
            }
        }

        let mut pinned = BTreeSet::new();
        for row in INVENTORY {
            assert!(
                pinned.insert((String::from(row.pallet), String::from(row.call))),
                "duplicate S5 inventory row: {}.{}",
                row.pallet,
                row.call
            );
        }
        if let Some(missing) = actual.difference(&pinned).next() {
            panic!(
                "runtime metadata call has no S5 classification inventory row: {}.{}",
                missing.0, missing.1
            );
        }
        if let Some(stale) = pinned.difference(&actual).next() {
            panic!(
                "stale S5 classification inventory row has no runtime metadata call: {}.{}",
                stale.0, stale.1
            );
        }
        assert_eq!(pinned, actual);
    });
}

#[test]
fn metadata_call_carriers_equal_the_pinned_closed_wrapper_set() {
    with_metadata(|metadata| {
        let detected = metadata.call_carrying_variants();
        let mut pinned: BTreeSet<_> = INVENTORY
            .iter()
            .filter_map(|row| match row.expected {
                ExpectedTreatment::Wrapper(shape) if shape.carries_call() => {
                    Some((String::from(row.pallet), String::from(row.call)))
                }
                _ => None,
            })
            .collect();
        for carrier in SEMANTIC_CARRIERS {
            let inventory_row = INVENTORY
                .iter()
                .find(|row| row.pallet == carrier.pallet && row.call == carrier.call)
                .unwrap_or_else(|| {
                    panic!(
                        "semantic carrier has no classification inventory row: {}.{}",
                        carrier.pallet, carrier.call
                    )
                });
            assert_eq!(
                inventory_row.expected, carrier.expected,
                "semantic-carrier treatment drift for {}.{}",
                carrier.pallet, carrier.call
            );
            assert!(
                pinned.insert((String::from(carrier.pallet), String::from(carrier.call))),
                "semantic carrier duplicates a call-carrying wrapper row: {}.{}",
                carrier.pallet,
                carrier.call
            );
        }
        let unexpected: Vec<_> = detected.difference(&pinned).collect();
        assert!(
            unexpected.is_empty(),
            "metadata found unpinned RuntimeCall/semantic carriers: {unexpected:?}"
        );
        let stale: Vec<_> = pinned.difference(&detected).collect();
        assert!(
            stale.is_empty(),
            "pinned call carriers are no longer detected in metadata: {stale:?}"
        );
        assert_eq!(detected, pinned);

        let referenda_submit = INVENTORY
            .iter()
            .find(|row| row.pallet == "Referenda" && row.call == "submit")
            .expect("referenda.submit is pinned");
        let (_, referenda_submit) = metadata.call_variant(referenda_submit);
        assert!(metadata.variant_reaches_runtime_call(referenda_submit));

        let xcm_execute = INVENTORY
            .iter()
            .find(|row| row.pallet == "PolkadotXcm" && row.call == "execute")
            .expect("PolkadotXcm.execute is pinned");
        let (_, xcm_execute) = metadata.call_variant(xcm_execute);
        assert!(!metadata.variant_reaches_runtime_call(xcm_execute));
        assert!(metadata.variant_reaches_opaque_runtime_call_carrier(xcm_execute));

        for remote_unit_carrier in ["send", "transfer_assets_using_type_and_then"] {
            let row = INVENTORY
                .iter()
                .find(|row| row.pallet == "PolkadotXcm" && row.call == remote_unit_carrier)
                .expect("remote XCM call is pinned");
            let (_, variant) = metadata.call_variant(row);
            assert!(variant
                .fields
                .iter()
                .any(|field| { metadata.type_reaches_opaque_semantic_carrier(field.ty.id) }));
            assert!(!metadata.variant_reaches_opaque_runtime_call_carrier(variant));
        }

        assert!(INVENTORY.iter().any(|row| {
            row.pallet == "Multisig"
                && row.call == "approve_as_multi"
                && matches!(
                    row.expected,
                    ExpectedTreatment::Wrapper(WrapperShape::MultisigApproveAsMulti)
                )
        }));
        assert!(!detected.contains(&(String::from("Multisig"), String::from("approve_as_multi"))));
    });
}

#[test]
fn every_inventory_row_materializes_as_a_real_runtime_call() {
    with_metadata(|metadata| {
        for row in INVENTORY {
            let _ = metadata.materialize(row);
        }
    });
}
