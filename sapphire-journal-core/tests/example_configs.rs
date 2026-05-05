//! Verify that the example config files under `docs/config/` stay in sync
//! with the `UserConfig` / `JournalConfig` schemas.

use sapphire_journal_core::journal::JournalConfig;
use sapphire_journal_core::user_config::UserConfig;

#[test]
fn user_config_example_parses() {
    let raw = include_str!("../../docs/config/user-config.toml");
    toml::from_str::<UserConfig>(raw).expect("docs/config/user-config.toml must parse as UserConfig");
}

#[test]
fn journal_config_example_parses() {
    let raw = include_str!("../../docs/config/journal-config.toml");
    toml::from_str::<JournalConfig>(raw)
        .expect("docs/config/journal-config.toml must parse as JournalConfig");
}
