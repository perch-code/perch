//! Syntax highlighting with tree-sitter.
//!
//! Produces byte-range spans tagged with a capture name. Mapping a capture to
//! a colour is the caller's job, so this crate carries no theme and no UI
//! dependency.

use std::sync::OnceLock;

use tree_sitter_highlight::{Highlight, HighlightConfiguration, HighlightEvent, Highlighter};

/// Capture names requested from every grammar, in priority order.
///
/// tree-sitter reports a capture as an index into this list, so the order
/// here is the order a theme must use.
pub const CAPTURES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    // Markdown, from both the block and the inline grammar. Appended, so
    // every earlier index keeps its colour.
    "punctuation.special",
    "text.emphasis",
    "text.literal",
    "text.reference",
    "text.strong",
    "text.title",
    "text.uri",
];

/// The name of a capture index, for a styling that cares about more than
/// colour.
pub fn capture_name(index: usize) -> &'static str {
    CAPTURES.get(index).copied().unwrap_or("")
}

/// A highlighted run of bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    /// Index into [`CAPTURES`].
    pub capture: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Json,
    Markdown,
    Toml,
    Yaml,
    Go,
    /// JavaScript, JSX among it.
    JavaScript,
    TypeScript,
    Tsx,
    Java,
}

impl Language {
    /// Pick a grammar from a file path. `None` renders as plain text.
    pub fn from_path(path: &str) -> Option<Self> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());

        match ext.as_deref() {
            Some("rs") => Some(Language::Rust),
            Some("json") => Some(Language::Json),
            Some("md" | "markdown") => Some(Language::Markdown),
            Some("toml") => Some(Language::Toml),
            Some("yaml" | "yml") => Some(Language::Yaml),
            Some("go") => Some(Language::Go),
            Some("js" | "mjs" | "cjs" | "jsx") => Some(Language::JavaScript),
            Some("ts" | "mts" | "cts") => Some(Language::TypeScript),
            Some("tsx") => Some(Language::Tsx),
            Some("java") => Some(Language::Java),
            // Files whose name carries the type rather than an extension.
            _ => match name {
                "Cargo.lock" => Some(Language::Toml),
                _ => None,
            },
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::Json => "JSON",
            Language::Markdown => "Markdown",
            Language::Toml => "TOML",
            Language::Yaml => "YAML",
            Language::Go => "Go",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Tsx => "TSX",
            Language::Java => "Java",
        }
    }

    /// The grammar a fenced block's language names, as GitHub reads it.
    pub fn from_fence(lang: &str) -> Option<Self> {
        match lang.to_ascii_lowercase().as_str() {
            "rust" | "rs" => Some(Language::Rust),
            "json" => Some(Language::Json),
            "markdown" | "md" => Some(Language::Markdown),
            "toml" => Some(Language::Toml),
            "yaml" | "yml" => Some(Language::Yaml),
            "go" | "golang" => Some(Language::Go),
            "javascript" | "js" | "jsx" | "mjs" | "cjs" => Some(Language::JavaScript),
            "typescript" | "ts" | "mts" | "cts" => Some(Language::TypeScript),
            "tsx" => Some(Language::Tsx),
            "java" => Some(Language::Java),
            _ => None,
        }
    }

    fn config(self) -> Option<&'static HighlightConfiguration> {
        macro_rules! grammar {
            ($cell:ident, $lang:expr, $name:expr, $hl:expr, $inj:expr, $loc:expr) => {{
                static $cell: OnceLock<Option<HighlightConfiguration>> = OnceLock::new();
                $cell
                    .get_or_init(|| {
                        let mut config =
                            HighlightConfiguration::new($lang.into(), $name, $hl, $inj, $loc)
                                .map_err(|e| {
                                    tracing::warn!(language = $name, error = %e, "grammar failed to load");
                                })
                                .ok()?;
                        config.configure(CAPTURES);
                        Some(config)
                    })
                    .as_ref()
            }};
        }

        match self {
            Language::Rust => grammar!(
                RUST,
                tree_sitter_rust::LANGUAGE,
                "rust",
                tree_sitter_rust::HIGHLIGHTS_QUERY,
                tree_sitter_rust::INJECTIONS_QUERY,
                ""
            ),
            Language::Json => grammar!(
                JSON,
                tree_sitter_json::LANGUAGE,
                "json",
                tree_sitter_json::HIGHLIGHTS_QUERY,
                "",
                ""
            ),
            Language::Markdown => grammar!(
                MD,
                tree_sitter_md::LANGUAGE,
                "markdown",
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
                tree_sitter_md::INJECTION_QUERY_BLOCK,
                ""
            ),
            Language::Toml => grammar!(
                TOML,
                tree_sitter_toml_ng::LANGUAGE,
                "toml",
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
                "",
                ""
            ),
            Language::Yaml => grammar!(
                YAML,
                tree_sitter_yaml::LANGUAGE,
                "yaml",
                tree_sitter_yaml::HIGHLIGHTS_QUERY,
                "",
                ""
            ),
            Language::Go => grammar!(
                GO,
                tree_sitter_go::LANGUAGE,
                "go",
                tree_sitter_go::HIGHLIGHTS_QUERY,
                "",
                ""
            ),
            Language::JavaScript => grammar!(
                JAVASCRIPT,
                tree_sitter_javascript::LANGUAGE,
                "javascript",
                &JAVASCRIPT_HIGHLIGHTS,
                tree_sitter_javascript::INJECTIONS_QUERY,
                tree_sitter_javascript::LOCALS_QUERY
            ),
            Language::TypeScript => grammar!(
                TYPESCRIPT,
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
                "typescript",
                &TYPESCRIPT_HIGHLIGHTS,
                tree_sitter_javascript::INJECTIONS_QUERY,
                &TYPESCRIPT_LOCALS
            ),
            Language::Tsx => grammar!(
                TSX,
                tree_sitter_typescript::LANGUAGE_TSX,
                "tsx",
                &TSX_HIGHLIGHTS,
                tree_sitter_javascript::INJECTIONS_QUERY,
                &TYPESCRIPT_LOCALS
            ),
            Language::Java => grammar!(
                JAVA,
                tree_sitter_java::LANGUAGE,
                "java",
                tree_sitter_java::HIGHLIGHTS_QUERY,
                "",
                ""
            ),
        }
    }
}

// TypeScript's queries cover only what it adds to JavaScript, so each is
// its own put before JavaScript's: the first pattern that matches a node
// wins. JSX's are JavaScript's own grammar's, and TSX's too.
static JAVASCRIPT_HIGHLIGHTS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    [
        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
        tree_sitter_javascript::HIGHLIGHT_QUERY,
    ]
    .join("\n")
});
static TYPESCRIPT_HIGHLIGHTS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    [
        tree_sitter_typescript::HIGHLIGHTS_QUERY,
        tree_sitter_javascript::HIGHLIGHT_QUERY,
    ]
    .join("\n")
});
static TSX_HIGHLIGHTS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    [
        tree_sitter_typescript::HIGHLIGHTS_QUERY,
        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
        tree_sitter_javascript::HIGHLIGHT_QUERY,
    ]
    .join("\n")
});
static TYPESCRIPT_LOCALS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    [
        tree_sitter_typescript::LOCALS_QUERY,
        tree_sitter_javascript::LOCALS_QUERY,
    ]
    .join("\n")
});

/// The inline half of the markdown grammar.
fn markdown_inline() -> Option<&'static HighlightConfiguration> {
    static INLINE: OnceLock<Option<HighlightConfiguration>> = OnceLock::new();
    INLINE
        .get_or_init(|| {
            let mut config = HighlightConfiguration::new(
                tree_sitter_md::INLINE_LANGUAGE.into(),
                "markdown_inline",
                tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
                tree_sitter_md::INJECTION_QUERY_INLINE,
                "",
            )
            .map_err(|e| tracing::warn!(error = %e, "markdown inline grammar failed to load"))
            .ok()?;
            config.configure(CAPTURES);
            Some(config)
        })
        .as_ref()
}

/// Highlight a whole file into byte-range spans.
///
/// Returns an empty vector for an unknown language or a grammar error, which
/// renders as unhighlighted text rather than failing the open.
pub fn highlight(language: Language, source: &str) -> Vec<Span> {
    let Some(config) = language.config() else {
        return Vec::new();
    };

    let mut highlighter = Highlighter::new();
    // Markdown's emphasis, code spans and links live in a second grammar
    // the block grammar injects; everything else injects nothing.
    let events = match highlighter.highlight(config, source.as_bytes(), None, None, |name| {
        if name == "markdown_inline" { markdown_inline() } else { None }
    }) {
        Ok(events) => events,
        Err(e) => {
            tracing::warn!(language = language.name(), error = %e, "highlight failed");
            return Vec::new();
        }
    };

    let mut spans = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    for event in events {
        match event {
            Ok(HighlightEvent::HighlightStart(Highlight(index))) => stack.push(index),
            Ok(HighlightEvent::HighlightEnd) => {
                stack.pop();
            }
            Ok(HighlightEvent::Source { start, end }) => {
                // The innermost capture wins, which is what a theme expects.
                if let Some(&capture) = stack.last() {
                    if start < end {
                        spans.push(Span { start, end, capture });
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "highlight event error");
                break;
            }
        }
    }

    spans
}

/// Markdown, with inline styles.
///
/// The block grammar injects an inline grammar for emphasis, code spans and
/// links, but tree-sitter-highlight emits nothing from that layer here, so
/// the inline grammar is run over the text directly and its spans dropped
/// inside fenced code. For a question in a text box, that is the whole job.
pub fn highlight_markdown(source: &str) -> Vec<Span> {
    let mut spans = highlight(Language::Markdown, source);

    let Some(inline) = markdown_inline() else {
        return spans;
    };
    let fences = fenced_ranges(source);
    let mut highlighter = Highlighter::new();
    let events = match highlighter.highlight(inline, source.as_bytes(), None, None, |_| None) {
        Ok(events) => events,
        Err(_) => return spans,
    };
    let mut stack: Vec<usize> = Vec::new();
    for event in events.flatten() {
        match event {
            HighlightEvent::HighlightStart(Highlight(index)) => stack.push(index),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                if let Some(&capture) = stack.last() {
                    let in_fence = fences.iter().any(|f| f.contains(&start));
                    if start < end && !in_fence {
                        spans.push(Span { start, end, capture });
                    }
                }
            }
        }
    }
    spans.sort_by_key(|s| (s.start, s.end));
    spans
}

/// Byte ranges of the content of fenced code blocks, fences included.
fn fenced_ranges(source: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut open: Option<usize> = None;
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            match open.take() {
                Some(start) => out.push(start..offset + line.len()),
                None => open = Some(offset),
            }
        }
        offset += line.len();
    }
    if let Some(start) = open {
        out.push(start..source.len());
    }
    out
}

/// Spans grouped per line, with byte offsets relative to the line start.
///
/// The editor styles one line at a time, so doing the split once here keeps
/// the per-line path free of searching.
#[derive(Debug, Clone, Default)]
pub struct LineSpans {
    lines: Vec<Vec<Span>>,
}

impl LineSpans {
    pub fn new(source: &str, spans: &[Span]) -> Self {
        // Byte offset where each line starts.
        let mut starts = vec![0usize];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        let line_count = source.lines().count().max(1);
        let mut lines = vec![Vec::new(); line_count];

        for span in spans {
            // The line this span begins on.
            let line = match starts.binary_search(&span.start) {
                Ok(i) => i,
                Err(i) => i.saturating_sub(1),
            };
            if line >= line_count {
                continue;
            }

            // A span crossing a newline is cut at each boundary so every
            // piece is addressable in its own line's coordinates.
            let mut current_line = line;
            let mut start = span.start;
            while start < span.end && current_line < line_count {
                let line_start = starts[current_line];
                let line_end = starts
                    .get(current_line + 1)
                    .copied()
                    .unwrap_or(source.len() + 1)
                    .saturating_sub(1)
                    .min(source.len());

                let piece_end = span.end.min(line_end);
                if piece_end > start {
                    lines[current_line].push(Span {
                        start: start - line_start,
                        end: piece_end - line_start,
                        capture: span.capture,
                    });
                }

                current_line += 1;
                start = starts.get(current_line).copied().unwrap_or(span.end);
            }
        }

        Self { lines }
    }

    pub fn line(&self, line: usize) -> &[Span] {
        self.lines.get(line).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_highlighting_covers_headings_inline_styles_and_skips_fences() {
        let src = "# Title\n\nplain **bold** and `code` and *em*\n\n```\n**not bold**\n```\n";
        let spans = highlight_markdown(src);
        let names: Vec<(&str, &str)> =
            spans.iter().map(|s| (super::capture_name(s.capture), &src[s.start..s.end])).collect();
        assert!(names.contains(&("text.title", "Title")), "{names:?}");
        assert!(names.contains(&("text.strong", "bold")), "{names:?}");
        assert!(names.contains(&("text.literal", "code")), "{names:?}");
        assert!(names.contains(&("text.emphasis", "em")), "{names:?}");
        // Inside a fence the block grammar's literal wins; nothing there is
        // strong.
        assert!(names.contains(&("text.literal", "**not bold**\n")), "{names:?}");
        assert!(!names.iter().any(|(n, t)| *n == "text.strong" && t.contains("not bold")), "{names:?}");
    }

    fn capture_name(span: &Span) -> &'static str {
        CAPTURES[span.capture]
    }

    #[test]
    fn extensions_select_a_grammar() {
        assert_eq!(Language::from_path("src/main.rs"), Some(Language::Rust));
        assert_eq!(Language::from_path("Cargo.toml"), Some(Language::Toml));
        assert_eq!(Language::from_path("a/b/c.json"), Some(Language::Json));
        assert_eq!(Language::from_path("README.md"), Some(Language::Markdown));
        assert_eq!(Language::from_path("ci.yml"), Some(Language::Yaml));
        assert_eq!(Language::from_path("cmd/main.go"), Some(Language::Go));
        assert_eq!(Language::from_path("src/index.js"), Some(Language::JavaScript));
        assert_eq!(Language::from_path("src/App.jsx"), Some(Language::JavaScript));
        assert_eq!(Language::from_path("lib/a.mjs"), Some(Language::JavaScript));
        assert_eq!(Language::from_path("src/index.ts"), Some(Language::TypeScript));
        assert_eq!(Language::from_path("src/a.d.mts"), Some(Language::TypeScript));
        assert_eq!(Language::from_path("src/App.tsx"), Some(Language::Tsx));
        assert_eq!(Language::from_path("src/Main.java"), Some(Language::Java));
    }

    #[test]
    fn a_fence_names_its_grammar() {
        assert_eq!(Language::from_fence("Go"), Some(Language::Go));
        assert_eq!(Language::from_fence("ts"), Some(Language::TypeScript));
        assert_eq!(Language::from_fence("tsx"), Some(Language::Tsx));
        assert_eq!(Language::from_fence("javascript"), Some(Language::JavaScript));
        assert_eq!(Language::from_fence("java"), Some(Language::Java));
        assert_eq!(Language::from_fence("diff"), None);
    }

    #[test]
    fn cargo_lock_is_toml_despite_its_extension() {
        assert_eq!(Language::from_path("Cargo.lock"), Some(Language::Toml));
    }

    #[test]
    fn unknown_extensions_render_as_plain_text() {
        assert_eq!(Language::from_path("LICENSE"), None);
        assert_eq!(Language::from_path("a.xyz"), None);
    }

    #[test]
    fn rust_keywords_and_strings_are_captured() {
        let src = r#"fn main() { let x = "hello"; }"#;
        let spans = highlight(Language::Rust, src);
        assert!(!spans.is_empty(), "the rust grammar produced no spans");

        let names: Vec<&str> = spans.iter().map(capture_name).collect();
        assert!(names.iter().any(|n| n.starts_with("keyword")), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("string")), "{names:?}");
    }

    #[test]
    fn rust_comments_are_captured() {
        let spans = highlight(Language::Rust, "// a note\nfn f() {}\n");
        assert!(spans.iter().any(|s| capture_name(s) == "comment"));
    }

    #[test]
    fn spans_stay_within_the_source() {
        let src = "fn main() {}\n";
        for s in highlight(Language::Rust, src) {
            assert!(s.end <= src.len());
            assert!(s.start < s.end);
        }
    }

    #[test]
    fn every_grammar_loads_and_produces_spans() {
        let cases = [
            (Language::Rust, "fn f() {}\n"),
            (Language::Json, "{\"a\": 1}\n"),
            (Language::Toml, "[table]\nkey = 1\n"),
            (Language::Yaml, "key: value\n"),
            (Language::Markdown, "# Heading\n\ntext\n"),
            (Language::Go, "package main\n\nfunc f() int { return 1 }\n"),
            (Language::JavaScript, "const a = () => <div className=\"x\">{1}</div>;\n"),
            (Language::TypeScript, "interface A { b: number }\nconst c: A = { b: 1 };\n"),
            (Language::Tsx, "const a = (p: { n: number }) => <b>{p.n}</b>;\n"),
            (Language::Java, "class A { int f() { return 1; } }\n"),
        ];
        for (lang, src) in cases {
            assert!(lang.config().is_some(), "{} grammar failed to load", lang.name());
            assert!(
                !highlight(lang, src).is_empty(),
                "{} produced no spans for {src:?}",
                lang.name()
            );
        }
    }

    #[test]
    fn broken_source_still_returns_rather_than_panicking() {
        let spans = highlight(Language::Rust, "fn ( { ] unclosed \"string");
        let _ = spans;
    }

    #[test]
    fn line_spans_are_relative_to_their_line() {
        let src = "fn a() {}\nfn b() {}\n";
        let spans = highlight(Language::Rust, src);
        let by_line = LineSpans::new(src, &spans);

        assert_eq!(by_line.len(), 2);
        for line in 0..2 {
            for s in by_line.line(line) {
                assert!(s.end <= 9, "line {line} span {s:?} runs past the line");
            }
        }
        assert!(!by_line.line(0).is_empty());
        assert!(!by_line.line(1).is_empty());
    }

    #[test]
    fn a_span_crossing_a_newline_is_split_per_line() {
        let src = "/* one\n   two */\nfn f() {}\n";
        let spans = highlight(Language::Rust, src);
        let by_line = LineSpans::new(src, &spans);

        // The block comment covers lines 0 and 1, and each piece must be
        // addressable in its own line's coordinates.
        assert!(!by_line.line(0).is_empty());
        assert!(!by_line.line(1).is_empty());
        for s in by_line.line(1) {
            assert!(s.start <= "   two */".len());
        }
    }

    #[test]
    fn asking_beyond_the_last_line_is_empty_not_a_panic() {
        let by_line = LineSpans::new("fn f() {}\n", &highlight(Language::Rust, "fn f() {}\n"));
        assert!(by_line.line(999).is_empty());
    }

    #[test]
    fn an_empty_file_has_one_empty_line() {
        let by_line = LineSpans::new("", &[]);
        assert_eq!(by_line.len(), 1);
        assert!(by_line.line(0).is_empty());
    }
}
