use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use uuid::Uuid;

use crate::app::{App, AppEvent, DialogState};
use crate::registry::{JournalRegistry, RegistryEntry};
use sapphire_journal_core::journal::Journal;

pub fn show(app: &mut App, ctx: &egui::Context) {
    let mut close = false;
    let mut submit = false;

    let DialogState::Clone(state) = app.dialog.as_mut().unwrap() else {
        return;
    };

    let progress_arc = Arc::clone(&state.progress);
    let current_progress = *progress_arc.lock().unwrap();
    let in_progress = current_progress.is_some();

    let mut open = true;
    egui::Window::new("Clone Journal")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_min_width(420.0);

            ui.label("Journal Name");
            let name_resp = ui.add(
                egui::TextEdit::singleline(&mut state.name)
                    .hint_text("e.g. Work Journal")
                    .desired_width(f32::INFINITY),
            );
            if !in_progress && !name_resp.has_focus() && state.name.is_empty() {
                name_resp.request_focus();
            }

            ui.add_space(6.0);

            ui.label("Remote URL (HTTPS)");
            ui.add(
                egui::TextEdit::singleline(&mut state.url)
                    .hint_text("https://github.com/you/journal.git")
                    .desired_width(f32::INFINITY),
            );

            if let Some(progress) = current_progress {
                ui.add_space(8.0);
                ui.add(egui::ProgressBar::new(progress).show_percentage());
                ui.small(format!("Cloning… {:.0}%", progress * 100.0));
            }

            ui.add_space(8.0);

            let name_trimmed = state.name.trim().to_string();
            let url_trimmed = state.url.trim().to_string();
            let can_clone = !name_trimmed.is_empty() && !url_trimmed.is_empty() && !in_progress;

            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if in_progress { "Cloning…" } else { "Clone" };
                    if ui
                        .add_enabled(can_clone, egui::Button::new(label))
                        .clicked()
                    {
                        submit = true;
                    }
                    if ui
                        .add_enabled(!in_progress, egui::Button::new("Cancel"))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
        });

    if !open && !in_progress {
        close = true;
    }

    if submit {
        let name = state.name.trim().to_string();
        let url = state.url.trim().to_string();
        let storage_path = JournalRegistry::journals_dir().join(Uuid::new_v4().to_string());
        let progress = Arc::clone(&state.progress);
        *progress.lock().unwrap() = Some(0.0);

        let tx = app.event_tx.clone();
        let storage_clone = storage_path.clone();
        let progress_clone = Arc::clone(&progress);

        app.runtime.spawn(async move {
            let storage_for_blocking = storage_clone.clone();
            let progress_for_blocking = Arc::clone(&progress_clone);
            let result = tokio::task::spawn_blocking(move || {
                let mut callbacks = git2::RemoteCallbacks::new();
                let progress_cb = Arc::clone(&progress_for_blocking);
                callbacks.transfer_progress(move |stats| {
                    let total = stats.total_objects().max(1) as f32;
                    let done = stats.received_objects() as f32;
                    *progress_cb.lock().unwrap() = Some(done / total);
                    true
                });
                let mut fetch_opts = git2::FetchOptions::new();
                fetch_opts.remote_callbacks(callbacks);
                git2::build::RepoBuilder::new()
                    .fetch_options(fetch_opts)
                    .clone(&url, &storage_for_blocking)
            })
            .await;

            // Reset the progress so the dialog leaves "in progress" state.
            *progress_clone.lock().unwrap() = None;

            match result {
                Ok(Ok(_repo)) => match validate_journal(&storage_clone) {
                    Ok(journal_id) => {
                        let _ = tx.send(AppEvent::JournalAdded(RegistryEntry {
                            id: journal_id,
                            name,
                            storage_path: storage_clone,
                        }));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::CleanupAndError {
                            storage_path: storage_clone,
                            error: e,
                        });
                    }
                },
                Ok(Err(e)) => {
                    let _ = tx.send(AppEvent::CleanupAndError {
                        storage_path: storage_clone,
                        error: e.to_string(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::CleanupAndError {
                        storage_path: storage_clone,
                        error: e.to_string(),
                    });
                }
            }
        });
    }

    if close {
        app.dialog = None;
    }
}

fn validate_journal(storage_path: &PathBuf) -> Result<Uuid, String> {
    let journal = Journal::from_root(storage_path.clone()).map_err(|_| {
        "The cloned repository is not a sapphire journal (missing .sapphire-journal/)".to_string()
    })?;
    journal.journal_id().map_err(|e| e.to_string())
}
