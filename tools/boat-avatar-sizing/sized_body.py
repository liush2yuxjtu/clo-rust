"""Apply and independently bind the Rust sizing transform to a known body source."""
from hashlib import sha256
import json
from pathlib import Path

import numpy as np


def digest(path):
    return sha256(Path(path).read_bytes()).hexdigest()


def transform(positions, source, transform_record):
    positions = np.asarray(positions, dtype=float)
    spec = transform_record["transform"]
    ground = spec["ground_mm"] / 1000
    height = np.ptp(np.asarray(source["positions"], dtype=float).reshape(-1, 3)[:, 1])
    y = (positions[:, 1] - ground) / height
    knots = np.asarray(spec["normalized_y_and_scale"], dtype=float)
    if (knots.shape != (5, 2) or np.any(np.diff(knots[:, 0]) <= 0)
            or np.any(~np.isfinite(knots)) or np.any(knots[:, 1] <= 0)
            or not np.isfinite(height) or height <= 0):
        raise ValueError("Invalid mesh sizing transform")
    scale = np.ones(len(y))
    for (start, a), (end, b) in zip(knots, knots[1:]):
        mask = (y >= start) & (y <= end)
        t = (y[mask] - start) / (end - start)
        scale[mask] = a + (b-a) * t*t*(3-2*t)
    result = positions.copy()
    center = np.asarray(spec["center_xz_mm"]) / 1000
    result[:, [0, 2]] = center + (positions[:, [0, 2]] - center)*scale[:, None]
    result[:, 1] = ground + (positions[:, 1] - ground)*spec["height_scale"]
    return result


def verify_resized(original, resized, sizing):
    expected = transform(np.asarray(original["positions"]).reshape(-1, 3), original, sizing)
    vertices = np.asarray(resized["positions"]).reshape(-1, 3)
    if (vertices.shape != expected.shape or not np.allclose(expected, vertices, rtol=0, atol=3e-8)
            or not np.array_equal(original["triangles"], resized["triangles"])):
        raise ValueError("Resized collider differs from the verified Rust transform or source topology")


def build(source_path, obj_path, receipt_path, target):
    source = json.loads(source_path.read_bytes())
    sizing = json.loads(receipt_path.read_bytes())
    if sizing["status"] != "passed" or digest(obj_path) != sizing["avatar_obj_sha256"]:
        raise ValueError("Boat OBJ does not match the successful MCP sizing receipt")
    vertices, faces = [], []
    with obj_path.open() as stream:
        for line in stream:
            words = line.split()
            if not words:
                continue
            if words[0] == "v":
                vertices.extend(float(x)/1000 for x in words[1:])
            if words[0] == "f":
                faces.extend(int(x)-1 for x in words[1:])
    result = {**source, "name": source["name"] + " (CLO Rust sized)",
              "positions": vertices, "triangles": faces,
              "sizing": {"canonical_sha256": digest(source_path),
                         "receipt_sha256": digest(receipt_path),
                         "receipt": sizing}}
    verify_resized(source, result, sizing)
    target.write_text(json.dumps(result, separators=(",", ":"), ensure_ascii=False))


if __name__ == "__main__":
    import sys
    build(*(Path(arg) for arg in sys.argv[1:]))
