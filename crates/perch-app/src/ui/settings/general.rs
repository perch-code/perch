//! General: how Perch talks to GitHub, and where config.toml is.

use std::rc::Rc;

use floem::prelude::*;
use floem::views::{Decorators, Stack};
use perch_github::Setting;

use super::controls::{self, choices, select, setting_text};
use super::{Group, Page, Row};

/// The poll intervals offered, in seconds.
const POLL_INTERVALS: [u64; 5] = [30, 60, 120, 300, 600];

pub(super) fn groups(page: &Page) -> Vec<Group> {
    vec![
        Group::new("GitHub", vec![poll_interval(page), github_host(page)]),
        Group::new("Config file", vec![config_file(page)]),
    ]
}

/// `secs` as a person would say it: "30 seconds", "1 minute".
pub(super) fn interval_label(secs: u64) -> String {
    match secs {
        1 => "1 second".to_string(),
        60 => "1 minute".to_string(),
        s if s > 0 && s % 60 == 0 => format!("{} minutes", s / 60),
        s => format!("{s} seconds"),
    }
}

fn poll_interval(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            let (label_page, menu_page, enabled) = (page.clone(), page.clone(), page.clone());
            select(
                move || label_page.with_config(|c| interval_label(c.poll_interval_secs)),
                move || {
                    let current = menu_page.with_config(|c| c.poll_interval_secs);
                    let mut offered = POLL_INTERVALS.to_vec();
                    if !offered.contains(&current) {
                        offered.push(current);
                        offered.sort_unstable();
                    }
                    let write = menu_page.clone();
                    choices(
                        offered
                            .into_iter()
                            .map(|secs| (interval_label(secs), secs))
                            .collect(),
                        &current,
                        Rc::new(move |secs| write.write(Setting::PollIntervalSecs(Some(secs)))),
                    )
                },
                move || enabled.writable(),
                "settings-poll-interval",
            )
            .into_any()
        }
    };
    Row::new(
        "poll-interval",
        "Poll interval",
        || "How often Perch asks GitHub what's new. GitHub can ask for longer.".to_string(),
        control,
    )
    .keywords(&["refresh", "notifications", "poll_interval_secs"])
    .reset("poll_interval_secs", Setting::PollIntervalSecs(None))
}

fn github_host(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            let (value_page, write) = (page.clone(), page.clone());
            setting_text(
                &page,
                move || value_page.with_config(|c| c.github_host.clone().unwrap_or_default()),
                "github.example.com",
                move |typed| {
                    let host = typed.trim();
                    write.write(Setting::GithubHost(
                        (!host.is_empty()).then(|| host.to_string()),
                    ))
                },
                "settings-github-host",
            )
            .into_any()
        }
    };
    Row::new(
        "github-host",
        "GitHub Enterprise host",
        || {
            "A GitHub Enterprise Server whose repositories Perch reads, beside github.com."
                .to_string()
        },
        control,
    )
    .keywords(&["ghe", "enterprise", "server", "github_host"])
    .reset("github_host", Setting::GithubHost(None))
}

fn config_file(page: &Page) -> Row {
    let path_page = page.clone();
    let control = {
        let page = page.clone();
        move || {
            let reveal = page.clone();
            Stack::horizontal((
                controls::open_in_editor_button(page.clone(), "settings-open-config"),
                controls::button(
                    "Reveal",
                    "settings-reveal-config",
                    || true,
                    move || {
                        if let Some(path) = reveal.path() {
                            crate::ui::widgets::reveal_in_file_manager(&path);
                        }
                    },
                ),
            ))
            .style(|s| s.gap(8.0).items_center())
            .into_any()
        }
    };
    Row::new(
        "config-file",
        "config.toml",
        move || {
            path_page
                .path()
                .map(|path| controls::tilde(&path))
                .unwrap_or_default()
        },
        control,
    )
    .keywords(&["file", "path", "edit", "toml", "config"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interval_reads_in_the_largest_whole_unit() {
        assert_eq!(interval_label(30), "30 seconds");
        assert_eq!(interval_label(60), "1 minute");
        assert_eq!(interval_label(300), "5 minutes");
        assert_eq!(interval_label(90), "90 seconds");
        assert_eq!(interval_label(1), "1 second");
    }
}
