// tools/validator/src/live_check.rs
//
// Live structure-drift detection. For each valid source, this:
//
//   1. Tries every mirror in order (like the crate's own effective_mirrors
//      fallback) — first one that responds 2xx to a bare GET "wins" as
//      the mirror to test against. If NONE respond, that's a dead-domain
//      failure (the thing jackett-sync's job is to catch and fix
//      automatically before this ever fires in CI).
//   2. Builds the search URL from search_path with a fixed, bland test
//      query ("2024" — chosen because it's a plain 4-digit year that
//      returns non-empty results on essentially every general torrent
//      site without tripping adult-content or region filters).
//   3. For an HTML site: parses the response and checks `selectors.row`
//      matches at least one element, and that `selectors.title` and
//      `selectors.size`/`seeds`/`peers` each resolve inside at least
//      the first matched row. This is deliberately more thorough than
//      "row matches something" — a site can keep its outer row
//      structure but rename an inner class, which is a common, subtler
//      form of drift that a row-only check would miss.
//   4. For a JSON site: parses the response as JSON and checks the
//      `results_array` path resolves to a non-empty array, and that the
//      first element has the `title`/`infohash`/`size`/`seeds`/`peers`
//      keys.
//
// A failure here means "this site's selectors no longer match live
// HTML/JSON" — i.e. exactly the structure-drift scenario the user asked
// for automatic detection of. It does NOT try to fix anything; fixing
// selectors requires a human to look at the new HTML. What CAN be fixed
// automatically is a stale domain — see tools/jackett-sync, which
// handles that half by diffing against Jackett's own upstream
// definitions on a schedule.

use colored::Colorize;
use scraper::{Html, Selector};
use streamx_indexer::schema::SiteKind;

use crate::schema_check::FileCheck;

const TEST_QUERY: &str = "2024";

pub struct LiveResult {
    pub id: String,
    pub working_mirror: Option<String>,
    pub errors: Vec<String>,
}

pub async fn run(files: &[FileCheck], only: &[String]) -> anyhow::Result<Vec<LiveResult>> {
    let client = reqwest::Client::builder()
        .user_agent("streamx-torrent-indexer-validator/0.1 (+https://github.com/AeonCoreX-Lab/streamx-torrent-indexer)")
        .timeout(std::time::Duration::from_secs(20))
        .build()?;

    let mut out = Vec::new();
    for f in files {
        let Some(cfg) = &f.parsed else { continue };
        if !only.is_empty() && !only.contains(&cfg.id) {
            continue;
        }
        if !cfg.enabled {
            continue;
        }
        out.push(check_one(&client, cfg).await);
    }
    Ok(out)
}

async fn check_one(client: &reqwest::Client, cfg: &streamx_indexer::schema::SiteConfig) -> LiveResult {
    let mut errors = Vec::new();

    let mut working_mirror = None;
    for mirror in &cfg.mirrors {
        match client.get(mirror).send().await {
            Ok(resp) if resp.status().is_success() || resp.status().is_redirection() => {
                working_mirror = Some(mirror.clone());
                break;
            }
            Ok(resp) => {
                errors.push(format!("mirror {mirror} returned HTTP {}", resp.status()));
            }
            Err(e) => {
                errors.push(format!("mirror {mirror} unreachable: {e}"));
            }
        }
    }

    let Some(mirror) = working_mirror.clone() else {
        errors.insert(0, "ALL mirrors unreachable — domain likely dead or blocked".to_string());
        return LiveResult { id: cfg.id.clone(), working_mirror: None, errors };
    };

    let search_url = format!(
        "{}{}",
        mirror.trim_end_matches('/'),
        cfg.search_path
            .replace("{query}", &urlencoding::encode(TEST_QUERY))
            .replace("{page}", "1")
    );

    let body = match client.get(&search_url).send().await {
        Ok(resp) => match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("failed to read response body: {e}"));
                return LiveResult { id: cfg.id.clone(), working_mirror, errors };
            }
        },
        Err(e) => {
            errors.push(format!("search request failed: {e}"));
            return LiveResult { id: cfg.id.clone(), working_mirror, errors };
        }
    };

    match cfg.kind {
        SiteKind::Html => check_html(cfg, &body, &mut errors),
        SiteKind::Json => check_json(cfg, &body, &mut errors),
    }

    LiveResult { id: cfg.id.clone(), working_mirror, errors }
}

fn check_html(cfg: &streamx_indexer::schema::SiteConfig, body: &str, errors: &mut Vec<String>) {
    let Some(sel) = &cfg.selectors else {
        errors.push("kind is 'html' but selectors block is missing".to_string());
        return;
    };

    let doc = Html::parse_document(body);
    let row_sel = match Selector::parse(&sel.row) {
        Ok(s) => s,
        Err(e) => {
            errors.push(format!("selectors.row is not valid CSS: {e:?}"));
            return;
        }
    };

    let Some(first_row) = doc.select(&row_sel).next() else {
        errors.push(format!(
            "selectors.row (\"{}\") matched ZERO elements — site structure likely changed",
            sel.row
        ));
        return;
    };

    // Check title/size/seeds/peers resolve within the first matched row
    // — catches drift in an inner selector even when the outer row
    // selector still happens to match.
    for (field_name, field_sel) in [
        ("title", &sel.title),
        ("size", &sel.size),
        ("seeds", &sel.seeds),
        ("peers", &sel.peers),
    ] {
        match Selector::parse(field_sel) {
            Ok(s) => {
                if first_row.select(&s).next().is_none() {
                    errors.push(format!(
                        "selectors.{field_name} (\"{field_sel}\") matched nothing inside the first row"
                    ));
                }
            }
            Err(e) => errors.push(format!("selectors.{field_name} is not valid CSS: {e:?}")),
        }
    }
}

fn check_json(cfg: &streamx_indexer::schema::SiteConfig, body: &str, errors: &mut Vec<String>) {
    let Some(fields) = &cfg.json_fields else {
        errors.push("kind is 'json' but json_fields block is missing".to_string());
        return;
    };

    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            errors.push(format!("response is not valid JSON: {e}"));
            return;
        }
    };

    let array = if fields.results_array.is_empty() {
        &value
    } else {
        let mut cur = &value;
        for part in fields.results_array.split('.') {
            match cur.get(part) {
                Some(v) => cur = v,
                None => {
                    errors.push(format!(
                        "json_fields.results_array path \"{}\" not found in response",
                        fields.results_array
                    ));
                    return;
                }
            }
        }
        cur
    };

    let Some(items) = array.as_array() else {
        errors.push("results_array did not resolve to a JSON array".to_string());
        return;
    };
    let Some(first) = items.first() else {
        errors.push("results_array resolved to an EMPTY array — site structure or query likely changed".to_string());
        return;
    };

    for (field_name, key) in [
        ("title", &fields.title),
        ("infohash", &fields.infohash),
        ("size", &fields.size),
        ("seeds", &fields.seeds),
        ("peers", &fields.peers),
    ] {
        if first.get(key).is_none() {
            errors.push(format!(
                "json_fields.{field_name} key \"{key}\" missing from first result object"
            ));
        }
    }
}

pub fn report(results: &[LiveResult]) -> bool {
    let mut all_ok = true;
    for r in results {
        if r.errors.is_empty() {
            let mirror = r.working_mirror.as_deref().unwrap_or("?");
            println!("  {} {} {}", "✓".green(), r.id, format!("({mirror})").dimmed());
        } else {
            all_ok = false;
            println!("  {} {}", "✗".red().bold(), r.id);
            for e in &r.errors {
                println!("      {} {}", "-".red(), e);
            }
        }
    }
    if results.is_empty() {
        println!("  {}", "(nothing to live-check)".dimmed());
    }
    all_ok
}
