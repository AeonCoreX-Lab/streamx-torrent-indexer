// src/indexer/config/generic_json.rs
//
// Config-driven JSON-API client — replaces the hardcoded struct field
// access in therarbg.rs/tpb.rs. Field NAMES (which JSON key is "title",
// which is "seeders", etc) come from the remote config's json_fields
// block, so if TheRARBG or TPB ever rename a response field, that's a
// JSON edit in streamx-addons, not a Rust code change + APK release.
//
// Uses serde_json::Value generically rather than a typed struct per
// site, since the whole point is that the field layout isn't known at
// compile time.

use anyhow::Result;
use serde_json::Value;

use crate::schema::SiteConfig;
use crate::types::TorrentResult;

/// See generic_html.rs's search() doc comment for the full explanation
/// of `auth_cookie` — same contract here: the caller's per-user stored
/// cookie for this site, or None to search unauthenticated.
pub async fn search(
    client:      &reqwest::Client,
    site_id:     &str,
    config:      &SiteConfig,
    query:       &str,
    imdb_id:     Option<&str>,
    auth_cookie: Option<&str>,
) -> Vec<TorrentResult> {
    let fields = match &config.json_fields {
        Some(f) => f,
        None => {
            log::warn!("[{site_id}] JSON site config missing json_fields block");
            return vec![];
        }
    };

    let path = build_path(config, query, imdb_id);
    let body = build_body(config, query);

    for mirror in &config.mirrors {
        let url = format!("{mirror}{path}");
        match fetch_and_parse(client, config, fields, &url, body.as_deref(), auth_cookie).await {
            Ok(results) if !results.is_empty() => return results,
            Ok(_) => continue, // empty, try next mirror
            Err(e) => log::warn!("[{site_id}] mirror {mirror} failed: {e}"),
        }
    }
    vec![]
}

fn build_path(config: &SiteConfig, query: &str, imdb_id: Option<&str>) -> String {
    match imdb_id {
        Some(id) if config.imdb_path.is_some() => {
            config.imdb_path.as_ref().unwrap().replace("{imdb_id}", id)
        }
        _ => {
            let cleaned = if config.json_fields.as_ref()
                .map(|f| f.apply_tpb_query_cleanup)
                .unwrap_or(false)
            {
                clean_tpb_query(query)
            } else {
                query.to_string()
            };
            let q = urlencoding::encode(&cleaned);
            config.search_path.replace("{query}", &q)
        }
    }
}

/// See generic_html.rs's build_body doc comment — identical contract
/// here: {query}/{page} substitution into search_body, only when
/// search_method == Post. JSON sites don't currently template {page}
/// into the body (none of the ported ones paginate via POST), but the
/// substitution is harmless if a future config includes it.
fn build_body(config: &SiteConfig, query: &str) -> Option<String> {
    if config.search_method != crate::schema::SearchMethod::Post {
        return None;
    }
    let template = config.search_body.as_ref()?;
    let cleaned = if config.json_fields.as_ref()
        .map(|f| f.apply_tpb_query_cleanup)
        .unwrap_or(false)
    {
        clean_tpb_query(query)
    } else {
        query.to_string()
    };
    let q = urlencoding::encode(&cleaned);
    Some(template.replace("{query}", &q))
}

/// Port of thepiratebay.yml's keywordsfilters — apibay's search engine
/// handles both cases (a bare "it's" and CJK text) poorly, per Jackett's
/// own filter comments:
///   1. re_replace \bit's\b -> ""   (case-insensitive, standalone word only)
///   2. re_replace ([\p{IsCJKUnifiedIdeographs}\W]+) -> "."
///      (any run of CJK ideographs, optionally mixed with adjacent
///      non-word punctuation, collapses to a single ".")
///   3. tolower
fn clean_tpb_query(query: &str) -> String {
    // Step 1: strip standalone "it's" (word-boundary, case-insensitive).
    // Rust's regex crate has no \b Unicode word-boundary surprises here
    // since "it's" is plain ASCII — a simple case-insensitive literal
    // replace bounded by non-alphanumeric neighbors is sufficient and
    // avoids pulling in a heavier regex dependency for one filter.
    let no_its = strip_standalone_its(query);

    // Step 2: collapse CJK-ideograph runs (+ adjacent punctuation) to ".".
    let mut collapsed = String::with_capacity(no_its.len());
    let mut in_cjk_run = false;
    for ch in no_its.chars() {
        if is_cjk_unified_ideograph(ch) || (in_cjk_run && !ch.is_alphanumeric() && ch != ' ') {
            if !in_cjk_run {
                collapsed.push('.');
            }
            in_cjk_run = true;
        } else {
            in_cjk_run = false;
            collapsed.push(ch);
        }
    }

    // Step 3: lowercase.
    collapsed.to_lowercase()
}

fn strip_standalone_its(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut result = String::with_capacity(s.len());
    let bytes: Vec<char> = s.chars().collect();
    let lower_bytes: Vec<char> = lower.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        // Look for "it's" (4 chars) at this position, case-insensitively,
        // with a non-alphanumeric (or start/end of string) boundary on
        // both sides — matching \bit's\b.
        let is_match = i + 4 <= lower_bytes.len()
            && lower_bytes[i..i + 4] == ['i', 't', '\'', 's']
            && (i == 0 || !bytes[i - 1].is_alphanumeric())
            && (i + 4 == bytes.len() || !bytes[i + 4].is_alphanumeric());
        if is_match {
            i += 4;
            continue;
        }
        result.push(bytes[i]);
        i += 1;
    }
    result
}

/// Same ranges as regex's \p{IsCJKUnifiedIdeographs} — the main CJK
/// Unified Ideographs block (U+4E00–U+9FFF). Jackett's filter uses the
/// .NET Unicode category of the same name, which covers this block.
fn is_cjk_unified_ideograph(ch: char) -> bool {
    matches!(ch as u32, 0x4E00..=0x9FFF)
}

async fn fetch_and_parse(
    client:      &reqwest::Client,
    config:      &SiteConfig,
    fields:      &crate::schema::JsonFields,
    url:         &str,
    body:        Option<&str>,
    auth_cookie: Option<&str>,
) -> Result<Vec<TorrentResult>> {
    let mut req = match body {
        Some(b) => client.post(url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(b.to_string()),
        None => client.get(url),
    };
    for (k, v) in &config.request.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(cookie) = auth_cookie {
        req = req.header("Cookie", cookie);
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} for {url}", resp.status());
    }

    let json: Value = resp.json().await?;
    let array = navigate_to_array(&json, &fields.results_array)?;

    let mut results = Vec::new();
    for item in array {
        if let Some(r) = item_to_result(item, fields, &config.display_name) {
            results.push(r);
        }
    }
    Ok(results)
}

/// Follow a dotted path (e.g. "data.items") to find the results array.
/// An empty path means the response body itself is the array.
fn navigate_to_array(body: &Value, path: &str) -> Result<Vec<Value>> {
    if path.is_empty() {
        return body.as_array().cloned()
            .ok_or_else(|| anyhow::anyhow!("response body is not a JSON array"));
    }
    let mut current = body;
    for segment in path.split('.') {
        current = current.get(segment)
            .ok_or_else(|| anyhow::anyhow!("results_array path '{path}' not found in response"))?;
    }
    current.as_array().cloned()
        .ok_or_else(|| anyhow::anyhow!("results_array path '{path}' did not resolve to an array"))
}

fn item_to_result(
    item:         Value,
    fields:       &crate::schema::JsonFields,
    display_name: &str,
) -> Option<TorrentResult> {
    let title = get_str(&item, &fields.title)?;

    let magnet = if fields.infohash_is_full_magnet {
        get_str(&item, &fields.infohash)?
    } else {
        let hash = get_str(&item, &fields.infohash)?;
        if hash.is_empty() { return None; }
        build_magnet(&hash, &title)
    };

    let size  = get_str_any(&item, &fields.size).unwrap_or_default();
    let seeds = get_number(&item, &fields.seeds).unwrap_or(0);
    let peers = get_number(&item, &fields.peers).unwrap_or(0);

    if seeds == 0 { return None; }

    let mut r = TorrentResult {
        title,
        magnet,
        size,
        seeds,
        peers,
        source: display_name.to_string(),
        ..Default::default()
    };
    r.parse_tags();

    if let Some(cat_field) = &fields.category {
        if let Some(cat) = get_str_any(&item, cat_field) {
            if cat.to_lowercase().contains("xxx") { return None; }
        }
    }

    Some(r)
}

fn get_str(item: &Value, field: &str) -> Option<String> {
    item.get(field)?.as_str().map(|s| s.to_string())
}

/// Like get_str, but also accepts a JSON number/bool by stringifying it —
/// some APIs (TPB) return sizes/seeds as strings, others might not.
fn get_str_any(item: &Value, field: &str) -> Option<String> {
    let v = item.get(field)?;
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn get_number(item: &Value, field: &str) -> Option<u32> {
    let v = item.get(field)?;
    match v {
        Value::Number(n) => n.as_u64().map(|x| x as u32),
        Value::String(s) => s.trim().replace(',', "").parse::<u32>().ok(),
        _ => None,
    }
}

fn build_magnet(infohash: &str, title: &str) -> String {
    let dn = urlencoding::encode(title);
    format!(
        "magnet:?xt=urn:btih:{infohash}&dn={dn}\
         &tr=udp://tracker.opentrackr.org:1337/announce\
         &tr=udp://open.demonii.com:1337/announce\
         &tr=udp://tracker.openbittorrent.com:80\
         &tr=udp://exodus.desync.com:6969/announce\
         &tr=udp://tracker.torrent.eu.org:451/announce"
    )
}
