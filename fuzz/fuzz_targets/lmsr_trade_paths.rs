#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use bleavit_fuzz::{assert_lmsr_case, TradeCase};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut input = Unstructured::new(data);
    if let Ok(case) = TradeCase::arbitrary(&mut input) {
        assert_lmsr_case(&case);
    }
});
