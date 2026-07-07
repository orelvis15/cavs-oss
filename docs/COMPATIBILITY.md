# Compatibility policy (v1.x)

v1.0.0 freezes the documented CLI surface and file formats. This is
what "stable" means for the 1.x line.

## Stable file formats

| Format | Magic / schema | Documented in |
|---|---|---|
| `.cavs` container | `CAVS` | [FORMAT.md](FORMAT.md) |
| `.cavsmf2` compact manifest | `CAVSMF2` | [FORMAT.md](FORMAT.md) |
| `.cavssig` signature | `CAVSSIG1` | [SIGNATURE_FORMAT.md](SIGNATURE_FORMAT.md) |
| `.cavsplan` offline plan | `CAVSPLAN1` | [CAVSPLAN_FORMAT.md](CAVSPLAN_FORMAT.md) |
| `.cavspatch` sidecar | `CAVSPCH1`/v2 | [PAIRWISE_SIDECARS.md](PAIRWISE_SIDECARS.md) |
| certification summary JSON | `cavs-certify-summary/1` | [CERTIFICATION.md](CERTIFICATION.md) |
| route result JSON | `cavs-certify-routes/1` | [ROUTE_SELECTION.md](ROUTE_SELECTION.md) |
| regression baseline JSON | `cavs-certify-baseline/1` | [CERTIFICATION.md](CERTIFICATION.md) |

## Stable CLI families

```text
cavs pack             cavs pack-dir         cavs signature
cavs preview          cavs diff-plan        cavs apply
cavs verify-install   cavs bench            cavs analyze
cavs publish-preview  cavs plan-update      cavs install-plan
cavs workspace / depot / branch / build     cavs serve
cavs certify
```

Plus the Godot plugin runtime API: `CavsClient.fetch`,
`CavsClient.fetch_async`, `CavsClient.ensure_pack`
([GODOT_PLUGIN.md](GODOT_PLUGIN.md)).

## The rules

1. **No breaking changes to documented commands without deprecation.**
   A documented flag or subcommand keeps working through v1.x; removal
   requires a deprecation notice in at least one minor release first.
2. **Readers reject unsupported newer versions clearly.** Every format
   carries a version; a v1.x reader confronted with a newer version
   fails with a stable `CAVS-E-*` error code, never a silent
   misparse.
3. **Unknown sections are skipped where safe.** Formats with optional
   sections ignore unknown ones when integrity allows it, so additive
   format evolution does not break old readers.
4. **JSON schema changes are additive when possible.** The `schema`
   field only bumps (`…/2`) on a breaking change; new fields may appear
   at any time and consumers must tolerate them.
5. **Exit codes are frozen.** `cavs certify` exit codes 0–5 keep their
   documented meaning through v1.x ([CERTIFICATION.md](CERTIFICATION.md)).
6. **Error codes are stable.** `CAVS-E-*` identifiers may gain new
   members but existing ones keep their meaning.

## What is *not* covered

- Unstable/benchmark-only surfaces: `cavs bench` dataset shapes and
  report prose, `cavs test`, internal crate APIs (the crates are
  published for the CLI's benefit; semver applies to the CLI contract,
  not to `pub` items in `cavs-*` crates).
- Human-readable Markdown report wording — treat `*.json` as the
  machine contract.
- The local dev server's HTTP surface (`cavs serve` is explicitly
  development-only).
