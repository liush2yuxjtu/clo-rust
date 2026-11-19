//! Read a stored native drape without claiming that it was solved by the Rust engine.

use super::clo_native::{posed_topology, sized_list, sizes, take, uint};
use serde_json::{json, Value};

pub(super) const MAP_TYPE: u8 = 22;
pub(super) const LIST_TYPE: u8 = 20;
pub(super) const BYTE_TYPE: u8 = 15;
const UINT_TYPE: u8 = 5;
const LIMIT: usize = 4096;

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

    pub(super) fn replace(&self, name: &[u8], kind: u8, value: &[u8]) -> Result<Vec<u8>, String> {
        self.get(name, kind)?;
        let mut output = vec![self.0.len() as u8];
        output.extend(self.0.iter().map(|field| field.name.len() as u8));
        output.extend(self.0.iter().map(|field| field.kind));
        let values = self.0.iter().map(|field| {
            if field.name == name && field.kind == kind {
                value
            } else {
                field.value
            }
        });
        for bytes in values.clone() {
            output.extend(
                u32::try_from(bytes.len())
                    .map_err(|_| "PAC map field exceeds the size limit")?
                    .to_le_bytes(),
            );
        }
        for (field, bytes) in self.0.iter().zip(values) {
            output.extend(field.name);
            output.extend(bytes);
        }
        Ok(output)
    }

    pub(super) fn map(&self, name: &[u8]) -> Result<Self, String> {
        Self::read(self.get(name, MAP_TYPE)?)
    }
}

fn pattern_pose(record: &Fields, id: &Value, name: &str) -> Result<Value, String> {
    let geometry = record.get(b"mapFabricShape2D", MAP_TYPE)?;
    let positions3d = record.get(b"baPosition3D", BYTE_TYPE)?;
    let (triangles, positions2d) = posed_topology(geometry)?;
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
    Ok(
        json!({"id": id, "name": name, "positions": positions, "triangles": indices, "texcoords": texcoords}),
    )
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
    let editor = root.map(b"mapPatternEditor")?;
    let patterns = editor.map(b"mapPatternList")?;
    let records = map_list(patterns.get(b"listPattern", LIST_TYPE)?, LIMIT)?;
    let mut result = Vec::new();
    for record in records {
        let mut offset = 0;
        if uint(record.get(b"uiPatternType", UINT_TYPE)?, &mut offset)? != 0 {
            continue;
        }
        let element = record.map(b"mapElement")?;
        let name = std::str::from_utf8(element.get(b"qsNameUTF8", BYTE_TYPE)?)
            .map_err(|e| e.to_string())?;
        if panels
            .get(result.len())
            .is_none_or(|panel| panel["name"] != name)
        {
            return Err("native editable records do not match the imported panel order".into());
        }
        result.push(record);
    }
    if result.len() != panels.len() {
        return Err("native editable records do not match the imported panel count".into());
    }
    Ok(result)
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
