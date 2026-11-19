<h2 id="cut">1. Create a worktree</h2>
<p>Open the issue, choose <b>Start Work</b> from its details menu, and confirm with <b>Create worktree</b>. Perch gives the issue its own branch, such as <code>perch/issue-2233</code>.</p>
<figure class="plate-frame">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../img/guide/issue-worktree-dark.png">
    <img src="../img/guide/issue-worktree-light.png" width="393" height="159" loading="lazy" alt="Confirmation to create a worktree from the open issue, with its title and the Create worktree button">
  </picture>
</figure>
<p>Starting without an issue? Press <kbd>Cmd-N</kbd> (<kbd>Ctrl-N</kbd> on Linux) for the <a href="starting-work.html">new worktree form</a>.</p>

<h2 id="sandbox">2. Ask the agent to make the change</h2>
<p>Describe the fix. <b>Auto-review</b> is the default permission mode. When a command needs your approval, read it in the thread and choose whether to allow it once, save an approval, or deny it. See <a href="agents.html#approvals">agent permissions</a> for the other modes.</p>

<h2 id="inspect">3. Inspect the diff and commit the changes</h2>
<p>In <b>Code</b>, choose <b>Last run</b> to inspect the agent’s diff. You can also <a href="worktree-reviews.html">have other agents review it</a>.</p>
<figure>
  <div class="plate-frame">
    <picture>
      <source srcset="../img/work-dark.png?v=7" media="(prefers-color-scheme: dark)">
      <img src="../img/work-light.png?v=7" width="1794" height="1365" loading="lazy" alt="Perch worktree conversation showing the Pull request status bar and completed Codex Astra 6.2 Work summary">
    </picture>
  </div>
  <figcaption>The <b>Pull request</b> bar shows when you still need to commit.</figcaption>
</figure>
<p>Commit what you want to publish, in a terminal or by asking the agent. For Claude Code, approve <code>git commit</code> when prompted. Codex can make a plain commit directly.</p>

<h2 id="open-pr">4. Push and open the pull request</h2>
<p>With a commit ahead of the base, click <b>Open pull request…</b>. Check the title and description, then submit. Perch pushes the commits and opens the PR. Uncommitted edits stay in the worktree. See <a href="opening-a-pull-request.html">Opening a pull request</a> for drafting options and push errors.</p>
<div class="next-steps">
  <a href="index.html">
    <b>Browse all guide topics →</b>
    <span>Find commands, settings, and keyboard shortcuts.</span>
  </a>
</div>
