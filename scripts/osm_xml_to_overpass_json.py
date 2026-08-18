#!/usr/bin/env python3
"""Convert an OpenStreetMap XML file into the Overpass JSON that `--file` reads.

This fork's `--file` accepts Arnis's own JSON dump (which is Overpass JSON) and not
raw OSM XML: upstream's local-.osm support (louis-e/arnis#1205) was never ported here.
The golden-hash fixtures under tests/fixtures are kept in upstream's original .osm.gz
form so they stay byte-traceable to the commit they came from, and this script bridges
the format gap at harness runtime.

Deterministic on purpose: elements are emitted in document order, tags in the order the
file lists them, and nothing is inferred or defaulted. Two runs over the same fixture
produce byte-identical JSON, which is what makes the golden hashes meaningful.

If #1205 is ever ported, delete this and pass the .osm straight to --file.

Usage:
    osm_xml_to_overpass_json.py IN.osm OUT.json
    gunzip -c fixture.osm.gz | osm_xml_to_overpass_json.py - OUT.json
"""

import json
import sys
import xml.etree.ElementTree as ET


def tags_of(elem):
    """Tag map for one element, in document order."""
    tags = {}
    for tag in elem.findall("tag"):
        key = tag.get("k")
        value = tag.get("v")
        if key is not None and value is not None:
            tags[key] = value
    return tags


def convert(source):
    """Stream the XML into a list of Overpass-shaped elements."""
    elements = []
    for _event, elem in ET.iterparse(source, events=("end",)):
        kind = elem.tag
        if kind not in ("node", "way", "relation"):
            continue

        raw_id = elem.get("id")
        if raw_id is None:
            elem.clear()
            continue

        out = {"type": kind, "id": int(raw_id)}

        if kind == "node":
            lat, lon = elem.get("lat"), elem.get("lon")
            # A node with no position is unusable downstream; drop it rather than
            # emitting a null that the deserializer would have to tolerate.
            if lat is None or lon is None:
                elem.clear()
                continue
            out["lat"] = float(lat)
            out["lon"] = float(lon)
        elif kind == "way":
            out["nodes"] = [int(nd.get("ref")) for nd in elem.findall("nd") if nd.get("ref")]
        else:
            members = []
            for member in elem.findall("member"):
                ref = member.get("ref")
                if ref is None:
                    continue
                # `role` is not optional on the Rust side, so an absent role becomes
                # the empty string, which is what Overpass itself emits.
                members.append(
                    {
                        "type": member.get("type", ""),
                        "ref": int(ref),
                        "role": member.get("role", "") or "",
                    }
                )
            out["members"] = members

        tags = tags_of(elem)
        if tags:
            out["tags"] = tags

        elements.append(out)
        elem.clear()

    return elements


def main():
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2

    src, dst = sys.argv[1], sys.argv[2]
    source = sys.stdin.buffer if src == "-" else src
    elements = convert(source)
    if not elements:
        print(f"error: no node/way/relation elements found in {src}", file=sys.stderr)
        return 1

    with open(dst, "w", encoding="utf-8", newline="\n") as handle:
        json.dump({"version": 0.6, "elements": elements}, handle, sort_keys=False)
    print(f"{src} -> {dst}: {len(elements)} elements", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
