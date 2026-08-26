pub mod server;

#[cfg(feature = "http-server")]
pub mod http;

pub use server::{run, SapphireJournalServer, WriteObserver};
// Re-exported so callers driving tool methods directly (e.g. tests outside
// this crate) don't need their own dependency on `rmcp` just for the
// parameter wrapper.
pub use rmcp::handler::server::wrapper::Parameters;

#[cfg(feature = "http-server")]
pub use http::serve_http;
