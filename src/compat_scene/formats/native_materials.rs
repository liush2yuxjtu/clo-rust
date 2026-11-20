//! Restore recorded material identity and front-face color for scene queries and display.

use super::native_pose::{encoded_list, map_list, panel_records, Fields, BYTE_TYPE, LIST_TYPE, MAP_TYPE};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::HashMap;

const INT_TYPE: u8 = 4;
const UINT_TYPE: u8 = 5;
const VEC4_TYPE: u8 = 12;

pub(super) struct Materials {
    pub entries: Vec<Value>,
    pub assignments: Vec<usize>,
}

pub(super) fn read_colorways(pac: &[u8]) -> Result<(Vec<String>, usize), String> {
    let root = Fields::read(pac)?.map(b"mapColorWay")?;
    let is_new = root.get(b"bIsTheNewColorway", 1).ok() == Some(&[1]);
    let records = if is_new {
        root.map(b"mapNewColorway")?
            .get(b"listColorway", LIST_TYPE)?
    } else {
        root.get(b"listColorway", LIST_TYPE)?
    };
    let names = map_list(records, 4096)?
        .iter()
        .map(|record| {
            let name = record
                .get(b"qsNameUTF8", BYTE_TYPE)
                .or_else(|_| record.get(b"qsName", BYTE_TYPE))?;
            std::str::from_utf8(name)
                .map(str::to_owned)
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let current = u32::from_le_bytes(
        root.get(b"uiCurrentCoordinationIndex", UINT_TYPE)?
            .try_into()
            .map_err(|_| "invalid colorway index width")?,
    ) as usize;
    if current >= names.len() {
        return Err("native active colorway index is out of range".into());
    }
    Ok((names, current))
}

fn integer(bytes: &[u8]) -> Result<i32, String> {
    Ok(i32::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| "invalid native signed integer width")?,
    ))
}

fn rgba(bytes: &[u8]) -> Result<[f64; 4], String> {
    if bytes.len() != 16 {
        return Err("invalid native face color width".into());
    }
    let color = std::array::from_fn(|i| {
        let start = i * 4;
        f64::from(f32::from_le_bytes(
            bytes[start..start + 4].try_into().expect("color width"),
        ))
    });
    if color.iter().all(|c| (0.0..=1.0).contains(c)) {
        Ok(color)
    } else {
        Err("invalid native face color component".into())
    }
}

pub(super) fn read_face_colors(material: &Fields<'_>) -> Result<Vec<(i32, [f64; 4])>, String> {
    let (faces, legacy_front) = face_records(material)?;
    faces
        .iter()
        .enumerate()
        .map(|(index, face)| {
            let kind = face_type(face, index, legacy_front)?;
            let color = rgba(face.map(b"mapBaseColor")?.get(b"v4Color", VEC4_TYPE)?)?;
            Ok((kind, color))
        })
        .collect()
}

pub(super) fn face_records<'a>(material: &Fields<'a>) -> Result<(Vec<Fields<'a>>, bool), String> {
    let faces = map_list(material.get(b"listFaceMaterial", LIST_TYPE)?, 64)?;
    let legacy_front = faces.len() == 1 && faces[0].get(b"enMaterialFaceType", INT_TYPE).is_err();
    Ok((faces, legacy_front))
}

pub(super) fn face_type(
    face: &Fields<'_>,
    index: usize,
    legacy_front: bool,
) -> Result<i32, String> {
    if legacy_front && index == 0 {
        return Ok(0);
    }
    integer(face.get(b"enMaterialFaceType", INT_TYPE)?)
}

pub(super) fn material(record: &Fields<'_>, material: &Fields<'_>) -> Result<Value, String> {
    let name = std::str::from_utf8(record.get(b"qsFabricNameUTF8", BYTE_TYPE)?)
        .map_err(|_| "invalid UTF-8 in native fabric name")?;
    let faces = read_face_colors(material)?;
    let front = faces
        .iter()
        .find(|(kind, _)| *kind == 0)
        .map(|(_, color)| color);
    let mut entry = json!({
        "name": name, "texture_maps": {},
        "info": {
            "source": "native_saved_material",
            "catalog": super::native_fabric::catalog(record, material)?,
            "face_colors": faces.iter().map(|(type_code, rgba)| json!({"type": type_code, "rgba": rgba})).collect::<Vec<_>>()
        }
    });
    if let Some(color) = front {
        entry["color"] = json!(&color[..3]);
        entry["rgba"] = json!(color);
    }
    if let Ok(properties) = super::native_fabric::physical_properties(record) {
        entry["info"]["physical_properties"] = properties;
    }
    Ok(entry)
}

pub(super) fn colorway_materials<'a>(
    root: &Fields<'a>,
    fabrics: &[Fields<'a>],
) -> Result<Vec<Vec<Fields<'a>>>, String> {
    let table = root.map(b"mapColorWay")?;
    let colorways = map_list(
        table.map(b"mapNewColorway")?.get(b"listColorway", LIST_TYPE)?,
        4096,
    )?;
    if colorways.len().saturating_mul(fabrics.len()) > 65_536 {
        return Err("native fabric and colorway count exceeds the supported limit".into());
    }
    let id = |row: &Fields<'_>, key| {
        let data = row.get(key, UINT_TYPE)?;
        Ok::<_, String>(u32::from_le_bytes(data.try_into().map_err(|_| "invalid native colorway ID width")?))
    };
    let mut chips = HashMap::new();
    let vectors = table.map(b"mapStyleChipAssign")?;
    for vector in map_list(vectors.get(b"listFabricToStyleChipVector", LIST_TYPE)?, 4096)? {
        let style = id(&vector, b"uiStyleID")?;
        for chip in map_list(vector.get(b"listStyleChip", LIST_TYPE)?, 4096)? {
            let chip_id = id(&chip.map(b"mapElement")?, b"uiID")?;
            if chips.insert((style, chip_id), chip).is_some() {
                return Err("duplicate native fabric style chip".into());
            }
        }
    }
    let styles = fabrics.iter().map(|row| id(row, b"uiID")).collect::<Result<Vec<_>, _>>()?;
    colorways.iter().map(|colorway| {
        let assignments = colorway.map(b"mapAssignStyleChip")?;
        let mut selected = HashMap::new();
        for row in map_list(assignments.get(b"listFabricToStyleChip", LIST_TYPE)?, 4096)? {
            if selected.insert(id(&row, b"uiStyleID")?, id(&row, b"uiStyleChipID")?).is_some() {
                return Err("duplicate native colorway fabric assignment".into());
            }
        }
        styles.iter().map(|style| {
            let chip_id = selected.get(style).ok_or("native colorway does not assign a fabric")?;
            let chip = chips.get(&(*style, *chip_id)).ok_or("native colorway fabric style chip is missing")?;
            let materials = map_list(chip.get(b"listMaterial2D", LIST_TYPE)?, 64)?;
            let [material] = <[Fields<'_>; 1]>::try_from(materials)
                .map_err(|_| "native colorway fabric style chip must have one material")?;
            Ok(material)
        }).collect()
    }).collect()
}

pub(super) fn select_colorway(pac: &[u8], index: u32) -> Result<Vec<u8>, String> {
    let root = Fields::read(pac)?;
    let table = root.map(b"mapColorWay")?;
    let fabrics = map_list(root.get(b"listFabric", LIST_TYPE)?, 4096)?;
    let materials = colorway_materials(&root, &fabrics)?;
    let selected = materials.get(index as usize).ok_or("invalid native colorway index")?;
    let fabrics = fabrics.iter().zip(selected)
        .map(|(fabric, material)| select_fabric_material(fabric, material).map(Cow::Owned))
        .collect::<Result<Vec<_>, _>>()?;
    let table = table.replace(b"uiCurrentCoordinationIndex", UINT_TYPE, &index.to_le_bytes())?;
    let root = root.replace(b"mapColorWay", MAP_TYPE, &table)?;
    Fields::read(&root)?.replace(b"listFabric", LIST_TYPE, &encoded_list(&fabrics)?)
}

fn select_fabric_material(fabric: &Fields<'_>, selected: &Fields<'_>) -> Result<Vec<u8>, String> {
    let legacy = fabric.map(b"mapColorwayInfo")?;
    let legacy_rows = map_list(legacy.get(b"listColorwayInfo", LIST_TYPE)?, 4096)?;
    let [legacy_row] = legacy_rows.as_slice() else {
        return Err("native fabric has an unsupported legacy colorway table".into());
    };
    let material_list = legacy_row.get(b"listMaterial2D", LIST_TYPE)?;
    let old_material = fabric.map(b"mapMaterial2D")?;
    let legacy_material = map_list(material_list, 64)?;
    let old_faces = old_material.get(b"listFaceMaterial", LIST_TYPE)?;
    if !matches!(legacy_material.as_slice(), [material] if material.encoded() == old_material.encoded())
        || legacy_row.get(b"listFaceMaterial", LIST_TYPE)? != old_faces
    {
        return Err("native fabric legacy material differs from its active material".into());
    }
    // CLO stores the active style chip in the main material and both legacy mirrors.
    let selected_list = encoded_list(&[Cow::Borrowed(selected.encoded())])?;
    let legacy_row = legacy_row.replace(b"listMaterial2D", LIST_TYPE, &selected_list)?;
    let legacy_row = Fields::read(&legacy_row)?.replace(
        b"listFaceMaterial", LIST_TYPE, selected.get(b"listFaceMaterial", LIST_TYPE)?,
    )?;
    let legacy = legacy.replace(
        b"listColorwayInfo", LIST_TYPE, &encoded_list(&[Cow::Borrowed(&legacy_row)])?,
    )?;
    let fabric = fabric.replace(b"mapMaterial2D", MAP_TYPE, selected.encoded())?;
    Fields::read(&fabric)?.replace(b"mapColorwayInfo", MAP_TYPE, &legacy)
}

fn assignment(record: &Fields<'_>, count: usize) -> Result<usize, String> {
    let shape = record.map(b"mapFabricShape2D")?;
    let encoded = shape.get(b"iFabricIndex", UINT_TYPE)?;
    let index = u32::from_le_bytes(
        encoded
            .try_into()
            .map_err(|_| "invalid native fabric index width")?,
    ) as usize;
    if index < count {
        Ok(index)
    } else {
        Err("native panel fabric assignment is not in the stored material list".into())
    }
}

pub(super) fn read(pac: &[u8], patterns: &[Value]) -> Result<Materials, String> {
    let root = Fields::read(pac)?;
    let entries: Vec<_> = map_list(root.get(b"listFabric", LIST_TYPE)?, 4096)?
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let mut data = material(record, &record.map(b"mapMaterial2D")?)?;
            data["index"] = json!(index);
            Ok::<_, String>(data)
        })
        .collect::<Result<_, _>>()?;
    let assignments: Vec<_> = panel_records(&root, patterns)?
        .iter()
        .map(|record| assignment(record, entries.len()))
        .collect::<Result<_, _>>()?;
    Ok(Materials {
        entries,
        assignments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(items: &[(&[u8], u8, Vec<u8>)]) -> Vec<u8> {
        let mut result = vec![items.len() as u8];
        result.extend(items.iter().map(|(key, _, _)| key.len() as u8));
        result.extend(items.iter().map(|(_, kind, _)| *kind));
        for (_, _, value) in items {
            result.extend((value.len() as u32).to_le_bytes());
        }
        for (key, _, value) in items {
            result.extend_from_slice(key);
            result.extend_from_slice(value);
        }
        result
    }

    fn color_face(kind: i32, color: [f32; 4]) -> Vec<u8> {
        let base = map(&[(
            b"v4Color",
            VEC4_TYPE,
            color.into_iter().flat_map(f32::to_le_bytes).collect(),
        )]);
        map(&[
            (b"enMaterialFaceType", INT_TYPE, kind.to_le_bytes().to_vec()),
            (b"mapBaseColor", 22, base),
        ])
    }

    #[test]
    fn native_colorways_preserve_order_and_active_index() {
        use super::super::native_pose::{encoded_list, MAP_TYPE};
        use std::borrow::Cow;

        let records = encoded_list(&[
            Cow::Owned(map(&[(b"qsNameUTF8", BYTE_TYPE, b"Default".to_vec())])),
            Cow::Owned(map(&[(b"qsName", BYTE_TYPE, b"Second".to_vec())])),
        ])
        .unwrap();
        let colorways = map(&[(b"listColorway", LIST_TYPE, records)]);
        for index in [1u32, 2] {
            let record = map(&[
                (b"bIsTheNewColorway", 1, vec![1]),
                (b"mapNewColorway", MAP_TYPE, colorways.clone()),
                (
                    b"uiCurrentCoordinationIndex",
                    UINT_TYPE,
                    index.to_le_bytes().to_vec(),
                ),
            ]);
            let pac = map(&[(b"mapColorWay", MAP_TYPE, record)]);
            if index == 1 {
                assert_eq!(
                    read_colorways(&pac).unwrap(),
                    (vec!["Default".into(), "Second".into()], 1)
                );
            } else {
                assert!(read_colorways(&pac).is_err());
            }
        }
    }

    #[test]
    fn native_display_color_uses_front_tag_without_reordering_faces() {
        let front = color_face(0, [0.125, 0.25, 0.75, 0.5]);
        let side = color_face(1, [0.75, 0.125, 0.25, 1.]);
        let mut body = 2u32.to_le_bytes().to_vec();
        body.push(22);
        body.extend((side.len() as u32).to_le_bytes());
        body.extend((front.len() as u32).to_le_bytes());
        body.extend(side);
        body.extend(front);
        let bytes = map(&[
            (
                b"qsFabricNameUTF8",
                BYTE_TYPE,
                b"Stored test fabric".to_vec(),
            ),
            (
                b"mapMaterial2D",
                22,
                map(&[(b"listFaceMaterial", LIST_TYPE, body)]),
            ),
        ]);
        let record = Fields::read(&bytes).unwrap();
        let entry = material(&record, &record.map(b"mapMaterial2D").unwrap()).unwrap();
        assert_eq!(entry["name"], "Stored test fabric");
        assert_eq!(entry["color"], json!([0.125, 0.25, 0.75]));
        assert_eq!(entry["rgba"], json!([0.125, 0.25, 0.75, 0.5]));
        assert_eq!(entry["info"]["face_colors"][0]["type"], 1);
        assert!(rgba(&f32::NAN.to_le_bytes().repeat(4)).is_err());
    }
    #[test]
    fn assignments_use_the_unsigned_fabric_shape_index() {
        let authored_shape = map(&[(b"iFabricIndex", INT_TYPE, (-1i32).to_le_bytes().to_vec())]);
        let fabric_shape = map(&[(b"iFabricIndex", UINT_TYPE, 1u32.to_le_bytes().to_vec())]);
        let bytes = map(&[
            (b"mapShape2D", 22, authored_shape),
            (b"mapFabricShape2D", 22, fabric_shape),
        ]);
        let panel = Fields::read(&bytes).unwrap();
        assert_eq!(assignment(&panel, 2).unwrap(), 1);
        assert!(assignment(&panel, 1).is_err());
    }
}
