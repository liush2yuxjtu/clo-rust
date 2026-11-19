"""Exercise the common MCP entry point on Rust and, when configured, CLO 3D."""

import asyncio
import json
import os
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
