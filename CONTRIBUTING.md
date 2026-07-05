# Contributing to CAVS

Thanks for your interest in improving CAVS! This guide explains how to set up,
make a change, and open a pull request.

## Ground rules

- Be respectful and constructive — this project follows a
  [Code of Conduct](CODE_OF_CONDUCT.md).
- By contributing, you agree that your contributions are licensed under the
  project's [Apache License 2.0](LICENSE).
- Keep pull requests focused: one logical change per PR is much easier to review
  and merge than a large mixed one.

## Getting set up

You need a stable Rust toolchain ([rustup](https://rustup.rs)). Then:

```sh
git clone https://github.com/<your-fork>/cavs
cd cavs
cargo build            # build all crates and tools
cargo test             # run the test suite
```

`ffmpeg` on `PATH` is only needed for the optional video path; Godot 4 only for
the Godot plugin. See the [README](README.md) for the full build/usage guide.

## Development workflow

1. **Fork** the repository and create a topic branch from `main`:
   ```sh
   git checkout -b fix/short-description
   ```
2. **Make your change.** Keep the diff minimal and match the surrounding style.
3. **Run the checks locally** — CI enforces all three, so run them before pushing:
   ```sh
   cargo fmt --all              # format
   cargo clippy --all-targets -- -D warnings   # lint (no warnings allowed)
   cargo test --all             # tests
   ```
4. **Add or update tests** for behavior changes. Bug fixes should come with a
   regression test; new features with coverage of the happy path and the main
   failure modes.
5. **Commit** with a clear message. A short imperative summary line, then a body
   explaining the *why* if it isn't obvious:
   ```
   server: reject sessions for unknown assets

   Returning 404 instead of 500 lets clients distinguish a typo from an
   outage. Adds a regression test.
   ```
6. **Push** to your fork and **open a pull request** against `main`.

## Pull request checklist

Before requesting review, confirm:

- [ ] `cargo fmt --all --check` is clean
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test --all` passes
- [ ] Tests cover the change
- [ ] Documentation is updated (crate README, `docs/`, or code doc-comments) if
      behavior or interfaces changed
- [ ] The PR description explains what changed and why

The CI workflow runs format, clippy and tests on every PR. A PR needs green CI
and a maintainer review to be merged.

## Where things live

- `core/` — the engine crates and the `cavs` / `cavs-server` / `cavs-client`
  tools. Each crate has its own README describing its role.
- `steam-analyzer/` — the SteamPipe analyzer (`cavs-steam`).
- `godot-plugin/` — the Godot 4 runtime client.
- `docs/` — format spec, architecture, benchmarks, and the paper. If you change
  the on-disk format, update [`docs/FORMAT.md`](docs/FORMAT.md) **and** the
  pinned interoperability test vector in `core/cavs-hash`.

## Reporting bugs and proposing features

Open an issue with:

- **Bugs**: what you did, what you expected, what happened, and the versions
  involved (OS, `rustc --version`, the CAVS commit). A minimal reproduction is
  ideal.
- **Features**: the problem you're trying to solve and, if you have one, a
  sketch of the approach. For anything large, please open an issue to discuss
  before writing a lot of code.

## Releases

Releases are cut by the maintainer, not by merging. When a set of changes is
ready, the maintainer pushes a version tag (e.g. `v0.2.0`); the
[release workflow](.github/workflows/release.yml) then builds versioned binaries
for Linux, macOS and Windows and publishes them as a GitHub Release. You don't
need to bump versions in your PR unless asked.
