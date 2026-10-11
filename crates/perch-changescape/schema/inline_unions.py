"""Write a page schema to stdout with each discriminated union's members
inlined from $defs. typify 0.8 ignores `discriminator`, and makes an
internally tagged enum only from members written inline, each with its
tag as a `const`. From $refs it makes an untagged enum, which reads two
members of the same shape, such as a text block and a narration block,
as the first of them."""

import json
import sys


def inline(node, defs):
    if isinstance(node, dict):
        if "discriminator" in node and "oneOf" in node:
            node["oneOf"] = [
                dict(defs[m["$ref"].rsplit("/", 1)[-1]]) if "$ref" in m else m
                for m in node["oneOf"]
            ]
            del node["discriminator"]
        for value in node.values():
            inline(value, defs)
    elif isinstance(node, list):
        for value in node:
            inline(value, defs)


schema = json.load(open(sys.argv[1]))
inline(schema, schema.get("$defs", {}))
json.dump(schema, sys.stdout, indent=1)
