//! The wire-ready boundary: `Command` and `Event` as JSON, the connection
//! layer's window mapping and routing, the handshake, and real commands
//! through the wire to an engine that runs its command loop on a test
//! context (never `run`, which reads the reader's own config and token).

use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use super::connections::{Hub, Inbound};
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
            Inbound::Command(window, command) => (window, format!("{command:?}")),
            Inbound::Shutdown => (NO_WINDOW, "Shutdown".into()),
        })
        .collect()
}

#[test]
fn two_connections_keep_their_own_windows() {
    let (tx, mut commands) = unbounded_channel();
    let hub = Arc::new(Hub::new(tx, Lifetime::Service));
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
    a.send(1, Command::Resync).unwrap();
    b.send(NO_WINDOW, Command::Resync).unwrap();
    // A window that never opened sends nothing.
    a.send(7, Command::Resync).unwrap();
    assert_eq!(
        ran(&mut commands),
        vec![
            (3, "WindowOpened".into()),
            (4, "WindowOpened".into()),
            (3, "Resync".into()),
            (2, "Resync".into()),
        ]
    );

    // A window's own answer goes to its owner, as the owner's window.
    hub.route(3, notice("a's"));
    hub.route(4, notice("b's"));
    // A home's goes to its connection alone.
    hub.route(2, notice("b's home"));
    // What every window shows goes to all, the sender's as its window.
    hub.route(3, Event::Items(Vec::new()));
    hub.route(NO_WINDOW, notice("everyone"));
    assert_eq!(
        heard(&mut a_heard),
        vec![
            (1, "a's".into()),
            (1, "Items([])".into()),
            (NO_WINDOW, "everyone".into()),
        ]
    );
    assert_eq!(
        heard(&mut b_heard),
        vec![
            (1, "b's".into()),
            (NO_WINDOW, "b's home".into()),
            (NO_WINDOW, "Items([])".into()),
            (NO_WINDOW, "everyone".into()),
        ]
    );

    // Going closes a's window in the engine; b and the engine go on.
    drop(a);
    assert_eq!(ran(&mut commands), vec![(3, "WindowClosed".into())]);
    assert_eq!(engine.connections(), 1);
    hub.route(3, notice("late"));
    hub.route(NO_WINDOW, notice("still here"));
    assert_eq!(heard(&mut b_heard), vec![(NO_WINDOW, "still here".into())]);
    b.send(1, Command::WindowClosed).unwrap();
    b.send(1, Command::Resync).unwrap();
    drop(b);
    // A service engine outlives its clients.
    assert_eq!(ran(&mut commands), vec![(4, "WindowClosed".into())]);
}

#[test]
fn the_app_s_engine_stops_with_its_last_connection() {
    let (tx, mut commands) = unbounded_channel();
    let engine = Engine {
        hub: Arc::new(Hub::new(tx, Lifetime::WhileConnected)),
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
    let (mut ctx, _untagged) = ctx(dir, &nowhere(), &[]);
    let item = pr_item();
    ctx.with_cache(|cache| cache.insert_item(&item))
        .unwrap()
        .unwrap();
    let (tx, commands) = unbounded_channel();
    let hub = Arc::new(Hub::new(tx, Lifetime::Service));
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
    };
    let mut first = serde_json::to_vec(&hello).unwrap();
    first.push(b'\n');
    write.write_all(&first).await.unwrap();
    let answer: Answer = serde_json::from_str(&line(&mut read).await.unwrap()).unwrap();
    let Answer::Refused { reason } = answer else {
        panic!("let in: {answer:?}");
    };
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
        Err(wire::WireError::Refused(_))
    ));
    assert_eq!(engine.connections(), 0);

    // And `connect` says so.
    let (near, far) = tokio::io::duplex(1 << 16);
    tokio::spawn(async move {
        let (read, mut write) = tokio::io::split(far);
        let _ = line(&mut BufReader::new(read)).await;
        let refused = serde_json::to_string(&Answer::Refused {
            reason: "no".into(),
        })
        .unwrap();
        write
            .write_all(format!("{refused}\n").as_bytes())
            .await
            .unwrap();
    });
    assert!(matches!(
        wire::connect(near).await,
        Err(wire::WireError::Refused(reason)) if reason == "no"
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_line_closes_only_its_connection() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, _) = engine(dir.path());
    let (other, other_events) = Backend::in_process(&engine);

    let (near, far) = tokio::io::duplex(1 << 16);
    let served = tokio::spawn(wire::serve(engine.clone(), far));
    let (read, mut write) = tokio::io::split(near);
    let mut read = BufReader::new(read);
    let hello = serde_json::to_string(&Hello {
        protocol: PROTOCOL,
        app_version: VERSION.into(),
    })
    .unwrap();
    write
        .write_all(format!("{hello}\n").as_bytes())
        .await
        .unwrap();
    let answer: Answer = serde_json::from_str(&line(&mut read).await.unwrap()).unwrap();
    assert!(matches!(answer, Answer::Welcome(_)), "{answer:?}");
    write
        .write_all(b"{\"window\":1,\"command\":\"NoSuchThing\"}\n")
        .await
        .unwrap();
    assert!(matches!(
        served.await.unwrap(),
        Err(wire::WireError::Malformed(_))
    ));
    // The rest of what it was sent, then the end.
    while line(&mut read).await.is_some() {}
    assert_eq!(engine.connections(), 1);

    // The engine and its other client go on.
    other.open_window().rename_item("missing", None);
    assert_eq!(wait(&other_events, is_gone_notice), 1);
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
