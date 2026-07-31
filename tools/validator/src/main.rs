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
//      — for each site, fetch mirrors[0] + search_path with a bland
//      test query, and confirm `selectors.row` (HTML) or
//      `json_fields.results_array` (JSON) actually matches something
//      in the response. This is "structure drift detection": if a site
//      redesigns its HTML, `row` stops matching and this fails loudly
//      in CI instead of silently returning zero results in production.
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
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Check { root, live, only } => {
            let schema_path = root.join("schema/source.schema.json");
            let sources_dir = root.join("sources");

            println!("{}", "── Schema validation ──".bold());
            let schema_results = schema_check::run(&schema_path, &sources_dir, &only)?;
            let schema_ok = schema_check::report(&schema_results);

            let mut live_ok = true;
            if live {
                println!("\n{}", "── Live selector smoke-test ──".bold());
                let live_results = live_check::run(&schema_results, &only).await?;
                live_ok = live_check::report(&live_results);
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
