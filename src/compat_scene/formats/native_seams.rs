//! Recover native sewing membership and parametric endpoint data from PAC records.
//! Only single, full outline-edge joins are translated into editable Rust seams.

use super::clo_native::{consume, find, sized_list, sizes, take, token, uint};
use super::native_pose::{map_list, panel_records_with_slots, Fields, BYTE_TYPE, LIST_TYPE};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const GROUP: &[u8] = b"mapSeamLinePairList";
const LIST: &[u8] = b"listSeamLinePair";
const ENDPOINT: &[u8] = b"mapParamLine";
const NAME: &[u8] = b"qsNameUTF8";
const UUID: &[u8] = b"qsUuid";
const LIMIT: usize = 200_000;

pub(super) struct Decoded {
    pub annotations: Vec<Value>,
    pub constraints: Vec<Value>,
    pub unreadable: usize,
}

fn uuid_format(value: &[u8]) -> bool {
    value.len() == 36
        && value.iter().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                *b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

fn identity(mut tail: &[u8]) -> Option<(&str, &str)> {
    while let Some(pos) = find(tail, NAME) {
        tail = &tail[pos + NAME.len()..];
        if let Some((name, end)) = token(tail, UUID, 255) {
            if let Some(id) = tail.get(end..end + 36) {
                if !name.is_empty() && uuid_format(id) {
                    return Some((name, std::str::from_utf8(id).ok()?));
                }
            }
        }
    }
    None
}

fn tagged_uint(data: &[u8], cursor: &mut usize, key: &[u8]) -> Result<usize, String> {
    consume(data, cursor, key)?;
    uint(data, cursor)
}

fn ratio(data: &[u8], cursor: &mut usize, key: &[u8]) -> Result<f64, String> {
    let bits = tagged_uint(data, cursor, key)? as u32;
    let value = f64::from(f32::from_bits(bits));
    if !(0.0..=1.0).contains(&value) {
        return Err("invalid native sewing ratio".into());
    }
    Ok(value)
}

fn endpoint(data: &[u8], edge_counts: &[usize]) -> Result<Value, String> {
    let mut cursor =
        find(data, b"bDirection").ok_or("missing native seam direction")? + b"bDirection".len();
    let direction = match take(data, &mut cursor, 1)?[0] {
        0 => false,
        1 => true,
        _ => return Err("invalid native seam direction".into()),
    };
    let end_ratio = ratio(data, &mut cursor, b"fRatioEnd")?;
    let start_ratio = ratio(data, &mut cursor, b"fRatioStart")?;
    let end_line = tagged_uint(data, &mut cursor, b"uiEndIndex")?;
    let start_line = tagged_uint(data, &mut cursor, b"uiStartIndex")?;
    cursor += find(&data[cursor..], b"uiSelfIndex").ok_or("missing native seam panel index")?;
    let panel = tagged_uint(data, &mut cursor, b"uiSelfIndex")?;
    let shape = if data[cursor..].starts_with(b"uiShapeIndex") {
        Some(tagged_uint(data, &mut cursor, b"uiShapeIndex")?)
    } else {
        None
    };
    let depth = tagged_uint(data, &mut cursor, b"uiDepth")?;
    let edge_count = *edge_counts
        .get(panel)
        .ok_or("native sewing references an unreadable panel")?;
    if depth == 1 && (start_line >= edge_count || end_line >= edge_count) {
        return Err("native sewing references an out-of-range outline edge".into());
    }
    Ok(json!({
        "pattern_id": panel + 1, "shape_depth": depth, "shape_index": shape, "start_line": start_line, "end_line": end_line,
        "start_ratio": start_ratio, "end_ratio": end_ratio, "direction": direction
    }))
}

fn pair(data: &[u8], edge_counts: &[usize]) -> Result<[Value; 2], String> {
    let first =
        find(data, ENDPOINT).ok_or("missing first native sewing endpoint")? + ENDPOINT.len();
    let second =
        first + find(&data[first..], ENDPOINT).ok_or("missing second native sewing endpoint")?;
    if find(&data[second + ENDPOINT.len()..], ENDPOINT).is_some() {
        return Err("unexpected extra native sewing endpoints".into());
    }
    Ok([
        endpoint(&data[first..second], edge_counts)?,
        endpoint(&data[second + ENDPOINT.len()..], edge_counts)?,
    ])
}

fn group<'a>(
    data: &'a [u8],
    edge_counts: &[usize],
) -> Result<(&'a str, &'a str, Option<bool>, Vec<[Value; 2]>), String> {
    let mut cursor = find(data, LIST).ok_or("missing native sewing-pair list")? + LIST.len();
    let (table, count) = sized_list(data, &mut cursor, 0x16, LIMIT)?;
    let mut pairs = Vec::with_capacity(count);
    for size in sizes(table) {
        pairs.push(pair(take(data, &mut cursor, size)?, edge_counts)?);
    }
    let (name, uuid) = identity(&data[cursor..]).ok_or("missing native sewing group identity")?;
    let tail = &data[cursor..];
    let active = find(tail, b"mapSeamLinePairProperty").and_then(|start| {
        let tail = &tail[start..];
        let active = find(tail, b"bActive")? + b"bActive".len();
        match tail.get(active) {
            Some(0) => Some(false),
            Some(1) => Some(true),
            _ => None,
        }
    });
    Ok((name, uuid, active, pairs))
}

fn full_edge(endpoint: &Value) -> bool {
    let forward = endpoint["direction"].as_bool().unwrap_or(false);
    endpoint["shape_depth"] == 1
        && endpoint["start_line"] == endpoint["end_line"]
        && endpoint["start_ratio"].as_f64() == Some(if forward { 0. } else { 1. })
        && endpoint["end_ratio"].as_f64() == Some(if forward { 1. } else { 0. })
}

impl Decoded {
    fn add(&mut self, name: &str, active: Option<bool>, pairs: Vec<[Value; 2]>) {
        let id = self.annotations.len() + 1;
        let restored = active == Some(true) && pairs.len() == 1 && pairs[0].iter().all(full_edge);
        if restored {
            let [a, b] = &pairs[0];
            self.constraints.push(json!({
                "pattern_a": a["pattern_id"], "line_a": a["start_line"], "dir_a": a["direction"],
                "pattern_b": b["pattern_id"], "line_b": b["start_line"], "dir_b": b["direction"],
                "name": name, "native_group": id
            }));
        }
        self.annotations.push(json!({"id": id, "name": name, "segments": pairs, "restored": restored, "active": active}));
    }
}

fn unsigned(record: &Fields<'_>, key: &[u8]) -> Result<usize, String> {
    let field = record.get(key, 5)?;
    uint(field, &mut 0)
}

fn ordered_endpoint(
    record: &Fields<'_>,
    slots: &BTreeMap<usize, usize>,
    edge_counts: &[usize],
) -> Result<Value, String> {
    let direction = match record.get(b"bDirection", 1)? {
        [0] => false,
        [1] => true,
        _ => return Err("invalid native seam direction".into()),
    };
    let read_ratio = |key: &[u8]| {
        let value = f32::from_bits(uint(record.get(key, 6)?, &mut 0)? as u32);
        (0.0..=1.0)
            .contains(&value)
            .then_some(value)
            .ok_or_else(|| "invalid native seam ratio".to_string())
    };
    let start = unsigned(record, b"uiStartIndex")?;
    let end = unsigned(record, b"uiEndIndex")?;
    let depth = unsigned(record, b"uiDepth")?;
    let slot = unsigned(record, b"uiSelfIndex")?;
    let &panel = slots
        .get(&slot)
        .ok_or("native seam references an unreadable panel slot")?;
    if depth == 1 && (start >= edge_counts[panel] || end >= edge_counts[panel]) {
        return Err("native sewing references an out-of-range outline edge".into());
    }
    Ok(json!({
        "pattern_id":panel+1,"shape_depth":depth,"shape_index":unsigned(record,b"uiShapeIndex").ok(),
        "start_line":start,"end_line":end,"start_ratio":read_ratio(b"fRatioStart")?,
        "end_ratio":read_ratio(b"fRatioEnd")?,"direction":direction
    }))
}

fn ordered_group<'a>(
    record: &Fields<'a>,
    slots: &BTreeMap<usize, usize>,
    edge_counts: &[usize],
) -> Result<(&'a str, Option<bool>, Vec<[Value; 2]>), String> {
    let element = record.map(b"mapElement")?;
    let name = std::str::from_utf8(element.get(NAME, BYTE_TYPE)?).map_err(|e| e.to_string())?;
    let active = match record.map(b"mapSeamLinePairProperty")?.get(b"bActive", 1)? {
        [0] => Some(false),
        [1] => Some(true),
        _ => None,
    };
    let pair_list = record.map(GROUP)?;
    let pairs = map_list(pair_list.get(LIST, LIST_TYPE)?, LIMIT)?
        .iter()
        .map(|pair| {
            let a = pair.map(b"mapSeamLine0")?.map(ENDPOINT)?;
            let b = pair.map(b"mapSeamLine1")?.map(ENDPOINT)?;
            Ok([
                ordered_endpoint(&a, slots, edge_counts)?,
                ordered_endpoint(&b, slots, edge_counts)?,
            ])
        })
        .collect::<Result<Vec<_>, String>>()?;
    if pairs.is_empty() {
        return Err("native seam group has no endpoints".into());
    }
    Ok((name, active, pairs))
}

pub(super) fn read(
    data: &[u8],
    panels: &[Value],
    edge_counts: &[usize],
) -> Result<Decoded, String> {
    let table = Fields::read(data).and_then(|root| {
        let editor = root.map(b"mapPatternEditor")?;
        let group = editor.map(b"mapSeamLinePairGroup")?;
        Ok((root, group))
    });
    let Ok((root, table)) = table else {
        return read_legacy(data, edge_counts);
    };
    let slots = panel_records_with_slots(&root, panels)?
        .into_iter()
        .enumerate()
        .map(|(index, (slot, _))| (slot, index))
        .collect();
    let mut decoded = Decoded {
        annotations: Vec::new(),
        constraints: Vec::new(),
        unreadable: 0,
    };
    for record in map_list(table.get(b"listSeamLinePairGroup", LIST_TYPE)?, LIMIT)? {
        match ordered_group(&record, &slots, edge_counts) {
            Ok((name, active, pairs)) => decoded.add(name, active, pairs),
            Err(_) => decoded.unreadable += 1,
        }
    }
    Ok(decoded)
}

fn read_legacy(mut data: &[u8], edge_counts: &[usize]) -> Result<Decoded, String> {
    let mut decoded = Decoded {
        annotations: Vec::new(),
        constraints: Vec::new(),
        unreadable: 0,
    };
    let Some(first) = find(data, GROUP) else {
        return Ok(decoded);
    };
    data = &data[first + GROUP.len()..];
    let mut seen = BTreeMap::new();
    loop {
        let next = find(data, GROUP);
        let record = &data[..next.unwrap_or(data.len())];
        match group(record, edge_counts) {
            Ok((name, uuid, active, pairs)) => {
                let json_pairs = json!(pairs);
                if let Some(&i) = seen.get(uuid) {
                    let old: &Value = &decoded.annotations[i];
                    if old["name"] != name
                        || old["segments"] != json_pairs
                        || old["active"] != json!(active)
                    {
                        decoded.unreadable += 1;
                    }
                } else {
                    let id = decoded.annotations.len() + 1;
                    if id > LIMIT {
                        return Err("native PAC has too many sewing groups".into());
                    }
                    seen.insert(uuid, id - 1);
                    decoded.add(name, active, pairs);
                }
            }
            Err(_) => decoded.unreadable += 1,
        }
        let Some(next) = next else {
            break;
        };
        data = &data[next + GROUP.len()..];
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_endpoint_uses_raw_panel_slots() {
        let values: &[(&[u8], u8, &[u8])] = &[
            (b"uiSelfIndex", 5, &47u32.to_le_bytes()),
            (b"fRatioStart", 6, &0.0f32.to_le_bytes()),
            (b"uiDepth", 5, &2u32.to_le_bytes()),
            (b"bDirection", 1, &[1]),
            (b"uiEndIndex", 5, &0u32.to_le_bytes()),
            (b"uiStartIndex", 5, &0u32.to_le_bytes()),
            (b"fRatioEnd", 6, &1.0f32.to_le_bytes()),
            (b"uiShapeIndex", 5, &6u32.to_le_bytes()),
        ];
        let mut bytes = vec![values.len() as u8];
        bytes.extend(values.iter().map(|(key, _, _)| key.len() as u8));
        bytes.extend(values.iter().map(|(_, tag, _)| *tag));
        for (_, _, value) in values {
            bytes.extend((value.len() as u32).to_le_bytes());
        }
        for (key, _, value) in values {
            bytes.extend(*key);
            bytes.extend(*value);
        }
        let record = Fields::read(&bytes).unwrap();
        let endpoint = ordered_endpoint(&record, &BTreeMap::from([(47, 41)]), &[4; 42]).unwrap();
        assert_eq!(endpoint["pattern_id"], 42);
        assert_eq!(endpoint["shape_index"], 6);
        assert!(!full_edge(&endpoint));
        assert!(ordered_endpoint(&record, &BTreeMap::new(), &[4; 42]).is_err());
    }

    fn numeric(data: &mut Vec<u8>, tag: &[u8], value: u32) {
        data.extend_from_slice(tag);
        data.extend_from_slice(&value.to_le_bytes());
    }

    fn side(data: &mut Vec<u8>, panel: u32, line: u32, start: f32, end: f32, depth: u32) {
        data.extend_from_slice(ENDPOINT);
        data.extend_from_slice(b"bDirection");
        data.push(u8::from(start < end));
        for (tag, val) in [
            (b"fRatioEnd".as_slice(), end.to_bits()),
            (b"fRatioStart", start.to_bits()),
            (b"uiEndIndex", line),
            (b"uiStartIndex", line),
            (b"uiSelfIndex", panel),
            (b"uiDepth", depth),
        ] {
            numeric(data, tag, val);
        }
    }

    fn source(name: &str, id: &str, depth: u32, end: f32) -> Vec<u8> {
        let mut endpoints = Vec::new();
        side(&mut endpoints, 0, 1, 0., end, depth);
        side(&mut endpoints, 1, 2, 1., 0., 1);
        let mut data = GROUP.to_vec();
        numeric(&mut data, LIST, 1);
        data.push(0x16);
        numeric(&mut data, b"", endpoints.len() as u32);
        data.extend(endpoints);
        data.extend_from_slice(b"mapSeamLinePairPropertybActive\x01");
        data.extend_from_slice(NAME);
        data.extend_from_slice(name.as_bytes());
        data.extend_from_slice(UUID);
        data.extend_from_slice(id.as_bytes());
        data
    }

    #[test]
    fn native_synthetic_seams_restore_only_full_outline_pairs() {
        let first = source("Full join", "01234567-1234-1234-1234-0123456789ab", 1, 1.);
        let mut records = first.clone();
        records.extend(first);
        records.extend(source(
            "Half join",
            "01234567-1234-1234-1234-0123456789ac",
            1,
            0.5,
        ));
        records.extend(source(
            "Interior join",
            "01234567-1234-1234-1234-0123456789ad",
            2,
            1.,
        ));
        let decoded = read_legacy(&records, &[3, 3]).unwrap();
        assert_eq!(decoded.unreadable, 0);
        assert_eq!(decoded.annotations.len(), 3);
        assert_eq!(decoded.constraints.len(), 1);
        assert_eq!(decoded.constraints[0]["dir_b"], false);
        assert_eq!(decoded.annotations[1]["segments"][0][0]["end_ratio"], 0.5);
        assert_eq!(decoded.annotations[2]["segments"][0][0]["shape_depth"], 2);
    }
}
