"""Regenerate the setting-preview pictures: python -I scripts/previews3d/previews3d.py [name-prefix] [--exe PATH] [--out DIR] [--manifest FILE] [--keep]

For each {name, bbox, flags, crop} in previews3d.json (next to this file) builds the tiny world with the
Arnis binary into a temp dir (worlds with the same bbox and flags are built once) and renders
<out>/<name>.webp, 480x360. Needs nbtlib, numpy, Pillow. Default out: src/gui/images/previews3d.
"""
import json, os, shutil, subprocess, sys, tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))  # python -I does not
import iso_render_v2 as r

ROOT = HERE.parents[1]


def main():
    argv = sys.argv[1:]
    def opt(k, d):
        if k not in argv:
            return d
        i = argv.index(k)
        v = argv[i + 1]
        del argv[i:i + 2]
        return Path(v)

    exe = opt("--exe", Path(os.environ.get("ARNIS_EXE", ROOT / "target/release/arnis.exe")))
    out = opt("--out", ROOT / "src/gui/images/previews3d")
    manifest = opt("--manifest", HERE / "previews3d.json")
    keep = "--keep" in argv
    only = next((a for a in argv if not a.startswith("--")), "")
    tmp = Path(tempfile.mkdtemp(prefix="previews3d-"))
    built, total = {}, 0.0
    try:
        for e in json.loads(manifest.read_text(encoding="utf-8")):
            if not e["name"].startswith(only):
                continue
            key = (e["bbox"], tuple(e["flags"]))
            if key not in built:
                w = tmp / f"w{len(built)}"
                w.mkdir()
                cmd = [str(exe), f"--bbox={e['bbox']}", f"--output-dir={w}", "--no-3d", *e["flags"]]
                print(" ".join(cmd[1:]), flush=True)
                subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL)
                built[key] = next(w.rglob("region")).parent
            c = dict(e["crop"])
            img, _ = r.render(built[key], slab=c.get("slab"), vert=c.get("vert", r.VERT), box=c.get("box"),
                              fx=frozenset(r.STEPS), ss=2, floor=c.get("floor"), cut=c.get("cut", False), depth=c.get("depth", 3))
            kb = r.save(img, out / f"{e['name']}.webp")
            total += kb
            print(f"  {e['name']}.webp {kb:.1f} KB", flush=True)
        print(f"total {total:.1f} KB")
    finally:
        if not keep:
            shutil.rmtree(tmp, ignore_errors=True)


main()
