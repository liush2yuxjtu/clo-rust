//! Read a standalone CLO fabric archive using the same typed maps as native projects.

use super::clo_native::{bounded_file, member, open_archive};
use super::native_materials::read_face_colors;
use super::native_pose::{map_list, Fields, BYTE_TYPE, LIST_TYPE, MAP_TYPE};
use super::native_textures::fabric_maps;
use serde_json::{json, Value};
use std::path::Path;

const FAB: &[u8; 16] = b" FAB        CLO ";

fn text<'a>(record: &Fields<'a>, key: &[u8]) -> Option<&'a str> {
    std::str::from_utf8(record.get(key, BYTE_TYPE).ok()?)
        .ok()
        .filter(|name| !name.is_empty() && *name != "None")
}

fn float(record: &Fields<'_>, key: &[u8]) -> Option<f64> {
    let value = f32::from_le_bytes(record.get(key, 6).ok()?.try_into().ok()?);
    value.is_finite().then_some(f64::from(value))
}

fn active_material<'a>(record: &Fields<'a>) -> Result<Fields<'a>, String> {
    if let Ok(material) = record.get(b"mapMaterial2D", MAP_TYPE) {
        return Fields::read(material);
    }
    // Older fabrics store the active front material in the colorway record.
    let colorways = record.map(b"mapColorwayInfo")?;
    let current = u32::from_le_bytes(
        colorways
            .get(b"uiCurrentCoordinationIndex", 5)?
            .try_into()
            .map_err(|_| "invalid fabric colorway index width")?,
    ) as usize;
    map_list(colorways.get(b"listColorwayInfo", LIST_TYPE)?, 1024)?
        .into_iter()
        .nth(current)
        .ok_or_else(|| "active fabric colorway is missing".into())
}

pub(super) fn physical_properties(record: &Fields<'_>) -> Result<Value, String> {
    let physical = record.map(b"mapPhysical")?;
    let properties: serde_json::Map<String, Value> = [
        ("fSuK", "stretch_u"),
        ("fSvK", "stretch_v"),
        ("fHK", "shear"),
        ("fLeftShearK", "left_shear"),
        ("fBuK", "bend_u"),
        ("fBvK", "bend_v"),
        ("fBhK", "bend_bias"),
        ("fDensity", "density"),
        ("fThickness", "thickness"),
        ("fFriction", "friction"),
    ]
    .into_iter()
    .filter_map(|(key, name)| Some((name.to_owned(), json!(float(&physical, key.as_bytes())?))))
    .collect();
    Ok(Value::Object(properties))
}

pub(crate) fn read(path: &Path) -> Result<Value, String> {
    let mut archive = open_archive(bounded_file(path)?, FAB)?;
    let payload = member(&mut archive, "fab")?;
    let record = Fields::read(&payload)?;
    let properties = physical_properties(&record)?;
    let name = text(&record, b"qsNameUTF8")
        .or_else(|| text(&record, b"qsFabricNameUTF8"))
        .or_else(|| path.file_stem()?.to_str())
        .ok_or("native fabric name is missing")?;
    let material = active_material(&record)?;
    let faces = read_face_colors(&material)?;
    let color = faces
        .iter()
        .find(|(kind, _)| *kind == 0)
        .map(|(_, color)| *color)
        .ok_or("native fabric front face is missing")?;
    let (texture_maps, warnings) = fabric_maps(&material, &mut archive)?;
    // Preserve the scalar for consumers of previously saved .clorust projects.
    let mut stiffness = 0.9;
    if let (Some(u), Some(v)) = (properties.get("stretch_u"), properties.get("stretch_v")) {
        let axial = (u.as_f64().unwrap_or_default() + v.as_f64().unwrap_or_default()) * 0.5;
        if axial >= 0.0 {
            stiffness = axial / (axial + 20_000.0);
        }
    }
    Ok(json!({
        "name": name, "color": &color[..3], "rgba": color, "texture_maps": texture_maps,
        "stiffness": stiffness,
        "info": {
            "source": "native_zfab", "path": path,
            "fabric_type": text(&record, b"fabricTypeUTF8"),
            "composition": text(&record, b"fabricContentUTF8"),
            "face_colors": faces.iter().map(|(kind, rgba)| json!({"type": kind, "rgba": rgba})).collect::<Vec<_>>(),
            "physical_properties": properties,
            "stiffness": stiffness, "warnings": warnings,
        },
    }))
}
