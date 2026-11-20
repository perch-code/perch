<h2 id="open">Open a pull request</h2>
<p>Open a pull request from your <a href="inbox.html"><b>Inbox</b></a>. If it’s empty, select your repository under <b>Browse</b> (<kbd>Ctrl-3</kbd>), or press <kbd>Cmd-O</kbd> and enter a PR URL or number.</p>
<p>In <b>Conversation</b>, read the description and recent comments.</p>

<h2 id="code">Switch to Code</h2>
<p>Click <b>Code</b> and pick a changed file. Press <kbd>Cmd-P</kbd> to open any file on the branch.</p>
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
<p>Expand a caller in the panel below the diff, or switch to <b>Callees</b>. Go to Definition (<kbd>F12</kbd>) also works in dependencies. See <a href="code-navigation.html">Code navigation</a> for references, implementations, and the full shortcut list.</p>
<p class="note"><b>If nothing opens</b>Check the status bar. Try <b>Go to Definition</b> again to install a missing language server or choose <b>Trust and start</b> if you trust the checkout.</p>

<h2 id="ask">Ask an agent on the line (optional)</h2>
<p>Click <kbd>+</kbd> beside a line or <b>Ask about this</b> on a comment. Choose a <a href="install.html#tools">signed-in agent</a> and ask your question.</p>
<figure>
  <div class="plate-frame">
    <picture>
      <source srcset="../img/ask-dark.png?v=8" media="(prefers-color-scheme: dark)">
      <img src="../img/ask-light.png?v=8" width="768" height="306" loading="lazy" alt="Perch’s composer with the model dropdown open and Opus selected in the Claude submenu">
    </picture>
  </div>
  <figcaption>Choose a different model for a second opinion in the same thread.</figcaption>
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
