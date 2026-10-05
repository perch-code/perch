//! Invite's background half: check the login is a person on GitHub, look
//! for an open invite for them, and file one when there is none.

use super::{feedback, Ctx, Event};
use crate::feedback::{SendError, REPO};
use crate::invite;

/// Ask for `login` to be added. `InviteSent` answers with the issue, new
/// or the one already open, or `InviteFailed` with why not.
pub(super) async fn send(ctx: &Ctx, login: &str, note: &str) {
    let failed = |message: String, forbidden: bool| {
        let _ = ctx
            .events
            .send(Event::InviteFailed(SendError { message, forbidden }));
    };
    let Some(github) = ctx.github() else {
        failed("Offline.".into(), false);
        return;
    };
    let user = match github.user(login).await {
        Ok(user) => user,
        Err(perch_github::Error::Status { status: 404, .. }) => {
            failed(format!("There's no GitHub user called {login}."), false);
            return;
        }
        Err(e) => {
            let error = feedback::send_error(&e, "read");
            if error.forbidden {
                failed(format!("GitHub wouldn't look up {login}."), false);
            } else {
                failed(error.message, false);
            }
            return;
        }
    };
    if user.kind == "Organization" {
        failed(
            format!("{} is an organization, not a person.", user.login),
            false,
        );
        return;
    }
    // GitHub's spelling, whatever the case typed.
    let login = if user.login.is_empty() {
        login.to_string()
    } else {
        user.login
    };

    // Asked for already: say so rather than file it twice. A search that
    // fails files anyway; a second issue is cheap to close.
    match github
        .search_issues(&invite::search_query(&login), 1, 10)
        .await
    {
        Ok(page) => {
            if let Some(hit) = page
                .hits
                .iter()
                .find(|hit| hit.is_open() && invite::is_invite_for(&hit.title, &login))
            {
                let _ = ctx.events.send(Event::InviteSent {
                    number: hit.number,
                    login,
                    existing: true,
                });
                return;
            }
        }
        Err(e) => tracing::warn!("searching for an invite for {login}: {e}"),
    }

    match github
        .create_issue_labeled(
            REPO,
            &invite::title(&login),
            &invite::body(&login, note),
            &[],
        )
        .await
    {
        Ok(issue) => {
            let _ = ctx.events.send(Event::InviteSent {
                number: issue.number,
                login,
                existing: false,
            });
        }
        Err(e) => {
            let _ = ctx.events.send(Event::InviteFailed(feedback::send_error(
                &e,
                "open issues on",
            )));
        }
    }
}
