use dioxus::prelude::*;

use crate::registry::RegistryEntry;

#[component]
pub fn JournalCard(
    entry: RegistryEntry,
    on_open: EventHandler<()>,
    on_delete: EventHandler<()>,
) -> Element {
    let reachable = entry.storage_path.join(".sapphire-journal").is_dir();

    rsx! {
        div { class: "journal-card",
            div { class: "journal-card-info",
                div { class: "journal-card-name",
                    span { "{entry.name}" }
                    if !reachable {
                        span { class: "badge badge-warning", "unreachable" }
                    }
                }
            }
            div { class: "journal-card-actions",
                button {
                    class: "btn btn-primary",
                    disabled: !reachable,
                    onclick: move |_| on_open.call(()),
                    "Open"
                }
                button {
                    class: "btn btn-danger",
                    onclick: move |_| on_delete.call(()),
                    "Delete"
                }
            }
        }
    }
}
