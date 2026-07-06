# Publishing to crates.io

Maintainer notes for releasing the CAVS crates to [crates.io](https://crates.io).

## Automated release (normal path)

Publishing is automated. Pushing a version tag runs the
[release workflow](../.github/workflows/release.yml), which builds the binaries
and then publishes every crate to crates.io **in dependency order**:

```sh
# bump the version in Cargo.toml + CHANGELOG.md first, then:
git tag v0.1.1
git push origin v0.1.1
```

This requires a repository secret named **`CRATES_IO_TOKEN`** (Settings →
Secrets and variables → Actions) holding a crates.io API token with publish
scope. The publish step is idempotent — a version already on crates.io is
skipped, so the workflow can be safely re-run.

The rest of this document describes the manual process and the mechanics behind
it, useful for the very first publish (before the secret exists) or for
debugging a failed release.

## One-time setup

1. Create a crates.io account (sign in with GitHub) and generate an API token
   under **Account Settings → API Tokens**.
2. Authenticate the local toolchain:
   ```sh
   cargo login <your-token>
   ```
3. Crate names are **first-come, first-served and permanent**. All ten crate
   names (`cavs-hash`, `cavs-chunker`, `cavs-store`, `cavs-format`,
   `cavs-proto`, `cavs-manifest`, `cavs-cli`, `cavs-server`, `cavs-client`,
   `cavs-steam`) must be available. Publishing `cavs-hash` first also lets you
   claim the namespace.

## How versioning works here

Internal dependencies are declared in the root `Cargo.toml` with **both** a
`path` and a `version`:

```toml
cavs-hash = { path = "core/cavs-hash", version = "0.1.0" }
```

- Local builds (`cargo build`, `cargo test`) resolve through `path`.
- `cargo publish` strips the `path` and records the `version` requirement, so
  each published crate depends on its siblings by version from crates.io.

Because of this, crates must be published **in dependency order** — a crate
can only be verified once every crate it depends on is already live on
crates.io. All crates share `version = "0.1.0"` via `[workspace.package]`.

## Dependency-ordered publish sequence

Publish top to bottom. Wait for each crate to appear in the index (usually a few
seconds) before publishing the next; `cargo publish` polls the index and will
proceed automatically once the dependency is available.

```sh
# Tier 1 — no internal dependencies
cargo publish -p cavs-hash
cargo publish -p cavs-chunker

# Tier 2 — depend on cavs-hash
cargo publish -p cavs-store      # → cavs-hash
cargo publish -p cavs-proto      # → cavs-hash

# Tier 3 — depend on tier 1/2
cargo publish -p cavs-format     # → cavs-hash, cavs-store

# Tier 4 — depends on tier 1/2/3
cargo publish -p cavs-manifest   # → cavs-hash, cavs-proto, cavs-format

# Tier 5 — the binaries
cargo publish -p cavs-cli        # → cavs-hash, cavs-chunker, cavs-format, cavs-manifest, cavs-proto, cavs-store
cargo publish -p cavs-server     # → cavs-hash, cavs-format, cavs-manifest, cavs-proto, cavs-store
cargo publish -p cavs-client     # → cavs-hash, cavs-manifest, cavs-proto
cargo publish -p cavs-steam      # → cavs-hash, cavs-chunker
```

After this, users can:

```sh
cargo install cavs-cli      # the `cavs` CLI
cargo install cavs-server
cargo install cavs-client
cargo install cavs-steam    # the `cavs-steam` analyzer
```

and depend on the libraries (`cavs-hash`, `cavs-chunker`, `cavs-store`,
`cavs-format`, `cavs-proto`, `cavs-manifest`) directly. Docs build
automatically on [docs.rs](https://docs.rs).

## Validate before publishing

- Dry-run the leaf crate (no registry dependencies), which fully packages and
  verify-builds it:
  ```sh
  cargo publish --dry-run -p cavs-hash
  ```
  Dry-run on a crate with internal dependencies will fail until those
  dependencies are actually on crates.io — that is expected; rely on the
  ordered sequence above instead.
- Inspect exactly what ships in a crate (respects `include`/`exclude`):
  ```sh
  cargo package --list -p cavs-cli
  ```
- Confirm the whole workspace is green first:
  ```sh
  cargo fmt --all --check
  cargo clippy --all-targets -- -D warnings
  cargo test --all
  ```

## Cutting a new version

1. Bump `version` in `[workspace.package]` (root `Cargo.toml`) and the pinned
   `version` on each internal dependency to match.
2. Update [`CHANGELOG.md`](../CHANGELOG.md).
3. Re-run the checks and re-publish in the same dependency order.
4. Published versions are **immutable** — you can `cargo yank` a bad release to
   stop new dependents from selecting it, but you cannot overwrite or delete it.

> Publishing to crates.io is independent of GitHub Releases. Binaries for
> Linux/macOS/Windows are produced by the tag-triggered
> [release workflow](../.github/workflows/release.yml) when you push a `v*` tag.

## Godot Asset Library

The Godot plugin (`godot-plugin/addons/cavs`) is distributed on the
[Godot Asset Library](https://godotengine.org/asset-library) separately from the
crates. The Asset Library installs the archive of a single commit and expects
`addons/` at its root, so a CI step splits `godot-plugin/` onto a dedicated
`godot-asset` branch (root = `addons/cavs/...`) and points the library at it.

### First submission (manual, one time)

The Asset Library has no API to *create* an asset, so the first version is
submitted through the web form to obtain a numeric asset id:

1. Generate/refresh the distribution branch locally and push it:
   ```sh
   git subtree split --prefix=godot-plugin -b godot-asset
   git push -f origin godot-asset
   git rev-parse godot-asset   # copy this commit hash
   ```
2. Sign in at godotengine.org and open **Asset Library → Submit Asset**, then:
   - **Category**: Tools · **Godot version**: 4.2 (or your minimum)
   - **Repository**: `https://github.com/orelvis15/cavs-oss`
   - **Issues URL**: `https://github.com/orelvis15/cavs-oss/issues`
   - **Commit**: the hash from step 1
   - **Icon URL**:
     `https://raw.githubusercontent.com/orelvis15/cavs-oss/main/godot-plugin/addons/cavs/icon.png`
   - **License**: Apache-2.0 · fill in the name/description.
3. Wait for moderator approval (up to a few days). Note the asset id in the URL
   (`.../asset-library/asset/<ID>`).

### Automatic updates (every release)

Once the asset exists, configure the repository (Settings → Secrets and
variables → Actions):

- Variable `GODOT_ASSET_ID` = the numeric asset id
- Variable `GODOT_ASSET_LIB_USERNAME` = your godotengine.org username
- Secret `GODOT_ASSET_LIB_PASSWORD` = your godotengine.org password

> The API logs in with username + password. If your godotengine.org account was
> created via GitHub sign-in, set an account password first so API login works.

After that, each published GitHub Release triggers
[`godot-asset.yml`](../.github/workflows/godot-asset.yml), which regenerates the
`godot-asset` branch and submits an edit with the new version and commit. Like
all Asset Library changes, the edit is queued for manual moderation.
