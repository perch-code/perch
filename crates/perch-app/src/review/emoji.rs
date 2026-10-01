//! GitHub's emoji shortcodes: `:tada:` for 🎉. A composer's `:` lists
//! the emoji whose shortcodes have the letters typed after it, and a
//! comment's text has its shortcodes drawn as the emoji, as GitHub draws
//! them.
//!
//! The shortcodes are gemoji's, the set GitHub's own box offers.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::OnceLock;

/// The most emoji a `:` offers.
const MAX_EMOJI: usize = 10;

/// The emoji a review is written with, most used first, by a shortcode
/// of each: offered before the rest their letters find, so `:f` is 🔥.
#[rustfmt::skip]
const COMMON: &[&str] = &[
    "+1", "-1", "tada", "rocket", "heart", "eyes", "fire", "smile", "laughing", "joy",
    "thinking", "confused", "pray", "clap", "raised_hands", "100", "white_check_mark", "x",
    "warning", "bug", "sparkles", "wave", "muscle", "sweat_smile", "slightly_smiling_face",
    "wink", "sob", "see_no_evil", "facepalm", "shrug", "ok_hand", "point_up", "bulb", "memo",
    "construction", "zap", "boom", "star", "trophy",
];

/// An emoji in the list, with the shortcode it was found by and the
/// characters of it that the query matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmojiHit {
    pub emoji: &'static str,
    pub shortcode: &'static str,
    pub matches: Vec<Range<usize>>,
}

/// One shortcode: its emoji, where the emoji is in `COMMON` and in
/// Unicode's order, and whether it is a flag.
struct Entry {
    shortcode: &'static str,
    emoji: &'static str,
    common: usize,
    order: usize,
    flag: bool,
}

fn entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let common: std::collections::HashMap<&str, usize> = COMMON
            .iter()
            .enumerate()
            .filter_map(|(i, code)| Some((emojis::get_by_shortcode(code)?.as_str(), i)))
            .collect();
        emojis::iter()
            .enumerate()
            .flat_map(|(order, e)| {
                let flag = e.group() == emojis::Group::Flags;
                let common = common.get(e.as_str()).copied().unwrap_or(usize::MAX);
                e.shortcodes().map(move |shortcode| Entry {
                    shortcode,
                    emoji: e.as_str(),
                    common,
                    order,
                    flag,
                })
            })
            .collect()
    })
}

/// Whether `query` could be the start of a shortcode.
pub fn is_shortcode(query: &str) -> bool {
    !query.is_empty()
        && query
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-'))
}

/// The emoji to offer for `query`, each once: the shortcode it is, then
/// one that starts with it, then one with a word that does, then one that
/// has it inside. Within those, a common one first, then the shorter
/// shortcode, so `:fro` finds `frog` before `frowning`, and a country's
/// flag, whose shortcode is its two letters, after the rest.
// A match of one run is a list of one range.
#[allow(clippy::single_range_in_vec_init)]
pub fn emoji(query: &str) -> Vec<EmojiHit> {
    if !is_shortcode(query) {
        return Vec::new();
    }
    let q = query.to_ascii_lowercase();
    let mut ranked: Vec<_> = entries()
        .iter()
        .filter_map(|e| {
            let at = e.shortcode.find(&q)?;
            let word = e
                .shortcode
                .match_indices(&q)
                .map(|(i, _)| i)
                .find(|&i| i > 0 && matches!(e.shortcode.as_bytes()[i - 1], b'_' | b'-'));
            let (rank, at) = match (at, word) {
                _ if e.shortcode == q => (0, 0),
                (0, _) => (1, 0),
                (_, Some(i)) => (2, i),
                (at, None) => (3, at),
            };
            Some(((rank, e.flag, e.common, e.shortcode.len(), e.order), e, at))
        })
        .collect();
    ranked.sort_by_key(|(key, _, _)| *key);
    let mut seen = std::collections::HashSet::new();
    ranked
        .into_iter()
        .filter(|(_, e, _)| seen.insert(e.emoji))
        .take(MAX_EMOJI)
        .map(|(_, e, at)| EmojiHit {
            emoji: e.emoji,
            shortcode: e.shortcode,
            matches: vec![at..at + q.len()],
        })
        .collect()
}

/// `text` with each `:shortcode:` GitHub knows drawn as its emoji. One
/// glued to a word, or after a colon that does not close another, is left
/// as written, so a Rust path's `::new::` stays a path.
pub fn emojify(text: &str) -> Cow<'_, str> {
    if !text.contains(':') {
        return Cow::Borrowed(text);
    }
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let mut out = String::new();
    let mut from = 0;
    let mut search = 0;
    while let Some(open) = text[search..].find(':').map(|i| search + i) {
        search = open + 1;
        let Some(close) = text[open + 1..].find(':').map(|i| open + 1 + i) else {
            break;
        };
        let name = &text[open + 1..close];
        let before = text[..open].chars().next_back();
        let after_another = before == Some(':') && from == open && from > 0;
        let found = (is_shortcode(name)
            && !word(before)
            && (before != Some(':') || after_another)
            && !word(text[close + 1..].chars().next()))
        .then(|| emojis::get_by_shortcode(name))
        .flatten();
        if let Some(e) = found {
            out.push_str(&text[from..open]);
            out.push_str(e.as_str());
            from = close + 1;
            search = close + 1;
        }
    }
    if from == 0 {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[from..]);
    Cow::Owned(out)
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn shortcodes(hits: &[EmojiHit]) -> Vec<&str> {
        hits.iter().map(|h| h.shortcode).collect()
    }

    #[test]
    fn a_shortcode_that_starts_with_the_letters_comes_first_shortest_first() {
        let hits = emoji("fro");
        assert_eq!(hits[0].shortcode, "frog");
        assert_eq!(hits[0].emoji, "🐸");
        assert_eq!(hits[0].matches, [0..3]);
    }

    #[test]
    fn a_common_one_comes_first() {
        assert_eq!(emoji("f")[0].shortcode, "fire");
        assert_eq!(emoji("th")[0].emoji, "👍");
        assert!(COMMON.iter().all(|c| emojis::get_by_shortcode(c).is_some()));
    }

    #[test]
    fn a_word_inside_a_shortcode_outranks_letters_inside_a_word() {
        let hits = emoji("check");
        let codes = shortcodes(&hits);
        let word = codes.iter().position(|c| *c == "white_check_mark").unwrap();
        assert!(codes[..word].iter().all(|c| c.starts_with("check")));
        assert_eq!(hits[word].matches, [6..11]);
    }

    #[test]
    fn each_emoji_is_offered_once() {
        let hits = emoji("thumbs");
        assert_eq!(hits.iter().filter(|h| h.emoji == "👍").count(), 1);
    }

    #[test]
    fn a_countrys_flag_comes_after_the_rest() {
        let hits = emoji("f");
        assert!(
            !shortcodes(&hits).contains(&"fr"),
            "{:?}",
            shortcodes(&hits)
        );
        assert_eq!(
            emoji("fr")[0].shortcode,
            "fr",
            "but first when it is all that is typed"
        );
    }

    #[test]
    fn only_a_shortcodes_letters_list_anything() {
        assert!(emoji("").is_empty());
        assert!(emoji(")").is_empty());
        assert!(!emoji("+1").is_empty());
        assert_eq!(emoji("zzzzzz"), []);
        assert_eq!(emoji("FIRE")[0].shortcode, "fire");
    }

    #[test]
    fn shortcodes_in_text_are_drawn_as_their_emoji() {
        assert_eq!(emojify("ship it :rocket:!"), "ship it 🚀!");
        assert_eq!(emojify(":+1: :tada:"), "👍 🎉");
        assert_eq!(emojify("at 12:30:45"), "at 12:30:45");
        assert_eq!(emojify("Foo::new::bar"), "Foo::new::bar");
        assert_eq!(emojify("a:fire:b"), "a:fire:b");
        assert_eq!(emojify(":fire::rocket:"), "🔥🚀");
        assert_eq!(emojify("x::fire:"), "x::fire:");
        assert_eq!(emojify(":nope: :fire:"), ":nope: 🔥");
        assert_eq!(emojify("no colons"), "no colons");
        assert!(matches!(emojify("x: y"), Cow::Borrowed(_)));
    }
}
