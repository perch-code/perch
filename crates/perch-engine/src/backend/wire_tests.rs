//! The wire-ready boundary: `Command` and `Event` as JSON, the connection
//! layer's window mapping and routing, the handshake, and real commands
//! through the wire to an engine that runs its command loop on a test
//! context (never `run`, which reads the reader's own config and token).

use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use super::connections::{Hub, Inbound, RESYNC};
use super::tests::{ctx, nowhere, pull_request};
use super::*;
use crate::wire::{self, Answer, Hello, PROTOCOL, VERSION};

/// Through JSON and back, compared as JSON: the types have no `PartialEq`,
/// and what matters is that nothing is lost on the way.
fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(value: &T) -> T {
    let json = serde_json::to_value(value).unwrap();
    let back: T = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(serde_json::to_value(&back).unwrap(), json);
    back
}

#[test]
fn commands_cross_the_wire_whole() {
    for command in [
        Command::WindowOpened,
        Command::Resync,
        Command::OpenItem {
            id: "i".into(),
            navigation_id: 1 << 40 | 3,
        },
        Command::SetScope {
            item_id: "i".into(),
            scope: perch_git::Scope::Commit { sha: "abc".into() },
            navigation_id: 4,
            scope_id: 1 << 40 | 5,
        },
        Command::ArchiveItem {
            id: "i".into(),
            fence_id: 9,
        },
        Command::RenameItem {
            id: "i".into(),
            name: Some("mine".into()),
        },
    ] {
        round_trip(&command);
    }
    // Unit variants read as their names, the rest as one key.
    assert_eq!(
        serde_json::to_string(&Command::Resync).unwrap(),
        "\"Resync\""
    );
    assert!(serde_json::to_string(&Command::ArchiveItem {
        id: "i".into(),
        fence_id: 9
    })
    .unwrap()
    .starts_with("{\"ArchiveItem\":"));
}

fn pr_item() -> Item {
    let mut item = Item::new("a/b", Timestamp::now());
    item.subject = Some(ItemSubject::PullRequest(pull_request(7)));
    item
}

#[test]
fn events_cross_the_wire_whole() {
    let mut item = pr_item();
    item.name = Some("named".into());
    let Event::Items(items) = round_trip(&Event::Items(vec![item.clone()])) else {
        panic!("not items");
    };
    assert_eq!(items[0].id, item.id);
    assert_eq!(items[0].name.as_deref(), Some("named"));

    let file = build_loaded_file(
        "src/lib.rs".into(),
        "fn a() {}\nfn b() {}\n".into(),
        "fn a() {}\n".into(),
        Some(perch_git::ChangeStatus::Modified),
        false,
        perch_git::Scope::Branch,
        "blob".into(),
    );
    let path = file.path.clone();
    let Event::FileLoaded { file: back, .. } = round_trip(&Event::FileLoaded {
        navigation_id: 1,
        scope_gen: 2,
        item_id: "i".into(),
        path: "src/lib.rs".into(),
        file: Arc::new(file),
    }) else {
        panic!("not a file");
    };
    assert_eq!(back.path, path);

    let run = perch_github::RunRecord {
        id: "run".into(),
        model: "opus".into(),
        ..Default::default()
    };
    round_trip(&Event::AgentRunUpdated { run });

    // Reveals are keyed by (path, blob), which JSON cannot have as a key.
    let reveals: perch_github::StoredReveals =
        [(("src/lib.rs".to_string(), "blob".to_string()), vec![(3, 9)])].into();
    let Event::ViewedReady {
        reveals: Some(back),
        viewed,
        ..
    } = round_trip(&Event::ViewedReady {
        item_id: "i".into(),
        viewed: Arc::new([("src/lib.rs".to_string(), "blob".to_string())].into()),
        reveals: Some(Arc::new(reveals.clone())),
    })
    else {
        panic!("not viewed, or no reveals");
    };
    assert_eq!(*back, reveals);
    assert_eq!(viewed.len(), 1);
    let Event::ViewedReady { reveals: None, .. } = round_trip(&Event::ViewedReady {
        item_id: "i".into(),
        viewed: Arc::default(),
        reveals: None,
    }) else {
        panic!("reveals from nowhere");
    };

    // Bytes go as base64, not as a list of numbers.
    let avatar = Event::Avatar {
        url: "u".into(),
        bytes: vec![0, 255, 7],
    };
    let json = serde_json::to_string(&avatar).unwrap();
    assert!(json.contains("\"AP8H\""), "{json}");
    let Event::Avatar { bytes, .. } = round_trip(&avatar) else {
        panic!("not an avatar");
    };
    assert_eq!(bytes, vec![0, 255, 7]);
    let Event::Picture {
        downloaded: Some(crate::review::images::Downloaded(picture)),
        ..
    } = round_trip(&Event::Picture {
        url: "u".into(),
        downloaded: Some(crate::review::images::Downloaded(Arc::from(&b"png"[..]))),
    })
    else {
        panic!("not a picture");
    };
    assert_eq!(&*picture, b"png");
}

#[test]
fn what_discovery_found_reaches_a_window_across_the_wire() {
    let mut config = Config::parse_text("[[repo]]\nroot = \"/srv/notes\"\n").unwrap();
    config
        .agent
        .codex
        .efforts
        .insert("gpt".into(), vec!["low".into(), "high".into()]);
    config.repo[0].resolve(Some("me/notes".into()));
    let Event::AgentConfig(agent) = round_trip(&Event::AgentConfig(config.agent.clone())) else {
        panic!("not agent config");
    };
    assert_eq!(agent.codex.efforts, config.agent.codex.efforts);
    let Event::Repos(repos) = round_trip(&Event::Repos(config.declared_repos())) else {
        panic!("not repos");
    };
    assert_eq!(repos[0].identity, "me/notes");
}

#[test]
fn the_token_stays_with_the_engine() {
    let config = Config::parse_text("token = \"ghp_secret\"\npoll_interval_secs = 90\n").unwrap();
    assert_eq!(config.token.as_deref(), Some("ghp_secret"));
    let settings = Event::Settings(SettingsFile {
        path: "/home/me/.config/perch/config.toml".into(),
        config: Some(Arc::new(config)),
        error: None,
        present: ["token".to_string()].into(),
        warnings: Vec::new(),
        knowledge_problems: Default::default(),
    });
    let json = serde_json::to_string(&settings).unwrap();
    assert!(!json.contains("ghp_secret"), "{json}");
    let Event::Settings(SettingsFile {
        config: Some(back), ..
    }) = serde_json::from_str(&json).unwrap()
    else {
        panic!("not settings");
    };
    assert_eq!((back.token.as_deref(), back.poll_interval_secs), (None, 90));

    // A file that doesn't parse because of its token line is said by line
    // number and toml's message, which never quotes the line, so its
    // reason crosses the wire with no token in it.
    for line in [
        "token = ghp_unquoted",
        "token = \"ghp_unquoted\" trailing",
        "token = [\"ghp_unquoted\"]",
        "token = { value = \"ghp_unquoted\" }",
    ] {
        let error = Config::parse_text(&format!("poll_interval_secs = 90\n{line}\n")).unwrap_err();
        assert!(error.starts_with("line 2: "), "{error}");
        assert!(!error.contains("ghp_unquoted"), "{error}");
    }
}

type Heard = UnboundedReceiver<(WindowId, Event)>;

fn listener() -> (impl Fn(WindowId, Event) + Send + Sync + 'static, Heard) {
    let (tx, rx) = unbounded_channel();
    (
        move |window, event| {
            let _ = tx.send((window, event));
        },
        rx,
    )
}

fn notice(text: &str) -> Event {
    Event::Notice(text.into())
}

fn heard(rx: &mut Heard) -> Vec<(WindowId, String)> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .map(|(window, event)| match event {
            Event::Notice(text) => (window, text),
            other => (window, format!("{other:?}").chars().take(20).collect()),
        })
        .collect()
}

/// The engine window a command ran as, and the command.
fn ran(rx: &mut UnboundedReceiver<Inbound>) -> Vec<(WindowId, String)> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .map(|inbound| match inbound {
            Inbound::Command(window, command, _) => (window, format!("{command:?}")),
            Inbound::Shutdown => (NO_WINDOW, "Shutdown".into()),
        })
        .collect()
}

#[test]
fn two_connections_keep_their_own_windows() {
    let (tx, mut commands) = unbounded_channel();
    let hub = Arc::new(Hub::new(tx, Lifetime::Service, Fencing::default()));
    let engine = Engine {
        hub: hub.clone(),
        runtime: tokio::runtime::Runtime::new().unwrap().handle().clone(),
    };
    let (sink_a, mut a_heard) = listener();
    let (sink_b, mut b_heard) = listener();
    let a = engine.connect(sink_a);
    let b = engine.connect(sink_b);
    // Homes 1 and 2; both clients call their first window 1.
    a.send(1, Command::WindowOpened).unwrap();
    b.send(1, Command::WindowOpened).unwrap();
    // Only a window opens or closes a window.
    a.send(NO_WINDOW, Command::WindowOpened).unwrap();
    b.send(NO_WINDOW, Command::WindowClosed).unwrap();
    // A resync, from any window, is the connection's.
    a.send(1, Command::Resync).unwrap();
    b.send(NO_WINDOW, Command::Resync).unwrap();
    a.send(
        NO_WINDOW,
        Command::RefreshOpenItem {
            item_id: "i".into(),
        },
    )
    .unwrap();
    // A window that never opened sends nothing.
    a.send(
        7,
        Command::RefreshOpenItem {
            item_id: "i".into(),
        },
    )
    .unwrap();
    assert_eq!(
        ran(&mut commands),
        vec![
            (3, "WindowOpened".into()),
            (4, "WindowOpened".into()),
            (RESYNC | 1, "Resync".into()),
            (RESYNC | 2, "Resync".into()),
            (1, "RefreshOpenItem { item_id: \"i\" }".into()),
        ]
    );

    // A window's own answer goes to its owner, as the owner's window.
    hub.route(3, notice("a's"));
    hub.route(4, notice("b's"));
    // A home's goes to its connection alone.
    hub.route(2, notice("b's home"));
    // What every window shows goes to all, the sender's as its window,
    // from a home as from a window.
    hub.route(3, Event::Items(Vec::new()));
    hub.route(2, Event::Repos(Vec::new()));
    // A repository's grants show wherever its item is open, so a revoke
    // in one window is seen in the others.
    hub.route(
        3,
        Event::Grants {
            repo: "r".into(),
            grants: Vec::new(),
        },
    );
    hub.route(NO_WINDOW, notice("everyone"));
    // A resync's goes to its asker alone, whatever it is.
    hub.route(RESYNC | 2, Event::Notifications(Vec::new()));
    assert_eq!(
        heard(&mut a_heard),
        vec![
            (1, "a's".into()),
            (1, "Items([])".into()),
            (NO_WINDOW, "Repos([])".into()),
            (1, "Grants { repo: \"r\", ".into()),
            (NO_WINDOW, "everyone".into()),
        ]
    );
    assert_eq!(
        heard(&mut b_heard),
        vec![
            (1, "b's".into()),
            (NO_WINDOW, "b's home".into()),
            (NO_WINDOW, "Items([])".into()),
            (NO_WINDOW, "Repos([])".into()),
            (NO_WINDOW, "Grants { repo: \"r\", ".into()),
            (NO_WINDOW, "everyone".into()),
            (NO_WINDOW, "Notifications([])".into()),
        ]
    );

    // Going closes a's window in the engine; b and the engine go on.
    drop(a);
    assert_eq!(ran(&mut commands), vec![(3, "WindowClosed".into())]);
    assert_eq!(engine.connections(), 1);
    hub.route(3, notice("late"));
    // The tail of a's resync has nobody to go to, not everybody.
    hub.route(RESYNC | 1, Event::Items(Vec::new()));
    hub.route(NO_WINDOW, notice("still here"));
    assert_eq!(heard(&mut b_heard), vec![(NO_WINDOW, "still here".into())]);
    b.send(1, Command::WindowClosed).unwrap();
    b.send(
        1,
        Command::RefreshOpenItem {
            item_id: "i".into(),
        },
    )
    .unwrap();
    drop(b);
    // A service engine outlives its clients.
    assert_eq!(ran(&mut commands), vec![(4, "WindowClosed".into())]);
}

/// The view `window` has in the engine.
fn view(fencing: &Fencing, window: WindowId) -> View {
    fencing
        .views
        .lock()
        .unwrap()
        .iter()
        .find(|view| view.window == window)
        .cloned()
        .expect("no view")
}

#[test]
fn a_command_is_fenced_as_it_is_sent_not_when_the_loop_gets_to_it() {
    // Nothing reads the commands, as when the loop is busy with a poll.
    let (tx, mut commands) = unbounded_channel();
    let fencing = Fencing::default();
    let engine = Engine {
        hub: Arc::new(Hub::new(tx, Lifetime::Service, fencing.clone())),
        runtime: tokio::runtime::Runtime::new().unwrap().handle().clone(),
    };
    let (backend, _events) = Backend::in_process(&engine);
    let window = backend.open_window();
    let first = window.open_item("a");
    let second = window.open_item("b");
    let fences = view(&fencing, 2).navigation_requests;
    assert!(!fences.is_current(first));
    assert!(fences.is_current(second));

    // A scope change is begun, and an archive suspends what it showed, as
    // they are sent; the loop is handed what that gave.
    window.set_scope("b", perch_git::Scope::Branch);
    let fence = window.archive_item("b");
    let fenced: Vec<Fenced> = std::iter::from_fn(|| commands.try_recv().ok())
        .filter_map(|inbound| match inbound {
            Inbound::Command(_, Command::SetScope { .. } | Command::ArchiveItem { .. }, fenced) => {
                Some(fenced)
            }
            _ => None,
        })
        .collect();
    assert!(fenced[0].scope_request.is_some());
    assert_eq!(
        fenced[1]
            .suspended_navigation
            .as_ref()
            .map(|suspended| suspended.fence_id),
        Some(fence)
    );
}

#[test]
fn attempts_are_the_engine_s_own_and_follow_every_one_heard_of() {
    let (tx, _commands) = unbounded_channel();
    let engine = Engine {
        hub: Arc::new(Hub::new(tx, Lifetime::Service, Fencing::default())),
        runtime: tokio::runtime::Runtime::new().unwrap().handle().clone(),
    };
    let (a, _a_events) = Backend::in_process(&engine);
    let (b, _b_events) = Backend::in_process(&engine);
    let a_attempts: Vec<u64> = (0..10).map(|_| a.next_attempt()).collect();
    let b_first = b.next_attempt();
    // Never the same as another client's.
    assert!(!a_attempts.contains(&b_first));
    // A window of b's that heard a's latest takes b's next after it.
    let latest = *a_attempts.last().unwrap();
    engine.hub.route(
        NO_WINDOW,
        Event::PullRequestMergeStarted {
            item_id: "i".into(),
            repo_full_name: "a/b".into(),
            number: 7,
            expected_head_sha: "sha".into(),
            attempt_id: latest,
        },
    );
    let b_next = b.next_attempt();
    assert!(b_next > latest && !a_attempts.contains(&b_next));
}

#[test]
fn the_app_s_engine_stops_with_its_last_connection() {
    let (tx, mut commands) = unbounded_channel();
    let engine = Engine {
        hub: Arc::new(Hub::new(tx, Lifetime::WhileConnected, Fencing::default())),
        runtime: tokio::runtime::Runtime::new().unwrap().handle().clone(),
    };
    let connection = engine.connect(|_, _| {});
    connection.send(1, Command::WindowOpened).unwrap();
    drop(connection);
    assert_eq!(
        ran(&mut commands),
        vec![
            (2, "WindowOpened".into()),
            (2, "WindowClosed".into()),
            (NO_WINDOW, "Shutdown".into()),
        ]
    );
}

/// An engine running its command loop on a test context, with no GitHub
/// and one item, a pull request. Lives as a host's would.
fn engine(dir: &Path) -> (Engine, String) {
    engine_with(dir, |_| {})
}

/// `engine`, with `prepare` run on its context before the loop starts.
fn engine_with(dir: &Path, prepare: impl FnOnce(&Ctx)) -> (Engine, String) {
    let (mut ctx, _untagged) = ctx(dir, &nowhere(), &[]);
    prepare(&ctx);
    let item = pr_item();
    ctx.with_cache(|cache| cache.insert_item(&item))
        .unwrap()
        .unwrap();
    let (tx, commands) = unbounded_channel();
    let fencing = Fencing {
        views: ctx.views.clone(),
        navigation: ctx.navigation_requests.clone(),
        scopes: ctx.scope_requests.clone(),
    };
    let hub = Arc::new(Hub::new(tx, Lifetime::Service, fencing));
    let router = hub.clone();
    ctx.events = EventSender::to(move |window, event| {
        router.route(window, event);
        Ok(())
    });
    tokio::spawn(command_loop(ctx, commands));
    (
        Engine {
            hub,
            runtime: tokio::runtime::Handle::current(),
        },
        item.id,
    )
}

/// Wait for the first event `pick` takes, skipping the rest: the loop's
/// own first poll, against a GitHub that is not there, says things too.
fn wait<T>(
    events: &std::sync::mpsc::Receiver<(WindowId, Event)>,
    mut pick: impl FnMut(WindowId, &Event) -> Option<T>,
) -> T {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let (window, event) = events.recv_timeout(left).expect("the event never came");
        if let Some(found) = pick(window, &event) {
            return found;
        }
    }
}

/// Nothing `pick` takes arrives for a while.
fn never<T>(
    events: &std::sync::mpsc::Receiver<(WindowId, Event)>,
    mut pick: impl FnMut(WindowId, &Event) -> Option<T>,
) {
    let deadline = std::time::Instant::now() + Duration::from_millis(300);
    while let Ok((window, event)) =
        events.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
    {
        assert!(pick(window, &event).is_none(), "{event:?} for {window}");
    }
}

fn is_gone_notice(window: WindowId, event: &Event) -> Option<WindowId> {
    matches!(event, Event::Notice(text) if text == "That item is gone").then_some(window)
}

fn is_standing(window: WindowId, event: &Event) -> Option<WindowId> {
    matches!(event, Event::WorkDefaults(_)).then_some(window)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_hears_its_own_and_keeps_going_when_another_leaves() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, item_id) = engine(dir.path());
    let (a, a_events) = Backend::in_process(&engine);
    let (b, b_events) = Backend::in_process(&engine);
    let a_window = a.open_window();
    let b_window = b.open_window();
    assert_eq!((a_window.window(), b_window.window()), (1, 1));

    // Each window's answer reaches its own client.
    a_window.rename_item("missing", None);
    assert_eq!(wait(&a_events, is_gone_notice), 1);
    never(&b_events, is_gone_notice);

    // A resync is the asker's alone.
    b.resync();
    assert_eq!(wait(&b_events, is_standing), NO_WINDOW);
    never(&a_events, is_standing);

    // A rename is news to both: its window's, and every other client's.
    a_window.rename_item(&item_id, Some("after".into()));
    let renamed = |window: WindowId, event: &Event| match event {
        Event::Items(items) if items[0].name.as_deref() == Some("after") => Some(window),
        _ => None,
    };
    assert_eq!(wait(&a_events, renamed), 1);
    assert_eq!(wait(&b_events, renamed), NO_WINDOW);

    // a goes; its window closes in the engine, which goes on for b.
    drop((a, a_window));
    drop(a_events);
    while engine.connections() > 1 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    b_window.rename_item("missing", None);
    assert_eq!(wait(&b_events, is_gone_notice), 1);
    b.resync();
    assert_eq!(wait(&b_events, is_standing), NO_WINDOW);
}

/// Read one line off the client's end.
async fn line<R: tokio::io::AsyncBufRead + Unpin>(read: &mut R) -> Option<String> {
    let mut line = String::new();
    (read.read_line(&mut line).await.unwrap() > 0).then_some(line)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_on_another_protocol_is_told_both_versions() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, _) = engine(dir.path());
    let (near, far) = tokio::io::duplex(1 << 16);
    let served = tokio::spawn(wire::serve(engine.clone(), far));
    let (read, mut write) = tokio::io::split(near);
    let mut read = BufReader::new(read);
    let hello = Hello {
        protocol: PROTOCOL + 1,
        app_version: "99.0.0".into(),
        intent: wire::Intent::Connect,
    };
    let mut first = serde_json::to_vec(&hello).unwrap();
    first.push(b'\n');
    write.write_all(&first).await.unwrap();
    let answer: Answer = serde_json::from_str(&line(&mut read).await.unwrap()).unwrap();
    let Answer::Refused { reason, about } = answer else {
        panic!("let in: {answer:?}");
    };
    // The refusal says who the engine is, for an app that would replace it.
    assert_eq!(about.engine_version, VERSION);
    assert_eq!(about.protocol, PROTOCOL);
    assert_eq!(about.build, wire::build());
    for named in [
        "99.0.0".to_string(),
        VERSION.to_string(),
        format!("protocol {}", PROTOCOL + 1),
        format!("protocol {PROTOCOL}"),
        "Update the engine".to_string(),
    ] {
        assert!(reason.contains(&named), "{named} not in {reason}");
    }
    assert!(line(&mut read).await.is_none(), "the engine hung up");
    assert!(matches!(
        served.await.unwrap(),
        Err(wire::WireError::Refused(..))
    ));
    assert_eq!(engine.connections(), 0);

    // And `connect` says so.
    let (near, far) = tokio::io::duplex(1 << 16);
    tokio::spawn(async move {
        let (read, mut write) = tokio::io::split(far);
        let _ = line(&mut BufReader::new(read)).await;
        let refused = serde_json::to_string(&Answer::Refused {
            reason: "no".into(),
            about: wire::About::default(),
        })
        .unwrap();
        write
            .write_all(format!("{refused}\n").as_bytes())
            .await
            .unwrap();
    });
    assert!(matches!(
        wire::connect(near).await,
        Err(wire::WireError::Refused(reason, _)) if reason == "no"
    ));
}

/// A client's end of `serve` on a pipe, let in.
async fn raw_client(
    engine: &Engine,
    limits: wire::Limits,
) -> (
    BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    tokio::io::WriteHalf<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), wire::WireError>>,
) {
    let (near, far) = tokio::io::duplex(1 << 12);
    let served = tokio::spawn(wire::serve_with(engine.clone(), far, limits));
    let (read, mut write) = tokio::io::split(near);
    let mut read = BufReader::new(read);
    let hello = serde_json::to_string(&Hello::connect()).unwrap();
    write
        .write_all(format!("{hello}\n").as_bytes())
        .await
        .unwrap();
    let answer: Answer = serde_json::from_str(&line(&mut read).await.unwrap()).unwrap();
    assert!(matches!(answer, Answer::Welcome(_)), "{answer:?}");
    (read, write, served)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_line_is_dropped_and_the_next_still_runs() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, _) = engine(dir.path());
    let (mut read, mut write, served) = raw_client(&engine, wire::Limits::default()).await;
    write
        .write_all(b"{\"window\":0,\"command\":\"NoSuchThing\"}\nnot json at all\n")
        .await
        .unwrap();
    let resync = serde_json::to_string(&wire::CommandLine {
        window: NO_WINDOW,
        command: Command::Resync,
    })
    .unwrap();
    write
        .write_all(format!("{resync}\n").as_bytes())
        .await
        .unwrap();
    // The resync is answered: the connection outlived the lines before it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let next = tokio::time::timeout_at(deadline, line(&mut read))
            .await
            .expect("the resync never came")
            .expect("the engine hung up");
        if next.contains("\"WorkDefaults\"") {
            break;
        }
    }
    assert_eq!(engine.connections(), 1);
    write.shutdown().await.unwrap();
    while line(&mut read).await.is_some() {}
    assert!(served.await.unwrap().is_ok());
    assert_eq!(engine.connections(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_stops_reading_is_hung_up_on_once_it_is_too_far_behind() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, _) = engine(dir.path());
    let (other, other_events) = Backend::in_process(&engine);
    let limits = wire::Limits {
        backlog: 64 << 10,
        last_writes: Duration::from_millis(100),
        ..wire::Limits::default()
    };
    // Never read again: the pipe fills, and then the backlog.
    let (_read, _write, served) = raw_client(&engine, limits).await;
    let big = "x".repeat(4 << 10);
    for _ in 0..64 {
        engine.hub.route(NO_WINDOW, Event::Notice(big.clone()));
    }
    let ended = tokio::time::timeout(Duration::from_secs(10), served)
        .await
        .expect("the engine kept waiting on the client")
        .unwrap();
    assert!(matches!(ended, Err(wire::WireError::Behind)), "{ended:?}");
    assert_eq!(engine.connections(), 1);

    // The engine and its other client go on.
    other.open_window().rename_item("missing", None);
    assert_eq!(wait(&other_events, is_gone_notice), 1);
}

/// An engine with no command loop: enough for what the wire itself does.
fn bare_engine() -> (Engine, UnboundedReceiver<Inbound>) {
    let (tx, commands) = unbounded_channel();
    let engine = Engine {
        hub: Arc::new(Hub::new(tx, Lifetime::Service, Fencing::default())),
        runtime: tokio::runtime::Handle::current(),
    };
    (engine, commands)
}

#[tokio::test]
async fn a_client_is_kept_while_it_keeps_alive_and_hung_up_on_once_silent() {
    let (engine, _commands) = bare_engine();
    let limits = wire::Limits {
        idle: Duration::from_millis(400),
        ..wire::Limits::default()
    };
    let (_read, mut write, served) = raw_client(&engine, limits).await;
    // Keepalives alone hold the connection, for far longer than the wait.
    for _ in 0..12 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        write.write_all(b"\n").await.unwrap();
    }
    assert!(!served.is_finished());
    assert_eq!(engine.connections(), 1);

    // Then nothing: asleep, or the network gone with the stream still
    // open. The engine stops waiting, and the client's windows close.
    let silent_from = tokio::time::Instant::now();
    let ended = tokio::time::timeout(Duration::from_secs(10), served)
        .await
        .expect("the engine kept a silent client")
        .unwrap();
    assert!(matches!(ended, Err(wire::WireError::Silent)), "{ended:?}");
    assert!(silent_from.elapsed() >= Duration::from_millis(300));
    assert_eq!(engine.connections(), 0);
}

#[tokio::test]
async fn a_client_that_never_says_hello_is_hung_up_on() {
    let (engine, _commands) = bare_engine();
    let limits = wire::Limits {
        idle: Duration::from_millis(300),
        ..wire::Limits::default()
    };
    let (_near, far) = tokio::io::duplex(1 << 12);
    let ended = tokio::time::timeout(
        Duration::from_secs(10),
        wire::serve_with(engine.clone(), far, limits),
    )
    .await
    .expect("the engine waited on the hello for ever");
    assert!(matches!(ended, Err(wire::WireError::Silent)), "{ended:?}");
    assert_eq!(engine.connections(), 0);
}

#[tokio::test]
async fn a_quiet_client_says_it_is_there_every_little_while() {
    let (near, far) = tokio::io::duplex(1 << 16);
    let (read, mut write) = tokio::io::split(far);
    let mut read = BufReader::new(read);
    let engine_side = async {
        let hello = line(&mut read).await.unwrap();
        assert!(hello.contains("\"protocol\""), "{hello}");
        let welcome = serde_json::to_string(&Answer::Welcome(wire::Welcome {
            protocol: PROTOCOL,
            engine_version: VERSION.into(),
            connection: 1,
            host: "halo".into(),
            home: "/home/chris".into(),
            data_root: "/home/chris/.perch".into(),
            clients: 0,
            runs: 0,
            build: VERSION.into(),
        }))
        .unwrap();
        write
            .write_all(format!("{welcome}\n").as_bytes())
            .await
            .unwrap();
    };
    let (shaken, ()) = tokio::join!(wire::handshake(near), engine_side);
    let (welcome, mut session) = shaken.unwrap();
    session.keepalive = Duration::from_millis(50);
    let (commands, outgoing) = unbounded_channel();
    let (backend, navigations) = Backend::over_wire(commands, welcome.connection);
    let (events, _received) = std::sync::mpsc::channel();
    let _ending = session.carry(outgoing, navigations, events);
    let since = tokio::time::Instant::now();
    for _ in 0..3 {
        assert_eq!(line(&mut read).await.as_deref(), Some("\n"));
    }
    assert!(since.elapsed() >= Duration::from_millis(150));
    // A command is said in its place, and the keepalives go on after it.
    backend.resync();
    assert!(line(&mut read).await.unwrap().contains("Resync"));
    assert_eq!(line(&mut read).await.as_deref(), Some("\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_commands_go_through_the_json_wire() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, item_id) = engine(dir.path());
    let (near, far) = tokio::io::duplex(1 << 16);
    tokio::spawn(wire::serve(engine.clone(), far));
    let (backend, events, welcome) = wire::connect(near).await.unwrap();
    assert_eq!(welcome.protocol, PROTOCOL);
    assert_eq!(welcome.engine_version, VERSION);

    backend.resync();
    let items = wait(&events, |window, event| match event {
        Event::Items(items) if window == NO_WINDOW => Some(items.clone()),
        _ => None,
    });
    assert_eq!(items[0].id, item_id);

    let window = backend.open_window();
    window.rename_item(&item_id, Some("over the wire".into()));
    let named = wait(&events, |window, event| match event {
        Event::Items(items) if items[0].name.as_deref() == Some("over the wire") => Some(window),
        _ => None,
    });
    assert_eq!(named, 1);
    window.rename_item("missing", None);
    assert_eq!(wait(&events, is_gone_notice), 1);

    // Hanging up closes the client's window and leaves the engine running.
    drop((backend, window));
    while engine.connections() > 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resync_gives_a_late_client_what_one_there_all_along_has() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, _) = engine_with(dir.path(), |ctx| {
        // A look at who acted last happened before the client came.
        ctx.participants.lock().unwrap().insert(
            ("a/b".into(), 7),
            vec![perch_github::Participant {
                login: "octocat".into(),
                avatar_url: String::new(),
            }],
        );
        ctx.with_cache(|cache| cache.set_meta("drawer.width", "300"))
            .unwrap()
            .unwrap();
    });
    let (backend, events) = Backend::in_process(&engine);
    backend.resync();
    let mut meta = std::collections::BTreeMap::new();
    let mut whole_runs = None;
    let participants = wait(&events, |_, event| {
        match event {
            Event::Meta { key, value } => {
                meta.insert(key.clone(), value.clone());
            }
            Event::AgentRuns { whole, .. } => whole_runs = Some(*whole),
            Event::Participants(found) => return Some(found.clone()),
            _ => {}
        }
        None
    });
    assert_eq!(participants[0].0, ("a/b".to_string(), 7));
    // The runs replace what the client has, and every kept setting is
    // said, one never kept as none, so a reconnecting client's goes back.
    assert_eq!(whole_runs, Some(true));
    assert_eq!(meta.len(), UI_META_KEYS.len());
    assert_eq!(meta["drawer.width"].as_deref(), Some("300"));
    assert_eq!(meta["review.layout"], None);
}

#[test]
fn a_rescope_says_which_scope_pick_the_engine_had_heard_of() {
    let (tx, _commands) = unbounded_channel();
    let fencing = Fencing::default();
    let engine = Engine {
        hub: Arc::new(Hub::new(tx, Lifetime::Service, fencing.clone())),
        runtime: tokio::runtime::Runtime::new().unwrap().handle().clone(),
    };
    let (backend, _events) = Backend::in_process(&engine);
    let window = backend.open_window();
    window.open_item("a");
    let scopes = view(&fencing, 2).scope_requests;
    assert_eq!(scopes.heard(), 0);
    window.set_scope("a", perch_git::Scope::Branch);
    let picked = scopes.heard();
    assert!(picked > 0);
    // A look at the checkout carries the latest pick, and moves nothing.
    window.refresh_scope("a");
    assert_eq!(scopes.heard(), picked);
    window.set_scope("a", perch_git::Scope::Uncommitted);
    assert!(scopes.heard() > picked);
}

#[test]
fn loaded_file_travels_as_raw_inputs_and_rebuilds_whole() {
    let cases = [
        build_loaded_file(
            "src/lib.rs".into(),
            "fn a() {}\nfn b() {}\n\n".into(),
            "fn a() {}\nfn old() {}\n".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            "blob-mod".into(),
        ),
        build_loaded_file(
            "src/new.rs".into(),
            "pub const X: usize = 1;\n".into(),
            String::new(),
            Some(perch_git::ChangeStatus::Added),
            false,
            perch_git::Scope::Branch,
            "blob-add".into(),
        ),
        build_loaded_file(
            "src/gone.rs".into(),
            String::new(),
            "fn gone() {}\n".into(),
            Some(perch_git::ChangeStatus::Deleted),
            false,
            perch_git::Scope::Branch,
            String::new(),
        ),
        build_loaded_file(
            "src/plain.rs".into(),
            "fn plain() {}\n\n".into(),
            String::new(),
            None,
            false,
            perch_git::Scope::Branch,
            String::new(),
        ),
        build_loaded_file(
            "assets/logo.png".into(),
            String::new(),
            String::new(),
            Some(perch_git::ChangeStatus::Modified),
            true,
            perch_git::Scope::Branch,
            "blob-bin".into(),
        ),
        crate::review::model::LoadedFile::truncated(
            "/outside/big.rs".into(),
            "This file is too large to show.",
            None,
            perch_git::Scope::Branch,
            String::new(),
        ),
        build_loaded_file(
            "diagram.svg".into(),
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"5\"/></svg>".into(),
            "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            perch_git::Scope::Branch,
            "blob-svg".into(),
        ),
    ];

    for original in cases {
        let json = serde_json::to_value(&original).unwrap();
        let object = json.as_object().expect("LoadedFile serializes as object");
        for derived in [
            "buffer",
            "base_buffer",
            "head_spans",
            "base_spans",
            "alignment",
            "base_to_display",
            "display_to_base",
            "picture",
            "language",
            "origin",
        ] {
            assert!(
                !object.contains_key(derived),
                "derived field {derived} should not cross the wire for {}",
                original.path
            );
        }

        let back: crate::review::model::LoadedFile = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), json);
        assert_eq!(back.path, original.path);
        assert_eq!(back.origin, original.origin);
        assert_eq!(back.language, original.language);
        assert_eq!(back.status, original.status);
        assert_eq!(back.truncated, original.truncated);
        assert_eq!(back.scope, original.scope);
        assert_eq!(back.blob, original.blob);
        assert_eq!(back.buffer.text, original.buffer.text);
        assert_eq!(back.buffer.rows, original.buffer.rows);
        assert_eq!(back.buffer.change_starts, original.buffer.change_starts);
        assert_eq!(back.base_text, original.base_text);
        assert_eq!(
            back.base_buffer.as_ref().map(|b| (&b.text, &b.rows)),
            original.base_buffer.as_ref().map(|b| (&b.text, &b.rows))
        );
        assert_eq!(back.alignment, original.alignment);
        assert_eq!(back.base_to_display, original.base_to_display);
        assert_eq!(back.display_to_base, original.display_to_base);
        assert_eq!(back.head_spans.len(), original.head_spans.len());
        for i in 0..original.head_spans.len() {
            assert_eq!(back.head_spans.line(i), original.head_spans.line(i));
        }
        assert_eq!(back.base_spans.len(), original.base_spans.len());
        for i in 0..original.base_spans.len() {
            assert_eq!(back.base_spans.line(i), original.base_spans.line(i));
        }
        assert_eq!(back.picture.is_some(), original.picture.is_some());
    }
}
