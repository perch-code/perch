<h2 id="cut">1. Create a worktree</h2>
<p>Open the issue and choose <b>Start Work</b> from its details menu. Perch creates a branch such as <code>perch/issue-2233</code> in a worktree, a separate checkout where the agent can edit files. Your main checkout stays as you left it. Perch puts worktrees under <code>~/.perch/worktrees/</code> by default.</p>
<figure>
  <div class="plate-frame">
    <picture>
      <source srcset="../img/new-work-dark.png?v=7" media="(prefers-color-scheme: dark)">
      <img src="../img/new-work-light.png?v=7" width="1794" height="820" loading="lazy" alt="Perch New worktree composer with repository, base branch, and agent model pickers">
    </picture>
  </div>
  <figcaption>To start without an issue, press <kbd>Cmd-N</kbd> (<kbd>Ctrl-N</kbd> on Linux) and choose a repository and base branch. See <a href="starting-work.html">Worktrees</a> for branch names and cleanup.</figcaption>
</figure>

<h2 id="sandbox">2. Ask the agent to make the change</h2>
<p>Tell the agent what you want changed. The permission menu defaults to <b>Auto-review</b>. You can also choose <b>Ask me</b>, <b>Read only</b>, or <b>Allow all</b>. With the default sandbox, a Work run can edit project files. Perch protects Git metadata and agent configuration and keeps your GitHub token out of the agent’s environment.</p>
<p>If Perch asks for permission, read the command in the thread. Choose <b>Allow once</b>, <b>Always allow here</b>, or <b>Deny</b>. See <a href="agents.html#approvals">Approvals</a> for how automatic review and saved permissions work.</p>

<h2 id="inspect">3. Inspect the diff and commit the changes</h2>
<p>Open <b>Code</b> and choose <b>Last run</b> in the file tree to see what the agent changed. Perch saves the before and after versions under <code>refs/perch/runs/</code>. You can follow definitions or <a href="worktree-reviews.html">ask another agent to review the diff</a>.</p>
<figure>
  <div class="plate-frame">
    <picture>
      <source srcset="../img/work-dark.png?v=7" media="(prefers-color-scheme: dark)">
      <img src="../img/work-light.png?v=7" width="1794" height="1365" loading="lazy" alt="Perch worktree conversation showing the Pull request status bar and completed Codex Astra 6.2 Work summary">
    </picture>
  </div>
  <figcaption>The <b>Pull request</b> bar shows when you still need to commit.</figcaption>
</figure>
<p>Commit the changes you want to include. You can use a terminal in the worktree or ask the agent to commit. For Claude Code, approve <code>git commit</code> once or choose <b>Always let Work commit here</b>. Codex Work runs can make a plain commit directly. Perch lists any uncommitted files and leaves them out of the pull request.</p>

<h2 id="open-pr">4. Push and open the pull request</h2>
<p>Click <b>Open pull request…</b> when your branch has at least one commit ahead of its base. Edit the title and description yourself, or use <b>Write with Claude</b> or <b>Codex</b> for a draft. When you submit, Perch pushes the committed branch and opens the PR in the same window. See <a href="opening-a-pull-request.html">Opening a pull request</a> if the push fails.</p>
<div class="next-steps">
  <a href="index.html">
    <b>Browse all guide topics →</b>
    <span>Find commands, settings, and keyboard shortcuts.</span>
  </a>
</div>
