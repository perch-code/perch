//! `@` in a composer: the people GitHub would suggest, and the files
//! Cmd-P would find, in a list under what is being typed.
//!
//! An `@` at the start of the text or after a space starts a mention,
//! and what is typed after it, up to the caret, is what the list is for.
//! The item's own people come first, its author and then whoever has
//! written on it, then the rest of the repository's people that GitHub
//! finds for the letters, as GitHub's own box lists them; then the
//! worktree's files, as Cmd-P ranks them. The arrows move the pick,
//! Enter or Tab takes it, and Escape puts the list away until another
//! `@` is typed.
//!
//! A person goes in as `@login`. A file goes in as `@path` for an agent,
//! as Claude Code and Codex take one, and as a code span for GitHub,
//! where `@` would be a mention and a code span links to Code.
//!
//! A `#` lists the repository's issues and pull requests the same way:
//! the recently updated ones before anything is typed, then those whose
//! number starts with the digits or whose title has the letters, and
//! whatever GitHub's search found for them. One goes in as `#number`.
//!
//! A `:` and a letter list the emoji whose GitHub shortcodes have the
//! letters, as GitHub's box does. One goes in as the emoji itself, which
//! reads the same on GitHub and to an agent.
//!
//! The list is drawn over the window, not in the composer, so a composer
//! in a card in the code does not cut it off.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use floem::kurbo::Rect;
use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Position;
use floem::views::editor::core::cursor::{Cursor, CursorAffinity, CursorMode};
use floem::views::editor::core::selection::Selection;
use floem::views::editor::Editor;
use floem::views::{dyn_container, Container, Decorators, Empty, ScrollExt, Stack};
use perch_github::{IssueReference, MentionableUser};

use crate::backend::Backend;
use crate::review::emoji::{self, EmojiHit};
use crate::review::go_to::{self, Hit, Pick, Sources};
use crate::review::model::ReviewState;
use crate::state::AppState;
use crate::theme;
use crate::ui::widgets::{avatar, clickable, state_glyph};

/// The most people the list offers, as GitHub's does.
const MAX_PEOPLE: usize = 5;
/// The most files it offers after them.
const MAX_FILES: usize = 10;
/// The most issues and pull requests a `#` offers.
const MAX_REFERENCES: usize = 10;
/// The longest a mention's letters run before they are taken for
/// something else.
const MAX_QUERY: usize = 100;
/// The longest a login is on GitHub; letters past it name no one.
const MAX_LOGIN: usize = 39;
/// The room kept between the list and the window's edges.
const GUTTER: f64 = 8.0;
/// How long typing rests on a query before GitHub is asked about it.
const ASK_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

// ---------------------------------------------------------------------
// What is typed
// ---------------------------------------------------------------------

/// What a mention names: a person or a file after `@`, an issue or a
/// pull request after `#`, an emoji after `:`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    At,
    Hash,
    Colon,
}

impl Trigger {
    fn char(self) -> char {
        match self {
            Trigger::At => '@',
            Trigger::Hash => '#',
            Trigger::Colon => ':',
        }
    }
}

/// The mention the caret is in: where its `@` or `#` is, which it is,
/// and what is typed between it and the caret. One starts only at the
/// start of the text or after a space, so an address is not one. A `:`
/// needs a shortcode's letters after it, so a smiley is not one.
pub fn token(text: &str, caret: usize) -> Option<(usize, Trigger, &str)> {
    let before = text.get(..caret)?;
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let word = &before[start..];
    let trigger = match word.chars().next()? {
        '@' => Trigger::At,
        '#' => Trigger::Hash,
        ':' => Trigger::Colon,
        _ => return None,
    };
    let query = &word[1..];
    if trigger == Trigger::Colon && !emoji::is_shortcode(query) {
        return None;
    }
    (!query.contains(trigger.char()) && query.len() <= MAX_QUERY).then_some((start, trigger, query))
}

/// Whether GitHub could have someone for `query`: a login's letters, or
/// a name's. A path is not asked about.
fn names_someone(query: &str) -> bool {
    query.chars().count() <= MAX_LOGIN
        && query
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// Whether GitHub's search is asked about `query` after a `#`: a number,
/// or a word of a title. Nothing typed asks for the recent ones; a
/// search qualifier is not passed on.
fn names_a_reference(query: &str) -> bool {
    query
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
}

// ---------------------------------------------------------------------
// Who the list offers
// ---------------------------------------------------------------------

/// Someone the list can offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: String,
}

impl From<&MentionableUser> for Person {
    fn from(user: &MentionableUser) -> Self {
        Person {
            login: user.login.clone(),
            name: user.name.clone(),
            avatar_url: user.avatar_url.clone(),
        }
    }
}

/// A person in the list, with the characters of the login and of the
/// name that the query matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonHit {
    pub person: Person,
    pub login_matches: Vec<Range<usize>>,
    pub name_matches: Vec<Range<usize>>,
}

fn folded(text: &str) -> Vec<char> {
    text.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// Where `needle` is in `hay`, by characters, for each start that
/// `at_start` allows.
fn find(hay: &[char], needle: &[char], at_start: impl Fn(usize) -> bool) -> Option<usize> {
    (0..=hay.len().checked_sub(needle.len())?)
        .find(|&i| at_start(i) && hay[i..i + needle.len()] == *needle)
}

/// How well a query names someone, best first, and the characters of
/// the login and of the name it matched.
type Rank = (u8, Vec<Range<usize>>, Vec<Range<usize>>);

/// How well `query` names `person`: the login starts with it, a word of
/// the name does, or the login has it inside.
// A match of one run is a list of one range.
#[allow(clippy::single_range_in_vec_init)]
fn rank(person: &Person, query: &str) -> Option<Rank> {
    let q = folded(query);
    if q.is_empty() {
        return Some((0, Vec::new(), Vec::new()));
    }
    let login = folded(&person.login);
    let run = |i: usize| vec![i..i + q.len()];
    if login.starts_with(&q) {
        return Some((0, run(0), Vec::new()));
    }
    if let Some(name) = &person.name {
        let name = folded(name);
        let word_start = |i: usize| i == 0 || !name[i - 1].is_alphanumeric();
        if let Some(i) = find(&name, &q, word_start) {
            return Some((1, Vec::new(), run(i)));
        }
    }
    find(&login, &q, |_| true).map(|i| (2, run(i), Vec::new()))
}

fn is_bot(login: &str) -> bool {
    login.ends_with("[bot]")
}

/// The people to offer for `query`: the item's own that match, then
/// those GitHub found, in its order, and never the reader, who does not
/// mention themselves, or a bot. `exact` says GitHub found `found` for
/// this very query, so each is offered as GitHub matched them; found for
/// a shorter one, they must match here too. Before anything is typed,
/// only the item's own are offered: GitHub's for nothing are the first
/// few of the repository's in its order, and nobody in particular.
pub fn people(
    query: &str,
    participants: &[Person],
    found: &[Person],
    exact: bool,
    viewer: Option<&str>,
) -> Vec<PersonHit> {
    let names: HashMap<String, &Person> =
        found.iter().map(|p| (p.login.to_lowercase(), p)).collect();
    let mut seen: HashSet<String> = viewer.map(str::to_lowercase).into_iter().collect();
    let mut ranked: Vec<(u8, u8, PersonHit)> = Vec::new();
    let found = if query.is_empty() { &[][..] } else { found };
    for (group, person) in participants
        .iter()
        .map(|p| (0u8, p))
        .chain(found.iter().map(|p| (1u8, p)))
    {
        let key = person.login.to_lowercase();
        if is_bot(&person.login) || !seen.insert(key.clone()) {
            continue;
        }
        // Someone of the item's is offered with the name GitHub has for
        // them, when it has one: a comment says only who wrote it.
        let mut person = person.clone();
        if let Some(known) = names.get(&key) {
            if person.name.is_none() {
                person.name = known.name.clone();
            }
            if person.avatar_url.is_empty() {
                person.avatar_url = known.avatar_url.clone();
            }
        }
        let matched = rank(&person, query)
            .or_else(|| (group == 1 && exact).then(|| (3, Vec::new(), Vec::new())));
        if let Some((rank, login_matches, name_matches)) = matched {
            ranked.push((
                group,
                rank,
                PersonHit {
                    person,
                    login_matches,
                    name_matches,
                },
            ));
        }
    }
    // Stable, so each group keeps its order within a rank.
    ranked.sort_by_key(|(group, rank, _)| (*group, *rank));
    ranked
        .into_iter()
        .take(MAX_PEOPLE)
        .map(|(_, _, hit)| hit)
        .collect()
}

/// The item's own people, in the order they came to it: its author, then
/// whoever has commented, reviewed or written on a line.
fn participants(state: ReviewState) -> Vec<Person> {
    let mut out: Vec<Person> = Vec::new();
    let mut add = |login: &str, avatar_url: &str| {
        if login.is_empty() {
            return;
        }
        match out.iter_mut().find(|p| p.login == login) {
            Some(p) if p.avatar_url.is_empty() => p.avatar_url = avatar_url.to_string(),
            Some(_) => {}
            None => out.push(Person {
                login: login.to_string(),
                name: None,
                avatar_url: avatar_url.to_string(),
            }),
        }
    };
    state.item.with(|item| {
        let Some(item) = item else { return };
        if let Some(pr) = item.pull_request() {
            add(&pr.author_login, &pr.author_avatar_url);
        }
        if let Some(issue) = item.issue() {
            add(&issue.author_login, &issue.author_avatar_url);
        }
    });
    state.discussion.with(|d| {
        let Some(d) = d else { return };
        for c in &d.comments {
            add(&c.user, &c.user_avatar_url);
        }
        for r in &d.reviews {
            add(&r.user, "");
        }
        for c in &d.review_comments {
            add(&c.user, &c.user_avatar_url);
        }
    });
    state.issue_comments.with(|comments| {
        for c in comments.iter() {
            add(&c.user, &c.user_avatar_url);
        }
    });
    out
}

/// What GitHub found for the query, or for the longest query before it
/// that it has answered, and whether that was the query itself.
fn found_for<T: Clone>(
    query: &str,
    found: &HashMap<String, Option<Arc<Vec<T>>>>,
) -> (Vec<T>, bool) {
    let mut end = query.len();
    loop {
        if let Some(Some(answer)) = found.get(&query[..end]) {
            return (answer.to_vec(), end == query.len());
        }
        if end == 0 {
            return (Vec::new(), false);
        }
        end = query[..end]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
    }
}

/// Everyone GitHub has named for any query, by login, for the names
/// and the avatars of the item's own people.
fn known(found: &HashMap<String, Option<Arc<Vec<MentionableUser>>>>) -> HashMap<String, Person> {
    found
        .values()
        .flatten()
        .flat_map(|users| users.iter())
        .map(|u| (u.login.to_lowercase(), Person::from(u)))
        .collect()
}

/// `own` with the names and avatars GitHub gave for them.
fn named(own: &[Person], known: &HashMap<String, Person>) -> Vec<Person> {
    own.iter()
        .map(|p| {
            let mut p = p.clone();
            if let Some(k) = known.get(&p.login.to_lowercase()) {
                p.name = p.name.or_else(|| k.name.clone());
                if p.avatar_url.is_empty() {
                    p.avatar_url = k.avatar_url.clone();
                }
            }
            p
        })
        .collect()
}

// ---------------------------------------------------------------------
// Which issues the list offers
// ---------------------------------------------------------------------

/// An issue or a pull request in the list, with the characters of the
/// title and of its `#number` that the query matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceHit {
    pub reference: IssueReference,
    pub title_matches: Vec<Range<usize>>,
    pub number_matches: Vec<Range<usize>>,
}

/// The issues and pull requests to offer for `query`: those GitHub
/// found for it, or for the longest query before it, then the recently
/// updated ones, each once. A number the digits start comes first, then
/// a title with a word that starts with the letters, then one that has
/// them inside. `exact` says GitHub's search found `found` for this very
/// query, so one it matched on something the list cannot see, a word's
/// stem, is offered after those.
// A match of one run is a list of one range.
#[allow(clippy::single_range_in_vec_init)]
pub fn references(
    query: &str,
    found: &[IssueReference],
    exact: bool,
    recent: &[IssueReference],
) -> Vec<ReferenceHit> {
    let q = folded(query);
    let mut seen: HashSet<u64> = HashSet::new();
    let mut ranked: Vec<(u8, ReferenceHit)> = Vec::new();
    for (from_search, reference) in found
        .iter()
        .map(|r| (exact, r))
        .chain(recent.iter().map(|r| (false, r)))
    {
        if !seen.insert(reference.number) {
            continue;
        }
        let hit = |rank: u8, title_matches: Vec<Range<usize>>, number: bool| {
            (
                rank,
                ReferenceHit {
                    reference: reference.clone(),
                    title_matches,
                    // The `#` and the digits typed after it.
                    number_matches: if number {
                        vec![0..q.len() + 1]
                    } else {
                        Vec::new()
                    },
                },
            )
        };
        let number = reference.number.to_string();
        let title = folded(&reference.title);
        let word_start = |i: usize| i == 0 || !title[i - 1].is_alphanumeric();
        let matched = if q.is_empty() {
            Some(hit(0, Vec::new(), false))
        } else if number == query {
            Some(hit(0, Vec::new(), true))
        } else if number.starts_with(query) {
            Some(hit(1, Vec::new(), true))
        } else if let Some(i) = find(&title, &q, word_start) {
            Some(hit(2, vec![i..i + q.len()], false))
        } else if let Some(i) = find(&title, &q, |_| true) {
            Some(hit(3, vec![i..i + q.len()], false))
        } else {
            from_search.then(|| hit(4, Vec::new(), false))
        };
        ranked.extend(matched);
    }
    // Stable, so GitHub's order holds within a rank.
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked
        .into_iter()
        .take(MAX_REFERENCES)
        .map(|(_, hit)| hit)
        .collect()
}

/// The issues other than `number` that `query` finds among those GitHub
/// has answered with, ranked as a `#` ranks them: what Close as
/// duplicate offers. Tracked over the answers.
pub(crate) fn duplicate_candidates(
    state: ReviewState,
    query: &str,
    number: u64,
) -> Vec<ReferenceHit> {
    use perch_github::SubjectState;

    let issue = |r: &IssueReference| {
        r.number != number
            && matches!(
                r.state,
                SubjectState::IssueOpen | SubjectState::IssueClosed | SubjectState::IssueNotPlanned
            )
    };
    state.mentions.references.with(|f| {
        let (found, exact) = found_for(query, f);
        let found: Vec<_> = found.into_iter().filter(issue).collect();
        let recent: Vec<_> = f
            .get("")
            .cloned()
            .flatten()
            .map(|r| r.iter().filter(|r| issue(r)).cloned().collect())
            .unwrap_or_default();
        references(query, &found, exact, &recent)
    })
}

/// Ask GitHub for the item's issues and pull requests that `query` names
/// once typing rests on it, as a `#` does, unless it has been asked.
pub(crate) fn ask_references(state: ReviewState, backend: Rc<Backend>, query: &str) {
    let repo = state
        .item
        .with_untracked(|i| i.as_ref().map(|i| (i.id.clone(), i.repo_full_name.clone())));
    let Some((item_id, repo)) = repo else {
        return;
    };
    if perch_github::is_local(&repo)
        || !names_a_reference(query)
        || state.mentions.is_asked(Trigger::Hash, query)
    {
        return;
    }
    let ask = Ask {
        item_id,
        repo,
        trigger: Trigger::Hash,
        query: query.to_string(),
    };
    state.mentions.waiting.set(Some(ask.clone()));
    floem::action::exec_after(ASK_DELAY, move |_| {
        state.mentions.ask_if_waiting(&backend, &ask);
    });
}

// ---------------------------------------------------------------------
// State
// ---------------------------------------------------------------------

/// A row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Suggestion {
    Person(PersonHit),
    File(Hit),
    Reference(ReferenceHit),
    Emoji(EmojiHit),
}

/// The list while it is up, and the box it is for.
#[derive(Clone)]
pub struct Shown {
    owner: u64,
    pub suggestions: Arc<Vec<Suggestion>>,
    /// Where the `@` is in the window: its left edge, and the top and the
    /// bottom of its line. Read as the box scrolls.
    place: Rc<dyn Fn() -> (f64, f64, f64)>,
    /// Put a row's text in place of the mention.
    accept: Rc<dyn Fn(usize)>,
}

/// The list's state, in the review state, one for every box: only the
/// box with the keyboard has a list up.
#[derive(Clone, Copy)]
pub struct Mentions {
    pub shown: RwSignal<Option<Shown>>,
    /// The row Enter takes: the first, whenever the query changes.
    pub selected: RwSignal<usize>,
    /// The people GitHub found for each query it was asked, `None` while
    /// the answer is on its way. The item's repository's, so it goes with
    /// the item.
    pub found: RwSignal<HashMap<String, Option<Arc<Vec<MentionableUser>>>>>,
    /// The issues and pull requests GitHub found for each query after a
    /// `#`, as `found` has people.
    pub references: RwSignal<HashMap<String, Option<Arc<Vec<IssueReference>>>>>,
    /// The query GitHub is to be asked about once typing rests on it.
    pub waiting: RwSignal<Option<Ask>>,
}

/// A query for GitHub, and the item and repository it is for, and
/// whether it is for people or for issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub item_id: String,
    pub repo: String,
    pub trigger: Trigger,
    pub query: String,
}

impl Default for Mentions {
    fn default() -> Self {
        Self::new()
    }
}

impl Mentions {
    pub fn new() -> Self {
        Mentions {
            shown: RwSignal::new(None),
            selected: RwSignal::new(0),
            found: RwSignal::new(HashMap::new()),
            references: RwSignal::new(HashMap::new()),
            waiting: RwSignal::new(None),
        }
    }

    /// Put away with the item it was over.
    pub fn reset(&self) {
        if self.shown.with_untracked(Option::is_some) {
            self.shown.set(None);
        }
        if !self.found.with_untracked(HashMap::is_empty) {
            self.found.set(HashMap::new());
        }
        if !self.references.with_untracked(HashMap::is_empty) {
            self.references.set(HashMap::new());
        }
        self.stop_waiting();
    }

    /// Ask GitHub about the query that is waiting, if it is still `ask`:
    /// typing has rested on it. It is then asked, and not again.
    pub fn ask_if_waiting(&self, backend: &Backend, ask: &Ask) {
        if self.waiting.with_untracked(|w| w.as_ref() != Some(ask)) {
            return;
        }
        self.waiting.set(None);
        if self.is_asked(ask.trigger, &ask.query) {
            return;
        }
        match ask.trigger {
            Trigger::At => {
                self.found
                    .update(|f| drop(f.insert(ask.query.clone(), None)));
                backend.mentionable_users(&ask.item_id, &ask.repo, &ask.query);
            }
            Trigger::Hash => {
                self.references
                    .update(|f| drop(f.insert(ask.query.clone(), None)));
                backend.issue_references(&ask.item_id, &ask.repo, &ask.query);
            }
            Trigger::Colon => {}
        }
    }

    /// Whether GitHub has been asked about `query` after `trigger`.
    fn is_asked(&self, trigger: Trigger, query: &str) -> bool {
        match trigger {
            Trigger::At => self.found.with_untracked(|f| f.contains_key(query)),
            Trigger::Hash => self.references.with_untracked(|f| f.contains_key(query)),
            // The shortcodes are all here.
            Trigger::Colon => true,
        }
    }

    fn stop_waiting(&self) {
        if self.waiting.with_untracked(Option::is_some) {
            self.waiting.set(None);
        }
    }

    /// GitHub's answer for `query`.
    pub fn install(&self, query: String, users: Vec<MentionableUser>) {
        self.found
            .update(|found| drop(found.insert(query, Some(Arc::new(users)))));
    }

    /// GitHub's answer for `query` after a `#`.
    pub fn install_references(&self, query: String, references: Vec<IssueReference>) {
        self.references
            .update(|found| drop(found.insert(query, Some(Arc::new(references)))));
    }

    fn is_up_for(&self, owner: u64) -> bool {
        self.shown
            .with_untracked(|s| s.as_ref().is_some_and(|s| s.owner == owner))
    }

    fn put_away(&self, owner: u64) {
        if self.is_up_for(owner) {
            self.shown.set(None);
        }
    }

    fn len(&self) -> usize {
        self.shown
            .with_untracked(|s| s.as_ref().map_or(0, |s| s.suggestions.len()))
    }

    /// Up and down the list, wrapping at the ends.
    fn step(&self, by: isize) {
        let n = self.len();
        if n == 0 {
            return;
        }
        let at = self.selected.get_untracked() as isize;
        self.selected.set((at + by).rem_euclid(n as isize) as usize);
    }

    fn accept(&self, index: usize) {
        let accept = self
            .shown
            .with_untracked(|s| s.as_ref().map(|s| s.accept.clone()));
        if let Some(accept) = accept {
            accept(index);
        }
    }
}

// ---------------------------------------------------------------------
// A box's end of it
// ---------------------------------------------------------------------

/// The files a box's `@` lists, with the tree and the changes they were
/// listed from.
type Listed = (
    Arc<Vec<perch_git::TreeEntry>>,
    Arc<Vec<perch_git::ChangedFile>>,
    Arc<go_to::Files>,
);

/// A composer's hold on the list: what its key handler asks first, and
/// what it says when it gains and loses the keyboard.
#[derive(Clone)]
pub struct Mentioner {
    state: ReviewState,
    owner: u64,
    focused: RwSignal<bool>,
    /// The `@` Escape put the list away for.
    dismissed: RwSignal<Option<usize>>,
    /// The mention the list is up for: its `@`, and what is typed after it.
    last: RwSignal<Option<(usize, String)>>,
}

impl Mentioner {
    /// The list's keys, while it is up for this box: the arrows and
    /// Ctrl-N and Ctrl-P move the pick, Enter and Tab take it, and Escape
    /// puts it away. True for a key it took.
    pub fn key(&self, key: &KeyboardEvent) -> bool {
        let mentions = self.state.mentions;
        if key.is_composing || !mentions.is_up_for(self.owner) {
            return false;
        }
        let plain = key.modifiers.is_empty();
        let ctrl = key.modifiers == Modifiers::CONTROL;
        match key.code {
            Code::ArrowDown if plain => mentions.step(1),
            Code::ArrowUp if plain => mentions.step(-1),
            Code::KeyN if ctrl => mentions.step(1),
            Code::KeyP if ctrl => mentions.step(-1),
            Code::Enter | Code::Tab if plain => mentions.accept(mentions.selected.get_untracked()),
            Code::Escape if plain => {
                let at = self.token_at();
                self.dismissed.set(at);
                mentions.put_away(self.owner);
            }
            _ => return false,
        }
        true
    }

    /// The box gained or lost the keyboard. The list is up only while it
    /// has it; a press on a row takes the row before the box hears.
    pub fn focus(&self, focused: bool) {
        self.focused.set(focused);
    }

    /// The box is gone, and its list with it.
    pub fn cleanup(&self) {
        self.state.mentions.put_away(self.owner);
    }

    fn token_at(&self) -> Option<usize> {
        self.last.with_untracked(|l| l.as_ref().map(|(at, _)| *at))
    }
}

/// Give `editor` the list. `file_text` is what a file goes in as, asked
/// when one is taken, since the composer's destination can change while
/// it is typed into.
pub fn attach(
    app: AppState,
    state: ReviewState,
    backend: Rc<Backend>,
    editor: &Editor,
    file_text: impl Fn(&str) -> String + 'static,
) -> Mentioner {
    let owner = state.next_token();
    let focused = RwSignal::new(false);
    let dismissed = RwSignal::new(None);
    let last: RwSignal<Option<(usize, String)>> = RwSignal::new(None);
    let file_text: Rc<dyn Fn(&str) -> String> = Rc::new(file_text);

    // The worktree's files, listed the first time an `@` wants them and
    // again only when the tree or the changes do: a box opens far more
    // often than a mention is typed in it.
    let listed: RefCell<Option<Listed>> = RefCell::new(None);
    let files = move || {
        let tree = state.tree.get();
        let changed = state.changed_files.get();
        let mut listed = listed.borrow_mut();
        match &*listed {
            Some((t, c, files)) if Arc::ptr_eq(t, &tree) && Arc::ptr_eq(c, &changed) => {
                files.clone()
            }
            _ => {
                let files = Arc::new(go_to::Files::new(go_to::candidates(&tree, &changed)));
                *listed = Some((tree, changed, files.clone()));
                files
            }
        }
    };

    let ed = editor.clone();
    Effect::new(move |_| {
        let focused = focused.get();
        let cursor = ed.cursor.get();
        ed.doc().cache_rev().track();
        let caret = match cursor.get_selection() {
            Some((start, end)) if start == end && cursor.get_selection_count() == 1 => Some(end),
            _ => None,
        };
        let text = ed.doc().text().to_string();
        let mention = caret
            .filter(|_| focused && !ed.read_only.get())
            .and_then(|caret| token(&text, caret))
            .filter(|(at, _, _)| dismissed.get() != Some(*at));
        let Some((at, trigger, query)) = mention else {
            if last.with_untracked(Option::is_some) {
                last.set(None);
            }
            if dismissed.get_untracked().is_some() && caret.and_then(|c| token(&text, c)).is_none()
            {
                dismissed.set(None);
            }
            state.mentions.stop_waiting();
            state.mentions.put_away(owner);
            return;
        };

        // GitHub is asked once for each query, for a repository it has,
        // and only once typing rests on it: a name typed straight through
        // asks for the name, not for every letter of it.
        let repo = state
            .item
            .with_untracked(|i| i.as_ref().map(|i| (i.id.clone(), i.repo_full_name.clone())));
        // After a `#`, nothing typed asks for the recent ones.
        let askable = match trigger {
            Trigger::At => !query.is_empty() && names_someone(query),
            Trigger::Hash => names_a_reference(query),
            Trigger::Colon => false,
        };
        let ask = repo.filter(|(_, repo)| {
            askable && !perch_github::is_local(repo) && !state.mentions.is_asked(trigger, query)
        });
        match ask {
            Some((item_id, repo)) => {
                let ask = Ask {
                    item_id,
                    repo,
                    trigger,
                    query: query.to_string(),
                };
                if state
                    .mentions
                    .waiting
                    .with_untracked(|w| w.as_ref() != Some(&ask))
                {
                    state.mentions.waiting.set(Some(ask.clone()));
                    let backend = backend.clone();
                    floem::action::exec_after(ASK_DELAY, move |_| {
                        state.mentions.ask_if_waiting(&backend, &ask);
                    });
                }
            }
            None => state.mentions.stop_waiting(),
        }

        let suggestions = match trigger {
            Trigger::At => people_and_files(app, state, query, &files),
            Trigger::Hash => state.mentions.references.with(|f| {
                let (found, exact) = found_for(query, f);
                let recent = f.get("").cloned().flatten().unwrap_or_default();
                references(query, &found, exact, &recent)
                    .into_iter()
                    .map(Suggestion::Reference)
                    .collect()
            }),
            Trigger::Colon => emoji::emoji(query)
                .into_iter()
                .map(Suggestion::Emoji)
                .collect(),
        };
        if suggestions.is_empty() {
            state.mentions.put_away(owner);
            return;
        }

        // A new query picks the first row again.
        let key = (at, query.to_string());
        if last.with_untracked(|l| l.as_ref() != Some(&key)) {
            last.set(Some(key));
            state.mentions.selected.set(0);
        }
        let suggestions = Arc::new(suggestions);
        let place_ed = ed.clone();
        let place = Rc::new(move || {
            let origin = place_ed.window_origin.get();
            let viewport = place_ed.viewport.get();
            let (above, below) = place_ed.points_of_offset(at, CursorAffinity::Forward);
            (
                origin.x + above.x - viewport.x0,
                origin.y + above.y - viewport.y0,
                origin.y + below.y - viewport.y0,
            )
        });
        let accept_ed = ed.clone();
        let taken = suggestions.clone();
        let file_text = file_text.clone();
        let accept = Rc::new(move |index: usize| {
            let Some(pick) = taken.get(index) else {
                return;
            };
            let text = match pick {
                Suggestion::Person(hit) => format!("@{} ", hit.person.login),
                Suggestion::File(hit) => file_text(&hit.path),
                Suggestion::Reference(hit) => format!("#{} ", hit.reference.number),
                Suggestion::Emoji(hit) => format!("{} ", hit.emoji),
            };
            replace_mention(&accept_ed, &text);
        });
        state.mentions.shown.set(Some(Shown {
            owner,
            suggestions,
            place,
            accept,
        }));
    });

    Mentioner {
        state,
        owner,
        focused,
        dismissed,
        last,
    }
}

/// What an `@` offers for `query`: the people, then the files.
fn people_and_files(
    app: AppState,
    state: ReviewState,
    query: &str,
    files: &impl Fn() -> Arc<go_to::Files>,
) -> Vec<Suggestion> {
    let (found, exact, known) = state.mentions.found.with(|f| {
        let (found, exact) = found_for(query, f);
        let found: Vec<Person> = found.iter().map(Person::from).collect();
        (found, exact, known(f))
    });
    let own = named(&participants(state), &known);
    let viewer = app
        .viewer
        .with(|v| v.as_ref().map(|(login, _)| login.clone()));
    let mut suggestions: Vec<Suggestion> = people(query, &own, &found, exact, viewer.as_deref())
        .into_iter()
        .map(Suggestion::Person)
        .collect();
    let files = files();
    let recent = state.recent();
    let changed = state.changed_files.get();
    let results = go_to::results(
        query,
        &Sources {
            files: &files,
            recent: &recent,
            changed: &changed,
            active: None,
        },
    );
    suggestions.extend(
        results
            .picks
            .into_iter()
            .filter_map(|p| match p {
                Pick::File(hit) => Some(Suggestion::File(hit)),
                Pick::Line { .. } => None,
            })
            .take(MAX_FILES),
    );
    suggestions
}

/// Put `text` in place of the mention the caret is in, as if it were
/// typed over it, and give the box the keyboard back after a press on
/// the list.
fn replace_mention(editor: &Editor, text: &str) {
    let caret = editor.cursor.with_untracked(|c| c.offset());
    let whole = editor.doc().text().to_string();
    let Some((at, _, _)) = token(&whole, caret) else {
        return;
    };
    editor.cursor.set(Cursor::new(
        CursorMode::Insert(Selection::region(at, caret, CursorAffinity::Backward)),
        None,
        None,
    ));
    editor.receive_char(text);
    if let Some(id) = editor.editor_view_id.get_untracked() {
        id.request_focus();
    }
}

// ---------------------------------------------------------------------
// The list
// ---------------------------------------------------------------------

/// Where the list goes: under the `@`, or over it when there is not room
/// below, and moved in from the window's right edge when it would run
/// off it. The left edge and the top.
fn placement(at: (f64, f64, f64), size: (f64, f64), window: (f64, f64)) -> (f64, f64) {
    let (x, top, bottom) = at;
    let (width, height) = size;
    let left = x
        .min(window.0 - width - GUTTER)
        .max(GUTTER.min(window.0 - width).max(0.0));
    let below = bottom + 4.0;
    let above = top - 4.0 - height;
    let y = if below + height <= window.1 - GUTTER || above < GUTTER {
        below
    } else {
        above
    };
    (left, y)
}

/// The list over the window, while a box has one up.
pub fn popup(app: AppState, state: ReviewState) -> impl IntoView {
    let suggestions = Memo::new(move |_| {
        state
            .mentions
            .shown
            .with(|s| s.as_ref().map(|s| s.suggestions.clone()))
    });
    dyn_container(
        move || suggestions.get(),
        move |suggestions| match suggestions {
            None => Empty::new().into_any(),
            Some(suggestions) => list(app, state, suggestions).into_any(),
        },
    )
    .debug_name("mentions")
    .style(move |s| {
        let shown = state.mentions.shown.get();
        let Some(shown) = shown else {
            return s.hide();
        };
        let rows = shown.suggestions.len().min(theme::MENTION_ROWS) as f64;
        let height = rows * theme::GO_TO_ROW_HEIGHT + 2.0 * theme::GO_TO_PADDING + 2.0;
        let window = (app.window_width.get(), app.window_height.get());
        let width = theme::MENTION_WIDTH.min(window.0 - 2.0 * GUTTER).max(0.0);
        let (left, top) = placement((shown.place)(), (width, height), window);
        s.position(Position::Absolute)
            .inset_left(left)
            .inset_top(top)
            .width(width)
            .z_index(90)
            .padding(theme::GO_TO_PADDING)
            .apply(crate::ui::menu::surface(floem::style::Style::new()))
    })
}

fn list(app: AppState, state: ReviewState, suggestions: Arc<Vec<Suggestion>>) -> impl IntoView {
    Stack::vertical_from_iter(
        suggestions
            .iter()
            .enumerate()
            .map(|(index, s)| row(app, state, index, s)),
    )
    .style(|s| s.flex_col().width_full())
    .scroll()
    .ensure_visible(move || {
        let at = state.mentions.selected.get() as f64 * theme::GO_TO_ROW_HEIGHT;
        Rect::new(0.0, at, 1.0, at + theme::GO_TO_ROW_HEIGHT)
    })
    .style(|s| {
        s.width_full()
            .max_height(theme::MENTION_ROWS as f64 * theme::GO_TO_ROW_HEIGHT)
            .overflow_x(floem::taffy::Overflow::Hidden)
            .scrollbar_width(theme::GO_TO_PADDING)
            .margin_right(-theme::GO_TO_PADDING)
    })
}

fn row(app: AppState, state: ReviewState, index: usize, suggestion: &Suggestion) -> impl IntoView {
    let selected = move || state.mentions.selected.get() == index;
    let (name, content) = match suggestion {
        Suggestion::Person(hit) => (
            format!("@{}", hit.person.login),
            person_row(app, hit).into_any(),
        ),
        Suggestion::File(hit) => (hit.path.clone(), go_to::file_row(hit, None).into_any()),
        Suggestion::Reference(hit) => (
            format!("#{}", hit.reference.number),
            reference_row(hit).into_any(),
        ),
        Suggestion::Emoji(hit) => (format!(":{}:", hit.shortcode), emoji_row(hit).into_any()),
    };
    Container::new(Container::new(content).style(move |s| {
        let s = s
            .size_full()
            .items_center()
            .padding_horiz(8.0)
            .border_radius(crate::ui::menu::ROW_RADIUS);
        if selected() {
            s.background(theme::menu_hover())
        } else {
            s.hover(|s| s.background(theme::menu_pointed()))
        }
    }))
    // On the press, not the click: the press takes the keyboard from the
    // box, and the box puts the list away when it loses it.
    .on_event_stop(el::PointerDown, move |_, _| {
        state.mentions.selected.set(index);
        state.mentions.accept(index);
    })
    .debug_name(format!("mention-row:{name}"))
    .style(|s| {
        clickable(s)
            .width_full()
            .height(theme::GO_TO_ROW_HEIGHT)
            .flex_shrink(0.0_f32)
    })
}

/// A person's row: the avatar, the login, and the name in the subtle
/// colour, as GitHub's list has them.
fn person_row(app: AppState, hit: &PersonHit) -> impl IntoView {
    let url = hit.person.avatar_url.clone();
    let name = hit.person.name.clone().unwrap_or_default();
    Stack::horizontal((
        avatar(app, move || url.clone(), 16.0, hit.person.login.clone())
            .style(|s| s.flex_shrink(0.0_f32).margin_right(8.0)),
        go_to::highlighted(
            &hit.person.login,
            &hit.login_matches,
            theme::TEXT_META,
            theme::fg,
        )
        .style(|s| s.flex_shrink(0.0_f32).font_weight(theme::weight_strong())),
        go_to::highlighted(&name, &hit.name_matches, theme::TEXT_META, theme::fg_subtle)
            .style(|s| s.margin_left(6.0).min_width(0.0).flex_shrink(1.0_f32)),
    ))
    .style(|s| s.items_center().width_full().min_width(0.0))
}

/// An emoji's row: the emoji, and its shortcode between colons, as
/// GitHub's list has them.
fn emoji_row(hit: &EmojiHit) -> impl IntoView {
    let shortcode = format!(":{}:", hit.shortcode);
    // Past the opening colon.
    let matches: Vec<Range<usize>> = hit.matches.iter().map(|m| m.start + 1..m.end + 1).collect();
    Stack::horizontal((
        Label::new(hit.emoji).style(|s| {
            s.flex_shrink(0.0_f32)
                .margin_right(8.0)
                .font_size(theme::TEXT_META)
        }),
        go_to::highlighted(&shortcode, &matches, theme::TEXT_META, theme::fg)
            .style(|s| s.min_width(0.0).flex_shrink(1.0_f32)),
    ))
    .style(|s| s.items_center().width_full().min_width(0.0))
}

/// An issue's or a pull request's row: the state glyph, the title, and
/// the number in the subtle colour, as GitHub's list has them.
pub(crate) fn reference_row(hit: &ReferenceHit) -> impl IntoView {
    let state = hit.reference.state;
    let number = format!("#{}", hit.reference.number);
    Stack::horizontal((
        state_glyph(state, move || crate::icons::state_color(state))
            .style(|s| s.flex_shrink(0.0_f32).margin_right(8.0)),
        go_to::highlighted(
            &hit.reference.title,
            &hit.title_matches,
            theme::TEXT_META,
            theme::fg,
        )
        .style(|s| {
            s.min_width(0.0)
                .flex_shrink(1.0_f32)
                .font_weight(theme::weight_strong())
        }),
        go_to::highlighted(
            &number,
            &hit.number_matches,
            theme::TEXT_META,
            theme::fg_subtle,
        )
        .style(|s| s.flex_shrink(0.0_f32).margin_left(6.0)),
    ))
    .style(|s| s.items_center().width_full().min_width(0.0))
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn person(login: &str, name: Option<&str>) -> Person {
        Person {
            login: login.into(),
            name: name.map(Into::into),
            avatar_url: String::new(),
        }
    }

    fn logins(hits: &[PersonHit]) -> Vec<&str> {
        hits.iter().map(|h| h.person.login.as_str()).collect()
    }

    #[test]
    fn an_at_starts_a_mention_only_at_the_start_or_after_a_space() {
        use Trigger::At;
        assert_eq!(token("@", 1), Some((0, At, "")));
        assert_eq!(token("hi @cad", 7), Some((3, At, "cad")));
        assert_eq!(token("hi\n@src/sl", 10), Some((3, At, "src/sl")));
        assert_eq!(token("mail me@example.com", 19), None);
        assert_eq!(token("@a@b", 4), None);
        assert_eq!(token("@cad done", 9), None, "a space ends it");
        assert_eq!(
            token("@cadonna", 4),
            Some((0, At, "cad")),
            "up to the caret"
        );
        assert_eq!(token("é @x", 5), Some((3, At, "x")));
    }

    #[test]
    fn a_hash_starts_one_for_issues_the_same_way() {
        use Trigger::Hash;
        assert_eq!(token("#", 1), Some((0, Hash, "")));
        assert_eq!(token("see #rf", 7), Some((4, Hash, "rf")));
        assert_eq!(token("see #12", 7), Some((4, Hash, "12")));
        assert_eq!(token("a#12", 4), None, "not inside a word");
        assert_eq!(token("## Heading", 2), None, "a heading's marks");
        assert_eq!(token("# Heading", 9), None);
        assert_eq!(token("#a@b", 4), Some((0, Hash, "a@b")));
    }

    #[test]
    fn a_colon_and_a_letter_start_one_for_emoji() {
        use Trigger::Colon;
        assert_eq!(token("ship it :f", 10), Some((8, Colon, "f")));
        assert_eq!(token(":+1", 3), Some((0, Colon, "+1")));
        assert_eq!(token(":", 1), None, "nothing typed yet");
        assert_eq!(token("nice :)", 7), None, "a smiley");
        assert_eq!(token("Note:fi", 7), None, "not inside a word");
        assert_eq!(token(":fire:", 6), None, "closed");
        assert_eq!(token("::fire", 6), None);
    }

    fn reference(number: u64, title: &str) -> IssueReference {
        IssueReference {
            number,
            title: title.into(),
            state: perch_github::SubjectState::IssueOpen,
        }
    }

    fn numbers(hits: &[ReferenceHit]) -> Vec<u64> {
        hits.iter().map(|h| h.reference.number).collect()
    }

    #[test]
    fn nothing_typed_after_a_hash_offers_the_recent_ones_in_order() {
        let recent: Vec<IssueReference> = (0..15).map(|n| reference(100 - n, "x")).collect();
        let hits = references("", &recent, true, &recent);
        assert_eq!(numbers(&hits), (91..=100).rev().collect::<Vec<_>>());
    }

    #[test]
    fn a_number_the_digits_start_comes_before_a_title_that_has_them() {
        let recent = [
            reference(7, "Bump 63 deps"),
            reference(634, "Add RFC for CDC"),
            reference(63, "Fence"),
            reference(1634, "Other"),
        ];
        let hits = references("63", &[], false, &recent);
        assert_eq!(numbers(&hits), [63, 634, 7]);
        assert_eq!(hits[1].number_matches, [0..3]);
        assert_eq!(hits[2].title_matches, [5..7]);
    }

    #[test]
    fn a_title_word_that_starts_with_the_letters_outranks_one_that_has_them() {
        let recent = [
            reference(1, "Surf the web"),
            reference(2, "RFC 0029"),
            reference(3, "Nothing"),
        ];
        let hits = references("rf", &[], false, &recent);
        assert_eq!(numbers(&hits), [2, 1]);
        assert_eq!(hits[0].title_matches, [0..2]);
    }

    #[test]
    fn githubs_search_is_taken_at_its_word_only_for_its_query() {
        // GitHub matched a stem the list cannot see.
        let found = [reference(9, "Caching managers")];
        let recent = [
            reference(1, "Cache manager"),
            reference(9, "Caching managers"),
        ];
        assert_eq!(numbers(&references("managed", &found, true, &recent)), [9]);
        assert!(references("managed", &found, false, &recent).is_empty());
        assert_eq!(
            numbers(&references("cach", &found, true, &recent)),
            [9, 1],
            "each once, GitHub's first within a rank"
        );
    }

    #[test]
    fn only_a_login_or_a_name_is_asked_of_github() {
        assert!(names_someone("cadonna"));
        assert!(names_someone("gunnar-morling"));
        assert!(!names_someone("src/sl"));
        assert!(!names_someone("lib.rs"));
        assert!(!names_someone(&"a".repeat(40)));
    }

    #[test]
    fn the_items_people_come_first_then_githubs_and_never_the_reader() {
        let own = [
            person("criccomini", None),
            person("agavra", None),
            person("dependabot[bot]", None),
        ];
        let found = [
            person("almog", Some("Almog Gavra")),
            person("agavra", Some("Almog Gavra")),
            person("apurva", None),
        ];
        let hits = people("a", &own, &found, true, Some("criccomini"));
        assert_eq!(logins(&hits), ["agavra", "almog", "apurva"]);
        assert_eq!(
            hits[0].person.name.as_deref(),
            Some("Almog Gavra"),
            "the name GitHub has for them"
        );
        assert_eq!(hits[0].login_matches, [0..1]);
    }

    #[test]
    fn a_login_that_starts_with_the_query_outranks_a_name_and_a_login_that_has_it() {
        let own = [
            person("xcad", None),
            person("bruno", Some("Bruno Cadonna")),
            person("cadonna", None),
        ];
        let hits = people("cad", &own, &[], false, None);
        assert_eq!(logins(&hits), ["cadonna", "bruno", "xcad"]);
        assert_eq!(hits[1].name_matches, [6..9]);
        assert_eq!(hits[2].login_matches, [1..4]);
    }

    #[test]
    fn nothing_typed_offers_only_the_items_people() {
        let hits = people("", &[person("a", None)], &[person("b", None)], true, None);
        assert_eq!(logins(&hits), ["a"]);
    }

    #[test]
    fn github_is_taken_at_its_word_only_for_the_query_it_answered() {
        let found = [person("octo", Some("Mona Lisa"))];
        // GitHub matched `mona` on something the list cannot see.
        assert_eq!(logins(&people("zz", &[], &found, true, None)), ["octo"]);
        assert!(people("zz", &[], &found, false, None).is_empty());
    }

    #[test]
    fn at_most_five_people_are_offered() {
        let own: Vec<Person> = (0..8).map(|i| person(&format!("a{i}"), None)).collect();
        assert_eq!(people("a", &own, &[], false, None).len(), MAX_PEOPLE);
    }

    #[test]
    fn a_query_github_has_not_answered_uses_the_longest_one_before_it() {
        let user = |login: &str| MentionableUser {
            login: login.into(),
            name: None,
            avatar_url: String::new(),
        };
        let mut found = HashMap::new();
        found.insert("c".to_string(), Some(Arc::new(vec![user("cadonna")])));
        found.insert("ca".to_string(), None);
        let (people, exact) = found_for("cad", &found);
        assert_eq!(people, [user("cadonna")]);
        assert!(!exact);
        let (_, exact) = found_for("c", &found);
        assert!(exact);
        assert_eq!(
            found_for("x", &found),
            (Vec::<MentionableUser>::new(), false)
        );
    }

    #[test]
    fn the_list_hangs_under_the_at_or_over_it_near_the_bottom() {
        let window = (1000.0, 800.0);
        assert_eq!(
            placement((100.0, 200.0, 220.0), (400.0, 100.0), window),
            (100.0, 224.0)
        );
        assert_eq!(
            placement((100.0, 700.0, 720.0), (400.0, 100.0), window),
            (100.0, 596.0)
        );
        // Kept in from the right edge.
        assert_eq!(
            placement((900.0, 200.0, 220.0), (400.0, 100.0), window).0,
            592.0
        );
    }
}
