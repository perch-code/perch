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
import struct
from string import Template
from xml.etree import ElementTree as ET

ROOT = pathlib.Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"
WEBSITE = ROOT / "website"
GUIDE = WEBSITE / "guide"
TEMPLATES = WEBSITE / "templates"
PUBLIC_ORIGIN = "https://perch.chat"
HOME_TITLE = "Perch — Review GitHub pull requests with coding agents"
HOME_DESCRIPTION = "Review pull requests, discuss code with your team and coding agents, and start work from GitHub issues in one native app for macOS and Linux."
HOME_CARD_TITLE = "Agent PRs suck."
HOME_CARD_DESCRIPTION = "Perch gives you the best of GitHub, Visual Studio Code, and Claude Desktop in one native app. Human reviews with agentic assistance."
GUIDE_TITLE = "The Perch Field Guide"

SEARCH_FORM = """<form class="guide-search" role="search" aria-label="Field guide" action="index.html" method="get">
      <input type="search" name="q" aria-label="Search guide" placeholder="Search the guide" autocomplete="off">
      <button type="submit" aria-label="Search the guide"><svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" aria-hidden="true"><circle cx="10" cy="10" r="6.5"/><path d="m15 15 5 5"/></svg></button>
    </form>"""


PARTS = [
    {
        "title": "Get set up",
        "blurb": "Connect your GitHub account and tell Perch where your code lives.",
        "pages": [
            {
                "slug": "install",
                "title": "Install Perch",
                "kind": "chapter",
                "lede": "Sign in to GitHub, install the desktop app, and open your first repository.",
                "handwritten": True,
            },
            {
                "slug": "authentication",
                "heading_aliases": {
                    "where-perch-looks-for-a-token": ["credential-lookup-order"],
                    "signing-in-from-the-desktop": ["desktop-launches-and-login-shells"],
                },
                "title": "Authentication",
                "kind": "reference",
                "lede": "Perch can use your GitHub CLI login or a token you provide. The permissions you need depend on what you do on GitHub.",
                "sources": [("reference.md", ["Authentication"], False)],
                "promote": True,
                "aliases": ["authentication"],
            },
            {
                "slug": "remote-engine",
                "title": "Remote hosts (SSH)",
                "kind": "reference",
                "lede": "Keep the Perch window on your desktop and run Git, language servers, and agents on the machine where your code lives.",
                "custom_builder": "remote_engine",
                "aliases": ["an-engine-on-another-machine"],
            },
        ],
    },
    {
        "title": "Review a change",
        "blurb": "Read the diff, ask questions, and send feedback to the author.",
        "pages": [
            {
                "slug": "first-review",
                "title": "Review your first pull request",
                "kind": "chapter",
                "lede": "Read the discussion, follow a changed function to its callers, and decide what feedback to send the author.",
                "handwritten": True,
            },
            {
                "slug": "inbox",
                "heading_aliases": {
                    "when-the-inbox-refreshes": ["polling-and-screen-stability"],
                },
                "title": "The inbox",
                "kind": "reference",
                "lede": "Read GitHub notifications, set a reminder, or clear finished work from your inbox.",
                "sources": [("reference.md", ["GitHub inbox"], False)],
                "promote": True,
                "aliases": ["github-inbox"],
            },
            {
                "slug": "code-navigation",
                "title": "Code navigation",
                "kind": "reference",
                "lede": "Open a file anywhere on the branch. From a changed line, you can find a definition, inspect its callers, or jump back to the discussion.",
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
                "on_page_h3": True,
                "title": "Posting and drafts",
                "kind": "reference",
                "lede": "Line comments wait in your review drafts. A comment at the bottom of Conversation goes to GitHub when you confirm. Questions to agents stay in Perch.",
                "sources": [
                    ("reference.md", ["Timeline", "Reactions", "Posting"], False)
                ],
                "rename_headings": [
                    ("## Posting", "## Choose who gets your message"),
                ],
                "heading_aliases": {
                    "choose-who-gets-your-message": ["posting", "composers--mentions-and-destinations"],
                },
                "promote": False,
                "aliases": [],
            },
            {
                "slug": "worktree-reviews",
                "heading_aliases": {
                    "choose-your-reviewers": ["the-review-popover"],
                    "choose-what-to-review": ["diff-scopes-and-instructions"],
                    "reopen-or-repeat-a-review": ["immutable-snapshots-and-reconciling-findings"],
                    "check-drafts-before-posting": ["publishing-findings-as-github-drafts"],
                },
                "title": "Review a worktree with agents",
                "kind": "reference",
                "lede": "Choose which agents should read the diff. Check their findings and edit the draft comments before you send a review to GitHub.",
                "sources": [("reference.md", ["Worktree reviews"], False)],
                "promote": True,
                "aliases": ["worktree-reviews"],
            },
        ],
    },
    {
        "title": "Start work",
        "blurb": "Give an agent a branch, check its changes, and open a pull request.",
        "pages": [
            {
                "slug": "first-branch",
                "title": "Start work on an issue",
                "kind": "chapter",
                "lede": "Give an agent an issue to work on, check its changes, and open a pull request.",
                "handwritten": True,
            },
            {
                "slug": "starting-work",
                "heading_aliases": {
                    "create-a-worktree": ["new-worktree-composer"],
                },
                "title": "Worktrees",
                "kind": "reference",
                "lede": "Give a branch its own checkout so an agent can make changes while you keep working in yours.",
                "sources": [("reference.md", ["Starting work"], False)],
                "promote": True,
                "aliases": ["starting-work"],
            },
            {
                "slug": "opening-a-pull-request",
                "heading_aliases": {
                    "fill-in-the-pull-request": ["filling-the-pull-request-form"],
                    "push-the-branch-and-open-the-pr": ["pushing-and-creating-on-github"],
                    "add-commits-to-an-open-pr": ["pushing-new-commits-to-an-open-pr"],
                },
                "title": "Opening a pull request",
                "kind": "reference",
                "lede": "Open a GitHub pull request from the commits on your branch. Perch keeps the worktree’s discussion and agent runs with the PR.",
                "sources": [("reference.md", ["Opening a pull request"], False)],
                "promote": True,
                "aliases": ["opening-a-pull-request"],
            },
            {
                "slug": "agents",
                "title": "Agent sandbox and approvals",
                "kind": "reference",
                "lede": "Choose whether an agent can read or edit your checkout and who can approve a command that needs more access.",
                "custom_builder": "agents_sandbox",
                "heading_aliases": {
                    "read-only-sandbox-and-untrusted-text": ["what-the-agent-can-and-cannot-do"],
                    "work-mode": ["work"],
                    "approve-commands-and-set-repository-permissions": ["approvals", "approvals-and-repository-grants"],
                },
                "aliases": ["agents"],
            },
            {
                "slug": "agent-context",
                "title": "Agent context and history",
                "kind": "reference",
                "lede": "See which comments, checks, drafts, and past reviews an agent can read. You can also look up the same discussion from a terminal.",
                "custom_builder": "agent_context",
                "heading_aliases": {
                    "what-an-agent-sees-in-the-discussion": ["what-the-agent-is-told-about-the-discussion", "discussion-context-in-prompts"],
                },
                "aliases": [],
            },
        ],
    },
    {
        "title": "Preferences and shortcuts",
        "blurb": "Look up settings, change a shortcut, or check the engine on an SSH host.",
        "pages": [
            {
                "slug": "settings",
                "heading_aliases": {
                    "save-or-reset-a-setting": ["saving-and-live-reload"],
                },
                "title": "Settings window",
                "kind": "reference",
                "lede": "Change preferences for your desktop or the machine running your agents. Settings shows which changes need a restart.",
                "sources": [("reference.md", ["Settings"], False)],
                "promote": True,
                "aliases": ["settings"],
            },
            {
                "slug": "configuration",
                "heading_aliases": {
                    "environment-variables-and-settings-on-an-ssh-host": ["environment-variables-and-remote-split"],
                },
                "title": "Configuration file (config.toml)",
                "kind": "reference",
                "lede": "Edit config.toml to change where Perch finds your repositories, which agents it runs, and how the app looks. Every setting is optional.",
                "custom_builder": "configuration",
                "aliases": ["configuration"],
            },
            {
                "slug": "keyboard-shortcuts",
                "title": "Keyboard shortcuts",
                "kind": "reference",
                "lede": "Look up a shortcut by action, assign a different key, or remove a binding.",
                "sources": [("reference.md", ["Keyboard shortcuts"], False)],
                "promote": True,
                "on_page_h3": True,
                "aliases": ["keyboard-shortcuts"],
            },
            {
                "slug": "perchd",
                "title": "The remote engine (perchd)",
                "kind": "reference",
                "lede": "The desktop app usually starts perchd for you. Use these commands to check which build is running on a host, connect to it, or stop it.",
                "custom_builder": "perchd",
                "heading_aliases": {
                    "using-perchd-from-a-terminal": ["perchd", "headless-perchd-commands"],
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
            "To check or restart the engine yourself, see [The remote engine (perchd)](perchd.html)."
        )
        return "\n\n".join(blocks)

    if builder == "perchd":
        _, remote_subs = split_subsections(ref_secs["An engine on another machine"])
        perchd_sec = re.sub(
            r"^## perchd\b",
            "## Using perchd from a terminal",
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
        split_needle = "#### Finding an agent run"
        if split_needle not in disc_full:
            raise ValueError("Missing agent run section")
        disc_part, runs_part = disc_full.split(split_needle, 1)
        # Drop the `### What the agent is told about the discussion` line from disc_part
        disc_body = "\n".join(disc_part.splitlines()[1:]).strip()
        runs_body = runs_part.strip()

        if builder == "agents_sandbox":
            overview = (
                "## Overview\n\n"
                "Choose **Ask** for a question about the code, **Work** to let an agent edit your branch, "
                "or **Review** to check a diff for bugs. Perch runs the agent CLI on the machine "
                "with your checkout. See [Agent context and history](agent-context.html) for what goes into its prompt."
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
                "## Approve commands and set repository permissions",
                subs["Approvals"],
                count=1,
            )
            without_sandbox = promote_block(subs["Without Claude Code's sandbox"])
            return "\n\n".join([overview, asking, readonly, without_sandbox, work, approvals])

        # builder == "agent_context"
        disc_sec = (
            "## What an agent sees in the discussion\n\n"
            + disc_body
            + "\n\nFor tool permissions and how to approve a command, see [Agent sandbox and approvals](agents.html)."
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
        repo_intro, ref_parts = split_subsections(ref_cfg)
        repos_sec = "## Repositories and local roots\n\n" + repo_intro
        work_lsp_sec = promote_block(ref_parts["Work, language servers, and appearance"])

        _, cfg_parts = split_subsections(cfg_preamble)
        keys_sec = promote_block(cfg_parts["All config.toml keys"])
        env_sec = promote_block(cfg_parts["Environment variables and settings on an SSH host"])

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

        screenshot = re.fullmatch(r"!\[([^\]]+)\]\(\.\./website/img/([\w/-]+)-light\.png\)", line)
        if screenshot:
            alt, name = screenshot.groups()
            # Perch screenshots are captured at 2x. Reserve the native UI size
            # while the lazy image loads, and let CSS shrink it for narrow screens.
            with (WEBSITE / "img" / f"{name}-light.png").open("rb") as image:
                image.seek(16)
                width, height = struct.unpack(">II", image.read(8))
            out.append(
                f'<figure class="plate-frame"><picture>'
                f'<source media="(prefers-color-scheme: dark)" srcset="../img/{name}-dark.png">'
                f'<img src="../img/{name}-light.png" width="{width // 2}" height="{height // 2}" '
                f'loading="lazy" alt="{html.escape(alt)}"></picture></figure>'
            )
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


def canonical_url(path: str) -> str:
    url = f"{PUBLIC_ORIGIN}/{path}"
    return url.removesuffix("index.html") if path.rsplit("/", 1)[-1] == "index.html" else url


def write_crawl_files(flat_pages) -> None:
    paths = ["index.html", "guide/index.html"]
    paths.extend(f"guide/{page['slug']}.html" for _, page in flat_pages)
    sitemap = ET.Element("urlset", xmlns="http://www.sitemaps.org/schemas/sitemap/0.9")
    for path in paths:
        entry = ET.SubElement(sitemap, "url")
        ET.SubElement(entry, "loc").text = canonical_url(path)
    ET.indent(sitemap)
    ET.ElementTree(sitemap).write(WEBSITE / "sitemap.xml", encoding="utf-8", xml_declaration=True)
    (WEBSITE / "robots.txt").write_text(f"User-agent: *\nAllow: /\n\nSitemap: {PUBLIC_ORIGIN}/sitemap.xml\n")


def social_metadata(path: str, title: str, description: str) -> str:
    page = pathlib.PurePosixPath(path)
    is_index = page.name == "index.html"
    image = pathlib.PurePosixPath("img/social") / page.with_suffix(".png")
    if not (WEBSITE / image).is_file():
        raise FileNotFoundError(f"Missing {image}. Run website/build_social_cards.py to generate it.")
    if path == "index.html":
        title, description = HOME_CARD_TITLE, HOME_CARD_DESCRIPTION
        image_alt = f"{title} {description} Beside the text, Perch shows a pull request in Warm Linen."
    else:
        image_alt = f"Field Guide cover with the Perch wren and the title “{title.removesuffix(' · Perch Field Guide')}”."
    template = Template((TEMPLATES / "social.tmpl").read_text())
    return template.substitute(
        url=canonical_url(path),
        kind="website" if is_index else "article",
        title=html.escape(title), description=html.escape(description),
        image=f"{PUBLIC_ORIGIN}/{image}",
        image_alt=html.escape(image_alt),
    ).strip()


def render_page(
    shell: Template, *, path: str, title: str, description: str, content: str,
) -> str:
    guide = path.startswith("guide/")
    contents = path == "guide/index.html"
    prefix = "../" if guide else ""
    stylesheet = "guide.css?v=2" if guide else "tour.css?v=1"
    scripts = '<script src="heading-links.js"></script>' if guide else '<script src="tour.js?v=6"></script>'
    if contents:
        scripts = '<script src="search-index.js"></script>\n<script src="search.js"></script>'
    current = ""
    if guide:
        current = ' aria-current="page"' if contents else ' aria-current="location"'
    site_name = ""
    if path == "index.html":
        data = {"@context": "https://schema.org", "@type": "WebSite", "name": "Perch", "url": canonical_url(path)}
        site_name = f'\n<script type="application/ld+json">{json.dumps(data)}</script>'
    return shell.substitute(
        title=html.escape(title), description=html.escape(description, quote=True),
        social=social_metadata(path, title, description),
        site_name=site_name,
        prefix=prefix, page_class="guide-page" if guide else "tour-page",
        guide_current=current, install_id="footer-install" if guide else "install",
        page_css=f'<link rel="stylesheet" href="{prefix}{stylesheet}">',
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
        <h1 id="chapter-title">{html.escape(page["title"])}</h1>
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
            shell, path=f"guide/{slug}.html", title=page["title"] + " · Perch Field Guide",
            description=page["lede"], content=content,
        ))

    (GUIDE / "index.html").write_text(render_page(
        shell, path="guide/index.html", title=GUIDE_TITLE,
        description="How to install Perch, review pull requests, work with agents, and configure your workspace.",
        content=build_guide_index(),
    ))
    (WEBSITE / "index.html").write_text(render_page(
        shell, path="index.html", title=HOME_TITLE, description=HOME_DESCRIPTION,
        content=(TEMPLATES / "tour.inc").read_text(),
    ))
    redirect = Template((TEMPLATES / "redirect.tmpl").read_text())
    (WEBSITE / "field-notes.html").write_text(redirect.substitute(
        social=social_metadata("index.html", HOME_TITLE, HOME_DESCRIPTION),
    ))
    (WEBSITE / "install.sh").write_bytes((ROOT / "install.sh").read_bytes())
    build_search_index(flat_pages)
    write_crawl_files(flat_pages)
    print(f"Built the homepage, {len(flat_pages)} guide pages, guide contents, and search index.")


if __name__ == "__main__":
    main()
