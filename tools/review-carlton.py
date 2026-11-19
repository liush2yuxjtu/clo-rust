#!/usr/bin/env python3
"""Render Rust VTK fields and compare them with independently checked CLO exports."""

import argparse
import csv
import hashlib
import json
import shutil
import sys
from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.collections import PolyCollection
from matplotlib.colors import Normalize
from matplotlib.font_manager import fontManager, FontProperties
import numpy as np

POSES = ("垂臂", "斜展", "平举")


def read_vtk(path):
    lines = iter(path.read_text().splitlines()[4:])
    point_count = int(next(lines).split()[1])
    points = np.array([next(lines).split() for _ in range(point_count)], dtype=float)
    face_count = int(next(lines).split()[1])
    faces = np.array([next(lines).split()[1:] for _ in range(face_count)], dtype=int)
    if next(lines) != f"CELL_DATA {face_count}":
        raise ValueError(f"VTK 面数据数量不符：{path}")
    fields = {}
    for line in lines:
        _, name, _, _ = line.split()
        if next(lines) != "LOOKUP_TABLE default":
            raise ValueError(f"VTK 字段格式不符：{path}")
        fields[name] = np.fromiter((float(next(lines)) for _ in range(face_count)), float)
    return points, faces, fields


def render(records, views, field, limits, palette, title, units, path, font):
    norm = Normalize(*limits, clip=True)
    cmap = matplotlib.colormaps[palette]
    fig, axes = plt.subplots(len(records), len(views), figsize=(3.1 * len(views), 11),
                            layout="constrained")
    fig.suptitle(title, fontsize=17, fontproperties=font)
    vertices = np.concatenate([triangles.reshape(-1, 3) for triangles, _ in records])
    y_min, y_max = vertices[:, 1].min(), vertices[:, 1].max()
    pad = (y_max - y_min) * .025
    for row, ((triangles, fields), pose) in enumerate(zip(records, POSES)):
        colors = cmap(norm(fields[field]))
        if field == "engineering_strain_pct":
            colors[fields["strain_is_valid"] == 0] = (.70, .74, .78, 1)
        for column, (label, horizontal, depth, x_sign, depth_sign) in enumerate(views):
            ax = axes[row, column]
            order = np.argsort(depth_sign * triangles[:, :, depth].mean(axis=1), kind="stable")
            xy = triangles[:, :, [horizontal, 1]].copy()
            xy[:, :, 0] *= x_sign
            ax.add_collection(PolyCollection(xy[order], facecolors=colors[order],
                                            edgecolors="face", linewidths=.09, antialiaseds=False))
            x_extent = vertices[:, horizontal] * x_sign
            ax.set(xlim=(x_extent.min() - pad, x_extent.max() + pad),
                   ylim=(y_min - pad, y_max + pad), aspect="equal")
            ax.set_axis_off()
            ax.set_title(f"{pose} · {label}", fontproperties=font, fontsize=12)
    fig.colorbar(plt.cm.ScalarMappable(norm=norm, cmap=cmap), ax=axes,
                 orientation="horizontal", shrink=.65, aspect=33, extend="both"
                 if field == "engineering_strain_pct" else "max").set_label(
                     units, fontproperties=font)
    fig.savefig(path, dpi=175, facecolor="#ffffff", bbox_inches="tight")
    plt.close(fig)


def write_csv(path, rows):
    with path.open("w", encoding="utf-8-sig", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=rows[0].keys())
        writer.writeheader()
        writer.writerows(rows)


def render_sleeve_bands(bands, path, font):
    cmap = matplotlib.colormaps["viridis"]
    norm = Normalize(0, 50, clip=True)
    width, height = np.concatenate([xy.reshape(-1, 2) for xy, *_ in bands]).max(axis=0)
    fig, axes = plt.subplots(3, 2, figsize=(7, 12), layout="constrained")
    fig.suptitle("袖底上缘 · 平面纸样局部", fontproperties=font, fontsize=16)
    for ax, (xy, selected, strain, row) in zip(axes.flat, bands):
        colors = np.tile([.89, .91, .93, 1.0], (len(xy), 1))
        colors[selected] = cmap(norm(strain))
        ax.add_collection(PolyCollection(xy, facecolors=colors, edgecolors="face",
                                        linewidths=.12, antialiaseds=False))
        ax.axhline(240, color="#556277", linestyle="--", linewidth=.7)
        value = row["应变P95（%）"]
        ax.set_title(f'{row["姿势"]} · {row["区域"]} · P95 {value:.1f}%',
                     fontproperties=font, fontsize=11)
        ax.set(xlim=(-12, width + 12), ylim=(height + 12, -12), aspect="equal")
        ax.set_axis_off()
    fig.colorbar(plt.cm.ScalarMappable(norm=norm, cmap=cmap), ax=axes,
                 orientation="horizontal", shrink=.67, aspect=30, extend="max").set_label(
                     "主应变（%），彩色为参与统计的三角面", fontproperties=font)
    fig.savefig(path, dpi=160, facecolor="#ffffff", bbox_inches="tight")
    plt.close(fig)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("reference", type=Path)
    parser.add_argument("work", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--fit-scripts", required=True, type=Path)
    parser.add_argument("--font", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.fit_scripts))
    from export_fit import read_cloth

    font = FontProperties(fname=args.font) if args.font else FontProperties(family="sans-serif")
    if args.font:
        fontManager.addfont(args.font)
    comparison = []
    local = []
    bands = []
    records = []
    baseline = []
    sources = {}
    pattern_names = []
    for pose in POSES:
        ref = args.reference / pose
        ref_reports = args.work / f"{pose}-CLO复核"
        mesh_path = ref / "服装网格.glb"
        digest = hashlib.sha256(mesh_path.read_bytes()).hexdigest()
        expected = json.loads((ref_reports / "fit_statistics.json").read_text())
        mapping = json.loads((ref / "网格与裁片核对.json").read_text())["pieces"]
        names = [item["name"] for item in mapping]
        if (digest != expected["folds"]["source_sha256"] or
                names != [item["piece_id"] for item in expected["pieces"]] or
                (pattern_names and names != pattern_names)):
            raise ValueError(f"参考网格身份或裁片顺序不符：{pose}")
        pattern_names = names
        sources[pose] = digest
        pieces = read_cloth(mesh_path)
        rust_json = args.work / f"{pose}-rust.json"
        rust = json.loads(rust_json.read_text())
        points, faces, fields = read_vtk(args.work / f"{pose}.vtk")
        if not len(names) == len(pieces) == len(rust):
            raise ValueError(f"裁片数不符：{pose}")
        point_offset = face_offset = 0
        for i, ((xyz, uv, triangles), actual, check) in enumerate(zip(pieces, rust, expected["pieces"])):
            name = names[i]
            if pose == POSES[0]:
                baseline.append((uv, triangles))
            elif not (np.array_equal(triangles, baseline[i][1]) and
                      np.allclose(uv, baseline[i][0], rtol=0, atol=1e-5)):
                raise ValueError(f"跨姿势平面网格不同：{pose} / {name}")
            count = len(triangles)
            indices = slice(face_offset, face_offset + count)
            if not (np.array_equal(triangles + point_offset, faces[indices]) and
                    np.array_equal(xyz, points[point_offset:point_offset + len(xyz)])):
                raise ValueError(f"Rust VTK 与参考网格不同：{pose} / {name}")
            face_offset += count
            point_offset += len(xyz)
            strain_delta = abs(actual["strain_p95_pct"] - check["strain_p95_pct"])
            fold_delta = abs(actual["fold_p95_deg"] - check["fold_p95_deg"])
            if (max(strain_delta, fold_delta) > 1e-9 or
                    actual["analysed_triangles"] != check["included_triangles"] or
                    actual["fold_interior_edges"] != check["fold_interior_edge_count"]):
                raise ValueError(f"Rust 统计与独立参考不一致：{pose} / {name}")
            comparison.append({
                "姿势": pose, "裁片": name, "可用面数": actual["analysed_triangles"],
                "应变P95（%）": actual["strain_p95_pct"], "折角P95（度）": actual["fold_p95_deg"],
                "应变绝对差": strain_delta, "折角绝对差": fold_delta,
            })
            if name not in ("Under sleeve L", "Under sleeve R"):
                continue
            xy = uv[triangles].astype(float)
            edge_x, edge_y = xy[:, 1] - xy[:, 0], xy[:, 2] - xy[:, 0]
            areas = np.abs(edge_x[:, 0] * edge_y[:, 1] - edge_x[:, 1] * edge_y[:, 0]) / 2
            selected = (xy[:, :, 1].mean(axis=1) - uv[:, 1].min() <= 240)
            selected &= fields["strain_is_valid"][indices].astype(bool)
            weights = areas[selected]
            strain = fields["engineering_strain_pct"][indices][selected]
            order = np.argsort(strain)
            row = {
                "姿势": pose, "区域": "左袖底上缘" if name.endswith("L") else "右袖底上缘",
                "带宽（毫米）": 240, "应变P95（%）": np.interp(.95, np.cumsum(weights[order]) /
                                                             weights.sum(), strain[order]),
                "应变超过10%的面积比例（%）": 100 * weights[strain > 10].sum() / weights.sum(),
                "参考面积（平方毫米）": weights.sum(),
            }
            local.append(row)
            bands.append((xy - uv.min(axis=0), selected, strain, row))
        records.append((points[faces], fields))
        pose_output = args.output / pose
        pose_output.mkdir(exist_ok=True)
        shutil.copyfile(rust_json, pose_output / "Rust逐片数据.json")
        for file, target in [("fit_map.png", "CLO原生试穿图.png"), ("fit_stress.png", "CLO原生应力图.png")]:
            shutil.copyfile(ref_reports / file, pose_output / target)

    write_csv(args.output / "三姿势逐片对照.csv", comparison)
    write_csv(args.output / "左右袖底局部对照.csv", local)
    front_back = [("正面", 0, 2, 1, 1), ("背面", 0, 2, -1, -1)]
    sides = [("侧面（+X）", 2, 0, -1, 1), ("侧面（−X）", 2, 0, 1, -1)]
    render(records, front_back, "engineering_strain_pct", (-3, 25), "viridis",
           "Carlton · Rust 应变", "主应变（%），灰色表示无有效值",
           args.output / "Rust三姿势应变.png", font)
    render(records, front_back + sides, "fold_angle_deg", (0, 45), "magma",
           "Carlton · Rust 折角", "相邻三角面折角（度）",
           args.output / "Rust三姿势折角.png", font)
    render_sleeve_bands(bands, args.output / "左右袖底局部应变.png", font)
    print(json.dumps({"网格摘要": sources, "逐片对照数": len(comparison),
                      "最大应变差": max(row["应变绝对差"] for row in comparison),
                      "最大折角差": max(row["折角绝对差"] for row in comparison)},
                     ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
