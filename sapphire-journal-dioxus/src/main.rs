// SPDX-License-Identifier: GPL-3.0-or-later

use dioxus::prelude::*;
use uuid::Uuid;

use components::journal_home::JournalHomeScreen;
use components::journal_list::JournalListScreen;
use registry::JournalRegistry;

mod components;
mod error;
mod registry;

const MAIN_CSS: Asset = asset!("/assets/styling/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

fn main() {
    dioxus::launch(App);
}

#[derive(Clone, PartialEq)]
pub enum AppState {
    List,
    Home { journal_id: Uuid },
}

#[component]
fn App() -> Element {
    use_context_provider(|| Signal::new(AppState::List));
    use_context_provider(|| Signal::new(JournalRegistry::load().unwrap_or_default()));

    let app_state = use_context::<Signal<AppState>>();

    rsx! {
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }

        match app_state() {
            AppState::List => rsx! { JournalListScreen {} },
            AppState::Home { journal_id } => rsx! { JournalHomeScreen { journal_id } },
        }
    }
}
