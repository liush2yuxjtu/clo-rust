use serde_json::{json, Value};

const MEASUREMENTS: &[(&str, &str)] = &[
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

pub(super) fn rows(body: &Value) -> Value {
    let measurements: Vec<_> = MEASUREMENTS
        .iter()
        .filter_map(|(key, name)| {
            let value = body[*key].as_f64()?;
            let kind = name.split('_').next().unwrap_or_default();
            Some(
                json!({"basicLength":value,"surfaceLength":0.0,"measurementName":name,
            "measurementType":kind,"unit":"mm","value":value}),
            )
        })
        .collect();
    Value::Array(measurements)
}
