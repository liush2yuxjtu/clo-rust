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

Run `--verify-fabric-texture-clear` to clear a single map with an empty image
path. Check that the other maps persist through a portable save, the cleared base
is absent in GLB, and errors leave existing maps intact. With two fabrics, verify
that an unknown map or unreadable image leaves the valid target selected.
An invalid fabric index keeps the previously selected fabric.
Set `CLO_TEXTURE_CLEAR_FIXTURE` to a single-colorway `.zprj` or `.zpac` with
base and normal maps to also check native archive reopen and preservation of the
other maps. A live CLO reopen is a separate comparison.

Run `--verify-fabric-recolor-opacity` to recolor opaque and sheer fabrics with
base, normal and opacity maps. Verify the base image stays bound, its
desaturation remains set after reopen, and the GLB still contains a base
texture. The Rust texture test checks that the recolored snapshot keeps
detail from the original texels. Set
`CLO_RECOLOR_OPAQUE_FIXTURE` and `CLO_RECOLOR_SHEER_FIXTURE` to native archives
with opacity maps to check native save and reopen. The opaque case also sets a
new opacity image after recoloring and checks that its bytes survive the save.
Check each of faces 0, 1, and 2 through
`fabric_api.GetFabricPBRMaterialBaseColor` and the native face records when present.
Compare six-digit hex, shorthand hex, and a compound color name, and check that
invalid hex and invalid fabric indices leave the material list intact.

Run `--verify-fabric-assignment-workflow` to register a fabric through
`AddFabric`, then assign it to one panel through Python and apply a different
fabric to another panel. Use both its name and its numeric index as a string
while another panel has that same numeric name. Verify both references select
the index. Read back the effects of each failed stage: a missing panel leaves
the registered fabric and existing assignments, an invalid color leaves the
new assignment, and a texture error leaves the applied color and assignment.
Check that adding leaves both original assignments
and the drape intact. Reopen at zero steps, compare positions and velocities,
and check each GLB mesh's material. Optionally set `CLO_FABRIC_ASSIGN_FIXTURE`
to the absolute path of a real denim `.zfab` for the registration step and a
nested library case. Check that `fabric_library` follows path order, preserves
duplicate basenames, filters accented names case-insensitively, and skips
Rust material JSON when the library contains CLO fabrics. The `denim` keyword
must load the standard file at the library root even with a same-named nested file.

For release-download, checksum, or cache changes, use an existing release with the required assets and run `CLO_RUST_RELEASE=<tag> python -m clo_mcp.smoke_release` with GitHub CLI access. The smoke test exercises the actual published download, checksum verification, cache hit, seam audit, and replay through stdio. For new releases, build and test on each target platform, run the binary's `--version`, compute its SHA-256 checksum, and confirm the published tag and assets. Run the release-download smoke test against the published tag.

### Verify the 48 public CLO MCP tools

The public acceptance list is [`clo_mcp/clo_tool_contract.json`](../../../clo_mcp/clo_tool_contract.json).
Verify it through MCP stdio with matching requests sent to `CLO_MCP_BACKEND=official`
and `CLO_MCP_BACKEND=rust`. A tool is behaviorally matched only when its relevant
inputs, results, state changes, and error cases pass the dual-backend comparison.

| Check | Passing evidence |
| --- | --- |
| Public tool contract | Native reference MCP and both adapters advertise the same 48 pinned definitions, including names, descriptions, and input/output schemas. |
| Rust call smoke | One successful representative call per public tool in an isolated synthetic Rust scene. The JSON report shows each tool and its MCP content types. |
| Dual-backend behavior | The same relevant cases pass through both backends, with assertions for returned values, observable scene changes, errors, and produced files as applicable. |

From the repository root, use a Python 3.11+ environment with `clo_mcp/requirements.txt`
installed. Replace the example Python path with the reference MCP's venv interpreter.
Build the binary and save each run's exit status, JSON on stdout, and log on stderr:

```sh
cargo build --release --locked
export CLO_RUST_BINARY="$PWD/target/release/pattern-native"
export CLO_MCP_OFFICIAL_COMMAND='["/path/to/clo3d-mcp/.venv/bin/python","-m","clo3d_mcp.server"]'
evidence_dir="$(mktemp -d)"
git rev-parse HEAD > "$evidence_dir/revision.txt"
python -m clo_mcp.smoke_compat --verify-tool-contracts \
  > "$evidence_dir/contracts.json" 2> "$evidence_dir/contracts.log"
python -m clo_mcp.sweep_compat \
  > "$evidence_dir/rust-tool-calls.json" 2> "$evidence_dir/rust-tool-calls.log"
python -m clo_mcp.smoke_compat --compare-strapped-draft-preview \
  > "$evidence_dir/strapped-preview.json" 2> "$evidence_dir/strapped-preview.log"
printf 'Evidence: %s\n' "$evidence_dir"
```

The sweep exits nonzero for missing, unexpected, or failed calls. Its success criterion
is `expected_count = called_count = ok_count = 48` with empty `missing`,
`unexpected`, and `errors` arrays. Use `tools.<name>` to inspect each result.

#### What to verify for each tool

Use the pinned list as the row index. Record the Rust revision, reference MCP
and CLO 3D versions, exact MCP arguments, fixture checksum, and both runs'
results. For mutations, give each backend an independent copy of the same
starting scene. Capture the initial state so the expected change can be checked
explicitly. Run error cases on a fresh copy or capture the state just before
the rejected call.

| Verify | How to check |
| --- | --- |
| Input contract | Compare the `tools/list` entry, then exercise required arguments, defaults, and relevant boundary or invalid values through `tools/call`. |
| Return values | Compare structured fields, types, order and units. Treat volatile fields such as elapsed time or temporary paths according to what the tool promises. For numeric comparisons, record the tolerance and its units before running the case. |
| Scene or file changes | Read the affected objects back through a separate MCP call. For saved or exported output, reopen or parse it and check the properties relevant to the tool. |
| Error behavior | Use the same bad input in both backends. Compare the error outcome and read back the scene to check which operations took effect before the error. Some multi-stage tools keep completed changes. |

Record the verdict for each case as **PASS**, **FAIL**, **BLOCKED** (for example
when the required native bridge is down), or **NOT RUN**. Label saved CLO
captures and SDK callback tests with their evidence type. A dual-backend scene
case requires a live native run. Aggregate a tool as matched only after all
its applicable cases pass. Keep the 48-tool contract and Rust smoke totals
separate from the behavioral count.

#### Read the evidence and reproduce a matched case

The contract command reads `tools/list` directly from the reference MCP and
from both adapters without opening the CLO desktop or using its Python bridge.
Expect `public_tool_count: 48` and three `pass` fields. `sweep_compat` sends
actual `tools/call` requests against Rust. Its `OK` status means the MCP result
has no error flag for that representative call. The 11 diagnostic tools listed
by the adapter are outside the 48-tool acceptance count.

The preview command sends the same seven `garment_plan_preview` specs to both backends with
`use_scene_measurements=false`. It asserts equality of every returned field,
including pieces and seams, then checks that both reject `{"spec": []}` and
that `garment_last_spec` remains unchanged. The JSON records the piece and seam
counts and the error and state checks. This recipe exercises the draft preview
path without requiring a live CLO scene. For a scene-backed example, run
`--compare-saved-scene-info` with the same `.zprj` open in CLO, as described below.

Run `--compare-draft-option-semantics` to compare aliases, garment and fit
fallbacks, bodice ease, and numeric simulation options with the same standard
measurements. Check every preview field, invalid numeric values, and the last
garment spec in both sessions.

For the acceptance tally, create one row per pinned tool, with separate fields
for contract, Rust call, and dual-backend behavior. Fill in these fields with
the relevant evidence for each revision. See the [48-tool evidence record](../../../docs/verification/clo-mcp-acceptance.md) for the latest case inventory:

| Tool | Contract | Rust call | Dual-backend behavior | Evidence |
| --- | --- | --- | --- | --- |
| `garment_plan_preview` | PASS | PASS | PASS for the seven strap-style cases | `strapped-preview.json`, matching request, input error, and spec readback |
| `scene_info` when `clo_status.bridge` is `down` | PASS if listed | PASS if called | BLOCKED for the live scene comparison | Status response and the latest applicable saved-scene comparison |

For scene-backed checks, save `clo_status` and confirm `bridge=up` before
recording a live comparison. Keep results at case granularity. A tool-level
behavior PASS requires all applicable cases in the checklist below. For example,
compare `scene_info` with the same saved project and inspect ordered panels,
avatars, fabrics, and seam groups. For a mutation, also read back the affected
state after successful and rejected requests and compare with the expected changes.
For a file or image, reopen or parse it and check the relevant properties. Report
the three results separately and link the case evidence behind the behavioral tally.

Set `CLO_MCP_OFFICIAL_COMMAND` to the real CLO MCP stdio command as a JSON string array
to include the official backend smoke. Run it with the CLO 3D bridge connected.
`tools/list` is checked against the contract, including input and output schemas.
The Rust fixtures also exercise selected scene, image, export, failure, and session flows.

Run `python -m clo_mcp.smoke_compat --compare-control-guide` with
`CLO_MCP_OFFICIAL_COMMAND` set to verify the shared guide workflow without
starting CLO. The report preserves each backend's instructions and verifies
the route from API discovery and invocation through UI inspection, action,
and state readback.

Run `python -m clo_mcp.smoke_compat --compare-desktop-observation` with
`CLO_MCP_OFFICIAL_COMMAND` set and the CLO desktop available to the MCP host.
The case checks the native server's `tools/list` directly against the pinned
48-tool contract. It then compares the guide fields, tree limits and parent links,
stale snapshot rejection, and PNG response framing across both adapters. It
exercises read-only queries and a rejected action, so it needs no live CLO Python
bridge and does not activate the CLO window. Window titles, node counts, snapshot
IDs, and image dimensions can differ by backend. Use a separate disposable scene
case to verify effects of successful actions.
If native state lists no on-screen CLO window and capture returns an MCP error,
the script records the screenshot as `BLOCKED` and exits 2. It still records
the other observations and Rust screenshot dimensions.

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
For standalone snapshots, check that empty views and label use the default
front and oblique PNGs and `snap_` prefix. Export a 3D thumbnail immediately
after the snapshot and compare its bytes to the final image. Confirm a 2D
capture and an invalid first 3D view leave that camera unchanged. When a later
view is invalid, confirm the tool returns an error, the earlier PNG is still on
disk, the camera stays on the last completed view, and later views never run.
Check the same behavior when snapshots are requested by a garment build.

Run `python -m clo_mcp.smoke_compat --verify-garment-session-history` to
exercise two builds with different patterns. Within one MCP session,
opening or restoring the older scene and clearing the project change the
scene while the most recent successful build stays available for
`garment_rebuild`. Restarting the bridge within that session preserves the
record. A fresh MCP process can open the saved geometry and starts with
empty `garment_last_spec`; a rebuild before its first build returns an error.
Force a solver error during rebuild and save the scene. Verify the newly
drafted pieces, seams, material, and particle distance are present, with
no simulated pose, while `garment_last_spec` still reports the prior build.
Set `CLO_MCP_OFFICIAL_COMMAND` to also check empty history and the no-build
error on the official MCP server without requiring its live bridge.

Run `--verify-appended-build-report` to append a top beside existing
same-name panels. Compare the build's `a`, `b`, and `ok` sewing rows to the
plan, then reopen the project to verify the distinct panel IDs, actual
fabric and seam assignments, and the particle distance set by a zero-step build.

Run `--verify-appended-build-keeps-drape` to append a garment at 90 mm to
cloth simulated at 40 mm. Reopen the zero-step build and verify that the
new panels initialize while the previous grid and velocities are preserved.
Compare a positive-step build to the manually resumed state, including
the step count and quality. Check an explicit remesh separately.

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
Check numeric strings, relative movement with a stray `y` field, and rejection
of an absolute move missing `y`. Copy uses relative offsets even when absolute
fields are provided. Successful layout movement invalidates the old UI snapshot.
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
Compare the Python seam-group count, API names and name lookups, and panel
memberships against `seams_list` and `pattern_info`. Check the native groups
and the newly authored group on the same archive after reopening.

Run `--verify-delete-keeps-drape` to delete panels by name and index from
a simulated scene. Check that repeated references are deduplicated,
an invalid batch preserves the scene, and only the deleted panels'
meshes and sewing are removed. Select a surviving panel and a panel slated
for deletion. Verify the old UI snapshot is rejected, the surviving panel
still has selection, and a new panel reusing the deleted ID stays unselected.
Reopen and resume the surviving
simulation. Set `CLO_DISPLAY_FIXTURE` to also check archived meshes and
native seam groups after deleting a native panel.

Run `--verify-new-panels-keep-drape` to create, copy and mirror panels
beside settled cloth. Verify the OBJ contains both posed and newly
placed panels, then reopen and assemble the new meshes at zero steps.
An invalid outline leaves the current UI snapshot usable. If the outline
is valid but the requested arrangement is missing, the newly named panel
remains and the previous UI snapshot is rejected.
Check the original mesh, velocities and step count, then resume simulation.
Set `CLO_DISPLAY_FIXTURE` to also verify a new panel beside archived meshes.

Run `--verify-portable-append-keeps-drape` to append two simulated
`.clorust` projects with overlapping panel IDs and nonconsecutive
material indices. Reopen and verify that seams, fabrics and saved
meshes reference the remapped panels, both avatars are present, and
zero steps preserve both sets of positions and velocities. The receiving
scene's step count continues. An unknown saved mesh ID rejects the append.

Run `--verify-interchange-append-keeps-drape` to export and append both a
DXF layout and pattern JSON into simulated cloth. Use explicit append for DXF
and the default mode for pattern JSON. Open OBJ and FBX with the default mode
and verify the imported triangles coexist with the original simulated garment.
Reopen and initialize the
new panels at zero steps while the original mesh and velocities stay
identical. The JSON seam follows its remapped panels, and a malformed
seam reference leaves the receiving scene intact. In a two-colorway scene,
export DXF and pattern JSON with both `colorways` and `include_avatar` set.
Compare the files to exports without those flags. Check the return values,
nested Unicode destinations, and unchanged project identity. Check that a
blocked save, unsupported extension, and mesh export with missing archived
colorway materials preserve existing files and the scene.

For a live multi-colorway export comparison, use a disposable two-panel
project. In official CLO, call `utility_api.CopyColorway(0, 1)`, name the
colorways with `utility_api.SetColorwayName`, and activate each in turn
with `utility_api.SetCurrentColorwayIndex`. Set different fabric colors
with `fabric_set_color` and verify both RGBA values through the three-argument
`fabric_api.GetFabricPBRMaterialBaseColor` in `clo_run_python`. Save with
the second colorway active. Check `scene_info`, then call `scene_export`
with `colorways=true` for OBJ, FBX, GLB, and glTF. Compare ordered filenames,
count and material content, including OBJ material and glTF binary sidecars.
When running CLO on a remote Boat VM, copy the same saved project to the
Rust host and verify its SHA-256 before comparing. Paths and runtime versions
are host-specific.

Run `--verify-reordered-native-archives` with `CLO_REORDERED_ZPRJ` and
`CLO_REORDERED_ZPAC` set to the two Boat fixtures listed in the acceptance
record. Verify the two panels, named seam, material assignments, colorway
order and active index, and each archive's particle distance. Reopen the
portable save and check that zero simulation steps preserve triangle
indices, rest UVs, and posed vertices exactly. Read both front RGBA values
through the three-argument fabric API, then repeat the query from Python
after reopening. Verify the two-argument call reads the active colorway
and an invalid colorway index leaves the scene intact.

Run `--verify-native-colorway-exports` with `CLO_REORDERED_ZPRJ` set to
the two-colorway Boat project. Compare exact filename order for OBJ, FBX,
GLB, and glTF with the official `scene_export` response. Reopen OBJ and FBX
and inspect the material, read both glTF containers and their buffers, and
check that the two colors differ. With the official files in the
`native-obj`, `native-fbx`, `native-glb` and `native-gltf` subdirectories of
`CLO_NATIVE_COLORWAY_EXPORTS`, compare triangle counts and RGB values to
within 1e-6. Check CLO's linear glTF color factors for native materials.
Repeat an export, then block the second GLB path with a directory and
verify the earlier file and original scene are preserved.

Run `--verify-native-colorway-textures` with Pillow installed in the verifier
environment and `CLO_NATIVE_TEXTURE_EXPORTS` set to the Boat capture directory.
It contains the textured ZPAC, both swatch PNGs, and each official export
folder. After saving and reopening, compare both RGBA pixel grids and
effective UV bounds for OBJ, FBX, GLB and glTF. Inspect external PNG naming
for OBJ, FBX and glTF and verify that GLB embeds the images.

Run `--verify-native-colorway-normals` with `CLO_NATIVE_NORMAL_EXPORTS` set
to the Boat normal-map capture. Check the two normal maps in OBJ's MTL,
FBX material bindings, and glTF/GLB's `normalTexture`, including strength and
repeat scale. Set `CLO_NATIVE_PBR_EXPORTS` to the capture with roughness and
opacity as well to compare OBJ sidecar names and pixels, and FBX opacity
texture binding. Record the official glTF file list separately because
the tested CLO session returned both single-file and per-colorway results.

Run `--verify-geometry-edits-rebuild-one-panel` to verify point movement,
arrangement changes, flips, point deletion and particle distance edits.
Successful point movement must invalidate the pre-edit UI snapshot.
Save and reopen after each edit. At zero steps, rebuild the edited panel
and verify that the other panel's saved mesh and velocities match exactly.
Check that a subsequent step continues the original simulation.
Exercise numeric strings for corner coordinates and index, relative offsets
when just one absolute coordinate is present, and `MovePatternPoint` returning
`null` with a visible point change. Repeated relative offsets use the
point coordinates from before the batch. Verify a zero move preserves
the pose, and that a late invalid move keeps completed moves, invalidates
the old UI snapshot and preserves the other panel's pose.

Run `--verify-elastic-parameter-updates` after changing edge elastic settings.
Set strength and target length through the API on disabled edges, then check
the saved flags and zero solver constraints. Enable one edge through
`pattern_ops`, reopen the archive, and check that updating a disabled edge
still leaves the active constraint count unchanged.
Check that numeric zero skips the optional public elastic parameters, while
a string zero or an explicit API setter stores zero. An invalid strength
after valid enable and length settings retains those completed changes.

Run `--verify-avatar-changes-keep-drape` to add and replace avatars around
already simulated cloth. Reopen and compare the original positions and
velocities, then run a positive step to verify overlap correction by the
new collider. Set `CLO_AVATAR_FIXTURE` to a supported AVT to verify append
and default `scene_open` mode, direct typed `ImportAvatar`, and the equivalent
Python call. All four add an avatar and leave panels, fabrics, and drape intact.
Verify an import of a missing AVT preserves the avatar list.
Delete the primary avatar from Python and check that the remaining avatar is
promoted and the saved positions and velocities stay identical. Verify an
invalid batch leaves the avatars intact. Set `CLO_AVATAR_LIBRARY_FIXTURE` to an
`Avatar` folder containing `Female/FV2.1_Mia.avt` to check that alias resolution
prefers the standard library location over a same-name backup.

Run `--verify-simulation-reset-quality` to compare arranged meshes before
simulation and after reset. Exercise the public tools and corresponding
`utility_api` calls, checking that both use the same quality setting,
reset keeps the sewing and per-panel particle distance, and opening a
saved scene in a fresh session can recover the saved quality before reset.
Check that a requested CPU quality takes effect before an empty-scene error
or invalid particle distance and is used by the next call. Verify that the
invalid distance leaves the prior mesh and per-panel distances intact, that
rejected quality 3 leaves the previous setting, and that the public response
reports the requested steps and seconds to two decimal places.

Run `--verify-simulation-remesh-keeps-pose` to refine settled cloth from
90 mm to 25 mm at zero steps. Check exact shared vertices, inherited
velocity at interpolated vertices, and the measured displacement from
the reset arrangement. Reopen and continue the original step count.
Read `remesh_preserved_panels` and `remesh_reinitialized_panels` when
explicitly remeshing a scene that already has a pose.
Compare `SetParticleDistanceOfPatterns` against the same zero-step public
remesh. Check that setting distance before simulation affects the initial mesh,
the same setter on settled cloth retains the step count and pose, and
`simulate(particle_distance=0)` follows the omitted-distance path.

Set `CLO_RUNTIME_PATTERN_FIXTURE` to a saved `.zprj` containing patterns and
run `--verify-runtime-pattern-query`. Check four pattern catalog entries,
including two pages for `GetPatternIndex` and `GetPatternIndexW`. Verify both
index lookups against a named piece from `scene_info`, then read the name back
by index. Check search, description, doc lookup, Python output, and repeated
release of an `ImportExportOption` handle. Verify a call with the released
handle is rejected and the scene remains intact.
With the official command configured, load the same saved fixture in CLO first.
The case reads that scene without editing it. A down bridge or a different open
project is recorded as `BLOCKED` with exit 2, alongside the Rust observations.

Run `cargo test --locked completed_api_call_past_deadline_stops_the_script`
for a Python request whose synchronous API call finishes after the deadline.
Check that the completed mutation remains in the scene and the interpreter
receives no reply that would let it execute a subsequent statement. Through
MCP stdio, also create a pattern, call Python with a rename followed by a sleep
and a second rename, and read back the first rename after the timeout. Verify
a new Python request can read the scene.

Run `--verify-runtime-export-workflow` to verify Rust discovery by module and
keyword, catalog pagination, qualified and unqualified documentation lookup,
and invocation of the asset path, layout PNG, 3D thumbnail and FBX API entries.
The case compares the direct and Python layout exports with `snapshot_2d`, checks
the thumbnail camera direction, and exports and reopens an FBX at a Unicode
path through `ExportFBXW`. It exercises `ExportThumbnail3DW` through Python and
checks the wide asset and temporary folder calls. It rejects unsupported layout
modes without writing the destination, tests search limits, and checks missing
short and qualified documentation names. Set `CLO_RUNTIME_EXPORT_CATALOG` to a
native catalog capture indexed by module to compare the nine named entries
including their signatures and summaries. Compare live reference export and
argument behavior separately when the CLO bridge is available.

Run `--verify-checkpoint-workflow` for automatic checkpoint names, newest-first
ordering with local timestamps, state restore and malformed, missing or invalid names. Set
`CLO_CHECKPOINT_FIXTURE=/path/to/saved.zprj` to check that checkpoint restore
also preserves native re-export. Reopen the exported project and confirm the
patterns and avatars. Check that changing or removing the source archive
still allows checkpoint restore and stops native re-export.
Run `--verify-scene-clear-state` to compare the two Rust clear modes and
the scripting `NewProject` call. Check that keeping the avatar also keeps
materials and project identity, a fresh project resets material selection,
and stale UI observations are rejected in both modes. Read back the last
garment spec to confirm its session lifetime.
Run `--verify-scene-open-ui-state` to use an observed selection across
replacement, append and checkpoint restore. Verify stale tokens are rejected
after a successful load, with selection retained on append. A failed open or
restore should preserve both the original scene and its valid UI token. Set
`CLO_CHECKPOINT_FIXTURE` to also check restoration of a native archive.
Run `--verify-build-checkpoints` for the `auto_before_build_` path in build
and rebuild responses. Empty or appended scenes skip automatic checkpoints.
Read back the pre-replacement geometry, materials, and saved simulation.
Set `CLO_CHECKPOINT_FIXTURE` to a native project with sewing and saved drape
to check that replacement removes its seam groups and a checkpoint restore
recovers them and enables a new native save. Also check that an unavailable
checkpoint directory leaves a successful build with a null checkpoint path.
With the official stdio command configured, run `--compare-checkpoint-listing`
to compare order and local timestamps using disposable directories without
starting CLO. Compare full rows on a shared fixture with tied or nearly equal
modification times, and include a linked native checkpoint on POSIX.
Run `--compare-shared-checkpoint-folder` with the same official command and a
native `CLO_CHECKPOINT_FIXTURE` containing saved drape. Unset
`CLO_RUST_CHECKPOINT_DIR` for this case. Both adapters list the fixture from the
same disposable `CLO_MCP_STATE/checkpoints`. Rust restores it by name and checks
the archived geometry, fabrics, seams, and pose. It then saves a new named
native `.zprj` checkpoint and verifies both adapters list it, the active
project stays selected, and Rust restores its saved state. Omit the name and
then pass an empty name with existing timestamp files in the directory. Check
that both calls use distinct suffixed names and produce native projects
visible to both adapters. Check that
same-name `.clorust` writes preserve the existing native archive and failed
restores preserve the scene. A live CLO restore requires the bridge.

Set `CLO_NATIVE_APPEND_FIXTURE=/path/to/saved.zprj` and run
`--verify-native-append` with a native project that has fabrics, seams, an
avatar and a complete saved pose. Exercise typed `ImportZprj` and Python
with `bLoadGarment` or `bLoadAvatar` disabled. Check that partial replacement
and append affect the requested component, and that a corrupt avatar-only
import preserves the receiving scene. Check that full append preserves original project
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
in order. Compare the full `GetArrangementList` records, including offsets,
orientation, and type. In Rust, set a point by name and by native index and
verify the per-panel readback excludes the list index. To use captured
reference rows, set `CLO_ARRANGEMENT_REFERENCE` to a JSON capture containing
`scene.project_path` and `arrangements.points`.

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
