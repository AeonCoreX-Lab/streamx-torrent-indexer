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
