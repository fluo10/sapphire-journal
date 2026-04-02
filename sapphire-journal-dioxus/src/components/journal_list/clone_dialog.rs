use std::path::PathBuf;

use dioxus::prelude::*;
use uuid::Uuid;

use crate::registry::{JournalRegistry, RegistryEntry};
use sapphire_journal_core::journal::Journal;

#[derive(Clone, PartialEq)]
enum CloneStatus {
    Idle,
    Cloning { progress: f32 },
    Done,
}

#[component]
pub fn CloneDialog(
    on_cancel: EventHandler<()>,
    on_error: EventHandler<String>,
) -> Element {
    let mut journal_name = use_signal(String::new);
    let mut remote_url = use_signal(String::new);
    let mut clone_status: Signal<CloneStatus> = use_signal(|| CloneStatus::Idle);

    let registry = use_context::<Signal<JournalRegistry>>();

    let name_ok = !journal_name.read().trim().is_empty();
    let url_ok = !remote_url.read().trim().is_empty();
    let is_cloning = matches!(clone_status(), CloneStatus::Cloning { .. });
    let can_clone = name_ok && url_ok && !is_cloning;

    rsx! {
        div { class: "dialog-overlay",
            div { class: "dialog-box",
                h2 { class: "dialog-title", "Clone Journal" }

                div { class: "form-group",
                    label { class: "form-label", "Journal Name" }
                    input {
                        class: "form-input",
                        r#type: "text",
                        placeholder: "e.g. Work Journal",
                        value: "{journal_name}",
                        oninput: move |e| journal_name.set(e.value()),
                        autofocus: true,
                    }
                }

                div { class: "form-group",
                    label { class: "form-label", "Remote URL (HTTPS)" }
                    input {
                        class: "form-input",
                        r#type: "text",
                        placeholder: "https://github.com/you/journal.git",
                        value: "{remote_url}",
                        oninput: move |e| remote_url.set(e.value()),
                    }
                }

                // Progress bar
                if let CloneStatus::Cloning { progress } = clone_status() {
                    div { class: "progress-bar-track",
                        div {
                            class: "progress-bar-fill",
                            style: "width: {progress * 100.0:.0}%",
                        }
                    }
                    p { class: "progress-label", "Cloning… {(progress * 100.0) as u32}%" }
                }

                div { class: "dialog-actions",
                    button {
                        class: "btn btn-secondary",
                        disabled: is_cloning,
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: !can_clone,
                        onclick: move |_| {
                            let name = journal_name.read().trim().to_string();
                            let url = remote_url.read().trim().to_string();
                            if name.is_empty() || url.is_empty() {
                                return;
                            }
                            clone_status.set(CloneStatus::Cloning { progress: 0.0 });
                            let mut reg = registry;
                            let on_error = on_error.clone();
                            let storage_path =
                                JournalRegistry::journals_dir().join(Uuid::new_v4().to_string());
                            let storage_path_clone = storage_path.clone();

                            spawn(async move {
                                let (tx, mut rx) =
                                    tokio::sync::mpsc::channel::<f32>(32);

                                let result = tokio::task::spawn_blocking(move || {
                                    let mut callbacks = git2::RemoteCallbacks::new();
                                    let tx2 = tx.clone();
                                    callbacks.transfer_progress(move |stats| {
                                        let total = stats.total_objects().max(1) as f32;
                                        let done = stats.received_objects() as f32;
                                        let _ = tx2.blocking_send(done / total);
                                        true
                                    });
                                    let mut fetch_opts = git2::FetchOptions::new();
                                    fetch_opts.remote_callbacks(callbacks);
                                    git2::build::RepoBuilder::new()
                                        .fetch_options(fetch_opts)
                                        .clone(&url, &storage_path)
                                });

                                // Drain progress channel while clone runs.
                                // The receiver will return None once the sender is dropped
                                // (i.e. after spawn_blocking completes).
                                while let Some(pct) = rx.recv().await {
                                    clone_status.set(CloneStatus::Cloning { progress: pct });
                                }

                                match result.await {
                                    Ok(Ok(_repo)) => {
                                        // Verify it is a valid sapphire journal
                                        match validate_and_register(
                                            &storage_path_clone,
                                            name,
                                            &mut reg,
                                        ) {
                                            Ok(()) => {
                                                clone_status.set(CloneStatus::Done);
                                            }
                                            Err(e) => {
                                                // Clean up the cloned directory
                                                let _ =
                                                    std::fs::remove_dir_all(&storage_path_clone);
                                                on_error.call(e);
                                                clone_status.set(CloneStatus::Idle);
                                            }
                                        }
                                    }
                                    Ok(Err(e)) => {
                                        let _ = std::fs::remove_dir_all(&storage_path_clone);
                                        on_error.call(e.to_string());
                                        clone_status.set(CloneStatus::Idle);
                                    }
                                    Err(e) => {
                                        let _ = std::fs::remove_dir_all(&storage_path_clone);
                                        on_error.call(e.to_string());
                                        clone_status.set(CloneStatus::Idle);
                                    }
                                }
                            });
                        },
                        if is_cloning { "Cloning…" } else { "Clone" }
                    }
                }
            }
        }
    }
}

fn validate_and_register(
    storage_path: &PathBuf,
    name: String,
    registry: &mut Signal<JournalRegistry>,
) -> Result<(), String> {
    let journal = Journal::from_root(storage_path.clone())
        .map_err(|_| "The cloned repository is not a sapphire journal (missing .sapphire-journal/)".to_string())?;
    let id = journal
        .journal_id()
        .map_err(|e| e.to_string())?;
    let entry = RegistryEntry {
        id,
        name,
        storage_path: storage_path.clone(),
    };
    registry.write().add(entry);
    registry
        .read()
        .save()
        .map_err(|e| e.to_string())?;
    Ok(())
}
