#!/usr/bin/env bash
# Golden world-hash harness for generation changes.
#
# Runs deterministic fixture generations (committed .osm.gz files, flat ground,
# no 3D models, no Overture) and compares the ARNIS_BLOCK_HASH world hash against
# tests/golden_hashes.txt.
#
# Usage:
#   scripts/golden_hash.sh              # verify all fixtures against the manifest
#   scripts/golden_hash.sh --update     # rebaseline the manifest (intentional visual change)
#   scripts/golden_hash.sh munich_altstadt levittown   # subset
#
# The harness BUILDS FIRST (`cargo build --release --bin arnis`, default features, exactly
# what CI and the release ship) and then hashes what it just built. Before that it did not,
# so a green run could be certifying a binary from an hour ago while the change under test
# sat uncompiled in the working tree - and every generation gate in this repo is this script.
#
# ARNIS_BIN overrides the binary (default target/release/arnis[.exe]) AND SUPPRESSES THE
# BUILD: a pinned binary is hashed exactly as handed over, never rebuilt. During an upstream
# port the fork builds into an isolated target dir, so set it:
#   ARNIS_BIN=c:/tmp/arnis-port-target/release/arnis.exe scripts/golden_hash.sh
# The suppression is announced in the output, because "5/5 OK" against a stale pinned exe is
# the exact failure this build step exists to prevent.
#
# MELD-DIVERGENCE from upstream's copy of this script:
#   * `--canopy-height=false` is gone: this fork deleted the canopy module.
#   * `--offline` is added in its place. Upstream let the first run populate the land-cover
#     cache from the network, which makes that run's hash a function of whatever the tile
#     server returned that day. This fork bakes its data, so the harness reads the cache or
#     fails loudly.
#   * Each fixture is converted to Overpass JSON first, and --bbox is read from its own
#     <bounds> element - see the two comments in the loop below.
#
# The world hash covers placed blocks only, so it is stable across machines for identical
# inputs.
set -euo pipefail
cd "$(dirname "$0")/.."

MANIFEST="tests/golden_hashes.txt"
FIXDIR="tests/fixtures"
BIN="${ARNIS_BIN:-}"
if [[ -n "$BIN" ]]; then
    # Caller pinned a binary. Hash THAT, unbuilt - and say so loudly, so nobody reads the
    # result as a statement about the working tree.
    echo "WARN  ARNIS_BIN is set: hashing $BIN AS-IS, no rebuild."
    echo "WARN  This says nothing about the current source tree. Unset ARNIS_BIN to gate a change."
    if [[ ! -x "$BIN" ]]; then echo "error: ARNIS_BIN=$BIN is not an executable"; exit 1; fi
else
    # Build before hashing. `--bin arnis` skips the unrelated refresh_wikidata_index binary;
    # default features (gui) are deliberate - that is the binary CI and the release produce,
    # and a --no-default-features build is a different program to hash.
    echo "BUILD cargo build --release --bin arnis"
    if ! cargo build --release --bin arnis; then
        echo "error: cargo build --release --bin arnis failed; refusing to hash a stale binary"
        exit 1
    fi
    if [[ -x target/release/arnis.exe ]]; then BIN=target/release/arnis.exe
    elif [[ -x target/release/arnis ]]; then BIN=target/release/arnis
    else echo "error: cargo build succeeded but target/release/arnis[.exe] is missing"; exit 1
    fi
fi
echo "BIN   $BIN"

UPDATE=0
FIXTURES=()
for arg in "$@"; do
    case "$arg" in
        --update) UPDATE=1 ;;
        *) FIXTURES+=("$arg") ;;
    esac
done
if [[ ${#FIXTURES[@]} -eq 0 ]]; then
    for f in "$FIXDIR"/*.osm.gz; do
        [[ -e "$f" ]] || continue
        name="$(basename "$f" .osm.gz)"
        FIXTURES+=("$name")
    done
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

declare -A EXPECTED
if [[ -f "$MANIFEST" ]]; then
    while IFS=$'\t' read -r name hash; do
        hash="${hash%$'\r'}" # a CRLF checkout must not turn every row into a false DIFF
        [[ -n "$name" && "${name:0:1}" != "#" ]] && EXPECTED[$name]="$hash"
    done < "$MANIFEST"
fi

FAIL=0
RESULTS=()
for name in "${FIXTURES[@]}"; do
    gz="$FIXDIR/$name.osm.gz"
    if [[ ! -f "$gz" ]]; then echo "SKIP  $name (no fixture $gz)"; continue; fi
    gunzip -c "$gz" > "$TMP/$name.osm"
    # This fork requires an explicit --bbox (upstream's auto-derive from a local
    # .osm file was never ported). Every fixture carries an Overpass <bounds>
    # element, so read it from the fixture rather than hardcoding a table that
    # could drift away from the data it describes.
    bounds="$(grep -m1 -o '<bounds[^/]*/>' "$TMP/$name.osm" || true)"
    if [[ -z "$bounds" ]]; then echo "ERROR $name (fixture has no <bounds>)"; FAIL=1; continue; fi
    bbox="$(sed -E 's/.*minlat="([^"]*)".*minlon="([^"]*)".*maxlat="([^"]*)".*maxlon="([^"]*)".*/\1,\2,\3,\4/' <<<"$bounds")"
    # This fork reads Overpass JSON, not OSM XML (see the converter's header).
    if ! python "$(dirname "$0")/osm_xml_to_overpass_json.py" "$TMP/$name.osm" "$TMP/$name.json" 2>"$TMP/$name.conv.log"; then
        echo "ERROR $name (fixture conversion failed)"; FAIL=1; cat "$TMP/$name.conv.log"; continue
    fi
    mkdir -p "$TMP/world_$name"
    log="$TMP/$name.log"
    if ! ARNIS_BLOCK_HASH=1 "$BIN" \
        --file "$TMP/$name.json" \
        --bbox "$bbox" \
        --output-dir "$TMP/world_$name" \
        --mode geo-only --no-3d --overture=false --offline \
        >"$log" 2>&1; then
        echo "ERROR $name (generation failed, log: $log)"; FAIL=1
        tail -5 "$log"; continue
    fi
    hash="$(grep -o 'block_hash=[0-9a-f]*' "$log" | tail -1 | cut -d= -f2)"
    if [[ -z "$hash" ]]; then echo "ERROR $name (no block_hash in output)"; FAIL=1; continue; fi
    RESULTS+=("$name	$hash")
    if [[ $UPDATE -eq 1 ]]; then
        echo "BASE  $name $hash"
    elif [[ "${EXPECTED[$name]:-}" == "$hash" ]]; then
        echo "OK    $name $hash"
    else
        echo "DIFF  $name got=$hash want=${EXPECTED[$name]:-<none>}"; FAIL=1
    fi
done

if [[ $UPDATE -eq 1 ]]; then
    {
        echo "# Golden world hashes (scripts/golden_hash.sh). Regenerate with --update."
        printf '%s\n' "${RESULTS[@]}"
    } > "$MANIFEST"
    echo "manifest updated: $MANIFEST"
    exit 0
fi
exit $FAIL
