#!/usr/bin/env python3
"""Collect unmodified license texts for the registry packages in Cargo.lock.

Python 3.11+. Includes optional, build and target-specific lockfile packages as
an attribution superset. Never deletes old files or replaces a different text.
Run cargo fetch --locked first, using the same CARGO_HOME as the build.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
PREFIXES = ("license", "licence", "copying", "copyright", "notice", "unlicense")
# These crate archives omit their workspace-level MIT license.
WEBVIEW2_COMMITS = {
    "webview2-com-0.38.2": "b74dc5e2b394044bea5191052868ce7a106c202c",
    "webview2-com-sys-0.38.2": "b74dc5e2b394044bea5191052868ce7a106c202c",
    "webview2-com-macros-0.8.1": "dffa41a8a46d3f5565eefbff2de57d38d399f158",
}

def digest(data):
    return hashlib.sha256(data).hexdigest()

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lock", type=Path, default=ROOT / "Cargo.lock")
    parser.add_argument("--out", type=Path, default=ROOT / "THIRD_PARTY_LICENSES")
    parser.add_argument("--registry-src", type=Path, default=Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")) / "registry/src")
    parser.add_argument("--check", action="store_true", help="Verify the collected files and manifest without writing")
    args = parser.parse_args()
    args.registry_src = args.registry_src.resolve()
    packages = tomllib.loads(args.lock.read_text(encoding="utf-8"))["package"]
    inventory, planned = [], []
    for package in sorted(packages, key=lambda p: (p["name"], p["version"])):
        if not package.get("source", "").startswith("registry+"):
            continue
        identifier = f"{package['name']}-{package['version']}"
        candidates = list(args.registry_src.glob("*/" + identifier))
        if len(candidates) != 1:
            raise ValueError(f"Need one downloaded source for {identifier}; run cargo fetch --locked (found {len(candidates)})")
        source = candidates[0]
        metadata = tomllib.loads((source / "Cargo.toml").read_text(encoding="utf-8"))["package"]
        files = {p for p in source.rglob("*") if p.is_file() and p.name.lower().startswith(PREFIXES)}
        if metadata.get("license-file"):
            explicit = (source / metadata["license-file"]).resolve()
            if not explicit.is_relative_to(source.resolve()) or not explicit.is_file():
                raise ValueError("Invalid license-file path: " + identifier)
            files.add(explicit)
        checksum_path = source / ".cargo-checksum.json"
        upstream = None
        if not files and identifier in WEBVIEW2_COMMITS:
            commit = WEBVIEW2_COMMITS[identifier]
            if json.loads((source / ".cargo_vcs_info.json").read_text())["git"]["sha1"] != commit:
                raise ValueError("Unexpected WebView2 source revision: " + identifier)
            upstream = f"https://github.com/wravery/webview2-rs/blob/{commit}/LICENSE"
            source = ROOT / "THIRD_PARTY_LICENSES" / "webview2-com-0.38.2"
            files = {source / "LICENSE"}
            expected = {"LICENSE": "0dcf41516e608bbcb6cdc5229feb7b86fe4a643b85e7df251133c93408fdac73"}
        elif not files:
            raise ValueError("No license text found: " + identifier)
        elif checksum_path.is_file():
            checksums = json.loads(checksum_path.read_text(encoding="utf-8"))
            if checksums["package"] != package["checksum"]:
                raise ValueError("Source checksum disagrees with lockfile: " + identifier)
            expected = checksums["files"]
        else:
            # Newer Cargo caches omit .cargo-checksum.json. Validate the cached
            # registry archive against Cargo.lock, then compare each license to
            # its archive member without extracting or trusting archive paths.
            archive_path = args.registry_src.parent / "cache" / source.parent.name / (identifier + ".crate")
            if digest(archive_path.read_bytes()) != package["checksum"]:
                raise ValueError("Cached registry archive disagrees with lockfile: " + identifier)
            expected = {}
            with tarfile.open(archive_path, "r:gz") as archive:
                for path in files:
                    relative = path.relative_to(source).as_posix()
                    member = archive.extractfile(identifier + "/" + relative)
                    if member is None:
                        raise ValueError("License archive member is not a file: " + relative)
                    with member:
                        expected[relative] = digest(member.read())
        records = []
        for path in sorted(files, key=lambda p: p.relative_to(source).as_posix()):
            relative = path.relative_to(source).as_posix()
            data = path.read_bytes()
            checksum = digest(data)
            if expected.get(relative) != checksum:
                raise ValueError(f"License text differs from downloaded registry checksum: {identifier}/{relative}")
            destination = args.out / identifier / relative
            if destination.exists() and destination.read_bytes() != data:
                raise ValueError("Refusing to overwrite a different existing license: " + str(destination))
            if args.check and not destination.is_file():
                raise ValueError("Missing collected license: " + str(destination))
            planned.append((destination, data))
            records.append({"file": relative, "sha256": checksum})
        inventory.append({"name": package["name"], "version": package["version"], "license": metadata.get("license"), "registry_source": package["source"], "registry_checksum": package["checksum"], "files": records})
        if upstream:
            inventory[-1]["license_source"] = upstream
    manifest = {"schema": 1, "scope": "Complete Cargo.lock registry package superset, including optional/build/target-specific crates; not every listed crate is linked into each binary.", "packages": inventory}
    encoded = (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    manifest_path = args.out / "MANIFEST.json"
    if args.check:
        if manifest_path.read_bytes() != encoded:
            raise ValueError("License inventory does not match the current lockfile")
    else:
        for destination, data in planned:
            if not destination.exists():
                destination.parent.mkdir(parents=True, exist_ok=True)
                with destination.open("xb") as output:
                    output.write(data)
        manifest_path.write_bytes(encoded)
    print(f"{'Verified' if args.check else 'Collected'} {len(planned)} unchanged license texts for {len(inventory)} registry crates")

if __name__ == "__main__":
    main()
