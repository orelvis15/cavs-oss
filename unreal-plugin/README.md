# CAVS plugin for Unreal Engine — coming soon

An Unreal plugin that brings CAVS content delivery to PAK / IoStore
(`.ucas` / `.utoc`) containers: ship updates as only the chunks that changed,
verified and reconstructed on the client.

Planned:

- Runtime client (C++) over the CVSP protocol, reusing the Rust core via a
  stable C ABI.
- Build hook to package cooked PAK/IoStore output into `.cavs`.
- Alignment-aware packaging guidance (`cavs analyze steampipe` and
  `cavs analyze-packs` already flag Unreal pack update bloat).

Status: **not available yet.** The Godot plugin (`../godot-plugin`) is the
reference client today; the core engine, server and CLI (including the
SteamPipe-style analysis commands) are already usable.
