# This fork's releases

This copy of Horizon is built for `uznikturbo/horizon-fork`, and its launcher installs that repository's
builds rather than upstream `horizon-rust/horizon` (`launcher/src/backend.rs`, `REPO`). Nothing else about
the release model changes: an archive is installed only when its `SHA256SUMS` is signed by a key in
`crates/horizon-release-sign/release-keys.txt` (`docs/security.md`).

## What the workflow publishes

`.github/workflows/release.yml` builds and publishes, for Linux x86_64 only:

| Release | Tag | Asset |
|---|---|---|
| the game | `v<date>`, or `v<date>.<run>` for a second build that day | `horizon-linux-x86_64.tar.gz` |
| the launcher | `launcher@0.2.<run>` | `horizon-launcher-linux-x86_64.tar.gz` |

Both carry `SHA256SUMS` and `SHA256SUMS.sig`, signed by `tools/ci/sign_release.sh` with the
`RELEASE_SIGNING_KEY` secret. The launcher release is never GitHub's latest, so the game's stays it.

The launcher's installed version is the release tag, so every run's number moves launcher self-updates
forward without a version-bump commit (`HORIZON_LAUNCHER_TAG`, `HORIZON_LAUNCHER_VERSION`).

## Running it

A push builds nothing unless its commit message contains `[release]`; the Actions tab's **Run workflow**
(for `release.yml`) always builds one.

```sh
git commit --allow-empty -m "release [release]"
git push origin main
# or: gh workflow run release --repo uznikturbo/horizon-fork
```

The game job is a release build of the whole workspace (hours on a hosted runner). It also compiles the
Java exporters, because a release folder ships them so a player's machine prepares resources with a plain
runtime and no JDK. A hosted runner has no Minecraft install, so `tools/ci/fetch_minecraft.py` downloads the
pinned version JSON, jars and libraries from Mojang's public servers, checks them against
`research-lock.json`, and the preparer imports them; only the classes under
`exporters/*/dev/horizonlab/parity/` and their manifest leave the job.

## The signing key

The key pair was generated once with:

```sh
cargo run --quiet -p horizon-release-sign -- keygen --public-out release.pub > release.key
```

`release.pub` is the second key in `crates/horizon-release-sign/release-keys.txt`; `release.key` (base64
PKCS#8) is the repository's secret:

```sh
gh secret set RELEASE_SIGNING_KEY --repo uznikturbo/horizon-fork < release.key
```

Losing the private key means new builds can't be installed by an existing launcher until another key is
added to `release-keys.txt` and a launcher carrying it is installed by hand.
