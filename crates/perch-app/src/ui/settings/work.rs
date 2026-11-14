//! Work: how new work is set up in every repository.

use floem::prelude::*;
use perch_github::{Approvals, Setting};

use super::controls::{dropdown, setting_text};
use super::{Group, Page, Row};

/// What `[work] approvals` takes. "Allow all" is for one repository at a
/// time: the file's top level reads it as Auto-review.
const APPROVALS: [Approvals; 2] = [Approvals::Auto, Approvals::Ask];

pub(super) fn groups(page: &Page) -> Vec<Group> {
    vec![Group::new(
        "New work",
        vec![branch_prefix(page), approvals(page)],
    )]
}

fn branch_prefix(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            let (value_page, write) = (page.clone(), page.clone());
            setting_text(
                &page,
                move || value_page.with_config(|c| c.work.branch_prefix.clone()),
                "None",
                move |typed| {
                    let prefix = typed.trim();
                    write.write(Setting::WorkBranchPrefix(
                        (!prefix.is_empty()).then(|| prefix.to_string()),
                    ))
                },
                "settings-branch-prefix",
            )
            .into_any()
        }
    };
    Row::new(
        "branch-prefix",
        "Branch prefix",
        || "Put before the branch names Perch suggests for new work, such as chris/.".to_string(),
        control,
    )
    .keywords(&["branch", "prefix", "name", "work.branch_prefix"])
    .reset("work.branch_prefix", Setting::WorkBranchPrefix(None))
}

fn approvals(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            dropdown(
                &page,
                |page| {
                    let offered = APPROVALS
                        .into_iter()
                        .map(|a| (a.label().to_string(), a))
                        .collect();
                    (offered, page.with_config(|c| c.work.approvals))
                },
                |a| Setting::WorkApprovals(Some(a)),
                "settings-work-approvals",
            )
        }
    };
    Row::new(
        "work-approvals",
        "Approvals",
        || {
            "Who decides when a Work run asks to go past its sandbox: the agent's own \
             automatic reviewer, or you. A repository can choose its own."
                .to_string()
        },
        control,
    )
    .keywords(&[
        "approve",
        "permission",
        "sandbox",
        "auto-review",
        "work.approvals",
    ])
    .reset("work.approvals", Setting::WorkApprovals(None))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{enter_in, goes_dark, open_on, pick, shown, window, written};
    use super::super::Section;
    use super::*;

    #[test]
    fn work_shows_the_file_and_writes_what_is_changed() {
        let (mut harness, state, mut commands) = window(
            1400.0,
            900.0,
            "[work]\nbranch_prefix = \"chris/\"\napprovals = \"ask\"\n",
        );
        open_on(&mut harness, state, Section::Work);
        assert!(shown(&harness, "settings-row:branch-prefix"));
        assert!(shown(&harness, "settings-row:work-approvals"));
        assert!(shown(&harness, "settings-reset:work-approvals"));
        // The box holds the file's prefix, and what is typed goes after it.

        pick(&mut harness, "settings-work-approvals", "Auto-review");
        assert_eq!(
            written(&mut commands),
            vec![Setting::WorkApprovals(Some(Approvals::Auto))]
        );
        enter_in(&mut harness, "settings-branch-prefix", "x");
        assert_eq!(
            written(&mut commands),
            vec![Setting::WorkBranchPrefix(Some("chris/x".into()))]
        );
        goes_dark(&mut harness);
    }
}
