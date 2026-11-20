#!/usr/bin/env python3
"""Build the Peripheral Desk Windows distribution with its local MCHOSE driver center."""
import argparse
import filecmp
import hashlib
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
        if name == "index.html" or (name.startswith("vendor-cdn/") and name.endswith(".css")):
            path = site / name
            data = path.read_bytes()
            for remote, local in local_refs:
                data = data.replace(remote, local)
            path.write_bytes(data)
    tool = directory / "patch-site.exe"
    subprocess.run([rustc, "--edition=2021", str(PROJECT / "src/mhub/fixes/patch-site.rs"), "-o", str(tool)], check=True)
    subprocess.run([str(tool), str(site)], check=True)
    hub = PROJECT / "src/mhub/hub"
    head = (hub / "head.html").read_bytes().strip()
    index = site / "index.html"
    html = index.read_bytes()
    if b'name="peripheral-desk-driver-hub"' not in html:
        if b"<head>" not in html:
            raise ValueError("Cannot insert the local driver centre into index.html")
        index.write_bytes(html.replace(b"<head>", b"<head>\n" + head, 1))
    assets = site / "desk-hub"
    assets.mkdir(exist_ok=True)
    for name in ["desk.css", "desk.js"]:
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
        paths = ["LICENSE", "NOTICE", "scripts/package-mhub.py", "installer/PeripheralDesk.iss", "src/mhub.rs"]
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
