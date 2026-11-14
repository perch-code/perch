//! The page's pieces: a group's heading and card, a row, and the controls
//! a row ends in, a switch, a dropdown and a one-line box.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use floem::event::EventPropagation;
use floem::peniko::Brush;
use floem::prelude::*;
use floem::reactive::{Effect, RwSignal, SignalGet, SignalUpdate};
use floem::style::Position;
use floem::views::editor::SelectionColor;
use floem::views::{Container, Decorators, Empty, Label, PlaceholderTextClass, Stack, TextInput};
use floem::AnyView;
use perch_github::Setting;

use super::{Page, Row};
use crate::icons::SidebarIcon;
use crate::review::cards::tip;
use crate::theme;
use crate::ui::menu::{Menu, MenuExt};
use crate::ui::widgets::{action_button, clickable, sidebar_glyph, DISABLED_ALPHA};

/// How wide a row's dropdown or box is.
pub(crate) const CONTROL_WIDTH: f64 = 220.0;
const CONTROL_HEIGHT: f64 = 30.0;
const SWITCH_WIDTH: f64 = 36.0;
const SWITCH_HEIGHT: f64 = 20.0;
const KNOB: f64 = 16.0;

/// A group: its heading, a line under it if it has one, then its rows in
/// one card. Grey, as everything that is Perch's own is.
pub(crate) fn group(title: &str, subtitle: Option<&str>, rows: Vec<AnyView>) -> impl IntoView {
    let subtitle = subtitle.map(str::to_string);
    let has_subtitle = subtitle.is_some();
    let rows: Vec<AnyView> = rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            Container::new(row)
                .style(move |s| {
                    s.width_full().apply_if(index > 0, |s| {
                        s.border_top(1.0).border_color(theme::border_muted())
                    })
                })
                .into_any()
        })
        .collect();
    Stack::vertical((
        Label::new(title.to_string()).style(|s| {
            s.font_size(theme::TEXT_SECTION_TITLE)
                .font_weight(theme::weight_strong())
                .color(theme::fg())
        }),
        Label::new(subtitle.unwrap_or_default()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .margin_top(2.0)
                .text_wrap()
                .width_full()
                .apply_if(!has_subtitle, |s| s.hide())
        }),
        Stack::vertical_from_iter(rows)
            .debug_name("settings-card")
            .style(|s| {
                s.flex_col()
                    .width_full()
                    .margin_top(10.0)
                    .background(theme::perch_bg())
                    .border(1.0)
                    .border_color(theme::border())
                    .border_radius(theme::CARD_RADIUS)
            }),
    ))
    .debug_name(format!("settings-group:{title}"))
    .style(|s| s.flex_col().width_full())
}

/// A row: its title, with the reset arrow while the file sets its key,
/// the description under it, a note while it waits for a restart, and
/// the control at the end.
pub(crate) fn row(page: &Page, row: &Row) -> AnyView {
    let description = row.description.clone();
    let shown = description.clone();
    let reset = match row.reset.clone() {
        Some(reset) => reset_button(page.clone(), reset, &row.name).into_any(),
        None => Empty::new().style(|s| s.hide()).into_any(),
    };
    let pending = row.restart.clone();
    let words = Stack::vertical((
        Stack::horizontal((
            Label::new(row.title.clone()).style(|s| {
                s.font_size(theme::TEXT_BODY)
                    .font_weight(theme::weight_emphasis())
                    .color(theme::fg())
                    .min_width(0.0)
            }),
            reset,
        ))
        .style(|s| s.items_center().gap(6.0)),
        Label::derived(move || description()).style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(theme::fg_muted())
                .text_wrap()
                .width_full()
                .min_width(0.0)
                .apply_if(shown().is_empty(), |s| s.hide())
        }),
        Label::new("Applies after a restart")
            .debug_name(format!("settings-restart-note:{}", row.name))
            .style(move |s| {
                let waiting = pending.as_ref().is_some_and(|pending| pending());
                s.font_size(theme::TEXT_META)
                    .color(theme::notice_fg())
                    .apply_if(!waiting, |s| s.hide())
            }),
    ))
    .style(|s| {
        s.flex_col()
            .gap(3.0)
            .flex_grow(1.0_f32)
            .flex_basis(0.0)
            .min_width(0.0)
    });
    let control = Container::new((row.control)()).style(|s| {
        s.flex_shrink(0.0_f32)
            .margin_left(24.0)
            .items_center()
            .max_width_pct(55.0)
    });
    Stack::horizontal((words, control))
        .debug_name(format!("settings-row:{}", row.name))
        .style(|s| {
            s.items_center()
                .width_full()
                .min_height(60.0)
                .padding_horiz(theme::CARD_PADDING)
                .padding_vert(12.0)
        })
        .into_any()
}

/// The arrow beside a title that takes the key out of the file, so the
/// setting is back at its default. There while the file sets the key.
fn reset_button(page: Page, reset: super::Reset, name: &str) -> impl IntoView {
    let key = reset.key.clone();
    let shown = {
        let page = page.clone();
        move || page.writable() && page.present(&key)
    };
    let button = Container::new(
        sidebar_glyph(SidebarIcon::Undo, theme::fg_subtle)
            .style(|s| s.size(13.0, 13.0).pointer_events_none()),
    )
    .on_event_stop(el::Click, move |_, _| page.write(reset.setting.clone()))
    .debug_name(format!("settings-reset:{name}"))
    .style(move |s| {
        clickable(s)
            .size(20.0, 20.0)
            .items_center()
            .justify_center()
            .border_radius(theme::RADIUS)
            .hover(|s| s.background(theme::canvas_hover()))
            .apply_if(!shown(), |s| s.hide())
    });
    tip(button, "Reset to the default".to_string())
}

/// A switch, as T3 Code and macOS draw one: the knob to the right and
/// the track filled while it is on. A click flips it, and so do Space and
/// Enter once it has the keyboard.
pub(crate) fn toggle(
    on: impl Fn() -> bool + 'static,
    enabled: impl Fn() -> bool + 'static,
    set: impl Fn(bool) + 'static,
    debug: impl Into<String>,
) -> impl View {
    let on: Rc<dyn Fn() -> bool> = Rc::new(on);
    let enabled: Rc<dyn Fn() -> bool> = Rc::new(enabled);
    let flip: Rc<dyn Fn()> = {
        let (on, enabled) = (on.clone(), enabled.clone());
        Rc::new(move || {
            if enabled() {
                set(!on());
            }
        })
    };
    let knob_on = on.clone();
    let knob = Empty::new().style(move |s| {
        let left = if knob_on() {
            SWITCH_WIDTH - KNOB - 2.0
        } else {
            2.0
        };
        s.position(Position::Absolute)
            .inset_top((SWITCH_HEIGHT - KNOB) / 2.0)
            .inset_left(left)
            .size(KNOB, KNOB)
            .border_radius(KNOB / 2.0)
            .background(theme::canvas())
            .pointer_events_none()
    });
    let click = flip.clone();
    let style_enabled = enabled.clone();
    Container::new(knob)
        .on_event_stop(el::Click, move |_, _| click())
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if matches!(key.code, Code::Space | Code::Enter) && key.modifiers.is_empty() {
                flip();
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .debug_name(debug)
        .style(move |s| {
            let available = style_enabled();
            let track = if on() {
                theme::accent()
            } else {
                theme::border()
            };
            clickable(s)
                .position(Position::Relative)
                .size(SWITCH_WIDTH, SWITCH_HEIGHT)
                .flex_shrink(0.0_f32)
                .border_radius(SWITCH_HEIGHT / 2.0)
                .background(if available {
                    track
                } else {
                    track.with_alpha(DISABLED_ALPHA)
                })
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
                .set_disabled(!available)
                .apply_if(!available, |s| s.cursor(floem::style::CursorStyle::Default))
        })
}

/// A dropdown showing `label`, whose menu is `menu`. While `enabled` says
/// no it is faded and opens nothing.
pub(crate) fn select(
    label: impl Fn() -> String + 'static,
    menu: impl Fn() -> Menu + 'static,
    enabled: impl Fn() -> bool + 'static,
    debug: impl Into<String>,
) -> impl View {
    let enabled: Rc<dyn Fn() -> bool> = Rc::new(enabled);
    let (menu_enabled, label_enabled, style_enabled) =
        (enabled.clone(), enabled.clone(), enabled.clone());
    Stack::horizontal((
        Label::derived(label).style(move |s| {
            let color = theme::fg();
            s.font_size(theme::TEXT_META)
                .color(if label_enabled() {
                    color
                } else {
                    color.with_alpha(DISABLED_ALPHA)
                })
                .selectable(false)
                .pointer_events_none()
                .text_ellipsis()
                .min_width(0.0)
                .flex_grow(1.0_f32)
                .margin_right(6.0)
        }),
        sidebar_glyph(SidebarIcon::ChevronDown, theme::fg_subtle)
            .style(|s| s.size(9.0, 9.0).flex_shrink(0.0_f32).pointer_events_none()),
    ))
    .dropdown(move || if menu_enabled() { menu() } else { Menu::new() })
    .debug_name(debug)
    .style(move |s| {
        let available = style_enabled();
        clickable(s)
            .items_center()
            .width(CONTROL_WIDTH)
            .max_width_full()
            .height(CONTROL_HEIGHT)
            .padding_horiz(10.0)
            .background(theme::canvas())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
            .set_disabled(!available)
            .apply_if(!available, |s| s.cursor(floem::style::CursorStyle::Default))
            .apply_if(available, |s| {
                s.hover(|s| s.background(theme::canvas_hover()))
            })
    })
}

/// A menu of `choices`, the one equal to `current` checked, that hands the
/// one picked to `pick`.
pub(crate) fn choices<T: Clone + PartialEq + 'static>(
    choices: Vec<(String, T)>,
    current: &T,
    pick: Rc<dyn Fn(T)>,
) -> Menu {
    choices
        .into_iter()
        .fold(Menu::new(), |menu, (label, value)| {
            let checked = value == *current;
            let pick = pick.clone();
            menu.item(label, move |item| {
                item.checked(checked).action(move || pick(value.clone()))
            })
        })
}

/// A dropdown over what `offered` gives from the file: the choices, each
/// with its label, and the one the file holds now. Picking one writes the
/// setting `setting` makes of it.
pub(crate) fn dropdown<T: Clone + PartialEq + 'static>(
    page: &Page,
    offered: impl Fn(&Page) -> (Vec<(String, T)>, T) + 'static,
    setting: impl Fn(T) -> Setting + 'static,
    debug: impl Into<String>,
) -> AnyView {
    let debug: String = debug.into();
    let offered = Rc::new(offered);
    let setting: Rc<dyn Fn(T) -> Setting> = Rc::new(setting);
    let (label_page, menu_page, enabled) = (page.clone(), page.clone(), page.clone());
    let label_offered = offered.clone();
    select(
        move || {
            let (choices, current) = label_offered(&label_page);
            choices
                .into_iter()
                .find(|(_, value)| *value == current)
                .map(|(label, _)| label)
                .unwrap_or_default()
        },
        move || {
            let (offered, current) = offered(&menu_page);
            let (write, setting) = (menu_page.clone(), setting.clone());
            choices(
                offered,
                &current,
                Rc::new(move |value| write.write(setting(value))),
            )
        },
        move || enabled.writable(),
        debug,
    )
    .into_any()
}

/// A list as one line, between commas.
pub(crate) fn list_line(items: &[String]) -> String {
    items.join(", ")
}

/// A line of names between commas as a list, blank ones left out.
pub(crate) fn split_list(line: &str) -> Vec<String> {
    line.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// A one-line box for a setting's text. What is typed is written when
/// Enter is pressed and when the box loses the keyboard; Escape puts the
/// file's value back. Until something is typed, the box follows the file,
/// with the keyboard in it or not, and letting go of it writes nothing.
pub(crate) fn setting_text(
    page: &Page,
    value: impl Fn() -> String + 'static,
    placeholder: &str,
    commit: impl Fn(String) + 'static,
    debug: impl Into<String>,
) -> impl View {
    let editing = page.state.settings.editing;
    let value: Rc<dyn Fn() -> String> = Rc::new(value);
    let draft = RwSignal::new(Effect::untrack(|| value()));
    let typing = Rc::new(Cell::new(false));
    // The file's value the box last showed. The box has been typed in
    // when what it holds is something else.
    let base: Rc<RefCell<String>> = Rc::new(RefCell::new(draft.get_untracked()));
    // What was last sent and not yet answered, so Enter and then letting
    // go of the box before the file comes back write it once.
    let sent: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    // Takes the file's value when nothing was typed over it, or when what
    // was typed is now the file's. Every Settings that comes answers the
    // last write: it landed, or it was refused and the notice said so, and
    // either way the box shows the file again.
    {
        let (value, base, sent) = (value.clone(), base.clone(), sent.clone());
        Effect::new(move |_| {
            let now = value();
            let answered = sent.borrow_mut().take().is_some();
            let typed = draft.get_untracked();
            let untouched = typed.trim() == base.borrow().trim();
            if untouched || answered || typed.trim() == now.trim() {
                if typed != now {
                    draft.set(now.clone());
                }
                base.replace(now);
            }
        });
    }
    let commit: Rc<dyn Fn(String)> = Rc::new(commit);
    // Write what was typed, when something was, it isn't what the file
    // holds, and it isn't on its way already. Says whether it wrote.
    let send: Rc<dyn Fn() -> bool> = {
        let (value, base, sent) = (value.clone(), base.clone(), sent.clone());
        Rc::new(move || {
            let Some(typed) = draft.try_get_untracked() else {
                return false;
            };
            let trimmed = typed.trim();
            let edited = trimmed != base.borrow().trim();
            let again = sent.borrow().as_deref() == Some(trimmed);
            if edited && !again && trimmed != Effect::untrack(|| value()).trim() {
                sent.replace(Some(trimmed.to_string()));
                commit(typed);
                return true;
            }
            false
        })
    };
    // Show the file's value again, as when the box is let go of with
    // nothing to write.
    let show_file: Rc<dyn Fn()> = {
        let (value, base) = (value.clone(), base.clone());
        Rc::new(move || {
            let now = Effect::untrack(|| value());
            if draft.get_untracked() != now {
                draft.set(now.clone());
            }
            base.replace(now);
        })
    };
    let input = TextInput::new(draft)
        .placeholder(placeholder.to_string())
        .debug_name(debug);
    let id = input.view_id();
    let writable = page.clone();
    let (gained, lost, cleanup) = (typing.clone(), typing.clone(), typing);
    let (send_lost, send_key, send_cleanup) = (send.clone(), send.clone(), send);
    let revert = show_file.clone();
    input
        .on_event_cont(el::FocusGained, move |_, _| {
            gained.set(true);
            editing.set(Some(id));
        })
        .on_event_cont(el::FocusLost, move |_, _| {
            lost.set(false);
            if editing.get_untracked() == Some(id) {
                editing.set(None);
            }
            if !send_lost() && sent.borrow().is_none() {
                show_file();
            }
        })
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if !key.modifiers.is_empty() || key.is_composing {
                return EventPropagation::Continue;
            }
            match key.code {
                Code::Enter | Code::NumpadEnter => {
                    send_key();
                    EventPropagation::Stop
                }
                Code::Escape => {
                    revert();
                    id.clear_focus();
                    EventPropagation::Stop
                }
                _ => EventPropagation::Continue,
            }
        })
        // A page shut with the keyboard in the box, by Cmd-W say, takes
        // the box away without its losing the keyboard first.
        .on_cleanup(move || {
            if cleanup.get() {
                if editing.try_get_untracked() == Some(Some(id)) {
                    editing.set(None);
                }
                send_cleanup();
            }
        })
        .style(move |s| {
            s.width(CONTROL_WIDTH)
                .max_width_full()
                .height(CONTROL_HEIGHT)
                .font_size(theme::TEXT_META)
                .padding_horiz(10.0)
                .background(theme::canvas())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(theme::RADIUS)
                .focus(|s| s.border_color(theme::accent()))
                .set(SelectionColor, Brush::Solid(theme::selection_bg()))
                .class(PlaceholderTextClass, |s| s.color(theme::fg_subtle()))
                .set_disabled(!writable.writable())
        })
}

/// A small outlined button, for a row's actions.
pub(crate) fn button(
    label: &'static str,
    debug: impl Into<String>,
    enabled: impl Fn() -> bool + 'static,
    action: impl Fn() + 'static,
) -> impl View {
    action_button(move || label.to_string(), debug, false, enabled, action)
}

/// "Open in editor": config.toml in the reader's text editor.
pub(crate) fn open_in_editor_button(page: Page, debug: &'static str) -> impl View {
    button(
        "Open in editor",
        debug,
        || true,
        move || {
            if let Some(path) = page.path() {
                crate::ui::widgets::open_in_editor(&path);
            }
        },
    )
}

/// A path as the reader would type it, with the home folder as `~`.
pub(crate) fn tilde(path: &std::path::Path) -> String {
    match path.strip_prefix(perch_github::config::home_dir()) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}
