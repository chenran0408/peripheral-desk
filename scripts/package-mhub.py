#!/usr/bin/env python3
"""Build the Peripheral Desk Windows distribution with its local MCHOSE driver center."""
import argparse
import base64
import filecmp
import hashlib
import io
import json
import re
import shutil
import stat
import subprocess
import tempfile
import zipfile
from html.parser import HTMLParser
from pathlib import Path

PROJECT = Path(__file__).resolve().parents[1]
WEB_PARTS = ("index.html", "mchose.ico", "assets", "cizhou", "vendor-cdn")
MAGNETIC_PAGES = ("cizhou/index.html", "cizhou/preview/index.html")
CORE_DOCS = {"README.md", "CHANGELOG.md", "NOTICE", "LICENSE", "SOURCES.md"}
CORE_NAME = re.compile(r"PeripheralDesk-(\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?)-windows-x64\.zip")
ENTRY = re.compile(r"(?:\./|/)?assets/(main-[\w-]+\.js)(?:[?#].*)?")


class EntryPoint(HTMLParser):
    def __init__(self):
        super().__init__()
        self.build = None

    def handle_starttag(self, tag, attrs):
        props = dict(attrs)
        if tag == "script" and props.get("type") == "module":
            entry = ENTRY.fullmatch(props.get("src") or "")
            if entry:
                self.build = entry[1]


def web_files(folder, prefix):
    """Take real web files only. A junction or symlink can point outside the workspace."""
    path = folder / prefix
    attributes = getattr(path.lstat(), "st_file_attributes", 0)
    if path.is_symlink() or attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT:
        raise ValueError(f"Linked workspace entry: {path}")
    if path.is_dir():
        for child in sorted(path.iterdir()):
            yield from web_files(folder, prefix / child.name)
    elif path.is_file():
        yield prefix.as_posix(), path


def sha256(path):
    with path.open("rb") as data:
        return hashlib.file_digest(data, "sha256").hexdigest()


def inputs(workspace, extras, overlays):
    files = {}
    for part in WEB_PARTS:
        if (workspace / part).exists():
            files.update(web_files(workspace, Path(part)))
    for extra in extras:
        for part in WEB_PARTS:
            if (extra / part).exists():
                files.update(web_files(extra, Path(part)))
    edits = {}
    for overlay in overlays:
        for child in sorted(overlay.iterdir()):
            if child.name.startswith(".") or child.name == "_peripheral_desk":
                continue
            edits.update(web_files(overlay, Path(child.name)))
    return files, edits


def localize_cdn(content, refs):
    for remote, local in refs:
        content = content.replace(remote, local)
    return content


def localize_magnetic_html(html, refs):
    # Next.js serializes the same inline loader into its hydration stream. Its
    # T record is sized in UTF-8 bytes, including every URL in the loader.
    loader = re.search(rb'<script>(\(function\(\)\{var name="MC_CONFIG_GLOBAL_PROD_LOADER".*?)</script>', html, re.S)
    if not loader:
        raise ValueError("The magnetic page's inline loader changed")
    original = loader[1]
    frame = f":T{len(original):x},".encode()
    if html.count(frame) != 1:
        raise ValueError("Cannot identify the magnetic page's loader record")
    new_size = len(localize_cdn(original, refs))
    return localize_cdn(html, refs).replace(frame, f":T{new_size:x},".encode(), 1)


def integrate_device_connection(module, hub):
    # Attach to the published home component's actual scan lifecycle and card
    # action. The user can still use the complete vendor gallery and editors.
    source = module.read_text(encoding="utf-8")
    binding = (hub / "device-binding.js").read_text(encoding="utf-8-sig").strip()
    factory = (hub / "device-connection.js").read_text(encoding="utf-8-sig")
    anchors = (
        ("cl=ref(0);watch(()=>Ga.chargeStatus",
         binding + ",cl=ref(0);onBeforeUnmount($a.afterEach(pdConnection.arrived));watch(()=>Ga.chargeStatus"),
        ("const ko=ir=sa.beginLegacyRefresh();Ja.abort()",
         "const ko=ir=sa.beginLegacyRefresh();pdConnection.begin(ko);Ja.abort()"),
        ("markListInit({success:_o})", "pdConnection.finish(ko,_o),markListInit({success:_o})"),
    )
    for before, after in anchors:
        if source.count(before) != 1:
            raise ValueError("M HUB device connection anchor changed: " + before[:80])
        source = source.replace(before, after, 1)
    module.write_text(source + "\n" + factory, encoding="utf-8", newline="\n")


def fork_site(originals, directory, rustc):
    """Generate project edits from the published sources in a separate build tree."""
    site = directory / "site"
    for name, source in originals.items():
        target = site / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
    local_refs = [(("https://cdn.mchose.com.cn/" + name.removeprefix("vendor-cdn/")).encode(),
                   ("/" + name).encode()) for name in originals if name.startswith("vendor-cdn/")]
    for name in originals:
        if name == "index.html" or name in MAGNETIC_PAGES or name.endswith(".css"):
            path = site / name
            data = path.read_bytes()
            data = localize_magnetic_html(data, local_refs) if name in MAGNETIC_PAGES else localize_cdn(data, local_refs)
            path.write_bytes(data)
    tool = directory / "patch-site.exe"
    subprocess.run([rustc, "--edition=2021", str(PROJECT / "src/mhub/fixes/patch-site.rs"), "-o", str(tool)], check=True)
    subprocess.run([str(tool), str(site)], check=True)
    hub = PROJECT / "src/mhub/hub"
    integrate_device_connection(site / "assets/main-DIt5vAcV.js", hub)
    head = (hub / "head.html").read_bytes().strip()
    index = site / "index.html"
    html = index.read_bytes()
    if b'name="peripheral-desk-driver-hub"' not in html:
        if b"<head>" not in html:
            raise ValueError("Cannot insert the local driver centre into index.html")
        index.write_bytes(html.replace(b"<head>", b"<head>\n" + head, 1))
    # The keyboard editor opens these independent documents in same-origin frames.
    # Apply the local policy before their scripts, without loading the main Vue UI.
    policy = next(line for line in head.splitlines() if b'http-equiv="Content-Security-Policy"' in line)
    child_head = (policy + b'\n<script src="/_peripheral_desk/connection.js"></script>'
                  b'\n<script src="/desk-hub/magnetic-models.js"></script>'
                  b'\n<link rel="stylesheet" href="/desk-hub/magnetic.css">'
                  b'\n<script defer src="/desk-hub/magnetic.js"></script>')
    for name in MAGNETIC_PAGES:
        child = site / name
        html = child.read_bytes()
        if b"<head>" not in html:
            raise ValueError(f"Missing document head in {name}")
        child.write_bytes(html.replace(b"<head>", b"<head>\n" + child_head + b"\n", 1))
    assets = site / "desk-hub"
    assets.mkdir(exist_ok=True)
    # The magnetic editor expects its model directory synchronously on a cold
    # start. Seed only its published model table, never a user's board profile.
    table = json.loads((site / "vendor-cdn/configCenter/custom/keyboardConfig.json").read_bytes())
    with zipfile.ZipFile(io.BytesIO(base64.b64decode(table["__compressBase64__"]))) as archive:
        models = json.loads(archive.read("data.json"))
    cache_key = "cz@keyboardConfig@v2@https://cdn.mchose.com.cn/configCenter"
    script = f"localStorage.setItem({json.dumps(cache_key)}, {json.dumps(json.dumps(models, ensure_ascii=False, separators=(',', ':')))});\n"
    (assets / "magnetic-models.js").write_text(script, encoding="utf-8", newline="\n")
    for name in ["desk.css", "desk.js", "switch-device.js", "magnetic.css", "magnetic.js"]:
        shutil.copyfile(hub / name, assets / name)
    built = {}
    for child in site.iterdir():
        built.update(web_files(site, Path(child.name)))
    return {name: path for name, path in built.items()
            if name not in originals or not filecmp.cmp(path, originals[name], shallow=False)}


def payload(files, target, version):
    parser = EntryPoint()
    if "index.html" not in files:
        raise ValueError("Select the web workspace containing index.html")
    parser.feed(files["index.html"].read_text(encoding="utf-8-sig"))
    if not parser.build or f"assets/{parser.build}" not in files:
        raise ValueError("index.html does not reference an included M HUB entry module")
    target.mkdir(parents=True, exist_ok=True)
    archive = target / "payload.zip"
    with zipfile.ZipFile(archive, "w") as output:
        for name, source in sorted(files.items()):
            # Stable timestamps keep the revision identity unchanged when file content is unchanged.
            info = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            with source.open("rb") as data, output.open(info, "w") as member:
                shutil.copyfileobj(data, member)
    manifest = {"schema": 1, "app_version": version, "build": parser.build, "sha256": sha256(archive)}
    (target / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"{parser.build}: {len(files)} files, {archive.stat().st_size:,} bytes, SHA-256 {manifest['sha256']}")
    return manifest


def source_bundle(target, staging, originals, edits, manifest, version):
    """Keep the upstream build and our replacements separately, with a reproducible recipe."""
    tag_url = f"https://github.com/chenran0408/peripheral-desk/tree/v{version}"
    download = f"https://github.com/chenran0408/peripheral-desk/releases/download/v{version}/{target.name}"
    info = (PROJECT / "installer/mhub/MHUB-SOURCE.md.in").read_text(encoding="utf-8")
    info = info.replace("@VERSION@", version).replace("@BUILD@", manifest["build"])
    info = info.replace("@TAG@", tag_url).replace("@DOWNLOAD@", download)
    (staging / "MHUB-SOURCE.md").write_text(info, encoding="utf-8")
    for name in ["MHUB-NOTICE.txt", "MiSans-License.pdf"]:
        shutil.copyfile(PROJECT / "installer/mhub" / name, staging / name)
    index = []
    with zipfile.ZipFile(target, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name in sorted(originals.keys() | edits.keys()):
            record = {"path": name}
            upstream = None
            if name in originals:
                upstream = originals[name].read_bytes()
                archive.writestr(f"upstream/{name}", upstream)
                origin = ("https://cdn.mchose.com.cn/" + name.removeprefix("vendor-cdn/")
                          if name.startswith("vendor-cdn/") else "https://www.mchose.com.cn/" + ("" if name == "index.html" else name))
                record.update(url=origin, upstream_sha256=hashlib.sha256(upstream).hexdigest())
            shipped = (edits.get(name) or originals[name]).read_bytes()
            if shipped != upstream:
                archive.writestr(f"edits/{name}", shipped)
            record["shipped_sha256"] = hashlib.sha256(shipped).hexdigest()
            index.append(record)
        archive.writestr("resources.json", json.dumps(index, ensure_ascii=False, indent=2) + "\n")
        for name in ["MHUB-SOURCE.md", "MHUB-NOTICE.txt", "MiSans-License.pdf"]:
            archive.write(staging / name, name)
        paths = ["LICENSE", "NOTICE", "scripts/package-mhub.py", "scripts/mhub-cizhou-source.py", "installer/PeripheralDesk.iss", "src/mhub.rs"]
        for directory in ["src/mhub", "installer/mhub"]:
            paths.extend(p.relative_to(PROJECT).as_posix() for p in (PROJECT / directory).rglob("*") if p.is_file())
        for name in paths:
            archive.write(PROJECT / name, f"project/{name}")


def extract_core(core_zip, staging):
    with zipfile.ZipFile(core_zip) as archive:
        members = {item.filename.replace("\\", "/"): item for item in archive.infolist()}
        if "PeripheralDesk.exe" not in members or CORE_DOCS - members.keys():
            raise ValueError("The input ZIP must be a Peripheral Desk core update package")
        for name, item in members.items():
            if item.is_dir():
                continue
            parts = name.split("/")
            if any(part in ("", ".", "..") or ":" in part for part in parts):
                raise ValueError(f"Invalid core member: {name}")
            if name != "PeripheralDesk.exe" and name not in CORE_DOCS and parts[0] not in ("docs", "THIRD_PARTY_LICENSES"):
                raise ValueError(f"Unexpected core member: {name}")
            path = staging.joinpath(*parts)
            path.parent.mkdir(parents=True, exist_ok=True)
            with archive.open(item) as data, path.open("wb") as out:
                shutil.copyfileobj(data, out)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True, help="Unmodified upstream M HUB frontend, including index.html")
    parser.add_argument("--vendor-extra", type=Path, action="append", default=[], help="Extra files from official servers, rooted like the workspace")
    parser.add_argument("--overlay", type=Path, action="append", default=[], help="Replacement and additional frontend files, in priority order (last wins)")
    parser.add_argument("--core-zip", type=Path, required=True, help="Core update ZIP produced by scripts/package.ps1")
    parser.add_argument("--output", type=Path, required=True, help="Destination for the installer, portable ZIP, editable source and checksums")
    parser.add_argument("--work", type=Path, required=True, help="Temporary staging directory")
    parser.add_argument("--iscc", default=shutil.which("ISCC.exe"), help="Inno Setup 6 compiler")
    parser.add_argument("--rustc", default="rustc", help="Rust compiler for project web patches")
    args = parser.parse_args()
    if not args.iscc:
        parser.error("provide --iscc or add ISCC.exe to PATH")
    match = CORE_NAME.fullmatch(args.core_zip.name)
    if not match:
        parser.error("unexpected core ZIP filename")
    version = match[1]
    args.output.mkdir(parents=True, exist_ok=True)
    args.work.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=args.work, prefix="mhub-package-") as temp:
        staging = Path(temp) / "app"
        extract_core(args.core_zip, staging)
        originals, overrides = inputs(args.workspace.resolve(), args.vendor_extra, args.overlay)
        edits = fork_site(originals, Path(temp).resolve(), args.rustc) | overrides
        manifest = payload(originals | edits, staging / "mhub-builtin", version)
        sources = Path(temp) / f"PeripheralDesk-{version}-mhub-web-source.zip"
        source_bundle(sources, staging, originals, edits, manifest, version)
        resources = Path(temp) / f"PeripheralDesk-{version}-mhub-resources.zip"
        with zipfile.ZipFile(resources, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for name in ["manifest.json", "payload.zip"]:
                archive.write(staging / "mhub-builtin" / name, name)
            for name in ["MHUB-SOURCE.md", "MHUB-NOTICE.txt", "MiSans-License.pdf"]:
                archive.write(staging / name, name)
        basename = f"PeripheralDesk-{version}-windows-x64"
        portable = Path(shutil.make_archive(str(Path(temp) / f"{basename}-portable"), "zip", staging))
        subprocess.run([
            args.iscc, "/Q", f"/DAppVersion={version}", f"/DBundleDir={staging.resolve()}",
            f"/DOutputPath={Path(temp).resolve()}",
            str(PROJECT / "installer/PeripheralDesk.iss"),
        ], check=True)
        installer = Path(temp) / f"{basename}-setup.exe"
        lines = []
        for artifact in (installer, portable, sources, resources, args.core_zip):
            lines.append(f"{sha256(artifact)}  {artifact.name}")
            print(f"{artifact.name}: {artifact.stat().st_size:,} bytes")
            destination = args.output / artifact.name
            if artifact.resolve() != destination.resolve():
                shutil.copy2(artifact, destination)
        (args.output / "SHA256SUMS.txt").write_text("\n".join(lines) + "\n", encoding="ascii")


if __name__ == "__main__":
    main()
