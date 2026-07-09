# Releasing

CAVS ships four products from one repository, on **three independent release
trains**. Each train has its own version, its own tag prefix and its own GitHub
Actions workflow, so you can release one product without touching the others.

| Train | Products | Tag | Version source | Workflow |
|---|---|---|---|---|
| **core + SDKs** | Rust crates, `cavs`/`cavs-server`/`cavs-client` binaries, Go/Kotlin/Node SDKs | `vX.Y.Z` | `Cargo.toml` workspace version + `sdks/*` manifests | `.github/workflows/release.yml` |
| **engine plugins** | Godot plugin (Unity/Unreal later) | `plugins-vX.Y.Z` | `game-engine-plugins/godot-plugin/addons/cavs/plugin.cfg` | `.github/workflows/release-plugins.yml` |
| **desktop app** | CAVS Desktop (Tauri) — Windows/macOS/Linux | `desktop-vX.Y.Z` | `desktop/package.json` + `desktop/src-tauri/tauri.conf.json` | `.github/workflows/release-desktop.yml` |

The SDKs are **not** a separate train: they bind the Rust core through a C ABI,
so they share the core's version and ship from the `vX.Y.Z` tag.

## Why prefixes

The three tag patterns are mutually exclusive — `desktop-v*` and `plugins-v*`
start with `d`/`p`, so they never match the core train's `v*` glob. Pushing a
`desktop-v*` tag runs only the desktop workflow.

## Cutting a release

**Core + SDKs** (crates.io, npm, Maven Central, Go module tag, prebuilt CLI
binaries):

```sh
# bump the workspace version + sdks/node/package.json + sdks/kotlin/pom.xml first
git tag v1.3.0 && git push origin v1.3.0
```

**Engine plugins** (Godot plugin zip as a GitHub Release asset):

```sh
# bump version="…" in addons/cavs/plugin.cfg first (the workflow also stamps it)
git tag plugins-v0.1.3 && git push origin plugins-v0.1.3
```

**Desktop app** (native installers for all three platforms):

```sh
# bump desktop/package.json + desktop/src-tauri/tauri.conf.json first
# (the workflow also stamps the version from the tag)
git tag desktop-v1.0.1 && git push origin desktop-v1.0.1
```

### Releasing several at once

Push multiple tags in one go; each fires its own workflow in parallel:

```sh
git push origin v1.3.0 plugins-v0.2.0 desktop-v1.1.0
```

## "Latest" pointer

Only the **core + SDKs** release is marked as the repository's `latest`
release. The plugins and desktop releases are published with `make_latest:
false` so they don't hijack that pointer. The landing page and the README
resolve each product's latest version independently:

- **Landing** — `landing/assets/versions.js` fetches the GitHub Releases API
  and fills in per-product versions, release links and direct download links by
  matching each release's tag prefix and asset filenames.
- **README** — shields.io badges read the latest version live from crates.io,
  npm, Maven Central and (filtered by tag prefix) the GitHub Releases API.

Neither needs a manual edit or a commit when a new version ships.
