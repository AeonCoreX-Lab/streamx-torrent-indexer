// tools/cli/src/main.rs
//
// Small maintenance CLI, separate from the validator and jackett-sync
// so each tool stays focused. Currently one subcommand:
//
//   build-registry — assembles every file in sources/verified/ +
//   sources/community/ + sources/special-sites.json into a single
//   dist/registry.json. --version stamps the registry.updated field —
//   .github/workflows/release-registry.yml always passes an explicit
//   date-based version (e.g. "2026.07.31") so the file's own version
//   field matches its GitHub Release tag; without --version it falls
//   back to a UTC timestamp, which is fine for a local/manual run.
//   This is the same merge logic crate::registry::load_embedded() uses
//   internally, just writing the result to disk instead of baking it
//   into the binary via include_str! — see docs/CONSUMING.md for how
//   the app fetches the released copy at runtime.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "streamx-indexer-cli")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Merge sources/ into a single dist/registry.json for hosting.
    BuildRegistry {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value = "dist/registry.json")]
        out: PathBuf,
        /// Stamped into registry.updated. If omitted, falls back to a
        /// UTC timestamp — fine for local/manual runs, but
        /// .github/workflows/release-registry.yml always passes an
        /// explicit date-based version (e.g. "2026.07.31" or
        /// "2026.07.31.1" for a same-day second release) so the
        /// version in the file matches the GitHub Release tag exactly.
        #[arg(long)]
        version: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::BuildRegistry { root, out, version } => build_registry(&root, &out, version),
    }
}

fn build_registry(root: &PathBuf, out: &PathBuf, version: Option<String>) -> Result<()> {
    let sources_dir = root.join("sources");

    let mut verified_owned = Vec::new();
    let mut community_owned = Vec::new();
    read_json_dir(&sources_dir.join("verified"), &mut verified_owned)?;
    read_json_dir(&sources_dir.join("community"), &mut community_owned)?;

    let verified: Vec<(&str, &str)> = verified_owned
        .iter()
        .map(|(name, content)| (name.as_str(), content.as_str()))
        .collect();
    let community: Vec<(&str, &str)> = community_owned
        .iter()
        .map(|(name, content)| (name.as_str(), content.as_str()))
        .collect();

    let special_path = sources_dir.join("special-sites.json");
    let special_json = if special_path.exists() {
        fs::read_to_string(&special_path)?
    } else {
        "{}".to_string()
    };

    let mut registry = streamx_indexer::registry::build_from_files(&verified, &community, &special_json)
        .context("assembling registry from sources/")?;
    registry.updated = version.unwrap_or_else(now_rfc3339_ish);

    let out_path = root.join(out);
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&out_path, serde_json::to_string_pretty(&registry)?)?;

    println!(
        "Wrote {} — {} verified site(s), {} community site(s), {} special site(s)",
        out_path.display(),
        verified.len(),
        community.len(),
        registry.special_sites.len()
    );
    Ok(())
}

fn read_json_dir(dir: &PathBuf, out: &mut Vec<(String, String)>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
        .collect();
    entries.sort();
    for path in entries {
        let filename = path.file_name().unwrap().to_string_lossy().to_string();
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        out.push((filename, content));
    }
    Ok(())
}

fn now_rfc3339_ish() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let days = secs / 86_400;
    let time_of_day = secs % 86_400;
    let (h, m, s) = (time_of_day / 3600, (time_of_day % 3600) / 60, time_of_day % 60);
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
