//! streamx-indexer
//!
//! A modular, config-driven torrent indexer engine, extracted from
//! StreamX Ultra so its site definitions can live in their own repo,
//! be contributed to by the community as plain JSON files, and be kept
//! in sync with upstream Jackett automatically.
//!
//! ## Quick start
//!
//! ```no_run
//! # async fn example() {
//! let registry = streamx_indexer::registry::load_embedded();
//! let client = reqwest::Client::new();
//! let results = streamx_indexer::engine::search_all(&client, &registry, "some movie 2024").await;
//! # }
//! ```
//!
//! See docs/CONSUMING.md in the repo root for the recommended pattern
//! of fetching a hosted `dist/registry.json` at runtime and falling
//! back to `registry::load_embedded()` when that fails — this is how
//! StreamX Ultra gets no-APK-update site fixes.
//!
//! ## Adding a new source
//!
//! See docs/CONTRIBUTING.md. In short: drop a JSON file under
//! `sources/community/<id>.json` matching `schema/source.schema.json`,
//! open a PR — CI validates the schema and smoke-tests your selectors
//! against the live site automatically.

pub mod schema;
pub mod registry;
pub mod types;
pub mod dispatch;
pub mod engine;
pub mod generic_html;
pub mod generic_json;
pub mod proxy;

pub mod special {
    pub mod eztvco;
    pub mod kdrama;
    pub mod nyaa;
    pub mod tokyotosho;
}
