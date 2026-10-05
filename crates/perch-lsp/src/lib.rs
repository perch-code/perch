//! A small language server client.
//!
//! Hand-rolled rather than built on a framework: the review view needs a
//! dozen requests and three notifications, and owning the loop keeps request
//! matching, progress reporting, and shutdown explicit.

pub mod client;
pub mod install;
pub mod server;
pub mod transport;

pub use client::{Client, Request};
pub use install::install;
pub use server::{
    find, kinds_at_root, language_id, outdated, partial, start_warnings, sweep, Env, Install,
    Missing, Need, Outdated, ServerKind, ServerSpec, Status,
};

pub use lsp_types;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not start {binary}: {source}")]
    Spawn {
        binary: String,
        source: std::io::Error,
    },
    #[error("the language server exited")]
    Exited,
    #[error("the language server exited: {0}")]
    ExitedSaying(String),
    #[error("the language server did not answer in time")]
    Timeout,
    /// Asked before the server had read the workspace, and answered with
    /// nothing, or not at all, before it had.
    #[error("{server} is still loading")]
    Loading { server: &'static str },
    #[error("the language server reported: {0}")]
    Server(String),
}
