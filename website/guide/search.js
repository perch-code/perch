// Search the generated guide text locally. Each record points to a real section.
const form = document.querySelector(".guide-search");
const field = form.elements.q;
const results = document.querySelector("#search-results");
const contents = document.querySelector(".parts");
const statusLine = results.querySelector('[role="status"]');
const list = results.querySelector("ul");

function rank(section, phrase, terms) {
  const heading = (section.heading || section.chapter).toLowerCase();
  const chapter = section.chapter.toLowerCase();
  const body = section.body.toLowerCase();
  let score = 0;
  for (const term of terms) {
    if (heading.includes(term)) score += 12;
    else if (chapter.includes(term)) score += 5;
    else if (body.includes(term)) score += 1;
    else return 0;
  }
  if (heading === phrase) score += 40;
  if (heading.includes(phrase)) score += 15;
  if (body.includes(phrase)) score += 4;
  return score;
}

function excerpt(body, phrase, terms) {
  const lower = body.toLowerCase();
  let position = lower.indexOf(phrase);
  if (position < 0) position = terms.map(term => lower.indexOf(term)).find(index => index >= 0) ?? 0;
  const start = position > 65 ? body.lastIndexOf(" ", position - 40) + 1 : 0;
  const sample = body.slice(start, start + 185);
  return (start ? "…" : "") + sample + (start + sample.length < body.length ? "…" : "");
}

function search() {
  const phrase = field.value.trim().toLowerCase().replace(/\s+/g, " ");
  results.hidden = !phrase;
  contents.hidden = Boolean(phrase);
  list.replaceChildren();
  if (!phrase) return;
  const terms = phrase.split(" ");
  const matches = guideSections.map(section => ({section, score: rank(section, phrase, terms)}))
    .filter(match => match.score > 0).sort((a, b) => b.score - a.score);
  statusLine.textContent = matches.length
    ? matches.length + (matches.length === 1 ? " match" : " matches")
    : "No matches. Try a command, setting, or chapter title.";
  for (const {section} of matches) {
    const item = document.createElement("li");
    const link = document.createElement("a");
    link.href = section.href;
    link.textContent = section.heading ? section.chapter + " · " + section.heading : section.chapter;
    const snippet = document.createElement("p");
    snippet.textContent = excerpt(section.body, phrase, terms);
    item.append(link, snippet);
    list.append(item);
  }
}

field.value = new URLSearchParams(location.search).get("q") || "";
field.addEventListener("input", search);
search();
