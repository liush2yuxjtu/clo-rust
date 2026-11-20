"""Exercise the common MCP entry point on Rust and, when configured, CLO 3D."""

import argparse
import asyncio
import base64
import hashlib
import json
import math
import os
import re
import shutil
import struct
import sys
import zipfile
from io import BytesIO
from collections import Counter
from contextlib import asynccontextmanager
from datetime import datetime, timedelta
from pathlib import Path
from tempfile import TemporaryDirectory

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

from .compat import CONTRACT, official_parameters


@asynccontextmanager
async def open_backend(backend, environment=None):
    env = os.environ.copy()
    if environment:
        env.update(environment)
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


async def save_fabric_records(session, path):
    await json_result(session, "scene_save", {"path": str(path), "thumbnail": False})
    return json.loads(path.read_text())["scene"]["fabrics"]


def read_gltf_export(path):
    if path.suffix == ".glb":
        raw = path.read_bytes()
        assert raw[:4] == b"glTF" and raw[16:20] == b"JSON"
        binary_header = 20 + struct.unpack_from("<I", raw, 12)[0]
        assert raw[binary_header + 4:binary_header + 8] == b"BIN\0"
        return json.loads(raw[20:binary_header]), raw[binary_header + 8:]
    model = json.loads(path.read_text())
    uri = model["buffers"][0]["uri"]
    if uri.startswith("data:"):
        return model, base64.b64decode(uri.split(",", 1)[1])
    return model, (path.parent / uri).read_bytes()


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


async def verify_fabric_assignment_workflow():
    library_report = {}
    with TemporaryDirectory(prefix="clo-fabric-assignment-") as directory:
        root = Path(directory)
        async with open_backend("rust", {"CLO_RUST_FABRIC_DIR": "", "CLO_RUST_AVATAR_DIR": ""}) as rust:
            await inspect_contract(rust)

            async def saved(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path)})
                return path, json.loads(path.read_text())["scene"]

            async def assignments():
                rows = (await json_result(rust, "scene_info"))["patterns"]
                return [row["fabric_index"] for row in rows]

            for name, offset in (("1", 0), ("Back", 130)):
                await json_result(rust, "pattern_create", {"name": name,
                    "points": [[offset, 0], [offset + 100, 0], [offset, 100]]})
            await json_result(rust, "simulate", {"steps": 3, "particle_distance": 45})
            _, baseline = await saved("baseline")
            source = os.environ.get("CLO_FABRIC_ASSIGN_FIXTURE", "denim")
            new = await json_result(rust, "clo_api_call", {"name": "fabric_api.AddFabric", "args": [source]})
            new_index = new["value"]
            assert new_index > 0 and await assignments() == [0, 0]
            _, added = await saved("added")
            assert added["simulation"] == baseline["simulation"]
            assert added["fabrics"][new_index]["index"] == new_index

            assigned = await json_result(rust, "clo_run_python", {
                "code": f"fabric_api.AssignFabricToPattern({new_index}, 0, 3)",
            })
            assert assigned["value"] is True and await assignments() == [new_index, 0]
            apply = await json_result(rust, "fabric_apply", {
                "fabric": "cotton", "color": "#224466", "patterns": ["Back", "1"],
            })
            assert apply["assigned"] == {"assigned": [True, True], "patterns": [1, 1]}
            back_index = apply["fabric"]["index"]
            assert await assignments() == [new_index, back_index]
            numeric_assignment = await json_result(rust, "clo_api_call", {
                "name": "fabric_api.AssignFabricToPattern", "args": [back_index, 1, 3],
            })
            assert numeric_assignment["value"] is True
            assert await assignments() == [new_index, back_index]
            _, before_selection = await saved("before-selection-error")
            bad_selection = await rust.call_tool("fabric_apply", {
                "fabric": "cotton", "patterns": ["0", "missing-panel"],
            })
            assert bad_selection.isError and await assignments() == [new_index, back_index]
            after_selection_error = await save_fabric_records(rust, root / "after-selection-error.clorust")
            assert after_selection_error[:-1] == before_selection["fabrics"]
            assert after_selection_error[-1]["name"] == apply["fabric"]["name"]
            bad = await rust.call_tool("clo_api_call", {"name": "fabric_api.AssignFabricToPattern", "args": [new_index, 99, 3]})
            assert bad.isError and await assignments() == [new_index, back_index]
            path, updated = await saved("assigned")
            expected = [mesh | {"fabric_index": index} for mesh, index in
                        zip(baseline["simulation"]["meshes"], (new_index, back_index), strict=True)]
            assert updated["simulation"] == baseline["simulation"] | {"meshes": expected}

            await json_result(rust, "scene_open", {"path": str(path)})
            await json_result(rust, "simulate", {"steps": 0})
            _, reopened = await saved("reopened")
            assert reopened["simulation"]["steps"] == 3 and reopened["simulation"]["meshes"] == expected
            glb = root / "assignment.glb"
            await json_result(rust, "scene_export", {"path": str(glb)})
            model, _ = read_gltf_export(glb)
            material_names = [model["materials"][mesh["primitives"][0]["material"]]["name"] for mesh in model["meshes"]]
            assert material_names == [updated["fabrics"][index]["name"] for index in (new_index, back_index)]

            for label, options, expected_color, expect_error in (
                ("invalid-color", {"color": "#gggggg"}, [0.85, 0.85, 0.85], True),
                ("missing-texture", {"color": "#246", "texture_image": str(root / "missing.png")},
                 [34 / 255, 68 / 255, 102 / 255], False),
            ):
                before = await save_fabric_records(rust, root / "before-error.clorust")
                result = await rust.call_tool("fabric_apply", {
                    "fabric": "cotton", "patterns": ["Back"], **options,
                })
                assert bool(result.isError) == expect_error, label
                after = await save_fabric_records(rust, root / "after-error.clorust")
                assert after[:-1] == before
                index = after[-1]["index"]
                assert await assignments() == [new_index, index], label
                _, saved_after_error = await saved(label)
                assert saved_after_error["fabrics"][-1]["color"] == expected_color, label
                assert saved_after_error["simulation"] == reopened["simulation"] | {
                    "meshes": [expected[0], expected[1] | {"fabric_index": index}],
                }, label
        if os.environ.get("CLO_FABRIC_ASSIGN_FIXTURE"):
            library = root / "Fabric"
            (library / "A-nested").mkdir(parents=True)
            stem = "V2_Woven_Denim_1.zfab"
            canonical = library / stem
            shutil.copyfile(source, canonical)
            shutil.copyfile(source, library / "A-nested" / "Écru Denim.zfab")
            (library / "A-nested" / stem).write_bytes(b"not a fabric archive")
            (library / "extra.json").write_text("{}")
            async with open_backend("rust", {"CLO_RUST_FABRIC_DIR": str(library)}) as rust:
                listing = await json_result(rust, "fabric_library")
                assert listing["fabrics"] == [stem, "Écru Denim.zfab", stem], listing
                filtered = await json_result(rust, "fabric_library", {"filter": "ÉCRU"})
                assert filtered["fabrics"] == ["Écru Denim.zfab"], filtered
                applied = await json_result(rust, "fabric_apply", {"fabric": "denim"})
                assert applied["matched"] == "denim" and Path(applied["file"]) == canonical.resolve()
                readback = await save_fabric_records(rust, root / "library-materials.clorust")
                assert readback[applied["fabric"]["index"]]["name"] == applied["fabric"]["name"]
                library_report = {"native_filenames": listing["fabrics"],
                                  "native_keyword_file": canonical.name,
                                  "unicode_filter": filtered["fabrics"]}
    print(json.dumps({"tools": ["fabric_apply", "clo_api_call", "clo_run_python", "simulate", "scene_export"],
                      "case": "add fabric, assign one panel, preserve drape", "add_kept_assignments": True,
                      "assigned_indices": [new_index, back_index], "retained_step": 3,
                      "numeric_reference_preceded_name": True,
                      "selection_error_retained_fabric": True,
                      "color_error_retained_assignment": True,
                      "missing_texture_retained_color": True,
                      "invalid_assignment_preserved_scene": True, "glb_materials": material_names}
                     | library_report, ensure_ascii=False))


async def compare_live_fabric_edits():
    fixture = Path(os.environ["CLO_FABRIC_LIVE_FIXTURE"])
    image = Path(os.environ["CLO_FABRIC_LIVE_IMAGE"])
    wide = Path(os.environ["CLO_FABRIC_LIVE_WIDE_IMAGE"])
    saved_fabrics = Path(os.environ["CLO_FABRIC_LIVE_SAVED_FIXTURE"])
    assert all(path.is_file() for path in (fixture, image, wide, saved_fabrics))
    state = Path(os.environ["CLO_MCP_STATE"])
    missing = state / "missing-fabric-texture.png"
    corrupt = state / "corrupt-fabric-texture.png"
    assert state.is_dir() and not missing.exists() and not corrupt.exists()
    apply_cases = (
        ("denim default all", {"fabric": "denim"}, False, [1, 1]),
        ("cotton by name", {"fabric": "cotton", "patterns": ["Back"], "color": "#246"}, False, [0, 1]),
        ("satin duplicate references", {"fabric": "satin", "patterns": ["1", "1"]}, False, [0, 1]),
        ("jersey empty selection", {"fabric": "jersey", "patterns": []}, False, [1, 1]),
        ("chiffon named color", {"fabric": "chiffon", "color": "navy"}, False, [1, 1]),
        ("explicit library filename", {"fabric": "V2_Woven_Denim_1.zfab", "patterns": ["0"]}, False, [1, 0]),
        ("unrecognized keyword fallback", {"fabric": "fabric-absent-404"}, False, [1, 1]),
        ("missing name before assignment", {"fabric": "cotton", "patterns": ["Front", "missing-panel"]}, True, [0, 0]),
        ("out-of-range positive index", {"fabric": "cotton", "patterns": ["0", "99", "1"]}, False, [1, 1]),
        ("negative index after valid assignments", {"fabric": "cotton", "patterns": ["0", "99", "1", "-1"]}, True, [1, 1]),
        ("unsigned overflow after first assignment", {"fabric": "cotton", "patterns": ["0", "4294967296", "1"]}, True, [1, 0]),
        ("bad color after assignment", {"fabric": "cotton", "patterns": ["1"], "color": "#gggggg"}, True, [0, 1]),
        ("missing texture after recolor", {"fabric": "cotton", "patterns": ["1"], "color": "navy", "texture_image": str(missing)}, False, [0, 1]),
        ("wide texture after recolor", {"fabric": "cotton", "patterns": ["1"], "color": "navy", "texture_image": str(wide)}, False, [0, 1]),
    )
    edit_cases = (
        ("fabric_set_color", {"fabric_index": 0, "color": "#246"}, False, 1, ("None", image.stem)),
        ("fabric_set_color", {"fabric_index": 99, "color": "navy"}, False, 1, ("None", "None")),
        ("fabric_set_color", {"fabric_index": -1, "color": "navy"}, True, 1, ("None", image.stem)),
        ("fabric_set_color", {"fabric_index": 0, "color": "#gggggg"}, True, 1, ("None", image.stem)),
        ("fabric_set_texture", {"fabric_index": 0, "image_path": str(image)}, False, 1, (image.stem, image.stem)),
        ("fabric_set_texture", {"fabric_index": 0, "image_path": str(missing)}, False, 1, ("None", image.stem)),
        ("fabric_set_texture", {"fabric_index": 0, "image_path": str(corrupt)}, False, 1, ("None", image.stem)),
        ("fabric_set_texture", {"fabric_index": 0, "image_path": "", "map": "unknown"}, True, 0, ("None", image.stem)),
        ("fabric_set_texture", {"fabric_index": 99, "image_path": ""}, False, 1, ("None", "None")),
        ("fabric_set_texture", {"fabric_index": -1, "image_path": str(wide)}, False, 1, ("None", wide.stem)),
        ("fabric_set_texture", {"fabric_index": 2147483648, "image_path": ""}, True, 1, ("None", image.stem)),
    )

    async def read(session):
        scene = await json_result(session, "scene_info")
        listing = await json_result(session, "fabric_list")
        total = (await json_result(session, "clo_api_call", {
            "name": "fabric_api.GetFabricCount", "args": [False],
        }))["value"]
        return listing, [panel["fabric_index"] for panel in scene["patterns"]], total

    async def attempt(session, tool, args, error):
        if error:
            assert (await session.call_tool(tool, args)).isError, (tool, args)
            return "error"
        return await json_result(session, tool, args)

    async with open_backend("official") as clo, open_backend("rust") as rust:
        original = await json_result(clo, "scene_info")
        assert (await json_result(clo, "clo_status"))["bridge"] == "up"
        with corrupt.open("x", encoding="utf-8") as handle:
            handle.write("invalid image")
        try:
            for session in (clo, rust):
                await inspect_contract(session)
            apply_checks = []
            for label, args, error, assignment in apply_cases:
                observed = []
                for session in (clo, rust):
                    await json_result(session, "scene_open", {"path": str(fixture)})
                    reply = await attempt(session, "fabric_apply", args, error)
                    listing, panels, total = await read(session)
                    assert total == 2 and panels == assignment, (label, panels, total)
                    observed.append((reply, listing))
                assert observed[0] == observed[1], (label, observed)
                if label == "out-of-range positive index":
                    assert observed[0][0]["assigned"]["assigned"] == [True, False, True]
                if label == "wide texture after recolor":
                    assert observed[0][1]["fabrics"][1]["info"]["Texture"] == wide.stem
                apply_checks.append({"case": label, "assignment": assignment, "result": observed[0][0]})

            edit_checks = []
            for tool, args, error, current, textures in edit_cases:
                observed = []
                for session in (clo, rust):
                    await json_result(session, "scene_open", {"path": str(fixture)})
                    await json_result(session, "fabric_apply", {
                        "fabric": "cotton", "patterns": ["1"], "texture_image": str(image),
                    })
                    reply = await attempt(session, tool, args, error)
                    listing, panels, total = await read(session)
                    assert total == 2 and panels == [0, 1]
                    assert listing["current"] == current, (tool, args, listing)
                    assert tuple(row["info"]["Texture"] for row in listing["fabrics"]) == textures
                    observed.append((reply, listing))
                assert observed[0] == observed[1], (tool, args, observed)
                edit_checks.append({"tool": tool, "args": args, "current": current,
                                    "textures": textures, "result": observed[0][0]})
            for session in (clo, rust):
                await json_result(session, "scene_open", {"path": str(fixture)})
                await json_result(session, "fabric_apply", {"fabric": "cotton", "patterns": ["1"]})
                await json_result(session, "clo_api_call", {
                    "name": "fabric_api.SetCurrentFabricIndex", "args": [0],
                })
                assert (await read(session))[0]["current"] == 0
                await json_result(session, "fabric_set_color", {"fabric_index": 0, "color": "#246"})
                assert (await read(session))[0]["current"] == 1
            for backend, session in (("official", clo), ("rust", rust)):
                await json_result(session, "scene_open", {"path": str(saved_fabrics)})
                select = {"name": "fabric_api.SetCurrentFabricIndex", "args": [1]}
                for tool, args in (
                    ("fabric_set_color", {"fabric_index": 0, "color": "#246"}),
                    ("fabric_set_texture", {"fabric_index": 0, "image_path": str(image)}),
                ):
                    await json_result(session, "clo_api_call", select)
                    await json_result(session, tool, args)
                    listing, _, _ = await read(session)
                    assert listing["current"] == 0, (backend, tool, listing)
        finally:
            corrupt.unlink()
            if original["project_path"]:
                await json_result(clo, "scene_open", {"path": original["project_path"]})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    print(json.dumps({"assignment_cases": apply_checks, "edit_cases": edit_checks,
                      "complete_fabric_list_matched": True, "reselected_latest_added_fabric": True,
                      "reopen_reset_fabric_selection": True, "result": "pass"}, ensure_ascii=False))


async def verify_fabric_recolor_opacity():
    with TemporaryDirectory(prefix="clo-recolor-opacity-") as directory:
        root = Path(directory)
        async with open_backend("rust", {"CLO_RUST_FABRIC_DIR": ""}) as rust:
            await inspect_contract(rust)
            await json_result(rust, "pattern_create", {"name": "Panel", "points": [[0, 0], [100, 0], [0, 100]]})
            capture = await successful(rust, "snapshot_2d")
            png, = [block for block in capture.content if block.type == "image"]
            image = root.resolve() / "map.png"
            image.write_bytes(base64.b64decode(png.data, validate=True))

            async def materials():
                return await save_fabric_records(rust, root / "material-readback.clorust")

            async def maps(index):
                return next(fabric for fabric in await materials() if fabric["index"] == index)["texture_maps"]

            async def set_map(index, kind):
                await json_result(rust, "fabric_set_texture", {"fabric_index": index, "map": kind, "image_path": str(image)})

            async def check_faces(index, rgb):
                fabric = next(item for item in await materials() if item["index"] == index)
                rgba = rgb + [1.0]
                for row in fabric["info"].get("face_colors", []):
                    if row["type"] in (0, 1, 2):
                        assert all(abs(a - b) < 1e-6 for a, b in zip(row["rgba"], rgba, strict=True)), row
                for face in range(3):
                    result = await json_result(rust, "clo_api_call", {
                        "name": "fabric_api.GetFabricPBRMaterialBaseColor", "args": [index, face],
                    })
                    assert all(abs(a - b) < 1e-6 for a, b in zip(result["value"], rgba, strict=True)), result

            expected_rgb = [34 / 255, 68 / 255, 102 / 255]

            async def recolor(index, text="#224466", rgb=expected_rgb):
                result = await json_result(rust, "fabric_set_color", {"fabric_index": index, "color": text})
                assert result == {"rgb": rgb, "result": {"ok": [True, True, True]}}
                await check_faces(index, rgb)

            indices = []
            for name in ("cotton", "chiffon"):
                applied = await json_result(rust, "fabric_apply", {"fabric": name})
                index = applied["fabric"]["index"]
                indices.append(index)
                for kind in ("base", "normal", "opacity"):
                    await set_map(index, kind)
                await recolor(index)
            opaque, sheer = indices
            await recolor(sheer, " navy gold ", [0.83, 0.68, 0.22])
            await recolor(sheer, "#246")
            before_invalid = await materials()
            for index, color in ((opaque, "#x24"), (-1, "#246")):
                rejected = await rust.call_tool("fabric_set_color", {"fabric_index": index, "color": color})
                assert rejected.isError
            assert await materials() == before_invalid
            portable = root / "recolored.clorust"
            await json_result(rust, "scene_save", {"path": str(portable)})
            await json_result(rust, "scene_open", {"path": str(portable)})
            assert await maps(opaque) == {"base": str(image), "normal": str(image)}
            assert await maps(sheer) == {"base": str(image), "normal": str(image), "opacity": str(image)}
            assert all(item["base_texture_desaturated"] for item in await materials() if item["index"] in indices)
            glb = root / "recolored.glb"
            await json_result(rust, "scene_export", {"path": str(glb)})
            model, _ = read_gltf_export(glb)
            primitive = model["meshes"][0]["primitives"][0]
            assert "baseColorTexture" in model["materials"][primitive["material"]]["pbrMetallicRoughness"]
            for index in (opaque, sheer):
                await check_faces(index, expected_rgb)

            native = {}
            for label, fixture_key in (("opaque", "CLO_RECOLOR_OPAQUE_FIXTURE"), ("sheer", "CLO_RECOLOR_SHEER_FIXTURE")):
                if not (fixture := os.environ.get(fixture_key)):
                    continue
                await json_result(rust, "scene_open", {"path": fixture})
                fabric = next(item for item in await materials() if "opacity" in (item.get("texture_maps") or {}))
                index = fabric["index"]
                before = await maps(index)
                if label == "opaque":
                    await set_map(index, "opacity")
                await recolor(index)
                live = await maps(index)
                expected = {key: value for key, value in before.items() if key != "opacity" or label == "sheer"}
                assert live == expected
                path = root / (label + Path(fixture).suffix)
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                await json_result(rust, "scene_open", {"path": str(path)})
                assert await maps(index) == expected
                reopened = next(item for item in await materials() if item["index"] == index)
                assert reopened.get("base_texture_desaturated", False) == ("base" in expected)
                await check_faces(index, expected_rgb)
                native[label] = {"fabric": fabric["name"], "opacity_kept": "opacity" in expected}
                if label == "opaque":
                    await json_result(rust, "scene_open", {"path": fixture})
                    await recolor(index)
                    await set_map(index, "opacity")
                    await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                    await json_result(rust, "scene_open", {"path": str(path)})
                    assert Path((await maps(index))["opacity"]).read_bytes() == image.read_bytes()
                    native[label]["later_replacement_kept"] = True
            references = os.environ.get("CLO_DESATURATION_REFERENCE_DIR")
            reference_pixels = await compare_desaturation_pixels(rust, root, Path(references)) if references else {}
    print(json.dumps({"tool": "fabric_set_color", "case": "recolor opacity policy", "portable_reopen": True,
                      "opaque_map_cleared": True, "sheer_map_preserved": True, "native": native,
                      "face_rgba_readback": True, "invalid_recolor_kept_materials": True,
                      "base_texture_retained_and_desaturated": True, "glb_base_texture_retained": True,
                      "official_desaturation_pixels": reference_pixels}))


async def compare_desaturation_pixels(rust, root, reference):
    from io import BytesIO
    from PIL import Image

    def rgba(path):
        model, data = read_gltf_export(path)
        primitive = model["meshes"][0]["primitives"][0]
        pbr = model["materials"][primitive["material"]]["pbrMetallicRoughness"]
        texture = model["textures"][pbr["baseColorTexture"]["index"]]
        image = model["images"][texture["source"]]
        view = model["bufferViews"][image["bufferView"]]
        start = view.get("byteOffset", 0)
        with Image.open(BytesIO(data[start:start + view["byteLength"]])) as decoded:
            return decoded.size, decoded.convert("RGBA").tobytes()

    await json_result(rust, "scene_clear", {"keep_avatar": False})
    await json_result(rust, "pattern_create", {
        "name": "Desaturation Panel", "points": [[0, 0], [100, 0], [0, 100]],
    })
    applied = await json_result(rust, "fabric_apply", {"fabric": "cotton"})
    index = applied["fabric"]["index"]
    cases = [("palette", "shared-desat-palette.png", "shared-desat-after.glb")]
    cases.extend((case, f"desat-{case}.png", f"desat-probe-{case}-after.glb")
                 for case in ("ramp", "alpha", "varied", "gray", "single", "long-ramp"))
    pixels = {}
    for case, source, expected in cases:
        texture = (reference / source).resolve(strict=True)
        await json_result(rust, "fabric_set_texture", {
            "fabric_index": index, "map": "base", "image_path": str(texture),
        })
        await json_result(rust, "fabric_set_color", {"fabric_index": index, "color": "#27b46c"})
        output = root / f"desaturated-{case}.glb"
        await json_result(rust, "scene_export", {"path": str(output)})
        size, actual = rgba(output)
        assert (size, actual) == rgba(reference / expected), case
        pixels[case] = size[0] * size[1]
    return pixels


async def verify_fabric_texture_clear():
    with TemporaryDirectory(prefix="clo-clear-texture-") as directory:
        root = Path(directory)
        async with open_backend("rust", {"CLO_RUST_FABRIC_DIR": ""}) as rust:
            await inspect_contract(rust)
            await json_result(rust, "pattern_create", {"name": "Panel", "points": [[0, 0], [100, 0], [0, 100]]})
            fabric = await json_result(rust, "fabric_apply", {"fabric": "cotton"})
            index = fabric["fabric"]["index"]
            capture = await successful(rust, "snapshot_2d")
            png, = [block for block in capture.content if block.type == "image"]
            image = root.resolve() / "swatch.png"
            image.write_bytes(base64.b64decode(png.data, validate=True))

            async def maps(index):
                materials = await save_fabric_records(rust, root / "material-readback.clorust")
                return next(fabric for fabric in materials if fabric["index"] == index)["texture_maps"]

            async def texture(index, map, path=""):
                result = await json_result(rust, "fabric_set_texture", {
                    "fabric_index": index, "map": map, "image_path": str(path),
                })
                assert result == {"ok": True}

            await texture(index, "base", image)
            await texture(index, "normal", image)
            for _ in range(2):
                await texture(index, "base")
                assert await maps(index) == {"normal": str(image)}
            other = (await json_result(rust, "clo_api_call", {
                "name": "fabric_api.AddFabric", "args": ["denim"],
            }))["value"]
            assert other != index
            for patch, expected_current, error in (
                ({"map": "unknown"}, index, True),
                ({"image_path": str(root / "missing.png")}, other, False),
                ({"fabric_index": 9001}, other, False),
            ):
                await json_result(rust, "clo_api_call", {
                    "name": "fabric_api.SetCurrentFabricIndex", "args": [other],
                })
                result = await rust.call_tool("fabric_set_texture", {
                    "fabric_index": index, "image_path": "", "map": "normal", **patch,
                })
                assert bool(result.isError) == error, patch
                listing = await json_result(rust, "fabric_list")
                assert listing["current"] == expected_current, patch
                assert await maps(index) == {"normal": str(image)}
                assert await maps(other) == {}
            await texture(index, "base")
            assert await maps(index) == {"normal": str(image)}
            portable = root / "cleared.clorust"
            await json_result(rust, "scene_save", {"path": str(portable)})
            await json_result(rust, "scene_open", {"path": str(portable)})
            assert await maps(index) == {"normal": str(image)}
            glb = root / "cleared.glb"
            await json_result(rust, "scene_export", {"path": str(glb)})
            project, _ = read_gltf_export(glb)
            primitive = project["meshes"][0]["primitives"][0]
            assert "baseColorTexture" not in project["materials"][primitive["material"]]["pbrMetallicRoughness"]
            await successful(rust, "clo_api_call", {
                "name": "fabric_api.SetCurrentFabricIndex", "args": [index],
            })
            await successful(rust, "clo_api_call", {
                "name": "fabric_api.SetNormalMapImageGivenFilePath", "args": [""],
            })
            assert await maps(index) == {}

            native = None
            if fixture := os.environ.get("CLO_TEXTURE_CLEAR_FIXTURE"):
                await json_result(rust, "scene_open", {"path": fixture})
                materials = await save_fabric_records(rust, root / "material-readback.clorust")
                target = next(fabric for fabric in materials if {"base", "normal"} <= fabric.get("texture_maps", {}).keys())
                index = target["index"]
                before = await maps(index)
                await texture(index, "base")
                path = root / ("native-cleared" + Path(fixture).suffix)
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                await json_result(rust, "scene_open", {"path": str(path)})
                remaining = await maps(index)
                assert remaining == {key: path for key, path in before.items() if key != "base"}
                channels = ("base", "normal", "roughness", "displacement", "opacity", "metalness")
                added = [kind for kind in channels if kind not in remaining]
                for kind in added:
                    await texture(index, kind, image)
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                await json_result(rust, "scene_open", {"path": str(path)})
                restored = await maps(index)
                for key, original in remaining.items():
                    assert restored[key] == original, key
                for key in added:
                    assert Path(restored[key]).read_bytes() == image.read_bytes(), key
                native = {"suffix": path.suffix, "unchanged_maps": sorted(remaining),
                          "added_maps": added}
    print(json.dumps({"tool": "fabric_set_texture", "case": "clear only the selected map",
                      "portable_reopen": True, "glb_without_base_texture": True,
                      "native_reopen": native, "api_clear": True, "invalid_clear_preserves_maps": True,
                      "valid_target_selected_before_error": True, "invalid_index_kept_selection": True}))


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


async def inspect_reference_contract():
    expected = {entry["name"]: entry for entry in json.loads(CONTRACT.read_text())}
    async with stdio_client(official_parameters()) as (reader, writer):
        async with ClientSession(reader, writer) as native:
            await native.initialize()
            advertised = (await native.list_tools()).tools
            actual = {tool.name: tool.model_dump(mode="json", by_alias=True, exclude_none=True)
                      for tool in advertised}
            assert len(advertised) == len(expected) == 48
            assert actual == expected, sorted(name for name in actual.keys() | expected.keys()
                                              if actual.get(name) != expected.get(name))
            return len(advertised)


async def verify_tool_contracts():
    count = await inspect_reference_contract()
    for backend in ("official", "rust"):
        async with open_backend(backend) as session:
            await inspect_contract(session)
    print(json.dumps({"check": "tools/list", "public_tool_count": count,
                      "reference_mcp": "pass", "official_adapter": "pass", "rust_adapter": "pass"}))


async def inspect_control_guide(session):
    guide = await json_result(session, "clo_control_guide")
    assert guide.keys() == {"route", "codex_desktop", "standalone_mcp", "limits"}
    assert all(isinstance(guide[key], str) and guide[key] for key in guide.keys() - {"route"})
    route = guide["route"]
    assert isinstance(route, list) and len(route) == 4
    assert all(isinstance(step, str) and step for step in route)
    assert "clo_api_catalog" in route[0] and "clo_api_describe" in route[0]
    assert "clo_api_call" in route[1]
    assert "clo_ui_state" in route[2] and "clo_ui_screenshot" in route[2]
    assert "clo_ui_action" in " ".join(route[2:]) and "state" in route[3].lower()
    return guide


async def compare_control_guide():
    guides = {}
    for backend in ("official", "rust"):
        async with open_backend(backend) as session:
            await inspect_contract(session)
            guides[backend] = await inspect_control_guide(session)
    print(json.dumps({"tool": "clo_control_guide", "workflow": "pass",
                      "guides": guides}, ensure_ascii=False))


async def compare_desktop_observation():
    native_contract_count = await inspect_reference_contract()
    summary = {}
    invalid_limits = ({"max_nodes": -1}, {"max_nodes": 0}, {"max_nodes": 5001},
                      {"max_depth": -1}, {"max_depth": 0}, {"max_depth": 31})
    for backend in ("official", "rust"):
        async with open_backend(backend) as session:
            await inspect_contract(session)
            await inspect_control_guide(session)

            limits = {"max_nodes": 1.0, "max_depth": 1}
            first = await json_result(session, "clo_ui_state", limits)
            root, = first["nodes"]
            assert root["parent"] is None and root["AXRole"] == "AXApplication"
            assert first["truncated"] is True
            second = await json_result(session, "clo_ui_state", {"max_nodes": 24, "max_depth": 2})
            assert first["snapshot"] and second["snapshot"] and first["snapshot"] != second["snapshot"]
            assert first["pid"] == second["pid"] > 0 and second["coordinate_system"]
            assert 1 < len(second["nodes"]) <= 24
            depths = {}
            for node in second["nodes"]:
                parent = node["parent"]
                assert node["id"] not in depths and isinstance(node["actions"], list)
                depths[node["id"]] = 0 if parent is None else depths[parent] + 1
            assert max(depths.values()) <= 2
            assert isinstance(second["windows"], list)
            windows = second["windows"]
            if windows or backend == "rust":
                assert any(window["bounds"]["Width"] > 0 and window["bounds"]["Height"] > 0
                           for window in windows)

            stale = await session.call_tool("clo_ui_action", {
                "snapshot": first["snapshot"], "action": "press", "element": root["id"],
            })
            assert stale.isError
            for args in invalid_limits:
                result = await session.call_tool("clo_ui_state", args)
                assert result.isError, (backend, args, "invalid UI limits accepted")
            result = await session.call_tool("clo_ui_screenshot", {})
            observation = {"nodes": len(second["nodes"]), "invalid_limits_rejected": len(invalid_limits),
                           "stale_token_rejected": True}
            if backend == "official" and not windows and result.isError:
                observation["screenshot"] = {"status": "BLOCKED", "reason": "no on-screen CLO window"}
            else:
                assert not result.isError, (backend, result.content)
                png, = result.content
                assert png.type == "image" and png.mimeType == "image/png"
                payload = base64.b64decode(png.data, validate=True)
                assert payload[:8] == b"\x89PNG\r\n\x1a\n" and payload[12:16] == b"IHDR"
                width, height = struct.unpack_from(">II", payload, 16)
                assert width > 0 and height > 0
                observation["screenshot"] = {"status": "PASS", "width": width, "height": height}
            summary[backend] = observation
    print(json.dumps({"case": "desktop observation", "native_contract_count": native_contract_count,
                      "tools": ["clo_control_guide", "clo_ui_state", "clo_ui_action", "clo_ui_screenshot"],
                      "results": summary}))
    if summary["official"]["screenshot"]["status"] == "BLOCKED":
        raise SystemExit(2)


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


async def compare_native_fabric_list():
    fixture = Path(os.environ["CLO_FABRIC_LIST_FIXTURE"]).resolve(strict=True)
    reports = {}
    for backend in ("official", "rust"):
        async with open_backend(backend) as session:
            assert (await json_result(session, "clo_status"))["bridge"] == "up"
            path = str(fixture)
            if backend == "official":
                path = (await json_result(session, "scene_info"))["project_path"]
                assert Path(path).resolve() == fixture, "open the saved fixture in CLO first"
            request = {"path": path, "mode": "replace"}
            await json_result(session, "scene_open", request)
            first = await json_result(session, "scene_info")
            assert [panel["fabric_index"] for panel in first["patterns"]] == [1, 0]

            async def capture(case, expected_rows, selection, stored):
                listing = await json_result(session, "fabric_list")
                assert [row["index"] for row in listing["fabrics"]] == expected_rows
                assert listing["current"] == selection
                reply = await json_result(session, "clo_api_call", {
                    "name": "fabric_api.GetFabricCount", "args": [False],
                })
                assert reply["value"] == stored
                reports.setdefault(backend, {})[case] = listing

            try:
                await capture("saved", [0, 1], 0, 2)
                await json_result(session, "clo_api_call", {
                    "name": "fabric_api.SetCurrentFabricIndex", "args": [1],
                })
                await json_result(session, "clo_api_call", {
                    "name": "fabric_api.SetFabricName", "args": [1, "MCP verification fabric"],
                })
                await capture("renamed", [0, 1], 1, 2)
                assert reports[backend]["renamed"]["fabrics"][1]["name"] == "MCP verification fabric"
                for index, color in ((1, "#224466"), (0, "#662244")):
                    await json_result(session, "fabric_set_color", {"fabric_index": index, "color": color})
                    case = f"recolored_{index}"
                    await capture(case, [0, 1], index, 2)
                    assert reports[backend][case]["fabrics"][index]["info"]["ColorCode"] == color
                await json_result(session, "clo_api_call", {
                    "name": "fabric_api.SetCurrentFabricIndex", "args": [1],
                })
                await json_result(session, "pattern_delete", {"patterns": [first["patterns"][1]["name"]]})
                await capture("sparse", [0], 1, 2)
                await json_result(session, "scene_clear")
                await capture("cleared", [], 1, 2)
                await json_result(session, "pattern_create", {
                    "name": "After Clear", "points": [[0, 0], [25, 0], [0, 25]],
                })
                assert (await json_result(session, "scene_info"))["patterns"][0]["fabric_index"] == 1
                await capture("recreated", [0], 1, 2)
                await json_result(session, "scene_clear", {"keep_avatar": False})
                await capture("reset", [], 0, 1)
            finally:
                await json_result(session, "scene_open", request)
    for case, reference in reports["official"].items():
        assert reports["rust"][case] == reference, (case, reports["rust"][case], reference)
    print(json.dumps({"tool": "fabric_list", "fixture": fixture.name,
                      "catalog_fields_exact": True, "cases": reports["rust"]}, ensure_ascii=False))


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


async def compare_saved_pattern_children():
    await compare_saved_pattern_outline(include_children=True)


def compare_child_shapes(expected, observed, panel):
    assert expected[0] == observed[0], (panel, expected[0], observed[0])
    assert len(expected) == len(observed), panel
    count, largest = 0, 0.0
    for left, right in zip(expected[1:], observed[1:], strict=True):
        for key in ("Shape index", "Shape name", "Shape type", "PointList"):
            assert left[key] == right[key], (panel, left["Shape index"], key)
        assert left["LineList"][0] == right["LineList"][0], panel
        for a, b in zip(left["LineList"][1:], right["LineList"][1:], strict=True):
            assert a["Line index"] == b["Line index"] and a["Line type"] == b["Line type"], panel
            delta = abs(a["Line length"] - b["Line length"])
            assert delta <= 0.1, (panel, left["Shape index"], a["Line index"], delta)
            largest = max(largest, delta)
            count += 1
    return len(expected) - 1, count, largest


async def compare_saved_pattern_outline(include_children=False):
    async def read(session):
        listing = await json_result(session, "pattern_list", {"detail": True})
        assert listing["count"] == len(listing["patterns"]) > 0
        panels = [
            await json_result(session, "pattern_info", {"pattern": str(i)})
            for i in range(listing["count"])
        ]
        missing = await session.call_tool("pattern_info", {"pattern": "__clo_verify_missing_panel__"})
        assert missing.isError
        return listing, panels

    async with open_backend("official") as session:
        await inspect_contract(session)
        scene = await json_result(session, "scene_info")
        reference, expected_panels = await read(session)
    project = Path(scene["project_path"])
    assert project.is_file() and project.suffix.lower() == ".zprj", project
    async with open_backend("rust") as session:
        await inspect_contract(session)
        await json_result(session, "scene_open", {"path": str(project)})
        actual, observed_panels = await read(session)

    assert actual["count"] == reference["count"]
    checked = 0
    length_error = 0.0
    midpoint_error = 0.0
    child_count = child_lines = 0
    child_error = 0.0
    for expected, observed in zip(expected_panels, observed_panels, strict=True):
        assert expected["name"] == observed["name"] and expected["index"] == observed["index"]
        name, index = expected["name"], expected["index"]
        expected_input, = expected["input_info"]["Pattern InputInformation"]
        observed_input, = observed["input_info"]["Pattern InputInformation"]
        if include_children:
            shapes, segments, error = compare_child_shapes(
                expected_input["Children Information"], observed_input["Children Information"], name,
            )
            child_count += shapes
            child_lines += segments
            child_error = max(child_error, error)
        assert expected_input["PointList"] == observed_input["PointList"], name
        assert expected["line_info"]["lineCount"] == observed["line_info"]["lineCount"], name
        expected_bounds = expected["line_info"]["boundingBox"]
        observed_bounds = observed["line_info"]["boundingBox"]
        assert expected_bounds.keys() == observed_bounds.keys(), name
        for key, value in expected_bounds.items():
            assert abs(value - observed_bounds[key]) <= 0.001, (name, key)
        for expected_line, observed_line in zip(
            expected_input["LineList"][1:], observed_input["LineList"][1:], strict=True,
        ):
            assert expected_line["Line index"] == observed_line["Line index"], name
            assert expected_line["Line type"] == observed_line["Line type"], (name, expected_line, observed_line)
            assert abs(expected_line["Line length"] - observed_line["Line length"]) <= 0.1
        for e, o, listed_e, listed_o in zip(
            expected["line_info"]["lines"], observed["line_info"]["lines"],
            reference["patterns"][index]["lines"], actual["patterns"][index]["lines"], strict=True,
        ):
            for key in ("lineIndex", "location", "orientation", "start", "end"):
                assert e[key] == o[key], (name, e["lineIndex"], key, e[key], o[key])
            for axis in ("x", "y"):
                delta = abs(e["mid"][axis] - o["mid"][axis])
                assert delta <= 0.001, (name, e["lineIndex"], axis, delta)
                midpoint_error = max(midpoint_error, delta)
                assert abs(e["midNorm"][axis] - o["midNorm"][axis]) <= 0.001, (name, e["lineIndex"], axis)
            delta = abs(e["length"] - o["length"])
            assert delta <= 0.1, (name, e["lineIndex"], delta)
            length_error = max(length_error, delta)
            for key in ("line", "location", "start", "end"):
                assert listed_e[key] == listed_o[key], (name, key)
            assert abs(listed_e["length"] - listed_o["length"]) <= 0.100001
            checked += 1
    result = {
        "tools": ["pattern_info", "pattern_list"], "case": "saved outer outline",
        "patterns": actual["count"], "lines": checked, "max_length_error_mm": length_error,
        "max_midpoint_axis_error_mm": midpoint_error, "missing_panel_error": "pass",
    }
    if include_children:
        assert child_count > 0 and child_lines > 0, "use a project with saved interior shapes"
        result.update(child_shapes=child_count, child_lines=child_lines,
                      max_child_length_error_mm=child_error, case="saved outline and child shapes")
    print(json.dumps(result))


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
    assert official == rust, (official, rust)
    assert official_panels["seam_groups"] == rust_panels["seam_groups"] == rust["count"]
    for expected, observed in zip(official_panels["patterns"], rust_panels["patterns"], strict=True):
        assert expected["name"] == observed["name"]
        assert expected["seam_groups"] == observed["seam_groups"], (
            expected["name"], expected["seam_groups"], observed["seam_groups"],
        )
    print(json.dumps({"tool": "seams_list", "ordered_groups_matched": rust["count"],
                      "panel_memberships_matched": rust_panels["count"]}))


async def compare_native_sewing():
    fixture = Path(os.environ["CLO_SEW_FIXTURE"])
    reference_archive = os.environ.get("CLO_SEW_REFERENCE_ARCHIVE")
    include_self = os.environ.get("CLO_SEW_CONFIRM_SELF") == "1"
    assert not reference_archive or include_self, "the native reference includes a same-edge seam"
    join = {"pattern_a": "Edit Front", "line_a": 0, "pattern_b": "Edit Back", "line_b": 0,
            "dir_a": True, "dir_b": False}
    cases = [
        ("existing_group", join | {"pattern_a": "Back", "pattern_b": "Front"}, True, 0),
        ("join", join, True, 1),
        ("duplicate", join, True, 0),
        ("change_duplicate_directions", join | {"dir_a": False, "dir_b": True}, True, 0),
        ("reverse_duplicate", join | {"pattern_a": "Edit Back", "pattern_b": "Edit Front",
                                     "dir_a": False, "dir_b": True}, True, 0),
        ("share_edge_with_third_panel", join | {"pattern_b": "Third", "dir_a": False, "dir_b": False}, True, 1),
        ("share_edge_with_other_line", join | {"line_b": 1, "dir_a": False}, True, 1),
        ("same_panel", join | {"pattern_b": "Edit Front", "line_a": 2, "line_b": 3,
                               "dir_a": False, "dir_b": True}, True, 1),
    ]
    if include_self:
        cases.append(("same_edge", join | {"pattern_b": "Edit Front", "line_a": 1, "line_b": 1}, True, 1))
    cases.extend([
        ("out_of_range_line", join | {"line_a": 90}, False, 0),
        ("negative_line", join | {"line_a": -1}, False, 0),
        ("missing_panel", join | {"pattern_b": "no-such-panel"}, None, 0),
        ("negative_panel_index", join | {"pattern_a": "-1"}, False, 0),
        ("high_panel_index", join | {"pattern_a": "99"}, False, 0),
        ("overflowing_line", join | {"line_b": 2**31}, None, 0),
        ("numeric_panel_indices", join | {"pattern_a": "04", "line_a": 2, "pattern_b": "2", "line_b": 3}, True, 1),
    ])

    def public_groups(listing):
        assert listing["count"] == len(listing["groups"])
        assert listing.keys() == {"count", "groups"}
        assert all(group.keys() == {"index", "name", "patterns"} for group in listing["groups"])
        return listing["groups"]

    recorded = {}
    backend_names = {}
    for backend in ("official", "rust"):
        async with open_backend(backend) as session:
            current = await json_result(session, "scene_info")
            path = str(fixture if backend == "official" else fixture.resolve(strict=True))
            await json_result(session, "scene_open", {"path": path})
            try:
                baseline = public_groups(await json_result(session, "seams_list"))
                assert len(baseline) == 1 and baseline[0]["patterns"] == ["Front", "Back"], baseline
                initial = [group["name"] for group in baseline]
                for name, x in (("Edit Front", 0), ("Edit Back", 180), ("Third", 360)):
                    await json_result(session, "pattern_create", {
                        "name": name, "points": [[x, 150], [x + 100, 150], [x + 90, 220], [x + 10, 230]],
                    })
                await json_result(session, "pattern_edit_points", {
                    "pattern": "Edit Front", "moves": [{"point": 1, "dx": 10, "dy": 3}],
                })
                before, observations = baseline, {}
                for label, args, ok, added in cases:
                    result = await session.call_tool("sew", args)
                    if ok is None:
                        assert result.isError, (backend, label, result.content)
                        reply = {"is_error": True}
                    else:
                        assert not result.isError, (backend, label, result.content)
                        reply = result.structuredContent or json.loads(result.content[0].text)
                        assert reply == {"ok": ok, "seam_groups": len(before) + added}, (backend, label, reply)
                    after = public_groups(await json_result(session, "seams_list"))
                    assert len(after) == len(before) + added and after[:len(before)] == before, (backend, label, after)
                    observations[label] = {"request": args, "reply": reply, "memberships": [
                        {"index": group["index"], "patterns": group["patterns"]} for group in after
                    ]}
                    before = after
                names = [group["name"] for group in after]
                assert len(set(names)) == len(names) and all(re.fullmatch(r"Sewing_\d+", name) for name in names[1:])
                api = await json_result(session, "clo_api_call", {
                    "name": "pattern_api.GetSeamlinePairGroupIndexFromName", "args": [names[-1]],
                })
                assert api["value"] == len(names) - 1
                for pattern in ("Edit Front", "Edit Back", "Third"):
                    expected = [group["index"] for group in after if pattern in group["patterns"]]
                    info = await json_result(session, "pattern_info", {"pattern": pattern})
                    assert info["seam_groups"] == expected
                if backend == "rust":
                    with TemporaryDirectory(prefix="clo-sew-live-") as directory:
                        portable = Path(directory) / "joined.clorust"
                        await json_result(session, "scene_save", {"path": str(portable), "thumbnail": False})
                        seams = json.loads(portable.read_text())["scene"]["seams"]
                        await json_result(session, "scene_open", {"path": str(portable)})
                        assert public_groups(await json_result(session, "seams_list")) == after
                    rust_edges = [
                        [(seam[f"pattern_{side}"], seam[f"line_{side}"], seam[f"dir_{side}"]) for side in ("a", "b")]
                        for seam in seams
                    ]
                await json_result(session, "pattern_delete", {"patterns": ["Third"]})
                retained = [group for group in after if "Third" not in group["patterns"]]
                expected = [group | {"index": index} for index, group in enumerate(retained)]
                assert public_groups(await json_result(session, "seams_list")) == expected
                recorded[backend] = observations
                backend_names[backend] = {"original": initial, "all": names}
            finally:
                if backend == "official":
                    await json_result(session, "scene_open", {"path": current["project_path"]})
    assert recorded["official"] == recorded["rust"], recorded
    assert backend_names["official"]["original"] == backend_names["rust"]["original"]
    if reference_archive:
        with TemporaryDirectory(prefix="clo-sew-reference-") as directory:
            async with open_backend("rust") as session:
                await json_result(session, "scene_open", {"path": str(Path(reference_archive).resolve(strict=True))})
                path = Path(directory) / "official.clorust"
                await json_result(session, "scene_save", {"path": str(path), "thumbnail": False})
                groups = json.loads(path.read_text())["scene"]["native_seam_groups"]
        archived_edges = []
        for group in groups:
            pair, = group["segments"]
            assert group["restored"] and all(side["start_line"] == side["end_line"] for side in pair)
            archived_edges.append([(side["pattern_id"], side["start_line"], side["direction"]) for side in pair])
        assert archived_edges == rust_edges, (archived_edges, rust_edges)
    print(json.dumps({"fixture": fixture.name, "cases": recorded["rust"], "names": backend_names,
                      "directions": rust_edges, "native_archive_directions_matched": bool(reference_archive),
                      "portable_reopen": True, "delete_removed_only_incident_groups": True}))


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
    capture = os.environ.get("CLO_ARRANGEMENT_REFERENCE")
    if capture:
        reference = json.loads(Path(capture).read_text())
        scene = reference["scene"]
        reference_rows = reference["arrangements"]["points"]
        official = {"count": len(reference_rows), "names": [point["ArrangementName"] for point in reference_rows]}
    else:
        async with open_backend("official") as session:
            await inspect_contract(session)
            scene = await json_result(session, "scene_info")
            official = await json_result(session, "arrangement_points")
            reference_rows = (await json_result(session, "clo_api_call", {"name": "pattern_api.GetArrangementList"}))["value"]
    assert scene["avatars"], "open a saved CLO project with an avatar"
    project = Path(scene["project_path"])
    assert project.is_file() and project.suffix.lower() == ".zprj", project
    async with open_backend("rust") as session:
        await inspect_contract(session)
        await json_result(session, "scene_open", {"path": str(project)})
        rust_scene = await json_result(session, "scene_info")
        rust = await json_result(session, "arrangement_points")
        rust_rows = (await json_result(session, "clo_api_call", {"name": "pattern_api.GetArrangementList"}))["value"]
        assert rust_rows == reference_rows, [(a, b) for a, b in zip(rust_rows, reference_rows) if a != b][:5]
        point = reference_rows[0]
        edit = await json_result(session, "pattern_set_arrangement", {"pattern": "0", "point": point["ArrangementName"]})
        expected = {key: value for key, value in point.items() if key != "ArrangementIndex"}
        assert edit == {"index": 0, "arrangement": expected}
        await json_result(session, "clo_api_call", {"name": "pattern_api.SetArrangement",
                                                   "args": [0, int(reference_rows[-1]["ArrangementIndex"])]})
        info = await json_result(session, "pattern_info", {"pattern": "0"})
        assert info["arrangement"] == {key: value for key, value in reference_rows[-1].items()
                                        if key != "ArrangementIndex"}
    assert rust_scene["avatars"] == scene["avatars"]
    assert official["count"] == len(official["names"]) > 0
    assert rust == official, (official, rust)
    print(json.dumps({
        "tool": "arrangement_points", "project": scene["project"],
        "avatars": scene["avatars"], "ordered_names_matched": rust["count"],
        "full_records_matched": len(rust_rows), "rust_name_and_index_readback": "pass",
        "reference": "saved capture" if capture else "live CLO",
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
    assert rust["source"].startswith("approximate measurements from the avatar mesh")
    assert official["body"].keys() == rust["body"].keys()
    fields = {"height"} if height_only else set(official["body"]) - {"source"}
    tolerance_mm = 0.25
    differences = {
        key: round(rust["body"][key] - official["body"][key], 6)
        for key in sorted(fields)
    }
    assert all(abs(value) <= tolerance_mm for value in differences.values()), differences
    if not height_only:
        assert official["source"] == rust["source"]
        assert official["body"]["source"] == rust["body"]["source"]
    print(json.dumps({
        "tool": "avatar_measurements", "case": "geometry fallback",
        "avatar": rust["avatar"], "numeric_fields": len(fields),
        "body_mm": {key: rust["body"][key] for key in sorted(fields)},
        "tolerance_mm": tolerance_mm, "differences_mm": differences,
    }, ensure_ascii=False))


async def compare_saved_avatar_cache():
    preview_args = {"spec": {"type": "tshirt"}, "use_scene_measurements": True}
    async with open_backend("official") as session:
        await inspect_contract(session)
        scene = await json_result(session, "scene_info")
        official = await json_result(session, "avatar_measurements")
        official_preview = await json_result(session, "garment_plan_preview", preview_args)
    assert official["body"]["source"] == "clo", official["source"]
    assert official["source"].startswith("cached CLO measurements ("), official["source"]
    project = Path(scene["project_path"])
    assert project.is_file() and project.suffix.lower() == ".zprj", project
    async with open_backend("rust") as session:
        await inspect_contract(session)
        await json_result(session, "scene_open", {"path": str(project)})
        rust = await json_result(session, "avatar_measurements")
        refreshed = await json_result(session, "avatar_measurements", {"refresh": True})
        assert refreshed["avatar"] == rust["avatar"]
        assert not refreshed["source"].startswith("cached CLO measurements (")
        assert await json_result(session, "avatar_measurements") == rust
        preview = await json_result(session, "garment_plan_preview", preview_args)
    assert official == rust, {
        key: (official["body"].get(key), rust["body"].get(key))
        for key in official["body"].keys() | rust["body"].keys()
        if official["body"].get(key) != rust["body"].get(key)
    }
    assert official_preview["measurement_source"] == preview["measurement_source"] == rust["source"]
    print(json.dumps({
        "tool": "avatar_measurements", "case": "saved measurement cache",
        "avatar": rust["avatar"], "source": rust["source"],
        "numeric_fields_matched": len(rust["body"]) - 1,
        "refresh_bypassed_cache": True,
        "draft_preview_used_cache": True,
    }, ensure_ascii=False))


async def compare_saved_snapshot_2d():
    output_dir = os.environ.get("CLO_SNAPSHOT_2D_OUTPUTS")
    compare_triangles = os.environ.get("CLO_SNAPSHOT_2D_GEOMETRY") == "triangles"
    curve_reference_dir = os.environ.get("CLO_SNAPSHOT_2D_CURVES")

    def inspect_outline(png, extent, probes, interiors=()):
        from io import BytesIO
        from PIL import Image

        image = Image.open(BytesIO(png))
        alpha = image.getchannel("A")
        left, top, right, bottom = alpha.getbbox()
        x0, y0, x1, y1 = extent
        scale = (right - left - 1) / (x1 - x0)
        expected_height = (y1 - y0) * scale
        assert abs(expected_height - (bottom - top - 1)) <= 3

        def screen(x, y):
            return left + (x - x0) * scale, bottom - 1 - (y - y0) * scale

        mask = alpha.load()
        for point in interiors:
            assert mask[tuple(map(round, screen(*point)))] == 0
        max_delta = 0.0
        for point in probes:
            u, v = screen(*point)
            nearby = [math.hypot(i - u, j - v)
                      for i in range(max(0, round(u) - 3), min(image.width, round(u) + 4))
                      for j in range(max(0, round(v) - 3), min(image.height, round(v) + 4))
                      if mask[i, j] > 0]
            assert nearby and min(nearby) <= 3, point
            max_delta = max(max_delta, min(nearby) / scale)
        return {"outline_probes": len(probes), "max_probe_delta_mm": max_delta,
                "transparent_interiors": len(interiors)}

    def inspect_triangles(png, layout):
        patterns = layout["patterns"]
        assert len(patterns) == 2 and all(len(pattern["lines"]) == 3 for pattern in patterns)
        triangles = [[(line["start"]["x"], line["start"]["y"]) for line in pattern["lines"]]
                     for pattern in patterns]
        x, y = zip(*(point for triangle in triangles for point in triangle), strict=True)
        interior = [tuple(sum(axis) / 3 for axis in zip(*triangle, strict=True)) for triangle in triangles]
        probes = [tuple(p + (q - p) * n / 16 for p, q in zip(a, b, strict=True))
                  for triangle in triangles for a, b in zip(triangle, triangle[1:] + triangle[:1], strict=True)
                  for n in range(17)]
        return inspect_outline(png, (min(x), min(y), max(x), max(y)), probes, interior)

    async def capture(session, backend, triangles=False):
        scene = await json_result(session, "scene_info")
        async with asyncio.timeout(60):
            result = await successful(session, "snapshot_2d")
        assert [block.type for block in result.content] == ["text", "image"], result.content
        caption, image = result.content
        assert caption.text.startswith("2D snapshot: ") and image.mimeType == "image/png"
        png = base64.b64decode(image.data, validate=True)
        assert png.startswith(b"\x89PNG\r\n\x1a\n") and png[12:16] == b"IHDR"
        dimensions = struct.unpack_from(">II", png, 16)
        assert len(png) > 1000 and all(dimensions)
        assert await json_result(session, "scene_info") == scene
        if output_dir:
            target = Path(output_dir)
            target.mkdir(parents=True, exist_ok=True)
            (target / f"{backend}.png").write_bytes(png)
        details = {}
        if triangles:
            details = inspect_triangles(png, await json_result(session, "pattern_list", {"detail": True}))
        return scene, dimensions, details, png

    async with open_backend("official") as session:
        await inspect_contract(session)
        scene, official, official_geometry, _ = await capture(session, "official", compare_triangles)
    assert scene["pattern_count"] > 0
    project = Path(scene["project_path"])
    assert project.is_file() and project.suffix.lower() == ".zprj", project
    async with open_backend("rust") as session:
        await inspect_contract(session)
        await json_result(session, "scene_open", {"path": str(project)})
        rust_scene, rust, rust_geometry, _ = await capture(session, "rust", compare_triangles)
        curves = {}
        if curve_reference_dir:
            directory = Path(curve_reference_dir).resolve(strict=True)
            await json_result(session, "scene_open", {"path": str(directory / "snapshot-layout-curved.zpac")})
            _, _, _, rust_png = await capture(session, "rust-curves")
            native_png = (directory / "layout-curved-official.png").read_bytes()
            keys = [(34, 20), (50, 35), (66, 20), (290, 0), (330, 65), (370, 0), (330, 0)]
            for backend, png in (("official", native_png), ("rust", rust_png)):
                curves[backend] = inspect_outline(png, (0, 0, 370, 80), keys, [(210, 25)])
        await json_result(session, "scene_clear", {"keep_avatar": False})
        empty = await successful(session, "snapshot_2d")
        message, = empty.content
        assert message.type == "text" and message.text.startswith("2D snapshot failed:")
        assert (await json_result(session, "scene_info"))["pattern_count"] == 0
    assert scene["patterns"] == rust_scene["patterns"]
    print(json.dumps({
        "tool": "snapshot_2d", "pattern_count": scene["pattern_count"],
        "dimensions": {"official": official, "rust": rust}, "scene_readback": "pass",
        "geometry": {"official": official_geometry, "rust": rust_geometry},
        "curves": curves, "empty_rust_capture": "text_only",
    }))


async def compare_native_snapshot_views():
    from io import BytesIO
    from pathlib import PurePosixPath
    from PIL import Image

    fixture = Path(os.environ["CLO_SNAPSHOT_3D_FIXTURE"])
    native_folder = os.environ["CLO_SNAPSHOT_3D_NATIVE_OUTPUT"]
    output = os.environ.get("CLO_SNAPSHOT_3D_OUTPUTS")
    views = ["FRONT", "back", "left", "right", "top", "bottom", "3/4-left", "3/4-right", "Three-Quarter"]
    report = {}

    async def capture(session, requested=None, label=""):
        args = {"views": requested, "label": label} if requested is not None else {}
        response = await successful(session, "snapshot", args)
        expected = requested or ["front", "3/4-right"]
        assert len(response.content) == 2 * len(expected), response.content
        directory = None
        result = []
        for index, (view, caption, content) in enumerate(zip(expected, response.content[::2], response.content[1::2], strict=True)):
            assert caption.type == "text" and caption.text.startswith(f"[{view} view] ")
            path = PurePosixPath(caption.text.split("] ", 1)[1])
            assert path.name.startswith(f"{(label or 'snap').replace(' ', '_')}_{view.replace('/', '')}_")
            assert content.type == "image" and content.mimeType == "image/png"
            raw = base64.b64decode(content.data, validate=True)
            image = Image.open(BytesIO(raw))
            assert image.format == "PNG"
            directory = path.parent
            if output and label == "Camera Matrix":
                target = Path(output)
                target.mkdir(parents=True, exist_ok=True)
                (target / f"{backend}-{index}-{view.replace('/', '')}.png").write_bytes(raw)
            rgb = image.convert("RGB")
            channels = iter(rgb.tobytes())
            mask = Image.frombytes("L", rgb.size, bytes(
                255 if blue > red + 35 and blue > green + 30 else 0
                for red, green, blue in zip(channels, channels, channels, strict=True)
            ))
            box = mask.getbbox()
            result.append({"sha": hashlib.sha256(raw).hexdigest(), "size": image.size,
                           "cloth_bounds": box, "cloth_pixels": mask.histogram()[255]})
        return directory, result

    async def run_python(session, code):
        return (await json_result(session, "clo_run_python", {"code": code}))["value"]

    async def artifact_hashes(session, directory, prefix):
        return await run_python(session, f"import pathlib, hashlib\n"
            f"result = sorted(hashlib.sha256(p.read_bytes()).hexdigest() for p in "
            f"pathlib.Path({str(directory)!r}).glob({(prefix + '*.png')!r}))")

    async def thumbnail(session, directory):
        path = str(directory / "camera-retained.png")
        return await run_python(session, f"import pathlib, hashlib\n"
            f"export_api.ExportThumbnail3D({path!r})\n"
            f"result = hashlib.sha256(pathlib.Path({path!r}).read_bytes()).hexdigest()")

    for backend in ("official", "rust"):
        async with open_backend(backend) as session:
            await inspect_contract(session)
            original = await json_result(session, "scene_info")
            path = str(fixture if backend == "official" else fixture.resolve(strict=True))
            await json_result(session, "scene_open", {"path": path})
            try:
                scene = await json_result(session, "scene_info")
                directory, frames = await capture(session, views, "Camera Matrix")
                _, defaults = await capture(session, [], "")
                assert [item["sha"] for item in defaults] == [frames[0]["sha"], frames[-2]["sha"]]
                assert frames[-1] == frames[-2]
                assert len({row["sha"] for row in frames[:-1]}) == len(views) - 1
                assert all(row["cloth_pixels"] > 100 for row in frames), (backend, frames)

                for angle, index in (("right", 3), ("front", 0)):
                    _, commands = await capture(session, [angle, "fit", "focus"], "Camera Commands")
                    assert all(frame == frames[index] for frame in commands), (backend, angle, commands)
                    for preset in (9, 7):
                        await json_result(session, "clo_api_call", {
                            "name": "utility_api.SetCamViewPoint", "args": [preset],
                        })
                        assert await thumbnail(session, directory if backend == "rust" else PurePosixPath(native_folder)) == frames[index]["sha"]
                await capture(session, ["right"])
                async def rejected(prefix, request, written):
                    existing = await artifact_hashes(session, directory, prefix)
                    invalid = await session.call_tool("snapshot", {"views": request, "label": prefix})
                    assert invalid.isError, (backend, prefix, invalid.content)
                    observed = await artifact_hashes(session, directory, prefix)
                    assert observed == sorted([*existing, *written]), (backend, observed, existing)
                retain = directory if backend == "rust" else PurePosixPath(native_folder)
                await rejected("bad-first-camera", ["2d", "front"], [])
                assert await thumbnail(session, retain) == frames[3]["sha"]
                await rejected("bad-later-camera", ["front", "missing-camera", "back"], [frames[0]["sha"]])
                assert await thumbnail(session, retain) == frames[0]["sha"]
                assert await json_result(session, "scene_info") == scene
                report[backend] = dict(zip(views, frames, strict=True))
            finally:
                if backend == "official" and original["project_path"] != "NULL":
                    await json_result(session, "scene_open", {"path": original["project_path"]})

    def aspect(frame):
        left, top, right, bottom = frame["cloth_bounds"]
        return (right - left) / (bottom - top)

    aspect_deltas = {
        view: abs(aspect(report["official"][view]) - aspect(report["rust"][view]))
        for view in views
    }
    assert max(aspect_deltas[view] for view in ("FRONT", "back", "left", "right")) <= 0.01, aspect_deltas
    print(json.dumps({"fixture": fixture.name, "views": report, "focus_fit_preserve_view_without_selection": True,
                      "cloth_aspect_deltas": aspect_deltas,
                      "invalid_first_preserves_camera": True, "invalid_later_keeps_file_and_camera": True}))


async def compare_draft_option_semantics():
    accepted = (
        {"garment": "top", "fit": "bespoke", "length": "waist"},
        {"garment": "Blouse", "length": "waist"},
        {"silhouette": "fit-and-flare", "waist": "bespoke"},
        {"silhouette": ""},
        {"silhouette": "ball-gown"},
        {"garment": "top", "silhouette": "slip"},
        {"garment": "top", "length": "waist", "ease": {"hip": 120}},
        {"garment": "top", "ease": {"hip": 120}},
        {"garment": "skirt", "ease": {"hip": 120, "waist": 155}},
        {"length": "midi", "simulate_steps": "20", "particle_distance": " 10.5 ", "flare": "0.45"},
        {"length": "midi", "simulate_steps": 20.7, "particle_distance": 15},
    )
    rejected = (
        {"particle_distance": "invalid"}, {"simulate_steps": "20.5"}, {"flare": "wide"},
        {"garment": "top", "length": "waist", "ease": {"bust": "40"}},
    )
    results = []
    async with open_backend("official") as clo, open_backend("rust") as rust:
        for session in (clo, rust):
            await inspect_contract(session)
        for spec in accepted:
            args = {"spec": spec, "use_scene_measurements": False}
            reference = await json_result(clo, "garment_plan_preview", args)
            observed = await json_result(rust, "garment_plan_preview", args)
            assert reference.keys() == observed.keys(), spec
            for key in reference:
                assert reference[key] == observed[key], (spec, key, reference[key], observed[key])
            results.append({"spec": spec, "pieces": len(observed["pieces"]),
                            "seams": len(observed["seams"]), "simulation": observed["simulation"]})
        for session in (clo, rust):
            for spec in rejected:
                assert (await session.call_tool("garment_plan_preview", {"spec": spec})).isError, spec
            assert await json_result(session, "garment_last_spec") == {"spec": None, "pieces": []}
    print(json.dumps({"tool": "garment_plan_preview", "case": "draft option semantics",
                      "accepted": results, "rejected": rejected, "last_spec_unchanged": True}))


async def compare_strapped_draft_preview():
    cases = (
        {"silhouette": "strapless", "length": "mini"},
        {"silhouette": "tube", "length": "knee"},
        {"silhouette": "slip", "length": "midi"},
        {"garment": "top", "neckline": "sweetheart", "strapless": True, "sleeve": "long"},
        {"garment": "top", "neckline": "sweetheart", "straps": "wide", "sleeve": "long", "length": "waist"},
        {"garment": "top", "neckline": "straight", "length": "cropped"},
        {"silhouette": "slip", "straps": "none", "length": "mini"},
    )
    rows = []
    async with open_backend("official") as clo, open_backend("rust") as rust:
        for session in (clo, rust):
            await inspect_contract(session)
        previous_specs = [await json_result(session, "garment_last_spec") for session in (clo, rust)]
        for spec in cases:
            args = {"spec": spec, "use_scene_measurements": False}
            expected = await json_result(clo, "garment_plan_preview", args)
            observed = await json_result(rust, "garment_plan_preview", args)
            assert expected.keys() == observed.keys(), spec
            for key in expected:
                assert expected[key] == observed[key], (spec, key, expected[key], observed[key])
            rows.append({"pieces": len(observed["pieces"]), "seams": len(observed["seams"])})
        for session, previous in zip((clo, rust), previous_specs, strict=True):
            assert (await session.call_tool("garment_plan_preview", {"spec": []})).isError
            assert await json_result(session, "garment_last_spec") == previous
    print(json.dumps({"tool": "garment_plan_preview", "case": "strap styles", "comparisons": rows,
                      "last_spec_unchanged": True, "invalid_spec_error": "pass"}))


async def compare_neckline_and_sleeve_preview():
    fixture = {"garment": "top", "length": "waist"}
    cases = [
        {"neckline": "off-shoulder"},
        {"neckline": "halter"},
        {"neckline": "off-shoulder", "back_neckline": "boat"},
        {"neckline": "custom neckline", "back_neckline": "custom back"},
        {"sleeve": "long", "sleeve_style": "bishop"},
        {"sleeve": "long", "sleeve_style": "flutter"},
        {"sleeve": "long", "sleeve_style": "bell"},
        {"sleeve": "long", "sleeve_style": "puff"},
    ]
    report = []
    async with open_backend("official") as official, open_backend("rust") as rust:
        for session in (official, rust):
            await inspect_contract(session)
        for changes in cases:
            request = {"spec": fixture | changes, "use_scene_measurements": False}
            expected = await json_result(official, "garment_plan_preview", request)
            observed = await json_result(rust, "garment_plan_preview", request)
            assert expected.keys() == observed.keys(), changes
            for key in expected:
                assert expected[key] == observed[key], (changes, key, expected[key], observed[key])
            report.append(changes | {"pieces": len(observed["pieces"]), "warnings": len(observed["warnings"])})
        for session in (official, rust):
            bad = await session.call_tool("garment_plan_preview", {"spec": []})
            assert bad.isError
            assert await json_result(session, "garment_last_spec") == {"spec": None, "pieces": []}
    print(json.dumps({"tool": "garment_plan_preview", "case": "necklines and sleeve seam warnings",
                      "comparisons": report, "last_spec_unchanged": True, "invalid_spec_error": "pass"}))


async def compare_draft_material_preview():
    cases = [
        ("cotton", "#ABc"),
        ("lightweight denim", "rose gold"),
        ("double gauze cotton", "hot pink"),
        ("matte jersey", "red sky blue"),
        ("silk organza", "navy gold"),
        ("linen", [12, 64, 155]),
        ("precise weave", [0.25, "0.5", 0.75, 1.0]),
        ("V2_Woven_Denim_1", "mauve"),
        ("not-present.zfab", "#not-a-hex-color"),
        ("  Future/Swatch.ZFAB  ", [0.25, 0.5]),
        (None, None),
    ]
    invalid_cases = [
        {"fabric": ["cotton"]},
        {"color": "#xyz"},
        {"color": ["not a number", 0, 1]},
    ]
    base = {"garment": "top", "length": "waist"}
    report = []
    with TemporaryDirectory(prefix="clo-preview-material-") as directory:
        root = Path(directory)
        (root / "V2_Woven_Poplin_1.zfab").write_text("Invalid test asset")
        environment = {"CLO_RUST_FABRIC_DIR": directory, "CLO_RUST_AVATAR_DIR": ""}
        async with open_backend("official") as official, open_backend("rust", environment) as rust:
            for session in (official, rust):
                await inspect_contract(session)
            for fabric, color in cases:
                request = {"spec": base | {"fabric": fabric, "color": color}}
                expected = await json_result(official, "garment_plan_preview", request)
                observed = await json_result(rust, "garment_plan_preview", request)
                assert expected.keys() == observed.keys(), request
                for key in expected:
                    assert expected[key] == observed[key], (request, key, expected[key], observed[key])
                report.append(observed["fabric"])
            for session in (official, rust):
                for spec in invalid_cases:
                    assert (await session.call_tool("garment_plan_preview", {"spec": base | spec})).isError, spec
                assert await json_result(session, "garment_last_spec") == {"spec": None, "pieces": []}
        build_environment = {
            "CLO_RUST_FABRIC_DIR": "",
            "CLO_RUST_AVATAR_DIR": "",
            "CLO_RUST_CHECKPOINT_DIR": str(root / "checkpoints"),
        }
        async with open_backend("rust", build_environment) as rust:
            request = {"spec": base | {"fabric": "cotton", "color": "rose gold"},
                       "simulate_steps": 0, "snapshots": False}
            built = await json_result(rust, "garment_build", request)
            expected_color = [0.83, 0.68, 0.22]
            assert built["fabric"]["color"] == expected_color, built["fabric"]
            assert (await json_result(rust, "garment_last_spec"))["spec"] == built["spec"]
            saved_path = root / "gold-top.json"
            await json_result(rust, "scene_save", {"path": str(saved_path), "thumbnail": False})
            state = json.loads(saved_path.read_text())["scene"]
            assert state["fabrics"][-1]["color"] == expected_color, state["fabrics"]
            assert all(panel["fabric_index"] == state["fabrics"][-1]["index"]
                       for panel in state["patterns"])
            before = await json_result(rust, "checkpoint_list")
            bad = await rust.call_tool("garment_rebuild", {"changes": {"fabric": str(root / "missing.zfab")}})
            assert bad.isError
            assert await json_result(rust, "checkpoint_list") == before
            assert (await json_result(rust, "garment_last_spec"))["spec"] == built["spec"]
    print(json.dumps({
        "tool": "garment_plan_preview", "case": "fabric and color",
        "materials": report, "invalid_cases": len(invalid_cases),
        "last_spec_unchanged": True, "rust_build_color_and_invalid_rebuild": "pass",
    }))


async def compare_gathered_draft_preview():
    cases = (
        {"garment": "skirt", "silhouette": "tiered", "tiers": 4, "gather_ratio": 1.25, "length": "midi"},
        {"garment": "skirt", "silhouette": "gathered", "waist": "high", "length": "knee"},
        {"silhouette": "empire", "length": "mini"},
        {"silhouette": "babydoll", "length": "knee", "sleeve": "short"},
        {"silhouette": "drop-waist", "gather_ratio": "1.55"},
        {"silhouette": "smock", "length": "midi"},
        {"silhouette": "dirndl", "length": "mini"},
        {"silhouette": "tiered", "length": "ankle", "tiers": 2, "gather_ratio": 1.5},
        {"silhouette": "wrap", "length": "knee"},
    )
    results = []
    async with open_backend("official") as clo, open_backend("rust") as rust:
        for session in (clo, rust):
            await inspect_contract(session)
        for spec in cases:
            request = {"spec": spec, "use_scene_measurements": False}
            official = await json_result(clo, "garment_plan_preview", request)
            observed = await json_result(rust, "garment_plan_preview", request)
            assert official.keys() == observed.keys(), spec
            for key in official:
                assert official[key] == observed[key], (spec, key, official[key], observed[key])
            results.append({
                "style": spec["silhouette"], "pieces": len(observed["pieces"]),
                "gathered_seams": sum(seam["gather"] for seam in observed["seams"]),
            })
        for session in (clo, rust):
            invalid = await session.call_tool("garment_plan_preview", {
                "spec": {"garment": "skirt", "skirt": "tiered", "tiers": 0},
            })
            assert invalid.isError
            assert await json_result(session, "garment_last_spec") == {"spec": None, "pieces": []}
    print(json.dumps({"tool": "garment_plan_preview", "case": "gathered and tiered skirts",
                      "comparisons": results, "last_spec_unchanged": True, "zero_tiers_error": "pass"}))


async def verify_tiered_build_relaxation():
    spec = {"garment": "skirt", "silhouette": "tiered", "tiers": 3,
            "length": "midi", "particle_distance": 100.0}
    request = {"spec": spec, "snapshots": False, "replace_existing": False}
    with TemporaryDirectory(prefix="clo-tier-relaxation-") as directory:
        root = Path(directory)
        environment = {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": "",
                       "CLO_RUST_CHECKPOINT_DIR": str(root / "checkpoints")}
        async with open_backend("rust", environment) as rust:
            await inspect_contract(rust)

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]

            async def exercise(steps, relax):
                await json_result(rust, "scene_clear", {"keep_avatar": False})
                await json_result(rust, "pattern_create", {
                    "name": "Existing", "points": [[0, 0], [100, 0], [40, 60]],
                })
                await json_result(rust, "pattern_ops", {"pattern": "Existing", "op": "layer",
                                                       "args": {"layer": 4}})
                source, _ = await save("before-build")
                manual = await json_result(rust, "garment_build", request | {"simulate_steps": 0})
                assert manual["simulation"] is None
                _, drafted = await save("drafted")
                assert [p["layer"] for p in drafted["patterns"]] == [4, 0, 0, 1, 1, 2, 2]
                assert len(drafted["seams"]) == len(manual["seams"])
                await json_result(rust, "simulate", {"steps": steps, "particle_distance": 100.0})
                _, settled = await save("settled")
                for panel in drafted["patterns"]:
                    await json_result(rust, "pattern_ops", {
                        "pattern": panel["name"], "op": "layer", "args": {"layer": 0},
                    })
                _, reset = await save("reset")
                assert reset["simulation"] == settled["simulation"], "reset must preserve the settled pose"
                await json_result(rust, "simulate", {"steps": relax})
                _, reference = await save("manual")

                await json_result(rust, "scene_open", {"path": str(source)})
                result = await json_result(rust, "garment_build", request | {"simulate_steps": steps})
                assert result["simulation"]["steps"] == steps
                assert result["simulation"]["extra_steps_after_layer_reset"] == relax
                assert result["seams"] == manual["seams"]
                path, automatic = await save("automatic")
                assert all(panel["layer"] == 0 for panel in automatic["patterns"])
                assert automatic["simulation"]["steps"] == steps + relax
                assert automatic["simulation"] == reference["simulation"]
                await json_result(rust, "scene_open", {"path": str(path)})
                await json_result(rust, "simulate", {"steps": 1})
                _, continued = await save("continued")
                assert continued["simulation"]["steps"] == steps + relax + 1
                return {"initial_steps": steps, "relaxation_steps": relax, "panels": len(automatic["patterns"])}

            cases = [await exercise(1, 30), await exercise(124, 31)]
            rebuilt = await json_result(rust, "garment_rebuild", {
                "changes": {"tiers": 2}, "simulate_steps": 1, "snapshots": False,
            })
            assert rebuilt["simulation"]["extra_steps_after_layer_reset"] == 30
            _, rebuilt_scene = await save("rebuilt")
            assert [panel["layer"] for panel in rebuilt_scene["patterns"]] == [0] * 4
            assert rebuilt_scene["simulation"]["steps"] == 31
            plain = await json_result(rust, "garment_build", {
                "spec": {"garment": "top", "length": "waist", "particle_distance": 100.0},
                "simulate_steps": 1, "snapshots": False,
            })
            assert "extra_steps_after_layer_reset" not in plain["simulation"]
    print(json.dumps({"tools": ["garment_build", "garment_rebuild", "pattern_ops", "simulate"],
                      "case": "tiered build relaxation", "comparisons": cases,
                      "manual_pose_and_resume": "pass", "rebuild_and_nonlayered_controls": "pass"}))


async def verify_scene_open_ui_state():
    with TemporaryDirectory(prefix="clo-open-scene-ui-") as directory:
        root = Path(directory)
        checkpoints = root / "checkpoints"
        async with open_backend("rust", {"CLO_RUST_CHECKPOINT_DIR": str(checkpoints)}) as rust:
            await inspect_contract(rust)

            async def save_single(name):
                await json_result(rust, "scene_clear", {"keep_avatar": False})
                await json_result(rust, "pattern_create", {"name": name, "points": [[0, 0], [80, 0], [0, 80]]})
                path = str(root / f"{name}.clorust")
                await json_result(rust, "scene_save", {"path": path})
                return path

            async def select(name):
                ui = await json_result(rust, "clo_ui_state")
                row = next(node for node in ui["nodes"] if node["AXRole"] == "AXRow" and node["AXTitle"] == name)
                reply = await json_result(rust, "clo_ui_action", {"snapshot": ui["snapshot"], "action": "press", "element": row["id"]})
                return {"snapshot": reply["state"]["snapshot"], "action": "set_value", "element": 8, "value": "Renamed"}

            async def selected():
                reply = await json_result(rust, "clo_api_call", {"name": "pattern_api.GetSelectedPattern"})
                return reply["value"]

            async def stale(action):
                result = await rust.call_tool("clo_ui_action", action)
                assert result.isError and any("Stale UI snapshot" in block.text for block in result.content)

            first = await save_single("First")
            second = await save_single("Second")
            await json_result(rust, "checkpoint_save", {"name": "second-only"})
            action = await select("Second")
            await json_result(rust, "scene_open", {"path": first, "mode": "replace"})
            await stale(action)
            assert await selected() == []
            action = await select("First")
            await json_result(rust, "scene_open", {"path": second, "mode": "append"})
            await stale(action)
            assert await selected() == ["First"]
            interchange = root / "patterns.json"
            await json_result(rust, "scene_export", {"path": str(interchange)})
            action = await select("First")
            await json_result(rust, "scene_open", {"path": str(interchange)})
            await stale(action)
            assert await selected() == ["First"]
            action = await select("First")
            await json_result(rust, "checkpoint_restore", {"name": "second-only"})
            await stale(action)
            assert await selected() == []

            action = await select("Second")
            before = await json_result(rust, "scene_info")
            bad = root / "invalid.clorust"
            bad.write_text("invalid project")
            assert (await rust.call_tool("scene_open", {"path": str(bad)})).isError
            assert (await rust.call_tool("checkpoint_restore", {"name": "missing"})).isError
            assert await json_result(rust, "scene_info") == before
            await json_result(rust, "clo_ui_action", action)
            assert await selected() == ["Renamed"]

            native = False
            if fixture := os.environ.get("CLO_CHECKPOINT_FIXTURE"):
                shutil.copyfile(fixture, checkpoints / "original.zprj")
                action = await select("Renamed")
                reply = await json_result(rust, "checkpoint_restore", {"name": "original"})
                assert reply["pattern_count"] > 0
                await stale(action)
                assert await selected() == []
                native = True
    print(json.dumps({"tools": ["scene_open", "checkpoint_restore", "clo_ui_action"],
                      "case": "scene changes invalidate old UI snapshots", "replace_clears_selection": True,
                      "pattern_json_keeps_selection": True,
                      "append_keeps_selection": True, "portable_restore_clears_selection": True,
                      "failed_load_preserves_snapshot": True, "native_restore_clears_selection": native}))


async def compare_native_scene_clear():
    fixture = Path(os.environ["CLO_SCENE_CLEAR_FIXTURE"]).resolve(strict=True)
    args = {"mode": "replace"}
    reports = {}
    asset_fields = {"asset_folder", "temp_folder", "clo_version"}

    def normalized(scene):
        assert scene["clo_version"] and scene["temp_folder"]
        result = {key: value for key, value in scene.items() if key not in asset_fields}
        if result["project_path"] != "NULL":
            assert Path(result["project_path"]).resolve() == fixture
            result["project_path"] = fixture.name
        return result

    async def count(session, used):
        request = {"name": "fabric_api.GetFabricCount", "args": [used]}
        return (await json_result(session, "clo_api_call", request))["value"]

    async with open_backend("official") as clo, open_backend("rust") as rust:
        status = await json_result(clo, "clo_status")
        assert status["bridge"] == "up", status
        opened = await json_result(clo, "scene_info")
        native_path = opened["project_path"]
        assert Path(native_path).resolve() == fixture, "open the saved fixture in the official session first"
        sessions = ((clo, native_path), (rust, str(fixture)))
        sparse = None
        try:
            for case, options in (("default", {}), ("keep", {"keep_avatar": True}), ("new", {"keep_avatar": False})):
                outcomes = []
                for session, path in sessions:
                    await json_result(session, "scene_open", {**args, "path": path})
                    before = await json_result(session, "scene_info")
                    raw = await count(session, False)
                    assert before["pattern_count"] > 0 and raw > 0
                    invalid = await session.call_tool("scene_clear", {"keep_avatar": []})
                    assert invalid.isError and await json_result(session, "scene_info") == before
                    result = await json_result(session, "scene_clear", options)
                    after = await json_result(session, "scene_info")
                    assert all(after[key] == before[key] for key in asset_fields)
                    assert after["pattern_count"] == after["seam_groups"] == after["fabric_count"] == 0
                    assert after["patterns"] == after["fabrics"] == [] and await count(session, True) == 0
                    raw_after = await count(session, False)
                    if case == "new":
                        assert result == {"pattern_count": 0, "avatars": []}
                        assert after["project"] == after["project_path"] == "NULL"
                        assert after["colorways"] == ["Default Colorway"] and after["current_colorway"] == 0
                        assert after["avatars"] == [] and raw_after == 1
                    else:
                        assert result == {"deleted": before["pattern_count"], "pattern_count": 0}
                        stable = ("project", "project_path", "avatars", "colorways", "current_colorway")
                        assert all(after[key] == before[key] for key in stable)
                        assert raw_after == raw
                        assert await json_result(session, "scene_clear", options) == {"deleted": 0, "pattern_count": 0}
                        assert await json_result(session, "scene_info") == after
                    recreated = None
                    if case != "default":
                        await json_result(session, "pattern_create", {
                            "name": "After Clear", "points": [[0, 0], [25, 0], [0, 25]],
                        })
                        recreated = normalized(await json_result(session, "scene_info"))
                        assert recreated["fabric_count"] == 1 and recreated["patterns"][0]["fabric_index"] == 0
                        assert await count(session, False) == raw_after
                    outcomes.append((normalized(before), result, normalized(after), raw_after, recreated))
                assert outcomes[0] == outcomes[1], (case, outcomes)
                before, result, after, raw_after, recreated = outcomes[1]
                reports[case] = {
                    "before": before, "clear": result, "after": after,
                    "stored_fabrics_after": raw_after, "invalid_input_preserved_scene": True,
                    "next_pattern": recreated,
                }
            if (len(opened["patterns"]) == 2
                    and [piece["fabric_index"] for piece in opened["patterns"]] == [1, 0]):
                readback = []
                for session, path in sessions:
                    await json_result(session, "scene_open", {**args, "path": path})
                    await json_result(session, "pattern_delete", {"patterns": [opened["patterns"][1]["name"]]})
                    after = normalized(await json_result(session, "scene_info"))
                    assert after["patterns"][0]["fabric_index"] == 1 and after["fabric_count"] == 1
                    assert after["fabrics"] == opened["fabrics"][:1] and await count(session, False) == 2
                    readback.append(after)
                assert readback[0] == readback[1]
                sparse = readback[1]
        finally:
            await json_result(clo, "scene_open", {**args, "path": native_path})
    with fixture.open("rb") as file:
        digest = hashlib.file_digest(file, "sha256").hexdigest()
    print(json.dumps({"fixture": fixture.name, "sha256": digest,
                      "clo_version": opened["clo_version"], "cases": reports, "sparse_fabric": sparse},
                     ensure_ascii=False))


async def verify_scene_clear_state():
    with TemporaryDirectory(prefix="clo-clear-scene-") as directory:
        path = str(Path(directory) / "before-clear.clorust")
        async with open_backend("rust", {"CLO_RUST_FABRIC_DIR": "", "CLO_RUST_AVATAR_DIR": ""}) as rust:
            await inspect_contract(rust)
            await successful(rust, "garment_build", {"spec": {"garment": "top"}, "simulate_steps": 1, "snapshots": False})
            added = await json_result(rust, "fabric_apply", {"fabric": "denim", "patterns": ["0"]})
            index = added["fabric"]["index"]
            assert index > 0
            await json_result(rust, "scene_save", {"path": path})
            before = await json_result(rust, "scene_info")
            total_fabrics = {"name": "fabric_api.GetFabricCount", "args": [False]}
            stored_count = (await json_result(rust, "clo_api_call", total_fabrics))["value"]
            history = await json_result(rust, "garment_last_spec")
            assert before["pattern_count"] > 0 and before["avatars"] and before["seam_groups"] > 0 and history["spec"]

            async def observed_action():
                snapshot = await json_result(rust, "clo_ui_state")
                return {"snapshot": snapshot["snapshot"], "action": "press", "element": 2}

            async def stale(action):
                result = await rust.call_tool("clo_ui_action", action)
                assert result.isError and any("Stale UI snapshot" in block.text for block in result.content)

            async def current_fabric():
                reply = await json_result(rust, "clo_api_call", {"name": "fabric_api.GetCurrentFabricIndex"})
                return reply["value"]

            action = await observed_action()
            keep = await json_result(rust, "scene_clear", {"keep_avatar": True})
            assert keep == {"deleted": before["pattern_count"], "pattern_count": 0}
            await stale(action)
            kept = await json_result(rust, "scene_info")
            assert kept["pattern_count"] == kept["seam_groups"] == 0
            for key in ("project_path", "avatars", "colorways"):
                assert kept[key] == before[key], key
            assert kept["fabrics"] == []
            assert (await json_result(rust, "clo_api_call", total_fabrics))["value"] == stored_count
            assert await current_fabric() == index

            await json_result(rust, "scene_open", {"path": path})
            action = await observed_action()
            assert await json_result(rust, "scene_clear", {"keep_avatar": False}) == {"pattern_count": 0, "avatars": []}
            await stale(action)
            empty = await json_result(rust, "scene_info")
            assert empty["project"] == empty["project_path"] == "NULL"
            assert empty["colorways"] == ["Default Colorway"] and empty["current_colorway"] == 0
            assert empty["pattern_count"] == empty["seam_groups"] == empty["fabric_count"] == 0
            assert empty["avatars"] == [] and await current_fabric() == 0
            assert await json_result(rust, "garment_last_spec") == history

            await json_result(rust, "scene_open", {"path": path})
            action = await observed_action()
            fresh = await json_result(rust, "clo_api_call", {"name": "utility_api.NewProject"})
            assert fresh["value"] is None
            await stale(action)
            assert await json_result(rust, "scene_info") == empty and await current_fabric() == 0
            assert await json_result(rust, "garment_last_spec") == history
            assert (await json_result(rust, "clo_status"))["bridge"] == "up"
    print(json.dumps({"tools": ["scene_clear", "scene_info", "clo_ui_action", "clo_api_call", "garment_last_spec"],
                      "case": "scene clear boundaries", "deleted_patterns": keep["deleted"],
                      "keep_avatar_preserved_assets": True, "new_project_reset_fabric": True,
                      "old_snapshots_rejected": 3, "session_history_preserved": True}))


async def verify_garment_session_history():
    empty = {"spec": None, "pieces": []}
    options = {"simulate_steps": 0, "snapshots": False}
    official_baseline = None
    if os.environ.get("CLO_MCP_OFFICIAL_COMMAND"):
        async with open_backend("official") as official:
            await inspect_contract(official)
            assert await json_result(official, "garment_last_spec") == empty
            refused = await official.call_tool("garment_rebuild", options | {"changes": {}})
            assert refused.isError
            assert await json_result(official, "garment_last_spec") == empty
        official_baseline = "pass"
    with TemporaryDirectory(prefix="clo-garment-history-") as directory:
        root = Path(directory)
        environment = {
            "CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": "",
            "CLO_RUST_CHECKPOINT_DIR": str(root / "checkpoints"),
        }
        project = root / "long-sleeve.clorust"
        async with open_backend("rust", environment) as rust:
            await inspect_contract(rust)
            assert await json_result(rust, "garment_last_spec") == empty
            first = await json_result(rust, "garment_build", options | {
                "spec": {"garment": "top", "length": "waist", "sleeve": "long"},
            })
            first_pieces = (await json_result(rust, "garment_last_spec"))["pieces"]
            assert len(first_pieces) == 4
            await json_result(rust, "scene_save", {"path": str(project), "thumbnail": False})
            await json_result(rust, "checkpoint_save", {"name": "long-sleeve"})
            archived = json.loads(project.read_text())["scene"]
            assert "runtime" not in archived and archived["garment_spec"] == first["spec"]

            second = await json_result(rust, "garment_rebuild", options | {
                "changes": {"length": "cropped", "sleeve": "none"},
            })
            recent = await json_result(rust, "garment_last_spec")
            assert recent == {"spec": second["spec"], "pieces": ["Front", "Back"]}
            failed = await rust.call_tool("garment_rebuild", {
                "changes": {"sleeve": "long", "color": "navy", "particle_distance": 25},
                "simulate_steps": 2**32, "snapshots": False,
            })
            assert failed.isError and "steps must be a nonnegative 32-bit integer" in failed.content[0].text
            live = await json_result(rust, "scene_info")
            assert live["pattern_count"] == len(first_pieces)
            assert live["seam_groups"] == len(first["seams"])
            after_error = root / "after-solver-error.clorust"
            await json_result(rust, "scene_save", {"path": str(after_error), "thumbnail": False})
            partial = json.loads(after_error.read_text())["scene"]
            assert [piece["name"] for piece in partial["patterns"]] == first_pieces
            assert all(piece["particle_distance"] == 25 for piece in partial["patterns"])
            assert partial["fabrics"][-1]["color"] == [0.1, 0.16, 0.42]
            assert partial["simulation"]["steps"] == 0
            assert len(partial["simulation"]["meshes"]) == len(first_pieces)
            assert await json_result(rust, "garment_last_spec") == recent

            reopened = await json_result(rust, "scene_open", {"path": str(project), "mode": "replace"})
            assert reopened["pattern_count"] == len(first_pieces)
            assert await json_result(rust, "garment_last_spec") == recent
            await json_result(rust, "scene_clear", {"keep_avatar": True})
            assert (await json_result(rust, "scene_info"))["pattern_count"] == 0
            assert await json_result(rust, "garment_last_spec") == recent
            restored = await json_result(rust, "checkpoint_restore", {"name": "long-sleeve"})
            assert restored["pattern_count"] == len(first_pieces)
            assert await json_result(rust, "garment_last_spec") == recent
            await successful(rust, "clo_stop_bridge")
            assert await json_result(rust, "garment_last_spec") == recent
            await successful(rust, "clo_launch", {"with_bridge": True})
            await json_result(rust, "scene_clear", {"keep_avatar": False})
            assert (await json_result(rust, "scene_info"))["avatars"] == []
            assert await json_result(rust, "garment_last_spec") == recent
            rebuilt = await json_result(rust, "garment_rebuild", options | {"changes": {"color": "navy"}})
            assert rebuilt["spec"] == recent["spec"] | {"color": "navy"}
            assert set(rebuilt["pieces"]) == set(recent["pieces"])

            async with open_backend("rust", environment) as independent:
                opened = await json_result(independent, "scene_open", {"path": str(project)})
                assert opened["pattern_count"] == len(first_pieces)
                assert await json_result(independent, "garment_last_spec") == empty
                refused = await independent.call_tool("garment_rebuild", options | {"changes": {}})
                assert refused.isError and "nothing built yet in this session" in refused.content[0].text
                assert await json_result(independent, "garment_last_spec") == empty
                assert (await json_result(rust, "garment_last_spec"))["spec"] == rebuilt["spec"]
    print(json.dumps({"tools": ["garment_last_spec", "garment_rebuild", "scene_open",
                                "checkpoint_restore", "scene_clear"],
                      "case": "garment session history", "open_and_restore": "pass",
                      "clear_and_bridge_restart": "pass", "independent_client": "pass",
                      "solver_error_keeps_drafted_scene_and_prior_history": "pass",
                      "official_empty_session": official_baseline}))


async def verify_build_snapshot_workflow():
    defaults = ["front", "3/4-right", "back"]
    caption = "Snapshots after simulation (judge fit, seams, drape; fix with garment_rebuild or pattern tools):"
    with TemporaryDirectory(prefix="clo-build-capture-") as directory:
        root = Path(directory)
        output = root / "output"
        environment = {
            "CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": "",
            "CLO_RUST_OUTPUT_DIR": str(output),
            "CLO_RUST_CHECKPOINT_DIR": str(root / "checkpoints"),
        }

        def check_images(blocks, views):
            assert [block.type for block in blocks] == ["text", "image"] * len(views)
            images = []
            for view, text, image in zip(views, blocks[::2], blocks[1::2], strict=True):
                prefix = f"[{view} view] "
                assert text.text.startswith(prefix), text
                path = Path(text.text.removeprefix(prefix))
                assert path.parent == output.resolve() / "snapshots"
                raw = base64.b64decode(image.data, validate=True)
                assert image.mimeType == "image/png" and raw.startswith(b"\x89PNG\r\n\x1a\n")
                assert raw == path.read_bytes()
                images.append(raw)
            return images

        async with open_backend("rust", environment) as rust:
            await inspect_contract(rust)
            early = await rust.call_tool("garment_rebuild", {"changes": {}, "snapshots": False})
            assert early.isError
            assert await json_result(rust, "garment_last_spec") == {"spec": None, "pieces": []}

            async def build(name, arguments, views):
                response = await successful(rust, name, {"simulate_steps": 0} | arguments)
                assert response.content[0].type == "text"
                summary = json.loads(response.content[0].text)
                assert summary["simulation"] is None
                if views:
                    assert response.content[1].type == "text"
                    assert response.content[1].text == caption
                    check_images(response.content[2:], views)
                else:
                    assert len(response.content) == 1
                last = await json_result(rust, "garment_last_spec")
                assert last["spec"] == summary["spec"]
                assert set(last["pieces"]) == set(summary["pieces"])

            spec = {"garment": "top", "length": "waist"}
            await build("garment_build", {"spec": spec}, defaults)
            await build("garment_build", {"spec": spec, "views": []}, defaults)
            views = ["FRONT", "Three-Quarter", "3/4-LEFT"]
            await build("garment_build", {"spec": spec, "views": views}, views)
            await build("garment_rebuild", {"changes": {"length": "cropped"}}, defaults)
            await build("garment_build", {"spec": spec, "views": ["ignored"], "snapshots": False}, [])

            aliases = ["Three-Quarter", "3/4-right"]
            captures = await successful(rust, "snapshot", {"views": aliases})
            a, b = check_images(captures.content, aliases)
            assert a == b, "the three-quarter alias must produce the same camera"

            async def thumbnail(name):
                path = root / f"{name}.png"
                report = await json_result(rust, "clo_api_call", {
                    "name": "export_api.ExportThumbnail3D", "args": [str(path)],
                })
                assert report["value"] == str(path)
                return path.read_bytes()

            defaults_capture = await successful(rust, "snapshot", {"views": [], "label": ""})
            front, oblique = check_images(defaults_capture.content, ["front", "3/4-right"])
            assert front != oblique and oblique == b
            assert all(Path(block.text.split("] ", 1)[1]).name.startswith("snap_")
                       for block in defaults_capture.content[::2])
            assert await thumbnail("after-3d") == oblique
            await successful(rust, "snapshot_2d")
            assert await thumbnail("after-2d") == oblique
            snapshot_folder = output / "snapshots"
            before = set(snapshot_folder.glob("*.png"))
            invalid_first = await rust.call_tool("snapshot", {"views": ["unknown view", "front"]})
            assert invalid_first.isError
            assert await thumbnail("after-invalid-first") == oblique
            assert set(snapshot_folder.glob("*.png")) == before

            invalid_later = await rust.call_tool("snapshot", {"views": ["front", "unknown view", "back"]})
            assert invalid_later.isError
            completed, = set(snapshot_folder.glob("*.png")) - before
            assert completed.read_bytes() == await thumbnail("after-invalid-later") == front

            before = set(snapshot_folder.glob("*.png"))
            invalid = await rust.call_tool("garment_build", {
                "spec": spec | {"length": "cropped", "color": "navy"},
                "simulate_steps": 0, "views": ["front", "unknown view"],
            })
            assert invalid.isError and "unknown snapshot view" in invalid.content[0].text
            completed, = set(snapshot_folder.glob("*.png")) - before
            assert completed.read_bytes() == await thumbnail("after-build-view-error")
            last = await json_result(rust, "garment_last_spec")
            assert last["spec"]["color"] == "navy" and last["spec"]["length"] == "cropped"
            stored = root / "after-error.clorust"
            await json_result(rust, "scene_save", {"path": str(stored), "thumbnail": False})
            scene = json.loads(stored.read_text())["scene"]
            assert [panel["name"] for panel in scene["patterns"]] == last["pieces"]
            assert scene["fabrics"][-1]["color"] == [0.1, 0.16, 0.42]

            failed_draft = await rust.call_tool("garment_rebuild", {
                "changes": {"particle_distance": "invalid"}, "snapshots": False,
            })
            assert failed_draft.isError
            assert await json_result(rust, "garment_last_spec") == last

            snapshot_folder.rename(root / "prior-snapshots")
            snapshot_folder.write_text("Block creation of the output directory")
            failed_export = await rust.call_tool("garment_rebuild", {
                "changes": {"color": "red"}, "simulate_steps": 0,
            })
            assert failed_export.isError
            rebound = await json_result(rust, "garment_last_spec")
            assert rebound["spec"] == last["spec"] | {"color": "red"}
    print(json.dumps({"tools": ["garment_build", "garment_rebuild", "garment_last_spec", "snapshot"],
                      "case": "build snapshot workflow", "default_views": defaults,
                      "aliases_and_png_content": "pass", "capture_error_commits_build": "pass",
                      "draft_error_preserves_previous_build": "pass",
                      "thumbnail_matches_last_3d_view": True, "2d_and_invalid_first_keep_camera": True,
                      "later_view_error_keeps_completed_image_and_camera": True}))


async def verify_one_piece_dress_merge():
    cases = [
        {"silhouette": "a-line", "length": "midi", "particle_distance": 80},
        {"silhouette": "strapless", "length": "tea", "particle_distance": 70},
        {"silhouette": "sheath", "length": "knee", "sleeve": "long", "particle_distance": 80},
    ]
    expected_pairs = [{"base": side, "other": f"Lower_{side}", "ok": True} for side in ("Front", "Back")]
    results = []
    with TemporaryDirectory(prefix="clo-dress-merge-") as directory:
        root = Path(directory)
        environment = {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": "",
                       "CLO_RUST_CHECKPOINT_DIR": str(root / "checkpoints")}
        async with open_backend("rust", environment) as rust:
            await inspect_contract(rust)

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]

            def rest_area(mesh, panel=None):
                points = mesh["texcoords"]
                factor = 1.0
                if panel is not None:
                    x, y = zip(*(point[:2] for point in panel["points"]))
                    factor = (max(x) - min(x)) * (max(y) - min(y))
                return factor * sum(
                    abs((points[b][0] - points[a][0]) * (points[c][1] - points[a][1])
                        - (points[b][1] - points[a][1]) * (points[c][0] - points[a][0])) / 2
                    for a, b, c in mesh["triangles"]
                )

            for spec in cases:
                request = {"spec": spec, "snapshots": False}
                drafted = await json_result(rust, "garment_build", request | {"simulate_steps": 0})
                assert drafted["simulation"] is None
                await json_result(rust, "simulate", {"steps": 1, "particle_distance": spec["particle_distance"]})
                _, split = await save("split")
                split_panels = {panel["name"]: panel for panel in split["patterns"]}
                split_meshes = {mesh["name"]: mesh for mesh in split["simulation"]["meshes"]}
                built = await json_result(rust, "garment_build", request | {"simulate_steps": 1})
                assert built["simulation"]["merged_pieces"] == expected_pairs, built["simulation"]
                assert built["pieces"]["Lower_Front"]["index"] is None
                assert built["pieces"]["Lower_Back"]["index"] is None
                path, scene = await save("merged")
                assert scene["simulation"]["steps"] == 31
                panels = {panel["id"]: panel for panel in scene["patterns"]}
                names = [panel["name"] for panel in panels.values()]
                assert names[:2] == ["Front", "Back"] and len(names) == len(split_panels) - 2
                assert len(scene["seams"]) == len(split["seams"]) - 2
                for seam in scene["seams"]:
                    for end in ("a", "b"):
                        assert seam[f"line_{end}"] < len(panels[seam[f"pattern_{end}"]]["edges"])
                for name in ("Front", "Back"):
                    mesh = next(mesh for mesh in scene["simulation"]["meshes"] if mesh["name"] == name)
                    lower = f"Lower_{name}"
                    expected_area = sum(rest_area(split_meshes[p], split_panels[p]) for p in (name, lower))
                    assert abs(rest_area(mesh) - expected_area) < 1e-5
                    incidence = Counter(tuple(sorted((a, b)))
                                        for face in mesh["triangles"]
                                        for a, b in zip(face, face[1:] + face[:1]))
                    assert len(mesh["positions"]) - len(incidence) + len(mesh["triangles"]) == 1
                    waist_y = next(point[1] for point in split_panels[lower]["points"])
                    join_counts = [count for (a, b), count in incidence.items()
                                   if mesh["texcoords"][a][1] == mesh["texcoords"][b][1] == waist_y]
                    assert join_counts and all(count == 2 for count in join_counts)
                    if spec["silhouette"] == "strapless":
                        panel = panels[mesh["id"]]
                        assert panel["line_properties"]["0"]["elastic"]["on"]
                expected_names = [panel["name"] for panel in split["patterns"]]
                assert (await json_result(rust, "garment_last_spec"))["pieces"] == expected_names
                await json_result(rust, "scene_open", {"path": str(path)})
                await json_result(rust, "simulate", {"steps": 0})
                _, reopened = await save("reopened")
                assert reopened["simulation"] == scene["simulation"]
                await json_result(rust, "simulate", {"steps": 1})
                _, continued = await save("continued")
                assert continued["simulation"]["steps"] == 32
                results.append({"silhouette": spec["silhouette"], "panels": len(names), "seams": len(scene["seams"])})
            rebuilt = await json_result(rust, "garment_rebuild", {
                "changes": {"sleeve": "none"}, "simulate_steps": 1, "snapshots": False,
            })
            assert rebuilt["simulation"]["merged_pieces"] == expected_pairs
            _, scene = await save("rebuilt")
            assert [panel["name"] for panel in scene["patterns"]] == ["Front", "Back"]
    print(json.dumps({"tools": ["garment_build", "garment_rebuild", "scene_save", "simulate"],
                      "case": "one-piece dress waist merge", "builds": results,
                      "rest_area_and_shared_waist": "pass", "reopen_and_rebuild": "pass"}))


async def compare_curved_skirt_preview():
    cases = (
        {"garment": "skirt", "silhouette": "full-circle", "length": "midi"},
        {"garment": "skirt", "silhouette": "half-circle", "length": "mini", "fit": "bodycon"},
        {"garment": "skirt", "skirt": "quarter-circle", "waist": "drop", "length": "midi"},
        {"garment": "skirt", "skirt": "three-quarter-circle", "length": "ankle"},
        {"silhouette": "skater", "length": "mini", "sleeve": "short"},
        {"silhouette": "ball gown", "length": "floor", "neckline": "sweetheart", "straps": "wide"},
        {"silhouette": "mermaid", "length": "floor"},
        {"garment": "skirt", "skirt": "trumpet", "length": "ankle"},
    )
    results = []
    async with open_backend("official") as clo, open_backend("rust") as rust:
        for session in (clo, rust):
            await inspect_contract(session)
        for spec in cases:
            args = {"spec": spec, "use_scene_measurements": False}
            expected = await json_result(clo, "garment_plan_preview", args)
            observed = await json_result(rust, "garment_plan_preview", args)
            assert expected.keys() == observed.keys(), spec
            for key in expected:
                assert expected[key] == observed[key], (spec, key, expected[key], observed[key])
            results.append({
                "style": spec.get("skirt", spec.get("silhouette")),
                "pieces": len(observed["pieces"]), "seams": len(observed["seams"]),
            })
        for session in (clo, rust):
            assert await json_result(session, "garment_last_spec") == {"spec": None, "pieces": []}
    print(json.dumps({"tool": "garment_plan_preview", "case": "circle and mermaid skirts",
                      "comparisons": results, "last_spec_unchanged": True}))


async def verify_runtime_pattern_query():
    fixture = os.environ.get("CLO_RUNTIME_PATTERN_FIXTURE")
    assert fixture, "set CLO_RUNTIME_PATTERN_FIXTURE to a saved .zprj with at least one pattern"
    project = Path(fixture).resolve(strict=True)
    expected = {
        "GetPatternCount": "GetPatternCount() -> int",
        "GetPatternPieceName": "GetPatternPieceName(arg0: int) -> str",
    }

    async def observe(session, scene):
        findings = {}
        for short, summary in expected.items():
            name = f"pattern_api.{short}"
            entry = {"name": name, "kind": "function", "signature": None, "summary": summary}
            page = await json_result(session, "clo_api_catalog", {
                "query": short, "module": "pattern_api", "limit": 1,
            })
            assert page == {"entries": [entry], "total": 1, "next_offset": None, "unavailable_modules": {}}, page
            search = await json_result(session, "clo_api_search", {"query": short.lower(), "limit": 1})
            assert json.loads(search["result"]) == page
            description = await json_result(session, "clo_api_describe", {"name": name})
            assert description["name"] == name and description["members"] == []
            assert description["doc"].splitlines()[0] == summary
            for query in (short, name):
                doc = await json_result(session, "clo_api_doc", {"function_name": query})
                assert json.loads(doc["result"]) == description
            findings[short] = {"catalog": entry, "doc": description["doc"].splitlines()[0]}

        index_query = "pattern_api.GetPatternIndex arg0: str"
        for offset, short in enumerate(("GetPatternIndex", "GetPatternIndexW")):
            name = f"pattern_api.{short}"
            summary = f"{short}(arg0: str) -> int"
            entry = {"name": name, "kind": "function", "signature": None, "summary": summary}
            page = await json_result(session, "clo_api_catalog", {
                "query": index_query, "module": "pattern_api", "limit": 1, "offset": offset,
            })
            assert page == {"entries": [entry], "total": 2, "next_offset": 1 if offset == 0 else None,
                            "unavailable_modules": {}}, page
            if offset == 0:
                search = await json_result(session, "clo_api_search", {"query": index_query.lower(), "limit": 1})
                assert json.loads(search["result"]) == page
            description = await json_result(session, "clo_api_describe", {"name": name})
            assert description["name"] == name and description["members"] == []
            assert description["doc"].splitlines()[0] == summary
            doc = await json_result(session, "clo_api_doc", {"function_name": short})
            assert json.loads(doc["result"]) == description
            findings[short] = {"catalog": entry, "doc": summary}

        count = scene["pattern_count"]
        assert count > 0
        first = scene["patterns"][0]["name"]
        index, selected = next(((i, pattern["name"]) for i, pattern in enumerate(scene["patterns"])
                                if pattern["name"] != first), (0, first))
        for short, args, value in (("GetPatternCount", [], count), ("GetPatternPieceName", [index], selected),
                                   ("GetPatternIndex", [selected], index), ("GetPatternIndexW", [selected], index)):
            name = f"pattern_api.{short}"
            assert await json_result(session, "clo_api_call", {"name": name, "args": args}) == {
                "name": name, "value": value,
            }
        name = "pattern_api.GetPatternPieceName"
        for index_arg, value in ((-1, ""), (count, ""), (2**31 - 1, ""), (False, first)):
            assert await json_result(session, "clo_api_call", {"name": name, "args": [index_arg]}) == {
                "name": name, "value": value,
            }
        for short in ("GetPatternIndex", "GetPatternIndexW"):
            name = f"pattern_api.{short}"
            assert await json_result(session, "clo_api_call", {
                "name": name, "args": ["MissingMcpVerificationPattern"],
            }) == {"name": name, "value": -1}
        empty_search = await json_result(session, "clo_api_search", {
            "query": "MissingMcpVerificationPattern", "limit": 1,
        })
        assert json.loads(empty_search["result"]) == {
            "total": 0, "next_offset": None, "entries": [], "unavailable_modules": {},
        }
        empty_doc = await json_result(session, "clo_api_doc", {"function_name": "MissingMcpVerificationPattern"})
        assert json.loads(empty_doc["result"]) == {"matches": [], "help": "Use a qualified name from clo_api_catalog."}
        assert await json_result(session, "clo_run_python", {"code": "pattern_api.GetPatternCount()"}) == {
            "value": count, "stdout": "",
        }
        assert await json_result(session, "clo_run_python", {
            "code": "print(pattern_api.GetPatternCount()); result = pattern_api.GetPatternPieceName(0)",
        }) == {"value": first, "stdout": f"{count}\n"}
        handle_type = "ApiTypes.ImportExportOption"
        description = await json_result(session, "clo_api_describe", {"name": handle_type})
        required_fields = {"bAdd", "bExportAvatar", "bExportGarment", "bSaveColorWays", "bSingleObject", "scale"}
        assert description["name"] == handle_type
        assert required_fields <= {member["name"] for member in description["members"]}
        handle = (await json_result(session, "clo_api_call", {"name": handle_type}))["value"]
        assert handle["$handle"] and handle["type"] == "ImportExportOption"
        for released in (1, 0):
            assert await json_result(session, "clo_api_release", {"handles": [handle["$handle"]]}) == {
                "released": released,
            }
        errors = (
            ("clo_api_catalog", {"module": "pattern_api", "offset": -1}),
            ("clo_api_search", {"query": "GetPatternCount", "limit": 0}),
            ("clo_api_search", {"query": "GetPatternCount", "limit": 501}),
            ("clo_api_doc", {"function_name": "pattern_api.MissingMcpVerificationPattern"}),
            ("clo_api_describe", {"name": "pattern_api.MissingMcpVerificationPattern"}),
            ("clo_api_call", {"name": "pattern_api.GetPatternCount", "args": [42]}),
            ("clo_api_call", {"name": "pattern_api.GetPatternIndex",
                              "args": [{"$handle": handle["$handle"]}]}),
            ("clo_run_python", {"code": "raise ValueError('verification control')"}),
        ) + tuple(("clo_api_call", {"name": "pattern_api.GetPatternPieceName", "args": [value]})
                  for value in (2**31, "0", 1.0, None))
        for tool, args in errors:
            assert (await session.call_tool(tool, args)).isError, (tool, args)
        fresh = (await json_result(session, "clo_api_call", {"name": handle_type}))["value"]
        assert fresh["type"] == handle["type"] and fresh["$handle"] != handle["$handle"]
        assert await json_result(session, "clo_api_release", {
            "handles": [handle["$handle"], fresh["$handle"]],
        }) == {"released": 1}
        after = await json_result(session, "scene_info")
        for key in ("project_path", "patterns", "pattern_count", "avatars", "fabric_count", "seam_groups"):
            assert after[key] == scene[key], key
        return {"apis": findings, "pattern_count": count, "first_pattern": first,
                "index_lookup": {"name": selected, "index": index}, "catalog_pages": 2,
                "handle_release_counts": [1, 0, 1], "errors_rejected": len(errors),
                "read_only_scene_unchanged": True}

    async def verify_scene_edit(session, scene):
        original = scene["patterns"][0]["name"]
        api = "pattern_api.SetPatternPieceName"
        renamed = original + " [MCP]"
        partial = original + " [MCP Python]"
        try:
            changed = await json_result(session, "clo_api_call", {"name": api, "args": [0, renamed]})
            assert changed == {"name": api, "value": None}
            assert (await json_result(session, "pattern_info", {"pattern": "0"}))["name"] == renamed
            script = f"{api}(0, {partial!r})\nraise ValueError('verify scene readback')"
            assert (await session.call_tool("clo_run_python", {"code": script})).isError
            assert await json_result(session, "clo_run_python", {
                "code": "pattern_api.GetPatternPieceName(0)",
            }) == {"value": partial, "stdout": ""}
            assert (await json_result(session, "scene_info"))["patterns"][0]["name"] == partial
        finally:
            await json_result(session, "clo_api_call", {"name": api, "args": [0, original]})
        assert await json_result(session, "scene_info") == scene
        return {"typed_rename_readback": True, "python_error_keeps_rename": True, "restored": True}

    async with open_backend("rust") as rust:
        await inspect_contract(rust)
        await json_result(rust, "scene_open", {"path": str(project)})
        scene = await json_result(rust, "scene_info")
        observations = await observe(rust, scene)
        observations["edit"] = await verify_scene_edit(rust, scene)
    reference = {"status": "NOT_RUN"}
    if os.environ.get("CLO_MCP_OFFICIAL_COMMAND"):
        async with open_backend("official") as clo:
            await inspect_contract(clo)
            if (await json_result(clo, "clo_status"))["bridge"] != "up":
                reference = {"status": "BLOCKED", "reason": "CLO bridge is down"}
            else:
                scene = await json_result(clo, "scene_info")
                if Path(scene.get("project_path") or ".").resolve() != project:
                    reference = {"status": "BLOCKED", "reason": "CLO has not opened the saved fixture"}
                else:
                    native = await observe(clo, scene)
                    native["edit"] = await verify_scene_edit(clo, scene)
                    assert native == observations
                    reference = {"status": "PASS", "clo_version": scene["clo_version"]}
    print(json.dumps({"case": "pattern API workflow", "rust": observations, "official": reference}))
    if reference["status"] == "BLOCKED":
        raise SystemExit(2)


async def compare_python_response_timeout():
    fixture = Path(os.environ["CLO_RUNTIME_PATTERN_FIXTURE"]).resolve(strict=True)
    setter = "pattern_api.SetPatternPieceName"
    checked = {}
    for backend in ("official", "rust"):
        async with open_backend(backend) as session:
            assert (await json_result(session, "clo_status"))["bridge"] == "up"
            if backend == "rust":
                await json_result(session, "scene_open", {"path": str(fixture)})
            scene = await json_result(session, "scene_info")
            assert Path(scene["project_path"]).resolve() == fixture
            original = scene["patterns"][0]["name"]
            observed = []
            try:
                for late_error in (False, True):
                    final = original + (" [late error]" if late_error else " [late success]")
                    script = (
                        f"import time\n{setter}(0, {original + ' [before deadline]'!r})\n"
                        f"time.sleep(2.5)\n{setter}(0, {final!r})"
                    )
                    if late_error:
                        script += "\nraise ValueError('after client deadline')"
                    loop = asyncio.get_running_loop()
                    start = loop.time()
                    result = await session.call_tool("clo_run_python", {"code": script, "timeout_seconds": 1})
                    elapsed = loop.time() - start
                    assert result.isError and 0.7 <= elapsed < 2.4, (backend, elapsed)
                    # This request queues behind the outstanding script on both backends.
                    state = await json_result(session, "scene_info")
                    assert state["patterns"][0]["name"] == final, (backend, state["patterns"][0]["name"])
                    checked_name = await json_result(session, "clo_run_python", {
                        "code": "pattern_api.GetPatternPieceName(0)",
                    })
                    assert checked_name == {"value": final, "stdout": ""}
                    observed.append({"late_error": late_error, "returned_error_seconds": round(elapsed, 3),
                                     "late_change": final, "new_request_ok": True})
            finally:
                await json_result(session, "clo_api_call", {"name": setter, "args": [0, original]})
            assert await json_result(session, "scene_info") == scene
            checked[backend] = {"version": scene["clo_version"], "cases": observed, "restored": True}
    print(json.dumps({"case": "Python response timeout and late execution", "observations": checked},
                     ensure_ascii=False))


async def verify_runtime_export_workflow():
    names = (
        "export_api.ExportFBX", "export_api.ExportSnapshot2D",
        "export_api.ExportThumbnail3D", "utility_api.GetCLOAssetFolderPath",
    )
    aliases = (
        "export_api.ExportFBXW", "export_api.ExportThumbnail3DW",
        "utility_api.GetCLOAssetFolderPathW", "utility_api.GetCLOTemporaryFolderPathW",
    )
    checked_names = (*names, "utility_api.GetCLOTemporaryFolderPath", *aliases)
    captured = {}
    if capture := os.environ.get("CLO_RUNTIME_EXPORT_CATALOG"):
        modules = json.loads(Path(capture).read_text())
        captured = {entry["name"]: entry for rows in modules.values() for entry in rows}
        assert set(checked_names) <= captured.keys()
    async with open_backend("rust") as session:
        await inspect_contract(session)
        for name in checked_names:
            module, short = name.split(".")
            request = {"module": module, "query": name, "limit": 1}
            expected = await json_result(session, "clo_api_catalog", request)
            entry, = expected["entries"]
            assert entry["name"] == name and entry["signature"] is None
            if captured:
                assert entry == captured[name], (entry, captured[name])
            search = await json_result(session, "clo_api_search", {"query": name.lower(), "limit": 1})
            assert json.loads(search["result"]) == expected
            describe = await json_result(session, "clo_api_describe", {"name": name})
            assert describe["doc"].splitlines()[0] == entry["summary"]
            for query in (name, short):
                doc = await json_result(session, "clo_api_doc", {"function_name": query})
                assert json.loads(doc["result"]) == describe
            assert describe["name"] == name and describe["doc"]
        request = {"module": "export_api", "query": "ExportFBX", "limit": 1}
        first = await json_result(session, "clo_api_catalog", request)
        second = await json_result(session, "clo_api_catalog", {**request, "offset": first["next_offset"]})
        assert first["next_offset"] == 1 and second["next_offset"] is None
        assert [page["entries"][0]["name"] for page in (first, second)] == [names[0], aliases[0]]
        missing = await json_result(session, "clo_api_doc", {"function_name": "ExportUnknownFixture"})
        assert json.loads(missing["result"]) == {"matches": [], "help": "Use a qualified name from clo_api_catalog."}
        assert (await session.call_tool("clo_api_doc", {"function_name": "export_api.ExportUnknownFixture"})).isError
        for limit in (0, 501):
            assert (await session.call_tool("clo_api_search", {"query": "ExportFBX", "limit": limit})).isError

        async def invoke(name, args=None):
            request = {"name": name}
            if args is not None:
                request["args"] = args
            return (await json_result(session, "clo_api_call", request))["value"]

        await json_result(session, "pattern_create", {
            "name": "export test", "points": [[0, 0], [160, 0], [130, 110]],
        })
        scene = await json_result(session, "scene_info")
        assert await invoke(names[3]) == (scene["asset_folder"] or "").replace("\\", "/")
        assert await invoke(aliases[2], [False]) == await invoke(names[3], [False])
        assert await invoke(aliases[3]) == await invoke("utility_api.GetCLOTemporaryFolderPath")
        with TemporaryDirectory() as directory:
            folder = Path(directory)
            layout = str(folder / "layout.png")
            assert await invoke(names[1], [layout, 0]) == []
            capture = await successful(session, "snapshot_2d")
            image, = [block for block in capture.content if block.type == "image"]
            assert base64.b64decode(image.data, validate=True) == Path(layout).read_bytes()
            scripted_path = str(folder / "scripted.png")
            scripted = await json_result(session, "clo_run_python", {
                "code": f"result = export_api.ExportSnapshot2D({scripted_path!r}, 0)",
            })
            assert scripted["value"] == [] and Path(scripted_path).read_bytes() == Path(layout).read_bytes()
            invalid = await session.call_tool("clo_api_call", {"name": names[1], "args": [layout, 1]})
            assert invalid.isError and Path(layout).read_bytes() == base64.b64decode(image.data)
            thumbnails = []
            for camera in (2, 4):
                await invoke("utility_api.SetCamViewPoint", [camera])
                path = folder / f"view-{camera}.png"
                assert await invoke(names[2], [str(path)]) == str(path)
                thumbnails.append(path.read_bytes())
            assert all(image.startswith(b"\x89PNG\r\n\x1a\n") for image in thumbnails)
            assert thumbnails[0] != thumbnails[1]
            wide_png = folder / "斜面.png"
            wide = await json_result(session, "clo_run_python", {
                "code": f"export_api.ExportThumbnail3DW({str(wide_png)!r})",
            })
            assert wide["value"] == str(wide_png) and wide_png.read_bytes() == thumbnails[-1]
            fbx = folder / "纸样.fbx"
            options = {"$type": "ApiTypes.ImportExportOption"}
            assert await invoke(names[0], [str(folder / "piece.fbx"), options]) == [str(folder / "piece.fbx")]
            assert await invoke(aliases[0], [str(fbx), options]) == [str(fbx)]
            assert fbx.stat().st_size > 1000
            assert await json_result(session, "scene_info") == scene
            await json_result(session, "scene_open", {"path": str(fbx)})
            saved = folder / "reopened.clorust"
            await json_result(session, "scene_save", {"path": str(saved)})
            mesh, = json.loads(saved.read_text())["scene"]["imported_meshes"]
            assert len(mesh["positions"]) == 3 and len(mesh["triangles"]) == 1
    print(json.dumps({"workflow": "runtime export", "discovered_and_called": checked_names,
                      "native_catalog_matches": len(checked_names) if captured else None,
                      "unicode_exports": True, "missing_doc_and_search_limits": "pass",
                      "png_matches_snapshot": True, "python_entry_point": True,
                      "camera_changes_thumbnail": True,
                      "fbx_reopened": True, "invalid_mode_error": "pass"}))


async def verify_checkpoint_workflow():
    fixture = os.environ.get("CLO_CHECKPOINT_FIXTURE")
    with TemporaryDirectory() as scratch:
        folder = Path(scratch)
        settings = {"CLO_RUST_CHECKPOINT_DIR": str(folder / "checkpoints")}
        async with open_backend("rust", settings) as session:
            await inspect_contract(session)
            assert (await json_result(session, "checkpoint_list"))["checkpoints"] == []
            points = [[0, 0], [160, 0], [130, 110]]
            await json_result(session, "pattern_create", {"name": "before", "points": points})
            original = await json_result(session, "scene_info")
            first = await json_result(session, "checkpoint_save")
            original |= {"project": first["name"], "project_path": first["path"]}
            assert await json_result(session, "scene_info") == original
            await json_result(session, "pattern_create", {"name": "after", "points": points})
            changed = await json_result(session, "scene_info")
            second = await json_result(session, "checkpoint_save")
            changed |= {"project": second["name"], "project_path": second["path"]}
            assert await json_result(session, "scene_info") == changed
            assert first["name"] != second["name"]
            listing = (await json_result(session, "checkpoint_list"))["checkpoints"]
            assert [row["name"] for row in listing] == [second["name"], first["name"]]
            for row in listing:
                path = Path(row["path"])
                assert path.is_file()
                assert row["mtime"] == datetime.fromtimestamp(path.stat().st_mtime).strftime("%Y-%m-%d %H:%M")
            for saved, expected in ((first, original), (second, changed)):
                restored = await json_result(session, "checkpoint_restore", {"name": saved["name"]})
                assert restored["ok"] and restored["path"] == saved["path"]
                assert await json_result(session, "scene_info") == expected
            bad = Path(second["path"]).with_stem("bad")
            bad.write_text("{broken JSON")
            for name in ("bad", "missing", "../outside"):
                result = await session.call_tool("checkpoint_restore", {"name": name})
                assert result.isError
                assert await json_result(session, "scene_info") == changed
            if fixture:
                source = folder / Path(fixture).name
                shutil.copy2(fixture, source)
                await json_result(session, "scene_open", {"path": str(source)})
                await json_result(session, "checkpoint_save", {"name": "native"})
                native = await json_result(session, "scene_info")
                await json_result(session, "scene_clear", {"keep_avatar": False})
                await json_result(session, "checkpoint_restore", {"name": "native"})
                assert await json_result(session, "scene_info") == native
                target = folder / "restored.zprj"
                await json_result(session, "scene_save", {"path": str(target), "thumbnail": False})
                reopened = await json_result(session, "scene_open", {"path": str(target)})
                assert reopened["pattern_count"] == native["pattern_count"]
                assert reopened["avatars"] == native["avatars"]
                source.unlink()
                await json_result(session, "checkpoint_restore", {"name": "native"})
                assert await json_result(session, "scene_info") == native
                paths = folder / "missing-source.zprj"
                rejected = await session.call_tool("scene_save", {"path": str(paths)})
                assert rejected.isError and not paths.exists()
    print(json.dumps({"tools": ["checkpoint_save", "checkpoint_list", "checkpoint_restore"],
                      "named_restore": True, "distinct_automatic_names": True, "atomic_failures": True,
                      "native_project_reexported": bool(fixture)}))


async def verify_interchange_append_keeps_drape():
    with TemporaryDirectory(prefix="clo-interchange-append-") as directory:
        root = Path(directory)
        async with open_backend("rust", {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": ""}) as rust:
            await inspect_contract(rust)

            async def create(name, x):
                await json_result(rust, "pattern_create", {
                    "name": name, "points": [[x, 0], [x + 100, 0], [x + 50, 75]],
                })

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]

            await create("Front", 0)
            await create("Back", 160)
            await json_result(rust, "sew", {"pattern_a": "Front", "line_a": 0,
                                            "pattern_b": "Back", "line_b": 0})
            fixtures = [root / "panels.json", root / "panels.dxf"]
            mesh_fixtures = [root / "panels.obj", root / "panels.fbx"]
            for path in fixtures + mesh_fixtures:
                await json_result(rust, "scene_export", {"path": str(path)})

            await json_result(rust, "fabric_apply", {"fabric": "cotton", "patterns": ["Front", "Back"]})
            project_path, _ = await save("multi-colorway")
            document = json.loads(project_path.read_text())
            document["scene"]["colorways"] = ["Original", "Alternate"]
            project_path.write_text(json.dumps(document))
            await json_result(rust, "scene_open", {"path": str(project_path)})
            scene = await json_result(rust, "scene_info")
            assert scene["colorways"] == ["Original", "Alternate"]
            for path in fixtures:
                plain = root / "plain" / path.name
                await json_result(rust, "scene_export", {"path": str(plain)})
                nested = root / "alternate" / f"裁片{path.suffix.upper()}"
                result = await json_result(rust, "scene_export", {
                    "path": str(nested), "colorways": True, "include_avatar": True,
                })
                expected = True if path.suffix == ".json" else str(nested)
                assert result == {"path": str(nested), "result": expected}
                assert nested.read_bytes() == plain.read_bytes()
            assert await json_result(rust, "scene_info") == scene

            existing_mesh = mesh_fixtures[0].read_bytes()
            rejected = await rust.call_tool("scene_export", {
                "path": str(mesh_fixtures[0]), "colorways": True,
            })
            assert rejected.isError and "no archived material" in rejected.content[0].text
            assert mesh_fixtures[0].read_bytes() == existing_mesh
            unknown = root / "unsupported.bin"
            rejected = await rust.call_tool("scene_export", {"path": str(unknown), "colorways": True})
            assert rejected.isError and "unsupported export extension" in rejected.content[0].text
            assert not unknown.exists()
            blocked = root / "blocked.clorust"
            blocked.mkdir()
            marker = blocked / "existing.txt"
            marker.write_text("existing file")
            rejected = await rust.call_tool("scene_save", {"path": str(blocked)})
            assert rejected.isError and marker.read_text() == "existing file"
            assert not list(root.glob(".clo-rust-*.tmp"))
            assert await json_result(rust, "scene_info") == scene
            await json_result(rust, "scene_clear", {"keep_avatar": False})
            await create("Existing", 320)
            await json_result(rust, "simulate", {"steps": 2, "particle_distance": 50})
            original_path, original = await save("original")
            results = []
            for path in fixtures:
                await json_result(rust, "scene_open", {"path": str(original_path)})
                request = {"path": str(path)}
                if path.suffix == ".dxf":
                    request["mode"] = "append"
                opened = await json_result(rust, "scene_open", request)
                assert opened["pattern_count"] == 3
                saved_path, combined = await save(f"combined-{path.suffix[1:]}")
                assert combined["simulation"] == original["simulation"] | {"source": "mixed_saved_pose"}
                assert combined["patterns"][0] == original["patterns"][0]
                assert len({panel["id"] for panel in combined["patterns"]}) == 3
                seams = len(combined["seams"])
                assert seams == int(path.suffix == ".json")
                await json_result(rust, "scene_open", {"path": str(saved_path)})
                await json_result(rust, "simulate", {"steps": 0})
                _, initialized = await save("initialized")
                assert initialized["simulation"]["meshes"][:1] == original["simulation"]["meshes"]
                assert len(initialized["simulation"]["meshes"]) == 3
                assert initialized["simulation"]["steps"] == 2
                await json_result(rust, "simulate", {"steps": 1})
                _, continued = await save("continued")
                assert continued["simulation"]["steps"] == 3
                results.append({"format": path.suffix, "appended_panels": 2, "seams": seams})

            mesh_counts = {}
            for path in mesh_fixtures:
                await json_result(rust, "scene_open", {"path": str(original_path)})
                opened = await json_result(rust, "scene_open", {"path": str(path)})
                assert opened["pattern_count"] == 1
                _, combined = await save(f"default-{path.suffix[1:]}")
                assert combined["patterns"] == original["patterns"]
                assert combined["simulation"] == original["simulation"]
                assert sum(len(mesh["triangles"]) for mesh in combined["imported_meshes"]) == 2
                mesh_counts[path.suffix] = len(combined["imported_meshes"])

            invalid = root / "invalid.json"
            broken = json.loads(fixtures[0].read_text())
            broken["seams"][0]["line_a"] = 99
            invalid.write_text(json.dumps(broken))
            await json_result(rust, "scene_open", {"path": str(original_path)})
            result = await rust.call_tool("scene_open", {"path": str(invalid), "mode": "append"})
            assert result.isError
            _, after_error = await save("after-error")
            assert after_error["simulation"] == original["simulation"]
            assert after_error["patterns"] == original["patterns"]
    print(json.dumps({"tools": ["scene_open", "scene_export", "simulate"],
                      "case": "append interchange patterns and meshes", "formats": results,
                      "default_mesh_import_kept_drape": mesh_counts,
                      "original_mesh_and_velocities": "unchanged", "continued_step": 3,
                      "invalid_seam_is_atomic": True, "pattern_exports_ignore_colorway_and_avatar_options": True,
                      "save_and_export_errors_preserve_files_and_scene": True}))


async def verify_portable_append_keeps_drape():
    with TemporaryDirectory(prefix="clo-portable-append-") as directory:
        root = Path(directory)
        async with open_backend("rust", {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": ""}) as rust:
            await inspect_contract(rust)

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())

            async def make_scene(name, avatar, color, steps, distance):
                await json_result(rust, "scene_clear", {"keep_avatar": False})
                await json_result(rust, "avatar_load", {"name_or_path": avatar})
                for panel in ("Front", "Back"):
                    await json_result(rust, "pattern_create", {
                        "name": panel, "points": [[0, 0], [110, 0], [55, 90]],
                        "arrangement": panel,
                    })
                await json_result(rust, "fabric_apply", {"fabric": "cotton", "color": color})
                await json_result(rust, "sew", {"pattern_a": "Front", "line_a": 0,
                                                "pattern_b": "Back", "line_b": 0})
                await json_result(rust, "simulate", {"steps": steps, "particle_distance": distance})
                return await save(name)

            original, destination = await make_scene("first", "mia", "red", 3, 50)
            imported, source = await make_scene("second", "luka", "navy", 2, 80)
            left, right = destination["scene"], source["scene"]
            source_index = right["patterns"][0]["fabric_index"]
            source_material = next(fabric for fabric in right["fabrics"] if fabric["index"] == source_index)
            source_material["index"] = 12
            for record in right["patterns"] + right["simulation"]["meshes"]:
                record["fabric_index"] = 12
            imported.write_text(json.dumps(source))
            await json_result(rust, "scene_open", {"path": str(original)})
            opened = await json_result(rust, "scene_open", {"path": str(imported), "mode": "append"})
            assert opened["pattern_count"] == 4 and len(opened["avatars"]) == 2
            path, appended = await save("appended")
            combined = appended["scene"]
            assert combined["patterns"][:2] == left["patterns"]
            panel_ids = [p["id"] for p in combined["patterns"]]
            assert len(set(panel_ids)) == 4
            assert combined["seams"][0] == left["seams"][0]
            assert [combined["seams"][1][key] for key in ("pattern_a", "pattern_b")] == panel_ids[2:]
            assert combined["fabrics"][:len(left["fabrics"])] == left["fabrics"]
            remapped_index = combined["patterns"][2]["fabric_index"]
            new_fabric = next(fabric for fabric in combined["fabrics"] if fabric["index"] == remapped_index)
            assert new_fabric["color"] == [0.1, 0.16, 0.42]
            assert remapped_index not in {fabric["index"] for fabric in left["fabrics"]}
            assert all(p["fabric_index"] == new_fabric["index"] for p in combined["patterns"][2:])
            expected = left["simulation"]["meshes"] + [
                mesh | {"id": panel, "fabric_index": new_fabric["index"]}
                for mesh, panel in zip(right["simulation"]["meshes"], panel_ids[2:], strict=True)
            ]
            assert combined["simulation"]["meshes"] == expected
            wrong = root / "broken.clorust"
            right["simulation"]["meshes"][0]["id"] = "no corresponding panel"
            wrong.write_text(json.dumps(source))
            refused = await rust.call_tool("scene_open", {"path": str(wrong), "mode": "append"})
            assert refused.isError
            _, after_error = await save("after-error")
            for key in combined.keys() - {"project", "project_path"}:
                assert after_error["scene"][key] == combined[key], key
            await json_result(rust, "scene_open", {"path": str(path)})
            await json_result(rust, "simulate", {"steps": 0})
            _, resumed = await save("zero")
            assert resumed["scene"]["simulation"]["meshes"] == expected
            assert resumed["scene"]["simulation"]["steps"] == 3
            await json_result(rust, "simulate", {"steps": 1})
            _, advanced = await save("continued")
            assert advanced["scene"]["simulation"]["steps"] == 4
    print(json.dumps({"tools": ["scene_open", "scene_save", "simulate"],
                      "case": "append saved Rust scenes", "panels": 4, "fabrics": 2, "avatars": 2,
                      "seams": 2, "preserved_meshes": len(expected), "continued_step": 4,
                      "invalid_mesh_rejected_atomically": True}))


async def verify_new_panels_keep_drape():
    with TemporaryDirectory(prefix="clo-add-panel-drape-") as directory:
        root = Path(directory)
        async with open_backend("rust", {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": ""}) as rust:
            await inspect_contract(rust)

            async def create(name):
                return await json_result(rust, "pattern_create", {
                    "name": name, "points": [[0, 0], [120, 0], [45, 80]],
                })

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]

            await create("Base")
            await json_result(rust, "simulate", {"steps": 3, "particle_distance": 50})
            _, before = await save("before")
            await create("Added")
            copy = await json_result(rust, "pattern_ops", {
                "pattern": "Base", "op": "copy", "args": {"dx": 100},
            })
            assert copy["new_index"] == 2
            await json_result(rust, "pattern_ops", {
                "pattern": "Base", "op": "symmetry", "args": {"with_sewing": False},
            })
            path, mixed = await save("mixed")
            assert len(mixed["patterns"]) == 4
            assert mixed["simulation"] == before["simulation"] | {"source": "mixed_saved_pose"}
            obj = root / "mixed.obj"
            await json_result(rust, "scene_export", {"path": str(obj)})
            assert sum(line.startswith("o ") for line in obj.read_text().splitlines()) == 4
            await json_result(rust, "scene_open", {"path": str(path)})
            result = await json_result(rust, "simulate", {"steps": 0})
            assert result["initialization"] == "mixed_saved_and_initialized_pose"
            assert not any("Native topology" in limit for limit in result["limitations"])
            _, initialized = await save("initialized")
            meshes = initialized["simulation"]["meshes"]
            assert len(meshes) == 4 and meshes[0] == before["simulation"]["meshes"][0]
            assert initialized["simulation"]["steps"] == 3
            await json_result(rust, "simulate", {"steps": 1})
            _, continued = await save("continued")
            assert continued["simulation"]["steps"] == 4

            observed = await json_result(rust, "clo_ui_state")
            base_row = next(node for node in observed["nodes"] if node["AXRole"] == "AXRow" and node["AXTitle"] == "Base")
            invalid_outline = await rust.call_tool("pattern_create", {"points": [[0, 0], [80, 0]]})
            assert invalid_outline.isError
            selected = await json_result(rust, "clo_ui_action", {
                "snapshot": observed["snapshot"], "action": "press", "element": base_row["id"],
            })
            invalid_arrangement = await rust.call_tool("pattern_create", {
                "name": "Unarranged", "arrangement": "missing-arrangement",
                "points": [[0, 0], [80, 0], [0, 80]],
            })
            assert invalid_arrangement.isError
            stale = await rust.call_tool("clo_ui_action", {
                "snapshot": selected["state"]["snapshot"], "action": "press", "element": base_row["id"],
            })
            assert stale.isError and any("Stale UI snapshot" in block.text for block in stale.content)
            _, partial = await save("arrangement-failed")
            assert partial["patterns"][:-1] == continued["patterns"]
            assert partial["patterns"][-1]["name"] == "Unarranged"
            assert partial["patterns"][-1]["arrangement"] == ""
            assert partial["simulation"] == continued["simulation"] | {"source": "mixed_saved_pose"}

            native_count = None
            if fixture := os.environ.get("CLO_DISPLAY_FIXTURE"):
                await json_result(rust, "scene_open", {"path": fixture})
                _, original = await save("native")
                native_count = len(original["simulation"]["meshes"])
                assert native_count > 0 and original["simulation"]["source"] == "native_saved_pose"
                await create("Added to received pose")
                path, edited = await save("native-edited")
                assert edited["simulation"]["source"] == "mixed_saved_pose"
                assert edited["simulation"]["meshes"] == [
                    mesh | {"texcoords_units": "mm"} for mesh in original["simulation"]["meshes"]
                ]
                await json_result(rust, "scene_open", {"path": str(path)})
                await json_result(rust, "simulate", {"steps": 0})
                _, initialized = await save("native-initialized")
                restored = initialized["simulation"]["meshes"]
                assert len(restored) == native_count + 1
                for saved, mesh in zip(original["simulation"]["meshes"], restored[:-1], strict=True):
                    for field in ("id", "positions", "triangles", "texcoords"):
                        assert mesh[field] == saved[field], field
                assert restored[-1].get("texcoords_units") is None
    print(json.dumps({"tools": ["pattern_create", "pattern_ops", "scene_export", "simulate"],
                      "case": "new panels alongside a saved drape", "new_meshes_initialized": 3,
                      "continued_step": 4, "native_pose_meshes_preserved": native_count,
                      "invalid_outline_kept_ui_snapshot": True,
                      "invalid_arrangement_kept_panel_and_invalidated_snapshot": True}))


async def compare_native_pattern_creation():
    fixture = Path(os.environ["CLO_PATTERN_CREATE_FIXTURE"]).resolve(strict=True)
    observations = {}
    triangle = [[250, 0], [330, 0], [270, 60]]
    curved = [[500, 0, 0], [560, 0, 2], [575, 43, 3], [515, 60, 0]]
    missing = "__missing_mcp_point__"
    for backend in ("official", "rust"):
        async with open_backend(backend) as session:
            assert (await json_result(session, "clo_status"))["bridge"] == "up"
            path = str(fixture)
            if backend == "official":
                path = (await json_result(session, "scene_info"))["project_path"]
                assert Path(path).resolve() == fixture, "open the saved fixture in CLO first"
            request = {"path": path, "mode": "replace"}
            await json_result(session, "scene_open", request)
            original = await json_result(session, "scene_info")
            first, second = [piece["name"] for piece in original["patterns"]]
            arrangements = await json_result(session, "arrangement_points")
            assert arrangements["count"] > 1
            first_point, last_point = arrangements["names"][0], arrangements["names"][-1]
            generated = []
            cases = {"arrangement_catalog": arrangements}
            try:
                await json_result(session, "clo_api_call", {
                    "name": "fabric_api.SetCurrentFabricIndex", "args": [1],
                })
                for label, args, name, placed in (
                    ("unnamed", {"points": triangle}, None, "Arrangement Point"),
                    ("empty_name", {"points": triangle, "name": ""}, None, "Arrangement Point"),
                    ("duplicate", {"points": triangle, "name": first}, first, "Arrangement Point"),
                    ("curved", {"points": curved, "name": "Curve Probe"}, "Curve Probe", "Arrangement Point"),
                    ("arranged", {"points": triangle, "name": "Arranged", "arrangement": first_point}, "Arranged", first_point),
                ):
                    created = await json_result(session, "pattern_create", args)
                    after = await json_result(session, "scene_info")
                    index = len(original["patterns"]) + len(cases) - 1
                    assert after["pattern_count"] == index + 1 and created["index"] == index
                    actual_name = created["name"]
                    if name is None:
                        assert re.fullmatch(r"Pattern_\d+", actual_name) and actual_name not in generated
                        generated.append(actual_name)
                    else:
                        assert actual_name == name
                    row = after["patterns"][-1]
                    assert row == {"index": index, "name": actual_name, "arrangement": placed, "fabric_index": 1}
                    assert after["patterns"][:2] == original["patterns"]
                    assert after["seam_groups"] == original["seam_groups"]
                    info = await json_result(session, "pattern_info", {"pattern": str(index)})
                    line_count = len(info["line_info"]["lines"])
                    assert line_count == (2 if label == "curved" else 3), (backend, label, line_count)
                    cases[label] = {"pattern": row | {"name": name}, "outline_lines": line_count}

                before = await json_result(session, "scene_info")
                assert (await session.call_tool("pattern_create", {"points": triangle[:2]})).isError
                assert await json_result(session, "pattern_create", {
                    "name": "Rejected", "points": [[250, 0, 7], triangle[1], triangle[2]],
                }) == {"index": -1, "name": ""}
                if backend == "rust":
                    assert await json_result(session, "pattern_create", {
                        "points": [[0, 0], [0, 0], [0, 0]],
                    }) == {"index": -1, "name": ""}
                assert await json_result(session, "scene_info") == before
                partial = await session.call_tool("pattern_create", {
                    "name": "Pending", "points": triangle, "arrangement": missing,
                })
                assert partial.isError
                after = await json_result(session, "scene_info")
                assert after["patterns"][:-1] == before["patterns"]
                assert after["patterns"][-1] == {
                    "index": before["pattern_count"], "name": "Pending",
                    "arrangement": "Arrangement Point", "fabric_index": 1,
                }
                cases["failed_creation"] = {"no_creation": True, "named_panel_after_arrangement_error": after["patterns"][-1]}

                for label, index, pattern, point in (("named", 1, second, first_point), ("index", 0, "0", last_point)):
                    result = await json_result(session, "pattern_set_arrangement", {"pattern": pattern, "point": point})
                    info = await json_result(session, "pattern_info", {"pattern": str(index)})
                    assert result == {"index": index, "arrangement": info["arrangement"]}
                    assert result["arrangement"]["ArrangementName"] == point
                    cases[f"set_{label}"] = result
                before = await json_result(session, "scene_info")
                for reference, index in (("99", 99), ("-1", -1)):
                    assert await json_result(session, "pattern_set_arrangement", {
                        "pattern": reference, "point": first_point,
                    }) == {"index": index, "arrangement": {}}
                for reference, point in (("0", missing), ("missing MCP panel", first_point), ("+1", first_point)):
                    assert (await session.call_tool("pattern_set_arrangement", {"pattern": reference, "point": point})).isError
                assert await json_result(session, "scene_info") == before
                cases["set_invalid"] = {"out_of_range_empty": 2, "rejected": 3, "scene_unchanged": True}
                observations[backend] = cases
            finally:
                await json_result(session, "scene_open", request)
    assert observations["official"] == observations["rust"], observations
    with TemporaryDirectory(prefix="clo-two-corner-") as directory:
        root = Path(directory)
        async with open_backend("rust") as session:
            await json_result(session, "pattern_create", {"name": "Curve Probe", "points": curved})
            portable = root / "curve.clorust"
            await json_result(session, "scene_save", {"path": str(portable), "thumbnail": False})
            await json_result(session, "scene_open", {"path": str(portable)})
            info = await json_result(session, "pattern_info", {"pattern": "0"})
            assert len(info["line_info"]["lines"]) == 2
            await json_result(session, "simulate", {"steps": 0, "particle_distance": 18})
            glb = root / "curve.glb"
            await json_result(session, "scene_export", {"path": str(glb)})
            model, _ = read_gltf_export(glb)
            primitive = model["meshes"][0]["primitives"][0]
            assert model["accessors"][primitive["indices"]]["count"] > 0
    print(json.dumps({"fixture": fixture.name, "cases": observations["rust"],
                      "two_corner_rust_portable_mesh": True}, ensure_ascii=False))


async def compare_native_pattern_point_edits():
    fixture = Path(os.environ["CLO_PATTERN_EDIT_FIXTURE"])
    observations = {}

    async def read(session, pattern):
        info = await json_result(session, "pattern_info", {"pattern": pattern})
        brief = await json_result(session, "pattern_list")
        detailed = await json_result(session, "pattern_list", {"detail": True})
        index = info["index"]
        brief_row, detail_row = brief["patterns"][index], detailed["patterns"][index]
        assert "lines" not in brief_row and "seam_groups" not in brief_row
        assert {key: detail_row[key] for key in brief_row} == brief_row
        assert detailed["count"] == brief["count"]
        return {"info": info, "listing": detailed}

    def compare(expected, observed, path):
        if isinstance(expected, dict):
            assert expected.keys() == observed.keys(), path
            for key in expected:
                compare(expected[key], observed[key], (*path, key))
        elif isinstance(expected, list):
            for index, (left, right) in enumerate(zip(expected, observed, strict=True)):
                compare(left, right, (*path, index))
        elif type(expected) in (int, float) or path[-1] in ("Width", "Height"):
            assert abs(float(expected) - float(observed)) <= 0.01, (path, expected, observed)
        else:
            assert expected == observed, (path, expected, observed)

    for backend in ("official", "rust"):
        async with open_backend(backend) as session:
            current = await json_result(session, "scene_info")
            path = str(fixture if backend == "official" else fixture.resolve(strict=True))
            await json_result(session, "scene_open", {"path": path})
            cases = {}
            try:
                piece = await json_result(session, "pattern_create", {
                    "name": "EditProbe", "points": [[500, 0], [620, 0], [610, 100], [500, 90]],
                })
                pattern = str(piece["index"])
                start = await read(session, pattern)
                zero = await json_result(session, "pattern_edit_points", {
                    "pattern": "EditProbe", "moves": [{"point": "0", "x": "ignored", "dy": "0"}],
                })
                assert zero == {"index": piece["index"], "moved": [{"point": 0, "x": 500, "y": 0}]}
                assert await read(session, pattern) == start
                moves = [{"point": " 1 ", "x": "ignored", "dx": "40"},
                         {"point": 2.9, "x": "625.5", "y": "100"}, {"point": 1, "dx": 50}]
                reply = await json_result(session, "pattern_edit_points", {"pattern": "EditProbe", "moves": moves})
                assert reply == {"index": piece["index"], "moved": [
                    {"point": 1, "x": 660, "y": 0}, {"point": 2, "x": 625.5, "y": 100},
                    {"point": 1, "x": 670, "y": 0},
                ]}
                cases["new_panel_moved"] = await read(session, pattern)
                partial = await session.call_tool("pattern_edit_points", {
                    "pattern": pattern, "moves": [{"point": 0, "dx": 15}, {"point": 1, "dy": "invalid"}],
                })
                assert partial.isError
                cases["partial_batch"] = await read(session, pattern)
                assert cases["partial_batch"]["info"]["line_info"]["lines"][0]["start"] == {"x": 515, "y": 0}
                for invalid in ("20", "-1", "bad"):
                    result = await session.call_tool("pattern_edit_points", {
                        "pattern": pattern, "moves": [{"point": invalid, "dx": 3, "dy": 4}],
                    })
                    assert result.isError
                assert await read(session, pattern) == cases["partial_batch"]

                for name, moves in (
                    ("Front", [{"point": 1, "dx": 15, "dy": 10}]),
                    ("EndpointControls", [{"point": 1, "x": 644, "y": 30}, {"point": 0, "x": 505, "y": -9}]),
                    ("EndpointSpline", [{"point": 1, "x": 944, "y": 30}, {"point": 0, "x": 805, "y": -9}]),
                ):
                    cases[name] = {
                        "reply": await json_result(session, "pattern_edit_points", {"pattern": name, "moves": moves}),
                        "state": await read(session, name),
                    }
                observations[backend] = cases
            finally:
                if backend == "official":
                    await json_result(session, "scene_open", {"path": current["project_path"]})
    print(json.dumps({"tools": ["pattern_edit_points", "pattern_info", "pattern_list"],
                      "fixture": fixture.name, "tolerance_mm": 0.01,
                      "zero_offset_preserved_state": True, "rejected_point_indices": 3,
                      "cases": observations}))
    compare(observations["official"], observations["rust"], ("point_edits",))


async def compare_native_pattern_delete():
    fixture = Path(os.environ["CLO_PATTERN_DELETE_FIXTURE"]).resolve(strict=True)
    observations = {}
    for backend in ("official", "rust"):
        async with open_backend(backend) as session:
            assert (await json_result(session, "clo_status"))["bridge"] == "up"
            path = str(fixture)
            if backend == "official":
                path = (await json_result(session, "scene_info"))["project_path"]
                assert Path(path).resolve() == fixture, "open the saved fixture in CLO first"
            request = {"path": path, "mode": "replace"}
            await json_result(session, "scene_open", request)
            original = await json_result(session, "scene_info")
            first, second = [piece["name"] for piece in original["patterns"]]
            assert original["seam_groups"] == 1

            async def observe(label, tool, arguments, expected, remaining):
                await json_result(session, "scene_open", request)
                result = await session.call_tool(tool, arguments)
                if expected is None:
                    assert result.isError, (backend, label)
                    response = None
                else:
                    assert not result.isError, (backend, label, result.content)
                    response = result.structuredContent or json.loads(result.content[0].text)
                    assert response == expected, (backend, label, response, expected)
                after = await json_result(session, "scene_info")
                assert [piece["name"] for piece in after["patterns"]] == remaining
                assert [piece["index"] for piece in after["patterns"]] == list(range(len(remaining)))
                count = await json_result(session, "seams_list")
                assert count["count"] == after["seam_groups"] == int(len(remaining) == 2)
                for field in ("project", "project_path", "avatars", "colorways", "current_colorway"):
                    assert after[field] == original[field], (backend, label, field)
                observations.setdefault(backend, {})[label] = {
                    "response": response, "patterns": after["patterns"], "seam_groups": count["count"],
                    "fabrics": after["fabrics"],
                }

            try:
                cases = (
                    ("empty", [], 0, [first, second]),
                    ("name", [first], 1, [second]),
                    ("aliases", [second, "01", "1", "1"], 1, [first]),
                    ("descending", ["0", second, first], 2, []),
                    ("missing_name", [first, "missing MCP panel"], None, [first, second]),
                    ("positive_out_of_range", ["200"], 1, [first, second]),
                    ("positive_mixed", ["200", second], 2, [first]),
                    ("negative_out_of_range", ["-1"], 1, [first, second]),
                    ("negative_mixed", ["-1", first], 2, [second]),
                    ("positive_overflow_first", ["2147483648", first], None, [first, second]),
                    ("negative_overflow_last", ["-2147483649", first], None, [second]),
                    ("plus_is_name", ["+1"], None, [first, second]),
                    ("malformed_index", ["--1", second], None, [first, second]),
                )
                for label, indices, deleted, remaining in cases:
                    expected = None if deleted is None else {"deleted": deleted, "pattern_count": len(remaining)}
                    await observe(label, "pattern_delete", {"patterns": indices}, expected, remaining)
                api_name = "pattern_api.DeletePatternPiece"
                for index, remaining in ((0, [second]), (99, [first, second]), (-1, [first, second])):
                    await observe(f"typed_{index}", "clo_api_call", {"name": api_name, "args": [index]},
                                  {"name": api_name, "value": None}, remaining)

                await json_result(session, "scene_open", request)
                original_groups = await json_result(session, "seams_list")
                for name, offset in (("Spare A", 250), ("Spare B", 500)):
                    await json_result(session, "pattern_create", {
                        "name": name, "points": [[offset, 0], [offset + 80, 0], [offset + 20, 40]],
                    })
                sewn = await json_result(session, "sew", {
                    "pattern_a": "Spare A", "line_a": 0, "pattern_b": "Spare B", "line_b": 0,
                })
                assert sewn == {"ok": True, "seam_groups": 2}
                assert await json_result(session, "pattern_delete", {
                    "patterns": ["Spare A", "Spare B", "2"],
                }) == {"deleted": 2, "pattern_count": 2}
                assert await json_result(session, "seams_list") == original_groups
                assert await json_result(session, "scene_info") == original
                observations[backend]["unrelated_group"] = [{
                    key: group[key] for key in ("index", "name", "patterns")
                } for group in original_groups["groups"]]
            finally:
                await json_result(session, "scene_open", request)
    assert observations["official"] == observations["rust"], observations
    with fixture.open("rb") as file:
        digest = hashlib.file_digest(file, "sha256").hexdigest()
    print(json.dumps({"fixture": fixture.name, "sha256": digest, "cases": observations["rust"]}, ensure_ascii=False))


async def verify_delete_keeps_drape():
    with TemporaryDirectory(prefix="clo-delete-drape-") as directory:
        root = Path(directory)
        async with open_backend("rust", {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": ""}) as rust:
            await inspect_contract(rust)

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]

            names = ("Front", "Drop 1", "Back", "Drop 2")
            for i, name in enumerate(names):
                x = 140 * i
                await json_result(rust, "pattern_create", {
                    "name": name, "points": [[x, 0], [x + 100, 0], [x + 45, 70]],
                })
            for a, b in (("Front", "Back"), ("Front", "Drop 1"), ("Drop 2", "Back")):
                await json_result(rust, "sew", {"pattern_a": a, "line_a": 0,
                                                "pattern_b": b, "line_b": 0})
            await json_result(rust, "simulate", {"steps": 3, "particle_distance": 50})
            _, before = await save("before")
            for index in (0, 3):
                await json_result(rust, "clo_api_call", {
                    "name": "pattern_api.SelectPatternViaIndex", "args": [index, True],
                })
            observed = await json_result(rust, "clo_ui_state")
            invalid = await rust.call_tool("pattern_delete", {"patterns": ["Drop 1", "missing"]})
            assert invalid.isError
            _, rejected = await save("rejected")
            assert rejected["patterns"] == before["patterns"]
            assert rejected["simulation"] == before["simulation"]
            assert rejected["seams"] == before["seams"]

            result = await json_result(rust, "pattern_delete", {"patterns": ["Drop 1", "3", "Drop 1"]})
            assert result == {"deleted": 2, "pattern_count": 2}
            stale = await rust.call_tool("clo_ui_action", {
                "snapshot": observed["snapshot"], "action": "set_value", "element": 8, "value": "Stale",
            })
            assert stale.isError and any("Stale UI snapshot" in block.text for block in stale.content)
            path, kept = await save("edited")
            assert [p["name"] for p in kept["patterns"]] == ["Front", "Back"]
            assert kept["seams"] == before["seams"][:1]
            remaining = [mesh for mesh in before["simulation"]["meshes"] if mesh["name"] in ("Front", "Back")]
            assert kept["simulation"] == before["simulation"] | {"meshes": remaining}
            assert (await json_result(rust, "seams_list"))["count"] == 1
            await json_result(rust, "pattern_create", {
                "name": "Replacement", "points": [[0, 0], [80, 0], [0, 80]],
            })
            _, reused = await save("reused-id")
            assert reused["patterns"][-1]["id"] == before["patterns"][-1]["id"]
            selected = await json_result(rust, "clo_api_call", {"name": "pattern_api.GetSelectedPattern"})
            assert selected["value"] == ["Front"]
            ui = await json_result(rust, "clo_ui_state")
            assert next(node for node in ui["nodes"] if node["id"] == 8)["AXValue"] == "Front"
            await json_result(rust, "scene_open", {"path": str(path)})
            await json_result(rust, "simulate", {"steps": 0})
            _, zero = await save("zero")
            assert zero["simulation"] == kept["simulation"]
            await json_result(rust, "simulate", {"steps": 1})
            _, continued = await save("continued")
            assert continued["simulation"]["steps"] == 4
            await json_result(rust, "pattern_delete", {"patterns": ["Front", "Back"]})
            _, empty = await save("empty")
            assert empty["simulation"] is None and not empty["seams"]

            native_count = None
            if fixture := os.environ.get("CLO_DISPLAY_FIXTURE"):
                await json_result(rust, "scene_open", {"path": fixture})
                _, original = await save("native")
                removed = original["patterns"][0]["id"]
                expected = [m for m in original["simulation"]["meshes"] if m["id"] != removed]
                assert 0 < len(expected) < len(original["simulation"]["meshes"])
                groups = [g for g in original["native_seam_groups"] if not any(
                    end["pattern_id"] == removed for pair in g["segments"] for end in pair
                )]
                await json_result(rust, "pattern_delete", {"patterns": ["0"]})
                path, native = await save("native-edited")
                assert native["native_seam_groups"] == groups
                await json_result(rust, "scene_open", {"path": str(path)})
                _, opened = await save("native-reopened")
                assert opened["simulation"] == original["simulation"] | {"meshes": expected}
                native_count = len(expected)
    print(json.dumps({"tools": ["pattern_delete", "seams_list", "scene_open", "simulate"],
                      "case": "delete panels without resetting the remaining drape",
                      "retained_panels": 2, "retained_seam_groups": 1, "continued_step": 4,
                      "invalid_batch_and_empty_scene": "pass", "native_meshes_preserved": native_count,
                      "deleted_selection_cannot_follow_reused_id": True, "pre_delete_ui_token_rejected": True}))


async def verify_sew_keeps_drape():
    with TemporaryDirectory(prefix="clo-sew-drape-") as directory:
        root = Path(directory)
        async with open_backend("rust", {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": ""}) as rust:
            await inspect_contract(rust)

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]

            async def seam_readback():
                catalog = await json_result(rust, "seams_list")
                groups = catalog["groups"]
                count = len(groups)
                result = await json_result(rust, "clo_run_python", {"code": "pattern_api.GetSeamlinePairGroupCount()"})
                assert result["value"] == catalog["count"] == count
                for index in sorted({0, count // 2, count - 1}) if count else []:
                    expected = groups[index]
                    response = await json_result(rust, "clo_api_call", {
                        "name": "pattern_api.GetSeamlinePairGroupNameW", "args": [index],
                    })
                    assert response["value"] == expected["name"]
                    lookup = await json_result(rust, "clo_api_call", {
                        "name": "pattern_api.GetSeamlinePairGroupIndexFromName", "args": [expected["name"]],
                    })
                    assert lookup["value"] == next(i for i, group in enumerate(groups) if group["name"] == expected["name"])
                info = await json_result(rust, "pattern_info", {"pattern": "0"})
                response = await json_result(rust, "clo_api_call", {
                    "name": "pattern_api.GetSeamlinePairGroupListInPattern", "args": [0],
                })
                assert response["value"] == info["seam_groups"]
                return count

            await json_result(rust, "pattern_create", {
                "name": "Front", "points": [[0, 0], [100, 0], [45, 70]],
            })
            await json_result(rust, "pattern_create", {
                "name": "Back", "points": [[160, 0], [260, 0], [205, 70]],
            })
            await json_result(rust, "simulate", {"steps": 3, "particle_distance": 50})
            _, before = await save("before")
            seam = {"pattern_a": "Front", "line_a": 0, "pattern_b": "Back", "line_b": 0,
                    "dir_a": True, "dir_b": False}
            joined = await json_result(rust, "sew", seam)
            assert joined == {"ok": True, "seam_groups": 1}
            assert await seam_readback() == 1
            duplicate = await json_result(rust, "sew", {
                "pattern_a": "Back", "line_a": 0, "pattern_b": "Front", "line_b": 0,
            })
            assert duplicate == {"ok": True, "seam_groups": 1}
            invalid = await json_result(rust, "sew", seam | {"line_a": 99})
            assert invalid == {"ok": False, "seam_groups": 1}
            path, stitched = await save("stitched")
            assert stitched["simulation"] == before["simulation"]
            await json_result(rust, "scene_open", {"path": str(path)})
            constraints = await json_result(rust, "simulate", {"steps": 0})
            assert constraints["stitch_count"] > 0
            _, zero = await save("zero")
            assert zero["simulation"] == before["simulation"]
            await json_result(rust, "simulate", {"steps": 1})
            _, solved = await save("solved")
            assert solved["simulation"]["steps"] == 4
            assert solved["simulation"]["meshes"] != before["simulation"]["meshes"]

            native_meshes = None
            native_counts = None
            if fixture := os.environ.get("CLO_DISPLAY_FIXTURE"):
                await json_result(rust, "scene_open", {"path": fixture})
                _, source = await save("native")
                native_meshes = len(source["simulation"]["meshes"])
                assert native_meshes > 0 and source["simulation"]["source"] == "native_saved_pose"
                initial_seams = await seam_readback()
                for i in range(1, len(source["patterns"])):
                    result = await json_result(rust, "sew", seam | {"pattern_a": "0", "pattern_b": str(i)})
                    if result["ok"]:
                        break
                else:
                    raise AssertionError("fixture had no new boundary seam")
                path, native = await save("native-stitched")
                assert native["simulation"] == source["simulation"]
                await json_result(rust, "scene_open", {"path": str(path)})
                after = await seam_readback()
                assert after == initial_seams + 1
                native_counts = [initial_seams, after]
                _, reopened = await save("native-reopened")
                assert reopened["simulation"] == source["simulation"]
    print(json.dumps({"tools": ["sew", "seams_list", "scene_open", "simulate", "clo_api_call", "clo_run_python"],
                      "case": "sewing an existing drape", "new_stitch_constraints": constraints["stitch_count"],
                      "continued_step": 4, "pose_and_velocities": "unchanged",
                      "duplicate_and_out_of_range": "pass", "native_pose_meshes_preserved": native_meshes,
                      "native_group_count_before_after": native_counts, "api_order_and_memberships_match": True}))


async def compare_live_pattern_ops():
    fixture = Path(os.environ["CLO_PATTERN_OPS_FIXTURE"])
    assert fixture.is_file(), fixture
    outline = [[20, 100], [90, 100], [120, 178], [45, 202]]
    cases = (
        ("rename", {"name": "服装 A"}, False, False),
        ("move_2d", {"dx": "25.5", "dy": -11}, False, False),
        ("move_2d", {"x": "20", "y": -23}, False, False),
        ("move_2d", {"x": 20}, False, True),
        ("flip", {}, True, False),
        ("flip", {"horizontal": False, "each": False}, True, False),
        ("copy", {}, False, False),
        ("copy", {"dx": 35, "dy": -18}, True, False),
        ("symmetry", {"with_sewing": False}, True, False),
        ("symmetry", {}, False, False),
        ("unfold", {"line": 0}, True, False),
        ("unfold", {"line": "2", "half_symmetry": False}, True, False),
        ("unfold", {"line": -1}, True, False),
        ("unfold", {"line": 99}, True, False),
        ("layer", {"layer": "3"}, False, False),
        ("layer", {"layer": -2}, False, False),
        ("particle_distance", {"mm": "9.33"}, False, False),
        ("particle_distance", {"mm": 0}, False, False),
        ("particle_distance", {"mm": -10}, False, False),
        ("particle_distance", {"mm": 10000}, False, False),
        ("elastic", {"line": 99, "strength": 20}, False, False),
        ("delete_line", {"line": 1}, True, False),
        ("delete_line", {"line": 99}, True, False),
        ("delete_point", {"point": 1}, True, False),
        ("delete_point", {"point": 1}, False, False),
        ("delete_point", {"point": 99}, True, False),
        ("unknown", {}, False, True),
    )
    tolerance = 0.001

    async def read(session):
        listing = await json_result(session, "pattern_list")
        panels = []
        for i in range(listing["count"]):
            panel = await json_result(session, "pattern_info", {"pattern": str(i)})
            source = panel["input_info"]["Pattern InputInformation"][0]
            points = [(point["Point positionX"], point["Point positionY"]) for point in source["PointList"]
                      if "Point index" in point]
            properties = {key: panel[key] for key in
                          ("name", "fabric_index", "layer", "particle_distance", "seam_groups")}
            coordinates = [*panel["pos_2d"], float(panel["bbox"]["Width"]), float(panel["bbox"]["Height"])]
            coordinates.extend(coordinate for point in points for coordinate in point)
            panels.append((properties, coordinates))
        seams = await json_result(session, "seams_list")
        return panels, [group["patterns"] for group in seams["groups"]]

    def same(clo, rust):
        assert clo[1] == rust[1] and len(clo[0]) == len(rust[0]), (clo, rust)
        for (a, x), (b, y) in zip(clo[0], rust[0], strict=True):
            assert a == b, (a, b)
            assert len(x) == len(y) and all(abs(i - j) <= tolerance for i, j in zip(x, y, strict=True)), (x, y)

    async with open_backend("official") as clo, open_backend("rust") as rust:
        original = await json_result(clo, "scene_info")
        assert (await json_result(clo, "clo_status"))["bridge"] == "up"
        evidence = []
        try:
            for session in (clo, rust):
                await inspect_contract(session)
            for op, args, add_panel, expected_error in cases:
                trial = {"pattern": "Trial_01" if add_panel else "Front", "op": op, "args": args}
                before, after, replies = [], [], []
                for session in (clo, rust):
                    await json_result(session, "scene_open", {"path": str(fixture)})
                    if add_panel:
                        await json_result(session, "pattern_create", {"name": "Trial_01", "points": outline})
                    before.append(await read(session))
                    if expected_error:
                        rejected = await session.call_tool("pattern_ops", trial)
                        assert rejected.isError, trial
                        replies.append("error")
                    else:
                        replies.append(await json_result(session, "pattern_ops", trial))
                    after.append(await read(session))
                    if expected_error or replies[-1].get("ok") is False:
                        assert before[-1] == after[-1], trial
                assert replies[0] == replies[1], (trial, replies)
                same(*before)
                same(*after)
                evidence.append({"op": op, "args": args, "response": replies[0],
                                 "panels_after": [p[0]["name"] for p in after[0][0]], "seams_after": after[0][1]})

            for session in (clo, rust):
                await json_result(session, "scene_clear", {"keep_avatar": False})
                await json_result(session, "pattern_create", {"name": "Other", "points": [[0, 0], [45, 0], [60, 60], [8, 80]]})
                await json_result(session, "sew", {"pattern_a": "Other", "line_a": 0,
                                                  "pattern_b": "Other", "line_b": 1})
                for expected in ("Other_1", "Other_2"):
                    await json_result(session, "pattern_ops", {"pattern": "Other", "op": "symmetry"})
                    panel = await json_result(session, "pattern_info", {"pattern": expected})
                    assert panel["pos_2d"] == [160.0, 40.0]
                assert (await read(session))[1] == [["Other"], ["Other_1"], ["Other_2"]]
        finally:
            if original["project_path"]:
                await json_result(clo, "scene_open", {"path": original["project_path"]})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    print(json.dumps({"tool": "pattern_ops", "cases": evidence, "repeated_self_seam_symmetry": True,
                      "geometry_tolerance_mm": tolerance, "result": "pass"}, ensure_ascii=False))


async def verify_display_edits_keep_drape():
    with TemporaryDirectory(prefix="clo-display-edits-") as directory:
        root = Path(directory)
        environment = {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": "", "CLO_RUST_OUTPUT_DIR": directory}
        async with open_backend("rust", environment) as rust:
            await inspect_contract(rust)
            for name, x in (("Front", 0), ("Back", 240)):
                await json_result(rust, "pattern_create", {
                    "name": name, "points": [[x, 0], [x + 100, 0], [x + 50, 100]],
                })
            await json_result(rust, "simulate", {"steps": 3, "particle_distance": 50})

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]

            async def capture(tool):
                args = {"views": ["front"]} if tool == "snapshot" else {}
                result = await successful(rust, tool, args)
                image, = [block for block in result.content if block.type == "image"]
                return base64.b64decode(image.data, validate=True)

            async def operate(panel, op, args):
                await json_result(rust, "pattern_ops", {"pattern": panel, "op": op, "args": args})

            before = (await json_result(rust, "pattern_info", {"pattern": "Front"}))["pos_2d"]
            assert isinstance(before, list)
            _, scene = await save("before")
            original_3d, original_2d = await capture("snapshot"), await capture("snapshot_2d")
            invalid = await rust.call_tool("pattern_ops", {
                "pattern": "Front", "op": "move_2d", "args": {"x": "140"},
            })
            assert invalid.isError
            assert (await json_result(rust, "pattern_info", {"pattern": "Front"}))["pos_2d"] == before
            ui = await json_result(rust, "clo_ui_state")
            await operate("Front", "move_2d", {"dx": "80", "dy": "55", "y": "ignored"})
            stale = await rust.call_tool("clo_ui_action", {
                "snapshot": ui["snapshot"], "action": "press", "element": 2,
            })
            assert stale.isError and any("Stale UI snapshot" in block.text for block in stale.content)
            moved = (await json_result(rust, "pattern_info", {"pattern": "Front"}))["pos_2d"]
            assert moved == [before[0] + 80, before[1] + 55]
            moved_2d = await capture("snapshot_2d")
            assert moved_2d != original_2d and await capture("snapshot") == original_3d
            await operate("Front", "hide", {"on": True})
            assert await capture("snapshot") != original_3d and await capture("snapshot_2d") == moved_2d
            mesh_path = root / "visible.obj"
            await json_result(rust, "scene_export", {"path": str(mesh_path)})
            assert [line for line in mesh_path.read_text().splitlines() if line.startswith("o ")] == ["o Back"]
            await operate("Front", "hide", {"on": False})
            await operate("Front", "strengthen", {"on": True})
            assert await capture("snapshot") == original_3d

            path, edited = await save("edited")
            assert edited["simulation"] == scene["simulation"]
            assert edited["patterns"][0]["strengthen"] is True
            await json_result(rust, "scene_open", {"path": str(path)})
            await json_result(rust, "simulate", {"steps": 1})
            _, continued = await save("continued")
            assert continued["simulation"]["steps"] == 4
            await operate("Front", "flip", {"horizontal": True})
            _, flipped = await save("flipped")
            assert flipped["simulation"]["meshes"] == [continued["simulation"]["meshes"][1]]
            source = (await json_result(rust, "pattern_info", {"pattern": "Back"}))["pos_2d"]
            copied = await json_result(rust, "pattern_ops", {
                "pattern": "Back", "op": "copy", "args": {"x": 0, "y": 0, "dy": "-18"},
            })
            duplicate = await json_result(rust, "pattern_info", {"pattern": str(copied["new_index"])})
            assert duplicate["pos_2d"] == [source[0] + 300, source[1] - 18]

            native_meshes = None
            if fixture := os.environ.get("CLO_DISPLAY_FIXTURE"):
                await json_result(rust, "scene_open", {"path": fixture})
                _, original = await save("native")
                native_meshes = len(original["simulation"]["meshes"])
                assert native_meshes > 0 and original["simulation"]["source"] == "native_saved_pose"
                await operate("0", "move_2d", {"x": "500", "y": "250", "dx": "ignored"})
                front = await json_result(rust, "pattern_info", {"pattern": "0"})
                assert front["pos_2d"] == [500, 250]
                await operate("0", "hide", {"on": True})
                path, changed = await save("native-edited")
                assert changed["simulation"] == original["simulation"]
                await json_result(rust, "scene_open", {"path": str(path)})
                await operate("0", "hide", {"on": False})
                _, restored = await save("native-restored")
                assert restored["simulation"] == original["simulation"]
    print(json.dumps({"tools": ["pattern_ops", "snapshot", "snapshot_2d", "scene_export", "simulate"],
                      "case": "layout and display edits preserve drape",
                      "pose_and_velocities": "unchanged", "continue_and_flip_control": "pass",
                      "native_pose_meshes_preserved": native_meshes,
                      "reference_move_and_copy_argument_rules": True, "old_ui_snapshot_rejected": True}))


async def verify_simulation_remesh_keeps_pose():
    with TemporaryDirectory(prefix="clo-simulation-remesh-") as directory:
        root = Path(directory)
        async with open_backend("rust", {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": ""}) as rust:
            await inspect_contract(rust)

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]["simulation"]

            await json_result(rust, "pattern_create", {
                "name": "Panel", "points": [[0, 500], [140, 500], [58, 610]],
            })
            async def batch_distance(distance):
                return await json_result(rust, "clo_api_call", {
                    "name": "pattern_api.SetParticleDistanceOfPatterns", "args": [distance],
                })

            assert (await batch_distance(90))["value"] is None
            await json_result(rust, "simulate", {"steps": 0})
            _, rest = await save("rest")
            await json_result(rust, "simulate", {"steps": 4})
            posed_path, posed = await save("posed")
            result = await json_result(rust, "simulate", {"steps": 0, "particle_distance": 25})
            assert result["remesh_preserved_panels"] == 1 and result["remesh_reinitialized_panels"] == 0
            path, refined = await save("refined")
            old, = posed["meshes"]
            new, = refined["meshes"]
            assert len(new["positions"]) > len(old["positions"]) and refined["steps"] == 4
            indices = {tuple(uv): i for i, uv in enumerate(new["texcoords"])}
            anchors = [(i, indices[tuple(uv)]) for i, uv in enumerate(old["texcoords"]) if tuple(uv) in indices]
            assert len(anchors) >= 3
            for a, b in anchors:
                assert old["positions"][a] == new["positions"][b]
                assert old["velocities"][a] == new["velocities"][b]
            for velocity in new["velocities"]:
                assert all(abs(a-b) < 1e-8 for a, b in zip(velocity, old["velocities"][0], strict=True))
            await json_result(rust, "scene_open", {"path": str(posed_path)})
            assert (await batch_distance(25))["value"] is None
            info = await json_result(rust, "pattern_info", {"pattern": "Panel"})
            assert info["particle_distance"] == 25
            _, api_refined = await save("api-refined")
            assert api_refined == refined
            denied = await rust.call_tool("clo_api_call", {
                "name": "pattern_api.SetParticleDistanceOfPatterns", "args": [1e39],
            })
            assert denied.isError
            result = await json_result(rust, "simulate", {"steps": 0, "particle_distance": 0})
            assert "remesh_preserved_panels" not in result
            _, zero_distance = await save("zero-distance")
            assert zero_distance == refined
            await json_result(rust, "scene_open", {"path": str(path)})
            await json_result(rust, "simulate", {"steps": 1})
            _, resumed = await save("resumed")
            assert resumed["steps"] == 5
            await json_result(rust, "simulation_reset")
            await json_result(rust, "simulate", {"steps": 0})
            _, initialized = await save("initialized")
            assert initialized["steps"] == 0
            fresh, = initialized["meshes"]
            assert fresh["triangles"] == new["triangles"]
            offset = old["positions"][0][1] - rest["meshes"][0]["positions"][0][1]
            assert offset < 0
            assert all(abs(posed[1] - start[1] - offset) < 1e-8
                       for posed, start in zip(new["positions"], fresh["positions"], strict=True))
    print(json.dumps({"tools": ["simulate", "simulation_reset", "scene_open", "clo_api_call"],
                      "case": "remesh transfers the current cloth pose",
                      "coarse_vertices": len(old["positions"]), "fine_vertices": len(new["positions"]),
                      "exact_shared_vertices": len(anchors), "continued_step": 5,
                      "api_remesh_matches_simulate": True, "zero_distance_kept_mesh": True}))


async def verify_native_simulation_reference():
    root = Path(os.environ["CLO_SIMULATION_REFERENCES"]).resolve(strict=True)
    observed = json.loads((root / "native-simulation-probe.json").read_text())["official"]
    limits = {row["label"]: row for row in map(json.loads, (root / "native-simulation-limits.jsonl").read_text().splitlines())}
    rejected_stage = json.loads((root / "native-simulation-error-stage.json").read_text())
    archives = root / "native-simulation-archives"

    async def save(session, target):
        await json_result(session, "scene_save", {"path": str(target), "thumbnail": False})
        return json.loads(target.read_text())["scene"]

    def meshes(scene):
        return scene["simulation"]["meshes"]

    def verify_rest(expected, actual):
        for left, right in zip(meshes(expected), meshes(actual), strict=True):
            assert left["name"] == right["name"] and left["triangles"] == right["triangles"]
            assert left["texcoords"] == right["texcoords"]
            assert all(math.dist(a, b) <= 0.001 for a, b in zip(left["positions"], right["positions"], strict=True))

    def seam_gap(scene):
        front, back = [[(uv[0], xyz) for uv, xyz in zip(mesh["texcoords"], mesh["positions"], strict=True)
                        if abs(uv[1]) < 0.001] for mesh in meshes(scene)]
        pairs = zip(sorted(front), sorted(back, reverse=True), strict=True)
        return max(math.dist(a[1], b[1]) for a, b in pairs)

    with TemporaryDirectory(prefix="clo-sim-reference-") as directory:
        target = Path(directory) / "state.clorust"
        captures = {}
        async with open_backend("rust") as reader:
            for stage in ("initial", "zero", "three", "next", "coarser", "default_particle_zero"):
                await json_result(reader, "scene_open", {"path": str(archives / f"simulation-stage-{stage}.zpac")})
                captures[stage] = await save(reader, target)
        verify_rest(captures["initial"], captures["zero"])
        verify_rest(captures["coarser"], captures["default_particle_zero"])
        async with open_backend("rust") as rust:
            await json_result(rust, "scene_open", {"path": str(archives / "simulation-stage-original.zpac")})
            reply = await json_result(rust, "simulation_reset")
            assert reply == observed["reset"]
            verify_rest(captures["initial"], await save(rust, target))
            gaps = {"official": {}, "rust": {}}
            for stage, steps, accumulated in (("zero", 0, 0), ("three", 3, 3), ("next", 2, 5)):
                args = {"steps": steps} | ({"quality": 0} if stage == "three" else {})
                reply = await json_result(rust, "simulate", args)
                expected = observed[stage]["value"]
                assert reply["ok"] == expected["ok"] and reply["steps"] == expected["steps"] == steps
                assert reply["seconds"] >= 0 and reply["seconds"] == round(reply["seconds"], 2)
                record = await save(rust, target)
                assert record["simulation"]["steps"] == accumulated
                if stage == "zero":
                    verify_rest(captures[stage], record)
                gaps["official"][stage] = seam_gap(captures[stage])
                gaps["rust"][stage] = seam_gap(record)
            for result in gaps.values():
                assert result["three"] < result["zero"] and result["next"] < 1.0, result
            assert await json_result(rust, "simulation_reset") == observed["reset_after_drape"]
            restored = await save(rust, target)
            verify_rest(captures["initial"], restored)
            assert restored["native_seam_groups"] == captures["initial"]["native_seam_groups"]

            qualities = {}
            for quality in (0, 1, 2, -1, 4):
                reply = await json_result(rust, "simulate", {"steps": 0, "quality": quality})
                assert reply["ok"] and reply["steps"] == 0
                lookup = await json_result(rust, "clo_api_call", {
                    "name": "utility_api.GetSimulationQuality", "args": [],
                })
                assert lookup == json.loads(limits[f"observed_quality_{quality}"]["value"])
                qualities[quality] = lookup["value"]
            checked = {}
            for label, distance in (
                ("coarser", 32), ("default_particle_zero", 0), ("distance_above", 501),
                ("distance_small", 2), ("distance_negative", -5),
            ):
                args = {"steps": 0, "particle_distance": distance}
                reply = await json_result(rust, "simulate", args)
                assert reply["ok"] is observed[label]["value"]["ok"] and reply["steps"] == 0
                distances = [(await json_result(rust, "pattern_info", {"pattern": name}))["particle_distance"]
                             for name in ("Front", "Back")]
                assert distances == observed[f"{label}_distances"], (label, distances)
                checked[label] = distances
            invalid = await rust.call_tool("simulate", {"steps": -1, "particle_distance": 28, "quality": 2})
            assert invalid.isError == rejected_stage["rejected"]
            distances = [(await json_result(rust, "pattern_info", {"pattern": name}))["particle_distance"]
                         for name in ("Front", "Back")]
            assert distances == rejected_stage["distances"]
            for phase in ("quality", "quality_after_reset"):
                if phase == "quality_after_reset":
                    assert await json_result(rust, "simulation_reset") == rejected_stage["reset"]
                lookup = await json_result(rust, "clo_api_call", {
                    "name": "utility_api.GetSimulationQuality", "args": [],
                })
                assert lookup["value"] == rejected_stage[phase]
            await json_result(rust, "scene_clear", {"keep_avatar": False})
            for label, request in (("empty_zero", {"steps": 0, "quality": 1}), ("empty_positive", {"steps": 2})):
                assert await json_result(rust, "simulate", request) == observed[label]["value"]
            assert await json_result(rust, "simulate", {"steps": 3001}) == json.loads(limits["empty_steps_3001"]["value"])
            invalid = await rust.call_tool("simulate", {"steps": -1})
            assert invalid.isError and limits["empty_steps_-1"]["is_error"]
            assert await json_result(rust, "simulation_reset") == observed["empty_reset"]
            empty = await save(rust, target)
            assert not empty["patterns"] and empty["simulation"] is None
    print(json.dumps({"reset_pose_tolerance_mm": 0.001, "distances": checked, "quality_readback": qualities,
                      "seam_gaps_mm": gaps,
                      "accumulated_rust_steps": 5, "empty_scene_remained_empty": True,
                      "invalid_steps_settings": {"particle_distances": distances, "quality": lookup["value"]}}))


async def verify_simulation_reset_quality():
    with TemporaryDirectory(prefix="clo-reset-quality-") as directory:
        root = Path(directory)
        environment = {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": ""}
        async with open_backend("rust", environment) as rust:
            await inspect_contract(rust)

            async def save(session, name):
                path = root / f"{name}.clorust"
                await json_result(session, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]

            async def api(name, *args):
                return await successful(rust, "clo_api_call", {"name": f"utility_api.{name}", "args": list(args)})

            empty = await json_result(rust, "simulate", {"steps": 0, "quality": 1})
            assert empty == {"ok": True, "steps": 0, "seconds": 0}
            _, empty_scene = await save(rust, "after-empty")
            assert not empty_scene["patterns"] and empty_scene["simulation"] is None

            for name, x in (("Front", 0), ("Back", 140)):
                await json_result(rust, "pattern_create", {
                    "name": name, "points": [[x, 0], [x + 100, 0], [x + 45, 70]],
                })
            await json_result(rust, "sew", {"pattern_a": "Front", "line_a": 0,
                                            "pattern_b": "Back", "line_b": 0})
            result = await json_result(rust, "simulate", {"steps": 0, "particle_distance": 50})
            assert result["ok"] is True and result["steps"] == 0
            assert result["seconds"] >= 0 and result["seconds"] == round(result["seconds"], 2)
            _, after_empty = await save(rust, "after-empty")
            assert after_empty["simulation"]["quality"] == 1

            await api("SetSimulationQuality", 2)
            await json_result(rust, "simulate", {"steps": 0})
            _, initialized = await save(rust, "initial")
            assert initialized["simulation"]["quality"] == 2
            await api("Simulate", 3)
            _, settled = await save(rust, "settled")
            assert settled["simulation"]["steps"] == 3
            assert settled["simulation"]["meshes"] != initialized["simulation"]["meshes"]
            await api("ResetClothArrangement")
            _, reset = await save(rust, "api-reset")
            assert reset["simulation"] is None and reset["seams"] == initialized["seams"]
            await json_result(rust, "simulate", {"steps": 0})
            _, rebuilt = await save(rust, "after-api-reset")
            assert rebuilt["patterns"] == initialized["patterns"]
            assert rebuilt["simulation"]["meshes"] == initialized["simulation"]["meshes"]
            assert rebuilt["simulation"]["steps"] == 0 and rebuilt["simulation"]["quality"] == 2

            await json_result(rust, "simulate", {"steps": 1, "quality": 1})
            persisted, _ = await save(rust, "persisted")
            await json_result(rust, "simulation_reset")
            await api("Simulate", 0)
            _, after_public = await save(rust, "after-public-reset")
            assert after_public["simulation"] == rebuilt["simulation"] | {"quality": 1}
            denied = await rust.call_tool("simulate", {"steps": 0, "quality": 3})
            assert denied.isError
            await api("Simulate", 0)
            _, after_denied = await save(rust, "after-denied")
            assert after_denied["simulation"]["quality"] == 1
            failed_distance = await rust.call_tool(
                "simulate", {"steps": 0, "quality": 2, "particle_distance": 1e39},
            )
            assert failed_distance.isError
            _, after_error = await save(rust, "after-invalid-distance")
            assert after_error["patterns"] == after_denied["patterns"]
            assert after_error["simulation"] == after_denied["simulation"]
            await api("Simulate", 0)
            _, after_retry = await save(rust, "after-retry")
            assert after_retry["simulation"] == after_denied["simulation"] | {"quality": 2}

            async with open_backend("rust", environment) as fresh:
                await json_result(fresh, "scene_open", {"path": str(persisted)})
                await json_result(fresh, "simulation_reset")
                await json_result(fresh, "simulate", {"steps": 0})
                _, reopened = await save(fresh, "fresh-reset")
                assert reopened["simulation"] == after_public["simulation"]
    print(json.dumps({"tools": ["simulate", "simulation_reset", "clo_api_call", "scene_open"],
                      "case": "reset arrangement without changing solver quality",
                      "quality_api_to_mcp": 2, "quality_mcp_to_api_and_reopen": 1,
                      "arranged_mesh_restored": True, "unsupported_quality_preserves_setting": True,
                      "quality_retained_after_empty_simulation": True,
                      "quality_retained_after_invalid_distance": True,
                      "failed_distance_preserves_mesh": True, "seconds_resolution": 0.01}))


async def compare_live_garment_workflow():
    fixture = Path(os.environ["CLO_GARMENT_LIVE_FIXTURE"])
    state_dir = Path(os.environ["CLO_MCP_STATE"])
    cache = state_dir / "measurements" / "Mara_09_eye_Joint.json"
    assert fixture.is_file() and cache.is_file()
    spec = {"garment": "top", "silhouette": "regular", "neckline": "crew", "sleeve": "short",
            "fabric": "cotton", "color": "navy", "particle_distance": 45}
    empty = {"spec": None, "pieces": []}
    names = ["Front", "Back", "Lower_Front", "Lower_Back", "Sleeve_L", "Sleeve_R"]

    async def read(session):
        scene = await json_result(session, "scene_info")
        patterns = await json_result(session, "pattern_list")
        seams = await json_result(session, "seams_list")
        properties = ("index", "name", "arrangement", "fabric_index")
        panels = [{key: panel[key] for key in properties} for panel in patterns["patterns"]]
        geometry = [[*panel["pos_2d"], *(float(panel["bbox"][key]) for key in ("Width", "Height"))]
                    for panel in patterns["patterns"]]
        return {"project": scene["project_path"], "avatars": scene["avatars"], "panels": panels,
                "geometry": geometry, "seams": [group["patterns"] for group in seams["groups"]],
                "fabrics": await json_result(session, "fabric_list"),
                "last": await json_result(session, "garment_last_spec")}

    def same(left, right, check_project=False):
        general = ("avatars", "panels", "seams", "fabrics", "last")
        for key in (*general, "project") if check_project else general:
            assert left[key] == right[key], (key, left[key], right[key])
        for a, b in zip(left["geometry"], right["geometry"], strict=True):
            assert all(abs(x - y) <= 0.001 for x, y in zip(a, b, strict=True)), (a, b)

    async def build(session, tool, args):
        result = await successful(session, tool, args)
        assert [block.type for block in result.content] == ["text"]
        return json.loads(result.content[0].text)

    async def rejected(session, tool, args):
        assert (await session.call_tool(tool, args)).isError, (tool, args)

    def checkpoint(report, expected_suffix, scene):
        path = Path(report.pop("checkpoint_before_build"))
        assert path.suffix == expected_suffix and path.name.startswith("auto_before_build_"), path
        assert path.parent == state_dir / "checkpoints" and path.stat().st_size > 0, path
        assert scene["project"] == str(path)
        return path

    async with open_backend("official") as clo, open_backend("rust") as rust:
        original = await json_result(clo, "scene_info")
        assert (await json_result(clo, "clo_status"))["bridge"] == "up"
        sessions = (clo, rust)
        try:
            previews = []
            for session in sessions:
                await inspect_contract(session)
                await json_result(session, "scene_open", {"path": str(fixture)})
                before = await read(session)
                assert before["avatars"] == [] and before["last"] == empty
                await rejected(session, "garment_rebuild", {"changes": {}})
                assert await read(session) == before
                await rejected(session, "garment_build", {
                    "spec": spec | {"particle_distance": []}, "snapshots": False, "simulate_steps": 0,
                })
                assert await read(session) == before | {"avatars": ["Mara_09_eye_Joint"]}
                preview_state = await read(session)
                previews.append(await json_result(session, "garment_plan_preview", {
                    "spec": spec, "use_scene_measurements": True,
                }))
                assert await read(session) == preview_state
                await json_result(session, "scene_open", {"path": str(fixture)})
                await json_result(session, "pattern_delete", {"patterns": ["0"]})

            assert previews[0] == previews[1]
            assert previews[0]["measurement_source"] == f"cached CLO measurements ({cache.name})"

            append = [await build(session, "garment_build", {
                "spec": spec, "replace_existing": False, "simulate_steps": 0, "snapshots": False,
            }) for session in sessions]
            assert append[0] == append[1], append
            assert append[0]["checkpoint_before_build"] is None and append[0]["simulation"] is None
            assert append[0]["measurements"] == f"cached CLO measurements ({cache.name})"
            appended = [await read(session) for session in sessions]
            same(*appended, check_project=True)
            assert [row["name"] for row in appended[0]["panels"]] == ["Back", *names]
            assert [row["fabric_index"] for row in appended[0]["panels"]] == [0, *([1] * len(names))]
            assert appended[0]["last"] == {"spec": append[0]["spec"], "pieces": names}
            assert len(append[0]["seams"]) == 14 and append[0]["arrangement"]

            rebuilt = [await build(session, "garment_rebuild", {
                "changes": {"color": "#246", "sleeve": "long"}, "simulate_steps": 0, "snapshots": False,
            }) for session in sessions]
            latest = [await read(session) for session in sessions]
            same(*latest)
            checkpoints = [checkpoint(report, suffix, scene) for report, suffix, scene in
                           zip(rebuilt, (".zprj", ".clorust"), latest, strict=True)]
            assert rebuilt[0] == rebuilt[1], rebuilt
            assert [row["name"] for row in latest[0]["panels"]] == names
            assert [row["fabric_index"] for row in latest[0]["panels"]] == [2] * len(names)
            assert latest[0]["last"] == {"spec": rebuilt[0]["spec"], "pieces": names}
            for session, scene in zip(sessions, latest, strict=True):
                await rejected(session, "garment_rebuild", {"changes": {"particle_distance": []}, "snapshots": False})
                assert await read(session) == scene

            simulated = [await build(session, "garment_rebuild", {
                "changes": {"sleeve": "short"}, "simulate_steps": 1, "snapshots": False,
            }) for session in sessions]
            simulated_state = [await read(session) for session in sessions]
            same(*simulated_state)
            for report, suffix, scene in zip(simulated, (".zprj", ".clorust"), simulated_state, strict=True):
                checkpoint(report, suffix, scene)
                assert report["simulation"].pop("seconds") >= 0
            assert simulated[0] == simulated[1], simulated
            assert simulated[0]["simulation"]["ok"] is True and simulated[0]["simulation"]["steps"] == 1
            assert [pair["ok"] for pair in simulated[0]["simulation"]["merged_pieces"]] == [False, False]

            failed_snapshot = []
            for session in sessions:
                await rejected(session, "garment_build", {
                    "spec": spec | {"color": "red"}, "simulate_steps": 0, "views": ["invalid-verify-view"],
                })
                scene = await read(session)
                assert scene["last"]["spec"]["color"] == "red"
                assert scene["last"]["pieces"] == names
                failed_snapshot.append(scene)
            same(*failed_snapshot)

            for session, path, original_panels, history in zip(sessions, checkpoints, appended, failed_snapshot, strict=True):
                result = await json_result(session, "checkpoint_restore", {"name": path.stem})
                assert result["ok"] and result["path"] == str(path) and result["pattern_count"] == 7
                restored = await read(session)
                expected = original_panels | {"last": history["last"], "project": str(path),
                                              "fabrics": original_panels["fabrics"] | {"current": 0}}
                same(restored, expected, check_project=True)
        finally:
            if original["project_path"] not in ("NULL", "", None):
                await json_result(clo, "scene_open", {"path": original["project_path"]})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    for backend in ("official", "rust"):
        async with open_backend(backend) as session:
            assert await json_result(session, "garment_last_spec") == empty
    print(json.dumps({"append_new_pieces": len(names), "append_seams": 14, "summary_matched": True,
                      "checkpoints_reopened": [path.suffix for path in checkpoints],
                      "restored_default_fabric_selection": True, "live_measurement_preview_matched": True,
                      "invalid_draft_kept_loaded_avatar": True, "invalid_rebuild_kept_history": True,
                      "invalid_snapshot_kept_successful_build": True, "positive_step_merge_flags": [False, False],
                      "new_session_history_empty": True, "result": "pass"}))


async def compare_live_avatar_workflow():
    fixture = os.environ["CLO_AVATAR_PROJECT_FIXTURE"]
    assert Path(fixture).is_file(), fixture
    root = Path(os.environ["CLO_RUST_AVATAR_DIR"])
    cases = (
        ("default", {}, "Mara_09_eye_Joint"),
        ("normalized alias", {"name_or_path": " FeMaLe "}, "Mara_09_eye_Joint"),
        ("native import with replace=false", {"name_or_path": "luka", "replace": False},
         "MV2_Thomas_1_eye_Joint"),
        ("file name with replace=false", {"name_or_path": "MV2.1_Luka.avt", "replace": False},
         "MV2_Thomas_1_eye_Joint"),
        ("explicit replace", {"name_or_path": "mia", "replace": True}, "Mara_09_eye_Joint"),
    )
    garment_keys = ("pattern_count", "patterns", "seam_groups", "fabrics", "current_colorway")

    async def read(session):
        listing = await json_result(session, "avatar_list")
        scene = await json_result(session, "scene_info")
        assert scene["avatars"] == listing["in_scene"]
        points = await json_result(session, "arrangement_points")
        return listing, {key: scene[key] for key in garment_keys}, points

    async with open_backend("official") as clo, open_backend("rust") as rust:
        original = await json_result(clo, "scene_info")
        try:
            for session in (clo, rust):
                await inspect_contract(session)
                await json_result(session, "scene_open", {"path": fixture})
                assert (await json_result(session, "scene_info"))["pattern_count"] > 0
            native_baseline, rust_baseline = await read(clo), await read(rust)
            libraries = [baseline[0]["library"] for baseline in (native_baseline, rust_baseline)]
            assert libraries[0] == libraries[1] and len(libraries[0]) >= 2
            assert str(root / "Female/FV2.1_Mia.avt") in libraries[0]
            assert str(root / "Male/MV2.1_Luka.avt") in libraries[0]
            results = []
            for label, args, name in cases:
                expected = {"ok": True, "avatars": [name]}
                assert await json_result(clo, "avatar_load", args) == expected
                assert await json_result(rust, "avatar_load", args) == expected
                native, independent = await read(clo), await read(rust)
                assert native[0] == independent[0]
                assert native[2] == independent[2] and native[2]["count"] == 110
                assert native[1] == native_baseline[1] and independent[1] == rust_baseline[1]
                results.append({"case": label, "in_scene": [name], "ordered_arrangement_points": native[2]["count"]})
            for mode in ("append", "replace"):
                args = {"path": str(root / "Male/MV2.1_Luka.avt"), "mode": mode}
                for session, baseline in ((clo, native_baseline), (rust, rust_baseline)):
                    await json_result(session, "avatar_load")
                    opened = await json_result(session, "scene_open", args)
                    assert opened["ok"] is True and opened["avatars"] == ["MV2_Thomas_1_eye_Joint"]
                    _, garment, points = await read(session)
                    assert garment == baseline[1] and points["count"] == 110

            for session in (clo, rust):
                before = await read(session)
                missing = await session.call_tool("avatar_load", {"name_or_path": "absent-avatar-for-parity-404"})
                assert missing.isError and await read(session) == before
                invalid = await session.call_tool("avatar_load", {"name_or_path": []})
                assert invalid.isError and await read(session) == before
                await json_result(session, "scene_clear", {"keep_avatar": False})
                listing, _, points = await read(session)
                assert listing["in_scene"] == [] and points == {"count": 0, "names": []}
        finally:
            if original["project_path"]:
                await json_result(clo, "scene_open", {"path": original["project_path"]})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    print(json.dumps({"tools": ["avatar_load", "avatar_list", "arrangement_points"],
                      "native_assets": len(libraries[0]), "cases": results, "success_shape": "exact",
                      "missing_and_invalid_preserved_state": True, "garment_preserved": True,
                      "native_scene_open_modes": ["append", "replace"], "empty_scene": True, "result": "pass"}))


async def compare_live_avatar_measurements():
    state = Path(os.environ["CLO_MCP_STATE"])
    cache_dir = state / "measurements"
    assert state.is_dir() and not any(cache_dir.glob("*.json")), state
    aliases = ("mia", "luka", "female mannequin", "male mannequin", "melody", "oliver")
    cases = []
    async with open_backend("official") as clo, open_backend("rust") as rust:
        original = await json_result(clo, "scene_info")
        assert (await json_result(clo, "clo_status"))["bridge"] == "up"
        try:
            for session in (clo, rust):
                await inspect_contract(session)
                await json_result(session, "scene_clear", {"keep_avatar": False})
                empty = await session.call_tool("avatar_measurements", {})
                assert empty.isError
            for alias in aliases:
                args = {"name_or_path": alias}
                assert await json_result(clo, "avatar_load", args) == await json_result(rust, "avatar_load", args)
                native_scene = await json_result(clo, "scene_info")
                rust_scene = await json_result(rust, "scene_info")
                normal = await json_result(clo, "avatar_measurements")
                assert normal["body"]["source"] == "obj", normal["source"]
                assert len(normal["body"]) == 33
                assert await json_result(rust, "avatar_measurements") == normal
                for session, scene in ((clo, native_scene), (rust, rust_scene)):
                    refreshed = await json_result(session, "avatar_measurements", {"refresh": True})
                    assert refreshed == normal, (alias, refreshed, normal)
                    invalid = await session.call_tool("avatar_measurements", {"refresh": []})
                    assert invalid.isError and await json_result(session, "scene_info") == scene
                cases.append({"alias": alias, "scene_name": normal["avatar"], "numeric_fields": 32})
            assert not any(cache_dir.glob("*.json")), "mesh fallback should not populate the CLO measurement cache"

            await json_result(clo, "avatar_load")
            await json_result(rust, "avatar_load")
            mesh = await json_result(clo, "avatar_measurements")
            name = mesh["avatar"]
            cache = cache_dir / f"{name}.json"
            values = {
                "CIRCUMFERENCE_Bust": {"value": 864.125, "unit": "mm"},
                "CIRCUMFERENCE_Waist": {"value": 657.5, "unit": "mm"},
                "HEIGHT_Total": 1732.25,
            }
            payload = json.dumps({"avatar": name, "measurements": values})
            cache_dir.mkdir(exist_ok=True)
            with cache.open("x", encoding="utf-8") as handle:
                handle.write(payload)
            try:
                cached = await json_result(clo, "avatar_measurements")
                assert cached["source"] == f"cached CLO measurements ({cache.name})"
                assert cached["body"]["source"] == "clo"
                assert (cached["body"]["height"], cached["body"]["bust"], cached["body"]["waist"]) == (
                    1732.25, 864.125, 657.5)
                for session in (clo, rust):
                    assert await json_result(session, "avatar_measurements") == cached
                    assert await json_result(session, "avatar_measurements", {"refresh": True}) == mesh
                    assert await json_result(session, "avatar_measurements") == cached
                assert cache.read_text(encoding="utf-8") == payload
            finally:
                cache.unlink()
        finally:
            if original["project_path"]:
                await json_result(clo, "scene_open", {"path": original["project_path"]})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    print(json.dumps({"tool": "avatar_measurements", "cases": cases, "mesh_responses": "exact",
                      "seeded_cache_read_refresh_read": "exact", "cache_bytes_unchanged": True,
                      "missing_avatar_and_invalid_refresh": "pass", "result": "pass"}))


async def compare_live_avatar_editor_cache():
    directory = Path(os.environ["CLO_MCP_STATE"]) / "measurements"
    assert not any(directory.glob("*.json")), directory
    async with open_backend("official") as clo, open_backend("rust") as rust:
        original = await json_result(clo, "scene_info")
        try:
            assert (await json_result(clo, "clo_status"))["bridge"] == "up"
            assert original["avatars"] == ["Mara_09_eye_Joint"], "load Mia and open Avatar Editor in CLO first"
            for session in (clo, rust):
                await inspect_contract(session)
            await json_result(rust, "avatar_load")
            exact = await json_result(clo, "avatar_measurements", {"refresh": True})
            assert exact["source"] == "CLO avatar measurements", exact["source"]
            cache = directory / f'{exact["avatar"]}.json'
            payload = cache.read_bytes()
            records = json.loads(payload)["measurements"]
            assert len(records) >= 32 and all(row["unit"] == "mm" for row in records.values())
            cached = await json_result(clo, "avatar_measurements")
            assert cached["source"] == f"cached CLO measurements ({cache.name})"
            assert exact["body"] == cached["body"] and len(exact["body"]) == 33
            assert await json_result(rust, "avatar_measurements") == cached
            assert cache.read_bytes() == payload
        finally:
            if original["project_path"]:
                await json_result(clo, "scene_open", {"path": original["project_path"]})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    print(json.dumps({"tool": "avatar_measurements", "avatar": exact["avatar"],
                      "native_cache_records": len(records), "matched_numeric_fields": 32,
                      "cache_sha256": hashlib.sha256(payload).hexdigest(), "result": "pass"}))


async def verify_avatar_changes_keep_drape():
    with TemporaryDirectory(prefix="clo-avatar-drape-") as directory:
        root = Path(directory)
        collider = root / "contact.obj"
        # A closed shell surrounding the settled test panel. The solver
        # should separate its overlap after a positive step only.
        collider.write_text(
            "v -1000 0 -1000\nv 1000 0 -1000\nv 1000 2000 -1000\nv -1000 2000 -1000\n"
            "v -1000 0 1000\nv 1000 0 1000\nv 1000 2000 1000\nv -1000 2000 1000\n"
            "f 1 4 3 2\nf 5 6 7 8\nf 1 2 6 5\nf 4 8 7 3\nf 1 5 8 4\nf 2 3 7 6\n"
        )
        async with open_backend("rust", {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": ""}) as rust:
            await inspect_contract(rust)

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]

            await json_result(rust, "avatar_load", {"name_or_path": "mia"})
            await json_result(rust, "pattern_create", {"name": "Panel", "arrangement": "Front",
                                                      "points": [[0, 0], [120, 0], [50, 90]]})
            await json_result(rust, "fabric_apply", {"fabric": "cotton", "color": "navy"})
            control = await json_result(rust, "simulate", {"steps": 3, "particle_distance": 50})
            assert control["mesh_initial_overlap_corrections"] == 0
            _, before = await save("before")
            loaded = await json_result(rust, "avatar_load", {"name_or_path": str(collider), "replace": False})
            assert len(loaded["avatars"]) == 2 and loaded["avatars"][-1] == "contact"
            path, appended = await save("appended")
            assert appended["simulation"] == before["simulation"]
            invalid_delete = await rust.call_tool("clo_api_call", {
                "name": "utility_api.DeleteAvatar", "args": [[0, 99]],
            })
            assert invalid_delete.isError and (await json_result(rust, "avatar_list"))["in_scene"] == loaded["avatars"]
            deleted = await json_result(rust, "clo_run_python", {"code": "utility_api.DeleteAvatar([0, 0])"})
            assert deleted["value"] is True
            assert (await json_result(rust, "avatar_list"))["in_scene"] == ["contact"]
            _, unbound = await save("primary-deleted")
            assert unbound["simulation"] == before["simulation"]
            await json_result(rust, "scene_open", {"path": str(path)})
            await json_result(rust, "simulate", {"steps": 0})
            _, zero = await save("zero")
            assert zero["simulation"] == before["simulation"]
            resumed = await json_result(rust, "simulate", {"steps": 1})
            assert resumed["mesh_initial_overlap_corrections"] > 0
            _, continued = await save("continued")
            assert continued["simulation"]["steps"] == 4

            replaced = await json_result(rust, "avatar_load", {"name_or_path": "luka", "replace": True})
            assert replaced["avatars"] == ["luka"]
            _, replaced_scene = await save("replaced")
            assert replaced_scene["simulation"] == continued["simulation"]
            invalid = await rust.call_tool("avatar_load", {"name_or_path": str(root / "missing.obj")})
            assert invalid.isError
            assert (await json_result(rust, "avatar_list"))["in_scene"] == ["luka"]
            _, rejected = await save("rejected")
            assert rejected["simulation"] == continued["simulation"]
            native_append = None
            if fixture := os.environ.get("CLO_AVATAR_FIXTURE"):
                opened = await json_result(rust, "scene_open", {"path": fixture, "mode": "append"})
                name, = opened["avatars"]
                assert opened["rest_geometry"]["triangles"] > 0
                default_open = await json_result(rust, "scene_open", {"path": fixture})
                assert default_open["avatars"] == [name]
                assert default_open["rest_geometry"] == opened["rest_geometry"]
                function = "import_api.ImportAvatar"
                description = await json_result(rust, "clo_api_describe", {"name": function})
                assert description["doc"] == "ImportAvatar(arg0: str, arg1: Marvelous::ImportExportOption) -> bool"
                result = await json_result(rust, "clo_api_call", {"name": function, "args": [
                    fixture, {"$type": "ApiTypes.ImportExportOption"},
                ]})
                assert result["value"] is True
                code = f"import_api.ImportAvatar({fixture!r}, ApiTypes.ImportExportOption())"
                assert (await json_result(rust, "clo_run_python", {"code": code}))["value"] is True
                assert (await json_result(rust, "avatar_list"))["in_scene"] == [name]
                _, with_native = await save("native")
                assert with_native["simulation"] == continued["simulation"]
                assert with_native["patterns"] == continued["patterns"]
                assert with_native["fabrics"] == continued["fabrics"]
                invalid = await rust.call_tool("clo_api_call", {
                    "name": function, "args": [str(root / "missing.avt"), {"$type": "ApiTypes.ImportExportOption"}],
                })
                assert invalid.isError
                assert (await json_result(rust, "avatar_list"))["in_scene"] == [name]
                native_append = {"scene_open_modes_replace_avatars": ["append", "default"],
                                 "default_typed_and_python_import_replaced_avatars": True,
                                 "missing_import_preserved_avatars": True}
        alias_directory = None
        if library := os.environ.get("CLO_AVATAR_LIBRARY_FIXTURE"):
            relative = Path("Female/FV2.1_Mia.avt")
            configured = root.resolve() / "library"
            original = configured / relative
            original.parent.mkdir(parents=True)
            shutil.copyfile(Path(library) / relative, original)
            shadow = configured / "A-backup" / relative.name
            shadow.parent.mkdir()
            shadow.write_text("not a native avatar")
            async with open_backend("rust", {"CLO_RUST_AVATAR_DIR": str(configured)}) as rust:
                assert (await json_result(rust, "avatar_list"))["library"] == [str(shadow), str(original)]
                loaded = await json_result(rust, "avatar_load", {"name_or_path": " FeMaLe "})
                direct = await json_result(rust, "avatar_load", {"name_or_path": str(original)})
                assert loaded == direct
                portable = root / "resolved-avatar.clorust"
                await json_result(rust, "scene_save", {"path": str(portable)})
                assert json.loads(portable.read_text())["scene"]["avatar"]["path"] == str(original)
                alias_directory = relative.as_posix()
    print(json.dumps({"tools": ["avatar_load", "avatar_list", "simulate", "scene_open", "clo_api_call", "clo_run_python"],
                      "case": "change avatars while retaining cloth",
                      "append_and_replace_preserved_drape": True, "continued_step": 4,
                      "api_delete_preserved_drape": True, "canonical_alias_path": alias_directory,
                      "new_mesh_overlap_corrections": resumed["mesh_initial_overlap_corrections"],
                      "native_avt_append": native_append}))


async def verify_geometry_edits_rebuild_one_panel():
    with TemporaryDirectory(prefix="clo-rebuild-edited-panel-") as directory:
        root = Path(directory)
        async with open_backend("rust", {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": ""}) as rust:
            await inspect_contract(rust)

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]

            await json_result(rust, "avatar_load", {"name_or_path": "mia"})
            for name, x in (("Front", 0), ("Back", 180)):
                await json_result(rust, "pattern_create", {
                    "name": name, "points": [[x, 0], [x + 100, 0], [x + 55, 90], [x, 60]],
                    "arrangement": name,
                })
            await json_result(rust, "simulate", {"steps": 3, "particle_distance": 50})
            initial, original = await save("initial")
            first, remaining = original["simulation"]["meshes"]
            no_op = await json_result(rust, "pattern_edit_points", {
                "pattern": "Front", "moves": [{"point": "0", "x": "ignored", "dy": "0"}],
            })
            assert no_op == {"index": 0, "moved": [{"point": 0, "x": 0.0, "y": 0.0}]}
            _, unchanged = await save("unchanged")
            assert unchanged["patterns"] == original["patterns"]
            assert unchanged["simulation"] == original["simulation"]
            observed = await json_result(rust, "clo_ui_state")
            invalid = await rust.call_tool("pattern_edit_points", {
                "pattern": "Front", "moves": [{"point": 0, "dx": 15}, {"point": 1, "dy": "invalid"}],
            })
            assert invalid.isError
            stale = await rust.call_tool("clo_ui_action", {
                "snapshot": observed["snapshot"], "action": "press", "element": 2,
            })
            assert stale.isError and any("Stale UI snapshot" in block.text for block in stale.content)
            partial_path, partial = await save("partial")
            assert partial["patterns"][1] == original["patterns"][1]
            assert partial["simulation"]["meshes"] == [remaining]
            await json_result(rust, "scene_open", {"path": str(partial_path)})
            info = await json_result(rust, "pattern_info", {"pattern": "Front"})
            points = {row["Point index"]: row for row in info["input_info"]["Pattern InputInformation"][0]["PointList"]
                      if "Point index" in row}
            assert (points["0"]["Point positionX"], points["1"]["Point positionY"]) == (15, 0)
            results = []
            for tool, args in (
                ("pattern_edit_points", {"moves": [{"point": " 1 ", "x": "ignored", "dx": "40"},
                                                   {"point": 2.9, "x": "56.5", "y": "90"},
                                                   {"point": 1, "dx": 50}]}),
                ("pattern_set_arrangement", {"point": "Back"}),
                ("pattern_ops", {"op": "flip", "args": {"horizontal": True}}),
                ("pattern_ops", {"op": "delete_point", "args": {"point": 1}}),
                ("pattern_ops", {"op": "particle_distance", "args": {"mm": 24}}),
            ):
                await json_result(rust, "scene_open", {"path": str(initial)})
                if tool == "pattern_edit_points":
                    observed = await json_result(rust, "clo_ui_state")
                    front = next(node for node in observed["nodes"]
                                 if node["AXRole"] == "AXRow" and node["AXTitle"] == "Front")
                reply = await json_result(rust, tool, {"pattern": "Front"} | args)
                if tool == "pattern_edit_points":
                    stale = await rust.call_tool("clo_ui_action", {
                        "snapshot": observed["snapshot"], "action": "press", "element": front["id"],
                    })
                    assert stale.isError and any("Stale UI snapshot" in block.text for block in stale.content)
                    assert reply["moved"] == [{"point": 1, "x": 140, "y": 0},
                                              {"point": 2, "x": 56.5, "y": 90},
                                              {"point": 1, "x": 150, "y": 0}]
                    raw = await json_result(rust, "clo_api_call", {
                        "name": "pattern_api.MovePatternPoint", "args": [0, 3, 0, 61],
                    })
                    assert raw["value"] is None
                    info = await json_result(rust, "pattern_info", {"pattern": "Front"})
                    record = info["input_info"]["Pattern InputInformation"][0]["PointList"][4]
                    assert record["Point index"] == "3" and record["Point positionY"] == 61
                path, edited = await save("edited")
                partial = edited["simulation"]
                assert partial["source"] == "mixed_saved_pose" and partial["meshes"] == [remaining]
                await json_result(rust, "scene_open", {"path": str(path)})
                await json_result(rust, "simulate", {"steps": 0})
                _, initialized = await save("initialized")
                simulation = initialized["simulation"]
                new, kept = simulation["meshes"]
                assert simulation["steps"] == 3 and kept == remaining
                assert new["id"] == first["id"] and new["positions"] != first["positions"]
                results.append(args.get("op", tool))
            await json_result(rust, "simulate", {"steps": 1})
            _, continued = await save("continued")
            assert continued["simulation"]["steps"] == 4
            native_count = None
            if fixture := os.environ.get("CLO_DISPLAY_FIXTURE"):
                await json_result(rust, "scene_open", {"path": fixture})
                _, native = await save("native-before")
                panel_id = native["patterns"][0]["id"]
                saved_meshes = native["simulation"]["meshes"]
                expected = [mesh | {"texcoords_units": "mm"} for mesh in saved_meshes if mesh["id"] != panel_id]
                assert 0 < len(expected) < len(saved_meshes)
                await json_result(rust, "pattern_edit_points", {
                    "pattern": "0", "moves": [{"point": 0, "dx": 1}],
                })
                path, _ = await save("native-edited")
                await json_result(rust, "scene_open", {"path": str(path)})
                _, edited = await save("native-reopened")
                assert edited["simulation"]["meshes"] == expected
                native_count = len(expected)
    print(json.dumps({"tools": ["pattern_edit_points", "pattern_set_arrangement", "pattern_ops", "simulate"],
                      "case": "rebuild only the edited panel", "edits": results,
                      "unaffected_pose_and_velocities": "unchanged", "continued_step": 4,
                      "native_meshes_preserved": native_count, "point_inputs": "pass",
                      "zero_move_preserved_pose": True, "failed_batch_kept_completed_move": True,
                      "repeated_offsets_used_original_point": True, "api_returned_null": True}))


async def verify_symmetry_sewing():
    with TemporaryDirectory(prefix="clo-symmetry-sewing-") as directory:
        root = Path(directory)
        async with open_backend("rust") as rust:
            await inspect_contract(rust)
            for name, x in (("Front", 0), ("Back", 200)):
                await json_result(rust, "pattern_create", {
                    "name": name, "points": [[x, 0], [x + 100, 0], [x + 100, 50], [x, 60]],
                })
            await json_result(rust, "sew", {"pattern_a": "Front", "line_a": 0,
                                            "pattern_b": "Back", "line_b": 0})
            await json_result(rust, "sew", {"pattern_a": "Front", "line_a": 1,
                                            "pattern_b": "Front", "line_b": 3})

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())

            fixture, document = await save("fixture")
            initial = document["scene"]
            a, b = (panel["id"] for panel in initial["patterns"])

            def endpoint(panel, line, depth=1, start=0.0):
                return {"pattern_id": panel, "shape_depth": depth, "shape_index": 0 if depth == 2 else None,
                        "start_line": line, "end_line": line, "start_ratio": start,
                        "end_ratio": 1.0, "direction": True}

            initial["seams"][0]["native_group"] = 1
            initial["patterns"][0]["inner_seam_rest"] = [[[[15, 12], [30, 35]]]]
            initial["native_seam_groups"] = [
                {"id": 1, "name": "Full edge", "restored": True, "active": True,
                 "segments": [[endpoint(a, 0), endpoint(b, 0)]]},
                {"id": 2, "name": "Partial inner", "restored": False, "active": True,
                 "segments": [[endpoint(a, 0, depth=2, start=0.25), endpoint(b, 2, start=0.5)]]},
            ]
            fixture.write_text(json.dumps(document))
            for with_sewing in (False, True):
                await json_result(rust, "scene_open", {"path": str(fixture)})
                await json_result(rust, "pattern_ops", {"pattern": "Front", "op": "symmetry",
                                                       "args": {"with_sewing": with_sewing}})
                path, result = await save(f"symmetry-{with_sewing}")
                scene = result["scene"]
                assert scene["patterns"][:2] == initial["patterns"]
                mirrored = scene["patterns"][2]
                assert mirrored["id"] not in (a, b)
                assert [point[0] for point in mirrored["points"]] == [100, 0, 0, 100]
                assert mirrored["inner_seam_rest"] == [[[[85, 12], [70, 35]]]]
                assert scene["seams"][:2] == initial["seams"]
                assert scene["native_seam_groups"][:2] == initial["native_seam_groups"]
                assert scene["native_seam_groups"] == initial["native_seam_groups"]
                if with_sewing:
                    self_seam, = scene["seams"][2:]
                    assert self_seam["pattern_a"] == self_seam["pattern_b"] == mirrored["id"]
                else:
                    assert scene["seams"] == initial["seams"]
                await json_result(rust, "scene_open", {"path": str(path)})
                assert (await json_result(rust, "seams_list"))["count"] == (4 if with_sewing else 3)
                if with_sewing:
                    simulated = await json_result(rust, "simulate", {"steps": 0, "particle_distance": 50})
                    assert simulated["native_inner_seam_groups"] == 1
    print(json.dumps({"tools": ["pattern_ops", "seams_list", "scene_open", "simulate"],
                      "case": "symmetry with sewing", "with_sewing_false_groups": 3,
                      "with_sewing_true_groups": 4, "solver_inner_seam_groups": 1,
                      "reopen_and_remap": "pass"}))


async def verify_elastic_parameter_updates():
    with TemporaryDirectory(prefix="clo-elastic-settings-") as directory:
        archive = str(Path(directory) / "elastic.clorust")
        async with open_backend("rust") as rust:
            await inspect_contract(rust)
            await json_result(rust, "pattern_create", {
                "name": "Panel", "points": [[0, 0], [120, 0], [120, 100], [0, 100]],
            })

            async def elastic(**args):
                await json_result(rust, "pattern_ops", {"pattern": "Panel", "op": "elastic", "args": args})

            async def api(name, *args):
                reply = await json_result(rust, "clo_api_call", {"name": f"pattern_api.{name}", "args": args})
                assert reply["value"] is None

            async def saved_options():
                await json_result(rust, "scene_save", {"path": archive})
                state = json.loads(Path(archive).read_text())["scene"]
                return {key: value["elastic"] for key, value in state["patterns"][0]["line_properties"].items()}

            await elastic(line="0", on=False)
            await api("SetPatternPieceElasticStrength", 0, 0, 25)
            await api("SetPatternPieceElasticTotalLength", 0, 1, 65)
            initial = await saved_options()
            assert initial == {"0": {"on": False, "strength": 25}, "1": {"total_length": 65}}
            off = await json_result(rust, "simulate", {"steps": 0, "particle_distance": 40})
            assert off["elastic_constraint_count"] == 0

            await elastic(line="1", on=True, strength="30.5")
            await api("SetPatternPieceElasticStrength", 0, -1, 20)
            active = await saved_options()
            assert active == {"-1": {"strength": 20}, "0": {"on": False},
                              "1": {"on": True, "total_length": 65}}
            await json_result(rust, "scene_open", {"path": archive})
            on = await json_result(rust, "simulate", {"steps": 0})
            assert on["elastic_constraint_count"] > 0
            await api("SetPatternPieceElasticTotalLength", 0, 0, 44)
            assert (await saved_options())["0"] == {"on": False, "total_length": 44}
            result = await json_result(rust, "simulate", {"steps": 0})
            assert result["elastic_constraint_count"] == on["elastic_constraint_count"]
            await elastic(line=0, on=0, strength=0, total_length=0)
            assert (await saved_options())["0"] == {"on": False, "total_length": 44}
            await elastic(line=0, on=False, strength="0")
            await api("SetPatternPieceElasticTotalLength", 0, 0, 0)
            assert (await saved_options())["0"] == {"on": False, "total_length": 0, "strength": 0}
            failure = await rust.call_tool("pattern_ops", {"pattern": "Panel", "op": "elastic",
                "args": {"line": 0, "on": True, "total_length": "55", "strength": "invalid"}})
            assert failure.isError
            staged = await saved_options()
            assert staged["0"] == {"on": True, "total_length": 55, "strength": 0}
            await json_result(rust, "scene_open", {"path": archive})
            assert await saved_options() == staged
    print(json.dumps({"tools": ["pattern_ops", "clo_api_call", "simulate"],
                      "case": "elastic parameters preserve enable flags",
                      "constraints_disabled": off["elastic_constraint_count"],
                      "constraints_enabled_after_reopen": on["elastic_constraint_count"],
                      "disabled_edge_stayed_disabled": True, "public_zero_vs_explicit_zero": "pass",
                      "late_error_kept_enable_and_length": True}))


async def verify_outline_edit_remaps():
    with TemporaryDirectory(prefix="clo-outline-edit-") as directory:
        root = Path(directory)
        async with open_backend("rust") as rust:
            await inspect_contract(rust)
            await json_result(rust, "pattern_create", {
                "name": "Front", "points": [[0, 0], [100, 0], [120, 50], [100, 100], [0, 100]],
            })
            await json_result(rust, "pattern_create", {
                "name": "Back", "points": [[200, 0], [300, 0], [300, 100], [200, 100]],
            })
            moved = await json_result(rust, "pattern_edit_points", {
                "pattern": "Back", "moves": [{"point": 0, "dx": 3}, {"point": 0, "dx": 8}],
            })
            assert [point["x"] for point in moved["moved"]] == [203, 208]
            await json_result(rust, "pattern_ops", {"pattern": "Front", "op": "elastic",
                                                   "args": {"line": -1, "strength": 0.5}})
            await json_result(rust, "pattern_ops", {"pattern": "Front", "op": "elastic",
                                                   "args": {"line": 3, "total_length": 50}})
            await json_result(rust, "pattern_ops", {"pattern": "Front", "op": "shirring",
                                                   "args": {"line": 4}})
            await json_result(rust, "sew", {"pattern_a": "Front", "line_a": 3,
                                            "pattern_b": "Back", "line_b": 0})

            fixture = root / "before.clorust"
            await json_result(rust, "scene_save", {"path": str(fixture), "thumbnail": False})
            document = json.loads(fixture.read_text())
            original = document["scene"]
            a, b = (panel["id"] for panel in original["patterns"])

            def edge(panel, start, end):
                return {"pattern_id": panel, "shape_depth": 1, "start_line": start, "end_line": end,
                        "start_ratio": 0.0, "end_ratio": 1.0, "direction": True}

            original["seams"][0]["native_group"] = 1
            original["native_seam_groups"] = [
                {"id": 1, "name": "Keep", "segments": [[edge(a, 3, 3), edge(b, 0, 0)]],
                 "active": True, "restored": True},
                {"id": 2, "name": "Affected", "segments": [[edge(a, 4, 2), edge(b, 1, 1)]],
                 "active": True, "restored": False},
            ]
            fixture.write_text(json.dumps(document))
            report = []
            for op, args, line, shirring in (
                ("delete_point", {"point": 1}, 2, 3),
                ("unfold", {"line": 4, "half_symmetry": False}, 3, None),
            ):
                await json_result(rust, "scene_open", {"path": str(fixture)})
                await json_result(rust, "pattern_ops", {"pattern": "Front", "op": op, "args": args})
                path = root / f"{op}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                await json_result(rust, "scene_open", {"path": str(path)})
                groups = await json_result(rust, "seams_list")
                group, = groups["groups"]
                assert group["name"] == "Keep"
                edited = json.loads(path.read_text())["scene"]
                native_group, = edited["native_seam_groups"]
                endpoint = native_group["segments"][0][0]
                assert endpoint["start_line"] == endpoint["end_line"] == line
                seam, = edited["seams"]
                assert seam["line_a"] == line and seam["native_group"] == 1
                properties = edited["patterns"][0]["line_properties"]
                assert properties[str(line)]["elastic"]["total_length"] == 50
                assert properties["-1"]["elastic"]["strength"] == 0.5
                shirred = [int(n) for n, p in properties.items() if p.get("shirring", {}).get("on")]
                assert shirred == ([] if shirring is None else [shirring])
                invalid = await json_result(rust, "pattern_ops", {"pattern": "Front", "op": "delete_point",
                                                                  "args": {"point": 99}})
                assert invalid == {"index": 0, "op": "delete_point", "ok": True}
                assert await json_result(rust, "seams_list") == groups
                report.append({"operation": op, "retained_line": line, "native_groups": groups["count"]})
    print(json.dumps({"tools": ["pattern_edit_points", "pattern_ops", "seams_list", "scene_open"],
                      "case": "outline-edit seam remap", "edits": report, "repeated_relative_move": "pass"}))


async def verify_calibrated_build_arrangement():
    with TemporaryDirectory(prefix="clo-build-arrangement-") as directory:
        root = Path(directory)
        cache = root / "calibration"
        cache.mkdir()
        environment = {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": "",
                       "CLO_MCP_STATE": directory, "CLO_RUST_CHECKPOINT_DIR": str(root / "checkpoints")}
        request = {"spec": {"garment": "top", "length": "waist", "particle_distance": 100},
                   "snapshots": False, "simulate_steps": 0}
        async with open_backend("rust", environment) as rust:
            await inspect_contract(rust)
            loaded = await json_result(rust, "avatar_load", {"name_or_path": "mia"})
            avatar, = loaded["avatars"]
            preview = await json_result(rust, "garment_plan_preview", {"spec": request["spec"]})
            front, back = (piece["centre_height"] for piece in preview["pieces"])
            points = {
                "Body_Front_Center_1": [8, front - 20, 155],
                "Body_Front_Center_2": [24, front + 30, 170],
                "Body_Front_Center_3": [0, front + 30, 155],
                "Body_Back_Center_1": None,
                "Body_Back_Center_2": {"x": -12, "y": back + 12, "z": -162},
                "Body_Back_Center_3": {"y": "invalid"},
            }
            cache_path = cache / f"{avatar}.json"
            cache_path.write_text(json.dumps({"avatar": "other avatar", "points": points}))
            fallback = await json_result(rust, "garment_build", request)
            assert fallback["arrangement"] == []
            assert fallback["pieces"]["Front"]["arrangement"] == "Body_Front_Center_3"
            cache_path.write_text(json.dumps({"avatar": avatar, "points": points}))
            result = await json_result(rust, "garment_build", request)
            assert result["arrangement"] == [
                f"Front: centre {front:.0f} -> Body_Front_Center_2 ({front + 30:.0f})",
                f"Back: centre {back:.0f} -> Body_Back_Center_2 ({back + 12:.0f})",
            ]
            assert await json_result(rust, "garment_plan_preview", {"spec": request["spec"]}) == preview

            path = root / "arranged.clorust"
            await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
            cache_path.unlink()
            await json_result(rust, "scene_open", {"path": str(path)})
            for side in ("Front", "Back"):
                panel = await json_result(rust, "pattern_info", {"pattern": side})
                assert panel["arrangement"]["ArrangementName"] == f"Body_{side}_Center_2"
            await json_result(rust, "simulate", {"steps": 0})
            await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
            scene = json.loads(path.read_text())["scene"]
            expected = {"Front": [24, front + 30, 170], "Back": [-12, back + 12, -162]}
            for mesh in scene["simulation"]["meshes"]:
                coordinates = zip(*mesh["positions"])
                center = [(min(axis) + max(axis)) / 2 for axis in coordinates]
                assert all(abs(a - b) < 1e-6 for a, b in zip(center, expected[mesh["name"]], strict=True))
            await json_result(rust, "pattern_set_arrangement", {"pattern": "Front", "point": "Body_Front_Center_3"})
            await json_result(rust, "simulate", {"steps": 0})
            await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
            mesh = json.loads(path.read_text())["scene"]["simulation"]["meshes"][0]
            heights = [p[1] for p in mesh["positions"]]
            assert abs((min(heights) + max(heights)) / 2 - front) < 0.1
    print(json.dumps({"tools": ["garment_build", "pattern_info", "pattern_set_arrangement", "simulate"],
                      "case": "calibrated build arrangement",
                      "arrangements": result["arrangement"], "saved_mesh_centres_mm": expected,
                      "mismatched_cache_and_manual_override": "pass"}))


async def verify_appended_build_keeps_drape():
    with TemporaryDirectory(prefix="clo-appended-build-drape-") as directory:
        root = Path(directory)
        environment = {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": "",
                       "CLO_RUST_CHECKPOINT_DIR": str(root / "checkpoints")}
        async with open_backend("rust", environment) as rust:
            await inspect_contract(rust)
            await json_result(rust, "avatar_load", {"name_or_path": "mia"})
            await json_result(rust, "pattern_create", {
                "name": "Existing", "points": [[0, 0], [140, 0], [66, 100]], "arrangement": "Front",
            })
            await json_result(rust, "simulate", {"steps": 3, "particle_distance": 40, "quality": 1})

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]

            original_path, original = await save("original")
            old_mesh, = original["simulation"]["meshes"]
            request = {"spec": {"garment": "top", "length": "waist", "particle_distance": 90},
                       "replace_existing": False, "snapshots": False}
            built = await json_result(rust, "garment_build", request | {"simulate_steps": 0})
            assert built["simulation"] is None and built["checkpoint_before_build"] is None
            path, appended = await save("appended")
            expected = old_mesh | {"rest_particle_distance": 40.0}
            assert appended["simulation"]["meshes"] == [expected]
            assert [p["particle_distance"] for p in appended["patterns"]] == [90.0] * 3
            await json_result(rust, "scene_open", {"path": str(path)})
            await json_result(rust, "simulate", {"steps": 0})
            _, initialized = await save("initialized")
            assert len(initialized["simulation"]["meshes"]) == 3
            assert initialized["simulation"]["meshes"][0] == expected
            assert initialized["simulation"]["steps"] == 3
            await json_result(rust, "simulate", {"steps": 1})
            _, manually_advanced = await save("manual")
            await json_result(rust, "scene_open", {"path": str(original_path)})
            built = await json_result(rust, "garment_build", request | {"simulate_steps": 1})
            assert built["simulation"]["steps"] == 1
            _, automatic = await save("automatic")
            assert automatic["simulation"] == manually_advanced["simulation"]
            assert automatic["simulation"]["steps"] == 4
            assert automatic["simulation"]["quality"] == 1
            await json_result(rust, "simulate", {"steps": 0, "particle_distance": 90})
            _, explicitly_remeshed = await save("remeshed")
            assert explicitly_remeshed["simulation"]["steps"] == 4
            assert "rest_particle_distance" not in explicitly_remeshed["simulation"]["meshes"][0]
    print(json.dumps({"tools": ["garment_build", "scene_open", "simulate"],
                      "case": "build beside settled cloth", "requested_distance_mm": 90,
                      "preserved_mesh_distance_mm": 40, "continued_step": 4,
                      "matches_manual_continuation": True, "explicit_remesh": "pass"}))


async def verify_appended_build_report():
    with TemporaryDirectory(prefix="clo-appended-build-") as directory:
        root = Path(directory)
        environment = {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": "",
                       "CLO_RUST_CHECKPOINT_DIR": str(root / "checkpoints")}
        async with open_backend("rust", environment) as rust:
            await inspect_contract(rust)
            for name in ("Front", "Back"):
                await json_result(rust, "pattern_create", {
                    "name": name, "points": [[0, 0], [100, 0], [40, 70]],
                })
            await json_result(rust, "fabric_apply", {"fabric": "cotton", "color": "red"})
            await json_result(rust, "sew", {"pattern_a": "Front", "line_a": 0,
                                           "pattern_b": "Back", "line_b": 0})

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]

            _, before = await save("before")
            spec = {"garment": "top", "length": "cropped", "fabric": "linen",
                    "color": "navy", "particle_distance": 76}
            plan = await json_result(rust, "garment_plan_preview", {"spec": spec})
            request = {"spec": spec, "replace_existing": False, "simulate_steps": 0, "snapshots": False}
            response = await json_result(rust, "garment_build", request)
            assert response["simulation"] is None and response["checkpoint_before_build"] is None
            expected_report = [
                {"a": f"{seam['piece_a']}:{seam['line_a']}",
                 "b": f"{seam['piece_b']}:{seam['line_b']}", "ok": True}
                for seam in plan["seams"]
            ]
            assert response["seams"] == expected_report
            assert [response["pieces"][name]["index"] for name in ("Front", "Back")] == [2, 3]
            path, after = await save("after")
            old, new = after["patterns"][:2], after["patterns"][2:]
            assert [panel["name"] for panel in old] == [panel["name"] for panel in new] == ["Front", "Back"]
            assert [p["fabric_index"] for p in old] == [p["fabric_index"] for p in before["patterns"]]
            assert after["fabrics"][:-1] == before["fabrics"]
            assert all(p["fabric_index"] == after["fabrics"][-1]["index"] for p in new)
            assert after["fabrics"][-1]["color"] == [0.1, 0.16, 0.42]
            assert after["seams"][0] == before["seams"][0]
            ids = {panel["name"]: panel["id"] for panel in new}
            assert len({panel["id"] for panel in after["patterns"]}) == 4
            for stored, drafted in zip(after["seams"][1:], plan["seams"], strict=True):
                for end in ("a", "b"):
                    assert stored[f"pattern_{end}"] == ids[drafted[f"piece_{end}"]]
                    assert stored[f"line_{end}"] == drafted[f"line_{end}"]
                    assert stored[f"dir_{end}"] == drafted[f"dir_{end}"]
            await json_result(rust, "scene_open", {"path": str(path)})
            for index in range(4):
                panel = await json_result(rust, "pattern_info", {"pattern": str(index)})
                assert panel["particle_distance"] == 76
    print(json.dumps({"tools": ["garment_build", "pattern_info", "scene_save"],
                      "case": "append beside same-name patterns",
                      "piece_indices": [2, 3], "sewing": response["seams"],
                      "fabric_and_existing_ids": "pass", "zero_step_particle_distance": 76}))


async def verify_build_checkpoints():
    with TemporaryDirectory(prefix="clo-build-checkpoint-") as directory:
        root = Path(directory)
        checkpoint_dir = root / "checkpoints"
        environment = {
            "CLO_RUST_FABRIC_DIR": "", "CLO_RUST_AVATAR_DIR": "",
            "CLO_RUST_CHECKPOINT_DIR": str(checkpoint_dir),
        }
        options = {"simulate_steps": 0, "snapshots": False}
        request = options | {"spec": {"garment": "top", "length": "waist"}}
        async with open_backend("rust", environment) as rust:
            await inspect_contract(rust)

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return json.loads(path.read_text())["scene"]

            async def check_checkpoint(result, before):
                path = Path(result["checkpoint_before_build"])
                assert path.parent == checkpoint_dir
                assert re.fullmatch(r"auto_before_build_\d{8}_\d{6}(?:_\d+)?\.clorust", path.name)
                saved = json.loads(path.read_text())["scene"]
                for field in ("patterns", "seams", "native_seam_groups", "avatar", "fabrics", "simulation"):
                    assert saved.get(field) == before.get(field), field
                listed = (await json_result(rust, "checkpoint_list"))["checkpoints"]
                assert any(row["name"] == path.stem and row["path"] == str(path) for row in listed)
                return path

            empty = await json_result(rust, "garment_build", request)
            assert empty["checkpoint_before_build"] is None
            await json_result(rust, "scene_clear", {"keep_avatar": False})
            await json_result(rust, "pattern_create", {
                "name": "Existing", "points": [[0, 0], [100, 0], [40, 70]],
            })
            appended = await json_result(rust, "garment_build", request | {"replace_existing": False})
            assert appended["checkpoint_before_build"] is None
            assert (await json_result(rust, "scene_info"))["pattern_count"] == 3
            assert (await json_result(rust, "checkpoint_list"))["checkpoints"] == []
            await json_result(rust, "simulate", {"steps": 1, "particle_distance": 100})
            before = await save("before-replace")
            replaced = await json_result(rust, "garment_build", request)
            original = await check_checkpoint(replaced, before)
            before_rebuild = await save("before-rebuild")
            rebuilt = await json_result(rust, "garment_rebuild", options | {"changes": {"length": "cropped"}})
            newer = await check_checkpoint(rebuilt, before_rebuild)
            assert newer != original
            restored = await json_result(rust, "checkpoint_restore", {"name": original.stem})
            assert restored["pattern_count"] == 3
            restored_scene = await save("restored")
            assert restored_scene["simulation"] == before["simulation"]
            assert (await json_result(rust, "garment_last_spec"))["spec"] == rebuilt["spec"]

            native_count = None
            if fixture := os.environ.get("CLO_CHECKPOINT_FIXTURE"):
                await json_result(rust, "scene_open", {"path": fixture})
                native = await save("before-native-replace")
                native_count = len(native["native_seam_groups"])
                assert native_count > 0 and native["simulation"]["meshes"]
                result = await json_result(rust, "garment_build", request)
                path = await check_checkpoint(result, native)
                replaced_native = await save("replaced-native")
                assert replaced_native["native_seam_groups"] == []
                assert len(replaced_native["patterns"]) == len(result["pieces"])
                await json_result(rust, "checkpoint_restore", {"name": path.stem})
                restored = await save("restored-native")
                assert restored["native_seam_groups"] == native["native_seam_groups"]
                assert restored["simulation"] == native["simulation"]
                copy = root / "restored-native.zprj"
                await json_result(rust, "scene_save", {"path": str(copy), "thumbnail": False})
                opened = await json_result(rust, "scene_open", {"path": str(copy)})
                assert opened["pattern_count"] == len(native["patterns"])

            checkpoint_dir.rename(root / "previous-checkpoints")
            checkpoint_dir.write_text("Block creation of the checkpoint directory")
            result = await json_result(rust, "garment_build", request)
            assert result["checkpoint_before_build"] is None
            assert (await json_result(rust, "scene_info"))["pattern_count"] == len(result["pieces"])

    print(json.dumps({"tools": ["garment_build", "garment_rebuild", "checkpoint_list",
                                "checkpoint_restore"],
                      "case": "automatic checkpoints before build", "empty_and_append": "pass",
                      "replace_and_rebuild": "pass", "write_failure_best_effort": "pass",
                      "restored_native_seam_groups": native_count}))


async def compare_checkpoint_listing():
    with TemporaryDirectory() as scratch:
        root = Path(scratch)
        listings = {}
        for backend, settings, folder, ext in (
            ("official", {"CLO_MCP_STATE": scratch}, root / "checkpoints", "zprj"),
            ("rust", {"CLO_RUST_CHECKPOINT_DIR": str(root / "rust")}, root / "rust", "clorust"),
        ):
            folder.mkdir()
            names = ("first", "second")
            for index, name in enumerate(names):
                path = folder / f"{name}.{ext}"
                path.touch()
                timestamp = 1_764_900_000 + 3600 * index
                os.utime(path, (timestamp, timestamp))
            async with open_backend(backend, settings) as session:
                rows = (await json_result(session, "checkpoint_list"))["checkpoints"]
            assert [row["name"] for row in rows] == list(reversed(names))
            assert all(Path(row["path"]).parent == folder for row in rows)
            listings[backend] = [(row["name"], row["mtime"]) for row in rows]
        assert listings["official"] == listings["rust"], listings
        shared = root / "checkpoints"
        stamp = 1_764_900_000_000_000_000
        tied = ("zeta", "alpha", "middle", "测试", "Zebra", "Apple", "b", "a")
        for offset, name in enumerate(tied):
            path = shared / f"{name}.zprj"
            path.touch()
            os.utime(path, ns=(stamp, stamp + offset))
        linked = os.name == "posix"
        if linked:
            # The reference lists links to accessible checkpoint files using the target's mtime.
            target = root / "archived.zprj"
            target.touch()
            os.utime(target, ns=(stamp, stamp))
            (shared / "from-archive.zprj").symlink_to(target)
        async with open_backend("official", {"CLO_MCP_STATE": scratch}) as official:
            reference = await json_result(official, "checkpoint_list")
        async with open_backend("rust", {"CLO_RUST_CHECKPOINT_DIR": str(shared)}) as rust:
            observed = await json_result(rust, "checkpoint_list")
        assert observed == reference, (observed, reference)
        tie_order = [row["name"] for row in observed["checkpoints"] if row["name"] in tied]
        assert set(tie_order) == set(tied)
        assert any(row["name"] == "from-archive" for row in observed["checkpoints"]) == linked
    print(json.dumps({"tool": "checkpoint_list", "same_order_and_local_timestamps": listings["rust"],
                      "shared_tie_order": tie_order, "linked_checkpoint_listed": linked}, ensure_ascii=False))


async def compare_live_checkpoint_roundtrip():
    fixture = Path(os.environ["CLO_CHECKPOINT_FIXTURE"])
    directory = Path(os.environ["CLO_MCP_STATE"]) / "checkpoints"
    assert "CLO_RUST_CHECKPOINT_DIR" not in os.environ
    assert fixture.is_file() and fixture.suffix == ".zprj", fixture
    name = "检查点 É"
    state_keys = ("project", "project_path", "pattern_count", "patterns", "seam_groups",
                  "avatars", "fabric_count", "fabrics", "colorways", "current_colorway")
    digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
    source_hash = digest(fixture)

    async def scene(session):
        state = await json_result(session, "scene_info")
        return {key: state[key] for key in state_keys}

    async def listing(session):
        return (await json_result(session, "checkpoint_list"))["checkpoints"]

    async def restore(session, checkpoint, expected):
        await json_result(session, "scene_clear", {"keep_avatar": False})
        result = await json_result(session, "checkpoint_restore", {"name": checkpoint["name"]})
        assert result == {"ok": True, "path": checkpoint["path"],
                          "pattern_count": expected["pattern_count"], "avatars": expected["avatars"]}
        assert await scene(session) == expected

    async with open_backend("official") as clo, open_backend("rust") as rust:
        original = await scene(clo)
        try:
            for session in (clo, rust):
                await inspect_contract(session)
            assert await listing(clo) == await listing(rust) == []
            await json_result(clo, "scene_open", {"path": str(fixture)})
            baseline = await scene(clo)
            assert baseline["pattern_count"] == 2 and baseline["seam_groups"] == 1
            saved = await json_result(clo, "checkpoint_save", {"name": name})
            target = directory / f"{name}.zprj"
            assert saved == {"name": name, "path": str(target)}
            initial = baseline | {"project": name, "project_path": str(target)}
            assert await scene(clo) == initial
            before = digest(target)
            await restore(rust, saved, initial)

            await json_result(rust, "pattern_ops", {
                "pattern": "Front", "op": "rename", "args": {"name": "Rust Revision"},
            })
            expected_rust = await scene(rust)
            assert await json_result(rust, "checkpoint_save", {"name": name}) == saved
            assert await scene(rust) == expected_rust
            assert not target.with_suffix(".clorust").exists()
            assert digest(target) != before
            await restore(clo, saved, expected_rust)
            assert expected_rust["patterns"][0]["name"] == "Rust Revision"

            await json_result(clo, "pattern_ops", {
                "pattern": "Rust Revision", "op": "rename", "args": {"name": "CLO Revision"},
            })
            assert await json_result(clo, "checkpoint_save", {"name": name}) == saved
            expected_clo = await scene(clo)
            await restore(rust, saved, expected_clo)
            assert expected_clo["patterns"][0]["name"] == "CLO Revision"
            native_auto = await json_result(clo, "checkpoint_save")
            rust_auto = await json_result(rust, "checkpoint_save", {"name": ""})
            assert len({saved["path"], native_auto["path"], rust_auto["path"]}) == 3
            for item in (native_auto, rust_auto):
                assert re.fullmatch(r"cp_\d{8}_\d{6}(?:_\d+)?", item["name"])
                assert Path(item["path"]) == directory / (item["name"] + ".zprj")
            expected_auto = lambda item: expected_clo | {"project": item["name"], "project_path": item["path"]}
            assert await scene(clo) == expected_auto(native_auto)
            assert await scene(rust) == expected_auto(rust_auto)
            await restore(rust, native_auto, expected_auto(native_auto))
            await restore(clo, rust_auto, expected_auto(rust_auto))

            official_rows, rust_rows = await listing(clo), await listing(rust)
            assert official_rows == rust_rows and len(rust_rows) == 3
            assert [row["path"] for row in rust_rows] == [rust_auto["path"], native_auto["path"], saved["path"]]
            for session in (clo, rust):
                before_error = await scene(session)
                missing = await session.call_tool("checkpoint_restore", {"name": "not-present"})
                assert missing.isError and await scene(session) == before_error
            assert digest(fixture) == source_hash
        finally:
            if original["project_path"] != "NULL":
                await json_result(clo, "scene_open", {"path": original["project_path"]})
    with TemporaryDirectory(prefix="clo-checkpoint-readback-") as temporary:
        portable = Path(temporary) / "geometry.clorust"
        async with open_backend("rust") as reader:
            async def archived_meshes(path):
                await json_result(reader, "scene_open", {"path": str(path)})
                await json_result(reader, "scene_save", {"path": str(portable), "thumbnail": False})
                state = json.loads(portable.read_text())["scene"]
                return [{key: mesh[key] for key in ("positions", "triangles", "texcoords")}
                        for mesh in state["simulation"]["meshes"]]
            geometry = await archived_meshes(fixture)
            assert len(geometry) == 2
            for row in rust_rows:
                assert await archived_meshes(row["path"]) == geometry, row
    print(json.dumps({"fixture": fixture.name, "checkpoints": rust_rows, "cross_backend_restores": 5,
                      "same_file_overwrites": 2, "original_sha256": source_hash,
                      "scene_identity_follows_checkpoint": True, "missing_keeps_scene": True,
                      "mesh_vertices_preserved": sum(len(mesh["positions"]) for mesh in geometry)},
                     ensure_ascii=False))


async def compare_shared_checkpoint_folder():
    fixture = os.environ.get("CLO_CHECKPOINT_FIXTURE")
    assert fixture, "set CLO_CHECKPOINT_FIXTURE to a saved native CLO project"
    assert "CLO_RUST_CHECKPOINT_DIR" not in os.environ, "unset CLO_RUST_CHECKPOINT_DIR for this case"
    with TemporaryDirectory() as scratch:
        settings = {"CLO_MCP_STATE": scratch}
        directory = Path(scratch) / "checkpoints"
        directory.mkdir()
        native = directory / "native-drape.zprj"
        shutil.copyfile(fixture, native)
        with native.open("rb") as source:
            native_digest = hashlib.file_digest(source, "sha256").hexdigest()
        async with open_backend("official", settings) as clo, open_backend("rust", settings) as rust:
            async def listing(session):
                return (await json_result(session, "checkpoint_list"))["checkpoints"]

            async def saved_scene(name):
                path = Path(scratch) / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path)})
                return json.loads(path.read_text())["scene"]

            for session in (clo, rust):
                await inspect_contract(session)
            rows = await listing(clo)
            assert await listing(rust) == rows
            row, = rows
            assert row["name"] == "native-drape" and Path(row["path"]) == native

            direct = await json_result(rust, "scene_open", {"path": str(native)})
            assert direct["pattern_count"] and direct["native_import"]["saved_pose_loaded"]
            scene = await json_result(rust, "scene_info")
            baseline = await saved_scene("direct")
            assert baseline["simulation"]["meshes"]
            await json_result(rust, "scene_clear", {"keep_avatar": False})
            restored = await json_result(rust, "checkpoint_restore", {"name": "native-drape"})
            assert restored == {key: direct[key] for key in ("ok", "path", "pattern_count", "avatars")}
            info = await json_result(rust, "scene_info")
            unequal = sorted(key for key in scene.keys() | info.keys() if scene.get(key) != info.get(key))
            assert not unequal, [(key, str(scene.get(key))[:200], str(info.get(key))[:200]) for key in unequal]
            state = await saved_scene("restored")
            for field in ("patterns", "fabrics", "native_seam_groups", "simulation", "avatar"):
                assert state.get(field) == baseline.get(field), field

            before_publish = await json_result(rust, "scene_info")
            published = await json_result(rust, "checkpoint_save", {"name": "复用检查点"})
            checkpoint = directory / "复用检查点.zprj"
            assert published == {"name": "复用检查点", "path": str(checkpoint)}
            assert await json_result(rust, "scene_info") == before_publish | {
                "project": published["name"], "project_path": published["path"],
            }
            shared_rows = await listing(clo)
            assert await listing(rust) == shared_rows
            assert any(row["path"] == str(checkpoint) for row in shared_rows)
            await json_result(rust, "scene_clear", {"keep_avatar": False})
            readback = await json_result(rust, "checkpoint_restore", {"name": published["name"]})
            assert readback["pattern_count"] == direct["pattern_count"]
            shared_state = await saved_scene("shared-readback")
            for field in ("patterns", "fabrics", "native_seam_groups", "simulation"):
                assert shared_state.get(field) == baseline.get(field), field
            previous_avatar, restored_avatar = baseline["avatar"], shared_state["avatar"]
            if isinstance(previous_avatar, dict) and "source_project" in previous_avatar:
                assert Path(restored_avatar["source_project"]).resolve() == checkpoint.resolve()
                assert restored_avatar == previous_avatar | {"source_project": restored_avatar["source_project"]}
            else:
                assert previous_avatar == restored_avatar

            # Occupy timestamp-only names around the call so the suffix path is exercised.
            occupied = set()
            automatic = []
            for args in ({}, {"name": ""}):
                now = datetime.now()
                for offset in range(-1, 16):
                    stem = (now + timedelta(seconds=offset)).strftime("cp_%Y%m%d_%H%M%S")
                    path = directory / (stem + ".zprj")
                    if not path.exists():
                        path.write_text("existing timestamp checkpoint")
                        occupied.add(path)
                automatic.append(await json_result(rust, "checkpoint_save", args))
            first_auto, second_auto = automatic
            assert first_auto["name"] != second_auto["name"]
            new_paths = {Path(result["path"]) for result in automatic}
            assert new_paths.isdisjoint(occupied)
            assert all(path.suffix == ".zprj" and path.stem.count("_") == 3 for path in new_paths)
            assert all(path.read_text() == "existing timestamp checkpoint" for path in occupied)
            for session in (clo, rust):
                assert new_paths <= {Path(row["path"]) for row in await listing(session)}
            auto_readback = await json_result(rust, "checkpoint_restore", {"name": second_auto["name"]})
            assert auto_readback["pattern_count"] == direct["pattern_count"]

            (directory / "corrupt.zprj").write_text("not a CLO project")
            before_error = await json_result(rust, "scene_info")
            for name in ("missing", "corrupt"):
                assert (await rust.call_tool("checkpoint_restore", {"name": name})).isError
                assert await json_result(rust, "scene_info") == before_error

            await json_result(rust, "scene_clear", {"keep_avatar": False})
            auto_portable = await json_result(rust, "checkpoint_save")
            auto_portable_path = Path(auto_portable["path"])
            assert auto_portable_path.suffix == ".clorust"
            assert not auto_portable_path.with_suffix(".zprj").exists()
            portable = await json_result(rust, "checkpoint_save", {"name": "native-drape"})
            assert Path(portable["path"]) == native.with_suffix(".clorust")
            rust_name, = [row for row in await listing(rust) if row["name"] == "native-drape"]
            clo_name, = [row for row in await listing(clo) if row["name"] == "native-drape"]
            assert rust_name["path"] == portable["path"] and clo_name["path"] == str(native)
            assert (await json_result(rust, "checkpoint_restore", {"name": "native-drape"}))["pattern_count"] == 0
            with native.open("rb") as source:
                assert hashlib.file_digest(source, "sha256").hexdigest() == native_digest
    print(json.dumps({"tools": ["checkpoint_list", "checkpoint_restore", "checkpoint_save"],
                      "case": "shared native checkpoint folder", "same_native_listing": True,
                      "restored_patterns": direct["pattern_count"],
                      "restored_pose_meshes": len(baseline["simulation"]["meshes"]),
                      "corrupt_and_missing_preserve_scene": True, "portable_name_precedence": True,
                      "rust_native_checkpoint_listed_by_reference": True,
                      "auto_native_checkpoints_listed_by_reference": 2,
                      "auto_collision_preserves_existing": True}))


async def verify_reordered_native_archives():
    fixtures = [
        (Path(os.environ["CLO_REORDERED_ZPRJ"]).resolve(strict=True), "zprj", 20, 52),
        (Path(os.environ["CLO_REORDERED_ZPAC"]).resolve(strict=True), "zpac", 25, 36),
    ]
    checked = []
    with TemporaryDirectory(prefix="clo-reordered-native-") as directory:
        for fixture, kind, distance, vertices in fixtures:
            async with open_backend("rust") as session:
                await inspect_contract(session)
                reply = await json_result(session, "scene_open", {"path": str(fixture)})
                assert reply["native_import"]["rest_vertices_validated"] == vertices
                assert reply["native_import"]["saved_pose_meshes"] == 2
                assert reply["native_import"]["full_edge_seams_restored"] == 1
                scene = await json_result(session, "scene_info")
                assert scene["pattern_count"] == 2 and scene["fabric_count"] == 1
                assert scene["colorways"] == ["Original", "Alternate"] and scene["current_colorway"] == 1
                assert [row["name"] for row in scene["patterns"]] == ["Front", "Back"]
                assert all(row["fabric_index"] == 0 for row in scene["patterns"])
                function = "fabric_api.GetFabricPBRMaterialBaseColor"
                expected_colors = [[0.6, 0, 0.2, 1], [36 / 255, 95 / 255, 221 / 255, 1]]
                for index, expected in enumerate(expected_colors):
                    result = await json_result(session, "clo_api_call", {
                        "name": function, "args": [index, 0, 0],
                    })
                    assert all(abs(a - b) < 1e-6 for a, b in zip(result["value"], expected, strict=True))
                active = await json_result(session, "clo_api_call", {"name": function, "args": [0, 0]})
                assert all(abs(a - b) < 1e-6 for a, b in zip(active["value"], expected_colors[1], strict=True))
                invalid = await session.call_tool("clo_api_call", {"name": function, "args": [2, 0, 0]})
                assert invalid.isError and await json_result(session, "scene_info") == scene
                groups = await json_result(session, "seams_list")
                group, = groups["groups"]
                assert group["name"] == "Sewing_8359" and group["patterns"] == ["Front", "Back"]
                for name in ("Front", "Back"):
                    panel = await json_result(session, "pattern_info", {"pattern": name})
                    lines = panel["line_info"]["lines"]
                    assert panel["particle_distance"] == distance and len(lines) == 3
                    assert abs(lines[0]["length"] - 100) < 0.001
                path = Path(directory) / f"{kind}.clorust"
                await json_result(session, "scene_save", {"path": str(path), "thumbnail": False})
                saved = json.loads(path.read_text())["scene"]
                native_group, = saved["native_seam_groups"]
                pair, = native_group["segments"]
                assert [edge["direction"] for edge in pair] == [True, False]
                pose = saved["simulation"]
                assert pose["source"] == "native_saved_pose"
                assert sum(len(mesh["positions"]) for mesh in pose["meshes"]) == vertices
                await json_result(session, "scene_open", {"path": str(path)})
                colors = await json_result(session, "clo_run_python", {
                    "code": f"[list({function}(i, 0, 0)) for i in (0, 1)]",
                })
                for actual, expected in zip(colors["value"], expected_colors, strict=True):
                    assert all(abs(a - b) < 1e-6 for a, b in zip(actual, expected, strict=True))
                result = await json_result(session, "simulate", {"steps": 0})
                assert result["ok"] and result["steps"] == 0
                reopened = path.with_name(f"{kind}-reopened.clorust")
                await json_result(session, "scene_save", {"path": str(reopened), "thumbnail": False})
                checked_meshes = json.loads(reopened.read_text())["scene"]["simulation"]["meshes"]
                for before, after in zip(pose["meshes"], checked_meshes, strict=True):
                    assert all(before[key] == after[key] for key in ("id", "positions", "triangles", "texcoords"))
                    assert after["velocities"] == [[0, 0, 0]] * len(before["positions"])
                checked.append({"format": kind, "panels": 2, "colorways": scene["colorways"],
                                "seam_groups": groups["count"], "pose_vertices": vertices,
                                "particle_distance": distance, "zero_step_pose_exact": True,
                                "colorway_rgba_after_reopen": colors["value"]})
    print(json.dumps({"case": "reordered native PAC fields", "fixtures": checked}, ensure_ascii=False))


async def verify_native_two_corner_archives():
    source = Path(os.environ["CLO_NATIVE_TWO_CORNER_FIXTURES"]).resolve(strict=True)
    native = json.loads((source / "reopen.json").read_text())
    saved_outputs = os.environ.get("CLO_RUST_TWO_CORNER_OUTPUTS")
    cases = {}

    def mesh_samples(path):
        model, binary = read_gltf_export(path)

        def values(index, width):
            accessor = model["accessors"][index]
            view = model["bufferViews"][accessor["bufferView"]]
            component = {5123: "H", 5125: "I", 5126: "f"}[accessor["componentType"]]
            fmt = f"<{width}{component}"
            offset = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
            stride = view.get("byteStride", struct.calcsize(fmt))
            return [struct.unpack_from(fmt, binary, offset + i * stride) for i in range(accessor["count"])]

        return [(values(p["attributes"]["POSITION"], 3), values(p["indices"], 1))
                for mesh in model["meshes"] for p in mesh["primitives"]]

    with TemporaryDirectory(prefix="clo-native-two-corner-") as directory:
        root = Path(directory)
        output = Path(saved_outputs) if saved_outputs else root
        expected_meshes = mesh_samples(source / "two-corner-curve.glb")
        async with open_backend("rust") as session:
            for suffix in ("zprj", "zpac"):
                await json_result(session, "scene_clear", {"keep_avatar": False})
                fixture = source / f"two-corner-curve.{suffix}"
                opened = await json_result(session, "scene_open", {"path": str(fixture), "mode": "replace"})
                assert opened["pattern_count"] == 3 and opened["native_import"]["saved_pose_meshes"] == 3
                state = await json_result(session, "scene_info")
                reference = native[suffix]
                scene_fields = ("project", "patterns", "avatars", "fabrics", "colorways", "current_colorway", "seam_groups")
                for key in scene_fields:
                    assert state[key] == reference["scene"][key], (suffix, key)
                seams = await json_result(session, "seams_list")
                fields = ("index", "name", "patterns")
                assert [{key: group[key] for key in fields} for group in seams["groups"]] == reference["seams"]["groups"]
                curve = await json_result(session, "pattern_info", {"pattern": "Curve Probe"})
                actual = curve["line_info"]["lines"]
                expected = reference["curve"]["line_info"]["lines"]
                assert len(actual) == len(expected) == 2
                for a, b in zip(actual, expected, strict=True):
                    assert all(a[key] == b[key] for key in ("lineIndex", "start", "end"))
                length_errors = [abs(a["length"] - b["length"]) for a, b in zip(actual, expected, strict=True)]
                assert max(length_errors) < 0.01, length_errors

                name = "Curve Readback" if suffix == "zprj" else "曲线 É"
                renamed = await json_result(session, "clo_api_call", {
                    "name": "pattern_api.SetPatternPieceName", "args": [2, name],
                })
                assert renamed["value"] is None
                archive = output / f"two-corner-rust.{suffix}"
                await json_result(session, "scene_save", {"path": str(archive), "thumbnail": False})
                await json_result(session, "scene_open", {"path": str(archive), "mode": "replace"})
                restored = await json_result(session, "pattern_info", {"pattern": name})
                assert restored["line_info"]["lines"] == actual
                assert await json_result(session, "seams_list") == seams
                first = root / "before.clorust"
                second = root / "after.clorust"
                await json_result(session, "scene_save", {"path": str(first), "thumbnail": False})
                result = await json_result(session, "simulate", {"steps": 0})
                assert result["ok"] and result["steps"] == 0
                await json_result(session, "scene_save", {"path": str(second), "thumbnail": False})
                before = json.loads(first.read_text())["scene"]["simulation"]["meshes"]
                after = json.loads(second.read_text())["scene"]["simulation"]["meshes"]
                for old, new in zip(before, after, strict=True):
                    assert all(old[key] == new[key] for key in ("name", "positions", "triangles", "texcoords"))
                glb = root / f"two-corner-{suffix}.glb"
                await json_result(session, "scene_export", {"path": str(glb), "colorways": False})
                assert mesh_samples(glb) == expected_meshes
                cases[suffix] = {"archived_curve_lines": 2, "length_errors_mm": length_errors,
                                 "renamed_curve": name,
                                 "saved_pose_unchanged_at_zero_steps": True, "glb_positions_and_indices_exact": True,
                                 "glb_mesh_counts": [(len(positions), len(indices) // 3)
                                                     for positions, indices in expected_meshes]}
    print(json.dumps({"case": "native two-corner outline archives", "formats": cases}))


async def verify_native_curve_measurements():
    folder = Path(os.environ["CLO_NATIVE_TWO_CORNER_FIXTURES"]).resolve(strict=True)
    reference = json.loads((folder / "curve-measure-native.json").read_text())
    fixture = folder / "curve-measure-reference.zpac"
    reports = {}
    async with open_backend("rust") as session:
        for mode in ("created", "reopened"):
            if mode == "created":
                for name, row in reference.items():
                    await json_result(session, "pattern_create", {"name": name, "points": row["points"]})
            else:
                opened = await json_result(session, "scene_open", {"path": str(fixture), "mode": "replace"})
                assert opened["pattern_count"] == len(reference)
            results = {}
            for name, row in reference.items():
                actual = await json_result(session, "pattern_info", {"pattern": name})
                measured = actual["line_info"]["lines"]
                length_error = 0.0
                for expected, observed in zip(row["edges"], measured, strict=True):
                    assert expected["lineIndex"] == observed["lineIndex"]
                    for end in ("start", "end"):
                        assert all(abs(expected[end][axis] - observed[end][axis]) < 0.001 for axis in ("x", "y"))
                    length_error = max(length_error, abs(observed["length"] - expected["length"]))
                bounds_error = max(abs(float(row["bbox"][key]) - float(actual["bbox"][key]))
                                   for key in ("Width", "Height"))
                assert max(length_error, bounds_error) < 0.01, (mode, name, length_error, bounds_error)
                results[name] = {"length_error_mm": length_error, "bounds_error_mm": bounds_error}
            reports[mode] = results
    print(json.dumps({"case": "native curved outline measurements", "tolerance_mm": 0.01, "modes": reports}))


async def verify_native_colorway_exports():
    fixture = Path(os.environ["CLO_REORDERED_ZPRJ"]).resolve(strict=True)
    expected_names = ("Original", "Alternate")
    reference = os.environ.get("CLO_NATIVE_COLORWAY_EXPORTS")
    with TemporaryDirectory(prefix="clo-colorway-export-") as directory:
        root = Path(directory)
        async with open_backend("rust") as session:
            await inspect_contract(session)
            await json_result(session, "scene_open", {"path": str(fixture)})
            before = await json_result(session, "scene_info")
            name = "fabric_api.GetFabricPBRMaterialBaseColor"
            expected = [(await json_result(session, "clo_api_call", {"name": name, "args": [i, 0, 0]}))["value"]
                        for i in range(len(expected_names))]
            assert expected[0] != expected[1]
            async def read_export(path):
                if path.suffix in (".glb", ".gltf"):
                    model, buf = read_gltf_export(path)
                    primitives = [p for mesh in model["meshes"] for p in mesh["primitives"]]
                    color = model["materials"][primitives[0]["material"]]["pbrMetallicRoughness"]["baseColorFactor"][:3]
                    triangles = sum(model["accessors"][p["indices"]]["count"] // 3 for p in primitives)
                    assert len(buf) >= model["buffers"][0]["byteLength"]
                    return color, triangles
                async with open_backend("rust") as imported:
                    await json_result(imported, "scene_open", {"path": str(path)})
                    save = root / "readback.clorust"
                    await json_result(imported, "scene_save", {"path": str(save), "thumbnail": False})
                    readback = json.loads(save.read_text())["scene"]
                    return readback["fabrics"][0]["color"], sum(len(mesh["triangles"]) for mesh in readback["imported_meshes"])

            report = {}
            for ext in ("obj", "fbx", "glb", "gltf"):
                base = root / ext / f"colored.{ext}"
                outputs = [base.with_stem(f"colored_{name}") for name in expected_names]
                companion_ext = {"obj": "mtl", "gltf": "bin"}.get(ext)
                companions = [p.with_suffix(f".{companion_ext}") for p in outputs] if companion_ext else []
                args = {"path": str(base), "colorways": True}
                result = await json_result(session, "scene_export", args)
                assert result == {"path": str(base), "result": [str(p) for p in outputs + companions]}
                assert not base.exists() and all(p.stat().st_size > 0 for p in outputs + companions)
                report[ext] = [p.name for p in outputs + companions]
                for i, output in enumerate(outputs):
                    color, triangles = await read_export(output)
                    expected_rgb = [v ** 2.2 for v in expected[i][:3]] if ext in ("glb", "gltf") else expected[i][:3]
                    assert all(abs(a - b) < 1e-6 for a, b in zip(color, expected_rgb, strict=True))
                    assert triangles == 68
                    if ext == "gltf":
                        assert json.loads(output.read_text())["buffers"][0]["uri"] == companions[i].name
                    if reference:
                        native = Path(reference) / f"native-{ext}" / output.name
                        native_color, native_triangles = await read_export(native)
                        assert native_triangles == triangles
                        assert all(abs(a - b) < 1e-6 for a, b in zip(color, native_color, strict=True))

                if ext == "glb":
                    assert await json_result(session, "scene_export", args) == result
                    prior = outputs[0].read_bytes()
                    outputs[1].unlink()
                    outputs[1].mkdir()
                    blocked = await session.call_tool("scene_export", args)
                    assert blocked.isError and outputs[0].read_bytes() == prior and outputs[1].is_dir()
                    assert set(base.parent.iterdir()) == set(outputs)
            active = root / "active.glb"
            await json_result(session, "scene_export", {"path": str(active)})
            active_color, _ = await read_export(active)
            assert all(abs(a - b ** 2.2) < 1e-6 for a, b in zip(active_color, expected[1][:3], strict=True))
            assert await json_result(session, "scene_info") == before
    print(json.dumps({"case": "multi-colorway mesh export", "outputs": report, "rgba": expected,
                      "failed_bundle_preserves_existing_files": True, "scene_unchanged": True,
                      "official_mesh_files_compared": bool(reference), "triangles_per_file": 68}))


async def verify_native_colorway_textures():
    from io import BytesIO
    from PIL import Image

    reference = Path(os.environ["CLO_NATIVE_TEXTURE_EXPORTS"]).resolve(strict=True)
    fixture = reference / "multi-colorway-textured.zpac"
    names = ("Original", "Alternate")

    def gltf_texture(path):
        model, binary = read_gltf_export(path)
        primitive = model["meshes"][0]["primitives"][0]
        material = model["materials"][primitive["material"]]["pbrMetallicRoughness"]["baseColorTexture"]
        image = model["images"][model["textures"][material["index"]]["source"]]
        if "uri" in image:
            data = path.with_name(image["uri"]).read_bytes()
        else:
            view = model["bufferViews"][image["bufferView"]]
            start = view.get("byteOffset", 0)
            data = binary[start:start + view["byteLength"]]
        accessor = model["accessors"][primitive["attributes"]["TEXCOORD_0"]]
        view = model["bufferViews"][accessor["bufferView"]]
        start = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
        coordinates = [struct.unpack_from("<2f", binary, start + i * view.get("byteStride", 8))
                       for i in range(accessor["count"])]
        scale = material["extensions"]["KHR_texture_transform"]["scale"]
        return data, [[u * scale[0], v * scale[1]] for u, v in coordinates]

    with TemporaryDirectory(prefix="clo-colorway-textures-") as directory:
        root = Path(directory)

        async def read_texture(path):
            if path.suffix in (".glb", ".gltf"):
                return gltf_texture(path)
            if path.suffix == ".obj":
                material = path.with_suffix(".mtl").read_text().splitlines()
                name = next(line.split(maxsplit=1)[1] for line in material if line.startswith("map_Kd "))
                data = path.with_name(Path(name).name).read_bytes()
                uv = [[float(value) for value in line.split()[1:3]]
                      for line in path.read_text().splitlines() if line.startswith("vt ")]
                return data, uv
            async with open_backend("rust") as reader:
                await json_result(reader, "scene_open", {"path": str(path)})
                saved = root / "fbx-readback.clorust"
                await json_result(reader, "scene_save", {"path": str(saved), "thumbnail": False})
                scene = json.loads(saved.read_text())["scene"]
                data = Path(scene["fabrics"][0]["texture_maps"]["base"]).read_bytes()
                return data, [uv for mesh in scene["imported_meshes"] for uv in mesh["texcoords"]]

        def pixels(data):
            with Image.open(BytesIO(data)) as image:
                return image.size, image.convert("RGBA").tobytes()

        def bounds(uv):
            return tuple(pair(axis) for axis in zip(*uv) for pair in (min, max))

        async with open_backend("rust") as session:
            await json_result(session, "scene_open", {"path": str(fixture)})
            saved = root / "swatches.clorust"
            await json_result(session, "scene_save", {"path": str(saved), "thumbnail": False})
            await json_result(session, "scene_open", {"path": str(saved)})
            expected = [pixels((reference / f"colorway-base-{i}.png").read_bytes()) for i in range(2)]
            assert expected[0] != expected[1]
            checked = {}
            for ext in ("obj", "fbx", "glb", "gltf"):
                output = root / ext / f"swatches.{ext}"
                await json_result(session, "scene_export", {"path": str(output), "colorways": True})
                if ext != "glb":
                    assert {p.name for p in output.parent.glob("*.png")} == {"colorway-base-0.png", "colorway-base-1.png"}
                for index, name in enumerate(names):
                    file = f"swatches_{name}.{ext}"
                    actual, actual_uv = await read_texture(output.with_name(file))
                    native, native_uv = await read_texture(reference / f"textured-{ext}" / file)
                    assert pixels(actual) == pixels(native) == expected[index], (ext, name)
                    assert all(abs(a - b) < 1e-5
                               for a, b in zip(bounds(actual_uv), bounds(native_uv), strict=True)), (ext, name)
                checked[ext] = {"colorway_textures": 2, "pixel_exact": True, "uv_bounds_tolerance": 1e-5}
    print(json.dumps({"case": "native multi-colorway base textures", "formats": checked}))


async def verify_native_texture_transforms():
    reference = Path(os.environ["CLO_NATIVE_TEXTURE_TRANSFORM_EXPORTS"]).resolve(strict=True)
    tolerance = 2e-5
    raw_millimetre_tolerance = 5e-5

    def gltf_uv(path):
        model, binary = read_gltf_export(path)
        coordinates = {"accessor": []}
        for mesh in model["meshes"]:
            for primitive in mesh["primitives"]:
                attributes = primitive["attributes"]
                assert {name for name in attributes if name.startswith("TEXCOORD_")} == {"TEXCOORD_0"}
                accessor = model["accessors"][attributes["TEXCOORD_0"]]
                view = model["bufferViews"][accessor["bufferView"]]
                start = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
                points = [
                    struct.unpack_from("<2f", binary, start + i * view.get("byteStride", 8))
                    for i in range(accessor["count"])
                ]
                coordinates["accessor"].extend(points)
                material = model["materials"][primitive["material"]]
                base = material["pbrMetallicRoughness"].get("baseColorTexture")
                for kind, texture in (("base", base), ("normal", material.get("normalTexture"))):
                    if texture is None:
                        continue
                    transform = texture["extensions"]["KHR_texture_transform"]
                    assert set(transform) <= {"scale", "offset"}
                    coordinates.setdefault(kind + "_scale", []).append(transform["scale"])
                    if "offset" in transform:
                        coordinates.setdefault(kind + "_offset", []).append(transform["offset"])
                    su, sv = transform["scale"]
                    x, y = transform.get("offset", [0, 0])
                    coordinates.setdefault(kind, []).extend((u * su + x, v * sv + y) for u, v in points)
        return coordinates

    with TemporaryDirectory(prefix="clo-texture-transforms-") as directory:
        root = Path(directory)

        async def read_uv(path):
            if path.suffix in (".glb", ".gltf"):
                return gltf_uv(path)
            if path.suffix == ".obj":
                return {"accessor": [
                    tuple(map(float, line.split()[1:3]))
                    for line in path.read_text().splitlines() if line.startswith("vt ")
                ]}
            async with open_backend("rust") as reader:
                await json_result(reader, "scene_open", {"path": str(path)})
                saved = root / "fbx-readback.clorust"
                await json_result(reader, "scene_save", {"path": str(saved), "thumbnail": False})
                scene = json.loads(saved.read_text())["scene"]
                return {"accessor": [uv for mesh in scene["imported_meshes"] for uv in mesh["texcoords"]]}

        checked = {}
        async with open_backend("rust") as session:
            for case in ("base", "rotate", "offset", "mixed", "dual", "normal-only"):
                stem = f"uv-probe-{case}"
                fixture = reference / (stem + ".zpac")
                await json_result(session, "scene_open", {"path": str(fixture), "mode": "replace"})
                before = await json_result(session, "scene_info")
                checked[case] = {}
                for extension in ("glb", "gltf", "obj", "fbx"):
                    output = root / f"{stem}.{extension}"
                    await json_result(session, "scene_export", {"path": str(output), "colorways": False})
                    native = await read_uv(reference / output.name)
                    rust = await read_uv(output)
                    assert native.keys() == rust.keys(), (case, extension, native.keys(), rust.keys())
                    for key, expected in native.items():
                        actual = rust[key]
                        limit = raw_millimetre_tolerance if key == "accessor" and extension in ("glb", "gltf") else tolerance
                        # Exporters can deduplicate or order UVs differently. Check both directions.
                        for points, targets in ((actual, expected), (expected, actual)):
                            assert points and targets, (case, extension, key)
                            for u, v in points:
                                assert any(abs(u - x) < limit and abs(v - y) < limit
                                           for x, y in targets), (case, extension, key, (u, v))
                    checked[case][extension] = len(rust["accessor"])
                assert await json_result(session, "scene_info") == before
    print(json.dumps({"case": "native planar texture transforms", "uv_tolerance": tolerance,
                      "raw_mm_tolerance": raw_millimetre_tolerance,
                      "coordinate_counts": checked, "scene_unchanged": True}))


async def verify_native_colorway_normals():
    from io import BytesIO
    from PIL import Image

    reference = Path(os.environ["CLO_NATIVE_NORMAL_EXPORTS"]).resolve(strict=True)
    fixture = reference / "multi-colorway-normal.zpac"
    pbr_capture = os.environ.get("CLO_NATIVE_PBR_EXPORTS")
    names = ("Original", "Alternate")

    def pixels(data):
        with Image.open(BytesIO(data)) as image:
            return image.size, image.convert("RGBA").tobytes()

    def gltf_normal(path):
        model, binary = read_gltf_export(path)
        normal = model["materials"][0]["normalTexture"]
        image = model["images"][model["textures"][normal["index"]]["source"]]
        if "uri" in image:
            data = path.with_name(image["uri"]).read_bytes()
        else:
            view = model["bufferViews"][image["bufferView"]]
            start = view.get("byteOffset", 0)
            data = binary[start:start + view["byteLength"]]
        return pixels(data), normal["scale"], normal["extensions"]["KHR_texture_transform"]["scale"]

    with TemporaryDirectory(prefix="clo-colorway-normal-") as directory:
        root = Path(directory)

        async def read_fbx_map(path, kind):
            async with open_backend("rust") as session:
                await json_result(session, "scene_open", {"path": str(path)})
                copy = root / "copied.clorust"
                await json_result(session, "scene_save", {"path": str(copy), "thumbnail": False})
                scene = json.loads(copy.read_text())["scene"]
                return pixels(Path(scene["fabrics"][0]["texture_maps"][kind]).read_bytes())

        async with open_backend("rust") as session:
            await json_result(session, "scene_open", {"path": str(fixture)})
            before = await json_result(session, "scene_info")
            expected = [pixels((reference / "normal-obj" / f"pbr-normal-{i}.png").read_bytes())
                        for i in range(2)]
            assert expected[0] != expected[1]
            for ext in ("obj", "fbx", "glb", "gltf"):
                base = root / ext / f"channels.{ext}"
                await json_result(session, "scene_export", {"path": str(base), "colorways": True})
                for index, colorway in enumerate(names):
                    output = base.with_stem(f"channels_{colorway}")
                    if ext == "obj":
                        line = next(line for line in output.with_suffix(".mtl").read_text().splitlines()
                                    if line.startswith("map_bump "))
                        assert line.split(maxsplit=1)[1] == f"pbr-normal-{index}.png"
                        result = pixels(output.with_name(f"pbr-normal-{index}.png").read_bytes())
                    elif ext == "fbx":
                        result = await read_fbx_map(output, "normal")
                        assert result == await read_fbx_map(reference / "normal-fbx" / output.name, "normal")
                    else:
                        if ext == "gltf" and index == 1:
                            native = reference / "normal-gltf" / "channels.gltf"
                        else:
                            native = reference / "pbr-glb" / f"normal-control-0_{colorway}.glb"
                        result, strength, uv_scale = gltf_normal(output)
                        native_pixels, native_strength, native_scale = gltf_normal(native)
                        assert result == native_pixels
                        assert abs(strength - native_strength) < 1e-6
                        assert all(abs(a - b) < 1e-6 for a, b in zip(uv_scale, native_scale, strict=True))
                    assert result == expected[index], (ext, colorway)
            assert await json_result(session, "scene_info") == before

            if pbr_capture:
                capture = Path(pbr_capture).resolve(strict=True)
                fixture = capture / "multi-colorway-pbr.zpac"
                await json_result(session, "scene_open", {"path": str(fixture)})
                for ext in ("obj", "fbx"):
                    base = root / f"pbr-{ext}" / f"channels.{ext}"
                    await json_result(session, "scene_export", {"path": str(base), "colorways": True})
                    official = capture / f"pbr-{ext}"
                    assert {p.name for p in base.parent.iterdir()} == {p.name for p in official.iterdir()}
                    for image in base.parent.glob("*.png"):
                        assert pixels(image.read_bytes()) == pixels((official / image.name).read_bytes())
                    if ext == "fbx":
                        for colorway in names:
                            output = base.with_stem(f"channels_{colorway}")
                            assert await read_fbx_map(output, "opacity") == await read_fbx_map(
                                official / output.name, "opacity",
                            )
    print(json.dumps({"case": "native normal maps", "formats": ["obj", "fbx", "glb", "gltf"],
                      "colorways": 2, "pixel_exact": True, "gltf_normal_strength_and_scale": True,
                      "scene_unchanged": True, "obj_and_fbx_pbr_sidecars": bool(pbr_capture)}))


async def verify_native_colorway_refresh():
    fixtures = [Path(os.environ[key]).resolve(strict=True)
                for key in ("CLO_REORDERED_ZPRJ", "CLO_REORDERED_ZPAC")]
    names = ("Original", "Alternate")
    texture_archives = []
    with TemporaryDirectory(prefix="clo-colorway-refresh-") as directory:
        root = Path(directory)
        for fixture in fixtures:
            async with open_backend("rust") as session:
                async def api(name, *args):
                    return (await json_result(session, "clo_api_call", {"name": name, "args": list(args)}))["value"]

                async def archived_rgba():
                    rgba = await api("fabric_api.GetFabricPBRMaterialBaseColor", 0, 0)
                    return list(struct.unpack("<4f", struct.pack("<4f", *rgba)))

                async def export(stem, ext, grouped):
                    path = root / fixture.suffix[1:] / f"{stem}.{ext}"
                    result = await json_result(session, "scene_export", {"path": str(path), "colorways": True})
                    files = [path.with_stem(f"{stem}_{colorway}") for colorway in names] if grouped else [path]
                    companions = {"obj": "mtl", "gltf": "bin"}.get(ext)
                    if companions:
                        files += [file.with_suffix(f".{companions}") for file in files]
                    assert result == {"path": str(path), "result": [str(file) for file in files]}
                    assert all(file.is_file() for file in files)
                    return files[0]

                select = "utility_api.SetCurrentColorwayIndex"
                refresh = "utility_api.UpdateColorways"
                color = "fabric_api.GetFabricPBRMaterialBaseColor"
                await json_result(session, "scene_open", {"path": str(fixture)})
                expected = [await api(color, i, 0, 0) for i in range(2)]
                await export("opened", "glb", True)
                saved = root / f"copy{fixture.suffix}"
                await json_result(session, "scene_save", {"path": str(saved), "thumbnail": False})
                glb = await export("after-save", "glb", False)
                await export("after-save", "gltf", False)
                await export("after-save", "obj", True)
                model, _ = read_gltf_export(glb)
                active_color = model["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"]
                assert all(abs(a - b ** 2.2) < 1e-6 for a, b in zip(active_color[:3], expected[1][:3], strict=True))
                assert await api(select, 2) is None
                failed = await session.call_tool("clo_api_call", {"name": select, "args": [-1]})
                assert failed.isError
                assert await api(refresh, False) is None
                await export("no-refresh", "glb", False)
                assert await api(select, 1) is None
                await export("after-reselect", "glb", True)
                await json_result(session, "scene_save", {"path": str(saved), "thumbnail": False})
                assert await api(refresh, True) is None
                await export("after-refresh", "gltf", True)
                await json_result(session, "scene_save", {"path": str(saved), "thumbnail": False})
                await json_result(session, "fabric_set_color", {"fabric_index": 0, "color": "#2244aa"})
                await export("after-color-edit", "glb", True)
                edited = await api(color, 0, 0)
                await json_result(session, "clo_run_python", {"code": f"{select}(0)"})
                assert await api(color, 0, 0) == expected[0]
                assert await api(color, 1, 0, 0) == edited
                assert (await json_result(session, "scene_info"))["current_colorway"] == 0
                await api(select, 1)
                assert await api(color, 0, 0) == edited
                await json_result(session, "scene_open", {"path": str(saved)})
                await export("reopened", "glb", True)
                importer, option = (
                    ("ImportZprj", "ImportZPRJOption") if saved.suffix == ".zprj"
                    else ("ImportZpac", "ImportExportOption")
                )
                await json_result(session, "scene_save", {"path": str(saved), "thumbnail": False})
                await export("before-python-reopen", "glb", False)
                code = f"import_api.{importer}({str(saved)!r}, ApiTypes.{option}())"
                assert (await json_result(session, "clo_run_python", {"code": code}))["value"] is True
                scene = await json_result(session, "scene_info")
                assert scene["colorways"] == list(names) and scene["current_colorway"] == 1
                assert [await api(color, i, 0, 0) for i in range(2)] == expected
                await export("python-reopened", "glb", True)
                for selected in (0, 1):
                    await api(select, selected)
                    await json_result(session, "scene_save", {"path": str(saved), "thumbnail": False})
                    glb = await export(f"saved-colorway-{selected}", "glb", False)
                    model, _ = read_gltf_export(glb)
                    rgb = model["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"][:3]
                    assert all(abs(a - b ** 2.2) < 1e-6
                               for a, b in zip(rgb, expected[selected][:3], strict=True))
                    await json_result(session, "scene_open", {"path": str(saved)})
                    assert (await json_result(session, "scene_info"))["current_colorway"] == selected
                    assert await api(color, 0, 0) == expected[selected]
                    assert [await api(color, i, 0, 0) for i in range(2)] == expected
                await json_result(session, "fabric_set_color", {"fabric_index": 0, "color": "#123456"})
                first_edit = await archived_rgba()
                await json_result(session, "scene_save", {"path": str(saved), "thumbnail": False})
                await json_result(session, "scene_open", {"path": str(saved)})
                assert [await api(color, i, 0, 0) for i in range(2)] == [expected[0], first_edit]
                await json_result(session, "fabric_set_color", {"fabric_index": 0, "color": "#2468ac"})
                alternate = await archived_rgba()
                await api(select, 0)
                await json_result(session, "fabric_set_color", {"fabric_index": 0, "color": "#a34429"})
                original = await archived_rgba()
                await json_result(session, "scene_save", {"path": str(saved), "thumbnail": False})
                await json_result(session, "scene_open", {"path": str(saved)})
                assert (await json_result(session, "scene_info"))["current_colorway"] == 0
                assert [await api(color, i, 0, 0) for i in range(2)] == [original, alternate]
                assert await api(color, 0, 0) == original
                async def texture_images(kind="base"):
                    portable_path = saved.with_suffix(".clorust")
                    await json_result(session, "scene_save", {"path": str(portable_path)})
                    scene = json.loads(portable_path.read_text())["scene"]
                    fabric = scene["fabrics"][0]
                    results = []
                    for index, variant in enumerate(fabric["native_colorway_materials"]["values"]):
                        material = fabric if index == scene["current_colorway"] else variant
                        path = material["texture_maps"].get(kind)
                        results.append(Path(path).read_bytes() if path else None)
                    return results

                async def set_base(path):
                    await json_result(session, "fabric_set_texture", {
                        "fabric_index": 0, "map": "base", "image_path": str(path),
                    })

                baseline = await texture_images()
                if all(baseline) and baseline[0] != baseline[1]:
                    replacement = saved.with_suffix(".png")
                    replacement.write_bytes(baseline[0])
                    await api(select, 1)
                    await set_base(replacement)
                    await api(select, 0)
                    await set_base("")
                    await json_result(session, "scene_save", {"path": str(saved), "thumbnail": False})
                    await json_result(session, "scene_open", {"path": str(saved)})
                    assert await texture_images() == [None, baseline[0]]
                    assert (await json_result(session, "scene_info"))["current_colorway"] == 0
                    await set_base(replacement)
                    await json_result(session, "scene_save", {"path": str(saved), "thumbnail": False})
                    await json_result(session, "scene_open", {"path": str(saved)})
                    assert await texture_images() == [baseline[0], baseline[0]]
                    channels = ("normal", "roughness", "displacement", "opacity", "metalness")
                    original = {kind: await texture_images(kind) for kind in channels}
                    for kind in channels:
                        await json_result(session, "fabric_set_texture", {
                            "fabric_index": 0, "map": kind, "image_path": str(replacement),
                        })
                    await json_result(session, "scene_save", {"path": str(saved), "thumbnail": False})
                    await json_result(session, "scene_open", {"path": str(saved)})
                    for kind in channels:
                        assert await texture_images(kind) == [baseline[0], original[kind][1]], kind
                    target_bytes = saved.read_bytes()
                    missing = replacement.with_name("missing.png")
                    missing.write_bytes(baseline[0])
                    await json_result(session, "fabric_set_texture", {
                        "fabric_index": 0, "map": "normal", "image_path": str(missing),
                    })
                    missing.unlink()
                    rejected = await session.call_tool("scene_save", {"path": str(saved), "thumbnail": False})
                    assert rejected.isError and saved.read_bytes() == target_bytes
                    texture_archives.append(fixture.name)
    shared_fixture = os.environ.get("CLO_SHARED_COLORWAY_FIXTURE")
    if shared_fixture:
        await verify_shared_colorway_edits(Path(shared_fixture).resolve(strict=True))
    print(json.dumps({"case": "native save colorway refresh", "formats": ["zprj", "zpac"],
                      "gltf_and_glb_current_after_save": True, "obj_stays_grouped": True,
                      "valid_refresh_restores_bundle": True, "active_colorway_materials_match": True,
                      "public_and_python_reopen_restore_bundle": True,
                      "native_save_preserves_selected_colorway": True,
                      "native_save_preserves_recolors_by_colorway": True,
                      "verified_colorway_texture_archives": texture_archives,
                      "shared_colorway_verified": bool(shared_fixture)}))


async def verify_shared_colorway_edits(fixture):
    with TemporaryDirectory(prefix="clo-shared-colorway-") as directory:
        root = Path(directory)
        path = root / "shared.zpac"
        async with open_backend("rust") as session:
            async def call(tool, **args):
                return await json_result(session, tool, args)

            async def colors():
                return [(await call("clo_api_call", name="fabric_api.GetFabricPBRMaterialBaseColor",
                                    args=[index, 0, 0]))["value"] for index in range(3)]

            async def base_images():
                portable = root / "shared.clorust"
                await call("scene_save", path=str(portable))
                scene = json.loads(portable.read_text())["scene"]
                fabric = scene["fabrics"][0]
                images = []
                for index, variant in enumerate(fabric["native_colorway_materials"]["values"]):
                    material = fabric if index == scene["current_colorway"] else variant
                    image = material["texture_maps"].get("base")
                    images.append(Path(image).read_bytes() if image else None)
                return images

            await call("scene_open", path=str(fixture))
            assert (await call("scene_info"))["current_colorway"] == 2
            original = await colors()
            images = await base_images()
            assert original[0] == original[2] != original[1]
            assert images[0] == images[2] != images[1] and all(images)
            await call("fabric_set_color", fabric_index=0, color="#27b46c")
            changed = await colors()
            assert changed[0] == changed[2] != original[0] and changed[1] == original[1]
            await call("fabric_set_texture", fabric_index=0, map="base", image_path="")
            assert await base_images() == [None, images[1], None]
            await call("scene_save", path=str(path), thumbnail=False)
            await call("scene_open", path=str(path))
            assert (await call("scene_info"))["current_colorway"] == 2
            assert await colors() == [
                list(struct.unpack("<4f", struct.pack("<4f", *color))) for color in changed
            ]
            assert await base_images() == [None, images[1], None]
            replacement = root / "replacement.png"
            replacement.write_bytes(images[1])
            await call("fabric_set_texture", fabric_index=0, map="base", image_path=str(replacement))
            await call("scene_save", path=str(path), thumbnail=False)
            await call("scene_open", path=str(path))
            assert await base_images() == [images[1], images[1], images[1]]


async def compare_live_scene_open():
    project = Path(os.environ["CLO_SCENE_OPEN_PROJECT"])
    garment = Path(os.environ["CLO_SCENE_OPEN_GARMENT"])
    with_avatar = Path(os.environ["CLO_SCENE_OPEN_AVATAR_PROJECT"])
    assert all(path.is_file() for path in (project, garment, with_avatar))
    state_dir = Path(os.environ["CLO_MCP_STATE"])
    invalid = state_dir / "corrupt-scene-open.zprj"
    missing = state_dir / "missing-scene-open.zprj"
    assert state_dir.is_dir() and not missing.exists()
    cases = (
        ("empty ZPRJ append", None, project, "append", 2, 2, [1, 1]),
        ("empty ZPAC append", None, garment, "append", 2, 2, [1, 1]),
        ("append ZPRJ to project", project, project, "append", 4, 2, [0, 0, 1, 1]),
        ("append ZPAC to project", project, garment, "append", 4, 2, [0, 0, 1, 1]),
        ("append embedded avatar", project, with_avatar, "append", 4, 3, [0, 0, 2, 1]),
        ("retain existing avatar", with_avatar, project, "append", 4, 3, [1, 0, 2, 2]),
        ("append same avatar", with_avatar, with_avatar, "append", 4, 4, [1, 0, 3, 2]),
        ("replace garment with avatar present", with_avatar, garment, "replace", 2, 1, [0, 0]),
        ("missing file", project, missing, "append", 2, 1, [0, 0]),
        ("unrecognized mode defaults to replace", project, project, "unrecognized", 2, 1, [0, 0]),
        ("invalid ZPRJ append", project, invalid, "append", 2, 1, [0, 0]),
        ("invalid ZPRJ replaces scene", project, invalid, "replace", 0, 1, []),
    )
    keys = ("project", "project_path", "patterns", "avatars", "colorways", "current_colorway",
            "pattern_count", "seam_groups", "fabric_count", "fabrics")

    async def read(session):
        scene = await json_result(session, "scene_info")
        total = (await json_result(session, "clo_api_call", {
            "name": "fabric_api.GetFabricCount", "args": [False],
        }))["value"]
        fabrics = await json_result(session, "fabric_list")
        return ({key: scene[key] for key in keys}, fabrics, total,
                await json_result(session, "seams_list"))

    async with open_backend("official") as clo, open_backend("rust") as rust:
        original = await json_result(clo, "scene_info")
        assert (await json_result(clo, "clo_status"))["bridge"] == "up"
        reports = []
        with invalid.open("xb") as handle:
            handle.write(b"invalid project")
        try:
            for session in (clo, rust):
                await inspect_contract(session)
            for label, initial, source, mode, count, material_count, assignments in cases:
                observed = []
                for backend, session in (("official", clo), ("rust", rust)):
                    await json_result(session, "scene_clear", {"keep_avatar": False})
                    if initial:
                        await json_result(session, "scene_open", {"path": str(initial)})
                    if mode == "unrecognized":
                        await json_result(session, "pattern_ops", {
                            "pattern": "0", "op": "rename", "args": {"name": "sentinel"},
                        })
                    before = await read(session)
                    response = await session.call_tool("scene_open", {"path": str(source), "mode": mode})
                    after = await read(session)
                    scene, _, total, _ = after
                    assert (scene["pattern_count"], total) == (count, material_count), (backend, label, after)
                    assert [panel["fabric_index"] for panel in scene["patterns"]] == assignments
                    if source == missing:
                        assert response.isError and after == before
                    else:
                        assert not response.isError, (backend, label, response.content)
                        reply = response.structuredContent or json.loads(response.content[0].text)
                        assert {key: reply[key] for key in ("ok", "path", "pattern_count", "avatars")} == {
                            "ok": True, "path": str(source), "pattern_count": count, "avatars": scene["avatars"]}
                    if mode == "append":
                        assert scene["project_path"] == before[0]["project_path"]
                        assert scene["colorways"] == before[0]["colorways"]
                    if source == invalid and mode == "append":
                        assert after == before
                    if source == garment and mode == "replace":
                        assert scene["project_path"] == "NULL" and scene["avatars"] == before[0]["avatars"]
                    if mode == "unrecognized":
                        assert scene["patterns"][0]["name"] == "Front"
                    observed.append(after)
                assert observed[0] == observed[1], (label, observed)
                reports.append({"case": label, "panels": count, "materials": material_count,
                                "assignments": assignments, "project_path": observed[0][0]["project_path"],
                                "avatars": observed[0][0]["avatars"]})
        finally:
            invalid.unlink()
            if original["project_path"] not in ("NULL", "", None):
                await json_result(clo, "scene_open", {"path": original["project_path"]})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    print(json.dumps({"tool": "scene_open", "cases": reports, "scene_and_fabric_readback": "exact",
                      "result": "pass"}, ensure_ascii=False))


async def composed_archive_api(session, name, *args):
    return (await json_result(session, "clo_api_call", {"name": name, "args": list(args)}))["value"]


async def read_composed_archive(session):
    scene = await json_result(session, "scene_info")
    fabrics = await json_result(session, "fabric_list")
    seams = await json_result(session, "seams_list")
    total = await composed_archive_api(session, "fabric_api.GetFabricCount", False)
    colors = []
    for colorway in range(len(scene["colorways"])):
        row = []
        for fabric in range(total):
            rgba = await composed_archive_api(session, "fabric_api.GetFabricPBRMaterialBaseColor", colorway, fabric, 0)
            row.append(struct.pack("<4f", *rgba).hex())
        colors.append(row)
    return {"patterns": scene["patterns"], "colorways": scene["colorways"],
            "current_colorway": scene["current_colorway"], "materials": total,
            "fabric_rgba_f32": colors, "fabrics": fabrics, "seams": seams,
            "catalog_names": [await composed_archive_api(session, "fabric_api.GetFabricName", slot)
                              for slot in range(total)]}


async def snapshot_composed_archive(session, path):
    await json_result(session, "scene_save", {"path": str(path)})
    saved = json.loads(path.read_text())["scene"]
    simulation = saved["simulation"]
    assert simulation["source"] == "native_saved_pose"
    meshes = [{key: mesh[key] for key in ("positions", "triangles", "texcoords")}
              for mesh in simulation["meshes"]]
    return (meshes, saved["native_seam_groups"])


async def compare_live_composed_zpac():
    project = Path(os.environ["CLO_SCENE_OPEN_PROJECT"])
    garment = Path(os.environ["CLO_SCENE_OPEN_GARMENT"])
    directory = Path(os.environ["CLO_MCP_STATE"])
    assert project.is_file() and garment.is_file() and directory.is_dir()

    api = composed_archive_api
    read = read_composed_archive

    cases = (("empty", None, garment, [1, 1], [0, 0], 1, 0),
             ("empty_project", None, project, [1, 1], [0, 0], 1, 0),
             ("nonempty", project, garment, [0, 0, 1, 1], [0, 0, 1, 1], 2, 1))
    selected = os.environ.get("CLO_COMPOSED_ZPAC_CASE")
    variants = {
        "active_zero": (garment, "utility_api.SetCurrentColorwayIndex(0)"),
        "renamed": (garment, "utility_api.SetColorwayName(0,'Spare'); utility_api.SetColorwayName(1,'Original')"),
        "short": (garment, "utility_api.DeleteColorwayItem(1)"),
        "long": (project, "assert utility_api.CopyColorway(0,2) == 2; "
                          "utility_api.SetColorwayName(2,'Third'); utility_api.SetCurrentColorwayIndex(2)"),
    }
    sources = {label: directory / f"source-{label}.zpac" for label in variants}
    same_indices = [0, 0, 1, 1]
    palette_cases = tuple((label, initial, source, same_indices, same_indices, 2, active)
                          for label, initial, source, active in (
                              ("active_index", project, sources["active_zero"], 1),
                              ("renamed_palette", project, sources["renamed"], 1),
                              ("short_source", project, sources["short"], 1),
                              ("short_receiver", sources["short"], garment, 0),
                              ("long_receiver", sources["long"], garment, 2)))
    unused = os.environ.get("CLO_COMPOSED_UNUSED_FABRIC")
    unused_cases = (
        ("empty_double", None, garment, [1, 1, 2, 2], same_indices, 2, 0),
        ("unused_between", project, garment, [0, 0, 2, 2], same_indices, 2, 1),
        ("unused_after", project, garment, same_indices, same_indices, 2, 1),
        ("empty_unused_between", None, garment, [1, 1, 3, 3], same_indices, 2, 0),
    ) if unused else ()
    cases += palette_cases + unused_cases
    if selected:
        cases = tuple(case for case in cases if case[0] == selected
                      or selected == "palette" and case in palette_cases
                      or selected == "unused" and case in unused_cases)
        assert cases, f"unknown composed ZPAC case {selected}"
    reports = []
    async with open_backend("official") as clo, open_backend("rust") as rust:
        original = (await json_result(clo, "scene_info"))["project_path"]
        status = await json_result(clo, "clo_status")
        assert status["bridge"] == "up"
        try:
            required_sources = {path for case in cases for path in case[1:3]}
            await json_result(clo, "scene_clear", {"keep_avatar": False})
            for label, (fixture, code) in variants.items():
                path = sources[label]
                if path in required_sources:
                    assert not path.exists(), "use a fresh test directory"
                    await json_result(clo, "scene_open", {"path": str(fixture)})
                    await json_result(clo, "clo_run_python", {"code": code + "; utility_api.UpdateColorways(True)"})
                    await json_result(clo, "scene_export", {"path": str(path)})
                    assert path.is_file()
            for label, initial, source, before, after, count, active in cases:
                archives = {backend: directory / f"composition-{label}-{backend}.zpac"
                            for backend in ("official", "rust")}
                assert not any(path.exists() for path in archives.values()), "use a fresh test directory"
                expected = None
                expected_archive = None
                expected_import = None
                report = {"case": label, "checked": {}, "export_errors": {}}
                for backend, session in (("official", clo), ("rust", rust)):
                    await json_result(session, "scene_clear", {"keep_avatar": False})
                    if initial:
                        await json_result(session, "scene_open", {"path": str(initial)})
                    if label in ("empty_double", "empty_unused_between"):
                        await json_result(session, "scene_open", {"path": str(garment), "mode": "append"})
                    if label in ("unused_between", "empty_unused_between"):
                        assert await api(session, "fabric_api.AddFabric", unused) == (1 if initial else 2)
                        assert await api(session, "fabric_api.GetCurrentFabricIndex") == 1
                    await json_result(session, "scene_open", {"path": str(source), "mode": "append"})
                    if label == "unused_after":
                        assert await api(session, "fabric_api.AddFabric", unused) == 2
                        assert await api(session, "fabric_api.GetCurrentFabricIndex") == 1
                    else:
                        assert await api(session, "fabric_api.GetCurrentFabricIndex") == 0
                    composed = await read(session)
                    assert [piece["fabric_index"] for piece in composed["patterns"]] == before
                    if expected_import is None:
                        expected_import = composed
                    assert composed == expected_import, (label, backend, composed, expected_import)
                    response = await session.call_tool("scene_export", {"path": str(archives[backend])})
                    assert await read(session) == composed, (label, backend, "export changed the scene")
                    if response.isError:
                        assert not archives[backend].exists()
                        report["export_errors"][backend] = response.content[0].text
                        continue
                    reply = response.structuredContent or json.loads(response.content[0].text)
                    assert reply["path"] == str(archives[backend]) and archives[backend].is_file()
                    if any(label == case[0] for case in unused_cases):
                        repeated = archives[backend].with_stem(archives[backend].stem + "-again")
                        assert not repeated.exists()
                        await json_result(session, "scene_export", {"path": str(repeated)})
                        assert repeated.is_file() and await read(session) == composed
                    for reader, target in (("official", clo), ("rust", rust)):
                        await json_result(target, "scene_open", {"path": str(archives[backend])})
                        reopened = await read(target)
                        assert [piece["fabric_index"] for piece in reopened["patterns"]] == after
                        assert reopened["materials"] == count and reopened["current_colorway"] == active
                        assert reopened["colorways"] == composed["colorways"]
                        if expected is None:
                            expected = reopened
                        assert reopened == expected, (label, backend, reader, reopened, expected)
                        report["checked"][f"{backend}_written_{reader}_read"] = True
                        if reader == "rust":
                            snapshot = archives[backend].with_suffix(".clorust")
                            archive = await snapshot_composed_archive(target, snapshot)
                            assert len(archive[0]) == len(after)
                            if expected_archive is None:
                                expected_archive = archive
                            assert archive == expected_archive, (label, backend, "stored meshes or seam endpoints differ")
                        if label == "short_source":
                            await json_result(target, "fabric_set_color", {"fabric_index": 1, "color": "#123456"})
                            recolored = (await read(target))["fabric_rgba_f32"]
                            assert recolored[0][1] == recolored[1][1] != reopened["fabric_rgba_f32"][0][1]
                report["stored_meshes_and_seams_compared"] = not report["export_errors"]
                reports.append(report)
        finally:
            if original not in ("NULL", "", None):
                await json_result(clo, "scene_open", {"path": original})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    success = all(len(report["checked"]) == 4 for report in reports)
    print(json.dumps({"tool": "scene_export", "format": "ZPAC", "cases": reports,
                      "result": "pass" if success else "fail"}, ensure_ascii=False))
    if not success:
        raise SystemExit(1)


async def compare_live_composed_zprj():
    root = Path(os.environ["CLO_MCP_STATE"])
    garment = Path(os.environ["CLO_SCENE_OPEN_GARMENT"])
    unused = os.environ["CLO_COMPOSED_UNUSED_FABRIC"]
    project = Path(os.environ["CLO_SCENE_OPEN_PROJECT"])
    avatar_project = Path(os.environ["CLO_SCENE_OPEN_AVATAR_PROJECT"])
    assert all(path.is_file() for path in (garment, project, avatar_project, Path(unused)))
    assert root.is_dir()
    cases = (("sparse", project, [0, 0, 2, 2], [0, 0, 1, 1]),
             ("avatar", avatar_project, [1, 0, 2, 2], [1, 0, 2, 2]))
    new_avatar = os.environ.get("CLO_ZPRJ_AVATAR_FIXTURE")
    replacement = os.environ.get("CLO_ZPRJ_REPLACEMENT_AVATAR")
    if new_avatar:
        assert Path(new_avatar).is_file()
        cases += (("avatar_add", project, [0, 0, 1, 1], [0, 0, 1, 1]),
                  ("avatar_append_textures", project, [0, 0, 2, 1], [0, 0, 2, 1]),
                  ("avatar_append", avatar_project, [1, 0, 3, 2], [1, 0, 3, 2]))
    if replacement:
        assert Path(replacement).is_file()
        cases += (("avatar_replace", avatar_project, [1, 0, 2, 2], [1, 0, 2, 2]),)
    if selected := os.environ.get("CLO_COMPOSED_ZPRJ_CASE"):
        cases = tuple(case for case in cases if case[0] == selected)
        assert cases, f"unknown composed ZPRJ case {selected}"

    async def identity(session, path, avatars):
        scene = await json_result(session, "scene_info")
        assert (scene["project_path"], scene["avatars"]) == (str(path), avatars), (
            str(path), avatars, scene["project_path"], scene["avatars"])

    def embedded_avatars(path):
        with zipfile.ZipFile(path) as archive:
            return [hashlib.sha256(archive.read(name)).hexdigest()
                    for name in archive.namelist() if name.lower().endswith(".avt")]

    def embedded_textures(path):
        with zipfile.ZipFile(path) as archive:
            garment_member = next(name for name in archive.namelist() if name.lower().endswith(".zpac"))
            with zipfile.ZipFile(BytesIO(archive.read(garment_member))) as clothes:
                return {name: hashlib.sha256(clothes.read(name)).hexdigest()
                        for name in clothes.namelist() if name.startswith("Woven_Denim_")}

    reports = []
    async with open_backend("official") as clo, open_backend("rust") as rust:
        assert (await json_result(clo, "clo_status"))["bridge"] == "up"
        original = (await json_result(clo, "scene_info"))["project_path"]
        backends = (("official", clo), ("rust", rust))
        try:
            for label, initial, live_indices, saved_indices in cases:
                outputs = {}
                for backend, session in backends:
                    await json_result(session, "scene_clear", {"keep_avatar": False})
                    await json_result(session, "scene_open", {"path": str(initial)})
                    if label == "sparse":
                        assert await composed_archive_api(session, "fabric_api.AddFabric", unused) == 1
                    if label in ("avatar_add", "avatar_replace"):
                        path = new_avatar if label == "avatar_add" else replacement
                        await json_result(session, "scene_open", {"path": path, "mode": "replace"})
                    appended = avatar_project if label in ("avatar_append", "avatar_append_textures") else garment
                    await json_result(session, "scene_open", {"path": str(appended), "mode": "append"})
                    avatars = (await json_result(session, "scene_info"))["avatars"]
                    assert len(avatars) == (2 if label == "avatar_append" else int(label != "sparse"))
                    before = await read_composed_archive(session)
                    assert [panel["fabric_index"] for panel in before["patterns"]] == live_indices
                    target = root / f"{label}-{backend}.zprj"
                    copy = target.with_suffix(".zpac")
                    assert not target.exists() and not copy.exists()
                    await json_result(session, "scene_export", {"path": str(copy)})
                    await identity(session, initial, avatars)
                    for _ in range(2):
                        assert await json_result(session, "scene_save", {"path": str(target), "thumbnail": False}) == {"path": str(target)}
                        await identity(session, target, avatars)
                        assert await read_composed_archive(session) == before
                    assert target.is_file()
                    if backend == "rust":
                        avatar_payloads = embedded_avatars(initial)
                        if label in ("avatar_add", "avatar_replace"):
                            avatar_payloads = [hashlib.sha256(Path(path).read_bytes()).hexdigest()]
                        if label == "avatar_append":
                            saved_avatars = embedded_avatars(target)
                            assert len(saved_avatars) == 2 and saved_avatars[0] == avatar_payloads[0]
                            assert saved_avatars[1] != saved_avatars[0]
                            avatar_payloads = saved_avatars
                        if label == "avatar_append_textures":
                            avatar_payloads = embedded_avatars(avatar_project)
                            textures = embedded_textures(avatar_project)
                            assert textures and embedded_textures(target) == textures
                        assert embedded_avatars(target) == avatar_payloads

                    name = f"{root.name}-{label}-{backend}"
                    checkpoint = await json_result(session, "checkpoint_save", {"name": name})
                    checkpoint_file = Path(checkpoint["path"])
                    assert checkpoint["name"] == name and checkpoint_file.suffix == ".zprj" and checkpoint_file.is_file()
                    await identity(session, checkpoint_file, avatars)
                    assert await read_composed_archive(session) == before
                    await json_result(session, "scene_clear", {"keep_avatar": False})
                    assert await json_result(session, "checkpoint_restore", {"name": name}) == {
                        "ok": True, "path": str(checkpoint_file), "pattern_count": 4, "avatars": avatars}
                    await identity(session, checkpoint_file, avatars)
                    outputs[backend] = (target, checkpoint_file, avatars)

                expected = None
                geometry = None
                cross_opens = 0
                for backend, (target, checkpoint_file, avatars) in outputs.items():
                    for file in (target, checkpoint_file):
                        for reader, session in backends:
                            await json_result(session, "scene_clear", {"keep_avatar": False})
                            await json_result(session, "scene_open", {"path": str(file)})
                            await identity(session, file, avatars)
                            reopened = await read_composed_archive(session)
                            assert [panel["fabric_index"] for panel in reopened["patterns"]] == saved_indices
                            assert reopened["materials"] == len(set(saved_indices))
                            if expected is None:
                                expected = reopened
                            assert reopened == expected, (label, backend, reader, reopened, expected)
                            cross_opens += 1
                            if reader == "rust":
                                portable = root / "composed-readback.clorust"
                                pose = await snapshot_composed_archive(session, portable)
                                assert len(pose[0]) == len(saved_indices)
                                if label == "avatar_append" and backend == "rust":
                                    saved = json.loads(portable.read_text())["scene"]
                                    assert saved["avatar"]["mesh"] == saved["additional_avatars"][0]["mesh"]
                                if geometry is None:
                                    geometry = pose
                                assert pose == geometry, (label, backend, reader, file)
                reports.append({"case": label, "reads": cross_opens, "avatars": avatars,
                                "saved_indices": saved_indices, "checkpoints": True,
                                "geometry_and_seam_endpoints": "exact"})
        finally:
            if original not in ("NULL", "", None):
                await json_result(clo, "scene_open", {"path": original})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    print(json.dumps({"tool": "scene_save", "format": "ZPRJ", "cases": reports, "result": "pass"}))


async def compare_live_authored_projects():
    root = Path(os.environ["CLO_MCP_STATE"])
    assert root.is_dir() and not os.environ.get("CLO_RUST_CHECKPOINT_DIR")
    first = ("Rust 纸样", [[0, 0], [137, 0], [159, 188], [0, 208]])
    second = ("Concave", [[320, 0], [430, 0], [430, 100], [385, 55], [320, 100]])
    cases = [
        ("project", "scene_save", ".zprj", [first]),
        ("garment", "scene_export", ".zpac", [first, second]),
        ("checkpoint", "checkpoint_save", ".zprj", [first, second]),
    ]
    reports = []
    async with open_backend("official") as clo, open_backend("rust") as rust:
        assert (await json_result(clo, "clo_status"))["bridge"] == "up"
        original = (await json_result(clo, "scene_info"))["project_path"]
        backends = (("official", clo), ("rust", rust))
        try:
            for label, tool, extension, patterns in cases:
                outputs = {}
                for writer, session in backends:
                    await json_result(session, "scene_clear", {"keep_avatar": False})
                    for index, (name, points) in enumerate(patterns):
                        value = await json_result(session, "pattern_create", {"name": name, "points": points})
                        assert value == {"index": index, "name": name}
                    if label == "project":
                        # Save once, then edit and save again without opening an external archive.
                        initial = root / f"{label}-initial-{writer}.zprj"
                        await json_result(session, "scene_save", {"path": str(initial), "thumbnail": False})
                        await json_result(session, "pattern_ops", {"pattern": first[0], "op": "move_2d",
                                                                   "args": {"dx": 15, "dy": -20}})
                        await json_result(session, "fabric_set_color", {"fabric_index": 0, "color": "#336699"})
                    if tool == "checkpoint_save":
                        result = await json_result(session, tool, {"name": f"{label}-{writer}"})
                        output = Path(result["path"])
                        assert result["name"] == f"{label}-{writer}"
                    else:
                        output = root / f"{label}-{writer}{extension}"
                        args = {"path": str(output)}
                        if tool == "scene_save":
                            args["thumbnail"] = True
                        assert await json_result(session, tool, args) == {"path": str(output)}
                    assert output.suffix == extension and output.is_file()
                    outputs[writer] = output

                reads = 0
                for writer, output in outputs.items():
                    for reader, session in backends:
                        await json_result(session, "scene_clear", {"keep_avatar": False})
                        if label == "checkpoint":
                            await json_result(session, "checkpoint_restore", {"name": f"{label}-{writer}"})
                        else:
                            await json_result(session, "scene_open", {"path": str(output)})
                        info = await json_result(session, "scene_info")
                        assert [panel["name"] for panel in info["patterns"]] == [name for name, _ in patterns]
                        assert [panel["fabric_index"] for panel in info["patterns"]] == [0] * len(patterns)
                        assert info["fabric_count"] == 1 and info["colorways"] == ["Default Colorway"]
                        fabric = (await json_result(session, "fabric_list"))["fabrics"]
                        expected_color = "#336699" if label == "project" else "#ffffff"
                        assert len(fabric) == 1 and fabric[0]["info"]["ColorCode"] == expected_color, (
                            label, writer, reader, fabric)
                        for name, points in patterns:
                            detail = await json_result(session, "pattern_info", {"pattern": name})
                            lines = detail["line_info"]["lines"]
                            assert len(lines) == len(points) and detail["particle_distance"] == 20
                            shift = (15, -20) if label == "project" else (0, 0)
                            for i, line in enumerate(lines):
                                a, b = points[i], points[(i + 1) % len(points)]
                                assert math.isclose(line["length"], math.dist(a, b), abs_tol=0.001)
                                for endpoint, point in (("start", a), ("end", b)):
                                    for k, axis in enumerate(("x", "y")):
                                        assert math.isclose(line[endpoint][axis], point[k] + shift[k], abs_tol=0.001), (
                                            label, writer, reader, line)
                        mesh = root / f"{label}-{writer}-{reader}.glb"
                        await json_result(session, "scene_export", {"path": str(mesh)})
                        model, _ = read_gltf_export(mesh)
                        assert len(model["meshes"]) >= 1 and sum(
                            model["accessors"][primitive["indices"]]["count"]
                            for mesh in model["meshes"] for primitive in mesh["primitives"]
                        ) >= sum(3 * (len(points) - 2) for _, points in patterns)
                        if label == "project":
                            positions = [
                                model["accessors"][primitive["attributes"]["POSITION"]]
                                for mesh in model["meshes"] for primitive in mesh["primitives"]
                            ]
                            for key, combine, bounds in (
                                ("min", min, (0, 0, 200)), ("max", max, (159, 208, 200)),
                            ):
                                actual = [combine(position[key][i] for position in positions) for i in range(3)]
                                assert all(math.isclose(value, expected, abs_tol=0.001)
                                           for value, expected in zip(actual, bounds, strict=True)), (
                                               writer, reader, key, actual)
                            factors = [material["pbrMetallicRoughness"]["baseColorFactor"]
                                       for material in model["materials"]]
                            expected = (0.2 ** 2.2, 0.4 ** 2.2, 0.6 ** 2.2, 1.0)
                            assert factors and all(
                                all(math.isclose(value, channel, abs_tol=0.000001)
                                    for value, channel in zip(factor, expected, strict=True))
                                for factor in factors
                            ), (writer, reader, factors)
                        reads += 1
                reports.append({"case": label, "panels": len(patterns), "cross_reopens": reads,
                                "coordinate_tolerance_mm": 0.001})
        finally:
            if original not in (None, "", "NULL"):
                await json_result(clo, "scene_open", {"path": original})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    print(json.dumps({"cases": reports, "result": "pass"}))


async def compare_live_authored_curves():
    root = Path(os.environ["CLO_MCP_STATE"])
    assert root.is_dir()
    panels = (
        ("Mixed", [[0, 0], [60, 0, 2], [75, 43, 3], [15, 60]]),
        ("Spline 1", [[220, 0], [315, 34, 2], [245, 80]]),
        ("Spline 2", [[440, 0], [475, -20, 2], [540, 15, 2], [525, 100]]),
        ("Spline 3", [[660, 0], [698, -9, 2], [742, 18, 2], [759, 55, 2], [715, 80]]),
        ("Bézier", [[880, 0], [940, 0, 3], [955, 43, 3], [895, 60]]),
        ("曲边三角形", [[1100, 0], [1135, -20, 2], [1180, 0], [1150, 60]]),
        ("Asymmetric", [[1320, 0], [1435, 5, 2], [1440, 37, 2], [1370, 80]]),
    )

    def compare(expected, actual, case):
        assert [actual[key] for key in ("name", "index", "fabric_index", "particle_distance")] == [
            expected[key] for key in ("name", "index", "fabric_index", "particle_distance")
        ], case
        for key in ("Width", "Height"):
            assert abs(float(expected["bbox"][key]) - float(actual["bbox"][key])) < 0.01, (
                case, key, expected["bbox"], actual["bbox"])
        for before, after in zip(expected["line_info"]["lines"], actual["line_info"]["lines"], strict=True):
            assert before["lineIndex"] == after["lineIndex"], case
            assert abs(before["length"] - after["length"]) < 0.01, (case, before, after)
            for endpoint in ("start", "end"):
                assert all(abs(before[endpoint][axis] - after[endpoint][axis]) < 0.001
                           for axis in ("x", "y")), (case, before, after)

    async def measure(session, name):
        return await json_result(session, "pattern_info", {"pattern": name})

    async def edit_endpoint(session, name):
        return await json_result(session, "pattern_edit_points", {
            "pattern": name, "moves": [{"point": 1, "dx": 12, "dy": -9}],
        })

    reports = []
    async with open_backend("official") as clo, open_backend("rust") as rust:
        assert (await json_result(clo, "clo_status"))["bridge"] == "up"
        original = (await json_result(clo, "scene_info"))["project_path"]
        backends = (("official", clo), ("rust", rust))
        try:
            outputs = {}
            baseline = {}
            for writer, session in backends:
                await json_result(session, "scene_clear", {"keep_avatar": False})
                for index, (name, points) in enumerate(panels):
                    assert await json_result(session, "pattern_create", {"name": name, "points": points}) == {
                        "index": index, "name": name}
                    measured = await measure(session, name)
                    if writer == "official":
                        baseline[name] = measured
                    else:
                        compare(baseline[name], measured, (writer, "created", name))
                for suffix in ("zprj", "zpac"):
                    path = root / f"authored-curves-{writer}.{suffix}"
                    assert await json_result(session, "scene_save", {"path": str(path), "thumbnail": False}) == {
                        "path": str(path)}
                    assert path.is_file()
                    outputs[writer, suffix] = path

            edits = {}
            for (writer, suffix), path in outputs.items():
                for reader, session in backends:
                    case = (writer, suffix, reader)
                    await json_result(session, "scene_clear", {"keep_avatar": False})
                    opened = await json_result(session, "scene_open", {"path": str(path)})
                    assert opened["pattern_count"] == len(panels), case
                    for name, _ in panels:
                        compare(baseline[name], await measure(session, name), (*case, name))
                    for name in ("Bézier", "Spline 2"):
                        moved = await edit_endpoint(session, name)
                        after = await measure(session, name)
                        if writer == reader == "official" and suffix == "zprj":
                            edits[name] = (moved, after)
                        expected_move, expected_info = edits[name]
                        assert moved == expected_move, (case, name, moved)
                        compare(expected_info, after, (*case, name, "edited"))
                    reports.append({"writer": writer, "format": suffix, "reader": reader,
                                    "panels": len(panels), "edited_curves": 2})
        finally:
            if original not in (None, "", "NULL"):
                await json_result(clo, "scene_open", {"path": original})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    print(json.dumps({"cases": reports, "result": "pass", "bounds_and_length_tolerance_mm": 0.01,
                      "endpoint_tolerance_mm": 0.001}))


async def compare_live_authored_seams():
    root = Path(os.environ["CLO_MCP_STATE"])
    assert root.is_dir() and not os.environ.get("CLO_RUST_CHECKPOINT_DIR")
    names = ["Front", "后片"]
    outlines = (
        [[0, 0], [120, 0], [120, 100], [0, 100]],
        [[220, 0], [280, -15, 2], [340, 0], [340, 100], [220, 100]],
    )
    directions = [(0, True, False), (2, False, True)]

    async def verify_segments(session, label):
        path = root / f"{label}-decoded.clorust"
        await json_result(session, "scene_save", {"path": str(path)})
        scene = json.loads(path.read_text())["scene"]
        groups = scene["native_seam_groups"]
        assert len(groups) == len(scene["seams"]) == len(directions), (label, groups)
        for group, (line_index, forward_a, forward_b) in zip(groups, directions, strict=True):
            assert group["active"] and group["restored"], (label, group)
            [pair] = group["segments"]
            for endpoint, panel, direction in zip(pair, scene["patterns"], (forward_a, forward_b), strict=True):
                assert endpoint == {
                    "pattern_id": panel["id"], "shape_depth": 1, "shape_index": None,
                    "start_line": line_index, "end_line": line_index, "direction": direction,
                    "start_ratio": 0 if direction else 1, "end_ratio": 1 if direction else 0,
                }, (label, endpoint)

    reports = []
    async with open_backend("official") as clo, open_backend("rust") as rust:
        assert (await json_result(clo, "clo_status"))["bridge"] == "up"
        original = (await json_result(clo, "scene_info"))["project_path"]
        backends = (("official", clo), ("rust", rust))
        try:
            outputs = {}
            for writer, session in backends:
                await json_result(session, "scene_clear", {"keep_avatar": False})
                for name, points in zip(names, outlines, strict=True):
                    await json_result(session, "pattern_create", {"name": name, "points": points})
                for index, (line, forward_a, forward_b) in enumerate(directions):
                    request = {"pattern_a": names[0], "line_a": line, "pattern_b": names[1], "line_b": line,
                               "dir_a": forward_a, "dir_b": forward_b}
                    assert await json_result(session, "sew", request) == {"ok": True, "seam_groups": index + 1}
                expected = await json_result(session, "seams_list")
                assert expected["count"] == 2 and all(row["patterns"] == names for row in expected["groups"])
                for suffix in ("zprj", "zpac"):
                    path = root / f"sewn-{writer}.{suffix}"
                    assert await json_result(session, "scene_save", {"path": str(path), "thumbnail": False}) == {
                        "path": str(path)}
                    outputs[writer, suffix] = (path, expected)
                checkpoint = await json_result(session, "checkpoint_save", {"name": f"sewn-{writer}"})
                path = Path(checkpoint["path"])
                assert path.suffix == ".zprj" and path.is_file()
                outputs[writer, "checkpoint"] = (path, expected)

            async with open_backend("rust") as verifier:
                for (writer, kind), (path, expected) in outputs.items():
                    for reader, session in backends:
                        label = f"{writer}-{kind}-{reader}"
                        await json_result(session, "scene_clear", {"keep_avatar": False})
                        if kind == "checkpoint":
                            opened = await json_result(session, "checkpoint_restore", {"name": f"sewn-{writer}"})
                        else:
                            opened = await json_result(session, "scene_open", {"path": str(path)})
                        assert opened["pattern_count"] == len(names), (label, opened)
                        assert await json_result(session, "seams_list") == expected, label
                        for name in names:
                            detail = await json_result(session, "pattern_info", {"pattern": name})
                            assert detail["seam_groups"] == [0, 1], (label, detail["seam_groups"])
                        if reader == "official":
                            exported = root / f"{label}-official-resave.zpac"
                            await json_result(session, "scene_save", {"path": str(exported), "thumbnail": False})
                            await json_result(verifier, "scene_open", {"path": str(exported)})
                            await verify_segments(verifier, label)
                        else:
                            await verify_segments(session, label)
                            if writer == "rust" and kind == "zprj":
                                initial = await json_result(session, "simulate", {"steps": 0})
                                settled = await json_result(session, "simulate", {"steps": 15})
                                assert initial["stitch_count"] > 0
                                assert settled["stitch_count"] == initial["stitch_count"]
                                assert 0 <= settled["max_stitch_gap_mm"] < initial["max_stitch_gap_mm"]
                                reports.append({
                                    "rust_stitches": initial["stitch_count"],
                                    "gap_before_mm": initial["max_stitch_gap_mm"],
                                    "gap_after_mm": settled["max_stitch_gap_mm"],
                                })
                        reports.append({"writer": writer, "format": kind, "reader": reader, "seam_groups": 2})
        finally:
            if original not in (None, "", "NULL"):
                await json_result(clo, "scene_open", {"path": original})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    print(json.dumps({"cases": reports, "result": "pass"}))


async def compare_live_authored_poses():
    root = Path(os.environ["CLO_MCP_STATE"])
    assert root.is_dir() and not os.environ.get("CLO_RUST_CHECKPOINT_DIR")
    names = ["Front", "后片"]
    outlines = ([[0, 0], [120, 0], [120, 100], [0, 100]],
                [[220, 0], [280, -60, 2], [340, 100]])

    async def pose(session, label):
        path = root / f"{label}.clorust"
        await json_result(session, "scene_save", {"path": str(path)})
        state = json.loads(path.read_text())["scene"]
        meshes = state["simulation"]["meshes"]
        assert [mesh["name"] for mesh in meshes] == names
        assert all(mesh["positions"] and mesh["triangles"] for mesh in meshes)
        return meshes

    def delta(reference, observed):
        errors = []
        for a, b in zip(reference, observed, strict=True):
            assert a["triangles"] == b["triangles"]
            for p, q in zip(a["positions"], b["positions"], strict=True):
                errors.extend(abs(x - y) for x, y in zip(p, q, strict=True))
        return max(errors)

    def compare_surface(reference, observed):
        """Compare world positions at the other mesh's rest coordinates."""
        errors, outside, projected = [], [], []
        for incoming, captured in zip(reference, observed, strict=True):
            triangles = []
            for triangle in incoming["triangles"]:
                uv = [incoming["texcoords"][i] for i in triangle]
                xyz = [incoming["positions"][i] for i in triangle]
                (ax, ay), (bx, by), (cx, cy) = uv
                det = (by - cy) * (ax - cx) + (cx - bx) * (ay - cy)
                if abs(det) > 1e-9:
                    triangles.append((uv, xyz, det))
            assert triangles
            for (x, y), actual in zip(captured["texcoords"], captured["positions"], strict=True):
                candidates = []
                for ((ax, ay), (bx, by), (cx, cy)), (a, b, c), det in triangles:
                    p = ((by - cy) * (x - cx) + (cx - bx) * (y - cy)) / det
                    q = ((cy - ay) * (x - cx) + (ax - cx) * (y - cy)) / det
                    r = 1 - p - q
                    if min(p, q, r) >= -1e-3:
                        at = [p*v + q*w + r*z for v, w, z in zip(a, b, c, strict=True)]
                        candidates.append(math.dist(at, actual))
                if candidates:
                    errors.append(min(candidates))
                    continue
                # A remesh may put a vertex just outside the discretized curve.
                # Measure both the rest-space offset and its world-space projection.
                best = (math.inf, math.inf)
                for uv, xyz, _ in triangles:
                    for i, j in ((0, 1), (1, 2), (2, 0)):
                        ax, ay = uv[i]
                        bx, by = uv[j]
                        dx, dy = bx - ax, by - ay
                        if dx == dy == 0:
                            continue
                        t = max(0.0, min(1.0, ((x - ax) * dx + (y - ay) * dy) / (dx*dx + dy*dy)))
                        rest_distance = math.hypot(x - ax - t * dx, y - ay - t * dy)
                        world = [a + t * (b - a) for a, b in zip(xyz[i], xyz[j], strict=True)]
                        best = min(best, (rest_distance, math.dist(world, actual)))
                outside.append(best[0])
                projected.append(best[1])
        return {"inside_vertices": len(errors), "projected_vertices": len(outside),
                "outside_rest_error_mm": max(outside, default=0), "inside_position_error_mm": max(errors),
                "projected_position_error_mm": max(projected, default=0)}

    reports = []
    async with open_backend("official") as clo, open_backend("rust") as rust, open_backend("rust") as inspector:
        assert (await json_result(clo, "clo_status"))["bridge"] == "up"
        original = (await json_result(clo, "scene_info"))["project_path"]
        backends = (("official", clo), ("rust", rust))
        try:
            outputs = {}
            original_rust_pose = None
            for writer, session in backends:
                await json_result(session, "scene_clear", {"keep_avatar": False})
                for name, points in zip(names, outlines, strict=True):
                    await json_result(session, "pattern_create", {"name": name, "points": points})
                assert (await json_result(session, "sew", {
                    "pattern_a": "Front", "line_a": 0, "pattern_b": "后片", "line_b": 0,
                    "dir_a": True, "dir_b": False,
                }))["ok"]
                assert (await json_result(session, "simulate", {"steps": 15, "particle_distance": 20}))["ok"]
                if writer == "rust":
                    original_rust_pose = await pose(rust, "source-rust-pose")
                for suffix in ("zprj", "zpac"):
                    path = root / f"pose-{writer}.{suffix}"
                    assert await json_result(session, "scene_save", {"path": str(path), "thumbnail": False}) == {
                        "path": str(path)}
                    outputs[writer, suffix] = path
                checkpoint = await json_result(session, "checkpoint_save", {"name": f"pose-{writer}"})
                path = Path(checkpoint["path"])
                assert path.suffix == ".zprj" and path.is_file()
                outputs[writer, "checkpoint"] = path

            for (writer, kind), path in outputs.items():
                await json_result(inspector, "scene_open", {"path": str(path)})
                expected = await pose(inspector, f"{writer}-{kind}-packed")
                if writer == "rust":
                    assert delta(original_rust_pose, expected) < 0.0001
                for reader, session in backends:
                    label = f"{writer}-{kind}-{reader}"
                    await json_result(session, "scene_clear", {"keep_avatar": False})
                    if kind == "checkpoint":
                        opened = await json_result(session, "checkpoint_restore", {"name": f"pose-{writer}"})
                    else:
                        opened = await json_result(session, "scene_open", {"path": str(path)})
                    assert opened["pattern_count"] == 2
                    assert (await json_result(session, "seams_list"))["count"] == 1
                    if reader == "official":
                        native_capture = root / f"{label}-captured.zpac"
                        await json_result(clo, "scene_save", {"path": str(native_capture)})
                        await json_result(inspector, "scene_open", {"path": str(native_capture)})
                        observed = await pose(inspector, f"{label}-loaded")
                    else:
                        observed = await pose(rust, f"{label}-loaded")
                    report = {"writer": writer, "format": kind, "reader": reader,
                              "vertices": sum(len(mesh["positions"]) for mesh in observed)}
                    same_rest_mesh = all(a["triangles"] == b["triangles"] and a["texcoords"] == b["texcoords"]
                                         for a, b in zip(expected, observed, strict=True))
                    if reader == "rust" or same_rest_mesh:
                        error = delta(expected, observed)
                        assert error < 0.001, (label, error)
                        report["position_error_mm"] = error
                    else:
                        surface = compare_surface(expected, observed)
                        assert surface["inside_vertices"] >= 0.95 * report["vertices"], (label, surface)
                        assert surface["inside_position_error_mm"] < 0.01, (label, surface)
                        assert surface["outside_rest_error_mm"] < 0.01, (label, surface)
                        report.update(surface)
                    if writer == "rust" and kind == "zprj":
                        result = await json_result(session, "simulate", {"steps": 10})
                        assert result["ok"], (label, result)
                        if reader == "official":
                            resumed_file = root / f"{label}-resumed.zpac"
                            await json_result(clo, "scene_save", {"path": str(resumed_file)})
                            await json_result(inspector, "scene_open", {"path": str(resumed_file)})
                            resumed = await pose(inspector, f"{label}-resumed")
                        else:
                            assert result["stitch_count"] > 0
                            resumed = await pose(rust, f"{label}-resumed")
                        report["resumed_displacement_mm"] = delta(observed, resumed)
                        assert report["resumed_displacement_mm"] > 0.001
                    reports.append(report)
        finally:
            if original not in (None, "", "NULL"):
                await json_result(clo, "scene_open", {"path": original})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    print(json.dumps({"cases": reports, "result": "pass"}))


async def compare_live_garment_to_project():
    source = Path(os.environ["CLO_SCENE_OPEN_GARMENT"])
    root = Path(os.environ["CLO_MCP_STATE"])
    assert source.is_file() and root.is_dir()
    assert not os.environ.get("CLO_RUST_CHECKPOINT_DIR")
    cases = [
        ("replace", "scene_save", True, None),
        ("append", "scene_export", False, None),
        ("replace", "checkpoint_save", False, None),
    ]
    if avatar := os.environ.get("CLO_ZPRJ_AVATAR_FIXTURE"):
        assert Path(avatar).is_file()
        cases.append(("replace", "scene_save", False, avatar))
    reports = []
    async with open_backend("official") as clo, open_backend("rust") as rust:
        assert (await json_result(clo, "clo_status"))["bridge"] == "up"
        original = (await json_result(clo, "scene_info"))["project_path"]
        backends = (("official", clo), ("rust", rust))
        try:
            for mode, tool, thumbnail, avatar in cases:
                label = f"{tool}-{mode}" + ("-avatar" if avatar else "")
                outputs = {}
                for writer, session in backends:
                    await json_result(session, "scene_clear", {"keep_avatar": False})
                    await json_result(session, "scene_open", {"path": str(source), "mode": mode})
                    if avatar:
                        await json_result(session, "scene_open", {"path": avatar})
                    if tool == "checkpoint_save":
                        result = await json_result(session, tool, {"name": f"{label}-{writer}"})
                        path = Path(result["path"])
                    else:
                        path = root / f"{label}-{writer}.zprj"
                        args = {"path": str(path)}
                        if tool == "scene_save":
                            args["thumbnail"] = thumbnail
                        assert await json_result(session, tool, args) == {"path": str(path)}
                    assert path.suffix == ".zprj" and path.is_file()
                    assert (await json_result(session, "scene_info"))["project_path"] == str(path)
                    outputs[writer] = path

                generated = outputs["rust"].read_bytes()
                start = struct.unpack_from("<I", generated, 32)[0]
                views = generated[:start].count(b"\x89PNG\r\n\x1a\n")
                assert views == (4 if thumbnail else 1), (label, views)
                if mode == "replace":
                    with zipfile.ZipFile(outputs["rust"]) as archive:
                        name = next(name for name in archive.namelist() if name.endswith(".zpac"))
                        assert archive.read(name) == source.read_bytes()
                        if avatar:
                            embedded = [name for name in archive.namelist() if name.endswith(".avt")]
                            assert len(embedded) == 1
                            assert archive.read(embedded[0]) == Path(avatar).read_bytes()

                reference = None
                for writer, path in outputs.items():
                    for reader, session in backends:
                        await json_result(session, "scene_clear", {"keep_avatar": False})
                        await json_result(session, "scene_open", {"path": str(path)})
                        readback = await read_composed_archive(session)
                        readback["avatars"] = (await json_result(session, "scene_info"))["avatars"]
                        assert len(readback["avatars"]) == int(avatar is not None)
                        assert readback["patterns"] and readback["materials"] > 0
                        if reference is None:
                            reference = readback
                        assert readback == reference, (label, writer, reader, {
                            key: (reference[key], value) for key, value in readback.items()
                            if reference[key] != value})
                reports.append({"tool": tool, "mode": mode, "cross_reopens": 4,
                                "panels": len(reference["patterns"]),
                                "colorways": reference["colorways"], "avatars": reference["avatars"],
                                "rust_pngs": views})
        finally:
            if original not in (None, "", "NULL"):
                await json_result(clo, "scene_open", {"path": original})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    print(json.dumps({"cases": reports, "result": "pass"}))


async def compare_live_texture_collisions():
    from PIL import Image

    source = Path(os.environ["CLO_TEXTURE_COLLISION_FIXTURE"])
    root = Path(os.environ["CLO_MCP_STATE"])
    names = ("colorway-base-0.png", "colorway-base-1.png")
    incoming = root / "same-filenames.zpac"
    assert source.is_file() and root.is_dir() and not incoming.exists()

    def rgba(data):
        with Image.open(BytesIO(data)) as image:
            return image.size, image.convert("RGBA").tobytes()

    def texture_members(file):
        with zipfile.ZipFile(file) as archive:
            return {name: archive.read(name) for name in archive.namelist()
                    if Path(name).name.startswith("colorway-base-") and name.endswith(".png")}

    def saved_images(file):
        if file.suffix == ".zpac":
            return texture_members(file)
        with zipfile.ZipFile(file) as project:
            member = next(name for name in project.namelist() if name.endswith(".zpac"))
            return texture_members(BytesIO(project.read(member)))

    def visible_images(file):
        model, binary = read_gltf_export(file)
        result = []
        for mesh in model["meshes"]:
            for primitive in mesh["primitives"]:
                pbr = model["materials"][primitive["material"]]["pbrMetallicRoughness"]
                texture = model["textures"][pbr["baseColorTexture"]["index"]]
                image = model["images"][texture["source"]]
                view = model["bufferViews"][image["bufferView"]]
                start = view.get("byteOffset", 0)
                result.append(rgba(binary[start:start + view["byteLength"]]))
        return result

    baseline = texture_members(source)
    assert set(baseline) == set(names)
    modified = {}
    for name, color in zip(names, [(255, 44, 7, 255), (8, 190, 48, 255)], strict=True):
        image = BytesIO()
        Image.new("RGBA", (16, 16), color).save(image, format="PNG")
        modified[name] = image.getvalue()
    output = BytesIO()
    with zipfile.ZipFile(source) as original, zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as changed:
        for member in original.namelist():
            changed.writestr(member, modified[member] if member in modified else original.read(member))
    raw = source.read_bytes()
    start, length = struct.unpack_from("<II", raw, 32)
    assert start >= 40 and start + length == len(raw)
    prefix = bytearray(raw[:start])
    struct.pack_into("<I", prefix, 36, len(output.getvalue()))
    incoming.write_bytes(prefix + output.getvalue())

    reference_project = root / "texture-collision-source.zprj"
    assert not reference_project.exists()
    reports = []
    async with open_backend("official") as clo, open_backend("rust") as rust:
        original = (await json_result(clo, "scene_info"))["project_path"]
        assert (await json_result(clo, "clo_status"))["bridge"] == "up"
        backends = (("official", clo), ("rust", rust))
        try:
            await json_result(clo, "scene_clear", {"keep_avatar": False})
            await json_result(clo, "scene_open", {"path": str(source)})
            await json_result(clo, "scene_save", {"path": str(reference_project), "thumbnail": False})
            assert set(saved_images(reference_project).values()) == set(baseline.values())
            for label, initial, appended, expected, extension in (
                ("same", source, source, baseline, ".zpac"),
                ("collision", source, incoming, modified, ".zpac"),
                ("project", reference_project, incoming, modified, ".zprj"),
            ):
                reads = 0
                outputs = {}
                for writer, session in backends:
                    await json_result(session, "scene_clear", {"keep_avatar": False})
                    await json_result(session, "scene_open", {"path": str(initial)})
                    await json_result(session, "scene_open", {"path": str(appended), "mode": "append"})
                    before = await read_composed_archive(session)
                    assert [panel["fabric_index"] for panel in before["patterns"]] == [0, 0, 1, 1]
                    target = root / f"{label}-{writer}{extension}"
                    tool = "scene_save" if extension == ".zprj" else "scene_export"
                    args = {"path": str(target)}
                    if tool == "scene_save":
                        args["thumbnail"] = False
                    assert await json_result(session, tool, args) == {"path": str(target)}
                    assert await read_composed_archive(session) == before
                    packed = saved_images(target)
                    assert set(packed.values()) == set(baseline.values()) | set(expected.values())
                    assert all(packed[name] == baseline[name] for name in names)
                    outputs[writer] = target

                reference = None
                for writer, file in outputs.items():
                    for reader, session in backends:
                        await json_result(session, "scene_open", {"path": str(file)})
                        reopened = await read_composed_archive(session)
                        if reference is None:
                            reference = reopened
                        assert reopened == reference, (label, writer, reader, {
                            key: (value, reference[key]) for key, value in reopened.items() if value != reference[key]})
                        for colorway, name in enumerate(names):
                            await composed_archive_api(session, "utility_api.SetCurrentColorwayIndex", colorway)
                            glb = root / f"{label}-{writer}-{reader}-{colorway}.glb"
                            await json_result(session, "scene_export", {"path": str(glb)})
                            expected_binding = [rgba(baseline[name])] * 2 + [rgba(expected[name])] * 2
                            assert visible_images(glb) == expected_binding, (label, writer, reader, colorway)
                            reads += 1
                reports.append({"case": label, "colorway_reads": reads, "file_hashes_exact": True})
        finally:
            if original not in ("NULL", "", None):
                await json_result(clo, "scene_open", {"path": original})
            else:
                await json_result(clo, "scene_clear", {"keep_avatar": False})
    print(json.dumps({"tool": "scene_export", "case": "colliding native texture filenames",
                      "tests": reports, "result": "pass"}))


async def verify_native_append():
    fixture = os.environ.get("CLO_NATIVE_APPEND_FIXTURE")
    assert fixture, "set CLO_NATIVE_APPEND_FIXTURE to a saved .zprj with an avatar and pose"
    project = str(Path(fixture).resolve(strict=True))
    with TemporaryDirectory() as scratch:
        folder = Path(scratch)
        async with open_backend("rust") as session:
            await inspect_contract(session)
            baseline = await json_result(session, "scene_open", {"path": project})
            assert baseline["native_import"]["saved_pose_loaded"] and baseline["avatars"]
            first = await json_result(session, "scene_info")
            assert first["fabric_count"] and first["seam_groups"]

            async def save(name):
                path = folder / f"{name}.clorust"
                await json_result(session, "scene_save", {"path": str(path)})
                return path, json.loads(path.read_text())["scene"]

            _, before = await save("baseline")
            reply = await json_result(session, "scene_open", {"path": project, "mode": "append"})
            assert reply["native_import"]["saved_pose_loaded"]
            appended = await json_result(session, "scene_info")
            assert appended["project"] == "baseline"
            for field in ("pattern_count", "fabric_count", "seam_groups"):
                assert appended[field] == 2 * first[field], (field, first[field], appended[field])
            assert appended["avatars"] == first["avatars"] * 2
            path, both = await save("combined")
            old = before["simulation"]["meshes"]
            meshes = both["simulation"]["meshes"]
            assert len(meshes) == len(old) * 2
            assert meshes[:len(old)] == old
            assert [mesh["id"] for mesh in meshes] == [panel["id"] for panel in both["patterns"]]
            for a, b in zip(old, meshes[len(old):], strict=True):
                assert a["id"] != b["id"]
                assert {k: v for k, v in a.items() if k != "id"} == {k: v for k, v in b.items() if k != "id"}
            ids = [panel["id"] for panel in both["patterns"]]
            assert len(ids) == len(set(ids))
            count = first["pattern_count"]
            remap = dict(zip(ids[:count], ids[count:], strict=True))
            original_groups = before["native_seam_groups"]
            kept, added = both["native_seam_groups"][:len(original_groups)], both["native_seam_groups"][len(original_groups):]
            assert kept == original_groups
            for a, b in zip(original_groups, added, strict=True):
                assert a["name"] == b["name"] and a["id"] != b["id"]
                for old_pair, new_pair in zip(a["segments"], b["segments"], strict=True):
                    for old, new in zip(old_pair, new_pair, strict=True):
                        assert {**old, "pattern_id": remap[old["pattern_id"]]} == new
            for a, b in zip(both["patterns"][:count], both["patterns"][count:], strict=True):
                assert a["name"] == b["name"]
                assert b["fabric_index"] == a["fabric_index"] + first["fabric_count"]
            await json_result(session, "scene_open", {"path": str(path)})
            stable = await json_result(session, "scene_info")
            corrupt_path = folder / "corrupt.zprj"
            corrupt_path.write_bytes(b"not a native project")
            invalid_append = await json_result(session, "scene_open", {"path": str(corrupt_path), "mode": "append"})
            assert invalid_append["ok"] and await json_result(session, "scene_info") == stable
            invalid_replace = await json_result(session, "scene_open", {"path": str(corrupt_path), "mode": "replace"})
            assert invalid_replace == {"ok": True, "path": str(corrupt_path), "pattern_count": 0, "avatars": []}
            assert (await json_result(session, "scene_info"))["project_path"] == str(corrupt_path)
            await json_result(session, "scene_clear", {"keep_avatar": False})
            empty_append = await json_result(session, "scene_open", {"path": project, "mode": "append"})
            assert empty_append["native_import"]["saved_pose_loaded"]
            assert empty_append["avatars"] == first["avatars"]
            blank_append = await json_result(session, "scene_info")
            assert blank_append["project_path"] == "NULL"
            assert blank_append["colorways"] == ["Default Colorway"]
            assert blank_append["current_colorway"] == 0
            assert [piece["fabric_index"] for piece in blank_append["patterns"]] == [
                piece["fabric_index"] + 1 for piece in first["patterns"]]
            blank_path, blank_state = await save("blank-appended")
            await json_result(session, "scene_open", {"path": str(blank_path)})
            _, blank_reopened = await save("blank-reopened")
            assert blank_reopened["simulation"] == blank_state["simulation"]
            await json_result(session, "scene_open", {"path": project})
            garment = folder / "copy.zpac"
            await json_result(session, "scene_export", {"path": str(garment)})
            garment_append = await json_result(session, "scene_open", {"path": str(garment), "mode": "append"})
            assert garment_append["pattern_count"] == count * 2
            assert garment_append["avatars"] == first["avatars"]
            assert garment_append["native_import"]["saved_pose_loaded"]

            async def import_project(fields, source=project):
                return await json_result(session, "clo_api_call", {
                    "name": "import_api.ImportZprj", "args": [
                        source, {"$type": "ApiTypes.ImportZPRJOption", "fields": fields},
                    ],
                })

            await json_result(session, "scene_open", {"path": str(path)})
            _, before_selective = await save("before-selective")
            no_components = await import_project({"bLoadGarment": False, "bLoadAvatar": False})
            assert no_components["value"] is True
            _, unchanged = await save("no-components")
            for field in ("patterns", "fabrics", "native_seam_groups", "simulation", "avatar", "additional_avatars"):
                assert unchanged[field] == before_selective[field], field

            code = f"opt = ApiTypes.ImportZPRJOption(); opt.bLoadGarment = False; result = import_api.ImportZprj({project!r}, opt)"
            assert (await json_result(session, "clo_run_python", {"code": code}))["value"] is True
            assert (await json_result(session, "scene_info"))["avatars"] == first["avatars"]
            _, avatar_only = await save("only-avatar")
            for field in ("patterns", "fabrics", "native_seam_groups", "simulation"):
                assert avatar_only[field] == before_selective[field], field

            await json_result(session, "scene_open", {"path": str(path)})
            assert (await import_project({"bLoadAvatar": False}))["value"] is True
            state = await json_result(session, "scene_info")
            assert state["pattern_count"] == count and state["avatars"] == first["avatars"] * 2
            _, garment_only = await save("only-garment")
            for field in ("avatar", "additional_avatars"):
                assert garment_only[field] == before_selective[field], field
            assert garment_only["simulation"]["meshes"] == before["simulation"]["meshes"]
            before_error = await json_result(session, "scene_info")
            rejected = await session.call_tool("clo_api_call", {
                "name": "import_api.ImportZprj", "args": [
                    str(corrupt_path), {"$type": "ApiTypes.ImportZPRJOption", "fields": {"bLoadGarment": False}},
                ],
            })
            assert rejected.isError
            assert await json_result(session, "scene_info") == before_error

            assert (await import_project({"bAppend": True, "bLoadGarment": False}))["value"] is True
            after_avatars = await json_result(session, "scene_info")
            assert after_avatars["pattern_count"] == count and after_avatars["avatars"] == first["avatars"] * 3
            assert (await import_project({"bAppend": True, "bLoadAvatar": False}))["value"] is True
            after_garment = await json_result(session, "scene_info")
            assert after_garment["pattern_count"] == count * 2 and after_garment["avatars"] == after_avatars["avatars"]
    print(json.dumps({"tool": "scene_open", "case": "append two saved native poses",
                      "patterns": appended["pattern_count"], "fabrics": appended["fabric_count"],
                      "seam_groups": appended["seam_groups"], "avatars": len(appended["avatars"]),
                      "pose_meshes_matched": len(meshes), "invalid_append_and_replacement_matched": True,
                      "empty_append_keeps_receiving_colorway_and_saved_pose": True, "garment_append_keeps_avatar": True,
                      "selective_avatar_only_kept_garment": True,
                      "selective_garment_only_kept_avatars": True, "select_neither_kept_scene": True,
                      "selective_append_and_corrupt_input": "pass"}))


async def verify_mixed_native_append():
    fixture = os.environ.get("CLO_NATIVE_APPEND_FIXTURE")
    assert fixture, "set CLO_NATIVE_APPEND_FIXTURE to a saved .zprj"
    request = {"path": str(Path(fixture).resolve(strict=True))}
    result = []
    with TemporaryDirectory() as scratch:
        folder = Path(scratch)
        async with open_backend("rust") as session:
            await inspect_contract(session)

            async def saved(name):
                path = folder / f"{name}.clorust"
                await json_result(session, "scene_save", {"path": str(path)})
                return path, json.loads(path.read_text())["scene"]

            await json_result(session, "scene_open", request)
            _, baseline = await saved("baseline")
            native = baseline["simulation"]["meshes"]
            assert native
            for prior_simulated in (False, True):
                await json_result(session, "scene_clear", {"keep_avatar": False})
                await json_result(session, "pattern_create", {
                    "name": "Local panel", "points": [[0, 0], [160, 0], [130, 110]],
                })
                if prior_simulated:
                    await json_result(session, "simulate", {
                        "steps": 1, "quality": 1, "particle_distance": 90,
                    })
                    _, prior = await saved("prior")
                imported = await json_result(session, "scene_open", {**request, "mode": "append"})
                assert imported["native_import"]["saved_pose_loaded"]
                combined, before = await saved("combined")
                before_meshes = before["simulation"]["meshes"]
                assert len(before_meshes) == len(native) + int(prior_simulated)
                obj = folder / "combined.obj"
                await json_result(session, "scene_export", {"path": str(obj)})
                assert sum(line.startswith("o ") for line in obj.read_text().splitlines()) == len(native) + 1
                await json_result(session, "scene_open", {"path": str(combined)})
                sim = await json_result(session, "simulate", {"steps": 0})
                assert sim["initialization"] == "mixed_saved_and_initialized_pose"
                _, after = await saved("continued")
                after_meshes = after["simulation"]["meshes"]
                assert len(after_meshes) == len(native) + 1
                assert after["simulation"]["steps"] == int(prior_simulated)
                for original, observed in zip(native, after_meshes[1:], strict=True):
                    assert observed["texcoords_units"] == "mm"
                    for key in ("positions", "triangles", "texcoords"):
                        assert original[key] == observed[key], (original["name"], key)
                if prior_simulated:
                    old = prior["simulation"]["meshes"][0]
                    for key in ("positions", "triangles", "texcoords", "velocities"):
                        assert old[key] == after_meshes[0][key], key
                result.append({"prior_simulated": prior_simulated, "meshes": len(after_meshes),
                               "inner_seam_groups": sim["native_inner_seam_groups"]})
    print(json.dumps({"tools": ["scene_open", "scene_export", "simulate"],
                      "case": "mixed native append", "results": result}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    comparisons = parser.add_mutually_exclusive_group()
    comparisons.add_argument("--verify-tool-contracts", action="store_const", dest="comparison",
                             const=verify_tool_contracts,
                             help="compare native and adapter tools/list without launching CLO")
    comparisons.add_argument("--verify-fabric-recolor-opacity", action="store_const", dest="comparison",
                             const=verify_fabric_recolor_opacity,
                             help="verify opacity map policy after recolor and archive reopen")
    comparisons.add_argument("--compare-live-fabric-edits", action="store_const", dest="comparison",
                             const=compare_live_fabric_edits,
                             help="compare native fabric assignment, colors, textures and failure-state readback")
    comparisons.add_argument("--verify-fabric-assignment-workflow", action="store_const", dest="comparison",
                             const=verify_fabric_assignment_workflow,
                             help="add and assign fabrics separately while retaining a simulated pose")
    comparisons.add_argument("--verify-fabric-texture-clear", action="store_const", dest="comparison",
                             const=verify_fabric_texture_clear,
                             help="clear one fabric map and verify saved scenes and export")
    comparisons.add_argument("--compare-desktop-observation", action="store_const", dest="comparison",
                             const=compare_desktop_observation,
                             help="compare desktop state bounds, stale actions and screenshot payloads")
    comparisons.add_argument("--compare-control-guide", action="store_const", dest="comparison",
                             const=compare_control_guide,
                             help="compare control workflow guidance without a running CLO desktop")
    comparisons.add_argument("--compare-draft-option-semantics", action="store_const", dest="comparison",
                             const=compare_draft_option_semantics,
                             help="compare drafting fallbacks, ease and simulation parameter types")
    comparisons.add_argument("--compare-fabric-library", action="store_const", dest="comparison",
                             const=compare_fabric_library,
                        help="compare both live backends against the same CLO fabric library")
    comparisons.add_argument("--compare-avatar-library", action="store_const", dest="comparison",
                             const=compare_avatar_library,
                             help="compare both live backends against the same CLO avatar library")
    comparisons.add_argument("--compare-saved-fabric-list", action="store_const", dest="comparison",
                             const=compare_saved_fabric_list,
                             help="compare live fabric metadata against the same saved CLO project in Rust")
    comparisons.add_argument("--compare-native-fabric-list", action="store_const", dest="comparison",
                             const=compare_native_fabric_list,
                             help="compare fabric listing and selection through garment clear and reset")
    comparisons.add_argument("--compare-saved-pattern-settings", action="store_const", dest="comparison",
                             const=compare_saved_pattern_settings,
                             help="compare live pattern settings against the same saved CLO project in Rust")
    comparisons.add_argument("--compare-saved-pattern-outline", action="store_const", dest="comparison",
                             const=compare_saved_pattern_outline,
                             help="compare outer outline line descriptors for the same saved CLO project")
    comparisons.add_argument("--compare-saved-pattern-children", action="store_const", dest="comparison",
                             const=compare_saved_pattern_children,
                             help="also compare native child shapes and baselines in the saved project")
    comparisons.add_argument("--compare-saved-seams", action="store_const", dest="comparison",
                             const=compare_saved_seams,
                             help="compare ordered seam groups and panel membership in a saved CLO project")
    comparisons.add_argument("--compare-native-sewing", action="store_const", dest="comparison",
                             const=compare_native_sewing,
                             help="compare sewing replies, memberships and direction on a disposable native fixture")
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
    comparisons.add_argument("--compare-saved-avatar-cache", action="store_const", dest="comparison",
                             const=compare_saved_avatar_cache,
                             help="compare the same saved avatar measurement cache across backends")
    comparisons.add_argument("--compare-saved-snapshot-2d", action="store_const", dest="comparison",
                             const=compare_saved_snapshot_2d,
                             help="capture both backends' 2D layout and verify the PNG response")
    comparisons.add_argument("--compare-native-snapshot-views", action="store_const", dest="comparison",
                             const=compare_native_snapshot_views,
                             help="compare native 3D view order, cloth silhouettes and camera retention")
    comparisons.add_argument("--compare-strapped-draft-preview", action="store_const", dest="comparison",
                             const=compare_strapped_draft_preview,
                             help="compare offline strapless and strap garment draft previews")
    comparisons.add_argument("--compare-neckline-and-sleeve-preview",
                             action="store_const", dest="comparison",
                             const=compare_neckline_and_sleeve_preview,
                             help="compare neckline fallbacks and sleeve seam warnings")
    comparisons.add_argument("--compare-draft-material-preview", action="store_const", dest="comparison",
                             const=compare_draft_material_preview,
                             help="compare garment fabric aliases, color and preview validation")
    comparisons.add_argument("--compare-gathered-draft-preview", action="store_const", dest="comparison",
                             const=compare_gathered_draft_preview,
                             help="compare offline gathered and tiered garment draft previews")
    comparisons.add_argument("--verify-tiered-build-relaxation", action="store_const", dest="comparison",
                             const=verify_tiered_build_relaxation,
                             help="verify layer reset, solver continuation and saved drape after tiered builds")
    comparisons.add_argument("--verify-one-piece-dress-merge", action="store_const", dest="comparison",
                             const=verify_one_piece_dress_merge,
                             help="verify welded waist topology, remapped seams and reopened drape")
    comparisons.add_argument("--verify-build-snapshot-workflow", action="store_const", dest="comparison",
                             const=verify_build_snapshot_workflow,
                             help="verify build snapshot order, view aliases, images and error readback")
    comparisons.add_argument("--verify-scene-open-ui-state", action="store_const", dest="comparison",
                             const=verify_scene_open_ui_state,
                             help="verify UI token and selection readback across scene changes")
    comparisons.add_argument("--compare-native-scene-clear", action="store_const", dest="comparison",
                             const=compare_native_scene_clear,
                             help="compare cleared garment and empty project state on both backends")
    comparisons.add_argument("--verify-scene-clear-state", action="store_const", dest="comparison",
                             const=verify_scene_clear_state,
                             help="verify clear and fresh project state, stale UI and session history")
    comparisons.add_argument("--verify-garment-session-history", action="store_const", dest="comparison",
                             const=verify_garment_session_history,
                             help="verify last build history across scene changes and isolated MCP clients")
    comparisons.add_argument("--compare-curved-skirt-preview", action="store_const", dest="comparison",
                             const=compare_curved_skirt_preview,
                             help="compare offline circular and mermaid skirt draft previews")
    comparisons.add_argument("--verify-runtime-pattern-query", action="store_const", dest="comparison",
                             const=verify_runtime_pattern_query,
                             help="verify pattern discovery, calls, Python and handles against a saved .zprj")
    comparisons.add_argument("--compare-python-response-timeout", action="store_const", dest="comparison",
                             const=compare_python_response_timeout,
                             help="compare scene changes and recovery after an MCP Python response timeout")
    comparisons.add_argument("--verify-runtime-export-workflow", action="store_const", dest="comparison",
                             const=verify_runtime_export_workflow,
                             help="verify Rust discovery and calls for the snapshot and FBX workflow")
    comparisons.add_argument("--verify-checkpoint-workflow", action="store_const", dest="comparison",
                             const=verify_checkpoint_workflow,
                             help="verify Rust checkpoint ordering, restore, errors and optional native re-export")
    comparisons.add_argument("--verify-build-checkpoints", action="store_const", dest="comparison",
                             const=verify_build_checkpoints,
                             help="verify automatic pre-build checkpoints and replacement cleanup")
    comparisons.add_argument("--verify-appended-build-report", action="store_const", dest="comparison",
                             const=verify_appended_build_report,
                             help="verify public build sewing rows and append beside same-name pieces")
    comparisons.add_argument("--verify-appended-build-keeps-drape", action="store_const", dest="comparison",
                             const=verify_appended_build_keeps_drape,
                             help="verify builds append to an existing simulation without resetting settled cloth")
    comparisons.add_argument("--verify-calibrated-build-arrangement", action="store_const", dest="comparison",
                             const=verify_calibrated_build_arrangement,
                             help="verify build placement using saved avatar arrangement calibration")
    comparisons.add_argument("--verify-elastic-parameter-updates", action="store_const", dest="comparison",
                             const=verify_elastic_parameter_updates,
                             help="verify elastic parameter writes preserve enable flags and solver behavior")
    comparisons.add_argument("--verify-outline-edit-remaps", action="store_const", dest="comparison",
                             const=verify_outline_edit_remaps,
                             help="verify boundary edits preserve unaffected sewing and edge properties")
    comparisons.add_argument("--verify-symmetry-sewing", action="store_const", dest="comparison",
                             const=verify_symmetry_sewing,
                             help="verify symmetry duplicates native sewing and mirrored rest geometry")
    comparisons.add_argument("--compare-live-pattern-ops", action="store_const", dest="comparison",
                             const=compare_live_pattern_ops,
                             help="compare public pattern operations with native CLO and read back the affected state")
    comparisons.add_argument("--verify-display-edits-keep-drape", action="store_const", dest="comparison",
                             const=verify_display_edits_keep_drape,
                             help="verify layout, visibility and strength edits preserve saved drape")
    comparisons.add_argument("--verify-sew-keeps-drape", action="store_const", dest="comparison",
                             const=verify_sew_keeps_drape,
                             help="verify newly added seams solve from the saved drape")
    comparisons.add_argument("--compare-native-pattern-creation", action="store_const", dest="comparison",
                             const=compare_native_pattern_creation,
                             help="compare native pattern creation and arrangement states")
    comparisons.add_argument("--compare-native-pattern-point-edits", action="store_const", dest="comparison",
                             const=compare_native_pattern_point_edits,
                             help="compare point edits and post-edit pattern readback on live CLO and Rust")
    comparisons.add_argument("--compare-native-pattern-delete", action="store_const", dest="comparison",
                             const=compare_native_pattern_delete,
                             help="compare pattern deletion and scene readback using a live saved fixture")
    comparisons.add_argument("--verify-delete-keeps-drape", action="store_const", dest="comparison",
                             const=verify_delete_keeps_drape,
                             help="verify panel deletion removes only associated meshes and sewing")
    comparisons.add_argument("--verify-new-panels-keep-drape", action="store_const", dest="comparison",
                             const=verify_new_panels_keep_drape,
                             help="verify newly created or copied panels can join settled cloth")
    comparisons.add_argument("--verify-portable-append-keeps-drape", action="store_const", dest="comparison",
                             const=verify_portable_append_keeps_drape,
                             help="verify append remaps persisted panels, materials and simulation meshes")
    comparisons.add_argument("--verify-interchange-append-keeps-drape", action="store_const", dest="comparison",
                             const=verify_interchange_append_keeps_drape,
                             help="verify DXF and pattern JSON append preserve the receiving drape")
    comparisons.add_argument("--verify-geometry-edits-rebuild-one-panel", action="store_const", dest="comparison",
                             const=verify_geometry_edits_rebuild_one_panel,
                             help="verify geometric edits preserve other panels and their continuation")
    comparisons.add_argument("--compare-live-garment-workflow", action="store_const", dest="comparison",
                             const=compare_live_garment_workflow,
                             help="compare live build, rebuild, checkpoint and session history")
    comparisons.add_argument("--compare-live-avatar-editor-cache", action="store_const", dest="comparison",
                             const=compare_live_avatar_editor_cache,
                             help="compare an actual native Avatar Editor cache with Rust")
    comparisons.add_argument("--compare-live-avatar-measurements", action="store_const", dest="comparison",
                             const=compare_live_avatar_measurements,
                             help="compare live avatar mesh measurements and shared measurement cache behavior")
    comparisons.add_argument("--compare-live-avatar-workflow", action="store_const", dest="comparison",
                             const=compare_live_avatar_workflow,
                             help="compare native avatar loads, scene names, library and arrangement on both backends")
    comparisons.add_argument("--verify-avatar-changes-keep-drape", action="store_const", dest="comparison",
                             const=verify_avatar_changes_keep_drape,
                             help="verify avatar changes preserve cloth, with updated collision on resume")
    comparisons.add_argument("--verify-native-simulation-reference", action="store_const", dest="comparison",
                             const=verify_native_simulation_reference,
                             help="compare Rust simulation boundaries and arrangement with captured native results")
    comparisons.add_argument("--verify-simulation-reset-quality", action="store_const", dest="comparison",
                             const=verify_simulation_reset_quality,
                             help="verify reset keeps shared and persisted solver settings")
    comparisons.add_argument("--verify-simulation-remesh-keeps-pose", action="store_const", dest="comparison",
                             const=verify_simulation_remesh_keeps_pose,
                             help="verify new mesh vertices inherit current position and velocity")
    comparisons.add_argument("--compare-checkpoint-listing", action="store_const", dest="comparison",
                             const=compare_checkpoint_listing,
                             help="compare offline checkpoint list ordering and local timestamps")
    comparisons.add_argument("--compare-live-checkpoint-roundtrip", action="store_const", dest="comparison",
                             const=compare_live_checkpoint_roundtrip,
                             help="save, overwrite, list and restore native checkpoints across both live backends")
    comparisons.add_argument("--compare-shared-checkpoint-folder", action="store_const", dest="comparison",
                             const=compare_shared_checkpoint_folder,
                             help="compare a shared checkpoint listing and restore the native archive in Rust")
    comparisons.add_argument("--verify-reordered-native-archives", action="store_const", dest="comparison",
                             const=verify_reordered_native_archives,
                             help="verify the two native Boat colorway archives and saved poses")
    comparisons.add_argument("--verify-native-two-corner-archives", action="store_const", dest="comparison",
                             const=verify_native_two_corner_archives,
                             help="reopen and export two-corner native project and garment archives")
    comparisons.add_argument("--verify-native-curve-measurements", action="store_const", dest="comparison",
                             const=verify_native_curve_measurements,
                             help="compare curve lengths and bounds against native creation and archive captures")
    comparisons.add_argument("--verify-native-colorway-exports", action="store_const", dest="comparison",
                             const=verify_native_colorway_exports,
                             help="reopen all mesh formats exported from a native two-colorway archive")
    comparisons.add_argument("--verify-native-colorway-textures", action="store_const", dest="comparison",
                             const=verify_native_colorway_textures,
                             help="verify textured native exports against a Boat reference capture")
    comparisons.add_argument("--verify-native-colorway-normals", action="store_const", dest="comparison",
                             const=verify_native_colorway_normals,
                             help="compare exported normal maps with the Boat reference")
    comparisons.add_argument("--verify-native-texture-transforms", action="store_const", dest="comparison",
                             const=verify_native_texture_transforms,
                             help="compare planar texture rotations and offsets with Boat exports")
    comparisons.add_argument("--verify-native-colorway-refresh", action="store_const", dest="comparison",
                             const=verify_native_colorway_refresh,
                             help="verify colorway exports after native save and refresh")
    comparisons.add_argument("--compare-live-scene-open", action="store_const", dest="comparison",
                             const=compare_live_scene_open,
                             help="compare native append and replacement with scene and fabric readback")
    comparisons.add_argument("--compare-live-composed-zpac", action="store_const", dest="comparison",
                             const=compare_live_composed_zpac,
                             help="compare exported composed ZPACs and reopen them with both backends")
    comparisons.add_argument("--compare-live-composed-zprj", action="store_const", dest="comparison",
                             const=compare_live_composed_zprj,
                             help="verify composed project saves and checkpoints with both backends")
    comparisons.add_argument("--compare-live-authored-projects", action="store_const", dest="comparison",
                             const=compare_live_authored_projects,
                             help="write and cross-open native projects created from editable MCP panels")
    comparisons.add_argument("--compare-live-authored-curves", action="store_const", dest="comparison",
                             const=compare_live_authored_curves,
                             help="cross-open ZPRJ and ZPAC containing newly authored editable curves")
    comparisons.add_argument("--compare-live-authored-seams", action="store_const", dest="comparison",
                             const=compare_live_authored_seams,
                             help="write fresh seams to native archives and verify both readers and stitch constraints")
    comparisons.add_argument("--compare-live-authored-poses", action="store_const", dest="comparison",
                             const=compare_live_authored_poses,
                             help="cross-open solved ZPRJ, ZPAC and checkpoints and resume the saved drape")
    comparisons.add_argument("--compare-live-garment-to-project", action="store_const", dest="comparison",
                             const=compare_live_garment_to_project,
                             help="cross-open ZPRJ saves and checkpoints after importing only a ZPAC")
    comparisons.add_argument("--compare-live-texture-collisions", action="store_const", dest="comparison",
                             const=compare_live_texture_collisions,
                             help="verify conflicting texture payloads and their colorway bindings")
    comparisons.add_argument("--verify-native-append", action="store_const", dest="comparison",
                             const=verify_native_append,
                             help="verify pose, avatar, fabric and sew group readback on Rust native append")
    comparisons.add_argument("--verify-mixed-native-append", action="store_const", dest="comparison",
                             const=verify_mixed_native_append,
                             help="verify mixed procedural and native mesh state through append and simulation")
    args = parser.parse_args()
    if (args.comparison not in (None, verify_fabric_assignment_workflow, verify_fabric_recolor_opacity, verify_fabric_texture_clear,
                                verify_runtime_pattern_query, verify_runtime_export_workflow, verify_checkpoint_workflow,
                                verify_native_append, verify_mixed_native_append, verify_reordered_native_archives,
                                verify_native_colorway_exports, verify_native_colorway_textures, verify_native_two_corner_archives,
                                verify_native_curve_measurements,
                                verify_native_colorway_normals, verify_native_colorway_refresh,
                                verify_native_texture_transforms,
                                verify_tiered_build_relaxation,
                                verify_one_piece_dress_merge, verify_build_snapshot_workflow,
                                verify_scene_open_ui_state, verify_scene_clear_state, verify_garment_session_history,
                                verify_build_checkpoints,
                                verify_appended_build_report, verify_calibrated_build_arrangement,
                                verify_outline_edit_remaps, verify_elastic_parameter_updates, verify_symmetry_sewing,
                                verify_display_edits_keep_drape, verify_sew_keeps_drape,
                                verify_delete_keeps_drape, verify_new_panels_keep_drape,
                                verify_geometry_edits_rebuild_one_panel, verify_portable_append_keeps_drape,
                                verify_interchange_append_keeps_drape, verify_appended_build_keeps_drape,
                                verify_avatar_changes_keep_drape, verify_simulation_reset_quality,
                                verify_native_simulation_reference,
                                verify_simulation_remesh_keeps_pose)
            and not os.environ.get("CLO_MCP_OFFICIAL_COMMAND")
            and not (args.comparison == compare_saved_arrangements and os.environ.get("CLO_ARRANGEMENT_REFERENCE"))):
        parser.error("live comparisons require CLO_MCP_OFFICIAL_COMMAND")
    asyncio.run(main(args.comparison))
