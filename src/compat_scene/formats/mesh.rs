mod fbx_export;
#[cfg(not(target_arch = "wasm32"))]
mod fbx_import;
mod obj_export;
mod obj_import;

use super::{
    super::{array, flag, patterns},
    write,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Serialize, Deserialize)]
struct Mesh {
    #[serde(default)]
    id: Value,
    #[serde(default)]
    name: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    fabric_index: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    texcoords: Vec<[f64; 2]>,
    #[serde(skip)]
    rest_mm_uv: bool,
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
        if !self.texcoords.is_empty()
            && (self.texcoords.len() != self.positions.len()
                || self
                    .texcoords
                    .iter()
                    .flatten()
                    .any(|coord| !coord.is_finite()))
        {
            return Err(format!(
                "mesh {:?} has invalid texture coordinates",
                self.name
            ));
        }
        Ok(())
    }
}

fn filename(path: &Path) -> &str {
    path.file_stem().and_then(|n| n.to_str()).unwrap_or("Mesh")
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
                rest_mm_uv: false,
                id: Value::Null,
                name: name.into(),
                fabric_index: Value::Null,
                texcoords: reader.read_tex_coords(0).map_or_else(Vec::new, |coords| {
                    coords.into_f32().map(|uv| uv.map(f64::from)).collect()
                }),
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
        obj_import::read(path, false)?.0
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

pub(in crate::compat_scene) fn read_mesh_scene(path: &Path, format: &str) -> Result<Value, String> {
    let (meshes, fabrics, warnings) = match format {
        "obj" => obj_import::read(path, true)?,
        #[cfg(not(target_arch = "wasm32"))]
        "fbx" => fbx_import::read(path)?,
        _ => return Err(format!("{format} mesh import is unavailable on this target")),
    };
    Ok(json!({"imported_meshes": meshes, "fabrics": fabrics, "warnings": warnings}))
}

fn cross(a: patterns::Point, b: patterns::Point, c: patterns::Point) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

pub(super) fn triangulate(points: &[patterns::Point]) -> Result<Vec<[u32; 3]>, String> {
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
        rest_mm_uv: false,
        id: pattern["id"].clone(),
        name: pattern["name"].as_str().unwrap_or("Pattern").into(),
        fabric_index: pattern["fabric_index"].clone(),
        texcoords: crate::compat_fit::uv_coordinates(
            &points
                .iter()
                .map(|point| [point.x, point.y])
                .collect::<Vec<_>>(),
        ),
        positions,
        triangles: triangulate(&points)?,
    })
}

fn fabric_base_color(fabric: &Value) -> [f64; 4] {
    let rgb: [f64; 3] = serde_json::from_value(fabric["color"].clone()).unwrap_or([1.0; 3]);
    let alpha = fabric["rgba"]
        .as_array()
        .and_then(|rgba| rgba.get(3))
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    [rgb[0], rgb[1], rgb[2], alpha].map(|value| value.clamp(0.0, 1.0))
}

fn write_colorway_image(path: &Path, fabric: &Value, kind: &str, png: &[u8]) -> Result<String, String> {
    let label = fabric["native_texture_names"][kind].as_str()
        .filter(|name| !name.is_empty() && !name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|', '#'])
            && !name.chars().any(char::is_control))
        .unwrap_or(kind);
    for index in 0.. {
        let name = if index == 0 { format!("{label}.png") } else { format!("{label}-{index}.png") };
        let target = path.with_file_name(&name);
        match fs::read(&target) {
            Ok(existing) if existing == png => return Ok(name),
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                write(&target, png)?;
                return Ok(name);
            }
            Err(error) => return Err(format!("{}: {error}", target.display())),
        }
    }
    unreachable!("sidecar suffix counter exhausted")
}

fn gltf_export(
    meshes: &[Mesh],
    fabrics: &[Value],
    external_images: Option<&Path>,
) -> Result<(Value, Vec<u8>), String> {
    let mut bin = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut definitions = Vec::new();
    let mut nodes = Vec::new();
    let mut images = Vec::new();
    let mut textures = Vec::new();
    let mut texture_transform = false;
    let mut materials = Vec::new();
    let mut material_indices = HashMap::new();
    let fabric_by_index: HashMap<_, _> = fabrics
        .iter()
        .filter_map(|fabric| Some((fabric["index"].as_u64()?, fabric)))
        .collect();
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
        let position_accessor = accessors.len();
        accessors.push(json!({"bufferView": views.len(), "componentType": 5126, "count": mesh.positions.len(), "type": "VEC3", "min": min, "max": max}));
        views.push(json!({"buffer": 0, "byteOffset": start, "byteLength": bin.len() - start, "target": 34962}));
        let start = bin.len();
        for index in mesh.triangles.iter().flatten() {
            bin.extend_from_slice(&index.to_le_bytes());
        }
        let indices_accessor = accessors.len();
        accessors.push(json!({"bufferView": views.len(), "componentType": 5125, "count": mesh.triangles.len() * 3, "type": "SCALAR"}));
        views.push(json!({"buffer": 0, "byteOffset": start, "byteLength": bin.len() - start, "target": 34963}));
        let mut attributes = json!({"POSITION": position_accessor});
        if !mesh.texcoords.is_empty() {
            let start = bin.len();
            for coordinate in mesh.texcoords.iter().flatten() {
                let coord = *coordinate as f32;
                if !coord.is_finite() {
                    return Err("texture coordinates exceed the glTF float32 range".into());
                }
                bin.extend_from_slice(&coord.to_le_bytes());
            }
            attributes["TEXCOORD_0"] = json!(accessors.len());
            accessors.push(json!({"bufferView": views.len(), "componentType": 5126, "count": mesh.texcoords.len(), "type": "VEC2"}));
            views.push(json!({"buffer": 0, "byteOffset": start, "byteLength": bin.len() - start, "target": 34962}));
        }
        let mut primitive =
            json!({"attributes": attributes, "indices": indices_accessor, "mode": 4});
        if let Some((id, fabric)) = mesh
            .fabric_index
            .as_u64()
            .and_then(|id| fabric_by_index.get(&id).map(|fabric| (id, *fabric)))
        {
            let kinds = ["base", "normal"];
            let paths = kinds.map(|kind| texture_map(fabric, kind).filter(|_| !mesh.texcoords.is_empty()));
            let key = (id, paths.map(|path| path.is_some()), mesh.rest_mm_uv);
            let material_index = if let Some(index) = material_indices.get(&key) {
                *index
            } else {
                let mut color = fabric_base_color(fabric);
                if fabric["info"]["source"] == "native_saved_material" {
                    // CLO stores colorway RGB as gamma-encoded values and exports linear glTF factors.
                    for channel in &mut color[..3] {
                        *channel = channel.powf(2.2);
                    }
                }
                let mut pbr = json!({"baseColorFactor": color, "metallicFactor": 0.0});
                let mut material = json!({"name": fabric["name"].as_str().unwrap_or("Fabric"), "doubleSided": true});
                if color[3] < 1.0 || paths[0].is_some() {
                    material["alphaMode"] = json!("BLEND");
                }
                for (kind, path) in kinds.into_iter().zip(paths) {
                    let Some(path) = path else { continue };
                    let png = crate::compat_fit::png_for_export(
                        Path::new(path), kind == "base" && fabric["base_texture_desaturated"] == true,
                    )?;
                    let image = if let Some(output) = external_images {
                        json!({"uri": write_colorway_image(output, fabric, kind, &png)?})
                    } else {
                        let start = bin.len();
                        bin.extend_from_slice(&png);
                        let view = views.len();
                        views.push(json!({"buffer": 0, "byteOffset": start, "byteLength": png.len()}));
                        bin.resize(bin.len().next_multiple_of(4), 0);
                        json!({"bufferView": view, "mimeType": "image/png"})
                    };
                    images.push(image);
                    let mut reference = json!({"index": textures.len()});
                    if let Some(mapping) = texture_mapping(fabric, kind).filter(|_| mesh.rest_mm_uv) {
                        let mut transform = json!({"scale": mapping.scale});
                        if mapping.translation != [0.0; 2] {
                            let [u, v] = mapping.rotate(mapping.translation);
                            transform["offset"] = json!([-u * mapping.scale[0], -v * mapping.scale[1]]);
                        }
                        reference["extensions"] = json!({"KHR_texture_transform": transform});
                        texture_transform = true;
                    }
                    textures.push(json!({"source": images.len() - 1}));
                    if kind == "normal" {
                        if let Some(strength) = fabric["native_normal_strength"].as_f64() {
                            reference["scale"] = json!(strength);
                        }
                        material["normalTexture"] = reference;
                    } else {
                        pbr["baseColorTexture"] = reference;
                    }
                }
                material["pbrMetallicRoughness"] = pbr;
                let index = materials.len();
                materials.push(material);
                material_indices.insert(key, index);
                index
            };
            primitive["material"] = json!(material_index);
        }
        definitions.push(json!({"name": mesh.name, "primitives": [primitive]}));
        nodes.push(json!({"mesh": i, "name": mesh.name}));
    }
    let mut root = json!({
        "asset": {"version": "2.0", "generator": "CLO Rust (millimetres)"}, "scene": 0,
        "scenes": [{"nodes": (0..nodes.len()).collect::<Vec<_>>()}],
        "nodes": nodes, "meshes": definitions, "accessors": accessors, "bufferViews": views, "buffers": [{"byteLength": bin.len()}]
    });
    if !materials.is_empty() {
        root["materials"] = json!(materials);
    }
    if !images.is_empty() {
        root["images"] = json!(images);
        root["textures"] = json!(textures);
    }
    if texture_transform {
        root["extensionsUsed"] = json!(["KHR_texture_transform"]);
    }
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

fn procedural_avatar_mesh(avatar: &Value) -> Result<Mesh, String> {
    let mut mesh = Mesh {
        rest_mm_uv: false,
        id: avatar["id"].clone(),
        name: avatar["name"].as_str().unwrap_or("Avatar").to_owned(),
        fabric_index: Value::Null,
        texcoords: Vec::new(),
        positions: Vec::new(),
        triangles: Vec::new(),
    };
    for (positions, triangles) in crate::compat_fit::avatar_geometry(avatar) {
        let offset = mesh.positions.len();
        for [a, b, c] in triangles {
            let index = |i| {
                u32::try_from(offset + i).map_err(|_| "avatar mesh index exceeds u32".to_owned())
            };
            mesh.triangles.push([index(a)?, index(b)?, index(c)?]);
        }
        mesh.positions.extend(positions);
    }
    Ok(mesh)
}

pub(in crate::compat_scene) fn write_mesh(
    path: &Path,
    format: &str,
    state: &Value,
    include_avatar: bool,
    colorways: bool,
) -> Result<Vec<PathBuf>, String> {
    let (meshes, garment_count) = export_geometry(state, include_avatar)?;
    let names = array(state, "colorways");
    let fabrics = array(state, "fabrics");
    if colorways && names.len() > 1 {
        // A native save leaves the glTF exporter on the current colorway until refreshed.
        if state["runtime"]["gltf_export_active_only"] == true {
            if format == "gltf" {
                return write_active_gltf(path, &meshes, garment_count, fabrics);
            }
            if format == "glb" {
                return write_mesh_files(path, format, &meshes, garment_count, fabrics, false);
            }
        }
        return write_colorway_bundle(path, format, state, names, &meshes, garment_count);
    }
    write_mesh_files(path, format, &meshes, garment_count, fabrics, false)
}

fn export_geometry(state: &Value, include_avatar: bool) -> Result<(Vec<Mesh>, usize), String> {
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
                let mut export_mesh: Mesh = serde_json::from_value(mesh.clone())
                    .map_err(|error| format!("invalid simulation mesh: {error}"))?;
                if let Some(pattern) = array(state, "patterns")
                    .iter()
                    .find(|p| p["id"] == mesh["id"])
                {
                    export_mesh.fabric_index = pattern["fabric_index"].clone();
                }
                export_mesh.rest_mm_uv = crate::compat_fit::mesh_uses_rest_mm(state, mesh);
                meshes.push(export_mesh);
            }
        }
        if state["simulation"]["source"] == "mixed_saved_pose" {
            for pattern in array(state, "patterns").iter().filter(|p| !flag(p, "hide", false)) {
                if !simulated.iter().any(|mesh| mesh["id"] == pattern["id"]) {
                    meshes.push(layout_mesh(pattern)?);
                }
            }
        }
    }
    for mesh in array(state, "imported_meshes") {
        meshes.push(
            serde_json::from_value(mesh.clone())
                .map_err(|error| format!("invalid imported mesh: {error}"))?,
        );
    }
    let garment_mesh_count = meshes.len();
    if include_avatar {
        for avatar in std::iter::once(&state["avatar"])
            .chain(array(state, "additional_avatars"))
            .filter(|avatar| !avatar.is_null())
        {
            if let Some(mesh) = avatar.get("mesh") {
                let mut mesh: Mesh = serde_json::from_value(mesh.clone())
                    .map_err(|error| format!("invalid avatar mesh: {error}"))?;
                if mesh.name.is_empty() {
                    mesh.name = avatar["name"].as_str().unwrap_or("Avatar").to_owned();
                }
                meshes.push(mesh);
            } else if let Some(avatar_path) = avatar["path"].as_str() {
                for mesh in read_mesh(Path::new(avatar_path))? {
                    meshes.push(
                        serde_json::from_value(mesh)
                            .map_err(|error| format!("invalid avatar mesh: {error}"))?,
                    );
                }
            } else if avatar["source"] == "rust_procedural_proxy" {
                meshes.push(procedural_avatar_mesh(avatar)?);
            } else {
                return Err("the loaded avatar has no exportable triangle mesh".into());
            }
        }
    }
    if meshes.is_empty() {
        return Err("the scene has no mesh geometry to export".into());
    }
    for mesh in &meshes {
        mesh.validate()?;
    }
    Ok((meshes, garment_mesh_count))
}

fn texture_map<'a>(fabric: &'a Value, kind: &str) -> Option<&'a str> {
    fabric["texture_maps"][kind].as_str().filter(|path| !path.is_empty())
}

struct TextureMapping {
    scale: [f64; 2],
    angle: f64,
    translation: [f64; 2],
}

impl TextureMapping {
    fn rotate(&self, [u, v]: [f64; 2]) -> [f64; 2] {
        let (sin, cos) = self.angle.sin_cos();
        [u * cos - v * sin, u * sin + v * cos]
    }
}

fn texture_mapping(fabric: &Value, kind: &str) -> Option<TextureMapping> {
    texture_map(fabric, kind)?;
    let size = fabric["native_texture_sizes_mm"].get(kind)
        .or_else(|| (kind == "base").then(|| &fabric["native_base_texture_size_mm"]))?;
    let size: [f64; 2] = serde_json::from_value(size.clone()).ok()?;
    if !size.iter().all(|n| n.is_finite() && *n > 0.0) {
        return None;
    }
    let transform = &fabric["native_texture_transforms"][kind];
    let angle = transform["angle_degrees"].as_f64().unwrap_or_default().to_radians();
    let [x, y] = serde_json::from_value(transform["translation_mm"].clone()).unwrap_or_default();
    Some(TextureMapping { scale: size.map(f64::recip), angle, translation: [x, -y] })
}

fn export_uvs<'a>(meshes: &'a [Mesh], fabrics: &[Value], format: &str) -> Cow<'a, [Mesh]> {
    if !meshes.iter().any(|mesh| mesh.rest_mm_uv) {
        return Cow::Borrowed(meshes);
    }
    let use_gltf = matches!(format, "glb" | "gltf");
    let mut prepared = meshes.to_vec();
    for mesh in prepared.iter_mut().filter(|mesh| mesh.rest_mm_uv) {
        let mapping = fabrics.iter().find(|fabric| fabric["index"] == mesh.fabric_index)
            .and_then(|fabric| texture_mapping(fabric, "base").or_else(|| {
                let mut normal = texture_mapping(fabric, "normal")?;
                // With only a normal map, glTF and FBX retain rest coordinates.
                // glTF still emits the normal map's own scale and offset.
                if format != "obj" {
                    normal = TextureMapping { scale: [1.0, 1.0], angle: 0.0, translation: [0.0; 2] };
                }
                Some(normal)
            }));
        if let Some(mapping) = mapping.filter(|_| !mesh.texcoords.is_empty()) {
            let center = std::array::from_fn::<_, 2, _>(|axis| {
                let values = mesh.texcoords.iter().map(|point| point[axis]);
                let (lo, hi) = values.fold((f64::INFINITY, f64::NEG_INFINITY),
                    |(lo, hi), n| (lo.min(n), hi.max(n)));
                (lo + hi) / 2.0
            });
            for point in &mut mesh.texcoords {
                let (u, v) = (point[0] - center[0], center[1] - point[1]);
                *point = if use_gltf {
                    // CLO shares the base rotation across the UV accessor, with per-map scale and offset.
                    mapping.rotate([u, v])
                } else {
                    let [x, y] = mapping.translation;
                    let [s, t] = mapping.rotate([u - x, v - y]);
                    [s * mapping.scale[0], 1.0 + t * mapping.scale[1]]
                };
            }
        } else {
            mesh.rest_mm_uv = false;
            mesh.texcoords = crate::compat_fit::uv_coordinates(&mesh.texcoords);
        }
    }
    Cow::Owned(prepared)
}

fn write_mesh_files(
    path: &Path,
    format: &str,
    meshes: &[Mesh],
    garment_count: usize,
    fabrics: &[Value],
    named_sidecars: bool,
) -> Result<Vec<PathBuf>, String> {
    let meshes = export_uvs(meshes, fabrics, format);
    let mut files = vec![path.to_owned()];
    match format {
        "fbx" => fbx_export::export(path, &meshes, garment_count, fabrics, named_sidecars)?,
        "obj" => files.push(obj_export::export(path, &meshes, fabrics, named_sidecars)?),
        _ => {
            let external_images = (named_sidecars && format == "gltf").then_some(path);
            let (mut root, bin) = gltf_export(&meshes, fabrics, external_images)?;
            if format == "glb" {
                write(path, &glb_encode(&root, bin)?)?;
            } else {
                let uri = if named_sidecars {
                    let buffer = path.with_extension("bin");
                    let name = buffer.file_name().and_then(|name| name.to_str())
                        .ok_or("glTF buffer filename must be valid UTF-8")?.to_owned();
                    write(&buffer, &bin)?;
                    files.push(buffer);
                    name
                } else {
                    format!("data:application/octet-stream;base64,{}", to_base64(&bin))
                };
                root["buffers"][0]["uri"] = json!(uri);
                super::write_json(path, &root)?;
            }
        }
    }
    Ok(files)
}

struct StagedExport(PathBuf);

impl StagedExport {
    fn new(parent: &Path) -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let folder = parent.join(format!(".clo-colorways-{}-{id}", std::process::id()));
            match fs::create_dir(&folder) {
                Ok(()) => return Ok(Self(folder)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("{}: {error}", folder.display())),
            }
        }
    }
}

impl Drop for StagedExport {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_active_gltf(
    path: &Path,
    meshes: &[Mesh],
    garment_count: usize,
    fabrics: &[Value],
) -> Result<Vec<PathBuf>, String> {
    let filename = path.file_name().ok_or("export path has no filename")?;
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let stage = StagedExport::new(parent)?;
    let files = write_mesh_files(&stage.0.join(filename), "gltf", meshes, garment_count, fabrics, true)?;
    publish_colorway_files(&stage.0, parent, &[filename.to_owned()])?;
    Ok(files.iter().map(|file| path.with_file_name(file.file_name().unwrap())).collect())
}

fn write_colorway_bundle(
    path: &Path,
    format: &str,
    state: &Value,
    names: &[Value],
    meshes: &[Mesh],
    garment_count: usize,
) -> Result<Vec<PathBuf>, String> {
    let stem = path.file_stem().ok_or("export path has no filename")?;
    let mut used = HashSet::new();
    let destinations: Vec<_> = names.iter().map(|name| {
        let name = name.as_str()
            .filter(|name| !name.is_empty() && !name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|', '#'])
                && !name.chars().any(char::is_control))
            .ok_or("invalid colorway name for export")?;
        if !used.insert(name.to_lowercase()) {
            return Err("colorway export names must be distinct");
        }
        let mut filename = OsString::from(stem);
        filename.push(format!("_{name}.{format}"));
        Ok(filename)
    }).collect::<Result<_, _>>()?;
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let stage = StagedExport::new(parent)?;
    let mut companions = Vec::new();
    for (index, filename) in destinations.iter().enumerate() {
        let fabrics = array(state, "fabrics").iter()
            .map(|fabric| crate::compat_scene::fabric_for_colorway(state, fabric, index).cloned())
            .collect::<Result<Vec<_>, _>>()?;
        let files = write_mesh_files(&stage.0.join(filename), format, meshes, garment_count, &fabrics, true)?;
        companions.extend(files.iter().skip(1).map(|file| path.with_file_name(file.file_name().unwrap())));
    }
    publish_colorway_files(&stage.0, parent, &destinations)?;
    let mut result: Vec<_> = destinations.iter().map(|name| path.with_file_name(name)).collect();
    result.extend(companions);
    Ok(result)
}

fn publish_colorway_files(stage: &Path, parent: &Path, primary: &[OsString]) -> Result<(), String> {
    let mut files = fs::read_dir(stage)
        .and_then(|entries| entries.map(|entry| entry.map(|entry| entry.file_name())).collect::<Result<Vec<_>, _>>())
        .map_err(|error| error.to_string())?;
    // Publish buffers, material files, and textures before the meshes that reference them.
    files.sort_by_key(|name| primary.contains(name));
    let backups = stage.join("previous");
    fs::create_dir(&backups).map_err(|error| error.to_string())?;
    for name in &files {
        let target = parent.join(name);
        match fs::symlink_metadata(&target) {
            Ok(meta) if meta.is_file() => fs::hard_link(&target, backups.join(name))
                .map_err(|error| format!("{}: {error}", target.display()))?,
            Ok(_) => return Err(format!("{} is not a regular export file", target.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(format!("{}: {error}", target.display())),
        }
    }
    for (index, name) in files.iter().enumerate() {
        let target = parent.join(name);
        if let Err(error) = fs::rename(stage.join(name), &target) {
            for published in files[..index].iter().rev() {
                let target = parent.join(published);
                let old = backups.join(published);
                if old.is_file() {
                    let _ = fs::rename(old, target);
                } else {
                    let _ = fs::remove_file(target);
                }
            }
            return Err(format!("{}: {error}", target.display()));
        }
    }
    Ok(())
}
