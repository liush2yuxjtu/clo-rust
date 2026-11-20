//! Combine supported archived panels and their fabric, colorway and sewing records.

use super::native_materials::{read_colorways, select_colorway};
use super::native_pose::{
    encode_map, encoded_list, map_list, Fields, BYTE_TYPE, LIST_TYPE, MAP_TYPE,
};
use crate::compat_scene::appended_colorway;
use std::borrow::Cow;
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};

const UINT: u8 = 5;
const INT_ARRAY: u8 = 21;
const LIMIT: usize = 4096;

fn uint(bytes: &[u8]) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| "invalid native PAC integer width")?,
    ))
}

fn maps<'a>(record: &Fields<'a>, key: &[u8]) -> Result<Vec<Fields<'a>>, String> {
    map_list(record.get(key, LIST_TYPE)?, LIMIT)
}

fn list_of_maps(data: &[u8]) -> bool {
    data.get(4) == Some(&MAP_TYPE)
}

fn maximum_id(record: &Fields<'_>, depth: usize) -> Result<u32, String> {
    if depth == 0 {
        return Err("native PAC composition has too many nested maps".into());
    }
    let mut largest = 0;
    for (name, kind, data) in record.fields() {
        match kind {
            UINT if name == b"uiID" => {
                let id = uint(data)?;
                if id != u32::MAX {
                    largest = largest.max(id);
                }
            }
            MAP_TYPE => largest = largest.max(maximum_id(&Fields::read(data)?, depth - 1)?),
            LIST_TYPE if list_of_maps(data) => {
                for nested in map_list(data, LIMIT)? {
                    largest = largest.max(maximum_id(&nested, depth - 1)?);
                }
            }
            _ => {}
        }
    }
    Ok(largest)
}

struct Offsets {
    object: u32,
    panels: u32,
    fabrics: u32,
}

impl Offsets {
    fn uuid(&self, source: &[u8]) -> Vec<u8> {
        let hash = |half: bool| {
            let mut hasher = DefaultHasher::new();
            (self.object, source, half).hash(&mut hasher);
            hasher.finish()
        };
        let value = (u128::from(hash(false)) << 64) | u128::from(hash(true));
        format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            value >> 96,
            (value >> 80) & 0xffff,
            (value >> 64) & 0xfff | 0x4000,
            (value >> 48) & 0x3fff | 0x8000,
            value & 0xffffffffffff
        )
        .into_bytes()
    }

    fn append_record(
        &self,
        record: &Fields<'_>,
        sewing: bool,
        depth: usize,
    ) -> Result<Vec<u8>, String> {
        if depth == 0 {
            return Err("native PAC composition has too many nested maps".into());
        }
        let fields = record
            .fields()
            .map(|(name, kind, data)| {
                let mut offset = 0;
                let bytes = match kind {
                    MAP_TYPE => {
                        Cow::Owned(self.append_record(&Fields::read(data)?, sewing, depth - 1)?)
                    }
                    LIST_TYPE if list_of_maps(data) => {
                        let children = map_list(data, LIMIT)?
                            .iter()
                            .map(|entry| {
                                self.append_record(entry, sewing, depth - 1).map(Cow::Owned)
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        Cow::Owned(encoded_list(&children)?)
                    }
                    BYTE_TYPE if matches!(name, b"qsUuid" | b"qsUuidUTF8") && data.len() == 36 => {
                        Cow::Owned(self.uuid(data))
                    }
                    UINT => {
                        if matches!(
                            name,
                            b"uiID" | b"uiItemID" | b"uiStyleID" | b"uiStyleChipID"
                        ) && uint(data)? != 0
                        {
                            offset = self.object;
                        } else if sewing && name == b"uiSelfIndex" {
                            offset = self.panels;
                        } else if name == b"iFabricIndex" {
                            offset = self.fabrics;
                        }
                        Cow::Borrowed(data)
                    }
                    _ => Cow::Borrowed(data),
                };
                if offset != 0 {
                    let index = uint(data)?;
                    if index != u32::MAX {
                        let value = index
                            .checked_add(offset)
                            .ok_or("native PAC composition index overflow")?;
                        return Ok((name, kind, Cow::Owned(value.to_le_bytes().to_vec())));
                    }
                }
                Ok((name, kind, bytes))
            })
            .collect::<Result<Vec<_>, String>>()?;
        encode_map(
            &fields
                .iter()
                .map(|(name, kind, bytes)| (*name, *kind, bytes.as_ref()))
                .collect::<Vec<_>>(),
        )
    }

    fn append_list(
        &self,
        a: &Fields<'_>,
        b: &Fields<'_>,
        key: &[u8],
        sewing: bool,
    ) -> Result<Vec<u8>, String> {
        let left = maps(a, key)?;
        let right = maps(b, key)?;
        if left.len() + right.len() > LIMIT {
            return Err("native PAC composition record count exceeds its limit".into());
        }
        let mut output: Vec<_> = left
            .iter()
            .map(|record| Cow::Borrowed(record.encoded()))
            .collect();
        for record in &right {
            output.push(Cow::Owned(self.append_record(record, sewing, 32)?));
        }
        encoded_list(&output)
    }
}

fn read_indices(data: &[u8]) -> Result<Vec<u32>, String> {
    let count = uint(data.get(..4).ok_or("missing native index array length")?)? as usize;
    if count == 0 && data.len() == 4 {
        return Ok(Vec::new());
    }
    if data.get(4) != Some(&4) || count > LIMIT || data.len() != 5 + count * 4 {
        return Err("invalid native index array".into());
    }
    data[5..].chunks_exact(4).map(uint).collect()
}

fn write_indices(indices: &[u32]) -> Vec<u8> {
    let mut output = (indices.len() as u32).to_le_bytes().to_vec();
    if !indices.is_empty() {
        output.push(4);
        output.extend(indices.iter().flat_map(|index| index.to_le_bytes()));
    }
    output
}

fn append_link_indices(a: &[u8], b: &[u8], offset: u32) -> Result<Vec<u8>, String> {
    let mut left = read_indices(a)?;
    for index in read_indices(b)? {
        left.push(if index == u32::MAX {
            index
        } else {
            index
                .checked_add(offset)
                .ok_or("native PAC link index overflow")?
        });
    }
    Ok(write_indices(&left))
}

fn join_group(
    a: &Fields<'_>,
    b: &Fields<'_>,
    offsets: &Offsets,
    count: u32,
) -> Result<Vec<u8>, String> {
    let key = b"listSeamLinePairGroup";
    let data = offsets.append_list(a, b, key, true)?;
    let links = b"arrSelfIndex";
    let indices = append_link_indices(a.get(links, INT_ARRAY)?, b.get(links, INT_ARRAY)?, count)?;
    let joined = a.replace(key, LIST_TYPE, &data)?;
    Fields::read(&joined)?.replace(links, INT_ARRAY, &indices)
}

fn editor(a: &Fields<'_>, b: &Fields<'_>, offsets: &Offsets) -> Result<Vec<u8>, String> {
    let key = b"mapPatternList";
    let left = a.map(key)?;
    let right = b.map(key)?;
    if left.get(b"listSizeName", LIST_TYPE)? != right.get(b"listSizeName", LIST_TYPE)? {
        return Err("native PAC composition requires matching size tables".into());
    }
    let patterns = offsets.append_list(&left, &right, b"listPattern", false)?;
    let instances = offsets.append_list(&left, &right, b"listInstance", false)?;
    let links = b"arrSymmetricIndex";
    let indices = append_link_indices(
        left.get(links, INT_ARRAY)?,
        right.get(links, INT_ARRAY)?,
        offsets.panels,
    )?;
    let list = left.replace(b"listPattern", LIST_TYPE, &patterns)?;
    let list = Fields::read(&list)?.replace(b"listInstance", LIST_TYPE, &instances)?;
    let list = Fields::read(&list)?.replace(links, INT_ARRAY, &indices)?;
    let mut output = a.replace(key, MAP_TYPE, &list)?;
    for key in [
        b"mapSeamLinePairGroup".as_slice(),
        b"mapSeamLinePairGroupWithHalfSymmetry",
    ] {
        let left = a.map(key)?;
        let right = b.map(key)?;
        let count = maps(&left, b"listSeamLinePairGroup")?.len() as u32;
        let group = join_group(&left, &right, offsets, count)?;
        output = Fields::read(&output)?.replace(key, MAP_TYPE, &group)?;
    }
    let max_gid = uint(a.get(b"uiMaxGID", UINT)?)?.max(uint(b.get(b"uiMaxGID", UINT)?)?);
    Fields::read(&output)?.replace(b"uiMaxGID", UINT, &max_gid.to_le_bytes())
}

fn assignment(
    a: &Fields<'_>,
    b: &Fields<'_>,
    offsets: &Offsets,
    allowed: &[&[u8]],
) -> Result<Vec<u8>, String> {
    if b.fields()
        .any(|(name, kind, data)| kind == LIST_TYPE && data != [0; 4] && !allowed.contains(&name))
    {
        return Err(
            "native PAC composition cannot preserve an unsupported assignment table".into(),
        );
    }
    let mut output = a.encoded().to_vec();
    for key in allowed {
        let list = offsets.append_list(a, b, key, false)?;
        output = Fields::read(&output)?.replace(key, LIST_TYPE, &list)?;
    }
    Ok(output)
}

fn colorways(a: &Fields<'_>, b: &Fields<'_>, offsets: &Offsets) -> Result<Vec<u8>, String> {
    let list = b"listColorway";
    let key = b"mapNewColorway";
    let left = a.map(key)?;
    let right = b.map(key)?;
    let source = maps(&right, list)?;
    if source.is_empty() {
        return Err("native PAC composition is missing incoming colorway rows".into());
    }
    let rows = maps(&left, list)?
        .iter()
        .enumerate()
        .map(|(position, l)| {
            let r = &source[appended_colorway(position, source.len())];
            let mut output = l.encoded().to_vec();
            for (key, supported) in [
                (
                    b"mapAssign".as_slice(),
                    [
                        b"listPatternToFabric".as_slice(),
                        b"listFabricShape2DToGrainLineInfo",
                        b"listUVTransformerToMatrix",
                    ]
                    .as_slice(),
                ),
                (
                    b"mapAssignStyleChip".as_slice(),
                    [b"listFabricToStyleChip".as_slice()].as_slice(),
                ),
            ] {
                let merged = assignment(&l.map(key)?, &r.map(key)?, offsets, supported)?;
                output = Fields::read(&output)?.replace(key, MAP_TYPE, &merged)?;
            }
            Ok(Cow::Owned(output))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let colorways = left.replace(list, LIST_TYPE, &encoded_list(&rows)?)?;
    let key = b"mapStyleChipAssign";
    let chips = assignment(
        &a.map(key)?,
        &b.map(key)?,
        offsets,
        &[b"listFabricToStyleChipVector"],
    )?;
    let table = a.replace(b"mapNewColorway", MAP_TYPE, &colorways)?;
    Fields::read(&table)?.replace(key, MAP_TYPE, &chips)
}

fn counts(root: &Fields<'_>) -> Result<(u32, u32), String> {
    let editor = root.map(b"mapPatternEditor")?;
    let list = editor.map(b"mapPatternList")?;
    let records = maps(&list, b"listPattern")?;
    let fabrics = maps(root, b"listFabric")?.len() as u32;
    let mut used = std::collections::HashSet::new();
    for record in &records {
        if uint(record.get(b"uiPatternType", UINT)?)? != 0 {
            return Err(
                "native PAC composition cannot preserve non-editable pattern records".into(),
            );
        }
        let index = uint(
            record
                .map(b"mapFabricShape2D")?
                .get(b"iFabricIndex", UINT)?,
        )?;
        if index >= fabrics {
            return Err("native PAC pattern refers to an unknown fabric".into());
        }
        used.insert(index);
    }
    if used.len() != fabrics as usize {
        return Err("native PAC composition requires all source fabrics to be used".into());
    }
    Ok((records.len() as u32, fabrics))
}

pub(super) fn join(receiver: &[u8], incoming: &[u8]) -> Result<Vec<u8>, String> {
    let (_, active) = read_colorways(receiver)?;
    let (source_colorways, source_active) = read_colorways(incoming)?;
    let mapped = appended_colorway(active, source_colorways.len());
    let incoming_selected = (mapped != source_active)
        .then(|| select_colorway(incoming, mapped as u32)).transpose()?;
    let a = Fields::read(receiver)?;
    let b = Fields::read(incoming_selected.as_deref().unwrap_or(incoming))?;
    let (panels, fabrics) = counts(&a)?;
    counts(&b)?;
    let ap = a.map(b"mapPatternEditor")?;
    let bp = b.map(b"mapPatternEditor")?;
    if bp
        .fields()
        .any(|(_, kind, data)| kind == LIST_TYPE && data != [0; 4])
        || b.fields().any(|(name, kind, data)| {
            kind == LIST_TYPE
                && data != [0; 4]
                && !matches!(name, b"listFabric" | b"listClothFileInfo")
        })
    {
        return Err("native PAC composition cannot preserve an unsupported garment record".into());
    }
    let offsets = Offsets {
        object: maximum_id(&a, 32)?,
        panels,
        fabrics,
    };
    let editor = editor(&ap, &bp, &offsets)?;
    let fabric = offsets.append_list(&a, &b, b"listFabric", false)?;
    let colors = colorways(&a.map(b"mapColorWay")?, &b.map(b"mapColorWay")?, &offsets)?;
    let result = a.replace(b"mapPatternEditor", MAP_TYPE, &editor)?;
    let result = Fields::read(&result)?.replace(b"listFabric", LIST_TYPE, &fabric)?;
    Fields::read(&result)?.replace(b"mapColorWay", MAP_TYPE, &colors)
}

fn remapped_index(index: u32, slots: &[Option<u32>]) -> Result<Option<u32>, String> {
    if index == u32::MAX {
        return Ok(Some(index));
    }
    slots.get(index as usize).copied().ok_or_else(|| "native archive index is outside its table".into())
}

fn keep_seam_group(record: &Fields<'_>, panels: &[Option<u32>], depth: usize) -> Result<Option<Vec<u8>>, String> {
    if depth == 0 {
        return Err("native PAC sewing group has too many nested maps".into());
    }
    let mut values = Vec::new();
    for (name, kind, data) in record.fields() {
        let changed = match kind {
            UINT if name == b"uiSelfIndex" => {
                let Some(index) = remapped_index(uint(data)?, panels)? else {
                    return Ok(None);
                };
                Cow::Owned(index.to_le_bytes().to_vec())
            }
            MAP_TYPE => {
                let Some(kept) = keep_seam_group(&Fields::read(data)?, panels, depth - 1)? else {
                    return Ok(None);
                };
                Cow::Owned(kept)
            }
            LIST_TYPE if list_of_maps(data) => {
                let mut children = Vec::new();
                for child in map_list(data, LIMIT)? {
                    let Some(kept) = keep_seam_group(&child, panels, depth - 1)? else {
                        return Ok(None);
                    };
                    children.push(Cow::Owned(kept));
                }
                Cow::Owned(encoded_list(&children)?)
            }
            _ => Cow::Borrowed(data),
        };
        values.push((name, kind, changed));
    }
    encode_map(&values.iter().map(|(name, kind, data)| (*name, *kind, data.as_ref())).collect::<Vec<_>>()).map(Some)
}

fn keep_seams(table: &Fields<'_>, panels: &[Option<u32>]) -> Result<Vec<u8>, String> {
    let key = b"listSeamLinePairGroup";
    let mut output = Vec::new();
    let records = maps(table, key)?;
    let mut kept = Vec::with_capacity(records.len());
    for record in records {
        let group = keep_seam_group(&record, panels, 32)?;
        kept.push(group.as_ref().map(|_| output.len() as u32));
        if let Some(group) = group {
            output.push(Cow::Owned(group));
        }
    }
    let indices = read_indices(table.get(b"arrSelfIndex", INT_ARRAY)?)?;
    if indices.len() != kept.len() {
        return Err("native seam link count does not match its groups".into());
    }
    let indices = indices.into_iter().zip(&kept).filter(|(_, slot)| slot.is_some())
        .map(|(index, _)| Ok(remapped_index(index, &kept)?.unwrap_or(u32::MAX)))
        .collect::<Result<Vec<_>, String>>()?;
    let groups = table.replace(key, LIST_TYPE, &encoded_list(&output)?)?;
    Fields::read(&groups)?.replace(b"arrSelfIndex", INT_ARRAY, &write_indices(&indices))
}

fn panel_ids(record: &Fields<'_>, ids: &mut HashSet<u32>, depth: usize) -> Result<(), String> {
    if depth == 0 {
        return Err("native PAC panel has too many nested maps".into());
    }
    for (name, kind, data) in record.fields() {
        match kind {
            UINT if name == b"uiID" => {
                let id = uint(data)?;
                if id != 0 && id != u32::MAX {
                    ids.insert(id);
                }
            }
            MAP_TYPE => panel_ids(&Fields::read(data)?, ids, depth - 1)?,
            LIST_TYPE if list_of_maps(data) => {
                for child in map_list(data, LIMIT)? {
                    panel_ids(&child, ids, depth - 1)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn remove_colorway_assignments(root: &Fields<'_>, removed: &HashSet<u32>) -> Result<Vec<u8>, String> {
    let colors = root.map(b"mapColorWay")?;
    let table = colors.map(b"mapNewColorway")?;
    let rows = maps(&table, b"listColorway")?;
    let mut kept_rows = Vec::new();
    for row in rows {
        let assign = row.map(b"mapAssign")?;
        let mut updated = assign.encoded().to_vec();
        for (key, kind, data) in assign.fields() {
            if kind != LIST_TYPE || data == [0; 4] {
                continue;
            }
            if !matches!(key, b"listPatternToFabric" | b"listFabricShape2DToGrainLineInfo" | b"listUVTransformerToMatrix") {
                return Err("native panel removal cannot update an unsupported colorway assignment".into());
            }
            let records = map_list(data, LIMIT)?;
            let mut kept = Vec::new();
            for record in &records {
                if !removed.contains(&uint(record.get(b"uiItemID", UINT)?)?) {
                    kept.push(Cow::Borrowed(record.encoded()));
                }
            }
            updated = Fields::read(&updated)?.replace(key, LIST_TYPE, &encoded_list(&kept)?)?;
        }
        kept_rows.push(Cow::Owned(row.replace(b"mapAssign", MAP_TYPE, &updated)?));
    }
    let table = table.replace(b"listColorway", LIST_TYPE, &encoded_list(&kept_rows)?)?;
    colors.replace(b"mapNewColorway", MAP_TYPE, &table)
}

pub(super) fn keep_panels(pac: &[u8], retained: &[usize]) -> Result<Vec<u8>, String> {
    let root = Fields::read(pac)?;
    let editor = root.map(b"mapPatternEditor")?;
    if editor.fields().any(|(_, kind, data)| kind == LIST_TYPE && data != [0; 4]) {
        return Err("native panel removal cannot update an unsupported pattern editor list".into());
    }
    let list = editor.map(b"mapPatternList")?;
    let panels = maps(&list, b"listPattern")?;
    let instances = maps(&list, b"listInstance")?;
    let links = read_indices(list.get(b"arrSymmetricIndex", INT_ARRAY)?)?;
    if panels.len() != instances.len() || panels.len() != links.len()
        || retained.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err("native panel removal requires matching pattern tables and ordered slots".into());
    }
    let mut slots = vec![None; panels.len()];
    for (new, &old) in retained.iter().enumerate() {
        *slots.get_mut(old).ok_or("native panel removal references an unknown slot")? = Some(new as u32);
    }
    let mut removed = HashSet::new();
    let mut kept_ids = HashSet::new();
    let mut kept = Vec::new();
    let mut kept_instances = Vec::new();
    let mut kept_links = Vec::new();
    for (index, panel) in panels.iter().enumerate() {
        if uint(panel.get(b"uiPatternType", UINT)?)? != 0 {
            return Err("native panel removal requires editable pattern records".into());
        }
        if slots[index].is_some() {
            panel_ids(panel, &mut kept_ids, 32)?;
            kept.push(Cow::Borrowed(panel.encoded()));
            kept_instances.push(Cow::Borrowed(instances[index].encoded()));
            kept_links.push(remapped_index(links[index], &slots)?.unwrap_or(u32::MAX));
        } else {
            panel_ids(panel, &mut removed, 32)?;
        }
    }
    let list = list.replace(b"listPattern", LIST_TYPE, &encoded_list(&kept)?)?;
    let list = Fields::read(&list)?.replace(b"listInstance", LIST_TYPE, &encoded_list(&kept_instances)?)?;
    let list = Fields::read(&list)?.replace(b"arrSymmetricIndex", INT_ARRAY, &write_indices(&kept_links))?;
    let mut updated = editor.replace(b"mapPatternList", MAP_TYPE, &list)?;
    for key in [b"mapSeamLinePairGroup".as_slice(), b"mapSeamLinePairGroupWithHalfSymmetry"] {
        let kept = keep_seams(&editor.map(key)?, &slots)?;
        updated = Fields::read(&updated)?.replace(key, MAP_TYPE, &kept)?;
    }
    removed.retain(|id| !kept_ids.contains(id));
    let colors = remove_colorway_assignments(&root, &removed)?;
    let result = root.replace(b"mapPatternEditor", MAP_TYPE, &updated)?;
    Fields::read(&result)?.replace(b"mapColorWay", MAP_TYPE, &colors)
}

fn keep_styles(table: &Fields<'_>, list: &[u8], styles: &HashSet<u32>) -> Result<Vec<u8>, String> {
    let rows = maps(table, list)?;
    let mut kept = Vec::new();
    for row in &rows {
        if styles.contains(&uint(row.get(b"uiStyleID", UINT)?)?) {
            kept.push(Cow::Borrowed(row.encoded()));
        }
    }
    table.replace(list, LIST_TYPE, &encoded_list(&kept)?)
}

fn update_main_fabric(row: &Fields<'_>, styles: &HashSet<u32>, first: u32) -> Result<Vec<u8>, String> {
    let key = b"uiColorwayMainFabricUID";
    let id = uint(row.get(key, UINT)?)?;
    if id == 0 || id == u32::MAX || styles.contains(&id) {
        return Ok(row.encoded().to_vec());
    }
    row.replace(key, UINT, &first.to_le_bytes())
}

pub(super) fn compact_deleted_panel_fabrics(pac: Vec<u8>) -> Result<Vec<u8>, String> {
    let root = Fields::read(&pac)?;
    let fabrics = maps(&root, b"listFabric")?;
    let ids = fabrics.iter().map(|fabric| uint(fabric.get(b"uiID", UINT)?))
        .collect::<Result<Vec<_>, String>>()?;
    let editor = root.map(b"mapPatternEditor")?;
    let table = editor.map(b"mapPatternList")?;
    let panels = maps(&table, b"listPattern")?;
    let indices = panels.iter().map(|panel| uint(panel.map(b"mapFabricShape2D")?.get(b"iFabricIndex", UINT)?))
        .collect::<Result<Vec<_>, String>>()?;
    let colors = root.map(b"mapColorWay")?;
    let colorway_table = colors.map(b"mapNewColorway")?;
    let rows = maps(&colorway_table, b"listColorway")?;
    let mut styles = HashSet::new();
    for index in &indices {
        styles.insert(*ids.get(*index as usize).ok_or("native panel has an unknown fabric index")?);
    }
    for row in &rows {
        for assignment in maps(&row.map(b"mapAssign")?, b"listPatternToFabric")? {
            styles.insert(uint(assignment.get(b"uiStyleID", UINT)?)?);
        }
    }
    if styles.iter().any(|id| !ids.contains(id)) {
        return Err("retained colorway assignment references an unknown fabric".into());
    }
    if styles.len() == fabrics.len() {
        return Ok(pac);
    }
    let mut slots = vec![None; fabrics.len()];
    let mut kept = Vec::new();
    let mut first = None;
    for (index, (id, fabric)) in ids.into_iter().zip(&fabrics).enumerate() {
        if styles.contains(&id) {
            first.get_or_insert(id);
            slots[index] = Some(kept.len());
            kept.push(Cow::Borrowed(fabric.encoded()));
        }
    }
    let first = first.ok_or("native panel removal left no fabrics")?;
    let recalculated = panels.iter().zip(indices).map(|(panel, original)| {
        let index = slots[original as usize].ok_or("retained panel lost its native fabric")? as u32;
        let key = b"mapFabricShape2D";
        let shape = panel.map(key)?.replace(b"iFabricIndex", UINT, &index.to_le_bytes())?;
        panel.replace(key, MAP_TYPE, &shape).map(Cow::Owned)
    }).collect::<Result<Vec<_>, String>>()?;
    let table = table.replace(b"listPattern", LIST_TYPE, &encoded_list(&recalculated)?)?;
    let editor = editor.replace(b"mapPatternList", MAP_TYPE, &table)?;
    let rows = rows.iter().map(|row| {
        let key = b"mapAssignStyleChip";
        let assignments = keep_styles(&row.map(key)?, b"listFabricToStyleChip", &styles)?;
        let updated = row.replace(key, MAP_TYPE, &assignments)?;
        update_main_fabric(&Fields::read(&updated)?, &styles, first).map(Cow::Owned)
    }).collect::<Result<Vec<_>, String>>()?;
    let colorway_table = colorway_table.replace(b"listColorway", LIST_TYPE, &encoded_list(&rows)?)?;
    let legacy = maps(&colors, b"listColorway")?.iter()
        .map(|row| update_main_fabric(row, &styles, first).map(Cow::Owned))
        .collect::<Result<Vec<_>, String>>()?;
    let chips = keep_styles(&colors.map(b"mapStyleChipAssign")?, b"listFabricToStyleChipVector", &styles)?;
    let colors = colors.replace(b"mapNewColorway", MAP_TYPE, &colorway_table)?;
    let colors = Fields::read(&colors)?.replace(b"mapStyleChipAssign", MAP_TYPE, &chips)?;
    let colors = Fields::read(&colors)?.replace(b"listColorway", LIST_TYPE, &encoded_list(&legacy)?)?;
    let root = root.replace(b"mapPatternEditor", MAP_TYPE, &editor)?;
    let root = Fields::read(&root)?.replace(b"mapColorWay", MAP_TYPE, &colors)?;
    Fields::read(&root)?.replace(b"listFabric", LIST_TYPE, &encoded_list(&kept)?)
}
