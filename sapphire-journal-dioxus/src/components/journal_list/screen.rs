use dioxus::prelude::*;

use super::{
    clone_dialog::CloneDialog, confirm_delete::DeleteDialog, journal_card::JournalCard,
    new_journal_dialog::NewJournalDialog,
};
use crate::registry::{JournalRegistry, RegistryEntry};
use crate::AppState;

#[component]
pub fn JournalListScreen() -> Element {
    let registry = use_context::<Signal<JournalRegistry>>();
    let mut app_state = use_context::<Signal<AppState>>();

    let mut show_new_dialog = use_signal(|| false);
    let mut show_clone_dialog = use_signal(|| false);
    let mut delete_target: Signal<Option<RegistryEntry>> = use_signal(|| None);
    let mut error_msg: Signal<Option<String>> = use_signal(|| None);

    let journals = registry.read().journals.clone();

    rsx! {
        div { class: "journal-list-screen",
            // Header
            div { class: "list-header",
                h1 { class: "list-title", "Sapphire Journal" }
                div { class: "list-actions",
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| show_new_dialog.set(true),
                        "New Journal"
                    }
                    button {
                        class: "btn btn-secondary",
                        onclick: move |_| show_clone_dialog.set(true),
                        "Clone"
                    }
                }
            }

            // Error banner
            if let Some(msg) = error_msg.read().clone() {
                div { class: "error-banner",
                    span { "{msg}" }
                    button {
                        class: "error-dismiss",
                        onclick: move |_| error_msg.set(None),
                        "×"
                    }
                }
            }

            // Journal list
            if journals.is_empty() {
                div { class: "empty-state",
                    p { "No journals yet." }
                    p { "Create a new journal or clone an existing one to get started." }
                }
            } else {
                div { class: "journal-cards",
                    for entry in journals {
                        JournalCard {
                            key: "{entry.id}",
                            entry: entry.clone(),
                            on_open: move |_| {
                                app_state.set(AppState::Home { journal_id: entry.id });
                            },
                            on_delete: move |_| {
                                delete_target.set(Some(entry.clone()));
                            },
                        }
                    }
                }
            }

            // New journal dialog
            if show_new_dialog() {
                NewJournalDialog {
                    on_cancel: move |_| show_new_dialog.set(false),
                    on_error: move |msg: String| {
                        error_msg.set(Some(msg));
                        show_new_dialog.set(false);
                    },
                }
            }

            // Clone dialog
            if show_clone_dialog() {
                CloneDialog {
                    on_cancel: move |_| show_clone_dialog.set(false),
                    on_error: move |msg: String| {
                        error_msg.set(Some(msg));
                        show_clone_dialog.set(false);
                    },
                }
            }

            // Delete confirmation dialog
            if let Some(entry) = delete_target.read().clone() {
                DeleteDialog {
                    entry: entry.clone(),
                    on_cancel: move |_| delete_target.set(None),
                    on_error: move |msg: String| {
                        error_msg.set(Some(msg));
                        delete_target.set(None);
                    },
                }
            }
        }
    }
}
