"""Package saved CLO evidence and frame recordings without altering the captures."""
import csv
import hashlib
import json
import math
import shutil
from html import escape
from pathlib import Path

IMAGE_LIMIT = 20 * 1024 * 1024
PAGE = """<!doctype html><html lang="zh"><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title><style>
body{{font:16px system-ui;margin:2rem auto;padding:0 1rem;max-width:1000px;color:#172320}}
figure{{margin:0;min-width:0}}img{{display:block;max-width:100%;max-height:72vh}}
figcaption{{margin-top:.5rem}}.row{{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:1.5rem}}
@media(max-width:600px){{.row{{grid-template-columns:1fr}}}}
</style><main><h1>{title}</h1>{body}</main></html>"""


def write_json(path, data):
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2, allow_nan=False) + "\n", encoding="utf-8")


def capture(source, output, name):
    path = Path(source).expanduser().resolve(strict=True)
    if not path.is_file() or path.stat().st_size > IMAGE_LIMIT:
        raise ValueError("Capture must be a local PNG or JPEG file up to 20 MiB")
    content = path.read_bytes()
    if content.startswith(b"\x89PNG\r\n\x1a\n"):
        extension = ".png"
    elif content.startswith(b"\xff\xd8\xff"):
        extension = ".jpg"
    else:
        raise ValueError("Capture must be PNG or JPEG")
    target = output / (name + extension)
    target.write_bytes(content)
    return {"file": target.name, "sha256": hashlib.sha256(content).hexdigest()}


def figure(capture_info, label):
    return (f'<figure><img src="{capture_info["file"]}" alt="{escape(label)}">'
            f'<figcaption>{escape(label)}</figcaption></figure>')


def page(output, title, body):
    path = output / "index.html"
    path.write_text(PAGE.format(title=escape(title), body=body), encoding="utf-8")
    return str(path)


def native_map(output, kind, source, pose, unit, scale, front, back, samples_csv):
    if not all(value.strip() for value in (source, pose, unit, scale)):
        raise ValueError("Provide the recorded source, pose, display unit and scale")
    field = kind + "_kpa"
    record = {"status": "recorded", "source": source, "pose": pose, "display_unit": unit,
              "display_scale": scale, "views": {}, "samples": None}
    for name, value in (("front", front), ("back", back)):
        if value is not None:
            record["views"][name] = capture(value, output, name)
    if samples_csv is not None:
        path = Path(samples_csv).expanduser().resolve(strict=True)
        if path.stat().st_size > 10 * 1024 * 1024:
            raise ValueError("Recorded samples CSV must be at most 10 MiB")
        count = 0
        with path.open(encoding="utf-8-sig", newline="") as stream:
            reader = csv.DictReader(stream)
            if not {"piece_id", "location", field}.issubset(reader.fieldnames or []):
                raise ValueError(f"Required columns: piece_id,location,{field}")
            for row in reader:
                if not row["piece_id"] or not row["location"] or not math.isfinite(float(row[field])):
                    raise ValueError("Missing location or invalid recorded sample")
                if kind == "pressure" and float(row[field]) < 0:
                    raise ValueError("Contact pressure must be nonnegative")
                count += 1
        if count:
            target = output / "samples.csv"
            shutil.copyfile(path, target)
            record["samples"] = {"file": target.name, "count": count, "unit": "kPa"}
    if len(record["views"]) < 2:
        record["status"] = "partial" if record["views"] or record["samples"] else "unavailable"
    title = {"stress": "材料应力", "pressure": "试穿压力"}[kind]
    body = f"<p>{escape(pose)} · {escape(source)} · {escape(unit)} · {escape(scale)}</p>"
    images = "".join(figure(info, {"front": "正面", "back": "背面"}[view])
                     for view, info in record["views"].items())
    missing = "、".join(label for view, label in (("front", "正面"), ("back", "背面"))
                       if view not in record["views"])
    body += f'<div class="row">{images}</div>'
    count = record["samples"]["count"] if record["samples"] else 0
    body += f"<p>缺少视图：{missing or '无'}。记录的 kPa 读数：{count}。</p>"
    write_json(output / "evidence.json", record)
    return {**record, "report": page(output, title, body), "manifest": str(output / "evidence.json")}


def comparison(output, line_art, garment, source, pose, view):
    if not all(value.strip() for value in (source, pose, view)):
        raise ValueError("Provide both file sources, the pose and the matching camera view")
    record = {"source": source, "pose": pose, "view": view,
              "line_art": capture(line_art, output, "line-art"),
              "garment": capture(garment, output, "garment")}
    write_json(output / "evidence.json", record)
    body = f"<p>{escape(pose)} · {escape(view)} · {escape(source)}</p>"
    body += f'<div class="row">{figure(record["line_art"], "设计线稿")}{figure(record["garment"], "成衣视图")}</div>'
    body += "<p>核对轮廓、衣长、领口、门襟、袖口、口袋位置及垂感。</p>"
    return {**record, "report": page(output, "线稿与同视角成衣", body)}


def check_frame_times(times, maximum):
    if not 2 <= len(times) <= maximum:
        raise ValueError(f"Provide 2–{maximum} recorded frames")
    if (any(not math.isfinite(time) or time < 0 for time in times) or
            any(a >= b or b - a > 3600 for a, b in zip(times, times[1:]))):
        raise ValueError("Frame timestamps must be finite, nonnegative, increasing, and at most one hour apart")


def replay(output, frames, source):
    times = [frame["time_s"] for frame in frames]
    check_frame_times(times, 1000)
    if sum(Path(frame["image"]).expanduser().stat().st_size for frame in frames) > 512 * 1024 * 1024:
        raise ValueError("Replay source frames must total at most 512 MiB")
    records = [{"time_s": frame["time_s"], **capture(frame["image"], output, f"frame-{index:04d}")}
               for index, frame in enumerate(frames)]
    write_json(output / "replay.json", {"source": source, "frames": records})
    body = f"""<p>{escape(source)}</p>
<button id="play">播放</button> <label>帧 <input id="seek" type="range"
value="0" min="0" max="{len(records)-1}"></label> <output id="time"></output>
<img id="frame" alt="仿真录制帧">
<script>
const frames = {json.dumps(records)};
const seek = document.getElementById("seek");
const image = document.getElementById("frame");
const time = document.getElementById("time");
const play = document.getElementById("play");
let timer;
function show() {{
  const current = frames[Number(seek.value)];
  image.src = current.file;
  time.textContent = current.time_s.toFixed(3) + " s";
}}
function stop() {{ clearTimeout(timer); timer = undefined; play.textContent = "播放"; }}
function next() {{
  const i = Number(seek.value);
  if (i === frames.length - 1) {{ stop(); return; }}
  timer = setTimeout(() => {{ seek.value = i + 1; show(); next(); }},
    (frames[i+1].time_s - frames[i].time_s) * 1000);
}}
play.onclick = () => {{
  if (timer !== undefined) {{ stop(); return; }}
  if (Number(seek.value) === frames.length - 1) {{ seek.value = 0; show(); }}
  play.textContent = "暂停";
  next();
}};
seek.oninput = () => {{ stop(); show(); }};
show();
</script>"""
    return {"status": "recorded", "frames": len(records), "duration_s": times[-1] - times[0],
            "report": page(output, "仿真录制回放", body), "manifest": str(output / "replay.json")}
