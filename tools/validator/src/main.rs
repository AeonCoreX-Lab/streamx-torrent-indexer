// tools/validator/src/main.rs
//
// CI gate for this repo. Two things it checks, either or both:
//
//   1. SCHEMA validation (always, fast, no network) — every file under
//      sources/verified/ and sources/community/ must parse as valid
//      JSON matching schema/source.schema.json, `id` must match the
//      filename, and `id` must not collide between the two
//      directories.
//
//   2. LIVE smoke-test (`--live` flag, hits real sites, used on a
//      schedule and for manual "does this still work" checks — not on
//      every PR, to avoid hammering third-party sites on every commit)
//      — for each site, fetch a working mirror + search_path with a
//      bland test query, and confirm `selectors.row` (HTML) or
//      `json_fields.results_array` (JSON) actually matches something
//      in the response. This is "structure drift detection": if a site
//      redesigns its HTML, `row` stops matching and this fails loudly
//      in CI instead of silently returning zero results in production.
//
//      A mirror that returns 403/503/429 (a bot-challenge wall, e.g.
//      Cloudflare) is treated as BLOCKED, not DEAD — see
//      live_check.rs's module doc for the full reasoning. `--tolerate-
//      blocked` (used by CI) means a source whose ONLY problem is
//      "every mirror blocked, none genuinely dead" doesn't fail the
//      run, since a CI runner's IP being on a WAF blocklist isn't
//      evidence the source is broken for real users.
//
// Exit code is non-zero if anything fails, so this drops straight into
// a GitHub Actions job with no extra glue.

mod schema_check;
mod live_check;

use clap::{Parser, Subcommand};
use colored::Colorize;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "validator", about = "Validates streamx-torrent-indexer source definitions")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate schema (and optionally live-fetch) for all sources, or
    /// just the ones listed with --only.
    Check {
        /// Repo root (contains sources/, schema/). Defaults to CWD.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Also do a live HTTP fetch + selector smoke-test.
        #[arg(long)]
        live: bool,
        /// Only check these site ids (comma-separated). Default: all.
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        /// When a source's ONLY problem is that every reachable mirror
        /// returned a bot-challenge status (403/503/429) — not a dead
        /// mirror, not structure drift — don't fail the run for it.
        /// Intended for CI (see .github/workflows/validate-sources.yml),
        /// since GitHub Actions runner IPs commonly sit on Cloudflare's
        /// own datacenter/bot blocklists regardless of request headers,
        /// so "blocked from this runner" isn't reliable evidence a
        /// source is actually broken for real users. Off by default for
        /// local/manual runs, where a human may want every non-clean
        /// result surfaced as a failure worth a second look.
        #[arg(long)]
        tolerate_blocked: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Check { root, live, only, tolerate_blocked } => {
            let schema_path = root.join("schema/source.schema.json");
            let sources_dir = root.join("sources");

            println!("{}", "── Schema validation ──".bold());
            let schema_results = schema_check::run(&schema_path, &sources_dir, &only)?;
            let schema_ok = schema_check::report(&schema_results);

            let mut live_ok = true;
            if live {
                println!("\n{}", "── Live selector smoke-test ──".bold());
                let live_results = live_check::run(&schema_results, &only).await?;
                live_ok = live_check::report(&live_results, tolerate_blocked);
            } else {
                println!(
                    "\n{}",
                    "(skipped live smoke-test — pass --live to run it)".dimmed()
                );
            }

            if schema_ok && live_ok {
                println!("\n{}", "All checks passed.".green().bold());
                Ok(())
            } else {
                println!("\n{}", "Validation failed — see above.".red().bold());
                std::process::exit(1);
            }
        }
    }
}
