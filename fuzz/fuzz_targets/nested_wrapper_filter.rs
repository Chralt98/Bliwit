#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use bleavit_fuzz::{assert_raw_call_decode, assert_wrapper_case, WrapperCase};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut input = Unstructured::new(data);
    if let Ok(case) = WrapperCase::arbitrary(&mut input) {
        assert_wrapper_case(&case);
    }
    assert_raw_call_decode(data);
});
