//! Perch's windows. Cmd-Shift-N opens another, each with an item of its own
//! open. They share the one backend: every window hears what is about the
//! rows, the inbox and the agents, and a window alone hears the answers to
//! what it asked (`Event::for_one_window`).

use std::cell::RefCell;
use std::rc::Rc;

use floem::ext_event::update_signal_from_channel;
use floem::kurbo::Size;
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
}

#[derive(Clone, Copy)]
struct Window {
    id: WindowId,
    state: AppState,
    review: ReviewState,
    /// Brings the window to the front.
    raise: floem::ViewId,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
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
    let state = new_state();
    state.seed_from(&app.model, settings.as_ref().unwrap_or(&app.model));
    let review = state.review.get_untracked()?;
    let backend = Rc::new(app.backend.open_window());
    Some((state, review, backend))
}

/// The window's whole view: the app's, and what makes it one of several.
pub fn view(state: AppState, review: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    let raise = Raise::new();
    let raise_id = raise.id;
    if let Some(app) = app() {
        app.windows.borrow_mut().push(Window {
            id: backend.window(),
            state,
            review,
            raise: raise_id,
        });
    }
    let id = backend.window();
    let closing = backend.clone();
    Stack::new((
        crate::ui::app_view(state, review, backend),
        raise.style(|s| s.absolute()),
    ))
    .style(|s| s.size_full())
    .on_event_cont(floem::event::listener::WindowClosed, move |_, _| {
        if let Some(app) = app() {
            app.windows.borrow_mut().retain(|w| w.id != id);
        }
        closing.close_window();
    })
}

/// Open a new window, laid out as `from` is.
pub fn open(from: Option<AppState>) {
    floem::new_window(
        move |_| match window(from) {
            Some((state, review, backend)) => view(state, review, backend).into_any(),
            None => floem::views::Empty::new().into_any(),
        },
        Some(config()),
    );
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
        .copied();
    match holding {
        Some(window) => {
            window.raise.update_state(());
            true
        }
        None => false,
    }
}

pub fn config() -> WindowConfig {
    let config = WindowConfig::default();
    let config = match cascaded() {
        Some(at) => config.position(at),
        None => config,
    };
    config
        .size(Size::new(1280.0, 820.0))
        .min_size(Size::new(900.0, 600.0))
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
        })
}

/// Down and to the right of the key window, as macOS cascades a
/// document's windows, so a new one does not land square on the last.
/// Top-left, from the top of the main screen, which is what floem takes.
#[cfg(target_os = "macos")]
fn cascaded() -> Option<floem::kurbo::Point> {
    use objc2_app_kit::{NSApplication, NSScreen};
    const STEP: f64 = 28.0;
    let mtm = objc2::MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(mtm);
    // Perch's frontmost when another app is active.
    let key = app
        .keyWindow()
        .or_else(|| app.orderedWindows().firstObject())?;
    let main = NSScreen::screens(mtm).firstObject()?.frame();
    let frame = key.frame();
    let top = main.size.height - (frame.origin.y + frame.size.height);
    Some(floem::kurbo::Point::new(frame.origin.x + STEP, top + STEP))
}

#[cfg(not(target_os = "macos"))]
fn cascaded() -> Option<floem::kurbo::Point> {
    None
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
