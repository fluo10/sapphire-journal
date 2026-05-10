use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::registry::{JournalRegistry, RegistryEntry};
use crate::screens;

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
    pub dialog: Option<DialogState>,
    pub error_msg: Option<String>,
    pub runtime: Arc<tokio::runtime::Runtime>,
    pub event_tx: mpsc::UnboundedSender<AppEvent>,
    pub event_rx: mpsc::UnboundedReceiver<AppEvent>,
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
        Self {
            screen: AppState::List,
            registry: JournalRegistry::load().unwrap_or_default(),
            dialog: None,
            error_msg: None,
            runtime,
            event_tx,
            event_rx,
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
