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

    match site.kind {
        SiteKind::Html => crate::generic_html::search(client, site_id, site, query, imdb_id).await,
        SiteKind::Json => crate::generic_json::search(client, site_id, site, query, imdb_id).await,
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
) -> Vec<TorrentResult> {
    let futures = site_ids
        .iter()
        .map(|id| search_site(client, registry, id, query, imdb_id));
    let results = futures::future::join_all(futures).await;
    results.into_iter().flatten().collect()
}
