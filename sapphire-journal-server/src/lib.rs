//! `sapphire-journal-server` — framework の同期 API と journal の MCP を
//! 1 プロセスで提供する。
//!
//! ## 前提
//!
//! **プライベート網（VPN / Tailscale / LAN）でのみ使うこと。** TLS も OAuth も
//! 持たない。認証は共有の bearer トークンだけで、鍵は平文で保存される。

pub mod cli;
pub mod keys;
