#!/usr/bin/env python3
"""Build the static homepage, guide pages, and search index.

Re-run after changing the site templates or guide sources in docs/:
    python3 website/build_guide.py
"""
from __future__ import annotations

import html
import json
from html.parser import HTMLParser
import pathlib
import re
from string import Template

ROOT = pathlib.Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"
WEBSITE = ROOT / "website"
GUIDE = WEBSITE / "guide"
TEMPLATES = WEBSITE / "templates"

SEARCH_FORM = """<form class="guide-search" role="search" aria-label="Field guide" action="index.html" method="get">
      <input type="search" name="q" aria-label="Search guide" placeholder="Search the guide" autocomplete="off">
      <button type="submit" aria-label="Search the guide"><svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" aria-hidden="true"><circle cx="10" cy="10" r="6.5"/><path d="m15 15 5 5"/></svg></button>
    </form>"""


PARTS = [
    {
        "title": "Get set up",
        "blurb": "Install the app, sign in to GitHub, connect a repository, or attach an SSH host.",
        "pages": [
            {
                "slug": "install",
                "title": "Install Perch",
                "kind": "chapter",
                "desc": "Install Perch on macOS or Linux, sign in to GitHub, and add your first repository.",
                "lede": "Five minutes from a fresh terminal to your GitHub inbox in a native window.",
                "handwritten": True,
            },
            {
                "slug": "authentication",
                "title": "Authentication",
                "kind": "reference",
                "desc": "How Perch resolves a GitHub token from a GUI launch and which repository permissions each action needs.",
                "lede": "How Perch finds a GitHub token from a desktop launch, and what permissions reviews, merges, and issues need.",
                "sources": [("reference.md", ["Authentication"], False)],
                "promote": True,
                "aliases": ["authentication"],
                "inject_headings": [
                    ("Perch calls GitHub's API directly.", "## Credential lookup order"),
                    ("Using `gh auth login` is the simplest", "## Desktop launches and login shells"),
                    ("The credential\nmust be allowed", "## Required GitHub permissions"),
                ],
            },
            {
                "slug": "remote-engine",
                "title": "Remote hosts (SSH)",
                "kind": "reference",
                "desc": "Run Perch's headless engine over SSH on a Linux or Mac machine while keeping native windows on your laptop.",
                "lede": "Keep checkouts, language servers, and agent runs on an SSH host while driving native Perch windows locally.",
                "custom_builder": "remote_engine",
                "aliases": ["an-engine-on-another-machine"],
            },
        ],
    },
    {
        "title": "Review a change",
        "blurb": "Triage the inbox, follow a symbol, draft feedback, and submit a review.",
        "pages": [
            {
                "slug": "first-review",
                "title": "Review your first pull request",
                "kind": "chapter",
                "desc": "A walkthrough of reviewing a pull request in Perch: read the thread, follow a symbol, ask an agent, and post a review.",
                "lede": "Trace a changed symbol through the codebase, ask an agent about it privately, and submit your review.",
                "handwritten": True,
            },
            {
                "slug": "inbox",
                "title": "The inbox",
                "kind": "reference",
                "desc": "How Perch polls GitHub notifications, keeps your sidebar list steady while you read, and surfaces finished local agent runs.",
                "lede": "Notifications, read and Done state, snoozing, filter menus, and local inbox rows for finished agent runs.",
                "sources": [("reference.md", ["GitHub inbox"], False)],
                "promote": True,
                "aliases": ["github-inbox"],
                "inject_headings": [
                    ("Perch builds its inbox", "## Polling and screen stability"),
                    ("- Viewing a pull request or an issue", "## Read state, Done, and snoozing"),
                ],
            },
            {
                "slug": "code-navigation",
                "title": "Code navigation",
                "kind": "reference",
                "desc": "Conversation and Code modes, the file tree drawer, Go to File, language server navigation, and clickable code links.",
                "lede": "Switch between Conversation and Code, filter the file tree, jump to symbols with a language server, and click symbols in comments.",
                "sources": [
                    (
                        "reference.md",
                        [
                            "Conversation and Code",
                            "The file tree",
                            "Going to a file",
                            "Code navigation",
                            "Links to code",
                        ],
                        False,
                    )
                ],
                "rename_headings": [
                    ("## Code navigation", "## Language server navigation"),
                ],
                "heading_aliases": {
                    "language-server-navigation": ["code-navigation"],
                },
                "promote": False,
                "aliases": [],
            },
            {
                "slug": "posting",
                "title": "Posting and drafts",
                "kind": "reference",
                "desc": "The unified timeline, reactions, @-mentions, offline review drafts, inline suggestions, and Perch-only replies.",
                "lede": "Read commits and events in time order, draft line comments offline, apply suggestions, and choose what reaches GitHub.",
                "sources": [
                    ("reference.md", ["Timeline", "Reactions", "Posting"], False)
                ],
                "rename_headings": [
                    ("## Posting", "## Composers, @-mentions, and destinations"),
                ],
                "heading_aliases": {
                    "composers-mentions-and-destinations": ["posting"],
                },
                "inject_headings": [
                    ("On a line of a file, or under a thread", "## Offline review drafts"),
                    ("A composer on lines has **Suggest**", "## Inline suggestions"),
                    ("At the foot of Conversation, a message to GitHub", "## Comments and closing items"),
                    ("GitHub's review threads are drawn beside", "## Review threads and Perch-only replies"),
                ],
                "promote": False,
                "aliases": [],
            },
            {
                "slug": "worktree-reviews",
                "title": "Agent reviews of a worktree",
                "kind": "reference",
                "desc": "Run Claude Code and Codex reviews on immutable snapshots, reconcile findings across runs, and turn findings into GitHub drafts.",
                "lede": "Run Claude Code and Codex against an immutable branch or working-tree snapshot and turn findings into review drafts.",
                "sources": [("reference.md", ["Worktree reviews"], False)],
                "promote": True,
                "aliases": ["worktree-reviews"],
                "inject_headings": [
                    ("**Review** in the bar opens", "## The Review popover"),
                    ("Running the agents starts", "## Diff scopes and instructions"),
                    ("Every review uses a clean", "## Immutable snapshots and reconciling findings"),
                    ("On an open pull request, a new finding", "## Publishing findings as GitHub drafts"),
                ],
            },
        ],
    },
    {
        "title": "Start work",
        "blurb": "Take an issue into a worktree, change it with an agent, and open a pull request.",
        "pages": [
            {
                "slug": "first-branch",
                "title": "Start work on an issue",
                "kind": "chapter",
                "desc": "Cut an isolated git worktree from an issue or Cmd-N, let an agent edit in a sandbox, inspect the diff, and open a pull request.",
                "lede": "Cut an isolated branch worktree, let an agent make the change inside a sandbox, inspect the diff, and open a pull request.",
                "handwritten": True,
            },
            {
                "slug": "starting-work",
                "title": "Worktrees",
                "kind": "reference",
                "desc": "Create isolated git worktrees from Cmd-N, an issue, or a pull request, and keep uncommitted or unpushed work safe.",
                "lede": "Cut a branch into its own worktree from a prompt, an issue, or any pull request without disturbing your main clone.",
                "sources": [("reference.md", ["Starting work"], False)],
                "promote": True,
                "aliases": ["starting-work"],
                "inject_headings": [
                    ("`+` at the top of the sidebar", "## New worktree composer"),
                ],
            },
            {
                "slug": "opening-a-pull-request",
                "title": "Opening a pull request",
                "kind": "reference",
                "desc": "Draft titles and descriptions from repository templates or an agent, push branches safely, and turn a worktree into a pull request.",
                "lede": "Fill the title and description from templates or an agent, push with your own git credentials, and turn the worktree into a PR.",
                "sources": [("reference.md", ["Opening a pull request"], False)],
                "promote": True,
                "aliases": ["opening-a-pull-request"],
                "inject_headings": [
                    ("A worktree of your own with commits", "## Filling the pull request form"),
                    ("The form fills itself once", "## Pushing and creating on GitHub"),
                    ("On a pull request of yours whose worktree", "## Pushing new commits to an open PR"),
                ],
            },
            {
                "slug": "agents",
                "title": "Agent sandbox and approvals",
                "kind": "reference",
                "desc": "Ask and Work modes, read-only and worktree sandboxes, secret stripping, protected .git paths, and command approvals.",
                "lede": "How Ask, Work, and Review runs are sandboxed, which secrets and paths stay off-limits, and how approvals and repository grants work.",
                "custom_builder": "agents_sandbox",
                "heading_aliases": {
                    "read-only-sandbox-and-untrusted-text": ["what-the-agent-can-and-cannot-do"],
                    "work-mode": ["work"],
                    "approvals-and-repository-grants": ["approvals"],
                },
                "aliases": ["agents"],
            },
            {
                "slug": "agent-context",
                "title": "Agent context and knowledge",
                "kind": "reference",
                "desc": "What Perch puts in each agent prompt, perch context for external terminals, synced GitHub history, and external Markdown knowledge.",
                "lede": "What Perch tells an agent about GitHub discussions, how to run perch context from a terminal, and how synced history and external notes work.",
                "custom_builder": "agent_context",
                "heading_aliases": {
                    "discussion-context-in-prompts": ["what-the-agent-is-told-about-the-discussion"],
                },
                "aliases": [],
            },
        ],
    },
    {
        "title": "Preferences and shortcuts",
        "blurb": "Settings, config.toml keys, keyboard bindings, and headless engine details.",
        "pages": [
            {
                "slug": "settings",
                "title": "Settings window",
                "kind": "reference",
                "desc": "Live-reloading preferences in the Settings window, reset controls, and parse-error handling.",
                "lede": "How the Settings window edits ~/.perch/config.toml in place while Perch runs.",
                "sources": [("reference.md", ["Settings"], False)],
                "promote": True,
                "aliases": ["settings"],
                "inject_headings": [
                    ("`Cmd-,` (`Ctrl-,` off macOS)", "## Sections and search"),
                    ("The environment choice under **This window**", "## Saving and live reload"),
                ],
            },
            {
                "slug": "configuration",
                "title": "Configuration file (config.toml)",
                "kind": "reference",
                "desc": "Repository roots, Work and LSP trust, every key in ~/.perch/config.toml, worktree directories, history sync, external knowledge, and OpenCode.",
                "lede": "Perch reads ~/.perch/config.toml (or $PERCH_HOME/config.toml). Every key is optional, and most edits apply within seconds.",
                "custom_builder": "configuration",
                "aliases": ["configuration"],
            },
            {
                "slug": "keyboard-shortcuts",
                "title": "Keyboard shortcuts",
                "kind": "reference",
                "desc": "Every default keyboard shortcut in Perch grouped by view, plus how to customize bindings in Settings or config.toml.",
                "lede": "Every action ID and default shortcut in Perch, and how to rebind keys in Settings or ~/.perch/config.toml.",
                "sources": [("reference.md", ["Keyboard shortcuts"], False)],
                "promote": True,
                "on_page_h3": True,
                "aliases": ["keyboard-shortcuts"],
                "inject_headings": [
                    ("Every key below can be changed", "## Customizing keybindings"),
                    ("Each action's id, its default keys", "## Default shortcuts by view"),
                ],
            },
            {
                "slug": "perchd",
                "title": "perchd and engine lifecycle",
                "kind": "reference",
                "desc": "Single-engine Perch home locks, headless perchd commands, bundled remote binaries, and replacing or sharing builds over SSH.",
                "lede": "How one engine locks a Perch home, how headless perchd serves and bridges over SSH, and how the app manages remote builds.",
                "custom_builder": "perchd",
                "heading_aliases": {
                    "headless-perchd-commands": ["perchd"],
                },
                "aliases": [],
            },
        ],
    },
]


def slugify(text: str) -> str:
    clean = re.sub(r"`([^`]*)`", r"\1", text)
    clean = clean.lower()
    clean = re.sub(r"[^\w\s-]", "", clean)
    clean = re.sub(r"\s+", "-", clean.strip())
    return clean


def split_top_sections(md_text: str) -> tuple[str, dict[str, str]]:
    lines = md_text.splitlines()
    preamble: list[str] = []
    sections: dict[str, list[str]] = {}
    current: str | None = None
    for line in lines:
        if line.startswith("# ") and current is None and not preamble:
            continue
        if line.startswith("## "):
            current = line[3:].strip()
            sections[current] = [line]
        elif current is None:
            preamble.append(line)
        else:
            sections[current].append(line)
    return "\n".join(preamble).strip(), {
        k: "\n".join(v).strip() for k, v in sections.items()
    }


def split_subsections(sec_md: str) -> tuple[str, dict[str, str]]:
    """Split a `## Section` block into (intro_without_h2, {h3_title: h3_block})."""
    lines = sec_md.splitlines()
    if lines and lines[0].startswith("## "):
        lines = lines[1:]
    intro: list[str] = []
    subs: dict[str, list[str]] = {}
    cur: str | None = None
    for line in lines:
        if line.startswith("### "):
            cur = line[4:].strip()
            subs[cur] = [line]
        elif cur is None:
            intro.append(line)
        else:
            subs[cur].append(line)
    return "\n".join(intro).strip(), {k: "\n".join(v).strip() for k, v in subs.items()}


def promote_block(md: str) -> str:
    """Promote ### -> ## and #### -> ###."""
    out: list[str] = []
    for line in md.splitlines():
        if line.startswith("#### "):
            out.append("### " + line[5:])
        elif line.startswith("### "):
            out.append("## " + line[4:])
        else:
            out.append(line)
    return "\n".join(out).strip()


def build_custom_markdown(
    builder: str,
    docs_cache: dict[str, tuple[str, dict[str, str]]],
) -> str:
    cfg_preamble, cfg_secs = docs_cache["configuration.md"]
    _, ref_secs = docs_cache["reference.md"]

    if builder == "remote_engine":
        _, subs = split_subsections(ref_secs["An engine on another machine"])
        order = [
            "Setting up a host",
            "Adding a repository on a host",
            "Connecting",
            "What runs where",
            "What the app checks",
            "Limits",
        ]
        blocks = [promote_block(subs[name]) for name in order]
        blocks.append(
            "For `perchd` CLI flags, socket locking, and how the app uploads and replaces remote engine binaries, see [perchd and engine lifecycle](perchd.html)."
        )
        return "\n\n".join(blocks)

    if builder == "perchd":
        _, remote_subs = split_subsections(ref_secs["An engine on another machine"])
        perchd_sec = re.sub(
            r"^## perchd\b",
            "## Headless perchd commands",
            ref_secs["perchd"],
            count=1,
        )
        return "\n\n".join(
            [
                ref_secs["One engine to a Perch home"],
                perchd_sec,
                promote_block(remote_subs["The app's own engine"]),
                promote_block(remote_subs["When the engine is another build"]),
            ]
        )

    if builder in ("agents_sandbox", "agent_context"):
        agents_intro, subs = split_subsections(ref_secs["Agents"])
        disc_full = subs["What the agent is told about the discussion"]
        # Split discussion prompt details vs. managing runs in the UI/CLI
        split_needle = "An item's runs are in the **Agents** row"
        if split_needle not in disc_full:
            raise ValueError("Expected Agents row needle in Agents section")
        disc_part, runs_part = disc_full.split(split_needle, 1)
        # Drop the `### What the agent is told about the discussion` line from disc_part
        disc_body = "\n".join(disc_part.splitlines()[1:]).strip()
        runs_body = (split_needle + runs_part).strip()

        if builder == "agents_sandbox":
            overview = (
                "## Overview\n\n"
                "Perch runs your locally installed coding-agent CLIs (Claude Code, Codex, Antigravity, and OpenCode) in three modes: "
                "**Ask** answers questions in a read-only sandbox with no network or secrets, **Work** edits files inside your own branch worktree "
                "with explicit approvals for anything outside the sandbox, and **Review** checks a diff snapshot for bugs. "
                "Nothing an agent produces reaches GitHub unless you post it. For prompt contents, `perch context`, and repository history, see [Agent context and knowledge](agent-context.html)."
            )
            asking = f"## Asking questions and managing runs\n\n{agents_intro}\n\n{runs_body}"
            readonly = re.sub(
                r"^### What the agent can and cannot do\b",
                "## Read-only sandbox and untrusted text",
                subs["What the agent can and cannot do"],
                count=1,
            )
            work = re.sub(
                r"^### Work\b",
                "## Work mode",
                subs["Work"],
                count=1,
            )
            approvals = re.sub(
                r"^### Approvals\b",
                "## Approvals and repository grants",
                subs["Approvals"],
                count=1,
            )
            without_sandbox = promote_block(subs["Without Claude Code's sandbox"])
            return "\n\n".join([overview, asking, readonly, without_sandbox, work, approvals])

        # builder == "agent_context"
        disc_sec = (
            "## Discussion context in prompts\n\n"
            + disc_body
            + "\n\nFor sandbox rules, secret stripping, and Work approvals, see [Agent sandbox and approvals](agents.html)."
        )
        return "\n\n".join(
            [
                disc_sec,
                promote_block(subs["Context for agents outside Perch"]),
                promote_block(subs["Repository history for agents"]),
                promote_block(subs["External knowledge for agents"]),
            ]
        )

    if builder == "configuration":
        # 1. Split reference.md Configuration into Repositories vs. Work/LSP/Display/Trust
        ref_cfg = ref_secs["Configuration"]
        ref_cfg = re.sub(r"^## Configuration\s+", "", ref_cfg, count=1)
        ref_cfg = ref_cfg.replace(
            "Perch reads `~/.perch/config.toml` (or `$PERCH_HOME/config.toml`). ",
            "",
            1,
        )
        work_needle = "New work (see [Starting work](#starting-work)) reads two more tables:"
        if work_needle not in ref_cfg:
            raise ValueError("Expected New work needle in reference.md Configuration")
        repos_part, rest_ref_cfg = ref_cfg.split(work_needle, 1)
        repos_sec = "## Repositories and local roots\n\n" + repos_part.strip()
        work_lsp_sec = (
            "## Work, language servers, and appearance\n\n"
            + work_needle
            + rest_ref_cfg
        ).strip()

        # 2. Split configuration.md preamble into All keys table vs. Environment variables & remote split
        settings_needle = "Settings (`Cmd-,`, or the gear at the foot of the sidebar)"
        env_needle = "`PERCH_HOME` is also where the cache"
        if settings_needle not in cfg_preamble or env_needle not in cfg_preamble:
            raise ValueError("Expected needles in configuration.md preamble")
        _, after_intro = cfg_preamble.split(env_needle, 1)
        env_remote_part, keys_part = (env_needle + after_intro).split(settings_needle, 1)
        keys_sec = "## All config.toml keys\n\n" + (settings_needle + keys_part).strip()
        env_sec = "## Environment variables and remote split\n\n" + env_remote_part.strip()

        return "\n\n".join(
            [
                repos_sec,
                work_lsp_sec,
                keys_sec,
                cfg_secs["Worktree directory"],
                cfg_secs["GitHub history"],
                cfg_secs["External knowledge"],
                cfg_secs["OpenCode"],
                env_sec,
            ]
        )

    raise ValueError(f"Unknown custom_builder: {builder}")


def extract_page_markdown(page: dict, docs_cache: dict[str, tuple[str, dict[str, str]]]) -> str:
    if page.get("custom_builder"):
        return build_custom_markdown(page["custom_builder"], docs_cache)

    chunks: list[str] = []
    for doc_name, sec_names, _ in page["sources"]:
        preamble, sections = docs_cache[doc_name]
        if sec_names is None:
            chunks.append(preamble)
            for sec_body in sections.values():
                chunks.append(sec_body)
        else:
            for name in sec_names:
                chunks.append(sections[name])
    md = "\n\n".join(chunks)

    if page.get("promote"):
        out_lines = []
        skipped_top = False
        for line in md.splitlines():
            if not skipped_top and line.startswith("## "):
                skipped_top = True
                continue
            if line.startswith("#### "):
                out_lines.append("### " + line[5:])
            elif line.startswith("### "):
                out_lines.append("## " + line[4:])
            else:
                out_lines.append(line)
        md = "\n".join(out_lines).strip()

    for old_h, new_h in page.get("rename_headings", []):
        if old_h not in md:
            raise ValueError(f"Heading {old_h!r} not found in {page['slug']}")
        md = md.replace(old_h, new_h, 1)

    for needle, heading in page.get("inject_headings", []):
        if needle not in md:
            raise ValueError(f"Needle {needle!r} not found in {page['slug']}")
        md = md.replace(needle, f"{heading}\n\n{needle}", 1)

    if page.get("preamble_md"):
        md = page["preamble_md"] + "\n\n" + md

    return md


def collect_headings(md: str) -> list[tuple[int, str, str]]:
    """Return [(level, id, plain_title)] including short bold group headings."""
    items: list[tuple[int, str, str]] = []
    used: set[str] = set()
    lines = md.splitlines()
    in_code = False
    for idx, line in enumerate(lines):
        if line.startswith("```"):
            in_code = not in_code
            continue
        if in_code:
            continue
        m = re.match(r"^(#{2,4})\s+(.+)$", line)
        if m:
            level = len(m.group(1))
            raw_title = m.group(2).strip()
            hid = slugify(raw_title)
            base = hid
            suffix = 2
            while hid in used:
                hid = f"{base}-{suffix}"
                suffix += 1
            used.add(hid)
            plain = re.sub(r"`([^`]*)`", r"\1", raw_title)
            items.append((level, hid, plain))
            continue

        # Detect standalone **Group** lines (e.g. Keyboard shortcuts view groups)
        prev_blank = idx == 0 or not lines[idx - 1].strip()
        next_blank = idx + 1 == len(lines) or not lines[idx + 1].strip()
        if prev_blank and next_blank and len(line.strip()) < 60:
            mg = re.match(r"^\*\*([^*]+)\*\*(.*)$", line.strip())
            if mg and (mg.group(2) == "" or mg.group(2).startswith(",")):
                gid = slugify(mg.group(1))
                if gid not in used:
                    used.add(gid)
                    items.append((3, gid, mg.group(1)))
    return items


def render_inline(text: str, current_slug: str, anchor_map: dict[str, str]) -> str:
    text = re.sub(r"\s*\n\s*", " ", text)

    tokens: list[tuple[str, str]] = []
    pos = 0
    code_re = re.compile(r"`` (.+?) ``|``(.+?)``|`([^`]+)`")
    for m in code_re.finditer(text):
        if m.start() > pos:
            tokens.append(("text", text[pos : m.start()]))
        code_content = next(g for g in m.groups() if g is not None)
        tokens.append(("code", f"<code>{html.escape(code_content)}</code>"))
        pos = m.end()
    if pos < len(text):
        tokens.append(("text", text[pos:]))

    def resolve_href(raw_href: str) -> str:
        href = raw_href.strip()
        if href.startswith(("http://", "https://", "mailto:")):
            return html.escape(href, quote=True)
        if href == "reference.md":
            return "index.html"
        if href == "configuration.md":
            return "configuration.html"
        m = re.match(r"^(?:reference\.md|configuration\.md)?#([a-z0-9_-]+)$", href)
        if m:
            anchor = m.group(1)
            target_slug = anchor_map.get(anchor)
            if not target_slug:
                raise ValueError(f"Unresolved doc anchor #{anchor} on {current_slug}")
            if target_slug == current_slug:
                return f"#{anchor}"
            return f"{target_slug}.html#{anchor}"
        return html.escape(href, quote=True)

    placeholder_map: dict[str, str] = {}
    work_buf: list[str] = []
    for idx, (kind, val) in enumerate(tokens):
        if kind == "code":
            key = f"\x00CODE{idx}\x00"
            placeholder_map[key] = val
            work_buf.append(key)
        else:
            work_buf.append(html.escape(val, quote=False))

    joined = "".join(work_buf)

    def repl_link(m: re.Match[str]) -> str:
        label_html = m.group(1)
        url = resolve_href(html.unescape(m.group(2)))
        return f'<a href="{url}">{label_html}</a>'

    joined = re.sub(r"\[([^\]]+)\]\(([^)]+)\)", repl_link, joined)
    joined = re.sub(r"\*\*([^*]+)\*\*", r"<b>\1</b>", joined)
    joined = re.sub(r"(?<!\w)\*([^*]+)\*(?!\w)", r"<em>\1</em>", joined)

    for key, code_html in placeholder_map.items():
        joined = joined.replace(key, code_html)

    return joined


def render_markdown(
    md: str,
    page: dict,
    anchor_map: dict[str, str],
) -> str:
    current_slug = page["slug"]
    heading_aliases: dict[str, list[str]] = page.get("heading_aliases", {})
    include_h3 = bool(page.get("on_page_h3"))
    lines = md.splitlines()
    out: list[str] = []
    i = 0
    n = len(lines)
    used_ids: set[str] = set()

    all_headings = collect_headings(md)
    jump_targets = [
        (hid, plain)
        for lvl, hid, plain in all_headings
        if lvl == 2 or (include_h3 and lvl == 3)
    ]
    if len(jump_targets) >= 2:
        links = "".join(
            f'<li><a href="#{hid}">{html.escape(plain)}</a></li>' for hid, plain in jump_targets
        )
        out.append(f'<nav class="on-page" aria-label="On this page"><details><summary>On this page</summary><ul>{links}</ul></details></nav>')

    while i < n:
        line = lines[i]
        if not line.strip():
            i += 1
            continue

        if line.startswith("```"):
            i += 1
            code_lines: list[str] = []
            while i < n and not lines[i].startswith("```"):
                code_lines.append(lines[i])
                i += 1
            if i < n and lines[i].startswith("```"):
                i += 1
            escaped = html.escape("\n".join(code_lines))
            out.append(f"<pre><code>{escaped}</code></pre>")
            continue

        m_head = re.match(r"^(#{2,4})\s+(.+)$", line)
        if m_head:
            level = len(m_head.group(1))
            raw_title = m_head.group(2).strip()
            hid = slugify(raw_title)
            base_id = hid
            suffix = 2
            while hid in used_ids:
                hid = f"{base_id}-{suffix}"
                suffix += 1
            used_ids.add(hid)
            alias_html = "".join(
                f'<span id="{a}"></span>' for a in heading_aliases.get(hid, [])
            )
            title_html = render_inline(raw_title, current_slug, anchor_map)
            out.append(f'<h{level} id="{hid}">{alias_html}{title_html}</h{level}>')
            i += 1
            continue

        if line.startswith("|") and i + 1 < n and re.match(r"^\|[\s:|-]+\|\s*$", lines[i + 1]):
            def split_row(row_line: str) -> list[str]:
                s = row_line.strip()
                if s.startswith("|"):
                    s = s[1:]
                if s.endswith("|"):
                    s = s[:-1]
                cells: list[str] = []
                cur: list[str] = []
                in_bt = False
                for ch in s:
                    if ch == "`":
                        in_bt = not in_bt
                        cur.append(ch)
                    elif ch == "|" and not in_bt:
                        cells.append("".join(cur).strip())
                        cur = []
                    else:
                        cur.append(ch)
                cells.append("".join(cur).strip())
                return cells

            headers = split_row(line)
            i += 2
            rows: list[list[str]] = []
            while i < n and lines[i].startswith("|"):
                rows.append(split_row(lines[i]))
                i += 1
            thead = "".join(
                f"<th>{render_inline(h, current_slug, anchor_map)}</th>"
                for h in headers
            )
            tbody_rows = []
            for r in rows:
                tds = "".join(
                    f"<td>{render_inline(c, current_slug, anchor_map)}</td>"
                    for c in r
                )
                tbody_rows.append(f"<tr>{tds}</tr>")
            out.append(
                f'<div class="table"><table><thead><tr>{thead}</tr></thead>'
                f'<tbody>{"".join(tbody_rows)}</tbody></table></div>'
            )
            continue

        m_ul = re.match(r"^-\s+(.*)$", line)
        m_ol = re.match(r"^\d+\.\s+(.*)$", line)
        if m_ul or m_ol:
            is_ol = bool(m_ol)
            tag = "ol" if is_ol else "ul"
            item_re = re.compile(r"^\d+\.\s+(.*)$" if is_ol else r"^-\s+(.*)$")
            items: list[list[str]] = []
            while i < n:
                l = lines[i]
                m_item = item_re.match(l)
                if m_item:
                    items.append([m_item.group(1)])
                    i += 1
                elif items and l.startswith("  ") and l.strip():
                    items[-1].append(l.strip())
                    i += 1
                else:
                    break
            lis = "".join(
                f"<li>{render_inline(' '.join(parts), current_slug, anchor_map)}</li>"
                for parts in items
            )
            out.append(f"<{tag}>{lis}</{tag}>")
            continue

        para_lines = [line]
        i += 1
        while i < n:
            nxt = lines[i]
            if (
                not nxt.strip()
                or nxt.startswith("```")
                or re.match(r"^#{2,4}\s+", nxt)
                or (nxt.startswith("|") and i + 1 < n and re.match(r"^\|[\s:|-]+\|\s*$", lines[i + 1]))
                or re.match(r"^-\s+", nxt)
                or re.match(r"^\d+\.\s+", nxt)
            ):
                break
            para_lines.append(nxt)
            i += 1
        raw_para = "\n".join(para_lines).strip()
        m_group = re.match(r"^\*\*([^*]+)\*\*(.*)$", raw_para)
        if m_group and "\n" not in raw_para and len(raw_para) < 60 and (
            m_group.group(2) == "" or m_group.group(2).startswith(",")
        ):
            gid = slugify(m_group.group(1))
            out.append(f'<h3 id="{gid}">{html.escape(m_group.group(1))}</h3>')
            trailing = m_group.group(2).lstrip(", ").strip()
            if trailing:
                trailing = trailing[0].upper() + trailing[1:]
                out.append(f"<p>{render_inline(trailing, current_slug, anchor_map)}</p>")
        else:
            out.append(f"<p>{render_inline(raw_para, current_slug, anchor_map)}</p>")

    return "\n".join(out)


def render_page(
    shell: Template, *, title: str, description: str, content: str,
    guide: bool = False, contents: bool = False,
) -> str:
    prefix = "../" if guide else ""
    stylesheet = "guide.css" if guide else "tour.css"
    scripts = "" if guide else '<script src="tour.js?v=6"></script>'
    if contents:
        scripts = '<script src="search-index.js"></script>\n<script src="search.js"></script>'
    current = ""
    if guide:
        current = ' aria-current="page"' if contents else ' aria-current="location"'
    return shell.substitute(
        title=html.escape(title), description=html.escape(description, quote=True),
        prefix=prefix, page_class="guide-page" if guide else "tour-page",
        guide_current=current, install_id="footer-install" if guide else "install",
        page_css=f'<link rel="stylesheet" href="{prefix}{stylesheet}?v=1">',
        content=content.strip(), page_scripts=scripts,
    )


def build_toc_html(current_slug: str) -> str:
    lines = ['<details class="toc" open><summary>Guide contents</summary><nav aria-label="Guide contents">']
    for part in PARTS:
        lines.append(f'<h2>{html.escape(part["title"])}</h2><ul>')
        for page in part["pages"]:
            current = ' aria-current="page"' if page["slug"] == current_slug else ""
            lines.append(f'<li><a href="{page["slug"]}.html"{current}>{html.escape(page["title"])}</a></li>')
        lines.append("</ul>")
    lines.append("</nav></details>")
    return "\n".join(lines)


def build_pager_html(flat_pages: list[tuple[str, dict]], idx: int) -> str:
    links = []
    for index, label, rel in ((idx - 1, "Previous", "prev"), (idx + 1, "Next", "next")):
        if 0 <= index < len(flat_pages):
            page = flat_pages[index][1]
            href, title = page["slug"] + ".html", page["title"]
        else:
            href, title = "index.html", "Guide contents"
        links.append(f'<a href="{href}" rel="{rel}"><span>{label}</span>{html.escape(title)}</a>')
    return '<nav class="pager" aria-label="Chapter navigation">' + "\n".join(links) + '</nav>'


def wrap_guide_page(part_title: str, page: dict, body_html: str, pager_html: str) -> str:
    aliases = "".join(f'<span id="{a}"></span>' for a in page.get("aliases", []))
    return f"""<main id="main-content">
  <div class="wrap guide-layout">
    {build_toc_html(page["slug"])}
    <div class="reading-column">
      <header class="chapter-head">
        {aliases}<p class="crumb"><a href="index.html">Field guide</a><span aria-hidden="true"> / </span>{html.escape(part_title)}</p>
        <h1>{html.escape(page["title"])}</h1>
        <p class="lede">{html.escape(page["lede"])}</p>
      </header>
      <article class="prose">
{body_html}
      </article>
      {pager_html}
    </div>
  </div>
</main>"""


def build_guide_index() -> str:
    sections = []
    for part in PARTS:
        items = "\n".join(
            f'<li><a href="{p["slug"]}.html">{html.escape(p["title"])}</a></li>'
            for p in part["pages"]
        )
        sections.append(
            f'<section><h2>{html.escape(part["title"])}</h2>'
            f'<p>{html.escape(part["blurb"])}</p><ul>\n{items}\n</ul></section>'
        )
    parts = "\n".join(sections)
    return f"""<main id="main-content">
  <div class="wrap guide-index">
    <header class="guide-intro">
      <h1>The Perch <em>Field Guide</em></h1>
      <p>Install Perch, review your first pull request, or start work on an issue with an agent.</p>
      {SEARCH_FORM}
    </header>
    <div class="parts">{parts}</div>
    <section id="search-results" hidden aria-label="Search results">
      <h2>Search results</h2>
      <p role="status"></p>
      <ul></ul>
    </section>
  </div>
</main>"""


class SectionIndex(HTMLParser):
    """Extract visible prose under each addressable heading in a guide article."""

    def __init__(self, page: dict):
        super().__init__()
        self.title = page["title"]
        self.path = page["slug"] + ".html"
        self.sections = []
        self.heading = False
        self.start_section("")

    def start_section(self, anchor: str) -> None:
        self.sections.append({
            "chapter": self.title, "href": self.path + anchor, "heading": "", "body": "",
        })

    def handle_starttag(self, tag, attrs):
        anchor = dict(attrs).get("id")
        if tag in ("h2", "h3", "h4") and anchor:
            self.start_section("#" + anchor)
            self.heading = True
        elif tag in ("p", "li", "pre", "th", "td", "br", "dt", "dd"):
            self.sections[-1]["body"] += " "

    def handle_endtag(self, tag):
        if tag in ("h2", "h3", "h4"):
            self.heading = False

    def handle_data(self, data):
        key = "heading" if self.heading else "body"
        self.sections[-1][key] += data


def build_search_index(flat_pages) -> None:
    records = []
    for _, page in flat_pages:
        source = (GUIDE / (page["slug"] + ".html")).read_text()
        body = source.split('<article class="prose">', 1)[1].split('</article>', 1)[0]
        parser = SectionIndex(page)
        parser.feed(re.sub(r"<nav\b.*?</nav>", "", body, flags=re.DOTALL))
        for section in parser.sections:
            section["body"] = " ".join(section["body"].split())
            if not section["heading"]:
                section["body"] = (page.get("lede", "") + " " + section["body"]).strip()
            records.append(section)
    index = json.dumps(records, ensure_ascii=False, separators=(",", ":"))
    (GUIDE / "search-index.js").write_text("const guideSections = " + index + ";\n")


def main() -> None:
    docs_cache = {
        "configuration.md": split_top_sections((DOCS / "configuration.md").read_text()),
        "reference.md": split_top_sections((DOCS / "reference.md").read_text()),
    }

    flat_pages: list[tuple[str, dict]] = []
    for part in PARTS:
        for page in part["pages"]:
            flat_pages.append((part["title"], page))

    anchor_map: dict[str, str] = {}
    page_markdowns: dict[str, str] = {}
    for _, page in flat_pages:
        if page.get("handwritten"):
            continue
        slug = page["slug"]
        md = extract_page_markdown(page, docs_cache)
        page_markdowns[slug] = md
        for alias in page.get("aliases", []):
            anchor_map[alias] = slug
        for _, aliases in page.get("heading_aliases", {}).items():
            for alias in aliases:
                anchor_map[alias] = slug
        for _, hid, _ in collect_headings(md):
            anchor_map.setdefault(hid, slug)

    shell = Template((TEMPLATES / "page.tmpl").read_text())
    GUIDE.mkdir(exist_ok=True)
    for idx, (part_title, page) in enumerate(flat_pages):
        slug = page["slug"]
        if page.get("handwritten"):
            body_html = (TEMPLATES / "chapters" / f"{slug}.inc").read_text().strip()
        else:
            body_html = render_markdown(page_markdowns[slug], page, anchor_map)
        content = wrap_guide_page(part_title, page, body_html, build_pager_html(flat_pages, idx))
        (GUIDE / f"{slug}.html").write_text(render_page(
            shell, guide=True, title=page["title"] + " · Perch Field Guide",
            description=page["desc"], content=content,
        ))

    (GUIDE / "index.html").write_text(render_page(
        shell, guide=True, contents=True, title="The Perch Field Guide",
        description="How to install Perch, review pull requests, work with agents, and configure your workspace.",
        content=build_guide_index(),
    ))
    (WEBSITE / "index.html").write_text(render_page(
        shell, title="Agents PRs suck. Perch helps you review them.",
        description="The best of GitHub, Visual Studio Code, and Claude Desktop in one native app. Human reviews with agentic assistance.",
        content=(TEMPLATES / "tour.inc").read_text(),
    ))
    (WEBSITE / "install.sh").write_bytes((ROOT / "install.sh").read_bytes())
    build_search_index(flat_pages)
    print(f"Built the homepage, {len(flat_pages)} guide pages, guide contents, and search index.")


if __name__ == "__main__":
    main()
