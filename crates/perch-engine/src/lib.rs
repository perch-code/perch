//! Everything below Perch's window: the GitHub client and the cache, the
//! checkouts, the language servers and the agent runs, and the data the
//! window and the engine pass each other. Nothing here draws, so the
//! engine runs the same in the app's process or on a host with no
//! window.

pub mod approvals;
pub mod ask;
pub mod backend;
pub mod context;
pub mod feedback;
pub mod inbox;
pub mod items;
pub mod lock;
pub mod log_file;
mod nested_git;
pub mod repos;
pub mod review;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod update;
pub mod wire;
