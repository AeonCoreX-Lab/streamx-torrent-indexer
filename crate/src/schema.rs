// crate/src/schema.rs
//
// Canonical Rust types for a StreamX torrent-indexer source definition.
// This is the single source of truth for the JSON shape — the JSON
// Schema at /schema/source.schema.json is generated FROM these types
// (see tools/validator's `--emit-json-schema` mode) so the two can
// never drift apart.
//
// A "source" here means one site definition: either a fully
// config-driven HTML/JSON scraper (the common case — see SiteConfig),
// or a lightweight override for a "special" site whose scraping logic
// is too bespoke for the generic engine and lives in crate/src/special/
// instead (see SpecialSiteOverride).
//
// Every field that can reasonably change when a site redesigns its
// HTML lives in this schema, not in Rust code — that's the whole point:
// a selector or domain fix should be a JSON PR, not an app release.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level registry, assembled at build time (or fetched at runtime
/// from a hosted copy) by merging every file under sources/verified/
/// and sources/community/ — see crate/src/registry.rs.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndexerRegistry {
    pub schema_version: u32,
    #[serde(default)]
    pub updated: String,
    pub sites: HashMap<String, SiteConfig>,
    #[serde(default)]
    pub special_sites: HashMap<String, SpecialSiteOverride>,
}

/// One config-driven site definition. This is also the exact shape of
/// a single file under sources/community/<id>.json or
/// sources/verified/<id>.json — the registry is just these merged by
/// `id`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SiteConfig {
    /// Unique slug, must match the filename (sources/*/<id>.json) and
    /// the map key in IndexerRegistry.sites. Lowercase, no spaces —
    /// same convention as a Jackett indexer id.
    pub id: String,

    #[serde(default = "default_true")]
    pub enabled: bool,

    pub display_name: String,
    pub kind: SiteKind,
    pub mirrors: Vec<String>,
    pub search_path: String,

    /// Mirrors that must NEVER be written into `mirrors[]` by
    /// tools/jackett-sync, even though Jackett's own upstream `links:`
    /// still lists them. Exists for domains confirmed genuinely dead
    /// (DNS failure, not just bot-challenge-blocked — see
    /// tools/validator's BLOCKED vs DEAD distinction) that Jackett
    /// hasn't pruned from their own definition yet. Without this,
    /// jackett-sync would silently re-add a manually-removed dead
    /// mirror on its very next daily run, since as far as it's
    /// concerned nothing upstream changed.
    ///
    /// This is NOT for a mirror that's merely Cloudflare/WAF-blocked
    /// from wherever validated it — that's still a live mirror for real
    /// users and belongs in `mirrors[]`, not here. Only add an entry
    /// here once you've confirmed the domain itself doesn't resolve or
    /// serve anything for anyone (e.g. DNS_PROBE_FINISHED_NXDOMAIN in a
    /// real browser), not just "the validator got a 403".
    #[serde(default)]
    pub excluded_mirrors: Vec<String>,

    #[serde(default)]
    pub imdb_path: Option<String>,

    #[serde(default = "default_one")]
    pub pages: u32,

    #[serde(default)]
    pub selectors: Option<HtmlSelectors>,
    #[serde(default)]
    pub json_fields: Option<JsonFields>,
    #[serde(default)]
    pub request: RequestConfig,

    /// Provenance block. Optional, but required for anything that was
    /// ported from Jackett (which is most sources) — this is what lets
    /// tools/jackett-sync automatically detect a domain change upstream
    /// and open a PR against this exact file. A source with no
    /// `origin.jackett_id` is assumed hand-analyzed and is skipped by
    /// the sync bot (still fully supported, just not auto-tracked).
    #[serde(default)]
    pub origin: Option<SourceOrigin>,
}

/// Where this definition came from, and how to keep it in sync.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SourceOrigin {
    /// "jackett" | "manual" — manual means a contributor analyzed the
    /// site themselves rather than porting a Jackett Cardigann def.
    pub kind: OriginKind,
    /// Jackett's own indexer id (the `id:` field in its YAML, e.g.
    /// "1337x", "torrentsome"). Required when kind == "jackett" —
    /// this is the join key jackett-sync uses to find the upstream
    /// YAML file at
    /// github.com/Jackett/Jackett/blob/master/src/Jackett.Common/Definitions/<jackett_id>.yml
    #[serde(default)]
    pub jackett_id: Option<String>,
    /// Last time jackett-sync confirmed this source's mirrors/status
    /// still match upstream. Set automatically by the sync tool —
    /// contributors don't need to touch this.
    #[serde(default)]
    pub last_verified: Option<String>,
    /// Set to true automatically by jackett-sync when the upstream
    /// Jackett YAML this was ported from has been deleted (a 404 on
    /// fetch — exactly what happened to torrentqq.yml). The sync tool
    /// never deletes the source file itself on this signal; a
    /// maintainer decides whether to keep maintaining it independently
    /// (set kind: "manual" and clear this flag), or retire it.
    #[serde(default)]
    pub upstream_removed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OriginKind {
    Jackett,
    Manual,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpecialSiteOverride {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub mirrors: Vec<String>,
    #[serde(default)]
    pub origin: Option<SourceOrigin>,
}

fn default_true() -> bool { true }
fn default_one() -> u32 { 1 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SiteKind {
    Html,
    Json,
}

/// CSS selector set for an HTML-scraped site. Fields are plain strings
/// so they can be edited in JSON without touching Rust code;
/// `Selector::parse()` is called at request time, not at deploy time,
/// so a bad selector fails that one site's fetch rather than the build.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HtmlSelectors {
    pub row: String,
    pub title: String,
    /// "text" = use element text content; anything else = read that
    /// attribute.
    #[serde(default = "default_text")]
    pub title_attr: String,

    // Listing-page magnet (used when magnet_location == "listing")
    #[serde(default)]
    pub magnet: Option<String>,
    #[serde(default)]
    pub magnet_attr: Option<String>,
    /// If set, the magnet isn't the attribute value directly — it's
    /// URL-encoded inside a querystring parameter of that attribute's
    /// value. Set this to the param name to extract and decode it.
    #[serde(default)]
    pub magnet_querystring_param: Option<String>,

    #[serde(default = "default_listing")]
    pub magnet_location: String, // "listing" | "detail"

    // Detail-page magnet (used when magnet_location == "detail")
    #[serde(default)]
    pub detail_link: Option<String>,
    #[serde(default)]
    pub detail_link_attr: Option<String>,
    #[serde(default)]
    pub detail_magnet_selector: Option<String>,
    #[serde(default)]
    pub detail_magnet_selector_fallback: Option<String>,

    pub size: String,
    pub seeds: String,
    #[serde(default)]
    pub seeds_index: usize,
    pub peers: String,
    #[serde(default)]
    pub peers_index: usize,

    /// Optional: used when the visible title text is truncated by the
    /// site and the real title has to be decoded from a detail link's
    /// href instead.
    #[serde(default)]
    pub title_fallback_href_selector: Option<String>,
    #[serde(default = "default_title_fallback_segment")]
    pub title_fallback_href_segment: usize,

    /// Optional: site's own category label selector, folded into
    /// audio_tags if it hints at a dub/region we'd otherwise miss.
    #[serde(default)]
    pub category: Option<String>,
}

fn default_text() -> String { "text".to_string() }
fn default_listing() -> String { "listing".to_string() }
fn default_title_fallback_segment() -> usize { 3 }

/// Field-name map for a JSON-API site. Values are the JSON key names
/// in that site's own response shape.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonFields {
    #[serde(default)]
    pub results_array: String,
    pub title: String,
    /// Either "infohash" (raw hex hash, we build the magnet) or
    /// "magnet" (field already contains a full magnet: URI).
    pub infohash: String,
    #[serde(default)]
    pub infohash_is_full_magnet: bool,
    pub size: String,
    pub seeds: String,
    pub peers: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub imdb: Option<String>,
    #[serde(default)]
    pub apply_tpb_query_cleanup: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RequestConfig {
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub delay_ms: u64,
    #[serde(default)]
    pub min_seeds_for_detail_fetch: Option<u32>,
}
