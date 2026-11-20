//! Write full-edge sewing groups and their ordered panel mesh paths.

use super::{
    array, element, encode_map, float_bytes, records, Panel, ARRAY, BOOL, BYTE_TYPE, FLOAT, INT,
    LIST_TYPE, MAP_TYPE, TRUE, UINT, VEC3, ZERO,
};
use crate::compat_scene::formats::native_avatar_ids::record_uuid;
use serde_json::Value;

fn endpoint(
    patterns: &[Value],
    panels: &[Panel],
    pattern_id: &Value,
    line: &Value,
    forward: bool,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let slot = patterns
        .iter()
        .position(|pattern| pattern["id"] == *pattern_id)
        .ok_or("native seam refers to an unknown panel")?;
    let line = line
        .as_u64()
        .and_then(|line| usize::try_from(line).ok())
        .filter(|&line| line < panels[slot].edge_vertices.len())
        .ok_or("native seam refers to an unknown panel edge")?;
    let vertices = &panels[slot].edge_vertices[line];
    let mut mesh_path = (vertices.len() as u32).to_le_bytes().to_vec();
    mesh_path.push(UINT);
    if forward {
        mesh_path.extend(vertices.iter().flat_map(|vertex| vertex.to_le_bytes()));
    } else {
        mesh_path.extend(
            vertices
                .iter()
                .rev()
                .flat_map(|vertex| vertex.to_le_bytes()),
        );
    }

    let start = if forward { 0f32 } else { 1f32 };
    let param = encode_map(&[
        (b"fRatioStart", FLOAT, &start.to_le_bytes()),
        (b"uiEndIndex", UINT, &(line as u32).to_le_bytes()),
        (b"uiDepth", UINT, &1u32.to_le_bytes()),
        (b"uiSelfIndex", UINT, &(slot as u32).to_le_bytes()),
        (b"v3Color", VEC3, &float_bytes([0.72, 0.15, 0.28])?),
        (b"uiStartIndex", UINT, &(line as u32).to_le_bytes()),
        (b"fRatioEnd", FLOAT, &(1. - start).to_le_bytes()),
        (b"bDirection", BOOL, &[u8::from(forward)]),
    ])?;
    let seam_line = encode_map(&[
        (b"mapParamLine", MAP_TYPE, &param),
        (b"arrMeshPointIndex", ARRAY, &mesh_path),
    ])?;
    Ok((records(&[param])?, seam_line))
}

pub(super) fn encode(
    state: &Value,
    panels: &[Panel],
    next_id: &mut u32,
) -> Result<Vec<u8>, String> {
    let patterns = array(state, "patterns");
    let seams = array(state, "seams");
    if seams.len() > 200_000 {
        return Err("native archive exceeds the sewing group limit".into());
    }
    let one = 1f32.to_le_bytes();
    let properties = encode_map(&[
        (b"bActive", BOOL, TRUE),
        (b"bAddedByUser", BOOL, TRUE),
        (b"bShow", BOOL, TRUE),
        (b"enSeamOn3D", INT, &3i32.to_le_bytes()),
        (b"fBKScale", FLOAT, &one),
        (b"fRestSineAngle", FLOAT, &ZERO),
    ])?;
    let mut groups = Vec::with_capacity(seams.len());
    for seam in seams {
        if seam["gather"] == true || seam.get("native_group").is_some() {
            return Err("native authoring cannot preserve this sewing group".into());
        }
        let name = seam["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or("native sewing group name is missing")?;
        let (group_a, a) = endpoint(
            patterns,
            panels,
            &seam["pattern_a"],
            &seam["line_a"],
            seam["dir_a"].as_bool().unwrap_or(true),
        )?;
        let (group_b, b) = endpoint(
            patterns,
            panels,
            &seam["pattern_b"],
            &seam["line_b"],
            seam["dir_b"].as_bool().unwrap_or(true),
        )?;
        let pair = encode_map(&[
            (b"bActive", BOOL, TRUE),
            (b"uiLineGID", UINT, &ZERO),
            (b"bAddedByUser", BOOL, TRUE),
            (b"mapSeamLine0", MAP_TYPE, &a),
            (b"mapSeamLine1", MAP_TYPE, &b),
            (b"fBKScale", FLOAT, &one),
            (b"fRestSineAngle", FLOAT, &ZERO),
            (b"arrSpringIndex", ARRAY, &ZERO),
        ])?;
        let pairs = encode_map(&[(b"listSeamLinePair", LIST_TYPE, &records(&[pair])?)])?;
        *next_id += 1;
        let uuid = record_uuid(&seam.to_string().into_bytes(), *next_id as usize);
        groups.push(encode_map(&[
            (b"mapElement", MAP_TYPE, &element(name, *next_id)?),
            (b"qsUuid", BYTE_TYPE, &uuid),
            (b"qsUuidUTF8", BYTE_TYPE, &uuid),
            (b"mapSeamLinePairProperty", MAP_TYPE, &properties),
            (b"mapSeamLinePairList", MAP_TYPE, &pairs),
            (b"listParamLineGroup0", LIST_TYPE, &group_a),
            (b"listParamLineGroup1", LIST_TYPE, &group_b),
        ])?);
    }
    let mut indices = (groups.len() as u32).to_le_bytes().to_vec();
    if !groups.is_empty() {
        indices.push(INT);
        indices.extend((0..groups.len()).flat_map(|_| (-1i32).to_le_bytes()));
    }
    encode_map(&[
        (b"listSeamLinePairGroup", LIST_TYPE, &records(&groups)?),
        (b"arrSelfIndex", ARRAY, &indices),
    ])
}
