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
use crate::ui::menu::{Menu, MenuExt, Shortcut};
use crate::ui::widgets::{action_button, clickable, sidebar_glyph, DISABLED_ALPHA};

/// How wide a row's dropdown or box is.
pub(crate) const CONTROL_WIDTH: f64 = 220.0;
const CONTROL_HEIGHT: f64 = 30.0;
/// How wide a shortcut's button is: room for two keys, ⌘1, /.
const SHORTCUT_WIDTH: f64 = 140.0;
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
                        s.border_top(theme::DIVIDER_WIDTH)
                            .border_color(theme::border_muted())
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
                    .text_wrap()
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
    let main = Stack::horizontal((words, control)).style(|s| {
        s.items_center()
            .width_full()
            .min_height(60.0)
            .padding_horiz(theme::CARD_PADDING)
            .padding_vert(12.0)
    });
    let content = match &row.preview {
        Some(preview) => Stack::vertical((
            main,
            Container::new(preview()).style(|s| {
                s.width_full()
                    .padding_horiz(theme::CARD_PADDING)
                    .padding_bottom(12.0)
            }),
        ))
        .style(|s| s.width_full())
        .into_any(),
        None => main.into_any(),
    };
    content.debug_name(format!("settings-row:{}", row.name))
}

/// The arrow beside a title that takes the key out of the file, so the
/// setting is back at its default. There while the file sets the key.
fn reset_button(page: Page, reset: super::Reset, name: &str) -> impl IntoView {
    let page = if reset.setting.is_client_s() {
        page.here()
    } else {
        page
    };
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

/// A multiline box for a setting's text, such as a prompt override. What is
/// typed is written when the box loses the keyboard or on Cmd/Ctrl+Enter;
/// Escape puts the file's value back. Until something is typed, the box
/// follows the file, and letting go of it writes nothing.
pub(crate) fn setting_multiline(
    page: &Page,
    value: impl Fn() -> String + 'static,
    placeholder: &str,
    height: f64,
    commit: impl Fn(String) + 'static,
    debug: impl Into<String>,
) -> impl View {
    use floem::views::editor::command::CommandExecuted;
    use floem::views::editor::core::cursor::CursorAffinity;
    use floem::views::editor::core::editor::EditType;
    use floem::views::editor::core::selection::Selection;

    let editing = page.state.settings.editing;
    let value: Rc<dyn Fn() -> String> = Rc::new(value);
    let initial = Effect::untrack(|| value());
    let draft = RwSignal::new(initial.clone());
    let typing = Rc::new(Cell::new(false));
    let base: Rc<RefCell<String>> = Rc::new(RefCell::new(initial.clone()));
    let sent: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    let editor = crate::ui::widgets::prose_editor(initial, placeholder, None);
    let handle = editor.editor().clone();

    let replace_doc = {
        let handle = handle.clone();
        Rc::new(move |next: &str| {
            let doc = handle.doc();
            let current = doc.text().to_string();
            if current == next {
                return;
            }
            let len = doc.text().len();
            doc.edit_single(
                Selection::region(0, len, CursorAffinity::Backward),
                next,
                EditType::Other,
            );
        })
    };

    {
        let (value, base, sent, replace_doc) = (
            value.clone(),
            base.clone(),
            sent.clone(),
            replace_doc.clone(),
        );
        Effect::new(move |_| {
            let now = value();
            let answered = sent.borrow_mut().take().is_some();
            let typed = draft.get_untracked();
            let untouched = typed.trim() == base.borrow().trim();
            if untouched || answered || typed.trim() == now.trim() {
                if typed != now {
                    draft.set(now.clone());
                    replace_doc(&now);
                }
                base.replace(now);
            }
        });
    }

    let commit: Rc<dyn Fn(String)> = Rc::new(commit);
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

    let show_file: Rc<dyn Fn()> = {
        let (value, base, replace_doc) = (value.clone(), base.clone(), replace_doc.clone());
        Rc::new(move || {
            let now = Effect::untrack(|| value());
            if draft.get_untracked() != now {
                draft.set(now.clone());
                replace_doc(&now);
            }
            base.replace(now);
        })
    };

    let focused = RwSignal::new(false);
    let writable = page.clone();
    let box_writable = page.clone();
    let (gained, lost, cleanup) = (typing.clone(), typing.clone(), typing);
    let (send_lost, send_key, send_cleanup) = (send.clone(), send.clone(), send);
    let revert = show_file.clone();

    let id = editor.view_id();
    let editor = editor
        .pre_command(move |cmd| {
            if crate::review::threads::cmd_enter(&cmd) {
                send_key();
                return CommandExecuted::Yes;
            }
            CommandExecuted::No
        })
        .update(move |update| {
            if let Some(ed) = update.editor {
                draft.set(ed.doc().text().to_string());
            }
        })
        .on_event_cont(el::FocusGained, move |_, _| {
            gained.set(true);
            focused.set(true);
            editing.set(Some(id));
        })
        .on_event_cont(el::FocusLost, move |_, _| {
            lost.set(false);
            focused.set(false);
            if editing.get_untracked() == Some(id) {
                editing.set(None);
            }
            if !send_lost() && sent.borrow().is_none() {
                show_file();
            }
        })
        .on_event_with_config(
            el::KeyDown,
            floem::context::EventCallbackConfig {
                phases: floem::context::Phases::CAPTURE,
            },
            move |_, key: &KeyboardEvent| {
                if key.code == Code::Escape && key.modifiers.is_empty() && !key.is_composing {
                    revert();
                    id.clear_focus();
                    EventPropagation::Stop
                } else {
                    EventPropagation::Continue
                }
            },
        )
        .on_cleanup(move || {
            if cleanup.get() {
                if editing.try_get_untracked() == Some(Some(id)) {
                    editing.set(None);
                }
                send_cleanup();
            }
        })
        .style(move |s| {
            s.size_full()
                .font_family(theme::FONT_STACK.to_string())
                .font_size(theme::TEXT_META)
                .set_disabled(!writable.writable())
        })
        .debug_name(debug);

    Container::new(editor).style(move |s| {
        let disabled = !box_writable.writable();
        s.width_full()
            .height(height)
            .padding_horiz(10.0)
            .padding_vert(8.0)
            .background(theme::canvas())
            .border(1.0)
            .border_color(if focused.get() {
                theme::accent()
            } else {
                theme::border()
            })
            .border_radius(theme::RADIUS)
            .apply_if(disabled, |s| s.color(theme::fg_subtle()))
    })
}

/// A one-line box with a button after it, for something to add rather
/// than a setting to change: Enter or the button hands what was typed to
/// `submit`, and letting go of the box sends nothing, so a name half
/// typed is never acted on. Nothing is sent while the page can't be
/// written, or again while `sent` still holds the same words, which the
/// caller clears when it hears back. `draft` is the caller's to keep and
/// to empty: the box may be built again while something is being added.
/// `submit` says whether it sent.
pub(crate) fn entry(
    settings: super::SettingsState,
    enabled: impl Fn() -> bool + Clone + 'static,
    draft: RwSignal<String>,
    sent: RwSignal<Option<String>>,
    placeholder: &str,
    label: &'static str,
    submit: impl Fn(String) -> bool + 'static,
    debug: &'static str,
) -> impl View {
    let send: Rc<dyn Fn()> = {
        let enabled = enabled.clone();
        Rc::new(move || {
            let Some(typed) = draft.try_get_untracked() else {
                return;
            };
            let typed = typed.trim();
            let waiting = sent.with_untracked(|sent| sent.as_deref() == Some(typed));
            if typed.is_empty() || waiting || !enabled() {
                return;
            }
            if submit(typed.to_string()) {
                sent.set(Some(typed.to_string()));
            }
        })
    };
    let input = entry_input(
        settings,
        enabled.clone(),
        draft,
        placeholder,
        {
            let send = send.clone();
            move || send()
        },
        debug,
    );
    Stack::horizontal((
        input,
        button(
            label,
            format!("{debug}-go"),
            move || enabled() && draft.with(|s| !s.trim().is_empty()),
            move || send(),
        ),
    ))
    .style(|s| s.gap(8.0).items_center())
}

/// A field for a form that submits with Enter or a separate button.
pub(crate) fn entry_input(
    settings: super::SettingsState,
    enabled: impl Fn() -> bool + 'static,
    draft: RwSignal<String>,
    placeholder: &str,
    submit: impl Fn() + 'static,
    debug: &'static str,
) -> impl View {
    let editing = settings.editing;
    let input = TextInput::new(draft)
        .placeholder(placeholder.to_string())
        .debug_name(debug);
    let id = input.view_id();
    input
        .on_event_cont(el::FocusGained, move |_, _| editing.set(Some(id)))
        .on_event_cont(el::FocusLost, move |_, _| {
            if editing.get_untracked() == Some(id) {
                editing.set(None);
            }
        })
        .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
            if !key.modifiers.is_empty() || key.is_composing {
                return EventPropagation::Continue;
            }
            match key.code {
                Code::Enter | Code::NumpadEnter => {
                    submit();
                    EventPropagation::Stop
                }
                Code::Escape => {
                    draft.set(String::new());
                    id.clear_focus();
                    EventPropagation::Stop
                }
                _ => EventPropagation::Continue,
            }
        })
        .on_cleanup(move || {
            if editing.try_get_untracked() == Some(Some(id)) {
                editing.set(None);
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
                .set_disabled(!enabled())
        })
}

/// The keys macOS keeps for itself: Quit, Hide and Hide Others, which the
/// app menu holds, and the app switcher. They never reach Perch.
const TAKEN: [&str; 4] = ["super+q", "super+h", "alt+super+h", "super+tab"];

/// Keys macOS apps leave to macOS, Minimize and the next window. Perch
/// hears them, but the button refuses them, as a reader expects them to
/// do what they do everywhere.
const LEFT_TO_MACOS: [&str; 2] = ["super+m", "super+`"];

fn one_of(list: &[&str], keys: Shortcut) -> bool {
    cfg!(target_os = "macos") && list.iter().any(|r| Shortcut::parse(r) == Some(keys))
}

/// Whether macOS takes `keys` before Perch hears them, so an action
/// bound to them by hand never runs.
pub(crate) fn taken(keys: Shortcut) -> bool {
    one_of(&TAKEN, keys)
}

/// Whether the button refuses `keys` as macOS's own.
fn reserved(keys: Shortcut) -> bool {
    taken(keys) || one_of(&LEFT_TO_MACOS, keys)
}

/// Why `keys` can't be a shortcut, or None when it can.
fn refused(keys: Shortcut) -> Option<String> {
    if reserved(keys) {
        return Some(format!("macOS keeps {} for itself", keys.label()));
    }
    // Off macOS the logo key has no name in config.toml.
    if Shortcut::parse(&keys.to_config()) != Some(keys) {
        return Some(format!("Perch can't write {}", keys.label()));
    }
    None
}

/// Whether `code` is a modifier on its own, which waits for the key it
/// goes with.
fn modifier_only(code: Code) -> bool {
    matches!(
        code,
        Code::ShiftLeft
            | Code::ShiftRight
            | Code::ControlLeft
            | Code::ControlRight
            | Code::AltLeft
            | Code::AltRight
            | Code::MetaLeft
            | Code::MetaRight
            | Code::Fn
            | Code::FnLock
            | Code::CapsLock
            | Code::NumLock
    )
}

/// Whether `key` presses a button that has the keyboard: Enter or Space
/// going down, with no modifiers, and not repeating. floem sends the
/// button a Click as well as the key comes up, and as it repeats, which
/// the button ignores, so a press is heard once.
fn activates(key: &KeyboardEvent) -> bool {
    matches!(key.code, Code::Enter | Code::NumpadEnter | Code::Space)
        && key.modifiers.is_empty()
        && !key.repeat
}

/// Whether a Click is the one floem sends for Enter or Space.
fn by_keyboard(cx: &floem::context::EventCx<'_>) -> bool {
    cx.triggered_by.is_some_and(|e| e.is_keyboard_trigger())
}

/// A shortcut's keys, as a button that shows them, or "None". A click,
/// Enter or Space sets it waiting for keys; the next key pressed with its
/// modifiers, Tab and Cmd-W included, goes to `write`, and Escape on its
/// own stops waiting with no change. So does the keyboard going
/// elsewhere. While it waits, the Perch menu lets its keys through, so
/// Cmd-O is a key for it rather than Open…. A key macOS keeps is refused
/// with a note under the button, and the button goes on waiting; Cmd-Q,
/// Cmd-H, Cmd-Opt-H and Cmd-Tab never reach it, as macOS has them first.
/// The key written, held down, repeats into `held` rather than the page.
/// The × beside it takes every key away.
pub(crate) fn shortcut(
    settings: super::SettingsState,
    keys: impl Fn() -> Vec<Shortcut> + 'static,
    enabled: impl Fn() -> bool + 'static,
    write: impl Fn(Option<Shortcut>) + 'static,
    debug: &str,
) -> AnyView {
    let (capturing, held) = (settings.capturing, settings.held);
    let keys: Rc<dyn Fn() -> Vec<Shortcut>> = Rc::new(keys);
    let enabled: Rc<dyn Fn() -> bool> = Rc::new(enabled);
    let write: Rc<dyn Fn(Option<Shortcut>)> = Rc::new(write);
    let note: RwSignal<Option<String>> = RwSignal::new(None);
    // The button's id, for its label to know when the wait is its; set
    // as soon as the button is made.
    let own: Rc<Cell<Option<floem::ViewId>>> = Rc::new(Cell::new(None));
    let label_own = own.clone();
    let label_keys = keys.clone();
    let label = Label::derived(move || {
        let keys = label_keys();
        let now = capturing.get();
        if now.is_some() && now == label_own.get() {
            "Press keys…".to_string()
        } else if keys.is_empty() {
            "None".to_string()
        } else {
            keys.iter()
                .map(Shortcut::label)
                .collect::<Vec<_>>()
                .join(", ")
        }
    });
    let label = label.style(|s| s.text_ellipsis().min_width(0.0).pointer_events_none());
    let button = Container::new(label);
    let id = button.view_id();
    own.set(Some(id));
    let waiting = move || capturing.get() == Some(id);
    // Stops the wait. The menu has its keys back unless a key just given
    // is still down, which has them back as it comes up.
    let stop: Rc<dyn Fn()> = Rc::new(move || {
        if capturing.try_get_untracked() == Some(Some(id)) {
            capturing.set(None);
        }
        if note.try_get_untracked().flatten().is_some() {
            note.set(None);
        }
        if held.try_get_untracked().flatten().is_none() {
            settings.let_menu_keys_back();
        }
    });
    let start: Rc<dyn Fn()> = {
        let enabled = enabled.clone();
        Rc::new(move || {
            if enabled() {
                note.set(None);
                capturing.set(Some(id));
                id.request_focus();
                if settings.menu_keys.with_untracked(Option::is_none) {
                    settings
                        .menu_keys
                        .set(Some(crate::app_menu::let_keys_through()));
                }
            }
        })
    };
    // A button that can't be pressed hears no keys, Escape included, so a
    // wait it can no longer end ends as it stops being enabled: when
    // config.toml no longer reads, say.
    {
        let (enabled, stop) = (enabled.clone(), stop.clone());
        Effect::new(move |_| {
            if !enabled() && capturing.get_untracked() == Some(id) {
                stop();
            }
        });
    }
    let click = start.clone();
    let (click_stop, key_stop, lost_stop) = (stop.clone(), stop.clone(), stop.clone());
    let key_write = write.clone();
    let style_enabled = enabled.clone();
    let button = button
        .on_event_stop(el::Click, move |cx, _| {
            // Enter or Space did what it does as it went down.
            if by_keyboard(cx) {
                return;
            }
            if capturing.get_untracked() == Some(id) {
                click_stop();
            } else {
                click();
            }
        })
        .on_event(el::KeyDown, move |cx, key: &KeyboardEvent| {
            if capturing.get_untracked() != Some(id) {
                if activates(key) {
                    start();
                    return EventPropagation::Stop;
                }
                return EventPropagation::Continue;
            }
            // Every key is the button's while it waits, Tab included. A
            // key repeating is the one that started the wait, still down.
            cx.prevent_default();
            if key.is_composing || key.repeat || modifier_only(key.code) {
                return EventPropagation::Stop;
            }
            let modifiers = key.modifiers
                & (Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT | Modifiers::META);
            if key.code == Code::Escape && modifiers.is_empty() {
                key_stop();
                return EventPropagation::Stop;
            }
            let keys = Shortcut {
                modifiers,
                code: key.code,
            };
            if let Some(why) = refused(keys) {
                note.set(Some(why));
                return EventPropagation::Stop;
            }
            held.set(Some(key.code));
            key_stop();
            key_write(Some(keys));
            EventPropagation::Stop
        })
        .on_event_cont(el::FocusLost, move |_, _| lost_stop())
        // A key given and still down as the button goes, as search rows
        // can go with a write, is let go by the window as it comes up.
        .on_cleanup(move || stop())
        .debug_name(format!("settings-shortcut:{debug}"))
        .style(move |s| {
            let available = style_enabled();
            let waiting = waiting();
            let color = if waiting {
                theme::fg_muted()
            } else {
                theme::fg()
            };
            clickable(s)
                .items_center()
                .width(SHORTCUT_WIDTH)
                .height(CONTROL_HEIGHT)
                .padding_horiz(10.0)
                .font_size(theme::TEXT_META)
                .color(if available {
                    color
                } else {
                    color.with_alpha(DISABLED_ALPHA)
                })
                .background(theme::canvas())
                .border(1.0)
                .border_color(if waiting {
                    theme::accent()
                } else {
                    theme::border()
                })
                .border_radius(theme::RADIUS)
                .keyboard_navigable()
                .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
                .set_disabled(!available)
                .apply_if(!available, |s| s.cursor(floem::style::CursorStyle::Default))
                .apply_if(available, |s| {
                    s.hover(|s| s.background(theme::canvas_hover()))
                })
        });
    let clear_keys = keys.clone();
    let clear_enabled = enabled.clone();
    let clear_write = write;
    let clear_action = move || {
        if clear_enabled() {
            clear_write(None);
        }
    };
    let clear_key = clear_action.clone();
    let clear = Container::new(
        sidebar_glyph(SidebarIcon::Close, theme::fg_subtle)
            .style(|s| s.size(12.0, 12.0).pointer_events_none()),
    )
    .on_event_stop(el::Click, move |cx, _| {
        if !by_keyboard(cx) {
            clear_action();
        }
    })
    .on_event(el::KeyDown, move |_, key: &KeyboardEvent| {
        if activates(key) {
            clear_key();
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    })
    .debug_name(format!("settings-shortcut-clear:{debug}"))
    .style(move |s| {
        let shown = enabled() && !clear_keys().is_empty();
        clickable(s)
            .size(22.0, 22.0)
            .flex_shrink(0.0_f32)
            .items_center()
            .justify_center()
            .border_radius(theme::RADIUS)
            .keyboard_navigable()
            .focus_visible(|s| s.outline(2.0).outline_color(theme::accent_ring()))
            .hover(|s| s.background(theme::canvas_hover()))
            .apply_if(!shown, |s| s.hide())
    });
    let note_label = Label::derived(move || note.get().unwrap_or_default())
        .debug_name(format!("settings-shortcut-note:{debug}"))
        .style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(theme::notice_fg())
                .apply_if(note.with(Option::is_none), |s| s.hide())
        });
    Stack::vertical((
        Stack::horizontal((tip(clear, "Remove the keys".to_string()), button))
            .style(|s| s.items_center().gap(6.0)),
        note_label,
    ))
    .style(|s| s.flex_col().items_end().gap(4.0))
    .into_any()
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
                if crate::windows::engine_settings_heard(page.state) {
                    crate::ui::widgets::open_in_editor(page.state, &path);
                } else {
                    crate::ui::widgets::open_in_editor_here(&path);
                }
            }
        },
    )
}
