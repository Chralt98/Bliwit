#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! # `pallet-origins` — custom governance origins + `SafetyFilter` (A4)
//!
//! Production FRAME shim over the frame-free functional core [`origins_core`],
//! which remains the differential oracle (Python M3 ≡ Rust core ≡ this pallet)
//! and the auditor-/WASM-consumable port. Like Polkadot's OpenGov
//! `pallet-custom-origins`, this is a **stateless** pallet: no storage, no
//! extrinsics, no events, no genesis. Its entire surface is the origin enum,
//! the `EnsureOrigin` set, and the base call filter.
//!
//! Spec: `docs/architecture/06` §3 (custom origins §3.1, authority matrix §3.2,
//! `BaseCallFilter`/closed wrapper set §3.3, scheduler revalidation §3.4),
//! `01` §6 (origins-and-authority summary). Invariants: **I-8** (values scope ⟂
//! beliefs scope), **I-10** (no external origin reaches Root; the closed wrapper
//! set incl. `proxy_announced`/`as_multi_threshold_1`), **I-11** (executed batch
//! domains ⊆ declared; wrappers recursively filtered) — 15 §1.
//!
//! ## The eight custom origins (06 §3.1)
//!
//! [`Origin`] is a real `#[pallet::origin]` enum. Every variant is produced by
//! exactly one pallet through exactly one code path — the `Futarchy*` four by
//! `pallet-execution-guard` per passed proposal class, `ConstitutionalValues`
//! and `OracleResolution` by the values referenda tracks, `GuardianHold` and
//! `EmergencyPlaybook` by `pallet-guardian` on 5-of-7 approval. **None is
//! obtainable from a signed extrinsic, an XCM origin conversion, or a wrapper
//! call** (G-5, I-10): the mock's `construct_runtime!` integration proves at the
//! type level that no `RawOrigin::Signed`/`Root` resolves to a custom variant.
//! There is no fifth `Futarchy*` origin (`ProposalClass::Emergency` is deleted,
//! D-7); `Constitutional`-class subjects route to the values track, so
//! [`Origin::from_proposal_class`] has no belief-side origin for them.
//!
//! ## `SafetyFilter` and the closed wrapper set (06 §3.3)
//!
//! [`SafetyFilter`] is the `BaseCallFilter`. Because the concrete `RuntimeCall`
//! is a runtime-level (B1a) artifact, the filter is parameterized by a
//! [`SafetyClassifier`] that projects a runtime call onto the frame-free filter
//! model ([`FilterCall`]); the reviewed decision logic (nobody row, closed
//! wrapper recursion, depth/count budget, `dispatch_as`/`as_derivative` denial,
//! scheduler values-only revalidation) lives in [`origins_core::SafetyFilter`]
//! and is shared byte-for-byte with the differential oracle. Adding a
//! call-carrying `RuntimeCall` variant with no projection breaks the
//! exhaustiveness test (06 §3.3, I-8/I-10 CI row) — the set stays *closed*.
//!
//! Two checks, per the spec: [`SafetyFilter::contains`] is origin-less (the
//! signed-extrinsic base filter — denies the "nobody" row and dangerous
//! wrappers, and a bare governance-privileged leaf that no signed submitter may
//! reach); [`SafetyFilter::contains_for`] is origin-aware (guard step 5 / I-11
//! and the scheduler's captured-origin re-entry, 06 §3.4). Governance enactment
//! dispatches with its custom origin and re-validates via `contains_for` plus
//! each pallet's `EnsureOrigin` — the "two independent checks" of 06 §3.3. The
//! concrete `RuntimeCall` projection and the guard/scheduler dispatch wiring are
//! B1a/A11.

extern crate alloc;

pub use pallet::*;
pub use weights::WeightInfo;

pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

// The frame-free model surface (differential oracle + B1a's classifier + the
// auditor/WASM port). Re-exported under names that leave `Origin`/`RuntimeCall`
// free for the FRAME `#[pallet::origin]` and the classifier's associated call.
pub use origins_core::{
    BoxedCall, CallDomain, Error as FilterError, Origin as ClassOrigin, RuntimeCall as FilterCall,
};
// The 06 §3.3 wrapper bounds are kernel `K` constants single-homed in
// `futarchy-primitives` (13 §1 `MAX_NESTED`; 01 §5.2 — cores import, never
// re-declare). Re-exported for the filter's consumers (tests, benches, and
// B1a's real-`RuntimeCall` classifier).
pub use futarchy_primitives::kernel::{MAX_NESTED_CALLS, MAX_NESTED_LEVELS};

use core::marker::PhantomData;
use frame_support::traits::{Contains, EnsureOrigin};

/// Projects a runtime `Call` onto the frame-free filter model (06 §3.2 domain
/// for leaves; wrapper structure for the call-carrying variants of 06 §3.3).
///
/// This is the single extension point [`SafetyFilter`] needs to become a
/// concrete `Contains<RuntimeCall>` at runtime assembly (B1a): the runtime
/// implements it once for its `RuntimeCall`, mapping every pallet call to its
/// authority-matrix domain and every wrapper to its inner calls. The closed
/// wrapper set is enforced by the filter-exhaustiveness test over `RuntimeCall`
/// (06 §3.3): a new call-carrying variant with no projection here is caught.
///
/// **B1a implementation constraints (A4 adversarial review):**
/// - **Bound during traversal, do not project-then-clone.** [`ModelClassifier`]
///   returns an owned tree because the model call *is* the filter model; the
///   real classifier MUST NOT clone the whole `RuntimeCall` before the
///   depth/count budget applies. Walk the borrowed call and fail closed at the
///   first `MAX_NESTED_LEVELS`/`MAX_NESTED_CALLS` breach, so an oversized call is
///   rejected before it is fully traversed or allocated (rule 3; the input is
///   already SCALE-decode-bounded, so this is robustness/weight, not a live
///   DoS). B5 benchmarks the maximum-admitted *and* first-rejected shapes.
/// - **Never trust a call-embedded origin.** The model's
///   `RuntimeCall::Scheduler { origin, .. }` field is a *modeling device* for
///   the captured track origin; the real classifier MUST source the scheduler's
///   origin from trusted `pallet-scheduler` agenda state, never from decoded
///   call data, and reject scheduled non-values / `None` origins (06 §3.4).
pub trait SafetyClassifier {
    /// The runtime call type being filtered.
    type Call;
    /// Project one call onto the frame-free filter model.
    fn project(call: &Self::Call) -> FilterCall;
}

/// `BaseCallFilter = SafetyFilter<C>` (06 §3.3). Delegates every decision to the
/// reviewed frame-free core over the [`SafetyClassifier`] projection, so the
/// chain filter and the differential oracle share one implementation.
///
/// **Dispatch protocol B1a MUST honor (A4 adversarial review; SQ-32).** A
/// call-only `Contains<RuntimeCall>` cannot see the dispatch origin, so
/// [`Self::contains`] (origin-less) is only the *signed-extrinsic* base filter:
/// it denies the "nobody" row and the dangerous wrappers, and — since a signed
/// submitter carries no matching custom origin — a bare governance-privileged
/// leaf. The "two independent checks" of 06 §3.3(b) are therefore split: the
/// base filter is check one; the **origin-aware** re-check plus each pallet's
/// `EnsureOrigin` is check two, applied **at the governance dispatch boundary**,
/// not inside `BaseCallFilter`. `pallet-execution-guard` already does this —
/// `SafetyFilter::validate(Some(class_origin), call)` at its execute-time step
/// (I-11) — and the scheduler re-enters the same origin-aware check with the
/// track origin captured at scheduling (06 §3.4). Consequently B1a MUST dispatch
/// passed governance calls via `dispatch_bypass_filter` after that origin-aware
/// validation; **naïvely routing governance through the origin-less base filter
/// would reject every custom-origin call** (`BadOrigin`) and stall enactment.
pub struct SafetyFilter<C>(PhantomData<C>);

impl<C: SafetyClassifier> Contains<C::Call> for SafetyFilter<C> {
    /// Origin-less base filter (signed-extrinsic path): the "nobody" row and
    /// dangerous wrappers are denied, and a bare governance-privileged leaf —
    /// which carries no matching custom origin — is refused. Governance
    /// dispatch supplies its origin and re-enters via [`Self::contains_for`].
    fn contains(call: &C::Call) -> bool {
        origins_core::SafetyFilter::contains(&C::project(call))
    }
}

impl<C: SafetyClassifier> SafetyFilter<C> {
    /// Origin-aware check (guard step 5 / I-11; scheduler captured-origin
    /// re-entry, 06 §3.4): admissible iff the call's domain is reachable by
    /// `origin` and every wrapper layer is clean.
    pub fn contains_for(origin: impl Into<ClassOrigin>, call: &C::Call) -> bool {
        origins_core::SafetyFilter::contains_for(origin.into(), &C::project(call))
    }

    /// Typed validation for precise error reporting (guard/tests). `None` is the
    /// origin-less base-filter reading of [`Self::contains`].
    pub fn validate(origin: Option<ClassOrigin>, call: &C::Call) -> Result<(), FilterError> {
        origins_core::SafetyFilter::validate(origin, &C::project(call))
    }
}

/// The identity [`SafetyClassifier`] over the frame-free model call itself
/// ([`FilterCall`]). Lets `SafetyFilter<ModelClassifier>` act as a concrete
/// `Contains<FilterCall>` for the benchmark harness and any consumer that
/// already speaks the model call; B1a supplies the real-`RuntimeCall`
/// classifier.
pub struct ModelClassifier;

impl SafetyClassifier for ModelClassifier {
    type Call = FilterCall;
    fn project(call: &FilterCall) -> FilterCall {
        call.clone()
    }
}

/// Declares one canonical `EnsureOrigin` per custom origin (the OpenGov
/// `pallet-custom-origins` pattern): it succeeds iff the runtime origin is
/// exactly that custom variant, and yields `()`. The generic bound is exactly
/// what `construct_runtime!` provides for a `#[pallet::origin]` type.
macro_rules! decl_unit_ensure {
    ( $( #[doc = $doc:expr] $name:ident => $origin:ident ),+ $(,)? ) => {
        $(
            #[doc = $doc]
            pub struct $name;
            impl<O> EnsureOrigin<O> for $name
            where
                O: Into<Result<Origin, O>> + From<Origin>,
            {
                type Success = ();
                fn try_origin(o: O) -> Result<Self::Success, O> {
                    o.into().and_then(|o| match o {
                        Origin::$origin => Ok(()),
                        r => Err(O::from(r)),
                    })
                }
                #[cfg(feature = "runtime-benchmarks")]
                fn try_successful_origin() -> Result<O, ()> {
                    Ok(O::from(Origin::$origin))
                }
            }
        )+
    };
}

decl_unit_ensure!(
    #[doc = "Ensures `Origin::FutarchyParam` — the execution guard dispatching a passed PARAM proposal (06 §3.2 row 1)."]
    EnsureFutarchyParam => FutarchyParam,
    #[doc = "Ensures `Origin::FutarchyTreasury` — a passed TREASURY proposal (06 §3.2 row 2)."]
    EnsureFutarchyTreasury => FutarchyTreasury,
    #[doc = "Ensures `Origin::FutarchyCode` — a passed CODE proposal's `authorize_upgrade` (06 §3.2 row 3)."]
    EnsureFutarchyCode => FutarchyCode,
    #[doc = "Ensures `Origin::FutarchyMeta` — a passed META proposal (06 §3.2 row 4)."]
    EnsureFutarchyMeta => FutarchyMeta,
    #[doc = "Ensures `Origin::ConstitutionalValues` — the values referenda tracks (06 §3.2 rows 5–6)."]
    EnsureConstitutionalValues => ConstitutionalValues,
    #[doc = "Ensures `Origin::OracleResolution` — the terminal oracle-adjudication track only (06 §3.2)."]
    EnsureOracleResolution => OracleResolution,
    #[doc = "Ensures `Origin::GuardianHold` — `pallet-guardian` 5-of-7 subtractive powers (06 §3.2, §5)."]
    EnsureGuardianHold => GuardianHold,
    #[doc = "Ensures `Origin::EmergencyPlaybook` — enumerated pre-ratified playbook dispatch only (06 §3.2, §6.2)."]
    EnsureEmergencyPlaybook => EmergencyPlaybook,
);

/// Succeeds for **any** of the eight custom governance origins, yielding which
/// one. Sibling pallets adapt the returned [`Origin`] into their local origin
/// type (e.g. `pallet-constitution`'s
/// `GovernanceOrigin: EnsureOrigin<_, Success = ConstitutionOrigin>`) and then
/// enforce the per-call 06 §3.2 authority-matrix predicate — the second of the
/// two independent checks. A signed, root, or none origin is refused here.
pub struct EnsureFutarchyOrigin;

impl<O> EnsureOrigin<O> for EnsureFutarchyOrigin
where
    O: Into<Result<Origin, O>> + From<Origin>,
{
    type Success = Origin;
    fn try_origin(o: O) -> Result<Self::Success, O> {
        o.into()
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<O, ()> {
        Ok(O::from(Origin::FutarchyParam))
    }
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;
    use futarchy_primitives::ProposalClass;

    /// The in-code storage version. The shim holds no storage, so this never
    /// migrates; it is present for `construct_runtime!` uniformity.
    const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// Weight information. The shim has no extrinsics; the trait carries the
        /// bounded-filter benchmark so the runtime can attribute base-call-filter
        /// evaluation cost (06 §3.3: "filter evaluation weight is bounded").
        type WeightInfo: WeightInfo;
    }

    /// 06 §3.1 (frozen): the eight custom governance origins.
    ///
    /// **Declaration order is the SCALE index order** and MUST match
    /// [`origins_core::Origin`] byte-for-byte — [`crate::tests`] pins it with an
    /// encode-equality differential over all eight variants. All-unit, so it is
    /// `Copy`; every variant produced by exactly one pallet/one path (module
    /// docs), never by a signed/XCM/wrapper origin (G-5, I-10).
    #[derive(
        Clone,
        Copy,
        Debug,
        PartialEq,
        Eq,
        Encode,
        Decode,
        DecodeWithMemTracking,
        MaxEncodedLen,
        TypeInfo,
    )]
    #[pallet::origin]
    pub enum Origin {
        /// Passed PARAM proposal (execution guard).
        FutarchyParam,
        /// Passed TREASURY proposal (execution guard).
        FutarchyTreasury,
        /// Passed CODE proposal's `authorize_upgrade` (execution guard).
        FutarchyCode,
        /// Passed META proposal (execution guard).
        FutarchyMeta,
        /// Values referenda tracks: metric/constitutional registry, elections,
        /// ratification.
        ConstitutionalValues,
        /// Terminal oracle adjudication only.
        OracleResolution,
        /// `pallet-guardian` 5-of-7: pause-intake / delay-once / force-rerun /
        /// gate-suspend.
        GuardianHold,
        /// Enumerated pre-ratified playbook dispatch only (06 §6.2).
        EmergencyPlaybook,
    }

    impl Origin {
        /// All eight variants in SCALE-index order (test/benchmark helper).
        pub const ALL: [Origin; 8] = [
            Origin::FutarchyParam,
            Origin::FutarchyTreasury,
            Origin::FutarchyCode,
            Origin::FutarchyMeta,
            Origin::ConstitutionalValues,
            Origin::OracleResolution,
            Origin::GuardianHold,
            Origin::EmergencyPlaybook,
        ];

        /// The belief-side origin the execution guard produces for a passed
        /// proposal of `class` (06 §3.1). `Constitutional` routes to the values
        /// track — a referendum, not a market — so it has no `Futarchy*` origin.
        pub const fn from_proposal_class(class: ProposalClass) -> Option<Self> {
            match class {
                ProposalClass::Param => Some(Self::FutarchyParam),
                ProposalClass::Treasury => Some(Self::FutarchyTreasury),
                ProposalClass::Code => Some(Self::FutarchyCode),
                ProposalClass::Meta => Some(Self::FutarchyMeta),
                ProposalClass::Constitutional => None,
            }
        }

        /// Lower this FRAME origin to the frame-free model origin
        /// ([`ClassOrigin`]) consumed by `SafetyFilter::contains_for`, the
        /// execution-guard core (I-11), and the differential oracle.
        pub const fn to_model(self) -> ClassOrigin {
            match self {
                Self::FutarchyParam => ClassOrigin::FutarchyParam,
                Self::FutarchyTreasury => ClassOrigin::FutarchyTreasury,
                Self::FutarchyCode => ClassOrigin::FutarchyCode,
                Self::FutarchyMeta => ClassOrigin::FutarchyMeta,
                Self::ConstitutionalValues => ClassOrigin::ConstitutionalValues,
                Self::OracleResolution => ClassOrigin::OracleResolution,
                Self::GuardianHold => ClassOrigin::GuardianHold,
                Self::EmergencyPlaybook => ClassOrigin::EmergencyPlaybook,
            }
        }
    }

    /// Ergonomic lift from the frame-free model origin (guard core → dispatch)
    /// and the `impl Into<ClassOrigin>` used by [`SafetyFilter::contains_for`].
    impl From<ClassOrigin> for Origin {
        fn from(o: ClassOrigin) -> Self {
            match o {
                ClassOrigin::FutarchyParam => Self::FutarchyParam,
                ClassOrigin::FutarchyTreasury => Self::FutarchyTreasury,
                ClassOrigin::FutarchyCode => Self::FutarchyCode,
                ClassOrigin::FutarchyMeta => Self::FutarchyMeta,
                ClassOrigin::ConstitutionalValues => Self::ConstitutionalValues,
                ClassOrigin::OracleResolution => Self::OracleResolution,
                ClassOrigin::GuardianHold => Self::GuardianHold,
                ClassOrigin::EmergencyPlaybook => Self::EmergencyPlaybook,
            }
        }
    }

    impl From<Origin> for ClassOrigin {
        fn from(o: Origin) -> Self {
            o.to_model()
        }
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// 15 §1 try-state. This shim owns no storage, so there is no
        /// state invariant to reconstruct — the origin set is a compile-time
        /// type guarantee (proven by the mock's `construct_runtime!` and the
        /// SCALE differential) and the filter is a pure function. The hook is
        /// present, and green, per rule 8; it deliberately asserts nothing.
        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), sp_runtime::TryRuntimeError> {
            Ok(())
        }
    }
}
