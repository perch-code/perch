//! Who someone is, as their page on GitHub says: what the card under a
//! login shows.

use serde::Deserialize;

/// An account, as `GET /users/{login}` gives it. Everything but the login
/// and the face is the account's own to fill in, and often is not.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Profile {
    pub login: String,
    #[serde(default)]
    pub avatar_url: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    company: Option<String>,
    #[serde(default)]
    location: Option<String>,
}

impl Profile {
    /// The name they go by, when they gave one that is not their login
    /// again.
    pub fn name(&self) -> Option<&str> {
        filled(&self.name).filter(|name| *name != self.login)
    }

    /// Where they say they work: free text, `@slatedb` as often as a name.
    pub fn company(&self) -> Option<&str> {
        filled(&self.company)
    }

    pub fn location(&self) -> Option<&str> {
        filled(&self.location)
    }
}

/// A field with something in it.
fn filled(field: &Option<String>) -> Option<&str> {
    field
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_fields_and_a_name_that_is_the_login_are_left_out() {
        let profile: Profile = serde_json::from_str(
            r#"{"login":"nomiero","avatar_url":"https://avatars/1","name":"nomiero",
                "company":"  ","location":null}"#,
        )
        .unwrap();
        assert_eq!(profile.name(), None);
        assert_eq!(profile.company(), None);
        assert_eq!(profile.location(), None);

        let profile: Profile = serde_json::from_str(
            r#"{"login":"rohan","name":"Rohan D","company":" @slatedb ","location":"Lisbon"}"#,
        )
        .unwrap();
        assert_eq!(profile.name(), Some("Rohan D"));
        assert_eq!(profile.company(), Some("@slatedb"));
        assert_eq!(profile.location(), Some("Lisbon"));
        assert_eq!(profile.avatar_url, "");
    }
}
