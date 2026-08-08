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
///
/// `auth_cookie`: the raw `Cookie:` header value for this specific site,
/// if the caller already has one stored for it (see
/// SiteConfig::requires_auth() and schema.rs's AuthConfig doc comment
/// for where this comes from — it's never read from `config` itself).
/// Ignored for sites where `config.requires_auth()` is false. Passing
/// `None` for a site that DOES require auth isn't an error here — it
/// just searches unauthenticated, which for most private trackers means
/// a login-wall response with zero parseable rows, i.e. a normal-looking
/// "no results", not a crash. The app-side caller is responsible for
/// checking requires_auth() and prompting the user for a cookie before
/// ever calling this, if it wants a clearer error than that.
pub async fn search(
    client:      &reqwest::Client,
    site_id:     &str,
    config:      &SiteConfig,
    query:       &str,
    imdb_id:     Option<&str>,
    auth_cookie: Option<&str>,
) -> Vec<TorrentResult> {
    let mut all_results = Vec::new();

    for page in 1..=config.pages.max(1) {
        let path = build_path(config, query, imdb_id, page);
        let body = build_body(config, query, page);
        match fetch_and_parse_page(client, site_id, config, &path, body.as_deref(), auth_cookie).await {
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

/// Builds the POST body for this search, if the site is configured for
/// one — same {query}/{page} placeholder substitution as build_path,
/// applied to `search_body` instead of `search_path`. Returns None for
/// any GET site (the common case) or a POST site missing search_body
/// (a config error, but this just falls back to no body rather than
/// panicking — the request will likely come back empty/login-walled,
/// which surfaces as a normal "zero results" for that site).
fn build_body(config: &SiteConfig, query: &str, page: u32) -> Option<String> {
    if config.search_method != crate::schema::SearchMethod::Post {
        return None;
    }
    let template = config.search_body.as_ref()?;
    let q = urlencoding::encode(query);
    Some(template.replace("{query}", &q).replace("{page}", &page.to_string()))
}

async fn fetch_and_parse_page(
    client:      &reqwest::Client,
    site_id:     &str,
    config:      &SiteConfig,
    path:        &str,
    body:        Option<&str>,
    auth_cookie: Option<&str>,
) -> Result<Vec<TorrentResult>> {
    let html = get_html_with_fallback(client, config, path, body, auth_cookie).await?;
    parse_html(client, site_id, config, &html, auth_cookie).await
}

async fn parse_html(
    client:      &reqwest::Client,
    site_id:     &str,
    config:      &SiteConfig,
    html:        &str,
    auth_cookie: Option<&str>,
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
        magnet_or_detail: ResolvedOrDetail,
    }
    // Named generically ("Resolved", not "Magnet") because this same
    // path now serves both magnet mode and torrent_file mode — a
    // listing-page magnet_sel match, or a resolved detail-page href,
    // either one is just "the URL string this row points at"; whether
    // it means "magnet URI" or "authenticated .torrent download link"
    // is decided by HtmlSelectors::download_type, not by this enum.
    enum ResolvedOrDetail {
        Resolved(String),
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
            ResolvedOrDetail::Resolved(resolved)
        } else {
            let dsel = match &detail_link_sel { Some(s) => s, None => continue };
            let attr = sel.detail_link_attr.as_deref().unwrap_or("href");
            match row.select(dsel).next().and_then(|e| e.value().attr(attr)) {
                Some(href) => ResolvedOrDetail::DetailUrl(href.to_string()),
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

    let requires_auth = config.requires_auth();
    let is_torrent_file_mode = sel.download_type == "torrent_file";

    let futures = metas.into_iter().map(|meta| {
        let client = client.clone();
        let base_mirror = base_mirror.clone();
        let detail_sel = sel.detail_magnet_selector.clone();
        let detail_fallback = sel.detail_magnet_selector_fallback.clone();
        // Owned clone, not a borrow — this closure runs inside
        // future::join_all where each future needs its own copy, same
        // reasoning as client/base_mirror/detail_sel above.
        let auth_cookie = auth_cookie.map(|c| c.to_string());
        async move {
            let resolved_url = match meta.magnet_or_detail {
                ResolvedOrDetail::Resolved(m) => m,
                ResolvedOrDetail::DetailUrl(href) => {
                    if meta.seeds < min_seeds { return None; }
                    let detail_url = if href.starts_with("http") {
                        href
                    } else {
                        format!("{base_mirror}{href}")
                    };
                    // torrent_file mode's "detail" page is the row's
                    // download link resolved to an absolute URL — no
                    // detail-page fetch needed (unlike magnet mode's
                    // DetailUrl, which fetches a details page to find a
                    // separate magnet selector on it), since the href
                    // itself already IS the thing we want to store.
                    if is_torrent_file_mode {
                        detail_url
                    } else {
                        match fetch_detail_magnet(&client, &detail_url, detail_sel.as_deref(), detail_fallback.as_deref(), auth_cookie.as_deref()).await {
                            Ok(m) => m,
                            Err(_) => return None,
                        }
                    }
                }
            };

            let mut r = TorrentResult {
                title:  meta.title,
                size:   meta.size,
                seeds:  meta.seeds,
                peers:  meta.peers,
                source: String::new(), // filled in by caller with display_name
                ..Default::default()
            };

            if is_torrent_file_mode {
                // Absolutize a listing-relative href the same way the
                // DetailUrl branch above does for a bare path, since
                // magnet_location=="listing" torrent_file rows never go
                // through the detail_url absolutize step.
                let absolute = if resolved_url.starts_with("http") {
                    resolved_url
                } else {
                    format!("{base_mirror}{resolved_url}")
                };
                r.torrent_file_url = Some(absolute);
                r.requires_torrent_auth = requires_auth;
            } else {
                if !resolved_url.starts_with("magnet:") { return None; }
                r.magnet = resolved_url;
            }

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
        // Only needed when the app has to look up a cookie to fetch
        // torrent_file_url later — see TorrentResult::site_id's doc
        // comment. Left empty for ordinary magnet results so the JSON
        // payload doesn't carry a meaningless id for the common case.
        if r.requires_torrent_auth {
            r.site_id = site_id.to_string();
        }
    }

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
    client:      &reqwest::Client,
    url:         &str,
    primary:     Option<&str>,
    fallback:    Option<&str>,
    auth_cookie: Option<&str>,
) -> Result<String> {
    let mut headers = std::collections::HashMap::new();
    if let Some(cookie) = auth_cookie {
        headers.insert("Cookie".to_string(), cookie.to_string());
    }
    let html = get_html(client, url, &headers).await?;
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

/// Always GET — used for detail-page fetches (fetch_detail_magnet), which
/// are per-row href lookups and never templated search requests, so they
/// never need the POST-body path below.
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

/// Search-request fetch — GET or POST depending on `body`. `Some(body)`
/// sends it as a POST with Content-Type: application/x-www-form-urlencoded
/// (every Cardigann POST search body observed so far is plain
/// form-encoded); `None` sends a plain GET, same as before this existed.
async fn fetch_search_page(
    client:  &reqwest::Client,
    url:     &str,
    body:    Option<&str>,
    headers: &std::collections::HashMap<String, String>,
) -> Result<String> {
    let mut req = match body {
        Some(b) => client.post(url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(b.to_string()),
        None => client.get(url),
    };
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
    client:      &reqwest::Client,
    config:      &SiteConfig,
    path:        &str,
    body:        Option<&str>,
    auth_cookie: Option<&str>,
) -> Result<String> {
    // Merge the site's static config headers with the runtime auth
    // cookie, rather than passing them separately — this is the ONE
    // place a private site's request headers and its per-user cookie
    // come together into a single header set, cloned once per mirror
    // attempt rather than mutating config.request.headers itself.
    let mut headers = config.request.headers.clone();
    if let Some(cookie) = auth_cookie {
        headers.insert("Cookie".to_string(), cookie.to_string());
    }

    for mirror in &config.mirrors {
        let url = format!("{mirror}{path}");
        match fetch_search_page(client, &url, body, &headers).await {
            Ok(html) => return Ok(html),
            Err(e) => log::warn!("[{}] mirror {mirror} failed: {e}", config.display_name),
        }
    }
    anyhow::bail!("all mirrors failed for {path}")
}
