"""Collect both public magnetic-keyboard entrypoints for a reviewed M HUB snapshot."""
import argparse
import base64
import hashlib
import io
import json
import re
import urllib.request
import zipfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from urllib.parse import urljoin


SITE = "https://www.mchose.com.cn/"
CDN = "https://cdn.mchose.com.cn/"
EDITOR = "cizhou/index.html"
PREVIEW = "cizhou/preview/index.html"
STATIC = "cizhou/_next/"
COMMIT = "bdf4d2c350a0fa0e93e443aa5375e26bc83cf419"
ASSET = re.compile(r"(?:/_next/|\b)(static/(?:chunks|css|media)/[A-Za-z0-9_./-]+\.(?:js|css|woff2?|ttf|png|jpe?g|webp|svg|gif|wasm|mp3))\b")


def webpack_files(text):
    # Next's runtime uses explicit filenames for the common chunks, and an ID
    # plus a digest table for the remaining lazy-loaded modules and styles.
    js = re.search(r'"static/chunks/"\+\w+\+"\."\+\(\{([^}]+)\}\)', text)
    css = re.search(r'"static/css/"\+\(\{([^}]+)\}\)', text)
    if not js or not css:
        raise ValueError("Unexpected Next.js chunk map; review this upstream build")
    pairs = r'(\d+):"([0-9a-f]+)"'
    files = {STATIC + f"static/chunks/{key}.{digest}.js" for key, digest in re.findall(pairs, js[1])}
    files.update(STATIC + f"static/css/{digest}.css" for _, digest in re.findall(pairs, css[1]))
    return files


def discover(path, body):
    if path == "vendor-cdn/configCenter/custom/keyboardPreset.json":
        # The preset map references the factory layouts for individual revisions.
        with zipfile.ZipFile(io.BytesIO(base64.b64decode(json.loads(body)["__compressBase64__"]))) as archive:
            presets = json.loads(archive.read("data.json"))
        prefix = CDN + "configCenter/static/files/"
        if not all(url.startswith(prefix) and url.endswith(".json") for url in presets.values()):
            raise ValueError("Unexpected factory preset format; review this snapshot")
        return {"vendor-cdn/" + url.removeprefix(CDN) for url in presets.values()}
    if path.startswith("vendor-cdn/"):
        return set()
    if not path.endswith((".html", ".css", ".js")):
        return set()
    text = body.decode("utf-8")
    if path in (EDITOR, PREVIEW) and f'data-commit-hash-long="{COMMIT}"' not in text:
        raise ValueError(f"{path}: upstream commit changed; review the new build")
    files = {STATIC + ref for ref in ASSET.findall(text)}
    if path in (EDITOR, PREVIEW) and '"/favicon.ico' in text:
        files.add("cizhou/favicon.ico")
    # Calibration illustrations, key editor icons and other static visual assets.
    files.update("vendor-cdn/" + p for p in re.findall(
        r'https://cdn\.mchose\.com\.cn/(configCenter/static/(?:images|files)/[A-Za-z0-9_.-]+\.(?:json|png|svg|jpe?g|webp))\b', text))
    if "72159:function(" in text and "parentLang:" in text:
        languages = re.findall(r'key:"([a-z]+)",parentLang:', text)
        files.update(f"cizhou/locales/{language}/common.json" for language in languages)
    if Path(path).name.startswith("webpack-") and path.endswith(".js"):
        files.update(webpack_files(text))
        no_js = re.search(r'else if\(/(\^\([0-9|()]+\)\$)/\.test\(\w+\)\)', text)
        if not no_js:
            raise ValueError("Cannot identify CSS-only chunks in upstream Webpack runtime")
        files = {f for f in files if not (
            f.endswith(".js") and re.fullmatch(no_js[1], re.split(r"[.-]", Path(f).name)[0]))}
    if path.endswith(".css"):
        for item in re.findall(r"""url\(\s*['"]?([^\s'")]+)""", text):
            file = urljoin(path, item).split("?")[0].split("#")[0]
            if file.startswith(STATIC + "static/media/"):
                files.add(file)
    return files


def collect(output, manifest):
    cdn_files = ["customPage/driver-ui/driver-ui.css", "customPage/driver-ui/driver-ui.global.js",
                 "customPage/driver-ui/react/driver-ui.react.js", "configCenter/custom/keyboardPreset.json"]
    todo = {EDITOR, PREVIEW, *("vendor-cdn/" + f for f in cdn_files)}
    records = {}

    def get(path):
        vendor = path.startswith("vendor-cdn/")
        url = CDN + path.removeprefix("vendor-cdn/") if vendor else SITE + path
        with urllib.request.urlopen(url, timeout=35) as response:
            body = response.read()
        if not path.endswith(".html") and body.lstrip().lower().startswith(b"<!doctype html"):
            raise ValueError(f"Unexpected HTML for {url}")
        out = output / path
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(body)
        return path, {"url": url, "size": len(body), "sha256": hashlib.sha256(body).hexdigest()}, discover(path, body)

    with ThreadPoolExecutor(max_workers=10) as pool:
        while todo:
            batch, todo = sorted(todo), set()
            for path, info, dependencies in pool.map(get, batch):
                records[path] = info
                todo.update(dependencies)
            todo.difference_update(records)
    manifest.parent.mkdir(parents=True, exist_ok=True)
    manifest.write_text(json.dumps({"commit": COMMIT, "files": records}, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    print(f"{len(records)} files, {sum(r['size'] for r in records.values()):,} bytes")


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("output", type=Path, help="Empty directory for upstream files")
    cli.add_argument("manifest", type=Path, help="Write URL and SHA-256 inventory outside the upstream directory")
    args = cli.parse_args()
    if args.output.exists() and any(args.output.iterdir()):
        cli.error("Choose an empty output directory")
    collect(args.output, args.manifest)
