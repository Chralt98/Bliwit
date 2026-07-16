//! Bleavit collator node (B3).
//!
//! A thin branding of the `polkadot-omni-node` stack (01 §9 pinned family):
//! the runtime ships inside the chain spec (02 §11 release artifacts), the
//! node embeds none, and consensus is the standard Cumulus Aura sr25519
//! pipeline the runtime's `AuraApi`/`AuraUnincludedSegmentApi` already serve.
//! Chain specs are produced by `tools/deploy/generate-chain-specs.sh` from
//! the runtime's genesis presets and validated against the 02 §8/§10
//! identity and bootnode requirements.

use polkadot_omni_node_lib::{
    chain_spec::DiskChainSpecLoader, run, runtime::DefaultRuntimeResolver, CliConfig, RunConfig,
};

struct BleavitCliConfig;

impl CliConfig for BleavitCliConfig {
    fn impl_version() -> String {
        env!("CARGO_PKG_VERSION").into()
    }

    fn author() -> String {
        "Bleavit".into()
    }

    fn support_url() -> String {
        "https://github.com/bleavit/bleavit/issues".into()
    }

    fn copyright_start_year() -> u16 {
        2026
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = RunConfig::new(
        Box::new(DefaultRuntimeResolver),
        Box::new(DiskChainSpecLoader),
    );
    run::<BleavitCliConfig>(config)?;
    Ok(())
}
