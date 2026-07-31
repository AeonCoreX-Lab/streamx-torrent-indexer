// crate/src/registry.rs
//
// Assembles one IndexerRegistry from the individual per-site JSON files
// under sources/verified/ and sources/community/.
//
// Two ways to consume this crate:
//
//   1. Compile-time embed (default, used by StreamX Ultra's release
//      builds): `load_embedded()` reads the same files via
//      `include_str!` at compile time, so the app ships with a working
//      set even with zero network access. Regenerated automatically by
//      `build.rs` — see that file for how the file list is discovered.
//
//   2. Runtime fetch (used for the no-APK-update path): `load_from_json`
//      parses a JSON blob shaped like IndexerRegistry, typically
//      downloaded from a hosted copy of this repo's `dist/registry.json`
//      (built by `cargo run -p streamx-indexer-cli -- build-registry`,
//      see tools/). StreamX Ultra tries this first and falls back to
//      load_embedded() on failure — see its indexer/config/loader.rs.
//
// verified/ vs community/: both are merged into the same map. A file in
// community/ that hasn't been promoted yet is still fully usable — the
// split exists for review workflow (see docs/CONTRIBUTING.md), not for
// gating functionality. If both directories somehow define the same
// `id` (shouldn't happen — CI's validator rejects duplicate ids across
// the two directories in the same PR), verified/ wins.

// pub use, not a plain use: dispatch.rs and engine.rs both write
// `use crate::registry::IndexerRegistry;`, and lib.rs's own doc example
// calls `streamx_indexer::registry::load_embedded()` and expects the
// return type to be reachable at this path too. A private `use` here
// would compile fine for registry.rs's own internal references but
// leave IndexerRegistry unreachable from outside this module — exactly
// the E0603 "private struct import" error that surfaces at every other
// call site instead of here.
pub use crate::schema::IndexerRegistry;
use crate::schema::SiteConfig;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("failed to parse source file {file}: {source}")]
    Parse {
        file: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("duplicate source id '{0}' defined in both verified/ and community/")]
    DuplicateId(String),
    #[error("registry JSON parse error: {0}")]
    RegistryParse(#[from] serde_json::Error),
}

/// Build a registry from raw (filename, content) pairs — used by both
/// the embedded loader (build.rs-generated) and by tools/validator,
/// which reads straight off disk instead of through include_str!.
pub fn build_from_files(
    verified: &[(&str, &str)],
    community: &[(&str, &str)],
    special_sites_json: &str,
) -> Result<IndexerRegistry, RegistryError> {
    let mut sites: HashMap<String, SiteConfig> = HashMap::new();

    for (file, content) in verified {
        let cfg: SiteConfig = serde_json::from_str(content).map_err(|e| RegistryError::Parse {
            file: file.to_string(),
            source: e,
        })?;
        sites.insert(cfg.id.clone(), cfg);
    }
    for (file, content) in community {
        let cfg: SiteConfig = serde_json::from_str(content).map_err(|e| RegistryError::Parse {
            file: file.to_string(),
            source: e,
        })?;
        if sites.contains_key(&cfg.id) {
            // verified/ already claimed this id — verified wins, but
            // this should have been caught by CI before merge.
            continue;
        }
        sites.insert(cfg.id.clone(), cfg);
    }

    let special_sites = serde_json::from_str(special_sites_json)?;

    Ok(IndexerRegistry {
        schema_version: 1,
        updated: String::new(), // stamped by build-registry tool, not here
        sites,
        special_sites,
    })
}

/// Parse a fully-assembled registry JSON blob, as fetched from a hosted
/// dist/registry.json at runtime.
pub fn load_from_json(raw: &str) -> Result<IndexerRegistry, RegistryError> {
    Ok(serde_json::from_str(raw)?)
}

include!(concat!(env!("OUT_DIR"), "/embedded_sources.rs"));

/// Load the registry embedded at compile time. Always succeeds against
/// whatever was in sources/ at build time — panics only on a bug (a
/// file that passed CI validation but somehow fails to parse here would
/// mean build.rs and this loader disagree on the file list, which is a
/// crate bug, not a data problem).
pub fn load_embedded() -> IndexerRegistry {
    build_from_files(EMBEDDED_VERIFIED, EMBEDDED_COMMUNITY, EMBEDDED_SPECIAL_SITES)
        .expect("embedded sources/ must always parse — this is a crate bug, please report it")
}
