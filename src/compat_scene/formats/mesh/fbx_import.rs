use super::{filename, Mesh};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use ufbx::{Material, Node, Texture};

struct Part {
    mesh: Mesh,
    corners: HashMap<(u32, Option<u32>), u32>,
}

impl Part {
    fn new(name: &str, fabric_index: Option<usize>) -> Self {
        Self {
            mesh: Mesh {
                rest_mm_uv: false,
                id: Value::Null,
                name: name.into(),
                fabric_index: json!(fabric_index),
                texcoords: Vec::new(),
                positions: Vec::new(),
                triangles: Vec::new(),
            },
            corners: HashMap::new(),
        }
    }

    fn add_corner(&mut self, node: &Node, source: &ufbx::Mesh, corner: u32) -> Result<u32, String> {
        let corner = corner as usize;
        let vertex = *source
            .vertex_indices
            .get(corner)
            .ok_or("invalid FBX corner index")?;
        let uv = source.vertex_uv.indices.get(corner).copied();
        let key = (vertex, uv);
        if let Some(index) = self.corners.get(&key) {
            return Ok(*index);
        }
        let start = *source
            .vertices
            .get(vertex as usize)
            .ok_or("invalid FBX vertex index")?;
        let p = ufbx::transform_position(&node.geometry_to_world, start);
        if source.vertex_uv.exists {
            let coord = uv
                .and_then(|index| source.vertex_uv.values.get(index as usize))
                .ok_or("invalid FBX UV index")?;
            self.mesh.texcoords.push([coord.x, 1.0 - coord.y]);
        }
        let index =
            u32::try_from(self.mesh.positions.len()).map_err(|_| "too many FBX vertices")?;
        self.mesh.positions.push([p.x, p.y, p.z]);
        self.corners.insert(key, index);
        Ok(index)
    }
}

fn external_image(directory: &Path, texture: &Texture) -> Option<PathBuf> {
    for original in [
        texture.relative_filename.as_ref(),
        texture.filename.as_ref(),
        texture.absolute_filename.as_ref(),
    ] {
        let normalized = original.replace('\\', "/");
        let path = Path::new(&normalized);
        // Some exporters keep the old absolute path in the archive. Check the
        // adjacent basename too so a copied FBX can find its companion images.
        for candidate in [
            directory.join(path),
            directory.join(path.file_name().unwrap_or_default()),
        ] {
            let Ok(candidate) = candidate.canonicalize() else {
                continue;
            };
            if candidate.starts_with(directory)
                && candidate.extension().is_some_and(|ext| {
                    ["png", "jpg", "jpeg"]
                        .iter()
                        .any(|kind| ext.eq_ignore_ascii_case(kind))
                })
                && candidate.is_file()
            {
                return Some(candidate);
            }
        }
    }
    None
}

fn fabric(material: &Material, id: usize, directory: &Path, warnings: &mut Vec<String>) -> Value {
    let [r, g, b] = {
        let v = material.pbr.base_color.value_vec4;
        [v.x, v.y, v.z].map(|channel| channel.clamp(0.0, 1.0))
    };
    let opacity = &material.pbr.opacity;
    let alpha = if opacity.has_value {
        opacity.value_vec4.x.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let mut output = json!({
        "index": id, "name": material.element.name.as_ref(), "color": [r, g, b],
        "rgba": [r, g, b, alpha], "texture_maps": {}, "info": {"source": "fbx"}
    });
    for (kind, map) in [
        ("base", &material.pbr.base_color),
        ("normal", &material.pbr.normal_map),
    ] {
        if let Some(texture) = &map.texture {
            if let Some(path) = external_image(directory, texture) {
                output["texture_maps"][kind] = json!(path);
            } else {
                warnings.push(format!(
                    "FBX {kind} texture is unavailable or unsupported: {}",
                    texture.element.name
                ));
            }
        }
    }
    output
}

pub(super) fn read(path: &Path) -> Result<(Vec<Mesh>, Vec<Value>, Vec<String>), String> {
    let absolute = path
        .canonicalize()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let directory = absolute
        .parent()
        .ok_or("FBX file has no parent directory")?;
    let path_str = absolute
        .to_str()
        .ok_or("FBX input path must be valid UTF-8")?;
    let scene = ufbx::load_file(
        path_str,
        ufbx::LoadOpts {
            file_format: ufbx::FileFormat::Fbx,
            target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
            target_unit_meters: 0.001,
            index_error_handling: ufbx::IndexErrorHandling::AbortLoading,
            ignore_animation: true,
            ignore_embedded: true,
            skip_skin_vertices: true,
            skip_mesh_parts: true,
            node_depth_limit: 256,
            temp_allocator: ufbx::AllocatorOpts {
                memory_limit: 512 * 1024 * 1024,
                ..Default::default()
            },
            result_allocator: ufbx::AllocatorOpts {
                memory_limit: 512 * 1024 * 1024,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .map_err(|error| format!("read FBX {}: {}", path.display(), error.description))?;

    let mut fabrics = Vec::new();
    let mut warnings = Vec::new();
    let mut imported = Vec::new();
    let mut fabric_ids = HashMap::new();
    let mut polygon = Vec::new();
    for node in &scene.nodes {
        let Some(mesh) = node.mesh.as_deref() else {
            continue;
        };
        let name = node.element.name.as_ref();
        let name = if name.is_empty() {
            filename(path)
        } else {
            name
        };
        let mirror = ufbx::matrix_determinant(&node.geometry_to_world) < 0.0;
        let mut parts: Vec<Part> = Vec::new();
        let mut slots = HashMap::new();
        for (index, face) in mesh.faces.iter().copied().enumerate() {
            if face.num_indices < 3 {
                continue;
            }
            let slot = mesh.face_material.get(index).copied().unwrap_or(0) as usize;
            let part = *slots.entry(slot).or_insert_with(|| {
                let material = node
                    .materials
                    .as_ref()
                    .get(slot)
                    .or_else(|| mesh.materials.as_ref().get(slot));
                let fabric_index = material.map(|material| {
                    *fabric_ids
                        .entry(material.element.element_id)
                        .or_insert_with(|| {
                            let id = fabrics.len();
                            fabrics.push(fabric(material, id, directory, &mut warnings));
                            id
                        })
                });
                let index = parts.len();
                parts.push(Part::new(name, fabric_index));
                index
            });
            let part = &mut parts[part];
            let triangles = ufbx::triangulate_face_vec(&mut polygon, mesh, face) as usize;
            for corners in polygon[..triangles * 3].chunks_exact(3) {
                let mut face = [0; 3];
                for (result, &corner) in face.iter_mut().zip(corners) {
                    *result = part.add_corner(node, mesh, corner)?;
                }
                if mirror {
                    face.swap(1, 2);
                }
                part.mesh.triangles.push(face);
            }
        }
        if !mesh.skin_deformers.is_empty() || !mesh.blend_deformers.is_empty() {
            warnings.push(format!(
                "FBX mesh {name} loaded as static geometry; deformers were not imported"
            ));
        }
        for part in parts {
            part.mesh.validate()?;
            imported.push(part.mesh);
        }
    }
    if imported.is_empty() {
        return Err("FBX has no triangle geometry".into());
    }
    Ok((imported, fabrics, warnings))
}
