//! Perch: a desktop reader for the pull requests and issues in the
//! repositories the reader declares.

mod agents_cli;
mod app_icon;
mod app_menu;
mod approvals;
mod ask;
mod backend;
mod context;
mod icons;
mod inbox;
mod items;
mod items_cli;
mod reference;
mod repos;
mod review;
mod scrollers;
mod snooze;
mod state;
mod theme;
mod timefmt;
mod ui;
mod windows;

use floem::reactive::SignalWith;
use floem::{Application, IntoView};

mod dump;
mod scan_cli;
mod scope_cli;
mod timeline_cli;

fn main() {
    // Perch started by `cargo run` inherits rustup's pin to Perch's own
    // toolchain and the path to that toolchain's cargo, and would hand both
    // to everything it starts in a checkout: agents, git and its hooks, the
    // language server. rustup ranks the pin above the checkout's
    // rust-toolchain.toml, so a `cargo build` there would use Perch's
    // compiler, which the checkout may not build with, and leave a second
    // build in its target dir. RUSTUP_TOOLCHAIN_SOURCE only says where the
    // pin came from. Removed before any thread starts, since nothing may
    // read the environment while it changes. CARGO_HOME and RUSTUP_HOME
    // stay: they say where the user's installs are.
    for name in ["RUSTUP_TOOLCHAIN", "RUSTUP_TOOLCHAIN_SOURCE", "CARGO"] {
        std::env::remove_var(name);
    }
    // A Finder launch gets a bare PATH. Everything Perch starts inherits it:
    // gh, the agents, an npm-installed agent's `node`. Adding the install
    // dirs here, at the end, fixes them all at once.
    if let Some(path) = perch_agent::path_with_install_dirs(
        std::env::var_os("PATH").as_deref(),
        &perch_agent::install_dirs(),
    ) {
        std::env::set_var("PATH", path);
    }
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
    // The sidebar as text, from the cache alone: Pinned, then each section
    // and its rows.
    if args.get(1).map(String::as_str) == Some("--items") {
        items_cli::run(&args[2..]);
        return;
    }
    // App-wide agent activity and retained history, from the cache alone.
    if args.get(1).map(String::as_str) == Some("--agents") {
        agents_cli::run(&args[2..]);
        return;
    }
    // What the root scan finds under each repository's roots, without writing.
    if args.get(1).map(String::as_str) == Some("--scan") {
        scan_cli::run(&args[2..]);
        return;
    }
    // The changed list under a scope, as Code would show it.
    if args.get(1).map(String::as_str) == Some("--scope") {
        scope_cli::run(&args[2..]);
        return;
    }
    // An item's Conversation as text, from the cache alone: one line per
    // card, in the order the window shows them.
    if args.get(1).map(String::as_str) == Some("--timeline") {
        timeline_cli::run(&args[2..]);
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
    theme::install_code_font();
    theme::install_symbol_fallbacks();

    let (backend, events) = backend::spawn();
    let mut started = Some((backend, events));

    // A development hook: open a pull request at launch rather than by
    // clicking, so the whole chain can be exercised from a terminal.
    //   PERCH_OPEN_PR=slatedb/slatedb#2092 perch
    let open_at_launch = std::env::var("PERCH_OPEN_PR").ok().and_then(|spec| reference::launch_hook(&spec));
    tracing::info!(hook = ?open_at_launch, "launch hook");

    Application::new()
        // A click on the Dock icon with every window shut opens one.
        .on_event(|event| {
            if let floem::AppEvent::Reopen {
                has_visible_windows: false,
            } = event
            {
                windows::reopen();
            }
        })
        .window(
            move |_| {
                app_icon::install();
                app_menu::install();
                // Backend events arrive on a std channel, which floem drains
                // on the UI thread into the windows' state.
                if let Some((backend, events)) = started.take() {
                    windows::start(backend, events);
                }
                let Some((state, review, backend)) = windows::window(None) else {
                    return floem::views::Empty::new().into_any();
                };

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
                                    .find(|i| {
                                        i.repo_full_name.eq_ignore_ascii_case(&repo)
                                            && i.branch() == Some(branch.as_str())
                                    })
                                    .map(|i| i.id.clone())
                            });
                            match found {
                                Some(id) => {
                                    let navigation_id = backend.open_item(&id);
                                    state.begin_item_review(
                                        review,
                                        "Reading the worktree",
                                        &id,
                                        navigation_id,
                                    );
                                    true
                                }
                                None => false,
                            }
                        });
                    }
                }

                // PERCH_OPEN_ISSUE=owner/repo#N opens an issue the same way.
                if let Some((repo, number)) =
                    std::env::var("PERCH_OPEN_ISSUE").ok().and_then(|spec| reference::launch_hook(&spec))
                {
                    let navigation_id =
                        backend.open_subject(&repo, perch_github::ItemKind::Issue, number);
                    state.begin_open("Reading the issue", navigation_id);
                }

                // PERCH_START_WORK=1 presses Start work once the issue is up.
                if std::env::var("PERCH_START_WORK").is_ok() {
                    let backend = backend.clone();
                    floem::reactive::Effect::new(move |pressed: Option<bool>| {
                        if pressed == Some(true) {
                            return true;
                        }
                        let issue_item = review
                            .issue
                            .with(|i| i.is_some())
                            .then(|| review.item.with(|i| i.as_ref().map(|i| i.id.clone())))
                            .flatten();
                        match issue_item {
                            Some(id) => {
                                crate::review::start_work::start_work(state, &backend, &id);
                                true
                            }
                            None => false,
                        }
                    });
                }

                if let Some((repo, number)) = open_at_launch.clone() {
                    let navigation_id =
                        backend.open_subject(&repo, perch_github::ItemKind::PullRequest, number);
                    state.begin_open("Reading the pull request", navigation_id);

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
                            review.show_in_tree(&path);
                            true
                        });
                    }
                }

                windows::view(state, review, backend).into_any()
            },
            Some(windows::config()),
        )
        .run();
}
