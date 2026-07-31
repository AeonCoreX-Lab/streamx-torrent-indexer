// src/indexer/sites/kdrama.rs
//
// Korean drama indexers — ported from Jackett's torrentsome.yml and
// torrenttip.yml. Both are KOREAN public trackers with no login.
// TorrentTip additionally has explicit "한국드라마" (Korean TV/Drama) and
// Netflix Movies/Series categories, which Torrentsome doesn't break out
// separately — useful as a stronger drama-specific signal when available.
//
// REMOVED (2026): TorrentQQ / torrentqq.yml. Upstream Jackett has dropped
// this definition from its Definitions folder entirely, so it's no
// longer considered a viable/maintained tracker. search_torrentqq() and
// its helpers were removed from this file and from engine.rs's
// search_drama() dispatch to match.
//
// IMPORTANT SITE LIMITATION (verified against both YAML definitions):
// Neither exposes real seeder/leecher counts — Jackett's own definitions
// hardcode `seeders: {text: 1}` because these sites don't publish swarm
// health data at all (Korean torrent sites of this type tend to be
// direct-download-oriented, DHT swarm stats aren't tracked the way
// 1337x/TGx/RARBG track them). TorrentTip additionally hardcodes a
// placeholder size (`text: 512MB`) — its real size lives on the detail
// page, not the listing, and Jackett doesn't bother fetching it. We
// surface all of this honestly by setting seeds = 1 (matching Jackett's
// own placeholder) and leaving size empty for TorrentTip, rather than
// pretending we have real data — the UI should treat a `source` of
// "Torrentsome"/"TorrentTip" as "health unknown", not "low seeders" or
// "unknown size" specifically for TorrentTip.
//
// Both require a detail-page fetch to extract the magnet/infohash —
// magnet is not present in the search listing itself.

use anyhow::Result;
use scraper::{Html, Selector};
use crate::types::TorrentResult;

/// Fallback mirrors used only if the caller passes an empty list (e.g.
/// remote config fetch failed AND the bundled default somehow didn't
/// parse — belt-and-suspenders, should be unreachable in practice since
/// loader.rs's bundled config always includes these).
const TORRENTSOME_FALLBACK: &str = "https://torrentsome256.com";
const TORRENTTIP_FALLBACK: &str = "https://torrenttip237.top";

/// Placeholder seed value used when a site provides no real swarm data.
/// Matches Jackett's own convention for these two indexers (`text: 1`).
/// TorrentEngine should treat this specially — see doc comment above.
const UNKNOWN_HEALTH_SEEDS: u32 = 1;

fn effective_mirrors(configured: &[String], fallback: &'static str) -> Vec<String> {
    if configured.is_empty() { vec![fallback.to_string()] } else { configured.to_vec() }
}

// ── Torrentsome ───────────────────────────────────────────────────────────

pub async fn search_torrentsome(client: &reqwest::Client, mirrors: &[String], query: &str) -> Vec<TorrentResult> {
    let mirrors = effective_mirrors(mirrors, TORRENTSOME_FALLBACK);
    fetch_torrentsome_list(client, &mirrors, query).await.unwrap_or_default()
}

async fn fetch_torrentsome_list(client: &reqwest::Client, mirrors: &[String], query: &str) -> Result<Vec<TorrentResult>> {
    let q = urlencoding::encode(query);
    let path = format!("/search/index?keywords={q}&search_type=0&order=time&page=1");
    let html = get_html(client, mirrors, &path).await?;
    let doc  = Html::parse_document(&html);

    // Jackett: rows = div.topic-item:not(:has(div:nth-child(3):contains("-")))
    let row_sel   = Selector::parse("div.topic-item").unwrap();
    let link_sel  = Selector::parse(r#"a[href^="/v/"]"#).unwrap();
    let size_sel  = Selector::parse("div:nth-last-child(2)").unwrap();

    let mut metas = Vec::new();
    for row in doc.select(&row_sel) {
        let link_el = match row.select(&link_sel).next() { Some(e) => e, None => continue };
        let title   = link_el.value().attr("title")
            .map(|s| s.to_string())
            .unwrap_or_else(|| link_el.text().collect::<String>());
        let title = title.trim().to_string();
        let detail = match link_el.value().attr("href") { Some(h) => h, None => continue };
        let size   = row.select(&size_sel).next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        if title.is_empty() { continue; }
        metas.push((title, detail.to_string(), size));
    }

    let base_mirror = mirrors.first().cloned().unwrap_or_else(|| TORRENTSOME_FALLBACK.to_string());
    let futures = metas.into_iter().map(|(title, detail, size)| {
        let client = client.clone();
        let base_mirror = base_mirror.clone();
        async move {
            let detail_url = format!("{base_mirror}{detail}");
            match fetch_torrentsome_magnet(&client, &detail_url).await {
                Ok(magnet) => {
                    let mut r = TorrentResult {
                        title,
                        magnet,
                        size,
                        seeds: UNKNOWN_HEALTH_SEEDS,
                        peers: 0,
                        source: "Torrentsome".to_string(),
                        ..Default::default()
                    };
                    r.parse_tags();
                    Some(r)
                }
                Err(e) => { log::warn!("[Torrentsome] magnet fetch failed: {e}"); None }
            }
        }
    });
    let results: Vec<TorrentResult> = futures::future::join_all(futures).await
        .into_iter().flatten().collect();
    Ok(results)
}

async fn fetch_torrentsome_magnet(client: &reqwest::Client, detail_url: &str) -> Result<String> {
    let html = get_html_raw(client, detail_url).await?;
    let doc  = Html::parse_document(&html);

    // Jackett: magnet directly present on detail page, hash extracted via regex
    let sel = Selector::parse(r#"a[href^="magnet:?xt="]"#).unwrap();
    if let Some(el) = doc.select(&sel).next() {
        if let Some(href) = el.value().attr("href") {
            return Ok(href.to_string());
        }
    }
    anyhow::bail!("no magnet found on {detail_url}")
}

// ── TorrentTip ────────────────────────────────────────────────────────────
//
// Jackett's torrenttip.yml search paths hit /search up to 3 pages, sorted
// by time (newest first) — there's no seeders-sort option on this site.
// Category comes through as Korean text (e.g. "[
// 한국드라마 ]") which we map to our own audio_tags via keyword matching
// in fold_torrenttip_category() below, same pattern used for
// ExtraTorrent's "in Bollywood" category hint in the generic engine.

pub async fn search_torrenttip(client: &reqwest::Client, mirrors: &[String], query: &str) -> Vec<TorrentResult> {
    let mirrors = effective_mirrors(mirrors, TORRENTTIP_FALLBACK);
    fetch_torrenttip_list(client, &mirrors, query).await.unwrap_or_default()
}

async fn fetch_torrenttip_list(client: &reqwest::Client, mirrors: &[String], query: &str) -> Result<Vec<TorrentResult>> {
    let q = urlencoding::encode(query);
    let path = format!("/search?q={q}&sort=time");
    let html = get_html(client, mirrors, &path).await?;
    let doc  = Html::parse_document(&html);

    // Jackett: rows = ul.page-list > li:has(a[href$=".html"][title])
    let row_sel  = Selector::parse(r#"ul.page-list > li:has(a[href$=".html"][title])"#).unwrap();
    let link_sel = Selector::parse(r#"a[href$=".html"][title]"#).unwrap();
    // Jackett: category = div:nth-child(2), regexp "[ (.+?) ]"
    let cat_sel  = Selector::parse("div:nth-child(2)").unwrap();
    let cat_re   = regex::Regex::new(r"\[\s*(.+?)\s*\]").expect("valid regex");

    let mut metas = Vec::new();
    for row in doc.select(&row_sel) {
        let link_el = match row.select(&link_sel).next() { Some(e) => e, None => continue };
        let title   = link_el.value().attr("title")
            .map(|s| s.to_string())
            .unwrap_or_else(|| link_el.text().collect::<String>());
        let title = title.trim().to_string();
        let detail = match link_el.value().attr("href") { Some(h) => h, None => continue };
        if title.is_empty() { continue; }

        let category = row.select(&cat_sel).next()
            .map(|e| e.text().collect::<String>())
            .and_then(|t| cat_re.captures(&t).map(|c| c[1].to_string()));

        metas.push((title, detail.to_string(), category));
    }

    let base_mirror = mirrors.first().cloned().unwrap_or_else(|| TORRENTTIP_FALLBACK.to_string());
    let futures = metas.into_iter().map(|(title, detail, category)| {
        let client = client.clone();
        let base_mirror = base_mirror.clone();
        async move {
            let detail_url = format!("{base_mirror}{detail}");
            match fetch_torrenttip_magnet(&client, &detail_url).await {
                Ok(magnet) => {
                    let mut r = TorrentResult {
                        title,
                        magnet,
                        // TorrentTip's listing has no real size field —
                        // Jackett hardcodes "512MB" as a placeholder,
                        // which we deliberately do NOT replicate since a
                        // fake size is actively misleading in a picker
                        // UI (unlike a fake seed count, which at least
                        // has an honest "health unknown" convention).
                        size: String::new(),
                        seeds: UNKNOWN_HEALTH_SEEDS,
                        peers: 0,
                        source: "TorrentTip".to_string(),
                        ..Default::default()
                    };
                    r.parse_tags();
                    if let Some(cat) = &category {
                        fold_torrenttip_category(&mut r, cat);
                    }
                    Some(r)
                }
                Err(e) => { log::warn!("[TorrentTip] magnet fetch failed: {e}"); None }
            }
        }
    });
    let results: Vec<TorrentResult> = futures::future::join_all(futures).await
        .into_iter().flatten().collect();
    Ok(results)
}

async fn fetch_torrenttip_magnet(client: &reqwest::Client, detail_url: &str) -> Result<String> {
    let html = get_html_raw(client, detail_url).await?;
    let doc  = Html::parse_document(&html);

    // Jackett: infohash from div.p-2:has(i.fa-magnet), regex-extracted
    let hash_sel = Selector::parse("div.p-2:has(i.fa-magnet)").unwrap();
    let hash_re  = regex_hash();

    for el in doc.select(&hash_sel) {
        let text = el.text().collect::<String>();
        if let Some(hash) = hash_re.find(&text) {
            return Ok(build_magnet(hash.as_str()));
        }
    }
    anyhow::bail!("no infohash found on {detail_url}")
}

/// Map TorrentTip's Korean-text category labels to audio_tags. Mirrors
/// the same "fold site's own classification into tags" pattern used for
/// ExtraTorrent's Bollywood/Dubbed categories in the generic engine —
/// this is a stronger signal than title parsing since it's the site's
/// own classification, not a guess from the release name.
fn fold_torrenttip_category(r: &mut TorrentResult, category: &str) {
    let hints: &[(&str, &str)] = &[
        ("한국드라마", "Korean"),      // Korean TV/Drama
        ("한국영화",   "Korean"),      // Korean Movies
        ("넷플릭스",   "Netflix"),     // Netflix (Movies or Series)
        ("애니메이션", "Anime"),       // Anime
        ("해외드라마", "Foreign"),     // Foreign TV (non-Korean, still Asian-market)
    ];
    for (pat, tag) in hints {
        if category.contains(pat) && !r.audio_tags.iter().any(|t| t == tag) {
            r.audio_tags.push(tag.to_string());
        }
    }
}

// ── Shared helpers ────────────────────────────────────────────────────────

fn regex_hash() -> regex::Regex {
    regex::Regex::new(r"[A-Fa-f0-9]{40}").expect("valid regex")
}

fn build_magnet(infohash: &str) -> String {
    format!(
        "magnet:?xt=urn:btih:{infohash}\
         &tr=udp://tracker.opentrackr.org:1337/announce\
         &tr=udp://open.demonii.com:1337/announce\
         &tr=udp://tracker.openbittorrent.com:80\
         &tr=udp://exodus.desync.com:6969/announce"
    )
}

// Korean sites block common Linux/bot User-Agents — Jackett notes this
// explicitly for torrentsome. Use a Windows Chrome UA for both.
const KOREAN_SITE_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/87.0.4280.88 Safari/537.35";

async fn get_html(client: &reqwest::Client, mirrors: &[String], path: &str) -> Result<String> {
    for mirror in mirrors {
        let url = format!("{mirror}{path}");
        match get_html_raw(client, &url).await {
            Ok(html) => return Ok(html),
            Err(e) => log::warn!("[KDrama] mirror {mirror} failed: {e}"),
        }
    }
    anyhow::bail!("all mirrors failed for {path}")
}

async fn get_html_raw(client: &reqwest::Client, url: &str) -> Result<String> {
    let resp = client.get(url)
        .header("User-Agent", KOREAN_SITE_UA)
        .header("Accept", "text/html,application/xhtml+xml")
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} for {url}", resp.status());
    }
    Ok(resp.text().await?)
}
