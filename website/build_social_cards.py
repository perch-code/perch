"""Regenerate the checked-in PNGs when changing the card design or page titles.

Install Instrument Serif (regular and italic), then run:
    python3 website/build_social_cards.py --chrome /path/to/chromium
The ordinary site build uses the PNGs directly.
"""
import argparse
import html
import pathlib
import subprocess
import tempfile
from string import Template

from build_guide import HOME_CARD_DESCRIPTION, HOME_CARD_TITLE, PARTS, TEMPLATES, WEBSITE


def book(title: str = "", section: str = "") -> str:
    series = '<p class="series">The Perch <em>Field Guide</em></p>' if title else ""
    heading = html.escape(title) if title else "The Perch <em>Field Guide</em>"
    part = f'<p class="part">{html.escape(section)}</p>' if section else ""
    return f"""<div class="plate"><div class="spine"></div>
      <div class="book-copy">{series}<h1>{heading}</h1>{part}</div>
      <img class="bird" src="img/wren.svg" alt=""></div>"""


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--chrome", default="chromium", help="Chromium or Chrome executable")
    args = parser.parse_args()
    template = Template((TEMPLATES / "social-card.tmpl").read_text())
    cover = f"""<div class="brand"><img src="img/wren.svg" alt="">Perch</div>
      <div class="home-copy"><h1>{html.escape(HOME_CARD_TITLE)}</h1>
      <p>{html.escape(HOME_CARD_DESCRIPTION)}</p></div>
      <img class="screen" src="img/tour/overview-light.webp" alt="">"""
    cards = [("index", "home", cover), ("guide/index", "guide", book())]
    for part in PARTS:
        for page in part["pages"]:
            cards.append(("guide/" + page["slug"], "guide", book(page["title"], part["title"])))

    with tempfile.TemporaryDirectory() as scratch:
        source = pathlib.Path(scratch) / "card.html"
        for slug, kind, art in cards:
            output = WEBSITE / "img/social" / (slug + ".png")
            output.parent.mkdir(parents=True, exist_ok=True)
            source.write_text(template.substitute(base=WEBSITE.as_uri() + "/", kind=kind, art=art))
            subprocess.run([
                args.chrome, "--headless", "--disable-gpu", "--hide-scrollbars",
                "--force-device-scale-factor=1", "--window-size=1200,630",
                f"--user-data-dir={scratch}/profile", f"--screenshot={output}", source.as_uri(),
            ], check=True)


if __name__ == "__main__":
    main()
