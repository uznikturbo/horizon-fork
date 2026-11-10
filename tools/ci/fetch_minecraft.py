#!/usr/bin/env python3
"""Downloads the pinned Minecraft files, without an account: the version JSON, the client and server jars and
every library the version JSON lists, checked against the SHA-1s research-lock.json pins.

`horizon-prepare compile-exporters --prepare --version-json ... --client-jar --server-jar --libraries-dir`
then imports them and builds `exporters/`, which is what a release folder ships so players need no JDK. A
release runner has no Minecraft install of its own; Mojang serves these files publicly, and nothing of them is
published, only the classes the preparer compiles from `mods/horizon/src/test/java`.

    python3 tools/ci/fetch_minecraft.py --out .local/minecraft-download
"""

import argparse
import hashlib
import json
import sys
import urllib.request
from pathlib import Path

MANIFEST = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
# The version JSON and the jars are tens of megabytes, the libraries a few more: never trust a size.
TIMEOUT = 120
TRIES = 4


def fetch(url: str) -> bytes:
    """`url`'s bytes; GitHub's or Mojang's CDN drops a connection now and then, so a few tries."""
    last = None
    for attempt in range(TRIES):
        try:
            with urllib.request.urlopen(url, timeout=TIMEOUT) as answer:
                return answer.read()
        except OSError as error:  # includes HTTPError and timeouts
            last = error
            print(f"{url}: {error} (try {attempt + 1}/{TRIES})", file=sys.stderr)
    raise SystemExit(f"{url}: {last}")


def checked(url: str, sha1: str, what: str) -> bytes:
    body = fetch(url)
    digest = hashlib.sha1(body).hexdigest()
    if digest != sha1:
        raise SystemExit(f"{what}: {url} is sha1 {digest}, not the pinned {sha1}")
    return body


def download(url: str, sha1: str, path: Path, what: str) -> None:
    if path.is_file() and hashlib.sha1(path.read_bytes()).hexdigest() == sha1:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(checked(url, sha1, what))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--root", type=Path, default=Path("."), help="the repository (research-lock.json)")
    parser.add_argument("--out", type=Path, required=True, help="where the files go")
    args = parser.parse_args()

    lock = json.loads((args.root / "research-lock.json").read_text())["minecraft"]
    version = lock["version"]
    accepted = [lock["manifest_sha1"], *lock.get("accepted_manifest_sha1", [])]

    manifest = json.loads(fetch(MANIFEST))
    url = next(
        (v["url"] for v in manifest["versions"] if v["id"] == version),
        None,
    )
    if url is None:
        raise SystemExit(f"Mojang's version manifest lists no {version}")

    text = fetch(url)
    digest = hashlib.sha1(text).hexdigest()
    if digest not in accepted:
        raise SystemExit(f"{version}'s version JSON is sha1 {digest}, which research-lock.json does not pin")
    version_json = args.out / "version.json"
    version_json.parent.mkdir(parents=True, exist_ok=True)
    version_json.write_bytes(text)

    files = json.loads(text)["downloads"]
    client = args.out / "client.jar"
    server = args.out / "server.jar"
    download(files["client"]["url"], lock["client_sha1"], client, "the client jar")
    download(files["server"]["url"], lock["server_sha1"], server, "the server jar")

    libraries = args.out / "libraries"
    for library in json.loads(text)["libraries"]:
        artifact = library.get("downloads", {}).get("artifact")
        if artifact is None:
            continue
        path = libraries / artifact["path"]
        download(artifact["url"], artifact["sha1"], path, f"library {library['name']}")

    print(
        json.dumps(
            {
                "version_json": str(version_json.resolve()),
                "client_jar": str(client.resolve()),
                "server_jar": str(server.resolve()),
                "libraries_dir": str(libraries.resolve()),
            }
        )
    )


if __name__ == "__main__":
    main()
