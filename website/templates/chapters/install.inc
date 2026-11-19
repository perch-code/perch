<p class="note"><b>Using SSH?</b><a href="#install">Install Perch</a> on your desktop, then <a href="remote-engine.html#setting-up-a-host">set up the host</a> where your code lives.</p>

<h2 id="sign-in">1. Sign in to the GitHub CLI</h2>
<p>Perch runs on macOS (Apple Silicon or Intel) and Linux (<code>x86_64</code> or <code>arm64</code> with glibc 2.35+ and GTK 3). Install Git and the <a href="https://cli.github.com/">GitHub CLI</a>, then run this command.</p>
<pre><code>gh auth login</code></pre>
<p>Perch can use your GitHub CLI login even when you launch it outside a terminal. For token settings and permissions, see <a href="authentication.html">Authentication</a>.</p>

<h2 id="install">2. Install Perch</h2>
<pre><code>curl https://perch.code/install.sh | sh</code></pre>
<p>On macOS, the installer puts the signed and notarized <code>Perch.app</code> in <code>/Applications</code>. On Linux, it installs Perch in <code>~/.local/share/perch</code>. Both create a launcher at <code>~/.local/bin/perch</code>. Set <code>PERCH_NO_MODIFY_PATH=1</code> if you manage your shell’s <code>PATH</code> yourself.</p>
<p>You can also download the archive for your platform from <a href="https://github.com/perch-code/perch/releases/latest">Releases</a>.</p>

<h2 id="repositories">3. Launch Perch and connect a repository</h2>
<p>Launch Perch from Applications on macOS or run <code>perch</code> on Linux. Your GitHub inbox opens first.</p>
<figure class="plate-frame">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../img/guide/inbox-dark.png">
    <img src="../img/guide/inbox-light.png" width="318" height="330" loading="lazy" alt="Inbox tab selected beside Worktrees and Browse, above a list of GitHub notifications">
  </picture>
</figure>
<p>Press <kbd>Cmd-,</kbd> (<kbd>Ctrl-,</kbd> on Linux), choose <b>Repositories</b>, and add your local clone. You can also add a repository in <a href="configuration.html#repositories-and-local-roots">config.toml</a>.</p>

<h2 id="tools">4. Set up optional tools</h2>
<p>Agents are optional. To use one, sign in on the machine with your checkout.</p>
<ul>
<li><a href="https://claude.com/claude-code">Claude Code</a> <code>2.1.223+</code> (on Linux, also install <code>bubblewrap</code> and <code>socat</code> for Claude Code's sandbox)</li>
<li><a href="https://github.com/openai/codex">Codex CLI</a> <code>0.155.1+</code></li>
<li><a href="https://antigravity.google/docs/cli/reference">Antigravity CLI</a> (<code>agy</code>) <code>1.5.5+</code></li>
<li>OpenCode <code>2.3.0+</code> (uses a separate Perch profile. See <a href="configuration.html#opencode">OpenCode setup</a>)</li>
</ul>
<p>For <a href="code-navigation.html">code navigation</a>, Perch offers to install a language server when you need one. It supports Rust, Go, TypeScript, JavaScript, Java, and Python. On external PRs it <a href="configuration.html#work-language-servers-and-appearance">asks before starting a Rust or Java server</a>.</p>

<h2 id="next">What to do next</h2>
<p>Choose your repository under <b>Browse</b> (<kbd>Cmd-4</kbd> on macOS). Then try a walkthrough.</p>
<div class="next-steps">
  <a href="first-review.html">
    <b>Review your first pull request →</b>
    <span>Read the diff, ask a question, and post feedback.</span>
  </a>
  <a href="first-branch.html">
    <b>Start work on an issue →</b>
    <span>Give an agent an issue and review its changes on a new branch.</span>
  </a>
</div>

<h2 id="updates">Staying up to date</h2>
<p>Perch checks for updates in the background. Click <b>Restart to update</b> when a release is ready.</p>
