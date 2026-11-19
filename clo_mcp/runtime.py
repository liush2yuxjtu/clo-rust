"""Resolve an explicit native binary or prewarm a versioned GitHub Release."""
import hashlib
import os
import platform
import re
import subprocess
import tempfile
from pathlib import Path

REPO = "liush2yuxjtu/clo-rust"
TAG = os.environ.get("CLO_RUST_RELEASE", "v0.1.0-mcp.2")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def prewarm():
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[a-zA-Z0-9.-]+)?", TAG):
        raise ValueError("CLO_RUST_RELEASE must be an explicit version tag")
    architecture = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64"}.get(
        platform.machine().lower()
    )
    system = platform.system().lower()
    if system not in {"linux", "darwin"} or not architecture:
        raise ValueError(f"Unsupported release platform: {system}/{platform.machine()}")
    asset = f"pattern-native-{system}-{architecture}"
    cache = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache"))
    destination = cache / "clo-rust" / TAG / asset
    metadata = destination / "SHA256"
    binary = destination / "pattern-native"
    if binary.is_file() and metadata.is_file() and digest(binary) == metadata.read_text():
        return binary, "cache"

    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination.parent) as temporary:
        work = Path(temporary)
        try:
            subprocess.run(
                ["gh", "release", "download", TAG, "--repo", REPO,
                 "--pattern", asset, "--pattern", asset + ".sha256", "--dir", str(work)],
                check=True, capture_output=True, text=True, timeout=120,
            )
        except FileNotFoundError as exc:
            raise RuntimeError("Install GitHub CLI or configure CLO_RUST_BINARY") from exc
        except subprocess.CalledProcessError as exc:
            raise RuntimeError(f"GitHub release download failed: {exc.stderr.strip()}") from exc
        downloaded = work / asset
        checksum = (work / (asset + ".sha256")).read_text().split()
        if (len(checksum) != 2 or checksum[1] != asset or
                not re.fullmatch(r"[a-f0-9]{64}", checksum[0]) or digest(downloaded) != checksum[0]):
            raise ValueError(f"Release checksum mismatch for {asset}")
        downloaded.chmod(0o755)
        subprocess.run([str(downloaded), "--version"], check=True, capture_output=True, timeout=10)
        destination.mkdir(mode=0o700, exist_ok=True)
        downloaded.replace(binary)
        verified = work / "SHA256"
        verified.write_text(checksum[0])
        verified.replace(metadata)
    return binary, "github_release"


def native_binary():
    configured = os.environ.get("CLO_RUST_BINARY")
    if configured:
        path = Path(configured).expanduser().resolve(strict=True)
        if not path.is_file() or not os.access(path, os.X_OK):
            raise ValueError("CLO_RUST_BINARY must point to an executable file")
        return path, "configured"
    return prewarm()
