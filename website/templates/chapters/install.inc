<p class="note"><b>Where will your code and agents run?</b>If your clones live on this machine, follow steps 1–4 below. If your code lives on a remote Linux or Mac machine over SSH, install the Perch app locally (<a href="#install">step 2</a>) and then jump to <a href="remote-engine.html#setting-up-a-host">Remote hosts (SSH)</a>—<code>gh</code>, your repositories, and your agent CLIs belong on the SSH host, and Perch uploads its own headless <code>perchd</code> engine automatically.</p>

<h2 id="sign-in">1. Sign in to the GitHub CLI</h2>
<p>Perch runs on macOS (Apple Silicon or Intel) and Linux (<code>x86_64</code> or <code>arm64</code> with glibc 2.35+ and GTK 3). Install <code>git</code> and the <a href="https://cli.github.com/">GitHub CLI</a>, then sign in once:</p>
<pre><code>gh auth login</code></pre>
<p>Desktop apps launched from Finder or an application launcher do not always inherit terminal exports, so signing in to <code>gh</code> gives Perch a token reliably. See <a href="authentication.html">Authentication</a> for environment variable fallbacks and required token permissions.</p>

<h2 id="install">2. Install Perch</h2>
<pre><code>curl https://perch.code/install.sh | sh</code></pre>
<p>On macOS, the installer places the signed and notarized <code>Perch.app</code> in <code>/Applications</code>. On Linux, it unpacks into <code>~/.local/share/perch</code>. Both platforms link <code>~/.local/bin/perch</code> onto your <code>PATH</code> (set <code>PERCH_NO_MODIFY_PATH=1</code> to skip shell profile edits).</p>
<p>Prefer downloading an archive directly? Grab <code>Perch-macos-arm64.tar.gz</code>, <code>Perch-macos-x86_64.tar.gz</code>, or the Linux tarball from <a href="https://github.com/perch-code/perch/releases/latest">Releases</a>.</p>

<h2 id="repositories">3. Launch Perch and connect a repository</h2>
<p>Open <code>Perch.app</code> from <code>/Applications</code> (or run <code>perch</code> from a terminal on Linux). The first window opens on your GitHub <b>Inbox</b>, with your GitHub login and <b>This computer</b> shown at the foot of the sidebar.</p>
<p>Press <kbd>Cmd-,</kbd> (<kbd>Ctrl-,</kbd> on Linux) to open <b>Settings → Repositories</b> and add a local clone, or write it directly in <code>~/.perch/config.toml</code>:</p>
<pre><code>[[repo]]
github = "slatedb/slatedb"
root = "~/Code/slatedb"</code></pre>
<p>Perch reloads <a href="configuration.html"><code>config.toml</code></a> within a couple of seconds while running. A local-only repository (<code>root</code> with no <code>github</code>) works for local branches and agent worktrees too.</p>

<h2 id="tools">4. Optional: coding agents and language servers</h2>
<p>You can read pull requests and submit GitHub reviews right away. To use <b>Ask</b>, <b>Work</b>, or <b>Review</b> with an agent, sign in to at least one supported CLI on the machine running the engine:</p>
<ul>
<li><a href="https://claude.com/claude-code">Claude Code</a> <code>2.1.223+</code> (on Linux, also install <code>bubblewrap</code> and <code>socat</code> for Claude Code's sandbox)</li>
<li><a href="https://github.com/openai/codex">Codex CLI</a> <code>0.155.1+</code></li>
<li><a href="https://antigravity.google/docs/cli/reference">Antigravity CLI</a> (<code>agy</code>) <code>1.5.5+</code></li>
<li>OpenCode <code>2.3.0+</code> (uses an isolated Perch profile; see <a href="configuration.html#opencode">OpenCode setup</a>)</li>
</ul>
<p>For symbol navigation and Call Hierarchy, Perch supports <code>rust-analyzer</code> (Rust), <code>gopls</code> (Go), <code>typescript-language-server</code> (TypeScript/JavaScript), <code>jdtls</code> (Java), and <code>ty</code> (Python). Perch offers to install missing servers the first time you open a matching file, and asks before running <code>rust-analyzer</code> or <code>jdtls</code> on external pull requests (<a href="configuration.html#work-language-servers-and-appearance">LSP trust</a>).</p>

<h2 id="next">What to do next</h2>
<p>Once your repository appears under <b>Browse</b> (<kbd>Cmd-4</kbd>) in the sidebar, pick a first workflow:</p>
<div class="next-steps">
  <a href="first-review.html">
    <b>Review your first pull request →</b>
    <span>Open a PR from Inbox or Browse, inspect callers with a language server, and publish review drafts.</span>
  </a>
  <a href="first-branch.html">
    <b>Start work on an issue →</b>
    <span>Cut an isolated branch worktree, let an agent edit in a sandbox, commit, and open a PR.</span>
  </a>
</div>

<h2 id="updates">Staying up to date</h2>
<p>Perch checks for new releases in the background; click <b>Restart to update</b> in the window when a new version is ready.</p>
