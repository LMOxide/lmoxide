//! `lmoxide-content`: import the old data into the content tree, check it, format it, write
//! its JSON Schemas.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use lmoxide_content::{Options, Severity, canon, import, load};

#[derive(Parser)]
#[command(about = "The game's content tree: import, check, format, schemas")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Converts data/*.json (and the world editor's published set) into a content tree. Writes
    /// the whole tree anew: edits made in it are lost.
    Import {
        #[arg(long, default_value = "data")]
        data: PathBuf,
        /// The world editor's published set.
        #[arg(long)]
        survey: Option<PathBuf>,
        #[arg(long, default_value = "content")]
        out: PathBuf,
    },
    /// Loads the tree and lists every problem (exit code 1 on errors).
    Check {
        #[arg(long, default_value = "content")]
        dir: PathBuf,
        /// Active features (default: live).
        #[arg(long = "feature")]
        features: Vec<String>,
        /// Show warnings too.
        #[arg(long)]
        warnings: bool,
    },
    /// Rewrites every file in its canonical form (field order, number and array layout).
    Fmt {
        #[arg(long, default_value = "content")]
        dir: PathBuf,
        /// Only report files that would change (exit code 1 if any).
        #[arg(long)]
        check: bool,
    },
    /// Writes one JSON Schema per kind and `index.json` (for the world editor's forms and editors).
    Schema {
        #[arg(long, default_value = "content/schemas")]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    match Cli::parse().command {
        Command::Import { data, survey, out } => {
            let tree = import::read(&import::Sources { data, survey })?;
            for n in &tree.notes {
                println!("note: {n}");
            }
            let n = import::write(tree, &out)?;
            println!("wrote {n} records to {}", out.display());
        }
        Command::Check {
            dir,
            features,
            warnings,
        } => {
            let mut options = Options::default();
            if !features.is_empty() {
                options.features = features;
            }
            let (content, problems) = load(&dir, &options);
            let errors = problems
                .iter()
                .filter(|p| p.severity == Severity::Error)
                .count();
            for p in problems
                .iter()
                .filter(|p| warnings || p.severity == Severity::Error)
            {
                println!("{p}");
            }
            println!(
                "{} worlds, {} spawns, {} npcs, {} objects, {} skills, {} buffs, {} loot tables, {} items, {} minifigures, {} achievements, {} quests; {errors} errors, {} warnings",
                content.worlds.len(),
                content.spawns.len(),
                content.npcs.len(),
                content.objects.len(),
                content.skills.len(),
                content.buffs.len(),
                content.loot.len(),
                content.items.len(),
                content.minifigures.len(),
                content.achievements.len(),
                content.quests.len(),
                problems.len() - errors,
            );
            if errors > 0 {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Fmt { dir, check } => {
            let changed = lmoxide_content::load::format_tree(&dir, check)?;
            for f in &changed {
                println!("{}", f.display());
            }
            if check && !changed.is_empty() {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Schema { out } => {
            std::fs::create_dir_all(&out)?;
            for (kind, schema) in lmoxide_content::schemas() {
                std::fs::write(
                    out.join(format!("{kind}.schema.json")),
                    canon::to_string(&schema)?,
                )?;
            }
            std::fs::write(
                out.join("index.json"),
                canon::to_string(&lmoxide_content::schema::index())?,
            )?;
            println!("wrote schemas to {}", out.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}
