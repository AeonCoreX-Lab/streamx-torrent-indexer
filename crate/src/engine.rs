// crate/src/engine.rs
//
// Orchestrates parallel search across all indexer sites and merges
// results. This is the crate's main entry point — StreamX Ultra's JNI
// layer calls into these functions the same way it used to call its own
// in-app indexer/engine.rs.
//
// DIFFERENCE FROM THE ORIGINAL IN-APP VERSION: this crate does not own
// any cache-directory / "fetch remote config" state itself — every
// search_*() function here takes an `&IndexerRegistry` parameter
// instead of reaching for a global. That's a deliberate boundary: this
// crate only knows how to scrape given a registry; deciding WHERE that
// registry comes from (bundled dist/registry.json, a hosted URL fetch
// with on-disk caching, Android's Context.cacheDir, etc.) is the
// embedding app's job. See crate::registry::load_embedded() for the
// zero-network fallback and docs/CONSUMING.md for the recommended
// runtime-fetch-with-fallback pattern StreamX Ultra uses.
//
// Four sites (Torrentsome, TorrentTip, Nyaa, Tokyo Toshokan) have
// scraping logic too bespoke for the generic engine (regex infohash
// extraction, multi-category queries, placeholder seed handling) — they
// keep hand-written modules under crate::special, but still pull their
// mirror list from the registry's `special_sites` block, so a dead
// domain there is still fixable without a release.
//
// PRIVATE TRACKER SUPPORT (2026-07-25): every search_*() function below
// now takes an `auth: &dyn AuthProvider` parameter (see dispatch.rs),
// threaded straight through to crate::dispatch::search_site/search_sites
// — which is the ONLY place a cookie actually gets looked up, and only
// for sites whose SiteConfig declares it needs one. Nothing in this
// file branches on site identity for auth purposes; a private tracker
// added to the registry JSON with an `auth` block "just works" through
// the exact same DEDICATED_IDS/generic-dispatch path every public site
// already uses, with zero changes needed here. crate::dispatch::NoAuth
// is available for callers (examples/, some validator runs) that don't
// need private-tracker support at all.

use crate::dispatch::AuthProvider;
use crate::registry::IndexerRegistry;
use crate::types::TorrentResult;

/// Mirror list for a "special" (non-generic-engine) site, pulled from
/// the registry's special_sites block. Empty Vec if disabled or absent.
fn special_mirrors(registry: &IndexerRegistry, site_id: &str) -> Vec<String> {
    match registry.special_sites.get(site_id) {
        Some(o) if o.enabled => o.mirrors.clone(),
        _ => vec![],
    }
}

fn is_special_site_enabled(registry: &IndexerRegistry, site_id: &str) -> bool {
    registry.special_sites.get(site_id).map(|o| o.enabled).unwrap_or(true)
}

// ── Universal fallback (1337x) ──────────────────────────────────────────
//
// Whatever the category — movie, series, anime, any dub language — if
// the dedicated sources for that category come up short, 1337x gets
// queried broadly (no category restriction) as a last resort, since
// it's simply the largest general-purpose library of the sites covered.
const FALLBACK_MIN_RESULTS: usize = 3;

async fn with_1337x_fallback(
    client: &reqwest::Client,
    registry: &IndexerRegistry,
    query: &str,
    existing: Vec<TorrentResult>,
    auth: &dyn AuthProvider,
) -> Vec<TorrentResult> {
    if existing.len() >= FALLBACK_MIN_RESULTS {
        return existing;
    }
    log::info!(
        "[fallback] only {} result(s) from dedicated sources for \"{}\" — querying 1337x broadly",
        existing.len(),
        query
    );
    let mut merged = existing;
    merged.extend(crate::dispatch::search_site(client, registry, "x1337x", query, None, auth).await);
    merged
}

const DEDICATED_IDS: [&str; 7] =
    ["tgx", "kat", "torrentdownload", "extratorrent", "therarbg", "tpb", "kat_ws"];

/// Search all sites for dubbed/dual-audio results matching `query`.
///
/// imdb_id is deliberately NOT passed to the dedicated-site search
/// below. Sites with an `imdb_path` (TGx, TheRARBG) will silently
/// ignore the caller's query string entirely and search by IMDB ID
/// alone if given one — which returns whatever cut of the movie that
/// site has, with no language filtering, since IMDB search has no way
/// to express "the Hindi dub of this movie". The caller's query already
/// has the dub language baked into the title text, so searching by
/// title text is what actually targets the dub.
pub async fn search_dubbed(
    client: &reqwest::Client,
    registry: &IndexerRegistry,
    query: &str,
    _imdb_id: Option<&str>,
    auth: &dyn AuthProvider,
) -> Vec<TorrentResult> {
    let raw = crate::dispatch::search_sites(client, registry, &DEDICATED_IDS, query, None, auth).await;

    let tagged: Vec<TorrentResult> = raw.iter().cloned().filter(|r| r.is_dubbed()).collect();

    let after_1337x = with_1337x_fallback(client, registry, query, tagged, auth).await;
    let tagged_final: Vec<TorrentResult> =
        after_1337x.iter().cloned().filter(|r| r.is_dubbed()).collect();

    if !tagged_final.is_empty() {
        return dedupe_and_sort(tagged_final);
    }

    log::info!(
        "[search_dubbed] no dub-tagged results for \"{}\" — falling back to untagged matches",
        query
    );
    let mut untagged = with_1337x_fallback(client, registry, query, raw, auth).await;
    untagged.extend(search_eztvco(client, query).await);
    for r in &mut untagged {
        r.is_confirmed_dub = false;
    }
    dedupe_and_sort(untagged)
}

/// Best-effort call into eztvco. Strips quality/dub-language noise
/// before searching, since eztvco's own search engine works on the bare
/// title. eztvco is a public site with no auth block, so this
/// intentionally does not take an AuthProvider — nothing to look up.
async fn search_eztvco(client: &reqwest::Client, query: &str) -> Vec<TorrentResult> {
    let noise_re = regex::Regex::new(
        r"(?i)\b(1080p|720p|480p|2160p|4k|bluray|web-dl|webrip|hdtv|dubbed|dub|hindi|tamil|telugu|bengali|kannada|malayalam|marathi|korean|chinese|turkish|dual audio|multi audio|s\d{2}e?\d{0,2})\b"
    ).unwrap();
    let year_re = regex::Regex::new(r"\((19|20)\d{2}\)|\b(19|20)\d{2}\b").unwrap();
    let cleaned = noise_re.replace_all(query, "");
    let cleaned = year_re.replace_all(&cleaned, "");
    let title = cleaned.trim();

    if title.is_empty() {
        return vec![];
    }
    crate::special::eztvco::search(client, title).await
}

/// Plain keyword search across all sites, no dub filtering.
pub async fn search_all(
    client: &reqwest::Client,
    registry: &IndexerRegistry,
    query: &str,
    auth: &dyn AuthProvider,
) -> Vec<TorrentResult> {
    let mut merged = crate::dispatch::search_sites(client, registry, &DEDICATED_IDS, query, None, auth).await;
    merged.extend(search_eztvco(client, query).await);
    let merged = with_1337x_fallback(client, registry, query, merged, auth).await;
    dedupe_and_sort(merged)
}

// ── Merge helpers ─────────────────────────────────────────────────────

/// Remove near-duplicate releases and sort by seeds descending.
/// Universal exit gate for every search_*() function — dedupes, removes
/// adult/XXX content unconditionally, and sorts by seeds.
///
/// The adult-content filter lives HERE rather than per-site, because
/// XXX releases leak in through general-purpose sites whenever a search
/// term happens to also overlap a performer/scene title. Filtering once
/// here, after every site's results are already merged, guarantees no
/// site (existing or future) can bypass it.
fn dedupe_and_sort(mut results: Vec<TorrentResult>) -> Vec<TorrentResult> {
    use std::collections::HashSet;

    results.retain(|r| !r.is_adult_content());

    let mut seen_magnets: HashSet<String> = HashSet::new();
    let mut seen_signature: HashSet<(String, String)> = HashSet::new();

    results.retain(|r| {
        let hash = extract_btih(&r.magnet).unwrap_or_else(|| r.magnet.clone());
        if !seen_magnets.insert(hash) {
            return false;
        }
        let sig = (r.title.to_lowercase(), r.size.clone());
        seen_signature.insert(sig)
    });

    results.sort_by(|a, b| b.seeds.cmp(&a.seeds));
    results
}

fn extract_btih(magnet: &str) -> Option<String> {
    let marker = "btih:";
    let start = magnet.find(marker)? + marker.len();
    let rest = &magnet[start..];
    let end = rest.find('&').unwrap_or(rest.len());
    Some(rest[..end].to_lowercase())
}

pub async fn search_dubbed_json(
    client: &reqwest::Client,
    registry: &IndexerRegistry,
    query: &str,
    imdb_id: Option<&str>,
    auth: &dyn AuthProvider,
) -> String {
    let results = search_dubbed(client, registry, query, imdb_id, auth).await;
    serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string())
}

pub async fn search_all_json(client: &reqwest::Client, registry: &IndexerRegistry, query: &str, auth: &dyn AuthProvider) -> String {
    let results = search_all(client, registry, query, auth).await;
    serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string())
}

// ── Drama (K-drama / C-drama / Turkish drama) ───────────────────────────

pub async fn search_drama(
    client: &reqwest::Client,
    registry: &IndexerRegistry,
    query: &str,
    auth: &dyn AuthProvider,
) -> Vec<TorrentResult> {
    let mut merged = crate::dispatch::search_sites(client, registry, &DEDICATED_IDS, query, None, auth).await;

    if is_special_site_enabled(registry, "torrentsome") {
        let mirrors = special_mirrors(registry, "torrentsome");
        merged.extend(crate::special::kdrama::search_torrentsome(client, &mirrors, query).await);
    }
    if is_special_site_enabled(registry, "torrenttip") {
        let mirrors = special_mirrors(registry, "torrenttip");
        merged.extend(crate::special::kdrama::search_torrenttip(client, &mirrors, query).await);
    }

    merged.extend(search_eztvco(client, query).await);

    let merged = with_1337x_fallback(client, registry, query, merged, auth).await;
    dedupe_and_sort(merged)
}

pub async fn search_drama_english(
    client: &reqwest::Client,
    registry: &IndexerRegistry,
    query: &str,
    auth: &dyn AuthProvider,
) -> Vec<TorrentResult> {
    let mut results = search_drama(client, registry, query, auth).await;
    results.retain(|r| r.audio_tags.iter().any(|t| t == "English Dub" || t == "English Sub"));
    results
}

pub async fn search_drama_json(client: &reqwest::Client, registry: &IndexerRegistry, query: &str, auth: &dyn AuthProvider) -> String {
    let results = search_drama(client, registry, query, auth).await;
    serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string())
}

// ── Anime ────────────────────────────────────────────────────────────────
//
// Anime sources (nyaa, tokyotosho) are special-cased public sites with
// no auth block — AuthProvider isn't threaded into their calls, since
// there's nothing for them to look up. torrentdownload IS a generic
// dispatch site though, so it takes `auth` like any DEDICATED_IDS site
// would (currently always None in practice since torrentdownload has
// no auth block either, but the call site stays consistent with every
// other crate::dispatch::search_site call rather than special-casing
// "this one never needs it").

pub async fn search_anime_english(
    client: &reqwest::Client,
    registry: &IndexerRegistry,
    query: &str,
    auth: &dyn AuthProvider,
) -> Vec<TorrentResult> {
    let mut merged = Vec::new();

    if is_special_site_enabled(registry, "nyaa") {
        let mirrors = special_mirrors(registry, "nyaa");
        merged.extend(crate::special::nyaa::search_english(client, &mirrors, query).await);
    }

    let td_results = crate::dispatch::search_site(client, registry, "torrentdownload", query, None, auth).await;
    merged.extend(td_results.into_iter().filter(|r| r.title.to_lowercase().contains("anime")));

    if is_special_site_enabled(registry, "tokyotosho") {
        let mirrors = special_mirrors(registry, "tokyotosho");
        let tokyo_results = crate::special::tokyotosho::search(client, &mirrors, query).await;
        merged.extend(tokyo_results.into_iter().filter(|r| !r.title.to_lowercase().contains("raw]")));
    }

    let merged = with_1337x_fallback(client, registry, query, merged, auth).await;
    let merged: Vec<TorrentResult> = merged
        .into_iter()
        .filter(|r| {
            let t = r.title.to_lowercase();
            r.source != "1337x"
                || t.contains("anime")
                || r.audio_tags.iter().any(|tag| tag == "English Dub" || tag == "English Sub")
        })
        .collect();

    dedupe_and_sort(merged)
}

pub async fn search_anime_other_dub(
    client: &reqwest::Client,
    registry: &IndexerRegistry,
    query: &str,
) -> Vec<TorrentResult> {
    if !is_special_site_enabled(registry, "nyaa") {
        return vec![];
    }
    let mirrors = special_mirrors(registry, "nyaa");
    let results = crate::special::nyaa::search_other_dub(client, &mirrors, query).await;
    dedupe_and_sort(results)
}

pub async fn search_anime_all(
    client: &reqwest::Client,
    registry: &IndexerRegistry,
    query: &str,
) -> Vec<TorrentResult> {
    if !is_special_site_enabled(registry, "nyaa") {
        return vec![];
    }
    let mirrors = special_mirrors(registry, "nyaa");
    let results = crate::special::nyaa::search(client, &mirrors, query).await;
    dedupe_and_sort(results)
}

pub async fn search_anime_english_json(client: &reqwest::Client, registry: &IndexerRegistry, query: &str, auth: &dyn AuthProvider) -> String {
    let results = search_anime_english(client, registry, query, auth).await;
    serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string())
}

pub async fn search_anime_other_dub_json(client: &reqwest::Client, registry: &IndexerRegistry, query: &str) -> String {
    let results = search_anime_other_dub(client, registry, query).await;
    serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string())
}
