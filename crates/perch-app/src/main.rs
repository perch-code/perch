//! Perch: a read-only GitHub notifications inbox for the desktop.

mod app_icon;
mod ask;
mod backend;
mod filter;
mod icons;
mod items;
mod items_cli;
mod knowledge;
mod review;
mod state;
mod theme;
mod timefmt;
mod ui;

use std::rc::Rc;

use floem::ext_event::update_signal_from_channel;
use floem::kurbo::Size;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::window::WindowConfig;
use floem::Application;

use crate::state::AppState;

mod dump;
mod scan_cli;
    // The changed list under a scope, as the Files tab would show it.
    if args.get(1).map(String::as_str) == Some("--scope") {
        scope_cli::run(&args[2..]);
        return;
    }
    // An item's Conversation as text, from the cache alone: one line per
    // card, in the order the window shows them.
    if args.get(1).map(String::as_str) == Some("--timeline") {
        timeline_cli::run(&args[2..]);

fn main() {
    // A debugging mode that renders a file the way the editor would, without
    // a window. The screen has to be unlocked to look at the real thing, and
    // this checks everything up to the pixels.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--dump-file") {
        dump::run(&args[2..]);
        return;
    }
    // The same, for a question: the prompt as the app would build it, the
    // run's events one per line, then the answer.
    if args.get(1).map(String::as_str) == Some("--ask") {
        ask::run_cli(&args[2..]);
        return;
    }
    // The sidebar as text, from the cache alone: Priority, Pinned, then each
    // section and its rows.
    if args.get(1).map(String::as_str) == Some("--items") {
        items_cli::run(&args[2..]);
        return;
    }
    // What the root scan finds under each project's roots, without writing.
    if args.get(1).map(String::as_str) == Some("--scan") {
        scan_cli::run(&args[2..]);
        return;
    }
<<<<<<< HEAD
    // The changed list under a scope, as the Files tab would show it.
    if args.get(1).map(String::as_str) == Some("--scope") {
        scope_cli::run(&args[2..]);
=======
    // An item's Conversation as text, from the cache alone: one line per
    // card, in the order the window shows them.
    if args.get(1).map(String::as_str) == Some("--timeline") {
        timeline_cli::run(&args[2..]);
>>>>>>> worktree-wf_6e414703-a53-1
        return;
    }

    tracing_subscriber::fmt()
        // stderr, so logs are not held in a block-buffered stdout when the
        // app is launched with its output redirected to a file.
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "perch=info,warn".into()),
        )
        .init();
    tracing::info!("perch starting");

    let (backend, events) = backend::spawn();
    let backend = Rc::new(backend);

    // A development hook: open a pull request at launch rather than by
    // clicking, so the whole chain can be exercised from a terminal.
    //   PERCH_OPEN_PR=slatedb/slatedb#2092 perch
    let open_at_launch = std::env::var("PERCH_OPEN_PR").ok().and_then(|spec| {
        let (repo, number) = spec.split_once('#')?;
        Some((repo.to_string(), number.parse::<u64>().ok()?))
    });
    tracing::info!(hook = ?open_at_launch, "launch hook");

    Application::new()
        .window(
            move |_| {
                app_icon::install();
                let state = AppState::new();
                let review = review::model::ReviewState::new();

                // Backend events arrive on a std channel. floem drains it on
                // the UI thread and writes each one into this signal; the
                // effect below folds them into app state one at a time.
                let incoming = RwSignal::new(None::<backend::Event>);
                update_signal_from_channel(incoming.write_only(), events);
                floem::reactive::Effect::new(move |_| {
                    if let Some(event) = incoming.get() {
                        // Folding an event is not a derivation. Untracked, so
                        // that a signal this happens to read on the way past
                        // cannot subscribe the effect to its own output and
                        // have the last event applied twice.
                        floem::reactive::Effect::untrack(|| {
                            state.apply_with_review(event, review)
                        });
                    }
                });

                // PERCH_OPEN_BRANCH=owner/repo:branch opens a branch item once
                // the cache has painted it. Its id is not known until then.
                if let Ok(spec) = std::env::var("PERCH_OPEN_BRANCH") {
                    if let Some((repo, branch)) = spec.split_once(':') {
                        let (repo, branch) = (repo.to_string(), branch.to_string());
                        let backend = backend.clone();
                        floem::reactive::Effect::new(move |opened: Option<bool>| {
                            if opened == Some(true) {
                                return true;
                            }
                            let found = state.items.with(|all| {
                                all.iter()
                                    .find(|i| i.repo_full_name.eq_ignore_ascii_case(&repo) && i.branch() == Some(branch.as_str()))
                                    .map(|i| i.id.clone())
                            });
                            match found {
                                Some(id) => {
                                    review.opening.set(true);
                                    backend.open_item(&id);
                                    true
                                }
                                None => false,
                            }
                        });
                    }
                }

                // PERCH_OPEN_ISSUE=owner/repo#N opens an issue the same way.
                if let Some((repo, number)) = std::env::var("PERCH_OPEN_ISSUE").ok().and_then(|spec| {
                    let (repo, number) = spec.split_once('#')?;
                    Some((repo.to_string(), number.parse::<u64>().ok()?))
                }) {
                    review.opening.set(true);
                    backend.open_subject(&repo, perch_github::ItemKind::Issue, number);
                }

                // PERCH_START_WORK=1 presses Start work once the issue is up.
                if std::env::var("PERCH_START_WORK").is_ok() {
                    let backend = backend.clone();
                    floem::reactive::Effect::new(move |pressed: Option<bool>| {
                        if pressed == Some(true) {
                            return true;
                        }
                        let issue_item = review.issue.with(|i| i.is_some())
                            .then(|| review.item.with(|i| i.as_ref().map(|i| i.id.clone())))
                            .flatten();
                        match issue_item {
                            Some(id) => {
                                backend.start_work(&id);
                                true
                            }
                            None => false,
                        }
                    });
                }

                if let Some((repo, number)) = open_at_launch.clone() {
                    review.opening.set(true);
                    backend.open_subject(&repo, perch_github::ItemKind::PullRequest, number);

                    // Optionally open a file once the checkout lands, so the
                    // whole path can be exercised without a pointer.
                    if let Ok(path) = std::env::var("PERCH_OPEN_FILE") {
                        let backend = backend.clone();
                        floem::reactive::Effect::new(move |opened: Option<bool>| {
                            if opened == Some(true) || !review.is_open() {
                                return opened.unwrap_or(false);
                            }
                            if review.open_tab(&path) {
                                backend.open_file(&path);
                            }
                            true
                        });
                    }
                }

                ui::app_view(state, review, backend.clone())
            },
            Some(
                WindowConfig::default()
                    .size(Size::new(1280.0, 820.0))
                    .min_size(Size::new(900.0, 600.0))
                    .title("Perch")
                    .with_mac_os_config(|mac| {
                        // No title bar: the content runs to the top of the
                        // window and the app draws its own strip, with the
                        // traffic lights sitting in it.
                        //
                        // The offset is absolute. floem pins the red button's
                        // left edge here and places each next one 6px after
                        // the previous, at macOS's 14pt width, so the three
                        // span 54px. Centred in the 72px activity rail that
                        // leaves 9px on each side, and the same 9px above so
                        // the group sits square in its corner.
                        mac.full_size_content_view(true)
                            .transparent_title_bar(true)
                            .hide_title(true)
                            .traffic_lights_offset((9.0, 9.0))
                    }),
            ),
        )
        .run();
}
