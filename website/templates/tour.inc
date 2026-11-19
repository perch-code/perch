<main id="main-content">
  <section class="tour wrap" aria-label="A tour of Perch">
    <div class="tour-stage" hidden>
      <div class="tour-captions"></div>
      <div class="tour-screen"><a class="tour-frame"><canvas aria-hidden="true"></canvas></a></div>
    </div>
    <div class="tour-steps">
      <article class="tour-step" id="tour-overview" data-view="overview">
        <div class="tour-copy">
          <div class="tour-intro">
            <h1>Agent PRs suck. <em>Perch helps you review them.</em></h1>
            <p>The best of GitHub, Visual Studio Code, and Claude Desktop in one native app. Human reviews with agentic assistance.</p>
            <a class="button" href="#install"><svg width="21" height="21" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 3v12m-5-5 5 5 5-5M4 16v4h16v-4"/></svg>Install Perch</a>
          </div>
        </div>
        <a class="tour-still" href="img/tour/overview-light.webp" aria-label="View full screenshot. Perch in Warm Linen with the GitHub inbox, an inline Rust review thread, and the file tree.">
          <picture>
            <source media="(prefers-color-scheme: dark)" srcset="img/tour/overview-dark.webp">
            <img src="img/tour/overview-light.webp" width="2760" height="1760" fetchpriority="high" alt="Perch in Warm Linen with the GitHub inbox, an inline Rust review thread, and the file tree.">
          </picture>
        </a>
      </article>
      <article class="tour-step" id="tour-thread" data-view="thread">
        <div class="tour-copy">
          <h2>Multiple agents, one thread</h2>
          <p>Ask Claude about a line of code, then bring Codex, Gemini, or OpenCode into the same conversation.</p>
        </div>
        <a class="tour-still" href="img/tour/thread-light.webp" aria-label="View full screenshot. Two developers, Claude, and Codex discuss snapshot retention in one inline thread.">
          <picture>
            <source media="(prefers-color-scheme: dark)" srcset="img/tour/thread-dark.webp">
            <img src="img/tour/thread-light.webp" width="2760" height="1760" loading="lazy" alt="Two developers, Claude, and Codex discuss snapshot retention in one inline thread.">
          </picture>
        </a>
      </article>
      <article class="tour-step" id="tour-code" data-view="code">
        <div class="tour-copy">
          <h2>Browse the whole codebase</h2>
          <p>Follow a symbol beyond the diff. Open the file tree and use call hierarchy to see who calls a function.</p>
        </div>
        <a class="tour-still" href="img/tour/code-light.webp" aria-label="View full screenshot. The SlateDB repository tree beside the Rust editor, with the callers of start_compaction_job open below.">
          <picture>
            <source media="(prefers-color-scheme: dark)" srcset="img/tour/code-dark.webp">
            <img src="img/tour/code-light.webp" width="2760" height="1760" loading="lazy" alt="The SlateDB repository tree beside the Rust editor, with the callers of start_compaction_job open below.">
          </picture>
        </a>
      </article>
      <article class="tour-step" id="tour-review" data-view="review">
        <div class="tour-copy">
          <h2>Multi-agent code reviews</h2>
          <p>Choose the agents you want on a review. Their findings land as draft comments for you to check before posting.</p>
        </div>
        <a class="tour-still" href="img/tour/review-light.webp" aria-label="View full screenshot. The Review menu with Claude and Codex checked, each with its own model and effort setting.">
          <picture>
            <source media="(prefers-color-scheme: dark)" srcset="img/tour/review-dark.webp">
            <img src="img/tour/review-light.webp" width="2760" height="1760" loading="lazy" alt="The Review menu with Claude and Codex checked, each with its own model and effort setting.">
          </picture>
        </a>
      </article>
      <article class="tour-step" id="tour-issue" data-view="issue">
        <div class="tour-copy">
          <h2>Start work from an issue</h2>
          <p>Give an agent its own Git worktree and keep the work linked to the GitHub issue.</p>
        </div>
        <a class="tour-still" href="img/tour/issue-light.webp" aria-label="View full screenshot. The Create worktree confirmation above SlateDB issue 2233.">
          <picture>
            <source media="(prefers-color-scheme: dark)" srcset="img/tour/issue-dark.webp">
            <img src="img/tour/issue-light.webp" width="2760" height="1760" loading="lazy" alt="The Create worktree confirmation above SlateDB issue 2233.">
          </picture>
        </a>
      </article>
      <article class="tour-step" id="tour-agents" data-view="agents">
        <div class="tour-copy">
          <h2>Bring your coding agents</h2>
          <p>Use Claude Code, Codex, Gemini through Antigravity, or OpenCode. Pick your default in Settings.</p>
        </div>
        <a class="tour-still" href="img/tour/agents-light.webp" aria-label="View full screenshot. Perch’s Agents settings show the executables and models for the four supported agents.">
          <picture>
            <source media="(prefers-color-scheme: dark)" srcset="img/tour/agents-dark.webp">
            <img src="img/tour/agents-light.webp" width="2760" height="1760" loading="lazy" alt="Perch’s Agents settings show the executables and models for the four supported agents.">
          </picture>
        </a>
      </article>
      <article class="tour-step" id="tour-environment" data-view="environment">
        <div class="tour-copy">
          <h2>Local or remote</h2>
          <p>Switch environments from your profile. Git, language servers, and agents run where your code lives.</p>
        </div>
        <a class="tour-still" href="img/tour/environment-light.webp" aria-label="View full screenshot. The account menu with the current computer selected and two saved SSH hosts.">
          <picture>
            <source media="(prefers-color-scheme: dark)" srcset="img/tour/environment-dark.webp">
            <img src="img/tour/environment-light.webp" width="2760" height="1760" loading="lazy" alt="The account menu with the current computer selected and two saved SSH hosts.">
          </picture>
        </a>
      </article>
    </div>
  </section>
</main>
<dialog class="tour-viewer" aria-label="Full-size Perch screenshot">
  <div class="viewer-image"></div>
  <button type="button" class="viewer-close" aria-label="Close screenshot" autofocus>
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18"/></svg>
  </button>
</dialog>
