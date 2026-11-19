"""Add guarded resized-body support to the verified, source-pinned Boat engine."""
from pathlib import Path
import shutil
import sys

engine = Path(sys.argv[1]).resolve()
physics = engine / "experiments/intent-edit-review/physics"
shutil.copyfile(Path(__file__).with_name("sized_body.py"), physics / "sized_body.py")

def edit(name, edits):
    path = physics / name
    text = path.read_text()
    for old, new in edits:
        if text.count(old) != 1:
            raise ValueError(f"Pinned engine source mismatch: {name}, {old}")
        text = text.replace(old, new)
    path.write_text(text)

edit("prepare_experiment.py", [
    ('import argparse\n', 'import argparse\nimport os\nfrom sized_body import transform as resize_points, verify_resized\n'),
    ("    roots = np.array(list(first.values()), int)",
'''    sized_source = os.environ.get("CLO_SIZED_BODY_ASSET")
    if sized_source:
        original_path = ROOT/cfg['人体']['文件']
        original = json.loads(original_path.read_bytes())
        sized_path = (ROOT/sized_source).resolve()
        if not sized_path.is_relative_to(ROOT):
            raise ValueError("Resized body must be in the engine directory")
        resized = json.loads(sized_path.read_bytes())
        provenance = resized["sizing"]
        if sha(original_path.read_bytes()) != provenance["canonical_sha256"]:
            raise ValueError("Resized body source mismatch")
        verify_resized(original, resized, provenance["receipt"])
        # The canonical rig supplies the wearable seed. Move the seed and
        # collider by the same independently verified body-space transform.
        xyz = resize_points(xyz, original, provenance["receipt"])
        body, packed_body = body_asset(sized_path)
        packed_body["canonical_source"] = str(original_path.relative_to(ROOT))
        packed_body["canonical_source_sha256"] = provenance["canonical_sha256"]
        if arm_seed is not None:
            arm_seed["resized_body_source_sha256"] = packed_body["source_sha256"]
    roots = np.array(list(first.values()), int)'''),
    ("    (out/'input.json').write_text",'''    if sized_source:
        for path in (sized_path, original_path, Path(__file__).with_name("sized_body.py")):
            result['source_hashes'][str(path.relative_to(ROOT))] = sha(path.read_bytes())
    (out/'input.json').write_text'''),
])
edit("arm_wear_oracle.py", [
    ('NUMERICAL_M = 1e-6', 'from sized_body import transform as resize_points, verify_resized\n\nNUMERICAL_M = 1e-6'),
    ('    canonical, joints, original_provenance = _source_joints(str(source_path), packed["source_sha256"])',
'''    original_path = (ROOT / packed.get("canonical_source", packed["source"])).resolve()
    if not original_path.is_relative_to(ROOT):
        raise ValueError("Canonical arm rig is outside the engine")
    canonical, joints, original_provenance = _source_joints(
        str(original_path), packed.get("canonical_source_sha256", packed["source_sha256"]))
    if source_path != original_path:
        raw = source_path.read_bytes()
        if _sha(raw) != packed["source_sha256"]:
            raise ValueError("Resized collider checksum mismatch")
        resized = json.loads(raw)
        sizing = resized["sizing"]["receipt"]
        verify_resized(canonical, resized, sizing)
        joints = {side: resize_points(points, canonical, sizing) for side, points in joints.items()}
        canonical = resized'''),
    ('    proxy_sha = _bind_proxy_provenance(provenance)', '''    proxy_sha = _bind_proxy_provenance(provenance)
    if source_path != original_path:
        provenance["resized_body_sha256"] = packed["source_sha256"]
        provenance["source_hashes"][str(source_path.relative_to(ROOT))] = packed["source_sha256"]
        adapter = Path(__file__).with_name("sized_body.py")
        provenance["source_hashes"][str(adapter.relative_to(ROOT))] = _sha(adapter.read_bytes())'''),
])
