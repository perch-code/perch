//! Perch's windows. Cmd-Shift-N, New Window in the Perch menu, opens
//! another, each with an item of its own open; Cmd-W closes one. They
//! share the one backend: every window hears what is about the rows, the
//! inbox and the agents, and a window alone hears the answers to what it
//! asked (`Event::for_one_window`).

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

struct App {
    /// The handle windows are opened from. It sends nothing itself.
    backend: Backend,
    /// What the backend has said, heard as a window would. The backend
    /// sends most of it once, at launch, and a new window starts from it.
    model: AppState,
    windows: RefCell<Vec<Window>>,
    /// The window that last had the keyboard, for New Window to take its
    /// layout.
    focused: Cell<Option<WindowId>>,
    /// Open… was asked for with no window open: the next one opens with
    /// the box up.
    open_box_on_open: Cell<bool>,
    /// Send Feedback… was asked for with no window open, the same way.
    feedback_on_open: Cell<bool>,
}

#[derive(Clone)]
struct Window {
    id: WindowId,
    state: AppState,
    review: ReviewState,
    /// Brings the window to the front.
    raise: floem::ViewId,
    /// What the window asks the backend with, for a link followed in it.
    backend: Rc<Backend>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
    /// Where the last window moved or was resized to, for the next one
    /// opened with none open to go. Read from the database at launch.
    static LEFT_AT: Cell<Option<Rect>> = const { Cell::new(None) };
}

/// The meta key a window's frame is kept under: `x,y,width,height`, in
/// points, the top-left from the top of the main screen, as floem places
/// windows.
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
    let frame = perch_github::config::db_path()
        .and_then(|db| perch_github::Cache::peek_meta(&db, FRAME_KEY))
        .and_then(|value| parse_frame(&value));
    LEFT_AT.set(frame);
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

/// Take the backend and its events. From the first window's view function:
/// floem wants a window about before a channel can feed it. What this makes
/// outlives that window.
pub fn start(backend: Backend, events: std::sync::mpsc::Receiver<(WindowId, Event)>) {
    Scope::new().enter(move || {
        let model = new_state();
        let incoming = RwSignal::new(None::<(WindowId, Event)>);
        update_signal_from_channel(incoming.write_only(), events);
        // Folding an event is not a derivation. Untracked, so that a
        // signal this happens to read on the way past cannot subscribe the
        // effect to its own output and have the last event applied twice.
        Effect::new(move |_| {
            if let Some((window, event)) = incoming.get() {
                Effect::untrack(|| deliver(window, event));
            }
        });
        APP.set(Some(Rc::new(App {
            backend,
            model,
            windows: RefCell::new(Vec::new()),
            focused: Cell::new(None),
            open_box_on_open: Cell::new(false),
            feedback_on_open: Cell::new(false),
        })));
    });
}

fn new_state() -> AppState {
    let state = AppState::new();
    let mut review = ReviewState::new();
    review.agent = state.agent;
    state.review.set(Some(review));
    state
}

fn deliver(window: WindowId, event: Event) {
    let Some(app) = app() else {
        return;
    };
    // A copy, so a window closing while the event is applied does not find
    // the list borrowed.
    let windows = app.windows.borrow().clone();
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

/// A window's state and backend handle, from inside its view function.
/// `settings` is the window it was opened from, whose layout it takes.
pub fn window(settings: Option<AppState>) -> Option<(AppState, ReviewState, Rc<Backend>)> {
    let app = app()?;
    let mut state = new_state();
    // Send Feedback's draft and the problems it reports are the app's,
    // not a window's.
    state.feedback = app.model.feedback;
    state.recent_problems = app.model.recent_problems;
    state.seed_from(&app.model, settings.as_ref().unwrap_or(&app.model));
    let review = state.review.get_untracked()?;
    if app.open_box_on_open.replace(false) {
        crate::ui::open_box::show(state, None);
    }
    if app.feedback_on_open.replace(false) {
        crate::ui::feedback::open(state, review, crate::feedback::FeedbackKind::Idea, None);
    }
    let backend = Rc::new(app.backend.open_window());
    Some((state, review, backend))
}

/// The window's whole view: the app's, and what makes it one of several.
/// `frame` is where it was opened, from `config`.
pub fn view(
    state: AppState,
    review: ReviewState,
    backend: Rc<Backend>,
    frame: Rect,
) -> impl IntoView {
    let raise = Raise::new();
    let raise_id = raise.id;
    if let Some(app) = app() {
        app.windows.borrow_mut().push(Window {
            id: backend.window(),
            state,
            review,
            raise: raise_id,
            backend: backend.clone(),
        });
    }
    let id = backend.window();
    let closing = backend.clone();
    // Where the window is, kept as it moves and resizes, and written once
    // it has stopped.
    let frame = Rc::new(Cell::new(frame));
    let timer = Rc::new(Cell::new(None::<TimerToken>));
    let remember = {
        let backend = backend.clone();
        move |at: Rect| {
            if let Some(timer) = timer.take() {
                timer.cancel();
            }
            let backend = backend.clone();
            timer.set(Some(floem::action::exec_after(SAVE_AFTER, move |_| {
                // A full-screen window is the screen's size; the next one
                // should open at the size it came out of full screen to.
                if key_window_full_screen() {
                    return;
                }
                LEFT_AT.set(Some(at));
                backend.set_meta(FRAME_KEY, &format_frame(at));
            })));
        }
    };
    let (resized, moved) = (frame.clone(), frame);
    let remember_size = remember.clone();
    Stack::new((
        crate::ui::app_view(state, review, backend),
        raise.style(|s| s.absolute()),
    ))
    .style(|s| s.size_full())
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
    // and dark.
    .on_event_cont(floem::event::listener::ThemeChanged, |_, theme: &floem::window::Theme| {
        crate::theme::set_appearance((*theme).into());
    })
    .on_event_cont(floem::event::listener::WindowGainedFocus, move |_, _| {
        if let Some(app) = app() {
            app.focused.set(Some(id));
        }
    })
    .on_event_cont(floem::event::listener::WindowClosed, move |_, _| {
        if let Some(app) = app() {
            app.windows.borrow_mut().retain(|w| w.id != id);
        }
        closing.close_window();
    })
}

/// Open a new window, laid out as `from` is.
pub fn open(from: Option<AppState>) {
    let (config, frame) = config();
    floem::new_window(
        move |_| match window(from) {
            Some((state, review, backend)) => view(state, review, backend, frame).into_any(),
            None => floem::views::Empty::new().into_any(),
        },
        Some(config),
    );
}

/// New Window, from the Perch menu: laid out as the window in front, or
/// as the first window was with none left.
pub fn open_new() {
    let from = app().and_then(|app| {
        let focused = app.focused.get()?;
        app.windows
            .borrow()
            .iter()
            .find(|w| w.id == focused)
            .map(|w| w.state)
    });
    open(from);
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
            .find(|w| Some(w.id) == focused)
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
            .find(|w| Some(w.id) == focused)
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
            .find(|w| Some(w.id) == focused)
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
    if let Some(app) = app() {
        app.backend.want_picture(url);
    }
}

/// A click on the Dock icon opens a window when none is left. A minimized
/// one macOS brings back itself.
pub fn reopen() {
    if app().is_some_and(|app| app.windows.borrow().is_empty()) {
        open(None);
    }
}

/// Bring to the front the window, other than `asking`, that has `item_id`
/// open. Whether there was one.
pub fn raise_holding(item_id: &str, asking: WindowId) -> bool {
    let Some(app) = app() else {
        return false;
    };
    let holding = app
        .windows
        .borrow()
        .iter()
        .find(|w| w.id != asking && w.review.is_item(item_id))
        .cloned();
    match holding {
        Some(window) => {
            window.raise.update_state(());
            true
        }
        None => false,
    }
}

/// A new window's config, and the frame it opens at.
pub fn config() -> (WindowConfig, Rect) {
    let frame = placement();
    let config = WindowConfig::default()
        .position(frame.origin())
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
            // lights in its 56px title strip.
            mac.full_size_content_view(true)
                .transparent_title_bar(true)
                .hide_title(true)
                .traffic_lights_offset((16.0, 21.0))
        });
    (config, frame)
}

/// Where a new window goes. Down and to the right of the key window, at
/// its size, as macOS cascades a document's windows, so a new one does
/// not land square on the last. With none open, where the last one was
/// left; the first time, the middle of the main screen.
fn placement() -> Rect {
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
fn key_window_full_screen() -> bool {
    use objc2_app_kit::{NSApplication, NSWindowStyleMask};
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return false;
    };
    NSApplication::sharedApplication(mtm)
        .keyWindow()
        .is_some_and(|key| key.styleMask().contains(NSWindowStyleMask::FullScreen))
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
fn key_window_full_screen() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
fn screens() -> Vec<Rect> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::new(0.0, 24.0, 1920.0, 1080.0);

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
