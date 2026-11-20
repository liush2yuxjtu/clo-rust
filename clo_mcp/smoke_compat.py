"""Exercise the common MCP entry point on Rust and, when configured, CLO 3D."""

import argparse
import asyncio
import base64
import hashlib
import json
import os
import re
import shutil
import struct
import sys
from collections import Counter
from contextlib import asynccontextmanager
from datetime import datetime
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
            before_selection_error = (await json_result(rust, "fabric_list"))["fabrics"]
            bad_selection = await rust.call_tool("fabric_apply", {
                "fabric": "cotton", "patterns": ["0", "missing-panel"],
            })
            assert bad_selection.isError and await assignments() == [new_index, back_index]
            after_selection_error = (await json_result(rust, "fabric_list"))["fabrics"]
            assert after_selection_error[:-1] == before_selection_error
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

            for label, options, expected_color in (
                ("invalid-color", {"color": "#gggggg"}, [0.85, 0.85, 0.85]),
                ("invalid-texture", {"color": "#246", "texture_image": str(root / "missing.png")},
                 [34 / 255, 68 / 255, 102 / 255]),
            ):
                before = (await json_result(rust, "fabric_list"))["fabrics"]
                result = await rust.call_tool("fabric_apply", {
                    "fabric": "cotton", "patterns": ["Back"], **options,
                })
                assert result.isError, label
                after = (await json_result(rust, "fabric_list"))["fabrics"]
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
                readback = await json_result(rust, "fabric_list")
                assert readback["fabrics"][applied["fabric"]["index"]]["name"] == applied["fabric"]["name"]
                library_report = {"native_filenames": listing["fabrics"],
                                  "native_keyword_file": canonical.name,
                                  "unicode_filter": filtered["fabrics"]}
    print(json.dumps({"tools": ["fabric_apply", "clo_api_call", "clo_run_python", "simulate", "scene_export"],
                      "case": "add fabric, assign one panel, preserve drape", "add_kept_assignments": True,
                      "assigned_indices": [new_index, back_index], "retained_step": 3,
                      "numeric_reference_preceded_name": True,
                      "selection_error_retained_fabric": True,
                      "color_error_retained_assignment": True,
                      "texture_error_retained_color": True,
                      "invalid_assignment_preserved_scene": True, "glb_materials": material_names}
                     | library_report, ensure_ascii=False))


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
                return (await json_result(rust, "fabric_list"))["fabrics"]

            async def maps(index):
                return next(fabric for fabric in await materials() if fabric["index"] == index)["info"]["texture_maps"]

            async def set_map(index, kind):
                await json_result(rust, "fabric_set_texture", {"fabric_index": index, "map": kind, "image_path": str(image)})

            async def check_faces(index, rgb):
                fabric = next(item for item in await materials() if item["index"] == index)
                rgba = rgb + [1.0]
                for row in fabric["info"]["material"].get("face_colors", []):
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
                for kind in ("normal", "opacity"):
                    await set_map(index, kind)
                await recolor(index)
            opaque, sheer = indices
            await recolor(sheer, " navy gold ", [0.83, 0.68, 0.22])
            await recolor(sheer, "#246")
            before_invalid = await materials()
            for index, color in ((opaque, "#x24"), (9001, "#246")):
                rejected = await rust.call_tool("fabric_set_color", {"fabric_index": index, "color": color})
                assert rejected.isError
            assert await materials() == before_invalid
            portable = root / "recolored.clorust"
            await json_result(rust, "scene_save", {"path": str(portable)})
            await json_result(rust, "scene_open", {"path": str(portable)})
            assert await maps(opaque) == {"normal": str(image)}
            assert await maps(sheer) == {"normal": str(image), "opacity": str(image)}
            for index in (opaque, sheer):
                await check_faces(index, expected_rgb)

            native = {}
            for label, fixture_key in (("opaque", "CLO_RECOLOR_OPAQUE_FIXTURE"), ("sheer", "CLO_RECOLOR_SHEER_FIXTURE")):
                if not (fixture := os.environ.get(fixture_key)):
                    continue
                await json_result(rust, "scene_open", {"path": fixture})
                fabric = next(item for item in await materials() if "opacity" in (item["info"]["texture_maps"] or {}))
                index = fabric["index"]
                before = await maps(index)
                if label == "opaque":
                    await set_map(index, "opacity")
                await recolor(index)
                live = await maps(index)
                expected = {key: value for key, value in before.items() if key != "opacity" or label == "sheer"}
                assert live == {key: value for key, value in expected.items() if key != "base"}
                path = root / (label + Path(fixture).suffix)
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                await json_result(rust, "scene_open", {"path": str(path)})
                assert await maps(index) == expected
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
    print(json.dumps({"tool": "fabric_set_color", "case": "recolor opacity policy", "portable_reopen": True,
                      "opaque_map_cleared": True, "sheer_map_preserved": True, "native": native,
                      "face_rgba_readback": True, "invalid_recolor_kept_materials": True}))


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
                materials = (await json_result(rust, "fabric_list"))["fabrics"]
                return next(fabric for fabric in materials if fabric["index"] == index)["info"]["texture_maps"]

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
            for patch, expected_current in (
                ({"map": "unknown"}, index),
                ({"image_path": str(root / "missing.png")}, index),
                ({"fabric_index": 9001}, other),
            ):
                await json_result(rust, "clo_api_call", {
                    "name": "fabric_api.SetCurrentFabricIndex", "args": [other],
                })
                assert (await rust.call_tool("fabric_set_texture", {
                    "fabric_index": index, "image_path": "", "map": "normal", **patch,
                })).isError
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
                "name": "fabric_api.SetNormalMapImageGivenFilePath", "args": [""],
            })
            assert await maps(index) == {}

            native = None
            if fixture := os.environ.get("CLO_TEXTURE_CLEAR_FIXTURE"):
                await json_result(rust, "scene_open", {"path": fixture})
                materials = (await json_result(rust, "fabric_list"))["fabrics"]
                target = next(fabric for fabric in materials if {"base", "normal"} <= fabric["info"]["texture_maps"].keys())
                index = target["index"]
                before = await maps(index)
                await texture(index, "base")
                path = root / ("native-cleared" + Path(fixture).suffix)
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                await json_result(rust, "scene_open", {"path": str(path)})
                remaining = await maps(index)
                assert remaining == {key: path for key, path in before.items() if key != "base"}
                native = {"suffix": path.suffix, "unchanged_maps": sorted(remaining)}
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
        assert refreshed["source"] == "rust_mesh_estimate"
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
    async def capture(session):
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
        return scene, dimensions

    async with open_backend("official") as session:
        await inspect_contract(session)
        scene, official = await capture(session)
    assert scene["pattern_count"] > 0
    project = Path(scene["project_path"])
    assert project.is_file() and project.suffix.lower() == ".zprj", project
    async with open_backend("rust") as session:
        await inspect_contract(session)
        await json_result(session, "scene_open", {"path": str(project)})
        rust_scene, rust = await capture(session)
    assert scene["patterns"] == rust_scene["patterns"]
    print(json.dumps({
        "tool": "snapshot_2d", "pattern_count": scene["pattern_count"],
        "dimensions": {"official": official, "rust": rust}, "scene_readback": "pass",
    }))


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
                      "append_keeps_selection": True, "portable_restore_clears_selection": True,
                      "failed_load_preserves_snapshot": True, "native_restore_clears_selection": native}))


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
            for key in ("project_path", "avatars", "fabrics", "colorways"):
                assert kept[key] == before[key], key
            assert await current_fabric() == index

            await json_result(rust, "scene_open", {"path": path})
            action = await observed_action()
            assert await json_result(rust, "scene_clear", {"keep_avatar": False}) == {"pattern_count": 0, "avatars": []}
            await stale(action)
            empty = await json_result(rust, "scene_info")
            assert empty["project"] == "Untitled" and empty["project_path"] == ""
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
            invalid_snapshot = await rust.call_tool("snapshot", {"views": ["front", "unknown view"]})
            assert invalid_snapshot.isError
            assert await thumbnail("after-invalid") == oblique

            before = sorted((output / "snapshots").glob("*.png"))
            invalid = await rust.call_tool("garment_build", {
                "spec": spec | {"length": "cropped", "color": "navy"},
                "simulate_steps": 0, "views": ["front", "unknown view"],
            })
            assert invalid.isError and "unknown snapshot view" in invalid.content[0].text
            assert sorted((output / "snapshots").glob("*.png")) == before
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

            snapshot_folder = output / "snapshots"
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
                      "thumbnail_matches_last_3d_view": True, "2d_and_errors_keep_camera": True}))


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
        assert await json_result(session, "clo_run_python", {"code": "pattern_api.GetPatternCount()"}) == {
            "value": count, "stdout": "",
        }
        assert await json_result(session, "clo_run_python", {
            "code": "print(pattern_api.GetPatternCount()); result = pattern_api.GetPatternPieceName(0)",
        }) == {"value": first, "stdout": f"{count}\n"}
        handle_type = "ApiTypes.ImportExportOption"
        handle = (await json_result(session, "clo_api_call", {"name": handle_type}))["value"]
        assert handle["$handle"] and handle["type"] == "ImportExportOption"
        for released in (1, 0):
            assert await json_result(session, "clo_api_release", {"handles": [handle["$handle"]]}) == {
                "released": released,
            }
        for tool, args in (
            ("clo_api_catalog", {"module": "pattern_api", "offset": -1}),
            ("clo_api_search", {"query": "GetPatternCount", "limit": 0}),
            ("clo_api_call", {"name": "pattern_api.GetPatternCount", "args": [42]}),
            ("clo_api_call", {"name": "pattern_api.GetPatternIndex",
                              "args": [{"$handle": handle["$handle"]}]}),
            ("clo_run_python", {"code": "raise ValueError('verification control')"}),
        ):
            assert (await session.call_tool(tool, args)).isError, (tool, args)
        after = await json_result(session, "scene_info")
        for key in ("project_path", "patterns", "pattern_count", "avatars", "fabric_count", "seam_groups"):
            assert after[key] == scene[key], key
        return {"apis": findings, "pattern_count": count, "first_pattern": first,
                "index_lookup": {"name": selected, "index": index}, "catalog_pages": 2,
                "handle_release_counts": [1, 0], "errors_rejected": 5, "scene_unchanged": True}

    async with open_backend("rust") as rust:
        await inspect_contract(rust)
        await json_result(rust, "scene_open", {"path": str(project)})
        observations = await observe(rust, await json_result(rust, "scene_info"))
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
                    assert await observe(clo, scene) == observations
                    reference = {"status": "PASS"}
    print(json.dumps({"case": "read-only pattern API workflow", "rust": observations, "official": reference}))
    if reference["status"] == "BLOCKED":
        raise SystemExit(2)


async def verify_runtime_export_workflow():
    names = (
        "export_api.ExportFBX", "export_api.ExportSnapshot2D",
        "export_api.ExportThumbnail3D", "utility_api.GetCLOAssetFolderPath",
    )
    async with open_backend("rust") as session:
        await inspect_contract(session)
        for name in names:
            module, short = name.split(".")
            request = {"module": module, "query": short, "limit": 1}
            expected = await json_result(session, "clo_api_catalog", request)
            assert expected["total"] == 1 and expected["entries"][0]["name"] == name
            assert expected["next_offset"] is None
            search = await json_result(session, "clo_api_search", {"query": short.lower(), "limit": 1})
            assert json.loads(search["result"])["entries"] == expected["entries"]
            describe = await json_result(session, "clo_api_describe", {"name": name})
            for query in (name, short):
                doc = await json_result(session, "clo_api_doc", {"function_name": query})
                assert json.loads(doc["result"]) == describe
            assert describe["name"] == name and describe["doc"]
        request = {"module": "export_api", "query": "Export", "limit": 1}
        first = await json_result(session, "clo_api_catalog", request)
        second = await json_result(session, "clo_api_catalog", {**request, "offset": first["next_offset"]})
        assert first["next_offset"] == 1 and first["entries"] != second["entries"]

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
        with TemporaryDirectory() as directory:
            folder = Path(directory)
            layout = str(folder / "layout.png")
            assert await invoke(names[1], [layout, 0]) == [layout]
            capture = await successful(session, "snapshot_2d")
            image, = [block for block in capture.content if block.type == "image"]
            assert base64.b64decode(image.data, validate=True) == Path(layout).read_bytes()
            scripted_path = str(folder / "scripted.png")
            scripted = await json_result(session, "clo_run_python", {
                "code": f"result = export_api.ExportSnapshot2D({scripted_path!r}, 0)",
            })
            assert scripted["value"] == [scripted_path] and Path(scripted_path).read_bytes() == Path(layout).read_bytes()
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
            fbx = folder / "piece.fbx"
            assert await invoke(names[0], [str(fbx), {"$type": "ApiTypes.ImportExportOption"}]) == [str(fbx)]
            assert fbx.stat().st_size > 1000
            assert await json_result(session, "scene_info") == scene
            await json_result(session, "scene_open", {"path": str(fbx)})
            saved = folder / "reopened.clorust"
            await json_result(session, "scene_save", {"path": str(saved)})
            mesh, = json.loads(saved.read_text())["scene"]["imported_meshes"]
            assert len(mesh["positions"]) == 3 and len(mesh["triangles"]) == 1
    print(json.dumps({"workflow": "runtime export", "discovered_and_called": names,
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
            await json_result(session, "pattern_create", {"name": "after", "points": points})
            changed = await json_result(session, "scene_info")
            second = await json_result(session, "checkpoint_save")
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
            for path in fixtures:
                await json_result(rust, "scene_export", {"path": str(path)})
            await json_result(rust, "scene_clear", {"keep_avatar": False})
            await create("Existing", 320)
            await json_result(rust, "simulate", {"steps": 2, "particle_distance": 50})
            original_path, original = await save("original")
            results = []
            for path in fixtures:
                await json_result(rust, "scene_open", {"path": str(original_path)})
                opened = await json_result(rust, "scene_open", {"path": str(path), "mode": "append"})
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
                      "case": "append 2D interchange patterns", "formats": results,
                      "original_mesh_and_velocities": "unchanged", "continued_step": 3,
                      "invalid_seam_is_atomic": True}))


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
            right["fabrics"][0]["index"] = 12
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
            assert combined["fabrics"][0] == left["fabrics"][0]
            new_fabric = combined["fabrics"][1]
            assert new_fabric["color"] == [0.1, 0.16, 0.42] and new_fabric["index"] != left["fabrics"][0]["index"]
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
                      "continued_step": 4, "native_pose_meshes_preserved": native_count}))


async def verify_delete_keeps_drape():
    with TemporaryDirectory(prefix="clo-delete-drape-") as directory:
        root = Path(directory)
        async with open_backend("rust", {"CLO_RUST_AVATAR_DIR": "", "CLO_RUST_FABRIC_DIR": ""}) as rust:
            await inspect_contract(rust)

            async def save(name):
                path = root / f"{name}.clorust"
                await json_result(rust, "scene_save", {"path": str(path), "thumbnail": False})
                return path, json.loads(path.read_text())["scene"]

            names = ("Front", "Drop 1", "Drop 2", "Back")
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
            invalid = await rust.call_tool("pattern_delete", {"patterns": ["Drop 1", "missing"]})
            assert invalid.isError
            _, rejected = await save("rejected")
            assert rejected["patterns"] == before["patterns"]
            assert rejected["simulation"] == before["simulation"]
            assert rejected["seams"] == before["seams"]

            result = await json_result(rust, "pattern_delete", {"patterns": ["Drop 1", "2", "Drop 1"]})
            assert result == {"deleted": 2, "pattern_count": 2}
            path, kept = await save("edited")
            assert [p["name"] for p in kept["patterns"]] == ["Front", "Back"]
            assert kept["seams"] == before["seams"][:1]
            remaining = [mesh for mesh in before["simulation"]["meshes"] if mesh["name"] in ("Front", "Back")]
            assert kept["simulation"] == before["simulation"] | {"meshes": remaining}
            assert (await json_result(rust, "seams_list"))["count"] == 1
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
                      "invalid_batch_and_empty_scene": "pass", "native_meshes_preserved": native_count}))


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
            assert duplicate == {"ok": False, "seam_groups": 1}
            invalid = await json_result(rust, "sew", seam | {"line_a": 99})
            assert invalid == duplicate
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
            await operate("Front", "move_2d", {"dx": 80, "dy": 55})
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

            native_meshes = None
            if fixture := os.environ.get("CLO_DISPLAY_FIXTURE"):
                await json_result(rust, "scene_open", {"path": fixture})
                _, original = await save("native")
                native_meshes = len(original["simulation"]["meshes"])
                assert native_meshes > 0 and original["simulation"]["source"] == "native_saved_pose"
                await operate("0", "move_2d", {"x": 500, "y": 250})
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
                      "native_pose_meshes_preserved": native_meshes}))


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
                "name": "Panel", "points": [[0, 0], [140, 0], [58, 110]],
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
                "name": "pattern_api.SetParticleDistanceOfPatterns", "args": [0],
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

            for name, x in (("Front", 0), ("Back", 140)):
                await json_result(rust, "pattern_create", {
                    "name": name, "points": [[x, 0], [x + 100, 0], [x + 45, 70]],
                })
            await json_result(rust, "sew", {"pattern_a": "Front", "line_a": 0,
                                            "pattern_b": "Back", "line_b": 0})
            await api("SetSimulationQuality", 2)
            await json_result(rust, "simulate", {"steps": 0, "particle_distance": 50})
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
            async with open_backend("rust", environment) as fresh:
                await json_result(fresh, "scene_open", {"path": str(persisted)})
                await json_result(fresh, "simulation_reset")
                await json_result(fresh, "simulate", {"steps": 0})
                _, reopened = await save(fresh, "fresh-reset")
                assert reopened["simulation"] == after_public["simulation"]
    print(json.dumps({"tools": ["simulate", "simulation_reset", "clo_api_call", "scene_open"],
                      "case": "reset arrangement without changing solver quality",
                      "quality_api_to_mcp": 2, "quality_mcp_to_api_and_reopen": 1,
                      "arranged_mesh_restored": True, "unsupported_quality_preserves_setting": True}))


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
                assert len(opened["avatars"]) == 2 and opened["avatars"][0] == "luka"
                _, with_native = await save("native")
                assert with_native["simulation"] == continued["simulation"]
                native_append = "pass"
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
                assert loaded["path"] == direct["path"] == str(original)
                assert loaded["avatars"] == direct["avatars"] and loaded["rest_geometry"] == direct["rest_geometry"]
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
            invalid = await rust.call_tool("pattern_edit_points", {
                "pattern": "Front", "moves": [{"point": 0, "dx": 15}, {"point": 1, "dy": "invalid"}],
            })
            assert invalid.isError
            _, unchanged = await save("unchanged")
            assert unchanged["patterns"] == original["patterns"]
            assert unchanged["simulation"] == original["simulation"]
            results = []
            for tool, args in (
                ("pattern_edit_points", {"moves": [{"point": " 1 ", "x": "ignored", "dx": "40"},
                                                   {"point": 2.9, "x": "56.5", "y": "90"}]}),
                ("pattern_set_arrangement", {"point": "Back"}),
                ("pattern_ops", {"op": "flip", "args": {"horizontal": True}}),
                ("pattern_ops", {"op": "delete_point", "args": {"point": 1}}),
                ("pattern_ops", {"op": "particle_distance", "args": {"mm": 24}}),
            ):
                await json_result(rust, "scene_open", {"path": str(initial)})
                reply = await json_result(rust, tool, {"pattern": "Front"} | args)
                if tool == "pattern_edit_points":
                    assert reply["moved"] == [{"point": 1, "x": 140, "y": 0},
                                              {"point": 2, "x": 56.5, "y": 90}]
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
                      "zero_move_and_invalid_batch_preserved_pose": True, "api_returned_null": True}))


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
                if with_sewing:
                    full, partial = scene["native_seam_groups"][2:]
                    assert full["id"] == 3 and partial["id"] == 4
                    for copied, source in zip((full, partial), initial["native_seam_groups"], strict=True):
                        expected = json.loads(json.dumps(source["segments"]))
                        expected[0][0]["pattern_id"] = mirrored["id"]
                        assert copied["segments"] == expected
                    copied_seam, self_seam = scene["seams"][2:]
                    assert copied_seam["pattern_a"] == mirrored["id"] and copied_seam["native_group"] == full["id"]
                    assert copied_seam["pattern_b"] == b
                    assert self_seam["pattern_a"] == self_seam["pattern_b"] == mirrored["id"]
                else:
                    assert len(scene["seams"]) == len(scene["native_seam_groups"]) == 2
                await json_result(rust, "scene_open", {"path": str(path)})
                assert (await json_result(rust, "seams_list"))["count"] == (6 if with_sewing else 3)
                if with_sewing:
                    simulated = await json_result(rust, "simulate", {"steps": 0, "particle_distance": 50})
                    assert simulated["native_inner_seam_groups"] == 2
    print(json.dumps({"tools": ["pattern_ops", "seams_list", "scene_open", "simulate"],
                      "case": "symmetry with sewing", "with_sewing_false_groups": 3,
                      "with_sewing_true_groups": 6, "solver_inner_seam_groups": 2,
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
    print(json.dumps({"tools": ["pattern_ops", "clo_api_call", "simulate"],
                      "case": "elastic parameters preserve enable flags",
                      "constraints_disabled": off["elastic_constraint_count"],
                      "constraints_enabled_after_reopen": on["elastic_constraint_count"],
                      "disabled_edge_stayed_disabled": True}))


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
                ("delete_line", {"line": 1}, 2, 3),
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
                endpoint = group["native_segments"][0][0]
                assert endpoint["start_line"] == endpoint["end_line"] == line
                edited = json.loads(path.read_text())["scene"]
                seam, = edited["seams"]
                assert seam["line_a"] == line and seam["native_group"] == 1
                properties = edited["patterns"][0]["line_properties"]
                assert properties[str(line)]["elastic"]["total_length"] == 50
                assert properties["-1"]["elastic"]["strength"] == 0.5
                shirred = [int(n) for n, p in properties.items() if p.get("shirring", {}).get("on")]
                assert shirred == ([] if shirring is None else [shirring])
                invalid = await rust.call_tool("pattern_ops", {"pattern": "Front", "op": "delete_point",
                                                               "args": {"point": 99}})
                assert invalid.isError and await json_result(rust, "seams_list") == groups
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
    print(json.dumps({"tool": "checkpoint_list", "same_order_and_local_timestamps": listings["rust"]}))


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
            assert restored == direct
            info = await json_result(rust, "scene_info")
            unequal = sorted(key for key in scene.keys() | info.keys() if scene.get(key) != info.get(key))
            assert not unequal, [(key, str(scene.get(key))[:200], str(info.get(key))[:200]) for key in unequal]
            state = await saved_scene("restored")
            for field in ("patterns", "fabrics", "native_seam_groups", "simulation", "avatar"):
                assert state.get(field) == baseline.get(field), field

            (directory / "corrupt.zprj").write_text("not a CLO project")
            before_error = await json_result(rust, "scene_info")
            for name in ("missing", "corrupt"):
                assert (await rust.call_tool("checkpoint_restore", {"name": name})).isError
                assert await json_result(rust, "scene_info") == before_error

            await json_result(rust, "scene_clear", {"keep_avatar": False})
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
                      "corrupt_and_missing_preserve_scene": True, "portable_name_precedence": True}))


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
            for mode in ("append", "replace"):
                invalid = await session.call_tool("scene_open", {"path": str(corrupt_path), "mode": mode})
                assert invalid.isError
                assert await json_result(session, "scene_info") == stable
            await json_result(session, "scene_clear", {"keep_avatar": False})
            empty_append = await json_result(session, "scene_open", {"path": project, "mode": "append"})
            assert empty_append["native_import"]["saved_pose_loaded"]
            assert empty_append["avatars"] == first["avatars"]
            assert (await json_result(session, "scene_info"))["colorways"] == first["colorways"]
            garment = folder / "copy.zpac"
            await json_result(session, "scene_export", {"path": str(garment)})
            garment_append = await json_result(session, "scene_open", {"path": str(garment), "mode": "append"})
            assert garment_append["pattern_count"] == count * 2
            assert garment_append["avatars"] == first["avatars"]
            assert garment_append["native_import"]["saved_pose_loaded"]
    print(json.dumps({"tool": "scene_open", "case": "append two saved native poses",
                      "patterns": appended["pattern_count"], "fabrics": appended["fabric_count"],
                      "seam_groups": appended["seam_groups"], "avatars": len(appended["avatars"]),
                      "pose_meshes_matched": len(meshes), "corrupt_input_preserves_scene": True,
                      "empty_append_reexports_native": True, "garment_append_keeps_avatar": True}))


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
    comparisons.add_argument("--verify-display-edits-keep-drape", action="store_const", dest="comparison",
                             const=verify_display_edits_keep_drape,
                             help="verify layout, visibility and strength edits preserve saved drape")
    comparisons.add_argument("--verify-sew-keeps-drape", action="store_const", dest="comparison",
                             const=verify_sew_keeps_drape,
                             help="verify newly added seams solve from the saved drape")
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
    comparisons.add_argument("--verify-avatar-changes-keep-drape", action="store_const", dest="comparison",
                             const=verify_avatar_changes_keep_drape,
                             help="verify avatar changes preserve cloth, with updated collision on resume")
    comparisons.add_argument("--verify-simulation-reset-quality", action="store_const", dest="comparison",
                             const=verify_simulation_reset_quality,
                             help="verify reset keeps shared and persisted solver settings")
    comparisons.add_argument("--verify-simulation-remesh-keeps-pose", action="store_const", dest="comparison",
                             const=verify_simulation_remesh_keeps_pose,
                             help="verify new mesh vertices inherit current position and velocity")
    comparisons.add_argument("--compare-checkpoint-listing", action="store_const", dest="comparison",
                             const=compare_checkpoint_listing,
                             help="compare offline checkpoint list ordering and local timestamps")
    comparisons.add_argument("--compare-shared-checkpoint-folder", action="store_const", dest="comparison",
                             const=compare_shared_checkpoint_folder,
                             help="compare a shared checkpoint listing and restore the native archive in Rust")
    comparisons.add_argument("--verify-native-append", action="store_const", dest="comparison",
                             const=verify_native_append,
                             help="verify pose, avatar, fabric and sew group readback on Rust native append")
    comparisons.add_argument("--verify-mixed-native-append", action="store_const", dest="comparison",
                             const=verify_mixed_native_append,
                             help="verify mixed procedural and native mesh state through append and simulation")
    args = parser.parse_args()
    if (args.comparison not in (None, verify_fabric_assignment_workflow, verify_fabric_recolor_opacity, verify_fabric_texture_clear,
                                verify_runtime_pattern_query, verify_runtime_export_workflow, verify_checkpoint_workflow,
                                verify_native_append, verify_mixed_native_append, verify_tiered_build_relaxation,
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
                                verify_simulation_remesh_keeps_pose)
            and not os.environ.get("CLO_MCP_OFFICIAL_COMMAND")):
        parser.error("live comparisons require CLO_MCP_OFFICIAL_COMMAND")
    asyncio.run(main(args.comparison))
