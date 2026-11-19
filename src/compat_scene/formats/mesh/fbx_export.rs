use super::{fabric_base_color, write, Mesh};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const VERSION: u32 = 7700;
const RECORD_LEN: usize = 25;

enum Property {
    Integer(i32),
    Id(i64),
    Real(f64),
    Text(String),
    Indices(Vec<i32>),
    Coordinates(Vec<f64>),
}

impl From<&str> for Property {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

fn size(value: usize) -> Result<[u8; 4], String> {
    u32::try_from(value)
        .map(u32::to_le_bytes)
        .map_err(|_| "array exceeds FBX size limits".into())
}

fn array_header(out: &mut Vec<u8>, tag: u8, count: usize, width: usize) -> Result<(), String> {
    out.push(tag);
    out.extend(size(count)?);
    out.extend([0; 4]); // Uncompressed array data.
    out.extend(size(count.saturating_mul(width))?);
    Ok(())
}

impl Property {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), String> {
        match self {
            Self::Integer(value) => {
                out.push(b'I');
                out.extend(value.to_le_bytes());
            }
            Self::Id(value) => {
                out.push(b'L');
                out.extend(value.to_le_bytes());
            }
            Self::Real(value) => {
                out.push(b'D');
                out.extend(value.to_le_bytes());
            }
            Self::Text(value) => {
                out.push(b'S');
                out.extend(size(value.len())?);
                out.extend(value.as_bytes());
            }
            Self::Indices(values) => {
                array_header(out, b'i', values.len(), 4)?;
                out.extend(values.iter().flat_map(|value| value.to_le_bytes()));
            }
            Self::Coordinates(values) => {
                array_header(out, b'd', values.len(), 8)?;
                out.extend(values.iter().flat_map(|value| value.to_le_bytes()));
            }
        }
        Ok(())
    }
}

struct Node {
    name: &'static str,
    properties: Vec<Property>,
    children: Vec<Node>,
}

impl Node {
    fn new(name: &'static str, properties: Vec<Property>, children: Vec<Node>) -> Self {
        Self {
            name,
            properties,
            children,
        }
    }

    fn leaf(name: &'static str, value: Property) -> Self {
        Self::new(name, vec![value], vec![])
    }

    fn group(name: &'static str, children: Vec<Node>) -> Self {
        Self::new(name, vec![], children)
    }

    fn encode(&self, out: &mut Vec<u8>) -> Result<(), String> {
        let start = out.len();
        out.extend([0; RECORD_LEN - 1]);
        out.push(self.name.len() as u8);
        out.extend(self.name.as_bytes());
        let properties_start = out.len();
        for property in &self.properties {
            property.encode(out)?;
        }
        let properties_len = out.len() - properties_start;
        for child in &self.children {
            child.encode(out)?;
        }
        if !self.children.is_empty() {
            out.extend([0; RECORD_LEN]);
        }
        let end = out.len() as u64;
        out[start..start + 8].copy_from_slice(&end.to_le_bytes());
        out[start + 8..start + 16].copy_from_slice(&(self.properties.len() as u64).to_le_bytes());
        out[start + 16..start + 24].copy_from_slice(&(properties_len as u64).to_le_bytes());
        Ok(())
    }
}

use Property::{Coordinates, Id, Indices, Integer, Real, Text};

fn setting(name: &str, kind: &str, subtype: &str, flags: &str, values: Vec<Property>) -> Node {
    let mut properties = vec![name.into(), kind.into(), subtype.into(), flags.into()];
    properties.extend(values);
    Node::new("P", properties, vec![])
}

fn object_name(name: &str, kind: &str) -> Property {
    Text(format!("{}\0\x01{kind}", name.replace('\0', "")))
}

fn connection(from: i64, to: i64, property: Option<&str>) -> Node {
    let properties = match property {
        Some(name) => vec!["OP".into(), Id(from), Id(to), name.into()],
        None => vec!["OO".into(), Id(from), Id(to)],
    };
    Node::new("C", properties, vec![])
}

fn layer_reference(kind: &str) -> Node {
    Node::group(
        "LayerElement",
        vec![
            Node::leaf("Type", kind.into()),
            Node::leaf("TypedIndex", Integer(0)),
        ],
    )
}

fn vertex_normals(mesh: &Mesh) -> impl Iterator<Item = f64> {
    let mut normals = vec![[0.0; 3]; mesh.positions.len()];
    for face in &mesh.triangles {
        let [a, b, c] = face.map(|index| mesh.positions[index as usize]);
        let [ux, uy, uz] = std::array::from_fn(|axis| b[axis] - a[axis]);
        let [vx, vy, vz] = std::array::from_fn(|axis| c[axis] - a[axis]);
        let normal = [uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx];
        for &index in face {
            for (component, weight) in normals[index as usize].iter_mut().zip(normal) {
                *component += weight;
            }
        }
    }
    normals.into_iter().flat_map(|normal| {
        let length = normal[0].hypot(normal[1]).hypot(normal[2]);
        if length.is_finite() && length > 0.0 {
            normal.map(|component| component / length)
        } else {
            [0.0, 0.0, 1.0]
        }
    })
}

fn geometry(
    id: i64,
    name: &str,
    meshes: &[Mesh],
    face_materials: Vec<i32>,
) -> Result<Node, String> {
    let mut vertices = Vec::new();
    let mut faces = Vec::new();
    let mut normals = Vec::new();
    let mut uv = Vec::new();
    let mut uv_indices = Vec::new();
    let has_uv = meshes.iter().any(|mesh| !mesh.texcoords.is_empty());
    for mesh in meshes {
        let offset = i32::try_from(vertices.len() / 3).map_err(|_| "too many FBX vertices")?;
        vertices.extend(mesh.positions.iter().flatten().map(|mm| mm * 0.1));
        normals.extend(vertex_normals(mesh));
        if has_uv {
            if mesh.texcoords.is_empty() {
                uv.extend(mesh.positions.iter().flat_map(|_| [0.0, 0.0]));
            } else {
                uv.extend(mesh.texcoords.iter().flat_map(|[u, v]| [*u, 1.0 - v]));
            }
        }
        for [a, b, c] in &mesh.triangles {
            let vertex_index = |index| {
                i32::try_from(index)
                    .ok()
                    .and_then(|index| index.checked_add(offset))
                    .ok_or("too many FBX vertices")
            };
            let indices = [vertex_index(*a)?, vertex_index(*b)?, vertex_index(*c)?];
            faces.extend([indices[0], indices[1], !indices[2]]);
            if has_uv {
                uv_indices.extend(indices);
            }
        }
    }
    let mut children = vec![
        Node::leaf("GeometryVersion", Integer(124)),
        Node::leaf("Vertices", Coordinates(vertices)),
        Node::leaf("PolygonVertexIndex", Indices(faces)),
        Node::new(
            "LayerElementNormal",
            vec![Integer(0)],
            vec![
                Node::leaf("Version", Integer(101)),
                Node::leaf("Name", "".into()),
                Node::leaf("MappingInformationType", "ByVertice".into()),
                Node::leaf("ReferenceInformationType", "Direct".into()),
                Node::leaf("Normals", Coordinates(normals)),
            ],
        ),
        Node::new(
            "LayerElementMaterial",
            vec![Integer(0)],
            vec![
                Node::leaf("Version", Integer(101)),
                Node::leaf("Name", "".into()),
                Node::leaf("MappingInformationType", "ByPolygon".into()),
                Node::leaf("ReferenceInformationType", "IndexToDirect".into()),
                Node::leaf("Materials", Indices(face_materials)),
            ],
        ),
    ];
    let mut layers = vec![
        Node::leaf("Version", Integer(100)),
        layer_reference("LayerElementMaterial"),
        layer_reference("LayerElementNormal"),
    ];
    if has_uv {
        children.push(Node::new(
            "LayerElementUV",
            vec![Integer(0)],
            vec![
                Node::leaf("Version", Integer(101)),
                Node::leaf("Name", "UVMap".into()),
                Node::leaf("MappingInformationType", "ByPolygonVertex".into()),
                Node::leaf("ReferenceInformationType", "IndexToDirect".into()),
                Node::leaf("UV", Coordinates(uv)),
                Node::leaf("UVIndex", Indices(uv_indices)),
            ],
        ));
        layers.push(layer_reference("LayerElementUV"));
    }
    children.push(Node::new("Layer", vec![Integer(0)], layers));
    Ok(Node::new(
        "Geometry",
        vec![Id(id), object_name(name, "Geometry"), "Mesh".into()],
        children,
    ))
}

fn material(id: i64, name: &str, fabric: Option<&Value>) -> Node {
    let [r, g, b, alpha] = fabric.map_or([1.0; 4], fabric_base_color);
    Node::new(
        "Material",
        vec![Id(id), object_name(name, "Material"), "".into()],
        vec![
            Node::leaf("Version", Integer(102)),
            Node::leaf("ShadingModel", "lambert".into()),
            Node::group(
                "Properties70",
                vec![
                    setting(
                        "DiffuseColor",
                        "Color",
                        "",
                        "A",
                        vec![Real(r), Real(g), Real(b)],
                    ),
                    setting(
                        "TransparencyFactor",
                        "Number",
                        "",
                        "A",
                        vec![Real(1.0 - alpha)],
                    ),
                ],
            ),
        ],
    )
}

fn texture(id: i64, video_id: i64, filename: &str, objects: &mut Vec<Node>) {
    objects.push(Node::new(
        "Texture",
        vec![Id(id), object_name(filename, "Texture"), "".into()],
        vec![
            Node::leaf("Type", "TextureVideoClip".into()),
            Node::leaf("Version", Integer(202)),
            Node::leaf("TextureName", object_name(filename, "Texture")),
            Node::leaf("Media", object_name(filename, "Video")),
            Node::leaf("FileName", filename.into()),
            Node::leaf("RelativeFilename", filename.into()),
            Node::group(
                "Properties70",
                vec![setting("UVSet", "KString", "", "", vec!["UVMap".into()])],
            ),
        ],
    ));
    objects.push(Node::new(
        "Video",
        vec![Id(video_id), object_name(filename, "Video"), "Clip".into()],
        vec![
            Node::leaf("Type", "Clip".into()),
            Node::leaf("Filename", filename.into()),
            Node::leaf("RelativeFilename", filename.into()),
        ],
    ));
}

fn write_texture(path: &Path, source: &str, files: &mut Vec<PathBuf>) -> Result<String, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let png = crate::compat_fit::png_for_export(Path::new(source))?;
    loop {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("clo-fbx-{}-{id}.png", std::process::id());
        let target = path.with_file_name(&name);
        match File::create_new(&target) {
            Ok(mut file) => {
                files.push(target);
                file.write_all(&png)
                    .and_then(|_| file.sync_all())
                    .map_err(|error| format!("write FBX texture {name}: {error}"))?;
                return Ok(name);
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("{}: {error}", target.display())),
        }
    }
}

fn encode(objects: Vec<Node>, connections: Vec<Node>) -> Result<Vec<u8>, String> {
    let mut out = b"Kaydara FBX Binary  \0\x1a\0".to_vec();
    out.extend(VERSION.to_le_bytes());
    let header = Node::group(
        "FBXHeaderExtension",
        vec![
            Node::leaf("FBXHeaderVersion", Integer(1003)),
            Node::leaf("FBXVersion", Integer(VERSION as i32)),
            Node::leaf("Creator", "CLO Rust".into()),
        ],
    );
    let mut settings: Vec<_> = [
        ("UpAxis", 1),
        ("UpAxisSign", 1),
        ("FrontAxis", 2),
        ("FrontAxisSign", 1),
        ("CoordAxis", 0),
        ("CoordAxisSign", 1),
    ]
    .into_iter()
    .map(|(name, value)| setting(name, "int", "Integer", "", vec![Integer(value)]))
    .collect();
    settings.push(setting(
        "UnitScaleFactor",
        "double",
        "Number",
        "",
        vec![Real(1.0)],
    ));
    for node in [
        header,
        Node::group(
            "GlobalSettings",
            vec![
                Node::leaf("Version", Integer(1000)),
                Node::group("Properties70", settings),
            ],
        ),
        Node::group("Objects", objects),
        Node::group("Connections", connections),
    ] {
        node.encode(&mut out)?;
    }
    out.extend([0; RECORD_LEN]);
    out.extend([
        0xfa, 0xbc, 0xab, 0x09, 0xd0, 0xc8, 0xd4, 0x66, 0xb1, 0x76, 0xfb, 0x83, 0x1c, 0xf7, 0x26,
        0x7e,
    ]);
    out.extend([0; 4]);
    out.resize(out.len() + 16 - out.len() % 16, 0);
    out.extend(VERSION.to_le_bytes());
    out.extend([0; 120]);
    out.extend([
        0xf8, 0x5a, 0x8c, 0x6a, 0xde, 0xf5, 0xd9, 0x7e, 0xec, 0xe9, 0x0c, 0xe3, 0x75, 0x8f, 0x29,
        0x0b,
    ]);
    Ok(out)
}

fn write_scene(
    path: &Path,
    groups: impl IntoIterator<Item = Vec<Mesh>>,
    fabrics: &[Value],
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let fabrics: HashMap<_, _> = fabrics
        .iter()
        .filter_map(|fabric| Some((fabric["index"].as_u64()?, fabric)))
        .collect();
    let mut objects = Vec::new();
    let mut connections = Vec::new();
    let mut next = 0;
    let mut id = || {
        next += 1;
        next
    };
    let mut materials = HashMap::new();
    for meshes in groups {
        let geometry_id = id();
        let model_id = id();
        let mut slots = HashMap::new();
        let mut face_materials = Vec::new();
        for mesh in &meshes {
            let fabric = mesh
                .fabric_index
                .as_u64()
                .and_then(|index| fabrics.get(&index).copied());
            let textured = !mesh.texcoords.is_empty();
            let key = (fabric.and_then(|fabric| fabric["index"].as_u64()), textured);
            let material_id = if let Some(existing) = materials.get(&key) {
                *existing
            } else {
                let material_id = id();
                let name = fabric
                    .and_then(|fabric| fabric["name"].as_str())
                    .unwrap_or("Default");
                objects.push(material(material_id, name, fabric));
                if let Some(fabric) = fabric.filter(|_| textured) {
                    for (map, property) in [("base", "DiffuseColor"), ("normal", "NormalMap")] {
                        if let Some(source) = fabric["texture_maps"][map]
                            .as_str()
                            .filter(|path| !path.is_empty())
                        {
                            let filename = write_texture(path, source, files)?;
                            let texture_id = id();
                            let video_id = id();
                            texture(texture_id, video_id, &filename, &mut objects);
                            connections.push(connection(texture_id, material_id, Some(property)));
                            connections.push(connection(video_id, texture_id, None));
                        }
                    }
                }
                materials.insert(key, material_id);
                material_id
            };
            let next_slot = i32::try_from(slots.len()).map_err(|_| "too many FBX materials")?;
            let slot = *slots.entry(material_id).or_insert_with(|| {
                connections.push(connection(material_id, model_id, None));
                next_slot
            });
            face_materials.resize(face_materials.len() + mesh.triangles.len(), slot);
        }
        let name = if meshes.len() == 1 {
            meshes[0].name.as_str()
        } else {
            "Garment"
        };
        objects.push(geometry(geometry_id, name, &meshes, face_materials)?);
        objects.push(Node::new(
            "Model",
            vec![Id(model_id), object_name(name, "Model"), "Mesh".into()],
            vec![Node::leaf("Version", Integer(232))],
        ));
        connections.push(connection(geometry_id, model_id, None));
        connections.push(connection(model_id, 0, None));
    }
    write(path, &encode(objects, connections)?)
}

pub(super) fn export(
    path: &Path,
    mut meshes: Vec<Mesh>,
    garment_mesh_count: usize,
    fabrics: &[Value],
) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let avatars = meshes
        .split_off(garment_mesh_count)
        .into_iter()
        .map(|mesh| vec![mesh]);
    let groups = Some(meshes)
        .filter(|parts| !parts.is_empty())
        .into_iter()
        .chain(avatars);
    let mut files = Vec::new();
    if let Err(error) = write_scene(path, groups, fabrics, &mut files) {
        for file in files {
            let _ = fs::remove_file(file);
        }
        return Err(error);
    }
    Ok(())
}
