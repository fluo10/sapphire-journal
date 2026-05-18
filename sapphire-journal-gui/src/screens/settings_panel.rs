//! Settings panel — per-journal (`.sapphire-journal/config.toml` + git remote)
//! and global (`$XDG_CONFIG_HOME/sapphire-journal/config.toml`) preferences.
//!
//! Opened from the journal-switcher menu in `journal_home`.

use std::path::{Path, PathBuf};

use eframe::egui;

use sapphire_journal_core::{
    journal::{DuplicateTitlePolicy, Journal, JournalConfig},
    user_config::{SyncBackendKind, UserConfig},
};

use crate::app::App;

const DUP_TITLE_OPTIONS: &[&str] = &["allow", "warn", "error"];
const SYNC_BACKEND_OPTIONS: &[&str] = &["auto", "none", "git"];

pub struct SettingsPanelState {
    journal_root: PathBuf,

    // ── Per-journal ────────────────────────────────────────────────────────
    git_remote: String,
    duplicate_title: String,
    entries_dir: String,

    // ── Global ─────────────────────────────────────────────────────────────
    sync_interval_minutes: String,
    sync_backend: String,

    error_msg: Option<String>,
    info_msg: Option<String>,
}

impl SettingsPanelState {
    /// Build the state by reading current on-disk values.
    pub fn open(journal_root: PathBuf) -> Self {
        let journal_cfg = Journal::from_root(journal_root.clone())
            .and_then(|j| j.config())
            .unwrap_or_default();
        let duplicate_title = match journal_cfg.journal.duplicate_title {
            DuplicateTitlePolicy::Allow => "allow",
            DuplicateTitlePolicy::Warn => "warn",
            DuplicateTitlePolicy::Error => "error",
        }
        .to_string();
        let entries_dir = journal_cfg.journal.entries_dir.unwrap_or_default();
        let git_remote = git2::Repository::open(&journal_root)
            .ok()
            .and_then(|repo| {
                repo.find_remote("origin")
                    .ok()
                    .and_then(|r| r.url().map(str::to_owned))
            })
            .unwrap_or_default();

        let user_cfg = UserConfig::load().unwrap_or_default();
        let sync_interval_minutes = user_cfg
            .sync_interval_minutes
            .map(|n| n.to_string())
            .unwrap_or_else(|| "10".to_string());
        let sync_backend = match user_cfg.sync.backend {
            SyncBackendKind::Auto => "auto",
            SyncBackendKind::None => "none",
            SyncBackendKind::Git => "git",
        }
        .to_string();

        Self {
            journal_root,
            git_remote,
            duplicate_title,
            entries_dir,
            sync_interval_minutes,
            sync_backend,
            error_msg: None,
            info_msg: None,
        }
    }

    fn save(&mut self) -> bool {
        self.error_msg = None;
        self.info_msg = None;

        let interval = match self.sync_interval_minutes.trim().parse::<u32>() {
            Ok(n) => n,
            Err(_) => {
                self.error_msg =
                    Some("Sync interval must be a non-negative integer (0 to disable).".to_string());
                return false;
            }
        };

        let mut user_cfg = UserConfig::load().unwrap_or_default();
        user_cfg.sync_interval_minutes = Some(interval);
        user_cfg.sync.backend = match self.sync_backend.as_str() {
            "none" => SyncBackendKind::None,
            "git" => SyncBackendKind::Git,
            _ => SyncBackendKind::Auto,
        };
        if let Err(e) = write_user_config(&user_cfg) {
            self.error_msg = Some(format!("Failed to save user config: {e}"));
            return false;
        }

        let journal = match Journal::from_root(self.journal_root.clone()) {
            Ok(j) => j,
            Err(e) => {
                self.error_msg = Some(format!("Failed to open journal: {e}"));
                return false;
            }
        };
        let mut jc = journal.config().unwrap_or_default();
        jc.journal.duplicate_title = match self.duplicate_title.as_str() {
            "allow" => DuplicateTitlePolicy::Allow,
            "error" => DuplicateTitlePolicy::Error,
            _ => DuplicateTitlePolicy::Warn,
        };
        let new_entries_dir = if self.entries_dir.trim().is_empty() {
            None
        } else {
            Some(self.entries_dir.trim().to_string())
        };
        let entries_dir_changed = jc.journal.entries_dir != new_entries_dir;
        jc.journal.entries_dir = new_entries_dir;
        if let Err(e) = write_journal_config(&journal, &jc) {
            self.error_msg = Some(format!("Failed to save journal config: {e}"));
            return false;
        }

        if !self.git_remote.trim().is_empty() {
            if let Err(e) = set_git_remote(&self.journal_root, self.git_remote.trim()) {
                self.error_msg = Some(format!("Failed to set git remote: {e}"));
                return false;
            }
        }

        self.info_msg = Some(if entries_dir_changed {
            "Saved. Restart or refresh to see entries from the new directory."
        } else {
            "Saved."
        }
        .to_string());
        true
    }
}

fn write_user_config(cfg: &UserConfig) -> Result<(), String> {
    let path = UserConfig::path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = toml::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

fn write_journal_config(journal: &Journal, cfg: &JournalConfig) -> Result<(), String> {
    let path = journal.journal_dir().join("config.toml");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = toml::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

fn set_git_remote(journal_root: &Path, url: &str) -> Result<(), String> {
    let repo = git2::Repository::open(journal_root).map_err(|e| e.to_string())?;
    match repo.find_remote("origin") {
        Ok(_) => {
            repo.remote_set_url("origin", url).map_err(|e| e.to_string())?;
        }
        Err(_) => {
            repo.remote("origin", url).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Render the panel.  No-op when `app.settings_panel` is `None`.
pub fn show(app: &mut App, ctx: &egui::Context) {
    let mut close = false;
    let mut do_save = false;
    let mut open_flag = true;

    let Some(state) = app.settings_panel.as_mut() else {
        return;
    };

    egui::Window::new("Settings")
        .collapsible(false)
        .resizable(true)
        .default_width(480.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open_flag)
        .show(ctx, |ui| {
            ui.heading("This journal");
            ui.add_space(4.0);

            ui.label("Git remote (origin)");
            ui.add(
                egui::TextEdit::singleline(&mut state.git_remote)
                    .hint_text("git@github.com:user/repo.git")
                    .desired_width(f32::INFINITY),
            );
            ui.weak("Empty input leaves the remote unchanged.  Use `git` to remove it.");
            ui.add_space(6.0);

            ui.label("Duplicate title policy");
            egui::ComboBox::from_id_salt("settings_dup_title")
                .selected_text(&state.duplicate_title)
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for v in DUP_TITLE_OPTIONS {
                        ui.selectable_value(&mut state.duplicate_title, (*v).to_string(), *v);
                    }
                });
            ui.add_space(6.0);

            ui.label("Entries directory (relative to journal root; blank = root)");
            ui.add(
                egui::TextEdit::singleline(&mut state.entries_dir)
                    .hint_text("entries")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);

            ui.heading("Global");
            ui.weak("Applies to all journals — takes effect on next launch.");
            ui.add_space(4.0);

            ui.label("Sync interval (minutes; 0 to disable)");
            ui.add(
                egui::TextEdit::singleline(&mut state.sync_interval_minutes)
                    .desired_width(80.0),
            );
            ui.add_space(6.0);

            ui.label("Sync backend");
            egui::ComboBox::from_id_salt("settings_sync_backend")
                .selected_text(&state.sync_backend)
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for v in SYNC_BACKEND_OPTIONS {
                        ui.selectable_value(&mut state.sync_backend, (*v).to_string(), *v);
                    }
                });

            if let Some(msg) = state.error_msg.clone() {
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::LIGHT_RED, msg);
            }
            if let Some(msg) = state.info_msg.clone() {
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Save").clicked() {
                        do_save = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        });

    if !open_flag {
        close = true;
    }
    if do_save {
        state.save();
    }
    if close {
        app.settings_panel = None;
    }
}
