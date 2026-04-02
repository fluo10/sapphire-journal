use dioxus::prelude::*;
use uuid::Uuid;

use crate::registry::{init_journal, JournalRegistry, RegistryEntry};

#[component]
pub fn NewJournalDialog(
    on_cancel: EventHandler<()>,
    on_error: EventHandler<String>,
) -> Element {
    let mut journal_name = use_signal(String::new);
    let mut creating = use_signal(|| false);

    let registry = use_context::<Signal<JournalRegistry>>();

    let name_trimmed = journal_name.read().trim().to_string();
    let can_create = !name_trimmed.is_empty() && !creating();

    rsx! {
        div { class: "dialog-overlay",
            div { class: "dialog-box",
                h2 { class: "dialog-title", "New Journal" }

                div { class: "form-group",
                    label { class: "form-label", "Journal Name" }
                    input {
                        class: "form-input",
                        r#type: "text",
                        placeholder: "e.g. My Journal",
                        value: "{journal_name}",
                        oninput: move |e| journal_name.set(e.value()),
                        autofocus: true,
                    }
                }

                div { class: "dialog-actions",
                    button {
                        class: "btn btn-secondary",
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: !can_create,
                        onclick: move |_| {
                            let name = journal_name.read().trim().to_string();
                            if name.is_empty() {
                                return;
                            }
                            creating.set(true);
                            let mut reg = registry;
                            let on_error = on_error.clone();
                            // Generate the storage UUID here so we can use it both inside and
                            // outside the blocking closure.
                            let storage_uuid = Uuid::new_v4();
                            let storage_path =
                                JournalRegistry::journals_dir().join(storage_uuid.to_string());
                            let storage_path_clone = storage_path.clone();
                            spawn(async move {
                                let result =
                                    tokio::task::spawn_blocking(move || init_journal(&storage_path))
                                        .await;
                                match result {
                                    Ok(Ok(journal_id)) => {
                                        let entry = RegistryEntry {
                                            id: journal_id,
                                            name,
                                            storage_path: storage_path_clone,
                                        };
                                        reg.write().add(entry);
                                        if let Err(e) = reg.read().save() {
                                            on_error.call(e.to_string());
                                        }
                                    }
                                    Ok(Err(e)) => on_error.call(e.to_string()),
                                    Err(e) => on_error.call(e.to_string()),
                                }
                                creating.set(false);
                            });
                        },
                        if creating() { "Creating…" } else { "Create" }
                    }
                }
            }
        }
    }
}
