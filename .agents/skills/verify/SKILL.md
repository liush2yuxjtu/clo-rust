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
is absent in GLB, and other maps remain intact. With two fabrics, verify
that an unknown map returns an error after selecting the target. Successful
map calls select the target in the reopened fixture. After adding a fabric,
CLO’s colorway update can reselect the newly added fabric. Missing or unreadable
images return success and keep the existing textures. A signed index that does
not select a valid fabric uses the previously selected material.
Set `CLO_TEXTURE_CLEAR_FIXTURE` to a single-colorway `.zprj` or `.zpac` with
base and normal maps to also check native archive reopen and preservation of the
other maps. Add the missing channels after reopening and verify that all new
images and the preexisting maps survive another save. A live CLO reopen is a
separate comparison.

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
invalid hex and negative fabric indices return errors. For an in-range
unsigned integer that names no fabric, verify three false face results and
the bridge's texture cleanup on the current fabric.

Set `CLO_DESATURATION_REFERENCE_DIR` to the official GLB capture directory
and install Pillow to verify the decoded base image after direct Rust export.
The palette pair is `shared-desat-palette.png` and `shared-desat-after.glb`.
For `ramp`, `alpha`, `varied`, `gray`, `single`, and `long-ramp`, use
`desat-<case>.png` and `desat-probe-<case>-after.glb`. Compare decoded RGBA
pixels exactly, including alpha.

Run `--verify-fabric-assignment-workflow` to register a fabric through
`AddFabric`, then assign it to one panel through Python and apply a different
fabric to another panel. Use both its name and its numeric index as a string
while another panel has that same numeric name. Verify both references select
the index. Read back the effects of each failed stage: a missing panel leaves
the registered fabric and existing assignments, an invalid color leaves the
new assignment, and a missing texture returns success and leaves the applied
color and assignment.
Check that adding leaves both original assignments
and the drape intact. Reopen at zero steps, compare positions and velocities,
and check each GLB mesh's material. Optionally set `CLO_FABRIC_ASSIGN_FIXTURE`
to the absolute path of a real denim `.zfab` for the registration step and a
nested library case. Check that `fabric_library` follows path order, preserves
duplicate basenames, filters accented names case-insensitively, and skips
Rust material JSON when the library contains CLO fabrics. The `denim` keyword
must load the standard file at the library root even with a same-named nested file.

Run `--compare-live-fabric-edits` with both backends sharing the native
Fabric library and a fresh `CLO_MCP_STATE` directory. Set
`CLO_FABRIC_LIVE_FIXTURE` to the two-panel ZPRJ, and
`CLO_FABRIC_LIVE_SAVED_FIXTURE` to a ZPRJ with two saved fabrics.
Set `CLO_FABRIC_LIVE_IMAGE` and `CLO_FABRIC_LIVE_WIDE_IMAGE` to static PNGs readable
from both backends. Use the same configured library path in both backends,
including the Wine drive prefix on Boat. Compare whole responses and full
fabric lists, the assigned fabric indices, and the stored material count.
Check repeated and missing references, a positive nonexistent panel and
signed/unsigned overflow after a valid assignment. Check successful and
failed recolors and map edits, including the selected material and base map
readback. Open the other fixture after adding materials and verify the saved
project selects the actual edit target again. Include a wide PNG of
6300 × 1485. Check renderer and archive
persistence through the separate fabric export and native reopen cases.

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
Build the binary and keep the candidate revision, exit status, JSON on stdout,
and log on stderr together. The three commands below run even if one fails.
The final status is nonzero if any check failed.

```sh
cargo build --release --locked || exit 1
export CLO_RUST_BINARY="$PWD/target/release/pattern-native"
export CLO_MCP_OFFICIAL_COMMAND='["/path/to/clo3d-mcp/.venv/bin/python","-m","clo3d_mcp.server"]'
evidence_dir="$(mktemp -d)" || exit 1
git rev-parse HEAD > "$evidence_dir/revision.txt"
"$CLO_RUST_BINARY" --version > "$evidence_dir/runtime.txt"
status=0
record() {
  name="$1"
  shift
  code=0
  "$@" > "$evidence_dir/$name.json" 2> "$evidence_dir/$name.log" || code=$?
  printf '%s %s\n' "$name" "$code" >> "$evidence_dir/exit-status.txt"
  if [ "$code" -ne 0 ]; then
    status=1
  fi
}
record contracts python -m clo_mcp.smoke_compat --verify-tool-contracts
record rust-tool-calls python -m clo_mcp.sweep_compat
record strapped-preview python -m clo_mcp.smoke_compat --compare-strapped-draft-preview
cat "$evidence_dir/exit-status.txt"
printf 'Evidence: %s\n' "$evidence_dir"
test "$status" -eq 0
```

The sweep exits nonzero for missing, unexpected, or failed calls. Its success criterion
is `expected_count = called_count = ok_count = 48` with empty `missing`,
`unexpected`, and `errors` arrays. Use `tools.<name>` to inspect each result.

```sh
jq '{public_tool_count, reference_mcp, official_adapter, rust_adapter}' "$evidence_dir/contracts.json"
jq '{expected_count, called_count, ok_count, missing, unexpected, errors}' "$evidence_dir/rust-tool-calls.json"
jq '.tools.garment_plan_preview' "$evidence_dir/rust-tool-calls.json"
jq '{cases: (.comparisons | length), invalid_spec_error, last_spec_unchanged}' "$evidence_dir/strapped-preview.json"
```

To audit coverage, compare the exact tool names in the pinned contract, the Rust
report, and the 48-row evidence table. A passing command produces no diff.

```sh
jq -r '.[].name' clo_mcp/clo_tool_contract.json | LC_ALL=C sort > "$evidence_dir/pinned-names.txt"
jq -r '.tools | keys[]' "$evidence_dir/rust-tool-calls.json" | LC_ALL=C sort > "$evidence_dir/called-names.txt"
sed -n '/^## 48 项逐项记录/,$s/^| `\([^`]*\)` |.*/\1/p' docs/verification/clo-mcp-acceptance.md \
  | LC_ALL=C sort > "$evidence_dir/evidence-names.txt"
diff -u "$evidence_dir/pinned-names.txt" "$evidence_dir/called-names.txt" &&
  diff -u "$evidence_dir/pinned-names.txt" "$evidence_dir/evidence-names.txt"
```

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

Copy this record for each behavior case. Use `N/A` for an inapplicable
assertion and give its reason. Keep missing applicable evidence visible.

```text
Tool and case:
Rust commit, reference MCP version, CLO version and bridge status:
Fixture and SHA-256:
Exact MCP arguments and any setup calls:
Expected result, state change or retained state, and artifact checks:
Official observation and evidence file:
Rust observation and evidence file:
Comparison, numeric tolerance with units if used, and verdict:
```

#### Read the evidence and reproduce a matched case

Report three independent results: 48 pinned tool definitions, 48 representative
Rust calls, and case-level behavior on both backends. Derive the matched-tool
total from the case records only after every applicable case for a tool has a
passing verdict. Keep failed, blocked, and unrun cases visible.

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

#### Worked example from a live CLO scene

Use independent copies of the two-colorway ZPRJ from the
[Boat evidence record](../../../docs/verification/clo-mcp-acceptance.md#boat-原生多色组导出基准).
Confirm both starting archives match the recorded SHA-256 and both scenes
report active index `1`. Use a dedicated CLO session and a writable output
directory on each host. Boat file paths are resolved on the CLO host. For a
forwarded bridge, set `CLO_MCP_PORT` to the local forwarded port and confirm
`clo_status.bridge` using the same reference command.

| MCP calls in order | Expected observations on both backends |
| --- | --- |
| Open the fixture, select active index `1` with `clo_api_call`, then call `scene_export` with `colorways: true` and a GLB target | Two GLBs with `_Original` and `_Alternate` suffixes, in that order. |
| Call `scene_save` to a fresh native archive with `thumbnail: false`, then repeat the export for fresh GLB and glTF targets | One unsuffixed GLB and one glTF with its BIN companion. OBJ still produces both colorways. |
| Call `clo_api_call` with `name: "utility_api.SetCurrentColorwayIndex"` and `args: [1]`, then export GLB again | Both colorway files return. `scene_info.current_colorway` is still `1`. |

Compare the ordered filenames separately from their host paths. Record the raw
MCP results, then inspect actual files when claiming artifact compatibility.
For the Rust replay, use the ZPRJ and ZPAC checksums in the evidence record:

```sh
CLO_REORDERED_ZPRJ=/absolute/path/to/multi-colorway-colored.zprj \
  CLO_REORDERED_ZPAC=/absolute/path/to/multi-colorway-simulated.zpac \
  python -m clo_mcp.smoke_compat --verify-native-colorway-refresh
```

The Rust replay also checks the stored GLB material color and negative controls
for invalid selection and a false refresh. Label those assertions as Rust
regression evidence unless the native run has matching observations.

A native file case needs a writer/reader matrix. Save to separate paths on both
backends, then reopen each file with both readers. For each of the four
writer/reader pairs, assert that the expected panels, seams, materials or
avatars are actually present. Check the file contents or exported geometry as
applicable. An open response can report success even when the expected scene
data is missing. The [same-name texture case](../../../docs/verification/clo-mcp-acceptance.md#同名贴图的组合写入)
applies this matrix to two colorways and checks the archive PNG bytes and the
decoded RGBA bound to each exported GLB primitive.

For an existing native scene with unchanged topology, run the continuation replay
on the shared CLO host with a fresh `CLO_MCP_STATE` used by both servers:

```sh
export CLO_SCENE_OPEN_PROJECT=/absolute/path/to/multi-colorway-colored.zprj
export CLO_SCENE_OPEN_GARMENT=/absolute/path/to/multi-colorway-simulated.zpac
unset CLO_RUST_CHECKPOINT_DIR
python -m clo_mcp.smoke_compat --compare-live-resumed-native-save
```

Verify 12 writer/reader cases across ZPRJ, ZPAC and checkpoints. Each reader
must recover its writer's state, rest coordinates, triangles and saved pose.
Require `max_position_error_mm < 0.001` for each case and
`remesh_rejected_without_changes=true`. The Rust-side assertions pack the
pre-save vertex coordinates as native 32-bit floats, require exact archive
readback and check that archive members outside the garment records remain
byte-identical. The official reader captures another ZPAC for independent
mesh inspection. Use the fixture hashes and scope in the
[continuation evidence](../../../docs/verification/clo-mcp-acceptance.md#已有原生工程续算后的再次保存).

#### Additional verification recipes

Set `CLO_MCP_OFFICIAL_COMMAND` to the real CLO MCP stdio command as a JSON string array
to include the official backend smoke. Run it with the CLO 3D bridge connected.
`tools/list` is checked against the contract, including input and output schemas.
The Rust fixtures also exercise selected scene, image, export, failure, and session flows.

Run `python -m clo_mcp.smoke_compat --compare-control-guide` with
`CLO_MCP_OFFICIAL_COMMAND` set to verify the shared guide workflow without
starting CLO. The report preserves each backend's instructions and verifies
the route from API discovery and invocation through UI inspection, action,
and state readback.

Run `python -m clo_mcp.smoke_compat --compare-live-bridge-lifecycle` from a
dedicated CLO session. Start with the official bridge up. Set
`CLO_SCENE_OPEN_PROJECT` to the shared two-panel fixture and `CLO_MCP_STATE`
to a fresh writable directory. Verify that repeat launch describes readiness,
invalid arguments leave the bridge online, stop leaves the app running, and
scene/API/Python calls fail while offline. Check that checkpoint listing stays
available. When the script prints the native restart prompt, restart the
bridge from inside CLO within 60 seconds. On the Boat reference installation,
reload the bridge's packaged operations after starting it and confirm
`scene_info` is available. Confirm the same process returns and both the scene
summary and detailed panels match the pre-stop state. On Rust,
`clo_launch(with_bridge=true)` restarts the embedded bridge. On the official
backend an already running CLO instance returns the native restart instructions.
Record a native process exit or restart timeout as a failed recovery case.
The first launch from a closed official application is a separate case.

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

Use `--compare-native-fabric-list` with `CLO_FABRIC_LIST_FIXTURE` pointing
to the saved two-panel project with assignments `[1, 0]` and two fabrics.
Open the same project in the dedicated native session first. Compare full
`fabric_list` results after rename, two recolors, deletion of the panel
assigned to index 0, and both clear modes. The selected index can be absent
from the listing. After a keep-avatar clear, create a new panel and read
back its assignment to the selected fabric. The comparison restores the
starting file in both sessions. For Rust-specific map and material-state
checks, read the saved `.clorust` records rather than the public catalog.

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
The case includes a 524-point boundary and checks visible internal paths
against intentionally hidden shapes and baselines. Run
`--compare-saved-snapshot-2d` with the official 2D pane visible and the
saved project accessible to both servers. See the
[CLO window layout guide](https://support.clo3d.com/hc/en-us/articles/115000428988-Window-Layout).
For Wine with a Linux MCP host,
map the shared drive and normalize the bridge's mixed path separators before
the official image loader reads the PNG.

Set `CLO_SNAPSHOT_2D_GEOMETRY=triangles` with the two-triangle
`multi-colorway-colored.zprj` to check 102 points along its edges, transparent
interiors, and unchanged scene readback. Set `CLO_SNAPSHOT_2D_CURVES` to a
directory containing `snapshot-layout-curved.zpac` and
`layout-curved-official.png` to check seven anchor points on the third
panel's outline and the internal curve on Front. Set `CLO_SNAPSHOT_2D_OUTPUTS`
to retain PNGs. Geometry is compared after removing canvas scale and translation.
For the empty native scene, acknowledge the “There are no patterns” dialog
and check that the tool returns one text block without writing a PNG.

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

Run `--compare-native-snapshot-views` with the official 3D pane visible.
Set `CLO_SNAPSHOT_3D_FIXTURE` to the same
`simulation-stage-next.zpac` file on both hosts and
`CLO_SNAPSHOT_3D_NATIVE_OUTPUT` to a writable native directory for the
thumbnail check. On Boat, map the bridge's returned PNG paths to the shared
drive before the MCP image loader reads them. Set `CLO_SNAPSHOT_3D_OUTPUTS`
to retain the nine PNGs per backend.

Compare order, default and mixed-case aliases, garment visibility and the
front, back and side silhouette aspect ratios within 0.01. Check that
`fit` and `focus` preserve the requested direction when nothing is selected,
including SDK presets 9 and 7. On an invalid first or later view, compare
the destination PNG list and the subsequent thumbnail with the last valid
view. Run `cargo test --locked edge_on_snapshot_keeps_a_one_pixel_profile`
to verify an exact planar panel still yields an edge profile in four views.

Run `--compare-live-garment-workflow` with a live CLO bridge,
`CLO_GARMENT_LIVE_FIXTURE` set to the shared two-panel
`multi-colorway-colored.zprj`, and the same Avatar and Fabric libraries.
Set a shared disposable `CLO_MCP_STATE` with the real Mia measurement table
from Avatar Editor in `measurements/Mara_09_eye_Joint.json`. Reuse the
official scene calibration under `calibration/Mara_09_eye_Joint.json`
or let the official build generate it before the first Rust build.
Check that an invalid draft keeps the newly loaded avatar and leaves the
existing panels and history intact. Compare a live-measurement preview using
that avatar, and read back the unchanged scene and history. Delete the first
source panel before appending six new pieces so a gap in Rust's panel IDs exercises assignment
by scene index. Match the complete append report and read back geometry,
material assignments and 14 seam group memberships. Rebuild with no steps
and then with one step, comparing the reports except elapsed time and the
checkpoint path. Restore each artifact through `checkpoint_restore`, check
the checkpoint becomes the active project and the current fabric resets to 0. The observed official artifact is ZPRJ and
the independently authored Rust artifact is CLORUST. Compare a bad
rebuild, an invalid view after a successful build, history after reopen,
and an empty fresh MCP session. Simulation geometry needs a separate
comparison.

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
point deletion and unfolding in a portable scene. Reopen each edited
scene and verify that unaffected sewing uses the new line index, the
elastic and shirring settings follow the surviving lines, and a seam
spanning a removed line is discarded. Check that an invalid edit leaves
the surviving sewing unchanged.

Run `--verify-symmetry-sewing` with both values of `with_sewing`.
Check that self-sewing is copied to the mirrored panel when requested while
sewing to another panel remains on the original. Reopen and verify panel
IDs, reflected internal rest coordinates and preserved native seam groups.

Use `--compare-live-pattern-ops` with a live bridge and
`CLO_PATTERN_OPS_FIXTURE` pointing to the shared two-panel
`multi-colorway-colored.zprj`. Start each request on the same scene. Compare
the response, panel order, names, fabric, layer, particle distance, seam
members and ordered outline coordinates. Check copied names and positions,
self-sewing, numeric strings, default and invalid arguments, the order
after unfolding, and no changes after rejected requests. Deleting an outline
line removes the whole piece. Deleting a triangle corner also removes the
piece. An out-of-range deletion returns success with the scene unchanged.
Invalid unfold indices return `ok=false`. Coordinate tolerance is 0.001 mm
for the native single-precision geometry. Verify curved folds, internal-line
deletion and post-simulation behavior separately.

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
scene, then advance one simulation step. Verify that a duplicate reports
success without adding or changing sewing, and that an out-of-range edge
reports false. Set `CLO_DISPLAY_FIXTURE` to check
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

Use `--compare-native-pattern-delete` with `CLO_PATTERN_DELETE_FIXTURE`
set to the same disposable native project open in CLO. The fixture contains
two panels and one seam group. Compare the exact public replies for empty,
named, duplicate, and descending batches. A missing name aborts the batch.
An out-of-range signed 32-bit index contributes to the reported deletion
count without removing a panel. For overflow, check the state left by the
descending order: a positive overflow occurs before valid indices, whereas
a negative overflow can leave earlier deletions applied. Also check typed
`DeletePatternPiece` responses, the unaffected seam group, and the project,
avatar and colorways readback. The comparison reopens the original project.

Run `--verify-new-panels-keep-drape` to create, copy and mirror panels
beside settled cloth. Verify the OBJ contains both posed and newly
placed panels, then reopen and assemble the new meshes at zero steps.
An invalid outline leaves the current UI snapshot usable. If the outline
is valid but the requested arrangement is missing, the newly named panel
remains and the previous UI snapshot is rejected.
Check the original mesh, velocities and step count, then resume simulation.
Set `CLO_DISPLAY_FIXTURE` to also verify a new panel beside archived meshes.

Use `--compare-native-pattern-creation` with `CLO_PATTERN_CREATE_FIXTURE`
set to the same saved project open in native CLO. Use the two-panel fixture
with an avatar, two fabrics and a seam group. Compare generated names by
format and verify that the newly created pieces have distinct names. Check duplicate explicit names,
two-corner curves, the active fabric and untouched original panels. Confirm
that an invalid arrangement leaves the named piece, while short input and
an unsupported point type leave the scene intact. Test both positive and
negative out-of-range arrangement indices and compare the full arrangement
records on valid calls. The Rust curve case also saves, reopens and exports
a simulated mesh. A native attempt with three coincident points may open a
remeshing dialog on the dedicated desktop, so verify its eventual reply in a
manual case.

Use `--compare-native-pattern-point-edits` with `CLO_PATTERN_EDIT_FIXTURE`
set to the same disposable archive on both hosts. Use the fixture with
`Front`, `EndpointControls` (Bézier), and `EndpointSpline` (spline).
Keep the starting native project available to reopen when the case ends.
Compare absolute, relative, repeated, zero-offset, and partially committed
moves. Check invalid point indices leave the scene unchanged. Compare brief
and detailed lists and the full edited pattern records, including baseline
points and lengths. Use 0.01 mm for numeric geometry and compare indices,
arrangement labels, and orientation exactly. Preserve both readbacks and
use the exit status to determine the verdict.

Run `--verify-native-two-corner-archives` with
`CLO_NATIVE_TWO_CORNER_FIXTURES` set to the native capture directory. It
contains `two-corner-curve.zprj`, `two-corner-curve.zpac`, the official GLB,
and `reopen.json` with the `scene`, `curve` and `seams` readback for each
suffix. Compare outline endpoints and require both edge lengths to be
within 0.01 mm of official CLO.
Rename the curve, save and reopen each native format, then run simulation
at zero steps. The saved positions, topology and UV coordinates should stay
intact. Compare GLB positions and triangle indices to the official export.
Set `CLO_RUST_TWO_CORNER_OUTPUTS` to a writable directory to retain the Rust
archives for re-opening in native CLO. Confirm the renamed curve, native
curve metrics and original seam group.

Run `--verify-native-curve-measurements` against the same fixture directory
with `curve-measure-reference.zpac` and `curve-measure-native.json`. The
JSON holds the original controls and official `pattern_info` readback.
Compare seven Bézier, spline and mixed outlines in two ways: create them
through `pattern_create_from_points` and reopen the native ZPAC. Require
endpoints within 0.001 mm and lengths and bounding-box spans within 0.01 mm
in both cases. Record each delta and the maximum for each metric.

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
texture binding.

#### Compare native planar texture transforms

In official CLO, open the same two-panel ZPAC fresh for each case. Set the
active colorway to 0. Use `utility_api.GetMaterialTextureTransformations`
and `SetMaterialTextureTransformations` with fabric 0, colorway 0 and front
material options. Export GLB, glTF with its BIN, OBJ, and FBX through
`scene_export` with `colorways: false`. Save the ZPAC for each export set.
The [capture settings and archive hashes](../../../docs/verification/clo-mcp-acceptance.md#boat-原生平面贴图映射)
identify the reference files.

```sh
CLO_NATIVE_TEXTURE_TRANSFORM_EXPORTS=/absolute/path/to/capture \
  python -m clo_mcp.smoke_compat --verify-native-texture-transforms
```

The Rust replay opens each captured archive and calls `scene_export` with the
same options. Compare OBJ UVs directly and reopen FBX for its UVs. For GLB and
glTF, compare the shared accessor, per-map scale and offset, and effective UV
coordinates. The raw glTF millimetre tolerance is 0.00005. The UV tolerance
after mapping is 0.00002. Check the scene state after the exports.

Run `--verify-native-colorway-refresh` with the paired ZPRJ and ZPAC fixtures.
A native save makes subsequent GLB and glTF exports use the active colorway,
even when the call requests all colorways. Verify the unsuffixed primary file
and glTF BIN. Check that OBJ still exports both colorways. Reselect the active
colorway, call `UpdateColorways(true)`, or edit a fabric to restore grouped
exports. An out-of-range positive selection and `UpdateColorways(false)` leave
the export mode intact. Switch between both colorways and verify the live
material edit remains attached to its original colorway. Reopen the saved
archive with `scene_open` and with the default Python `ImportZprj` or
`ImportZpac` option object. On each route, confirm grouped GLB output after
reopening, the active index, and both archived RGBA values. Record the
preceding save and any colorway refresh calls in the evidence.

Switch the archived active colorway from index `1` to `0`, save in the
same native format and reopen. Verify the active RGBA and both indexed
materials, GLB output after saving, and the switch back to index `1`.
With distinct source textures, open the Rust-written archive in official CLO
and compare the active texture with the official export from the same source.
Recolor only the active colorway, save and reopen, then compare the edited and
untouched RGBA values. Recolor both colorways before saving again and confirm
each keeps its own edit. Include the textured ZPAC to check the base texture
binding and desaturation after reopening. With the textured ZPAC, replace the
existing base image in `Alternate`, switch to `Original`, and clear its base
image. Save, reopen in both backends, and compare which GLB files contain a base
image and its decoded pixels. Check that the replacement retains its source
label. Add a base image back to the cleared colorway and verify both colorways
after reopening. Repeat with a non-square PNG with explicit pixel density.
Compare the stored physical size, the UV bounds and the decoded image against
official CLO. Add normal, roughness, displacement, opacity and metalness to the
active colorway. Check the normal mode and strength, retained displacement
settings, original labels and decoded images after official reopen. Remove a
staged PNG before attempting another save and verify that the previous archive
remains intact.

Set `CLO_SHARED_COLORWAY_FIXTURE` to the official three-colorway archive
created by copying `Original` from a textured two-colorway ZPAC. Compare
the original, copied and independent materials. Edit the copied colorway
and verify that the original shares its new RGBA and texture, while
`Alternate` keeps its original material. Reopen in official CLO and compare
the GLBs for all three colorways after replacement, recolor and clearing.

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
and default `scene_open` mode. Both modes replace the AVT in the scene, as
do a default typed `ImportAvatar` call and its Python equivalent.
Compare panels, fabrics, positions and velocities after these operations.
Verify an import of a missing AVT preserves the avatar list.
Delete the primary avatar from Python and check that the remaining avatar is
promoted and the saved positions and velocities stay identical. Verify an
invalid batch leaves the avatars intact. Set `CLO_AVATAR_LIBRARY_FIXTURE` to an
`Avatar` folder containing `Female/FV2.1_Mia.avt` to check that alias resolution
prefers the standard library location over a same-name backup.

Run `python -m clo_mcp.smoke_compat --compare-live-avatar-workflow` with a live
CLO bridge, the configured official MCP command, `CLO_AVATAR_PROJECT_FIXTURE`
pointing to the shared two-panel `multi-colorway-colored.zprj`, and
`CLO_RUST_AVATAR_DIR` pointing to the same Avatar library path as official CLO.
On Boat, both MCP processes can resolve that library and the project under
the same Wine `C:/` mapping. The verifier calls `avatar_load` with its defaults,
a mixed-case alias, a file name, and both values of `replace`. It compares
the whole response, the ordered library and alias lists, and the active
names in both `scene_info` and `avatar_list`. It checks 110 ordered
`arrangement_points` names for Mia and Luka and reads back the panels,
sewing, fabrics, and current colorway. The native AVT import leaves one
avatar even with `replace=false` in this fixture. Separately reopen Luka by
the shared file path via `scene_open` in both modes and check the same
avatar replacement with the garment intact. A missing avatar and a
non-string argument leave all tracked state unchanged. Clearing the scene
returns zero names and points. Record absolute paths, other avatars, and
measurement source and value comparisons as separate cases.

Run `--verify-simulation-reset-quality` to compare arranged meshes before
simulation and after reset. Exercise the public tools and corresponding
`utility_api` calls, checking that both use the same quality setting,
reset keeps the sewing and per-panel particle distance, and opening a
saved scene in a fresh session can recover the saved quality before reset.
Check that a requested CPU quality takes effect on an empty-scene success
or before a nonrepresentable particle distance and is used by the next call. Verify that the
nonrepresentable distance leaves the prior mesh and per-panel distances intact, that
rejected quality 3 leaves the previous setting, and that the public response
reports the requested steps and seconds to two decimal places.

Run `--verify-native-simulation-reference` with `CLO_SIMULATION_REFERENCES`
pointing to the directory containing the official call captures
`native-simulation-probe.json`, `native-simulation-limits.jsonl` and
`native-simulation-error-stage.json`, plus
`native-simulation-archives/simulation-stage-*.zpac`. The fixture is
`multi-colorway-colored.zprj` with two sewn triangles and no avatar.
Capture initial, zero-, three- and five-step poses, and poses after a
zero-step remesh to 32 mm and a subsequent zero-distance request. Replay
these operations and both reset paths. Verify rest positions within
0.001 mm and native topology exactly. Record the maximum paired seam gap
at each step on each solver. Check distances 0, 2, 501 and -5 mm,
CPU qualities 0–2, ignored quality values -1 and 4, and the
`GetSimulationQuality` readback. Check that an empty scene accepts 0, 2
and 3001 steps. Verify that a negative step count returns a tool error
and preserves the quality and distance applied earlier in that request.

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
by index. Check the empty-string result for out-of-range 32-bit indices and
rejection of oversized integers, strings, floats, and null. Check empty and
missing search and doc results, search limits, Python stdout, and the six
supported fields on `ImportExportOption`. Release a handle, confirm repeat
release is a no-op, construct a fresh handle, and release it in a mixed batch.

With the official command configured, open the same fixture in a dedicated
CLO session. Use a disposable project with at least one named pattern. The
case temporarily renames the first pattern through `clo_api_call`, reads it
through `pattern_info`, and uses `clo_run_python` to rename it again before
raising an exception. Verify the second rename survives the error and a later
Python call can read it. The test restores the original name and compares
`scene_info` with the starting scene without saving the changes. A down
bridge or a different project is `BLOCKED` with exit 2.

With the same fixture open on a dedicated live CLO bridge, run
`python -m clo_mcp.smoke_compat --compare-python-response-timeout` from the
verification environment. Both adapters should return an MCP error around the
one-second response deadline, before the script finishes its 2.5-second wait.
The next scene request should read the second rename in each backend, including
when the script raises after that rename. Check a follow-up Python request and
restore the starting scene. Before changing session teardown, also close a
Rust MCP session after a script starts and the response times out. Use a
temporary marker file to verify the script cannot write after session close.
The native engine's direct timed request is independently covered by
`cargo test --locked completed_api_call_past_deadline_stops_the_script`.

Run `--verify-runtime-export-workflow` to verify Rust discovery by module and
keyword, catalog pagination, qualified and unqualified documentation lookup,
and invocation of the asset path, layout PNG, 3D thumbnail and FBX API entries.
The case compares the files written by direct and Python mode-0 layout
exports with `snapshot_2d` and checks their empty-list SDK receipts. It checks
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
project becomes that checkpoint, and Rust restores its saved state. Omit the name and
then pass an empty name with existing timestamp files in the directory. Check
that both calls use distinct suffixed names and produce native projects
visible to both adapters. Check that
same-name `.clorust` writes preserve the native archive. A missing checkpoint
preserves the scene. Restoring an unsigned file as a native checkpoint
replaces the scene with an empty project. A live CLO restore requires the bridge.

Run `--compare-live-checkpoint-roundtrip` with a live bridge, the same
`multi-colorway-colored.zprj` on both backends, and an empty shared
`CLO_MCP_STATE/checkpoints` directory. Unset `CLO_RUST_CHECKPOINT_DIR`.
On Boat the Rust and official MCP processes can run on Linux and share the
Wine drive through a relative `C:` mapping. Save a Unicode name from CLO,
restore in Rust, change the front panel's name and save the same name.
Check the native file changes, the portable shadow is absent, and CLO
can restore the Rust revision. Repeat the overwrite in reverse, then
cross-restore two automatically named checkpoints. Compare the full list,
the active project path, materials, colors and seam groups, and vertex,
triangle and UV readback from each archive. A missing name leaves the scene
in place.

Run `--compare-live-damaged-projects` with a live bridge and a fresh
`CLO_MCP_STATE` directory visible to both adapters. Set
`CLO_SCENE_OPEN_PROJECT` to the shared two-panel ZPRJ and
`CLO_SCENE_OPEN_AVATAR_PROJECT` to the shared project with an embedded avatar.
Unset `CLO_RUST_CHECKPOINT_DIR`. Compare nine damaged ZPRJ fixtures and one
missing file through append, replace, and checkpoint restore. For damaged
files, append reports success and preserves the existing garment and path.
Replace and restore report success, adopt the bad file's path, and clear
the garment. In both modes, independently stored avatars are imported.
All three calls against the missing file return an error and preserve the
scene. Check detailed panels and seams after every call, and verify that
the input fixtures remain byte-identical.

Run `--compare-live-damaged-garments` with a live official bridge, a fresh
`CLO_MCP_STATE`, the shared `CLO_SCENE_OPEN_GARMENT` and
`CLO_SCENE_OPEN_AVATAR_PROJECT` fixtures, and the CLO window visible on the test
host. Acknowledge the native damaged-file dialog when the comparison prints its
prompt. Compare append and replace for an unsigned file, a header-only archive,
a missing ZIP footer, a missing PAC, a PAC with no readable garment records,
and a missing file. An unreadable archive preserves the scene in both modes.
An open ZIP with a missing or wholly unreadable PAC preserves the scene on
append. On replace it clears the garment, resets the project path and colorway,
and retains the avatar. A complete central directory with only the end record
missing should recover the same garment as the intact ZPAC. Compare saved
mesh positions, topology and UV after reopening it in Rust. Missing files
return an MCP error and preserve the scene. Compare the response and the
scene, panel and seam readback on both backends.

Run `--compare-live-empty-native-save` with a live bridge, a fresh shared
`CLO_MCP_STATE`, and `CLO_SCENE_OPEN_AVATAR_PROJECT` pointing to the project with
a saved avatar. Unset `CLO_RUST_CHECKPOINT_DIR`. Use an empty scene and a scene
cleared with the avatar retained. For ZPAC save and export, check the returned
path, parent directory, absent output or preserved existing bytes, and
unchanged scene. Acknowledge the official no-pattern dialog when prompted.
For ZPRJ save with and without thumbnails and for named checkpoints, test
both writers and both readers. Confirm the archive contains an OSW workspace,
the requested avatars, and no embedded ZPAC. On reopen check zero panels and
seams, the avatar names, default colorway and one default fabric.
Verify a new Rust panel can be saved after an empty project save.

Run `--compare-live-recovered-zpac-save` with a live bridge, a fresh shared
`CLO_MCP_STATE`, `CLO_SCENE_OPEN_GARMENT` pointing to the saved-pose ZPAC, and
`CLO_SCENE_OPEN_PROJECT` pointing to the two-panel ZPRJ. Unset
`CLO_RUST_CHECKPOINT_DIR`. Remove the ZIP end record from a disposable copy.
Open it directly or append it to the project. Each backend writes a ZPAC,
a ZPRJ and a named checkpoint. Reopen every output in both backends.
Compare scene structure, fabric and colorway data, and archive integrity.
For Rust outputs, compare pose coordinates, triangles and UV to the intact
source case. The directly saved garment payloads must match the intact file
byte for byte. Change disposable source copies after opening them and
verify that saving fails without changing the output file or live scene.

Run `--compare-live-scene-open` with a live bridge and a fresh disposable
`CLO_MCP_STATE`. Set `CLO_SCENE_OPEN_PROJECT` to `multi-colorway-colored.zprj`,
`CLO_SCENE_OPEN_GARMENT` to `multi-colorway-simulated.zpac`, and
`CLO_SCENE_OPEN_AVATAR_PROJECT` to `scene-clear-avatar-fixture.zprj`. Use the
same files on both backends. If CLO asks about pose and size on the test
scene, choose **Maintain Current Avatar's Size & Pose**. Compare return
fields, assigned and stored fabric indices, full fabric list, seam groups,
avatar names and active project. Empty append keeps the destination
default colorway. ZPAC replacement retains the avatar and clears the
active project path. Use a missing path, an unrecognized mode and a
short file with no ZPRJ signature. The malformed-signature append leaves
the scene unchanged. The malformed-signature replacement reinitializes
the scene and takes the requested file name. A recognizable header with
invalid or unsupported embedded records requires separate verification.

Set `CLO_NATIVE_APPEND_FIXTURE=/path/to/saved.zprj` and run
`--verify-native-append` with a native project that has fabrics, seams, an
avatar and a complete saved pose. Exercise typed `ImportZprj` and Python
with `bLoadGarment` or `bLoadAvatar` disabled. Check that partial replacement
and append affect the requested component, and that a corrupt avatar-only
import preserves the receiving scene. Check that full append preserves original project
identity, shifts new panel and fabric indices, and restores both saved meshes
and embedded avatars. Save and reopen the combined project. Test invalid input
in both modes. Append to an empty scene and confirm the receiving default
fabric and colorway remain. Save and reopen the composed scene as CLORUST.
To check native garment passthrough, replace-open the original ZPRJ and
export its ZPAC, then append that archive and verify both saved poses.
Use the same native fixtures and a fresh `CLO_MCP_STATE` for
`python -m clo_mcp.smoke_compat --compare-live-composed-zpac`.
Set `CLO_COMPOSED_ZPAC_CASE=empty` to isolate the empty-receiver ZPAC case
or `CLO_COMPOSED_ZPAC_CASE=empty_project` for the ZPRJ source.
Export through `scene_export` and confirm the live scene stays intact.
Reopen each public output in both backends. The native export omits the
unused default fabric, so both panels change from index 1 in the live
scene to index 0 after reopening. Require the first source material
variant even when the source was saved with its second colorway active.
Read back fabrics, colorways and seams. Compare saved mesh coordinates,
triangle indices, 2D rest coordinates and sewing endpoints via the reopened
Rust scenes. The `nonempty` case appends a saved native garment to a
native project. Require four panels, two independent fabric slots and two
seam groups with endpoints on the corresponding panels.
Set `CLO_COMPOSED_ZPAC_CASE=palette` to cover five more combinations with
different names, active indices or palette lengths. Compare the exact f32
material colors with `fabric_api.GetFabricPBRMaterialBaseColor` across all
slots before export and after reopening. The receiver keeps its own names
and active index. Source variants are copied by ordinal, and extra receiver
slots use the first source style chip. Recolor the imported material in
`short_source` after reopening, then check that both linked slots update.
The three-argument color query uses the ordinal in the assigned-material
list. After appending into a fresh scene, query ordinal 0 and verify the
appended fabric's color while the garment still uses catalog index 1.
Set `CLO_COMPOSED_UNUSED_FABRIC` to a native ZFAB fixture, then run with
`CLO_COMPOSED_ZPAC_CASE=unused` for four cases with unused materials.
Compare both the stored panel assignments and the complete fabric catalog
before exporting. The catalog lists primary materials before appended
garment groups. Read the selected catalog row after adding a ZFAB and
after appending. Export twice and check the live state retained its indices.
Reopen each first output in both backends. Require compact assignments and
the same two used materials, stored meshes and sewing endpoints.
Run `python -m clo_mcp.smoke_compat --compare-live-composed-zprj` with these
same fixtures plus `CLO_SCENE_OPEN_AVATAR_PROJECT`, with
`CLO_RUST_CHECKPOINT_DIR` unset and a fresh `CLO_MCP_STATE`.
The two cases start from a loaded native ZPRJ, with a sparse fabric catalog
or with an embedded avatar. After append, export ZPAC and then save the
ZPRJ twice at the same path. Require transition to the new active project,
unchanged in-session assignments, and a native checkpoint which restores
by name. Reopen both writers' saves and checkpoints in both backends.
Compare catalog, colorways, sewing endpoints and stored meshes. For the
avatar case, compare the embedded AVT bytes in the Rust output against
the receiving ZPRJ and confirm that official CLO can reopen it.
Set `CLO_ZPRJ_AVATAR_FIXTURE` to `scene-clear-avatar.avt` and
`CLO_ZPRJ_REPLACEMENT_AVATAR` to `MV2.1_Luka.avt` to extend the same
comparison to adding an AVT, replacing a different embedded avatar,
and appending a project that contributes a second avatar. Compare the
reopened avatar names and their order. Verify original AVT bytes for
unique avatars. When a second instance reuses the first avatar archive,
check that the packed instance has a distinct identity and equal mesh
positions and triangles. Include the textured avatar project as an
append source and verify its case-sensitive texture names and digests
after writing the destination project.
For the Rust source-file guard, load a disposable AVT copy into the
avatar-free ZPRJ. Change the AVT copy before saving over another disposable
ZPRJ and confirm the error preserves both the target bytes and the live
scene. Reload an intact AVT copy and save successfully. Remove the loose
AVT copy, save the resulting embedded project again, then append a
second AVT-bearing project and verify both avatars after reopening.
Run `--compare-live-authored-poses` with the native bridge up and a fresh shared
`CLO_MCP_STATE`. Leave `CLO_RUST_CHECKPOINT_DIR` unset. Solve the newly created
sewn panels before writing ZPRJ, ZPAC and a named checkpoint. Cross-open each
writer's files in both readers. Compare matching meshes vertex by vertex.
If CLO remeshes on load or re-save, compare world positions by interpolating
in rest-space triangles. Record the maximum interior error and report the
rest-space offset and world-space error for projected boundary samples.
Resume the Rust-written ZPRJ in both solvers and verify additional movement.
For the existing portable-state check, open the Rust pose saved before
native export and save it as ZPRJ. Change a stored triangle in another
portable copy and verify native save rejects it while preserving the target
archive and live scene.

Run `--compare-live-authored-seams` with the native bridge up and
`CLO_MCP_STATE` set to a fresh shared directory. Leave
`CLO_RUST_CHECKPOINT_DIR` unset. Create two panels and sew two whole edges
with opposite directions, including one curved edge. Cross-open the ZPRJ,
ZPAC, and named checkpoint from both writers. Check the group names and
membership against the writer's in-memory scene. Decode both readers'
saved output to verify exact panel slots, edge indices, directions, and
ratios. For Rust's reopened project verify that simulation reconstructs
the stitch constraints and reduces the maximum seam gap in 15 steps.
For the official simulation check, open each writer's ZPRJ in CLO and save a
ZPAC before and after `simulate(steps=15)`. Read the panel positions from
each saved archive. Measure the distance between the two panels' mean vertex
positions and verify that both projects draw together.

Run `--compare-live-authored-curves` with the native bridge up and
`CLO_MCP_STATE` set to a fresh shared directory. Create spline, Bézier and
mixed-control panels from scratch, save ZPRJ and ZPAC on both backends, and
reopen each file with both readers. Compare edge order and endpoint
coordinates within 0.001 mm, and edge lengths and bounds within 0.01 mm.
After each reopen move one Bézier endpoint and one spline endpoint using
`pattern_edit_points` and compare the recalculated measurements.

Run `--compare-live-authored-projects` with the native bridge up and
`CLO_MCP_STATE` set to a fresh shared directory. Leave `CLO_RUST_CHECKPOINT_DIR`
unset. Start each backend with a cleared scene and create the same editable
panels through `pattern_create`. Compare two writers and two readers for a
ZPRJ save, ZPAC export, and native checkpoint restore. Include distinct panel
names, a concave outline, a move and recolor after the first save, and GLB export
after reopening. Assert ordered edge endpoints and lengths within 0.001 mm,
the material assignment and color, and nonempty exported mesh geometry. For
the moved project check that 3D bounds stay at the original placement within
0.001 mm. Check the recolored GLB base-color factors within 0.000001.

Run `--compare-live-garment-to-project` with the native bridge up.
Set `CLO_SCENE_OPEN_GARMENT` to the verified `multi-colorway-simulated.zpac`,
and `CLO_MCP_STATE` to a fresh directory writable by both backends. Leave
`CLO_RUST_CHECKPOINT_DIR` unset so checkpoints use the shared native format.
Load the ZPAC into a cleared scene using replace and append, then save, export,
or checkpoint as ZPRJ. Cross-open both writers' output on both readers and
compare the actual panels, seams, fabrics, RGBA and colorway state. Check that
replacement keeps the nested ZPAC bytes and that requesting thumbnails adds
three PNG views to the primary preview. Set `CLO_ZPRJ_AVATAR_FIXTURE` to
`scene-clear-avatar.avt` to add the avatar case. Verify the packed AVT matches
the input bytes and both readers report the same avatar name.

Run `--compare-live-texture-collisions` with Pillow installed, a fresh
`CLO_MCP_STATE`, and `CLO_TEXTURE_COLLISION_FIXTURE` set to
`multi-colorway-textured.zpac`. The verifier makes an incoming copy
with different PNG contents under the same archive filenames. Compare
identical-input ZPAC append, conflicting ZPAC append, and conflicting
append to a native ZPRJ. Check the archive payloads, the full fabric
readback, and the RGBA image bound to each of the four exported GLB
primitives in both colorways after cross-opening each writer's output.
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

Use `--compare-native-sewing` with `CLO_SEW_FIXTURE` pointing to the same
disposable `multi-colorway-colored.zprj` on both hosts. It starts with Front
and Back joined on edge 0. The case creates three panels, moves an endpoint,
and adds seams in both directions. Compare repeated and reversed requests,
edge and panel index boundaries, missing names, shared edges, group names,
memberships, and group removal after deleting the third panel. Compare the
format and uniqueness of generated `Sewing_` names and check their lookup.
Set `CLO_SEW_CONFIRM_SELF=1` for the same-edge case and dismiss the native
overlap warning on the dedicated desktop. To check directions against the
saved official groups, set `CLO_SEW_REFERENCE_ARCHIVE` to the captured
`sewing-live-probe.zpac`. The Rust run preserves directions and memberships
through portable save and reopen before deleting the panel.

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

For a disposable saved ZPRJ with fabric and seams, open it on the dedicated
official bridge and run `CLO_SCENE_CLEAR_FIXTURE=/path/to/fixture.zprj python -m
clo_mcp.smoke_compat --compare-native-scene-clear`. Repeat with a project
containing an avatar. The test restores the saved starting scene after the
checks. On separate hosts, map the official path to the byte-identical
local fixture as seen by the verifier. Record its SHA-256. Compare scene
fields, the clear reply, rejection without mutation, and a repeated clear.
Compare the stored fabric count via `GetFabricCount(False)` separately from
the count used by the scene summary. Confirm the default fabric on the
first new pattern after clearing.

Use `--compare-saved-arrangements` with the same saved project and a loaded
avatar. Compare the scene's avatar names, the arrangement count and every name
in order. Compare the full `GetArrangementList` records, including offsets,
orientation, and type. In Rust, set a point by name and by native index and
verify the per-panel readback excludes the list index. To use captured
reference rows, set `CLO_ARRANGEMENT_REFERENCE` to a JSON capture containing
`scene.project_path` and `arrangements.points`.

Run `--compare-live-avatar-measurements` with a live CLO bridge, the
official MCP command, the same Avatar library on both backends, and an
existing disposable shared `CLO_MCP_STATE` directory with no measurement
records. The Avatar Editor should be unopened for the mesh fallback case.
Load the six library aliases and compare each complete response for default
and forced refresh, including all 32 numeric fields, source, and avatar name.
Verify both backends reject a missing avatar and an invalid refresh argument.
Seed a measurement record for Mia with three known millimetre values. Both
backends must return the same cached response, bypass it on forced refresh,
and return it again without changing the file. The verifier removes that
record.

For the real cache, load Mia in the test CLO scene, then open Avatar Editor. With a
fresh shared `CLO_MCP_STATE`, run `--compare-live-avatar-editor-cache`.
Force an exact Mia reading through official MCP, verify it creates a cache
with millimetre records, then compare the full cached response with Rust.
Keep the cache SHA-256 in the evidence. Independent Rust computation of
the exact Avatar Editor values requires a separate comparison.

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
count remain intact. If the stored region mapping is incomplete or references vertices outside its
mesh, check that the avatar opens and uses the unclassified mesh estimate.
A valid render mapping can use different triangulation from the physical mesh.

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
