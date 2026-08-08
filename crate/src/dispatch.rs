// crate/src/dispatch.rs
//
// Adapted from StreamX Ultra's original indexer/config/mod.rs. Runs a
// search against ONE site by its registry key, dispatching to the
// generic HTML or JSON engine based on SiteConfig.kind. This is the
// entry point every consumer (the app, the validator, examples/) should
// go through instead of calling generic_html/generic_json directly.

use crate::registry::IndexerRegistry;
use crate::schema::SiteKind;
use crate::types::TorrentResult;

/// Supplies a per-user auth cookie for any site that needs one
/// (SiteConfig::requires_auth() == true), without this crate ever
/// knowing HOW that cookie is stored — see schema.rs's AuthConfig doc
/// comment for the full split between public site metadata (this
/// crate's job) and the actual per-user secret (the embedding app's
/// job).
///
/// This is what makes private-tracker support "dynamic" in the sense
/// StreamX Ultra needs: adding the 500th private site to the registry
/// JSON requires zero changes here or in the app's JNI layer — as soon
/// as that site's SiteConfig has an `auth` block, search_site() below
/// asks whatever AuthProvider the caller supplied for a cookie by
/// site_id, generically. The app's own implementation (backed by its
/// encrypted PrivateTrackerCookieStore) is the only place that knows
/// Android exists.
pub trait AuthProvider: Send + Sync {
    /// Return the stored `Cookie:` header value for `site_id`, or None
    /// if nothing's stored (the user hasn't logged into that tracker
    /// yet, or never will — public sites always hit this path since
    /// they have no `auth` block to trigger the lookup in the first
    /// place, see the `requires_auth()` check below).
    fn cookie_for(&self, site_id: &str) -> Option<String>;
}

/// Zero-op provider for callers that don't support private trackers at
/// all (examples/, some validator runs) — every lookup returns None,
/// meaning every site is searched unauthenticated. Public sites are
/// completely unaffected either way.
pub struct NoAuth;
impl AuthProvider for NoAuth {
    fn cookie_for(&self, _site_id: &str) -> Option<String> {
        None
    }
}

/// Run a search against ONE site by its registry key (e.g. "x1337x",
/// "tgx", "therarbg"). Returns an empty Vec if the site is disabled or
/// missing from the registry — callers don't need to special-case
/// either.
pub async fn search_site(
    client: &reqwest::Client,
    registry: &IndexerRegistry,
    site_id: &str,
    query: &str,
    imdb_id: Option<&str>,
    auth: &dyn AuthProvider,
) -> Vec<TorrentResult> {
    let site = match registry.sites.get(site_id) {
        Some(s) => s,
        None => {
            log::warn!("[indexer] no registry entry for site '{site_id}'");
            return vec![];
        }
    };
    if !site.enabled {
        log::info!("[indexer] site '{site_id}' disabled in registry, skipping");
        return vec![];
    }

    // Only sites whose config actually declares an auth requirement
    // ever ask the provider for anything — this is what keeps every
    // existing public site (the overwhelming majority of the registry)
    // completely unaffected by this feature's existence. A cookie
    // present for a site that doesn't require one is never looked up
    // in the first place, let alone sent.
    let auth_cookie = if site.requires_auth() {
        let cookie = auth.cookie_for(site_id);
        if cookie.is_none() {
            log::info!("[indexer] site '{site_id}' requires auth but no cookie stored — searching unauthenticated (likely zero results)");
        }
        cookie
    } else {
        None
    };

    match site.kind {
        SiteKind::Html => crate::generic_html::search(client, site_id, site, query, imdb_id, auth_cookie.as_deref()).await,
        SiteKind::Json => crate::generic_json::search(client, site_id, site, query, imdb_id, auth_cookie.as_deref()).await,
    }
}

/// Convenience: run search_site() across several site keys concurrently
/// and flatten the results.
pub async fn search_sites(
    client: &reqwest::Client,
    registry: &IndexerRegistry,
    site_ids: &[&str],
    query: &str,
    imdb_id: Option<&str>,
    auth: &dyn AuthProvider,
) -> Vec<TorrentResult> {
    let futures = site_ids
        .iter()
        .map(|id| search_site(client, registry, id, query, imdb_id, auth));
    let results = futures::future::join_all(futures).await;
    results.into_iter().flatten().collect()
}

/// Every registry site id that should participate in a "search
/// everything generic" pass — every enabled site in `registry.sites`
/// EXCEPT `exclude` (the caller's special-cased ids, e.g. any id that
/// also has hand-written scraping logic under crate::special and is
/// called separately, or a site the caller is already querying by name
/// for a different reason).
///
/// This is what replaced engine.rs's old `const DEDICATED_IDS: [&str; 7]`
/// (fixed 2026-07-25): that array had to be hand-edited every time a
/// public site JSON was added to sources/verified/ or sources/community/
/// — a new site would sit in the registry, fully valid and CI-verified,
/// but silently never get searched until someone remembered to add its
/// id to that array too. Deriving the list from the registry itself
/// means adding a new site JSON is now sufficient on its own — no
/// engine.rs change needed, which was the actual point of moving sites
/// to individual JSON files under sources/ in the first place.
///
/// Private trackers work through this exact same path: a site with an
/// `auth` block is not excluded here just because it needs a cookie —
/// search_site() (called per id from search_sites_dynamic below) checks
/// requires_auth() and asks the caller's AuthProvider itself. A private
/// tracker the user hasn't configured yet simply returns zero results
/// via that path (see search_site's own doc comment), same as it would
/// if it were still hardcoded — the only thing this function changes is
/// whether the id reaches search_site() at all.
pub fn dynamic_site_ids(registry: &IndexerRegistry, exclude: &[&str]) -> Vec<String> {
    registry
        .sites
        .iter()
        .filter(|(id, cfg)| cfg.enabled && !exclude.contains(&id.as_str()))
        .map(|(id, _)| id.clone())
        .collect()
}

/// Same as search_sites, but takes owned `Vec<String>` ids — the shape
/// dynamic_site_ids() returns, since a registry-derived list can't
/// borrow `&str` with a lifetime any of engine.rs's callers could
/// satisfy (the HashMap it's built from is a local temporary in most
/// call sites). Functionally identical otherwise.
pub async fn search_sites_dynamic(
    client: &reqwest::Client,
    registry: &IndexerRegistry,
    site_ids: &[String],
    query: &str,
    imdb_id: Option<&str>,
    auth: &dyn AuthProvider,
) -> Vec<TorrentResult> {
    let futures = site_ids
        .iter()
        .map(|id| search_site(client, registry, id.as_str(), query, imdb_id, auth));
    let results = futures::future::join_all(futures).await;
    results.into_iter().flatten().collect()
}
