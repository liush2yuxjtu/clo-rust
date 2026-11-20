"""Select the official CLO or independent Rust backend for the shared tool interface."""

import asyncio
import base64
import json
import os
import sys
from contextlib import asynccontextmanager
from pathlib import Path

import jsonschema
from mcp import ClientSession, StdioServerParameters
from mcp import types
from mcp.client.stdio import stdio_client

from . import runtime

CONTRACT = Path(__file__).with_name("clo_tool_contract.json")
IMAGE_TOOLS = frozenset(("snapshot", "snapshot_2d", "garment_build", "garment_rebuild", "clo_ui_screenshot"))


class RustBackend:
    def __init__(self):
        self._process = None
        self._lock = asyncio.Lock()

    async def close(self):
        process, self._process = self._process, None
        if process and process.returncode is None:
            try:
                process.terminate()
            except ProcessLookupError:
                pass
            try:
                await asyncio.wait_for(process.wait(), 5)
            except TimeoutError:
                process.kill()
                await process.wait()

    async def call(self, name, arguments):
        async with self._lock:
            if self._process is None:
                binary, _ = await asyncio.to_thread(runtime.native_binary)
                self._process = await asyncio.create_subprocess_exec(
                    str(binary),
                    "mcp-compat",
                    stdin=asyncio.subprocess.PIPE,
                    stdout=asyncio.subprocess.PIPE,
                    limit=64 * 1024 * 1024,
                )
            request = json.dumps({"name": name, "args": arguments}, allow_nan=False).encode()
            try:
                self._process.stdin.write(request + b"\n")
                await self._process.stdin.drain()
                raw = await self._process.stdout.readline()
                if not raw:
                    raise RuntimeError("CLO Rust engine exited")
                response = json.loads(raw)
            except asyncio.CancelledError:
                await self.close()
                raise
            except (ValueError, OSError, RuntimeError) as exc:
                await self.close()
                raise RuntimeError("CLO Rust engine disconnected. Open the last saved project to recover the session.") from exc
            if "error" in response:
                raise ValueError(response["error"])
            return response["result"]


def official_parameters():
    raw = os.environ.get("CLO_MCP_OFFICIAL_COMMAND")
    command = json.loads(raw) if raw else [sys.executable, "-m", "clo3d_mcp.server"]
    if not isinstance(command, list) or not command or not all(isinstance(arg, str) and arg for arg in command):
        raise ValueError("CLO_MCP_OFFICIAL_COMMAND must be a JSON array of command arguments")
    settings = ("CLO_APP", "CLO_APP_PATH", "CLO_MCP_ASSETS", "CLO_MCP_HOST", "CLO_MCP_PORT", "CLO_MCP_STATE")
    env = {key: os.environ[key] for key in settings if key in os.environ}
    return StdioServerParameters(command=command[0], args=command[1:], env=env)


@asynccontextmanager
async def backend_lifespan(_server):
    backend = os.environ.get("CLO_MCP_BACKEND", "rust")
    if backend == "official":
        async with stdio_client(official_parameters()) as (reader, writer):
            async with ClientSession(reader, writer) as client:
                await client.initialize()
                yield client
    elif backend == "rust":
        backend = RustBackend()
        try:
            yield backend
        finally:
            await backend.close()
    else:
        raise ValueError("CLO_MCP_BACKEND must be 'rust' or 'official'")


def image_blocks(name, result):
    blocks = []
    summary = {key: value for key, value in result.items() if key != "images"}
    if name == "snapshot_2d":
        summary.pop("path", None)
    if summary and name != "clo_ui_screenshot":
        blocks.append(types.TextContent(type="text", text=json.dumps(summary, ensure_ascii=False)))
    for image in result.get("images", []):
        path = Path(image["path"])
        data = base64.b64encode(path.read_bytes()).decode("ascii")
        view = image.get("view")
        if name != "clo_ui_screenshot":
            caption = f"2D snapshot: {path}" if name == "snapshot_2d" else f"[{view} view] {path}" if view else str(path)
            blocks.append(types.TextContent(type="text", text=caption))
        blocks.append(types.ImageContent(type="image", data=data, mimeType=image.get("mime_type", "image/png")))
    return blocks


def register(server):
    tools = {entry["name"]: types.Tool.model_validate(entry) for entry in json.loads(CONTRACT.read_text())}
    lowlevel = server._mcp_server

    @lowlevel.list_tools()
    async def list_tools():
        return [*tools.values(), *await server.list_tools()]

    @lowlevel.call_tool(validate_input=False)
    async def call_tool(name, arguments):
        definition = tools.get(name)
        if definition is None:
            return await server.call_tool(name, arguments)
        backend = lowlevel.request_context.lifespan_context
        if isinstance(backend, ClientSession):
            return await backend.call_tool(name, arguments)
        try:
            jsonschema.validate(arguments, definition.inputSchema)
        except jsonschema.ValidationError as exc:
            raise ValueError(f"Input validation error: {exc.message}") from exc
        result = await backend.call(name, arguments)
        if name in IMAGE_TOOLS and isinstance(result, dict) and "images" in result:
            return await asyncio.to_thread(image_blocks, name, result)
        if definition.outputSchema:
            return {"result": result} if isinstance(result, str) else result
        text = result if isinstance(result, str) else json.dumps(result, ensure_ascii=False, indent=2)
        return [types.TextContent(type="text", text=text)]
