mod api;
mod enums;
mod http;
mod measurements;
mod python;
mod ui;

use serde_json::{json, Value};

pub type DomainCall = fn(&str, &Value, &mut Value) -> Result<Value, String>;
pub(crate) type ToolResult = Result<Value, String>;

const START_HELP: &str = "Call clo_launch(with_bridge=true) to start the Rust scripting bridge.";

pub fn dispatch(
    name: &str,
    args: &Value,
    state: &mut Value,
    call: DomainCall,
) -> Option<ToolResult> {
    let result = match name {
        "clo_status" => Ok(status(state)),
        "clo_launch" => {
            let running = is_running(state);
            state["runtime"]["running"] = json!(true);
            if args["with_bridge"].as_bool().unwrap_or(true) {
                state["runtime"]["bridge"] = json!(true);
            }
            let workbench = if running {
                "CLO Rust is already running."
            } else {
                "CLO Rust workbench is running."
            };
            let bridge = if is_up(state) { "Bridge is up." } else { START_HELP };
            Ok(json!(format!("{workbench} {bridge}")))
        }
        "clo_stop_bridge" => {
            state["runtime"]["bridge"] = json!(false);
            state["runtime"]["handles"] = json!({});
            python::stop();
            Ok(json!("bridge stopped"))
        }
        "clo_control_guide" => Ok(json!({
            "route": [
                "Inspect the callable Rust API with clo_api_catalog and clo_api_describe.",
                "Use a domain tool or clo_api_call to modify the Rust scene.",
                "Use clo_ui_state and clo_ui_screenshot to inspect the Rust workbench.",
                "Call clo_ui_action with a current snapshot token and inspect its returned state."
            ],
            "codex_desktop": "The Rust workbench exposes its own accessibility tree and visual viewport.",
            "standalone_mcp": "The Rust workbench runs inside the CLO Rust engine session.",
            "limits": "The catalog is the surface currently implemented by this backend."
        })),
        "clo_ui_state" => ui::state(args, state),
        "clo_ui_action" => ui::action(args, state, call),
        "clo_ui_screenshot" => ui::screenshot(state, call),
        "clo_api_catalog" | "clo_api_search" | "clo_api_describe" | "clo_api_doc"
        | "clo_api_release" | "clo_api_call" | "clo_run_python" => {
            if !is_up(state) {
                return Some(Err(
                    "The CLO Rust bridge is stopped. Call clo_launch(with_bridge=true).".into(),
                ));
            }
            match name {
                "clo_api_catalog" => api::catalog(args),
                "clo_api_search" => {
                    let limit = args["limit"].as_i64().unwrap_or(40);
                    if !(1..=500).contains(&limit) {
                        Err("limit must be between 1 and 500".into())
                    } else {
                        api::catalog(&json!({"query": args["query"], "limit": limit})).and_then(pretty)
                    }
                }
                "clo_api_describe" => api::describe(args["name"].as_str().unwrap_or_default()),
                "clo_api_doc" => api::doc(args["function_name"].as_str().unwrap_or_default()).and_then(pretty),
                "clo_api_release" => api::release(args, state),
                "clo_api_call" => api::call(args, state, call),
                "clo_run_python" => python::run(args, state, call),
                _ => unreachable!(),
            }
        }
        _ => return None,
    };
    Some(result)
}

fn pretty(value: Value) -> ToolResult {
    serde_json::to_string_pretty(&value)
        .map(Value::String)
        .map_err(|error| error.to_string())
}

pub(super) fn is_running(state: &Value) -> bool {
    state["runtime"]["running"].as_bool().unwrap_or(true)
}

pub(crate) fn is_up(state: &Value) -> bool {
    state["runtime"]["bridge"].as_bool().unwrap_or(true)
}

fn scene_ui(state: &mut Value) -> Option<&mut serde_json::Map<String, Value>> {
    state
        .get_mut("runtime")
        .and_then(|runtime| runtime.get_mut("ui"))
        .and_then(Value::as_object_mut)
}

pub(crate) fn invalidate_scene_ui_snapshot(state: &mut Value) {
    if let Some(ui) = scene_ui(state) {
        ui.remove("observed");
    }
}

pub(crate) fn remove_deleted_pattern_selection(state: &mut Value, ids: &[Value]) {
    let Some(ui) = scene_ui(state) else {
        return;
    };
    for key in ["observed", "menu_x", "menu_y"] {
        ui.remove(key);
    }
    let last_selected = ui
        .get_mut("selected_keys")
        .and_then(Value::as_array_mut)
        .and_then(|keys| {
            keys.retain(|key| !ids.contains(key));
            keys.last().cloned()
        });
    if ui.get("primary_key").is_some_and(|key| ids.contains(key)) {
        ui.insert("primary_key".to_owned(), last_selected.unwrap_or(Value::Null));
        ui.remove("focus");
    }
}

pub(crate) fn clear_scene_ui(state: &mut Value, keep_view: bool) {
    let Some(ui) = scene_ui(state) else {
        return;
    };
    if keep_view {
        for key in ["observed", "selected_keys", "primary_key", "focus", "menu_x", "menu_y"] {
            ui.remove(key);
        }
    } else {
        ui.retain(|key, _| key == "sequence");
    }
}

fn status(state: &Value) -> Value {
    let mut status = json!({
        "backend": "clo-rust",
        "bridge": if is_up(state) { "up" } else { "down" },
        "clo_running": is_running(state),
        "pid": std::process::id(),
        "engine_version": env!("CARGO_PKG_VERSION")
    });
    if !is_up(state) {
        status["how_to_start"] = json!(START_HELP);
    }
    status
}

fn text<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{name} must be a string"))
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

#[cfg(test)]
mod tests;
