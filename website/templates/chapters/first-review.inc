<h2 id="open">Open a pull request</h2>
<p>Open a pull request from your <a href="inbox.html"><b>Inbox</b></a>. If it’s empty, select your repository under <b>Browse</b> (<kbd>Cmd-4</kbd>), or press <kbd>Cmd-O</kbd> and enter a PR URL or number.</p>
<p>Start in <b>Conversation</b>. Read the description and recent comments to see what the author changed and what reviewers have already covered.</p>

<h2 id="code">Switch to Code</h2>
<p>Click <b>Code</b> in the top bar, or press <kbd>Cmd-Shift-2</kbd> (<kbd>Ctrl-Shift-2</kbd> on Linux). Choose a changed file from the tree on the right. To open any file on the branch, press <kbd>Cmd-P</kbd>.</p>
<figure>
  <div class="plate-frame">
    <picture>
      <source srcset="../img/review-dark.png?v=8" media="(prefers-color-scheme: dark)">
      <img src="../img/review-light.png?v=8" width="2474" height="2070" loading="lazy" alt="Perch Code view showing compactor_executor.rs diff lines, the changed-files drawer, and the open Callers of start_compaction_job call hierarchy panel">
    </picture>
  </div>
  <figcaption>The file tree is on the right. The panel below the diff lists callers of <code>start_compaction_job</code>.</figcaption>
</figure>

<h2 id="symbol">Follow a symbol</h2>
<p>Hover over a name in the code to see its signature and documentation. To see where a changed function is used, choose <b>Show Call Hierarchy</b> from its right-click menu.</p>
<div class="table"><table>
<thead><tr><th>Action</th><th>Key</th></tr></thead>
<tbody>
<tr><td>Go to Definition</td><td><kbd>Cmd-click</kbd> or <kbd>F12</kbd></td></tr>
<tr><td>Go to Implementations</td><td><kbd>Cmd-F12</kbd></td></tr>
<tr><td>Find All References</td><td><kbd>Shift-F12</kbd></td></tr>
<tr><td>Show Call Hierarchy</td><td><kbd>Shift-Alt-H</kbd></td></tr>
<tr><td>Back, Forward</td><td><kbd>Ctrl--</kbd>, <kbd>Ctrl-Shift--</kbd></td></tr>
</tbody></table></div>
<p>Perch opens a single match immediately. For several matches, choose one in the panel below the editor. Definitions in a dependency or the standard library open in a read-only tab. A label such as <code>tokio 1.40.0</code> tells you which version you’re reading. See <a href="code-navigation.html">Code navigation</a> for more shortcuts.</p>
<p class="note"><b>If nothing opens</b>Check the status bar. Try <b>Go to Definition</b> again to install a missing language server or choose <b>Trust and start</b> if you trust the checkout.</p>

<h2 id="ask">Ask an agent on the line (optional)</h2>
<p>Sign in to a <a href="install.html#tools">supported agent</a>. Click <kbd>+</kbd> beside a line, or select <b>Ask about this</b> on a GitHub comment. Pick a model from the dropdown and ask your question. You can choose a different model for your next reply in the same thread. See <a href="agents.html">Agent sandbox and approvals</a> for what the agent can access.</p>
<figure>
  <div class="plate-frame">
    <picture>
      <source srcset="../img/ask-dark.png?v=8" media="(prefers-color-scheme: dark)">
      <img src="../img/ask-light.png?v=8" width="768" height="306" loading="lazy" alt="Perch’s composer with the model dropdown open and Opus selected in the Claude submenu">
    </picture>
  </div>
  <figcaption>The model picker works the same way on an issue and in a line comment.</figcaption>
</figure>

<h2 id="post">Post your review</h2>
<p>To leave a line comment for the author, choose <b>GitHub</b> in the dropdown. Perch marks it <b>Draft</b> and saves it locally, even offline or across restarts.</p>
<p>Click <b>Review</b> to check your drafts, choose a verdict, and submit them to GitHub together. You can also <a href="worktree-reviews.html">ask agents to review the diff</a> before posting. See <a href="posting.html">Posting and drafts</a> for suggestions and replies.</p>
<div class="next-steps">
  <a href="first-branch.html">
    <b>Next walkthrough: Start work on an issue →</b>
    <span>Give an agent an issue and open a pull request with its changes.</span>
  </a>
</div>
