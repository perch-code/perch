//! Perch's menus: every list of commands that pops up, from a button or a
//! right-click, drawn by Perch as Codex draws its dropdowns rather than
//! handed to macOS.
//!
//! A menu is built as floem's native one was, `Menu::new().item(..)
//! .separator().submenu(..)`, and hung from a view with `dropdown` (under
//! the view, at a press) or `right_click_menu` (at the pointer). Up, it is
//! a rounded white panel over the window, rows of a leading check or icon,
//! the name, a shortcut and a submenu's chevron, and it is as modal as a
//! native menu: a press anywhere else puts it away and goes no further,
//! and it keeps the keyboard until it goes. The arrows move through it,
//! Right and Left open and shut a submenu, Enter or Space picks, Escape
//! backs out, a letter goes to the next row it starts, and a row's
//! shortcut picks that row. Picking puts the menu away first and gives
//! the keyboard back to whatever had it, then runs the row, except a row
//! that toggles: it flips its check and leaves the menu up. A row that
//! can be taken out of its list shows a × while it is lit; the × or
//! Delete takes the row out and leaves the menu up. A menu with a filter
//! has a box over its rows that keeps only those matching what is typed;
//! the box has the keyboard, but for the keys that move, pick and back
//! out.
//!
//! One menu is up at a time, app-wide, as with native menus.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::str::FromStr;
use std::time::{Duration, Instant};

use floem::event::EventPropagation;
use floem::kurbo::{Point, Rect, Size};
use floem::peniko::{Brush, Color};
use floem::prelude::*;
use floem::reactive::{Effect, RwSignal, Scope, SignalGet, SignalTrack, SignalUpdate, SignalWith};
use floem::ui_events::pointer::PointerButton;
use floem::views::editor::SelectionColor;
use floem::views::{
    dyn_stack, Container, Decorators, Empty, Label, PlaceholderTextClass, ScrollExt, Stack,
    TextInput,
};
use floem::{View, ViewId};

use crate::icons::SidebarIcon;
use crate::theme;
use crate::ui::widgets::sidebar_glyph;

/// A menu, or a submenu: its entries in order, whether it can be opened
/// at all (a submenu greyed as a whole), and a submenu's icon.
pub struct Menu {
    entries: Vec<Entry>,
    enabled: bool,
    icon: Option<SidebarIcon>,
    return_focus: Option<ViewId>,
    /// What the filter box shows while it is empty, for a menu with one.
    filter: Option<String>,
    /// The most rows that may be checked at once.
    most_checked: Option<usize>,
}

enum Entry {
    Item(Item),
    Submenu { label: String, menu: Rc<Menu> },
    Separator,
}

/// One command: its name, what it does, and how it looks.
pub struct Item {
    label: String,
    icon: Option<SidebarIcon>,
    /// Whether it is checked, for a row of a set; none for a command.
    /// A row that toggles flips it while the menu is up.
    checked: Option<Cell<bool>>,
    enabled: bool,
    shortcut: Option<Shortcut>,
    /// Words at the row's end in the shortcut's place, such as when a
    /// snooze ends.
    detail: Option<String>,
    /// A line under the name saying what the row does, for a set whose
    /// names alone do not.
    note: Option<String>,
    /// The name's and note's colour in place of the usual, for a row
    /// that warns.
    tint: Option<fn() -> Color>,
    action: Option<Rc<dyn Fn()>>,
    /// What picking a row that toggles does in place of `action`, with
    /// the check's new state.
    toggle: Option<Rc<dyn Fn(bool)>>,
    /// What the row's × does, for a row that can be taken out of its list.
    remove: Option<Rc<dyn Fn()>>,
    /// Taken out while the menu is up.
    gone: Cell<bool>,
}

impl Default for Menu {
    fn default() -> Self {
        Self::new()
    }
}

impl Menu {
    pub fn new() -> Self {
        Menu {
            entries: Vec::new(),
            enabled: true,
            icon: None,
            return_focus: None,
            filter: None,
            most_checked: None,
        }
    }

    pub fn item(mut self, label: impl Into<String>, build: impl FnOnce(Item) -> Item) -> Self {
        let item = Item {
            label: label.into(),
            icon: None,
            checked: None,
            enabled: true,
            shortcut: None,
            detail: None,
            note: None,
            tint: None,
            action: None,
            toggle: None,
            remove: None,
            gone: Cell::new(false),
        };
        self.entries.push(Entry::Item(build(item)));
        self
    }

    pub fn submenu(mut self, label: impl Into<String>, build: impl FnOnce(Menu) -> Menu) -> Self {
        self.entries.push(Entry::Submenu {
            label: label.into(),
            menu: Rc::new(build(Menu::new())),
        });
        self
    }

    /// A line between groups. Doubled lines and lines at either end are
    /// dropped when the menu is drawn, so a builder need not count.
    pub fn separator(mut self) -> Self {
        self.entries.push(Entry::Separator);
        self
    }

    /// A submenu greyed as a whole.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// A submenu's icon, beside its name in the menu above.
    pub fn icon(mut self, icon: SidebarIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The view to have the keyboard once the menu goes, when nothing had
    /// it as the menu opened: a press on a view that takes no focus, such
    /// as a chip under a text box, takes it from the box first.
    pub fn return_focus(mut self, id: Option<ViewId>) -> Self {
        self.return_focus = id;
        self
    }

    /// A box over the rows that keeps only those whose name or detail has
    /// what is typed in it, in any case, showing `placeholder` while it is
    /// empty. It has the keyboard while the menu is up.
    pub fn filter(mut self, placeholder: impl Into<String>) -> Self {
        self.filter = Some(placeholder.into());
        self
    }

    /// No more than `most` rows checked at once: with that many, the rows
    /// that are not are greyed.
    pub fn most_checked(mut self, most: usize) -> Self {
        self.most_checked = Some(most);
        self
    }

    pub fn is_empty(&self) -> bool {
        !self.entries.iter().any(|e| !matches!(e, Entry::Separator))
    }

    /// The entries as drawn: no separator first, last or twice.
    fn tidy(mut self) -> Self {
        let mut entries = Vec::with_capacity(self.entries.len());
        for entry in self.entries {
            let separator = matches!(entry, Entry::Separator);
            if separator && entries.last().is_none_or(|e| matches!(e, Entry::Separator)) {
                continue;
            }
            entries.push(entry);
        }
        if matches!(entries.last(), Some(Entry::Separator)) {
            entries.pop();
        }
        self.entries = entries;
        self
    }

    /// The menu as a reader would list it: `✓ ` before a checked row, a
    /// greyed one in parentheses, `▸` after a submenu, `—` for a line.
    #[cfg(test)]
    pub fn outline(&self) -> Vec<String> {
        self.entries
            .iter()
            .enumerate()
            .map(|(index, e)| match e {
                Entry::Item(i) => {
                    let mark = if i.is_checked() { "✓ " } else { "" };
                    if self.live(index) {
                        format!("{mark}{}", i.label)
                    } else {
                        format!("{mark}({})", i.label)
                    }
                }
                Entry::Submenu { label, menu } if menu.enabled => format!("{label} ▸"),
                Entry::Submenu { label, .. } => format!("({label} ▸)"),
                Entry::Separator => "—".to_string(),
            })
            .collect()
    }

    /// The submenu under `label`.
    #[cfg(test)]
    pub fn submenu_named(&self, label: &str) -> Option<Rc<Menu>> {
        self.entries.iter().find_map(|e| match e {
            Entry::Submenu { label: l, menu } if l == label => Some(menu.clone()),
            _ => None,
        })
    }

    /// Pick the row named `label`, as a click on it would: a row that
    /// toggles flips. Panics when there is none, or it is greyed.
    #[cfg(test)]
    pub fn choose(&self, label: &str) {
        let (index, item) = self
            .entries
            .iter()
            .enumerate()
            .find_map(|(index, e)| match e {
                Entry::Item(i) if i.label == label => Some((index, i)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {label:?} in {:?}", self.outline()));
        assert!(self.live(index), "{label:?} is greyed");
        match &item.toggle {
            Some(toggle) => toggle(item.flip()),
            None => (item.action.as_ref().expect("no action"))(),
        }
    }

    /// Whether as many rows are checked as may be.
    fn full(&self) -> bool {
        self.most_checked.is_some_and(|most| {
            let checked = self
                .entries
                .iter()
                .filter(|e| matches!(e, Entry::Item(i) if i.is_checked()))
                .count();
            checked >= most
        })
    }

    /// Whether row `index` can be picked, whatever is typed: not greyed,
    /// not taken out, and not one more check than the menu takes.
    fn live(&self, index: usize) -> bool {
        match &self.entries[index] {
            Entry::Item(i) => {
                let over = i.checked.is_some() && !i.is_checked() && self.full();
                i.enabled && !i.gone.get() && !over
            }
            Entry::Submenu { menu, .. } => menu.enabled,
            Entry::Separator => false,
        }
    }

    /// Whether row `index` has `typed`, already in lower case, in its name
    /// or its detail.
    fn matches(&self, index: usize, typed: &str) -> bool {
        let has = |text: &str| text.to_lowercase().contains(typed);
        match &self.entries[index] {
            Entry::Item(i) => has(&i.label) || i.detail.as_deref().is_some_and(has),
            Entry::Submenu { label, .. } => has(label),
            Entry::Separator => false,
        }
    }
}

impl Item {
    /// A check before the name: the one picked of a set, or a setting
    /// that is on. A menu with any such row keeps room for the check on
    /// every row, so the names line up whichever is checked.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(Cell::new(checked));
        self
    }

    /// A check that picking flips, leaving the menu up, for a set any
    /// number of which may be on: `toggled` runs with the new state.
    pub fn toggle(mut self, toggled: impl Fn(bool) + 'static) -> Self {
        self.checked.get_or_insert_with(|| Cell::new(false));
        self.toggle = Some(Rc::new(toggled));
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn icon(mut self, icon: SidebarIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The keys shown at the row's end. While the menu is up they pick
    /// the row, as a native menu's key equivalents do.
    pub fn accelerator(mut self, shortcut: Shortcut) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    /// Words shown at the row's end, where a shortcut would be.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A line under the name, in a smaller, fainter face, which makes
    /// the row taller.
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Draw the name and note in `color`, while the row can be picked.
    pub fn tint(mut self, color: fn() -> Color) -> Self {
        self.tint = Some(color);
        self
    }

    pub fn action(mut self, action: impl Fn() + 'static) -> Self {
        self.action = Some(Rc::new(action));
        self
    }

    /// A × at the row's end while it is lit, as a list of recent things
    /// has: it takes the row out of the menu, which stays up, and runs
    /// `remove`. Delete does the same for the row the keyboard is on.
    pub fn removable(mut self, remove: impl Fn() + 'static) -> Self {
        self.remove = Some(Rc::new(remove));
        self
    }

    fn is_checked(&self) -> bool {
        self.checked.as_ref().is_some_and(Cell::get)
    }

    /// Flip the check, and say what it is now.
    fn flip(&self) -> bool {
        let Some(checked) = &self.checked else {
            return false;
        };
        checked.set(!checked.get());
        checked.get()
    }
}

impl Entry {
    fn label(&self) -> &str {
        match self {
            Entry::Item(i) => &i.label,
            Entry::Submenu { label, .. } => label,
            Entry::Separator => "",
        }
    }

    /// How tall its row is: taller with a note under the name.
    fn height(&self) -> f64 {
        match self {
            Entry::Item(Item { note: Some(_), .. }) => NOTE_ROW_HEIGHT,
            _ => ROW_HEIGHT,
        }
    }
}

/// A key and its modifiers, as a menu row shows them and a press matches
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: Modifiers,
    pub code: Code,
}

impl Shortcut {
    /// A bare key: a letter that works while the menu is up.
    pub const fn key(code: Code) -> Self {
        Shortcut {
            modifiers: Modifiers::empty(),
            code,
        }
    }

    /// Keys written as `super+shift+KeyH` or `F12`: modifiers first,
    /// joined by `+`, then the key's code. `super` is the platform's
    /// command key, as in VS Code's keybindings: Cmd on macOS, Ctrl
    /// elsewhere.
    pub fn parse(keys: &str) -> Option<Self> {
        let mut parts: Vec<&str> = keys.split('+').collect();
        let code = Code::from_str(parts.pop()?).ok()?;
        let mut modifiers = Modifiers::empty();
        for part in parts {
            modifiers |= match part.to_ascii_lowercase().as_str() {
                "super" | "cmd" | "command" | "meta" => crate::ui::OS_MOD,
                "shift" => Modifiers::SHIFT,
                "alt" | "option" => Modifiers::ALT,
                "ctrl" | "control" => Modifiers::CONTROL,
                _ => return None,
            };
        }
        Some(Shortcut { modifiers, code })
    }

    /// The written form of keys `parse` reads, `super+KeyP` as ⌘P on
    /// macOS and Ctrl+P elsewhere; the keys themselves if they don't parse.
    pub fn label_of(keys: &str) -> String {
        Shortcut::parse(keys).map_or_else(|| keys.to_string(), |s| s.label())
    }

    /// How the platform writes it. macOS: `⌃⌥⇧⌘` in that order, then the
    /// key. Elsewhere: `Ctrl+Alt+Shift+Super+`, then the key.
    pub fn label(&self) -> String {
        let mut out = String::new();
        let marks: [(Modifiers, &str); 4] = if cfg!(target_os = "macos") {
            [
                (Modifiers::CONTROL, "⌃"),
                (Modifiers::ALT, "⌥"),
                (Modifiers::SHIFT, "⇧"),
                (Modifiers::META, "⌘"),
            ]
        } else {
            [
                (Modifiers::CONTROL, "Ctrl+"),
                (Modifiers::ALT, "Alt+"),
                (Modifiers::SHIFT, "Shift+"),
                (Modifiers::META, "Super+"),
            ]
        };
        for (flag, mark) in marks {
            if self.modifiers.contains(flag) {
                out.push_str(mark);
            }
        }
        let code = self.code.to_string();
        let key = match self.code {
            Code::Enter | Code::NumpadEnter => "↩".to_string(),
            Code::Escape => "⎋".to_string(),
            Code::Backspace => "⌫".to_string(),
            Code::Delete => "⌦".to_string(),
            Code::Tab => "⇥".to_string(),
            Code::Space => "Space".to_string(),
            Code::ArrowUp => "↑".to_string(),
            Code::ArrowDown => "↓".to_string(),
            Code::ArrowLeft => "←".to_string(),
            Code::ArrowRight => "→".to_string(),
            Code::BracketLeft => "[".to_string(),
            Code::BracketRight => "]".to_string(),
            Code::Minus => "-".to_string(),
            Code::Equal => "=".to_string(),
            Code::Comma => ",".to_string(),
            Code::Period => ".".to_string(),
            Code::Slash => "/".to_string(),
            _ => code
                .strip_prefix("Key")
                .or_else(|| code.strip_prefix("Digit"))
                .unwrap_or(&code)
                .to_string(),
        };
        out.push_str(&key);
        out
    }

    fn matches(&self, key: &KeyboardEvent) -> bool {
        key.code == self.code && key.modifiers == self.modifiers
    }
}

/// Codex's menu, in its measures: a 4px gutter inside a 16px corner, rows
/// 30 high with 12px corners, 8px in from each side, and lines 4px clear
/// of the rows on either side, inset to the rows' text.
pub const GUTTER: f64 = 4.0;
pub const RADIUS: f64 = 16.0;
const ROW_HEIGHT: f64 = 30.0;
/// A row with a note under its name, as Codex's approvals menu has.
const NOTE_ROW_HEIGHT: f64 = 46.0;
pub const ROW_RADIUS: f64 = 12.0;
const ROW_PADDING: f64 = 8.0;
const SEPARATOR_HEIGHT: f64 = 9.0;
const MIN_WIDTH: f64 = 180.0;
const MAX_WIDTH: f64 = 480.0;
/// The room kept between a menu and the window's edges.
const EDGE: f64 = 8.0;
/// Between a dropdown and the view it hangs from.
const DROP_GAP: f64 = 4.0;
/// How long a submenu stays after the pointer leaves its row for another,
/// so a diagonal on the way to it does not shut it.
const SUBMENU_GRACE: Duration = Duration::from_millis(300);
/// A release this soon after the menu opened, or with the pointer still
/// where it pressed, is the end of the press that opened it, not a pick:
/// a menu can open over the view it hangs from.
const RELEASE_GRACE: Duration = Duration::from_millis(300);
/// How far past a row's × a release still counts as on it.
const CROSS_SLOP: f64 = 4.0;

/// The frame of a menu, and of every list that pops up as one does: the
/// `@` list, Cmd-P, Open…, the ways to close, the reactions. White, in a
/// faint ring, with Codex's soft shadow under it.
pub fn surface(s: floem::style::Style) -> floem::style::Style {
    s.background(theme::menu_bg())
        .border(1.0)
        .border_color(theme::menu_ring())
        .border_radius(RADIUS)
        .box_shadow_blur(16.0)
        .box_shadow_v_offset(8.0)
        .box_shadow_spread(-4.0)
        .box_shadow_color(theme::menu_shadow())
}

/// The frame of a panel hanging from the bar, Agents, usage and Review:
/// a menu's, with the deeper shadow Codex gives its panels.
pub fn panel_surface(s: floem::style::Style) -> floem::style::Style {
    surface(s)
        .box_shadow_blur(32.0)
        .box_shadow_v_offset(16.0)
        .box_shadow_spread(-8.0)
        .box_shadow_color(theme::panel_shadow())
}

/// Where a menu hangs from, in window coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Anchor {
    /// Under a view, from its left edge, or over it without room below.
    Below(Rect),
    /// At a point, the pointer's: its top left corner there, or its right
    /// edge without room on the right.
    At(Point),
    /// Beside a row of the menu above, its rows level with that row.
    Beside(Rect),
}

/// Where a menu `size` big goes for `anchor`, in a window `window` big:
/// its top left corner, kept inside the window.
fn place(anchor: Anchor, size: Size, window: Size) -> Point {
    let clamp_x = |x: f64| x.min(window.width - EDGE - size.width).max(EDGE);
    let clamp_y = |y: f64| y.min(window.height - EDGE - size.height).max(EDGE);
    let fits_right = |x: f64| x + size.width <= window.width - EDGE;
    match anchor {
        Anchor::Below(r) => {
            let below = r.y1 + DROP_GAP;
            let above = r.y0 - DROP_GAP - size.height;
            let y = if below + size.height <= window.height - EDGE || above < EDGE {
                clamp_y(below)
            } else {
                above
            };
            Point::new(clamp_x(r.x0), y)
        }
        Anchor::At(p) => {
            let x = if fits_right(p.x) {
                p.x
            } else {
                p.x - size.width
            };
            Point::new(clamp_x(x), clamp_y(p.y))
        }
        Anchor::Beside(r) => {
            // The panel's edge over the row's, less its gutter and ring,
            // so its first row sits level with the row it came from.
            let right = r.x1 + GUTTER + 2.0;
            let left = r.x0 - GUTTER - 2.0 - size.width;
            let x = if fits_right(right) || left < EDGE {
                right
            } else {
                left
            };
            Point::new(clamp_x(x), clamp_y(r.y0 - GUTTER - 1.0))
        }
    }
}

/// One panel up: the top menu or a submenu opened from it.
#[derive(Clone)]
struct Level {
    key: u64,
    depth: usize,
    menu: Rc<Menu>,
    anchor: Anchor,
    highlight: RwSignal<Option<usize>>,
    /// Its size as first laid out, before it was placed.
    measured: RwSignal<Option<Size>>,
    rows: Rc<RefCell<Vec<Option<ViewId>>>>,
    /// How many times a row has been taken out or its check flipped, for
    /// the views to redraw by.
    changed: RwSignal<usize>,
    /// What is in the filter box, for a menu with one.
    query: RwSignal<String>,
}

/// The menu while it is up.
struct Host {
    scope: Scope,
    levels: RwSignal<Vec<Level>>,
    window: RwSignal<Size>,
    min_width: f64,
    opened: Instant,
    pressed_inside: Cell<bool>,
    /// Whether the pointer has moved since the menu opened.
    moved: Cell<bool>,
    /// What had the keyboard before, to have it back.
    restore: Option<ViewId>,
    overlay: Cell<Option<ViewId>>,
    /// The view that has the keyboard while the menu is up.
    keys: Cell<Option<ViewId>>,
    next_key: Cell<u64>,
    closed: Cell<bool>,
}

thread_local! {
    static OPEN: RefCell<Option<Rc<Host>>> = const { RefCell::new(None) };
}

/// Whether a menu is up.
#[cfg(test)]
pub fn is_open() -> bool {
    OPEN.with(|open| open.borrow().is_some())
}

/// Put away whatever menu is up, without picking anything.
pub fn close() {
    let host = OPEN.with(|open| open.borrow().clone());
    if let Some(host) = host {
        host.close();
    }
}

/// Open `menu` under `trigger`, at least as wide as it. `keyboard` when a
/// key opened it: the first row starts picked.
pub fn open_below(cx: &floem::context::EventCx<'_>, trigger: ViewId, menu: Menu, keyboard: bool) {
    let rect = trigger.get_visual_rect_no_clip();
    let restore = crate::ui::focused_descendant(cx, trigger.root());
    open(Anchor::Below(rect), menu, rect.width(), keyboard, restore);
}

/// Open `menu` with its corner at `at`, in window coordinates.
pub fn open_at(cx: &floem::context::EventCx<'_>, near: ViewId, at: Point, menu: Menu) {
    let restore = crate::ui::focused_descendant(cx, near.root());
    open(Anchor::At(at), menu, 0.0, false, restore);
}

fn open(anchor: Anchor, menu: Menu, min_width: f64, keyboard: bool, restore: Option<ViewId>) {
    close();
    let menu = menu.tidy();
    if menu.is_empty() {
        return;
    }
    let restore = restore.or(menu.return_focus);
    let scope = Scope::new();
    let host = scope.enter(|| {
        Rc::new(Host {
            scope,
            levels: RwSignal::new(Vec::new()),
            window: RwSignal::new(Size::ZERO),
            min_width: min_width.max(MIN_WIDTH),
            opened: Instant::now(),
            pressed_inside: Cell::new(false),
            moved: Cell::new(false),
            restore,
            overlay: Cell::new(None),
            keys: Cell::new(None),
            next_key: Cell::new(0),
            closed: Cell::new(false),
        })
    });
    let first = host.level(0, Rc::new(menu), anchor);
    if keyboard {
        first.highlight.set(first.step(None, 1));
    }
    host.levels.set(vec![first]);
    let view = scope.enter(|| host_view(host.clone()));
    host.overlay.set(Some(floem::action::add_overlay(view)));
    OPEN.with(|open| *open.borrow_mut() = Some(host));
}

impl Level {
    /// What is typed in the filter box, trimmed and in lower case, as rows
    /// are matched against it. Tracked, as are changes to the rows.
    fn typed(&self) -> String {
        self.changed.track();
        self.query.with(|q| q.trim().to_lowercase())
    }

    /// Whether row `index` is drawn for `typed`: not taken out, and
    /// matching it. Lines have rules of their own.
    fn shown(&self, index: usize, typed: &str) -> bool {
        match &self.menu.entries[index] {
            Entry::Item(i) if i.gone.get() => false,
            _ => self.menu.matches(index, typed),
        }
    }

    /// Whether the line at `index` is drawn for `typed`: with rows drawn
    /// on both sides of it.
    fn line_shown(&self, index: usize, typed: &str) -> bool {
        let n = self.menu.entries.len();
        (0..index).any(|i| self.shown(i, typed)) && (index + 1..n).any(|i| self.shown(i, typed))
    }

    fn selectable(&self, index: usize, typed: &str) -> bool {
        self.menu.live(index) && self.shown(index, typed)
    }

    /// The next row from `from` that can be picked, `by` rows on,
    /// wrapping: the first (or last) with nothing picked.
    fn step(&self, from: Option<usize>, by: isize) -> Option<usize> {
        let typed = self.typed();
        let n = self.menu.entries.len() as isize;
        if n == 0 {
            return None;
        }
        let mut at = match from {
            Some(i) => i as isize,
            None if by > 0 => -1,
            None => n,
        };
        for _ in 0..n {
            at = (at + by).rem_euclid(n);
            if self.selectable(at as usize, &typed) {
                return Some(at as usize);
            }
        }
        None
    }

    /// Where row `index` starts, down from the top of the panel's content.
    fn row_top(&self, index: usize) -> f64 {
        let typed = self.typed();
        let height = |i: usize| match &self.menu.entries[i] {
            Entry::Separator if self.line_shown(i, &typed) => SEPARATOR_HEIGHT,
            Entry::Separator => 0.0,
            entry if self.shown(i, &typed) => entry.height(),
            _ => 0.0,
        };
        GUTTER + (0..index).map(height).sum::<f64>()
    }
}

impl Host {
    fn level(&self, depth: usize, menu: Rc<Menu>, anchor: Anchor) -> Level {
        let key = self.next_key.get();
        self.next_key.set(key + 1);
        let rows = vec![None; menu.entries.len()];
        self.scope.enter(|| Level {
            key,
            depth,
            menu,
            anchor,
            highlight: RwSignal::new(None),
            measured: RwSignal::new(None),
            rows: Rc::new(RefCell::new(rows)),
            changed: RwSignal::new(0),
            query: RwSignal::new(String::new()),
        })
    }

    fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        OPEN.with(|open| {
            let mut open = open.borrow_mut();
            if open.as_ref().is_some_and(|h| std::ptr::eq(&**h, self)) {
                *open = None;
            }
        });
        if let Some(overlay) = self.overlay.take() {
            match self.restore {
                Some(id) => id.request_focus(),
                None => overlay.clear_focus(),
            }
            floem::action::remove_overlay(overlay);
        }
    }

    /// Open the submenu at row `index` of the panel `depth` deep, shutting
    /// any other open under it. `keyboard` picks its first row.
    fn open_submenu(&self, depth: usize, index: usize, keyboard: bool) {
        let Some(level) = self.levels.with_untracked(|l| l.get(depth).cloned()) else {
            return;
        };
        let Some(Entry::Submenu { menu, .. }) = level.menu.entries.get(index) else {
            return;
        };
        if !menu.enabled {
            return;
        }
        let already = self.levels.with_untracked(|l| {
            l.get(depth + 1)
                .is_some_and(|child| Rc::ptr_eq(&child.menu, menu))
        });
        if !already {
            let row = level.rows.borrow().get(index).copied().flatten();
            let rect = row.map(|r| r.get_visual_rect_no_clip()).unwrap_or_default();
            let child = self.level(depth + 1, menu.clone(), Anchor::Beside(rect));
            self.levels.update(|l| {
                l.truncate(depth + 1);
                l.push(child);
            });
        }
        if keyboard {
            if let Some(child) = self.levels.with_untracked(|l| l.get(depth + 1).cloned()) {
                child.highlight.set(child.step(None, 1));
            }
        }
    }

    /// Shut every panel under the one `depth` deep.
    fn collapse(&self, depth: usize) {
        if self.levels.with_untracked(|l| l.len() > depth + 1) {
            self.levels.update(|l| l.truncate(depth + 1));
        }
    }

    /// Pick row `index` of the panel `depth` deep: run an item, flip a
    /// row that toggles, open a submenu.
    fn activate(&self, depth: usize, index: usize, keyboard: bool) {
        let Some(level) = self.levels.with_untracked(|l| l.get(depth).cloned()) else {
            return;
        };
        if index >= level.menu.entries.len() || !level.menu.live(index) {
            return;
        }
        match &level.menu.entries[index] {
            Entry::Item(item) => match item.toggle.clone() {
                Some(toggle) => {
                    let on = item.flip();
                    level.changed.update(|n| *n += 1);
                    toggle(on);
                }
                None => {
                    let action = item.action.clone();
                    self.close();
                    if let Some(action) = action {
                        action();
                    }
                }
            },
            Entry::Submenu { .. } => self.open_submenu(depth, index, keyboard),
            Entry::Separator => {}
        }
    }

    /// Take row `index` of the panel `depth` deep out of the menu and run
    /// its `remove`. `keyboard` lights the next row in its place.
    fn remove(&self, depth: usize, index: usize, keyboard: bool) {
        let Some(level) = self.levels.with_untracked(|l| l.get(depth).cloned()) else {
            return;
        };
        let Some(Entry::Item(item)) = level.menu.entries.get(index) else {
            return;
        };
        let Some(remove) = item.remove.clone() else {
            return;
        };
        if !level.menu.live(index) {
            return;
        }
        item.gone.set(true);
        level.changed.update(|n| *n += 1);
        level.highlight.set(if keyboard {
            level.step(Some(index), 1)
        } else {
            None
        });
        remove();
    }

    /// A key while the menu is up. With a filter, the box has the keys
    /// that type, edit and move along what is typed, and whatever an
    /// input method is composing with.
    fn key_down(&self, key: &KeyboardEvent) -> EventPropagation {
        let Some(level) = self.levels.with_untracked(|l| l.last().cloned()) else {
            return EventPropagation::Stop;
        };
        let depth = level.depth;
        let at = level.highlight.get_untracked();
        let plain = key.modifiers.is_empty();
        let typing = depth == 0 && level.menu.filter.is_some();
        let edits = matches!(
            key.code,
            Code::Space
                | Code::Backspace
                | Code::Delete
                | Code::ArrowLeft
                | Code::ArrowRight
                | Code::Home
                | Code::End
        );
        if typing && (key.is_composing || edits) {
            return EventPropagation::Continue;
        }
        match key.code {
            Code::ArrowDown if plain => level.highlight.set(level.step(at, 1)),
            Code::ArrowUp if plain => level.highlight.set(level.step(at, -1)),
            Code::Home | Code::PageUp if plain => level.highlight.set(level.step(None, 1)),
            Code::End | Code::PageDown if plain => level.highlight.set(level.step(None, -1)),
            Code::ArrowRight if plain => {
                if let Some(at) = at {
                    self.open_submenu(depth, at, true);
                }
            }
            Code::ArrowLeft | Code::Escape if plain && depth > 0 => self.collapse(depth - 1),
            Code::Escape if plain => self.close(),
            Code::Enter | Code::NumpadEnter | Code::Space if plain => {
                if let Some(at) = at {
                    self.activate(depth, at, true);
                }
            }
            Code::Backspace | Code::Delete if plain || key.modifiers == crate::ui::OS_MOD => {
                if let Some(at) = at {
                    self.remove(depth, at, true);
                }
            }
            _ => {
                // A bare key in a box is typed, not a shortcut.
                let shortcut = level.menu.entries.iter().position(|e| match e {
                    Entry::Item(i) => {
                        i.enabled
                            && i.shortcut.is_some_and(|s| {
                                s.matches(key) && !(typing && s.modifiers.is_empty())
                            })
                    }
                    _ => false,
                });
                if let Some(index) = shortcut {
                    self.activate(depth, index, true);
                } else if typing {
                    return EventPropagation::Continue;
                } else if let (true, Key::Character(c)) = (plain, &key.key) {
                    self.type_select(&level, c);
                }
            }
        }
        EventPropagation::Stop
    }

    /// A letter: the next row, after the one picked, whose name starts
    /// with it.
    fn type_select(&self, level: &Level, typed: &str) {
        let typed = typed.to_lowercase();
        if typed.trim().is_empty() {
            return;
        }
        let n = level.menu.entries.len();
        let from = level.highlight.get_untracked().map_or(0, |i| i + 1);
        let shown = level.typed();
        let found = (0..n).map(|k| (from + k) % n).find(|&i| {
            level.selectable(i, &shown)
                && level.menu.entries[i]
                    .label()
                    .to_lowercase()
                    .starts_with(&typed)
        });
        if found.is_some() {
            level.highlight.set(found);
        }
    }
}

/// The menu over the window: a sheet that takes every press outside the
/// panels, and the panels.
fn host_view(host: Rc<Host>) -> impl View {
    let levels = host.levels;
    let panel_host = host.clone();
    let press_host = host.clone();
    let wheel_host = host.clone();
    let key_host = host.clone();
    let layout_host = host.clone();
    let move_host = host.clone();
    let asked = Cell::new(false);
    let root = dyn_stack(
        move || levels.get(),
        |level: &Level| level.key,
        move |level| panel(panel_host.clone(), level),
    );
    let root_id = root.id();
    // A filter's box, when there is one, has the keyboard instead.
    if host.keys.get().is_none() {
        host.keys.set(Some(root_id));
    }
    root.on_event_stop(el::PointerDown, move |_, _| press_host.close())
        .on_event_stop(el::PointerWheel, move |_, _| wheel_host.close())
        .on_event_cont(el::PointerMove, move |_, _| move_host.moved.set(true))
        // Caught on the way down, ahead of a filter's box.
        .on_event_with_config(
            el::KeyDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE_AND_TARGET,
            },
            move |_, key: &KeyboardEvent| key_host.key_down(key),
        )
        .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
            let size = root_id.get_layout_rect().size();
            if size.width > 0.0 && layout_host.window.get_untracked() != size {
                layout_host.window.set(size);
            }
            if !asked.replace(true) {
                layout_host.keys.get().unwrap_or(root_id).request_focus();
            }
        })
        .on_cleanup({
            let scope = host.scope;
            move || scope.dispose()
        })
        .debug_name("menu")
        .style(|s| {
            s.inset(0.0)
                .focusable()
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_META)
                .font_weight(theme::weight_body())
                .color(theme::fg())
        })
}

/// One panel: the rows of a menu or a submenu, placed once measured.
fn panel(host: Rc<Host>, level: Level) -> impl IntoView {
    let menu = level.menu.clone();
    let checks = menu
        .entries
        .iter()
        .any(|e| matches!(e, Entry::Item(i) if i.checked.is_some()));
    let icons = menu.entries.iter().any(|e| match e {
        Entry::Item(i) => i.icon.is_some(),
        Entry::Submenu { menu, .. } => menu.icon.is_some(),
        Entry::Separator => false,
    });
    let rows = menu
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| match entry {
            Entry::Separator => separator(&level, index).into_any(),
            _ => row(host.clone(), &level, index, entry, checks, icons).into_any(),
        })
        .chain(menu.filter.is_some().then(|| no_matches(&level).into_any()))
        .collect::<Vec<_>>();
    let filter = menu
        .filter
        .clone()
        .filter(|_| level.depth == 0)
        .map(|placeholder| filter_box(&host, &level, placeholder));
    let highlight = level.highlight;
    let scroll_level = level.clone();
    let content = Stack::vertical_from_iter(rows)
        .style(|s| {
            s.flex_col()
                .min_width_full()
                .max_width_full()
                .padding(GUTTER)
        })
        .scroll()
        .ensure_visible(move || match highlight.get() {
            Some(i) => {
                let top = scroll_level.row_top(i);
                let height = scroll_level.menu.entries[i].height();
                Rect::new(0.0, top - GUTTER, 1.0, top + height + GUTTER)
            }
            None => Rect::ZERO,
        })
        .style(|s| {
            s.flex_col()
                .min_height(0.0)
                .flex_shrink(1.0_f32)
                .overflow_x(floem::taffy::Overflow::Hidden)
                // No gutter kept for a bar, which would narrow every row;
                // like a native menu's, a long one scrolls with no bar.
                .scrollbar_width(0.0)
        });

    let measured = level.measured;
    let anchor = level.anchor;
    let depth = level.depth;
    let window = host.window;
    let min_width = if depth == 0 {
        host.min_width
    } else {
        MIN_WIDTH
    };
    let panel = Stack::vertical((
        filter.map_or_else(|| Empty::new().into_any(), IntoView::into_any),
        content,
    ));
    let panel_id = panel.id();
    let enter_host = host.clone();
    let leave_host = host.clone();
    let press_host = host.clone();
    panel
        // A press on a row takes the keyboard from the menu, so it is
        // handed back.
        .on_event_stop(el::PointerDown, move |_, _| {
            press_host.pressed_inside.set(true);
            if let Some(keys) = press_host.keys.get() {
                keys.request_focus();
            }
        })
        .on_event_stop(el::PointerWheel, |_, _| {})
        .on_event_cont(el::UpdatePhaseLayout, move |_, _| {
            if measured.get_untracked().is_none() {
                let size = panel_id.get_layout_rect().size();
                if size.width > 0.0 && size.height > 0.0 {
                    measured.set(Some(size));
                }
            }
        })
        // Coming into a submenu keeps its row in the menu above lit.
        .on_event_cont(el::PointerEnter, move |_, _| {
            if depth == 0 {
                return;
            }
            enter_host.levels.with_untracked(|levels| {
                let (Some(parent), Some(me)) = (levels.get(depth - 1), levels.get(depth)) else {
                    return;
                };
                let row = parent.menu.entries.iter().position(|e| match e {
                    Entry::Submenu { menu, .. } => Rc::ptr_eq(menu, &me.menu),
                    _ => false,
                });
                if parent.highlight.get_untracked() != row {
                    parent.highlight.set(row);
                }
            });
        })
        // Leaving a panel unlights its row, unless that row's submenu is
        // what the pointer went to.
        .on_event_cont(el::PointerLeave, move |_, _| {
            let child_open = leave_host.levels.with_untracked(|l| l.len() > depth + 1);
            if !child_open && highlight.get_untracked().is_some() {
                highlight.set(None);
            }
        })
        .debug_name(format!("menu-panel:{depth}"))
        .style(move |s| {
            let window = window.get();
            let s = s
                .absolute()
                .flex_col()
                .min_width(min_width)
                .max_width(MAX_WIDTH)
                .apply(surface(floem::style::Style::new()))
                .apply_if(window.height > 2.0 * EDGE, |s| {
                    s.max_height(window.height - 2.0 * EDGE)
                });
            match measured.get() {
                Some(size) if window.width > 0.0 => {
                    let at = place(anchor, size, window);
                    let s = s.inset_left(at.x).width(size.width);
                    // Over the view it hangs from, it keeps its bottom
                    // edge by the view as rows go.
                    match anchor {
                        Anchor::Below(r) if at.y + size.height <= r.y0 => {
                            s.inset_bottom(window.height - at.y - size.height)
                        }
                        _ => s.inset_top(at.y),
                    }
                }
                // Laid out once in the corner to be measured, where the
                // pointer must not find it.
                _ => s.inset_left(0.0).inset_top(0.0).pointer_events_none(),
            }
        })
}

/// The line at `index`, gone with every row on one side of it.
fn separator(level: &Level, index: usize) -> impl IntoView {
    let level = level.clone();
    Container::new(Empty::new().style(|s| {
        s.height(1.0)
            .width_full()
            .background(theme::menu_separator())
    }))
    .style(move |s| {
        let alone = !level.line_shown(index, &level.typed());
        s.height(SEPARATOR_HEIGHT)
            .flex_shrink(0.0_f32)
            .items_center()
            .padding_horiz(ROW_PADDING)
            .apply_if(alone, |s| s.hide())
    })
}

/// A filter's box, over the rows. It has the keyboard from the start, and
/// typing lights the first row left.
fn filter_box(host: &Host, level: &Level, placeholder: String) -> impl IntoView {
    let input = TextInput::new(level.query)
        .placeholder(placeholder)
        .debug_name("menu-filter")
        .style(|s| {
            s.width_full()
                .min_width(0.0)
                .height(ROW_HEIGHT)
                .padding_horiz(ROW_PADDING)
                .background(theme::canvas())
                .hover(|s| s.background(theme::canvas()))
                .border(1.0)
                .border_color(theme::border())
                .border_radius(ROW_RADIUS)
                .focus(|s| s.border_color(theme::accent()))
                .set(SelectionColor, Brush::Solid(theme::selection_bg()))
                .class(PlaceholderTextClass, |s| s.color(theme::fg_subtle()))
        });
    host.keys.set(Some(input.view_id()));
    let level = level.clone();
    Effect::new(move |last: Option<String>| {
        let typed = level.query.get();
        if last.is_some_and(|last| last != typed) && !typed.trim().is_empty() {
            let first = Effect::untrack(|| level.step(None, 1));
            level.highlight.set(first);
        }
        typed
    });
    Container::new(input).style(|s| s.padding(GUTTER).padding_bottom(0.0).flex_shrink(0.0_f32))
}

/// What a filter shows when no row has what is typed.
fn no_matches(level: &Level) -> impl IntoView {
    let level = level.clone();
    Container::new(
        Label::derived(|| "No matches".to_string())
            .style(|s| s.color(theme::fg_subtle()).selectable(false)),
    )
    .debug_name("menu-no-matches")
    .style(move |s| {
        let typed = level.typed();
        let any = (0..level.menu.entries.len()).any(|i| level.shown(i, &typed));
        s.height(ROW_HEIGHT)
            .flex_shrink(0.0_f32)
            .items_center()
            .padding_horiz(ROW_PADDING)
            .apply_if(any, |s| s.hide())
    })
}

/// One row: a check, an icon, the name, the shortcut, a submenu's
/// chevron.
fn row(
    host: Rc<Host>,
    level: &Level,
    index: usize,
    entry: &Entry,
    checks: bool,
    icons: bool,
) -> impl IntoView {
    let (label, icon, shortcut, detail, submenu, enabled) = match entry {
        Entry::Item(i) => (
            i.label.clone(),
            i.icon,
            i.shortcut,
            i.detail.clone(),
            false,
            i.enabled,
        ),
        Entry::Submenu { label, menu } => {
            (label.clone(), menu.icon, None, None, true, menu.enabled)
        }
        Entry::Separator => unreachable!("a separator is not a row"),
    };
    let removable = matches!(entry, Entry::Item(i) if i.remove.is_some());
    // Greyed for good. A row of a full set greys only its name, as it
    // shows no check and has no icon.
    let fg = if enabled { theme::fg } else { theme::fg_subtle };
    let highlight = level.highlight;
    let depth = level.depth;
    let menu = level.menu.clone();
    let live = {
        let menu = menu.clone();
        move || menu.live(index)
    };

    let check_level = level.clone();
    let check_name = format!("menu-check:{}", entry.label());
    let check = checks.then(|| {
        Container::new(
            sidebar_glyph(SidebarIcon::Check, fg)
                .style(move |s| {
                    check_level.changed.track();
                    let checked = matches!(
                        &check_level.menu.entries[index],
                        Entry::Item(i) if i.is_checked()
                    );
                    s.size(14.0, 14.0).apply_if(!checked, |s| s.hide())
                })
                .debug_name(check_name),
        )
        .style(|s| {
            s.width(16.0)
                .flex_shrink(0.0_f32)
                .items_center()
                .justify_center()
        })
    });
    let icon = icons.then(|| match icon {
        Some(icon) => sidebar_glyph(icon, fg)
            .style(|s| s.size(16.0, 16.0).flex_shrink(0.0_f32))
            .into_any(),
        None => Empty::new()
            .style(|s| s.width(16.0).flex_shrink(0.0_f32))
            .into_any(),
    });
    let (note, tint) = match entry {
        Entry::Item(i) => (i.note.clone(), i.tint),
        _ => (None, None),
    };
    let name_level = level.clone();
    let name = Label::derived(move || label.clone()).style(move |s| {
        name_level.changed.track();
        let fg = match tint {
            _ if !name_level.menu.live(index) => theme::fg_subtle(),
            Some(tint) => tint(),
            None => theme::fg(),
        };
        s.color(fg)
            .text_ellipsis()
            .min_width(0.0)
            .flex_grow(1.0_f32)
            .selectable(false)
    });
    let name = match note {
        Some(note) => {
            let note_level = level.clone();
            let note = Label::derived(move || note.clone()).style(move |s| {
                note_level.changed.track();
                let fg = match tint {
                    Some(tint) if note_level.menu.live(index) => tint(),
                    _ => theme::fg_subtle(),
                };
                s.font_size(theme::TEXT_BADGE)
                    .color(fg)
                    .text_ellipsis()
                    .min_width(0.0)
                    .selectable(false)
            });
            Stack::vertical((name, note))
                .style(|s| s.min_width(0.0).flex_grow(1.0_f32).gap(2.0))
                .into_any()
        }
        None => name.into_any(),
    };
    let keys = shortcut.map(|shortcut| {
        let text = shortcut.label();
        Label::derived(move || text.clone()).style(|s| {
            s.font_size(theme::TEXT_BADGE)
                .color(theme::fg_subtle())
                .margin_left(20.0)
                .flex_shrink(0.0_f32)
                .selectable(false)
        })
    });
    let keys = keys.map(|k| k.into_any()).or_else(|| {
        detail.map(|text| {
            Label::derived(move || text.clone())
                .style(|s| {
                    s.color(theme::fg_subtle())
                        .margin_left(20.0)
                        .flex_shrink(0.0_f32)
                        .selectable(false)
                })
                .into_any()
        })
    });
    let chevron = submenu.then(|| {
        sidebar_glyph(SidebarIcon::ChevronRight, theme::fg_subtle)
            .style(|s| s.size(12.0, 12.0).margin_left(16.0).flex_shrink(0.0_f32))
    });
    // Its room is kept whether it shows or not, so lighting a row does
    // not cut its name short.
    let cross_name = format!("menu-cross:{}", entry.label());
    let cross = removable.then(|| {
        Container::new(
            sidebar_glyph(SidebarIcon::Close, theme::fg_subtle)
                .style(move |s| {
                    s.size(12.0, 12.0)
                        .apply_if(highlight.get() != Some(index), |s| s.hide())
                })
                .debug_name(cross_name),
        )
        .style(|s| {
            s.size(16.0, 16.0)
                .margin_left(12.0)
                .flex_shrink(0.0_f32)
                .items_center()
                .justify_center()
        })
    });
    let cross_id = cross.as_ref().map(|c| c.id());

    let content = Stack::horizontal((
        check
            .map(|c| c.into_any())
            .unwrap_or_else(|| Empty::new().into_any()),
        icon.unwrap_or_else(|| Empty::new().into_any()),
        name,
        keys.unwrap_or_else(|| Empty::new().into_any()),
        chevron
            .map(|c| c.into_any())
            .unwrap_or_else(|| Empty::new().into_any()),
        cross
            .map(|c| c.into_any())
            .unwrap_or_else(|| Empty::new().into_any()),
    ))
    .style(|s| s.items_center().gap(8.0).width_full().pointer_events_none());

    let row = Container::new(content);
    let row_id = row.id();
    level.rows.borrow_mut()[index] = Some(row_id);
    let enter_host = host.clone();
    let up_host = host.clone();
    let enter_live = live.clone();
    let style_level = level.clone();
    row.on_event_cont(el::PointerEnter, move |_, _| {
        if !enter_live() {
            if highlight.get_untracked().is_some() {
                highlight.set(None);
            }
            schedule_collapse(enter_host.clone(), depth, None);
            return;
        }
        if highlight.get_untracked() != Some(index) {
            highlight.set(Some(index));
        }
        if submenu {
            enter_host.open_submenu(depth, index, false);
        } else {
            schedule_collapse(enter_host.clone(), depth, Some(index));
        }
    })
    .on_event_stop(el::PointerUp, move |cx, up: &PointerButtonEvent| {
        let dragged = up_host.moved.get() && up_host.opened.elapsed() >= RELEASE_GRACE;
        let released = up_host.pressed_inside.get() || dragged;
        if !(live() && released) {
            return;
        }
        let at = cx.world_transform.inverse() * up.state.logical_point();
        let on_cross = cross_id.is_some_and(|id| {
            id.get_visual_rect_no_clip()
                .inflate(CROSS_SLOP, CROSS_SLOP)
                .contains(at)
        });
        if on_cross {
            up_host.remove(depth, index, false);
        } else {
            up_host.activate(depth, index, false);
        }
    })
    .debug_name(format!("menu-row:{}", entry.label()))
    .style(move |s| {
        let typed = style_level.typed();
        let lit = menu.live(index) && highlight.get() == Some(index);
        s.height(menu.entries[index].height())
            .flex_shrink(0.0_f32)
            .items_center()
            .padding_horiz(ROW_PADDING)
            .border_radius(ROW_RADIUS)
            .apply_if(lit, |s| s.background(theme::menu_hover()))
            .apply_if(!style_level.shown(index, &typed), |s| s.hide())
    })
}

/// Shut the submenus under the panel `depth` deep a moment from now, if
/// the pointer is still on row `from` then (a row with no submenu, or
/// none at all).
fn schedule_collapse(host: Rc<Host>, depth: usize, from: Option<usize>) {
    if host.levels.with_untracked(|l| l.len() <= depth + 1) {
        return;
    }
    floem::action::exec_after(SUBMENU_GRACE, move |_| {
        if host.closed.get() {
            return;
        }
        let Some(level) = host.levels.with_untracked(|l| l.get(depth).cloned()) else {
            return;
        };
        if level.highlight.get_untracked() == from {
            host.collapse(depth);
        }
    });
}

/// Hanging a menu from a view.
pub trait MenuExt: View + Sized + 'static {
    /// A dropdown: the menu opens under the view at a press, or at Enter,
    /// Space or Down while the view has the keyboard.
    fn dropdown(self, menu: impl Fn() -> Menu + 'static) -> Self {
        let id = self.id();
        let menu = Rc::new(menu);
        let key_menu = menu.clone();
        self.on_event(el::PointerDown, move |cx, press: &PointerButtonEvent| {
            if press.button != Some(PointerButton::Primary) {
                return EventPropagation::Continue;
            }
            open_below(cx, id, menu(), false);
            EventPropagation::Stop
        })
        .on_event(el::KeyDown, move |cx, key: &KeyboardEvent| {
            let opens = matches!(key.code, Code::Enter | Code::Space | Code::ArrowDown)
                && key.modifiers.is_empty();
            if opens && cx.window_state.is_focused(id) {
                open_below(cx, id, key_menu(), true);
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
    }

    /// A context menu: it opens at the pointer at a right-click.
    fn right_click_menu(self, menu: impl Fn() -> Menu + 'static) -> Self {
        let id = self.id();
        self.on_event(el::PointerDown, move |cx, press: &PointerButtonEvent| {
            if press.button != Some(PointerButton::Secondary) {
                return EventPropagation::Continue;
            }
            let at = cx.world_transform.inverse() * press.state.logical_point();
            open_at(cx, id, at, menu());
            EventPropagation::Stop
        })
    }
}

impl<V: View + Sized + 'static> MenuExt for V {}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::headless::{HeadlessHarness, TestRoot};

    fn named(id: ViewId, name: &str) -> Option<ViewId> {
        if id.debug_name().contains(name) {
            return Some(id);
        }
        id.children()
            .into_iter()
            .find_map(|child| named(child, name))
    }

    fn settle(harness: &mut HeadlessHarness) {
        for _ in 0..3 {
            harness.rebuild();
            harness.paint();
        }
    }

    fn rect(harness: &HeadlessHarness, name: &str) -> Rect {
        let id = named(harness.root_id(), name).unwrap_or_else(|| panic!("no {name}"));
        id.get_visual_rect_no_clip()
    }

    fn shown(harness: &HeadlessHarness, name: &str) -> bool {
        named(harness.root_id(), name).is_some()
    }

    fn key(code: Code, key: Key) -> floem::event::Event {
        floem::event::Event::Key(KeyboardEvent {
            state: KeyState::Down,
            key,
            code,
            location: Default::default(),
            modifiers: Modifiers::empty(),
            repeat: false,
            is_composing: false,
        })
    }

    fn named_key(code: Code, named: NamedKey) -> floem::event::Event {
        key(code, Key::Named(named))
    }

    /// A button in a window `size` big, at `at`, whose dropdown is
    /// `menu`, and a log of what the menu ran.
    fn window_with(
        at: (f64, f64),
        size: (f64, f64),
        menu: impl Fn(Rc<RefCell<Vec<String>>>) -> Menu + 'static,
    ) -> (HeadlessHarness, Rc<RefCell<Vec<String>>>) {
        let root = TestRoot::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        let menu_log = log.clone();
        let button = Empty::new()
            .dropdown(move || menu(menu_log.clone()))
            .debug_name("button")
            .style(move |s| {
                s.absolute()
                    .inset_left(at.0)
                    .inset_top(at.1)
                    .size(60.0, 20.0)
                    .keyboard_navigable()
            });
        let page = Stack::new((button,)).style(|s| s.size_full());
        let mut harness = HeadlessHarness::new_with_size(root, page, size.0, size.1);
        settle(&mut harness);
        (harness, log)
    }

    fn logs(log: &Rc<RefCell<Vec<String>>>, name: &'static str) -> impl Fn() + 'static {
        let log = log.clone();
        move || log.borrow_mut().push(name.to_string())
    }

    fn sample(log: Rc<RefCell<Vec<String>>>) -> Menu {
        Menu::new()
            .item("Pin", |i| {
                i.icon(SidebarIcon::Pin).action(logs(&log, "pin"))
            })
            .item("Rename", |i| i.enabled(false).action(logs(&log, "rename")))
            .separator()
            .separator()
            .submenu("Open in", |sub| {
                sub.item("Finder", |i| i.action(logs(&log, "finder")))
                    .item("Terminal", |i| i.action(logs(&log, "terminal")))
            })
            .item("Delete", |i| {
                i.accelerator(Shortcut::key(Code::KeyD))
                    .action(logs(&log, "delete"))
            })
            .separator()
    }

    /// Press the button and let go off it, on no row: a release on a row
    /// the menu opened over could count as a pick, on a slow run.
    fn open_it(harness: &mut HeadlessHarness) {
        let button = rect(harness, "button").center();
        harness.pointer_down(button.x, button.y);
        settle(harness);
        harness.pointer_up(1.0, 1.0);
        settle(harness);
    }

    #[test]
    fn separators_are_dropped_at_the_ends_and_when_doubled() {
        let menu = sample(Rc::default()).tidy();
        assert_eq!(
            menu.outline(),
            vec!["Pin", "(Rename)", "—", "Open in ▸", "Delete"]
        );
        assert!(Menu::new().separator().is_empty());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn shortcuts_read_as_macos_writes_them() {
        let parse = |s: &str| Shortcut::parse(s).unwrap().label();
        assert_eq!(parse("F12"), "F12");
        assert_eq!(parse("super+F12"), "⌘F12");
        assert_eq!(parse("shift+alt+KeyH"), "⌥⇧H");
        assert_eq!(parse("super+KeyC"), "⌘C");
        assert_eq!(Shortcut::key(Code::KeyP).label(), "P");
        assert_eq!(
            Shortcut {
                modifiers: Modifiers::META | Modifiers::ALT,
                code: Code::KeyR
            }
            .label(),
            "⌥⌘R"
        );
        assert!(Shortcut::parse("hyper+KeyA").is_none());
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn shortcuts_read_with_ctrl_for_super_off_macos() {
        let parse = |s: &str| Shortcut::parse(s).unwrap();
        assert_eq!(parse("super+KeyC").modifiers, Modifiers::CONTROL);
        assert_eq!(parse("F12").label(), "F12");
        assert_eq!(parse("super+F12").label(), "Ctrl+F12");
        assert_eq!(parse("shift+alt+KeyH").label(), "Alt+Shift+H");
        assert_eq!(parse("alt+super+KeyR").label(), "Ctrl+Alt+R");
        assert_eq!(Shortcut::label_of("super+BracketLeft"), "Ctrl+[");
        assert!(Shortcut::parse("hyper+KeyA").is_none());
    }

    #[test]
    fn a_row_with_a_note_is_taller_and_picks_like_any_other() {
        let (mut harness, log) = window_with((40.0, 30.0), (600.0, 400.0), |log| {
            Menu::new()
                .item("Ask me", |i| {
                    i.checked(true)
                        .note("Every request comes to you")
                        .action(logs(&log, "ask"))
                })
                .item("Allow all", |i| {
                    i.note("Perch allows every request")
                        .tint(theme::alert_caution)
                        .action(logs(&log, "all"))
                })
                .item("Plain", |i| i.action(logs(&log, "plain")))
        });
        open_it(&mut harness);
        let ask = rect(&harness, "menu-row:Ask me");
        let all = rect(&harness, "menu-row:Allow all");
        let plain = rect(&harness, "menu-row:Plain");
        assert_eq!(ask.height(), NOTE_ROW_HEIGHT);
        assert_eq!(all.y0, ask.y1, "rows stack with no gap");
        assert_eq!(plain.height(), ROW_HEIGHT);
        let panel = rect(&harness, "menu-panel:0");
        assert_eq!(
            panel.height(),
            2.0 * NOTE_ROW_HEIGHT + ROW_HEIGHT + 2.0 * GUTTER + 2.0
        );
        let all = all.center();
        harness.click(all.x, all.y);
        settle(&mut harness);
        assert!(!is_open());
        assert_eq!(*log.borrow(), vec!["all".to_string()]);
    }

    #[test]
    fn a_dropdown_hangs_under_its_button_and_a_click_picks_a_row() {
        let (mut harness, log) = window_with((40.0, 30.0), (600.0, 400.0), sample);
        open_it(&mut harness);
        assert!(is_open());
        let button = rect(&harness, "button");
        let panel = rect(&harness, "menu-panel:0");
        assert_eq!(panel.x0, button.x0, "left edges level");
        assert_eq!(panel.y0, button.y1 + DROP_GAP, "just under");
        assert!(panel.width() >= MIN_WIDTH);
        let pin = rect(&harness, "menu-row:Pin");
        assert_eq!(
            (pin.x0, pin.x1),
            (panel.x0 + 1.0 + GUTTER, panel.x1 - 1.0 - GUTTER),
            "rows fill the panel inside its gutter"
        );
        // Five rows and a line, with the gutter and the ring.
        assert_eq!(
            panel.height(),
            4.0 * ROW_HEIGHT + SEPARATOR_HEIGHT + 2.0 * GUTTER + 2.0
        );

        let greyed = rect(&harness, "menu-row:Rename").center();
        harness.click(greyed.x, greyed.y);
        settle(&mut harness);
        assert!(is_open(), "a greyed row does nothing");
        assert!(log.borrow().is_empty());

        for n in [
            "menu-row:Pin",
            "menu-row:Rename",
            "menu-row:Open in",
            "menu-row:Delete",
        ] {
            eprintln!("DBG {n} {:?}", rect(&harness, n));
        }
        let pin = rect(&harness, "menu-row:Pin").center();
        harness.click(pin.x, pin.y);
        settle(&mut harness);
        assert_eq!(*log.borrow(), vec!["pin"]);
        assert!(!is_open());
        assert!(!shown(&harness, "menu-panel:0"));
    }

    #[test]
    fn a_press_outside_puts_it_away_and_picks_nothing() {
        let (mut harness, log) = window_with((40.0, 30.0), (600.0, 400.0), sample);
        open_it(&mut harness);
        harness.click(500.0, 350.0);
        settle(&mut harness);
        assert!(!is_open());
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn near_the_bottom_right_it_opens_over_the_button_and_inside_the_window() {
        let (mut harness, _) = window_with((560.0, 370.0), (600.0, 400.0), sample);
        open_it(&mut harness);
        let button = rect(&harness, "button");
        let panel = rect(&harness, "menu-panel:0");
        assert_eq!(panel.y1, button.y0 - DROP_GAP, "over it");
        assert_eq!(panel.x1, 600.0 - EDGE, "slid in from the right");
    }

    #[test]
    fn a_row_too_long_for_the_widest_panel_stays_inside_it() {
        let long = "d7766ec  require the observed epoch for conditional acquisition \
                    in every writer and compactor";
        let (mut harness, _) = window_with((40.0, 30.0), (900.0, 400.0), move |_| {
            Menu::new()
                .item("Short", |i| i)
                .item(long, |i| i.detail("3 days ago"))
        });
        open_it(&mut harness);
        let panel = rect(&harness, "menu-panel:0");
        assert_eq!(panel.width(), MAX_WIDTH);
        let row = rect(&harness, &format!("menu-row:{long}"));
        assert_eq!(
            (row.x0, row.x1),
            (panel.x0 + 1.0 + GUTTER, panel.x1 - 1.0 - GUTTER),
            "the row fills the panel and no more"
        );
    }

    #[test]
    fn a_submenu_opens_beside_its_row_and_its_rows_pick() {
        let (mut harness, log) = window_with((40.0, 30.0), (600.0, 400.0), sample);
        open_it(&mut harness);
        let open_in = rect(&harness, "menu-row:Open in");
        harness.pointer_move(open_in.center().x, open_in.center().y);
        settle(&mut harness);
        let sub = rect(&harness, "menu-panel:1");
        let finder = rect(&harness, "menu-row:Finder");
        assert!(sub.x0 > open_in.x1, "to the right");
        assert_eq!(finder.y0, open_in.y0, "its first row level with the row");

        harness.pointer_move(finder.center().x, finder.center().y);
        settle(&mut harness);
        harness.click(finder.center().x, finder.center().y);
        settle(&mut harness);
        assert_eq!(*log.borrow(), vec!["finder"]);
        assert!(!is_open());
    }

    #[test]
    fn the_keyboard_moves_past_greyed_rows_into_submenus_and_out() {
        let (mut harness, log) = window_with((40.0, 30.0), (600.0, 400.0), sample);
        open_it(&mut harness);
        harness.dispatch_event(named_key(Code::ArrowDown, NamedKey::ArrowDown));
        harness.dispatch_event(named_key(Code::ArrowDown, NamedKey::ArrowDown));
        settle(&mut harness);
        // Pin, then past Rename to Open in, which Right opens.
        harness.dispatch_event(named_key(Code::ArrowRight, NamedKey::ArrowRight));
        settle(&mut harness);
        assert!(shown(&harness, "menu-panel:1"));
        harness.dispatch_event(named_key(Code::ArrowLeft, NamedKey::ArrowLeft));
        settle(&mut harness);
        assert!(!shown(&harness, "menu-panel:1"), "Left shuts the submenu");
        assert!(is_open());

        harness.dispatch_event(named_key(Code::ArrowRight, NamedKey::ArrowRight));
        harness.dispatch_event(named_key(Code::ArrowDown, NamedKey::ArrowDown));
        harness.dispatch_event(named_key(Code::Enter, NamedKey::Enter));
        settle(&mut harness);
        assert_eq!(*log.borrow(), vec!["terminal"]);
        assert!(!is_open());
    }

    fn recent(log: Rc<RefCell<Vec<String>>>) -> Menu {
        Menu::new()
            .item("perch", |i| {
                i.action(logs(&log, "perch"))
                    .removable(logs(&log, "x perch"))
            })
            .item("slatedb", |i| {
                i.action(logs(&log, "slatedb"))
                    .removable(logs(&log, "x slatedb"))
            })
            .separator()
            .item("Add…", |i| i.action(logs(&log, "add")))
    }

    #[test]
    fn a_lit_rows_cross_takes_it_out_and_leaves_the_menu_up() {
        let (mut harness, log) = window_with((40.0, 30.0), (600.0, 400.0), recent);
        open_it(&mut harness);
        let first = rect(&harness, "menu-row:perch");
        assert!(
            rect(&harness, "menu-cross:perch").is_zero_area(),
            "hidden until lit"
        );
        harness.pointer_move(first.center().x, first.center().y);
        settle(&mut harness);
        let cross = rect(&harness, "menu-cross:perch");
        assert!(!cross.is_zero_area(), "lit, it shows");
        assert!(
            cross.x1 <= first.x1 && cross.x0 > first.center().x,
            "at the row's end"
        );

        harness.click(cross.center().x, cross.center().y);
        settle(&mut harness);
        assert_eq!(*log.borrow(), vec!["x perch"]);
        assert!(is_open());
        assert_eq!(
            rect(&harness, "menu-row:slatedb").y0,
            first.y0,
            "the next row takes its place"
        );
        assert!(
            !rect(&harness, "menu-cross:slatedb").is_zero_area(),
            "under the pointer now, it is lit"
        );

        // Delete takes out the lit row and lights the next; with nothing
        // left above it, the line goes too.
        harness.dispatch_event(named_key(Code::Backspace, NamedKey::Backspace));
        settle(&mut harness);
        assert_eq!(*log.borrow(), vec!["x perch", "x slatedb"]);
        assert_eq!(rect(&harness, "menu-row:Add…").y0, first.y0);
        harness.dispatch_event(named_key(Code::Enter, NamedKey::Enter));
        settle(&mut harness);
        assert_eq!(*log.borrow(), vec!["x perch", "x slatedb", "add"]);
        assert!(!is_open());
    }

    /// Three people, one checked, at most two of whom may be, behind a
    /// filter.
    fn people(log: Rc<RefCell<Vec<String>>>) -> Menu {
        let toggled = |login: &'static str| {
            let log = log.clone();
            move |on: bool| {
                log.borrow_mut()
                    .push(format!("{login} {}", if on { "on" } else { "off" }))
            }
        };
        Menu::new()
            .filter("Filter people")
            .most_checked(2)
            .item("agavra", |i| {
                i.detail("Almog Gavra")
                    .checked(true)
                    .toggle(toggled("agavra"))
            })
            .item("rohan", |i| {
                i.detail("Rohan Desai").toggle(toggled("rohan"))
            })
            .item("vigneshc", |i| i.toggle(toggled("vigneshc")))
    }

    fn click_row(harness: &mut HeadlessHarness, name: &str) {
        let at = rect(harness, &format!("menu-row:{name}")).center();
        harness.pointer_move(at.x, at.y);
        settle(harness);
        harness.click(at.x, at.y);
        settle(harness);
    }

    fn type_text(harness: &mut HeadlessHarness, text: &str) {
        for c in text.chars() {
            let code = Code::from_str(&format!("Key{}", c.to_ascii_uppercase())).unwrap();
            harness.dispatch_event(key(code, Key::Character(c.to_string())));
        }
        settle(harness);
    }

    #[test]
    fn a_row_that_toggles_flips_in_place_and_a_full_set_greys_the_rest() {
        let (mut harness, log) = window_with((40.0, 30.0), (600.0, 400.0), people);
        open_it(&mut harness);
        assert!(!rect(&harness, "menu-check:agavra").is_zero_area());
        assert!(rect(&harness, "menu-check:rohan").is_zero_area());

        click_row(&mut harness, "rohan");
        assert_eq!(*log.borrow(), vec!["rohan on"]);
        assert!(is_open(), "the menu stays up");
        assert!(
            !rect(&harness, "menu-check:rohan").is_zero_area(),
            "checked now"
        );

        // Two is the most: the third is greyed and does nothing.
        click_row(&mut harness, "vigneshc");
        assert_eq!(*log.borrow(), vec!["rohan on"]);
        click_row(&mut harness, "agavra");
        click_row(&mut harness, "vigneshc");
        assert_eq!(*log.borrow(), vec!["rohan on", "agavra off", "vigneshc on"]);
        assert!(rect(&harness, "menu-check:agavra").is_zero_area());
        assert!(is_open());
        close();
    }

    #[test]
    fn a_filter_keeps_the_rows_with_what_is_typed_and_keeps_the_keyboard() {
        let (mut harness, log) = window_with((40.0, 30.0), (600.0, 400.0), people);
        open_it(&mut harness);
        let filter = named(harness.root_id(), "menu-filter").expect("a box");
        assert!(harness.is_focused(filter), "the box has the keyboard");
        let panel = rect(&harness, "menu-panel:0");
        assert!(rect(&harness, "menu-filter").y1 < rect(&harness, "menu-row:agavra").y0);

        // By name, not only login: Rohan Desai.
        type_text(&mut harness, "desai");
        assert!(rect(&harness, "menu-row:agavra").is_zero_area());
        assert!(rect(&harness, "menu-row:vigneshc").is_zero_area());
        assert!(!rect(&harness, "menu-row:rohan").is_zero_area());
        assert!(rect(&harness, "menu-no-matches").is_zero_area());
        let narrowed = rect(&harness, "menu-panel:0");
        assert_eq!(narrowed.y0, panel.y0, "it shrinks from the bottom");
        assert!(narrowed.height() < panel.height());

        // The first row left is lit: Enter flips it, and the box keeps
        // the keyboard.
        harness.dispatch_event(named_key(Code::Enter, NamedKey::Enter));
        settle(&mut harness);
        assert_eq!(*log.borrow(), vec!["rohan on"]);
        assert!(is_open());
        assert!(harness.is_focused(filter));

        for _ in 0.."desai".len() {
            harness.dispatch_event(named_key(Code::Backspace, NamedKey::Backspace));
        }
        type_text(&mut harness, "zz");
        assert!(rect(&harness, "menu-row:rohan").is_zero_area());
        assert!(!rect(&harness, "menu-no-matches").is_zero_area());

        // The arrows move through the rows left; a click keeps the
        // keyboard in the box.
        harness.dispatch_event(named_key(Code::Backspace, NamedKey::Backspace));
        harness.dispatch_event(named_key(Code::Backspace, NamedKey::Backspace));
        type_text(&mut harness, "a");
        harness.dispatch_event(named_key(Code::ArrowDown, NamedKey::ArrowDown));
        harness.dispatch_event(named_key(Code::Enter, NamedKey::Enter));
        settle(&mut harness);
        assert_eq!(*log.borrow(), vec!["rohan on", "rohan off"]);
        click_row(&mut harness, "agavra");
        assert_eq!(*log.borrow(), vec!["rohan on", "rohan off", "agavra off"]);
        assert!(harness.is_focused(filter));

        harness.dispatch_event(named_key(Code::Escape, NamedKey::Escape));
        settle(&mut harness);
        assert!(!is_open());
    }

    #[test]
    fn a_click_off_the_cross_picks_the_row() {
        let (mut harness, log) = window_with((40.0, 30.0), (600.0, 400.0), recent);
        open_it(&mut harness);
        let row = rect(&harness, "menu-row:slatedb");
        harness.pointer_move(row.x0 + 20.0, row.center().y);
        settle(&mut harness);
        harness.click(row.x0 + 20.0, row.center().y);
        settle(&mut harness);
        assert_eq!(*log.borrow(), vec!["slatedb"]);
        assert!(!is_open());
    }

    #[test]
    fn a_rows_letter_picks_it_and_escape_backs_out() {
        let (mut harness, log) = window_with((40.0, 30.0), (600.0, 400.0), sample);
        open_it(&mut harness);
        harness.dispatch_event(named_key(Code::Escape, NamedKey::Escape));
        settle(&mut harness);
        assert!(!is_open());

        open_it(&mut harness);
        harness.dispatch_event(key(Code::KeyD, Key::Character("d".into())));
        settle(&mut harness);
        assert_eq!(*log.borrow(), vec!["delete"]);
        assert!(!is_open());
    }

    #[test]
    fn enter_on_a_focused_button_opens_it_with_the_first_row_lit() {
        let (mut harness, log) = window_with((40.0, 30.0), (600.0, 400.0), sample);
        let button = named(harness.root_id(), "button").unwrap();
        button.request_focus();
        settle(&mut harness);
        harness.dispatch_event(named_key(Code::Enter, NamedKey::Enter));
        settle(&mut harness);
        assert!(is_open());
        harness.dispatch_event(named_key(Code::Enter, NamedKey::Enter));
        settle(&mut harness);
        assert_eq!(*log.borrow(), vec!["pin"]);
        assert!(harness.is_focused(button), "the keyboard goes back");
    }

    #[test]
    fn a_right_click_opens_it_at_the_pointer() {
        let root = TestRoot::new();
        let log: Rc<RefCell<Vec<String>>> = Rc::default();
        let menu_log = log.clone();
        let page = Empty::new()
            .right_click_menu(move || sample(menu_log.clone()))
            .debug_name("page")
            .style(|s| s.size_full());
        let mut harness = HeadlessHarness::new_with_size(root, page, 600.0, 400.0);
        settle(&mut harness);
        harness.secondary_pointer_down(120.0, 90.0);
        settle(&mut harness);
        let panel = rect(&harness, "menu-panel:0");
        assert_eq!((panel.x0, panel.y0), (120.0, 90.0));
        close();
        settle(&mut harness);

        // Too near the right edge: it opens leftward from the pointer.
        harness.secondary_pointer_down(590.0, 90.0);
        settle(&mut harness);
        let panel = rect(&harness, "menu-panel:0");
        assert_eq!(panel.x1, 590.0);
        close();
    }

    #[test]
    fn a_long_menu_fits_the_window_and_scrolls_to_the_row_picked() {
        let branches = |_: Rc<RefCell<Vec<String>>>| {
            (0..40).fold(Menu::new(), |menu, n| {
                menu.item(format!("branch-{n}"), |i| i)
            })
        };
        let (mut harness, _) = window_with((40.0, 30.0), (600.0, 300.0), branches);
        open_it(&mut harness);
        let panel = rect(&harness, "menu-panel:0");
        assert_eq!(
            (panel.y0, panel.y1),
            (EDGE, 300.0 - EDGE),
            "as tall as the window allows"
        );
        harness.dispatch_event(named_key(Code::End, NamedKey::End));
        settle(&mut harness);
        let last = rect(&harness, "menu-row:branch-39");
        assert!(
            last.y1 <= panel.y1 && last.y0 >= panel.y0,
            "{last:?} in {panel:?}"
        );
        close();
    }

    #[test]
    fn letting_go_where_the_press_was_picks_nothing_the_menu_opened_over() {
        let branches = |log: Rc<RefCell<Vec<String>>>| {
            (0..40).fold(Menu::new(), |menu, n| {
                menu.item(format!("branch-{n}"), |i| i.action(logs(&log, "picked")))
            })
        };
        let (mut harness, log) = window_with((40.0, 30.0), (600.0, 300.0), branches);
        let button = rect(&harness, "button").center();
        harness.pointer_down(button.x, button.y);
        settle(&mut harness);
        std::thread::sleep(RELEASE_GRACE);
        harness.pointer_up(button.x, button.y);
        settle(&mut harness);
        assert!(is_open(), "a slow click still leaves it up");
        assert!(log.borrow().is_empty());
        close();
    }

    #[test]
    fn placement_keeps_a_tall_menu_inside_the_window() {
        let window = Size::new(400.0, 300.0);
        let at = place(
            Anchor::At(Point::new(50.0, 250.0)),
            Size::new(200.0, 120.0),
            window,
        );
        assert_eq!(at, Point::new(50.0, 300.0 - EDGE - 120.0), "slid up");
        let at = place(
            Anchor::Beside(Rect::new(250.0, 40.0, 390.0, 70.0)),
            Size::new(200.0, 100.0),
            window,
        );
        assert_eq!(at.x, 250.0 - GUTTER - 2.0 - 200.0, "flipped to the left");
        let at = place(
            Anchor::Below(Rect::new(10.0, 100.0, 60.0, 120.0)),
            Size::new(200.0, 500.0),
            window,
        );
        assert_eq!(at.y, EDGE, "taller than the window: from the top");
    }
}
