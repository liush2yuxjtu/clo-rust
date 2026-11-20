//! Material and panel assignments required when merging authored garment records.

use super::{element, encode_map, records, Fields, Panel, BOOL, BYTE_TYPE, INT, LIST_TYPE,
    MAP_TYPE, MAT3, TRUE, UINT, ZERO};
use crate::compat_scene::formats::native_pose::map_list;
use std::borrow::Cow;

pub(super) fn encode(
    palettes: &[Vec<u8>], active: u32, panels: &[Panel], fabrics: &[Vec<u8>], next_id: &mut u32,
) -> Result<Vec<u8>, String> {
    let fabrics = fabrics.iter().map(|data| Fields::read(data)).collect::<Result<Vec<_>, _>>()?;
    let mut vectors = Vec::new();
    let mut chips = Vec::new();
    for fabric in &fabrics {
        let style = fabric.get(b"uiID", UINT)?;
        *next_id += 1;
        let chip_id = next_id.to_le_bytes();
        let source = fabric.get(b"listStyleChip", LIST_TYPE).ok()
            .map(|data| map_list(data, 1)).transpose()?.unwrap_or_default();
        let chip = if let [chip] = source.as_slice() {
            let entry = chip.map(b"mapElement")?.replace(b"uiID", UINT, &chip_id)?;
            chip.replace(b"mapElement", MAP_TYPE, &entry)?
        } else {
            let material = fabric.get(b"mapMaterial2D", MAP_TYPE)?;
            let materials = super::encoded_list(&[Cow::Borrowed(material)])?;
            let name = std::str::from_utf8(fabric.get(b"qsFabricNameUTF8", BYTE_TYPE)?)
                .map_err(|_| "invalid UTF-8 in assigned native fabric name")?;
            encode_map(&[
                (b"mapElement", MAP_TYPE, &element(name, *next_id)?),
                (b"listMaterial2D", LIST_TYPE, &materials),
            ])?
        };
        vectors.push(encode_map(&[
            (b"uiStyleID", UINT, style),
            (b"listStyleChip", LIST_TYPE, &records(&[chip])?),
        ])?);
        chips.push(encode_map(&[
            (b"uiStyleID", UINT, style),
            (b"uiStyleChipID", UINT, &chip_id),
        ])?);
    }

    let mut assignments = Vec::new();
    let mut grains = Vec::new();
    let mut uv = Vec::new();
    for panel in panels {
        let panel = Fields::read(&panel.record)?;
        let shape = panel.map(b"mapFabricShape2D")?;
        let item = panel.map(b"mapElement")?.get(b"uiID", UINT)?;
        let index = u32::from_le_bytes(shape.get(b"iFabricIndex", UINT)?.try_into()
            .map_err(|_| "invalid authored fabric index")?) as usize;
        let style = fabrics.get(index).ok_or("authored panel refers to an unknown fabric")?;
        assignments.push(encode_map(&[
            (b"uiStyleID", UINT, style.get(b"uiID", UINT)?),
            (b"uiItemID", UINT, item),
        ])?);
        let grain = map_list(shape.get(b"listGrainLineInfo", LIST_TYPE)?, 1)?;
        let [grain] = grain.as_slice() else { return Err("missing authored panel grain".into()) };
        grains.push(encode_map(&[
            (b"uiItemID", UINT, item),
            (b"mapGrainLineInfo", MAP_TYPE, grain.encoded()),
        ])?);
        uv.push(encode_map(&[
            (b"uiItemID", UINT, item),
            (b"m3LtoUV", MAT3, shape.get(b"m3LtoUV", MAT3)?),
            (b"iUVTransformerIndex", INT, &ZERO),
        ])?);
    }
    let assignment = encode_map(&[
        (b"listPatternToFabric", LIST_TYPE, &records(&assignments)?),
        (b"listFabricShape2DToGrainLineInfo", LIST_TYPE, &records(&grains)?),
        (b"listUVTransformerToMatrix", LIST_TYPE, &records(&uv)?),
    ])?;
    let styles = encode_map(&[(b"listFabricToStyleChip", LIST_TYPE, &records(&chips)?)])?;
    let rows = palettes.iter().map(|palette| {
        let row = Fields::read(palette)?.set(b"mapAssign", MAP_TYPE, &assignment)?;
        Fields::read(&row)?.set(b"mapAssignStyleChip", MAP_TYPE, &styles)
    }).collect::<Result<Vec<_>, _>>()?;
    let modern = encode_map(&[(b"listColorway", LIST_TYPE, &records(&rows)?)])?;
    let vectors = encode_map(&[(b"listFabricToStyleChipVector", LIST_TYPE, &records(&vectors)?)])?;
    encode_map(&[
        (b"listColorway", LIST_TYPE, &records(palettes)?),
        (b"uiCurrentCoordinationIndex", UINT, &active.to_le_bytes()),
        (b"mapNewColorway", MAP_TYPE, &modern),
        (b"mapStyleChipAssign", MAP_TYPE, &vectors),
        (b"bIsTheNewColorway", BOOL, TRUE),
    ])
}
