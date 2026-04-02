use dioxus::prelude::*;

use crate::registry::{JournalRegistry, RegistryEntry};

#[component]
pub fn DeleteDialog(
    entry: RegistryEntry,
    on_cancel: EventHandler<()>,
    on_error: EventHandler<String>,
) -> Element {
    let mut confirm_name = use_signal(String::new);
    let mut deleting = use_signal(|| false);

    let registry = use_context::<Signal<JournalRegistry>>();

    let expected = entry.name.clone();
    let name_matches = confirm_name.read().trim() == expected.as_str();
    let can_delete = name_matches && !deleting();

    rsx! {
        div { class: "dialog-overlay",
            div { class: "dialog-box dialog-danger",
                h2 { class: "dialog-title", "Delete Journal" }

                p { class: "dialog-warning",
                    "This will permanently delete the journal and all its entries. This action "
                    strong { "cannot be undone." }
                }

                div { class: "form-group",
                    label { class: "form-label",
                        "Type "
                        strong { "{expected}" }
                        " to confirm:"
                    }
                    input {
                        class: "form-input",
                        r#type: "text",
                        placeholder: "{expected}",
                        value: "{confirm_name}",
                        oninput: move |e| confirm_name.set(e.value()),
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
                        class: "btn btn-danger",
                        disabled: !can_delete,
                        onclick: move |_| {
                            let entry = entry.clone();
                            deleting.set(true);
                            let mut reg = registry;
                            let on_error = on_error.clone();
                            spawn(async move {
                                let storage = entry.storage_path.clone();
                                let result =
                                    tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&storage))
                                        .await;
                                match result {
                                    Ok(Ok(())) => {
                                        reg.write().remove_by_id(entry.id);
                                        if let Err(e) = reg.read().save() {
                                            on_error.call(e.to_string());
                                        }
                                    }
                                    Ok(Err(e)) => on_error.call(e.to_string()),
                                    Err(e) => on_error.call(e.to_string()),
                                }
                                deleting.set(false);
                            });
                        },
                        if deleting() { "Deleting…" } else { "Delete Journal" }
                    }
                }
            }
        }
    }
}
