for (const heading of document.querySelectorAll(".chapter-head h1[id], .prose :is(h2, h3, h4)[id]")) {
  const link = document.createElement("a");
  link.className = "heading-link";
  link.href = "#" + heading.id;
  link.textContent = "#";
  link.title = "Copy link to this heading";
  link.setAttribute("aria-label", "Copy link to " + heading.textContent.trim());
  heading.append(link);

  let resetTimer;
  link.addEventListener("click", async (event) => {
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    try {
      await navigator.clipboard.writeText(link.href);
      link.textContent = "✓";
      clearTimeout(resetTimer);
      resetTimer = setTimeout(() => { link.textContent = "#"; }, 1800);
    } catch {
      // The native link still opens the fragment and supports Copy Link Address.
    }
  });
}
