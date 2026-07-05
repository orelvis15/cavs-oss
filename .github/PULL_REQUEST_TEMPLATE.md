<!-- Thanks for contributing to CAVS! Please keep PRs focused: one logical change. -->

## What & why

<!-- What does this change do, and why? Link any related issue: "Closes #123". -->

## How it was tested

<!-- Commands you ran, new tests added, or manual verification. -->

## Checklist

- [ ] `cargo fmt --all --check` is clean
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test --all` passes
- [ ] Tests cover the change (regression test for bugs; happy path + main failure modes for features)
- [ ] Docs updated if behavior or interfaces changed (crate README, `docs/`, or doc-comments)
- [ ] If the on-disk format changed: `docs/FORMAT.md` and the pinned test vector in `core/cavs-hash` are updated

<!-- See CONTRIBUTING.md for the full guide. CI must be green to merge. -->
