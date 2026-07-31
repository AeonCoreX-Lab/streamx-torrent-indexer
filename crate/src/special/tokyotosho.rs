// src/indexer/sites/tokyotosho.rs
//
// Tokyo Toshokan indexer — ported from Jackett's tokyotosho.yml
//
// KEY POINTS from Jackett YAML:
//   Base URL   :  https://www.tokyotosho.info/
//   Search URL :  search.php?terms={QUERY}&cat=0   (cat=0 = all categories)
//   Row selector: table.listing tr.category_0   (skip header row via `after: 1`)
//   Title      :  td.desc-top a[type="application/x-bittorrent"]
//   Magnet     :  a[href^="magnet:?xt="]   ← optional; some releases are
//                 .torrent-file-only with no magnet at all (we skip those,
//                 since our engine (librqbit) needs a magnet URI)
//   Size/Date  :  td.desc-bot, pipe-delimited "Size: X | Date: Y"
//   Seeders    :  td.stats > span:nth-child(1)   ← REAL data, not a
//                 placeholder (unlike Torrentsome/TorrentTip)
//   Leechers   :  td.stats > span:nth-child(2)
//
// ROLE IN THE INDEXER: secondary/fallback anime source. Nyaa.si is the
// primary — Tokyo Toshokan is queried alongside it mainly for older or
// batch releases that predate common Nyaa uploads, and as a fallback if
// Nyaa's mirrors are all unreachable.

use anyhow::Result;
use scraper::{Html, Selector};
use crate::types::TorrentResult;

const MIRRORS_FALLBACK: &[&str] = &[
    "https://www.tokyotosho.info",
    "https://www.tokyotosho.se",
];

pub async fn search(client: &reqwest::Client, mirrors: &[String], query: &str) -> Vec<TorrentResult> {
    let q = urlencoding::encode(query);
    let path = format!("/search.php?terms={q}&cat=0");
    let effective_mirrors = if mirrors.is_empty() {
        MIRRORS_FALLBACK.iter().map(|s| s.to_string()).collect::<Vec<_>>()
    } else {
        mirrors.to_vec()
    };
    fetch_results(client, &effective_mirrors, &path).await.unwrap_or_default()
}

// ── Internal ─────────────────────────────────────────────────────────────────

async fn fetch_results(client: &reqwest::Client, mirrors: &[String], path: &str) -> Result<Vec<TorrentResult>> {
    let html = get_html_with_fallback(client, mirrors, path).await?;
    let doc  = Html::parse_document(&html);

    // Jackett: rows = table.listing tr.category_0, skip first match (header)
    let row_sel   = Selector::parse("table.listing tr.category_0").unwrap();
    let title_sel = Selector::parse(r#"td.desc-top a[type="application/x-bittorrent"]"#).unwrap();
    let magnet_sel = Selector::parse(r#"a[href^="magnet:?xt="]"#).unwrap();
    let desc_bot_sel = Selector::parse("td.desc-bot").unwrap();
    let seeds_sel = Selector::parse("td.stats > span:nth-child(1)").unwrap();
    let leech_sel = Selector::parse("td.stats > span:nth-child(2)").unwrap();

    let mut results = Vec::new();

    for (i, row) in doc.select(&row_sel).enumerate() {
        // Jackett's `after: 1` skips the first matched row (table header
        // repeated with the same class on this site) — replicate that.
        if i == 0 { continue; }

        let title_el = match row.select(&title_sel).next() { Some(e) => e, None => continue };
        let title = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() { continue; }

        // Magnet is optional on this site — skip releases without one,
        // since we have no .torrent-file download pipeline, only magnet.
        let magnet = match row.select(&magnet_sel).next()
            .and_then(|e| e.value().attr("href"))
        {
            Some(m) => m.to_string(),
            None => continue,
        };

        // "Size: 1.2 GB | Date: 2024-05-01 12:00 UTC" — pipe-delimited,
        // Jackett splits on "|" and regexes each half.
        let desc_bot = row.select(&desc_bot_sel).next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default();
        let size = desc_bot.split('|').next()
            .and_then(|s| s.trim().strip_prefix("Size:"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        let seeds = row.select(&seeds_sel).next()
            .and_then(|e| e.text().collect::<String>().trim().parse::<u32>().ok())
            .unwrap_or(0);
        let peers = row.select(&leech_sel).next()
            .and_then(|e| e.text().collect::<String>().trim().parse::<u32>().ok())
            .unwrap_or(0);

        if seeds == 0 { continue; }

        let mut r = TorrentResult {
            title,
            magnet,
            size,
            seeds,
            peers,
            source: "TokyoToshokan".to_string(),
            ..Default::default()
        };
        r.parse_tags();
        results.push(r);
    }
    Ok(results)
}

// ── HTTP helpers ─────────────────────────────────────────────────────────────

async fn get_html(client: &reqwest::Client, url: &str) -> Result<String> {
    let resp = client.get(url)
        .header("Accept", "text/html,application/xhtml+xml")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} for {url}", resp.status());
    }
    Ok(resp.text().await?)
}

async fn get_html_with_fallback(client: &reqwest::Client, mirrors: &[String], path: &str) -> Result<String> {
    for mirror in mirrors {
        let url = format!("{mirror}{path}");
        match get_html(client, &url).await {
            Ok(html) => return Ok(html),
            Err(e) => log::warn!("[TokyoToshokan] mirror {mirror} failed: {e}"),
        }
    }
    anyhow::bail!("all TokyoToshokan mirrors failed for {path}")
}
