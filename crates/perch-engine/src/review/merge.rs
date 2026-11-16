//! What an agent is asked when it resolves a pull request's conflicts.
//! The merge box is perch-app's `review::merge`.

/// The first line of what an agent is asked when it resolves conflicts,
/// by which its run is known again after the item is reopened.
pub const CONFLICTS_REQUEST: &str = "Resolve the merge conflicts with ";

/// What an agent is asked to resolve conflicts with `base`, which the
/// checkout has at `rev`, on `branch`: a merge, since the pull request's
/// commits are on GitHub already and a rebase would need a force push.
pub fn conflicts_request(base: &str, rev: &str, branch: &str) -> String {
    format!(
        "{CONFLICTS_REQUEST}{base}.\n\n\
         Merge {rev}, which is {base} as GitHub has it now, into {branch} with \
         `git merge -m \"Merge {base} into {branch}\" {rev}`. The merge writes to the git \
         directory as a commit does, so run it the way you would run a commit here. Resolve \
         each conflict so the changes on both sides still hold, then build and run the tests \
         that cover what you touched. Commit the merge once it is clean. Do not rebase, and \
         do not push."
    )
}
