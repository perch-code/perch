<h2 id="open">Open a pull request</h2>
<p>Pick a pull request under <b>Inbox</b> in the sidebar. If your inbox has none waiting yet, switch the sidebar to <b>Browse</b> (<kbd>Cmd-4</kbd>) with your repository selected to see open pull requests, or press <kbd>Cmd-O</kbd> to open any pull request by URL or number (see <a href="inbox.html">The inbox</a>).</p>
<p>Items open on <b>Conversation</b>: the description, timeline events, and review threads in chronological order. Read the description and latest discussion, and pick a question that depends on what the surrounding code does.</p>

<h2 id="code">Switch to Code</h2>
<p>Press <kbd>Cmd-Shift-2</kbd> (<kbd>Ctrl-Shift-2</kbd> on Linux), or click <b>Code</b> at the center of the top bar. Code opens the pull request's checkout with the changed-files drawer on the right. Open a file from the tree, or press <kbd>Cmd-P</kbd> to jump to any file on the branch, changed or not.</p>
<figure>
  <div class="plate-frame">
    <picture>
      <source srcset="../img/review-dark.png?v=8" media="(prefers-color-scheme: dark)">
      <img src="../img/review-light.png?v=8" width="2474" height="2070" loading="lazy" alt="Perch Code view showing compactor_executor.rs diff lines, the changed-files drawer, and the open Callers of start_compaction_job call hierarchy panel">
    </picture>
  </div>
  <figcaption>Code view shows the full file checkout, inline review threads, the changed-files drawer, and language-server panels such as Call Hierarchy.</figcaption>
</figure>

<h2 id="symbol">Follow a symbol</h2>
<p>Hover any identifier for its signature and doc comment, then navigate with the shortcuts you already know from VS Code:</p>
<div class="table"><table>
<thead><tr><th>Action</th><th>Key</th></tr></thead>
<tbody>
<tr><td>Go to Definition</td><td><kbd>Cmd-click</kbd> or <kbd>F12</kbd></td></tr>
<tr><td>Go to Implementations</td><td><kbd>Cmd-F12</kbd></td></tr>
<tr><td>Find All References</td><td><kbd>Shift-F12</kbd></td></tr>
<tr><td>Show Call Hierarchy</td><td><kbd>Shift-Alt-H</kbd></td></tr>
<tr><td>Back, Forward</td><td><kbd>Ctrl--</kbd>, <kbd>Ctrl-Shift--</kbd></td></tr>
</tbody></table></div>
<p>A single match opens right away; multiple matches open a panel under the editor. Definitions inside a dependency or the standard library open in a read-only tab badged with their crate or package, such as <code>tokio 1.40.0</code>. See <a href="code-navigation.html">Code navigation</a> for scopes, split diffs, and clickable code links.</p>
<p class="note"><b>If nothing opens</b>Check the status bar: if a language server is missing or waiting for workspace trust (such as <code>rust-analyzer</code> on an external PR), triggering Go to Definition offers to install or <b>Trust and start</b> it.</p>

<h2 id="ask">Ask an agent on the line (optional)</h2>
<p>With a supported coding-agent CLI signed in (<a href="install.html#tools">Claude Code, Codex, Antigravity, or OpenCode</a>), click <kbd>+</kbd> in the gutter beside a line—or click <b>Ask about this</b> on an existing GitHub comment—to open the composer. Choose a model from the dropdown, then type your question. You can bring multiple models into the same thread to cross-check an answer; see <a href="agents.html">Agent sandbox and approvals</a>.</p>
<figure>
  <div class="plate-frame">
    <picture>
      <source srcset="../img/ask-dark.png?v=8" media="(prefers-color-scheme: dark)">
      <img src="../img/ask-light.png?v=8" width="768" height="306" loading="lazy" alt="Perch’s composer with the model dropdown open and Opus selected in the Claude submenu">
    </picture>
  </div>
  <figcaption>The issue composer shown here uses the same model picker as a line thread. Choose a different model from the dropdown for a follow-up.</figcaption>
</figure>

<h2 id="post">Post your review</h2>
<p>To leave feedback for the author, switch the composer's destination to <b>GitHub</b>. Line comments save locally with a <code>Draft</code> pill, survive restarts and offline work, and stay hidden from github.com until you submit.</p>
<p>When you are ready, click <b>Review</b> in the top bar, choose your verdict, and submit—publishing all of your drafts in one batch. See <a href="posting.html">Posting and drafts</a> for inline suggestions and replies, or <a href="worktree-reviews.html">Agent reviews of a worktree</a> to run automated bug checks first.</p>
<div class="next-steps">
  <a href="first-branch.html">
    <b>Next walkthrough: Start work on an issue →</b>
    <span>Cut an isolated branch worktree, let an agent make the change, commit, and open a pull request.</span>
  </a>
</div>
