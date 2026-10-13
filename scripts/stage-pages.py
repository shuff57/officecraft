#!/usr/bin/env python3
"""Stage a trunk build (dist/web) as a Cloudflare Pages upload folder.

Pages and Workers static assets cap a single file at 25 MiB and the wasm bundle is far larger,
so the wasm is split into parts that index.html fetches in parallel and reassembles before
handing the bytes to the wasm-bindgen loader. The parts keep a `.wasm` extension so they are
served as application/wasm and compressed in transit.

    stage-pages.py <dist/web> <output-dir>
"""
import math
import pathlib
import re
import shutil
import sys

PART_LIMIT = 18 * 1024 * 1024  # well under the 25 MiB per-file cap


def main(src: pathlib.Path, out: pathlib.Path) -> None:
    html = (src / "index.html").read_text()
    m = re.search(r"module_or_path: '\./([^']+_bg\.wasm)'", html)
    if not m:
        sys.exit(f"{src}/index.html has no wasm loader line; was it built with `trunk build --release`?")
    wasm_name = m.group(1)

    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True)

    js = re.search(r"from '\./([^']+\.js)'", html)
    if not js:
        sys.exit(f"{src}/index.html has no module import line")

    # Only what the page references: dist/web also keeps older hashed builds.
    for name in set(re.findall(r'href="\./([^"]+)"', html)) | {js.group(1)}:
        if name != wasm_name:
            (out / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(src / name, out / name)
    # wasm-bindgen snippets (JS helpers some crates bring) are imported by the loader script.
    if (src / "snippets").is_dir():
        shutil.copytree(src / "snippets", out / "snippets", dirs_exist_ok=True)

    data = (src / wasm_name).read_bytes()
    n = math.ceil(len(data) / PART_LIMIT)
    size = math.ceil(len(data) / n)
    stem = wasm_name[: -len("_bg.wasm")]
    parts = []
    for i in range(n):
        name = f"{stem}_bg.part{i}.wasm"
        (out / name).write_bytes(data[i * size : (i + 1) * size])
        parts.append(name)

    loader = (
        f"const __parts = [{', '.join(repr('./' + p) for p in parts)}];\n"
        "const __bufs = await Promise.all(__parts.map(async (u) => {\n"
        "  const r = await fetch(u);\n"
        "  if (!r.ok) throw new Error(u + ': ' + r.status);\n"
        "  return new Uint8Array(await r.arrayBuffer());\n"
        "}));\n"
        "const __bytes = new Uint8Array(__bufs.reduce((n, b) => n + b.length, 0));\n"
        "{ let o = 0; for (const b of __bufs) { __bytes.set(b, o); o += b.length; } }\n"
        "const wasm = await init({ module_or_path: __bytes });"
    )
    old = f"const wasm = await init({{ module_or_path: './{wasm_name}' }});"
    html = html.replace(old, loader, 1)
    # The single-file preload (with its integrity hash) is replaced by one preload per part.
    preloads = "".join(f'<link rel="preload" href="./{p}" as="fetch" crossorigin="anonymous">' for p in parts)
    html = re.sub(r'<link rel="preload" href="\./[^"]+_bg\.wasm"[^>]*>', preloads, html, count=1)
    (out / "index.html").write_text(html)

    # Hashed filenames never change content, so they can be cached for good.
    (out / "_headers").write_text(
        "/*.js\n  Cache-Control: public, max-age=31536000, immutable\n"
        "/*.wasm\n  Cache-Control: public, max-age=31536000, immutable\n"
    )
    files = [f for f in out.rglob("*") if f.is_file()]
    total = sum(f.stat().st_size for f in files)
    print(f"staged {len(files)} files, {total / 1e6:.1f} MB, wasm in {n} parts -> {out}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]))
