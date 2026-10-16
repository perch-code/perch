//! A mutex per key, made when first asked for and gone once nobody holds
//! or waits on it.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use tokio::sync::{Mutex, OwnedMutexGuard};

/// Work on one key waits for other work on that key, and on no other.
#[derive(Debug, Default)]
pub struct KeyedMutex {
    locks: std::sync::Mutex<HashMap<String, Weak<Mutex<()>>>>,
}

impl KeyedMutex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hold `key` until the guard drops.
    pub async fn lock(&self, key: impl Into<String>) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self
                .locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // A waiter holds its Arc, so only keys nobody wants go.
            locks.retain(|_, lock| lock.strong_count() > 0);
            let key = key.into();
            match locks.get(&key).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(Mutex::new(()));
                    locks.insert(key, Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn one_key_waits_and_another_does_not() {
        let locks = Arc::new(KeyedMutex::new());
        let held = locks.lock("a").await;

        let other = tokio::time::timeout(Duration::from_millis(100), locks.lock("b")).await;
        assert!(other.is_ok(), "another key is free");

        let same = tokio::spawn({
            let locks = locks.clone();
            async move { locks.lock("a").await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!same.is_finished(), "the same key waits");
        drop(held);
        let _ = tokio::time::timeout(Duration::from_secs(1), same)
            .await
            .expect("the same key goes once it is let go");
    }

    #[tokio::test]
    async fn a_key_nobody_holds_is_forgotten() {
        let locks = KeyedMutex::new();
        drop(locks.lock("a").await);
        drop(locks.lock("b").await);
        assert_eq!(locks.locks.lock().unwrap().len(), 1, "only the last asked");
    }
}
