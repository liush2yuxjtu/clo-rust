use super::dispatch;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn scene() -> Value {
    json!({
        "patterns": [], "seams": [], "fabrics": [], "avatar": {"name": "Sample Avatar", "arrangement_points": ["Front"]},
        "simulation": null
    })
}

fn tool(state: &mut Value, name: &str, args: Value) -> Value {
    dispatch(name, &args, state)
        .expect("registered operation")
        .expect("successful operation")
}

fn piece(state: &mut Value, name: &str) {
    tool(
        state,
        "pattern_create",
        json!({
            "name": name, "points": [[0,0],[100,0],[100,50],[0,50]], "arrangement": "Front"
        }),
    );
}

struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("clo-rust-scene-test-{}-{now}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn pattern_edit_query_and_failed_edit_are_atomic() {
    let mut state = scene();
    piece(&mut state, "Front");
    tool(
        &mut state,
        "pattern_ops",
        json!({"pattern":"0","op":"move_2d","args":{"x":150,"y":10}}),
    );
    let info = tool(&mut state, "pattern_info", json!({"pattern":"Front"}));
    assert_eq!(info["pos_2d"], json!([150., 10.]));
    assert_eq!(info["bbox"]["Width"], "100.000000");
    assert_eq!(
        info["line_info"]["lines"][0]["start"],
        json!({"x":100., "y":-15.})
    );
    assert_eq!(info["line_info"]["lineCount"], 4);
    let r = tool(
        &mut state,
        "pattern_edit_points",
        json!({"pattern":"Front","moves":[{"point":0,"dx":-10},{"point":1,"x":230,"y":-10}]}),
    );
    assert_eq!(r["moved"][0]["x"], 90.);
    let current = state.clone();
    assert!(dispatch(
        "pattern_edit_points",
        &json!({"pattern":"Front","moves":[{"point":0,"dx":10},{"point":20,"dx":1}]}),
        &mut state
    )
    .unwrap()
    .is_err());
    assert_eq!(state, current);
    assert!(dispatch(
        "pattern_set_arrangement",
        &json!({"pattern":"0","point":"missing"}),
        &mut state
    )
    .unwrap()
    .is_err());
    assert_eq!(state, current);
}

#[test]
fn seams_survive_pattern_index_changes_and_round_trip() {
    let root = Temporary::new();
    let mut state = scene();
    piece(&mut state, "Remove");
    piece(&mut state, "Front");
    piece(&mut state, "Back");
    let request = json!({"pattern_a":"Front","line_a":0,"pattern_b":"2","line_b":2,"dir_b":false});
    assert_eq!(tool(&mut state, "sew", request.clone())["seam_groups"], 1);
    assert_eq!(tool(&mut state, "sew", request)["ok"], false);
    tool(
        &mut state,
        "pattern_delete",
        json!({"patterns":["0", "Remove"]}),
    );
    assert_eq!(
        tool(&mut state, "seams_list", json!({}))["groups"][0]["patterns"],
        json!(["Front", "Back"])
    );
    let path = root.0.join("test.clorust");
    tool(&mut state, "scene_save", json!({"path":path}));
    tool(&mut state, "scene_clear", json!({"keep_avatar":false}));
    assert!(state["avatar"].is_null());
    tool(&mut state, "scene_open", json!({"path":path}));
    assert_eq!(tool(&mut state, "seams_list", json!({}))["count"], 1);
    assert_eq!(state["seams"][0]["dir_b"], false);
    assert_eq!(state["avatar"]["name"], "Sample Avatar");
    let current = state.clone();
    let native = root.0.join("native.zprj");
    fs::write(&native, b" ZPRJ").unwrap();
    let error = dispatch("scene_open", &json!({"path":native}), &mut state)
        .unwrap()
        .unwrap_err();
    assert!(error.contains("native file header"));
    assert_eq!(state, current);
}

#[test]
fn export_import_formats_use_geometry_and_append_remaps_ids() {
    let root = Temporary::new();
    let mut state = scene();
    piece(&mut state, "Front");
    let draft = root.0.join("piece.json");
    let dxf = root.0.join("piece.dxf");
    for path in [&draft, &dxf] {
        tool(&mut state, "scene_export", json!({"path":path}));
    }
    for path in [&draft, &dxf] {
        tool(
            &mut state,
            "scene_open",
            json!({"path":path, "mode":"append"}),
        );
    }
    let ids: Vec<_> = state["patterns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| &p["id"])
        .collect();
    assert_eq!(ids, [&json!(1), &json!(2), &json!(3)]);
    for ext in ["obj", "glb", "gltf"] {
        let path = root.0.join(format!("piece.{ext}"));
        tool(&mut state, "scene_export", json!({"path":path}));
        let mut readback = scene();
        tool(&mut readback, "scene_open", json!({"path":path}));
        assert_eq!(readback["imported_meshes"].as_array().unwrap().len(), 3);
        for mesh in readback["imported_meshes"].as_array().unwrap() {
            assert_eq!(mesh["positions"].as_array().unwrap().len(), 4);
            assert_eq!(mesh["triangles"].as_array().unwrap().len(), 2);
            assert!(mesh["positions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| (p[0].as_f64().unwrap() - 100.).abs() < 1e-4));
        }
    }
}

#[test]
fn portable_project_ignores_archived_runtime_state() {
    let root = Temporary::new();
    let path = root.0.join("session.clorust");
    let mut state = scene();
    piece(&mut state, "Panel");
    state["runtime"] = json!({"instance": "before", "script_handles": ["one"]});
    tool(&mut state, "scene_save", json!({"path": path}));
    let mut archive: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(archive["scene"].get("runtime").is_none());
    archive["scene"]["runtime"] = json!({"instance": "forged"});
    fs::write(&path, serde_json::to_vec(&archive).unwrap()).unwrap();
    let active = json!({"instance": "current", "script_handles": ["two"]});
    state["runtime"] = active.clone();
    tool(&mut state, "scene_open", json!({"path": path}));
    assert_eq!(state["runtime"], active);
}
