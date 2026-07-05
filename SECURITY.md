# Security Policy

## Supported versions

CAVS is pre-1.0. Security fixes are applied to the latest release and `main`.

| Version | Supported |
|---|---|
| latest release / `main` | ✅ |
| older | ❌ |

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Report privately using GitHub's private vulnerability reporting:
<https://github.com/orelvis15/cavs-oss/security/advisories/new>

Or email **orelvis15@gmail.com** with:

- a description of the issue and its impact,
- steps to reproduce (a minimal proof of concept if possible),
- affected version or commit and your environment.

You can expect an acknowledgement within a few days. Once a fix is ready we
will coordinate a disclosure timeline with you and credit you in the release
notes unless you prefer to remain anonymous.

## Scope

CAVS parses untrusted `.cavs` files and serves content over HTTP/HTTPS. Of
particular interest: the format reader (memory safety on malformed input),
chunk/manifest verification (BLAKE3, Merkle root, Ed25519 signatures), and the
server's session/handling paths. The reader is designed to reject malformed or
adversarial input rather than panic or exhaust memory; reports of ways to
defeat that are especially welcome.
