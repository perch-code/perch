//! Perch's windows. Cmd-Shift-N, New Window in the Perch menu, opens
//! another, each with an item of its own open; Cmd-W closes one. They
//! share a backend and its broadcasts with other windows in the same
//! environment. Window-specific replies use the ID assigned by that backend.

mod hosts;
mod session;
use hosts::Hosts;
use session::{SavedWindow, Session};

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use floem::action::TimerToken;
use floem::ext_event::update_signal_from_channel;
use floem::kurbo::{Point, Rect, Size, Vec2};
use floem::prelude::*;
use floem::reactive::{Effect, RwSignal, Scope, SignalGet, SignalUpdate};
use floem::views::Stack;
use floem::window::WindowConfig;

use crate::backend::{Backend, Event, WindowId};
use crate::review::model::ReviewState;
use crate::state::AppState;

#[derive(Default)]
struct App {
    session: Option<Rc<Session>>,
    /// The SSH hosts windows have connected to, for the environment menu.
    hosts: Option<Hosts>,
    environments: RefCell<std::collections::HashMap<Option<String>, Rc<Environment>>>,
    windows: RefCell<Vec<Window>>,
    /// The window that last had the keyboard, for New Window to take its
    /// layout.
    focused: Cell<Option<floem::ViewId>>,
    /// Open… was asked for with no window open: the next one opens with
    /// the box up.
    open_box_on_open: Cell<bool>,
    /// Send Feedback… was asked for with no window open, the same way.
    feedback_on_open: Cell<bool>,
    /// Check for Updates… was, the same way: the next window asks.
    check_update_on_open: Cell<bool>,
    /// Settings… was, the same way: the next window opens on Settings.
    settings_on_open: Cell<bool>,
}

struct Environment {
    scope: Scope,
    pictures: Rc<crate::review::images::Store>,
    profiles: Rc<crate::ui::profile_card::Profiles>,
    host: Option<String>,
    /// The handle windows are opened from. It sends nothing itself.
    backend: Backend,
    /// What the backend has said, heard as a window would. The backend
    /// sends most of it once, at launch, and a new window starts from it.
    model: AppState,
    /// The engine's Settings as it last sent them, before this machine's
    /// tables were put in: with the engine on a host, they are put in
    /// again after each write of the client's own.
    engine_settings: RefCell<Option<crate::backend::SettingsFile>>,
    /// Whether this machine's config.toml did not parse when last read,
    /// with the engine on a host: the client's rows go by it, not by
    /// whether the host's file parses.
    client_file_broken: Cell<bool>,
}

#[derive(Clone)]
struct Window {
    id: WindowId,
    environment: Rc<Environment>,
    destination: RwSignal<Option<String>>,
    frame: Rc<Cell<Rect>>,
    close_requested: Rc<Cell<bool>>,
    state: AppState,
    review: ReviewState,
    /// Brings the window to the front.
    raise: floem::ViewId,
    /// What the window asks the backend with, for a link followed in it.
    backend: Rc<Backend>,
}

impl Window {
    fn session_item(&self) -> Option<String> {
        self.state
            .open_item
            .get()
            .or_else(|| self.review.item.with(|i| i.as_ref().map(|i| i.id.clone())))
            .or_else(|| self.state.pending_restore.get())
    }
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
    /// Where the last window moved or was resized to, for the next one
    /// opened with none open to go. Read from `FRAME_FILE` at launch.
    static LEFT_AT: Cell<Option<Rect>> = const { Cell::new(None) };
}

/// The file in this machine's Perch home a window's frame is kept in:
/// `x,y,width,height`, in points, the top-left from the top of the main
/// screen, as floem places windows. It is the client's, wherever the
/// engine is: an engine on a host has its own database, which knows
/// nothing of this machine's screens.
const FRAME_FILE: &str = "window-frame";
/// The meta key the frame was kept under in the database before
/// `FRAME_FILE`, read when there is no file so that nobody's window
/// moves after an upgrade.
const FRAME_KEY: &str = "window.frame";
const DEFAULT_SIZE: Size = Size::new(1280.0, 820.0);
const MIN_SIZE: Size = Size::new(900.0, 600.0);
/// How long a window has to sit still before where it is gets written: a
/// drag moves it on every frame.
const SAVE_AFTER: Duration = Duration::from_millis(500);
/// How far a cascaded window sits down and right of the key window.
const STEP: f64 = 28.0;
/// The strip along a window's top, which is what it is dragged by.
const DRAG_STRIP: f64 = 40.0;

/// Read where the last window was left. Before the backend starts, so
/// that it is not opening the database, a migration say, at the time.
pub fn load_frame() {
    LEFT_AT.set(read_frame(&perch_github::config::perch_home()));
}

/// The frame kept in `home`: the file, else the old meta key in this
/// machine's database, if it has one.
fn read_frame(home: &std::path::Path) -> Option<Rect> {
    if let Ok(value) = std::fs::read_to_string(home.join(FRAME_FILE)) {
        return parse_frame(value.trim());
    }
    perch_github::Cache::peek_meta(&home.join("perch.db"), FRAME_KEY)
        .and_then(|value| parse_frame(&value))
}

/// Keep `frame` in `home`, whole or not at all: written beside the file
/// and moved over it.
fn write_frame(home: &std::path::Path, frame: Rect) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    let path = home.join(FRAME_FILE);
    let partial = path.with_extension("partial");
    std::fs::write(&partial, format_frame(frame))?;
    std::fs::rename(&partial, &path)
}

fn parse_frame(value: &str) -> Option<Rect> {
    let numbers = value
        .split(',')
        .map(|n| n.trim().parse::<f64>().ok().filter(|n| n.is_finite()))
        .collect::<Option<Vec<_>>>()?;
    let [x, y, width, height] = numbers[..] else {
        return None;
    };
    (width > 0.0 && height > 0.0).then(|| Rect::new(x, y, x + width, y + height))
}

fn format_frame(frame: Rect) -> String {
    format!(
        "{},{},{},{}",
        frame.x0.round(),
        frame.y0.round(),
        frame.width().round(),
        frame.height().round()
    )
}

fn app() -> Option<Rc<App>> {
    APP.with_borrow(Clone::clone)
}

/// Floem creates initial windows by popping its queue. Enqueue the
/// frontmost window first so that it opens last and takes focus.
pub fn restore(
    mut application: floem::Application,
    legacy_host: Option<&str>,
) -> floem::Application {
    let session = Rc::new(Session::load(
        &perch_github::config::perch_home(),
        legacy_host,
    ));
    let saved = session.last.borrow().clone();
    let count = saved.len();
    for (index, saved) in saved.into_iter().enumerate().rev() {
        let (config, frame) = config(Some(&saved));
        let session = session.clone();
        application = application.window(
            move |_| {
                if app().is_none() {
                    crate::app_icon::install();
                    crate::app_menu::install();
                    start(session.clone());
                }
                let root = view(None, frame, index + 1 == count, Some(saved.clone())).into_any();
                session.opening.set(session.opening.get().saturating_sub(1));
                save_windows();
                root
            },
            Some(config),
        );
    }
    application
}

fn save_windows() {
    let Some(app) = app() else { return };
    let Some(session) = &app.session else { return };
    let mut windows = app.windows.borrow().clone();
    windows.sort_by_key(|w| Some(w.raise) == app.focused.get());
    let snapshot = windows
        .iter()
        .map(|w| SavedWindow {
            host: w.environment.host.clone(),
            item: Effect::untrack(|| w.session_item()),
            frame: Some(format_frame(w.frame.get())),
        })
        .collect();
    session.save(snapshot);
}

pub fn quit_app() {
    quitting();
    floem::quit_app();
}

pub fn quitting() {
    save_windows();
    if let Some(app) = app() {
        if let Some(session) = &app.session {
            session.quitting.set(true);
        }
    }
}

fn start(session: Rc<Session>) {
    Scope::new().enter(|| {
        // A mouse's back and forward buttons as Logitech's Options+ sends
        // them, as swipes. They go to the window that has the keyboard,
        // which is the one under the pointer as a rule.
        let swiped = floem::ext_event::create_trigger();
        crate::swipes::watch(swiped);
        Effect::new(move |_| {
            swiped.track();
            Effect::untrack(|| {
                for back in crate::swipes::take() {
                    travel_in_focused(back);
                }
            });
        });
        floem::action::exec_after(STAR_ASK_AFTER, |_| star_due());
        watch_client_file(None);
        APP.set(Some(Rc::new(App {
            session: Some(session),
            hosts: Some(Hosts::load(&perch_github::config::perch_home())),
            ..Default::default()
        })));
    });
}

fn environment(host: Option<String>) -> Rc<Environment> {
    let app = app().expect("window registry");
    if let Some(existing) = app.environments.borrow().get(&host) {
        return existing.clone();
    }
    let mut remote = perch_github::Config::load().remote;
    if remote.host().is_some() && host.as_deref() != remote.host() {
        remote.command = None;
    }
    remote.host = host.clone();
    if host.is_some() {
        static AWAKE: std::sync::Once = std::sync::Once::new();
        AWAKE.call_once(crate::remote::stay_awake_for_the_host);
    }
    let (backend, events) = crate::start_engine(&remote);
    register_environment(&app, host, backend, events)
}

fn register_environment(
    app: &App,
    host: Option<String>,
    backend: Backend,
    events: std::sync::mpsc::Receiver<(WindowId, Event)>,
) -> Rc<Environment> {
    let scope = Scope::new();
    scope.enter(|| {
        let model = new_state();
        floem::reactive::Context::provide(model);
        if let Some(host) = &host {
            model.place.set(crate::remote::Place::Host {
                host: host.clone(),
                home: Default::default(),
                data_root: Default::default(),
            });
            model.link.set(Some(crate::backend::EngineLink::Connecting {
                host: host.clone(),
            }));
        }
        let pictures = Rc::new(crate::review::images::Store::new(scope));
        let profiles = Rc::new(crate::ui::profile_card::Profiles::default());
        scope.provide_context(pictures.clone());
        scope.provide_context(profiles.clone());
        let env = Rc::new(Environment {
            pictures,
            profiles,
            scope,
            backend,
            model,
            host: host.clone(),
            engine_settings: RefCell::new(None),
            client_file_broken: Cell::new(false),
        });
        app.environments.borrow_mut().insert(host, env.clone());
        if env.host.is_some() {
            refresh_client_settings(&env);
        }
        let incoming = RwSignal::new(None::<(WindowId, Event)>);
        update_signal_from_channel(incoming.write_only(), events);
        let weak = Rc::downgrade(&env);
        Effect::new(move |_| {
            if let (Some(env), Some((window, event))) = (weak.upgrade(), incoming.get()) {
                Effect::untrack(|| env.scope.enter(|| deliver(&env, window, event)));
            }
        });
        env
    })
}

fn for_state(state: AppState) -> Option<Rc<Environment>> {
    let host = state.place.try_get_untracked()?.host();
    app()?
        .environments
        .borrow()
        .get(&host)
        .filter(|env| env.model.place == state.place)
        .cloned()
}

/// Back, or forward, in the window that last had the keyboard.
fn travel_in_focused(back: bool) {
    let Some(app) = app() else {
        return;
    };
    let focused = app.focused.get();
    let to = app
        .windows
        .borrow()
        .iter()
        .find(|w| Some(w.raise) == focused)
        .cloned();
    if let Some(to) = to {
        crate::ui::history::travel(to.state, to.review, &to.backend, back);
    }
}

fn new_state() -> AppState {
    let state = AppState::new();
    let mut review = ReviewState::new();
    review.agent = state.agent;
    state.review.set(Some(review));
    state
}

fn deliver(app: &Environment, window: WindowId, mut event: Event) {
    // With the engine on a host, its Settings have this machine's own
    // tables put in, which the client reads and writes here.
    if let Event::Settings(file) = &mut event {
        if app.host.is_some() {
            // A host file that doesn't parse leaves the engine on what it
            // read last, so the page keeps showing that.
            if file.config.is_none() {
                file.config = app
                    .engine_settings
                    .borrow()
                    .as_ref()
                    .and_then(|last| last.config.clone());
            }
            app.engine_settings.replace(Some(file.clone()));
            let local = crate::client_settings::Local::read_kept();
            app.client_file_broken.set(local.error.is_some());
            *file = crate::client_settings::merge(file.clone(), &local);
        }
    }
    let local_settings = app.host.is_none() && matches!(event, Event::Settings(_));
    deliver_as_is(app, window, event);
    if local_settings {
        client_settings_written();
    }
}

fn environment_windows(env: &Environment) -> Vec<Window> {
    app()
        .map(|app| {
            app.windows
                .borrow()
                .iter()
                .filter(|w| std::ptr::eq(w.environment.as_ref(), env))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Hand `event` to the windows as it is.
fn deliver_as_is(app: &Environment, window: WindowId, event: Event) {
    if let Event::Link(link) = &event {
        linked(app, link);
    }
    // `[ui] appearance` and `[keybindings]` are the whole app's, so they
    // are taken up once here rather than by each window's state.
    if let Event::Settings(file) = &event {
        if let Some(config) = &file.config {
            crate::theme::choose(config.ui.appearance);
            crate::keymap::take_up(&config.keybindings);
        }
    }
    // A copy, so a window closing while the event is applied does not find
    // the list borrowed.
    let windows = environment_windows(app);
    // The worker's own, a poll's complaint say, is everybody's.
    if window != crate::backend::NO_WINDOW && event.for_one_window() {
        if let Some(to) = windows.iter().find(|w| w.id == window) {
            to.state.apply_with_review(event, to.review);
        }
        return;
    }
    for to in &windows {
        to.state.apply_with_review(event.clone(), to.review);
    }
    if let Some(review) = app.model.review.get_untracked() {
        app.model.apply_with_review(event, review);
    }
}

/// Tell every window `text`: for what the app did itself, away from the
/// engine, such as opening an editor on the engine's host.
pub fn notice(text: String) {
    if let Some(state) = focused_state() {
        state.complain(text);
    }
}

/// The client's own settings were written to this machine's file: every
/// window is shown the engine's Settings with them put in again. Before
/// the engine on a host has sent its own, this machine's alone, so the
/// host can be put right when it cannot be reached.
pub fn client_settings_written() {
    if let Some(app) = app() {
        let remote: Vec<_> = app
            .environments
            .borrow()
            .values()
            .filter(|env| env.host.is_some())
            .cloned()
            .collect();
        for env in remote {
            refresh_client_settings(&env);
        }
    }
}

fn refresh_client_settings(app: &Environment) {
    let engine = app.engine_settings.borrow().clone();
    match engine {
        Some(engine) => deliver(app, crate::backend::NO_WINDOW, Event::Settings(engine)),
        None => {
            let Some(path) = perch_github::config::config_path() else {
                return;
            };
            let local = crate::client_settings::Local::read_kept();
            app.client_file_broken.set(local.error.is_some());
            let file = crate::client_settings::alone(path, &local);
            deliver_as_is(app, crate::backend::NO_WINDOW, Event::Settings(file));
        }
    }
}

/// Whether the engine's settings are known: always in process, and with
/// the engine on a host once it has sent them. Until then only the
/// client's own can be changed.
pub fn engine_settings_heard(state: AppState) -> bool {
    state.settings_file.with(|_| {
        for_state(state)
            .is_none_or(|env| env.host.is_none() || env.engine_settings.borrow().is_some())
    })
}

pub fn client_file_broken(state: AppState) -> bool {
    for_state(state).is_some_and(|env| env.client_file_broken.get())
}

/// How often this machine's config.toml is looked at for edits made
/// outside Perch, with the engine on a host: the engine watches its own.
const CLIENT_FILE_CHECK: Duration = Duration::from_secs(2);

/// Look at this machine's config.toml every [`CLIENT_FILE_CHECK`], and put
/// its tables in again when it has changed since `seen`.
fn watch_client_file(seen: Option<(Option<std::time::SystemTime>, u64)>) {
    floem::action::exec_after(CLIENT_FILE_CHECK, move |_| {
        let now = perch_github::config::config_path()
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|meta| (meta.modified().ok(), meta.len()));
        if now != seen {
            client_settings_written();
        }
        watch_client_file(now);
    });
}

/// How the connection to the engine on a host stands. Connected, its home
/// is what `~` means; connected again, or after a first try that failed,
/// each window opens its item again, as the engine has forgotten what the
/// windows had open, or never heard them ask. The engine has been told of
/// the windows and asked for everything by then.
fn linked(app: &Environment, link: &crate::backend::EngineLink) {
    let crate::backend::EngineLink::Up {
        host,
        welcome,
        again,
        ..
    } = link
    else {
        return;
    };
    app.model.place.update(|place| place.connected(welcome));
    if let (Some(host), Some(registry)) = (&app.host, self::app()) {
        if let Some(hosts) = &registry.hosts {
            hosts.remember(host);
        }
    }
    for window in environment_windows(app) {
        if !restore_item(&window) && *again {
            reopen_item(window.state, window.review, &window.backend, host);
        }
    }
    if *again {
        crate::review::images::ask_again();
    }
}

fn restore_item(window: &Window) -> bool {
    let Some(item) = window.state.pending_restore.get_untracked() else {
        return false;
    };
    let navigation = window.backend.open_item(&item);
    window
        .state
        .begin_item_review(window.review, "Reopening item", &item, navigation);
    true
}

/// Open again the item a window had open, or was opening. One showing is
/// read again under what shows, as an answer lands on an item opened
/// again while it shows, so nothing typed into it goes. One being opened
/// is the window's latest ask, and the one its progress card waits for.
/// Any other act the card waits for is said lost.
pub(crate) fn reopen_item(state: AppState, review: ReviewState, backend: &Backend, host: &str) {
    let showing = review
        .item
        .with_untracked(|item| item.as_ref().map(|item| item.id.clone()));
    let Some(item_id) = state.open_item.get_untracked().or_else(|| showing.clone()) else {
        state.drop_lost_open(host, true);
        return;
    };
    let navigation_id = backend.open_item(&item_id);
    if showing.as_deref() == Some(item_id.as_str()) {
        state.expect_item_navigation(&item_id, navigation_id);
        return;
    }
    state.begin_item_review(
        review,
        "Reconnected; reading the item again",
        &item_id,
        navigation_id,
    );
}

/// A window's state and backend handle, from inside its view function.
/// `settings` is the window it was opened from, whose layout it takes.
fn window(env: &Environment, settings: Option<AppState>) -> (AppState, ReviewState, Rc<Backend>) {
    let app = app().expect("window registry");
    let mut state = new_state();
    // Send Feedback's draft and the problems it reports are the app's,
    // not a window's.
    state.place = env.model.place;
    state.feedback = env.model.feedback;
    state.recent_problems = env.model.recent_problems;
    state.seed_from(&env.model, settings.as_ref().unwrap_or(&env.model));
    let review = state.review.get_untracked().unwrap();
    if app.open_box_on_open.replace(false) {
        crate::ui::open_box::show(state, None);
    }
    if app.settings_on_open.replace(false) {
        crate::ui::settings::show(state, None);
    }
    if app.feedback_on_open.replace(false) {
        crate::ui::feedback::open(state, review, crate::feedback::FeedbackKind::Idea, None);
    }
    let backend = Rc::new(env.backend.open_window());
    if app.check_update_on_open.replace(false) {
        backend.check_for_update();
    }
    (state, review, backend)
}

/// The window's whole view: the app's, and what makes it one of several.
/// `frame` is where it was opened, from `config`.
fn view(
    from: Option<AppState>,
    frame: Rect,
    run_launch_hooks: bool,
    saved: Option<SavedWindow>,
) -> impl IntoView {
    let raise = Raise::new();
    let raise_id = raise.id;
    let host = saved
        .as_ref()
        .map(|s| s.host.clone())
        .unwrap_or_else(|| from.and_then(|s| s.place.get_untracked().host()));
    let restore_item_id = saved.and_then(|s| s.item);
    let saved_frame = Rc::new(Cell::new(frame));
    let window_frame = saved_frame.clone();
    let close_requested = Rc::new(Cell::new(false));
    let window_close = close_requested.clone();
    let destination = RwSignal::new(host);
    let initial = Cell::new(true);
    let content = floem::views::dyn_container(
        move || destination.get(),
        move |host| {
            let initial = initial.replace(false);
            // A window that changes environment with Settings open stays
            // on Settings, on the section it showed.
            let settings = if initial {
                None
            } else {
                let section = open_settings_section(raise_id);
                detach(raise_id);
                section
            };
            let env = environment(host);
            let (state, review, backend) = window(&env, if initial { from } else { None });
            floem::reactive::Context::provide(state);
            floem::reactive::Context::provide(env.pictures.clone());
            floem::reactive::Context::provide(env.profiles.clone());
            if let Some(app) = app() {
                let window = Window {
                    frame: window_frame.clone(),
                    close_requested: window_close.clone(),
                    id: backend.window(),
                    state,
                    review,
                    raise: raise_id,
                    backend: backend.clone(),
                    environment: env,
                    destination,
                };
                app.windows.borrow_mut().push(window.clone());
                if initial {
                    state.pending_restore.set(restore_item_id.clone());
                    if run_launch_hooks {
                        crate::launch_hooks(state, review, backend.clone());
                    }
                }
                if window.environment.host.is_none()
                    || matches!(
                        state.link.get_untracked(),
                        Some(crate::backend::EngineLink::Up { .. })
                    )
                {
                    restore_item(&window);
                }
                let item = floem::reactive::Memo::new(move |_| window.session_item());
                Effect::new(move |_| {
                    item.track();
                    Effect::untrack(save_windows);
                });
            }
            if let Some(section) = settings {
                state.settings.section.set(section);
                crate::ui::settings::show(state, None);
            }
            crate::ui::app_view(state, review, backend).into_any()
        },
    )
    .style(|s| s.size_full());
    // Where the window is, kept as it moves and resizes, and written once
    // it has stopped.
    let frame = Rc::new(Cell::new(frame));
    let timer = Rc::new(Cell::new(None::<TimerToken>));
    let close_timer = timer.clone();
    let remember = {
        move |at: Rect| {
            if let Some(timer) = timer.take() {
                timer.cancel();
            }
            let saved_frame = saved_frame.clone();
            timer.set(Some(floem::action::exec_after(SAVE_AFTER, move |_| {
                // A full-screen window is the screen's size; the next one
                // should open at the size it came out of full screen to.
                if !can_save_frame(raise_id) {
                    return;
                }
                saved_frame.set(at);
                save_windows();
                LEFT_AT.set(Some(at));
                let home = perch_github::config::perch_home();
                if let Err(error) = write_frame(&home, at) {
                    tracing::warn!(%error, "could not keep where the window is");
                }
            })));
        }
    };
    let (resized, moved) = (frame.clone(), frame);
    let remember_size = remember.clone();
    Stack::new((content, raise.style(|s| s.absolute())))
        .style(|s| s.size_full())
        .window_title(move || {
            destination
                .get()
                .map_or_else(|| "Perch".into(), |h| format!("Perch — {h}"))
        })
        .window_scale(crate::theme::zoom)
        .on_event_cont(
            floem::event::listener::WindowResized,
            move |_, size: &Size| {
                let at = Rect::from_origin_size(resized.get().origin(), *size);
                resized.set(at);
                remember_size(at);
            },
        )
        .on_event_cont(floem::event::listener::WindowMoved, move |_, to: &Point| {
            let at = Rect::from_origin_size(*to, moved.get().size());
            moved.set(at);
            remember(at);
        })
        // Sent as the window opens and whenever macOS switches between light
        // and dark, which `[ui] appearance` can overrule.
        .on_event_cont(
            floem::event::listener::ThemeChanged,
            |_, theme: &floem::window::Theme| {
                crate::theme::system_appearance((*theme).into());
            },
        )
        .on_event_cont(floem::event::listener::WindowGainedFocus, move |_, _| {
            if let Some(app) = app() {
                app.focused.set(Some(raise_id));
                save_windows();
            }
        })
        .on_event_cont(floem::event::listener::WindowCloseRequested, {
            let requested = close_requested.clone();
            move |_, _| requested.set(true)
        })
        .on_event_cont(floem::event::listener::WindowClosed, move |_, _| {
            if let Some(timer) = close_timer.take() {
                timer.cancel();
            }
            // Its signals are still here, until the window's scope goes.
            if let Some(window) = raise_id.window_id() {
                crate::ui::menu::window_closed(window);
                crate::review::apply_suggestion::window_closed(window);
            }
            if close_requested.get() {
                save_windows();
            } else {
                // AppKit destroys windows one by one during Quit, without asking
                // them to close. Keep the snapshot from before the first one goes.
                quitting();
            }
            detach(raise_id);
            save_windows();
        })
}

pub fn close_window(id: floem::window::WindowId) {
    // Cmd-W directly closes the empty window, bypassing WindowCloseRequested.
    if let Some(app) = app() {
        if let Some(window) = app
            .windows
            .borrow()
            .iter()
            .find(|w| w.raise.window_id() == Some(id))
        {
            window.close_requested.set(true);
        }
    }
    floem::close_window(id);
}

fn detach(raise: floem::ViewId) {
    let Some(app) = app() else { return };
    let mut windows = app.windows.borrow_mut();
    let Some(index) = windows.iter().position(|w| w.raise == raise) else {
        return;
    };
    let window = windows.remove(index);
    drop(windows);
    window.state.settings.window_closed();
    window.backend.close_window();
    let env = window.environment;
    if env.host.is_some() && environment_windows(&env).is_empty() {
        app.environments.borrow_mut().remove(&env.host);
        env.backend.disconnect_remote();
        // Floem builds the replacement before running the old view’s cleanup.
        floem::action::exec_after(Duration::ZERO, move |_| env.scope.dispose());
    }
}

/// The section Settings shows in the window `raise` brings forward, while
/// Settings is open there.
fn open_settings_section(raise: floem::ViewId) -> Option<crate::ui::settings::Section> {
    let app = app()?;
    let windows = app.windows.borrow();
    let settings = windows.iter().find(|w| w.raise == raise)?.state.settings;
    settings
        .open
        .try_get_untracked()
        .filter(|open| *open)
        .and_then(|_| settings.section.try_get_untracked())
}

/// An environment a window can be in, as the menu at the drawer's foot
/// lists it.
pub struct EnvironmentChoice {
    /// The SSH host, or none for this machine.
    pub host: Option<String>,
    /// Whether it is the asking window's.
    pub current: bool,
    /// Who is signed in to GitHub there, known while its engine runs:
    /// a host's while a window has it open, this machine's from its first
    /// window on.
    pub login: Option<String>,
    /// Whether the reader can take it out of the menu: a host no window
    /// has open.
    pub forgettable: bool,
}

/// The environments the window of `state` can be in: this machine, then
/// the hosts windows have connected to, then any other a window has open.
pub fn environments(state: AppState) -> Vec<EnvironmentChoice> {
    let login = |state: AppState| {
        state
            .viewer
            .with_untracked(|viewer| viewer.as_ref().map(|(login, _)| login.clone()))
            .filter(|login| !login.is_empty())
    };
    let current = state.place.get_untracked().host();
    let app = app();
    // Each environment a window has open, and who is signed in there.
    let mut open: std::collections::HashMap<Option<String>, Option<String>> = app
        .iter()
        .flat_map(|app| {
            let environments = app.environments.borrow();
            environments
                .iter()
                .map(|(host, env)| (host.clone(), login(env.model)))
                .collect::<Vec<_>>()
        })
        .collect();
    open.insert(current.clone(), login(state));
    let mut hosts = app
        .as_ref()
        .and_then(|app| app.hosts.as_ref())
        .map(Hosts::list)
        .unwrap_or_default();
    let mut others: Vec<String> = open
        .keys()
        .flatten()
        .filter(|host| !hosts.contains(host))
        .cloned()
        .collect();
    others.sort();
    hosts.extend(others);
    std::iter::once(None)
        .chain(hosts.into_iter().map(Some))
        .map(|host| EnvironmentChoice {
            current: host == current,
            login: open.get(&host).cloned().flatten(),
            forgettable: host.is_some() && !open.contains_key(&host),
            host,
        })
        .collect()
}

/// Take `host` out of the environments the menu lists, unless a window
/// has it open. A menu can have been up since before another window
/// opened the host, and still offer to: it is put away then, having
/// already taken the row out, so the next one lists the host again.
pub fn forget_host(host: &str) {
    let Some(app) = app() else { return };
    let open = app
        .environments
        .borrow()
        .contains_key(&Some(host.to_owned()));
    if open {
        crate::ui::menu::close();
    } else if let Some(hosts) = &app.hosts {
        hosts.forget(host);
    }
}

/// Move the window of `state` to the engine on `host`, or with none to
/// this machine's. Settings stays open when it was.
pub fn switch_environment(state: AppState, host: Option<String>) {
    if let Some(app) = app() {
        let window = app
            .windows
            .borrow()
            .iter()
            .find(|w| w.state.settings.open == state.settings.open)
            .cloned();
        if let Some(w) = window.filter(|w| w.environment.host != host) {
            w.destination.set(host);
        }
    }
}

/// Open a new window, laid out as `from` is.
pub fn open(from: Option<AppState>) {
    let saved = if from.is_none() {
        last_window().map(|saved| SavedWindow {
            item: None,
            ..saved
        })
    } else {
        None
    };
    open_window(from, saved);
}

fn last_window() -> Option<SavedWindow> {
    app()?.session.as_ref()?.last.borrow().last().cloned()
}

fn open_window(from: Option<AppState>, saved: Option<SavedWindow>) {
    let (config, frame) = config(saved.as_ref());
    floem::new_window(
        // `from` may have closed before the new window is built.
        move |_| view(from.filter(AppState::is_live), frame, false, saved.clone()).into_any(),
        Some(config),
    );
}

/// New Window, from the Perch menu: laid out as the window in front, or
/// as the first window was with none left.
pub fn open_new() {
    open(focused_state());
}

/// Open…, from the Perch menu: the box in the window that last had the
/// keyboard, or in a new one with none left.
/// Send Feedback… from the Perch menu: the focused window's dropdown,
/// as an Idea unless a draft is there. With no window, one opens with it.
pub fn show_feedback() {
    let Some(app) = app() else {
        return;
    };
    let focused = app.focused.get();
    let window = {
        let windows = app.windows.borrow();
        windows
            .iter()
            .find(|w| Some(w.raise) == focused)
            .or(windows.first())
            .cloned()
    };
    match window {
        Some(window) => crate::ui::feedback::open(
            window.state,
            window.review,
            crate::feedback::FeedbackKind::Idea,
            None,
        ),
        None => {
            app.feedback_on_open.set(true);
            open(None);
        }
    }
}

/// Check for Updates… from the Perch menu: look now, and say what was
/// found in the focused window. With no window, one opens to say it.
pub fn check_for_update() {
    let Some(app) = app() else {
        return;
    };
    let focused = app.focused.get();
    let window = {
        let windows = app.windows.borrow();
        windows
            .iter()
            .find(|w| Some(w.raise) == focused)
            .or(windows.first())
            .cloned()
    };
    match window {
        Some(window) => window.backend.check_for_update(),
        None => {
            app.check_update_on_open.set(true);
            open(None);
        }
    }
}

/// How long Perch is up before it asks for a star: long enough to have
/// read something, and well past the busy start.
const STAR_ASK_AFTER: Duration = Duration::from_secs(3 * 60);

/// Perch has been up a while: have the backend ask for a star, in the
/// focused window, if it never has. With no window open it waits for one.
fn star_due() {
    let Some(app) = app() else {
        return;
    };
    let focused = app.focused.get();
    let window = {
        let windows = app.windows.borrow();
        windows
            .iter()
            .find(|w| Some(w.raise) == focused)
            .or(windows.first())
            .cloned()
    };
    match window {
        Some(window) => window.backend.star_due(),
        None => _ = floem::action::exec_after(Duration::from_secs(60), |_| star_due()),
    }
}

/// Star Perch on GitHub, from the Perch menu: its page, in the browser.
pub fn star_on_github() {
    crate::ui::widgets::open_in_browser(crate::feedback::REPO_URL);
}

/// Each screen's size and backing scale, and the key window's size, for
/// a bug report: a drawing fault often belongs to one scale.
pub fn display_facts() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSApplication, NSScreen};
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return Vec::new();
        };
        let mut lines: Vec<String> = NSScreen::screens(mtm)
            .iter()
            .enumerate()
            .map(|(i, screen)| {
                let frame = screen.frame();
                format!(
                    "Screen {}: {}x{} points at {}x",
                    i + 1,
                    frame.size.width.round(),
                    frame.size.height.round(),
                    screen.backingScaleFactor()
                )
            })
            .collect();
        if let Some(window) = NSApplication::sharedApplication(mtm).keyWindow() {
            let frame = window.frame();
            lines.push(format!(
                "Window: {}x{} points",
                frame.size.width.round(),
                frame.size.height.round()
            ));
        }
        lines
    }
    #[cfg(not(target_os = "macos"))]
    Vec::new()
}

pub fn show_open_box() {
    let Some(app) = app() else {
        return;
    };
    let focused = app.focused.get();
    let window = {
        let windows = app.windows.borrow();
        windows
            .iter()
            .find(|w| Some(w.raise) == focused)
            .or(windows.first())
            .cloned()
    };
    match window {
        Some(window) => crate::ui::open_box::show(window.state, None),
        None => {
            app.open_box_on_open.set(true);
            open(None);
        }
    }
}

/// Settings…, from the Perch menu: Settings over the window that last
/// had the keyboard, or a new window on Settings when none is open.
pub fn show_settings() {
    let Some(app) = app() else {
        return;
    };
    match focused_state() {
        Some(state) => crate::ui::settings::show(state, None),
        None => {
            app.settings_on_open.set(true);
            open(None);
        }
    }
}

/// Open a pull request's or an issue's link in the window that last had
/// the keyboard, which is the one it was clicked in. Whether it was one:
/// anything else, or a link with no window to open it, is the browser's.
pub fn open_link(url: &str) -> bool {
    let Some(app) = app() else {
        return false;
    };
    let focused = app.focused.get();
    let window = {
        let windows = app.windows.borrow();
        windows
            .iter()
            .find(|w| Some(w.raise) == focused)
            .or(windows.first())
            .cloned()
    };
    match window {
        Some(window) => crate::ui::open_box::open_link(window.state, &window.backend, url),
        None => false,
    }
}

/// Download a picture a comment shows, for every window.
pub fn want_picture(url: &str) {
    if let Some(app) = floem::reactive::Context::get::<AppState>().and_then(for_state) {
        app.backend.want_picture(url);
    }
}

/// Ask GitHub who a login is, for every window.
pub fn want_profile(state: Option<AppState>, login: &str) {
    if let Some(app) = state.and_then(for_state) {
        app.backend.want_profile(login);
    }
}

/// The window that last had the keyboard: the one the pointer is in.
pub fn focused_state() -> Option<AppState> {
    let app = app()?;
    let focused = app.focused.get();
    let windows = app.windows.borrow();
    windows
        .iter()
        .find(|w| Some(w.raise) == focused)
        .or(windows.first())
        .map(|w| w.state)
}

/// A click on the Dock icon opens a window when none is left. A minimized
/// one macOS brings back itself.
pub fn reopen() {
    if app().is_some_and(|app| app.windows.borrow().is_empty()) {
        open_window(None, last_window());
    }
}

/// Bring to the front the window, other than `asking`, that has `item_id`
/// open. Whether there was one.
pub fn raise_holding(state: AppState, item_id: &str, asking: WindowId) -> bool {
    let Some(app) = app() else {
        return false;
    };
    let holding = app
        .windows
        .borrow()
        .iter()
        .find(|w| w.state.place == state.place && w.id != asking && w.review.is_item(item_id))
        .cloned();
    match holding {
        Some(window) => {
            window.raise.update_state(());
            true
        }
        None => false,
    }
}

/// A new window's config, and the frame it opens at. Off macOS the frame's
/// origin is no position: Perch can't see the screens there to check one
/// against, so the window manager, which can, places the window.
fn config(saved: Option<&SavedWindow>) -> (WindowConfig, Rect) {
    let frame = saved
        .and_then(|s| parse_frame(s.frame.as_deref()?))
        .map(|r| {
            if cfg!(target_os = "macos") {
                fit(r, &screens())
            } else {
                Rect::from_origin_size(
                    Point::ZERO,
                    Size::new(
                        r.width().max(MIN_SIZE.width),
                        r.height().max(MIN_SIZE.height),
                    ),
                )
            }
        })
        .unwrap_or_else(placement);
    let config = WindowConfig::default();
    let config = match position(frame) {
        Some(at) => config.position(at),
        None => config,
    };
    let config = config
        .size(frame.size())
        .min_size(MIN_SIZE)
        .title("Perch")
        .with_mac_os_config(|mac| {
            // No title bar: the content runs to the top of the window and
            // the app draws its own strip, with the traffic lights sitting
            // in it.
            //
            // The offset is absolute. floem pins the red button's left edge
            // here and places each next one 6px after the previous, at
            // macOS's 14pt width, so the three span 54px. The Codex-shaped
            // navigation leaves 16px at the window edge and centres the
            // lights in its 56px title strip, both zoomed.
            mac.full_size_content_view(true)
                .transparent_title_bar(true)
                .hide_title(true)
                .traffic_lights_offset(crate::theme::traffic_lights_offset())
        });
    (config, frame)
}

/// Where `config` places a window opening at `frame`: there on macOS,
/// and nowhere of Perch's choosing elsewhere.
fn position(frame: Rect) -> Option<Point> {
    cfg!(target_os = "macos").then(|| frame.origin())
}

/// Where a new window goes. Down and to the right of the key window, at
/// its size, as macOS cascades a document's windows, so a new one does
/// not land square on the last. With none open, where the last one was
/// left; the first time, the middle of the main screen.
///
/// Off macOS only the size counts, the last window's or the default: with
/// no screens to fit a frame to, every window had opened in the corner.
fn placement() -> Rect {
    if !cfg!(target_os = "macos") {
        let size = LEFT_AT.get().map_or(DEFAULT_SIZE, |frame| frame.size());
        let size = Size::new(
            size.width.max(MIN_SIZE.width),
            size.height.max(MIN_SIZE.height),
        );
        return Rect::from_origin_size(Point::ZERO, size);
    }
    let screens = screens();
    match key_window_frame()
        .map(|key| key + Vec2::new(STEP, STEP))
        .or(LEFT_AT.get())
    {
        Some(frame) => fit(frame, &screens),
        None => centred(DEFAULT_SIZE, &screens),
    }
}

/// `frame`, kept on the screen its top is on and no bigger than it. One
/// whose top is on no screen, left on a display since unplugged say, could
/// not be dragged back, and goes to the middle of the main screen.
fn fit(frame: Rect, screens: &[Rect]) -> Rect {
    let strip = Rect::new(frame.x0, frame.y0, frame.x1, frame.y0 + DRAG_STRIP);
    let on = screens.iter().find(|screen| {
        let overlap = screen.intersect(strip);
        overlap.width() >= 100.0 && overlap.height() >= DRAG_STRIP / 2.0
    });
    let Some(screen) = on else {
        return centred(frame.size(), screens);
    };
    let size = Size::new(
        frame.width().min(screen.width()).max(MIN_SIZE.width),
        frame.height().min(screen.height()).max(MIN_SIZE.height),
    );
    let x = frame.x0.min(screen.x1 - size.width).max(screen.x0);
    let y = frame.y0.min(screen.y1 - size.height).max(screen.y0);
    Rect::from_origin_size((x, y), size)
}

fn centred(size: Size, screens: &[Rect]) -> Rect {
    let Some(main) = screens.first() else {
        return Rect::from_origin_size(Point::ZERO, size);
    };
    let size = Size::new(
        size.width.min(main.width()).max(MIN_SIZE.width),
        size.height.min(main.height()).max(MIN_SIZE.height),
    );
    let x = (main.x0 + ((main.width() - size.width) / 2.0).max(0.0)).round();
    let y = (main.y0 + ((main.height() - size.height) / 2.0).max(0.0)).round();
    Rect::from_origin_size((x, y), size)
}

/// The key window's frame, top-left from the top of the main screen,
/// which is what floem takes.
#[cfg(target_os = "macos")]
fn key_window_frame() -> Option<Rect> {
    use objc2_app_kit::NSApplication;
    let mtm = objc2::MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(mtm);
    // Perch's frontmost when another app is active.
    let key = app
        .keyWindow()
        .or_else(|| app.orderedWindows().firstObject())?;
    Some(flipped(key.frame(), main_screen_height(mtm)?))
}

#[cfg(target_os = "macos")]
fn can_save_frame(view: floem::ViewId) -> bool {
    use floem::window::WindowIdExt;
    use objc2_app_kit::{NSView, NSWindowStyleMask};
    use raw_window_handle::RawWindowHandle;

    view.window_id()
        .and_then(|id| {
            id.with_window_handle(|handle| {
                let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                    return false;
                };
                // The borrowed handle keeps its NSView valid for this callback on the UI thread.
                let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
                view.window().is_some_and(|window| {
                    !window.styleMask().contains(NSWindowStyleMask::FullScreen)
                })
            })
        })
        .unwrap_or(false)
}

/// Each screen's room for windows, less the menu bar and the Dock, top-left
/// from the top of the main screen. The main screen first.
#[cfg(target_os = "macos")]
fn screens() -> Vec<Rect> {
    use objc2_app_kit::NSScreen;
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return Vec::new();
    };
    let Some(height) = main_screen_height(mtm) else {
        return Vec::new();
    };
    NSScreen::screens(mtm)
        .iter()
        .map(|screen| flipped(screen.visibleFrame(), height))
        .collect()
}

#[cfg(target_os = "macos")]
fn main_screen_height(mtm: objc2::MainThreadMarker) -> Option<f64> {
    Some(
        objc2_app_kit::NSScreen::screens(mtm)
            .firstObject()?
            .frame()
            .size
            .height,
    )
}

/// An AppKit rect, whose origin is its bottom-left from the bottom of the
/// main screen, as floem's: top-left, from the top.
#[cfg(target_os = "macos")]
fn flipped(rect: objc2_foundation::NSRect, main_height: f64) -> Rect {
    let top = main_height - (rect.origin.y + rect.size.height);
    Rect::from_origin_size((rect.origin.x, top), (rect.size.width, rect.size.height))
}

#[cfg(not(target_os = "macos"))]
fn key_window_frame() -> Option<Rect> {
    None
}

#[cfg(not(target_os = "macos"))]
fn can_save_frame(_view: floem::ViewId) -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
fn screens() -> Vec<Rect> {
    Vec::new()
}

/// Nothing to see. A state update to it focuses its window: floem focuses
/// whichever window is handling an update, and a view's update is handled
/// by its own window.
struct Raise {
    id: floem::ViewId,
}

impl Raise {
    fn new() -> Self {
        Self {
            id: floem::ViewId::new(),
        }
    }
}

impl floem::View for Raise {
    fn id(&self) -> floem::ViewId {
        self.id
    }

    fn update(&mut self, _cx: &mut floem::context::UpdateCx, _state: Box<dyn std::any::Any>) {
        floem::action::focus_window();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::new(0.0, 24.0, 1920.0, 1080.0);

    #[test]
    fn switching_one_window_isolates_replies_and_broadcasts() {
        use floem::headless::{HeadlessHarness, TestRoot};

        let app = Rc::new(App::default());
        APP.set(Some(app.clone()));
        let root = TestRoot::new();
        let (_, local_events) = std::sync::mpsc::channel();
        let local = register_environment(&app, None, Backend::detached(), local_events);
        let (_, host_events) = std::sync::mpsc::channel();
        let remote = register_environment(
            &app,
            Some("devbox".into()),
            Backend::detached(),
            host_events,
        );
        let frame = Rect::from_origin_size(Point::ZERO, DEFAULT_SIZE);
        let _first = HeadlessHarness::new(root, view(Some(local.model), frame, false, None));
        let first = app.windows.borrow()[0].clone();
        let second_root = TestRoot::new();
        let mut second_view =
            HeadlessHarness::new(second_root, view(Some(first.state), frame, false, None));
        let second = app.windows.borrow()[1].clone();
        assert!(Rc::ptr_eq(&first.environment, &second.environment));

        // Settings is open: it stays open, on its section, in the
        // window's new environment.
        second
            .state
            .settings
            .section
            .set(crate::ui::settings::Section::Appearance);
        second.state.settings.open.set(true);
        switch_environment(second.state, Some("devbox".into()));
        second_view.rebuild();
        let switched = app.windows.borrow()[1].clone();
        assert_eq!(switched.raise, second.raise);
        assert!(Rc::ptr_eq(&switched.environment, &remote));
        assert_eq!(switched.id, first.id, "IDs belong to their engine");
        assert!(switched.state.settings.open.get_untracked());
        assert_eq!(
            switched.state.settings.section.get_untracked(),
            crate::ui::settings::Section::Appearance
        );
        let choices: Vec<_> = environments(switched.state)
            .into_iter()
            .map(|c| (c.host, c.current, c.forgettable))
            .collect();
        assert_eq!(
            choices,
            [(None, false, false), (Some("devbox".into()), true, false)]
        );
        deliver(&remote, switched.id, Event::Notice("remote only".into()));
        assert_eq!(switched.state.notice().as_deref(), Some("remote only"));
        assert_eq!(first.state.notice(), None);

        deliver(
            &local,
            crate::backend::NO_WINDOW,
            Event::Meta {
                key: "viewer.login".into(),
                value: Some("local-reader".into()),
            },
        );
        assert!(first.state.viewer.get_untracked().is_some());
        assert!(switched.state.viewer.get_untracked().is_none());

        // With Settings shut, as from the menu at the drawer's foot, it
        // stays shut.
        crate::ui::settings::close(switched.state, false);
        switch_environment(switched.state, None);
        second_view.rebuild();
        let returned = app.windows.borrow()[1].clone();
        assert!(Rc::ptr_eq(&returned.environment, &local));
        assert!(!returned.state.settings.open.get_untracked());
        assert_eq!(
            environments(returned.state)
                .iter()
                .find(|c| c.current)
                .and_then(|c| c.login.as_deref()),
            Some("local-reader")
        );
        assert_eq!(
            returned.state.viewer.get_untracked(),
            first.state.viewer.get_untracked()
        );
        assert!(!app
            .environments
            .borrow()
            .contains_key(&Some("devbox".into())));
        APP.set(None);
    }

    fn session_with_host(home: &std::path::Path) -> (Rc<App>, Rc<Environment>) {
        let _root = floem::headless::TestRoot::new();
        let session = Rc::new(Session::load(home, None));
        session.opening.set(0);
        let app = Rc::new(App {
            session: Some(session),
            hosts: Some(Hosts::load(home)),
            ..Default::default()
        });
        APP.set(Some(app.clone()));
        let (_, local_events) = std::sync::mpsc::channel();
        register_environment(&app, None, Backend::detached(), local_events);
        let (_, events) = std::sync::mpsc::channel();
        let remote = register_environment(&app, Some("devbox".into()), Backend::detached(), events);
        (app, remote)
    }

    fn saved_view(host: Option<&str>, item: &str) -> (floem::headless::HeadlessHarness, Window) {
        let root = floem::headless::TestRoot::new();
        let frame = Rect::from_origin_size(Point::ZERO, DEFAULT_SIZE);
        let saved = SavedWindow {
            host: host.map(str::to_owned),
            item: Some(item.into()),
            frame: Some(format_frame(frame)),
        };
        let harness =
            floem::headless::HeadlessHarness::new(root, view(None, frame, false, Some(saved)));
        let window = app().unwrap().windows.borrow().last().unwrap().clone();
        (harness, window)
    }

    fn connected(remote: &Environment) {
        deliver(
            remote,
            crate::backend::NO_WINDOW,
            Event::Link(crate::backend::EngineLink::Up {
                host: "devbox".into(),
                again: false,
                other: None,
                welcome: perch_engine::wire::Welcome {
                    protocol: perch_engine::wire::PROTOCOL,
                    engine_version: "test".into(),
                    connection: 1,
                    host: "devbox".into(),
                    home: "/home/reader".into(),
                    data_root: "/home/reader/.perch".into(),
                    clients: 1,
                    runs: 0,
                    build: "test".into(),
                },
            }),
        );
    }

    /// A host is kept for the menu once it connects, and the menu's ×
    /// takes it out only while no window has it open: a menu up since
    /// before a window opened the host still offers to.
    #[test]
    fn a_connected_host_is_kept_for_the_menu_until_forgotten_with_no_window_on_it() {
        let home = tempfile::tempdir().unwrap();
        let (app, remote) = session_with_host(home.path());
        let kept = || Hosts::load(home.path()).list();
        assert!(kept().is_empty(), "not before it connects");
        connected(&remote);
        connected(&remote);
        assert_eq!(kept(), ["devbox"]);

        forget_host("devbox");
        assert_eq!(kept(), ["devbox"], "a window has it open");
        app.environments.borrow_mut().remove(&remote.host);
        let choices: Vec<_> = environments(remote.model)
            .into_iter()
            .map(|c| (c.host, c.forgettable))
            .collect();
        assert_eq!(
            choices,
            [(None, false), (Some("devbox".into()), false)],
            "the asking window's own"
        );
        forget_host("devbox");
        assert!(kept().is_empty());
        APP.set(None);
    }

    #[test]
    fn restored_items_open_on_their_own_engines_unless_the_user_navigates_first() {
        let home = tempfile::tempdir().unwrap();
        let (_app, remote) = session_with_host(home.path());
        let (_a, local) = saved_view(None, "local-item");
        let (_b, waiting) = saved_view(Some("devbox"), "remote-item");
        let (_c, opening) = saved_view(Some("devbox"), "old-issue");
        let (_d, composing) = saved_view(Some("devbox"), "old-work");

        assert_eq!(
            local.state.open_item.get_untracked().as_deref(),
            Some("local-item")
        );
        assert!(waiting.state.open_item.get_untracked().is_none());
        let navigation = opening
            .backend
            .open_subject("a/b", perch_github::ItemKind::Issue, 7);
        opening.state.begin_open("Reading the issue", navigation);
        crate::ui::new_work::open(composing.state, &composing.backend);
        composing.state.new_work.open.set(false);
        save_windows();
        let snapshot = Session::load(home.path(), None).last.into_inner();
        assert_eq!(
            snapshot
                .iter()
                .map(|w| w.item.as_deref())
                .collect::<Vec<_>>(),
            [Some("local-item"), Some("remote-item"), None, None]
        );

        connected(&remote);
        assert_eq!(
            waiting.state.open_item.get_untracked().as_deref(),
            Some("remote-item")
        );
        assert!(opening.state.accepts_navigation(navigation));
        assert!(opening.state.open_item.get_untracked().is_none());
        assert!(composing.state.open_item.get_untracked().is_none());
        APP.set(None);
    }

    #[test]
    fn closing_one_shared_window_then_quitting_restores_the_two_remaining_windows() {
        use floem::event::{Event as UiEvent, WindowEvent};

        let home = tempfile::tempdir().unwrap();
        let (app, remote) = session_with_host(home.path());
        let (mut local_view, local) = saved_view(None, "local-item");
        let (mut first_view, first) = saved_view(Some("devbox"), "old-item");
        let (mut second_view, second) = saved_view(Some("devbox"), "remote-item");
        assert!(Rc::ptr_eq(&first.environment, &second.environment));
        second_view.dispatch_event(UiEvent::Window(WindowEvent::FocusGained));
        first_view.dispatch_event(UiEvent::Window(WindowEvent::CloseRequested));
        first_view.dispatch_event(UiEvent::Window(WindowEvent::Closed));
        assert_eq!(environment_windows(&remote).len(), 1);
        assert!(Rc::ptr_eq(&environment(Some("devbox".into())), &remote));
        connected(&remote);
        assert_eq!(
            second.state.open_item.get_untracked().as_deref(),
            Some("remote-item")
        );
        assert_eq!(
            local.state.open_item.get_untracked().as_deref(),
            Some("local-item")
        );

        // AppKit Quit destroys the remaining windows without individual close requests.
        second_view.dispatch_event(UiEvent::Window(WindowEvent::Closed));
        local_view.dispatch_event(UiEvent::Window(WindowEvent::Closed));
        assert!(app.windows.borrow().is_empty());
        assert!(!app
            .environments
            .borrow()
            .contains_key(&Some("devbox".into())));
        let restored = Session::load(home.path(), None).last.into_inner();
        assert_eq!(
            restored
                .iter()
                .map(|w| (w.host.as_deref(), w.item.as_deref()))
                .collect::<Vec<_>>(),
            [
                (None, Some("local-item")),
                (Some("devbox"), Some("remote-item"))
            ]
        );
        APP.set(None);
    }

    /// Off macOS a window opens at the last one's size, never under the
    /// minimum, and with no position, for the window manager to choose.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn off_macos_a_window_takes_the_last_size_and_no_position() {
        LEFT_AT.set(None);
        let frame = placement();
        assert_eq!(frame.size(), DEFAULT_SIZE);
        assert_eq!(position(frame), None);

        LEFT_AT.set(Some(Rect::new(300.0, 200.0, 1500.0, 1100.0)));
        let frame = placement();
        assert_eq!(frame.size(), Size::new(1200.0, 900.0));
        assert_eq!(
            position(frame),
            None,
            "where it was left is the window manager's to say"
        );

        LEFT_AT.set(Some(Rect::new(0.0, 0.0, 400.0, 300.0)));
        assert_eq!(placement().size(), MIN_SIZE);
        LEFT_AT.set(None);
    }

    #[test]
    fn the_frame_is_kept_on_this_machine_and_an_old_one_still_read() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(read_frame(home.path()), None);
        // Kept in the database, as before the file.
        let old = Rect::new(10.0, 20.0, 1010.0, 820.0);
        let cache = perch_github::Cache::open(&home.path().join("perch.db"), None).unwrap();
        cache.set_meta(FRAME_KEY, &format_frame(old)).unwrap();
        drop(cache);
        assert_eq!(read_frame(home.path()), Some(old));
        // Kept in the file, which wins from then on.
        let new = Rect::new(120.0, 80.0, 1520.0, 980.0);
        write_frame(home.path(), new).unwrap();
        assert_eq!(read_frame(home.path()), Some(new));
        assert!(!home.path().join("window-frame.partial").exists());
    }

    #[test]
    fn frames_round_trip() {
        let frame = Rect::new(120.0, 80.0, 1520.0, 980.0);
        assert_eq!(parse_frame(&format_frame(frame)), Some(frame));
        assert_eq!(parse_frame("1,2,3"), None);
        assert_eq!(parse_frame("1,2,0,4"), None);
        assert_eq!(parse_frame("a,b,c,d"), None);
    }

    #[test]
    fn a_frame_on_screen_stays_put() {
        let frame = Rect::new(120.0, 80.0, 1520.0, 980.0);
        assert_eq!(fit(frame, &[SCREEN]), frame);
    }

    #[test]
    fn a_frame_off_every_screen_is_centred() {
        let frame = Rect::new(3000.0, 80.0, 4400.0, 980.0);
        let placed = fit(frame, &[SCREEN]);
        assert_eq!(placed.size(), frame.size());
        assert_eq!(placed.center(), SCREEN.center());
    }

    #[test]
    fn a_frame_bigger_than_its_screen_shrinks_onto_it() {
        let frame = Rect::new(100.0, 100.0, 3000.0, 2000.0);
        assert_eq!(fit(frame, &[SCREEN]), SCREEN);
    }

    #[test]
    fn a_frame_is_never_under_the_minimum() {
        let frame = Rect::new(100.0, 100.0, 300.0, 300.0);
        assert_eq!(fit(frame, &[SCREEN]).size(), MIN_SIZE);
    }

    /// Connected again, a window opens its item again: the engine it now
    /// talks to has no view of it.
    #[test]
    fn a_window_reopens_its_item_when_the_engine_is_back() {
        let state = AppState::new();
        let review = ReviewState::new();
        let (backend, mut commands) = Backend::recording();
        // A window with nothing open asks for nothing.
        reopen_item(state, review, &backend, "halo");
        assert!(commands.try_recv().is_err());

        let mut item = perch_github::Item::new("a/b", "2026-09-18T09:59:00Z".parse().unwrap());
        item.id = "pr".into();
        review.item.set(Some(std::sync::Arc::new(item)));
        // What the reader was typing into it stays.
        review.composer_draft.set("half a comment".into());
        review.submit_review_draft.set("half a summary".into());
        reopen_item(state, review, &backend, "halo");
        match commands.try_recv() {
            Ok(crate::backend::Command::OpenItem { id, navigation_id }) => {
                assert_eq!(id, "pr");
                assert!(state.accepts_navigation(navigation_id));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(review.composer_draft.get_untracked(), "half a comment");
        assert_eq!(review.submit_review_draft.get_untracked(), "half a summary");
        assert!(review.is_item("pr"));

        // A window still opening another item, whose ask was lost with the
        // connection, opens that one: its card waits for it.
        state.begin_item_review(review, "Opening", "branch", 1);
        reopen_item(state, review, &backend, "halo");
        match commands.try_recv() {
            Ok(crate::backend::Command::OpenItem { id, navigation_id }) => {
                assert_eq!(id, "branch");
                assert!(state.accepts_navigation(navigation_id));
            }
            other => panic!("{other:?}"),
        }
    }

    /// An act that ends in an item opening, a notification read or Start
    /// work, whose answer went with the connection, is not done again;
    /// its card says so rather than waiting for good.
    #[test]
    fn a_window_waiting_on_a_lost_open_is_told() {
        use crate::review::model::Preparing;
        let state = AppState::new();
        let review = ReviewState::new();
        state.review.set(Some(review));
        let (backend, mut commands) = Backend::recording();
        state.begin_open("Reading the notification", 7);
        reopen_item(state, review, &backend, "halo");
        assert!(commands.try_recv().is_err());
        assert!(matches!(
            review.preparing.get_untracked(),
            Preparing::Failed { message } if message.contains("halo")
        ));

        // The link dropping says so at once. The ask had gone to the
        // engine, so it may have been done: Start work again would cut a
        // second branch.
        state.begin_open("Cutting a branch", 8);
        state.apply(crate::backend::Event::Link(
            crate::backend::EngineLink::Down {
                host: "halo".into(),
                why: "gone".into(),
                retry_in: None,
                ever: true,
                other: None,
            },
        ));
        assert!(matches!(
            review.preparing.get_untracked(),
            Preparing::Failed { message }
                if message.contains("may have been done") && !message.contains("Try it again")
        ));

        // Begun while down, its ask was refused, and nothing was done.
        state.begin_open("Reading the issue", 9);
        assert!(matches!(
            review.preparing.get_untracked(),
            Preparing::Failed { message }
                if message.ends_with("Try it again.") && !message.contains("may have been done")
        ));
    }
}
