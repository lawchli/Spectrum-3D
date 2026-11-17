#!/usr/bin/env python3
"""Static checks for the web version.

Catches what a browser would only show at runtime, or what breaks the house rules:
  - JavaScript syntax (node --check, as ES modules)
  - GLSL ES 3.00: #version line, no #extension, no reserved words as identifiers
  - interface text: no "·" joining values, every data-i18n key and t('key') present in both languages
  - style.css: no backdrop-filter (frosted glass), no external fonts or URLs
  - index.html: no external scripts or stylesheets (zero dependencies)
With --compile, a disposable headless Chrome also compiles every shader and links
the actual program pairs found in renderer.js. No preview server is needed.

    python3 scripts/web-check.py
"""
from pathlib import Path
import re
import shutil
import subprocess
import sys
import argparse
import json
import tempfile

ROOT = Path(__file__).resolve().parents[1]
WEB = ROOT / "web"

# GLSL ES 3.00 spec, section 3.8: keywords reserved for future use. Using one as a name
# compiles on some drivers and fails on others (the first round's `active` did).
RESERVED = set("""
attribute varying coherent volatile restrict readonly writeonly resource atomic_uint noperspective
patch sample subroutine common partition active asm class union enum typedef template this goto
inline noinline public static extern external interface long short double half fixed unsigned
superp input output hvec2 hvec3 hvec4 dvec2 dvec3 dvec4 fvec2 fvec3 fvec4 sampler3DRect
filter image1D image2D image3D imageCube iimage1D iimage2D iimage3D iimageCube uimage1D uimage2D
uimage3D uimageCube image1DArray image2DArray iimage1DArray iimage2DArray uimage1DArray
uimage2DArray imageBuffer iimageBuffer uimageBuffer sampler1D sampler1DShadow sampler1DArray
sampler1DArrayShadow isampler1D isampler1DArray usampler1D usampler1DArray sampler2DRect
sampler2DRectShadow isampler2DRect usampler2DRect samplerBuffer isamplerBuffer usamplerBuffer
sampler2DMS isampler2DMS usampler2DMS sampler2DMSArray isampler2DMSArray usampler2DMSArray
sizeof cast namespace using
""".split())
TYPES = r"(?:float|int|uint|bool|vec[234]|ivec[234]|uvec[234]|bvec[234]|mat[234](?:x[234])?)"

problems = []


def problem(path, line, text):
    problems.append(f"{path.relative_to(ROOT)}:{line}: {text}")


def check_js():
    node = shutil.which("node")
    if not node:
        print("note: node not found, JavaScript syntax not checked", file=sys.stderr)
        return
    for path in sorted(WEB.rglob("*.js")):
        r = subprocess.run([node, "--input-type=module", "--check"], stdin=path.open("rb"),
                           capture_output=True, text=True)
        if r.returncode:
            first = next((l for l in r.stderr.splitlines() if "Error" in l), r.stderr.strip()[:200])
            problem(path, 0, f"JavaScript syntax: {first}")


def check_glsl():
    for path in sorted(path for path in WEB.rglob("*") if path.suffix in (".glsl", ".vert", ".frag")):
        lines = path.read_text().splitlines()
        if path.suffix in (".vert", ".frag") and (not lines or lines[0].strip() != "#version 300 es"):
            problem(path, 1, "first line must be #version 300 es")
        for i, line in enumerate(lines, 1):
            code = line.split("//")[0]
            if code.strip().startswith("#extension"):
                problem(path, i, "no #extension: the same source must compile on Android GLES 3.0")
            if code.strip().startswith("#"):
                continue  # #version, #include "common.glsl" and other directives
            for word in re.findall(r"[A-Za-z_]\w*", code):
                if word in RESERVED:
                    problem(path, i, f"'{word}' is reserved in GLSL ES 3.00")
            for name in re.findall(rf"\b{TYPES}\s+([A-Za-z_]\w*)", code):
                if name.startswith("gl_") or "__" in name:
                    problem(path, i, f"'{name}': names starting with gl_ or containing __ are reserved")


def i18n_keys():
    text = (WEB / "src/i18n.js").read_text()
    blocks = re.split(r"\n  (zh|en): \{", text)
    keys = {}
    for lang, body in zip(blocks[1::2], blocks[2::2]):
        body = body.split("\n  },")[0]
        keys[lang] = set(re.findall(r"^\s*'?([\w.]+)'?\s*:", body, re.M))
    return keys


def check_text():
    keys = i18n_keys()
    for lang in ("zh", "en"):
        if lang not in keys:
            problem(WEB / "src/i18n.js", 0, f"no '{lang}' table found")
            return
    for missing in sorted(keys["zh"] ^ keys["en"]):
        problem(WEB / "src/i18n.js", 0, f"key '{missing}' is in only one language")
    html = WEB / "index.html"
    used = set(re.findall(r'data-i18n(?:-tip|-label)?="([\w.]+)"', html.read_text()))
    for path in (WEB / "src").glob("*.js"):
        used |= {k for k in re.findall(r"\bt\('([\w.]+)'", path.read_text())}
    for key in sorted(used - keys["zh"]):
        problem(html, 0, f"text key '{key}' is used but not defined")
    for path in [html, WEB / "src/i18n.js", WEB / "style.css"]:
        for i, line in enumerate(path.read_text().splitlines(), 1):
            if "·" in line:
                problem(path, i, "do not join values with '·'")


def check_css_html():
    css = WEB / "style.css"
    for i, line in enumerate(css.read_text().splitlines(), 1):
        if "backdrop-filter" in line:
            problem(css, i, "no backdrop-filter (frosted glass)")
        if re.search(r"@import|url\(\s*['\"]?https?:", line):
            problem(css, i, "no external fonts or resources")
    html = WEB / "index.html"
    for i, line in enumerate(html.read_text().splitlines(), 1):
        if re.search(r"<(script|link)[^>]+(src|href)=\"https?:", line):
            problem(html, i, "no external scripts or stylesheets")


def compile_glsl(args):
    shader_dir = Path(args.shader_dir).resolve() if args.shader_dir else WEB / "shaders"
    line_maps = {}

    def expand(path, stack=()):
        if path in stack:
            raise ValueError(f"cyclic include: {path.name}")
        lines, mapping = [], []
        for number, line in enumerate(path.read_text().splitlines(), 1):
            include = re.match(r'^\s*#include "([^\"]+)"', line)
            if include:
                include_path = path.parent / include[1]
                if not include_path.exists():
                    include_path = shader_dir / include[1]
                chunk, source_map = expand(include_path, (*stack, path))
                lines.extend(chunk)
                mapping.extend(source_map)
            else:
                lines.append(line)
                mapping.append((path.name, number))
        return lines, mapping

    shaders = []
    shader_paths = {path.name: path for path in shader_dir.iterdir()}
    if args.style:
        shader_paths.update({path.name: path for path in (WEB / "styles" / args.style / "shaders").iterdir()})
    for path in sorted(shader_paths.values()):
        if path.suffix not in (".vert", ".frag"):
            continue
        lines, mapping = expand(path)
        line_maps[path.name] = mapping
        shaders.append({"name": path.name, "kind": path.suffix[1:], "source": "\n".join(lines)})
    # The program list stays with the renderer, including extra passes in style worktrees.
    pairs = sorted(set(re.findall(
        r"createProgram\(\s*gl,\s*src\[['\"]([^'\"]+)['\"]\],\s*src\[['\"]([^'\"]+)['\"]\]",
        ((WEB / "src/renderer.js").read_text() + ((WEB / "styles" / args.style / "renderer.js").read_text() if args.style else "")))))
    covered = {name for pair in pairs for name in pair}
    available = {shader["name"] for shader in shaders}
    for missing in sorted(covered - available):
        problem(WEB / "shaders" / missing, 0, "program references a missing shader")
    for shader in shaders:
        if shader["name"] not in covered:
            problem(WEB / "shaders" / shader["name"], 0, "no createProgram pairing found in renderer.js")
    if not shaders or not pairs:
        raise ValueError("no shaders/programs found")
    node = shutil.which("node")
    if not node:
        raise ValueError("node required for --compile")
    with tempfile.TemporaryDirectory(prefix="spectrum-shaders-") as temp:
        manifest = Path(temp) / "manifest.json"
        manifest.write_text(json.dumps({"shaders": shaders, "pairs": pairs}))
        command = [node, str(ROOT / "scripts/shader-check.mjs"), "--manifest", str(manifest)]
        if args.cdp_port:
            command.extend(["--cdp-port", str(args.cdp_port)])
        if args.chrome:
            command.extend(["--chrome", args.chrome])
        result = subprocess.run(command, capture_output=True, text=True, timeout=90)
    try:
        report = json.loads(result.stdout)
    except json.JSONDecodeError:
        raise ValueError(f"Chrome compile runner failed: {result.stderr.strip()[:2000]}")
    for error in report["errors"]:
        for line in (error["log"] or "unknown shader error").splitlines():
            match = re.search(r"(?:ERROR|WARNING):\s*\d+:(\d+):", line)
            mapping = line_maps.get(error["shader"], [])
            number = int(match[1]) if match else 0
            name, number = mapping[number - 1] if 0 < number <= len(mapping) else (error["shader"], number)
            problem(WEB / "shaders" / name, number, f"{error['stage']}: {line}")
    if result.returncode and not report["errors"]:
        raise ValueError(f"Chrome runner exited {result.returncode}: {result.stderr[:1000]}")
    print(f"WebGL2 {args.style or 'tungsten'}: {report['compiled']}/{len(shaders)} shaders compiled, {report['linked']}/{len(pairs)} programs linked")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compile", action="store_true", help="compile/link all shaders in headless Chrome")
    parser.add_argument("--cdp-port", type=int, help="use existing Chrome debugging port")
    parser.add_argument("--chrome", help="Chrome executable")
    styles = sorted(path.parent.name for path in (WEB / "styles").glob("*/renderer.js"))
    parser.add_argument("--style", choices=styles, help="compile the shared shaders plus this style")
    parser.add_argument("--shader-dir", help="alternate shader directory for compile regression checks")
    args = parser.parse_args()
    check_js()
    check_glsl()
    check_text()
    check_css_html()
    if args.compile:
        try:
            compile_glsl(args)
        except (ValueError, OSError, subprocess.TimeoutExpired) as error:
            problem(WEB / "shaders", 0, str(error))
    for p in problems:
        print(p)
    print(f"web-check: {len(problems)} problem(s)" if problems else "web-check: OK")
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
