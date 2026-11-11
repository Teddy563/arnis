"""Builds docs/23-ARNIS-VS-CONFLUENCE.pdf from results.jsonl and results-b3.jsonl (this folder).
Usage: python make_pdf.py [out.pdf]     Needs Microsoft Edge (headless print-to-pdf); no other dependency.
Every benchmark number is computed here from the jsonl files. make_md.py reuses the helpers.
"""
import json, re, statistics, subprocess, sys, tempfile, time
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
RAM_MB = 31 * 1024
EDGE = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
AREAS = ("S8", "S16", "S24")


def load():
    rows, bad = [], []
    for name in ("results.jsonl", "results-b3.jsonl"):
        for l in (HERE / name).read_text().splitlines():
            if l.strip():
                r = json.loads(l)
                if r["arm"] == "prewarm":
                    continue
                (rows if r["exit"] == 0 else bad).append(r)
    return rows, bad


ROWS, BAD = load()
G = defaultdict(list)
for r in ROWS:
    G[(r["arm"], r["area"])].append(r)


def st(arm, area):
    rs = G[(arm, area)]
    w = sorted(r["wall_s"] for r in rs)
    return dict(n=len(w), min=w[0], med=statistics.median(w), max=w[-1],
                rss=max(r["peak_rss_mb"] for r in rs), cpu=statistics.median(r["cpu_s"] for r in rs),
                world=statistics.median(r["world_mb"] for r in rs), km2=rs[0]["km2"])


def cell_of(r):
    f = r["flags"]
    return int(f[f.index("--unit-regions") + 1]), int(f[f.index("--one-world-workers") + 1])


def sweep():
    c = defaultdict(list)
    for r in G[("A3", "S16")]:
        c[cell_of(r)].append(r)
    s = {k: dict(n=len(v), min=min(x["wall_s"] for x in v), med=statistics.median(x["wall_s"] for x in v),
                 max=max(x["wall_s"] for x in v), rss=max(x["peak_rss_mb"] for x in v)) for k, v in c.items()}
    ok = sorted((k for k in s if s[k]["rss"] < 0.7 * RAM_MB), key=lambda k: s[k]["med"])
    win, run = ok[0], ok[1]
    tie = s[win]["min"] <= s[run]["med"] <= s[win]["max"]
    pick = min((win, run), key=lambda k: s[k]["rss"]) if tie else win
    return s, win, run, tie, pick


def pct(a, b):
    return f"{(a / b - 1) * 100:+.1f}%"


def gb(mb):
    return f"{mb / 1024:.1f} GB"


def breakeven():
    out = []
    for area in ("B100", "B150", "B200", "S16"):
        a = st("A0b3" if area == "S16" else "A0", area)
        out.append((area, a["km2"], a, st("W", area)))
    return out


# ---------------------------------------------------------------- Table 1 (features)
UP = "from upstream Arnis"
FEATURES = [
    ("One World mode (many regions, one world folder)", "yes", "yes"),
    ("Build in pieces with parallel workers (<code>--unit-regions</code>, <code>--one-world-workers</code>)", "no", "yes"),
    ("Auto piece size and worker count from RAM and CPU", "no", "yes"),
    ("RAM budget and thread limit (<code>--ram-budget-mb</code>, <code>--threads</code>)", "no", "yes"),
    ("Resume a job, rebuild chosen pieces (<code>--skip-built</code>, <code>--only-pieces</code>)", "no", "yes"),
    ("Piece plan on the map, with time and RAM estimate", "no", "yes"),
    ("Map legend (? button)", "no", "yes"),
    ("Projects tab: open a world, expand it", "no", "yes"),
    ("Download now (fill the caches, build nothing)", "no", "yes"),
    ("Offline mode, local tile archive, Region Download", "no", "yes"),
    ("Water detail, river bed, climate mode, seed, height limits", "no", "yes"),
    ("Rocks, bushes, tree packs, props", "no", "yes"),
    ("Cave style and cave ores", "no", "yes"),
    ("Road detail, no-buildings, chest loot table", "no", "yes"),
    ("B_Linear region format, world border, origin", "no", "yes"),
    ("Polygon Area shapes for selecting land", "no", f"yes ({UP})"),
    ("Multi-area merge", "no", f"yes ({UP})"),
    ("Discord status", "no", f"yes ({UP})"),
    ("Parallel path for Bedrock output", "no", f"yes ({UP})"),
    ("Faster Java NBT writer", "no", f"yes ({UP})"),
]


def features_html():
    body = "".join(f"<tr><td>{n}</td><td>{a}</td><td>{b}</td></tr>" for n, a, b in FEATURES)
    return "<table class=f><tr><th>Feature</th><th>Arnis 3.4.0</th><th>Confluence.4</th></tr>" + body + "</table>"


# ---------------------------------------------------------------- Table 2 (speed)
def speed_html():
    rows = ""
    for area in AREAS:
        a, b = st("A0b3", area), st("A2b3", area)
        rows += (f"<tr><td>{area} ({a['km2']:.0f} km&sup2;)</td>"
                 f"<td>{a['med']:.1f} s</td><td>{b['med']:.1f} s</td><td>{pct(b['med'], a['med'])}</td>"
                 f"<td>{gb(a['rss'])}</td><td>{gb(b['rss'])}</td><td>{pct(b['rss'], a['rss'])}</td>"
                 f"<td>{a['world'] / 1024:.1f} GB</td><td>{b['world'] / 1024:.1f} GB</td></tr>")
    return ("<table><tr><th rowspan=2>Area</th><th colspan=3>Wall time, median of 3</th><th colspan=3>Peak RAM</th>"
            "<th colspan=2>World size</th></tr><tr><th>Stock</th><th>Tuned Auto</th><th>Ratio</th>"
            "<th>Stock</th><th>Tuned Auto</th><th>Ratio</th><th>Stock</th><th>Tuned Auto</th></tr>" + rows + "</table>")


def grid_html():
    s, win, run, tie, pick = sweep()
    h = ("<table class=g><tr><th>Regions per piece side / workers</th>" +
         "".join(f"<th>{w} workers</th>" for w in (2, 4, 6)) + "</tr>")
    for u in (2, 4, 8):
        h += f"<tr><th>{u}&times;{u}</th>"
        for w in (2, 4, 6):
            c = s[(u, w)]
            best = (u, w) == pick
            h += (f"<td{' class=win' if best else ''}>{c['med']:.1f} s ({c['min']:.1f}&ndash;{c['max']:.1f})"
                  f"<br>{gb(c['rss'])}{'<br><b>Recommended</b>' if best else ''}</td>")
        h += "</tr>"
    return h + "</table>"


def notes():
    s, win, run, tie, pick = sweep()
    a = {x: (st("A0b3", x), st("A2b3", x)) for x in AREAS}
    w16 = st("W", "S16")
    return {
        "speed": (f"Pieces trade a little speed for much lower RAM. Stock is faster at {a['S8'][0]['km2']:.0f} km&sup2; "
                  f"({a['S8'][0]['med']:.1f} s against {a['S8'][1]['med']:.1f} s; tuned Auto builds this size as one run) and at "
                  f"{a['S24'][0]['km2']:.0f} km&sup2; ({a['S24'][0]['med']:.1f} s against {a['S24'][1]['med']:.1f} s). "
                  f"At {a['S16'][0]['km2']:.0f} km&sup2; tuned Auto took {a['S16'][1]['med']:.1f} s against {a['S16'][0]['med']:.1f} s, "
                  f"with {gb(a['S16'][1]['rss'])} peak RAM against {gb(a['S16'][0]['rss'])} ({pct(a['S16'][1]['rss'], a['S16'][0]['rss'])}); "
                  f"at {a['S24'][0]['km2']:.0f} km&sup2; the RAM is {gb(a['S24'][1]['rss'])} against {gb(a['S24'][0]['rss'])} ({pct(a['S24'][1]['rss'], a['S24'][0]['rss'])}). "
                  f"Pieces write a slightly larger world ({pct(a['S16'][1]['world'], a['S16'][0]['world'])} at {a['S16'][0]['km2']:.0f} km&sup2;)."),
        "grid": (f"Sweep on {a['S16'][0]['km2']:.0f} km&sup2; (S16), median of 3 (min&ndash;max) and peak RAM, run before the tune. "
                 f"The best median is {s[win]['med']:.1f} s ({win[0]}&times;{win[0]} pieces, {win[1]} workers); the next, "
                 f"{run[0]}&times;{run[0]} with {run[1]} workers, is {s[run]['med']:.1f} s, "
                 f"{'inside' if tie else 'outside'} the best cell's range, so the tie rule "
                 f"{'picks the lower RAM' if tie else 'does not apply and the fastest cell wins'}. "
                 f"Tuned Auto matches the best sweep result only at {a['S16'][0]['km2']:.0f} km&sup2;: "
                 f"{a['S16'][1]['med']:.1f} s against {w16['med']:.1f} s for the fixed cell, both in the second session."),
    }


CSS = """
@page{size:A4;margin:11mm 12mm}
body{font:8.6pt/1.3 "Segoe UI",Arial,sans-serif;color:#1b1b1b;margin:0}
h1{font-size:15pt;margin:0 0 2px}h2{font-size:10.5pt;margin:9px 0 3px}
p{margin:2px 0 4px}.sub{color:#555;margin-bottom:5px}
table{border-collapse:collapse;width:100%;margin:2px 0 4px}
th,td{border:1px solid #c9c9c9;padding:1.5px 5px;text-align:left;vertical-align:top}
th{background:#eef1f4}table.f td:nth-child(2){width:14%}table.f td:nth-child(3){width:26%}
table.g td,table.g th{text-align:center}td.win{background:#e3f1e3}
code{font:7.8pt Consolas,monospace}.foot{font-size:7.6pt;color:#333;border-top:1px solid #bbb;margin-top:6px;padding-top:3px}
.pb{break-before:page}
"""


def footer():
    return ("Machine: 24 threads, 31 GB RAM, Windows 11; one machine, three areas. Runs on 2026-11-10 and 2026-11-11. "
            "Stock = Arnis 3.4.0 exe (sha256 aafa66e9&hellip;). Confluence.4 = branch tip build a78668b6 (sweep), "
            "plus the tuned Auto build (branch c35/c4-tune, commit 1cefd515, sha256 b295e96b&hellip;) for the tuned numbers. "
            "Caches warm; the two cold-cache runs (one each) are in docs/23-BENCH-PIECES-VS-SINGLE.md. "
            "Wall time is the median of 3 runs; the grid also shows min&ndash;max. "
            "The second session ran about 20% faster than the first (stock S16 "
            f"{st('A0', 'S16')['med']:.1f} s, then {st('A0b3', 'S16')['med']:.1f} s), so the tuned comparison uses stock reruns from the same session. "
            f"Failed, timed-out or out-of-memory runs: {len(BAD)}. Source: docs/bench/results*.jsonl, built by make_pdf.py.")


def html(part=0):
    n = notes()
    h = f"""<!doctype html><meta charset=utf-8><title>Arnis 3.4.0 and Confluence.4</title><style>{CSS}</style>
<h1>Arnis 3.4.0 and Confluence.4</h1>
<p class=sub>What differs, and what it costs on a large area. Confluence.4 is a test build from the fork, not an official Arnis release.</p>
<h2>1. Features</h2>{features_html()}
<p>With Advanced features off, the world matches stock Arnis main on the test area (0 of 840 files differ). Thirteen window controls
from the 3.4.0 fork are not back; their flags still work on the command line.</p>
<div class=pb></div>
<h2>2. Speed and RAM on three areas</h2>
<p>Stock 3.4.0 against the tuned Auto setting (<code>--one-world --unit-regions auto --one-world-workers auto</code>).
Ratio = tuned Auto against stock; a minus sign means tuned Auto is lower. No run timed out or ran out of memory.</p>
{speed_html()}<p>{n['speed']}</p>
<h2>Sweep: piece size and worker count</h2>{grid_html()}<p>{n['grid']}</p>
<p>These results come from one dense city on one machine. A rural area, a slower disk or less RAM can move them.</p>
<div class=foot>{footer()}</div>"""
    if part:  # preview of one page: split at the page break
        a, b = h.split("<div class=pb></div>")
        head = a.split("<h1>")[0]
        h = a if part == 1 else head + b
    return h


def previews(outdir):
    for part in (1, 2):
        h = Path(tempfile.gettempdir()) / f"arnis-vs-confluence-p{part}.html"
        h.write_text(html(part), encoding="utf-8")
        with tempfile.TemporaryDirectory() as prof:
            subprocess.run([EDGE, "--headless", "--disable-gpu", "--hide-scrollbars", f"--user-data-dir={prof}",
                            "--window-size=794,1123", f"--screenshot={outdir / f'preview-p{part}.png'}", h.as_uri()],
                           check=True, timeout=120)
        time.sleep(2)  # Edge returns before the file is written


def build(out):
    h = Path(tempfile.gettempdir()) / "arnis-vs-confluence.html"
    h.write_text(html(), encoding="utf-8")
    with tempfile.TemporaryDirectory() as prof:  # own profile: never joins a running Edge
        subprocess.run([EDGE, "--headless", "--disable-gpu", "--no-pdf-header-footer", f"--user-data-dir={prof}",
                        f"--print-to-pdf={out}", h.as_uri()], check=True, timeout=120)
    return out


if __name__ == "__main__":
    out = Path(sys.argv[1] if len(sys.argv) > 1 else HERE.parent / "23-ARNIS-VS-CONFLUENCE.pdf").resolve()
    build(out)
    previews(HERE)
    print(out, len(re.findall(rb"/Type\s*/Page[^s]", out.read_bytes())), "pages")
