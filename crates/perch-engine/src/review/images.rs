//! The pictures a comment shows, as the engine downloads them. Decoding
//! one into the pixels the renderer draws is the window's, in perch-app's
//! `review::images`.

use std::sync::Arc;

/// A picture's bytes as they came, shared by every window that hears of
/// them rather than copied to each.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Downloaded(#[serde(with = "crate::wire::bytes::shared")] pub Arc<[u8]>);

/// A picture can be megabytes; a log line wants its size.
impl std::fmt::Debug for Downloaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Downloaded({} bytes)", self.0.len())
    }
}
