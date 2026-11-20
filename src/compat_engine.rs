use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

#[derive(Deserialize)]
struct Request {
    name: String,
    #[serde(default = "empty_args")]
    args: Value,
}

fn empty_args() -> Value {
    json!({})
}

fn call_domain(name: &str, args: &Value, state: &mut Value) -> Result<Value, String> {
    crate::compat_scene::dispatch(name, args, state)
        .or_else(|| crate::compat_fit::dispatch(name, args, state))
        .ok_or_else(|| format!("Unknown CLO Rust domain operation: {name}"))?
}

fn execute(line: &str, state: &mut Value) -> Result<Value, String> {
    let request: Request = serde_json::from_str(line).map_err(|error| error.to_string())?;
    if !request.args.is_object() {
        return Err("Tool arguments must be a JSON object".into());
    }
    let mut candidate = state.clone();
    let result =
        crate::compat_runtime::dispatch(&request.name, &request.args, &mut candidate, call_domain)
            .unwrap_or_else(|| {
                let offline = match request.name.as_str() {
                    "checkpoint_list" | "garment_last_spec" => true,
                    "garment_plan_preview" => !request.args["use_scene_measurements"]
                        .as_bool()
                        .unwrap_or(false),
                    _ => false,
                };
                if offline || crate::compat_runtime::is_up(&candidate) {
                    call_domain(&request.name, &request.args, &mut candidate)
                } else {
                    Err("The CLO Rust bridge is stopped. Call clo_launch(with_bridge=true).".into())
                }
            });
    // Some tools retain completed steps if a later step fails.
    if result.is_ok() || matches!(request.name.as_str(),
        "clo_run_python" | "clo_ui_action" | "garment_build" | "garment_rebuild"
            | "fabric_apply" | "fabric_set_texture" | "pattern_create"
    ) {
        *state = candidate;
    }
    result
}

pub fn run() -> Result<(), String> {
    let mut state = json!({
        "patterns": [], "seams": [], "avatar": null, "fabrics": [],
        "garment_spec": null, "simulation": null
    });
    let stdout = io::stdout();
    let mut output = stdout.lock();
    for line in io::stdin().lock().lines() {
        let line = line.map_err(|error| error.to_string())?;
        let response = match execute(&line, &mut state) {
            Ok(result) => json!({"result": result}),
            Err(error) => json!({"error": error}),
        };
        serde_json::to_writer(&mut output, &response).map_err(|error| error.to_string())?;
        output.write_all(b"\n").map_err(|error| error.to_string())?;
        output.flush().map_err(|error| error.to_string())?;
    }
    Ok(())
}
