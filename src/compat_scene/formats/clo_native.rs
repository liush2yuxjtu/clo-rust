//! Read the verified panel, sewing, materials, and stored mesh fields from CLO archives.

use super::native_pose::{editable_records, Fields, BYTE_TYPE, LIST_TYPE, MAP_TYPE};
use crate::compat_scene::patterns::Point;
use serde_json::{json, Value};
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::Path;
use zip::{result::ZipError, ZipArchive};

pub(super) const ZPRJ: &[u8] = b" ZPRJ       CLO ";
pub(super) const ZPAC: &[u8; 16] = b" ZPAC       CLO ";
pub(super) const AVT: &[u8; 16] = b" AVT        CLO ";
const NAME: &[u8] = b"qsNameUTF8";
const CATEGORY: &[u8] = b"qsCategoryForDXFUTF8";
const DYNAMICS: &[u8] = b"mapPatternDynamics";
const TRI_COUNT: &[u8] = b"uiTriangleCount";
const VERTEX_COUNT: &[u8] = b"uiVertexCount2D";
const REMESH: &[u8] = b"bNeedToRemeshing";
const REST: &[u8] = b"baRest";
const TRIANGLES: &[u8] = b"baTri";
const BOUNDARY: &[u8] = b"listMeshPointList";
const POINT_INDEX: &[u8] = b"uiMeshPointIndex";
pub(super) const ARCHIVE_LIMIT: u64 = 512 * 1024 * 1024;
const PAYLOAD_LIMIT: u64 = 256 * 1024 * 1024;
const MAX_MEMBERS: usize = 4096;
const MAX_PANELS: usize = 4096;
const MAX_COORDS: usize = 2_000_000;
const MAX_OUTLINE_POINTS: usize = 1_000_000;

pub(super) fn find(data: &[u8], marker: &[u8]) -> Option<usize> {
    data.windows(marker.len()).position(|part| part == marker)
}

pub(super) fn take<'a>(
    data: &'a [u8],
    cursor: &mut usize,
    size: usize,
) -> Result<&'a [u8], String> {
    let end = cursor.checked_add(size).ok_or("PAC offset overflow")?;
    let selected = data.get(*cursor..end).ok_or("truncated PAC field")?;
    *cursor = end;
    Ok(selected)
}

pub(super) fn uint(data: &[u8], cursor: &mut usize) -> Result<usize, String> {
    Ok(u32::from_le_bytes(take(data, cursor, 4)?.try_into().unwrap_or_default()) as usize)
}

pub(super) fn consume(data: &[u8], cursor: &mut usize, marker: &[u8]) -> Result<(), String> {
    if take(data, cursor, marker.len())? == marker {
        Ok(())
    } else {
        Err(format!(
            "expected PAC field {:?}",
            String::from_utf8_lossy(marker)
        ))
    }
}

enum ArchiveOpenError {
    Malformed(String),
    Other(String),
}

impl ArchiveOpenError {
    fn message(self) -> String {
        match self {
            Self::Malformed(message) | Self::Other(message) => message,
        }
    }
}

fn parse_archive<R: Read + Seek>(mut reader: R, header: &[u8]) -> Result<ZipArchive<R>, ArchiveOpenError> {
    let mut prefix = [0u8; 16];
    reader.read_exact(&mut prefix).map_err(|error| {
        let message = format!("read native file header: {error}");
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            ArchiveOpenError::Malformed(message)
        } else {
            ArchiveOpenError::Other(message)
        }
    })?;
    if prefix != header {
        return Err(ArchiveOpenError::Malformed("unexpected native CLO file header".into()));
    }
    reader.rewind()
        .map_err(|error| ArchiveOpenError::Other(format!("seek before native archive: {error}")))?;
    let archive = ZipArchive::new(reader).map_err(|error| {
        let message = format!("read embedded CLO archive: {error}");
        if matches!(error, ZipError::InvalidArchive(_)) {
            ArchiveOpenError::Malformed(message)
        } else {
            ArchiveOpenError::Other(message)
        }
    })?;
    if archive.len() > MAX_MEMBERS {
        return Err(ArchiveOpenError::Other("native CLO archive has too many entries".into()));
    }
    Ok(archive)
}

pub(super) fn open_archive<R: Read + Seek>(reader: R, header: &[u8]) -> Result<ZipArchive<R>, String> {
    parse_archive(reader, header).map_err(ArchiveOpenError::message)
}

fn scene_archive<R: Read + Seek>(reader: R, header: &[u8]) -> Result<Option<ZipArchive<R>>, String> {
    match parse_archive(reader, header) {
        Ok(archive) => Ok(Some(archive)),
        Err(ArchiveOpenError::Malformed(_)) => Ok(None),
        Err(error) => Err(error.message()),
    }
}

pub(super) fn optional_member<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    extension: &str,
) -> Result<Option<(String, Vec<u8>)>, String> {
    let suffix = format!(".{extension}");
    let index = {
        let mut matching = archive
            .file_names_raw()
            .enumerate()
            .filter_map(|(i, name)| {
                let start = name.len().checked_sub(suffix.len())?;
                name[start..]
                    .eq_ignore_ascii_case(suffix.as_bytes())
                    .then_some(i)
            });
        let Some(i) = matching.next() else {
            return Ok(None);
        };
        if matching.next().is_some() {
            return Err(format!(
                "expected exactly one {suffix} payload in native CLO archive"
            ));
        }
        i
    };
    archive_member(archive, index, &suffix).map(Some)
}

fn archive_member<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    index: usize,
    suffix: &str,
) -> Result<(String, Vec<u8>), String> {
    let entry = archive
        .by_index(index)
        .map_err(|e| format!("open native {suffix} member: {e}"))?;
    if entry.is_dir() || entry.compressed_size() > PAYLOAD_LIMIT || entry.size() > PAYLOAD_LIMIT {
        return Err(format!(
            "native {suffix} payload exceeds the supported file size"
        ));
    }
    let expected = entry.size() as usize;
    let name = entry
        .name()
        .map(|decoded| decoded.into_owned())
        .unwrap_or_else(|_| String::from_utf8_lossy(entry.name_raw()).into_owned());
    let mut bytes = Vec::new();
    entry
        .take(PAYLOAD_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read native {suffix} member: {e}"))?;
    if bytes.len() != expected {
        return Err(format!("unexpected native {suffix} payload length"));
    }
    Ok((name, bytes))
}

pub(super) fn member<R: Read + Seek>(archive: &mut ZipArchive<R>, extension: &str) -> Result<Vec<u8>, String> {
    optional_member(archive, extension)?
        .map(|(_, bytes)| bytes)
        .ok_or_else(|| format!("missing .{extension} payload in native CLO archive"))
}

pub(super) fn avatar_members<R: Read + Seek>(archive: &ZipArchive<R>) -> Result<Vec<usize>, String> {
    let indices: Vec<_> = archive.file_names_raw().enumerate()
        .filter_map(|(index, name)| name.get(name.len().saturating_sub(4)..)
            .filter(|tail| tail.eq_ignore_ascii_case(b".avt")).map(|_| index))
        .collect();
    if indices.len() > 32 {
        return Err("native project has more than 32 embedded avatars".into());
    }
    Ok(indices)
}

pub(super) fn avatar_payload<R: Read + Seek>(
    archive: &mut ZipArchive<R>, index: usize,
) -> Result<(String, Vec<u8>), String> {
    archive_member(archive, index, ".avt")
}

pub(in crate::compat_scene) fn read_project_avatar_dan(
    path: &Path,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut project = open_archive(bounded_file(path)?, ZPRJ)?;
    let mut avatars = Vec::new();
    let mut payload_bytes = 0u64;
    for index in avatar_members(&project)? {
        let (name, avatar) = avatar_payload(&mut project, index)?;
        let mut archive = open_archive(Cursor::new(avatar), AVT)?;
        let dan = member(&mut archive, "dan")?;
        payload_bytes += dan.len() as u64;
        if payload_bytes > ARCHIVE_LIMIT {
            return Err("embedded avatar payloads exceed 512 MiB".into());
        }
        avatars.push((name, dan));
    }
    Ok(avatars)
}

pub(super) fn bounded_file(path: &Path) -> Result<std::io::BufReader<File>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if size > ARCHIVE_LIMIT {
        return Err("native CLO file exceeds 512 MiB".into());
    }
    Ok(std::io::BufReader::new(file))
}

pub(in crate::compat_scene) fn read_member(
    path: &Path,
    header: &[u8; 16],
    extension: &str,
) -> Result<Vec<u8>, String> {
    member(&mut open_archive(bounded_file(path)?, header)?, extension)
}

struct NamedPanel<'a> {
    start: usize,
    body: usize,
    name: &'a str,
}

pub(super) fn token<'a>(
    data: &'a [u8],
    marker: &[u8],
    max_length: usize,
) -> Option<(&'a str, usize)> {
    let end = find(&data[..data.len().min(max_length + marker.len())], marker)?;
    let text = std::str::from_utf8(&data[..end]).ok()?;
    if text.chars().any(char::is_control) {
        return None;
    }
    Some((text, end + marker.len()))
}

fn panel_tags(data: &[u8]) -> Result<Vec<NamedPanel<'_>>, String> {
    let mut result = Vec::new();
    let mut cursor = 0;
    while let Some(next) = find(&data[cursor..], NAME) {
        let start = cursor + next;
        cursor = start + NAME.len();
        let Some((name, used)) = token(&data[cursor..], CATEGORY, 255) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let Some((_, suffix)) = token(&data[cursor + used..], DYNAMICS, 255) else {
            continue;
        };
        cursor += used + suffix;
        result.push(NamedPanel {
            start,
            body: cursor,
            name,
        });
        if result.len() > MAX_PANELS {
            return Err("native CLO archive has too many panel records".into());
        }
    }
    Ok(result)
}

pub(super) fn sized_list<'a>(
    data: &'a [u8],
    cursor: &mut usize,
    item_code: u8,
    max_count: usize,
) -> Result<(&'a [u8], usize), String> {
    let count = uint(data, cursor)?;
    if count == 0 || count > max_count || take(data, cursor, 1)? != [item_code] {
        return Err("unexpected PAC list count or element type".into());
    }
    Ok((take(data, cursor, count * 4)?, count))
}

pub(super) fn sizes(table: &[u8]) -> impl Iterator<Item = usize> + '_ {
    table
        .chunks_exact(4)
        .map(|entry| u32::from_le_bytes(entry.try_into().unwrap_or_default()) as usize)
}

fn coord(positions: &[u8], index: usize) -> Result<[f64; 2], String> {
    let start = index
        .checked_mul(8)
        .ok_or("PAC coordinate offset overflow")?;
    let (x, y) = positions
        .get(
            start
                ..start
                    .checked_add(8)
                    .ok_or("PAC coordinate offset overflow")?,
        )
        .ok_or("PAC boundary vertex index is out of range")?
        .split_at(4);
    let x = f64::from(f32::from_le_bytes(x.try_into().unwrap_or_default()));
    let y = f64::from(f32::from_le_bytes(y.try_into().unwrap_or_default()));
    if !x.is_finite() || !y.is_finite() {
        return Err("non-finite native PAC coordinates".into());
    }
    Ok([x, y])
}

fn mesh(data: &[u8]) -> Result<(usize, usize, &[u8]), String> {
    let mut search = 0;
    while let Some(next) = find(&data[search..], TRI_COUNT) {
        let tag = search + next;
        search = tag + TRI_COUNT.len();
        let mut cursor = search;
        let triangles = uint(data, &mut cursor)?;
        if consume(data, &mut cursor, VERTEX_COUNT).is_err() {
            continue;
        }
        let vertices = uint(data, &mut cursor)?;
        if consume(data, &mut cursor, REMESH).is_err() {
            continue;
        }
        take(data, &mut cursor, 1)?;
        if consume(data, &mut cursor, REST).is_err() {
            continue;
        }
        if triangles == 0 || triangles > 2 * MAX_COORDS || vertices == 0 || vertices > MAX_COORDS {
            return Err("unsupported native PAC mesh counts".into());
        }
        let Some(triangles_start) = tag.checked_sub(triangles * 12) else {
            return Err("truncated native PAC triangle list".into());
        };
        let field = triangles_start
            .checked_sub(TRIANGLES.len())
            .ok_or("missing PAC triangle marker")?;
        if &data[field..triangles_start] != TRIANGLES {
            return Err("invalid PAC triangle marker".into());
        }
        if sizes(&data[triangles_start..tag]).any(|id| id >= vertices) {
            return Err("native PAC triangle index is out of range".into());
        }
        return Ok((field, triangles, take(data, &mut cursor, vertices * 8)?));
    }
    Err("missing native PAC rest mesh".into())
}

pub(super) fn mesh_fields<'a>(mesh: &Fields<'a>) -> Result<(&'a [u8], &'a [u8]), String> {
    let count = |key| {
        let bytes = mesh.get(key, 5)?;
        let value = u32::from_le_bytes(bytes.try_into().map_err(|_| "invalid PAC mesh count width")?);
        Ok::<_, String>(value as usize)
    };
    let vertex_count = count(VERTEX_COUNT)?;
    let triangle_count = count(TRI_COUNT)?;
    if vertex_count == 0 || vertex_count > MAX_COORDS
        || triangle_count == 0 || triangle_count > 2 * MAX_COORDS
    {
        return Err("unsupported native PAC mesh counts".into());
    }
    let rest = mesh.get(REST, BYTE_TYPE)?;
    let triangles = mesh.get(TRIANGLES, BYTE_TYPE)?;
    if rest.len() != vertex_count * 8 || triangles.len() != triangle_count * 12 {
        return Err("native PAC mesh has inconsistent array lengths".into());
    }
    if sizes(triangles).any(|index| index >= vertex_count) {
        return Err("native PAC triangle index is out of range".into());
    }
    Ok((triangles, rest))
}

fn outline(data: &[u8]) -> Result<(Vec<Value>, usize, usize), String> {
    let (mesh_start, triangles, positions) = mesh(data)?;
    let outline_data = &data[..mesh_start];
    let start =
        find(outline_data, BOUNDARY).ok_or("missing native PAC boundary list")? + BOUNDARY.len();
    let (lines, vertices) = outline_boundary(&outline_data[start..], positions, |node| {
        let mut field = find(node, POINT_INDEX).ok_or("missing native PAC boundary vertex")?
            + POINT_INDEX.len();
        uint(node, &mut field)
    })?;
    Ok((lines, vertices, triangles))
}

fn typed_outline(record: &Fields<'_>) -> Result<(Vec<Value>, usize, usize), String> {
    let mesh = record.map(b"mapFabricShape2D")?.map(b"mapMesh2D")?;
    let (triangles, positions) = mesh_fields(&mesh)?;
    let boundary = mesh.get(BOUNDARY, LIST_TYPE)?;
    let (lines, vertices) = outline_boundary(boundary, positions, |node| {
        let index = Fields::read(node)?.get(POINT_INDEX, 5)?;
        Ok(u32::from_le_bytes(index.try_into().map_err(|_| "invalid PAC boundary vertex width")?)
            as usize)
    })?;
    Ok((lines, vertices, triangles.len() / 12))
}

fn outline_boundary(
    boundary: &[u8],
    positions: &[u8],
    vertex_index: impl Fn(&[u8]) -> Result<usize, String>,
) -> Result<(Vec<Value>, usize), String> {
    let mut cursor = 0;
    let (lines, count) = sized_list(boundary, &mut cursor, LIST_TYPE, 65536)?;
    if count < 2 {
        return Err("native PAC boundary has fewer than two edges".into());
    }
    let (mut first, mut expected) = (None, None);
    let mut points = Vec::new();
    for size in sizes(lines) {
        let line = take(boundary, &mut cursor, size)?;
        let mut inner = 0;
        let (indices, path_count) = sized_list(line, &mut inner, MAP_TYPE, MAX_COORDS)?;
        if path_count < 2 || points.len().saturating_add(path_count) > MAX_OUTLINE_POINTS {
            return Err("native PAC boundary has too few or too many samples".into());
        }
        for (i, size) in sizes(indices).enumerate() {
            let node = take(line, &mut inner, size)?;
            let point = coord(positions, vertex_index(node)?)?;
            if i == 0 {
                if expected.is_some_and(|p| p != point) {
                    return Err("native PAC boundary edges are disconnected".into());
                }
                first.get_or_insert(point);
            }
            if i + 1 == path_count {
                expected = Some(point);
            } else {
                points.push(
                    Point {
                        x: point[0],
                        y: point[1],
                        kind: if i == 0 { 0 } else { 2 },
                    }
                    .record(),
                );
            }
        }
        if inner != line.len() {
            return Err("unexpected native PAC boundary length".into());
        }
    }
    if first != expected {
        return Err("native PAC boundary does not close".into());
    }
    if points.len() < 3 {
        return Err("native PAC boundary has fewer than three physical samples".into());
    }
    Ok((points, positions.len() / 8))
}

fn parse(pac: &[u8]) -> Result<Option<Value>, String> {
    let mut patterns = Vec::new();
    let mut edge_counts = Vec::new();
    let colorway_table_missing;
    let (mut rest_vertices, mut triangles) = (0usize, 0usize);
    let mut append_outline = |name: &str, outline: Result<(Vec<Value>, usize, usize), String>|
        -> Result<(), String> {
        let (points, verts, faces) = outline
            .map_err(|error| format!("native PAC panel {name:?}: {error}"))?;
        if rest_vertices.saturating_add(verts) > MAX_COORDS {
            return Err("native PAC project has too many rest mesh vertices".into());
        }
        rest_vertices += verts;
        triangles += faces;
        edge_counts.push(points.iter().filter(|point| point.get(2).is_none()).count());
        patterns.push(json!({
            "id": patterns.len() + 1, "name": name, "points": points, "sampled_polyline": true, "arrangement": "", "fabric_index": 0
        }));
        Ok(())
    };
    if let Ok(root) = Fields::read(pac) {
        colorway_table_missing = !root.fields().any(|(name, _, _)| name == b"mapColorWay");
        for (_, name, record) in editable_records(&root)? {
            append_outline(name, typed_outline(&record))?;
        }
    } else {
        colorway_table_missing = false;
        // Older partially decoded payloads can still contain ordered panel and mesh records.
        let tags = panel_tags(pac)?;
        if tags.is_empty() {
            return Ok(None);
        }
        for (i, panel) in tags.iter().enumerate() {
            let end = tags.get(i + 1).map_or(pac.len(), |record| record.start);
            append_outline(panel.name, outline(&pac[panel.body..end]))?;
        }
    }
    if patterns.is_empty() {
        return Err("native PAC has no recognized editable panel records".into());
    }
    let panel_settings_error = super::native_pose::restore_panel_settings(pac, &mut patterns).err();
    let sewing = super::native_seams::read(pac, &patterns, &edge_counts)?;
    let (fabrics, fabric_error) = match super::native_materials::read(pac, &patterns) {
        Ok(materials) => {
            for (pattern, index) in patterns.iter_mut().zip(materials.assignments) {
                pattern["fabric_index"] = json!(index);
            }
            (materials.entries, None)
        }
        Err(error) => (Vec::new(), Some(error)),
    };
    let (pose, pose_error) = match super::native_pose::read(pac, &patterns) {
        Ok(mut meshes) => {
            for (pattern, mesh) in patterns.iter_mut().zip(&mut meshes) {
                if let Some(rest) = mesh.as_object_mut().and_then(|record| record.remove("inner_seam_rest")) {
                    pattern["inner_seam_rest"] = rest;
                }
            }
            (meshes, None)
        }
        Err(error) => (Vec::new(), Some(error)),
    };
    let colorways = super::native_materials::read_colorways(pac).ok();
    let native_import = json!({
        "rest_vertices_validated": rest_vertices, "rest_triangles_validated": triangles,
        "seam_groups_recovered": sewing.annotations.len(),
        "full_edge_seams_restored": sewing.constraints.len(),
        "unreadable_seam_records": sewing.unreadable,
        "saved_pose_meshes": pose.len(), "saved_pose_unavailable": pose_error,
        "fabric_records": fabrics.len(), "fabric_records_unavailable": fabric_error,
        "panel_settings_unavailable": panel_settings_error,
        "colorway_count": colorways.as_ref().map(|(names, _)| names.len())
    });
    Ok(Some(json!({
        "patterns": patterns, "seams": sewing.constraints,
        "native_seam_groups": sewing.annotations, "native_import": native_import,
        "saved_pose": pose, "fabrics": fabrics,
        "native_simulation": super::native_simulation::garment_settings(pac),
        "colorway_table_missing": colorway_table_missing,
        "colorways": colorways.as_ref().map(|(names, _)| names),
        "current_colorway": colorways.map(|(_, current)| current)
    })))
}

fn load_garment<R: Read + Seek>(mut archive: ZipArchive<R>, pac: &[u8]) -> Result<Option<Value>, String> {
    let Some(mut state) = parse(pac)? else {
        return Ok(None);
    };
    if let Err(error) = super::native_textures::hydrate(pac, &mut archive, &mut state) {
        state["native_texture_warnings"] = json!([error]);
    }
    Ok(Some(state))
}

fn scene_garment<R: Read + Seek>(mut archive: ZipArchive<R>) -> Result<Option<Value>, String> {
    let Some((_, pac)) = optional_member(&mut archive, "pac")? else {
        return Ok(None);
    };
    load_garment(archive, &pac)
}

pub(in crate::compat_scene) enum SceneGarment {
    UnreadableArchive,
    Empty,
    Loaded { garment: Value, intact: bool },
}

fn locate_footerless_directory(data: &[u8]) -> Option<(usize, usize, usize)> {
    const CENTRAL: &[u8] = b"PK\x01\x02";
    const LOCAL: &[u8] = b"PK\x03\x04";
    let u16_at = |bytes: &[u8], offset| {
        u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as usize
    };
    let mut search = ZPAC.len();
    while let Some(relative) = find(data.get(search..)?, CENTRAL) {
        let central = search + relative;
        let mut cursor = central;
        let mut entries = Vec::new();
        while entries.len() < MAX_MEMBERS {
            let Some(header) = data.get(cursor..cursor + 46).filter(|header| header.starts_with(CENTRAL)) else {
                break;
            };
            let name_end = cursor + 46 + u16_at(header, 28);
            let end = name_end + u16_at(header, 30) + u16_at(header, 32);
            let local_offset = u32::from_le_bytes(header[42..46].try_into().ok()?) as usize;
            if end > data.len() || local_offset >= central || u16_at(header, 34) != 0 {
                break;
            }
            entries.push((local_offset, &data[cursor + 46..name_end]));
            cursor = end;
        }
        if cursor == data.len() && !entries.is_empty() {
            let matches_base = |base: usize| {
                entries.iter().all(|(relative, name)| {
                    let start = base + relative;
                    if start < ZPAC.len() {
                        return false;
                    }
                    let Some(header) = data[..central].get(start..start + 30) else {
                        return false;
                    };
                    header.starts_with(LOCAL)
                        && u16_at(header, 26) == name.len()
                        && data[..central].get(start + 30..start + 30 + name.len()) == Some(*name)
                })
            };
            if matches_base(0) {
                return Some((central, 0, entries.len()));
            }
            if entries.iter().any(|entry| entry.0 == 0) {
                let base = data[ZPAC.len()..central].windows(LOCAL.len()).enumerate()
                    .map(|(offset, bytes)| (offset + ZPAC.len(), bytes))
                    .find_map(|(base, bytes)| (bytes == LOCAL && matches_base(base)).then_some(base));
                if let Some(base) = base {
                    return Some((central, base, entries.len()));
                }
            }
        }
        search = cursor.max(central + CENTRAL.len());
    }
    None
}

pub(super) fn restore_zpac_footer(data: &mut Vec<u8>) -> bool {
    if !data.starts_with(ZPAC) || data.len() as u64 > ARCHIVE_LIMIT {
        return false;
    }
    let Some((central, base, count)) = locate_footerless_directory(data) else {
        return false;
    };
    let size = (data.len() - central) as u32;
    let offset = (central - base) as u32;
    // The directory and its local file records are intact. Recreate the
    // missing ZIP end record in memory and let the ZIP reader validate it.
    data.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    data.extend_from_slice(&(count as u16).to_le_bytes());
    data.extend_from_slice(&(count as u16).to_le_bytes());
    data.extend_from_slice(&size.to_le_bytes());
    data.extend_from_slice(&offset.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    true
}

fn read_footerless_garment(path: &Path) -> Result<SceneGarment, String> {
    let mut file = bounded_file(path)?;
    let mut prefix = [0; 16];
    match file.read_exact(&mut prefix) {
        Ok(()) if &prefix == ZPAC => {}
        Ok(()) => return Ok(SceneGarment::UnreadableArchive),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Ok(SceneGarment::UnreadableArchive);
        }
        Err(error) => return Err(error.to_string()),
    }
    let mut data = prefix.to_vec();
    file.take(ARCHIVE_LIMIT + 1 - prefix.len() as u64)
        .read_to_end(&mut data)
        .map_err(|error| error.to_string())?;
    if data.len() as u64 > ARCHIVE_LIMIT {
        return Err("native CLO file exceeds 512 MiB".into());
    }
    if !restore_zpac_footer(&mut data) {
        return Ok(SceneGarment::UnreadableArchive);
    }
    let Some(archive) = scene_archive(Cursor::new(data), ZPAC)? else {
        return Ok(SceneGarment::UnreadableArchive);
    };
    Ok(match scene_garment(archive)? {
        Some(garment) => SceneGarment::Loaded { garment, intact: false },
        None => SceneGarment::Empty,
    })
}

pub(in crate::compat_scene) fn read_scene_garment(path: &Path) -> Result<SceneGarment, String> {
    let Some(archive) = scene_archive(bounded_file(path)?, ZPAC)? else {
        return read_footerless_garment(path);
    };
    Ok(match scene_garment(archive)? {
        Some(garment) => SceneGarment::Loaded { garment, intact: true },
        None => SceneGarment::Empty,
    })
}

pub(in crate::compat_scene) fn read_scene_project(path: &Path) -> Result<Option<Value>, String> {
    let Some(mut outer) = scene_archive(bounded_file(path)?, ZPRJ)? else {
        return Ok(None);
    };
    let Some((_, garment)) = optional_member(&mut outer, "zpac")? else {
        return Ok(None);
    };
    let Some(inner) = scene_archive(Cursor::new(garment), ZPAC)? else {
        return Ok(None);
    };
    scene_garment(inner)
}

pub(in crate::compat_scene) fn read_project_garment(path: &Path) -> Result<Value, String> {
    let mut outer = open_archive(bounded_file(path)?, ZPRJ)?;
    let garment = member(&mut outer, "zpac")?;
    let mut inner = open_archive(Cursor::new(garment), ZPAC)?;
    let pac = member(&mut inner, "pac")?;
    load_garment(inner, &pac)?
        .ok_or_else(|| "native PAC has no recognized editable panel records".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    fn write_uint(bytes: &mut Vec<u8>, value: usize) {
        bytes.extend_from_slice(&(value as u32).to_le_bytes());
    }

    fn list(code: u8, entries: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_uint(&mut bytes, entries.len());
        bytes.push(code);
        for entry in entries {
            write_uint(&mut bytes, entry.len());
        }
        for entry in entries {
            bytes.extend_from_slice(entry);
        }
        bytes
    }

    fn sample() -> Vec<u8> {
        let mut data =
            b"qsNameUTF8Synthetic frontqsCategoryForDXFUTF8mapPatternDynamicsdummy".to_vec();
        data.extend_from_slice(BOUNDARY);
        let lines = [[0, 1, 2], [2, 3, 3], [3, 0, 0]];
        let mut wrapped = Vec::new();
        for (i, entries) in lines.iter().enumerate() {
            let records: Vec<_> = entries[..if i == 0 { 3 } else { 2 }]
                .iter()
                .map(|&id| {
                    let mut source = POINT_INDEX.to_vec();
                    write_uint(&mut source, id);
                    source
                })
                .collect();
            wrapped.push(list(0x16, &records));
        }
        data.extend(list(0x14, &wrapped));
        data.extend_from_slice(TRIANGLES);
        for v in [0, 1, 3, 1, 2, 3] {
            write_uint(&mut data, v);
        }
        data.extend_from_slice(TRI_COUNT);
        write_uint(&mut data, 2);
        data.extend_from_slice(VERTEX_COUNT);
        write_uint(&mut data, 4);
        data.extend_from_slice(REMESH);
        data.push(0);
        data.extend_from_slice(REST);
        for coord in [0f32, 0., 40., -2., 80., 0., 40., 100.] {
            data.extend_from_slice(&coord.to_le_bytes());
        }
        data
    }

    fn zip_member(header: &[u8], name: &str, bytes: &[u8]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                name,
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
        let mut all = header.to_vec();
        all.extend([0u8; 31]);
        all.extend(writer.finish().unwrap().into_inner());
        all
    }

    #[test]
    fn native_nested_archive_panel_geometry_and_bad_topology() {
        let garment = zip_member(ZPAC, "synthetic.pac", &sample());
        let footer = garment.len() - 22;
        assert_eq!(&garment[footer..footer + 4], b"PK\x05\x06");
        assert_eq!(locate_footerless_directory(&garment[..footer]).unwrap().2, 1);
        let project = zip_member(ZPRJ, "synthetic.zpac", &garment);
        let mut outer = open_archive(Cursor::new(project), ZPRJ).unwrap();
        let mut inner =
            open_archive(Cursor::new(member(&mut outer, "zpac").unwrap()), ZPAC).unwrap();
        let payload = member(&mut inner, "pac").unwrap();
        let extracted = parse(&payload).unwrap().unwrap();
        assert_eq!(extracted["patterns"][0]["name"], "Synthetic front");
        assert_eq!(
            extracted["patterns"][0]["points"],
            json!([[0., 0.], [40., -2., 2], [80., 0.], [40., 100.]])
        );
        assert_eq!(extracted["native_import"]["rest_vertices_validated"], 4);
        let mut corrupt = payload;
        let i = find(&corrupt, TRIANGLES).unwrap() + TRIANGLES.len();
        corrupt[i] = 9;
        assert!(parse(&corrupt)
            .unwrap_err()
            .contains("triangle index is out of range"));
    }
}
