"""Run the CLO Rust MCP server over stdio."""
import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

from mcp.server.fastmcp import FastMCP
from pydantic import BaseModel

from . import evidence, runtime

server = FastMCP("CLO Rust Boat", instructions=(
    "Use confirmable saved CLO evidence. Seam, strain and fold geometry run through "
    "pattern-native. Stress and pressure require their own recorded native evidence. "
    "Only replay timestamped simulation recordings."
))


def local_file(value):
    path = Path(value).expanduser().resolve(strict=True)
    if not path.is_file():
        raise ValueError(f"Expected a local file: {path}")
    return str(path)


def output_dir(label):
    root = Path(os.environ.get("CLO_RUST_OUTPUT_DIR", Path.cwd() / "clo-rust-results")).expanduser()
    root.mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(prefix=label + "-", dir=root.resolve()))


def subprocess_result(args):
    result = subprocess.run(args, capture_output=True, text=True, timeout=240, check=False)
    return result


def native(args):
    binary, mode = runtime.native_binary()
    return subprocess_result([str(binary), *args]), mode


def checked(result):
    if result.returncode:
        raise RuntimeError((result.stderr or result.stdout).strip()[-6000:])
    return json.loads(result.stdout)


def metric_run(glb, metric_provenance, debug):
    if not metric_provenance.strip():
        raise ValueError("Provide evidence for raw millimetre positions and metric planar UVs")
    glb = local_file(glb)
    output = output_dir("debug" if debug else "mesh")
    vtk = output / "garment.vtk"
    args = ["mesh-metrics", glb]
    if debug:
        args.append(str(vtk))
    result, mode = native(args)
    metrics = checked(result)
    evidence.write_json(output / "geometry.json", {
        "glb": glb, "glb_sha256": runtime.digest(Path(glb)), "metric_provenance": metric_provenance,
        "runtime": mode, "metrics": metrics,
    })
    return metrics, output


@server.tool()
def prewarm_runtime(download_release: bool = False) -> dict[str, Any]:
    """Verify and cache the pinned GitHub Release or check the configured executable. Set download_release to prefer the Release even with a local override."""
    binary, source = runtime.prewarm() if download_release else runtime.native_binary()
    probe = subprocess_result([str(binary), "--version"])
    if probe.returncode and source != "configured":
        raise RuntimeError(probe.stderr.strip())
    return {"status": "ready" if not probe.returncode else "configured_legacy",
            "release": runtime.TAG if source != "configured" else None,
            "source": source, "version": probe.stdout.strip() or None, "binary": str(binary),
            "sha256": runtime.digest(binary)}


@server.tool()
def verify_seam(project_json: str | None = None, edges_json: str | None = None,
                edge_map_json: str | None = None, seams_json: str | None = None,
                connectivity_json: str | None = None, include_constraints: bool = False) -> dict[str, Any]:
    """核查 CLO 缝合。传入一个 pattern-native 工程，或同时传入 CLO 边记录、边映射和缝合记录。可核对连通参照。"""
    exports = (edges_json, edge_map_json, seams_json)
    if not ((project_json and not any(exports)) or (not project_json and all(exports))):
        raise ValueError("Provide project_json or all three CLO export files")
    output = output_dir("seams")
    if project_json:
        project = local_file(project_json)
        result, _ = native(["audit", project])
    else:
        project = str(output / "normalized.pattern.json")
        result, _ = native(["clo-import", *(local_file(path) for path in exports), project])
    try:
        audit = json.loads(result.stdout or result.stderr)
    except json.JSONDecodeError:
        raise RuntimeError(result.stderr.strip()) from None
    record = {"status": "failed" if result.returncode else "passed", "audit": audit}
    if not result.returncode:
        if audit.get("length_review"):
            record["status"] = "needs_review"
        if connectivity_json:
            groups, _ = native(["connectivity", local_file(connectivity_json), "--strict"])
            if not groups.stdout.strip():
                checked(groups)
            record["connectivity"] = json.loads(groups.stdout)
            if groups.returncode:
                record["status"] = "failed"
        if include_constraints:
            constraints, _ = native(["constraints", project])
            target = output / "constraints.json"
            data = checked(constraints)
            evidence.write_json(target, data)
            record.update(constraint_count=len(data), constraints=str(target))
    evidence.write_json(output / "seam-review.json", record)
    return {**record, "manifest": str(output / "seam-review.json")}


def mesh_summary(metrics, output, field_names):
    return {"status": "computed", "pieces": [{name: row[name] for name in field_names}
                                            for row in metrics],
            "manifest": str(output / "geometry.json")}


@server.tool()
def strain(glb: str, metric_provenance: str) -> dict[str, Any]:
    """应变。用已核对原始位置和纸样 UV 均为毫米的服装 GLB 计算逐片主工程应变，返回面积加权 P95%。"""
    metrics, output = metric_run(glb, metric_provenance, False)
    return mesh_summary(metrics, output, ("piece_index", "analysed_triangles", "strain_p95_pct"))


@server.tool()
def fold_angles(glb: str, metric_provenance: str) -> dict[str, Any]:
    """褶皱折角。计算裁片内相邻三角面的无符号折角，0° 为平面，返回逐片 P95 和可测边数。"""
    metrics, output = metric_run(glb, metric_provenance, False)
    return mesh_summary(metrics, output, ("piece_index", "fold_interior_edges", "fold_p95_deg"))


@server.tool()
def stress(source: str, pose: str, display_unit: str, display_scale: str,
           front: str | None = None, back: str | None = None, samples_csv: str | None = None) -> dict[str, Any]:
    """应力。归档同姿势 CLO Stress Map 与实际记录的 stress_kpa 样本。返回缺失状态和证据报告。"""
    return evidence.native_map(output_dir("stress"), "stress", source, pose, display_unit,
                               display_scale, front, back, samples_csv)


@server.tool()
def fit_pressure(source: str, pose: str, display_unit: str, display_scale: str,
                 front: str | None = None, back: str | None = None,
                 samples_csv: str | None = None) -> dict[str, Any]:
    """试穿压力。归档同姿势 CLO Pressure Map 与实际记录的 pressure_kpa 样本。输入须为压力模式。"""
    return evidence.native_map(output_dir("pressure"), "pressure", source, pose, display_unit,
                               display_scale, front, back, samples_csv)


@server.tool()
def compare_line_art(line_art: str, garment_view: str, source: str, pose: str, view: str) -> dict[str, Any]:
    """线稿对照。用同视角线稿和 CLO 成衣快照生成并排证据，以供轮廓和设计细节核查。"""
    return evidence.comparison(output_dir("line-art"), line_art, garment_view, source, pose, view)


class ImageFrame(BaseModel):
    image: str
    time_s: float


class MeshFrame(BaseModel):
    glb: str
    time_s: float


def mesh_replay(source, frames, metric_provenance):
    if not metric_provenance or not metric_provenance.strip():
        raise ValueError("Provide evidence for raw millimetre positions and metric planar UVs")
    runner = os.environ.get("CLO_BOAT_REVIEW_SCRIPT")
    if not runner:
        raise ValueError("Set CLO_BOAT_REVIEW_SCRIPT to use the Boat mesh staging helper")
    times = [frame.time_s for frame in frames]
    evidence.check_frame_times(times, 120)
    paths = [local_file(frame.glb) for frame in frames]
    if sum(Path(path).stat().st_size for path in paths) > 512 * 1024 * 1024:
        raise ValueError("Replay source meshes must total at most 512 MiB")

    output = output_dir("mesh-replay")
    prepared = output / "prepared"
    helper = Path(runner).expanduser().resolve().with_name("prepare_meshes.py")
    args = [sys.executable, local_file(helper), str(prepared), "--mesh-source", source]
    for index, path in enumerate(paths):
        args.extend(("--pose", f"frame-{index:04d}", path))
    result = subprocess_result(args)
    if result.returncode:
        raise RuntimeError((result.stderr or result.stdout).strip()[-6000:])
    manifest = json.loads((prepared / "manifest.json").read_text())
    manifest["metric_provenance"] = metric_provenance

    binary, mode = runtime.native_binary()
    manifest["runtime"] = mode
    series = []
    for pose, timestamp in zip(manifest["poses"], times, strict=True):
        stem = pose["id"]
        vtk = output / (stem + ".vtk")
        result = subprocess_result([str(binary), "mesh-metrics", str(prepared / (stem + ".glb")), str(vtk)])
        evidence.write_json(output / (stem + ".json"), checked(result))
        pose["time_s"] = timestamp
        series.append({"name": vtk.name, "time": timestamp})
    evidence.write_json(output / "replay.json", manifest)
    series_path = output / "recording.vtk.series"
    evidence.write_json(series_path, {"file-series-version": "1.0", "files": series})
    return {"status": "recorded", "frames": len(series), "duration_s": times[-1] - times[0],
            "vtk_series": str(series_path), "manifest": str(output / "replay.json")}


@server.tool()
def physics_replay(source: str, frames: list[ImageFrame] | None = None,
                   mesh_frames: list[MeshFrame] | None = None,
                   metric_provenance: str | None = None) -> dict[str, Any]:
    """物理回放。传入带时间戳的已录制截图生成离线播放器，或传入同拓扑 GLB 序列生成含应变和折角的 ParaView 时间序列。"""
    if not source.strip() or (frames is None) == (mesh_frames is None):
        raise ValueError("Provide the simulation recording source and one frame sequence")
    if mesh_frames is not None:
        return mesh_replay(source, mesh_frames, metric_provenance)
    return evidence.replay(output_dir("replay"), [frame.model_dump() for frame in frames], source)


@server.tool()
def physics_debug_view(glb: str, metric_provenance: str, avatar_glb: str | None = None,
                       avatar_mesh: str | None = None, avatar_units: str | None = None,
                       cloth_mesh: str = "Cloth_mesh") -> dict[str, Any]:
    """物理调试视图。输出带应变和折角标量场的 VTK。可对同姿势、同原点的人台另外输出静态相交点。"""
    if any((avatar_glb, avatar_mesh, avatar_units)) and not all((avatar_glb, avatar_mesh, avatar_units)):
        raise ValueError("Avatar collision requires avatar_glb, avatar_mesh and avatar_units")
    metrics, output = metric_run(glb, metric_provenance, True)
    record = {"status": "computed", "piece_count": len(metrics), "vtk": str(output / "garment.vtk"),
              "manifest": str(output / "geometry.json")}
    if avatar_glb:
        collision_vtk = output / "contact.vtk"
        result, _ = native(["collision-avatar", local_file(avatar_glb), avatar_mesh, avatar_units,
                            local_file(glb), cloth_mesh, "mm", str(collision_vtk)])
        collision = checked(result)
        evidence.write_json(output / "contacts.json", collision)
        record.update(contacts=collision, contact_vtk=str(collision_vtk))
    return record


@server.tool()
def export_fit_maps(glb: str, metric_provenance: str, piece_ids: str | None = None,
                    project: str | None = None, fit_front: str | None = None, fit_back: str | None = None,
                    stress_front: str | None = None, stress_back: str | None = None,
                    stress_samples: str | None = None, strain_front: str | None = None,
                    strain_back: str | None = None, require_all_maps: bool = False) -> dict[str, Any]:
    """运行现有 clo-fit-export，生成试穿图、主工程应变图、应力图和折角图，返回真实的各图状态。"""
    if not metric_provenance.strip():
        raise ValueError("Provide evidence for raw millimetre positions and metric planar UVs")
    exporter = os.environ.get("CLO_FIT_EXPORT_SCRIPT")
    if not exporter:
        raise ValueError("Set CLO_FIT_EXPORT_SCRIPT to the installed clo-fit-export scripts/export_fit.py")
    output = output_dir("fit-maps")
    args = [sys.executable, local_file(exporter), local_file(glb), "--out-dir", str(output)]
    for name, value in {"project": project, "fit-front": fit_front, "fit-back": fit_back,
                        "stress-front": stress_front, "stress-back": stress_back,
                        "stress-samples": stress_samples, "strain-front": strain_front,
                        "strain-back": strain_back}.items():
        if value:
            args.extend(("--" + name, local_file(value)))
    if piece_ids:
        args.extend(("--piece-ids", piece_ids))
    if require_all_maps:
        args.append("--require-all-maps")
    result = subprocess_result(args)
    if result.returncode:
        raise RuntimeError(result.stderr.strip()[-6000:])
    statistics = output / "fit_statistics.json"
    evidence.write_json(output / "source.json", {"metric_provenance": metric_provenance})
    return {"maps": json.loads(statistics.read_text())["maps"], "statistics": str(statistics),
            "artifacts": sorted(str(path) for path in output.iterdir())}


@server.tool()
def review_boat_batch(input_dir: str, require_all_maps: bool = False) -> dict[str, Any]:
    """运行已安装的 CLO Rust Boat 多姿势批处理。输入为经原插件准备并保存 manifest.json 的目录。"""
    runner = os.environ.get("CLO_BOAT_REVIEW_SCRIPT")
    if not runner:
        raise ValueError("Set CLO_BOAT_REVIEW_SCRIPT to the installed Boat scripts/run_review.py")
    inputs = Path(input_dir).expanduser().resolve(strict=True)
    local_file(inputs / "manifest.json")
    output = output_dir("boat-fit") / "review"
    binary, mode = runtime.native_binary()
    args = [sys.executable, local_file(runner), str(inputs), str(output),
            "--native", str(binary), "--runtime-label", f"CLO Rust MCP · {mode}"]
    if require_all_maps:
        args.append("--require-all-maps")
    result = subprocess_result(args)
    if result.returncode:
        raise RuntimeError((result.stderr or result.stdout).strip()[-6000:])
    reports = sorted(output.glob("*.html"))
    if not reports:
        raise RuntimeError("Boat runner completed without an HTML report")
    return {"status": "reviewed", "output_dir": str(output), "reports": [str(path) for path in reports]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prewarm", action="store_true", help="prepare and print the native executable path")
    if parser.parse_args().prewarm:
        print(prewarm_runtime()["binary"])
    else:
        server.run(transport="stdio")
