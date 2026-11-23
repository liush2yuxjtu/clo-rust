use super::clo_native::{bounded_file, open_archive, optional_member, ZPRJ};
use super::native_pose::{encode_map, Fields, BYTE_TYPE, MAP_TYPE};
use serde_json::{json, Value};
use std::path::Path;

pub(super) const FILE_NAME: &str = "garment.smp";
const PROPERTIES: &[u8] = b"mapDeforMarvelProperty";
const GARMENT_SETTINGS: &[u8] = b"mapSMP";
const TIME: &[u8] = b"fEntireTimeStep";
const COUNT: &[u8] = b"uiSubStepCount";
const GRAVITY: &[u8] = b"fGravity_y";
const GROUND: &[u8] = b"bGroundCollisionHandling";
const HEIGHT: &[u8] = b"fGroundHeight";
const PROFILE: &[u8] = b"qsSpName2UTF8";
const XML_GROUND: &str = "DoesHandleGroundCollision";
const XML_HEIGHT: &str = "GroundHeight";
const FLOAT: u8 = 6;
const UINT: u8 = 5;
const BOOL: u8 = 1;
const DEFAULT_TIME: f32 = 0.033333;
const RUNTIME_KEYS: &[&str] = &[
    "quality", "simulation_time_step", "simulation_substeps", "simulation_gravity",
    "simulation_ground_collision", "simulation_ground_height",
];

fn float(record: &Fields<'_>, key: &[u8]) -> Option<f32> {
    let value = f32::from_le_bytes(record.get(key, FLOAT).ok()?.try_into().ok()?);
    value.is_finite().then_some(value)
}

fn read(bytes: &[u8]) -> Option<serde_json::Map<String, Value>> {
    let root = Fields::read(bytes).ok()?;
    let properties = root.map(PROPERTIES).ok()?;
    let quality = match root.get(PROFILE, BYTE_TYPE).ok()? {
        b"Normal" => 0,
        b"Complete" => 1,
        b"Nonlinear" => 2,
        _ => 4,
    };
    let mut settings = serde_json::Map::from_iter([("quality".into(), json!(quality))]);
    for (key, value) in [
        ("simulation_time_step", float(&root, TIME)),
        ("simulation_gravity", float(&properties, GRAVITY)),
        ("simulation_ground_height", float(&properties, HEIGHT)),
    ] {
        if let Some(value) = value {
            settings.insert(key.into(), json!(value));
        }
    }
    if let Ok(count) = root.get(COUNT, UINT).and_then(|bytes| {
        bytes.try_into().map(u32::from_le_bytes).map_err(|_| "invalid native substep count".into())
    }) {
        settings.insert("simulation_substeps".into(), json!(count.clamp(1, 64)));
    }
    if let Ok(enabled @ ([0] | [1])) = properties.get(GROUND, BOOL) {
        settings.insert("simulation_ground_collision".into(), json!(enabled[0] == 1));
    }
    Some(settings)
}

pub(super) fn garment_settings(data: &[u8]) -> Option<serde_json::Map<String, Value>> {
    let root = Fields::read(data).ok()?;
    read(root.get(GARMENT_SETTINGS, MAP_TYPE).ok()?)
}

pub(in crate::compat_scene) fn checkpoint_settings(state: &Value) -> Value {
    let runtime = &state["runtime"];
    Value::Object(RUNTIME_KEYS.iter().filter_map(|&key| {
        runtime.get(key).filter(|value| !value.is_null()).map(|value| (key.into(), value.clone()))
    }).collect())
}

pub(in crate::compat_scene) fn validate_checkpoint_settings(
    data: &Value,
) -> Result<serde_json::Map<String, Value>, String> {
    let data = data.as_object().ok_or("checkpoint simulation settings must be an object")?;
    let mut validated = serde_json::Map::new();
    for &key in RUNTIME_KEYS {
        let Some(value) = data.get(key) else { continue };
        let valid = match key {
            "quality" => matches!(value.as_u64(), Some(0..=2 | 4)),
            "simulation_substeps" => matches!(value.as_u64(), Some(1..=64)),
            "simulation_ground_collision" => value.is_boolean(),
            _ => value.as_f64().is_some_and(|number| (number as f32).is_finite()),
        };
        if !valid {
            return Err(format!("invalid checkpoint simulation setting: {key}"));
        }
        validated.insert(key.into(), value.clone());
    }
    Ok(validated)
}

pub(in crate::compat_scene) fn apply_settings(
    state: &mut Value,
    settings: serde_json::Map<String, Value>,
) {
    if let Some(runtime) = state["runtime"].as_object_mut() {
        for key in RUNTIME_KEYS {
            runtime.remove(*key);
        }
    }
    for (key, value) in settings {
        state["runtime"][key] = value;
    }
}

pub(in crate::compat_scene) fn restore(state: &mut Value, project: Option<&Path>, garment: &Value) {
    let mut settings = garment.as_object().cloned();
    let archive = project.and_then(|path| open_archive(bounded_file(path).ok()?, ZPRJ).ok());
    if let Some(mut archive) = archive {
        if settings.is_none() {
            settings = optional_member(&mut archive, "smp").ok().flatten().and_then(|(_, data)| read(&data));
        }
        if let Some((_, data)) = optional_member(&mut archive, "xml").ok().flatten() {
            if let Ok(xml) = std::str::from_utf8(&data) {
                let values = settings.get_or_insert_with(serde_json::Map::new);
                if let Some(enabled) = environment_field(xml, XML_GROUND).and_then(|range| xml[range].parse::<bool>().ok()) {
                    values.insert("simulation_ground_collision".into(), json!(enabled));
                }
                if let Some(height) = environment_field(xml, XML_HEIGHT).and_then(|range| xml[range].parse::<f32>().ok()) {
                    if height.is_finite() {
                        values.insert("simulation_ground_height".into(), json!(height));
                    }
                }
            }
        }
    }
    apply_settings(state, settings.unwrap_or_default());
}

fn environment_field(xml: &str, tag: &str) -> Option<std::ops::Range<usize>> {
    if !xml.contains("<Environment Version=") {
        return None;
    }
    let marker = format!("<{tag} Value=\"");
    let (prefix, rest) = xml.split_once(&marker)?;
    if rest.contains(&marker) {
        return None;
    }
    let start = prefix.len() + marker.len();
    Some(start..start + rest.find('"')?)
}

pub(super) fn update_environment(data: &[u8], state: &Value) -> Option<Vec<u8>> {
    let original = std::str::from_utf8(data).ok()?;
    let runtime = &state["runtime"];
    let enabled = runtime["simulation_ground_collision"].as_bool().unwrap_or(true);
    let height = runtime["simulation_ground_height"].as_f64().unwrap_or(0.0) as f32;
    let mut updated = original.to_owned();
    if let Some(range) = environment_field(&updated, XML_GROUND) {
        if updated[range.clone()].parse::<bool>().ok() != Some(enabled) {
            updated.replace_range(range, if enabled { "true" } else { "false" });
        }
    }
    if let Some(range) = environment_field(&updated, XML_HEIGHT) {
        if updated[range.clone()].parse::<f32>().ok() != Some(height) {
            updated.replace_range(range, &height.to_string());
        }
    }
    (updated != original).then(|| updated.into_bytes())
}

pub(super) fn has_overrides(state: &Value) -> bool {
    let runtime = &state["runtime"];
    RUNTIME_KEYS.iter().any(|key| !runtime[*key].is_null())
}

pub(super) fn update_garment(pac: &[u8], state: &Value) -> Result<Option<Vec<u8>>, String> {
    if !has_overrides(state) {
        return Ok(None);
    }
    let root = Fields::read(pac)?;
    let original = root.get(GARMENT_SETTINGS, MAP_TYPE).ok();
    let data = encode(original.filter(|data| *data != [0]), state)?;
    if original == Some(data.as_slice()) {
        return Ok(None);
    }
    root.set(GARMENT_SETTINGS, MAP_TYPE, &data).map(Some)
}

fn with_fields(record: &Fields<'_>, updates: &[(&[u8], u8, &[u8])]) -> Result<Vec<u8>, String> {
    let mut data = record.encoded().to_vec();
    for &(key, kind, value) in updates {
        let record = Fields::read(&data)?;
        if record.get(key, kind).ok() != Some(value) {
            data = record.set(key, kind, value)?;
        }
    }
    Ok(data)
}

pub(super) fn encode(existing: Option<&[u8]>, state: &Value) -> Result<Vec<u8>, String> {
    let runtime = &state["runtime"];
    let quality = crate::compat_fit::simulation_quality(state);
    let root = existing.map(Fields::read).transpose()?;
    let profile: &[u8] = match quality {
        0 => b"Normal", 1 => b"Complete", 2 => b"Nonlinear",
        _ => root.as_ref().and_then(|root| root.get(PROFILE, BYTE_TYPE).ok())
            .filter(|name| !matches!(*name, b"Normal" | b"Complete" | b"Nonlinear")).unwrap_or(b"Custom"),
    };
    let float = |key: &str, default: f32| (runtime[key].as_f64().unwrap_or(f64::from(default)) as f32).to_le_bytes();
    let time = float("simulation_time_step", DEFAULT_TIME);
    let count = (runtime["simulation_substeps"].as_u64()
        .unwrap_or(if matches!(quality, 1 | 2) { 5 } else { 1 }).clamp(1, 64) as u32).to_le_bytes();
    let gravity = float("simulation_gravity", -9800.0);
    let height = float("simulation_ground_height", 0.0);
    let ground = [u8::from(runtime["simulation_ground_collision"].as_bool().unwrap_or(true))];
    let nonlinear = [u8::from(quality == 2)];
    let fields: &[(&[u8], u8, &[u8])] = &[(GRAVITY, FLOAT, &gravity), (GROUND, BOOL, &ground), (HEIGHT, FLOAT, &height)];
    let properties = if let Some(root) = &root {
        let original = root.map(PROPERTIES)?;
        let data = with_fields(&original, fields)?;
        if root.get(PROFILE, BYTE_TYPE).ok() == Some(profile) {
            data
        } else {
            Fields::read(&data)?.set(b"bNonlinearSimulation", BOOL, &nonlinear)?
        }
    } else {
        encode_map(&[
            (b"uiVersion", UINT, &1u32.to_le_bytes()),
            (b"fTimeStep", FLOAT, &DEFAULT_TIME.to_le_bytes()),
            (b"bNonlinearSimulation", BOOL, &nonlinear),
            (GRAVITY, FLOAT, &gravity), (GROUND, BOOL, &ground), (HEIGHT, FLOAT, &height),
        ])?
    };
    let updated: &[(&[u8], u8, &[u8])] = &[
        (b"qsSpName2", BYTE_TYPE, profile), (PROFILE, BYTE_TYPE, profile),
        (TIME, FLOAT, &time), (COUNT, UINT, &count), (PROPERTIES, MAP_TYPE, &properties),
    ];
    if let Some(root) = &root { with_fields(root, updated) } else { encode_map(updated) }
}
