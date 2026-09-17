//! A small language server client.
//!
//! Hand-rolled rather than built on a framework: the review view needs seven
//! requests and three notifications, and owning the loop keeps request
//! matching, progress reporting, and shutdown explicit.

pub mod client;
pub mod server;
pub mod transport;

pub use client::{Client, Request};
pub use server::{find_server, ServerSpec, Status};

pub use lsp_types;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not start {binary}: {source}")]
    Spawn { binary: String, source: std::io::Error },
    #[error("the language server exited")]
    Exited,
    #[error("the language server did not answer in time")]
    Timeout,
    #[error("the language server reported: {0}")]
    Server(String),
}
