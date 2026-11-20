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

### Verify the 48 public CLO MCP tools

The public acceptance list is [`clo_mcp/clo_tool_contract.json`](../../../clo_mcp/clo_tool_contract.json).
Verify it through MCP stdio with matching requests sent to `CLO_MCP_BACKEND=official`
and `CLO_MCP_BACKEND=rust`. A tool is behaviorally matched only when its relevant
inputs, results, state changes, and error cases pass the dual-backend comparison.

From the repository root, use a Python 3.11+ environment with `clo_mcp/requirements.txt`
installed and build the Rust release binary with `cargo build --release --locked`.
Run the synthetic fixture smoke test with the asset overrides empty:

```sh
CLO_RUST_BINARY="$PWD/target/release/pattern-native" \
  CLO_RUST_AVATAR_DIR='' CLO_RUST_FABRIC_DIR='' \
  python -m clo_mcp.smoke_compat
```

Set `CLO_MCP_OFFICIAL_COMMAND` to the real CLO MCP stdio command as a JSON string array
to include the official backend smoke. Run it with the CLO 3D bridge connected.
`tools/list` is checked against the contract, including input and output schemas.
The Rust fixtures also exercise selected scene, image, export, failure, and session flows.

The first repeatable live comparison checks the full fabric catalog, a mixed-case filter,
an empty result, and an invalid filter type. Use the same CLO asset library on both
backends. Use the exact `folder` spelling returned by the official backend.

```sh
CLO_MCP_OFFICIAL_COMMAND='["/path/to/clo3d-mcp/.venv/bin/python","-m","clo3d_mcp.server"]' \
  CLO_RUST_BINARY="$PWD/target/release/pattern-native" \
  CLO_RUST_FABRIC_DIR='/path/to/CLO Assets/Fabric' \
  python -m clo_mcp.smoke_compat --compare-fabric-library
```

Use `--compare-avatar-library` with `CLO_RUST_AVATAR_DIR` set to the matching
`Avatar` directory, or set `CLO_MCP_ASSETS` to its parent. Compare the library
paths and aliases across backends, and verify each session's `in_scene` against
its own `scene_info` since this read-only case does not change the active project.

Use `--compare-saved-fabric-list` with the same stdio settings and a saved CLO
project available on disk. Open the project in CLO with no unsaved material edits.
The case reads the official `fabric_list`, opens the saved project in Rust,
sets the same current fabric in the disposable Rust session, and compares names,
indices, selection readback and each public catalog field reported by CLO.

Use `--compare-saved-pattern-settings` with a saved project and no unsaved
pattern edits in CLO. The case compares names, indices, materials and arrangement
names for every panel, checks detailed arrangement, layer and particle distance
for up to eight panels, and exercises a missing panel. The reported seam-group
counts are separate observations. The case does not assert line geometry or sewing parity.

Use `open_backend()`, `inspect_contract()`, and `json_result()` from
`clo_mcp.smoke_compat` for additional cases. Keep the same source fixture on both
backends. Use disposable projects on the CLO 3D side for state-changing cases.
Read state before the call and after the call, including after a rejected operation.
For saved or exported artifacts, independently reopen and inspect geometry, units,
material assignments, image content, or whichever properties the claim depends on.

| Tools | Cases to compare |
| --- | --- |
| `clo_status`, `clo_launch`, `clo_stop_bridge` | Readiness, repeat launch, stopped-tool error, restart and scene readback |
| `clo_api_search`, `clo_api_doc`, `clo_api_catalog`, `clo_api_describe`, `clo_api_call`, `clo_api_release`, `clo_run_python` | Discover callable functions used by CLO MCP workflows, inspect types, invoke read and write cases, release handles and query after Python errors or completed effects |
| `scene_info`, `scene_clear`, `scene_open`, `scene_save`, `scene_export`, `checkpoint_save`, `checkpoint_restore`, `checkpoint_list` | Replace and append, keep-avatar behavior, missing or corrupt input, output format and reopen, checkpoint ordering and restore |
| `snapshot`, `snapshot_2d`, `clo_control_guide`, `clo_ui_state`, `clo_ui_action`, `clo_ui_screenshot` | View and selection inputs, content blocks and image geometry, supported actions, readback and error cases |
| `avatar_list`, `avatar_load`, `avatar_measurements`, `arrangement_points` | Library and alias resolution, append and replace, named arrangement points, measurements and units, missing inputs |
| `pattern_list`, `pattern_info`, `pattern_create`, `pattern_delete`, `pattern_set_arrangement`, `pattern_edit_points`, `pattern_ops`, `sew`, `seams_list` | Identifiers and line indices, editable geometry, arrangement, sewing direction and group membership, rejected edits |
| `fabric_library`, `fabric_list`, `fabric_apply`, `fabric_set_color`, `fabric_set_texture` | Filter, aliases, filenames and paths, selected-piece assignment, list and export readback, invalid paths and indices |
| `simulate`, `simulation_reset` | Steps, quality and particle distance, saved pose continuation and reset, seams and contacts, invalid arguments |
| `garment_plan_preview`, `garment_build`, `garment_rebuild`, `garment_last_spec` | Draft parameters, pieces and seams, build/rebuild state and artifacts, validation and previous spec |

Record the tool, case, Git SHA, CLO version, fixture identity, request, raw results
and errors, state before and after, artifact checks, and verdict. Document which
fields may differ between runtimes, such as timings and temp paths, and define any
numeric tolerances before evaluating results. Keep full evidence in the local Board
and summarize confirmed results in the progress Page. A Rust-only result or matching
registration stays separate from behavioral PASS. Mark an unavailable comparison
BLOCKED and an observed mismatch FAIL. Claim full compatibility after all 48
public tools have passing behavioral evidence. The validation scope is the CLO MCP
workflows and tool contracts.

### Verdict

- **PASS** when the required tests pass and the changed behavior works at the caller boundary.
- **FAIL** when a required check fails or the observed behavior disagrees with the claim.
- **BLOCKED** when a required tool or runtime is unavailable. Record the blocker and any checks that did run.
- **SKIP** for runtime checks when the change has no executable behavior. Check paths and links, inspect the final diff, and report any configuration readback separately.

## Bare /verify in chat

If the project verifier is missing, seed it from the `dot-pi` project verify template. If it exists and the chat has prior work, add or update a `Chat verification recipe — YYYY-MM-DD — <slug>` section with claims, acceptance criteria, exact steps, and evidence to keep. A bare `/verify` runs the verifier once only when it is the first invocation in a chat with no prior work. The commit and push gate applies every time.
