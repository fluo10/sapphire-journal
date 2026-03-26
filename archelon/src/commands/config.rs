use anyhow::Result;
use archelon_core::{journal::Journal, user_config::UserConfig};
use clap::Subcommand;
use std::path::Path;

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Show the current effective configuration
    ///
    /// Displays the user-level config (~/.config/archelon/config.toml) and,
    /// when run inside an archelon journal, the journal-level config
    /// (.archelon/config.toml) as well.
    Show,
}

pub fn run(journal_dir: Option<&Path>, cmd: ConfigCommand) -> Result<()> {
    match cmd {
        ConfigCommand::Show => show(journal_dir),
    }
}

fn show(journal_dir: Option<&Path>) -> Result<()> {
    // ── User config ──────────────────────────────────────────────────────────
    let user_path = UserConfig::path();
    println!("## user config");
    println!("path: {}", user_path.display());
    if user_path.exists() {
        let raw = std::fs::read_to_string(&user_path)?;
        println!("{}", raw.trim_end());
    } else {
        println!("(file not found — using defaults)");
        println!();
        println!("# To customise, create the file above with settings such as:");
        println!("#");
        println!("# [cache.embedding]");
        println!("# enabled   = true");
        println!("# vector_db = \"sqlite_vec\"   # \"none\" | \"sqlite_vec\" | \"lancedb\"");
        println!("# provider  = \"openai\"");
        println!("# model     = \"text-embedding-3-small\"");
        println!("# dimension = 1536");
        println!("# api_key_env = \"OPENAI_API_KEY\"");
        println!("#");
        println!("# For local providers (Ollama, fastembed, etc.):");
        println!("# provider = \"ollama\"");
        println!("# model    = \"nomic-embed-text\"");
        println!("# dimension = 768");
        println!("# base_url = \"http://localhost:11434\"");
    }

    println!();

    // ── Journal config ───────────────────────────────────────────────────────
    println!("## journal config");
    let journal = match journal_dir {
        Some(dir) => Journal::from_root(dir.to_path_buf()).ok(),
        None => Journal::find().ok(),
    };
    match journal {
        Some(ref j) => {
            let journal_config_path = j.archelon_dir().join("config.toml");
            println!("path: {}", journal_config_path.display());
            if journal_config_path.exists() {
                let raw = std::fs::read_to_string(&journal_config_path)?;
                println!("{}", raw.trim_end());
            } else {
                println!("(file not found — using defaults)");
            }
        }
        None => {
            println!("(not inside an archelon journal)");
        }
    }

    Ok(())
}
