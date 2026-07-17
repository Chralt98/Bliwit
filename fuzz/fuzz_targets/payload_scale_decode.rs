#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use bleavit_fuzz::{assert_payload_bytes, assert_structured_payload, PayloadCase};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Raw-bytes decode robustness (bounded recursion/allocation).
    assert_payload_bytes(data);
    // Structured generation — reaches the guard's admit and bound-rejection
    // paths that random bytes almost never form.
    let mut input = Unstructured::new(data);
    if let Ok(case) = PayloadCase::arbitrary(&mut input) {
        assert_structured_payload(case);
    }
});
