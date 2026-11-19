//! Appearance: colors and how big things are drawn. Colors change at
//! once. The zoom and the code font wait for a restart, since
//! Perch sets them up as it starts.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Memo, SignalGet};
use floem::views::{ClipExt, Decorators, Label, Stack};
use perch_github::config::{Appearance, Theme};
use perch_github::Setting;

use super::controls::{choices, dropdown, select, setting_text};
use super::{Group, Page, Row};
use crate::icons::SidebarIcon;
use crate::theme;
use crate::ui::widgets::sidebar_glyph;

/// The zooms offered.
const ZOOMS: [f64; 8] = [0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

pub(super) fn groups(page: &Page) -> Vec<Group> {
    let page = &page.here();
    let mut display = vec![zoom(page), code_font_size(page)];
    // macOS draws code in SF Mono, whatever the file says.
    if !cfg!(target_os = "macos") {
        display.push(code_font(page));
    }
    vec![
        Group::new("Colors", vec![appearance(page), theme_choice(page)]),
        Group::new("Display", display),
    ]
}

pub(super) fn appearance_label(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::System => "System",
        Appearance::Light => "Light",
        Appearance::Dark => "Dark",
    }
}

pub(super) fn zoom_label(zoom: f64) -> String {
    format!("{}%", (zoom * 100.0).round())
}

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn appearance(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            let (label_page, menu_page, enabled) = (page.clone(), page.clone(), page.clone());
            select(
                move || label_page.with_config(|c| appearance_label(c.ui.appearance).to_string()),
                move || {
                    let current = menu_page.with_config(|c| c.ui.appearance);
                    let write = menu_page.clone();
                    choices(
                        Appearance::ALL
                            .into_iter()
                            .map(|a| (appearance_label(a).to_string(), a))
                            .collect(),
                        &current,
                        Rc::new(move |a| write.write(Setting::Appearance(Some(a)))),
                    )
                },
                move || enabled.writable(),
                "settings-appearance",
            )
            .into_any()
        }
    };
    Row::new(
        "appearance",
        "Appearance",
        || "Light or dark, or as the system has it.".to_string(),
        control,
    )
    .keywords(&["dark mode", "light mode", "colour", "color"])
    .reset("ui.appearance", Setting::Appearance(None))
}

fn theme_choice(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            dropdown(
                &page,
                |page| {
                    (
                        Theme::ALL
                            .into_iter()
                            .map(|theme| (theme.label().to_string(), theme))
                            .collect(),
                        page.with_config(|c| c.ui.theme),
                    )
                },
                |theme| Setting::Theme(Some(theme)),
                "settings-theme",
            )
        }
    };
    let preview_page = page.clone();
    Row::new(
        "theme",
        "Theme",
        || "The palette for both light and dark appearance.".to_string(),
        control,
    )
    .keywords(&["ui.theme", "classic", "warm linen", "blue mist"])
    .reset("ui.theme", Setting::Theme(None))
    .preview(move || theme_preview(preview_page.clone()).into_any())
}

/// The same small review in every theme and appearance.
fn theme_preview(page: Page) -> impl View {
    let selected = Memo::new(move |_| page.with_config(|c| c.ui.theme));
    let colors = move || theme::colors(selected.get(), theme::appearance());
    let files = Stack::vertical((
        Label::new("Files").style(move |s| {
            s.font_size(theme::TEXT_META)
                .color(colors().nav_fg)
                .padding_horiz(6.0)
        }),
        Stack::horizontal((
            sidebar_glyph(SidebarIcon::Suggest, move || colors().nav_fg)
                .style(|s| s.size(12.0, 12.0).flex_shrink(0.0_f32)),
            Label::new("poll.rs").style(move |s| {
                s.font_size(11.0)
                    .font_family(theme::CODE_FONT_STACK.to_string())
                    .color(colors().nav_fg)
                    .min_width(0.0)
                    .text_ellipsis()
            }),
        ))
        .style(move |s| {
            s.items_center()
                .gap(4.0)
                .padding_horiz(6.0)
                .padding_vert(4.0)
                .border_radius(4.0)
                .background(colors().nav_selected)
        }),
    ))
    .style(move |s| {
        s.width_pct(26.0)
            .max_width(120.0)
            .padding(8.0)
            .gap(6.0)
            .background(colors().nav_canvas)
            .border_right(theme::DIVIDER_WIDTH)
            .border_color(colors().border)
    });
    let diff_line = |number: &'static str, change: &'static str, code: &'static str| {
        Stack::horizontal((
            Label::new(number)
                .style(move |s| s.color(colors().fg_muted).width(16.0).flex_shrink(0.0_f32)),
            Label::new(change).style(|s| s.width(10.0).flex_shrink(0.0_f32)),
            Label::new(code).style(|s| s.min_width(0.0).text_ellipsis()),
        ))
        .style(move |s| {
            let palette = colors();
            let (background, rule, ink) = match change {
                "+" => (
                    palette.diff_added_bg,
                    palette.diff_added_bar,
                    palette.syntax_default,
                ),
                "−" => (
                    palette.diff_deleted_bg,
                    palette.diff_deleted_bar,
                    palette.diff_deleted_fg,
                ),
                _ => (palette.code_bg, palette.code_bg, palette.syntax_default),
            };
            s.width_full()
                .items_center()
                .gap(5.0)
                .font_family(theme::CODE_FONT_STACK.to_string())
                .font_size(11.0)
                .padding_horiz(8.0)
                .padding_vert(2.0)
                .border_left(1.0)
                .border_color(rule)
                .color(ink)
                .background(background)
        })
    };
    let comment = Stack::horizontal((
        sidebar_glyph(SidebarIcon::Comment, move || colors().fg_muted)
            .style(|s| s.size(12.0, 12.0).flex_shrink(0.0_f32)),
        Label::new("Is 30 seconds enough for a retry?")
            .style(|s| s.font_size(theme::TEXT_META).text_wrap().min_width(0.0)),
    ))
    .style(move |s| {
        s.items_center()
            .gap(6.0)
            .margin(8.0)
            .padding_vert(6.0)
            .padding_horiz(8.0)
            .border_radius(4.0)
            .background(colors().review_comment_bg)
            .color(colors().fg)
    });
    let editor = Stack::vertical((
        diff_line("8", "", "fn poll() {"),
        diff_line("9", "−", "    let delay = 60;"),
        diff_line("9", "+", "    let delay = 30;"),
        comment,
    ))
    .style(move |s| {
        s.flex_grow(1.0_f32)
            .flex_basis(0.0)
            .min_width(0.0)
            .padding_top(6.0)
            .background(colors().code_bg)
    });
    Stack::horizontal((files, editor))
        .style(|s| s.width_full())
        .clip()
        .debug_name("settings-theme-preview")
        .style(move |s| {
            s.width_full()
                .border(1.0)
                .border_color(colors().border)
                .border_radius(6.0)
                .background(colors().canvas)
        })
}

fn zoom(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            let (label_page, menu_page, enabled) = (page.clone(), page.clone(), page.clone());
            select(
                move || label_page.with_config(|c| zoom_label(c.display.zoom)),
                move || {
                    let current = menu_page.with_config(|c| c.display.zoom);
                    let mut offered = ZOOMS.to_vec();
                    if !offered.iter().any(|z| same(*z, current)) {
                        offered.push(current);
                        offered.sort_by(f64::total_cmp);
                    }
                    let write = menu_page.clone();
                    let choice = offered
                        .iter()
                        .copied()
                        .find(|z| same(*z, current))
                        .unwrap_or(current);
                    choices(
                        offered.into_iter().map(|z| (zoom_label(z), z)).collect(),
                        &choice,
                        Rc::new(move |z| write.write(Setting::Zoom(Some(z)))),
                    )
                },
                move || enabled.writable(),
                "settings-zoom",
            )
            .into_any()
        }
    };
    let pending = page.clone();
    Row::new(
        "zoom",
        "Zoom",
        || "How big everything in Perch's windows is drawn.".to_string(),
        control,
    )
    .keywords(&["scale", "size", "bigger", "smaller", "display.zoom"])
    .reset("display.zoom", Setting::Zoom(None))
    .restart(move || {
        let wanted = pending.with_config(|c| c.display.zoom);
        !same(wanted, theme::started_display().zoom)
    })
}

fn code_font_size(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            let (label_page, menu_page, enabled) = (page.clone(), page.clone(), page.clone());
            select(
                move || label_page.with_config(|c| format!("{} px", c.display.code_font_size)),
                move || {
                    let current =
                        menu_page.with_config(|c| c.display.code_font_size.round() as u32);
                    let write = menu_page.clone();
                    choices(
                        (8..=32).map(|n| (format!("{n} px"), n)).collect(),
                        &current,
                        Rc::new(move |n| write.write(Setting::CodeFontSize(Some(n as f32)))),
                    )
                },
                move || enabled.writable(),
                "settings-code-font-size",
            )
            .into_any()
        }
    };
    let pending = page.clone();
    Row::new(
        "code-font-size",
        "Code font size",
        || "The size of code in diffs, files and code blocks.".to_string(),
        control,
    )
    .keywords(&["text", "monospace", "display.code_font_size"])
    .reset("display.code_font_size", Setting::CodeFontSize(None))
    .restart(move || {
        let wanted = pending.with_config(|c| c.display.code_font_size);
        (wanted - theme::started_display().code_font_size).abs() > f32::EPSILON
    })
}

/// The file's code font, with a blank one read as none.
fn named_font(font: Option<&str>) -> Option<String> {
    font.map(str::trim)
        .filter(|font| !font.is_empty())
        .map(str::to_string)
}

fn code_font(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            let (value_page, write) = (page.clone(), page.clone());
            setting_text(
                &page,
                move || value_page.with_config(|c| c.ui.code_font.clone().unwrap_or_default()),
                "The system's monospace",
                move |typed| write.write(Setting::CodeFont(named_font(Some(&typed)))),
                "settings-code-font",
            )
            .into_any()
        }
    };
    let pending = page.clone();
    Row::new(
        "code-font",
        "Code font",
        || "A monospaced font installed on this machine, by its family name.".to_string(),
        control,
    )
    .keywords(&["monospace", "family", "ui.code_font"])
    .reset("ui.code_font", Setting::CodeFont(None))
    .restart(move || {
        let wanted = pending.with_config(|c| named_font(c.ui.code_font.as_deref()));
        wanted != named_font(theme::started_code_font().as_deref())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zoom_reads_as_a_whole_percentage() {
        assert_eq!(zoom_label(1.0), "100%");
        assert_eq!(zoom_label(0.9), "90%");
        assert_eq!(zoom_label(1.25), "125%");
        assert_eq!(zoom_label(1.333), "133%");
    }
}
