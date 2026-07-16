//! Local XCM-send health observation (09 §6.4; I-24).

use core::marker::PhantomData;
use staging_xcm::latest::{Assets, Location, SendError, SendResult, SendXcm, Xcm, XcmHash};

/// Locally observable XCM transport/probe signals consumed at B1a (09 §6.4).
///
/// A successful local delivery reveals nothing about remote execution. Likewise,
/// a failure here is a local validate/deliver failure only. The oracle timeout
/// fold is the B1a binding point for [`Self::note_probe_timeout`], the only
/// locally-observable signal that a sent reserve probe received no response.
/// `pallet-oracle` deliberately exposes no fold callback (its XCM surface is the
/// two B4 seams only), so B1a derives this signal from `ReserveHealth` deltas or
/// the `ReserveProbeResult { passed: false }` event stream at the welfare binding.
/// Remote outcomes are not runtime-readable: when X is partial, R alone drives
/// the C flag. Implementations must never use these signals to improve an input
/// or infer remote success (I-24).
pub trait LocalXcmHealthSink {
    /// The local router accepted and delivered a message to its transport.
    fn note_sent();
    /// Local validation or delivery failed.
    fn note_send_failure();
    /// The oracle folded an unanswered reserve probe at its bounded timeout.
    fn note_probe_timeout();
}

/// Pure-observation router: it never changes whether validation or delivery succeeds (09 §6.4).
///
/// These counters feed `C_onchain`'s X input at B1a. Fallback semantics, including R driving the
/// flag alone, are owned by 05/07 and are deliberately absent here (I-24).
pub struct HealthTrackingRouter<Inner, Sink>(PhantomData<(Inner, Sink)>);

impl<Inner: SendXcm, Sink: LocalXcmHealthSink> SendXcm for HealthTrackingRouter<Inner, Sink> {
    type Ticket = Inner::Ticket;

    fn validate(
        destination: &mut Option<Location>,
        message: &mut Option<Xcm<()>>,
    ) -> SendResult<Self::Ticket> {
        match Inner::validate(destination, message) {
            Ok(validated) => Ok(validated),
            Err(error) => {
                // `NotApplicable` is tuple-router control flow, not a failed send attempt.
                if error != SendError::NotApplicable {
                    Sink::note_send_failure();
                }
                Err(error)
            }
        }
    }

    fn deliver(ticket: Self::Ticket) -> Result<XcmHash, SendError> {
        match Inner::deliver(ticket) {
            Ok(hash) => {
                Sink::note_sent();
                Ok(hash)
            }
            Err(error) => {
                Sink::note_send_failure();
                Err(error)
            }
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    fn ensure_successful_delivery(location: Option<Location>) {
        Inner::ensure_successful_delivery(location);
    }
}

// Keep the SDK return's fee type explicit in this module's public seam.
#[allow(dead_code)]
fn _validated_shape<T>(value: (T, Assets)) -> (T, Assets) {
    value
}
