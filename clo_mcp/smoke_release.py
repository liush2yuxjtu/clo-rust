"""Exercise the published native runtime over a real MCP stdio connection."""
import asyncio
import base64
import os
import sys
import tempfile
from pathlib import Path

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

ROOT = Path(__file__).resolve().parents[1]


async def main():
    with tempfile.TemporaryDirectory() as temp:
        env = {**os.environ, "XDG_CACHE_HOME": temp, "CLO_RUST_OUTPUT_DIR": temp}
        env.pop("CLO_RUST_BINARY", None)
        params = StdioServerParameters(
            command=sys.executable, args=[str(ROOT / "clo_mcp" / "stdio.py")], env=env)
        async with stdio_client(params) as streams, ClientSession(*streams) as session:
            await session.initialize()
            tools = await session.list_tools()
            assert len(tools.tools) == 11

            async def call(name, **arguments):
                result = await session.call_tool(name, arguments)
                assert not result.isError, result.content
                assert result.structuredContent is not None
                return result.structuredContent

            cold = await call("prewarm_runtime")
            warm = await call("prewarm_runtime")
            assert cold["source"] == "github_release" and warm["source"] == "cache"
            assert cold["sha256"] == warm["sha256"] and warm["version"].startswith("pattern-native ")
            seam = await call("verify_seam", project_json=str(ROOT / "samples" / "Carlton外层.pattern.json"),
                              include_constraints=True)
            assert seam["audit"]["declared_seams"] == 66 and seam["constraint_count"] == 1983
            frame = Path(temp) / "frame.png"
            frame.write_bytes(base64.b64decode(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jE2sAAAAASUVORK5CYII="
            ))
            replay = await call("physics_replay", source="smoke fixture",
                                frames=[{"time_s": t, "image": str(frame)} for t in (0.0, 0.25)])
            assert replay["frames"] == 2 and Path(replay["report"]).is_file()
            print("Verified release download, checksum, cache hit and Carlton seam audit over MCP")


if __name__ == "__main__":
    asyncio.run(main())
