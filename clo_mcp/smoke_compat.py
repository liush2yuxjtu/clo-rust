"""Exercise the common MCP entry point on Rust and, when configured, CLO 3D."""

import argparse
import asyncio
import base64
import json
import os
import struct
import sys
from contextlib import asynccontextmanager
from pathlib import Path
from tempfile import TemporaryDirectory

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

from .compat import CONTRACT


@asynccontextmanager
async def open_backend(backend):
    env = os.environ.copy()
    env["CLO_MCP_BACKEND"] = backend
    server = StdioServerParameters(command=sys.executable, args=["-m", "clo_mcp.server"], env=env)
    async with stdio_client(server) as (reader, writer):
        async with ClientSession(reader, writer) as session:
            await session.initialize()
            yield session


async def inspect_contract(session):
    expected = {entry["name"]: entry for entry in json.loads(CONTRACT.read_text())}
    result = await session.list_tools()
    actual = {tool.name: tool.model_dump(mode="json", by_alias=True, exclude_none=True) for tool in result.tools}
    for name, entry in expected.items():
        assert actual[name] == entry, f"Tool contract changed for {name}"
    assert len(expected) == 48
    diagnostics = {
        "compare_line_art", "export_fit_maps", "fit_pressure", "fold_angles",
        "physics_debug_view", "physics_replay", "prewarm_runtime", "review_boat_batch",
        "strain", "stress", "verify_seam",
    }
    assert diagnostics <= actual.keys()


async def successful(session, name, args=None):
    result = await session.call_tool(name, args or {})
    assert not result.isError, f"{name} failed: {result.content}"
    return result


async def json_result(session, name, args=None):
    result = await successful(session, name, args)
    return result.structuredContent or json.loads(result.content[0].text)


def read_gltf_export(path):
    if path.suffix == ".glb":
        raw = path.read_bytes()
        assert raw[:4] == b"glTF" and raw[16:20] == b"JSON"
        binary_header = 20 + struct.unpack_from("<I", raw, 12)[0]
        assert raw[binary_header + 4:binary_header + 8] == b"BIN\0"
        return json.loads(raw[20:binary_header]), raw[binary_header + 8:]
    model = json.loads(path.read_text())
    return model, base64.b64decode(model["buffers"][0]["uri"].split(",", 1)[1])


async def check_material_export(session, directory, png):
    output = Path(directory) / "material.glb"
    fabric = await json_result(session, "fabric_apply", {
        "fabric": "cotton", "color": "#336699", "patterns": ["smoke-front"],
    })
    args = {"path": str(output), "colorways": True}
    result = await json_result(session, "scene_export", args)
    assert result == {"path": str(output), "result": [str(output)]}
    raw = output.read_bytes()
    model, _ = read_gltf_export(output)
    primitive = model["meshes"][0]["primitives"][0]
    assert model["materials"][primitive["material"]]["pbrMetallicRoughness"]["baseColorFactor"] == [
        0.2, 0.4, 0.6, 1.0,
    ]

    source = Path(directory) / "base.png"
    source.write_bytes(png)
    await successful(session, "fabric_set_texture", {
        "fabric_index": fabric["fabric"]["index"], "image_path": str(source),
    })
    for extension in ("glb", "gltf"):
        standalone = Path(directory) / f"textured.{extension}"
        await successful(session, "scene_export", {"path": str(standalone), "colorways": True})
        model, data = read_gltf_export(standalone)
        primitive = model["meshes"][0]["primitives"][0]
        coords = model["accessors"][primitive["attributes"]["TEXCOORD_0"]]
        assert coords["type"] == "VEC2" and coords["count"] == 3
        pbr = model["materials"][primitive["material"]]["pbrMetallicRoughness"]
        image = model["images"][model["textures"][pbr["baseColorTexture"]["index"]]["source"]]
        assert image["mimeType"] == "image/png"
        view = model["bufferViews"][image["bufferView"]]
        offset = view.get("byteOffset", 0)
        assert data[offset:offset + view["byteLength"]] == png

    obj = Path(directory) / "textured.obj"
    obj_args = {"path": str(obj), "colorways": True, "include_avatar": True}
    result = await json_result(session, "scene_export", obj_args)
    assert result["path"] == result["result"][0] == str(obj)
    assert len(result["result"]) == 2
    material_file = Path(result["result"][1])
    assert material_file.parent == obj.parent and material_file.suffix == ".mtl"
    object_bytes = obj.read_bytes()
    lines = object_bytes.decode().splitlines()
    assert f"mtllib {material_file.name}" in lines
    assert lines.count("usemtl material_0") == 1
    assert sum(line.startswith("vt ") for line in lines) == 3
    face = next(line for line in lines if line.startswith("f "))
    assert {vertex for vertex in face.split()[1:]} == {"1/1", "2/2", "3/3"}
    material_bytes = material_file.read_bytes()
    material_lines = material_bytes.decode().splitlines()
    assert "newmtl material_0" in material_lines and "Kd 0.2 0.4 0.6" in material_lines
    image_name = next(line.removeprefix("map_Kd ") for line in material_lines if line.startswith("map_Kd "))
    assert (material_file.parent / image_name).read_bytes() == png

    fbx = Path(directory) / "textured.fbx"
    fbx_args = {"path": str(fbx), "colorways": True, "include_avatar": True}
    files = set(Path(directory).iterdir())
    assert await json_result(session, "scene_export", fbx_args) == {
        "path": str(fbx), "result": [str(fbx)],
    }
    fbx_bytes = fbx.read_bytes()
    assert fbx_bytes[:27] == b"Kaydara FBX Binary  \0\x1a\0" + struct.pack("<I", 7700)
    companions = set(Path(directory).iterdir()) - files - {fbx}
    assert len(companions) == 1
    texture, = companions
    assert texture.suffix == ".png" and texture.read_bytes() == png
    assert texture.name.encode() in fbx_bytes
    forbidden_fbx = Path(directory) / "blocked.fbx"
    forbidden_fbx.mkdir()
    files = set(Path(directory).iterdir())
    rejected = await session.call_tool("scene_export", {**fbx_args, "path": str(forbidden_fbx)})
    assert rejected.isError and set(Path(directory).iterdir()) == files

    forbidden = Path(directory) / "blocked.obj"
    forbidden.mkdir()
    files = set(Path(directory).iterdir())
    rejected = await session.call_tool("scene_export", {"path": str(forbidden)})
    assert rejected.isError and set(Path(directory).iterdir()) == files

    source.unlink()
    failed = await session.call_tool("scene_export", args)
    assert failed.isError and output.read_bytes() == raw
    files = set(Path(directory).iterdir())
    failed = await session.call_tool("scene_export", fbx_args)
    assert failed.isError and set(Path(directory).iterdir()) == files
    assert fbx.read_bytes() == fbx_bytes and texture.read_bytes() == png
    failed = await session.call_tool("scene_export", obj_args)
    assert failed.isError and set(Path(directory).iterdir()) == files
    assert obj.read_bytes() == object_bytes and material_file.read_bytes() == material_bytes

    for mesh_file in (obj, fbx):
        async with open_backend("rust") as reopened:
            await successful(reopened, "scene_open", {"path": str(mesh_file)})
            info = await json_result(reopened, "scene_info")
            assert info["pattern_count"] == 0
            readback = Path(directory) / f"reopened-{mesh_file.suffix[1:]}.glb"
            await successful(reopened, "scene_export", {"path": str(readback)})
            model, data = read_gltf_export(readback)
            primitive = model["meshes"][0]["primitives"][0]
            assert model["meshes"][0]["name"] == "smoke-front"
            assert model["accessors"][primitive["attributes"]["TEXCOORD_0"]]["count"] == 3
            pbr = model["materials"][primitive["material"]]["pbrMetallicRoughness"]
            assert pbr["baseColorFactor"] == [0.2, 0.4, 0.6, 1.0]
            image = model["images"][model["textures"][pbr["baseColorTexture"]["index"]]["source"]]
            view = model["bufferViews"][image["bufferView"]]
            offset = view.get("byteOffset", 0)
            assert data[offset:offset + view["byteLength"]] == png
            snapshot = await successful(reopened, "snapshot", {"views": ["front"]})
            assert any(block.type == "image" for block in snapshot.content)
            if mesh_file == fbx:
                await successful(reopened, "scene_open", {"path": str(fbx), "mode": "append"})
                await successful(reopened, "scene_export", {"path": str(readback)})
                appended, _ = read_gltf_export(readback)
                assert len(appended["meshes"]) == 2 * len(model["meshes"])
                invalid = Path(directory) / "invalid.fbx"
                invalid.write_bytes(b"this is not FBX")
                result = await reopened.call_tool("scene_open", {"path": str(invalid)})
                assert result.isError
                await successful(reopened, "scene_export", {"path": str(readback)})
                assert read_gltf_export(readback)[0] == appended


async def smoke_rust():
    async with open_backend("rust") as session:
        await inspect_contract(session)
        await successful(session, "clo_status")
        await successful(session, "clo_launch", {"with_bridge": True})
        await successful(session, "scene_clear")
        initial = await json_result(session, "scene_info")
        triangle = [[0, 0], [160, 0], [80, 110]]
        await successful(session, "pattern_create", {"name": "smoke-front", "points": triangle})
        changed = await json_result(session, "scene_info")
        assert changed["pattern_count"] == initial["pattern_count"] + 1
        invalid = await session.call_tool("pattern_create", {"points": "triangle"})
        assert invalid.isError
        assert (await json_result(session, "scene_info"))["pattern_count"] == changed["pattern_count"]
        await successful(session, "pattern_info", {"pattern": "smoke-front"})
        image = await successful(session, "snapshot_2d")
        assert any(block.type == "image" for block in image.content)
        with TemporaryDirectory() as tmp:
            path = str(Path(tmp) / "smoke.obj")
            await successful(session, "scene_export", {"path": path})
            assert Path(path).stat().st_size > 0
            # Use named, independent triangle meshes with a nonzero height.
            for index, name in enumerate(("first-avatar", "second-avatar")):
                avatar = Path(tmp) / f"{name}.obj"
                avatar.write_text("v 0 0 0\nv 10 0 0\nv 0 20 0\nf 1 2 3\n")
                await successful(session, "avatar_load", {
                    "name_or_path": str(avatar), "replace": index == 0,
                })
            for include_avatar, expected in (
                (False, ["smoke-front"]),
                (True, ["smoke-front", "first-avatar", "second-avatar"]),
            ):
                obj = Path(tmp) / f"avatars-{include_avatar}.obj"
                await successful(session, "scene_export", {
                    "path": str(obj), "include_avatar": include_avatar,
                })
                lines = obj.read_text().splitlines()
                assert [line[2:] for line in lines if line.startswith("o ")] == expected
                assert sum(line.startswith("f ") for line in lines) == len(expected)
            with_mia = Path(tmp) / "with-mia.obj"
            await successful(session, "avatar_load", {"name_or_path": "mia", "replace": False})
            await successful(session, "scene_export", {
                "path": str(with_mia), "include_avatar": True,
            })
            lines = with_mia.read_text().splitlines()
            assert sum(line.startswith("o ") for line in lines) == 4
            assert sum(line.startswith("f ") for line in lines) > 3

            bad_path = Path(tmp) / "directory.obj"
            bad_path.mkdir()
            failed = await session.call_tool("scene_export", {
                "path": str(bad_path), "include_avatar": True,
            })
            assert failed.isError and bad_path.is_dir()
            png = base64.b64decode(next(block.data for block in image.content if block.type == "image"))
            await check_material_export(session, tmp, png)
        diagnostics = await json_result(
            session, "verify_seam", {"project_json": str(Path("samples/Carlton外层.pattern.json").resolve())}
        )
        assert diagnostics
        partial_script = await session.call_tool("clo_run_python", {"code": (
            "pattern_api.CreatePatternWithPoints([(0,0),(60,0),(30,50)]); "
            "raise RuntimeError('smoke failure after creating a piece')"
        )})
        assert partial_script.isError
        expected_count = changed["pattern_count"] + 1
        assert (await json_result(session, "scene_info"))["pattern_count"] == expected_count
        await successful(session, "clo_stop_bridge")
        assert (await session.call_tool("scene_info", {})).isError
        await successful(session, "clo_launch", {"with_bridge": True})
        assert (await json_result(session, "scene_info"))["pattern_count"] == expected_count
    print("Rust MCP: 48 contracts and 11 diagnostics listed. Scene mutation, error, image, export and seam check passed")


async def smoke_official():
    async with open_backend("official") as session:
        await inspect_contract(session)
        status = await json_result(session, "clo_status")
        assert status.get("bridge") == "up", status
        preview = await json_result(session, "garment_plan_preview", {
            "spec": {"type": "tshirt"}, "use_scene_measurements": False,
        })
        assert preview["pieces"] and "seams" in preview
        invalid = await session.call_tool("garment_plan_preview", {})
        assert invalid.isError
    print("Official MCP: 48 contracts and 11 diagnostics listed. Live CLO status, preview and input error passed")


async def compare_fabric_library():
    results = {}
    for backend in ("official", "rust"):
        async with open_backend(backend) as session:
            await inspect_contract(session)
            results[backend] = [
                await json_result(session, "fabric_library", arguments)
                for arguments in ({}, {"filter": "dEnIm"}, {"filter": "__clo_verify_missing_fabric__"})
            ]
            invalid = await session.call_tool("fabric_library", {"filter": []})
            assert invalid.isError, f"{backend} accepted a non-string fabric filter"
    official, rust = results["official"], results["rust"]
    for index, label in enumerate(("all", "mixed-case filter", "no match")):
        assert official[index] == rust[index], f"fabric_library mismatch for {label}"
    assert official[0]["fabrics"] and official[1]["fabrics"]
    assert official[2]["fabrics"] == []
    print(json.dumps({
        "tool": "fabric_library",
        "backends": ["official", "rust"],
        "catalog_size": len(official[0]["fabrics"]),
        "mixed_case_matches": official[1]["fabrics"],
        "empty_match": "pass",
        "invalid_filter_type": "pass",
    }, ensure_ascii=False))


async def compare_avatar_library():
    results = {}
    for backend in ("official", "rust"):
        async with open_backend(backend) as session:
            await inspect_contract(session)
            catalog = await json_result(session, "avatar_list")
            scene = await json_result(session, "scene_info")
            assert catalog["in_scene"] == scene["avatars"], backend
            assert catalog["library"] and catalog["aliases"], backend
            results[backend] = {key: catalog[key] for key in ("library", "aliases")}
    assert results["official"] == results["rust"], "avatar_list library or aliases differ"
    print(json.dumps({
        "tool": "avatar_list",
        "backends": ["official", "rust"],
        "library_size": len(results["rust"]["library"]),
        "alias_count": len(results["rust"]["aliases"]),
        "in_scene_readback": "pass",
    }))


async def compare_saved_fabric_list():
    async with open_backend("official") as session:
        await inspect_contract(session)
        scene = await json_result(session, "scene_info")
        official = await json_result(session, "fabric_list")
    project = Path(scene["project_path"])
    assert project.is_file() and project.suffix.lower() in {".zprj", ".zpac"}, project
    async with open_backend("rust") as session:
        await inspect_contract(session)
        await json_result(session, "scene_open", {"path": str(project)})
        if official["current"] >= 0:
            await json_result(session, "clo_api_call", {
                "name": "fabric_api.SetCurrentFabricIndex", "args": [official["current"]],
            })
        rust = await json_result(session, "fabric_list")
    assert official["current"] == rust["current"]
    assert official["fabrics"] and len(official["fabrics"]) == len(rust["fabrics"])
    compared = 0
    for expected, observed in zip(official["fabrics"], rust["fabrics"]):
        assert expected["index"] == observed["index"] and expected["name"] == observed["name"]
        assert expected["info"], expected["name"]
        for key, value in expected["info"].items():
            assert key in observed["info"], (expected["name"], key)
            assert value == observed["info"][key], (expected["name"], key, value, observed["info"][key])
            compared += 1
    print(json.dumps({"tool": "fabric_list", "fabrics": len(rust["fabrics"]),
                      "catalog_fields_compared": compared, "current": rust["current"]}))


async def compare_saved_pattern_settings():
    async def read(session):
        listed = await json_result(session, "pattern_list")
        count = listed["count"]
        assert count > 0
        sample = min(8, count)
        indices = [i * (count - 1) // max(1, sample - 1) for i in range(sample)]
        panels = [await json_result(session, "pattern_info", {"pattern": str(i)}) for i in indices]
        missing = await session.call_tool("pattern_info", {"pattern": "__clo_verify_missing_panel__"})
        assert missing.isError
        return listed, panels

    async with open_backend("official") as session:
        await inspect_contract(session)
        project = Path((await json_result(session, "scene_info"))["project_path"])
        official, reference_panels = await read(session)
    assert project.is_file() and project.suffix.lower() in {".zprj", ".zpac"}, project
    async with open_backend("rust") as session:
        await inspect_contract(session)
        await json_result(session, "scene_open", {"path": str(project)})
        rust, rust_panels = await read(session)

    def project_fields(rows, keys):
        return [{key: row[key] for key in keys} for row in rows]

    metadata = ("index", "name", "fabric_index", "arrangement")
    assert official["count"] == rust["count"]
    assert project_fields(official["patterns"], metadata) == project_fields(rust["patterns"], metadata)
    metadata += ("layer", "particle_distance")
    assert project_fields(reference_panels, metadata) == project_fields(rust_panels, metadata)
    print(json.dumps({
        "tools": ["pattern_list", "pattern_info"],
        "panels": rust["count"],
        "settings_samples": len(rust_panels),
        "missing_panel_error": "pass",
        "seam_groups": {"official": official["seam_groups"], "rust": rust["seam_groups"]},
    }))


async def main(comparison=None):
    if comparison is not None:
        await comparison()
        return
    await smoke_rust()
    if os.environ.get("CLO_MCP_OFFICIAL_COMMAND"):
        await smoke_official()


async def compare_saved_seams():
    async with open_backend("official") as session:
        await inspect_contract(session)
        project = Path((await json_result(session, "scene_info"))["project_path"])
        official = await json_result(session, "seams_list")
        official_panels = await json_result(session, "pattern_list", {"detail": True})
    assert project.is_file() and project.suffix.lower() in {".zprj", ".zpac"}, project
    async with open_backend("rust") as session:
        await inspect_contract(session)
        await json_result(session, "scene_open", {"path": str(project)})
        rust = await json_result(session, "seams_list")
        rust_panels = await json_result(session, "pattern_list", {"detail": True})
    assert official["count"] == rust["count"] > 0, (official["count"], rust["count"])
    for expected, observed in zip(official["groups"], rust["groups"], strict=True):
        for key in ("index", "name", "patterns"):
            assert expected[key] == observed[key], (expected["index"], key, expected[key], observed[key])
    assert official_panels["seam_groups"] == rust_panels["seam_groups"] == rust["count"]
    for expected, observed in zip(official_panels["patterns"], rust_panels["patterns"], strict=True):
        assert expected["name"] == observed["name"]
        assert expected["seam_groups"] == observed["seam_groups"], (
            expected["name"], expected["seam_groups"], observed["seam_groups"],
        )
    print(json.dumps({"tool": "seams_list", "ordered_groups_matched": rust["count"],
                      "panel_memberships_matched": rust_panels["count"]}))


async def compare_saved_scene_info():
    async with open_backend("official") as session:
        await inspect_contract(session)
        official = await json_result(session, "scene_info")
    project = Path(official["project_path"])
    assert project.is_file() and project.suffix.lower() == ".zprj", project
    async with open_backend("rust") as session:
        await inspect_contract(session)
        await json_result(session, "scene_open", {"path": str(project)})
        rust = await json_result(session, "scene_info")

    project_fields = (
        "project", "project_path", "pattern_count", "patterns", "avatars",
        "fabric_count", "fabrics", "colorways", "current_colorway", "seam_groups",
    )
    assert official.keys() == rust.keys(), (official.keys(), rust.keys())
    for key in project_fields:
        assert official[key] == rust[key], (key, official[key], rust[key])
    assert Path(official["asset_folder"]) == Path(rust["asset_folder"])
    for scene in (official, rust):
        assert scene["clo_version"] and scene["temp_folder"]
    print(json.dumps({
        "tool": "scene_info", "project": rust["project"],
        "project_fields_matched": len(project_fields),
        "pattern_count": rust["pattern_count"], "fabric_count": rust["fabric_count"],
        "seam_groups": rust["seam_groups"], "avatars": rust["avatars"],
        "colorways": rust["colorways"], "current_colorway": rust["current_colorway"],
        "asset_folder_matched": True,
        "runtime_versions": [official["clo_version"], rust["clo_version"]],
    }, ensure_ascii=False))


async def compare_saved_arrangements():
    async with open_backend("official") as session:
        await inspect_contract(session)
        scene = await json_result(session, "scene_info")
        official = await json_result(session, "arrangement_points")
    assert scene["avatars"], "open a saved CLO project with an avatar"
    project = Path(scene["project_path"])
    assert project.is_file() and project.suffix.lower() == ".zprj", project
    async with open_backend("rust") as session:
        await inspect_contract(session)
        await json_result(session, "scene_open", {"path": str(project)})
        rust_scene = await json_result(session, "scene_info")
        rust = await json_result(session, "arrangement_points")
    assert rust_scene["avatars"] == scene["avatars"]
    assert official["count"] == len(official["names"]) > 0
    assert rust == official, (official, rust)
    print(json.dumps({
        "tool": "arrangement_points", "project": scene["project"],
        "avatars": scene["avatars"], "ordered_names_matched": rust["count"],
    }, ensure_ascii=False))


async def compare_saved_avatar_height():
    await compare_saved_avatar_mesh(height_only=True)


async def compare_saved_avatar_mesh(height_only=False):
    async with open_backend("official") as session:
        await inspect_contract(session)
        scene = await json_result(session, "scene_info")
        official = await json_result(session, "avatar_measurements", {"refresh": True})
    assert official["body"]["source"] == "obj", (
        "This case compares geometry estimates, requiring CLO's OBJ fallback", official["source"],
    )
    project = Path(scene["project_path"])
    assert project.is_file() and project.suffix.lower() == ".zprj", project
    async with open_backend("rust") as session:
        await inspect_contract(session)
        await json_result(session, "scene_open", {"path": str(project)})
        rust = await json_result(session, "avatar_measurements", {"refresh": True})
    assert official["avatar"] == rust["avatar"]
    assert rust["source"] == "rust_mesh_estimate"
    assert official["body"].keys() == rust["body"].keys()
    fields = {"height"} if height_only else set(official["body"]) - {"source"}
    tolerance_mm = 0.25
    differences = {
        key: round(rust["body"][key] - official["body"][key], 6)
        for key in sorted(fields)
    }
    assert all(abs(value) <= tolerance_mm for value in differences.values()), differences
    if not height_only:
        assert official["body"]["source"] == rust["body"]["source"]
    print(json.dumps({
        "tool": "avatar_measurements", "case": "geometry fallback",
        "avatar": rust["avatar"], "numeric_fields": len(fields),
        "body_mm": {key: rust["body"][key] for key in sorted(fields)},
        "tolerance_mm": tolerance_mm, "differences_mm": differences,
    }, ensure_ascii=False))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    comparisons = parser.add_mutually_exclusive_group()
    comparisons.add_argument("--compare-fabric-library", action="store_const", dest="comparison",
                             const=compare_fabric_library,
                        help="compare both live backends against the same CLO fabric library")
    comparisons.add_argument("--compare-avatar-library", action="store_const", dest="comparison",
                             const=compare_avatar_library,
                             help="compare both live backends against the same CLO avatar library")
    comparisons.add_argument("--compare-saved-fabric-list", action="store_const", dest="comparison",
                             const=compare_saved_fabric_list,
                             help="compare live fabric metadata against the same saved CLO project in Rust")
    comparisons.add_argument("--compare-saved-pattern-settings", action="store_const", dest="comparison",
                             const=compare_saved_pattern_settings,
                             help="compare live pattern settings against the same saved CLO project in Rust")
    comparisons.add_argument("--compare-saved-seams", action="store_const", dest="comparison",
                             const=compare_saved_seams,
                             help="compare ordered seam groups and panel membership in a saved CLO project")
    comparisons.add_argument("--compare-saved-scene-info", action="store_const", dest="comparison",
                             const=compare_saved_scene_info,
                             help="compare scene metadata for the same saved CLO project")
    comparisons.add_argument("--compare-saved-arrangements", action="store_const", dest="comparison",
                             const=compare_saved_arrangements,
                             help="compare ordered avatar arrangement names in a saved CLO project")
    comparisons.add_argument("--compare-saved-avatar-height", action="store_const", dest="comparison",
                             const=compare_saved_avatar_height,
                             help="compare geometry fallback height in a saved CLO project")
    comparisons.add_argument("--compare-saved-avatar-mesh", action="store_const", dest="comparison",
                             const=compare_saved_avatar_mesh,
                             help="compare all geometry fallback body fields in a saved CLO project")
    args = parser.parse_args()
    if args.comparison is not None and not os.environ.get("CLO_MCP_OFFICIAL_COMMAND"):
        parser.error("live comparisons require CLO_MCP_OFFICIAL_COMMAND")
    asyncio.run(main(args.comparison))
