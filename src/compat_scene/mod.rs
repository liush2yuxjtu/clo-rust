mod annotations;
mod formats;
mod patterns;
mod scene;

use serde_json::{json, Value};

pub(crate) type InnerSeamPaths = Vec<Vec<Vec<usize>>>;
pub(crate) type InnerSeamRest = Vec<Vec<Vec<[f64; 2]>>>;
pub(crate) use patterns::{bridge_pattern_index, internal_strokes, layout_outline};
pub(crate) use patterns::sampled_perimeter;
pub(crate) use annotations::seam_group_names;
pub(crate) use formats::write as write_artifact;
pub(crate) use scene::{assigned_fabric_count, assigned_fabrics, avatar_names, checkpoint_before_build, clear_patterns, default_fabric, empty_project};
pub(crate) use formats::native_fabric::read as read_native_fabric;

pub(crate) fn fabric_catalog(state: &Value) -> impl Iterator<Item = &Value> {
    let fabrics = array(state, "fabrics");
    // CLO lists its primary fabrics before appended garment groups. Pattern
    // assignments retain the separate stored fabric indices.
    fabrics.iter().filter(|fabric| fabric["appended_catalog"] != true)
        .chain(fabrics.iter().filter(|fabric| fabric["appended_catalog"] == true))
}

pub(crate) fn current_fabric_catalog_index(state: &Value) -> usize {
    let selected = state["current_fabric"].as_u64().unwrap_or(0);
    fabric_catalog(state).position(|fabric| fabric["index"] == selected).unwrap_or(0)
}

fn appended_colorway(index: usize, source_count: usize) -> usize {
    // CLO fills receiver slots by position, reusing the first source style
    // chip when the receiver has more colorways.
    if index < source_count { index } else { 0 }
}

fn remap_appended_colorways(fabric: &mut Value, receiver: &Value, archived: &Value) {
    let names = &receiver["colorways"];
    let count = array(receiver, "colorways").len();
    let source_count = array(archived, "colorways").len();
    let active = receiver["current_colorway"].as_u64().unwrap_or(0) as usize;
    let source_active = archived["current_colorway"].as_u64().unwrap_or(0) as usize;
    if count == 0 || source_count == 0 || active >= count
        || (*names == archived["colorways"] && active == source_active)
    {
        return;
    }
    let variants = &fabric["native_colorway_materials"];
    let mut base = fabric.clone();
    base.as_object_mut().expect("archived fabric record").remove("native_colorway_materials");
    let mut materials = Vec::with_capacity(count);
    let mut chips = Vec::with_capacity(count);
    for position in 0..count {
        let source = appended_colorway(position, source_count);
        let mut material = if source == source_active {
            base.clone()
        } else if let Some(item) = variants["values"].get(source).filter(|item| item.is_object()) {
            item.clone()
        } else {
            return;
        };
        material["index"] = base["index"].clone();
        materials.push(material);
        chips.push(variants["chips"].get(source).cloned().unwrap_or_else(|| json!(source)));
    }
    *fabric = materials[active].clone();
    if count > 1 {
        fabric["native_colorway_materials"] = json!({"names": names, "values": materials, "chips": chips});
    }
}

pub(crate) fn fabric_for_colorway<'a>(
    state: &Value,
    fabric: &'a Value,
    colorway: usize,
) -> Result<&'a Value, String> {
    if state["current_colorway"].as_u64().unwrap_or(0) == colorway as u64 {
        return Ok(fabric);
    }
    let variants = &fabric["native_colorway_materials"];
    if variants["names"] == state["colorways"] {
        if let Some(material) = variants["values"].get(colorway).filter(|value| value.is_object()) {
            return Ok(material);
        }
    }
    Err(format!("fabric has no archived material for colorway {colorway}"))
}

pub(crate) fn refresh_colorway_exports(state: &mut Value) {
    if let Some(runtime) = state.get_mut("runtime").and_then(Value::as_object_mut) {
        runtime.remove("gltf_export_active_only");
    }
}

pub(crate) fn set_current_colorway(state: &mut Value, index: usize) -> Result<(), String> {
    if index >= array(state, "colorways").len().max(1) {
        return Ok(());
    }
    let previous = state["current_colorway"].as_u64().unwrap_or(0) as usize;
    if index != previous {
        let materials = array(state, "fabrics").iter().map(|fabric| {
            let mut material = fabric_for_colorway(state, fabric, index)?.clone();
            let mut active = fabric.clone();
            let mut variants = active.as_object_mut()
                .and_then(|record| record.remove("native_colorway_materials"))
                .ok_or("fabric has no archived colorway materials")?;
            *variants["values"].get_mut(previous)
                .ok_or("archived material for current colorway is missing")? = active;
            material["index"] = fabric["index"].clone();
            if fabric["appended_catalog"] == true {
                material["appended_catalog"] = json!(true);
            }
            material["native_colorway_materials"] = variants;
            Ok(material)
        }).collect::<Result<Vec<_>, String>>()?;
        state["fabrics"] = json!(materials);
        state["current_colorway"] = json!(index);
    }
    refresh_colorway_exports(state);
    Ok(())
}

pub(crate) fn append_curved_arrangement_fields(record: &mut Value, values: [Option<f32>; 4]) {
    record["ArrangementType"] = json!("Curved");
    for ((field, scale, origin), value) in [
        ("ArrangementOffsetX", 100.0, 50.0),
        ("ArrangementOffsetY", 100.0, 50.0),
        ("ArrangementOffsetZ", 0.5, 50.0),
        ("ArrangementOrientation", 1.0, 180.0),
    ].into_iter().zip(values) {
        if let Some(value) = value {
            let converted = value * scale + origin;
            // CLO's arrangement strings round away archive float noise.
            let rounded = format!("{converted:.5e}").parse::<f32>().unwrap_or(converted);
            record[field] = json!(rounded.to_string());
        }
    }
}

pub(crate) fn read_native_archive_member(
    path: &std::path::Path,
    header: &[u8; 16],
    extension: &str,
) -> Result<Vec<u8>, String> {
    formats::clo_native::read_member(path, header, extension)
}

pub(crate) fn read_project_avatar_dan(
    path: &std::path::Path,
) -> Result<Option<(String, Vec<u8>)>, String> {
    formats::clo_native::read_project_avatar_dan(path)
}

pub(crate) fn mark_native_fabric_color(state: &mut Value, index: u64, clear_opacity: bool) {
    sync_shared_colorway_material(state, index);
    formats::native_save::mark_color(state, index, clear_opacity);
}

pub(crate) fn mark_native_fabric_texture(state: &mut Value, index: u64, map: &str, clear: bool) {
    sync_shared_colorway_material(state, index);
    formats::native_save::mark_texture(state, index, map, clear);
}

fn canonical_colorway(fabric: &Value, colorway: usize) -> usize {
    let Some(chips) = fabric["native_colorway_materials"]["chips"].as_array() else {
        return colorway;
    };
    chips.get(colorway).filter(|chip| chip.is_u64())
        .and_then(|chip| chips.iter().position(|candidate| candidate == chip))
        .unwrap_or(colorway)
}

fn sync_shared_colorway_material(state: &mut Value, index: u64) {
    let active = state["current_colorway"].as_u64().unwrap_or(0) as usize;
    let names = state["colorways"].clone();
    let Some(fabric) = state["fabrics"].as_array_mut()
        .and_then(|fabrics| fabrics.iter_mut().find(|fabric| fabric["index"] == index))
    else {
        return;
    };
    let variants = &fabric["native_colorway_materials"];
    if variants["names"] != names {
        return;
    }
    let Some(chips) = variants["chips"].as_array() else { return };
    let Some(chip) = chips.get(active).filter(|chip| chip.is_u64()) else { return };
    let shared: Vec<_> = chips.iter().enumerate()
        .filter(|(colorway, candidate)| *colorway != active && *candidate == chip)
        .map(|(colorway, _)| colorway)
        .collect();
    if shared.is_empty() {
        return;
    }
    let material = Value::Object(fabric.as_object().expect("fabric record").iter()
        .filter(|(key, _)| key.as_str() != "native_colorway_materials")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect());
    for colorway in shared {
        if let Some(variant) = fabric["native_colorway_materials"]["values"].get_mut(colorway) {
            *variant = material.clone();
        }
    }
}

type Reply = Result<Value, String>;

pub fn dispatch(name: &str, args: &Value, state: &mut Value) -> Option<Reply> {
    let handler: fn(&Value, &mut Value) -> Reply = match name {
        "scene_info" => scene::info,
        "scene_open" => scene::open,
        "__clo_import_project" => scene::import_project,
        "scene_save" => scene::save,
        "scene_export" => scene::export,
        "scene_clear" => scene::clear,
        "checkpoint_list" => scene::checkpoint_list,
        "checkpoint_save" => scene::checkpoint_save,
        "checkpoint_restore" => scene::checkpoint_restore,
        "pattern_list" => patterns::list,
        "pattern_info" => patterns::info,
        "__clo_pattern_outline" => patterns::outline_query,
        "__clo_set_elastic_values" => patterns::set_elastic_values,
        "pattern_create" => patterns::create,
        "pattern_delete" => patterns::delete,
        "pattern_edit_points" => patterns::edit,
        "pattern_ops" => patterns::operate,
        "pattern_set_arrangement" => patterns::arrange,
        "seams_list" => patterns::seams,
        "sew" => patterns::sew,
        _ => return None,
    };
    Some(handler(args, state))
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map(Vec::as_slice).unwrap_or_default()
}

fn vec_mut<'a>(state: &'a mut Value, key: &str) -> &'a mut Vec<Value> {
    if !state[key].is_array() {
        state[key] = json!([]);
    }
    state[key].as_array_mut().expect("array initialized")
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("{key} must be a string"))
}

fn flag(value: &Value, key: &str, default: bool) -> bool {
    value[key].as_bool().unwrap_or(default)
}

fn index(value: &Value, key: &str) -> Result<usize, String> {
    value[key]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(|| format!("{key} must be a nonnegative integer"))
}

fn next_id(state: &Value) -> Result<u64, String> {
    array(state, "patterns")
        .iter()
        .filter_map(|p| p["id"].as_u64())
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| "pattern ID space exhausted".into())
}

fn invalidate_geometry(state: &mut Value) {
    state["simulation"] = Value::Null;
}

pub(crate) fn use_partial_pose(state: &mut Value) {
    let pose = &mut state["simulation"];
    let uses_mm = pose["source"] == "native_saved_pose" || pose["texcoords_units"] == "mm";
    let Some(meshes) = pose.get_mut("meshes").and_then(Value::as_array_mut) else {
        return;
    };
    if meshes.is_empty() {
        return;
    }
    if uses_mm {
        for mesh in meshes {
            mesh["texcoords_units"] = json!("mm");
        }
        pose.as_object_mut().expect("simulation record").remove("texcoords_units");
    }
    pose["source"] = json!("mixed_saved_pose");
}

#[cfg(test)]
mod tests;
