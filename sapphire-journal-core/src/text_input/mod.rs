//! Helpers that convert text-frontend inputs (strings from CLI args or MCP
//! JSON parameters) into the strongly-typed values consumed by [`ops`] and
//! related core APIs.
//!
//! Gated behind the `text-input` feature so GUI frontends — which build typed
//! values directly from widgets — don't pay for code they wouldn't use.
//!
//! [`ops`]: crate::ops

pub mod fields;
pub mod filter;
