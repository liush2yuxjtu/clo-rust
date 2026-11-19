"""Launch the stdio server from an arbitrary MCP client working directory."""
import sys
from pathlib import Path
from runpy import run_module

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
run_module("clo_mcp.server", run_name="__main__")
