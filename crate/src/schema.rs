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

    /// HTTP method for the search request. Defaults to GET (every
    /// existing source before this field was added). Some Cardigann
    /// definitions ported from Jackett submit search params as a POST
    /// body instead of a query string (e.g. TorrentBD's ajgettorrents.php)
    /// — for those, set this to "post" and provide `search_body`.
    #[serde(default)]
    pub search_method: SearchMethod,

    /// POST body template, used only when search_method == "post".
    /// Same placeholder substitution as search_path ({query}, {page}),
    /// applied after url-encoding {query} the same way. Content-Type is
    /// always application/x-www-form-urlencoded — every Cardigann POST
    /// search body observed so far is plain form-encoded, not JSON.
    #[serde(default)]
    pub search_body: Option<String>,

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

impl SiteConfig {
    /// Whether this site needs a per-user session cookie to search at
    /// all. Callers (the app-side search orchestrator, before calling
    /// generic_html::search / generic_json::search) use this to decide
    /// whether to look up a stored cookie for this site first — see
    /// AuthConfig's doc comment for the full split between this public
    /// metadata and the actual per-user secret.
    pub fn requires_auth(&self) -> bool {
        self.request.auth.is_some()
    }
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

/// HTTP method used for the search request itself. Most sites (every
/// public source so far) are Get — search params go in the URL, built
/// from `search_path`. Post is for Cardigann definitions whose search
/// endpoint expects a form-encoded POST body instead (see
/// SiteConfig::search_body).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SearchMethod {
    #[default]
    Get,
    Post,
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

    /// "magnet" (default) = this site's listing exposes a real magnet:
    /// URI, extracted via the magnet_* fields below exactly as before.
    /// "torrent_file" = this site has NO magnet on the page at all —
    /// only a link to download a .torrent file (the norm for private
    /// trackers, which use that download hit to track ratio/membership).
    /// In that mode, the SAME selector fields below (magnet /
    /// magnet_attr, or detail_link / detail_magnet_selector) are reused
    /// to point at the .torrent download link instead of a magnet —
    /// the field names stay the same to avoid doubling every selector
    /// field just for this, but the extracted URL is written into
    /// TorrentResult::torrent_file_url instead of ::magnet, and
    /// requires_torrent_auth is set to whatever
    /// SiteConfig::requires_auth() returns for this site.
    #[serde(default = "default_download_type")]
    pub download_type: String,

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
fn default_download_type() -> String { "magnet".to_string() }
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

    /// Metadata describing what this site needs to search as a logged-in
    /// user — see AuthConfig's doc comment for the full security model.
    /// `None` (the default, matching every existing public source) means
    /// exactly what it always meant: no auth, search anonymously.
    #[serde(default)]
    pub auth: Option<AuthConfig>,
}

/// Describes what kind of session a private tracker needs — NOT the
/// session itself. This struct is metadata that ships in the public
/// source JSON (same repo, same distribution as every public site's
/// config) and is identical for every user of a given tracker; it never
/// contains a secret.
///
/// The actual per-user secret (a session cookie the user obtained by
/// logging into the tracker's own website) lives ONLY on that user's
/// device, in the app's own encrypted local store — it is never part of
/// a SiteConfig, never committed to this repo, and never sent to any
/// StreamX-operated server. The engine functions that need it
/// (generic_html::search / generic_json::search) take it as a separate
/// runtime parameter (`auth_cookie: Option<&str>`) supplied by the
/// caller at search time, not read from this config. See
/// generic_html.rs's search() signature and its doc comment for exactly
/// how the two meet.
///
/// Why split it this way instead of one AuthConfig with an optional
/// cookie field: keeping the schema (this struct) and the secret
/// (device-local only) in genuinely separate types makes it a compile
/// error to accidentally serialize a live cookie into a source JSON
/// file that could get committed to sources/community/ — there's no
/// field to put it in.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthConfig {
    /// "cookie" is the only method the engine implements right now — the
    /// app shows the user a real login page for this site inside an
    /// in-app WebView, they log in exactly as they would in a normal
    /// browser, and the app captures the resulting session cookie
    /// automatically once login succeeds (via Android's CookieManager —
    /// see the app-side TrackerLoginScreen). The user never types
    /// anything into this app's own UI, and no password is ever seen or
    /// stored by it — only the cookie a real login already produced.
    /// See PrivateTrackerCookieStore for where that cookie ends up.
    ///
    /// Jackett's own login methods for private trackers also include
    /// "post" and "form" (submitting username/password to the tracker
    /// programmatically, without a visible page) — NOT implemented here.
    /// Deliberately staying with the safer, visible-WebView-login
    /// approach: this app never automates typing a tracker password
    /// anywhere, it only ever captures a cookie from a login the user
    /// performed themselves, on the tracker's own real page.
    pub method: AuthMethod,

    /// Human-readable hint shown above the login WebView for this
    /// specific site — e.g. "Log in with your account below. We'll
    /// detect when you're signed in automatically." Free text because
    /// every tracker's own quirks (2FA, a captcha, an unusual login
    /// path) differ enough that a single generic message isn't always
    /// enough context for the user.
    #[serde(default)]
    pub instructions: String,

    /// Optional CSS selector present only on a page you can reach when
    /// actually logged in (e.g. a "Logout" link, or a "My torrents"
    /// nav item) — used purely to validate that a cookie the user pasted
    /// is still working, BEFORE running an actual search with it and
    /// getting a confusing zero-results response instead of a clear
    /// "your cookie has expired" message. Checked once, against the
    /// site's own homepage/dashboard path — not run on every search.
    #[serde(default)]
    pub login_check_path: Option<String>,
    #[serde(default)]
    pub login_check_selector: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    Cookie,
}
