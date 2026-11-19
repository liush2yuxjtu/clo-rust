mod assets;
mod draft;
mod render;
mod solver;

use serde_json::{json, Value};

pub fn dispatch(name: &str, args: &Value, state: &mut Value) -> Option<Result<Value, String>> {
    Some(match name {
        "avatar_list" => Ok(assets::avatar_list(state)),
        "avatar_load" => assets::avatar_load(args, state),
        "avatar_measurements" => assets::avatar_measurements(args, state),
        "arrangement_points" => assets::arrangement_points(state),
        "fabric_library" => Ok(assets::fabric_library(args)),
        "fabric_list" => Ok(assets::fabric_list(state)),
        "fabric_apply" => assets::fabric_apply(args, state),
        "fabric_set_color" => assets::fabric_set_color(args, state),
        "fabric_set_texture" => assets::fabric_set_texture(args, state),
        "garment_plan_preview" => draft::preview(args, state),
        "garment_build" => draft::build(args, state, false),
        "garment_rebuild" => draft::build(args, state, true),
        "garment_last_spec" => Ok(draft::last_spec(state)),
        "simulate" => solver::simulate(args, state),
        "simulation_reset" => {
            state["simulation"] = Value::Null;
            Ok(json!({"ok": true}))
        }
        "snapshot" => render::snapshot(args, state, false),
        "snapshot_2d" => render::snapshot(args, state, true),
        _ => return None,
    })
}

fn array_mut<'a>(state: &'a mut Value, key: &str) -> &'a mut Vec<Value> {
    if !state[key].is_array() {
        state[key] = json!([]);
    }
    state[key]
        .as_array_mut()
        .expect("state array just initialized")
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn boolean(value: &Value, key: &str, default: bool) -> bool {
    value[key].as_bool().unwrap_or(default)
}

fn string<'a>(value: &'a Value, key: &str, default: &'a str) -> &'a str {
    value[key].as_str().unwrap_or(default)
}

fn number(value: &Value, key: &str, default: f64) -> f64 {
    value[key].as_f64().unwrap_or(default)
}

fn pattern_index(patterns: &[Value], reference: &Value) -> Result<usize, String> {
    if let Some(name) = reference.as_str() {
        if let Some(found) = patterns.iter().position(|p| p["name"] == name) {
            return Ok(found);
        }
        if let Ok(index) = name.parse::<usize>() {
            if index < patterns.len() {
                return Ok(index);
            }
        }
    }
    if let Some(found) = patterns.iter().position(|p| p["id"] == *reference) {
        return Ok(found);
    }
    Err(format!("no pattern matching {reference}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invoke(name: &str, args: Value, state: &mut Value) -> Value {
        dispatch(name, &args, state).unwrap().unwrap()
    }

    #[test]
    fn garment_workflow_changes_rust_state_and_writes_image() {
        let mut state = json!({});
        let preview = invoke(
            "garment_plan_preview",
            json!({"spec":{"garment":"top"}}),
            &mut state,
        );
        assert_eq!(preview["pieces"].as_array().unwrap().len(), 2);
        assert!(preview["pieces"][0].get("points").is_none());
        assert_eq!(state, json!({}));
        let build = invoke(
            "garment_build",
            json!({"spec":{"garment":"top"},"simulate_steps":0,"snapshots":false}),
            &mut state,
        );
        assert_eq!(build["pieces"].as_object().unwrap().len(), 2);
        let original = state["patterns"][0]["points"].clone();
        invoke(
            "garment_rebuild",
            json!({"changes":{"length":"cropped"},"simulate_steps":0,"snapshots":false}),
            &mut state,
        );
        assert_ne!(state["patterns"][0]["points"], original);
        assert_eq!(
            invoke("garment_last_spec", json!({}), &mut state)["spec"]["length"],
            "cropped"
        );
        let simulation = invoke(
            "simulate",
            json!({"steps":2,"particle_distance":35}),
            &mut state,
        );
        assert_eq!(simulation["ok"], true);
        assert!(
            state["simulation"]["meshes"][0]["triangles"]
                .as_array()
                .unwrap()
                .len()
                > 10
        );
        let previous = state["simulation"]["meshes"][0]["positions"].clone();
        invoke(
            "simulate",
            json!({"steps":2,"particle_distance":35}),
            &mut state,
        );
        assert_ne!(state["simulation"]["meshes"][0]["positions"], previous);
        assert_eq!(state["simulation"]["steps"], 4);
        let images = invoke("snapshot", json!({"views":["front","back"]}), &mut state);
        for img in array(&images["images"]) {
            let path = img["path"].as_str().unwrap();
            let png = std::fs::read(path).unwrap();
            assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n");
            std::fs::remove_file(path).unwrap();
        }
        let image = invoke("snapshot_2d", json!({}), &mut state);
        std::fs::remove_file(image["path"].as_str().unwrap()).unwrap();
        invoke("simulation_reset", json!({}), &mut state);
        assert!(state["simulation"].is_null());
    }

    #[test]
    fn invalid_material_application_does_not_mutate_state() {
        let mut state = json!({"patterns":[{"id":0,"name":"Panel"}]});
        assert!(dispatch(
            "fabric_apply",
            &json!({"fabric":"cotton","patterns":["missing"]}),
            &mut state
        )
        .unwrap()
        .is_err());
        assert_eq!(state, json!({"patterns":[{"id":0,"name":"Panel"}]}));
        let fabric = invoke(
            "fabric_apply",
            json!({"fabric":"cotton","color":"#0f0"}),
            &mut state,
        );
        let index = fabric["fabric"]["index"].clone();
        assert_eq!(state["patterns"][0]["fabric_index"], index);
        assert_eq!(fabric["rgb"], json!([0.0, 1.0, 0.0]));
        let color = invoke(
            "fabric_set_color",
            json!({"fabric_index":index,"color":"navy"}),
            &mut state,
        );
        assert_eq!(color["result"]["ok"], json!([true, true, true]));
    }
}
