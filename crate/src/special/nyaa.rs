// src/indexer/sites/nyaa.rs
//
// Nyaa.si indexer — ported from Jackett's nyaasi.yml
//
// KEY POINTS from Jackett YAML:
//   Base URL   :  https://nyaa.si/
//   Search URL :  /?q={QUERY}&c={cat}&f={filter}&s={sort}&o={order}
//   Category   :  c=1_2 → "Anime - English-translated" (i.e. English dub/sub)
//                 c=1_3 → "Anime - Non-English-translated" (other-language dub)
//                 c=1_4 → "Anime - Raw" (original Japanese, no translation)
//                 c=1_0 → "Anime" (all anime, unfiltered)
//   Row selector: tr.default, tr.danger, tr.success   (three possible row
//                 classes depending on trusted/remake flags Nyaa assigns)
//   Title      :  td:nth-child(2) a:last-of-type
//   Magnet     :  td:nth-child(3) a[href^="magnet:?"]   ← IN THE LISTING
//   Size       :  td:nth-child(4)
//   Seeders    :  td:nth-child(6)
//   Leechers   :  td:nth-child(7)
//
// Nyaa's own category split (English-translated vs Non-English-translated
// vs Raw) is a MUCH stronger dub/language signal than parsing the title,
// since it's the uploader's own classification. We query both the
// "English-translated" and "Non-English-translated" categories explicitly
// rather than relying on title text alone.

use anyhow::Result;
use scraper::{Html, Selector};
use crate::types::TorrentResult;

const MIRRORS_FALLBACK: &[&str] = &[
    "https://nyaa.si",
    "https://nyaa.land",
];

/// Nyaa category codes (see categorymappings above)
const CAT_ENGLISH_TRANSLATED: &str = "1_2";
const CAT_NON_ENGLISH_TRANSLATED: &str = "1_3";
const CAT_ALL_ANIME: &str = "1_0";

/// Search anime with English dub/sub — Nyaa's own "English-translated"
/// category, sorted by seeders so healthy swarms surface first.
pub async fn search_english(client: &reqwest::Client, mirrors: &[String], query: &str) -> Vec<TorrentResult> {
    fetch_category(client, mirrors, query, CAT_ENGLISH_TRANSLATED, "English").await
}

/// Search anime dubbed/subbed into a non-English language (Nyaa doesn't
/// split further than this — individual language is parsed from title
/// afterwards via TorrentResult::parse_tags, e.g. "Spanish", "German").
pub async fn search_other_dub(client: &reqwest::Client, mirrors: &[String], query: &str) -> Vec<TorrentResult> {
    fetch_category(client, mirrors, query, CAT_NON_ENGLISH_TRANSLATED, "Other").await
}

/// Plain search across all anime categories, no translation filter —
/// used when the caller wants raw + subbed + dubbed all together.
pub async fn search(client: &reqwest::Client, mirrors: &[String], query: &str) -> Vec<TorrentResult> {
    fetch_category(client, mirrors, query, CAT_ALL_ANIME, "").await
}

// ── Internal ─────────────────────────────────────────────────────────────────

async fn fetch_category(
    client:    &reqwest::Client,
    mirrors:   &[String],
    query:     &str,
    cat:       &str,
    tag_hint:  &str,
) -> Vec<TorrentResult> {
    let q = urlencoding::encode(query);
    // s=seeders&o=desc → sort by seeders descending (healthiest swarms first)
    let path = format!("/?q={q}&c={cat}&f=0&s=seeders&o=desc");

    let effective_mirrors = if mirrors.is_empty() {
        MIRRORS_FALLBACK.iter().map(|s| s.to_string()).collect::<Vec<_>>()
    } else {
        mirrors.to_vec()
    };

    let mut results = fetch_results(client, &effective_mirrors, &path, tag_hint).await.unwrap_or_default();
    results.sort_by(|a, b| b.seeds.cmp(&a.seeds));
    results
}

async fn fetch_results(
    client:    &reqwest::Client,
    mirrors:   &[String],
    path:      &str,
    tag_hint:  &str,
) -> Result<Vec<TorrentResult>> {
    let html = get_html_with_fallback(client, mirrors, path).await?;
    let doc  = Html::parse_document(&html);

    // Jackett: rows = tr.default, tr.danger, tr.success
    let row_sel    = Selector::parse("tr.default, tr.danger, tr.success").unwrap();
    // Jackett: title = td:nth-child(2) a:last-of-type
    let title_sel  = Selector::parse("td:nth-child(2) a:last-of-type").unwrap();
    // Jackett: magnet = td:nth-child(3) a[href^="magnet:?"]
    let magnet_sel = Selector::parse(r#"td:nth-child(3) a[href^="magnet:?"]"#).unwrap();
    let size_sel   = Selector::parse("td:nth-child(4)").unwrap();
    let seeds_sel  = Selector::parse("td:nth-child(6)").unwrap();
    let leech_sel  = Selector::parse("td:nth-child(7)").unwrap();

    let mut results = Vec::new();

    for row in doc.select(&row_sel) {
        let title_el = match row.select(&title_sel).next() { Some(e) => e, None => continue };
        // Nyaa titles sometimes carry a "title" attribute with the full
        // untruncated name; fall back to the link text otherwise.
        let title = title_el.value().attr("title")
            .map(|s| s.to_string())
            .unwrap_or_else(|| title_el.text().collect::<String>());
        let title = title.trim().to_string();
        if title.is_empty() { continue; }

        let magnet = match row.select(&magnet_sel).next()
            .and_then(|e| e.value().attr("href"))
        {
            Some(m) => m.to_string(),
            None => continue,
        };

        let size  = row.select(&size_sel).next()
            .map(|e| e.text().collect::<String>().trim().to_string())
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
            source: "Nyaa".to_string(),
            ..Default::default()
        };
        r.parse_tags();

        // Nyaa's own category is a stronger signal than title parsing —
        // fold it in explicitly so English-translated releases are always
        // tagged even if the title itself doesn't say "English" anywhere
        // (very common — subs/dubs are often implied by category alone).
        if tag_hint == "English" && !r.audio_tags.iter().any(|t| t == "English") {
            r.audio_tags.push("English".to_string());
        }

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
            Err(e) => log::warn!("[Nyaa] mirror {mirror} failed: {e}"),
        }
    }
    anyhow::bail!("all Nyaa mirrors failed for {path}")
}
