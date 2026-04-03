use dioxus::prelude::*;
use uuid::Uuid;

use crate::AppState;

#[component]
pub fn JournalHomeScreen(journal_id: Uuid) -> Element {
    let mut app_state = use_context::<Signal<AppState>>();

    rsx! {
        div { class: "journal-home-screen",
            div { class: "home-header",
                button {
                    class: "btn btn-secondary",
                    onclick: move |_| app_state.set(AppState::List),
                    "← Back"
                }
            }
            div { class: "home-placeholder",
                h1 { "Journal" }
                p { "Entry list coming soon." }
            }
        }
    }
}
