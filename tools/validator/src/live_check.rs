// tools/validator/src/live_check.rs
//
// Live structure-drift detection. For each valid source, this:
//
//   1. Tries every mirror in order (like the crate's own effective_mirrors
//      fallback) — first one that returns an actual page (2xx/3xx) "wins"
//      as the mirror to test the search against. See MirrorProbe below
//      for how a mirror's outcome is classified — this step deliberately
//      does NOT treat every non-2xx response the same way.
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
//
// ── BLOCKED vs DEAD (why this file doesn't just check status.is_success()) ──
//
// A mirror behind Cloudflare (or any bot-challenge WAF) returns 403 or
// 503 to a plain scripted GET even though a real browser reaches it
// fine — the site is live, it's just refusing this specific unverified
// client. Treating that identically to a DNS failure or connection
// refusal (an ACTUALLY dead domain) produces false "remove this mirror"
// signals on mirrors that are working perfectly well for real users —
// exactly the failure mode that prompted this comment to be written.
//
// So every mirror probe below is classified into one of three outcomes
// (see MirrorProbe): Live (2xx/3xx — use it), Blocked (403/503/429 —
// reachable, but a bot wall stopped us; skip to the next mirror, don't
// treat this one as dead), or Dead (connection error, timeout, DNS
// failure, or any other status — genuinely unreachable, a real pruning
// candidate). The final report distinguishes "all mirrors dead" from
// "all mirrors blocked" from "some blocked, one alive" so a human
// reading CI output knows whether a mirror needs removing or is simply
// out of reach for THIS specific bot, not for users.
//
// This also now sends the site's own `request.headers` (Accept,
// Accept-Language, etc. — same config the real production engine in
// crate/src/generic_html.rs already applies) plus reasonable default
// browser-like headers, instead of a bare bot user-agent with nothing
// else — a source author who already tuned headers to get past a
// mirror's basic bot-filtering shouldn't have that tuning ignored by
// the validator alone.

use colored::Colorize;
use scraper::{Html, Selector};
use streamx_indexer::schema::SiteKind;

use crate::schema_check::FileCheck;

const TEST_QUERY: &str = "2024";

/// Outcomes commonly returned by a bot-challenge/WAF layer (Cloudflare,
/// and others that follow the same convention) in front of a mirror
/// that is otherwise live. Not a hard failure for the mirror itself —
/// see the module doc above.
const BLOCKED_STATUSES: [u16; 3] = [403, 503, 429];

pub struct LiveResult {
    pub id: String,
    pub working_mirror: Option<String>,
    /// Mirrors that responded but were bot-challenge-blocked (403/503/429)
    /// rather than genuinely unreachable — NOT counted as failures, but
    /// surfaced so a human can tell "CI can't get past this WAF" apart
    /// from "this mirror doesn't work for anyone".
    pub blocked_mirrors: Vec<(String, u16)>,
    /// Mirrors that were genuinely unreachable (connection error, DNS
    /// failure, timeout, or a non-blocked non-2xx/3xx status like a
    /// plain 404) — these ARE real pruning candidates.
    pub dead_mirrors: Vec<(String, String)>,
    pub errors: Vec<String>,
}

pub async fn run(files: &[FileCheck], only: &[String]) -> anyhow::Result<Vec<LiveResult>> {
    let client = build_client()?;

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

fn build_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        // A realistic desktop-browser UA. The validator identifying
        // itself honestly (its old UA string) is part of what made
        // Cloudflare-style filters treat every request as a bot in the
        // first place — production's generic_html.rs doesn't send a
        // bot-identifying UA either, so this brings the validator's
        // request profile in line with what actually works.
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        )
        .timeout(std::time::Duration::from_secs(20))
        .build()
}

/// Applies the site's own configured request.headers (Accept,
/// Accept-Language, etc. — same as production's generic_html.rs) plus
/// sane defaults for anything the source didn't specify. A source
/// author who already tuned headers to get a mirror past basic
/// bot-filtering has that tuning respected here, not ignored.
fn apply_headers(
    mut req: reqwest::RequestBuilder,
    cfg: &streamx_indexer::schema::SiteConfig,
) -> reqwest::RequestBuilder {
    const DEFAULTS: &[(&str, &str)] = &[
        ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
        ("Accept-Language", "en-US,en;q=0.9"),
    ];
    for (k, v) in DEFAULTS {
        if !cfg.request.headers.contains_key(*k) {
            req = req.header(*k, *v);
        }
    }
    for (k, v) in &cfg.request.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    req
}

enum MirrorProbe {
    Live,
    Blocked(u16),
    Dead(String),
}

async fn probe_mirror(
    client: &reqwest::Client,
    cfg: &streamx_indexer::schema::SiteConfig,
    mirror: &str,
) -> MirrorProbe {
    let req = apply_headers(client.get(mirror), cfg);
    match req.send().await {
        Ok(resp) if resp.status().is_success() || resp.status().is_redirection() => MirrorProbe::Live,
        Ok(resp) if BLOCKED_STATUSES.contains(&resp.status().as_u16()) => {
            MirrorProbe::Blocked(resp.status().as_u16())
        }
        Ok(resp) => MirrorProbe::Dead(format!("HTTP {}", resp.status())),
        Err(e) => MirrorProbe::Dead(e.to_string()),
    }
}

async fn check_one(client: &reqwest::Client, cfg: &streamx_indexer::schema::SiteConfig) -> LiveResult {
    let mut errors = Vec::new();
    let mut blocked_mirrors = Vec::new();
    let mut dead_mirrors = Vec::new();

    let mut working_mirror = None;
    for mirror in &cfg.mirrors {
        match probe_mirror(client, cfg, mirror).await {
            MirrorProbe::Live => {
                working_mirror = Some(mirror.clone());
                break;
            }
            MirrorProbe::Blocked(status) => blocked_mirrors.push((mirror.clone(), status)),
            MirrorProbe::Dead(reason) => dead_mirrors.push((mirror.clone(), reason)),
        }
    }

    let Some(mirror) = working_mirror.clone() else {
        if !blocked_mirrors.is_empty() && dead_mirrors.is_empty() {
            // Every mirror is reachable but bot-walled — NOT a domain
            // problem. Reported distinctly so nobody prunes a live
            // mirror based on this alone; it needs a real-browser or
            // JS-challenge-solving check to confirm either way, which
            // is out of scope for this validator.
            errors.push(format!(
                "all {} mirror(s) reachable but bot-challenge-blocked (no genuine dead mirror found) — \
                 cannot confirm structure via automated fetch; verify manually in a real browser \
                 before assuming this source is broken",
                blocked_mirrors.len()
            ));
        } else if !dead_mirrors.is_empty() && blocked_mirrors.is_empty() {
            errors.push("ALL mirrors genuinely unreachable — domain likely dead, safe to investigate removing".to_string());
        } else {
            errors.push(format!(
                "no working mirror — {} genuinely dead, {} bot-challenge-blocked",
                dead_mirrors.len(),
                blocked_mirrors.len()
            ));
        }
        return LiveResult { id: cfg.id.clone(), working_mirror: None, blocked_mirrors, dead_mirrors, errors };
    };

    let search_url = format!(
        "{}{}",
        mirror.trim_end_matches('/'),
        cfg.search_path
            .replace("{query}", &urlencoding::encode(TEST_QUERY))
            .replace("{page}", "1")
    );

    // Mirrors the engine's own GET/POST branch (see generic_html.rs /
    // generic_json.rs's fetch_search_page) — a POST-configured source
    // validated with a plain GET would hit the wrong endpoint behavior
    // and produce a false "selectors don't match" failure that has
    // nothing to do with the selectors actually being wrong.
    let search_body = if cfg.search_method == streamx_indexer::schema::SearchMethod::Post {
        cfg.search_body.as_ref().map(|b| {
            b.replace("{query}", &urlencoding::encode(TEST_QUERY))
                .replace("{page}", "1")
        })
    } else {
        None
    };
    let base_req = match &search_body {
        Some(b) => client.post(&search_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(b.clone()),
        None => client.get(&search_url),
    };
    let search_req = apply_headers(base_req, cfg);
    let body = match search_req.send().await {
        Ok(resp) if BLOCKED_STATUSES.contains(&resp.status().as_u16()) => {
            // The homepage let us through but the search endpoint
            // itself is challenge-walled (some sites gate search
            // specifically, e.g. to slow down scraping). Same
            // "blocked, not dead" treatment — don't fail selectors
            // that were never actually exercised.
            errors.push(format!(
                "mirror {mirror} homepage was reachable, but the search request itself returned HTTP {} \
                 (bot-challenge-blocked) — cannot verify selectors against this mirror right now",
                resp.status()
            ));
            return LiveResult { id: cfg.id.clone(), working_mirror: Some(mirror), blocked_mirrors, dead_mirrors, errors };
        }
        Ok(resp) => match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("failed to read response body: {e}"));
                return LiveResult { id: cfg.id.clone(), working_mirror: Some(mirror), blocked_mirrors, dead_mirrors, errors };
            }
        },
        Err(e) => {
            errors.push(format!("search request failed: {e}"));
            return LiveResult { id: cfg.id.clone(), working_mirror: Some(mirror), blocked_mirrors, dead_mirrors, errors };
        }
    };

    match cfg.kind {
        SiteKind::Html => check_html(cfg, &body, &mut errors),
        SiteKind::Json => check_json(cfg, &body, &mut errors),
    }

    LiveResult { id: cfg.id.clone(), working_mirror: Some(mirror), blocked_mirrors, dead_mirrors, errors }
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

/// A source's live-check result falls into one of three buckets:
///   - clean pass (no errors at all)
///   - blocked-only failure (every error is "no working mirror because
///     everything reachable was bot-challenge-blocked" — NOT evidence
///     of a real problem, just that this specific requester got walled)
///   - a real failure (dead mirrors, structure drift, malformed
///     response, etc.)
///
/// report() always PRINTS every source's outcome. Whether a
/// blocked-only outcome fails the overall run is controlled by
/// `tolerate_blocked` — CI passes true (see main.rs's --tolerate-blocked)
/// because GitHub Actions runner IPs commonly sit on Cloudflare's own
/// datacenter/bot blocklists independent of request headers, so
/// "blocked from CI" isn't reliable evidence the source is actually
/// broken for real users. A local/manual run without that flag still
/// fails on a blocked-only outcome, since a human reviewing a
/// suspicious source may want to see every non-clean result as
/// worth a second look.
pub fn report(results: &[LiveResult], tolerate_blocked: bool) -> bool {
    let mut all_ok = true;
    for r in results {
        if r.errors.is_empty() {
            let mirror = r.working_mirror.as_deref().unwrap_or("?");
            print!("  {} {} {}", "✓".green(), r.id, format!("({mirror})").dimmed());
            if !r.blocked_mirrors.is_empty() {
                print!(
                    " {}",
                    format!("[{} other mirror(s) bot-blocked, skipped]", r.blocked_mirrors.len()).yellow()
                );
            }
            println!();
            continue;
        }

        let is_blocked_only = r.working_mirror.is_none()
            && !r.blocked_mirrors.is_empty()
            && r.dead_mirrors.is_empty();

        if is_blocked_only && tolerate_blocked {
            println!("  {} {} {}", "~".yellow().bold(), r.id, "(bot-challenge-blocked from this runner, not counted as failure)".dimmed());
        } else {
            all_ok = false;
            println!("  {} {}", "✗".red().bold(), r.id);
        }
        for e in &r.errors {
            println!("      {} {}", "-".red(), e);
        }
        for (mirror, status) in &r.blocked_mirrors {
            println!("      {} {} — bot-challenge-blocked (HTTP {status})", "~".yellow(), mirror);
        }
        for (mirror, reason) in &r.dead_mirrors {
            println!("      {} {} — {}", "x".red(), mirror, reason);
        }
    }
    if results.is_empty() {
        println!("  {}", "(nothing to live-check)".dimmed());
    }
    all_ok
}
