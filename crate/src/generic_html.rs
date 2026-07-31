// src/indexer/config/generic_html.rs
//
// A single, config-driven HTML scraper that replaces the old per-site
// hardcoded modules (x1337x.rs, tgx.rs, kat.rs, etc). Behavior for any
// given site is entirely determined by that site's SiteConfig — mirrors,
// search path, and every CSS selector come from the remote JSON, not
// from Rust source. Fixing a broken selector is now a JSON edit in the
// streamx-addons repo, not an APK release.

use anyhow::Result;
use scraper::{ElementRef, Html, Selector};

use crate::schema::SiteConfig;
use crate::types::TorrentResult;

/// Run a full search against one HTML-kind site, driven by its config.
/// `page` is 1-indexed; callers loop up to `config.pages` for multi-page
/// sites (only x1337x currently needs more than 1).
pub async fn search(
    client:      &reqwest::Client,
    site_id:     &str,
    config:      &SiteConfig,
    query:       &str,
    imdb_id:     Option<&str>,
) -> Vec<TorrentResult> {
    let mut all_results = Vec::new();

    for page in 1..=config.pages.max(1) {
        let path = build_path(config, query, imdb_id, page);
        match fetch_and_parse_page(client, site_id, config, &path).await {
            Ok(mut results) => all_results.append(&mut results),
            Err(e) => {
                log::warn!("[{site_id}] page {page} failed: {e}");
                break; // don't try page 2 if page 1 already failed
            }
        }
        if config.request.delay_ms > 0 && page < config.pages {
            tokio::time::sleep(std::time::Duration::from_millis(config.request.delay_ms)).await;
        }
    }

    all_results
}

fn build_path(config: &SiteConfig, query: &str, imdb_id: Option<&str>, page: u32) -> String {
    let template = match imdb_id {
        Some(id) if config.imdb_path.is_some() => {
            config.imdb_path.as_ref().unwrap()
                .replace("{imdb_id}", id)
        }
        _ => {
            let q = urlencoding::encode(query);
            config.search_path.replace("{query}", &q)
        }
    };
    template.replace("{page}", &page.to_string())
}

async fn fetch_and_parse_page(
    client:  &reqwest::Client,
    site_id: &str,
    config:  &SiteConfig,
    path:    &str,
) -> Result<Vec<TorrentResult>> {
    let html = get_html_with_fallback(client, config, path).await?;
    parse_html(client, site_id, config, &html).await
}

async fn parse_html(
    client:  &reqwest::Client,
    site_id: &str,
    config:  &SiteConfig,
    html:    &str,
) -> Result<Vec<TorrentResult>> {
    let sel = config.selectors.as_ref()
        .ok_or_else(|| anyhow::anyhow!("site config has no selectors block"))?;

    let doc = Html::parse_document(html);

    let row_sel   = parse_selector(&sel.row)?;
    let title_sel = parse_selector(&sel.title)?;
    let size_sel  = parse_selector(&sel.size)?;
    let seeds_sel = parse_selector(&sel.seeds)?;
    let peers_sel = parse_selector(&sel.peers)?;
    let magnet_sel = sel.magnet.as_deref().map(parse_selector).transpose()?;
    let detail_link_sel = sel.detail_link.as_deref().map(parse_selector).transpose()?;
    let category_sel = sel.category.as_deref().map(parse_selector).transpose()?;
    let title_fallback_sel = sel.title_fallback_href_selector.as_deref().map(parse_selector).transpose()?;

    // Collect lightweight row metadata first; detail-page fetches (if
    // needed) happen afterward and concurrently, same pattern the old
    // x1337x.rs used.
    struct RowMeta {
        title:  String,
        size:   String,
        seeds:  u32,
        peers:  u32,
        category: Option<String>,
        magnet_or_detail: MagnetOrDetail,
    }
    enum MagnetOrDetail {
        Magnet(String),
        DetailUrl(String),
    }

    let mut metas = Vec::new();

    for row in doc.select(&row_sel) {
        let title_el = match row.select(&title_sel).next() { Some(e) => e, None => continue };
        let mut title = read_field(&title_el, &sel.title_attr);

        // 1337x-style truncation fallback (Jackett's title_optional):
        // when the visible title ends in "...", the site cut it off and
        // the FULL title lives URL-encoded in the same/a related
        // anchor's href instead (e.g. detail link slug). Decode that
        // instead of keeping the truncated text, when the config
        // provides a selector for it.
        if title.ends_with("...") {
            if let Some(fallback_sel) = &title_fallback_sel {
                if let Some(full) = row.select(fallback_sel).next()
                    .and_then(|e| e.value().attr("href"))
                    .and_then(|href| decode_title_from_href(href, sel.title_fallback_href_segment))
                {
                    title = full;
                }
            }
        }
        if title.trim().is_empty() { continue; }

        let size  = row.select(&size_sel).next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        let seeds = extract_indexed_number(&row, &seeds_sel, sel.seeds_index);
        let peers = extract_indexed_number(&row, &peers_sel, sel.peers_index);

        if seeds == 0 { continue; }

        let category = category_sel.as_ref()
            .and_then(|s| row.select(s).next())
            .map(|e| e.text().collect::<String>().trim().to_string());

        let magnet_or_detail = if sel.magnet_location == "listing" {
            let msel = match &magnet_sel { Some(s) => s, None => continue };
            let attr = sel.magnet_attr.as_deref().unwrap_or("href");
            let raw_attr_value = match row.select(msel).next().and_then(|e| e.value().attr(attr)) {
                Some(v) => v,
                None => continue,
            };
            let resolved = match &sel.magnet_querystring_param {
                Some(param) => match extract_querystring_param(raw_attr_value, param) {
                    Some(v) => v,
                    None => continue,
                },
                None => raw_attr_value.to_string(),
            };
            MagnetOrDetail::Magnet(resolved)
        } else {
            let dsel = match &detail_link_sel { Some(s) => s, None => continue };
            let attr = sel.detail_link_attr.as_deref().unwrap_or("href");
            match row.select(dsel).next().and_then(|e| e.value().attr(attr)) {
                Some(href) => MagnetOrDetail::DetailUrl(href.to_string()),
                None => continue,
            }
        };

        metas.push(RowMeta { title, size, seeds, peers, category, magnet_or_detail });
    }

    // Resolve magnets — listing-sourced ones are immediate, detail-page
    // ones need a concurrent fetch per row (bounded by min_seeds_for_detail_fetch
    // if the config sets one, to avoid hammering a site for low-value rows).
    let min_seeds = config.request.min_seeds_for_detail_fetch.unwrap_or(0);
    let base_mirror = config.mirrors.first().cloned().unwrap_or_default();

    let futures = metas.into_iter().map(|meta| {
        let client = client.clone();
        let base_mirror = base_mirror.clone();
        let detail_sel = sel.detail_magnet_selector.clone();
        let detail_fallback = sel.detail_magnet_selector_fallback.clone();
        async move {
            let magnet = match meta.magnet_or_detail {
                MagnetOrDetail::Magnet(m) => m,
                MagnetOrDetail::DetailUrl(href) => {
                    if meta.seeds < min_seeds { return None; }
                    let detail_url = if href.starts_with("http") {
                        href
                    } else {
                        format!("{base_mirror}{href}")
                    };
                    match fetch_detail_magnet(&client, &detail_url, detail_sel.as_deref(), detail_fallback.as_deref()).await {
                        Ok(m) => m,
                        Err(_) => return None,
                    }
                }
            };
            if !magnet.starts_with("magnet:") { return None; }

            let mut r = TorrentResult {
                title:  meta.title,
                magnet,
                size:   meta.size,
                seeds:  meta.seeds,
                peers:  meta.peers,
                source: String::new(), // filled in by caller with display_name
                ..Default::default()
            };
            r.parse_tags();
            if let Some(cat) = meta.category {
                fold_category_hint(&mut r, &cat);
            }
            Some(r)
        }
    });

    let mut results: Vec<TorrentResult> = futures::future::join_all(futures).await
        .into_iter().flatten().collect();

    // Tag source with the config's display name after the fact — keeps
    // the closure above free of a borrow on `config`.
    let display_name = config.display_name.clone();
    for r in &mut results {
        r.source = display_name.clone();
    }
    let _ = site_id; // reserved for future per-site logging/metrics

    Ok(results)
}

/// Fold a site's own category label into audio_tags if it hints at a
/// dub/region parse_tags() might have missed from the title alone
/// (mirrors ExtraTorrent's "in Bollywood"/"in Dubbed Movies" handling).
fn fold_category_hint(r: &mut TorrentResult, category: &str) {
    let c = category.to_lowercase();
    let hints: &[(&str, &str)] = &[
        ("bollywood", "Hindi"),
        ("dubbed", "Dubbed"),
        ("korean", "Korean"),
        ("chinese", "Chinese"),
        ("turkish", "Turkish"),
        ("anime", "Anime"),
    ];
    for (pat, tag) in hints {
        if c.contains(pat) && !r.audio_tags.iter().any(|t| t == tag) {
            r.audio_tags.push(tag.to_string());
        }
    }
}

async fn fetch_detail_magnet(
    client:   &reqwest::Client,
    url:      &str,
    primary:  Option<&str>,
    fallback: Option<&str>,
) -> Result<String> {
    let html = get_html(client, url, &Default::default()).await?;
    let doc  = Html::parse_document(&html);

    if let Some(sel_str) = primary {
        let sel = parse_selector(sel_str)?;
        if let Some(el) = doc.select(&sel).next() {
            if let Some(href) = el.value().attr("href") {
                return Ok(href.to_string());
            }
        }
    }
    if let Some(sel_str) = fallback {
        let sel = parse_selector(sel_str)?;
        if let Some(el) = doc.select(&sel).next() {
            if let Some(href) = el.value().attr("href") {
                return Ok(href.to_string());
            }
        }
    }
    anyhow::bail!("no magnet found on detail page {url}")
}

/// Recovers a truncated title from a detail-page href, mirroring
/// Jackett's title_optional field filters: `urldecode` then
/// `split("/", segment)`. 1337x hrefs look like
/// "/torrent/1234567/Movie-Name-2024-1080p-BluRay-x264-GROUP/" — after
/// URL-decoding, splitting on "/" and taking the configured segment
/// (default index 3) yields the slug, which the caller then still runs
/// through the same dash-to-space title cleanup as any other title
/// (TorrentResult::parse_tags / existing title normalization), so this
/// only needs to recover the raw slug, not fully clean it.
fn decode_title_from_href(href: &str, segment: usize) -> Option<String> {
    let decoded = urlencoding::decode(href).ok()?.into_owned();
    let parts: Vec<&str> = decoded.split('/').collect();
    parts.get(segment).map(|s| s.replace('-', " ").trim().to_string())
}

fn extract_indexed_number(row: &ElementRef, sel: &Selector, index: usize) -> u32 {
    row.select(sel)
        .nth(index)
        .map(|e| e.text().collect::<String>())
        .map(|s| s.trim().replace(',', ""))
        .filter(|s| !s.eq_ignore_ascii_case("n/a"))
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0)
}

fn read_field(el: &ElementRef, attr_or_text: &str) -> String {
    if attr_or_text == "text" {
        el.text().collect::<String>().trim().to_string()
    } else {
        el.value().attr(attr_or_text)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| el.text().collect::<String>().trim().to_string())
    }
}

fn parse_selector(s: &str) -> Result<Selector> {
    Selector::parse(s).map_err(|e| anyhow::anyhow!("bad selector '{s}': {e:?}"))
}

/// Extract and URL-decode a named querystring parameter from a URL/href
/// string. Used for sites like kickass.ws that wrap the real magnet URI
/// inside a redirector link's `?url=` parameter instead of using it
/// directly as the href.
fn extract_querystring_param(href: &str, param: &str) -> Option<String> {
    let query_start = href.find('?')? + 1;
    let query = &href[query_start..];
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next()?;
        let val = parts.next()?;
        if key == param {
            return urlencoding::decode(val).ok().map(|s| s.into_owned());
        }
    }
    None
}

// ── HTTP helpers ─────────────────────────────────────────────────────────────

async fn get_html(
    client:  &reqwest::Client,
    url:     &str,
    headers: &std::collections::HashMap<String, String>,
) -> Result<String> {
    let mut req = client.get(url);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} for {url}", resp.status());
    }
    Ok(resp.text().await?)
}

async fn get_html_with_fallback(
    client: &reqwest::Client,
    config: &SiteConfig,
    path:   &str,
) -> Result<String> {
    for mirror in &config.mirrors {
        let url = format!("{mirror}{path}");
        match get_html(client, &url, &config.request.headers).await {
            Ok(html) => return Ok(html),
            Err(e) => log::warn!("[{}] mirror {mirror} failed: {e}", config.display_name),
        }
    }
    anyhow::bail!("all mirrors failed for {path}")
}
