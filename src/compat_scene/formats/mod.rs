pub(super) mod clo_native;
mod dxf;
mod mesh;
mod native_materials;
pub(super) mod native_fabric;
mod native_pose;
mod native_recolor;
pub(super) mod native_save;
mod native_textures;
mod native_seams;

pub(super) use dxf::{read_dxf, write_dxf};
pub(super) use mesh::{read_mesh, read_mesh_scene, write_mesh};

use super::patterns::Point;
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    publish(path, bytes, |temp| fs::rename(temp, path))
        .map_err(|err| format!("{}: {err}", path.display()))
}

pub(super) fn write_new(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    let mut collision = false;
    publish(path, bytes, |temp| match fs::hard_link(temp, path) {
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            collision = true;
            Ok(())
        }
        result => result,
    })
    .map(|()| !collision)
    .map_err(|err| format!("{}: {err}", path.display()))
}

fn publish(
    path: &Path,
    bytes: &[u8],
    commit: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let (mut file, temp) = loop {
        let suffix = NEXT.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(".clo-rust-{}-{suffix}.tmp", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => break (file, temp),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    };
    let result = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    let result = result.and_then(|_| commit(&temp));
    let _ = fs::remove_file(&temp);
    result
}

pub(super) fn read_json(path: &Path) -> Result<Value, String> {
    let data = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_reader(std::io::BufReader::new(data))
        .map_err(|error| format!("{}: {error}", path.display()))
}

pub(super) fn write_json(path: &Path, data: &Value) -> Result<(), String> {
    write(
        path,
        &serde_json::to_vec(data).map_err(|error| error.to_string())?,
    )
}

fn path_point(point: &Value, kind: u64) -> Result<Point, String> {
    let x = point["x"]
        .as_f64()
        .ok_or("pattern outline point has no numeric x")?;
    let y = point["y"]
        .as_f64()
        .ok_or("pattern outline point has no numeric y")?;
    Ok(Point { x, y: -y, kind })
}

pub(super) fn pattern_native(data: &Value) -> Result<Value, String> {
    if data["units"] != "mm" {
        return Err("pattern-native import requires mm units".into());
    }
    if data["seams"]
        .as_array()
        .is_some_and(|seams| !seams.is_empty())
    {
        return Err(
            "native diagnostic seam spans require an explicit editable outline-edge mapping".into(),
        );
    }
    let raw = data["pieces"].as_array().ok_or("missing pieces array")?;
    let mut pieces = Vec::new();
    for piece in raw {
        let name = piece["name"].as_str().ok_or("pattern has no name")?;
        let outline = piece["draft_paths"]["outline"]
            .as_array()
            .ok_or_else(|| format!("{name}: missing editable outline path"))?;
        let mut coords = Vec::new();
        for item in outline {
            match item["type"].as_str() {
                Some("move") if !coords.is_empty() => {
                    return Err(format!("{name}: multiple outline subpaths"))
                }
                Some("move" | "line") => coords.push(path_point(&item["to"], 0)?),
                Some("curve") => {
                    coords.push(path_point(&item["cp1"], 3)?);
                    coords.push(path_point(&item["cp2"], 3)?);
                    coords.push(path_point(&item["to"], 0)?);
                }
                Some("close") => break,
                _ => return Err(format!("{name}: unknown outline path operation")),
            }
        }
        if coords
            .first()
            .zip(coords.last())
            .is_some_and(|(a, b)| (a.x - b.x).hypot(a.y - b.y) < 1e-7)
        {
            coords.pop();
        }
        pieces.push(json!({
            "id": pieces.len() + 1, "name": name, "points": coords.into_iter().map(Point::record).collect::<Vec<_>>(),
            "arrangement": "", "fabric_index": 0
        }));
    }
    Ok(json!({"patterns": pieces, "seams": []}))
}
