//! Mock runtime with real class origins, real frame-system upgrade calls and
//! storage-backed seam doubles for R-7 rollback tests.

use crate as pallet_execution_guard;
use crate::*;
use frame_support::{
    derive_impl, parameter_types,
    traits::{EnsureOrigin, IsSubType, UnfilteredDispatchable},
};
use futarchy_primitives::{
    keeper::{CrankClass, KeeperRebateSink},
    BoundedVec as PrimitiveBoundedVec, RejectReason,
};
use pallet_origins::SafetyClassifier;
use parity_scale_codec::{DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::{crypto::AccountId32, H256 as SpH256};
use sp_runtime::{
    traits::{BlakeTwo256, Hash as HashT, IdentityLookup},
    BuildStorage, DispatchError, SaturatedConversion,
};

type Block = frame_system::mocking::MockBlock<Test>;

parameter_types! {
    pub static MockVersion: sp_version::RuntimeVersion = sp_version::RuntimeVersion {
        spec_name: alloc::borrow::Cow::Borrowed("test"),
        impl_name: alloc::borrow::Cow::Borrowed("execution-guard-test"),
        authoring_version: 1,
        spec_version: 1,
        impl_version: 1,
        apis: sp_version::create_apis_vec!([]),
        transaction_version: 1,
        system_version: 1,
    };
}

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Origins: pallet_origins,
        TestDispatch: pallet_test_dispatch,
        ExecutionGuard: pallet_execution_guard,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountId = AccountId32;
    type BaseCallFilter = pallet_origins::SafetyFilter<MockClassifier>;
    type Lookup = IdentityLookup<AccountId32>;
    type Hash = SpH256;
    type Hashing = BlakeTwo256;
    type Version = MockVersion;
}

impl pallet_origins::Config for Test {
    type WeightInfo = ();
}

#[derive(
    Clone,
    Copy,
    Debug,
    DecodeWithMemTracking,
    parity_scale_codec::Decode,
    parity_scale_codec::Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum WrapperKind {
    Batch,
    BatchAll,
    ForceBatch,
    DispatchAs,
    AsDerivative,
    WithWeight,
    Proxy,
    ProxyAnnounced,
    AsMulti,
    AsMultiThreshold1,
    Sudo,
}

#[derive(
    Clone,
    Debug,
    DecodeWithMemTracking,
    parity_scale_codec::Decode,
    parity_scale_codec::Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum EpochCall {
    Executed(ProposalId),
    Failed(ProposalId),
    RetryExhausted(ProposalId),
    Rejected(ProposalId, RejectReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpgradeDispatchOrigin {
    Root,
    Signed,
}

#[frame_support::pallet]
pub mod pallet_test_dispatch {
    use super::{EpochCall, WrapperKind};
    use crate::CallDomain;
    use frame_support::pallet_prelude::*;
    use frame_support::{
        dispatch::{DispatchResultWithPostInfo, Pays, PostDispatchInfo},
        traits::EnsureOrigin,
        weights::Weight,
    };
    use frame_system::pallet_prelude::*;
    use futarchy_primitives::BlockNumber;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config<RuntimeEvent: From<Event<Self>>> {
        type FutarchyOrigin: EnsureOrigin<Self::RuntimeOrigin>;
    }

    #[pallet::storage]
    pub type Value<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// Transactional fault switch: retry tests can execute the exact same
    /// encoded call after removing the injected environmental failure.
    #[pallet::storage]
    pub type DispatchFailure<T: Config> = StorageValue<_, bool, ValueQuery>;

    /// Storage-backed seam traces make outer `with_storage_layer` rollback
    /// observable instead of leaking through thread-local test parameters.
    #[pallet::storage]
    #[pallet::unbounded]
    pub type EpochLog<T: Config> = StorageValue<_, Vec<EpochCall>, ValueQuery>;

    #[pallet::storage]
    #[pallet::unbounded]
    pub type ReleaseLog<T: Config> = StorageValue<_, Vec<(u32, BlockNumber, bool)>, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        ValueSet(u32),
    }

    #[pallet::error]
    pub enum Error<T> {
        InjectedFailure,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::call_index(0)]
        #[pallet::weight(Weight::from_parts(1_000, 0))]
        pub fn set_value(origin: OriginFor<T>, value: u32) -> DispatchResult {
            T::FutarchyOrigin::ensure_origin(origin)?;
            Value::<T>::put(value);
            Self::deposit_event(Event::ValueSet(value));
            Ok(())
        }

        #[pallet::call_index(1)]
        #[pallet::weight(Weight::from_parts(1_000, 0))]
        pub fn fail_after_write(origin: OriginFor<T>, value: u32) -> DispatchResult {
            T::FutarchyOrigin::ensure_origin(origin)?;
            Value::<T>::put(value);
            Self::deposit_event(Event::ValueSet(value));
            if DispatchFailure::<T>::get() {
                Err(Error::<T>::InjectedFailure.into())
            } else {
                Ok(())
            }
        }

        /// Declares 1,000 ref-time but reports a lower actual weight so the
        /// guard tests exercise real PostDispatchInfo accumulation/refunds.
        #[pallet::call_index(3)]
        #[pallet::weight(Weight::from_parts(1_000, 100))]
        pub fn set_value_weighted(origin: OriginFor<T>, value: u32) -> DispatchResultWithPostInfo {
            T::FutarchyOrigin::ensure_origin(origin)?;
            Value::<T>::put(value);
            Self::deposit_event(Event::ValueSet(value));
            Ok(PostDispatchInfo {
                actual_weight: Some(Weight::from_parts(400, 40)),
                pays_fee: Pays::Yes,
            })
        }

        /// A mock runtime-call carrier for every closed-wrapper variant. It is
        /// never trusted for recursion: `MockClassifier` projects it to the
        /// reviewed pallet-origins model before dispatch.
        #[pallet::call_index(2)]
        #[pallet::weight(Weight::from_parts(1_000, 0))]
        pub fn wrapped(
            origin: OriginFor<T>,
            kind: WrapperKind,
            leaf: CallDomain,
        ) -> DispatchResult {
            T::FutarchyOrigin::ensure_origin(origin)?;
            let _ = (kind, leaf);
            Ok(())
        }

        /// A Normal-class call whose declared weight is guaranteed to exceed
        /// the guard's 25%-of-block payload ceiling.
        #[pallet::call_index(4)]
        #[pallet::weight(Weight::MAX)]
        pub fn heavy(origin: OriginFor<T>) -> DispatchResult {
            T::FutarchyOrigin::ensure_origin(origin)?;
            Ok(())
        }
    }
}

impl pallet_test_dispatch::Config for Test {
    type FutarchyOrigin = pallet_origins::EnsureFutarchyOrigin;
}

pub fn account(seed: u8) -> AccountId32 {
    AccountId32::new([seed; 32])
}

pub fn keeper() -> AccountId32 {
    account(1)
}

pub fn epoch_account() -> AccountId32 {
    account(2)
}

pub struct EnsureEpochDecision;

impl EnsureOrigin<RuntimeOrigin> for EnsureEpochDecision {
    type Success = ();
    fn try_origin(origin: RuntimeOrigin) -> Result<(), RuntimeOrigin> {
        let origin: Result<frame_system::RawOrigin<AccountId32>, RuntimeOrigin> = origin.into();
        origin.and_then(|origin| match origin {
            frame_system::RawOrigin::Signed(who) if who == epoch_account() => Ok(()),
            other => Err(RuntimeOrigin::from(other)),
        })
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(RuntimeOrigin::signed(epoch_account()))
    }
}

parameter_types! {
    pub static EpochRefuses: bool = false;
    pub static EpochTerminal: Vec<ProposalId> = Vec::new();
    pub static EpochPayloads: Vec<(ProposalId, H256)> = Vec::new();
    pub static PreimageData: Vec<(H256, Vec<u8>)> = Vec::new();
    pub static PreimageFetchRequests: Vec<(H256, u32)> = Vec::new();
    pub static Unpinned: Vec<H256> = Vec::new();
    pub static AttestationArtifact: Option<(u32, H256)> = None;
    pub static AttestationPresent: bool = true;
    pub static AttestationQuorum: bool = true;
    pub static GuardianHeld: Vec<ProposalId> = Vec::new();
    pub static GateSuspended: bool = false;
    pub static LedgerFrozen: bool = false;
    pub static Timelock: BlockNumber = 2;
    pub static Grace: BlockNumber = 10;
    pub static CodeSpacing: BlockNumber = 20;
    pub static AuthorizeCapabilityEnabled: bool = true;
    pub static ReleaseRefuses: bool = false;
    pub static ObservedSpecVersion: Option<u32> = Some(2);
    pub static ObservedSpecName: Vec<u8> = b"test".to_vec();
    pub static Checkpoint: (H256, H256) = ([11; 32], [12; 32]);
    pub static UpgradeDispatchOrigins: Vec<UpgradeDispatchOrigin> = Vec::new();
    pub static UpgradeSchedulingPerformed: bool = false;
    /// Disabled by default, so the mock behaves like the `()` sink unless a
    /// keeper-rebate regression explicitly enables recording.
    pub static RecordKeeperRebates: bool = false;
    pub static KeeperRebates: Vec<(AccountId32, CrankClass)> = Vec::new();
}

pub struct TestKeeperRebate;

impl KeeperRebateSink<AccountId32> for TestKeeperRebate {
    fn rebate(who: &AccountId32, class: CrankClass) {
        if RecordKeeperRebates::get() {
            let mut rebates = KeeperRebates::get();
            rebates.push((who.clone(), class));
            KeeperRebates::set(rebates);
        }
    }
}

pub struct TestEpoch;

impl EpochHandoff for TestEpoch {
    fn payload_hash(pid: ProposalId) -> Option<H256> {
        EpochPayloads::get()
            .into_iter()
            .find_map(|(candidate, hash)| (candidate == pid).then_some(hash))
    }

    fn mark_executed(pid: ProposalId) -> frame_support::dispatch::DispatchResult {
        if EpochRefuses::get() {
            return Err(DispatchError::Other("epoch refused"));
        }
        pallet_test_dispatch::EpochLog::<Test>::mutate(|calls| {
            calls.push(EpochCall::Executed(pid))
        });
        Ok(())
    }
    fn mark_failed_executed(pid: ProposalId) -> frame_support::dispatch::DispatchResult {
        if EpochRefuses::get() {
            return Err(DispatchError::Other("epoch refused"));
        }
        pallet_test_dispatch::EpochLog::<Test>::mutate(|calls| calls.push(EpochCall::Failed(pid)));
        Ok(())
    }
    fn retry_exhausted_to_measurement(pid: ProposalId) -> frame_support::dispatch::DispatchResult {
        if EpochRefuses::get() {
            return Err(DispatchError::Other("epoch refused"));
        }
        if EpochTerminal::get().contains(&pid) {
            return Ok(());
        }
        pallet_test_dispatch::EpochLog::<Test>::mutate(|calls| {
            calls.push(EpochCall::RetryExhausted(pid))
        });
        Ok(())
    }
    fn reject_or_stale(
        pid: ProposalId,
        reason: RejectReason,
    ) -> frame_support::dispatch::DispatchResult {
        if EpochRefuses::get() {
            return Err(DispatchError::Other("epoch refused"));
        }
        if EpochTerminal::get().contains(&pid) {
            return Ok(());
        }
        pallet_test_dispatch::EpochLog::<Test>::mutate(|calls| {
            calls.push(EpochCall::Rejected(pid, reason))
        });
        Ok(())
    }

    fn is_terminal(pid: ProposalId) -> bool {
        EpochTerminal::get().contains(&pid)
    }
}

pub struct TestPreimages;

impl Preimages for TestPreimages {
    fn len(hash: H256) -> Option<u32> {
        PreimageData::get()
            .into_iter()
            .find_map(|(candidate, bytes)| {
                (candidate == hash)
                    .then(|| u32::try_from(bytes.len()).ok())
                    .flatten()
            })
    }
    fn fetch(hash: H256, expected_len: u32) -> Option<Vec<u8>> {
        PreimageFetchRequests::mutate(|requests| requests.push((hash, expected_len)));
        if expected_len > MAX_PAYLOAD_BYTES {
            return None;
        }
        PreimageData::get()
            .into_iter()
            .find_map(|(candidate, bytes)| {
                (candidate == hash && bytes.len() == expected_len as usize).then_some(bytes)
            })
    }
    fn pin(_hash: H256) -> frame_support::dispatch::DispatchResult {
        Ok(())
    }
    fn unpin(hash: H256) -> frame_support::dispatch::DispatchResult {
        PreimageData::mutate(|items| items.retain(|(candidate, _)| *candidate != hash));
        Unpinned::mutate(|items| items.push(hash));
        Ok(())
    }
}

pub struct TestAttestations;

impl Attestations for TestAttestations {
    fn artifact_hash(attestation_id: u32) -> Option<H256> {
        AttestationArtifact::get().and_then(|(id, hash)| (id == attestation_id).then_some(hash))
    }
    fn present_unrevoked_unchallenged(_attestation_id: u32) -> bool {
        AttestationPresent::get()
    }
    fn has_quorum(_pid: ProposalId, artifact_hash: H256) -> bool {
        AttestationQuorum::get()
            && AttestationArtifact::get().is_some_and(|(_, hash)| hash == artifact_hash)
    }
}

pub struct TestGuardian;

impl GuardianState for TestGuardian {
    fn rerun_held(pid: ProposalId) -> bool {
        GuardianHeld::get().contains(&pid)
    }
    fn gate_suspended() -> bool {
        GateSuspended::get()
    }
    fn ledger_freeze_active() -> bool {
        LedgerFrozen::get()
    }
}

pub struct TestParams;

impl Params for TestParams {
    fn exec_timelock(_class: ProposalClass) -> BlockNumber {
        Timelock::get()
    }
    fn exec_grace(_class: ProposalClass) -> BlockNumber {
        Grace::get()
    }
    fn code_spacing() -> BlockNumber {
        CodeSpacing::get()
    }
}

pub struct TestCapabilities;

impl Capabilities<RuntimeCall> for TestCapabilities {
    fn call_enabled(class: ProposalClass, call: &RuntimeCall) -> bool {
        let Ok(analysis) = TestDispatcher::rederive_call(call) else {
            return false;
        };
        analysis.domains.iter().all(|domain| {
            execution_guard_core::domain_allowed(class, *domain)
                && (!matches!(domain, CallDomain::InternalRootAuthorizeUpgrade)
                    || AuthorizeCapabilityEnabled::get())
        })
    }
}

pub struct TestUpgradeSchedule;

impl UpgradeSchedule for TestUpgradeSchedule {
    fn scheduling_performed() -> bool {
        UpgradeSchedulingPerformed::get()
    }
}

pub struct TestReleaseChannel;

impl ReleaseChannelWriter for TestReleaseChannel {
    fn on_upgrade_authorized(
        target_spec_version: u32,
        authorized_at: BlockNumber,
    ) -> frame_support::dispatch::DispatchResult {
        if ReleaseRefuses::get() {
            return Err(DispatchError::Other("release channel refused"));
        }
        pallet_test_dispatch::ReleaseLog::<Test>::mutate(|items| {
            items.push((target_spec_version, authorized_at, false))
        });
        Ok(())
    }
    fn on_upgrade_applied(target_spec_version: u32) -> frame_support::dispatch::DispatchResult {
        if ReleaseRefuses::get() {
            return Err(DispatchError::Other("release channel refused"));
        }
        pallet_test_dispatch::ReleaseLog::<Test>::mutate(|items| {
            items.push((target_spec_version, 0, true))
        });
        Ok(())
    }
    fn on_upgrade_aborted(target_spec_version: u32) -> frame_support::dispatch::DispatchResult {
        // Tolerant by contract: a refusing channel must not wedge the abort
        // cleanup; the mock still logs the clear for assertions.
        pallet_test_dispatch::ReleaseLog::<Test>::mutate(|items| {
            items.push((target_spec_version, 0, true))
        });
        Ok(())
    }
}

pub struct MockClassifier;

impl pallet_origins::SafetyClassifier for MockClassifier {
    type Call = RuntimeCall;
    fn project(call: &RuntimeCall) -> pallet_origins::FilterCall {
        use pallet_origins::{BoxedCall, FilterCall};
        match call {
            RuntimeCall::TestDispatch(pallet_test_dispatch::Call::set_value { .. })
            | RuntimeCall::TestDispatch(pallet_test_dispatch::Call::fail_after_write { .. })
            | RuntimeCall::TestDispatch(pallet_test_dispatch::Call::heavy {})
            | RuntimeCall::TestDispatch(pallet_test_dispatch::Call::set_value_weighted {
                ..
            }) => FilterCall::Leaf(pallet_origins::CallDomain::Param),
            RuntimeCall::TestDispatch(pallet_test_dispatch::Call::wrapped { kind, leaf }) => {
                let leaf = FilterCall::Leaf(model_domain(*leaf));
                let boxed = || BoxedCall::new(leaf.clone());
                match kind {
                    WrapperKind::Batch => FilterCall::UtilityBatch(vec![leaf]),
                    WrapperKind::BatchAll => FilterCall::UtilityBatchAll(vec![leaf]),
                    WrapperKind::ForceBatch => FilterCall::UtilityForceBatch(vec![leaf]),
                    WrapperKind::DispatchAs => FilterCall::UtilityDispatchAs(boxed()),
                    WrapperKind::AsDerivative => FilterCall::UtilityAsDerivative(boxed()),
                    WrapperKind::WithWeight => FilterCall::UtilityWithWeight(boxed()),
                    WrapperKind::Proxy => FilterCall::Proxy(boxed()),
                    WrapperKind::ProxyAnnounced => FilterCall::ProxyAnnounced(boxed()),
                    WrapperKind::AsMulti => FilterCall::MultisigAsMulti(boxed()),
                    WrapperKind::AsMultiThreshold1 => {
                        FilterCall::MultisigAsMultiThreshold1(boxed())
                    }
                    WrapperKind::Sudo => FilterCall::Sudo(boxed()),
                }
            }
            RuntimeCall::System(frame_system::Call::authorize_upgrade { .. }) => {
                FilterCall::Leaf(pallet_origins::CallDomain::InternalRoot)
            }
            #[cfg(feature = "runtime-benchmarks")]
            RuntimeCall::System(frame_system::Call::remark { remark })
                if remark.first() == Some(&0xb5) =>
            {
                // The production runtime classifies System remarks as Public.
                // Keep that benchmark-only fixture recognizable without
                // widening the mock's I-10 arbitrary-System regression.
                FilterCall::Leaf(pallet_origins::CallDomain::Public)
            }
            RuntimeCall::System(frame_system::Call::apply_authorized_upgrade { .. }) => {
                FilterCall::Leaf(pallet_origins::CallDomain::InternalRoot)
            }
            RuntimeCall::ExecutionGuard(_) => FilterCall::Leaf(pallet_origins::CallDomain::Public),
            _ => FilterCall::Leaf(pallet_origins::CallDomain::Nobody),
        }
    }
}

fn model_domain(domain: CallDomain) -> pallet_origins::CallDomain {
    match domain {
        CallDomain::Public => pallet_origins::CallDomain::Public,
        CallDomain::Param => pallet_origins::CallDomain::Param,
        CallDomain::Treasury => pallet_origins::CallDomain::Treasury,
        CallDomain::Code => pallet_origins::CallDomain::Code,
        CallDomain::Meta => pallet_origins::CallDomain::Meta,
        CallDomain::InternalRootAuthorizeUpgrade | CallDomain::InternalRootApplyUpgrade => {
            pallet_origins::CallDomain::InternalRoot
        }
    }
}

fn core_domain(domain: pallet_origins::CallDomain) -> Option<CallDomain> {
    match domain {
        pallet_origins::CallDomain::Public => Some(CallDomain::Public),
        pallet_origins::CallDomain::Param => Some(CallDomain::Param),
        pallet_origins::CallDomain::Treasury => Some(CallDomain::Treasury),
        pallet_origins::CallDomain::Code => Some(CallDomain::Code),
        pallet_origins::CallDomain::Meta => Some(CallDomain::Meta),
        pallet_origins::CallDomain::InternalRoot => Some(CallDomain::InternalRootApplyUpgrade),
        _ => None,
    }
}

fn collect_domains(
    call: &pallet_origins::FilterCall,
    out: &mut ReDerivedDomains,
    nested_calls: &mut u32,
) -> Result<(), DispatchError> {
    use pallet_origins::FilterCall;
    *nested_calls = nested_calls
        .checked_add(1)
        .ok_or(DispatchError::Other("mock nested-call count overflow"))?;
    match call {
        FilterCall::Leaf(domain) => {
            let domain =
                core_domain(*domain).ok_or(DispatchError::Other("unprojected mock domain"))?;
            if !out.contains(&domain) {
                out.try_push(domain)
                    .map_err(|_| DispatchError::Other("too many mock domains"))?;
            }
        }
        FilterCall::UtilityBatch(calls)
        | FilterCall::UtilityBatchAll(calls)
        | FilterCall::UtilityForceBatch(calls) => {
            for call in calls {
                collect_domains(call, out, nested_calls)?;
            }
        }
        FilterCall::UtilityDispatchAs(call)
        | FilterCall::UtilityAsDerivative(call)
        | FilterCall::UtilityWithWeight(call)
        | FilterCall::Proxy(call)
        | FilterCall::ProxyAnnounced(call)
        | FilterCall::MultisigAsMulti(call)
        | FilterCall::MultisigAsMultiThreshold1(call)
        | FilterCall::Sudo(call) => collect_domains(&call.0, out, nested_calls)?,
        FilterCall::Scheduler { call, .. } => collect_domains(&call.0, out, nested_calls)?,
        FilterCall::MultisigApproveAsMulti => {}
    }
    Ok(())
}

pub struct TestDispatcher;

impl BatchDispatcher<RuntimeCall> for TestDispatcher {
    fn rederive_call(call: &RuntimeCall) -> Result<ReDerivedCall, DispatchError> {
        if Self::authorize_upgrade_hash(call).is_some() {
            let domains =
                ReDerivedDomains::try_from(vec![CallDomain::InternalRootAuthorizeUpgrade])
                    .map_err(|_| DispatchError::Other("mock domain bound"))?;
            return Ok(ReDerivedCall {
                domains,
                nested_calls: 1,
            });
        }
        let model = MockClassifier::project(call);
        let mut domains = ReDerivedDomains::default();
        let mut nested_calls = 0;
        collect_domains(&model, &mut domains, &mut nested_calls)?;
        Ok(ReDerivedCall {
            domains,
            nested_calls,
        })
    }

    fn safety_filter(class: ProposalClass, call: &RuntimeCall) -> bool {
        if matches!(
            call,
            RuntimeCall::TestDispatch(pallet_test_dispatch::Call::wrapped {
                kind: WrapperKind::Batch | WrapperKind::ForceBatch | WrapperKind::Sudo,
                ..
            })
        ) {
            return false;
        }
        pallet_origins::ClassOrigin::from_proposal_class(class).is_some_and(|origin| {
            pallet_origins::SafetyFilter::<MockClassifier>::contains_for(origin, call)
        })
    }

    fn authorize_upgrade_hash(call: &RuntimeCall) -> Option<H256> {
        let system: Option<&frame_system::Call<Test>> = call.is_sub_type();
        match system {
            Some(frame_system::Call::authorize_upgrade { code_hash }) => Some(code_hash.0),
            _ => None,
        }
    }

    fn dispatch_with_class_origin(
        call: RuntimeCall,
        class: ProposalClass,
    ) -> frame_support::dispatch::DispatchResult {
        let origin = pallet_origins::ClassOrigin::from_proposal_class(class)
            .ok_or(DispatchError::BadOrigin)?;
        let origin = pallet_origins::Origin::from(origin);
        call.dispatch_bypass_filter(RuntimeOrigin::from(origin))
            .map(|_| ())
            .map_err(|error| error.error)
    }

    fn dispatch_with_class_origin_post_info(
        call: RuntimeCall,
        class: ProposalClass,
    ) -> frame_support::dispatch::DispatchResultWithPostInfo {
        let origin = pallet_origins::ClassOrigin::from_proposal_class(class)
            .ok_or(DispatchError::BadOrigin)?;
        call.dispatch_bypass_filter(RuntimeOrigin::from(pallet_origins::Origin::from(origin)))
    }

    fn dispatch_authorize_upgrade(code_hash: H256) -> frame_support::dispatch::DispatchResult {
        UpgradeDispatchOrigins::mutate(|origins| origins.push(UpgradeDispatchOrigin::Root));
        let call = RuntimeCall::System(frame_system::Call::authorize_upgrade {
            code_hash: SpH256::from(code_hash),
        });
        call.dispatch_bypass_filter(RuntimeOrigin::root())
            .map(|_| ())
            .map_err(|error| error.error)
    }

    fn dispatch_apply_authorized_upgrade(code: Vec<u8>) -> frame_support::dispatch::DispatchResult {
        UpgradeDispatchOrigins::mutate(|origins| origins.push(UpgradeDispatchOrigin::Signed));
        let call = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade { code });
        call.dispatch_bypass_filter(RuntimeOrigin::signed(keeper()))
            .map(|_| ())
            .map_err(|error| error.error)
    }

    fn observed_runtime_version(_code: &[u8]) -> Option<RuntimeVersionConstraint> {
        let spec_version = ObservedSpecVersion::get()?;
        let spec_name = PrimitiveBoundedVec::try_from(ObservedSpecName::get()).ok()?;
        Some(RuntimeVersionConstraint {
            spec_name,
            spec_version,
        })
    }

    fn checkpoint() -> (H256, H256) {
        Checkpoint::get()
    }
}

#[cfg(feature = "runtime-benchmarks")]
pub struct TestBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_meters(pid: ProposalId) -> StoredMeters {
    let meters = (0..MAX_RESOURCE_LOCKS)
        .map(|index| {
            let mut meter = pid.to_le_bytes();
            if let Some(first) = meter.first_mut() {
                *first ^= index as u8;
            }
            meter
        })
        .collect::<Vec<_>>();
    StoredMeters::try_from(meters).unwrap_or_default()
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_enqueue(
    pid: ProposalId,
    class: ProposalClass,
    calls: Vec<RuntimeCall>,
    domains: Vec<CallDomain>,
    attestation_id: Option<u32>,
    ratify_ref: Option<u32>,
) -> H256 {
    let (payload_hash, payload_len) = put_preimage(&calls);
    commit_payload(pid, payload_hash);
    let mut item = queued_item(pid, class, payload_hash, payload_len, domains);
    item.meters_declared = benchmark_meters(pid);
    item.attestation_id = attestation_id;
    item.ratify_ref = ratify_ref;
    ExecutionGuard::enqueue(RuntimeOrigin::signed(epoch_account()), item, false)
        .expect("benchmark queue fixture must be admissible");
    payload_hash
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_queue() {
    let mut pid = 10_000;
    for _ in 0..MAX_QUEUE_BOUND {
        if Queue::<Test>::count() >= MAX_QUEUE_BOUND {
            break;
        }
        benchmark_enqueue(
            pid,
            ProposalClass::Param,
            vec![param_call(pid as u32)],
            vec![CallDomain::Param],
            None,
            None,
        );
        pid = pid.saturating_add(1);
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_records() {
    let records = (0..MAX_EXECUTION_RECORDS)
        .map(|index| futarchy_primitives::ExecutionRecord {
            pid: 20_000u64.saturating_add(index as u64),
            payload_hash: [index as u8; 32],
            class: ProposalClass::Param,
            executed_at: index as BlockNumber,
            result: futarchy_primitives::DispatchOutcomeCode::Ok,
        })
        .collect::<Vec<_>>();
    if let Ok(records) = StoredRecords::try_from(records) {
        ExecutionRecords::<Test>::put(records);
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_envelopes() {
    let blocked = (0..MAX_BLOCKED_METERS_BOUND)
        .map(|index| {
            let mut meter = [0xff; 8];
            meter[4..8].copy_from_slice(&index.to_le_bytes());
            meter
        })
        .collect::<Vec<_>>();
    if let Ok(blocked) = StoredBlockedMeters::try_from(blocked) {
        BlockedMeters::<Test>::put(blocked);
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_upgrade_history(now: BlockNumber) {
    let spacing = CodeSpacing::get();
    let count = MAX_EXECUTION_RECORDS as BlockNumber;
    let history = (0..count)
        .map(|index| {
            (
                now.saturating_sub(spacing.saturating_mul(count.saturating_sub(index))),
                spacing,
            )
        })
        .collect::<Vec<_>>();
    if let Ok(history) = StoredUpgradeSpacingHistory::try_from(history) {
        UpgradeSpacingHistory::<Test>::put(history.clone());
        if let Some((authorized_at, _)) = history.last() {
            LastUpgradeAuthorized::<Test>::put(*authorized_at);
        }
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_fill_ratifications() {
    let mut pid = 30_000;
    for _ in 0..MAX_RATIFICATIONS_BOUND {
        if Ratifications::<Test>::count().saturating_add(1) >= MAX_RATIFICATIONS_BOUND {
            break;
        }
        let payload_hash = [pid as u8; 32];
        commit_payload(pid, payload_hash);
        Ratifications::<Test>::insert(
            pid,
            RatificationRecord {
                referendum_index: pid as u32,
                payload_hash,
                ratified_at: 1,
            },
        );
        pid = pid.saturating_add(1);
    }
}

#[cfg(feature = "runtime-benchmarks")]
fn benchmark_execute_calls(artifact: H256, call_count: u32) -> Vec<RuntimeCall> {
    let mut calls = vec![authorize_call(artifact)];
    calls.extend((1..call_count).map(|index| {
        let mut remark = vec![index as u8; 4_000];
        remark[0] = 0xb5;
        RuntimeCall::System(frame_system::Call::remark { remark })
    }));
    if call_count > 1 {
        let target = MAX_PAYLOAD_BYTES as usize;
        loop {
            let encoded_len = calls.encode().len();
            match encoded_len.cmp(&target) {
                core::cmp::Ordering::Equal => break,
                core::cmp::Ordering::Less => {
                    let RuntimeCall::System(frame_system::Call::remark { remark }) = calls
                        .last_mut()
                        .expect("benchmark multi-call payload has a final remark")
                    else {
                        unreachable!("benchmark multi-call payload ends in a System remark")
                    };
                    remark.resize(remark.len().saturating_add(target - encoded_len), 0xff);
                }
                core::cmp::Ordering::Greater => {
                    let RuntimeCall::System(frame_system::Call::remark { remark }) = calls
                        .last_mut()
                        .expect("benchmark multi-call payload has a final remark")
                    else {
                        unreachable!("benchmark multi-call payload ends in a System remark")
                    };
                    remark.truncate(remark.len().saturating_sub(encoded_len - target));
                }
            }
        }
    }
    calls
}

#[cfg(feature = "runtime-benchmarks")]
impl BenchmarkHelper<RuntimeOrigin> for TestBenchmarkHelper {
    fn ratify_origin() -> RuntimeOrigin {
        RuntimeOrigin::from(pallet_origins::Origin::ConstitutionalValues)
    }
    fn prime_ratify(pid: ProposalId, referendum_index: u32) {
        let code = b"benchmark-ratify";
        let artifact = hash(code);
        AttestationArtifact::set(Some((7, artifact)));
        benchmark_enqueue(
            pid,
            ProposalClass::Code,
            vec![authorize_call(artifact)],
            vec![CallDomain::InternalRootAuthorizeUpgrade],
            Some(7),
            Some(referendum_index),
        );
        benchmark_fill_queue();
        benchmark_fill_records();
        benchmark_fill_envelopes();
        benchmark_fill_ratifications();
    }
    fn prime_execute(pid: ProposalId, calls: u32) {
        let artifact = [0x42; 32];
        AttestationArtifact::set(Some((7, artifact)));
        let domains = if calls > 1 {
            vec![CallDomain::Public, CallDomain::InternalRootAuthorizeUpgrade]
        } else {
            vec![CallDomain::InternalRootAuthorizeUpgrade]
        };
        System::set_block_number(
            u64::from(CodeSpacing::get()).saturating_mul(MAX_EXECUTION_RECORDS as u64 + 1),
        );
        let _payload_hash = benchmark_enqueue(
            pid,
            ProposalClass::Code,
            benchmark_execute_calls(artifact, calls),
            domains,
            Some(7),
            Some(pid as u32),
        );
        ExecutionGuard::ratify(
            RuntimeOrigin::from(pallet_origins::Origin::ConstitutionalValues),
            pid,
            pid as u32,
        )
        .expect("benchmark Code queue ratification must succeed");
        benchmark_fill_queue();
        benchmark_fill_records();
        benchmark_fill_envelopes();
        run_to_maturity(pid);
        benchmark_fill_upgrade_history(System::block_number().saturated_into());
    }
    fn prime_failed(pid: ProposalId) {
        benchmark_enqueue(
            pid,
            ProposalClass::Param,
            vec![failing_call(1)],
            vec![CallDomain::Param],
            None,
            None,
        );
        benchmark_fill_queue();
        benchmark_fill_records();
        benchmark_fill_envelopes();
        run_to_maturity(pid);
        set_dispatch_failure(true);
        let _ = ExecutionGuard::execute(RuntimeOrigin::signed(keeper()), pid);
        System::set_block_number(
            System::block_number()
                .saturating_add(RETRY_WINDOW.into())
                .saturating_add(1),
        );
    }
    fn prime_pending_upgrade(bytes: u32) -> Vec<u8> {
        let mut code = b"benchmark-runtime-v2".to_vec();
        code.resize(bytes as usize, 0);
        let hash = hash(&code);
        AttestationArtifact::set(Some((7, hash)));
        benchmark_enqueue(
            1,
            ProposalClass::Code,
            vec![authorize_call(hash)],
            vec![CallDomain::InternalRootAuthorizeUpgrade],
            Some(7),
            Some(9),
        );
        benchmark_fill_queue();
        benchmark_fill_records();
        benchmark_fill_envelopes();
        let _ = ExecutionGuard::ratify(
            RuntimeOrigin::from(pallet_origins::Origin::ConstitutionalValues),
            1,
            9,
        );
        run_to_maturity(1);
        let _ = ExecutionGuard::execute(RuntimeOrigin::signed(keeper()), 1);
        System::set_block_number(
            System::block_number().saturating_add(DESCRIPTOR_LEAD_TIME.into()),
        );
        code
    }
    fn prime_stale(pid: ProposalId) {
        benchmark_enqueue(
            pid,
            ProposalClass::Param,
            vec![param_call(1)],
            vec![CallDomain::Param],
            None,
            None,
        );
        benchmark_fill_queue();
        benchmark_fill_records();
        benchmark_fill_envelopes();
        CurrentSpecName::<Test>::put(spec(99));
    }
}

impl pallet_execution_guard::Config for Test {
    type Epoch = TestEpoch;
    type EnqueueAuthority = EnsureEpochDecision;
    type Attestations = TestAttestations;
    type Guardian = TestGuardian;
    type Params = TestParams;
    type Capabilities = TestCapabilities;
    type UpgradeSchedule = TestUpgradeSchedule;
    type Preimages = TestPreimages;
    type ReleaseChannel = TestReleaseChannel;
    type RatifyOrigin = pallet_origins::EnsureConstitutionalValues;
    type Dispatcher = TestDispatcher;
    type KeeperRebate = TestKeeperRebate;
    type MaxRuntimeCodeBytes = frame_support::traits::ConstU32<2_097_152>;
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = TestBenchmarkHelper;
}

struct ReadRuntimeVersion;

impl sp_core::traits::ReadRuntimeVersion for ReadRuntimeVersion {
    fn read_runtime_version(
        &self,
        _wasm_code: &[u8],
        _ext: &mut dyn sp_core::traits::Externalities,
    ) -> Result<Vec<u8>, String> {
        let mut version = <Test as frame_system::Config>::Version::get();
        version.spec_version = ObservedSpecVersion::get().unwrap_or_default();
        Ok(version.encode())
    }
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        execution_guard: pallet_execution_guard::GenesisConfig {
            _config: core::marker::PhantomData,
        },
    }
    .build_storage()
    .expect("mock genesis must build");
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.register_extension(sp_core::traits::ReadRuntimeVersionExt::new(
        ReadRuntimeVersion,
    ));
    ext.execute_with(|| {
        System::set_block_number(1);
        reset_statics();
    });
    ext
}

pub fn reset_statics() {
    EpochRefuses::set(false);
    EpochTerminal::set(Vec::new());
    EpochPayloads::set(Vec::new());
    PreimageData::set(Vec::new());
    PreimageFetchRequests::set(Vec::new());
    Unpinned::set(Vec::new());
    AttestationArtifact::set(None);
    AttestationPresent::set(true);
    AttestationQuorum::set(true);
    GuardianHeld::set(Vec::new());
    GateSuspended::set(false);
    LedgerFrozen::set(false);
    Timelock::set(2);
    Grace::set(10);
    CodeSpacing::set(20);
    AuthorizeCapabilityEnabled::set(true);
    UpgradeSchedulingPerformed::set(false);
    ReleaseRefuses::set(false);
    ObservedSpecVersion::set(Some(2));
    ObservedSpecName::set(b"test".to_vec());
    Checkpoint::set(([11; 32], [12; 32]));
    UpgradeDispatchOrigins::set(Vec::new());
    RecordKeeperRebates::set(false);
    KeeperRebates::set(Vec::new());
    pallet_test_dispatch::DispatchFailure::<Test>::put(false);
    pallet_test_dispatch::EpochLog::<Test>::kill();
    pallet_test_dispatch::ReleaseLog::<Test>::kill();
}

pub fn set_dispatch_failure(fails: bool) {
    pallet_test_dispatch::DispatchFailure::<Test>::put(fails);
}

pub fn epoch_calls() -> Vec<EpochCall> {
    pallet_test_dispatch::EpochLog::<Test>::get()
}

pub fn release_log() -> Vec<(u32, BlockNumber, bool)> {
    pallet_test_dispatch::ReleaseLog::<Test>::get()
}

pub fn commit_payload(pid: ProposalId, payload_hash: H256) {
    EpochPayloads::mutate(|items| {
        items.retain(|(candidate, _)| *candidate != pid);
        items.push((pid, payload_hash));
    });
}

pub fn hash(bytes: &[u8]) -> H256 {
    <Test as frame_system::Config>::Hashing::hash(bytes).0
}

pub fn put_preimage(calls: &[RuntimeCall]) -> (H256, u32) {
    let bytes = calls.encode();
    let hash = hash(&bytes);
    let len = u32::try_from(bytes.len()).expect("mock payload fits u32");
    PreimageData::mutate(|items| items.push((hash, bytes)));
    (hash, len)
}

pub fn param_call(value: u32) -> RuntimeCall {
    RuntimeCall::TestDispatch(pallet_test_dispatch::Call::set_value { value })
}

pub fn failing_call(value: u32) -> RuntimeCall {
    RuntimeCall::TestDispatch(pallet_test_dispatch::Call::fail_after_write { value })
}

pub fn heavy_call() -> RuntimeCall {
    RuntimeCall::TestDispatch(pallet_test_dispatch::Call::heavy {})
}

pub fn weighted_call(value: u32) -> RuntimeCall {
    RuntimeCall::TestDispatch(pallet_test_dispatch::Call::set_value_weighted { value })
}

pub fn wrapped_call(kind: WrapperKind, leaf: CallDomain) -> RuntimeCall {
    RuntimeCall::TestDispatch(pallet_test_dispatch::Call::wrapped { kind, leaf })
}

pub fn authorize_call(code_hash: H256) -> RuntimeCall {
    RuntimeCall::System(frame_system::Call::authorize_upgrade {
        code_hash: SpH256::from(code_hash),
    })
}

pub fn queued_item(
    pid: ProposalId,
    class: ProposalClass,
    payload_hash: H256,
    payload_len: u32,
    domains: Vec<CallDomain>,
) -> StoredQueuedExecution {
    let now: BlockNumber = System::block_number().saturated_into();
    let maturity = now.saturating_add(Timelock::get());
    let grace_end = maturity.saturating_add(Grace::get());
    StoredQueuedExecution {
        pid,
        payload_hash,
        payload_len,
        class,
        maturity,
        grace_end,
        version_constraint: CurrentSpecName::<Test>::get()
            .expect("mock genesis initializes current version"),
        meters_declared: frame_support::BoundedVec::try_from(vec![[pid as u8; 8]])
            .expect("one resource fits"),
        ratify_ref: None,
        ratification_passed: false,
        attestation_id: None,
        pre_upgrade_checkpoint: None,
        cancelled: false,
        declared_domains: frame_support::BoundedVec::try_from(domains).expect("mock domains fit"),
        failed_at: None,
    }
}

pub fn enqueue_calls(
    pid: ProposalId,
    class: ProposalClass,
    calls: Vec<RuntimeCall>,
    domains: Vec<CallDomain>,
) -> frame_support::dispatch::DispatchResult {
    let (payload_hash, payload_len) = put_preimage(&calls);
    commit_payload(pid, payload_hash);
    let item = queued_item(pid, class, payload_hash, payload_len, domains);
    ExecutionGuard::enqueue(RuntimeOrigin::signed(epoch_account()), item, false)
}

pub fn enqueue_code(
    pid: ProposalId,
    call: RuntimeCall,
    attestation_id: u32,
    referendum: u32,
) -> frame_support::dispatch::DispatchResult {
    let (payload_hash, payload_len) = put_preimage(&[call]);
    commit_payload(pid, payload_hash);
    let mut item = queued_item(
        pid,
        ProposalClass::Code,
        payload_hash,
        payload_len,
        vec![CallDomain::InternalRootAuthorizeUpgrade],
    );
    item.attestation_id = Some(attestation_id);
    item.ratify_ref = Some(referendum);
    ExecutionGuard::enqueue(RuntimeOrigin::signed(epoch_account()), item, false)
}

pub fn run_to_maturity(pid: ProposalId) {
    let maturity = Queue::<Test>::get(pid)
        .map(|queued| queued.maturity)
        .unwrap_or_else(|| System::block_number().saturated_into());
    System::set_block_number(maturity.into());
}

pub fn spec(v: u32) -> RuntimeVersionConstraint {
    RuntimeVersionConstraint {
        spec_name: PrimitiveBoundedVec::try_from(
            <Test as frame_system::Config>::Version::get()
                .spec_name
                .as_bytes()
                .to_vec(),
        )
        .expect("mock spec name fits"),
        spec_version: v,
    }
}
