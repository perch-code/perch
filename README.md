# Perch

Perch is an AI-native coding and review tool. It combines:

- Codex/Claude Desktop style agentic worktree coding
- A read-only VSCode style IDE with LSP integrations, full file trees, and code browsing
- Github style conversations, code reviews, issues, actions, and notifications with integrated human and AI conversation threads

It does all of this in a compact, Rust-based native (non-JS) desktop app on Floem and Kenn Forge.

## There are many like it, but this one is mine

I built this because I could not find a Harness or IDE that fit my workflow:

- I run several active open source projects and spend much of my times in GH issues, code reviews, discussions, Discord, and so on.
- When writing code, I almost exclusively use LLMs to write the code, but I have found I still need to look at what they're doing, ask questions (so I know what's happeninbg), and give feedback to have it adjust course. An IDE is best suited for file browsing, looking at call hierchies, seeing warnings, and so on.
- When doing code reviews or working with LLMs on code, I find I want to have access to the full codebase to be able to navigate around, look at call hiearchies and so on.
- I find myself wanting to have a threaded conversation in a GH issue or PR that includes both human and AI comments. I have yet to find something that will interleave discussions in a way that I like.

What I've landed on is effectively **Codex's user interface** with **VSCode's LSP server, file browser, and code browsing tools** and **full GitLab, Forgejo, and Gitea integration**.
