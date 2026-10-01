//! The bodies of recent REST reads, kept with their `ETag`s so the next
//! read of the same path can ask conditionally and take a 304 in place of
//! the body. A 304 does not count against GitHub's rate limit.
//!
//! Held in memory only, and bounded by the bytes of the bodies kept: when
//! the bound is passed, the paths read least recently go first.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How many bytes of bodies are kept. A large pull request's review
/// comments run to a few megabytes, so this holds several open at once.
pub(crate) const BUDGET_BYTES: usize = 32 * 1024 * 1024;

/// A kept body and the tag GitHub sent with it.
#[derive(Clone)]
pub(crate) struct Known {
    pub etag: String,
    pub body: Arc<str>,
}

struct Entry {
    known: Known,
    used: u64,
    /// When GitHub last said this body was current: it was read, or a
    /// conditional read of it came back 304.
    checked: Instant,
}

pub(crate) struct Responses {
    entries: HashMap<String, Entry>,
    bytes: usize,
    budget: usize,
    clock: u64,
}

impl Responses {
    pub fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            budget,
            clock: 0,
        }
    }

    /// What was last read at `path`, marked as just used.
    pub fn get(&mut self, path: &str) -> Option<Known> {
        self.clock += 1;
        let entry = self.entries.get_mut(path)?;
        entry.used = self.clock;
        Some(entry.known.clone())
    }

    /// What was last read at `path`, if GitHub said it was current no
    /// longer than `within` ago.
    pub fn fresh(&mut self, path: &str, within: Duration) -> Option<Known> {
        let checked = self.entries.get(path)?.checked;
        (checked.elapsed() <= within).then(|| self.get(path)).flatten()
    }

    /// GitHub said what is kept at `path` is still current.
    pub fn confirm(&mut self, path: &str) {
        if let Some(entry) = self.entries.get_mut(path) {
            entry.checked = Instant::now();
        }
    }

    /// Keep `body` as what `path` answers while `etag` holds. A body larger
    /// than the whole budget is not kept, and drops what was kept before.
    pub fn put(&mut self, path: &str, etag: String, body: Arc<str>) {
        self.remove(path);
        let size = cost(path, &body);
        if size > self.budget {
            return;
        }
        self.clock += 1;
        self.bytes += size;
        self.entries.insert(
            path.to_string(),
            Entry {
                known: Known { etag, body },
                used: self.clock,
                checked: Instant::now(),
            },
        );
        while self.bytes > self.budget {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(p, _)| p.clone())
            else {
                break;
            };
            self.remove(&oldest);
        }
    }

    pub fn remove(&mut self, path: &str) {
        if let Some(entry) = self.entries.remove(path) {
            self.bytes -= cost(path, &entry.known.body);
        }
    }
}

fn cost(path: &str, body: &str) -> usize {
    path.len() + body.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(n: usize) -> Arc<str> {
        "x".repeat(n).into()
    }

    #[test]
    fn a_kept_body_comes_back_with_its_tag() {
        let mut r = Responses::new(1000);
        r.put("/a", "\"1\"".into(), body(10));
        let known = r.get("/a").unwrap();
        assert_eq!(known.etag, "\"1\"");
        assert_eq!(known.body.len(), 10);
        assert!(r.get("/b").is_none());
    }

    #[test]
    fn a_new_body_replaces_the_old_one() {
        let mut r = Responses::new(1000);
        r.put("/a", "\"1\"".into(), body(10));
        r.put("/a", "\"2\"".into(), body(20));
        assert_eq!(r.get("/a").unwrap().etag, "\"2\"");
        assert_eq!(r.bytes, 22);
    }

    #[test]
    fn the_least_recently_read_path_goes_first() {
        let mut r = Responses::new(100);
        r.put("/a", "a".into(), body(38));
        r.put("/b", "b".into(), body(38));
        r.get("/a");
        r.put("/c", "c".into(), body(38));
        assert!(r.get("/a").is_some());
        assert!(r.get("/b").is_none());
        assert!(r.get("/c").is_some());
        assert_eq!(r.bytes, 80);
    }

    #[test]
    fn a_body_is_fresh_only_for_a_while_after_github_said_it_was_current() {
        let mut r = Responses::new(1000);
        r.put("/a", "a".into(), body(10));
        assert!(r.fresh("/a", Duration::from_secs(60)).is_some());
        assert!(r.fresh("/b", Duration::from_secs(60)).is_none());
        std::thread::sleep(Duration::from_millis(20));
        assert!(r.fresh("/a", Duration::from_millis(10)).is_none());
        r.confirm("/a");
        assert!(r.fresh("/a", Duration::from_millis(10)).is_some());
    }

    #[test]
    fn a_body_over_the_budget_is_not_kept() {
        let mut r = Responses::new(100);
        r.put("/a", "a".into(), body(10));
        r.put("/a", "a".into(), body(200));
        assert!(r.get("/a").is_none());
        assert_eq!(r.bytes, 0);
    }
}
