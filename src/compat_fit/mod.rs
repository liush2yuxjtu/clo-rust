mod assets;
mod avt;
mod draft;
mod render;
mod solver;
mod texture;

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

#[cfg(test)]
mod texture_tests {
    use super::*;
    use std::{
        fs,
        io::BufReader,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    fn call(name: &str, args: Value, state: &mut Value) -> Result<Value, String> {
        dispatch(name, &args, state).expect("compat tool")
    }

    struct TempDirectory(PathBuf);
    impl TempDirectory {
        fn new() -> Self {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir =
                std::env::temp_dir().join(format!("clo-rust-texture-{}-{now}", std::process::id()));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn rgba_png(path: &Path, pixels: [u8; 8]) {
        let file = fs::File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&pixels).unwrap();
        writer.finish().unwrap();
    }

    fn capture(state: &mut Value, is_flat: bool) -> ([[u8; 3]; 2], Value) {
        let label = if is_flat { "snapshot_2d" } else { "snapshot" };
        let screenshot = call(label, json!({"views":["front"]}), state).unwrap();
        let path = screenshot["images"][0]["path"].as_str().unwrap();
        let file = fs::File::open(path).unwrap();
        let mut reader = png::Decoder::new(BufReader::new(file)).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut pixels).unwrap();
        assert_eq!(frame.color_type, png::ColorType::Rgb);
        let sample = |x, y| {
            let offset = (y * frame.width as usize + x) * 3;
            pixels[offset..offset + 3].try_into().unwrap()
        };
        fs::remove_file(path).unwrap();
        ([sample(245, 340), sample(555, 340)], screenshot)
    }

    #[test]
    fn applies_distinct_base_texels_to_flat_and_simulated_captures() {
        let dir = TempDirectory::new();
        let first = dir.0.join("red-blue.png");
        let second = dir.0.join("transparent-green.png");
        let damaged = dir.0.join("damaged.png");
        let unsupported = dir.0.join("not-png.jpg");
        rgba_png(&first, [255, 0, 0, 255, 0, 0, 255, 255]);
        rgba_png(&second, [0, 255, 0, 0, 0, 255, 0, 255]);
        fs::write(&damaged, b"\x89PNG\r\n\x1a\n").unwrap();
        fs::write(&unsupported, [0xff, 0xd8, 0xff, 0x00]).unwrap();
        let mut state = json!({
            "patterns":[{"id":0,"name":"Panel","points":[[0,1000],[100,1000],[100,1100],[0,1100]],"arrangement":"Front"}],
            "native_seam_groups":[{"id":123,"restored":false},{"id":567,"restored":true}]
        });
        call(
            "fabric_apply",
            json!({"fabric":"cotton","texture_image":first}),
            &mut state,
        )
        .unwrap();
        let simulation = call(
            "simulate",
            json!({"steps":0,"particle_distance":40}),
            &mut state,
        )
        .unwrap();
        assert!(array(&simulation["limitations"])
            .iter()
            .any(|warning| warning
                .as_str()
                .unwrap()
                .contains("Unrecovered native seam groups omitted from constraints: 1")));
        assert!(
            state["simulation"]["meshes"][0]["texcoords"]
                .as_array()
                .unwrap()
                .len()
                > 4
        );
        for flat in [true, false] {
            let ([left, right], response) = capture(&mut state, flat);
            assert!(
                left[0] > 150 && left[1] == 0 && left[2] == 0,
                "left: {left:?}"
            );
            assert!(
                right[2] > 150 && right[0] == 0 && right[1] == 0,
                "right: {right:?}"
            );
            assert!(response["render_warnings"].is_null());
        }
        for path in [&damaged, &unsupported] {
            assert!(call(
                "fabric_set_texture",
                json!({"fabric_index":0,"image_path":path}),
                &mut state
            )
            .is_err());
            assert_eq!(
                state["fabrics"][0]["texture_maps"]["base"],
                json!(first.canonicalize().unwrap())
            );
        }
        call(
            "fabric_set_texture",
            json!({"fabric_index":0,"image_path":second}),
            &mut state,
        )
        .unwrap();
        for flat in [true, false] {
            let ([left, right], _) = capture(&mut state, flat);
            assert!(
                left[0] > 100 && left[0] == left[1] && left[1] == left[2],
                "transparent: {left:?}"
            );
            assert!(
                right[1] > 150 && right[0] == 0 && right[2] == 0,
                "green: {right:?}"
            );
        }
        // Older persisted simulation scenes can predate rest UVs but still render.
        state["simulation"]["meshes"][0]
            .as_object_mut()
            .unwrap()
            .remove("texcoords");
        let ([left, right], old_capture) = capture(&mut state, false);
        assert_eq!(left, right);
        assert!(old_capture["render_warnings"][0]
            .as_str()
            .unwrap()
            .contains("without rest UVs"));
        call(
            "fabric_set_color",
            json!({"fabric_index":0,"color":"#ee2211"}),
            &mut state,
        )
        .unwrap();
        assert!(state["fabrics"][0]["texture_maps"]["base"].is_null());
        let ([left, right], _) = capture(&mut state, true);
        assert_eq!(left, right);
        assert!(left[0] > 200 && left[1] < 40 && left[2] < 30);
    }
}
