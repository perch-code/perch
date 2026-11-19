// Keep the mobile scroll offset in sync with the sticky header, including its safe area.
const siteBar = document.querySelector(".bar");
new ResizeObserver(() => {
  document.documentElement.style.setProperty("--mobile-header-height", siteBar.offsetHeight + "px");
}).observe(siteBar, {box: "border-box"});

// Copy buttons beside install commands.
for (const button of document.querySelectorAll("[data-copy]")) {
  const status = button.parentElement.nextElementSibling;
  let resetTimer;
  button.addEventListener("click", async () => {
    const target = document.getElementById(button.dataset.copy);
    if (!target) return;
    const text = target.textContent.trim();
    clearTimeout(resetTimer);
    let copied = true;
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const selection = window.getSelection();
      selection.selectAllChildren(target);
      copied = document.execCommand("copy");
      if (copied) selection.removeAllRanges();
    }
    button.classList.toggle("is-copied", copied);
    status.textContent = copied ? "Command copied" : "Command selected. Use your device’s Copy action.";
    status.classList.toggle("show-message", !copied);
    resetTimer = setTimeout(() => {
      button.classList.remove("is-copied");
      status.textContent = "";
      status.classList.remove("show-message");
    }, copied ? 2000 : 8000);
  });
}

// Guide sidebar is collapsible on narrow screens and open on desktop.
const toc = document.querySelector("details.toc");
if (toc) {
  const wide = matchMedia("(min-width: 58.01rem)");
  const sync = () => (toc.open = wide.matches);
  wide.addEventListener("change", sync);
  sync();
}

// Match the browser's surrounding canvas to the footer as it comes into view.
const footer = document.querySelector(".foot");
if (footer && "IntersectionObserver" in window) {
  const root = document.documentElement;
  const scheme = matchMedia("(prefers-color-scheme: dark)");
  const themeColors = document.querySelectorAll('meta[name="theme-color"]');
  function syncTheme() {
    const property = root.classList.contains("at-footer") ? "--band-deep" : "--band";
    const color = getComputedStyle(root).getPropertyValue(property).trim();
    for (const meta of themeColors) {
      if (matchMedia(meta.media).matches) meta.content = color;
    }
  }
  new IntersectionObserver(([entry]) => {
    root.classList.toggle("at-footer", entry.isIntersecting);
    syncTheme();
  }).observe(footer);
  scheme.addEventListener("change", syncTheme);
}

// Search from any page using the guide's existing GET endpoint.
const headerNav = document.querySelector('.bar nav');
const headerSearch = document.createElement("form");
headerSearch.className = "header-search";
headerSearch.action = headerNav.querySelector(".nav-guide").href;
headerSearch.setAttribute("role", "search");
headerSearch.setAttribute("aria-label", "Site header");
headerSearch.innerHTML = `
  <button class="submit-query" type="submit" aria-label="Submit search" title="Search" hidden>
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><circle cx="10" cy="10" r="6.5"/><path d="m15 15 5 5"/></svg>
  </button>
  <input id="header-query" type="search" name="q" aria-label="Search the guide" placeholder="Search the guide" enterkeyhint="search" required hidden>
  <button type="button" class="search-toggle" aria-label="Search guide" title="Search guide" aria-expanded="false" aria-controls="header-query" aria-keyshortcuts="Meta+K Control+K /">
    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true">
      <g class="magnifier"><circle cx="10" cy="10" r="6.5"/><path d="m15 15 5 5"/></g>
      <path class="close-search" d="m6 6 12 12M18 6 6 18"/>
    </svg>
  </button>`;
headerNav.prepend(headerSearch);
const toggleSearch = headerSearch.querySelector(".search-toggle");
const headerQuery = headerSearch.elements.q;
const submitQuery = headerSearch.querySelector(".submit-query");

function expandHeaderSearch(open) {
  headerNav.classList.toggle("search-open", open);
  toggleSearch.setAttribute("aria-expanded", String(open));
  toggleSearch.title = open ? "Close search" : "Search guide";
  toggleSearch.setAttribute("aria-label", toggleSearch.title);
  headerQuery.hidden = submitQuery.hidden = !open;
  const target = open ? headerQuery : toggleSearch;
  const bounds = target.getBoundingClientRect();
  target.focus({preventScroll: bounds.top >= 0 && bounds.bottom <= innerHeight});
}
toggleSearch.addEventListener("click", () => expandHeaderSearch(headerQuery.hidden));
headerSearch.addEventListener("keydown", event => {
  if (event.key === "Escape" && !headerQuery.hidden) {
    event.preventDefault();
    expandHeaderSearch(false);
  }
});

document.addEventListener("keydown", event => {
  if (event.defaultPrevented || event.isComposing || event.altKey) return;
  const commandK = event.ctrlKey !== event.metaKey && !event.shiftKey && event.key.toLowerCase() === "k";
  const slash = event.key === "/" && !event.ctrlKey && !event.metaKey;
  const target = event.target;
  const editing = target instanceof HTMLElement &&
    (target.isContentEditable || target.closest("input, textarea, select, [role='textbox']"));
  if (commandK || (slash && !editing)) {
    event.preventDefault();
    expandHeaderSearch(true);
  }
});
