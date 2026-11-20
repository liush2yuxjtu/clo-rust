//! Read a stored native drape without claiming that it was solved by the Rust engine.

use super::clo_native::{mesh_fields, sized_list, sizes, take, uint};
use crate::compat_scene::patterns::{curve, curve_length, point_bounds, points, Point};
use crate::compat_scene::{InnerSeamPaths, InnerSeamRest};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::HashSet;

pub(super) const MAP_TYPE: u8 = 22;
pub(super) const LIST_TYPE: u8 = 20;
pub(super) const BYTE_TYPE: u8 = 15;
const UINT_TYPE: u8 = 5;
const LIMIT: usize = 4096;

#[derive(Clone, Copy)]
struct Field<'a> {
    name: &'a [u8],
    kind: u8,
    value: &'a [u8],
}

pub(super) struct Fields<'a>(Vec<Field<'a>>, &'a [u8]);

impl<'a> Fields<'a> {
    pub(super) fn read(data: &'a [u8]) -> Result<Self, String> {
        let mut cursor = 0;
        let count = take(data, &mut cursor, 1)?[0] as usize;
        let keys = take(data, &mut cursor, count)?;
        let types = take(data, &mut cursor, count)?;
        let lengths = take(data, &mut cursor, count * 4)?;
        let mut fields = Vec::with_capacity(count);
        for ((key, &kind), length) in keys.iter().zip(types).zip(sizes(lengths)) {
            let name = take(data, &mut cursor, *key as usize)?;
            let value = take(data, &mut cursor, length)?;
            fields.push(Field { name, kind, value });
        }
        if cursor != data.len() {
            return Err("unexpected bytes after native PAC map".into());
        }
        Ok(Self(fields, data))
    }

    pub(super) fn get(&self, name: &[u8], kind: u8) -> Result<&'a [u8], String> {
        self.0
            .iter()
            .find(|field| field.name == name && field.kind == kind)
            .map(|field| field.value)
            .ok_or_else(|| {
                format!(
                    "native PAC field {} is missing",
                    String::from_utf8_lossy(name)
                )
            })
    }

    pub(super) fn encoded(&self) -> &'a [u8] {
        self.1
    }

    pub(super) fn fields(&self) -> impl Iterator<Item = (&'a [u8], u8, &'a [u8])> + '_ {
        self.0.iter().map(|field| (field.name, field.kind, field.value))
    }

    pub(super) fn replace(&self, name: &[u8], kind: u8, value: &[u8]) -> Result<Vec<u8>, String> {
        self.get(name, kind)?;
        self.set(name, kind, value)
    }

    pub(super) fn set(&self, name: &[u8], kind: u8, value: &[u8]) -> Result<Vec<u8>, String> {
        let mut fields = self.0.to_vec();
        let replacement = Field { name, kind, value };
        let mut found = false;
        for field in fields.iter_mut().filter(|field| field.name == name && field.kind == kind) {
            *field = replacement;
            found = true;
        }
        if !found {
            if fields.iter().any(|field| field.name == name) {
                return Err("native PAC field exists with a different type".into());
            }
            fields.push(replacement);
        }
        encode_fields(&fields)
    }

    pub(super) fn map(&self, name: &[u8]) -> Result<Self, String> {
        Self::read(self.get(name, MAP_TYPE)?)
    }
}

pub(super) fn encode_map(values: &[(&[u8], u8, &[u8])]) -> Result<Vec<u8>, String> {
    let fields: Vec<_> = values.iter().map(|&(name, kind, value)| Field { name, kind, value }).collect();
    encode_fields(&fields)
}

fn encode_fields(fields: &[Field<'_>]) -> Result<Vec<u8>, String> {
    let count = u8::try_from(fields.len()).map_err(|_| "too many native PAC map fields")?;
    let mut output = vec![count];
    for field in fields {
        output.push(u8::try_from(field.name.len()).map_err(|_| "native PAC field name is too long")?);
    }
    output.extend(fields.iter().map(|field| field.kind));
    for field in fields {
        output.extend(
            u32::try_from(field.value.len())
                .map_err(|_| "PAC map field exceeds the size limit")?
                .to_le_bytes(),
        );
    }
    for field in fields {
        output.extend(field.name);
        output.extend(field.value);
    }
    Ok(output)
}

fn inner_seam_paths(record: &Fields<'_>) -> Result<InnerSeamPaths, String> {
    let shapes = map_list(record.get(b"listInnerShape", LIST_TYPE)?, LIMIT)?;
    Ok(shapes
        .iter()
        .map(|shape| {
            let lines = shape
                .get(b"listLine", LIST_TYPE)
                .and_then(|data| map_list(data, LIMIT));
            lines
                .unwrap_or_default()
                .iter()
                .map(|line| {
                    let points = line
                        .get(b"listMeshPoint", LIST_TYPE)
                        .and_then(|data| map_list(data, 65_536));
                    points
                        .unwrap_or_default()
                        .iter()
                        .map(|point| {
                            let index = point.get(b"uiMeshPointIndex", UINT_TYPE).ok()?;
                            Some(u32::from_le_bytes(index.try_into().ok()?) as usize)
                        })
                        .collect::<Option<Vec<_>>>()
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect())
}

fn pattern_pose(record: &Fields, id: &Value, name: &str) -> Result<Value, String> {
    let geometry = record.map(b"mapFabricShape2D")?.map(b"mapMesh2D")?;
    let positions3d = record.get(b"baPosition3D", BYTE_TYPE)?;
    let (triangles, positions2d) = mesh_fields(&geometry)?;
    if positions3d.len() % 12 != 0 || positions3d.len() / 12 != positions2d.len() / 8 {
        return Err(format!(
            "native pose for {name:?} has inconsistent position counts"
        ));
    }
    let positions: Vec<[f64; 3]> = positions3d
        .chunks_exact(12)
        .map(|bytes| {
            std::array::from_fn(|i| {
                f64::from(f32::from_le_bytes(
                    bytes[i * 4..i * 4 + 4].try_into().expect("float width"),
                ))
            })
        })
        .collect();
    let texcoords: Vec<[f64; 2]> = positions2d
        .chunks_exact(8)
        .map(|bytes| {
            std::array::from_fn(|i| {
                f64::from(f32::from_le_bytes(
                    bytes[i * 4..i * 4 + 4].try_into().expect("float width"),
                ))
            })
        })
        .collect();
    if positions
        .iter()
        .flatten()
        .chain(texcoords.iter().flatten())
        .any(|value| !value.is_finite())
    {
        return Err(format!(
            "native pose for {name:?} contains non-finite coordinates"
        ));
    }
    let indices: Vec<[usize; 3]> = triangles
        .chunks_exact(12)
        .map(|bytes| {
            std::array::from_fn(|i| {
                u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().expect("index width"))
                    as usize
            })
        })
        .collect();
    let mut pose = json!({"id": id, "name": name, "positions": positions, "triangles": indices, "texcoords": texcoords});
    if let Ok(mut paths) = inner_seam_paths(record) {
        if paths.iter().flatten().any(|path| path.len() > 1) {
            let edge = |a: usize, b: usize| (a.min(b), a.max(b));
            let edges: HashSet<_> = indices
                .iter()
                .flat_map(|&[a, b, c]| [edge(a, b), edge(b, c), edge(c, a)])
                .collect();
            for path in paths.iter_mut().flatten() {
                if path.len() < 2
                    || path
                        .windows(2)
                        .any(|pair| !edges.contains(&edge(pair[0], pair[1])))
                {
                    path.clear();
                }
            }
            let rest: InnerSeamRest = paths
                .iter()
                .map(|shape| {
                    shape
                        .iter()
                        .map(|path| path.iter().map(|&vertex| texcoords[vertex]).collect())
                        .collect()
                })
                .collect();
            pose["inner_seam_paths"] = json!(paths);
            pose["inner_seam_rest"] = json!(rest);
        }
    }
    Ok(pose)
}

pub(super) fn encoded_list(records: &[Cow<'_, [u8]>]) -> Result<Vec<u8>, String> {
    let mut output = (records.len() as u32).to_le_bytes().to_vec();
    if !records.is_empty() {
        output.push(MAP_TYPE);
        for record in records {
            output.extend(
                u32::try_from(record.len())
                    .map_err(|_| "PAC record too large")?
                    .to_le_bytes(),
            );
        }
        for record in records {
            output.extend_from_slice(record.as_ref());
        }
    }
    Ok(output)
}

pub(super) fn map_list(data: &[u8], limit: usize) -> Result<Vec<Fields<'_>>, String> {
    if data == 0u32.to_le_bytes() {
        return Ok(Vec::new());
    }
    let mut cursor = 0;
    let (lengths, count) = sized_list(data, &mut cursor, MAP_TYPE, limit)?;
    let mut maps = Vec::with_capacity(count);
    for length in sizes(lengths) {
        maps.push(Fields::read(take(data, &mut cursor, length)?)?);
    }
    if cursor != data.len() {
        return Err("unexpected bytes after native PAC map list".into());
    }
    Ok(maps)
}

pub(super) fn panel_records<'a>(
    root: &Fields<'a>,
    panels: &[Value],
) -> Result<Vec<Fields<'a>>, String> {
    Ok(panel_records_with_slots(root, panels)?
        .into_iter()
        .map(|(_, record)| record)
        .collect())
}

pub(super) fn panel_records_with_slots<'a>(
    root: &Fields<'a>,
    panels: &[Value],
) -> Result<Vec<(usize, Fields<'a>)>, String> {
    let mut result = Vec::new();
    for (slot, name, record) in editable_records(root)? {
        if panels
            .get(result.len())
            .is_none_or(|panel| panel["name"] != name)
        {
            return Err("native editable records do not match the imported panel order".into());
        }
        result.push((slot, record));
    }
    if result.len() != panels.len() {
        return Err("native editable records do not match the imported panel count".into());
    }
    Ok(result)
}

pub(super) fn editable_records<'a>(
    root: &Fields<'a>,
) -> Result<Vec<(usize, &'a str, Fields<'a>)>, String> {
    let patterns = root.map(b"mapPatternEditor")?.map(b"mapPatternList")?;
    let records = map_list(patterns.get(b"listPattern", LIST_TYPE)?, LIMIT)?;
    let mut editable = Vec::new();
    for (slot, record) in records.into_iter().enumerate() {
        if u32::from_le_bytes(
            record.get(b"uiPatternType", UINT_TYPE)?
                .try_into().map_err(|_| "invalid native panel type width")?,
        ) != 0 {
            continue;
        }
        let element = record.map(b"mapElement")?;
        let name = std::str::from_utf8(element.get(b"qsNameUTF8", BYTE_TYPE)?)
            .map_err(|e| e.to_string())?;
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err("invalid native editable panel name".into());
        }
        editable.push((slot, name, record));
    }
    Ok(editable)
}

pub(super) fn restore_panel_settings(pac: &[u8], panels: &mut [Value]) -> Result<(), String> {
    let root = Fields::read(pac)?;
    for (record, panel) in panel_records(&root, panels)?.iter().zip(panels) {
        if let Some(mm) = float(record, b"fEdgeLength").filter(|mm| *mm > 0.0) {
            panel["particle_distance"] = json!(mm);
        }
        if let Ok(dynamics) = record.map(b"mapPatternDynamics") {
            if let Some(bytes) = dynamics
                .get(b"iLayer", 4)
                .ok()
                .and_then(|value| value.try_into().ok())
            {
                panel["layer"] = json!(i32::from_le_bytes(bytes));
            }
        }
        if let Ok(arrangement) = record.map(b"mapArrange") {
            if let Some(settings) = arrangement_settings(&arrangement) {
                panel["arrangement"] = settings["ArrangementName"].clone();
                panel["saved_arrangement"] = settings;
            }
        }
        if let Some((metrics, limits, controls)) = outline_metrics(record, panel) {
            panel["native_outline_metrics"] = json!(metrics);
            panel["native_outline_bounds"] = json!(limits);
            panel["native_outline_controls"] = json!(controls);
        }
        if let Some(children) = child_shapes(record) {
            panel["native_children"] = json!(children);
        }
    }
    Ok(())
}

fn coordinates<const N: usize>(record: &Fields<'_>, key: &[u8], kind: u8) -> Option<[f64; N]> {
    let data = record.get(key, kind).ok()?;
    if data.len() != N * 4 {
        return None;
    }
    let mut result = [0.0; N];
    for (value, bytes) in result.iter_mut().zip(data.chunks_exact(4)) {
        *value = f64::from(f32::from_le_bytes(bytes.try_into().ok()?));
        if !value.is_finite() {
            return None;
        }
    }
    Some(result)
}

#[derive(Clone, Copy)]
struct Transform2D([f64; 6]);

impl Transform2D {
    fn vector(self, p: Point) -> Point {
        let [a, b, c, d, _, _] = self.0;
        Point { x: a * p.x + c * p.y, y: b * p.x + d * p.y, ..p }
    }

    fn position(self, p: Point) -> Point {
        let [a, b, c, d, tx, ty] = self.0.map(|n| n as f32);
        let (x, y) = (p.x as f32, p.y as f32);
        Point {
            x: f64::from(a * x + c * y + tx),
            y: f64::from(b * x + d * y + ty),
            ..p
        }
    }
}

fn shape_transform(shape: &Fields<'_>) -> Option<Transform2D> {
    let transform = shape.map(b"mapTransformer2D").ok()?;
    let [a, b, 0.0, c, d, 0.0, tx, ty, 1.0] = coordinates(&transform, b"m3Matrix", 13)? else {
        return None;
    };
    Some(Transform2D([a, b, c, d, tx, ty]))
}

fn shape_point(record: &Fields<'_>, kind: u64) -> Option<Point> {
    let [x, y] = coordinates(record, b"v2Position", 10)?;
    Some(Point { x, y, kind })
}

fn shape_vertices(shape: &Fields<'_>) -> Option<Vec<Point>> {
    map_list(shape.get(b"listPoint", LIST_TYPE).ok()?, LIMIT)
        .ok()?
        .iter()
        .map(|point| shape_point(point, 0))
        .collect()
}

fn line_shape(line: &Fields<'_>, vertices: &[Point]) -> Option<(Vec<Point>, bool)> {
    let endpoint = |key| {
        let i = uint(line.get(key, UINT_TYPE).ok()?, &mut 0).ok()?;
        vertices.get(i).copied()
    };
    let kind = i32::from_le_bytes(line.get(b"iLineType", 4).ok()?.try_into().ok()?);
    let mut edge = vec![endpoint(b"uiStartPointIndex")?];
    match kind {
        0 => (),
        2 | 3 => {
            let control = map_list(line.get(b"listPoint", LIST_TYPE).ok()?, LIMIT).ok()?;
            if control.is_empty() {
                return None;
            }
            for point in control {
                edge.push(shape_point(&point, kind as u64)?);
            }
        }
        _ => return None,
    }
    edge.push(endpoint(b"uiEndPointIndex")?);
    Some((edge, kind == 0))
}

fn line_metrics(edge: &[Point], straight: bool) -> Option<Value> {
    let length = curve_length(edge, false);
    length.is_finite().then(|| json!({"length": length, "straight": straight}))
}

fn outline_metrics(record: &Fields<'_>, panel: &Value) -> Option<(Vec<Value>, [f64; 4], Vec<Value>)> {
    let shape = record.map(b"mapShape2D").ok()?;
    let transform = shape_transform(&shape)?;
    let vertices = shape_vertices(&shape)?;
    let lines = map_list(shape.get(b"listLine", LIST_TYPE).ok()?, LIMIT).ok()?;
    let corners: Vec<_> = points(panel).ok()?.into_iter().filter(|p| p.kind == 0).collect();
    if corners.len() != lines.len() {
        return None;
    }
    let matches = |actual: Point, expected: Point| {
        let actual = transform.position(actual);
        (actual.x - expected.x).hypot(actual.y - expected.y) <= 0.05
    };
    let mut metrics = Vec::with_capacity(lines.len());
    let mut controls = Vec::with_capacity(lines.len());
    let mut limits = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for (i, line) in lines.iter().enumerate() {
        let (edge, straight) = line_shape(line, &vertices)?;
        if !matches(edge[0], corners[i])
            || !matches(*edge.last()?, corners[(i + 1) % corners.len()])
        {
            return None;
        }
        controls.push(corners[i].record());
        for point in &edge[1..edge.len() - 1] {
            let point = transform.position(*point);
            if !point.x.is_finite() || !point.y.is_finite() {
                return None;
            }
            controls.push(point.record());
        }
        let mut sampled = curve(&edge, false);
        let [left, bottom, right, top] =
            point_bounds(sampled.iter().map(|&p| transform.position(p)), [0.0; 2]);
        limits = [limits[0].min(left), limits[1].min(bottom), limits[2].max(right), limits[3].max(top)];
        for point in &mut sampled {
            *point = transform.vector(*point);
        }
        let length = curve_length(&sampled, true);
        if !length.is_finite() {
            return None;
        }
        metrics.push(json!({"length": length, "straight": straight}));
    }
    limits.iter().all(|n| n.is_finite()).then_some((metrics, limits, controls))
}

fn child_shape(shape: &Fields<'_>, parent: Transform2D, kind: &str) -> Option<Value> {
    let transform = shape_transform(shape)?;
    let vertices = shape_vertices(shape)?;
    let lines = map_list(shape.get(b"listLine", LIST_TYPE).ok()?, LIMIT).ok()?;
    let mut metrics = Vec::with_capacity(lines.len());
    for line in &lines {
        let (mut edge, straight) = line_shape(line, &vertices)?;
        let controls = edge
            .iter()
            .map(|&p| {
                let p = parent.position(transform.position(p));
                (p.x.is_finite() && p.y.is_finite()).then(|| p.record())
            })
            .collect::<Option<Vec<_>>>()?;
        for point in &mut edge {
            *point = parent.vector(transform.vector(*point));
        }
        let mut line = line_metrics(&edge, straight)?;
        line["points"] = json!(controls);
        metrics.push(line);
    }
    let points: Vec<_> = vertices
        .into_iter()
        .map(|p| {
            let point = parent.position(transform.position(p));
            (point.x.is_finite() && point.y.is_finite()).then(|| point.record())
        })
        .collect::<Option<_>>()?;
    let visible = shape.get(b"bVisible", 1).ok() != Some(&[0]);
    Some(json!({"type": kind, "points": points, "lines": metrics, "visible": visible}))
}

fn child_shapes(record: &Fields<'_>) -> Option<Vec<Value>> {
    let outer = record.map(b"mapShape2D").ok()?;
    let parent = shape_transform(&outer)?;
    let vertices = shape_vertices(&outer)?;
    let corners: Vec<_> = vertices.into_iter().map(|point| parent.position(point)).collect();
    let interiors = map_list(record.get(b"listInnerShape", LIST_TYPE).ok()?, LIMIT).ok()?;
    let baselines = map_list(record.get(b"listDummyShape", LIST_TYPE).ok()?, LIMIT).ok()?;
    let mut children = Vec::with_capacity(interiors.len() + baselines.len());
    for shape in &interiors {
        children.push(child_shape(shape, parent, "InnerShape")?);
    }
    for baseline in &baselines {
        if baseline.get(b"enDummyType", 4).ok()? != 1i32.to_le_bytes() {
            return None;
        }
        let mut child = child_shape(&baseline.map(b"mapShape2D").ok()?, parent, "Base Line")?;
        if baseline.get(b"bSeamAllowanceShape", 1).ok() == Some(&[1])
            && child["points"].as_array().is_some_and(|points| {
                points.len() == corners.len() && points.iter().zip(&corners).all(|(a, b)| {
                    a[0].as_f64().zip(a[1].as_f64())
                        .is_some_and(|(x, y)| (x - b.x).hypot(y - b.y) <= 0.001)
                })
            })
        {
            child["outline_copy"] = json!(true);
        }
        children.push(child);
    }
    Some(children)
}

fn float(record: &Fields<'_>, key: &[u8]) -> Option<f32> {
    let value = f32::from_le_bytes(record.get(key, 6).ok()?.try_into().ok()?);
    value.is_finite().then_some(value)
}

fn arrangement_settings(record: &Fields<'_>) -> Option<Value> {
    let name = std::str::from_utf8(record.get(b"qsNameUTF8", BYTE_TYPE).ok()?).ok()?;
    if name.is_empty() {
        return None;
    }
    let mut settings = json!({"ArrangementName": name});
    if record.get(b"enType", 4).ok() == Some(&1i32.to_le_bytes()) {
        let values = [b"fNormalizedArcLength".as_slice(), b"fNormalizedHeight", b"fOffset", b"fAngle"]
            .map(|key| float(record, key));
        crate::compat_scene::append_curved_arrangement_fields(&mut settings, values);
    }
    Some(settings)
}

pub(super) fn read(pac: &[u8], panels: &[Value]) -> Result<Vec<Value>, String> {
    panel_records(&Fields::read(pac)?, panels)?
        .iter()
        .zip(panels)
        .map(|(record, panel)| {
            pattern_pose(
                record,
                &panel["id"],
                panel["name"]
                    .as_str()
                    .ok_or("missing imported pattern name")?,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_map_table_respects_types_sizes_and_exact_bounds() {
        let mut bytes = vec![2, 1, 2, UINT_TYPE, BYTE_TYPE];
        bytes.extend(4u32.to_le_bytes());
        bytes.extend(2u32.to_le_bytes());
        bytes.extend(b"x");
        bytes.extend(27u32.to_le_bytes());
        bytes.extend(b"ysok");
        let fields = Fields::read(&bytes).unwrap();
        assert_eq!(fields.get(b"x", UINT_TYPE).unwrap(), 27u32.to_le_bytes());
        assert_eq!(fields.get(b"ys", BYTE_TYPE).unwrap(), b"ok");
        assert!(fields.get(b"x", BYTE_TYPE).is_err());
        assert!(Fields::read(&bytes[..bytes.len() - 1]).is_err());
    }
}
