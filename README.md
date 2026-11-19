# CLO Rust

Native garment pattern and mesh diagnostics for Linux and macOS. The Cargo package installs a CLI named `pattern-native`.

| Commands | Purpose |
| --- | --- |
| `clo-import`, `draft-import` | Convert saved CLO edge/seam exports or FreeSewing draft data into a pattern project. |
| `audit`, `constraints`, `connectivity` | Check seam intervals and lengths, sample paired seam constraints, and compare connected garment pieces. |
| `mesh-metrics` | Calculate triangle strain and fold angles from a prepared mesh. |
| `avatar-source`, `avatar-import` | Compare local avatar assets and bake a saved GLB avatar pose to OBJ in millimetres. |
| `collision-avatar`, `collision-obj` | Check static body–cloth triangle intersections and optionally export VTK contact markers. |

Build and verify with Rust, Cargo, a C compiler for the native FBX reader, and Node.js 22 or later.

```sh
cargo test --locked
cargo build --release --locked
npm --prefix vendor ci
sh verify-linux.sh
sh verify-boat.sh
sh verify-avatar.sh
```

`verify-linux.sh` checks the Carlton seam fixtures and regenerates the Toni references. The Boat script checks synthetic connectivity, and collision examples along with the Toni round trip. The avatar script generates a synthetic skinned GLB for import and collision checks. The FreeSewing adapters pin Carlton and Toni at 4.10.2.

```sh
./target/release/pattern-native audit samples/Carlton外层.pattern.json
./target/release/pattern-native constraints samples/Carlton外层.pattern.json
```

Run `pattern-native` without arguments for command syntax. See [项目说明](项目说明.md) for fixture provenance and diagnostic limits. The geometry checks operate on saved inputs. Build the executable with Cargo.

## MCP and native releases

See [CLO Rust MCP](clo_mcp/README.md) for GitHub Release prewarming, the tool
inventory and integration with the CLO Rust Boat and CLO fit export skills.

The [CLO Boat fit review skill](.agents/skills/clo-boat-fit-review/SKILL.md)
adapts the local CLO DXF review workflow to the MCP or a Boat VM, including
evidence interpretation and prioritized pattern revision options.
