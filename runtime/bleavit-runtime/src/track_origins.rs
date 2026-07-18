//! Runtime-internal origins that keep the five ConstitutionalValues referenda
//! tracks distinct through scheduling and enactment (06 §2.1).
//!
//! This pallet is deliberately origin-only: it has no calls, storage, events,
//! hooks, or configuration. The public `pallet-origins::ConstitutionalValues`
//! origin remains the conservative legacy values authority; referenda submitted
//! with it map to the entrenched track.

#[frame_support::pallet]
pub mod pallet {
    use frame_support::pallet_prelude::*;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {}

    #[pallet::origin]
    #[derive(
        Clone,
        Copy,
        Debug,
        Decode,
        DecodeWithMemTracking,
        Encode,
        Eq,
        MaxEncodedLen,
        PartialEq,
        TypeInfo,
    )]
    pub enum Origin {
        Metric,
        Constitution,
        Entrenched,
        GuardianTrack,
        Ratify,
    }
}

pub use pallet::*;
