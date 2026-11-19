<h2 id="cut">1. Cut a worktree</h2>
<p>Open any issue (or pull request) and click <b>Start Work</b> in its details menu—or press <kbd>Cmd-N</kbd> (<kbd>Ctrl-N</kbd> on Linux) from anywhere to start from a blank prompt. Perch cuts a fresh branch (such as <code>perch/issue-2233</code>) into <code>~/.perch/worktrees/</code> without touching whatever you have checked out in your main clone. See <a href="starting-work.html">Worktrees</a> for branch naming and cleanup rules.</p>
<figure>
  <div class="plate-frame">
    <picture>
      <source srcset="../img/new-work-dark.png?v=7" media="(prefers-color-scheme: dark)">
      <img src="../img/new-work-light.png?v=7" width="1794" height="820" loading="lazy" alt="Perch New worktree composer with repository, base branch, and agent model pickers">
    </picture>
  </div>
  <figcaption>Pressing <kbd>Cmd-N</kbd> (or clicking <b>Start Work</b> on an issue) opens a fresh worktree composer.</figcaption>
</figure>

<h2 id="sandbox">2. Let the agent edit inside the sandbox</h2>
<p>In a worktree of your own, the composer's permission picker defaults to <b>Auto-review</b> (with <b>Ask me</b>, <b>Read only</b>, and <b>Allow all</b> beside it). A Work run can edit files inside the worktree, while <code>.git</code>, repository agent configs, and your <code>GITHUB_TOKEN</code> stay locked out.</p>
<p>When a run needs something outside its sandbox—such as reaching a package registry or running <code>git commit</code>—Perch shows an approval card right in the thread with <b>Allow once</b>, <b>Always allow here</b>, and <b>Deny</b>. See <a href="agents.html#approvals">Approvals and repository grants</a>.</p>

<h2 id="inspect">3. Inspect the diff and commit the changes</h2>
<p>Before and after every Work run, Perch snapshots the worktree under <code>refs/perch/runs/</code>. Switch to <b>Code</b> (or choose <b>Last run</b> in the file tree's scope menu) to review the exact diff with language-server navigation and <a href="worktree-reviews.html">agent reviews</a>.</p>
<figure>
  <div class="plate-frame">
    <picture>
      <source srcset="../img/work-dark.png?v=7" media="(prefers-color-scheme: dark)">
      <img src="../img/work-light.png?v=7" width="1794" height="1365" loading="lazy" alt="Perch worktree conversation showing the Pull request status bar and completed Codex Astra 6.2 Work summary">
    </picture>
  </div>
  <figcaption>After a Work run edits files, the <b>Pull request</b> bar reports uncommitted changes until the branch has commits ahead of its base.</figcaption>
</figure>
<p>Opening a pull request requires at least one commit ahead of the base branch—uncommitted files are named and left out. Ask the agent in the follow-up box to commit the change (approving <code>git commit</code> once or granting <b>Always let Work commit here</b>; Codex Work runs can run a plain <code>git commit</code> directly), or commit from a terminal in the worktree.</p>

<h2 id="open-pr">4. Push and open the pull request</h2>
<p>Once your branch has commits past its base, click <b>Open pull request…</b>, adjust the title or click <b>Write with Claude</b> / <b>Codex</b> to summarize the branch, and submit. Perch pushes the branch with your git credentials—never with force—and turns the worktree item into the live pull request in place. See <a href="opening-a-pull-request.html">Opening a pull request</a> for push credentials, SSH key passphrases, and failure handling.</p>
<div class="next-steps">
  <a href="index.html">
    <b>Browse all guide topics →</b>
    <span>Look up sandbox boundaries, external knowledge, SSH hosts, config.toml keys, and keyboard shortcuts.</span>
  </a>
</div>
