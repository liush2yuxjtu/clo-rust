//! Read static Y-up geometry and arrangement points from a native avatar archive.

pub(super) mod arrangement;

use super::mesh_measure::{Region, RegionFace};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::{ops::Range, path::Path};

const HEADER: &[u8; 16] = b" AVT        CLO ";
const MAP: u8 = 22;
const LIST: u8 = 20;
const BYTES: u8 = 15;
const UINT: u8 = 5;
const MAX_PARTS: usize = 256;
const MAX_VERTICES: usize = 1_000_000;
const MAX_TRIANGLES: usize = 2_000_000;
const MAX_ARRANGEMENTS: usize = 2048;

type Point = [f64; 3];
type Triangle = [usize; 3];

pub(super) struct RestMesh {
    pub points: Vec<Point>,
    pub triangles: Vec<Triangle>,
    pub part_count: usize,
    pub arrangements: Vec<Value>,
    pub native_arrangements: BTreeMap<String, arrangement::CurvedPlacement>,
    pub body_faces: Option<Range<usize>>,
    pub regions: Vec<RegionFace>,
    pub(super) scene_name: Option<String>,
}

struct Field<'a> {
    name: &'a [u8],
    kind: u8,
    value: &'a [u8],
}

fn take<'a>(source: &mut &'a [u8], length: usize) -> Result<&'a [u8], String> {
    let (value, remaining) = source
        .split_at_checked(length)
        .ok_or("truncated native avatar DAN buffer")?;
    *source = remaining;
    Ok(value)
}

fn le_uint(bytes: &[u8]) -> Result<usize, String> {
    let le = bytes
        .try_into()
        .map_err(|_| "expected a little-endian u32 in native avatar DAN")?;
    Ok(u32::from_le_bytes(le) as usize)
}

fn fields(mut record: &[u8]) -> Result<Vec<Field<'_>>, String> {
    // The map header contains parallel arrays of key lengths, field kinds, and value lengths.
    let count = usize::from(take(&mut record, 1)?[0]);
    let names = take(&mut record, count)?;
    let types = take(&mut record, count)?;
    let sizes = take(&mut record, count * 4)?;
    let mut fields = Vec::with_capacity(count);
    for ((&name_length, &kind), length) in names.iter().zip(types).zip(sizes.chunks_exact(4)) {
        let name = take(&mut record, usize::from(name_length))?;
        let value = take(&mut record, le_uint(length)?)?;
        if name.is_empty() || fields.iter().any(|field: &Field<'_>| field.name == name) {
            return Err("invalid or repeated native avatar DAN field name".into());
        }
        fields.push(Field { name, kind, value });
    }
    if !record.is_empty() {
        return Err("unexpected trailing bytes in native avatar DAN map".into());
    }
    Ok(fields)
}

fn find<'a>(fields: &[Field<'a>], key: &str, kind: u8) -> Option<&'a [u8]> {
    fields
        .iter()
        .find(|field| field.name == key.as_bytes() && field.kind == kind)
        .map(|field| field.value)
}

fn require<'a>(fields: &[Field<'a>], key: &str, kind: u8) -> Result<&'a [u8], String> {
    find(fields, key, kind)
        .ok_or_else(|| format!("missing or incorrectly typed native avatar field {key}"))
}

fn element_name<'a>(record: &[Field<'a>]) -> Option<&'a [u8]> {
    let element = fields(find(record, "mapElement", MAP)?).ok()?;
    find(&element, "qsNameUTF8", BYTES).or_else(|| find(&element, "qsName", BYTES))
}

fn material_shapes<'a>(root: &[Field<'a>]) -> Option<Vec<(&'a [u8], &'a [u8])>> {
    let mut nodes = vec![fields(find(root, "mapTransformer3D", MAP)?).ok()?];
    let mut shapes = Vec::new();
    let mut remaining = 4096;
    while let Some(node) = nodes.pop() {
        remaining = usize::checked_sub(remaining, 1)?;
        if let Some((name, records)) = element_name(&node).zip(find(&node, "listMatShape", LIST)) {
            shapes.push((name, records));
        }
        if let Some(children) = find(&node, "listChildrenTransformer3D", LIST) {
            for child in list(children, MAX_PARTS).ok()?.into_iter().rev() {
                nodes.push(fields(child).ok()?);
            }
        }
    }
    Some(shapes)
}

fn mesh_regions(
    shapes: &[u8],
    vertex_range: Range<usize>,
    face_count: usize,
) -> Option<Vec<RegionFace>> {
    let mut regions = Vec::new();
    let mut remaining = face_count;
    for raw_shape in list(shapes, MAX_PARTS).ok()? {
        let shape = fields(raw_shape).ok()?;
        let materials = list(find(&shape, "listMatMesh", LIST)?, MAX_PARTS).ok()?;
        let shape = fields(find(&shape, "mapShape", MAP)?).ok()?;
        let indexed = list(find(&shape, "listIndexedMesh", LIST)?, MAX_PARTS).ok()?;
        let render_to_mesh = find(&shape, "baRenderToPhysicalPos", BYTES)?;
        if materials.len() != indexed.len() || render_to_mesh.len() % 4 != 0 {
            return None;
        }
        for (material, indices) in materials.into_iter().zip(indexed) {
            let material = fields(material).ok()?;
            let material = fields(find(&material, "mapMaterial", MAP)?).ok()?;
            let name = std::str::from_utf8(element_name(&material)?).ok()?;
            let indexed = fields(indices).ok()?;
            let indices = find(&indexed, "baPositionIndex", BYTES)?;
            if indices.len() % 12 != 0 {
                return None;
            }
            remaining = remaining.checked_sub(indices.len() / 12)?;
            let Some(region) = Region::from_material(name) else {
                continue;
            };
            for triangle in indices.chunks_exact(12) {
                let mut face = [0; 3];
                for (index, raw) in face.iter_mut().zip(triangle.chunks_exact(4)) {
                    let offset = le_uint(raw).ok()?.checked_mul(4)?;
                    let local =
                        le_uint(render_to_mesh.get(offset..offset.checked_add(4)?)?).ok()?;
                    if local >= vertex_range.len() {
                        return None;
                    }
                    *index = vertex_range.start + local;
                }
                regions.push((face, region));
            }
        }
    }
    // The render mesh can triangulate the same vertices differently from the
    // physical mesh. OBJ export uses the render indices and material order.
    (remaining == 0).then_some(regions)
}

fn list(mut data: &[u8], max_items: usize) -> Result<Vec<&[u8]>, String> {
    let count = le_uint(take(&mut data, 4)?)?;
    if count == 0 {
        return if data.is_empty() {
            Ok(Vec::new())
        } else {
            Err("unexpected data after an empty native avatar list".into())
        };
    }
    if count > max_items || take(&mut data, 1)?[0] != MAP {
        return Err("native avatar list has unsupported type or item count".into());
    }
    let table = take(&mut data, count * 4)?;
    let entries = table
        .chunks_exact(4)
        .map(|length| take(&mut data, le_uint(length)?))
        .collect::<Result<_, _>>()?;
    if !data.is_empty() {
        return Err("unexpected trailing bytes in native avatar DAN list".into());
    }
    Ok(entries)
}

fn arrangement_records(root: &[Field<'_>]) -> Result<Vec<Value>, String> {
    let Some(properties) = find(root, "listArr", LIST) else {
        return Ok(Vec::new());
    };
    let mut arrangements = Vec::new();
    for (index, record) in list(properties, MAX_ARRANGEMENTS)?.into_iter().enumerate() {
        let record = fields(record)?;
        let name = find(&record, "qsNameUTF8", BYTES).or_else(|| find(&record, "qsName", BYTES));
        if let Some(text) = name.and_then(|value| std::str::from_utf8(value).ok()) {
            if !text.is_empty() && text.len() <= 256 && !text.contains(char::is_control) {
                let mut arrangement = json!({"ArrangementIndex":index.to_string(),"ArrangementName":text});
                if find(&record, "enType", 4) == Some(&1i32.to_le_bytes()) {
                    let values = ["fNormalizedArcLength", "fNormalizedHeight", "fOffset", "fAngle"].map(|key| {
                        let raw = find(&record, key, 6)?.try_into().ok()?;
                        let value = f32::from_le_bytes(raw);
                        value.is_finite().then_some(value)
                    });
                    crate::compat_scene::append_curved_arrangement_fields(&mut arrangement, values);
                }
                arrangements.push(arrangement);
            }
        }
    }
    Ok(arrangements)
}

fn xyz(floats: &[u8]) -> Result<Point, String> {
    let mut point = [0.0; 3];
    for (axis, bytes) in floats.chunks_exact(4).enumerate() {
        let x = f32::from_le_bytes(bytes.try_into().expect("four-byte coordinate"));
        if !x.is_finite() || x.abs() > 100_000.0 {
            return Err("nonfinite or implausible native avatar coordinate in millimetres".into());
        }
        point[axis] = f64::from(x);
    }
    Ok(point)
}

fn append_mesh(ctl: &[Field<'_>], result: &mut RestMesh) -> Result<(), String> {
    let mesh = fields(require(ctl, "mapMesh", MAP)?)?;
    let count = |key| le_uint(require(&mesh, key, UINT)?);
    let num_vertices = count("uiVertexCount")?;
    let num_triangles = count("uiTriangleCount")?;
    if num_vertices == 0 && num_triangles == 0 {
        return Ok(());
    }
    let first = result.points.len();
    if num_vertices < 3
        || num_triangles == 0
        || num_vertices > MAX_VERTICES - first
        || num_triangles > MAX_TRIANGLES - result.triangles.len()
    {
        return Err("native avatar mesh has empty or excessive buffer counts".into());
    }
    let indices = require(&mesh, "baIndex", BYTES)?;
    let positions = require(&mesh, "baPosition", BYTES)?;
    if indices.len() != num_triangles * 12 || positions.len() != num_vertices * 12 {
        return Err(
            "native avatar index or position byte length disagrees with its declared count".into(),
        );
    }
    let first_face = result.triangles.len();
    for triangle in indices.chunks_exact(12) {
        let mut face = [0; 3];
        for (corner, bytes) in face.iter_mut().zip(triangle.chunks_exact(4)) {
            let index = le_uint(bytes)?;
            if index >= num_vertices {
                return Err("native avatar triangle references an out-of-range vertex".into());
            }
            *corner = first + index;
        }
        result.triangles.push(face);
    }
    for position in positions.chunks_exact(12) {
        result.points.push(xyz(position)?);
    }
    if element_name(ctl).is_some_and(|name| name.eq_ignore_ascii_case(b"body"))
        && result
            .body_faces
            .as_ref()
            .is_none_or(|faces| faces.len() < num_triangles)
    {
        result.body_faces = Some(first_face..result.triangles.len());
    }
    result.part_count += 1;
    Ok(())
}

pub(super) fn decode(data: &[u8]) -> Result<RestMesh, String> {
    let root = fields(data)?;
    let controllers = list(require(&root, "listSkinController", LIST)?, MAX_PARTS)?;
    let mut result = RestMesh {
        points: Vec::new(),
        triangles: Vec::new(),
        part_count: 0,
        arrangements: arrangement_records(&root).unwrap_or_default(),
        native_arrangements: arrangement::read(&root),
        body_faces: None,
        regions: Vec::new(),
        scene_name: find(&root, "qsNameUTF8", BYTES)
            .or_else(|| find(&root, "qsName", BYTES))
            .and_then(|raw| std::str::from_utf8(raw).ok())
            .filter(|name| !name.chars().any(char::is_control))
            .map(str::to_owned),
    };
    let shapes = material_shapes(&root).unwrap_or_default();
    let mut body_regions_found = false;
    for (index, controller) in controllers.into_iter().enumerate() {
        let controller = fields(controller)?;
        let first_vertex = result.points.len();
        let first_face = result.triangles.len();
        append_mesh(&controller, &mut result)
            .map_err(|error| format!("native avatar mesh {}: {error}", index + 1))?;
        if let Some(name) = element_name(&controller).filter(|_| first_face < result.triangles.len()) {
            let shape = shapes.iter().find(|(candidate, _)| {
                name.eq_ignore_ascii_case(candidate)
                    || name.strip_suffix(b"_Shape").is_some_and(|name| name.eq_ignore_ascii_case(candidate))
            });
            if let Some(regions) = shape.and_then(|(_, shape)| {
                mesh_regions(shape, first_vertex..result.points.len(), result.triangles.len() - first_face)
            }) {
                body_regions_found |= name.eq_ignore_ascii_case(b"body") && !regions.is_empty();
                result.regions.extend(regions);
            }
        }
    }
    if result.part_count == 0 {
        return Err("native avatar DAN contains no nonempty rest meshes".into());
    }
    if !body_regions_found {
        result.regions.clear();
    }
    Ok(result)
}

pub(super) fn read(path: &Path) -> Result<RestMesh, String> {
    let dan = crate::compat_scene::read_native_archive_member(path, HEADER, "dan")?;
    decode(&dan)
}

pub(super) fn read_project(path: &Path) -> Result<Vec<(String, String, RestMesh)>, String> {
    crate::compat_scene::read_project_avatar_dan(path)?
        .into_iter().map(|(member, dan)| {
            let mut mesh = decode(&dan)?;
            let name = mesh.scene_name.take().unwrap_or_else(|| {
                let basename = member.rsplit(['/', '\\']).next().unwrap_or("avatar");
                Path::new(basename).file_stem().and_then(|stem| stem.to_str()).unwrap_or("avatar").to_owned()
            });
            Ok((name, member, mesh))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat_fit::{dispatch, solver};
    use serde_json::json;
    use std::{
        fs,
        io::{Cursor, Write},
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn record(fields: &[(&str, u8, Vec<u8>)]) -> Vec<u8> {
        let mut data = vec![fields.len() as u8];
        data.extend(fields.iter().map(|(key, _, _)| key.len() as u8));
        data.extend(fields.iter().map(|(_, kind, _)| *kind));
        for (_, _, bytes) in fields {
            data.extend((bytes.len() as u32).to_le_bytes());
        }
        for (key, _, bytes) in fields {
            data.extend(key.as_bytes());
            data.extend(bytes);
        }
        data
    }

    fn map_list(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut data = Vec::from((entries.len() as u32).to_le_bytes());
        if !entries.is_empty() {
            data.push(MAP);
            for item in entries {
                data.extend((item.len() as u32).to_le_bytes());
            }
            for item in entries {
                data.extend(item);
            }
        }
        data
    }

    fn mesh(name: &str, points: &[[f32; 3]], triangles: &[[u32; 3]]) -> Vec<u8> {
        let rest = record(&[
            (
                "baPosition",
                BYTES,
                points
                    .iter()
                    .flatten()
                    .flat_map(|p| p.to_le_bytes())
                    .collect(),
            ),
            (
                "uiTriangleCount",
                UINT,
                (triangles.len() as u32).to_le_bytes().into(),
            ),
            (
                "fStaticFrictionCoefficient",
                6,
                0.5_f32.to_le_bytes().into(),
            ),
            (
                "baIndex",
                BYTES,
                triangles
                    .iter()
                    .flatten()
                    .flat_map(|index| index.to_le_bytes())
                    .collect(),
            ),
            (
                "uiVertexCount",
                UINT,
                (points.len() as u32).to_le_bytes().into(),
            ),
        ]);
        let element = record(&[("qsNameUTF8", BYTES, name.as_bytes().to_vec())]);
        record(&[("mapMesh", MAP, rest), ("mapElement", MAP, element)])
    }

    fn dan(meshes: &[Vec<u8>]) -> Vec<u8> {
        let placements = map_list(&[
            record(&[("qsNameUTF8", BYTES, b"Body_Front_Center_3".to_vec())]),
            record(&[("qsNameUTF8", BYTES, b"Arm_Outside_2_L".to_vec())]),
        ]);
        record(&[
            ("listArr", LIST, placements),
            ("listSkinController", LIST, map_list(meshes)),
        ])
    }

    fn zip_fixture(dan: &[u8], duplicate: bool) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let file = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("Avatar/rest.dan", file).unwrap();
        zip.write_all(dan).unwrap();
        if duplicate {
            zip.start_file("multiple.dan", file).unwrap();
            zip.write_all(dan).unwrap();
        }
        let mut result = HEADER.to_vec();
        result.extend([0_u8; 128]);
        result.extend(zip.finish().unwrap().into_inner());
        result
    }

    struct TempDirectory(PathBuf);
    impl TempDirectory {
        fn new() -> Self {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("clo-rust-avt-{}-{now}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn avatar_load_uses_native_rest_geometry_and_names() {
        let folder = TempDirectory::new();
        let path = folder.0.join("rest.avt");
        let points_a = [
            [-86.25, 20.5, 3.5],
            [121.0, 20.5, 3.5],
            [40.0, 1584.5, 3.5],
            [40.0, 300.0, 90.0],
        ];
        let points_b = [
            [3.0, 80.0, -44.0],
            [90.0, 85.0, -22.0],
            [91.0, 1650.0, -55.0],
        ];
        let a = mesh("body", &points_a, &[[0, 1, 2], [1, 2, 3]]);
        let b = mesh("hair", &points_b, &[[0, 1, 2]]);
        let file = dan(&[a, mesh("", &[], &[]), b]);
        fs::write(&path, zip_fixture(&file, false)).unwrap();
        let mut state = json!({});
        let mut call = |name, args| dispatch(name, &args, &mut state).unwrap();
        let response = call("avatar_load", json!({"name_or_path":path})).unwrap();
        assert_eq!(response, json!({"ok":true,"avatars":["rest"]}));
        let avatar = super::super::assets::avatar_from_path(&path).unwrap();
        assert_eq!(avatar["source"], "clo_avt_rest");
        assert_eq!(
            avatar["rest_geometry"],
            json!({
                "parts":2, "vertices":7, "triangles":3,
                "bounds_mm":[[-86.25,20.5,-55.0],[121.0,1650.0,90.0]]
            })
        );
        assert!(avatar["warnings"][0]
            .as_str()
            .unwrap()
            .contains("rest geometry only"));
        assert_eq!(
            call("arrangement_points", json!({})).unwrap(),
            json!({"count":2,"names":["Body_Front_Center_3","Arm_Outside_2_L"]})
        );
        let measures = call("avatar_measurements", json!({})).unwrap();
        assert_eq!(measures["body"]["height"], 1564.0);
        assert_eq!(measures["source"], "rust_mesh_estimate");
        let image = call("snapshot", json!({"views":["front"]})).unwrap();
        let image_path = image["images"][0]["path"].as_str().unwrap();
        let image_bytes = fs::read(image_path).unwrap();
        assert_eq!(&image_bytes[..8], b"\x89PNG\r\n\x1a\n");
        fs::remove_file(image_path).unwrap();
        let world = solver::avatar_geometry(&state["avatar"]);
        assert_eq!(world[0].0[0], [-86.25, 20.5, 3.5]);
        assert_eq!(world[0].1, vec![[0, 1, 2], [1, 2, 3], [4, 5, 6]]);
        state["simulation"] = json!({"steps": 5});
        let refreshed = dispatch("avatar_measurements", &json!({"refresh": true}), &mut state)
            .unwrap()
            .unwrap();
        assert_eq!(refreshed, measures);
        assert_eq!(state["simulation"]["steps"], 5);
        let prior = state.clone();
        fs::write(&path, zip_fixture(&file, true)).unwrap();
        assert!(
            dispatch("avatar_load", &json!({"name_or_path":path}), &mut state)
                .unwrap()
                .unwrap_err()
                .contains("exactly one")
        );
        assert_eq!(state, prior);
    }

    #[test]
    fn rejects_nonfinite_positions_out_of_range_indices_and_short_records() {
        let valid = [[0.0, 10.0, 12.0], [90.0, 0.0, 20.0], [30.0, 400.0, 50.0]];
        let invalid_floats = [[f32::NAN, 10.0, 12.0], valid[1], valid[2]];
        let invalid = dan(&[mesh("", &invalid_floats, &[[0, 1, 2]])]);
        assert!(decode(&invalid).err().unwrap().contains("nonfinite"));
        let invalid = dan(&[mesh("", &valid, &[[0, 1, 3]])]);
        assert!(decode(&invalid)
            .err()
            .unwrap()
            .contains("out-of-range vertex"));
        assert!(decode(&invalid[..invalid.len() - 1])
            .err()
            .unwrap()
            .contains("truncated"));
    }
}
