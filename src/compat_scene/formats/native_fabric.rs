//! Read a standalone CLO fabric archive using the same typed maps as native projects.

use super::clo_native::{bounded_file, member, open_archive};
use super::native_materials::{face_records, face_type, read_face_colors};
use super::native_pose::{map_list, Fields, BYTE_TYPE, LIST_TYPE, MAP_TYPE};
use super::native_textures::{
    archive_texture_labels, fabric_maps, front_desaturated, image_bytes, remap_archive_textures,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
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

pub(super) fn catalog(record: &Fields<'_>, material: &Fields<'_>) -> Result<Value, String> {
    let (faces, legacy_front) = face_records(material)?;
    let mut info = json!({
        "Classification": text(record, b"fabricTypeUTF8").unwrap_or("None"),
        "Content": text(record, b"fabricContentUTF8").unwrap_or("None"),
        "Owner": text(record, b"qsOwnerUTF8").unwrap_or("None"),
        "SupplierName": text(record, b"qsSupplierNameUTF8").unwrap_or("None"),
    });
    for (index, face) in faces.iter().enumerate() {
        if face_type(face, index, legacy_front)? == 0 {
            let color = face.map(b"mapBaseColor")?;
            info["Color"] = json!(text(&color, b"qsNameUTF8"));
            info["Texture"] = json!(super::native_textures::base_texture_label(face));
            break;
        }
    }
    Ok(info)
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
        "base_texture_desaturated": super::native_textures::front_desaturated(&material)?,
        "stiffness": stiffness,
        "info": {
            "source": "native_zfab", "path": path,
            "fabric_type": text(&record, b"fabricTypeUTF8"),
            "composition": text(&record, b"fabricContentUTF8"),
            "catalog": catalog(&record, &material)?,
            "face_colors": faces.iter().map(|(kind, rgba)| json!({"type": kind, "rgba": rgba})).collect::<Vec<_>>(),
            "physical_properties": properties,
            "stiffness": stiffness, "warnings": warnings,
        },
    }))
}

pub(super) fn author(
    fabric: &Value,
    id: u32,
    images: &mut BTreeMap<String, Vec<u8>>,
    image_total: &mut u64,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let path = fabric["info"]["path"].as_str().ok_or("native fabric source is missing")?;
    let mut archive = open_archive(bounded_file(Path::new(path))?, FAB)?;
    let payload = member(&mut archive, "fab")?;
    let record = Fields::read(&payload)?;
    let material = active_material(&record)?;
    let original_color = read_face_colors(&material)?.into_iter()
        .find(|(kind, _)| *kind == 0).ok_or("native fabric front face is missing")?.1;
    let rgba: [f32; 4] = serde_json::from_value(fabric["rgba"].clone())
        .map_err(|_| "invalid native fabric color")?;
    let desaturated = fabric["base_texture_desaturated"] == true;
    let recolored = original_color.map(|value| value as f32) != rgba
        || desaturated != front_desaturated(&material)?;
    let (mut baseline, warnings) = fabric_maps(&material, &mut archive)?;
    if recolored && !crate::compat_fit::preserves_opacity_on_recolor(fabric) {
        baseline.as_object_mut().expect("native texture map").remove("opacity");
    }
    if !warnings.is_empty() || baseline != fabric["texture_maps"] {
        return Err("native fabric authoring requires the source texture bindings".into());
    }
    let physical = record.get(b"mapPhysical", MAP_TYPE)?.to_vec();
    let edited = if recolored {
        super::native_recolor::recolor_fabric_record(&record, fabric)?
    } else {
        payload
    };
    let mut edited = Fields::read(&edited)?.replace(b"uiID", 5, &id.to_le_bytes())?;
    let name = fabric["name"].as_str().ok_or("missing native fabric name")?;
    for key in [b"qsName".as_slice(), b"qsNameUTF8", b"qsFabricName", b"qsFabricNameUTF8"] {
        edited = Fields::read(&edited)?.set(key, BYTE_TYPE, name.as_bytes())?;
    }
    let names = archive.file_names().map(|name| {
        let name = name.map_err(|error| error.to_string())?.into_owned();
        Ok((name.to_ascii_lowercase(), name))
    }).collect::<Result<HashMap<_, _>, String>>()?;
    let mut renamed = HashMap::new();
    for key in archive_texture_labels(&edited)?.into_keys() {
        let name = names.get(&key).ok_or("native fabric references a missing texture")?;
        let bytes = image_bytes(&mut archive, name)?;
        *image_total += bytes.len() as u64;
        if *image_total > 256 * 1024 * 1024 {
            return Err("authored fabric textures exceed 256 MiB".into());
        }
        let target = format!("fabric_{id}_{name}");
        renamed.insert(key, target.clone());
        images.insert(target, bytes);
    }
    Ok((remap_archive_textures(&edited, &renamed)?.unwrap_or(edited), physical))
}
