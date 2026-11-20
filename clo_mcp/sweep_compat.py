"""Call all 48 public MCP tools on an isolated synthetic Rust scene."""

import asyncio
import base64
import json
from pathlib import Path
from tempfile import TemporaryDirectory

from .compat import CONTRACT
from .smoke_compat import inspect_contract, open_backend


def data(result):
    if result.structuredContent:
        return result.structuredContent
    for block in result.content:
        if block.type == "text":
            try:
                return json.loads(block.text)
            except ValueError:
                pass
    return {}


async def main():
    expected = {x["name"] for x in json.loads(CONTRACT.read_text())}
    outcomes = {}
    async with open_backend("rust", environment={
        "CLO_RUST_AVATAR_DIR": "",
        "CLO_RUST_FABRIC_DIR": "",
    }) as session:
        await inspect_contract(session)

        async def call(name, /, **args):
            result = await session.call_tool(name, args)
            outcomes[name] = {
                "status": "ERROR" if result.isError else "OK",
                "content_types": [block.type for block in result.content],
            }
            if result.isError:
                outcomes[name]["error"] = [
                    block.text for block in result.content if block.type == "text"
                ]
            return result

        with TemporaryDirectory(prefix="clo-mcp-sweep-") as directory:
            root = Path(directory)
            await call("clo_status")
            await call("clo_launch")
            await call("clo_control_guide")
            await call("clo_api_catalog", module="pattern_api", limit=2)
            await call("clo_api_search", query="GetPatternCount")
            await call("clo_api_describe", name="pattern_api.GetPatternCount")
            await call("clo_api_doc", function_name="pattern_api.GetPatternCount")
            await call("clo_api_call", name="pattern_api.GetPatternCount")
            await call("clo_api_release", handles=[])
            await call("clo_run_python", code="pattern_api.GetPatternCount()")
            await call("scene_info")
            await call("scene_clear", keep_avatar=False)
            await call("avatar_list")
            await call("avatar_load", name_or_path="mia")
            await call("avatar_measurements")
            await call("arrangement_points")
            await call("pattern_create", name="smoke-front", points=[[0, 0], [160, 0], [80, 100]])
            await call("pattern_create", name="smoke-back", points=[[0, 0], [160, 0], [80, 100]])
            await call("pattern_list", detail=True)
            await call("pattern_info", pattern="smoke-front")
            await call("pattern_set_arrangement", pattern="smoke-front", point="Body_Front_Center_3")
            await call("pattern_edit_points", pattern="smoke-front", moves=[{"point": 0, "dx": 1}])
            await call("pattern_ops", pattern="smoke-front", op="rename", args={"name": "smoke-edit"})
            await call("sew", pattern_a="smoke-edit", line_a=0, pattern_b="smoke-back", line_b=0)
            await call("seams_list")
            await call("fabric_library")
            await call("fabric_apply", fabric="cotton", patterns=["smoke-edit"])
            await call("fabric_list")
            await call("fabric_set_color", fabric_index=0, color="#234567")
            image = await call("snapshot_2d")
            pictures = [block for block in image.content if block.type == "image"]
            if pictures:
                (root / "swatch.png").write_bytes(base64.b64decode(pictures[0].data))
            await call("fabric_set_texture", fabric_index=0, image_path=str(root / "swatch.png"))
            await call("simulate", steps=3, particle_distance=30)
            await call("snapshot", views=["front"])
            await call("checkpoint_save", name="before-edit")
            await call("checkpoint_list")
            await call("pattern_delete", patterns=["smoke-back"])
            await call("checkpoint_restore", name="before-edit")
            await call("scene_save", path=str(root / "cloth.json"), thumbnail=False)
            await call("scene_open", path=str(root / "cloth.json"), mode="replace")
            await call("scene_export", path=str(root / "cloth.obj"))
            await call("simulation_reset")
            spec = {"type": "tshirt"}
            await call("garment_plan_preview", spec=spec)
            await call("garment_build", spec=spec, simulate_steps=0, snapshots=False)
            await call("garment_last_spec")
            await call("garment_rebuild", changes={"fit": "loose"}, simulate_steps=0, snapshots=False)
            state = data(await call("clo_ui_state"))
            await call("clo_ui_action", snapshot=state.get("snapshot", ""), action="press", element=2)
            await call("clo_ui_screenshot")
            await call("clo_stop_bridge")
    missing = sorted(expected - outcomes.keys())
    unexpected = sorted(outcomes.keys() - expected)
    errors = sorted(name for name, result in outcomes.items() if result["status"] == "ERROR")
    print(json.dumps({
        "backend": "rust",
        "check": "call_smoke",
        "expected_count": len(expected),
        "called_count": len(outcomes),
        "ok_count": len(outcomes) - len(errors),
        "missing": missing,
        "unexpected": unexpected,
        "errors": errors,
        "tools": outcomes,
    }, indent=2, ensure_ascii=False))
    return int(bool(missing or unexpected or errors))


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
