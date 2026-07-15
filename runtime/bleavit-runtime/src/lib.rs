#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::{vec, vec::Vec};
use futarchy_primitives::{
    chain_identity, currency, kernel, Balance, BlockNumber, ParamKey, H256,
    INTEGRATION_CONTRACT_VERSION,
};
// The B1 frame-free composition model consumes the frame-free origins core
// directly; A4 turned `pallet-origins` into a real `#[frame_support::pallet]`
// whose `Origin`/`SafetyFilter` are the FRAME surface (B1a wires those).
use origins_core::{CallDomain, Origin, RuntimeCall, SafetyFilter};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub const MILLISECS_PER_BLOCK: u64 = kernel::MILLISECS_PER_BLOCK;
pub const SS58_PREFIX: u16 = chain_identity::SS58_PREFIX;
pub const RUNTIME_SPEC_NAME: &[u8] = b"bleavit";
pub const RUNTIME_IMPL_NAME: &[u8] = b"bleavit-runtime";
pub const RUNTIME_SPEC_VERSION: u32 = 1;
pub const TRANSACTION_VERSION: u32 = INTEGRATION_CONTRACT_VERSION;
pub const VIT_DECIMALS: u8 = currency::VIT_DECIMALS;
pub const USDC_DECIMALS: u8 = currency::USDC_DECIMALS;
pub const USDC_ASSET_ID: u32 = 1;
/// Compact model identifier for `Location { parents: 1, X3(Parachain(1000), PalletInstance(50), GeneralIndex(1337)) }`.
pub const USDC_LOCATION: [u8; 32] = [
    1, 0, 0, 0, 232, 3, 0, 0, 50, 0, 0, 0, 57, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0,
];
pub const FEE_VIT_USDC_RATE_KEY: ParamKey = *b"fee.vit_usdc_rat";

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum RuntimePallet {
    System,
    Timestamp,
    Balances,
    ForeignAssets,
    TransactionPayment,
    AssetTxPayment,
    Referenda,
    ConvictionVoting,
    Preimage,
    Scheduler,
    Utility,
    Proxy,
    Multisig,
    Migrations,
    MetadataHashExtension,
    Sudo,
    ParachainSystem,
    XcmpQueue,
    MessageQueue,
    Xcm,
    CollatorSelection,
    Session,
    Aura,
    Authorship,
    Constitution,
    ConditionalLedger,
    Market,
    Epoch,
    Welfare,
    Oracle,
    Registry,
    ExecutionGuard,
    FutarchyTreasury,
    Guardian,
    Attestor,
    Origins,
}

pub const STANDARD_PALLETS: &[RuntimePallet] = &[
    RuntimePallet::System,
    RuntimePallet::Timestamp,
    RuntimePallet::Balances,
    RuntimePallet::ForeignAssets,
    RuntimePallet::TransactionPayment,
    RuntimePallet::AssetTxPayment,
    RuntimePallet::Referenda,
    RuntimePallet::ConvictionVoting,
    RuntimePallet::Preimage,
    RuntimePallet::Scheduler,
    RuntimePallet::Utility,
    RuntimePallet::Proxy,
    RuntimePallet::Multisig,
    RuntimePallet::Migrations,
    RuntimePallet::MetadataHashExtension,
    RuntimePallet::Sudo,
    RuntimePallet::ParachainSystem,
    RuntimePallet::XcmpQueue,
    RuntimePallet::MessageQueue,
    RuntimePallet::Xcm,
    RuntimePallet::CollatorSelection,
    RuntimePallet::Session,
    RuntimePallet::Aura,
    RuntimePallet::Authorship,
];

pub const CUSTOM_PALLETS: &[RuntimePallet] = &[
    RuntimePallet::Constitution,
    RuntimePallet::ConditionalLedger,
    RuntimePallet::Market,
    RuntimePallet::Epoch,
    RuntimePallet::Welfare,
    RuntimePallet::Oracle,
    RuntimePallet::Registry,
    RuntimePallet::ExecutionGuard,
    RuntimePallet::FutarchyTreasury,
    RuntimePallet::Guardian,
    RuntimePallet::Attestor,
    RuntimePallet::Origins,
];

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum SystemCall {
    Remark,
    SetHeapPages,
    SetCode,
    SetCodeWithoutChecks,
    SetStorage,
    KillStorage,
    KillPrefix,
    AuthorizeUpgrade,
    AuthorizeUpgradeWithoutChecks,
    ApplyAuthorizedUpgrade,
}

impl SystemCall {
    pub const fn domain(self) -> CallDomain {
        match self {
            Self::Remark => CallDomain::Public,
            Self::AuthorizeUpgrade => CallDomain::InternalRoot,
            Self::ApplyAuthorizedUpgrade => CallDomain::Public,
            Self::SetHeapPages
            | Self::SetCode
            | Self::SetCodeWithoutChecks
            | Self::SetStorage
            | Self::KillStorage
            | Self::KillPrefix
            | Self::AuthorizeUpgradeWithoutChecks => CallDomain::Nobody,
        }
    }
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub enum BleavitCall {
    System(SystemCall),
    Domain(CallDomain),
    Wrapped(RuntimeCall),
}

impl BleavitCall {
    pub fn as_filter_call(&self) -> RuntimeCall {
        match self {
            Self::System(call) => RuntimeCall::leaf(call.domain()),
            Self::Domain(domain) => RuntimeCall::leaf(*domain),
            Self::Wrapped(call) => call.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct PendingUpgrade {
    pub code_hash: H256,
    pub authorized_at: BlockNumber,
    pub applicable_at: BlockNumber,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum RuntimeFilterError {
    SafetyFilter,
    PendingUpgradeMissing,
    PendingUpgradeTooEarly,
}

pub struct BaseCallFilter;

impl BaseCallFilter {
    pub fn contains(call: &BleavitCall) -> bool {
        SafetyFilter::contains(&call.as_filter_call())
    }

    pub fn contains_for(origin: Origin, call: &BleavitCall) -> bool {
        SafetyFilter::contains_for(origin, &call.as_filter_call())
    }

    pub fn validate_at(
        origin: Option<Origin>,
        call: &BleavitCall,
        now: BlockNumber,
        pending_upgrade: Option<PendingUpgrade>,
    ) -> Result<(), RuntimeFilterError> {
        let filter_call = call.as_filter_call();
        match origin {
            Some(origin) if !SafetyFilter::contains_for(origin, &filter_call) => {
                return Err(RuntimeFilterError::SafetyFilter);
            }
            None if !SafetyFilter::contains(&filter_call) => {
                return Err(RuntimeFilterError::SafetyFilter);
            }
            _ => {}
        }

        if matches!(
            call,
            BleavitCall::System(SystemCall::ApplyAuthorizedUpgrade)
        ) {
            let pending = pending_upgrade.ok_or(RuntimeFilterError::PendingUpgradeMissing)?;
            if now < pending.applicable_at {
                return Err(RuntimeFilterError::PendingUpgradeTooEarly);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum FeeAsset {
    Vit,
    Usdc,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct FeeConfig {
    pub native_asset: FeeAsset,
    pub foreign_fee_asset: u32,
    pub conversion_rate_key: ParamKey,
}

impl Default for FeeConfig {
    fn default() -> Self {
        Self {
            native_asset: FeeAsset::Vit,
            foreign_fee_asset: USDC_ASSET_ID,
            conversion_rate_key: FEE_VIT_USDC_RATE_KEY,
        }
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct ForeignAssetConfig {
    pub asset_id: u32,
    pub location: [u8; 32],
    pub decimals: u8,
    pub is_sufficient: bool,
    pub admin_origin: Origin,
    pub mint_burn_enabled: bool,
}

impl Default for ForeignAssetConfig {
    fn default() -> Self {
        Self {
            asset_id: USDC_ASSET_ID,
            location: USDC_LOCATION,
            decimals: USDC_DECIMALS,
            is_sufficient: true,
            admin_origin: Origin::ConstitutionalValues,
            mint_burn_enabled: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum RuntimeFilter {
    SafetyFilter,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct RuntimeConfig {
    pub millisecs_per_block: u64,
    pub ss58_prefix: u16,
    pub base_call_filter: RuntimeFilter,
    pub usdc: ForeignAssetConfig,
    pub fees: FeeConfig,
    pub sudo_phase_enabled: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            millisecs_per_block: MILLISECS_PER_BLOCK,
            ss58_prefix: SS58_PREFIX,
            base_call_filter: RuntimeFilter::SafetyFilter,
            usdc: ForeignAssetConfig::default(),
            fees: FeeConfig::default(),
            sudo_phase_enabled: true,
        }
    }
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct GenesisConfig {
    pub endowed_vit: Vec<([u8; 32], Balance)>,
    pub usdc_asset: ForeignAssetConfig,
    pub sudo: Option<[u8; 32]>,
    pub bootstrap_calls: Vec<BleavitCall>,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum GenesisError {
    FilteredBootstrapCall,
    UsdcMisconfigured,
}

impl GenesisConfig {
    pub fn validate(&self) -> Result<(), GenesisError> {
        let expected = ForeignAssetConfig::default();
        if self.usdc_asset != expected {
            return Err(GenesisError::UsdcMisconfigured);
        }
        for call in &self.bootstrap_calls {
            if !BaseCallFilter::contains(call) {
                return Err(GenesisError::FilteredBootstrapCall);
            }
        }
        Ok(())
    }
}

pub fn origin_for_proposal_class(class: futarchy_primitives::ProposalClass) -> Option<Origin> {
    Origin::from_proposal_class(class)
}

pub fn all_custom_origins() -> [Origin; 8] {
    [
        Origin::FutarchyParam,
        Origin::FutarchyTreasury,
        Origin::FutarchyCode,
        Origin::FutarchyMeta,
        Origin::ConstitutionalValues,
        Origin::OracleResolution,
        Origin::GuardianHold,
        Origin::EmergencyPlaybook,
    ]
}

pub fn exhaustive_filter_sample() -> Vec<BleavitCall> {
    use origins_core::{BoxedCall, RuntimeCall as C};
    fn boxed(call: C) -> BoxedCall {
        BoxedCall::new(call)
    }
    vec![
        BleavitCall::System(SystemCall::Remark),
        BleavitCall::System(SystemCall::SetHeapPages),
        BleavitCall::System(SystemCall::SetCode),
        BleavitCall::System(SystemCall::SetCodeWithoutChecks),
        BleavitCall::System(SystemCall::SetStorage),
        BleavitCall::System(SystemCall::KillStorage),
        BleavitCall::System(SystemCall::KillPrefix),
        BleavitCall::System(SystemCall::AuthorizeUpgrade),
        BleavitCall::System(SystemCall::AuthorizeUpgradeWithoutChecks),
        BleavitCall::System(SystemCall::ApplyAuthorizedUpgrade),
        BleavitCall::Domain(CallDomain::Public),
        BleavitCall::Domain(CallDomain::Nobody),
        BleavitCall::Domain(CallDomain::Param),
        BleavitCall::Domain(CallDomain::Treasury),
        BleavitCall::Domain(CallDomain::Code),
        BleavitCall::Domain(CallDomain::Meta),
        BleavitCall::Domain(CallDomain::ConstitutionalValues),
        BleavitCall::Domain(CallDomain::OracleResolution),
        BleavitCall::Domain(CallDomain::GuardianHold),
        BleavitCall::Domain(CallDomain::EmergencyPlaybook),
        BleavitCall::Domain(CallDomain::InternalRoot),
        BleavitCall::Wrapped(C::UtilityBatch(vec![C::leaf(CallDomain::Public)])),
        BleavitCall::Wrapped(C::UtilityBatchAll(vec![C::leaf(CallDomain::Public)])),
        BleavitCall::Wrapped(C::UtilityForceBatch(vec![C::leaf(CallDomain::Public)])),
        BleavitCall::Wrapped(C::UtilityDispatchAs(boxed(C::leaf(CallDomain::Public)))),
        BleavitCall::Wrapped(C::UtilityAsDerivative(boxed(C::leaf(CallDomain::Public)))),
        BleavitCall::Wrapped(C::UtilityWithWeight(boxed(C::leaf(CallDomain::Public)))),
        BleavitCall::Wrapped(C::Proxy(boxed(C::leaf(CallDomain::Public)))),
        BleavitCall::Wrapped(C::ProxyAnnounced(boxed(C::leaf(CallDomain::Public)))),
        BleavitCall::Wrapped(C::MultisigAsMulti(boxed(C::leaf(CallDomain::Public)))),
        BleavitCall::Wrapped(C::MultisigAsMultiThreshold1(boxed(C::leaf(
            CallDomain::Public,
        )))),
        BleavitCall::Wrapped(C::MultisigApproveAsMulti),
        BleavitCall::Wrapped(C::Scheduler {
            origin: Origin::ConstitutionalValues,
            call: boxed(C::leaf(CallDomain::ConstitutionalValues)),
        }),
        BleavitCall::Wrapped(C::Sudo(boxed(C::leaf(CallDomain::Public)))),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use futarchy_primitives::ProposalClass;
    use origins_core::{BoxedCall, RuntimeCall as C};

    fn boxed(call: C) -> BoxedCall {
        BoxedCall::new(call)
    }

    #[test]
    fn runtime_includes_standard_and_custom_pallet_sets() {
        assert_eq!(STANDARD_PALLETS.len(), 24);
        assert_eq!(CUSTOM_PALLETS.len(), 12);
        assert!(STANDARD_PALLETS.contains(&RuntimePallet::ForeignAssets));
        assert!(STANDARD_PALLETS.contains(&RuntimePallet::AssetTxPayment));
        assert!(CUSTOM_PALLETS.contains(&RuntimePallet::ExecutionGuard));
        assert_eq!(
            RuntimeConfig::default().base_call_filter,
            RuntimeFilter::SafetyFilter
        );
    }

    #[test]
    fn usdc_and_fee_configuration_match_contract_surface() {
        let cfg = RuntimeConfig::default();
        assert_eq!(cfg.usdc.asset_id, USDC_ASSET_ID);
        assert_eq!(cfg.usdc.location, USDC_LOCATION);
        assert!(cfg.usdc.is_sufficient);
        assert_eq!(cfg.usdc.admin_origin, Origin::ConstitutionalValues);
        assert!(!cfg.usdc.mint_burn_enabled);
        assert_eq!(cfg.fees.native_asset, FeeAsset::Vit);
        assert_eq!(cfg.fees.foreign_fee_asset, USDC_ASSET_ID);
        assert_eq!(cfg.fees.conversion_rate_key, FEE_VIT_USDC_RATE_KEY);
    }

    #[test]
    fn genesis_filter_denies_d13_system_calls_even_for_sudo_bootstrap() {
        for call in [
            SystemCall::SetHeapPages,
            SystemCall::SetCode,
            SystemCall::SetCodeWithoutChecks,
            SystemCall::SetStorage,
            SystemCall::KillStorage,
            SystemCall::KillPrefix,
            SystemCall::AuthorizeUpgradeWithoutChecks,
        ] {
            let genesis = GenesisConfig {
                endowed_vit: Vec::new(),
                usdc_asset: ForeignAssetConfig::default(),
                sudo: Some([7; 32]),
                bootstrap_calls: vec![BleavitCall::System(call)],
            };
            assert_eq!(genesis.validate(), Err(GenesisError::FilteredBootstrapCall));
            assert!(!BaseCallFilter::contains(&BleavitCall::System(call)));
        }
        assert!(GenesisConfig {
            endowed_vit: Vec::new(),
            usdc_asset: ForeignAssetConfig::default(),
            sudo: Some([7; 32]),
            bootstrap_calls: vec![BleavitCall::System(SystemCall::Remark)]
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn authorize_upgrade_is_internal_but_apply_is_permissionless_after_lead_time() {
        assert!(!BaseCallFilter::contains(&BleavitCall::System(
            SystemCall::AuthorizeUpgrade
        )));
        assert!(!BaseCallFilter::contains_for(
            Origin::FutarchyCode,
            &BleavitCall::System(SystemCall::AuthorizeUpgrade)
        ));

        let apply = BleavitCall::System(SystemCall::ApplyAuthorizedUpgrade);
        let pending = PendingUpgrade {
            code_hash: [9; 32],
            authorized_at: 10,
            applicable_at: 20,
        };
        assert!(BaseCallFilter::contains(&apply));
        assert_eq!(
            BaseCallFilter::validate_at(None, &apply, 19, Some(pending)),
            Err(RuntimeFilterError::PendingUpgradeTooEarly)
        );
        assert_eq!(
            BaseCallFilter::validate_at(None, &apply, 20, Some(pending)),
            Ok(())
        );
        assert_eq!(
            BaseCallFilter::validate_at(None, &apply, 20, None),
            Err(RuntimeFilterError::PendingUpgradeMissing)
        );
    }

    #[test]
    fn custom_origin_mapping_is_wired_to_proposal_classes() {
        assert_eq!(
            origin_for_proposal_class(ProposalClass::Param),
            Some(Origin::FutarchyParam)
        );
        assert_eq!(
            origin_for_proposal_class(ProposalClass::Treasury),
            Some(Origin::FutarchyTreasury)
        );
        assert_eq!(
            origin_for_proposal_class(ProposalClass::Code),
            Some(Origin::FutarchyCode)
        );
        assert_eq!(
            origin_for_proposal_class(ProposalClass::Meta),
            Some(Origin::FutarchyMeta)
        );
        assert_eq!(
            origin_for_proposal_class(ProposalClass::Constitutional),
            None
        );
        assert_eq!(all_custom_origins().len(), 8);
    }

    #[test]
    fn base_call_filter_delegates_to_safety_filter_for_domains_and_wrappers() {
        assert!(BaseCallFilter::contains(&BleavitCall::Domain(
            CallDomain::Public
        )));
        assert!(!BaseCallFilter::contains(&BleavitCall::Domain(
            CallDomain::Nobody
        )));
        assert!(BaseCallFilter::contains_for(
            Origin::FutarchyTreasury,
            &BleavitCall::Domain(CallDomain::Treasury)
        ));
        assert!(!BaseCallFilter::contains_for(
            Origin::FutarchyParam,
            &BleavitCall::Domain(CallDomain::Treasury)
        ));
        let hidden =
            BleavitCall::Wrapped(C::ProxyAnnounced(boxed(C::UtilityBatch(vec![C::leaf(
                CallDomain::Code,
            )]))));
        assert!(!BaseCallFilter::contains_for(Origin::FutarchyCode, &hidden));
        let threshold_one = BleavitCall::Wrapped(C::MultisigAsMultiThreshold1(boxed(C::leaf(
            CallDomain::Meta,
        ))));
        assert!(!BaseCallFilter::contains_for(
            Origin::FutarchyMeta,
            &threshold_one
        ));
    }

    #[test]
    fn filter_exhaustiveness_sample_covers_every_runtime_shape() {
        let sample = exhaustive_filter_sample();
        assert_eq!(sample.len(), 34);
        for call in &sample {
            let _ = BaseCallFilter::contains(call);
        }
        assert!(sample
            .iter()
            .any(|c| matches!(c, BleavitCall::Wrapped(C::ProxyAnnounced(_)))));
        assert!(sample
            .iter()
            .any(|c| matches!(c, BleavitCall::Wrapped(C::MultisigAsMultiThreshold1(_)))));
        assert!(sample
            .iter()
            .any(|c| matches!(c, BleavitCall::Wrapped(C::Scheduler { .. }))));
    }
}
