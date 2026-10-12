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


def rename_clashing_enums(node, defs):
    """typify names an inline enum by its `title`. An inline enum whose
    title is also a $defs name, with other values, such as the three-value
    `Status` of a type map, would make a second `Status` type. Such an
    enum takes its record's name as a prefix, such as `TypeEdgeStatus`."""
    if isinstance(node, dict):
        props = node.get("properties")
        if isinstance(props, dict):
            for prop in props.values():
                if isinstance(prop, dict) and "enum" in prop:
                    title = prop.get("title")
                    if title in defs and defs[title].get("enum") != prop["enum"]:
                        prop["title"] = node.get("title", "") + title
        for value in node.values():
            rename_clashing_enums(value, defs)
    elif isinstance(node, list):
        for value in node:
            rename_clashing_enums(value, defs)


schema = json.load(open(sys.argv[1]))
inline(schema, schema.get("$defs", {}))
rename_clashing_enums(schema, schema.get("$defs", {}))
json.dump(schema, sys.stdout, indent=1)
