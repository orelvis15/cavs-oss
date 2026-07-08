# CAVS plugin for Unity — coming soon

A Unity package that brings CAVS content delivery to AssetBundles and
Addressables: download only the chunks that changed between builds, verify
them, and mount the reconstructed bundles at runtime.

Planned:

- C# `CavsClient` over the CVSP protocol, backed by the Rust core via a
  stable C ABI (native plugin).
- Editor post-process to package AssetBundle / Addressables output into `.cavs`.
- Persistent runtime cache with byte-identical, verified reconstruction.

Status: **not available yet.** The Godot plugin (`../godot-plugin`) is the
reference client today; the core engine, server, CLI and SteamPipe analyzer
are already usable.
