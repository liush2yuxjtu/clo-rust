mod annotations;
mod formats;
mod patterns;
mod scene;

use serde_json::{json, Value};

pub(crate) fn read_native_archive_member(
    path: &std::path::Path,
    header: &[u8; 16],
    extension: &str,
) -> Result<Vec<u8>, String> {
    formats::clo_native::read_member(path, header, extension)
}

type Reply = Result<Value, String>;

pub fn dispatch(name: &str, args: &Value, state: &mut Value) -> Option<Reply> {
    let handler: fn(&Value, &mut Value) -> Reply = match name {
        "scene_info" => scene::info,
        "scene_open" => scene::open,
        "scene_save" => scene::save,
        "scene_export" => scene::export,
        "scene_clear" => scene::clear,
        "checkpoint_list" => scene::checkpoint_list,
        "checkpoint_save" => scene::checkpoint_save,
        "checkpoint_restore" => scene::checkpoint_restore,
        "pattern_list" => patterns::list,
        "pattern_info" => patterns::info,
        "__clo_pattern_outline" => patterns::outline_query,
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

fn number(value: &Value, key: &str) -> Result<f64, String> {
    value[key]
        .as_f64()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("{key} must be a finite number"))
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

#[cfg(test)]
mod tests;
