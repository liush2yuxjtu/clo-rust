//! Combine supported archived panels and their fabric, colorway and sewing records.

use super::native_materials::{read_colorways, select_colorway};
use super::native_pose::{
    encode_map, encoded_list, map_list, Fields, BYTE_TYPE, LIST_TYPE, MAP_TYPE,
};
use crate::compat_scene::appended_colorway;
use std::borrow::Cow;
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

fn append_link_indices(a: &[u8], b: &[u8], offset: u32) -> Result<Vec<u8>, String> {
    let decode = |data: &[u8]| -> Result<Vec<u32>, String> {
        let count = uint(data.get(..4).ok_or("missing native index array length")?)? as usize;
        if count == 0 && data.len() == 4 {
            return Ok(Vec::new());
        }
        if data.get(4) != Some(&4) || count > LIMIT || data.len() != 5 + count * 4 {
            return Err("invalid native index array".into());
        }
        data[5..].chunks_exact(4).map(uint).collect()
    };
    let mut left = decode(a)?;
    for index in decode(b)? {
        left.push(if index == u32::MAX {
            index
        } else {
            index
                .checked_add(offset)
                .ok_or("native PAC link index overflow")?
        });
    }
    let mut out = (left.len() as u32).to_le_bytes().to_vec();
    if !left.is_empty() {
        out.push(4);
        out.extend(left.iter().flat_map(|index| index.to_le_bytes()));
    }
    Ok(out)
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
