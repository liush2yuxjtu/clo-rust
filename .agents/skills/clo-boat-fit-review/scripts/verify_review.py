#!/usr/bin/env python3
"""Gate complete Boat fit reviews on distinct poses and five real maps per pose."""

import argparse
import hashlib
import json
from pathlib import Path
from xml.etree import ElementTree

MAPS = {
    "fit": "试穿合体",
    "strain": "应变",
    "stress": "材料应力",
    "pressure": "接触压力",
    "fold": "褶皱折角",
}
MIN_POSES = 2


def written(value):
    return isinstance(value, str) and bool(value.strip())


def image_digest(root, relative):
    path = (root / relative).resolve()
    if not path.is_relative_to(root) or not path.is_file():
        raise ValueError(f"图像须存在于清单目录内：{relative}")
    data = path.read_bytes()
    suffix = path.suffix.lower()
    if suffix == ".png":
        valid = data.startswith(b"\x89PNG\r\n\x1a\n")
    elif suffix in {".jpg", ".jpeg"}:
        valid = data.startswith(b"\xff\xd8\xff")
    elif suffix == ".webp":
        valid = data.startswith(b"RIFF") and data[8:12] == b"WEBP"
    elif suffix == ".svg":
        valid = ElementTree.fromstring(data).tag.split("}")[-1] == "svg"
    else:
        valid = False
    if not valid:
        raise ValueError(f"图像格式无效：{relative}")
    return hashlib.sha256(data).hexdigest()


def check(manifest, root):
    if not isinstance(manifest, dict) or not isinstance(manifest.get("poses"), list):
        return ["清单需要 poses 数组"]
    poses = manifest["poses"]
    issues = []
    labels = set()
    images = {}
    for index, pose in enumerate(poses, start=1):
        if not isinstance(pose, dict) or not all(written(pose.get(k)) for k in ("label", "source", "setup")):
            issues.append(f"姿势 {index} 缺名称、来源或设置")
            continue
        label = pose["label"].strip()
        key = label.casefold()
        if key in labels:
            issues.append(f"重复姿势名称：{label}")
        labels.add(key)
        for kind, name in MAPS.items():
            group = pose.get(kind)
            cell = f"{label} · {name}"
            if not isinstance(group, dict) or group.get("status") != "verified":
                issues.append(f"{cell} 缺已核实的图")
                continue
            fields = {"fit": ("method",), "stress": ("method", "unit", "scale"),
                      "pressure": ("method", "unit", "scale")}.get(kind, ())
            if not all(written(group.get(k)) for k in fields):
                issues.append(f"{cell} 缺取证方法、单位或色标")
            assets = group.get("assets")
            if not isinstance(assets, list) or not assets:
                issues.append(f"{cell} 缺实际图像文件")
                continue
            for asset in assets:
                if not isinstance(asset, dict) or not all(
                        written(asset.get(k)) for k in ("path", "source", "view", "caption")):
                    issues.append(f"{cell} 图像缺文件、来源、视角或图注")
                    continue
                try:
                    digest = image_digest(root, asset["path"])
                except (OSError, ValueError, ElementTree.ParseError) as error:
                    issues.append(f"{cell} · {error}")
                    continue
                previous = images.setdefault(digest, (index, cell))
                if previous != (index, cell):
                    issues.append(f"{cell} 与 {previous[1]} 重复使用同一图像")
    if len(labels) < MIN_POSES:
        issues.insert(0, f"需要至少 {MIN_POSES} 个不同姿势，清单提供 {len(labels)} 个")
    return issues


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    args = parser.parse_args()
    try:
        manifest = args.manifest.resolve(strict=True)
        records = json.loads(manifest.read_text(encoding="utf-8"))
        issues = check(records, manifest.parent)
    except (OSError, ValueError) as error:
        issues = [f"清单不可读：{error}"]
    if issues:
        parser.exit(2, "完整试穿评审未通过\n" + "\n".join(issues) + "\n")
    print(f"完整试穿评审证据通过：{len(records['poses'])} 个姿势，每姿势 {len(MAPS)} 类图")


if __name__ == "__main__":
    main()
