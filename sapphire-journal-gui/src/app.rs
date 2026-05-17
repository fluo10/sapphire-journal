use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui;
use grain_id::GrainId;
use sapphire_journal_core::entry::EntryHeader;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::registry::{JournalRegistry, RegistryEntry};
use crate::screens;
use crate::settings::Settings;

#[derive(Clone, PartialEq)]
pub enum AppState {
    List,
    Home { journal_id: Uuid },
}

pub enum DialogState {
    NewJournal(NewJournalState),
    Clone(CloneState),
    ConfirmDelete(ConfirmDeleteState),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    NewJournal,
    Clone,
    ConfirmDelete,
}

impl DialogState {
    pub fn kind(&self) -> DialogKind {
        match self {
            DialogState::NewJournal(_) => DialogKind::NewJournal,
            DialogState::Clone(_) => DialogKind::Clone,
            DialogState::ConfirmDelete(_) => DialogKind::ConfirmDelete,
        }
    }
}

pub struct NewJournalState {
    pub name: String,
    pub in_progress: bool,
}

pub struct CloneState {
    pub name: String,
    pub url: String,
    pub progress: Arc<Mutex<Option<f32>>>,
}

pub struct ConfirmDeleteState {
    pub entry: RegistryEntry,
    pub typed: String,
    pub in_progress: bool,
}

pub enum AppEvent {
    JournalAdded(RegistryEntry),
    JournalRemoved(Uuid),
    Error(String),
    CleanupAndError {
        storage_path: PathBuf,
        error: String,
    },
}

pub struct App {
    pub screen: AppState,
    pub registry: JournalRegistry,
    pub settings: Settings,
    pub dialog: Option<DialogState>,
    pub error_msg: Option<String>,
    pub home: Option<HomeState>,
    /// When the user went to the journal-list screen from an open journal,
    /// this records that journal so the list can offer a "back" button.
    pub previous_journal_id: Option<Uuid>,
    pub runtime: Arc<tokio::runtime::Runtime>,
    pub event_tx: mpsc::UnboundedSender<AppEvent>,
    pub event_rx: mpsc::UnboundedReceiver<AppEvent>,
}

/// State for the journal-home screen (sidebar + editor).
///
/// Created when entering `AppState::Home` for a given journal and cleared
/// when going back to the list.  Holds both transient UI state (filters,
/// selection, form fields) and the loaded entry headers shown in the sidebar.
pub struct HomeState {
    pub journal_id: Uuid,
    pub journal_root: PathBuf,
    pub journal_name: String,

    /// Cached headers for the entry sidebar; refreshed when `needs_reload` is set.
    pub entries: Vec<EntryHeader>,
    pub entries_error: Option<String>,
    pub needs_reload: bool,

    /// Currently-selected entry, if any.
    pub selected_path: Option<PathBuf>,
    pub editor: Option<EditorState>,

    /// Hierarchical (tree) or flat (list) display of entries.
    pub view_mode: ViewMode,
    /// IDs of tree nodes whose children are currently hidden.  Default open
    /// (a node not in this set is expanded).
    pub collapsed: HashSet<GrainId>,
    /// Whether the period / sort / order pickers are visible. Toggled by
    /// the funnel icon in the sidebar toolbar.  Search input stays visible.
    pub show_filters: bool,

    // ── Sidebar filter / sort state ─────────────────────────────────────────
    pub filter_text: String,
    /// Period preset (empty = all). One of "" | "today" | "yesterday" | ...
    pub period: String,
    /// Sort field. One of "updated_at" | "created_at" | "title" | "id"
    /// | "task_due" | "event_start".
    pub sort_by: String,
    /// "asc" | "desc".
    pub sort_order: String,

    pub confirm_delete_entry: bool,
    pub error_msg: Option<String>,
    pub info_msg: Option<String>,
}

impl HomeState {
    pub fn new(entry: RegistryEntry) -> Self {
        Self {
            journal_id: entry.id,
            journal_root: entry.storage_path,
            journal_name: entry.name,
            entries: Vec::new(),
            entries_error: None,
            needs_reload: true,
            selected_path: None,
            editor: None,
            view_mode: ViewMode::Tree,
            collapsed: HashSet::new(),
            show_filters: false,
            filter_text: String::new(),
            period: String::new(),
            sort_by: "updated_at".to_string(),
            sort_order: "desc".to_string(),
            confirm_delete_entry: false,
            error_msg: None,
            info_msg: None,
        }
    }
}

/// How the entry sidebar should display entries.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Hierarchical view following `parent_id` relationships.
    Tree,
    /// Flat list, ignoring hierarchy.
    List,
}

/// Form fields for the entry currently being edited in the main panel.
///
/// The path of the entry being edited lives in `HomeState::selected_path` so
/// that selection-aware UI (e.g. the sidebar's active-row highlight) and the
/// editor stay in sync.
pub struct EditorState {
    pub id: String,
    pub title: String,
    /// Comma-separated tag input.
    pub tags: String,
    pub body: String,
    pub has_task: bool,
    pub task_status: String,
    /// `YYYY-MM-DD` (empty = no due date).
    pub task_due: String,
    pub has_event: bool,
    pub event_start: String,
    pub event_end: String,
}

impl App {
    pub fn new() -> Self {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("failed to start tokio runtime"),
        );
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let registry = JournalRegistry::load().unwrap_or_default();
        let settings = Settings::load().unwrap_or_default();

        // Resume into the previously-open journal when possible; otherwise
        // fall back to the list screen as a first-run / picker fallback.
        let (screen, home) = match settings
            .last_opened_journal_id
            .and_then(|id| registry.journals.iter().find(|e| e.id == id).cloned())
            .filter(|entry| entry.storage_path.join(".sapphire-journal").is_dir())
        {
            Some(entry) => {
                let id = entry.id;
                (AppState::Home { journal_id: id }, Some(HomeState::new(entry)))
            }
            None => (AppState::List, None),
        };

        Self {
            screen,
            registry,
            settings,
            dialog: None,
            error_msg: None,
            home,
            previous_journal_id: None,
            runtime,
            event_tx,
            event_rx,
        }
    }

    /// Update `settings.last_opened_journal_id` and persist immediately.
    /// Errors fall into `self.error_msg`.
    pub fn remember_last_opened(&mut self, id: Option<Uuid>) {
        if self.settings.last_opened_journal_id == id {
            return;
        }
        self.settings.last_opened_journal_id = id;
        if let Err(e) = self.settings.save() {
            self.error_msg = Some(format!("Failed to save settings: {e}"));
        }
    }

    fn handle_events(&mut self) {
        while let Ok(ev) = self.event_rx.try_recv() {
            match ev {
                AppEvent::JournalAdded(entry) => {
                    self.registry.add(entry);
                    if let Err(e) = self.registry.save() {
                        self.error_msg = Some(e.to_string());
                    }
                    self.dialog = None;
                }
                AppEvent::JournalRemoved(id) => {
                    self.registry.remove_by_id(id);
                    if let Err(e) = self.registry.save() {
                        self.error_msg = Some(e.to_string());
                    }
                    self.dialog = None;
                }
                AppEvent::Error(msg) => {
                    self.error_msg = Some(msg);
                    self.dialog = None;
                }
                AppEvent::CleanupAndError {
                    storage_path,
                    error,
                } => {
                    let _ = std::fs::remove_dir_all(&storage_path);
                    self.error_msg = Some(error);
                    self.dialog = None;
                }
            }
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // While a clone is in progress we need the UI to keep repainting so the
        // progress bar advances even when no input events arrive.
        if let Some(DialogState::Clone(state)) = &self.dialog {
            if state.progress.lock().unwrap().is_some() {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
        }

        match self.screen.clone() {
            AppState::List => screens::journal_list::show(self, ui),
            AppState::Home { journal_id } => screens::journal_home::show(self, ui, journal_id),
        }
    }
}
