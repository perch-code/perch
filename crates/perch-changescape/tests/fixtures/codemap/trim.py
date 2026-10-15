"""Cut a real page.json down to what the code map reads.

Usage: python3 -I trim.py <page.json> <out.json>

Keeps the meta, every changed file (without its hunks or text), the module and
package elements (without their detail or rows), and the tour's map.
Everything else is emptied, so the result is still a valid page.
"""

import json
import sys


def trim(page):
    elements = {}
    for key, el in page["elements"].items():
        if el["kind"] not in ("module", "package"):
            continue
        el = dict(el, detail="", appears=[])
        el["refs"] = [dict(ref, rows=[]) for ref in el["refs"]]
        elements[key] = el
    files = [dict(f, hunks=[], ship={"ship": "hunks", "why": "trimmed"}) for f in page["files"]]
    tour = page.get("tour")
    tour_map = tour["map"] if tour else []
    return {
        "schema": page["schema"],
        "meta": page["meta"],
        "sections": [],
        "views": [],
        "elements": elements,
        "files": files,
        "comments": [],
        "facets": [],
        "parts": [],
        "tour": {
            "below": [],
            "chapters": [],
            "ending": None,
            "flags": [],
            "map": tour_map,
            "narrated": False,
            "summary": None,
        },
    }


def main():
    with open(sys.argv[1], encoding="utf-8") as f:
        page = json.load(f)
    out = trim(page)
    with open(sys.argv[2], "w", encoding="utf-8") as f:
        json.dump(out, f, indent=1, ensure_ascii=False)
        f.write("\n")


if __name__ == "__main__":
    main()
