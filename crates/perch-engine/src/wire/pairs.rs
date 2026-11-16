//! A map keyed by something other than a string, as a list of pairs. JSON
//! keys are strings, and serde_json refuses to write a map keyed by a
//! tuple, so the event that carries one would never leave the engine.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// For an `Option<Arc<HashMap<K, V>>>`, as every window shares it.
pub mod shared {
    use super::*;

    pub fn serialize<K, V, S>(
        map: &Option<Arc<HashMap<K, V>>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        K: Serialize,
        V: Serialize,
        S: Serializer,
    {
        map.as_ref()
            .map(|map| map.iter().collect::<Vec<_>>())
            .serialize(serializer)
    }

    pub fn deserialize<'de, K, V, D>(
        deserializer: D,
    ) -> Result<Option<Arc<HashMap<K, V>>>, D::Error>
    where
        K: Deserialize<'de> + Eq + Hash,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Ok(Option::<Vec<(K, V)>>::deserialize(deserializer)?
            .map(|pairs| Arc::new(pairs.into_iter().collect())))
    }
}
