//! Invite: ask for someone to be added to Perch's repository, as an issue
//! there for its owner to act on. This is the part with no window and no
//! network: the login as typed, and the issue's text. The backend checks
//! and files (`backend::invite`); the popover is `ui::invite`.

use crate::feedback::REPO;

/// The invite being written in a window's popover.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InviteDraft {
    pub login: String,
    pub note: String,
    pub sending: bool,
    pub error: Option<String>,
    /// The token can't open issues on Perch's repository: offer
    /// github.com's form instead.
    pub forbidden: bool,
}

/// A GitHub login out of what was typed: `name`, `@name`, or a link to
/// the profile. None when it can't be one.
pub fn parse_login(text: &str) -> Option<String> {
    let mut text = text.trim();
    for prefix in ["https://", "http://", "www.", "github.com/"] {
        text = text.strip_prefix(prefix).unwrap_or(text);
    }
    let text = text.strip_prefix('@').unwrap_or(text);
    let login = text.split(['/', '?', '#']).next().unwrap_or("");
    let valid = !login.is_empty()
        && login.len() <= 39
        && !login.starts_with('-')
        && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    valid.then(|| login.to_string())
}

pub fn title(login: &str) -> String {
    format!("Invite @{login}")
}

/// The issue's body: who, the note quoted, and the command that adds
/// them. The login is linked rather than mentioned, so nobody outside the
/// repository is pinged.
pub fn body(login: &str, note: &str) -> String {
    let mut out =
        format!("Please add [{login}](https://github.com/{login}) to {REPO} with read access.\n");
    let note = note.trim();
    if !note.is_empty() {
        out.push('\n');
        for line in note.lines() {
            if line.trim().is_empty() {
                out.push_str(">\n");
            } else {
                out.push_str(&format!("> {line}\n"));
            }
        }
    }
    out.push_str(&format!(
        "\n```bash\ngh api -X PUT repos/{REPO}/collaborators/{login} -f permission=pull\n```\n"
    ));
    out
}

/// The search for an open invite for `login`. Its hits are checked with
/// [`is_invite_for`], since the search matches words, not the title.
pub fn search_query(login: &str) -> String {
    format!("repo:{REPO} is:issue is:open in:title \"Invite {login}\"")
}

pub fn is_invite_for(title: &str, login: &str) -> bool {
    title.trim().eq_ignore_ascii_case(&self::title(login))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_is_read_from_a_name_a_mention_or_a_profile_link() {
        for typed in [
            "octo-cat",
            " @octo-cat ",
            "https://github.com/octo-cat",
            "github.com/octo-cat/",
            "https://www.github.com/octo-cat?tab=repositories",
        ] {
            assert_eq!(parse_login(typed).as_deref(), Some("octo-cat"), "{typed}");
        }
        for typed in ["", "@", "-octo", "octo cat", "octo_cat", &"a".repeat(40)] {
            assert_eq!(parse_login(typed), None, "{typed}");
        }
    }

    #[test]
    fn the_body_links_quotes_and_says_how_to_add_them() {
        let body = body("octo-cat", "From SlateDB.\n\nWants to try agents.");
        assert_eq!(
            body,
            "Please add [octo-cat](https://github.com/octo-cat) to perch-code/perch with read access.\n\
             \n\
             > From SlateDB.\n\
             >\n\
             > Wants to try agents.\n\
             \n\
             ```bash\n\
             gh api -X PUT repos/perch-code/perch/collaborators/octo-cat -f permission=pull\n\
             ```\n"
        );
        assert!(!body.contains("@octo-cat"), "nobody is pinged");
        assert!(
            !super::body("octo-cat", "  ").contains('>'),
            "no note, no quote"
        );
    }

    #[test]
    fn an_invite_is_found_by_its_title_in_any_case() {
        assert!(is_invite_for(" Invite @Octo-Cat", "octo-cat"));
        assert!(!is_invite_for("Invite @octo-cat2", "octo-cat"));
        assert!(!is_invite_for("Why invite @octo-cat?", "octo-cat"));
    }
}
