//! The card under a login, as GitHub shows one: the face, the name they
//! go by beside the login, where they work and where they are.
//!
//! It opens once the pointer has rested on a login or a face and GitHub
//! has said who it is, so it never opens half filled, and stays while the
//! pointer is over either the login or the card. An app's login has no
//! card: GitHub has no profile for one.
//!
//! Who a login is is asked once and kept for every window. Only one card
//! is open at a time.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use floem::action::{add_overlay, remove_overlay};
use floem::prelude::*;
use floem::reactive::Scope;
use floem::views::{Container, Decorators, Empty, Label, Stack};
use floem::ViewId;
use perch_github::Profile;

use crate::backend::ProfileAnswer;
use crate::icons::SidebarIcon;
use crate::review::cards::within_window;
use crate::theme;
use crate::ui::widgets::{avatar, sidebar_glyph};

/// How long the pointer rests on a login before its card is asked for.
const REST: Duration = Duration::from_millis(400);
/// How long the card stays once the pointer is over neither it nor the
/// login: long enough to cross the gap between them.
const GRACE: Duration = Duration::from_millis(200);
/// The gap between the login and its card.
const GAP: f64 = 4.0;
const FACE: f64 = 40.0;

#[derive(Clone)]
enum Known {
    Found(Rc<Profile>),
    /// GitHub has no account by the name: not asked after again.
    NoAccount,
}

thread_local! {
    /// Who each login is, as far as GitHub has said.
    static PROFILES: RefCell<HashMap<String, Known>> = RefCell::default();
    /// Every login asked after, so each is asked after once.
    static ASKED: RefCell<HashSet<String>> = RefCell::default();
    /// The login the pointer rested on last, while its card is open or
    /// waiting on GitHub.
    static RESTED: RefCell<Option<Rc<Hover>>> = const { RefCell::new(None) };
}

/// Whether a login has a profile to show: an app's has none.
fn carded(login: &str) -> bool {
    !login.is_empty() && login != "Copilot" && !login.ends_with("[bot]")
}

/// Ask GitHub who `login` is, unless it has been asked.
fn want(login: &str) {
    if ASKED.with(|asked| asked.borrow_mut().insert(login.to_string())) {
        #[cfg(test)]
        tests::WANTED.with(|wanted| wanted.borrow_mut().push(login.to_string()));
        crate::windows::want_profile(login);
    }
}

/// What GitHub said of who `login` is. Each window hears it; the first
/// puts it in place and opens the card that was waiting on it. One that
/// could not be asked is forgotten, to be asked again on the next rest.
pub fn arrived(login: String, answer: ProfileAnswer) {
    let known = match answer {
        ProfileAnswer::Found(profile) => Known::Found(Rc::new(profile)),
        ProfileAnswer::NoAccount => Known::NoAccount,
        ProfileAnswer::Failed => {
            ASKED.with(|asked| asked.borrow_mut().remove(&login));
            return;
        }
    };
    let first = PROFILES.with(|profiles| {
        let mut profiles = profiles.borrow_mut();
        let first = !profiles.contains_key(&login);
        profiles.insert(login.clone(), known.clone());
        first
    });
    let waiting = RESTED.with(|rested| rested.borrow().clone());
    if let (true, Known::Found(profile), Some(hover)) = (first, known, waiting) {
        if hover.rested.borrow().as_deref() == Some(login.as_str()) {
            hover.show(profile);
        }
    }
}

/// One login or face, and its card.
struct Hover {
    /// What the pointer rests on, which the card hangs under.
    trigger: ViewId,
    login: Box<dyn Fn() -> Option<String>>,
    over_trigger: Cell<bool>,
    over_card: Cell<bool>,
    /// Counts the pointer's comings and goings: a wait started before the
    /// last of them does nothing.
    moves: Cell<u64>,
    /// The login the pointer rested on, until the card closes.
    rested: RefCell<Option<String>>,
    /// The card, and the scope its views were built in.
    open: Cell<Option<(ViewId, Scope)>>,
}

impl Hover {
    /// The pointer came onto or left the login or the card.
    fn moved(self: &Rc<Self>) {
        let moves = self.moves.get() + 1;
        self.moves.set(moves);
        let hover = self.clone();
        let resting = self.rested.borrow().is_some();
        if self.over_trigger.get() && !resting {
            after(REST, move || {
                if hover.moves.get() == moves {
                    hover.rest();
                }
            });
        } else if !self.over_trigger.get() && !self.over_card.get() && resting {
            after(GRACE, move || {
                if hover.moves.get() == moves {
                    hover.close();
                }
            });
        }
    }

    /// The pointer has rested on the login: ask who it is, and show the
    /// card if GitHub has already said.
    fn rest(self: &Rc<Self>) {
        let Some(login) = (self.login)().filter(|login| carded(login)) else {
            return;
        };
        let last = RESTED.with(|rested| rested.borrow_mut().replace(self.clone()));
        if let Some(last) = last.filter(|last| !Rc::ptr_eq(last, self)) {
            last.close();
        }
        *self.rested.borrow_mut() = Some(login.clone());
        match PROFILES.with(|profiles| profiles.borrow().get(&login).cloned()) {
            Some(Known::Found(profile)) => self.show(profile),
            Some(Known::NoAccount) => {}
            None => want(&login),
        }
    }

    fn show(self: &Rc<Self>, profile: Rc<Profile>) {
        let open = self.open.take();
        if open.is_some() {
            self.open.set(open);
            return;
        }
        let under = self.trigger.get_visual_rect_no_clip();
        let hover = self.clone();
        let scope = Scope::new();
        let overlay = scope.enter(move || {
            add_overlay(
                within_window(card(&profile, hover))
                    .style(move |s| s.inset_left(under.x0).inset_top(under.y1 + GAP)),
            )
        });
        overlay.set_style_parent(self.trigger);
        self.open.set(Some((overlay, scope)));
    }

    fn close(self: &Rc<Self>) {
        self.moves.set(self.moves.get() + 1);
        self.over_card.set(false);
        *self.rested.borrow_mut() = None;
        if let Some((overlay, scope)) = self.open.take() {
            remove_overlay(overlay);
            scope.dispose();
        }
        RESTED.with(|rested| {
            let mut rested = rested.borrow_mut();
            if rested.as_ref().is_some_and(|last| Rc::ptr_eq(last, self)) {
                *rested = None;
            }
        });
    }
}

/// Run `action` after `wait`. A test runs it when it says so, since no
/// timer fires without a window.
fn after(wait: Duration, action: impl FnOnce() + 'static) {
    #[cfg(test)]
    {
        let _ = wait;
        tests::WAITS.with(|waits| waits.borrow_mut().push(Box::new(action)));
    }
    #[cfg(not(test))]
    floem::action::exec_after(wait, move |_| action());
}

/// `view` with the card of the login it stands for: `None` for a name
/// that is not a login.
///
/// Whether the pointer is over the view is read from floem's hover path,
/// as `track_hover` reads it, rather than taken from which event came.
pub fn with_profile_card<V: IntoView + 'static>(
    view: V,
    login: impl Fn() -> Option<String> + 'static,
) -> impl IntoView {
    let view = view.into_intermediate();
    let id = view.view_id();
    let hover = Rc::new(Hover {
        trigger: id,
        login: Box::new(login),
        over_trigger: Cell::new(false),
        over_card: Cell::new(false),
        moves: Cell::new(0),
        rested: RefCell::new(None),
        open: Cell::new(None),
    });
    let follow = {
        let hover = hover.clone();
        move |cx: &mut floem::event::EventCx| {
            let over = cx.window_state.is_hovered(id);
            if hover.over_trigger.replace(over) != over {
                hover.moved();
            }
        }
    };
    let (enter, leave) = (follow.clone(), follow);
    let (press, wheel, gone) = (hover.clone(), hover.clone(), hover);
    view.on_event_cont(el::PointerEnter, move |cx, _| enter(cx))
        .on_event_cont(el::PointerLeave, move |cx, _| leave(cx))
        // A press opens their page, and a turn of the wheel moves the
        // login out from over its card.
        .on_event_cont(el::PointerDown, move |_, _| press.close())
        .on_event_cont(el::PointerWheel, move |_, _| wheel.close())
        .on_cleanup(move || gone.close())
}

/// The card: the face, the name beside the login, then a line each for
/// where they work and where they are, when they say.
fn card(profile: &Profile, hover: Rc<Hover>) -> impl IntoView {
    let face = {
        let (url, seed) = (profile.avatar_url.clone(), profile.login.clone());
        match crate::windows::focused_state() {
            Some(app) => avatar(app, move || url.clone(), FACE, seed).into_any(),
            None => Empty::new().into_any(),
        }
    };
    let text = |text: &str, strong: bool| {
        let text = text.to_string();
        Label::derived(move || text.clone()).style(move |s| {
            let s = s.font_size(theme::TEXT_META).text_ellipsis().min_width(0.0);
            if strong {
                s.font_weight(theme::weight_emphasis())
                    .color(theme::fg())
                    .flex_shrink(0.0_f32)
                    .max_width_full()
            } else {
                s.color(theme::fg_muted())
            }
        })
    };
    let names = match profile.name() {
        Some(name) => Stack::horizontal((text(name, true), text(&profile.login, false))).into_any(),
        None => text(&profile.login, true).into_any(),
    }
    .style(|s| s.gap(5.0).margin_top(10.0).max_width_full().min_width(0.0));
    let line = |icon: SidebarIcon, said: Option<&str>, name: &'static str| match said {
        Some(said) => Stack::horizontal((
            sidebar_glyph(icon, theme::fg_muted).style(|s| s.size(14.0, 14.0).flex_shrink(0.0_f32)),
            text(said, false),
        ))
        .debug_name(name)
        .style(|s| s.items_center().gap(6.0).margin_top(6.0).max_width_full())
        .into_any(),
        None => Empty::new().into_any(),
    };
    let card = Container::new(Stack::vertical((
        face,
        names,
        line(
            SidebarIcon::Organization,
            profile.company(),
            "profile-company",
        ),
        line(
            SidebarIcon::Location,
            profile.location(),
            "profile-location",
        ),
    )))
    .debug_name("profile-card")
    .style(|s| {
        crate::ui::menu::panel_surface(s)
            .padding(14.0)
            .min_width(200.0)
            .max_width(320.0)
            .pointer_events_auto()
    })
    .into_intermediate();
    let id = card.view_id();
    let follow = move |cx: &mut floem::event::EventCx| {
        let over = cx.window_state.is_hovered(id);
        if hover.over_card.replace(over) != over {
            hover.moved();
        }
    };
    let (enter, leave) = (follow.clone(), follow);
    card.on_event_cont(el::PointerEnter, move |cx, _| enter(cx))
        .on_event_cont(el::PointerLeave, move |cx, _| leave(cx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::headless::{HeadlessHarness, TestRoot};
    use floem::kurbo::Rect;

    thread_local! {
        /// What is waiting to run, in the order it was asked.
        pub(super) static WAITS: RefCell<Vec<Box<dyn FnOnce()>>> = RefCell::default();
        /// The logins GitHub was asked about.
        pub(super) static WANTED: RefCell<Vec<String>> = RefCell::default();
    }

    /// Let every wait run out.
    fn wait(harness: &mut HeadlessHarness) {
        for action in WAITS.with(|waits| std::mem::take(&mut *waits.borrow_mut())) {
            action();
        }
        settle(harness);
    }

    fn settle(harness: &mut HeadlessHarness) {
        for _ in 0..3 {
            harness.rebuild();
            harness.paint();
        }
    }

    fn named(id: ViewId, name: &str) -> Option<ViewId> {
        if id.debug_name().contains(name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    fn rect(harness: &HeadlessHarness, name: &str) -> Option<Rect> {
        named(harness.root_id(), name).map(|id| id.get_visual_rect_no_clip())
    }

    fn wanted() -> Vec<String> {
        WANTED.with(|wanted| std::mem::take(&mut *wanted.borrow_mut()))
    }

    fn profile(login: &str, rest: &str) -> ProfileAnswer {
        ProfileAnswer::Found(
            serde_json::from_str(&format!(r#"{{"login":"{login}"{rest}}}"#)).unwrap(),
        )
    }

    /// A window with `login`'s name at (40, 30), 60 by 16.
    fn window(login: &'static str) -> HeadlessHarness {
        let root = TestRoot::new();
        let name = with_profile_card(
            Empty::new()
                .debug_name("login")
                .style(|s| s.size(60.0, 16.0)),
            move || Some(login.to_string()),
        );
        let page =
            Container::new(name).style(|s| s.size_full().padding_left(40.0).padding_top(30.0));
        let mut harness = HeadlessHarness::new_with_size(root, page, 600.0, 400.0);
        settle(&mut harness);
        harness
    }

    #[test]
    fn a_card_opens_under_a_login_the_pointer_rests_on_once_github_answers() {
        let mut harness = window("rests");
        harness.pointer_move(50.0, 38.0);
        settle(&mut harness);
        assert_eq!(wanted(), Vec::<String>::new(), "not before it has rested");
        wait(&mut harness);
        assert_eq!(wanted(), vec!["rests"]);
        assert_eq!(rect(&harness, "profile-card"), None, "not half filled");

        arrived(
            "rests".into(),
            profile(
                "rests",
                r#","name":"Rests Here","company":"@slatedb","location":null"#,
            ),
        );
        settle(&mut harness);
        let login = rect(&harness, "login").unwrap();
        let card = rect(&harness, "profile-card").expect("the card is open");
        assert_eq!((card.x0, card.y0), (login.x0, login.y1 + GAP));
        assert!(rect(&harness, "profile-company").is_some());
        assert_eq!(rect(&harness, "profile-location"), None, "nothing to say");
    }

    #[test]
    fn a_pointer_that_only_passes_over_asks_nothing() {
        let mut harness = window("passes");
        harness.pointer_move(50.0, 38.0);
        settle(&mut harness);
        harness.pointer_move(300.0, 300.0);
        wait(&mut harness);
        assert_eq!(wanted(), Vec::<String>::new());
        assert_eq!(rect(&harness, "profile-card"), None);
    }

    #[test]
    fn the_card_stays_under_the_pointer_and_goes_when_it_leaves_both() {
        let mut harness = window("stays");
        arrived("stays".into(), profile("stays", ""));
        harness.pointer_move(50.0, 38.0);
        wait(&mut harness);
        assert_eq!(wanted(), Vec::<String>::new(), "GitHub had said already");
        let card = rect(&harness, "profile-card").expect("the card is open");

        // Across the gap and onto the card: the wait that would have
        // closed it finds the pointer there.
        harness.pointer_move(card.center().x, card.center().y);
        wait(&mut harness);
        assert!(rect(&harness, "profile-card").is_some());

        harness.pointer_move(500.0, 350.0);
        wait(&mut harness);
        assert_eq!(rect(&harness, "profile-card"), None);
    }

    #[test]
    fn an_app_and_a_login_github_does_not_know_have_no_card() {
        let mut harness = window("dependabot[bot]");
        harness.pointer_move(50.0, 38.0);
        wait(&mut harness);
        assert_eq!(wanted(), Vec::<String>::new());

        let mut harness = window("ghost-of");
        arrived("ghost-of".into(), ProfileAnswer::NoAccount);
        harness.pointer_move(50.0, 38.0);
        wait(&mut harness);
        assert_eq!(wanted(), Vec::<String>::new());
        assert_eq!(rect(&harness, "profile-card"), None);
    }

    #[test]
    fn a_login_that_could_not_be_asked_after_is_asked_after_again() {
        let mut harness = window("flaky");
        harness.pointer_move(50.0, 38.0);
        wait(&mut harness);
        assert_eq!(wanted(), vec!["flaky"]);
        arrived("flaky".into(), ProfileAnswer::Failed);
        harness.pointer_move(300.0, 300.0);
        wait(&mut harness);
        harness.pointer_move(50.0, 38.0);
        wait(&mut harness);
        assert_eq!(wanted(), vec!["flaky"]);
    }
}
