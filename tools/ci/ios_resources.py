#!/usr/bin/env python3
"""Prepare inputs for the manually run iOS resources workflow, not the game itself.

Downloads go into the runner's temporary directory, never into Git. All game bytes
are verified against research-lock.json or metadata covered by its manifest hash.
The actual exports are produced by this checkout's horizon-prepare executable.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def sha1(path):
    digest = hashlib.sha1()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def child(root, relative):
    path = PurePosixPath(relative)
    if not path.parts or path.is_absolute() or ".." in path.parts or "\\" in relative:
        raise ValueError(f"Unsafe download path: {relative!r}")
    return root.joinpath(*path.parts)


def download(url, path, checksum, size=None):
    if not url.startswith("https://") or not re.fullmatch(r"[0-9a-f]{40}", checksum):
        raise ValueError(f"Invalid download metadata for {path.name}")
    if path.is_file() and sha1(path) == checksum and (size is None or path.stat().st_size == size):
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    partial = path.with_name(path.name + ".part")
    for attempt in range(3):
        try:
            request = urllib.request.Request(url, headers={"User-Agent": "Horizon-iOS-resources"})
            with urllib.request.urlopen(request, timeout=90) as response, partial.open("wb") as target:
                for chunk in iter(lambda: response.read(1024 * 1024), b""):
                    target.write(chunk)
            if sha1(partial) != checksum or (size is not None and partial.stat().st_size != size):
                raise ValueError(f"Checksum or size mismatch for {path.name}")
            partial.replace(path)
            return
        except (OSError, ValueError) as error:
            partial.unlink(missing_ok=True)
            if attempt == 2:
                raise RuntimeError(f"Could not download {url}: {error}") from error
            time.sleep(attempt + 1)


def download_inputs(folder, game):
    version = game["version"]
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", version) or version in (".", ".."):
        raise ValueError("Unsafe Minecraft version in research-lock.json")
    version_json = child(folder, f"versions/{version}/{version}.json")
    manifest_hash = game["manifest_sha1"]
    # Fetch the exact accepted JSON, not the latest entry for this version, which
    # Mojang may have republished since this checkout's lock was written.
    download(
        f"https://piston-meta.mojang.com/v1/packages/{manifest_hash}/{version}.json",
        version_json, manifest_hash,
    )
    manifest = read_json(version_json)
    if manifest["id"] != version or manifest["javaVersion"]["majorVersion"] != game["java_major"]:
        raise ValueError("Pinned version JSON disagrees with research-lock.json")
    jobs = {}

    def add(artifact, path, expected=None):
        checksum = artifact["sha1"]
        if expected is not None and checksum != expected:
            raise ValueError(f"Pinned jar mismatch: {path.name}")
        job = (artifact["url"], path, checksum, artifact.get("size"))
        if path in jobs and jobs[path] != job:
            raise ValueError(f"Conflicting download entries: {path}")
        jobs[path] = job

    add(manifest["downloads"]["client"], version_json.with_suffix(".jar"), game["client_sha1"])
    add(manifest["downloads"]["server"], folder / "server.jar", game["server_sha1"])
    # Import chooses the current platform's libraries. Downloading the full
    # artifact list avoids reimplementing the launcher's OS/architecture rules.
    for library in manifest["libraries"]:
        artifact = library.get("downloads", {}).get("artifact")
        if artifact is not None:
            add(artifact, child(folder / "libraries", artifact["path"]))
    index_meta = manifest["assetIndex"]
    index_file = child(folder / "assets/indexes", index_meta["id"] + ".json")
    download(index_meta["url"], index_file, index_meta["sha1"], index_meta.get("size"))
    for entry in read_json(index_file)["objects"].values():
        checksum = entry["hash"]
        if not re.fullmatch(r"[0-9a-f]{40}", checksum):
            raise ValueError("Invalid asset hash")
        relative = f"{checksum[:2]}/{checksum}"
        add(
            {"url": f"https://resources.download.minecraft.net/{relative}",
             "sha1": checksum, "size": entry["size"]},
            child(folder / "assets/objects", relative),
        )
    print(f"Downloading {len(jobs)} checked files for Minecraft {version}", flush=True)
    with ThreadPoolExecutor(max_workers=12) as pool:
        for completed, _ in enumerate(pool.map(lambda job: download(*job), jobs.values()), 1):
            if completed % 500 == 0 or completed == len(jobs):
                print(f"Verified {completed}/{len(jobs)} files", flush=True)


def verify_resources(folder, game):
    exported = read_json(folder / "block-behaviours.json")
    fluids = exported.get("fluids")
    definitions = exported.get("fluid_state_definitions")
    if not isinstance(fluids, list) or not fluids or not isinstance(definitions, dict):
        raise ValueError("Missing fluid_state_definitions: the resource export is incomplete")
    if len(definitions) != len(fluids) or set(definitions) != set(fluids):
        raise ValueError("fluid_state_definitions does not cover the exported fluid registry")
    for name, entry in definitions.items():
        if (not isinstance(entry, dict) or not isinstance(entry.get("properties"), dict)
                or not isinstance(entry.get("default"), dict) or entry["default"].get("id") != name):
            raise ValueError(f"Invalid fluid definition: {name}")
    commit = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout.strip()
    (folder.parent / "BUILD-INFO.txt").write_text(
        f"Horizon source commit: {commit}\nMinecraft: {game['version']}\n"
        f"Java major: {game['java_major']}\nFluid definitions verified: {len(definitions)}\n"
        "Use with an IPA built from the same checkout.\n"
        "Move the resources folder into Horizon's Documents folder; keep saves and options.\n",
        encoding="utf-8",
    )
    print(f"Verified {len(definitions)} fluid definitions; source commit {commit}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("versions", "download", "verify"))
    parser.add_argument("folder", type=Path, nargs="?")
    args = parser.parse_args()
    game = read_json(ROOT / "research-lock.json")["minecraft"]
    if args.command == "versions":
        versions = {"minecraft": game["version"], "java": game["java_major"],
                    "rust": (ROOT / "tools/ci/rust-version").read_text().strip()}
        print(json.dumps(versions), flush=True)
        if output := os.environ.get("GITHUB_OUTPUT"):
            with open(output, "a", encoding="utf-8") as target:
                for key, value in versions.items():
                    target.write(f"{key}={value}\n")
        return
    if args.folder is None:
        parser.error(f"{args.command} requires a destination folder")
    if args.command == "download":
        download_inputs(args.folder, game)
    else:
        verify_resources(args.folder, game)


if __name__ == "__main__":
    main()
