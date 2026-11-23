use super::native_pose::{encoded_list, map_list, Fields, LIST_TYPE, MAP_TYPE};
use crate::compat_scene::patterns::{outline_line_length, points, split_edges};
use serde_json::{json, Value};
use std::borrow::Cow;

const ENABLED: &[u8] = b"bRubberBand";
const RATIO: &[u8] = b"fRubberBandRatio";
const STIFFNESS: &[u8] = b"fRubberBandK";
const STRENGTH_SCALE: f64 = 1_000_000.0;
const DEFAULT_RATIO: f32 = 0.8;
const DEFAULT_STIFFNESS: f32 = 10_000_000.0;

fn real(record: &Fields<'_>, field: &[u8]) -> Option<f32> {
    let value = f32::from_le_bytes(record.get(field, 6).ok()?.try_into().ok()?);
    (value.is_finite() && value >= 0.0).then_some(value)
}

pub(super) fn restore(outline: &Fields<'_>, panel: &mut Value) -> Result<(), String> {
    for (index, edge) in map_list(outline.get(b"listLine", LIST_TYPE)?, 4096)?.iter().enumerate() {
        let on = edge.get(ENABLED, 1).ok() == Some(&[1]);
        let ratio = real(edge, RATIO).unwrap_or(DEFAULT_RATIO);
        let stiffness = real(edge, STIFFNESS).unwrap_or(DEFAULT_STIFFNESS);
        if on || ratio != DEFAULT_RATIO || stiffness != DEFAULT_STIFFNESS {
            // Native CLO retains the length ratio but reloads disabled edges at default stiffness.
            let strength = if on { stiffness } else { DEFAULT_STIFFNESS };
            panel["line_properties"][index.to_string()]["elastic"] = json!({
                "on": on, "ratio": ratio, "strength": f64::from(strength) / STRENGTH_SCALE,
            });
        }
    }
    Ok(())
}

pub(super) fn non_elastic(properties: &Value) -> impl Iterator<Item = (&String, &Value)> {
    properties.as_object().into_iter().flatten().filter(|(_, record)| {
        record.as_object().is_none_or(|fields| fields.keys().any(|key| key != "elastic"))
    })
}

fn has_settings(panel: &Value) -> bool {
    panel["line_properties"].as_object().is_some_and(|records| {
        records.values().any(|record| !record["elastic"].is_null())
    })
}

pub(super) fn update(outline: &Fields<'_>, panel: &Value) -> Result<Option<Vec<u8>>, String> {
    let properties = &panel["line_properties"];
    let all = &properties["-1"]["elastic"];
    if !has_settings(panel) {
        return Ok(None);
    }
    let points = points(panel)?;
    let edges = split_edges(&points);
    let lengths: Vec<_> = edges.iter().enumerate()
        .map(|(index, edge)| outline_line_length(panel, index, edge)).collect();
    let perimeter: f64 = lengths.iter().sum();
    let records = map_list(outline.get(b"listLine", LIST_TYPE)?, 4096)?;
    if records.len() != lengths.len() {
        return Err("native elastic settings require the original outline edges".into());
    }
    let mut changed = false;
    let mut lines = Vec::with_capacity(records.len());
    for (index, (record, length)) in records.iter().zip(lengths).enumerate() {
        let local = &properties[index.to_string()]["elastic"];
        if local.is_null() && all.is_null() {
            lines.push(Cow::Borrowed(record.encoded()));
            continue;
        }
        let on = local["on"].as_bool().or_else(|| all["on"].as_bool()).unwrap_or(false);
        let ratio = local["total_length"].as_f64().map(|target| target / length)
            .or_else(|| local["ratio"].as_f64())
            .or_else(|| all["total_length"].as_f64().map(|target| target / perimeter))
            .unwrap_or(f64::from(DEFAULT_RATIO)) as f32;
        let stiffness = local["strength"].as_f64().or_else(|| all["strength"].as_f64())
            .map_or(f64::from(DEFAULT_STIFFNESS), |strength| strength * STRENGTH_SCALE) as f32;
        if !ratio.is_finite() || ratio < 0.0 || !stiffness.is_finite() || stiffness < 0.0 {
            return Err("invalid native elastic length or strength".into());
        }
        if record.get(ENABLED, 1).ok() == Some(&[u8::from(on)])
            && real(record, RATIO) == Some(ratio) && real(record, STIFFNESS) == Some(stiffness)
        {
            lines.push(Cow::Borrowed(record.encoded()));
            continue;
        }
        let bytes = record.set(ENABLED, 1, &[u8::from(on)])?;
        let bytes = Fields::read(&bytes)?.set(RATIO, 6, &ratio.to_le_bytes())?;
        let bytes = Fields::read(&bytes)?.set(STIFFNESS, 6, &stiffness.to_le_bytes())?;
        lines.push(Cow::Owned(bytes));
        changed = true;
    }
    if changed {
        return outline.replace(b"listLine", LIST_TYPE, &encoded_list(&lines)?).map(Some);
    }
    Ok(None)
}

pub(super) fn update_panel(record: &Fields<'_>, panel: &Value) -> Result<Option<Vec<u8>>, String> {
    if !has_settings(panel) {
        return Ok(None);
    }
    let outline = record.map(b"mapShape2D")?;
    update(&outline, panel)?.map(|shape| record.replace(b"mapShape2D", MAP_TYPE, &shape)).transpose()
}
