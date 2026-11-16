//! The words a timeline's lines are made of that the engine writes too.
//! The timeline itself is perch-app's `review::timeline`.

/// The seven characters a commit is read by.
pub fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// `n` and the noun that counts it: "1 comment", "3 comments".
pub fn count_of(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
