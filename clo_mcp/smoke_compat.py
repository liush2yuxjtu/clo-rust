"""Exercise the common MCP entry point on Rust and, when configured, CLO 3D."""

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


async def check_gltf_material_export(session, directory, png):
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

    source.unlink()
    failed = await session.call_tool("scene_export", args)
    assert failed.isError and output.read_bytes() == raw


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
            await check_gltf_material_export(session, tmp, png)
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


async def main():
    await smoke_rust()
    if os.environ.get("CLO_MCP_OFFICIAL_COMMAND"):
        await smoke_official()


if __name__ == "__main__":
    asyncio.run(main())
