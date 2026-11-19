---
name: verify
description: Verify CLO Rust changes locally before commits, pushes, and releases. Covers the pattern-native CLI, MCP over stdio, and published runtime downloads. Handles the project /verify command.
---

# Verify CLO Rust

Adapted from the `dot-pi` project verify template and its meta-level verifier.

## Commit and push gate

Read the diff and name the behavior and candidate being verified. Run the smallest relevant existing tests, then exercise the changed behavior through the CLI or MCP stdio interface. Check one nearby edge case when executable behavior changes. Retain the command, exit status, and relevant output.

### Rust CLI

Run `cargo test --locked` and `cargo build --release --locked` for Rust changes. Use the freshly built `target/release/pattern-native` to run the relevant existing recipes.

| Surface | Command | Evidence |
| --- | --- | --- |
| Carlton import, seam constraints, and Toni drafting | `npm --prefix vendor ci` and `sh verify-linux.sh` | The Carlton project reopens with 66 seams and 1,983 constraints. The script rejects an invalid edge reference and verifies the Toni variants. |
| Connectivity and static collision | `npm --prefix vendor ci` and `sh verify-boat.sh` | The CLI checks connection groups, rejects a mismatch in strict mode, and marks only the intersecting synthetic triangle. |
| Avatar import | `sh verify-avatar.sh` | The CLI imports a synthetic skinned GLB, checks millimetre conversion and collision results, and rejects an invalid mesh selection. |

### MCP stdio and releases

For MCP changes, install `clo_mcp/requirements.txt` in a Python 3.11+ virtual environment. Start `python -m clo_mcp.server` from the repository root using an MCP stdio client. For local runtime checks, set `CLO_RUST_BINARY` to the absolute path of the freshly built native binary. Initialize the session, list tools, and call the changed tool with a representative input and a nearby error case. Keep the structured result.

For release-download, checksum, or cache changes, use an existing release with the required assets and run `CLO_RUST_RELEASE=<tag> python -m clo_mcp.smoke_release` with GitHub CLI access. The smoke test exercises the actual published download, checksum verification, cache hit, seam audit, and replay through stdio. For new releases, build and test on each target platform, run the binary's `--version`, compute its SHA-256 checksum, and confirm the published tag and assets. Run the release-download smoke test against the published tag.

### Verdict

- **PASS** when the required tests pass and the changed behavior works at the caller boundary.
- **FAIL** when a required check fails or the observed behavior disagrees with the claim.
- **BLOCKED** when a required tool or runtime is unavailable. Record the blocker and any checks that did run.
- **SKIP** for runtime checks when the change has no executable behavior. Check paths and links, inspect the final diff, and report any configuration readback separately.

## Bare /verify in chat

If the project verifier is missing, seed it from the `dot-pi` project verify template. If it exists and the chat has prior work, add or update a `Chat verification recipe — YYYY-MM-DD — <slug>` section with claims, acceptance criteria, exact steps, and evidence to keep. A bare `/verify` runs the verifier once only when it is the first invocation in a chat with no prior work. The commit and push gate applies every time.
