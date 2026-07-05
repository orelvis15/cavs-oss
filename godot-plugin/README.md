# CAVS plugin for Godot 4

Deduplicated content delivery for Godot games, **without changing the engine
or the PCK format**: the player downloads only the chunks that changed between
versions, the pack is reconstructed byte-identically (verified with SHA-256)
and mounted with `ProjectSettings.load_resource_pack()`.

Measured with real PCKs exported by Godot 4.7: updates of **−67% to −70%**
versus downloading the full compressed PCK, with near-free re-downloads. See
[`../docs/BENCHMARKS.md`](../docs/BENCHMARKS.md).

## Architecture

```
 build / CI                     server                  player (runtime)
+------------------+   +----------------------+   +----------------------------+
| godot --export-  |   | cavs-server          |   | CavsClient (pure GDScript) |
|  pack -> game.pck| ->|  releases/*.cavs     | ->|  cache in user://cavs_cache|
| cavs pack --raw  |   |  sessions + have-set |   |  only new chunks           |
|  -> game_vN.cavs |   |  immutable chunks    |   |  sha256 + load_resource_   |
+------------------+   +----------------------+   |  pack()                    |
                                                  +----------------------------+
```

- **100% GDScript client** (`addons/cavs/cavs_client.gd`, `class_name
  CavsClient`): parses the binary CVSP protocol, decompresses the wire with
  Godot's native zstd, keeps a content-addressable cache in `user://`, and
  verifies every reconstructed file's SHA-256 with `HashingContext`. No
  GDExtension, no native binaries: works on every platform Godot exports to.
- **Build side**: `tools/pack_release.sh` exports the PCK headless and packages
  it (FastCDC 64 KiB + zstd + optional Ed25519 signature).

## Install

1. Copy `addons/cavs/` into your project and enable the plugin in
   *Project -> Project Settings -> Plugins* (the runtime also works with a
   plain `preload` even if the plugin is not enabled).
2. Publish releases from CI:

```sh
./tools/pack_release.sh ~/my-game pck game_v42 keys/publisher.key
cavs-server releases/*.cavs --listen 0.0.0.0:8990 --tls-cert cert.pem --tls-key key.pem
```

## Runtime usage

```gdscript
# On your loading screen (see demo/boot.gd for the full version with progress):
var cavs := CavsClient.new("https://content.mygame.com")
cavs.progress.connect(func(done, total, stage):
    progress_bar.value = done * 100.0 / total, CONNECT_DEFERRED)
cavs.fetch_async("game_v42", func(result):
    if result.ok and cavs.ensure_pack("game_v42"):
        get_tree().change_scene_to_file("res://levels/level_new.tscn"))
```

`CavsClient` API:

| Member | Description |
|---|---|
| `new(url)` | Client against a cavs-server (`http://` or `https://`) |
| `fetch(asset) -> Dictionary` | Blocking (for threads/tests); returns `{ok, error, files, bytes_wire, chunks_inline, refs}` |
| `fetch_async(asset, on_done)` | Internal thread; delivers the result on the main thread |
| `ensure_pack(asset) -> bool` | `fetch()` + `load_resource_pack()` of the `.pck` |
| signal `progress(done, total, stage)` | Logical bytes for the progress bar; connect with `CONNECT_DEFERRED` |
| `max_retries` / `retry_base_ms` | Retries with exponential backoff on network errors and 5xx |
| `request_timeout_ms` | Per-request timeout (default 30 s) |
| `cache_dir` | Persistent cache (default `user://cavs_cache`) |
| `ca_cert_path` | Trusted PEM for self-signed dev TLS |
| `require_sha256` | Warn if the manifest carries no per-file digests |

Retries are safe by design: downloaded chunks stay in the content-addressable
cache, so a retried `fetch()` (even after the game was killed mid-download)
only fetches what is missing.

## Production notes

- **Integrity**: the server verifies BLAKE3 + Merkle + Ed25519 on load; the
  Godot client verifies the per-file SHA-256 the packer embeds in the manifest.
  A corrupt chunk or tampered pack never reaches `load_resource_pack()`.
- **Cache**: chunks are immutable by content address; the cache can be shared
  across versions and assets (that is where the savings come from). Deleting
  `user://cavs_cache` is always safe.
- **Early mounting**: mount packs as early as possible (ideally in the boot
  autoload/scene) - Godot recommends loading resource packs before scenes that
  depend on them are preloaded.
