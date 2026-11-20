use serde_json::{json, Value};
use std::{env, fs, path::PathBuf};

const DRAFT_FIELDS: [(&str, &str); 32] = [
    ("height", "HEIGHT_Total"),
    ("bust", "CIRCUMFERENCE_Bust"),
    ("underbust", "CIRCUMFERENCE_UnderBust"),
    ("waist", "CIRCUMFERENCE_Waist"),
    ("high_hip", "CIRCUMFERENCE_HighHip"),
    ("hip", "CIRCUMFERENCE_LowHip"),
    ("neck", "CIRCUMFERENCE_NeckBase"),
    ("armhole", "CIRCUMFERENCE_Armhole"),
    ("bicep", "CIRCUMFERENCE_Bicep"),
    ("elbow", "CIRCUMFERENCE_Elbow"),
    ("wrist", "CIRCUMFERENCE_Wrist"),
    ("thigh", "CIRCUMFERENCE_Thigh"),
    ("knee_circ", "CIRCUMFERENCE_Knee"),
    ("across_shoulder", "LENGTH_AcrossShoulder"),
    ("across_front", "LENGTH_AcrossFront"),
    ("across_back", "LENGTH_AcrossBack"),
    ("apex_width", "LENGTH_APEX_APEX"),
    ("h_hps", "HEIGHT_HPS"),
    ("h_shoulder", "HEIGHT_Shoulder"),
    ("h_apex", "HEIGHT_APEX"),
    ("h_underbust", "HEIGHT_UnderBust"),
    ("h_waist", "HEIGHT_Waist"),
    ("h_high_hip", "HEIGHT_HighHip"),
    ("h_hip", "HEIGHT_LowHip"),
    ("h_crotch", "HEIGHT_Crotch"),
    ("h_knee", "HEIGHT_Knee"),
    ("h_calf", "HEIGHT_Calf"),
    ("h_ankle", "HEIGHT_Ankle"),
    ("cf_neck_waist", "LENGTH_CFNeck_Waist"),
    ("cb_neck_waist", "LENGTH_CBNeck_Waist"),
    ("arm_length", "LENGTH_Arm"),
    ("shoulder_drop", "HEIGHT_ShoulderDrop"),
];

pub(super) fn read(avatar: &str) -> Option<(Value, String)> {
    let filename = cache_filename(avatar)?;
    let record = read_record("measurements", &filename)?;
    let body = draft_body(avatar, &record)?;
    Some((body, format!("cached CLO measurements ({filename})")))
}

pub(super) fn read_arrangement(avatar: &str) -> Option<Value> {
    let record = read_record("calibration", &cache_filename(avatar)?)?;
    if record["avatar"] != avatar || !record["points"].is_object() {
        return None;
    }
    Some(record["points"].clone())
}

fn read_record(directory: &str, filename: &str) -> Option<Value> {
    let state = env::var_os("CLO_MCP_STATE")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .or_else(|| env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".clo3d-mcp"))
        })?;
    let raw = fs::read(state.join(directory).join(filename)).ok()?;
    serde_json::from_slice(&raw).ok()
}

fn cache_filename(avatar: &str) -> Option<String> {
    if avatar.is_empty()
        || matches!(avatar, "." | "..")
        || avatar.contains(['/', '\\', ':'])
        || avatar.chars().any(char::is_control)
    {
        return None;
    }
    Some(format!("{avatar}.json"))
}

fn draft_body(avatar: &str, record: &Value) -> Option<Value> {
    if record
        .get("avatar")
        .is_some_and(|stored| stored.as_str() != Some(avatar))
    {
        return None;
    }
    let measurements = record["measurements"].as_object()?;
    let mut body = super::assets::default_body();
    let mut measured = false;
    for (field, name) in DRAFT_FIELDS {
        let Some(entry) = measurements.get(name) else {
            continue;
        };
        let value = if entry.is_object() {
            // CLO records these drafting measurements in millimetres.
            if entry["unit"].as_str().is_some_and(|unit| unit != "mm") {
                continue;
            }
            &entry["value"]
        } else {
            entry
        };
        if let Some(value) = value
            .as_f64()
            .or_else(|| value.as_str()?.parse::<f64>().ok())
            .filter(|value| value.is_finite())
        {
            measured = true;
            body[field] = json!(value);
        }
    }
    if !measured {
        return None;
    }
    body["source"] = json!("clo");
    Some(body)
}

#[cfg(test)]
#[test]
fn cache_uses_supported_names_units_and_avatar_identity() {
    let record = json!({
        "avatar": "Mia",
        "measurements": {
            "HEIGHT_Total": {"value": "1670.5", "unit": "mm"},
            "CIRCUMFERENCE_Waist": 700.0,
            "CIRCUMFERENCE_Bust": {"value": 38.0, "unit": "in"},
            "LENGTH_Arm": {"value": "NaN", "unit": "mm"}
        }
    });
    let body = draft_body("Mia", &record).unwrap();
    assert_eq!(body["height"], 1670.5);
    assert_eq!(body["waist"], 700.0);
    assert_eq!(body["bust"], super::assets::default_body()["bust"]);
    assert_eq!(
        body["arm_length"],
        super::assets::default_body()["arm_length"]
    );
    assert_eq!(body["source"], "clo");
    assert!(draft_body("Luka", &record).is_none());
    assert!(draft_body("Mia", &json!({"measurements": {}})).is_none());
    assert!(cache_filename("../Mia").is_none());
}
