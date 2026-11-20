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

| Check | Passing evidence |
| --- | --- |
| Public tool contract | `tools/list` entries match the pinned contract, including names, descriptions, and input/output schemas, on each backend. |
| Rust call smoke | One successful representative call per public tool in an isolated synthetic Rust scene. The JSON report shows each tool and its MCP content types. |
| Dual-backend behavior | The same relevant cases pass through both backends, with assertions for returned values, observable scene changes, errors, and produced files as applicable. |

From the repository root, use a Python 3.11+ environment with `clo_mcp/requirements.txt`
installed. Build the binary and save the all-tool call report and the detailed smoke log:

```sh
cargo build --release --locked
export CLO_RUST_BINARY="$PWD/target/release/pattern-native"
python -m clo_mcp.sweep_compat > rust-tool-calls.json 2> rust-tool-calls.log
CLO_RUST_AVATAR_DIR='' CLO_RUST_FABRIC_DIR='' python -m clo_mcp.smoke_compat
```

The sweep exits nonzero for missing, unexpected, or failed calls. Its success criterion
is `expected_count = called_count = ok_count = 48` with empty `missing`,
`unexpected`, and `errors` arrays. Use `tools.<name>` to inspect each result.

Set `CLO_MCP_OFFICIAL_COMMAND` to the real CLO MCP stdio command as a JSON string array
to include the official backend smoke. Run it with the CLO 3D bridge connected.
`tools/list` is checked against the contract, including input and output schemas.
The Rust fixtures also exercise selected scene, image, export, failure, and session flows.

For behavior verification, use the comparison recipes below and record one verdict
per tool after reviewing all applicable cases. Use a disposable CLO scene for tests
that change state, preserve the exact requests and both results, and reopen exported
files or read scene state back when the tool promises a side effect. A review begins
with an unknown behavioral tally until evidence is collected for that revision.

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

Use `--compare-saved-pattern-outline` with a saved project containing straight
and curved outer edges. Compare all panel corner records, line indices,
straight/curve type, endpoints, orientation and location, and check detail rows
from `pattern_list`. Accept at most 0.1 mm length difference, 0.001 mm midpoint
or corner-bounds axis difference, and 0.001 normalized midpoint axis difference.
Save and reopen a disposable Rust copy and confirm matching readback. Check
that a rejected point edit preserves the outline metrics and a successful edit
recomputes the affected lengths.

Use `--compare-saved-pattern-children` with saved internal shapes and baselines
to additionally compare the child count, order, type, names, points and line
descriptors. Check child line lengths to within 0.1 mm. Use a disposable Rust
scene to check save/reopen and a translated or reflected pattern. Verify
internal sewing via the separate solver cases.

For `snapshot_2d`, run `cargo test --locked flat_snapshot_renders_visible_inner_paths_only`.
Check an actual saved project with curved and straight internal lines.
Compare caption and PNG blocks and read the scene before and after capture
with `--compare-saved-snapshot-2d`. Inspect both images for relative layout,
outline completeness and visible internal paths. Renderer dimensions may differ.
Save and reopen the Rust project, render again and verify lines persist.

Use `--compare-strapped-draft-preview` to compare offline strapless, tube,
slip, sweetheart, straight neck and wide strap examples. Compare normalized
specs, pieces, edge lengths, seam order, elastic settings and last-spec
readback, including the drafted waist-merge pairs.
For the Rust build path, build a strapless top, check elastic constraints in
`simulate`, and repeat with straps to verify that the top-edge elastic is absent.

Use `--compare-neckline-and-sleeve-preview` for eight standard-measurement tops.
The test compares every response field for off-shoulder and halter necklines,
unknown neckline fallback, and bishop, flutter, bell, and puff sleeve previews.
It includes seam warning text, order, and counts and checks that invalid input
is rejected and `garment_last_spec` remains empty. This comparison uses both
MCP backends without needing the CLO bridge.

Use `--compare-draft-material-preview` to compare fabric stems, matching
keywords, RGB and unmodified color input in the full standard-measurement
preview. The cases cover missing and unreadable fabric assets, named and
hex colors, RGB arrays, defaults and rejected arguments. This comparison runs
without a live CLO bridge. The same recipe builds one Rust garment with a
named color and verifies that rebuilding with a missing fabric leaves the
last spec and checkpoint list unchanged.

Use `--compare-gathered-draft-preview` for standalone gathered skirts,
tiered skirts and dresses, empire, babydoll, drop-waist, smock, dirndl and wrap
presets. Compare complete preview results, including tier layers, intentional
length mismatches, seam directions, waist elastic, warnings and simulation defaults.
Build the tiered skirt in a disposable Rust scene and read back its layers,
seam flags and first-tier elastic before checking one simulation step.

Run `python -m clo_mcp.smoke_compat --verify-tiered-build-relaxation` to
exercise the full two-stage Rust build. A zero-step build retains initial
layers. Positive-step builds reset scene layers after the initial settle
and continue the same pose for `max(30, steps // 4)` steps. Compare a saved
auto-built pose and velocities to a manual sequence of the same MCP calls.
The fixture covers an appended pre-existing panel, reopening and resuming,
rebuild, and a plain top with only one stage. Live CLO comparison requires
a disposable scene with the bridge connected.

Use `--compare-curved-skirt-preview` for quarter-, half-, three-quarter- and
full-circle skirts, skater dresses, ball gowns, mermaid dresses and trumpet
skirts. Compare the complete preview, including radial seam order, the four
waist joins on dresses and standalone waist elastic. Build a skirt and dress
through Rust, read back the stored sewing and check one simulation step.

Run `python -m clo_mcp.smoke_compat --verify-one-piece-dress-merge` to
verify the Rust build for A-line, strapless and long-sleeve sheath dresses.
A zero-step build keeps the bodice and lower panels. A positive-step build
welds both waist joins and continues for 30 steps. Compare the rest area,
triangle incidence at the shared waist, surviving seam indices, elastic
settings, reopening and resumed steps. Live CLO comparison requires the
bridge and a disposable scene.

Run `python -m clo_mcp.smoke_compat --verify-build-snapshot-workflow` to
verify the Rust build and rebuild response through MCP. Check the summary,
caption, and ordered PNG blocks for default, empty, explicit, and disabled
views. Read the file bytes back and confirm that the mixed-case
`Three-Quarter` alias uses the same camera as `3/4-right`. A capture error
leaves the completed build available through `garment_last_spec` and scene
save. A failed draft preserves the prior build.

Run `python -m clo_mcp.smoke_compat --verify-garment-session-history` to
exercise two builds with different patterns. Within one MCP session,
opening or restoring the older scene and clearing the project change the
scene while the most recent successful build stays available for
`garment_rebuild`. Restarting the bridge within that session preserves the
record. A fresh MCP process can open the saved geometry and starts with
empty `garment_last_spec`; a rebuild before its first build returns an error.
Set `CLO_MCP_OFFICIAL_COMMAND` to also check empty history and the no-build
error on the official MCP server without requiring its live bridge.

Run `--verify-appended-build-report` to append a top beside existing
same-name panels. Compare the build's `a`, `b`, and `ok` sewing rows to the
plan, then reopen the project to verify the distinct panel IDs, actual
fabric and seam assignments, and the particle distance set by a zero-step build.

Run `--verify-calibrated-build-arrangement` to exercise a controlled
`CLO_MCP_STATE/calibration/<avatar>.json` fixture. Compare the reported
front and back arrangement points, the penalty for candidate points below
the target, and the saved initial mesh centres. Check that an unrelated
avatar cache is ignored, the saved placement is available after reopening
without the cache, and a manually changed arrangement takes precedence.
The standard-measurement preview stays independent of calibration.

Run `--verify-outline-edit-remaps` to exercise repeated relative moves and
line/point deletion and unfolding in a portable scene. Reopen each edited
scene and verify that unaffected sewing uses the new line index, the
elastic and shirring settings follow the surviving lines, and a seam
spanning a removed line is discarded. Check that an invalid edit leaves
the surviving sewing unchanged.

Run `--verify-symmetry-sewing` with both values of `with_sewing`.
Reopen the mirrored panel and verify that whole-edge sewing, partial
internal sewing and self-sewing reference the expected panel IDs.
Check reflected internal rest coordinates and confirm that both
internal seam groups reach the Rust solver after reopening.

Run `--verify-display-edits-keep-drape` to verify that 2D movement,
3D visibility and strengthening preserve an existing pose and velocities.
Compare the 2D and 3D snapshots, visible OBJ objects, saved state, and
step count after reopening. A geometric flip discards only the flipped panel's pose.
Set `CLO_DISPLAY_FIXTURE` to a native project with saved drape to also
verify absolute placement and visibility changes on archived meshes.

Run `--verify-sew-keeps-drape` to add sewing to a settled scene, reopen
it, and assemble the additional stitch constraints at zero steps.
Check that the saved positions and velocities match the pre-sewing
scene, then advance one simulation step. Verify duplicate and
out-of-range seam rejection. Set `CLO_DISPLAY_FIXTURE` to check
sewing and reopening with a native pose archive as well.

Run `--verify-delete-keeps-drape` to delete panels by name and index from
a simulated scene. Check that repeated references are deduplicated,
an invalid batch preserves the scene, and only the deleted panels'
meshes and sewing are removed. Reopen and resume the surviving
simulation. Set `CLO_DISPLAY_FIXTURE` to also check archived meshes and
native seam groups after deleting a native panel.

Run `--verify-new-panels-keep-drape` to create, copy and mirror panels
beside settled cloth. Verify the OBJ contains both posed and newly
placed panels, then reopen and assemble the new meshes at zero steps.
Check the original mesh, velocities and step count, then resume simulation.
Set `CLO_DISPLAY_FIXTURE` to also verify a new panel beside archived meshes.

Run `--verify-portable-append-keeps-drape` to append two simulated
`.clorust` projects with overlapping panel IDs and nonconsecutive
material indices. Reopen and verify that seams, fabrics and saved
meshes reference the remapped panels, both avatars are present, and
zero steps preserve both sets of positions and velocities. The receiving
scene's step count continues. An unknown saved mesh ID rejects the append.

Run `--verify-geometry-edits-rebuild-one-panel` to verify point movement,
arrangement changes, flips, point deletion and particle distance edits.
Save and reopen after each edit. At zero steps, rebuild the edited panel
and verify that the other panel's saved mesh and velocities match exactly.
Check that a subsequent step continues the original simulation.

Run `--verify-runtime-export-workflow` to verify Rust discovery by module and
keyword, catalog pagination, qualified and unqualified documentation lookup,
and invocation of the asset path, layout PNG, 3D thumbnail and FBX API entries.
The case compares the direct and Python layout exports with `snapshot_2d`, checks the
thumbnail camera direction and reopens the FBX. It rejects unsupported layout
modes without writing the destination. Compare live reference export and
argument behavior separately when the CLO bridge is available.

Run `--verify-checkpoint-workflow` for automatic checkpoint names, newest-first
ordering with local timestamps, state restore and malformed, missing or invalid names. Set
`CLO_CHECKPOINT_FIXTURE=/path/to/saved.zprj` to check that checkpoint restore
also preserves native re-export. Reopen the exported project and confirm the
patterns and avatars. Check that changing or removing the source archive
still allows checkpoint restore and stops native re-export.
Run `--verify-build-checkpoints` for the `auto_before_build_` path in build
and rebuild responses. Empty or appended scenes skip automatic checkpoints.
Read back the pre-replacement geometry, materials, and saved simulation.
Set `CLO_CHECKPOINT_FIXTURE` to a native project with sewing and saved drape
to check that replacement removes its seam groups and a checkpoint restore
recovers them and enables a new native save. Also check that an unavailable
checkpoint directory leaves a successful build with a null checkpoint path.
With the official stdio command configured, run `--compare-checkpoint-listing`
to compare order and local timestamps using disposable directories without
starting CLO.

Set `CLO_NATIVE_APPEND_FIXTURE=/path/to/saved.zprj` and run
`--verify-native-append` with a native project that has fabrics, seams, an
avatar and a complete saved pose. Check that append preserves original project
identity, shifts new panel and fabric indices, and restores both saved meshes
and embedded avatars. Save and reopen the combined project. Test invalid input
in both modes. Append to an empty scene, re-export as ZPAC, then append the
ZPAC and confirm the avatar stays in the scene and both poses are available.
Run `--verify-mixed-native-append` with the same fixture to cover a Rust panel
before and after simulation. Export all panels immediately after append,
save and reopen the combined project, then call `simulate` with zero steps.
Check that preserved meshes retain positions, topology and any Rust velocities.
Only panels without a mesh initialize from their outlines. A new explicit
particle distance requests a remesh.

Use `--compare-saved-seams` to compare each saved sewing group's index, name
and member list against the live CLO project, and check every panel's group
indices in `pattern_list(detail=True)`. Test with a saved project that contains
non-panel objects. This case verifies query results. Solver coverage requires
separate drape and seam-gap evidence.

For `scene_info`, leave a saved `.zprj` open in CLO 3D with no unsaved changes.
Set the official stdio command as above and point Rust to the same asset folder.

```sh
CLO_RUST_BINARY="$PWD/target/release/pattern-native" \
  CLO_MCP_ASSETS='/path/to/CLO Assets' \
  python -m clo_mcp.smoke_compat --compare-saved-scene-info
```

The recipe reads the official scene and opens the same file in an isolated
Rust session. It compares response keys, project identity, ordered patterns,
avatars and fabrics, seam group count, colorway names, active colorway index
and the shared asset path. It checks that each backend reports its own version
and temp path. Use additional fixtures to verify multiple avatars and colorways.

Use `--compare-saved-arrangements` with the same saved project and a loaded
avatar. Compare the scene's avatar names, the arrangement count and every name
in order. This case checks the names exposed by `arrangement_points`.

For mesh-based avatar girth estimates, run
`cargo test --locked avatar_tape_excludes_disconnected_arms`. Compare the
central torso slice against a known section with detached limb contours. For
an actual project, record the measurement source and differences in millimetres
against the CLO tool at the same avatar. Require independent evidence before
claiming parity for `avatar_measurements`.

For `--compare-saved-avatar-height`, use a saved `.zprj` containing a native
avatar with accessories. The check requires CLO's mesh fallback on
`avatar_measurements(refresh=True)`, which exports the current avatar to a
temporary OBJ. It compares height to within 0.25 mm. For a project with
recognized AVT body-region tags, run `--compare-saved-avatar-mesh` for all
32 numeric body fields with the same tolerance. An active CLO editor reading
requires a separate case. Check that the Rust avatar
still retains its complete mesh and arrangement names for collision and display.
Refresh a loaded avatar after stepping simulation and verify the pose and step
count remain intact. If the stored region indices disagree with the physical
mesh, check that the avatar opens and uses the unclassified mesh estimate.

For `--compare-saved-avatar-cache`, keep the saved project open in CLO and use
the same `CLO_MCP_STATE` directory on both backends (default `~/.clo3d-mcp`).
The Rust backend reads `measurements/<avatar>.json` and uses its millimetre
values for `avatar_measurements` and scene-based garment drafting. Compare every
body field and the cache source. Check that Rust `refresh=True` uses the
loaded mesh measurements and a subsequent normal read still returns the saved
table. A missing or unusable cache falls back to the mesh. Run
`cargo test --locked cache_uses_supported_names_units_and_avatar_identity`.

For saved internal seams, run `cargo test --locked native_inner_seam_uses_saved_mesh_edges`.
Open a native fixture with internal seams over MCP and use `simulate` without a
new particle distance. Save and reopen the resulting `.clorust` scene, then
compare `native_inner_seam_groups` and independently measure corresponding
points along the recorded mesh paths. For a negative control, remove
`inner_seam_paths` from the saved meshes in a disposable copy and repeat from
the same starting pose.

For intervals crossing multiple edges, run `cargo test --locked native_span_uses_distance_across_edges_in_either_direction`.
Use a saved scene with a seam interval that crosses a corner, inspect its
corresponding positions after `simulate`, and check reversed direction and a
disconnected path. A generated interval validates the Rust solver; compare a
native CLO-authored fixture separately before claiming double-backend parity.

For remeshing with internal seams, run `cargo test --locked rebuilt_mesh_anchors_internal_seams_in_rest_coordinates`.
Open the native fixture, call `simulate` with a different `particle_distance`,
and check the mesh vertex count changed while `native_inner_seam_groups` is
preserved. Reopen the Rust project and repeat from the new mesh. Compare against
a disposable copy without the recorded `inner_seam_rest` coordinates, and check
that points outside the new mesh are rejected rather than snapped to a boundary.

For local edge refinement, run `cargo test --locked adaptive_remesh_preserves_boundary_and_avoids_uniform_subdivision`.
At the MCP boundary, probe two particle distances on the same saved garment.
Check mesh size, preserved seams, triangulated area and boundary incidence.
Save and reopen both a new adaptive mesh and an older Rust mesh, and check
that a zero-step continuation preserves the meshes and accumulated step count.

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
