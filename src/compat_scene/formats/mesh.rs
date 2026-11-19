use super::{
    super::{array, flag, patterns},
    write,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize)]
struct Mesh {
    #[serde(default)]
    id: Value,
    #[serde(default)]
    name: String,
    positions: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
}

impl Mesh {
    fn validate(&self) -> Result<(), String> {
        if self.positions.is_empty() || self.triangles.is_empty() {
            return Err(format!("mesh {:?} has no triangle geometry", self.name));
        }
        if self.positions.iter().flatten().any(|v| !v.is_finite())
            || self
                .triangles
                .iter()
                .flatten()
                .any(|i| *i as usize >= self.positions.len())
        {
            return Err(format!(
                "mesh {:?} has invalid coordinates or indices",
                self.name
            ));
        }
        Ok(())
    }
}

fn filename(path: &Path) -> &str {
    path.file_stem().and_then(|n| n.to_str()).unwrap_or("Mesh")
}

fn face_vertex(term: &str, vertices: usize) -> Result<u32, String> {
    let v: i64 = term
        .split('/')
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| "invalid OBJ vertex index")?;
    let absolute = if v > 0 { v - 1 } else { vertices as i64 + v };
    if v == 0 || absolute < 0 || absolute >= vertices as i64 {
        return Err("OBJ face vertex index is out of range".into());
    }
    u32::try_from(absolute).map_err(|_| "OBJ has too many vertex indices".into())
}

fn obj_read(path: &Path) -> Result<Vec<Mesh>, String> {
    let data = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut positions = Vec::new();
    let mut parts: Vec<(String, Vec<[u32; 3]>)> = Vec::new();
    let mut name = filename(path).to_string();
    let mut triangles = Vec::new();
    for line in data.lines() {
        let mut words = line.split('#').next().unwrap_or("").split_whitespace();
        match words.next() {
            Some("v") => {
                let mut point = [0.; 3];
                for coordinate in &mut point {
                    *coordinate = words
                        .next()
                        .ok_or("OBJ vertex has fewer than three coordinates")?
                        .parse()
                        .map_err(|_| "invalid OBJ vertex coordinate")?;
                }
                positions.push(point);
            }
            Some("o" | "g") => {
                if !triangles.is_empty() {
                    parts.push((name, std::mem::take(&mut triangles)));
                }
                name = words.collect::<Vec<_>>().join(" ");
            }
            Some("f") => {
                let indices: Vec<_> = words
                    .map(|s| face_vertex(s, positions.len()))
                    .collect::<Result<_, _>>()?;
                if indices.len() < 3 {
                    return Err("OBJ face has fewer than three vertices".into());
                }
                for i in 1..indices.len() - 1 {
                    triangles.push([indices[0], indices[i], indices[i + 1]]);
                }
            }
            _ => {}
        }
    }
    if !triangles.is_empty() {
        parts.push((name, triangles));
    }
    if parts.is_empty() {
        return Err("OBJ has no triangle geometry".into());
    }
    let mut meshes = Vec::new();
    for (name, faces) in parts {
        let mut compact = Vec::new();
        let mut mapping = std::collections::HashMap::new();
        let triangles = faces
            .into_iter()
            .map(|face| {
                face.map(|index| {
                    *mapping.entry(index).or_insert_with(|| {
                        let next = compact.len() as u32;
                        compact.push(positions[index as usize]);
                        next
                    })
                })
            })
            .collect();
        meshes.push(Mesh {
            id: Value::Null,
            name,
            positions: compact,
            triangles,
        });
    }
    Ok(meshes)
}

const BASE64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn from_base64(input: &str) -> Result<Vec<u8>, String> {
    let input = input.as_bytes();
    if input.len() % 4 != 0 {
        return Err("invalid glTF data URL encoding".into());
    }
    let mut bytes = Vec::with_capacity(input.len() / 4 * 3);
    for (i, block) in input.chunks_exact(4).enumerate() {
        let final_block = i + 1 == input.len() / 4;
        let nibble = |j| {
            BASE64
                .iter()
                .position(|x| *x == block[j])
                .map(|v| v as u32)
                .ok_or("invalid glTF data URL byte")
        };
        let padded_c = block[2] == b'=';
        let padded_d = block[3] == b'=';
        if (padded_c && !padded_d) || ((padded_c || padded_d) && !final_block) {
            return Err("invalid glTF data URL padding".into());
        }
        let a = nibble(0)?;
        let b = nibble(1)?;
        let c = if padded_c { 0 } else { nibble(2)? };
        let d = if padded_d { 0 } else { nibble(3)? };
        bytes.push(((a << 2) | (b >> 4)) as u8);
        if !padded_c {
            bytes.push(((b << 4) | (c >> 2)) as u8);
        }
        if !padded_d {
            bytes.push(((c << 6) | d) as u8);
        }
    }
    Ok(bytes)
}

fn to_base64(input: &[u8]) -> String {
    let mut result = String::with_capacity(input.len().div_ceil(3) * 4);
    for bytes in input.chunks(3) {
        let a = bytes[0] as usize;
        let b = bytes.get(1).copied().unwrap_or(0) as usize;
        let c = bytes.get(2).copied().unwrap_or(0) as usize;
        result.push(BASE64[a >> 2] as char);
        result.push(BASE64[((a & 3) << 4) | (b >> 4)] as char);
        result.push(if bytes.len() > 1 {
            BASE64[((b & 15) << 2) | (c >> 6)] as char
        } else {
            '='
        });
        result.push(if bytes.len() > 2 {
            BASE64[c & 63] as char
        } else {
            '='
        });
    }
    result
}

fn external_buffer(path: &Path, uri: &str) -> Result<Vec<u8>, String> {
    if uri.contains([':', '\\', '?', '#'])
        || Path::new(uri)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("glTF buffer must be a relative local file or a base64 data URL".into());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent.canonicalize().map_err(|error| error.to_string())?;
    let file = parent
        .join(uri)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !file.starts_with(&parent) {
        return Err("external glTF buffer escapes the input directory".into());
    }
    fs::read(file).map_err(|error| error.to_string())
}

type Matrix = [[f64; 4]; 4];

fn matrix_product(a: &Matrix, b: &Matrix) -> Matrix {
    std::array::from_fn(|column| {
        std::array::from_fn(|row| (0..4).map(|k| a[k][row] * b[column][k]).sum())
    })
}

fn gltf_meshes(
    node: gltf::Node<'_>,
    parent: &Matrix,
    buffers: &[Vec<u8>],
    meshes: &mut Vec<Mesh>,
    depth: usize,
    units_to_mm: f64,
) -> Result<(), String> {
    if depth > 256 {
        return Err("glTF node hierarchy is too deep or cyclic".into());
    }
    let matrix = node.transform().matrix().map(|row| row.map(f64::from));
    let world = matrix_product(parent, &matrix);
    if let Some(mesh) = node.mesh() {
        let name = mesh.name().or_else(|| node.name()).unwrap_or("Mesh");
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                return Err("glTF input must contain triangle primitives".into());
            }
            let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(Vec::as_slice));
            let positions: Vec<[f64; 3]> = reader
                .read_positions()
                .ok_or("glTF primitive has no POSITION accessor")?
                .map(|point| {
                    std::array::from_fn(|i| {
                        (world[3][i]
                            + (0..3)
                                .map(|j| world[j][i] * f64::from(point[j]))
                                .sum::<f64>())
                            * units_to_mm
                    })
                })
                .collect();
            let indices: Vec<_> = match reader.read_indices() {
                Some(indices) => indices.into_u32().collect(),
                None => (0..u32::try_from(positions.len())
                    .map_err(|_| "glTF mesh has too many vertices")?)
                    .collect(),
            };
            if indices.len() % 3 != 0 {
                return Err("glTF index count is not a multiple of three".into());
            }
            let triangles = indices
                .chunks_exact(3)
                .map(|p| [p[0], p[1], p[2]])
                .collect();
            meshes.push(Mesh {
                id: Value::Null,
                name: name.into(),
                positions,
                triangles,
            });
        }
    }
    for child in node.children() {
        gltf_meshes(child, &world, buffers, meshes, depth + 1, units_to_mm)?;
    }
    Ok(())
}

fn gltf_read(path: &Path) -> Result<Vec<Mesh>, String> {
    let asset = gltf::Gltf::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut buffers = Vec::new();
    for buffer in asset.buffers() {
        let data = match buffer.source() {
            gltf::buffer::Source::Bin => asset.blob.clone().ok_or("GLB has no BIN chunk")?,
            gltf::buffer::Source::Uri(uri) if uri.starts_with("data:") => {
                let (mime, data) = uri.split_once(',').ok_or("invalid glTF buffer data URL")?;
                if !mime.ends_with(";base64") {
                    return Err("glTF data URLs must use base64".into());
                }
                from_base64(data)?
            }
            gltf::buffer::Source::Uri(uri) => external_buffer(path, uri)?,
        };
        if data.len() < buffer.length() {
            return Err("glTF buffer is shorter than declared".into());
        }
        buffers.push(data);
    }
    let mut meshes = Vec::new();
    let identity = std::array::from_fn(|i| std::array::from_fn(|j| if i == j { 1. } else { 0. }));
    let generator = asset
        .document
        .as_json()
        .asset
        .generator
        .as_deref()
        .unwrap_or("");
    let units_to_mm = if generator.starts_with("CLO ") && generator != "CLO Rust" {
        1.
    } else {
        1000.
    };
    let scene = asset
        .default_scene()
        .or_else(|| asset.scenes().next())
        .ok_or("glTF has no scene")?;
    for node in scene.nodes() {
        gltf_meshes(node, &identity, &buffers, &mut meshes, 0, units_to_mm)?;
    }
    if meshes.is_empty() {
        return Err("glTF scene contains no triangle meshes".into());
    }
    Ok(meshes)
}

pub(in crate::compat_scene) fn read_mesh(path: &Path) -> Result<Vec<Value>, String> {
    let meshes = if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("obj"))
    {
        obj_read(path)?
    } else {
        gltf_read(path)?
    };
    meshes
        .into_iter()
        .map(|mesh| {
            mesh.validate()?;
            serde_json::to_value(mesh).map_err(|err| err.to_string())
        })
        .collect()
}

fn cross(a: patterns::Point, b: patterns::Point, c: patterns::Point) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn triangulate(points: &[patterns::Point]) -> Result<Vec<[u32; 3]>, String> {
    let area: f64 = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(p, q)| p.x * q.y - q.x * p.y)
        .sum();
    if area.abs() < 1e-8 {
        return Err("cannot triangulate a zero-area pattern".into());
    }
    let sign = area.signum();
    let mut boundary: Vec<_> = (0..points.len()).collect();
    let mut faces = Vec::new();
    while boundary.len() >= 3 {
        let mut clip = None;
        for (cursor, &b) in boundary.iter().enumerate() {
            let a = boundary[(cursor + boundary.len() - 1) % boundary.len()];
            let c = boundary[(cursor + 1) % boundary.len()];
            let orient = cross(points[a], points[b], points[c]) * sign;
            if orient.abs() < 1e-9 {
                clip = Some((cursor, None));
                break;
            }
            if orient <= 0. {
                continue;
            }
            let contains = boundary
                .iter()
                .copied()
                .filter(|i| *i != a && *i != b && *i != c)
                .any(|i| {
                    cross(points[a], points[b], points[i]) * sign >= -1e-9
                        && cross(points[b], points[c], points[i]) * sign >= -1e-9
                        && cross(points[c], points[a], points[i]) * sign >= -1e-9
                });
            if !contains {
                clip = Some((cursor, Some([a as u32, b as u32, c as u32])));
                break;
            }
        }
        let (cursor, face) =
            clip.ok_or("cannot triangulate the pattern; check for intersecting edges")?;
        boundary.remove(cursor);
        faces.extend(face);
    }
    Ok(faces)
}

fn layout_mesh(pattern: &Value) -> Result<Mesh, String> {
    let points = patterns::outline(pattern)?;
    let positions = points.iter().map(|p| [p.x, p.y, 0.]).collect();
    Ok(Mesh {
        id: pattern["id"].clone(),
        name: pattern["name"].as_str().unwrap_or("Pattern").into(),
        positions,
        triangles: triangulate(&points)?,
    })
}

fn gltf_export(meshes: &[Mesh]) -> Result<(Value, Vec<u8>), String> {
    let mut bin = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut definitions = Vec::new();
    let mut nodes = Vec::new();
    for (i, mesh) in meshes.iter().enumerate() {
        let start = bin.len();
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for point in &mesh.positions {
            for j in 0..3 {
                let p = point[j] as f32;
                if !p.is_finite() {
                    return Err("coordinates exceed the glTF float32 range".into());
                }
                min[j] = min[j].min(p);
                max[j] = max[j].max(p);
                bin.extend_from_slice(&p.to_le_bytes());
            }
        }
        views.push(json!({"buffer": 0, "byteOffset": start, "byteLength": bin.len() - start, "target": 34962}));
        accessors.push(json!({"bufferView": 2 * i, "componentType": 5126, "count": mesh.positions.len(), "type": "VEC3", "min": min, "max": max}));
        let start = bin.len();
        for index in mesh.triangles.iter().flatten() {
            bin.extend_from_slice(&index.to_le_bytes());
        }
        views.push(json!({"buffer": 0, "byteOffset": start, "byteLength": bin.len() - start, "target": 34963}));
        accessors.push(json!({"bufferView": 2 * i + 1, "componentType": 5125, "count": mesh.triangles.len() * 3, "type": "SCALAR"}));
        definitions.push(json!({"name": mesh.name, "primitives": [{"attributes": {"POSITION": 2 * i}, "indices": 2 * i + 1, "mode": 4}]}));
        nodes.push(json!({"mesh": i, "name": mesh.name}));
    }
    let root = json!({
        "asset": {"version": "2.0", "generator": "CLO Rust (millimetres)"}, "scene": 0,
        "scenes": [{"nodes": (0..nodes.len()).collect::<Vec<_>>()}],
        "nodes": nodes, "meshes": definitions, "accessors": accessors, "bufferViews": views, "buffers": [{"byteLength": bin.len()}]
    });
    Ok((root, bin))
}

fn glb_encode(root: &Value, bin: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut json = serde_json::to_vec(root).map_err(|err| err.to_string())?;
    json.resize(json.len().next_multiple_of(4), b' ');
    let length = |n: usize| u32::try_from(n).map_err(|_| "GLB exceeds 4GiB".to_string());
    let mut file = Vec::new();
    file.extend_from_slice(b"glTF");
    file.extend_from_slice(&2u32.to_le_bytes());
    file.extend_from_slice(&length(28 + json.len() + bin.len())?.to_le_bytes());
    file.extend_from_slice(&length(json.len())?.to_le_bytes());
    file.extend_from_slice(b"JSON");
    file.extend_from_slice(&json);
    file.extend_from_slice(&length(bin.len())?.to_le_bytes());
    file.extend_from_slice(b"BIN\0");
    file.extend_from_slice(&bin);
    Ok(file)
}

pub(in crate::compat_scene) fn write_mesh(
    path: &Path,
    format: &str,
    state: &Value,
    include_avatar: bool,
) -> Result<(), String> {
    let mut meshes = Vec::new();
    let simulated = array(&state["simulation"], "meshes");
    if simulated.is_empty() {
        for pattern in array(state, "patterns")
            .iter()
            .filter(|p| !flag(p, "hide", false))
        {
            meshes.push(layout_mesh(pattern)?);
        }
    } else {
        for mesh in simulated {
            if !array(state, "patterns")
                .iter()
                .any(|p| p["id"] == mesh["id"] && flag(p, "hide", false))
            {
                meshes.push(
                    serde_json::from_value(mesh.clone())
                        .map_err(|error| format!("invalid simulation mesh: {error}"))?,
                );
            }
        }
    }
    for mesh in array(state, "imported_meshes") {
        meshes.push(
            serde_json::from_value(mesh.clone())
                .map_err(|error| format!("invalid imported mesh: {error}"))?,
        );
    }
    if include_avatar && !state["avatar"].is_null() {
        let avatar = &state["avatar"];
        if let Some(mesh) = avatar.get("mesh") {
            meshes.push(
                serde_json::from_value(mesh.clone())
                    .map_err(|error| format!("invalid avatar mesh: {error}"))?,
            );
        } else if let Some(avatar_path) = avatar["path"].as_str() {
            for mesh in read_mesh(Path::new(avatar_path))? {
                meshes.push(
                    serde_json::from_value(mesh)
                        .map_err(|error| format!("invalid avatar mesh: {error}"))?,
                );
            }
        } else {
            return Err("the loaded avatar has no exportable triangle mesh".into());
        }
    }
    if meshes.is_empty() {
        return Err("the scene has no mesh geometry to export".into());
    }
    for mesh in &meshes {
        mesh.validate()?;
    }
    if format == "obj" {
        let mut obj = String::from("# CLO Rust, millimetres, Y up\n");
        let mut offset = 1u64;
        for mesh in &meshes {
            let name = mesh.name.replace(['\n', '\r'], " ");
            let _ = writeln!(obj, "o {name}");
            for [x, y, z] in &mesh.positions {
                let _ = writeln!(obj, "v {x:.9} {y:.9} {z:.9}");
            }
            for [a, b, c] in &mesh.triangles {
                let _ = writeln!(
                    obj,
                    "f {} {} {}",
                    offset + u64::from(*a),
                    offset + u64::from(*b),
                    offset + u64::from(*c)
                );
            }
            offset = offset
                .checked_add(mesh.positions.len() as u64)
                .ok_or("OBJ vertex count overflow")?;
        }
        return write(path, obj.as_bytes());
    }
    let (mut root, bin) = gltf_export(&meshes)?;
    if format == "glb" {
        write(path, &glb_encode(&root, bin)?)
    } else {
        root["buffers"][0]["uri"] = json!(format!(
            "data:application/octet-stream;base64,{}",
            to_base64(&bin)
        ));
        super::write_json(path, &root)
    }
}
