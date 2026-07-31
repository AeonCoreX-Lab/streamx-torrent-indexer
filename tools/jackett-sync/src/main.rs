// tools/jackett-sync/src/main.rs
//
// Scheduled job (see .github/workflows/jackett-sync.yml — runs daily)
// that keeps our domains in sync with upstream Jackett automatically,
// which is the actual thing being asked for: "automatic domain
// structure verification system that analyzes Jackett and updates
// itself".
//
// HOW IT WORKS
// ────────────
// For every source under sources/verified/ and sources/community/ (and
// every entry in sources/special-sites.json) that has an
// `origin.jackett_id` set, this:
//
//   1. Fetches that indexer's Cardigann YAML straight from Jackett's
//      GitHub repo:
//      https://raw.githubusercontent.com/Jackett/Jackett/master/src/Jackett.Common/Definitions/<jackett_id>.yml
//   2. Parses out its `links:` list (Jackett's current, actively-used
//      mirrors — NOT `legacylinks:`, which is dead/historical on
//      purpose).
//   3. Compares that list against our own `mirrors[]` for that source.
//   4. If they differ, rewrites the source file's `mirrors[]` to
//      Jackett's current list (preserving everything else — selectors,
//      request config, etc. are untouched, since a domain change alone
//      doesn't imply a structure change) and bumps
//      `origin.last_verified` to now.
//   5. If the YAML fetch 404s — meaning Jackett has REMOVED that
//      indexer entirely (exactly what happened to torrentqq.yml) —
//      this flags the source as `origin.upstream_removed: true`
//      instead of touching mirrors, and reports it distinctly in the
//      summary so a human decides whether to keep maintaining it
//      independently or retire it. It never auto-deletes a source file.
//
// `--dry-run` (the default when run manually) only prints what would
// change. `--write` actually rewrites the files — this is what CI uses,
// followed by a step that opens a PR only if `git status` shows
// changes (see the workflow file).
//
// A source with no `origin.jackett_id` (kind: "manual") is skipped
// entirely — nothing to sync it against.

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use std::fs;
use std::path::{Path, PathBuf};

const JACKETT_RAW_BASE: &str =
    "https://raw.githubusercontent.com/Jackett/Jackett/master/src/Jackett.Common/Definitions";

#[derive(Parser)]
#[command(name = "jackett-sync", about = "Syncs source mirrors against upstream Jackett definitions")]
struct Cli {
    /// Repo root (contains sources/). Defaults to CWD.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// Actually rewrite files. Without this flag, only reports what
    /// would change.
    #[arg(long)]
    write: bool,
}

#[derive(serde::Deserialize)]
struct CardigannDef {
    id: String,
    #[serde(default)]
    links: Vec<String>,
}

enum SyncOutcome {
    UpToDate,
    Updated { old: Vec<String>, new: Vec<String> },
    UpstreamRemoved,
    FetchError(String),
    NoJackettId,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = reqwest::Client::builder()
        .user_agent("streamx-jackett-sync/0.1 (+https://github.com/AeonCoreX-Lab/streamx-torrent-indexer)")
        .build()?;

    let sources_dir = cli.root.join("sources");
    let mut any_changed = false;
    let mut any_removed_upstream = false;

    println!("{}", "── Syncing sources/verified + sources/community against Jackett ──".bold());
    for dir_name in ["verified", "community"] {
        let dir = sources_dir.join(dir_name);
        if !dir.exists() {
            continue;
        }
        let mut entries: Vec<PathBuf> = fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
            .collect();
        entries.sort();

        for path in entries {
            let outcome = sync_site_file(&client, &path, cli.write).await?;
            report_outcome(&path, &outcome);
            match outcome {
                SyncOutcome::Updated { .. } => any_changed = true,
                SyncOutcome::UpstreamRemoved => any_removed_upstream = true,
                _ => {}
            }
        }
    }

    println!("\n{}", "── Syncing sources/special-sites.json ──".bold());
    let special_path = sources_dir.join("special-sites.json");
    if special_path.exists() {
        let (changed, removed) = sync_special_sites(&client, &special_path, cli.write).await?;
        any_changed |= changed;
        any_removed_upstream |= removed;
    }

    if any_removed_upstream {
        println!(
            "\n{}",
            "NOTE: one or more sources were removed from upstream Jackett. \
             They were NOT deleted here — review flagged entries manually \
             (see origin.upstream_removed: true in the relevant files) and \
             decide whether to keep, mark manual, or remove."
                .yellow()
        );
    }

    if !cli.write && any_changed {
        println!(
            "\n{}",
            "Dry run — re-run with --write to apply these changes.".dimmed()
        );
    }

    Ok(())
}

async fn fetch_cardigann_links(client: &reqwest::Client, jackett_id: &str) -> Result<Option<Vec<String>>> {
    let url = format!("{JACKETT_RAW_BASE}/{jackett_id}.yml");
    let resp = client.get(&url).send().await.context("fetching Jackett definition")?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None); // upstream removed this indexer entirely
    }
    let resp = resp.error_for_status()?;
    let text = resp.text().await?;
    let def: CardigannDef = serde_yaml::from_str(&text)
        .with_context(|| format!("parsing YAML for {jackett_id}"))?;
    anyhow::ensure!(
        def.id == jackett_id,
        "fetched YAML id '{}' doesn't match requested '{jackett_id}' — possible redirect/rename upstream",
        def.id
    );
    // Normalize trailing slashes so comparisons don't false-positive.
    let links = def.links.into_iter().map(|l| l.trim_end_matches('/').to_string()).collect();
    Ok(Some(links))
}

async fn sync_site_file(client: &reqwest::Client, path: &Path, write: bool) -> Result<SyncOutcome> {
    let raw = fs::read_to_string(path)?;
    let mut value: serde_json::Value = serde_json::from_str(&raw)?;

    let jackett_id = value
        .get("origin")
        .and_then(|o| o.get("jackett_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let Some(jackett_id) = jackett_id else {
        return Ok(SyncOutcome::NoJackettId);
    };

    let upstream_links = match fetch_cardigann_links(client, &jackett_id).await {
        Ok(links) => links,
        Err(e) => return Ok(SyncOutcome::FetchError(e.to_string())),
    };

    let Some(upstream_links) = upstream_links else {
        if write {
            value["origin"]["upstream_removed"] = serde_json::Value::Bool(true);
            fs::write(path, serde_json::to_string_pretty(&value)? + "\n")?;
        }
        return Ok(SyncOutcome::UpstreamRemoved);
    };

    let our_mirrors: Vec<String> = value["mirrors"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).map(|s| s.trim_end_matches('/').to_string()).collect())
        .unwrap_or_default();

    if !upstream_links.is_empty() && our_mirrors != upstream_links {
        let old = our_mirrors.clone();
        if write {
            value["mirrors"] = serde_json::Value::Array(
                upstream_links.iter().map(|l| serde_json::Value::String(l.clone())).collect(),
            );
            value["origin"]["last_verified"] =
                serde_json::Value::String(chrono_like_now());
            fs::write(path, serde_json::to_string_pretty(&value)? + "\n")?;
        }
        return Ok(SyncOutcome::Updated { old, new: upstream_links });
    }

    if write {
        value["origin"]["last_verified"] = serde_json::Value::String(chrono_like_now());
        fs::write(path, serde_json::to_string_pretty(&value)? + "\n")?;
    }
    Ok(SyncOutcome::UpToDate)
}

/// Sync the special_sites.json map, which has a different shape
/// (site_id -> {enabled, mirrors, origin}) than the per-file sources.
async fn sync_special_sites(client: &reqwest::Client, path: &Path, write: bool) -> Result<(bool, bool)> {
    let raw = fs::read_to_string(path)?;
    let mut value: serde_json::Value = serde_json::from_str(&raw)?;
    let mut any_changed = false;
    let mut any_removed = false;

    let Some(obj) = value.as_object_mut() else {
        anyhow::bail!("special-sites.json is not a JSON object");
    };
    let keys: Vec<String> = obj.keys().cloned().collect();

    for key in keys {
        let entry = &obj[&key];
        let jackett_id = entry
            .get("origin")
            .and_then(|o| o.get("jackett_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let Some(jackett_id) = jackett_id else {
            println!("  {} {} — {}", "-".dimmed(), key, "no origin.jackett_id, skipped".dimmed());
            continue;
        };

        match fetch_cardigann_links(client, &jackett_id).await {
            Ok(Some(upstream_links)) if !upstream_links.is_empty() => {
                let ours: Vec<String> = entry["mirrors"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str()).map(|s| s.trim_end_matches('/').to_string()).collect())
                    .unwrap_or_default();
                if ours != upstream_links {
                    println!(
                        "  {} {} — {} {:?} {} {:?}",
                        "↻".yellow().bold(), key, "domain drift:".yellow(), ours, "->", upstream_links
                    );
                    any_changed = true;
                    if write {
                        obj.get_mut(&key).unwrap()["mirrors"] = serde_json::Value::Array(
                            upstream_links.iter().map(|l| serde_json::Value::String(l.clone())).collect(),
                        );
                        obj.get_mut(&key).unwrap()["origin"]["last_verified"] =
                            serde_json::Value::String(chrono_like_now());
                    }
                } else {
                    println!("  {} {} — up to date", "✓".green(), key);
                }
            }
            Ok(Some(_empty)) => {
                println!("  {} {} — {}", "?".yellow(), key, "upstream has no links[] entries".yellow());
            }
            Ok(None) => {
                println!("  {} {} — {}", "✗".red().bold(), key, "REMOVED upstream by Jackett".red());
                any_removed = true;
                if write {
                    obj.get_mut(&key).unwrap()["origin"]["upstream_removed"] = serde_json::Value::Bool(true);
                }
            }
            Err(e) => {
                println!("  {} {} — fetch error: {}", "!".red(), key, e);
            }
        }
    }

    if write && any_changed {
        fs::write(path, serde_json::to_string_pretty(&value)? + "\n")?;
    }
    Ok((any_changed, any_removed))
}

fn report_outcome(path: &Path, outcome: &SyncOutcome) {
    let name = path.file_stem().unwrap().to_string_lossy();
    match outcome {
        SyncOutcome::UpToDate => println!("  {} {} — up to date", "✓".green(), name),
        SyncOutcome::Updated { old, new } => println!(
            "  {} {} — {} {:?} {} {:?}",
            "↻".yellow().bold(),
            name,
            "domain drift:".yellow(),
            old,
            "->",
            new
        ),
        SyncOutcome::UpstreamRemoved => println!(
            "  {} {} — {}",
            "✗".red().bold(),
            name,
            "REMOVED upstream by Jackett (flagged, not deleted)".red()
        ),
        SyncOutcome::FetchError(e) => println!("  {} {} — fetch error: {}", "!".red(), name, e),
        SyncOutcome::NoJackettId => println!("  {} {} — {}", "-".dimmed(), name, "no origin.jackett_id, skipped".dimmed()),
    }
}

/// Minimal UTC RFC3339 timestamp (e.g. "2026-07-30T00:00:00Z") without
/// pulling in the `chrono` crate for one call site. Good enough for a
/// "last checked" marker — not a general-purpose calendar library.
fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let days = secs / 86_400;
    let time_of_day = secs % 86_400;
    let (h, m, s) = (time_of_day / 3600, (time_of_day % 3600) / 60, time_of_day % 60);

    // Civil-from-days algorithm (Howard Hinnant's public-domain date
    // algorithm) — converts a day count since the Unix epoch into a
    // proleptic-Gregorian (year, month, day).
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m_num = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m_num <= 2 { y + 1 } else { y };

    format!("{y:04}-{m_num:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}
