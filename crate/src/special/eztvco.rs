// src/indexer/sites/eztvco.rs
//
// eztvtorrent.co indexer — NOT the official EZTV (eztv.re/eztvx.to). This
// is an independent clone/aggregator site with its own frontend
// (ezcdn.online CDN) that re-packages torrents largely sourced from YTS
// (see the yts.gg torrent-download links on movie detail pages below).
//
// Not in Jackett's definition set — added directly from manual site
// analysis. Every selector in this file was verified against actual
// view-source HTML dumps of the homepage, a movie detail page, and a
// TV episode detail page (not the readability/markdown conversion our
// web-fetch tool normally returns, which is NOT reliable for confirming
// exact tag names/structure — see the git history of this file for the
// earlier, wrong version that assumed structure from that conversion).
//
// CONFIRMED SITE STRUCTURE:
//
//   Search        : GET /search?s={query}
//                   Same "box-item" grid markup as the homepage/listing
//                   pages — see search_result_boxes() below. Paginated
//                   via &page=2, &page=3, etc. (see has_next_page()).
//
//   Movie detail  : /movie/{slug}-{year}    e.g. /movie/daadi-ki-shaadi-2026
//   Series detail : /series/{slug}          e.g. /series/mystery-at-blind-frog-ranch
//                   (NO year suffix on series, confirmed — every sampled
//                   series URL omits it)
//   Episode detail: /episode/{series-slug}-season-{s}-episode-{e}
//
//   Download table on movie/episode detail pages:
//     <table class="table table-bordered small">
//       <thead><tr><th>Size</th><th>...</th>...</tr></thead>
//       <tbody>
//         <tr>
//           <td> 896.63 MB </td>
//           <td class="text-center">WEB.720p</td>
//           <td class="text-center"><a href="https://yts.gg/torrent/download/...">Download</a></td>
//           <td class="text-center"><a href="magnet:?xt=urn:btih:...">Magnet</a></td>
//         </tr>
//   IMPORTANT: column order is NOT consistent between page types — a
//   movie page had [Size, Quality, Torrent, Magnet] while a sampled
//   episode page had [Size, Title, Magnet, Torrent] (magnet and torrent
//   columns swapped). We therefore never rely on column position for
//   the magnet — we select `a[href^="magnet:?xt="]` directly, which is
//   unambiguous regardless of which column it lands in. The quality
//   string (e.g. "WEB.720p", "MSD.480p") IS still position-fragile, so
//   we read it from the sibling <td> immediately before the one
//   containing the magnet anchor, which held true in both samples.
//
// ROLE IN THE INDEXER: supplementary source only (like 1337x's broad
// fallback), not a dedicated tier — its catalog leans YTS-sourced
// English/original-language content plus a genuinely broad set of
// international titles (confirmed: Hindi, Tamil/Indonesian, Chinese,
// Turkish, French titles seen on the homepage alone), but it has no
// language/dub metadata of its own, so results are never tagged as a
// confirmed dub.

use anyhow::Result;
use scraper::{ElementRef, Html, Selector};
use crate::types::TorrentResult;

const MIRRORS: &[&str] = &[
    "https://eztvtorrent.co",
];

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// Search this site's real search endpoint (GET /search?s=...) and
/// resolve magnets for each matching result. This replaces the earlier,
/// wrong slug-guessing approach — view-source confirmed a real search
/// form (`<form action="https://eztvtorrent.co/search" ... ><input
/// name="s" ...>`) exists, so there is no need to guess a detail-page
/// slug at all.
pub async fn search(client: &reqwest::Client, query: &str) -> Vec<TorrentResult> {
    let q = urlencoding::encode(query);
    let path = format!("/search?s={q}");

    // CONFIRMED (view-source, "predator" query): result pages are paginated
    // via ?page=N, with a "page larger" link when a second page exists. A
    // broad/generic query like "predator" alone returns 2 pages (25 boxes
    // on page 1). Fetch page 1 first, and only fetch page 2 if the
    // pagination link is actually present — avoids a wasted request for
    // the common case (a specific movie/show title) where everything
    // fits on page 1.
    let html = match get_html(client, &path).await {
        Ok(h) => h,
        Err(e) => { log::warn!("[eztvtorrent.co] search failed: {e}"); return vec![]; }
    };
    let doc = Html::parse_document(&html);

    let mut boxes = search_result_boxes(&doc);

    if has_next_page(&doc) {
        let page2_path = format!("/search?s={q}&page=2");
        match get_html(client, &page2_path).await {
            Ok(h2) => {
                let doc2 = Html::parse_document(&h2);
                boxes.extend(search_result_boxes(&doc2));
            }
            Err(e) => log::warn!("[eztvtorrent.co] page 2 fetch failed: {e}"),
        }
    }

    if boxes.is_empty() {
        return vec![];
    }

    // RELEVANCE FILTER: the site's own search matches "predator" as a
    // loose substring against every title/description field, so a query
    // like "predator" returns unrelated results too — e.g. "National
    // Geographic Sharkfest Shark Quest: Hunt for the Apex Predator" and
    // "The TikTok Man: Catching a Predator" (confirmed via view-source).
    // Without filtering, fetch_detail_magnets() would waste requests on
    // (and could return magnets for) titles that aren't actually what
    // the caller searched for. We require every significant query word
    // to appear in the box title (case-insensitive), which keeps genuine
    // matches (e.g. "Predator 2", "The Predator") while dropping
    // coincidental substring hits.
    let relevant: Vec<String> = boxes.into_iter()
        .filter(|(title, _)| title_matches_query(title, query))
        .map(|(_, url)| url)
        .collect();

    if relevant.is_empty() {
        return vec![];
    }

    // Fetch each result's detail page concurrently to pull its magnet(s)
    // — the search results page itself only has a poster/title/IMDB
    // score per confirmed homepage markup, no magnet, so a second hop
    // per result is unavoidable here (unlike TGx/KAT-style sites where
    // the magnet is on the listing page itself).
    let futures = relevant.into_iter().take(10).map(|url| {
        let client = client.clone();
        async move { fetch_detail_magnets(&client, &url).await.unwrap_or_default() }
    });
    futures::future::join_all(futures).await.into_iter().flatten().collect()
}

/// True if every significant (len > 2) word in `query` appears somewhere
/// in `title`, case-insensitively. Short words (a, of, in, ...) are
/// ignored so they don't force spurious mismatches, matching how the
/// noise-stripped queries from engine.rs's search_eztvco() are already
/// reduced to the meaningful title words before reaching this function.
fn title_matches_query(title: &str, query: &str) -> bool {
    let title_lower = title.to_lowercase();
    let mut had_word = false;
    for word in query.to_lowercase().split_whitespace() {
        if word.len() <= 2 {
            continue;
        }
        had_word = true;
        if !title_lower.contains(word) {
            return false;
        }
    }
    // If the query had no words long enough to check (e.g. empty/short
    // after stripping), fall back to allowing the result through rather
    // than filtering everything out.
    had_word || title.is_empty()
}

// ── Search-results parsing ───────────────────────────────────────────────────

/// Extracts (title, detail-page URL) pairs from a "box-item" grid page
/// (confirmed identical markup on the homepage and, per view-source of
/// an actual /search?s= response, on the search-results page too — both
/// reuse the same box-item component, along with /movies, /tv-series,
/// and /top-imdb).
///
/// CONFIRMED markup (view-source, /search?s=predator):
///   <div class="box-item">
///     <a href="https://eztvtorrent.co/movie/x-1996" title="X"> ... </a>
///     <h3 class="h5 front_title"><a href="...">...<span>X</span></a></h3>
///     <span class="imdb-point">6.1</span>  (can be EMPTY — confirmed on
///                                            several boxes, e.g. "Predator
///                                            Hunters" — don't rely on it)
///     <a href="...movie/x-1996" class="btn btn-primary ...">Download</a>
///   </div>
/// The title text lives in `h3.front_title a span`, which we use in
/// preference to the outer `<a title="...">` attribute (both agree in
/// every sampled box, but the visible text is the one users would
/// actually compare against their search).
fn search_result_boxes(doc: &Html) -> Vec<(String, String)> {
    let box_sel = Selector::parse("div.box-item").unwrap();
    let link_sel = Selector::parse(r#"a[href*="/movie/"], a[href*="/series/"]"#).unwrap();
    let title_sel = Selector::parse("h3.front_title").unwrap();

    let mut seen = std::collections::HashSet::new();
    let mut boxes = Vec::new();
    for box_el in doc.select(&box_sel) {
        let href = match box_el.select(&link_sel).next().and_then(|a| a.value().attr("href")) {
            Some(h) => h.to_string(),
            None => continue,
        };
        if !seen.insert(href.clone()) {
            continue;
        }
        let title = box_el.select(&title_sel).next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        boxes.push((title, href));
    }
    boxes
}

/// True if the page contains a "next page" pagination link (confirmed
/// markup: `<a class="page larger" href=".../search?s=...&page=2">2</a>`
/// inside `#pagination`). We just check for the presence of any
/// `a.page.larger` link rather than parsing the page number, since we
/// only ever need to know whether page 2 exists.
fn has_next_page(doc: &Html) -> bool {
    let next_sel = Selector::parse("#pagination a.page.larger").unwrap();
    doc.select(&next_sel).next().is_some()
}

// ── Detail-page magnet extraction ────────────────────────────────────────────

async fn fetch_detail_magnets(client: &reqwest::Client, detail_url: &str) -> Result<Vec<TorrentResult>> {
    // detail_url is already absolute (came straight from an href on the
    // search-results page), so we fetch it directly rather than going
    // through get_html()'s mirror-prefixing.
    let resp = client.get(detail_url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "text/html,application/xhtml+xml")
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} for {detail_url}", resp.status());
    }
    let html = resp.text().await?;
    let doc = Html::parse_document(&html);

    let title_sel = Selector::parse("h1").unwrap();
    let page_title = doc.select(&title_sel).next()
        .map(|e| e.text().collect::<String>().trim().to_string())
        .unwrap_or_default();
    if page_title.is_empty() {
        return Ok(vec![]);
    }

    // Confirmed: magnet anchors always have href^="magnet:?xt=" and sit
    // inside a <tr> alongside a size cell and a quality-text cell.
    // Selecting the anchor directly and walking up to its row is
    // structure-order-independent (handles both observed column orders).
    let magnet_sel = Selector::parse(r#"a[href^="magnet:?xt="]"#).unwrap();
    let row_sel = Selector::parse("tr").unwrap();

    let mut results = Vec::new();
    for magnet_el in doc.select(&magnet_sel) {
        let magnet = match magnet_el.value().attr("href") {
            Some(m) => m.to_string(),
            None => continue,
        };

        // Find this anchor's enclosing <tr> to read its sibling cells
        // (size + quality) — both confirmed samples have the magnet
        // anchor's row also containing a size cell (e.g. " 896.63 MB ")
        // and a short quality-code cell (e.g. "WEB.720p", "MSD.1080p").
        let (size, quality_hint) = find_row_context(&doc, &row_sel, &magnet_el);

        let mut r = TorrentResult {
            title: format!("{page_title} {quality_hint}").trim().to_string(),
            magnet,
            size,
            // No seeder/leecher data anywhere on this site — same
            // "health unknown" convention as the Korean drama sites and
            // TorrentTip, rather than a fabricated or zero count.
            seeds: 1,
            peers: 0,
            source: "eztvtorrent.co".to_string(),
            ..Default::default()
        };
        r.parse_tags();
        results.push(r);
    }
    Ok(results)
}

/// Walks all <tr> elements looking for the one that contains this
/// specific magnet anchor (scraper's tree doesn't expose direct
/// parent-of-element lookup by identity, so we match by re-selecting
/// the magnet anchor within each candidate row rather than walking
/// upward), then reads that row's size cell (first <td>) and the
/// quality-code cell (immediately preceding the magnet's own <td>).
fn find_row_context(doc: &Html, row_sel: &Selector, magnet_el: &ElementRef) -> (String, String) {
    let magnet_href = magnet_el.value().attr("href").unwrap_or_default();
    let td_sel = Selector::parse("td").unwrap();
    let inner_magnet_sel = Selector::parse(r#"a[href^="magnet:?xt="]"#).unwrap();

    for row in doc.select(row_sel) {
        let row_has_this_magnet = row.select(&inner_magnet_sel)
            .any(|a| a.value().attr("href") == Some(magnet_href));
        if !row_has_this_magnet {
            continue;
        }

        let cells: Vec<String> = row.select(&td_sel)
            .map(|c| c.text().collect::<String>().trim().to_string())
            .collect();

        // First cell is consistently the size in both confirmed samples
        // (movie page and episode page both lead with the size <td>).
        let size = cells.first().cloned().unwrap_or_default();

        // The quality-code cell is a short alphanumeric-with-dots token
        // (e.g. "WEB.720p", "MSD.1080p", "MSD.SD") distinct from the
        // size (which always ends in "MB"/"GB") and from any cell whose
        // text is empty (the magnet/torrent button cells themselves
        // render as button text, not plain text, in our extraction).
        let quality_hint = cells.iter()
            .find(|c| !c.is_empty() && !c.ends_with("MB") && !c.ends_with("GB") && c.as_str() != size)
            .cloned()
            .unwrap_or_default();

        return (size, quality_hint);
    }
    (String::new(), String::new())
}

// ── HTTP helper ───────────────────────────────────────────────────────────────

async fn get_html(client: &reqwest::Client, path: &str) -> Result<String> {
    for mirror in MIRRORS {
        let url = format!("{mirror}{path}");
        let resp = client.get(&url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "text/html,application/xhtml+xml")
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await?;
        if resp.status().is_success() {
            return Ok(resp.text().await?);
        }
        log::warn!("[eztvtorrent.co] unexpected HTTP {} for {url}", resp.status());
    }
    anyhow::bail!("no mirror returned this path")
}
