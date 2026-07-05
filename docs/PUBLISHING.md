# Publishing to crates.io

Maintainer notes for releasing the CAVS crates to [crates.io](https://crates.io).
This is a manual, maintainer-only process — it is **not** run by CI.

## One-time setup

1. Create a crates.io account (sign in with GitHub) and generate an API token
   under **Account Settings → API Tokens**.
2. Authenticate the local toolchain:
   ```sh
   cargo login <your-token>
   ```
3. Crate names are **first-come, first-served and permanent**. All nine crate
   names (`cavs-hash`, `cavs-chunker`, `cavs-store`, `cavs-format`,
   `cavs-proto`, `cavs-cli`, `cavs-server`, `cavs-client`, `cavs-steam`) must be
   available. Publishing `cavs-hash` first also lets you claim the namespace.

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

# Tier 4 — the binaries
cargo publish -p cavs-cli        # → cavs-hash, cavs-chunker, cavs-format, cavs-store
cargo publish -p cavs-server     # → cavs-hash, cavs-format, cavs-proto, cavs-store
cargo publish -p cavs-client     # → cavs-hash, cavs-proto
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
`cavs-format`, `cavs-proto`) directly. Docs build automatically on
[docs.rs](https://docs.rs).

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
