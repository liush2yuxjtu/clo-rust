//! Import the rest outline and sampled line topology from observed CLO PAC files.
//! The archive reader never extracts entries. It does not load materials,
//! avatars, sewing, simulation or other workbench state.

use crate::compat_scene::patterns::Point;
use serde_json::{json, Value};
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::Path;
use zip::ZipArchive;

const ZPRJ: &[u8] = b" ZPRJ       CLO ";
const ZPAC: &[u8; 16] = b" ZPAC       CLO ";
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
const ARCHIVE_LIMIT: u64 = 512 * 1024 * 1024;
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

fn open_archive<R: Read + Seek>(mut reader: R, header: &[u8]) -> Result<ZipArchive<R>, String> {
    let mut prefix = [0u8; 16];
    reader
        .read_exact(&mut prefix)
        .map_err(|e| format!("read native file header: {e}"))?;
    if prefix != header {
        return Err("unexpected native CLO file header".into());
    }
    reader
        .rewind()
        .map_err(|e| format!("seek before native archive: {e}"))?;
    let archive = ZipArchive::new(reader).map_err(|e| format!("read embedded CLO archive: {e}"))?;
    if archive.len() > MAX_MEMBERS {
        return Err("native CLO archive has too many entries".into());
    }
    Ok(archive)
}

fn member<R: Read + Seek>(archive: &mut ZipArchive<R>, extension: &str) -> Result<Vec<u8>, String> {
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
        let i = matching
            .next()
            .ok_or_else(|| format!("missing {suffix} payload in native CLO archive"))?;
        if matching.next().is_some() {
            return Err(format!(
                "expected exactly one {suffix} payload in native CLO archive"
            ));
        }
        i
    };
    let entry = archive
        .by_index(index)
        .map_err(|e| format!("open native {suffix} member: {e}"))?;
    if entry.is_dir() || entry.compressed_size() > PAYLOAD_LIMIT || entry.size() > PAYLOAD_LIMIT {
        return Err(format!(
            "native {suffix} payload exceeds the supported file size"
        ));
    }
    let expected = entry.size() as usize;
    let mut bytes = Vec::new();
    entry
        .take(PAYLOAD_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read native {suffix} member: {e}"))?;
    if bytes.len() != expected {
        return Err(format!("unexpected native {suffix} payload length"));
    }
    Ok(bytes)
}

fn bounded_file(path: &Path) -> Result<std::io::BufReader<File>, String> {
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

fn package(path: &Path, project: bool) -> Result<Vec<u8>, String> {
    if !project {
        return crate::compat_scene::read_native_archive_member(path, ZPAC, "pac");
    }
    let mut outer = open_archive(bounded_file(path)?, ZPRJ)?;
    let garment = member(&mut outer, "zpac")?;
    let mut inner = open_archive(Cursor::new(garment), ZPAC)?;
    member(&mut inner, "pac")
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

fn outline(data: &[u8]) -> Result<(Vec<Value>, usize, usize), String> {
    let (mesh_start, triangles, positions) = mesh(data)?;
    let outline_data = &data[..mesh_start];
    let mut cursor =
        find(outline_data, BOUNDARY).ok_or("missing native PAC boundary list")? + BOUNDARY.len();
    let (lines, count) = sized_list(outline_data, &mut cursor, 0x14, 65536)?;
    if count < 3 {
        return Err("native PAC boundary has fewer than three edges".into());
    }
    let (mut first, mut expected) = (None, None);
    let mut points = Vec::new();
    for size in sizes(lines) {
        let line = take(outline_data, &mut cursor, size)?;
        let mut inner = 0;
        let (indices, path_count) = sized_list(line, &mut inner, 0x16, MAX_COORDS)?;
        if path_count < 2 || points.len().saturating_add(path_count) > MAX_OUTLINE_POINTS {
            return Err("native PAC boundary has too few or too many samples".into());
        }
        for (i, size) in sizes(indices).enumerate() {
            let node = take(line, &mut inner, size)?;
            let mut field = find(node, POINT_INDEX).ok_or("missing native PAC boundary vertex")?
                + POINT_INDEX.len();
            let point = coord(positions, uint(node, &mut field)?)?;
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
    Ok((points, positions.len() / 8, triangles))
}

fn parse(pac: &[u8]) -> Result<Value, String> {
    let tags = panel_tags(pac)?;
    if tags.is_empty() {
        return Err("native PAC has no recognized editable panel records".into());
    }
    let mut patterns = Vec::new();
    let mut edge_counts = Vec::new();
    let (mut rest_vertices, mut triangles) = (0usize, 0usize);
    for (i, panel) in tags.iter().enumerate() {
        let end = tags.get(i + 1).map_or(pac.len(), |r| r.start);
        let (points, verts, faces) = outline(&pac[panel.body..end])
            .map_err(|e| format!("native PAC panel {:?}: {e}", panel.name))?;
        if rest_vertices.saturating_add(verts) > MAX_COORDS {
            return Err("native PAC project has too many rest mesh vertices".into());
        }
        rest_vertices += verts;
        triangles += faces;
        edge_counts.push(points.iter().filter(|point| point.get(2).is_none()).count());
        patterns.push(json!({
            "id": i + 1, "name": panel.name, "points": points, "sampled_polyline": true, "arrangement": "", "fabric_index": 0
        }));
    }
    let sewing = super::native_seams::read(pac, &edge_counts)?;
    let native_import = json!({
        "rest_vertices_validated": rest_vertices, "rest_triangles_validated": triangles,
        "seam_groups_recovered": sewing.annotations.len(),
        "full_edge_seams_restored": sewing.constraints.len(),
        "unreadable_seam_records": sewing.unreadable
    });
    Ok(json!({
        "patterns": patterns, "seams": sewing.constraints,
        "native_seam_groups": sewing.annotations, "native_import": native_import
    }))
}

pub(in crate::compat_scene) fn read(path: &Path, project: bool) -> Result<Value, String> {
    parse(&package(path, project)?)
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
        let project = zip_member(ZPRJ, "synthetic.zpac", &garment);
        let mut outer = open_archive(Cursor::new(project), ZPRJ).unwrap();
        let mut inner =
            open_archive(Cursor::new(member(&mut outer, "zpac").unwrap()), ZPAC).unwrap();
        let payload = member(&mut inner, "pac").unwrap();
        let extracted = parse(&payload).unwrap();
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
