//! Bytes as base64 on the wire. serde writes a `Vec<u8>` to JSON as an
//! array of numbers, four characters a byte; a face or a picture is
//! kilobytes to megabytes, so it goes as one string instead.

use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    let text = String::deserialize(deserializer)?;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(serde::de::Error::custom)
}

/// The same for bytes behind an `Arc`, which every window shares.
pub mod shared {
    use std::sync::Arc;

    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Arc<[u8]>, serializer: S) -> Result<S::Ok, S::Error> {
        super::serialize(bytes, serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<[u8]>, D::Error> {
        super::deserialize(deserializer).map(Arc::from)
    }
}
