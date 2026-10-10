//! A pull request's Changescape, found in Changescape's cache off the UI
//! thread, and built by Claude Code with the Changescape skill when the
//! reader asks. File reads only: Perch computes nothing of a changescape.
//!
//! Builds run outside the durable run queue, so one cut short by quitting
//! leaves `running` until the next build.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use perch_changescape::{LoadedPage, Lookup, State};

use super::{Ctx, Event};

/// What the cache holds for one pull request at one head, and its page
/// when there is one to draw.
#[derive(Debug, Clone)]
pub struct Found {
    pub lookup: Lookup,
    pub page: Option<Arc<LoadedPage>>,
    /// Why the page that should be there could not be read.
    pub error: Option<String>,
    /// Whether this Perch can build one: the skill is installed, it
    /// writes a tour, and Claude Code is found.
    pub can_build: bool,
    /// The installed skill is from before the tour: a build with it would
    /// write a page with no tour over the one here.
    pub skill_too_old: bool,
    /// Whether a build this Perch started for the pull request is still
    /// running, whatever its status says: the skill marks the facts done
    /// before the agent writes the captions.
    pub building: bool,
    /// The reader's marks on the pull request's tour stops, by stop id,
    /// each with the stop's hash when it was made.
    pub marks: HashMap<String, perch_github::StoredMark>,
}

/// Look the pull request up and send what was found, `None` when
/// Changescape is turned off. Logs nothing when there is none.
pub(super) async fn lookup(
    ctx: &Ctx,
    item_id: String,
    repo: String,
    number: u64,
    head_sha: String,
) {
    let config = ctx.config();
    let found = if config.changescape.enabled {
        let root = config.changescape.cache_root();
        let head = head_sha.clone();
        let binary = config.agent.binary.clone();
        let building = building().lock().unwrap().contains(&(repo.clone(), number));
        let marks = ctx
            .with_cache(|c| c.load_tour_marks(&repo, number))
            .and_then(|marks| {
                marks
                    .map_err(|e| tracing::warn!(error = %e, "could not read tour marks"))
                    .ok()
            })
            .unwrap_or_default();
        tokio::task::spawn_blocking(move || {
            let mut found = if building {
                find_during_run(&root, &repo, number, &head)
            } else {
                find(&root, &repo, number, &head)
            };
            let ability = can_build(&binary, false);
            found.can_build = ability.can;
            found.skill_too_old = ability.skill_too_old;
            found.building = building;
            found.marks = marks;
            found
        })
        .await
        .ok()
    } else {
        None
    };
    let _ = ctx.events.send(Event::Changescape {
        item_id,
        head_sha,
        found: found.map(Arc::new),
    });
}

pub(crate) fn find(root: &Path, repo: &str, number: u64, head_sha: &str) -> Found {
    found(perch_changescape::lookup(root, repo, number, head_sha))
}

/// `find`, while this Perch's own build of the head still runs: a failure
/// the skill wrote mid-run is not the end yet (`lookup_during_run`).
pub(crate) fn find_during_run(root: &Path, repo: &str, number: u64, head_sha: &str) -> Found {
    found(perch_changescape::lookup_during_run(
        root, repo, number, head_sha,
    ))
}

fn found(mut lookup: Lookup) -> Found {
    let (page, error) = match lookup.drawable().map(perch_changescape::load) {
        Some(Ok(page)) => (Some(page), None),
        // The page's own version, where the status misstates it.
        Some(Err(perch_changescape::Error::Version { schema, .. })) => {
            lookup = lookup.with_page_schema(schema);
            (None, None)
        }
        Some(Err(e)) => {
            tracing::warn!(error = %e, "could not read a changescape");
            (None, Some(e.to_string()))
        }
        None => (None, None),
    };
    Found {
        lookup,
        page,
        error,
        can_build: false,
        skill_too_old: false,
        building: false,
        marks: HashMap::new(),
    }
}

/// Whether a build can run, and why not when the skill is the reason.
#[derive(Debug, Clone, Copy, Default)]
struct Ability {
    can: bool,
    skill_too_old: bool,
}

/// Whether the skill and Claude Code are both there, and the skill writes
/// a tour. Looked for once a session, and again on each request to
/// build, so installing or updating the skill takes effect at the next
/// one.
fn can_build(binary: &str, again: bool) -> Ability {
    static KNOWN: OnceLock<Mutex<Option<Ability>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Default::default);
    if !again {
        if let Some(ability) = *known.lock().unwrap() {
            return ability;
        }
    }
    let claude = perch_agent::find_binary(binary);
    let skill = perch_changescape::skill_available();
    let skill_too_old = skill
        .as_deref()
        .is_some_and(|dir| !perch_changescape::skill_writes_tour(dir));
    let can = claude.is_some() && skill.is_some() && !skill_too_old;
    tracing::info!(
        ?claude,
        ?skill,
        skill_too_old,
        can,
        "changescape build available"
    );
    let ability = Ability { can, skill_too_old };
    *known.lock().unwrap() = Some(ability);
    ability
}

/// The pull requests with a build running, by repository and number:
/// one at a time each.
fn building() -> &'static Mutex<HashSet<(String, u64)>> {
    static BUILDING: OnceLock<Mutex<HashSet<(String, u64)>>> = OnceLock::new();
    BUILDING.get_or_init(Default::default)
}

/// Build an item's changescape at its pull request's head. A second
/// request while one runs only looks again, which shows the running one.
pub(super) async fn build(ctx: &Ctx, item_id: String) {
    let config = ctx.config();
    if !config.changescape.enabled {
        return;
    }
    let item = match ctx.with_cache(|cache| cache.load_item(&item_id)) {
        Some(Ok(Some(item))) => item,
        _ => {
            ctx.notice("Could not build the changescape: the item is not in the cache");
            return;
        }
    };
    let Some(pull) = item.pull_request().cloned() else {
        return;
    };
    let repo = item.repo_full_name.clone();
    let (number, head) = (pull.number, pull.head_sha.clone());
    // A build that cannot run leaves the folder as it is: a page there
    // stays drawn.
    let checked = config.agent.binary.clone();
    let ability = tokio::task::spawn_blocking(move || can_build(&checked, true))
        .await
        .unwrap_or_default();
    if !ability.can {
        ctx.notice(if ability.skill_too_old {
            SKILL_TOO_OLD
        } else {
            "The Changescape skill or Claude Code is not installed."
        });
        lookup(ctx, item_id, repo, number, head).await;
        return;
    }
    let key = (repo.clone(), number);
    if !building().lock().unwrap().insert(key.clone()) {
        lookup(ctx, item_id, repo, number, head).await;
        return;
    }
    let result = run_build(ctx, &config, &item_id, &repo, number, &head).await;
    building().lock().unwrap().remove(&key);
    let root = config.changescape.cache_root();
    let dir = perch_changescape::build_dir(&root, &repo, number, &head);
    if let Err(message) = &result {
        tracing::warn!(item = %item_id, error = %message, "changescape build failed");
    }
    settle(&dir, result);
    lookup(ctx, item_id, repo, number, head).await;
}

async fn run_build(
    ctx: &Ctx,
    config: &perch_github::Config,
    item_id: &str,
    repo: &str,
    number: u64,
    head: &str,
) -> Result<(), String> {
    let configured = config.agent.binary.clone();
    let binary = perch_agent::find_binary(&configured).ok_or("Claude Code is not installed.")?;
    let id = perch_git::RepoId::parse(repo).ok_or("Not a GitHub repository.")?;
    let store = ctx.preparer.store();
    if !store.has_bare_clone(&id) {
        return Err("Perch has no clone of this repository yet.".into());
    }
    let clone = store.bare_path(&id);
    let root = config.changescape.cache_root();
    let dir = perch_changescape::build_dir(&root, repo, number, head);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    write_status(&dir, State::Running, None)?;
    // The pill comes now, with the build's progress under it.
    lookup(ctx, item_id.into(), repo.into(), number, head.into()).await;

    let token_config = config.clone();
    let github_token =
        tokio::task::spawn_blocking(move || perch_github::resolve_token(&token_config))
            .await
            .ok()
            .flatten()
            .map(|token| token.value);
    // The shell takes only the commands the build's rules name, so a loop,
    // a variable, or an environment prefix on a command is refused.
    let prompt = format!(
        "Use the changescape skill on pull request #{number} of {repo}.\n\
         The repository is at {}.\n\
         Write the output to {}/.\n\
         The shell runs only plain commands: no loops, no variables, and no \
         environment variables before a command. Use the Read and Grep tools \
         to read files.",
        clone.display(),
        dir.display()
    );
    let spec = perch_agent::ChangescapeSpec {
        binary,
        model: config.agent.model.clone(),
        repo: clone,
        cache_root: root,
        build_dir: dir.clone(),
        skill_dir: perch_changescape::skill_available(),
        github_token,
        prompt,
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = perch_agent::Run::spawn_changescape(spec, tx)
        .map_err(|e| format!("Could not start Claude Code: {e}"))?;
    tracing::info!(item = %item_id, number, "changescape build started");
    // The steps go to a file in the build's folder too, so a failed build
    // can be read after the tab is gone.
    let mut log = std::fs::File::create(dir.join(BUILD_LOG)).ok();
    while let Some(event) = rx.recv().await {
        if let Some(file) = log.as_mut() {
            use std::io::Write as _;
            let _ = writeln!(file, "{}", full_line(&event).unwrap_or_default());
        }
        if let Some(line) = progress_line(&event) {
            let _ = ctx.events.send(Event::ChangescapeProgress {
                item_id: item_id.to_string(),
                head_sha: head.to_string(),
                line,
            });
        }
    }
    match run.wait().await {
        perch_agent::RunOutcome::Finished { .. } => Ok(()),
        perch_agent::RunOutcome::Failed { message, .. } => Err(message),
        perch_agent::RunOutcome::Stopped { .. } => Err("The build was stopped.".into()),
    }
}

/// Why a build did not start with a skill from before the tour.
pub const SKILL_TOO_OLD: &str =
    "The installed Changescape skill writes no tour. Update the skill to build one.";

/// The file in a build's folder that keeps what the run did.
const BUILD_LOG: &str = "perch-build.log";

/// An event as the build's log keeps it: whole, where the tab shows one
/// line.
fn full_line(event: &perch_agent::RunEvent) -> Option<String> {
    use perch_agent::RunEvent;
    match event {
        RunEvent::Text(text) => Some(text.clone()),
        RunEvent::ToolResult { ok, summary, .. } => Some(format!(
            "  -> {}{summary}",
            if *ok { "" } else { "failed: " }
        )),
        RunEvent::Failed { message } => Some(format!("Failed: {message}")),
        other => progress_line(other),
    }
}

/// One line of progress for an event, as the tab lists them.
fn progress_line(event: &perch_agent::RunEvent) -> Option<String> {
    use perch_agent::RunEvent;
    match event {
        RunEvent::ToolCall { name, summary, .. } if summary.is_empty() => Some(name.clone()),
        RunEvent::ToolCall { name, summary, .. } => Some(format!("{name}: {summary}")),
        RunEvent::Text(text) => text
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_string()),
        RunEvent::PermissionDenied { tool, message } => Some(format!(
            "Refused{}: {message}",
            tool.as_deref().map(|t| format!(" {t}")).unwrap_or_default()
        )),
        _ => None,
    }
}

/// `status.json` as Perch writes it: a fresh one when a build starts, and
/// when one ends without the skill having said how, the skill's own with
/// the end written in, so its page's version stays.
fn write_status(dir: &Path, state: State, error: Option<&str>) -> Result<(), String> {
    let now = Some(jiff::Timestamp::now().to_string());
    let mut status = match state {
        State::Running => None,
        State::Done | State::Failed => read_status(dir),
    }
    .unwrap_or_else(|| perch_changescape::bare_status(state, None));
    status.state = state;
    status.error = error.map(str::to_string);
    status.started = status.started.or(now.clone());
    status.finished = if state == State::Running { None } else { now };
    let path = dir.join("status.json");
    let json = serde_json::to_string(&status).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("Could not write {}: {e}", path.display()))
}

fn read_status(dir: &Path) -> Option<perch_changescape::Status> {
    let bytes = std::fs::read(dir.join("status.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The status a finished run leaves: failed where it failed or wrote no
/// page, else done.
fn settle(dir: &Path, result: Result<(), String>) {
    match result {
        Err(message) => settle_failed(dir, &message),
        Ok(()) if !dir.join("page.json").is_file() => {
            settle_failed(dir, "The build ended without writing page.json.")
        }
        Ok(()) => settle_running(dir),
    }
}

/// A build that ended badly: failed, with why, while its status still
/// says running. A failure the skill wrote stands, and so does a finished
/// build that a run stopped before it began.
fn settle_failed(dir: &Path, message: &str) {
    let ended = read_status(dir).is_some_and(|status| status.state != State::Running);
    if !ended {
        let _ = write_status(dir, State::Failed, Some(message));
    }
}

/// A build with a page whose status still says running: done.
fn settle_running(dir: &Path) {
    let running = read_status(dir).is_none_or(|status| status.state == State::Running);
    if running {
        let _ = write_status(dir, State::Done, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_with_no_page_fails_and_one_with_a_page_settles_done() {
        let dir = tempfile::tempdir().unwrap();
        write_status(dir.path(), State::Running, None).unwrap();
        settle_failed(dir.path(), "no page");
        let status = read_status(dir.path()).unwrap();
        assert_eq!(status.state, State::Failed);
        assert_eq!(status.error.as_deref(), Some("no page"));

        // The skill's own failure stands.
        settle_failed(dir.path(), "something else");
        assert_eq!(
            read_status(dir.path()).unwrap().error.as_deref(),
            Some("no page")
        );

        // A finished build stays finished.
        write_status(dir.path(), State::Done, None).unwrap();
        settle_failed(dir.path(), "no clone");
        assert_eq!(read_status(dir.path()).unwrap().state, State::Done);

        write_status(dir.path(), State::Running, None).unwrap();
        settle_running(dir.path());
        assert_eq!(read_status(dir.path()).unwrap().state, State::Done);
    }

    #[test]
    fn a_run_that_ends_well_without_a_page_fails() {
        let dir = tempfile::tempdir().unwrap();
        write_status(dir.path(), State::Running, None).unwrap();
        settle(dir.path(), Ok(()));
        let status = read_status(dir.path()).unwrap();
        assert_eq!(status.state, State::Failed);
        assert_eq!(
            status.error.as_deref(),
            Some("The build ended without writing page.json.")
        );

        write_status(dir.path(), State::Running, None).unwrap();
        std::fs::write(dir.path().join("page.json"), "{}").unwrap();
        settle(dir.path(), Ok(()));
        assert_eq!(read_status(dir.path()).unwrap().state, State::Done);
    }

    #[test]
    fn a_status_perch_settles_keeps_the_skills_page_version() {
        let dir = tempfile::tempdir().unwrap();
        let mut status = perch_changescape::bare_status(State::Running, None);
        status.page_schema = Some(3);
        status.skill_version = "3.0.0".into();
        std::fs::write(
            dir.path().join("status.json"),
            serde_json::to_string(&status).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.path().join("page.json"), "{}").unwrap();
        settle(dir.path(), Ok(()));
        let settled = read_status(dir.path()).unwrap();
        assert_eq!(settled.state, State::Done);
        assert_eq!(settled.page_schema, Some(3));
        assert_eq!(settled.skill_version, "3.0.0");
        assert!(settled.finished.is_some());
    }

    #[test]
    fn progress_names_the_tool_and_the_first_line_said() {
        let call = perch_agent::RunEvent::ToolCall {
            id: "1".into(),
            name: "Bash".into(),
            summary: "uv run collect.py".into(),
            parent_id: None,
        };
        assert_eq!(
            progress_line(&call).as_deref(),
            Some("Bash: uv run collect.py")
        );
        let text = perch_agent::RunEvent::Text("\nReading the diff.\nMore.".into());
        assert_eq!(progress_line(&text).as_deref(), Some("Reading the diff."));
    }
}
