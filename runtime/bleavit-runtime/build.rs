fn main() {
    #[cfg(feature = "substrate-wasm-builder")]
    {
        substrate_wasm_builder::WasmBuilder::build_using_defaults();
    }
}
