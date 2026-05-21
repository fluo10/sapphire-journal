pub mod server;

#[cfg(feature = "http-server")]
pub mod http;

pub use server::{run, ArchelonServer};

#[cfg(feature = "http-server")]
pub use http::serve_http;
