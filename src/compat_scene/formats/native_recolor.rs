//! Update a native fabric and the records used to reconstruct its single colorway.

use super::native_pose::{encoded_list, map_list, Fields, BYTE_TYPE, LIST_TYPE, MAP_TYPE};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

const UINT: u8 = 5;
const INT: u8 = 4;
const FLOAT: u8 = 6;
const VEC4: u8 = 12;
const BASE_TEXTURE: u32 = 0;
const OPACITY_TEXTURE: u32 = 21;

struct Color {
    rgba: [f32; 4],
    name: String,
    keep_opacity: bool,
}

fn uint(record: &Fields<'_>, field: &[u8], kind: u8) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        record
            .get(field, kind)?
            .try_into()
            .map_err(|_| "invalid native fabric field width")?,
    ))
}

fn update_map(
    record: &Fields<'_>,
    key: &[u8],
    update: impl FnOnce(&Fields<'_>) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    let map = record.map(key)?;
    record.replace(key, MAP_TYPE, &update(&map)?)
}

fn update_list(
    record: &Fields<'_>,
    key: &[u8],
    limit: usize,
    mut update: impl FnMut(usize, &Fields<'_>) -> Result<Option<Vec<u8>>, String>,
) -> Result<Vec<u8>, String> {
    let list = map_list(record.get(key, LIST_TYPE)?, limit)?;
    let mut encoded = Vec::with_capacity(list.len());
    for (index, entry) in list.iter().enumerate() {
        encoded.push(match update(index, entry)? {
            Some(data) => Cow::Owned(data),
            None => Cow::Borrowed(entry.encoded()),
        });
    }
    record.replace(key, LIST_TYPE, &encoded_list(&encoded)?)
}

fn color_map(map: &Fields<'_>, color: &Color, factor: f32) -> Result<Vec<u8>, String> {
    let [r, g, b, a] = color.rgba;
    let floats: Vec<u8> = [r * factor, g * factor, b * factor, a]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    let data = map.replace(b"v4Color", VEC4, &floats)?;
    let data = Fields::read(&data)?.replace(b"qsNameUTF8", BYTE_TYPE, color.name.as_bytes())?;
    Fields::read(&data)?.replace(b"qsName", BYTE_TYPE, color.name.as_bytes())
}

fn update_face(face: &Fields<'_>, color: &Color) -> Result<Option<Vec<u8>>, String> {
    if uint(face, b"enMaterialFaceType", INT)? > 2 {
        return Ok(None);
    }
    let texture_data = face.get(b"listTexture", LIST_TYPE)?;
    let textures = map_list(texture_data, 64)?;
    let mut kept = Vec::with_capacity(textures.len());
    let mut has_base = false;
    for record in &textures {
        let kind = uint(record, b"enType", INT)?;
        has_base |= kind == BASE_TEXTURE;
        if kind != OPACITY_TEXTURE || color.keep_opacity {
            kept.push(Cow::Borrowed(record.encoded()));
        }
    }

    let mut data = update_map(face, b"mapBaseColor", |map| color_map(map, color, 1.0))?;
    // CLO derives ambient from the RGB channels at this factor in the paired native samples.
    data = update_map(&Fields::read(&data)?, b"mapAmbientColor", |map| {
        color_map(map, color, 0.25713187)
    })?;
    if has_base {
        data = Fields::read(&data)?.replace(b"fDesaturated", FLOAT, &1.0f32.to_le_bytes())?;
    }
    if kept.len() != textures.len() {
        data = Fields::read(&data)?.replace(b"listTexture", LIST_TYPE, &encoded_list(&kept)?)?;
    }
    Ok(Some(data))
}

fn update_faces(material: &Fields<'_>, color: &Color) -> Result<Vec<u8>, String> {
    update_list(material, b"listFaceMaterial", 64, |_, face| {
        update_face(face, color)
    })
}

fn material_list(record: &Fields<'_>, color: &Color) -> Result<Vec<u8>, String> {
    update_list(record, b"listMaterial2D", 64, |_, material| {
        Ok(Some(update_faces(material, color)?))
    })
}

fn update_fabric(record: &Fields<'_>, color: &Color) -> Result<Vec<u8>, String> {
    let original = update_map(record, b"mapMaterial2D", |map| update_faces(map, color))?;
    update_map(&Fields::read(&original)?, b"mapColorwayInfo", |info| {
        if map_list(info.get(b"listColorwayInfo", LIST_TYPE)?, 4096)?.len() != 1 {
            return Err("native fabric edits require one verified legacy colorway record".into());
        }
        update_list(info, b"listColorwayInfo", 1, |_, legacy| {
            let legacy = update_faces(legacy, color)?;
            Ok(Some(material_list(&Fields::read(&legacy)?, color)?))
        })
    })
}

fn update_colorway(record: &Fields<'_>, fabrics: &HashMap<u32, Color>) -> Result<Vec<u8>, String> {
    let colorways = record.map(b"mapNewColorway")?;
    let list = map_list(colorways.get(b"listColorway", LIST_TYPE)?, 4096)?;
    if list.len() != 1 || uint(record, b"uiCurrentCoordinationIndex", UINT)? != 0 {
        return Err("native recoloring is only verified for a single colorway".into());
    }
    let assignment = list[0].map(b"mapAssignStyleChip")?;
    let mut selected = HashMap::new();
    for row in map_list(assignment.get(b"listFabricToStyleChip", LIST_TYPE)?, 4096)? {
        let fabric = uint(&row, b"uiStyleID", UINT)?;
        let chip = uint(&row, b"uiStyleChipID", UINT)?;
        if selected.insert(fabric, chip).is_some() {
            return Err("duplicate native fabric style assignment".into());
        }
    }

    let mut updated = HashSet::new();
    let output = update_map(record, b"mapStyleChipAssign", |assignments| {
        update_list(
            assignments,
            b"listFabricToStyleChipVector",
            4096,
            |_, vector| {
                let uid = uint(vector, b"uiStyleID", UINT)?;
                let Some(color) = fabrics.get(&uid) else {
                    return Ok(None);
                };
                let chip_id = selected
                    .get(&uid)
                    .ok_or("native fabric is missing the active style chip")?;
                let encoded = update_list(vector, b"listStyleChip", 4096, |_, chip| {
                    if uint(&chip.map(b"mapElement")?, b"uiID", UINT)? != *chip_id {
                        return Ok(None);
                    }
                    if !updated.insert(uid) {
                        return Err("duplicate native material style chip".into());
                    }
                    Ok(Some(material_list(chip, color)?))
                })?;
                Ok(Some(encoded))
            },
        )
    })?;
    if updated.len() != fabrics.len() {
        return Err("native colorway is missing a target fabric style".into());
    }
    Ok(output)
}

fn scene_color(fabric: &Value, index: usize) -> Result<Color, String> {
    if fabric["index"].as_u64() != Some(index as u64) {
        return Err("native fabric order no longer matches the loaded scene".into());
    }
    let rgba: [f32; 4] = serde_json::from_value(fabric["rgba"].clone())
        .map_err(|error| format!("invalid native fabric color: {error}"))?;
    if rgba.iter().any(|channel| !(0.0..=1.0).contains(channel)) {
        return Err("native fabric color channels must be within 0 and 1".into());
    }
    let [r, g, b, _] = rgba.map(|channel| (channel * 255.0).round() as u8);
    let fabric_name = fabric["name"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let keep_opacity = [
        "chiffon",
        "organza",
        "tulle",
        "mesh",
        "lace",
        "voile",
        "georgette",
        "gauze",
        "eyelet",
        "crochet",
        "pointelle",
        "net",
    ]
    .iter()
    .any(|hint| fabric_name.contains(hint));
    Ok(Color {
        rgba,
        name: format!("{r:02X}{g:02X}{b:02X}"),
        keep_opacity,
    })
}

pub(super) fn recolor(
    data: &[u8],
    state: &Value,
    edited: &[Value],
) -> Result<Option<Vec<u8>>, String> {
    if edited.is_empty() {
        return Ok(None);
    }
    let root = Fields::read(data)?;
    let fabrics = super::super::array(state, "fabrics");
    let mut colors = HashMap::new();
    let rewritten = update_list(&root, b"listFabric", 4096, |index, record| {
        if !edited.iter().any(|id| id.as_u64() == Some(index as u64)) {
            return Ok(None);
        }
        let fabric = fabrics
            .get(index)
            .ok_or("native fabric index is no longer present")?;
        let color = scene_color(fabric, index)?;
        let result = update_fabric(record, &color)?;
        if colors.insert(uint(record, b"uiID", UINT)?, color).is_some() {
            return Err("duplicate native fabric identifier".into());
        }
        Ok(Some(result))
    })?;
    if colors.len() != edited.len() {
        return Err("native fabric edit could not be matched to the archive".into());
    }
    let root = Fields::read(&rewritten)?;
    Ok(Some(update_map(&root, b"mapColorWay", |map| {
        update_colorway(map, &colors)
    })?))
}
