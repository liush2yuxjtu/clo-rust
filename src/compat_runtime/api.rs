use super::{array, enums, text, DomainCall, ToolResult};
use crate::compat_scene::seam_group_names;
use serde_json::{json, Map, Value};

const MODULES: [&str; 7] = [
    "import_api",
    "export_api",
    "fabric_api",
    "pattern_api",
    "utility_api",
    "rest_api",
    "ApiTypes",
];

struct Entry {
    name: &'static str,
    signature: &'static str,
    doc: &'static str,
}

macro_rules! functions {
    ($(($name:literal, $signature:literal, $doc:literal)),* $(,)?) => {
        &[$(Entry { name: $name, signature: $signature, doc: $doc }),*]
    };
}

const FUNCTIONS: &[Entry] = functions![
    (
        "import_api.ImportAvatar",
        "(path, option=None)",
        "Import an avatar into the current scene."
    ),
    (
        "import_api.ImportDXF",
        "(path, option=None)",
        "Import DXF pattern geometry."
    ),
    (
        "import_api.ImportFile",
        "(path)",
        "Import a supported file into the current scene."
    ),
    (
        "import_api.ImportZpac",
        "(path, option=None)",
        "Open a garment project."
    ),
    (
        "import_api.ImportZprj",
        "(path, option=None)",
        "Open a garment scene."
    ),
    (
        "export_api.ExportDXF",
        "(path, option=None)",
        "Export the 2D pattern geometry as DXF."
    ),
    (
        "export_api.ExportFBX",
        "(path, option=None)",
        "Export the active garment as FBX and return the generated file paths."
    ),
    (
        "export_api.ExportGLB",
        "(path, option=None)",
        "Export the active garment as GLB."
    ),
    (
        "export_api.ExportGLTF",
        "(path, option=None, embed=False)",
        "Export the active garment as glTF."
    ),
    (
        "export_api.ExportOBJ",
        "(path, option=None)",
        "Export the active garment as OBJ."
    ),
    (
        "export_api.ExportSnapshot2D",
        "(path, mode)",
        "Export the current 2D pattern layout to PNG. The Rust renderer supports layout mode 0."
    ),
    (
        "export_api.ExportThumbnail3D",
        "(path)",
        "Export a PNG of the current garment from the selected camera direction."
    ),
    (
        "export_api.ExportZPac",
        "(path)",
        "Save the garment scene to a garment file."
    ),
    (
        "export_api.ExportZPrj",
        "(path, thumbnail=False)",
        "Save the scene to a project file."
    ),
    (
        "export_api.GetAvatarCount",
        "()",
        "Get the number of avatars in the current scene."
    ),
    (
        "export_api.GetAvatarNameList",
        "()",
        "Get the names of in-scene avatars."
    ),
    (
        "export_api.GetColorwayNameList",
        "()",
        "Get the colorways in this scene."
    ),
    (
        "fabric_api.AddFabric",
        "(path)",
        "Add a fabric from an available preset or file."
    ),
    (
        "fabric_api.AssignFabricToPattern",
        "(fabricIndex, patternIndex, option=3)",
        "Assign the fabric to the specified pattern."
    ),
    (
        "fabric_api.GetCurrentFabricIndex",
        "()",
        "Get the active fabric index."
    ),
    (
        "fabric_api.GetFabricCount",
        "(colorway=False)",
        "Get the number of fabrics."
    ),
    (
        "fabric_api.GetFabricInformation",
        "(index)",
        "Get descriptive or physical properties of the fabric."
    ),
    (
        "fabric_api.GetFabricName",
        "(index)",
        "Get the fabric name by index."
    ),
    (
        "fabric_api.GetFabricPBRMaterialBaseColor",
        "(index, face=0)",
        "Get the base material color."
    ),
    (
        "fabric_api.SetBaseTextureMapImageGivenFilePath",
        "(path)",
        "Set the active fabric's base texture."
    ),
    (
        "fabric_api.SetCurrentFabricIndex",
        "(index)",
        "Set the active fabric by index."
    ),
    (
        "fabric_api.SetDisplacementMapImageGivenFilePath",
        "(path)",
        "Set the active fabric's displacement map."
    ),
    (
        "fabric_api.SetFabricName",
        "(index, name)",
        "Set the fabric name by index."
    ),
    (
        "fabric_api.SetFabricPBRMaterialBaseColor",
        "(index, face, red, green, blue, alpha=1.0)",
        "Set the material base color."
    ),
    (
        "fabric_api.SetMetalnessMapImageGivenFilePath",
        "(path)",
        "Set the active fabric's metalness map."
    ),
    (
        "fabric_api.SetNormalMapImageGivenFilePath",
        "(path)",
        "Set the active fabric's normal map."
    ),
    (
        "fabric_api.SetOpacityMapImageGivenFilePath",
        "(path)",
        "Set the active fabric's opacity map."
    ),
    (
        "fabric_api.SetRoughnessMapImageGivenFilePath",
        "(path)",
        "Set the active fabric's roughness map."
    ),
    (
        "pattern_api.AddSeamlinePairGroup",
        "(patternA, lineA, patternB, lineB, directionA=True, directionB=True)",
        "Create a sewing group between two pattern edges."
    ),
    (
        "pattern_api.CopyPatternPieceMove",
        "(index, dx, dy)",
        "Duplicate and translate a pattern piece."
    ),
    (
        "pattern_api.CreatePatternWithPoints",
        "(points)",
        "Create a pattern from planar points in millimetres."
    ),
    (
        "pattern_api.DeleteLine",
        "(index, line)",
        "Delete a pattern line by index."
    ),
    (
        "pattern_api.DeletePatternPiece",
        "(index)",
        "Delete a pattern piece."
    ),
    (
        "pattern_api.DeletePoint",
        "(index, point)",
        "Delete a pattern point by index."
    ),
    (
        "pattern_api.ExportPatternJSON",
        "(path)",
        "Export the pattern data to a JSON file."
    ),
    (
        "pattern_api.FlipPatternPiece",
        "(index, horizontal=True, each=True)",
        "Reflect a pattern piece."
    ),
    (
        "pattern_api.GetArrangementList",
        "()",
        "Get available named avatar arrangement points."
    ),
    (
        "pattern_api.GetArrangementOfPattern",
        "(index)",
        "Get the arrangement assigned to a pattern."
    ),
    (
        "pattern_api.GetBoundingBoxOfPattern",
        "(index)",
        "Get the planar bounding box of a pattern."
    ),
    (
        "pattern_api.GetParticleDistanceOfPattern",
        "(index)",
        "Get the pattern particle distance in millimetres."
    ),
    (
        "pattern_api.GetLineLength",
        "(index, line, internalLine=None)",
        "Return the length of an outline edge in millimetres."
    ),
    (
        "pattern_api.GetPatternIndexFrom2DView",
        "(x, y)",
        "Find the pattern at a workbench viewport location."
    ),
    (
        "pattern_api.GetPatternPieceArea",
        "(index)",
        "Return the area of the pattern outline in square millimetres."
    ),
    (
        "pattern_api.GetPatternCount",
        "()",
        "Get the number of pattern pieces in the current scene."
    ),
    (
        "pattern_api.GetPatternInputInformation",
        "(index=None)",
        "Get the source points and segments as JSON text."
    ),
    (
        "pattern_api.GetPatternLayer",
        "(index)",
        "Get the pattern layer."
    ),
    (
        "pattern_api.GetPatternLineInfo",
        "(index)",
        "Get edge geometry and lengths as JSON text."
    ),
    (
        "pattern_api.GetPatternPieceFabricIndex",
        "(index)",
        "Get the fabric index assigned to this pattern."
    ),
    (
        "pattern_api.GetPatternPieceName",
        "(index)",
        "Get the pattern name by index."
    ),
    (
        "pattern_api.GetPatternPiecePos",
        "(index)",
        "Get the pattern's planar position."
    ),
    (
        "pattern_api.GetSeamlinePairGroupCount",
        "()",
        "Get the scene's sewing group count."
    ),
    (
        "pattern_api.GetSeamlinePairGroupListInPattern",
        "(index)",
        "Get the sewing groups connected to the pattern."
    ),
    (
        "pattern_api.GetSeamlinePairGroupName",
        "(index)",
        "Get a sewing group name by index."
    ),
    (
        "pattern_api.ImportPatternJSON",
        "(path)",
        "Import the pattern data from a JSON file."
    ),
    (
        "pattern_api.MovePatternPoint",
        "(index, point, x, y)",
        "Move a point to a planar position in millimetres."
    ),
    (
        "pattern_api.SetArrangement",
        "(index, arrangement)",
        "Place a pattern at a named arrangement point."
    ),
    (
        "pattern_api.SetParticleDistanceOfPattern",
        "(index, mm)",
        "Set pattern particle distance in millimetres."
    ),
    (
        "pattern_api.SetParticleDistanceOfPatterns",
        "(mm)",
        "Set particle distance for all patterns."
    ),
    (
        "pattern_api.SetPatternFreeze",
        "(index, on)",
        "Freeze or unfreeze a pattern in simulation."
    ),
    (
        "pattern_api.SetPatternHide3D",
        "(index, on)",
        "Hide or show a pattern in the simulated view."
    ),
    (
        "pattern_api.SetPatternLayer",
        "(index, layer)",
        "Set the pattern's simulation layer."
    ),
    (
        "pattern_api.SetPatternPieceElastic",
        "(index, line, on)",
        "Set elasticity on a pattern edge."
    ),
    (
        "pattern_api.SetPatternPieceElasticStrength",
        "(index, line, strength)",
        "Set strength of the elastic edge."
    ),
    (
        "pattern_api.SetPatternPieceElasticTotalLength",
        "(index, line, total_length)",
        "Set the total length of an elastic edge."
    ),
    (
        "pattern_api.SetPatternPieceMove",
        "(index, dx, dy)",
        "Translate a pattern in the 2D plane."
    ),
    (
        "pattern_api.SetPatternPieceName",
        "(index, name)",
        "Rename a pattern piece."
    ),
    (
        "pattern_api.SetPatternPiecePos",
        "(index, x, y)",
        "Set a pattern piece's planar position."
    ),
    (
        "pattern_api.SetPatternPieceShirring",
        "(index, line, on)",
        "Enable or disable shirring on a pattern edge."
    ),
    (
        "pattern_api.SetPatternStrengthen",
        "(index, on)",
        "Strengthen the pattern for simulation."
    ),
    (
        "pattern_api.SymmetryPatternPiece",
        "(index, with_sewing=True)",
        "Create a symmetric duplicate of a pattern."
    ),
    (
        "pattern_api.UnfoldPatternPiece",
        "(index, line, half_symmetry=True)",
        "Unfold a pattern around the selected edge."
    ),
    (
        "pattern_api.GetArrangementOfPatternW",
        "(name)",
        "Get the arrangement assigned to the named pattern."
    ),
    (
        "pattern_api.GetBoundingBoxOfPatternW",
        "(name)",
        "Get the bounding box of the named pattern."
    ),
    (
        "pattern_api.GetParticleDistanceOfPatternW",
        "(name)",
        "Get the particle distance of the named pattern."
    ),
    (
        "pattern_api.GetPatternIndex",
        "(name)",
        "Find the scene index of the named pattern."
    ),
    (
        "pattern_api.GetPatternIndexW",
        "(name)",
        "Find the scene index of the named pattern."
    ),
    (
        "pattern_api.GetPatternInputInformationW",
        "(index=None)",
        "Get one or all pattern inputs as JSON text."
    ),
    (
        "pattern_api.GetSeamlinePairGroupIndexFromName",
        "(name)",
        "Find the scene index of the named sewing group."
    ),
    (
        "pattern_api.GetSeamlinePairGroupIndexFromNameW",
        "(name)",
        "Find the scene index of the named sewing group."
    ),
    (
        "pattern_api.GetSeamlinePairGroupNameW",
        "(index)",
        "Get the name of the sewing group."
    ),
    (
        "pattern_api.GetSelectedPattern",
        "()",
        "List the pattern names selected in the workbench."
    ),
    (
        "pattern_api.GetSelectedPatternViaIndex",
        "(index)",
        "Check whether the pattern is selected."
    ),
    (
        "pattern_api.SelectPatternViaIndex",
        "(index, select)",
        "Change selection for one pattern."
    ),
    (
        "pattern_api.SelectPatternViaName",
        "(name, select)",
        "Change selection for the named pattern."
    ),
    (
        "pattern_api.SetPatternPieceFabricIndex",
        "(index, fabric)",
        "Assign an existing fabric to the specified pattern."
    ),
    (
        "utility_api.DeleteAvatar",
        "(indices)",
        "Delete the specified in-scene avatars."
    ),
    (
        "utility_api.GetAvatarMeasurements",
        "(index)",
        "Get the active avatar measurements."
    ),
    (
        "utility_api.GetCLOTemporaryFolderPath",
        "()",
        "Get the Rust workbench temporary directory."
    ),
    (
        "utility_api.GetCLOAssetFolderPath",
        "(bLinuxTypePathDelimeter=True)",
        "Return the configured CLO asset folder, or an empty string when unset."
    ),
    (
        "utility_api.GetCurrentColorwayIndex",
        "()",
        "Read the active colorway index."
    ),
    (
        "utility_api.GetMajorVersion",
        "()",
        "Get the Rust engine major version."
    ),
    (
        "utility_api.GetMinorVersion",
        "()",
        "Get the Rust engine minor version."
    ),
    (
        "utility_api.GetPatchVersion",
        "()",
        "Get the Rust engine patch version."
    ),
    (
        "utility_api.GetProjectFilePath",
        "()",
        "Get the current scene path."
    ),
    (
        "utility_api.GetProjectName",
        "()",
        "Get the current scene name."
    ),
    (
        "utility_api.NewProject",
        "()",
        "Start a scene with no garments and avatars."
    ),
    (
        "utility_api.ResetClothArrangement",
        "()",
        "Restore the pre-simulation garment arrangement."
    ),
    (
        "utility_api.SetCamViewPoint",
        "(view)",
        "Set the workbench camera direction."
    ),
    (
        "utility_api.SetSimulationQuality",
        "(quality, substeps=0)",
        "Set the subsequent simulation quality."
    ),
    (
        "utility_api.SetZoomView",
        "(zoom=None)",
        "Set the workbench zoom."
    ),
    (
        "utility_api.Simulate",
        "(steps)",
        "Run the Rust cloth simulation."
    ),
    (
        "rest_api.CallRESTGet",
        "(url, headers, option='')",
        "Get a REST response with headers and body."
    ),
    (
        "rest_api.CallRESTPost",
        "(url, body, headers, option='')",
        "Post a REST body and return the response headers and body."
    ),
];

const TYPES: &[(&str, &[(&str, &str)])] = &[
    (
        "ApiTypes.CloApiAvatarMeasurement",
        &[
            ("basicLength", "0.0"),
            ("measurementName", "\"\""),
            ("measurementType", "\"\""),
            ("surfaceLength", "0.0"),
            ("unit", "\"\""),
            ("value", "0.0"),
        ],
    ),
    ("ApiTypes.ExportDxfOption", &[("m_bMetric", "true")]),
    (
        "ApiTypes.ImportDxfOption",
        &[
            ("m_bAppend", "false"),
            ("m_bAutoScale", "true"),
            ("m_bAutoDistribute", "true"),
        ],
    ),
    (
        "ApiTypes.ImportExportOption",
        &[
            ("bAdd", "false"),
            ("bExportAvatar", "false"),
            ("bExportGarment", "true"),
            ("bSaveColorWays", "false"),
            ("bSingleObject", "true"),
            ("scale", "1.0"),
        ],
    ),
    (
        "ApiTypes.ImportZPRJOption",
        &[
            ("bAppend", "false"),
            ("bLoadAvatar", "true"),
            ("bLoadGarment", "true"),
        ],
    ),
];

fn native_pattern_doc(name: &str) -> Option<&'static str> {
    match name {
        "pattern_api.GetPatternCount" => Some("GetPatternCount() -> int"),
        "pattern_api.GetPatternIndex" => Some("GetPatternIndex(arg0: str) -> int"),
        "pattern_api.GetPatternIndexW" => Some("GetPatternIndexW(arg0: str) -> int"),
        "pattern_api.GetPatternPieceName" => Some("GetPatternPieceName(arg0: int) -> str"),
        "pattern_api.MovePatternPoint" => Some("MovePatternPoint(arg0: int, arg1: int, arg2: float, arg3: float) -> None"),
        _ => None,
    }
}

pub(super) fn catalog(args: &Value) -> ToolResult {
    let module = args["module"].as_str().unwrap_or_default();
    let query = args["query"].as_str().unwrap_or_default().to_lowercase();
    let offset = if args["offset"].is_null() {
        0
    } else {
        args["offset"]
            .as_u64()
            .ok_or("offset must be nonnegative")?
    };
    let limit = if args["limit"].is_null() {
        100
    } else {
        args["limit"].as_u64().ok_or("limit must be 1..500")?
    };
    if !module.is_empty() && !MODULES.contains(&module) {
        return Err(format!("Unknown module: {module}"));
    }
    if !(1..=500).contains(&limit) {
        return Err("limit must be 1..500".into());
    }
    let mut entries = Vec::new();
    let mut add = |name: &str, kind: &str, signature: Option<&str>, summary: &str| {
        if !module.is_empty() && name.split('.').next() != Some(module) {
            return;
        }
        let haystack = format!("{name} {summary}").to_lowercase();
        if query.split_whitespace().all(|word| haystack.contains(word)) {
            entries.push(json!({"name":name,"kind":kind,"signature":signature,"summary":summary}));
        }
    };
    for entry in FUNCTIONS {
        if let Some(doc) = native_pattern_doc(entry.name) {
            add(entry.name, "function", None, doc);
        } else {
            add(entry.name, "function", Some(entry.signature), entry.doc);
        }
    }
    for (name, _) in TYPES {
        add(name, "type", Some("()"), "Structured API value.");
    }
    for family in enums::FAMILIES {
        add(
            &format!("ApiTypes.{}", family.name),
            "type",
            None,
            "Members:",
        );
        for (member, _, global) in family.members {
            if *global {
                add(&format!("ApiTypes.{member}"), "constant", None, "Members:");
            }
        }
    }
    entries.sort_by_key(|entry| {
        let name = entry["name"].as_str().unwrap_or_default();
        (
            MODULES
                .iter()
                .position(|module| name.split('.').next() == Some(module)),
            name.to_owned(),
        )
    });
    let total = entries.len() as u64;
    let next = offset.saturating_add(limit);
    Ok(json!({
        "total":total, "entries":entries.into_iter().skip(offset.min(total) as usize).take(limit as usize).collect::<Vec<_>>(),
        "next_offset":(next < total).then_some(next), "unavailable_modules":{}
    }))
}

pub(super) fn describe(name: &str) -> ToolResult {
    if let Some(entry) = FUNCTIONS.iter().find(|entry| entry.name == name) {
        let doc = native_pattern_doc(name).map_or_else(
            || format!("{name}{}\n{}", entry.signature, entry.doc),
            str::to_owned,
        );
        return Ok(json!({"name":name,"doc":doc,"members":[]}));
    }
    if let Some((_, fields)) = TYPES.iter().find(|(qualified, _)| *qualified == name) {
        let members: Vec<_> = fields
            .iter()
            .map(|(field, default)| json!({"name":field,"doc":format!("Default: {default}")}))
            .collect();
        return Ok(json!({"name":name,"doc":"Structured API value.","members":members}));
    }
    if let Some(family) = enums::family(name) {
        let members = family
            .members
            .iter()
            .map(|(name, value, _)| json!({"name":name,"doc":format!("Value: {value}")}))
            .collect::<Vec<_>>();
        return Ok(json!({"name":name,"doc":"Members:","members":members}));
    }
    if let Some((family, member, value)) = enums::member(name) {
        return Ok(json!({"name":name,"doc":format!("{family}.{member}: {value}"),"members":[]}));
    }
    Err(format!(
        "Unknown Rust API name: {name}. Use clo_api_catalog for implemented entries."
    ))
}

pub(super) fn doc(name: &str) -> ToolResult {
    if name.contains('.') {
        return describe(name);
    }
    let available = catalog(&json!({"query":name,"limit":500}))?;
    let matches: Vec<_> = array(&available["entries"])
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .filter(|qualified| qualified.rsplit('.').next() == Some(name))
        .collect();
    match matches.as_slice() {
        [qualified] => describe(qualified),
        _ => Ok(json!({"matches":matches,"help":"Use a qualified name from clo_api_catalog."})),
    }
}

pub(super) fn defaults(name: &str) -> ToolResult {
    typed(name, &Value::Null)
}

fn typed(name: &str, fields: &Value) -> ToolResult {
    let (_, defaults) = TYPES
        .iter()
        .find(|(qualified, _)| *qualified == name)
        .ok_or_else(|| format!("Unknown Rust API type: {name}"))?;
    let mut instance = Map::new();
    for (field, default) in *defaults {
        let default: Value = serde_json::from_str(default).map_err(|error| error.to_string())?;
        let value = fields.get(*field).unwrap_or(&default);
        if !((default.is_number() && value.is_number())
            || (default.is_boolean() && value.is_boolean())
            || (default.is_string() && value.is_string()))
        {
            return Err(format!("Invalid value for field {name}.{field}"));
        }
        instance.insert((*field).into(), value.clone());
    }
    if let Some(values) = fields.as_object() {
        if let Some(field) = values.keys().find(|field| !instance.contains_key(*field)) {
            return Err(format!("Unknown or non-data field: {field}"));
        }
    } else if !fields.is_null() {
        return Err("Typed object fields must be an object".into());
    }
    Ok(Value::Object(instance))
}

fn decode(value: &Value, state: &Value) -> ToolResult {
    if let Some(items) = value.as_array() {
        return items
            .iter()
            .map(|item| decode(item, state))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array);
    }
    let Some(map) = value.as_object() else {
        return Ok(value.clone());
    };
    if map.contains_key("$handle") {
        if map.len() != 1 {
            return Err("Handle must be the only key".into());
        }
        let id = text(value, "$handle")?;
        return state["runtime"]["handles"]
            .get(id)
            .and_then(|handle| handle.get("fields"))
            .cloned()
            .ok_or_else(|| format!("Unknown or released API handle: {id}"));
    }
    if map.contains_key("$enum") {
        if map.len() != 1 {
            return Err("Enum must be the only key".into());
        }
        let selected = text(value, "$enum")?;
        return enums::member(selected)
            .map(|(_, _, value)| json!(value))
            .ok_or_else(|| format!("Unknown Rust API enum member: {selected}"));
    }
    if map.contains_key("$type") {
        let name = text(value, "$type")?;
        if map
            .keys()
            .any(|key| !["$type", "args", "fields"].contains(&key.as_str()))
        {
            return Err("Unknown typed object keys".into());
        }
        if let Some(family) = enums::family(name) {
            if value["fields"]
                .as_object()
                .is_some_and(|fields| !fields.is_empty())
            {
                return Err(format!("{name} has no writable fields"));
            }
            let arguments = decode(&value["args"], state)?;
            let (_, result) = enum_value(family.name, family.members, &arguments)?;
            return Ok(json!(result));
        }
        if value["args"]
            .as_array()
            .is_some_and(|args| !args.is_empty())
            || (!value["args"].is_null() && !value["args"].is_array())
        {
            return Err(format!("{name} accepts no positional arguments"));
        }
        return typed(name, &decode(&value["fields"], state)?);
    }
    map.iter()
        .map(|(key, item)| Ok((key.clone(), decode(item, state)?)))
        .collect::<Result<Map<_, _>, _>>()
        .map(Value::Object)
}

fn handle(kind: &str, repr: String, fields: Value, state: &mut Value) -> ToolResult {
    if state["runtime"]["handles"].as_object().map_or(0, Map::len) >= 1_000 {
        return Err("Handle storage full. Release handles before another call.".into());
    }
    let sequence = state["runtime"]["handle_sequence"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(1);
    state["runtime"]["handle_sequence"] = json!(sequence);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.subsec_nanos());
    let id = format!("{:08x}{timestamp:08x}{sequence:016x}", std::process::id());
    state["runtime"]["handles"][&id] = json!({"type":kind,"fields":fields});
    Ok(json!({"$handle":id,"type":kind,"repr":repr}))
}

pub(super) fn release(args: &Value, state: &mut Value) -> ToolResult {
    let handles = args["handles"]
        .as_array()
        .ok_or("handles must be an array")?;
    if handles.iter().any(|id| !id.is_string()) {
        return Err("Each handle must be a string".into());
    }
    let mut released = 0;
    if let Some(current) = state["runtime"]["handles"].as_object_mut() {
        for handle in handles {
            if current
                .remove(handle.as_str().unwrap_or_default())
                .is_some()
            {
                released += 1;
            }
        }
    }
    Ok(json!({"released":released}))
}

pub(super) fn call(args: &Value, state: &mut Value, domain: DomainCall) -> ToolResult {
    let name = text(args, "name")?;
    let mut value = invoke(name, &args["args"], &args["kwargs"], state, domain)?;
    if name == "utility_api.GetAvatarMeasurements" {
        let rows = value.as_array().ok_or("Invalid avatar measurement rows")?;
        let current = state["runtime"]["handles"].as_object().map_or(0, Map::len);
        if current.saturating_add(rows.len()) > 1_000 {
            return Err("Handle storage full. Release handles before another call.".into());
        }
        value = Value::Array(
            rows.iter()
                .map(|row| {
                    handle(
                        "CloApiAvatarMeasurement",
                        format!(
                            "<CloApiAvatarMeasurement {}>",
                            row["measurementName"].as_str().unwrap_or_default()
                        ),
                        row.clone(),
                        state,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    Ok(json!({"name":name,"value":value}))
}

fn bind(entry: &Entry, args: &Value, kwargs: &Value) -> Result<Vec<Value>, String> {
    let args = if args.is_null() {
        &[]
    } else {
        args.as_array().ok_or("args must be a list")?.as_slice()
    };
    let kwargs = if kwargs.is_null() {
        None
    } else {
        Some(kwargs.as_object().ok_or("kwargs must be an object")?)
    };
    let params: Vec<_> = entry
        .signature
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    if args.len() > params.len() {
        return Err(format!(
            "{} expects at most {} arguments",
            entry.name,
            params.len()
        ));
    }
    let mut resolved = Vec::with_capacity(params.len());
    for (i, parameter) in params.iter().enumerate() {
        let (name, default) = parameter
            .split_once('=')
            .map_or((*parameter, None), |(name, default)| (name, Some(default)));
        let positional = args.get(i);
        let keyword = kwargs.and_then(|kwargs| kwargs.get(name));
        let value = match (positional, keyword, default) {
            (Some(_), Some(_), _) => return Err(format!("Multiple values for {name}")),
            (Some(value), None, _) | (None, Some(value), _) => value.clone(),
            (None, None, Some(default)) => match default {
                "True" => json!(true),
                "False" => json!(false),
                "None" => Value::Null,
                "''" => json!(""),
                value => serde_json::from_str(value).map_err(|error| error.to_string())?,
            },
            (None, None, None) => return Err(format!("Missing API argument {name}")),
        };
        resolved.push(value);
    }
    if let Some(kwargs) = kwargs {
        if let Some(key) = kwargs.keys().find(|key| {
            !params
                .iter()
                .any(|param| param.split('=').next() == Some(key.as_str()))
        }) {
            return Err(format!("Unexpected API argument {key}"));
        }
    }
    Ok(resolved)
}

fn current_fabric(state: &Value) -> u64 {
    state["current_fabric"].as_u64().unwrap_or(0)
}

fn index(value: &Value, length: usize, kind: &str) -> Result<usize, String> {
    let index = value
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .filter(|index| *index < length);
    index.ok_or_else(|| format!("Invalid {kind} index: {value}"))
}

fn project_path(result: &Value) -> Value {
    result
        .get("result")
        .or_else(|| result.get("path"))
        .cloned()
        .unwrap_or(Value::Null)
}

fn success(result: &Value) -> Value {
    result.get("ok").cloned().unwrap_or(json!(true))
}

fn avatar_names(state: &Value) -> Vec<Value> {
    std::iter::once(&state["avatar"])
        .chain(array(&state["additional_avatars"]))
        .filter_map(|avatar| avatar.get("name").cloned())
        .collect()
}

fn texture_kind(name: &str) -> Option<&str> {
    match name {
        "fabric_api.SetBaseTextureMapImageGivenFilePath" => Some("base"),
        "fabric_api.SetDisplacementMapImageGivenFilePath" => Some("displacement"),
        "fabric_api.SetMetalnessMapImageGivenFilePath" => Some("metalness"),
        "fabric_api.SetNormalMapImageGivenFilePath" => Some("normal"),
        "fabric_api.SetOpacityMapImageGivenFilePath" => Some("opacity"),
        "fabric_api.SetRoughnessMapImageGivenFilePath" => Some("roughness"),
        _ => None,
    }
}

fn color_hex(rgb: &[Value]) -> Result<String, String> {
    let mut hex = String::from("#");
    for channel in rgb {
        let channel = channel
            .as_f64()
            .filter(|value| (0.0..=1.0).contains(value))
            .ok_or("Color channels must be numbers between 0 and 1")?;
        hex.push_str(&format!("{:02x}", (channel * 255.0).round() as u8));
    }
    Ok(hex)
}

fn color_channels(color: &Value) -> ToolResult {
    let hex = color
        .as_str()
        .unwrap_or("#ffffff")
        .strip_prefix('#')
        .ok_or("Invalid fabric color")?;
    if hex.len() != 6 || !hex.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err("Invalid fabric color".into());
    }
    let red = u8::from_str_radix(&hex[0..2], 16).map_err(|error| error.to_string())?;
    let green = u8::from_str_radix(&hex[2..4], 16).map_err(|error| error.to_string())?;
    let blue = u8::from_str_radix(&hex[4..6], 16).map_err(|error| error.to_string())?;
    Ok(json!([
        f64::from(red) / 255.0,
        f64::from(green) / 255.0,
        f64::from(blue) / 255.0,
        1.0
    ]))
}

fn property(value: &Value, name: &str) -> Value {
    value.get(name).cloned().unwrap_or(Value::Null)
}

fn pattern_ref(value: &Value) -> Result<String, String> {
    value
        .as_u64()
        .map(|index| index.to_string())
        .ok_or_else(|| format!("Invalid pattern index: {value}"))
}

fn find_name(records: &[Value], name: &Value) -> Option<usize> {
    let name = name.as_str()?;
    records
        .iter()
        .position(|record| record["name"].as_str() == Some(name))
}

fn pattern_reference(state: &Value, value: &Value) -> Result<String, String> {
    if value.is_number() {
        return pattern_ref(value);
    }
    find_name(array(&state["patterns"]), value)
        .map(|index| index.to_string())
        .ok_or_else(|| format!("Unknown pattern name: {value}"))
}

fn enum_value<'a>(
    family: &str,
    members: &'a [(&str, i32, bool)],
    args: &Value,
) -> Result<(&'a str, i32), String> {
    let values = args
        .as_array()
        .filter(|args| args.len() == 1)
        .ok_or_else(|| format!("{family} expects one integer"))?;
    let requested = values[0]
        .as_i64()
        .ok_or_else(|| format!("{family} expects one integer"))?;
    members
        .iter()
        .find(|(_, value, _)| i64::from(*value) == requested)
        .map(|(member, value, _)| (*member, *value))
        .ok_or_else(|| format!("Unknown {family} value: {requested}"))
}

pub(super) fn invoke(
    name: &str,
    args: &Value,
    kwargs: &Value,
    state: &mut Value,
    domain: DomainCall,
) -> ToolResult {
    let args = decode(args, state)?;
    let kwargs = decode(kwargs, state)?;
    if let Some((family, member, value)) = enums::member(name) {
        return handle(
            family,
            format!("<{family}.{member}: {value}>"),
            json!(value),
            state,
        );
    }
    if let Some(family) = enums::family(name) {
        let (member, value) = enum_value(family.name, family.members, &args)?;
        return handle(
            family.name,
            format!("<{}.{member}: {value}>", family.name),
            json!(value),
            state,
        );
    }
    if TYPES.iter().any(|(qualified, _)| *qualified == name) {
        if !array(&args).is_empty()
            || (!args.is_null() && !args.is_array())
            || kwargs.as_object().is_some_and(|item| !item.is_empty())
            || (!kwargs.is_null() && !kwargs.is_object())
        {
            return Err(format!("{name} accepts no arguments"));
        }
        let fields = typed(name, &Value::Null)?;
        let kind = name.rsplit('.').next().unwrap_or(name);
        return handle(kind, format!("<{name}>"), fields, state);
    }
    let entry = FUNCTIONS
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| {
            format!("Unknown Rust API name: {name}. Use clo_api_catalog for implemented entries.")
        })?;
    let arg = bind(entry, &args, &kwargs)?;
    if name.starts_with("rest_api.") {
        return super::http::request(name, &arg);
    }
    if let Some(kind) = texture_kind(name) {
        return domain(
            "fabric_set_texture",
            &json!({"fabric_index":current_fabric(state),"image_path":arg[0],"map":kind}),
            state,
        )
        .map(|result| success(&result));
    }

    let pattern_info = |state: &mut Value, field: &str| {
        domain(
            "pattern_info",
            &json!({"pattern":pattern_reference(state,&arg[0])?}),
            state,
        )
        .map(|result| property(&result, field))
    };
    let pattern_op = |state: &mut Value, op: &str, values: Value| {
        domain(
            "pattern_ops",
            &json!({"pattern":pattern_ref(&arg[0])?,"op":op,"args":values}),
            state,
        )
    };
    let patterns = array(&state["patterns"]);
    let fabrics = array(&state["fabrics"]);
    let outcome = match name {
        "import_api.ImportAvatar" => domain("avatar_load", &json!({"name_or_path":arg[0],"replace":!arg[1]["bAdd"].as_bool().unwrap_or(false)}),state).map(|result| success(&result)),
        "import_api.ImportDXF" | "import_api.ImportZpac" | "import_api.ImportZprj" => {
            let option = &arg[1];
            let append = ["bAdd","bAppend","m_bAppend"].iter().any(|key| option[*key] == json!(true));
            if option["bLoadGarment"] == json!(false) || option["bLoadAvatar"] == json!(false) {
                return Err("Selective native project loading is not implemented by the Rust backend".into());
            }
            domain("scene_open", &json!({"path":arg[0],"mode":if append {"append"} else {"replace"}}),state).map(|result| success(&result))
        }
        "import_api.ImportFile" | "pattern_api.ImportPatternJSON" => domain("scene_open", &json!({"path":arg[0],"mode":"append"}),state).map(|result| success(&result)),
        "export_api.ExportDXF" | "export_api.ExportFBX" | "export_api.ExportGLB" | "export_api.ExportGLTF" | "export_api.ExportOBJ" | "pattern_api.ExportPatternJSON" => {
            let option = arg.get(1).unwrap_or(&Value::Null);
            if name == "export_api.ExportFBX" {
                let path = arg[0].as_str().ok_or("expected an FBX path")?;
                if !std::path::Path::new(path).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("fbx")) {
                    return Err("FBX export requires a .fbx path".into());
                }
            }
            let result = domain("scene_export", &json!({"path":arg[0],"include_avatar":option["bExportAvatar"].as_bool().unwrap_or(false),"colorways":option["bSaveColorWays"].as_bool().unwrap_or(false)}),state)?;
            let path = project_path(&result);
            match name {
                "pattern_api.ExportPatternJSON" => Ok(json!(!path.is_null())),
                "export_api.ExportFBX" | "export_api.ExportGLB" | "export_api.ExportGLTF" | "export_api.ExportOBJ" if !path.is_array() => Ok(json!([path])),
                _ => Ok(path),
            }
        }
        "export_api.ExportSnapshot2D" | "export_api.ExportThumbnail3D" => {
            let flat = name == "export_api.ExportSnapshot2D";
            if flat && arg[1].as_i64() != Some(0) {
                return Err("Rust 2D snapshot export supports mode 0".into());
            }
            let path = arg[0].as_str().ok_or("expected a PNG path")?;
            crate::compat_fit::export_snapshot(std::path::Path::new(path), state, flat)?;
            Ok(if flat { json!([path]) } else { json!(path) })
        }
        "export_api.ExportZPac" | "export_api.ExportZPrj" => {
            let result = domain("scene_save", &json!({"path":arg[0],"thumbnail":arg.get(1).and_then(Value::as_bool).unwrap_or(false)}),state)?;
            Ok(project_path(&result))
        }
        "export_api.GetAvatarCount" => Ok(json!(avatar_names(state).len())),
        "export_api.GetAvatarNameList" => Ok(json!(avatar_names(state))),
        "export_api.GetColorwayNameList" => Ok(state.get("colorways").cloned().unwrap_or_else(|| json!(["Default Colorway"]))),
        "fabric_api.AddFabric" => {
            domain("__clo_fabric_add", &json!({"fabric":arg[0]}), state)
        }
        "fabric_api.GetCurrentFabricIndex" => Ok(json!(current_fabric(state))),
        "fabric_api.GetFabricCount" => Ok(json!(fabrics.len())),
        "fabric_api.GetFabricInformation" | "fabric_api.GetFabricName" | "fabric_api.GetFabricPBRMaterialBaseColor" => {
            let fabric = &fabrics[index(&arg[0], fabrics.len(), "fabric")?];
            match name {
                "fabric_api.GetFabricInformation" => Ok(property(fabric,"info")),
                "fabric_api.GetFabricName" => Ok(property(fabric,"name")),
                _ => color_channels(&fabric["color"]),
            }
        }
        "fabric_api.SetCurrentFabricIndex" => {
            let index = index(&arg[0],fabrics.len(),"fabric")?;
            state["current_fabric"] = json!(index);
            Ok(json!(true))
        }
        "fabric_api.SetFabricName" => {
            let index = index(&arg[0],fabrics.len(),"fabric")?;
            let name = arg[1].as_str().ok_or("Fabric name must be a string")?;
            state["fabrics"][index]["name"] = json!(name);
            Ok(json!(true))
        }
        "fabric_api.AssignFabricToPattern" => {
            domain("__clo_fabric_assign", &json!({"fabric_index":arg[0],"pattern":pattern_ref(&arg[1])?}), state)
        }
        "fabric_api.SetFabricPBRMaterialBaseColor" => {
            if arg[5].as_f64() != Some(1.0) {
                return Err("Rust material color currently supports alpha=1.0".into());
            }
            let values = json!({"fabric_index":arg[0],"color":color_hex(&arg[2..5])?});
            domain("fabric_set_color", &values,state).map(|result| success(&result))
        }
        "pattern_api.AddSeamlinePairGroup" => domain("sew", &json!({"pattern_a":pattern_ref(&arg[0])?,"line_a":arg[1],"pattern_b":pattern_ref(&arg[2])?,"line_b":arg[3],"dir_a":arg[4],"dir_b":arg[5]}),state).map(|result| success(&result)),
        "pattern_api.CreatePatternWithPoints" => domain("pattern_create", &json!({"points":arg[0]}),state).map(|result| property(&result,"index")),
        "pattern_api.DeletePatternPiece" => domain("pattern_delete", &json!({"patterns":[pattern_ref(&arg[0])?]}),state).map(|_| json!(true)),
        "pattern_api.GetPatternCount" => Ok(json!(patterns.len())),
        "pattern_api.GetPatternIndex" | "pattern_api.GetPatternIndexW" => {
            if !arg[0].is_string() {return Err("Pattern name must be a string".into());}
            Ok(json!(find_name(patterns,&arg[0]).map(|index| index as i64).unwrap_or(-1)))
        }
        "pattern_api.GetSelectedPattern" => {
            let current = super::ui::selected_indices(state).iter().map(|index| property(&patterns[*index],"name")).collect::<Vec<_>>();
            Ok(json!(current))
        }
        "pattern_api.GetSelectedPatternViaIndex" => {
            let index = arg[0].as_u64().ok_or("Invalid pattern index")?;
            Ok(json!(super::ui::selected_indices(state).contains(&(index as usize))))
        }
        "pattern_api.SelectPatternViaIndex" | "pattern_api.SelectPatternViaName" => {
            let reference = pattern_reference(state,&arg[0])?;
            let index = reference.parse::<usize>().map_err(|error| error.to_string())?;
            let on = arg[1].as_bool().ok_or("Selection flag must be boolean")?;
            super::ui::modify_selection(state,index,on)?;
            Ok(Value::Null)
        }
        "pattern_api.SetPatternPieceFabricIndex" => {
            let pattern = index(&arg[0],patterns.len(),"pattern")?;
            let fabric = index(&arg[1],fabrics.len(),"fabric")?;
            state["patterns"][pattern]["fabric_index"] = json!(fabric);
            state["simulation"] = Value::Null;
            Ok(Value::Null)
        }
        "pattern_api.GetLineLength" => {
            if !arg[2].is_null() { return Err("Internal line length is unavailable without an internal-edge model".into()); }
            let data = pattern_info(state,"line_info")?;
            let line = arg[1].as_u64().ok_or("Invalid line index")?;
            array(&data["lines"]).iter().find(|record| record["lineIndex"]==json!(line)).and_then(|record| record.get("length")).cloned()
                .ok_or_else(|| format!("Unknown outline line {line}"))
        }
        "pattern_api.GetPatternIndexFrom2DView" => {
            let x = arg[0].as_f64().ok_or("x must be a number")?;
            let y = arg[1].as_f64().ok_or("y must be a number")?;
            Ok(json!(super::ui::pattern_at(state,x,y,domain)?.map(|index| index as i64).unwrap_or(-1)))
        }
        "pattern_api.GetPatternPieceArea" => {
            let outline = domain("__clo_pattern_outline",&json!({"pattern":pattern_ref(&arg[0])?}),state)?;
            let vertices = array(&outline["points"]);
            let mut twice_area = 0.0;
            for (start,end) in vertices.iter().zip(vertices.iter().cycle().skip(1)) {
                let x1 = start[0].as_f64().ok_or("Invalid pattern outline x")?;
                let y1 = start[1].as_f64().ok_or("Invalid pattern outline y")?;
                let x2 = end[0].as_f64().ok_or("Invalid pattern outline x")?;
                let y2 = end[1].as_f64().ok_or("Invalid pattern outline y")?;
                twice_area += x1*y2 - x2*y1;
            }
            Ok(json!(twice_area.abs() / 2.0))
        }
        "pattern_api.GetPatternPieceName" => pattern_info(state,"name"),
        "pattern_api.GetPatternPiecePos" => pattern_info(state,"pos_2d"),
        "pattern_api.GetPatternPieceFabricIndex" => pattern_info(state,"fabric_index"),
        "pattern_api.GetPatternLayer" => pattern_info(state,"layer"),
        "pattern_api.GetArrangementOfPattern" | "pattern_api.GetArrangementOfPatternW" => pattern_info(state,"arrangement"),
        "pattern_api.GetParticleDistanceOfPattern" | "pattern_api.GetParticleDistanceOfPatternW" => pattern_info(state,"particle_distance"),
        "pattern_api.GetSeamlinePairGroupListInPattern" => pattern_info(state,"seam_groups"),
        "pattern_api.GetBoundingBoxOfPattern" | "pattern_api.GetBoundingBoxOfPatternW" => pattern_info(state,"bbox"),
        "pattern_api.GetPatternInputInformation" | "pattern_api.GetPatternInputInformationW" | "pattern_api.GetPatternLineInfo" => {
            let field = if name.ends_with("LineInfo") {"line_info"} else {"input_info"};
            let data = if field == "input_info" && arg[0].is_null() {
                let mut all = Vec::new();
                for index in 0..patterns.len() {
                    let single = domain("pattern_info",&json!({"pattern":index.to_string()}),state)?;
                    all.extend_from_slice(array(&single["input_info"]["Pattern InputInformation"]));
                }
                json!({"Pattern InputInformation":all})
            } else {
                pattern_info(state,field)?
            };
            serde_json::to_string(&data).map(Value::String).map_err(|error| error.to_string())
        }
        "pattern_api.GetArrangementList" => {
            let result = domain("arrangement_points", &json!({}),state)?;
            Ok(json!(array(&result["names"]).iter().enumerate().map(|(index,name)| json!({"ArrangementIndex":index.to_string(),"ArrangementName":name})).collect::<Vec<_>>()))
        }
        "pattern_api.GetSeamlinePairGroupCount" => Ok(json!(seam_group_names(state).count())),
        "pattern_api.GetSeamlinePairGroupIndexFromName" | "pattern_api.GetSeamlinePairGroupIndexFromNameW" => {
            if !arg[0].is_string() {return Err("Seam group name must be a string".into());}
            let index = seam_group_names(state).position(|name| name == &arg[0]);
            Ok(index.map_or(json!(-1), |index| json!(index)))
        }
        "pattern_api.GetSeamlinePairGroupName" | "pattern_api.GetSeamlinePairGroupNameW" => {
            arg[0].as_u64().and_then(|index| usize::try_from(index).ok())
                .and_then(|index| seam_group_names(state).nth(index))
                .cloned().ok_or_else(|| format!("Invalid seam group index: {}", arg[0]))
        }
        "pattern_api.MovePatternPoint" => domain("pattern_edit_points", &json!({"pattern":pattern_ref(&arg[0])?,"moves":[{"point":arg[1],"x":arg[2],"y":arg[3]}]}),state).map(|_| Value::Null),
        "pattern_api.SetArrangement" => {
            let result = domain("arrangement_points", &json!({}),state)?;
            let names = array(&result["names"]);
            let point = if arg[1].is_string() {arg[1].clone()} else {names[index(&arg[1],names.len(),"arrangement point")?].clone()};
            domain("pattern_set_arrangement", &json!({"pattern":pattern_ref(&arg[0])?,"point":point}),state).map(|_| json!(true))
        }
        "pattern_api.CopyPatternPieceMove" => pattern_op(state,"copy",json!({"dx":arg[1],"dy":arg[2]})).map(|result| property(&result,"new_index")),
        "pattern_api.DeleteLine" => pattern_op(state,"delete_line",json!({"line":arg[1]})).map(|result| success(&result)),
        "pattern_api.DeletePoint" => pattern_op(state,"delete_point",json!({"point":arg[1]})).map(|result| success(&result)),
        "pattern_api.FlipPatternPiece" => pattern_op(state,"flip",json!({"horizontal":arg[1],"each":arg[2]})).map(|result| success(&result)),
        "pattern_api.SetParticleDistanceOfPattern" => pattern_op(state,"particle_distance",json!({"mm":arg[1]})).map(|result| success(&result)),
        "pattern_api.SetParticleDistanceOfPatterns" => {
            let distance = arg[0]
                .as_f64()
                .filter(|distance| (3.0..=500.0).contains(distance))
                .ok_or("particle distance must be 3..500 mm")?;
            if !array(&state["simulation"]["meshes"]).is_empty() {
                domain("simulate", &json!({"steps":0,"particle_distance":distance}), state)?;
            } else {
                for index in 0..patterns.len() {
                    domain("pattern_ops", &json!({"pattern":index.to_string(),"op":"particle_distance","args":{"mm":distance}}),state)?;
                }
            }
            Ok(json!(true))
        }
        "pattern_api.SetPatternFreeze" => pattern_op(state,"freeze",json!({"on":arg[1]})).map(|result| success(&result)),
        "pattern_api.SetPatternHide3D" => pattern_op(state,"hide",json!({"on":arg[1]})).map(|result| success(&result)),
        "pattern_api.SetPatternLayer" => pattern_op(state,"layer",json!({"layer":arg[1]})).map(|result| success(&result)),
        "pattern_api.SetPatternPieceElastic" => pattern_op(state,"elastic",json!({"line":arg[1],"on":arg[2]})).map(|result| success(&result)),
        "pattern_api.SetPatternPieceElasticStrength" | "pattern_api.SetPatternPieceElasticTotalLength" => {
            let key = if name.ends_with("Strength") { "strength" } else { "total_length" };
            domain("__clo_set_elastic_values", &json!({"pattern":pattern_ref(&arg[0])?,"line":arg[1],(key):arg[2]}), state)
        }
        "pattern_api.SetPatternPieceMove" => pattern_op(state,"move_2d",json!({"dx":arg[1],"dy":arg[2]})).map(|result| success(&result)),
        "pattern_api.SetPatternPieceName" => pattern_op(state,"rename",json!({"name":arg[1]})).map(|result| success(&result)),
        "pattern_api.SetPatternPiecePos" => pattern_op(state,"move_2d",json!({"x":arg[1],"y":arg[2]})).map(|result| success(&result)),
        "pattern_api.SetPatternPieceShirring" => pattern_op(state,"shirring",json!({"line":arg[1],"on":arg[2]})).map(|result| success(&result)),
        "pattern_api.SetPatternStrengthen" => pattern_op(state,"strengthen",json!({"on":arg[1]})).map(|result| success(&result)),
        "pattern_api.SymmetryPatternPiece" => pattern_op(state,"symmetry",json!({"with_sewing":arg[1]})).map(|result| success(&result)),
        "pattern_api.UnfoldPatternPiece" => pattern_op(state,"unfold",json!({"line":arg[1],"half_symmetry":arg[2]})).map(|result| success(&result)),
        "utility_api.DeleteAvatar" => {
            let count = avatar_names(state).len();
            let indices = arg[0].as_array().ok_or("Avatar indices must be a list")?;
            let mut indices = indices.iter().map(|value| index(value,count,"avatar")).collect::<Result<Vec<_>,_>>()?;
            indices.sort_unstable();
            indices.dedup();
            for index in indices.into_iter().rev() {
                if index == 0 {
                    let replacement = state["additional_avatars"].as_array_mut().filter(|avatars| !avatars.is_empty()).map(|avatars| avatars.remove(0)).unwrap_or(Value::Null);
                    state["avatar"] = replacement;
                } else if let Some(avatars) = state["additional_avatars"].as_array_mut() {
                    avatars.remove(index - 1);
                }
            }
            Ok(json!(true))
        }
        "utility_api.GetAvatarMeasurements" => {
            let index = index(&arg[0],avatar_names(state).len(),"avatar")?;
            let body = if index == 0 {
                property(&domain("avatar_measurements", &json!({}),state)?,"body")
            } else {
                property(&state["additional_avatars"][index-1],"body")
            };
            Ok(super::measurements::rows(&body))
        }
        "utility_api.GetCurrentColorwayIndex" => Ok(json!(state["current_colorway"].as_u64().unwrap_or(0))),
        "utility_api.GetCLOTemporaryFolderPath" => Ok(json!(std::env::temp_dir())),
        "utility_api.GetCLOAssetFolderPath" => {
            let linux = arg[0].as_bool().ok_or("path delimiter option must be a boolean")?;
            let info = domain("scene_info", &json!({}), state)?;
            let path = info["asset_folder"].as_str().unwrap_or_default();
            Ok(json!(if linux { path.replace('\\', "/") } else { path.to_owned() }))
        }
        "utility_api.GetMajorVersion" | "utility_api.GetMinorVersion" | "utility_api.GetPatchVersion" => {
            let index = match name { "utility_api.GetMajorVersion" => 0, "utility_api.GetMinorVersion" => 1, _ => 2 };
            let part = env!("CARGO_PKG_VERSION").split('.').nth(index).and_then(|value| value.parse::<u32>().ok()).unwrap_or(0);
            Ok(json!(part))
        }
        "utility_api.GetProjectFilePath" => domain("scene_info", &json!({}),state).map(|result| property(&result,"project_path")),
        "utility_api.GetProjectName" => domain("scene_info", &json!({}),state).map(|result| property(&result,"project")),
        "utility_api.NewProject" => domain("scene_clear", &json!({"keep_avatar":false}),state).map(|_| json!(true)),
        "utility_api.ResetClothArrangement" => domain("simulation_reset", &json!({}),state).map(|result| success(&result)),
        "utility_api.SetCamViewPoint" => {
            state["runtime"]["ui"]["view"] = arg[0].clone();
            Ok(json!(true))
        }
        "utility_api.SetSimulationQuality" => {
            let quality = arg[0].as_u64().filter(|value| *value <= 3).ok_or("Quality must be 0..3")?;
            state["runtime"]["quality"] = json!(quality);
            Ok(json!(true))
        }
        "utility_api.SetZoomView" => {
            let zoom = if arg[0].is_null() {1.0} else {arg[0].as_f64().filter(|value| *value > 0.0).ok_or("Zoom must be positive")?};
            state["runtime"]["ui"]["zoom"] = json!(zoom.clamp(0.2,6.0));
            Ok(json!(true))
        }
        "utility_api.Simulate" => domain("simulate", &json!({"steps":arg[0]}),state).map(|result| success(&result)),
        _ => Err(format!("Runtime API entry has no implementation: {name}")),
    }?;
    if returns_none(name) {
        Ok(Value::Null)
    } else {
        Ok(outcome)
    }
}

fn returns_none(name: &str) -> bool {
    matches!(
        name,
        "pattern_api.DeleteLine"
            | "pattern_api.DeletePatternPiece"
            | "pattern_api.DeletePoint"
            | "pattern_api.FlipPatternPiece"
            | "pattern_api.MovePatternPoint"
            | "pattern_api.SetArrangement"
            | "pattern_api.SetParticleDistanceOfPattern"
            | "pattern_api.SetParticleDistanceOfPatterns"
            | "pattern_api.SetPatternFreeze"
            | "pattern_api.SetPatternHide3D"
            | "pattern_api.SetPatternLayer"
            | "pattern_api.SetPatternPieceElastic"
            | "pattern_api.SetPatternPieceElasticStrength"
            | "pattern_api.SetPatternPieceElasticTotalLength"
            | "pattern_api.SetPatternPieceMove"
            | "pattern_api.SetPatternPieceName"
            | "pattern_api.SetPatternPiecePos"
            | "pattern_api.SetPatternPieceShirring"
            | "pattern_api.SetPatternStrengthen"
            | "pattern_api.SymmetryPatternPiece"
            | "utility_api.NewProject"
            | "utility_api.ResetClothArrangement"
            | "utility_api.SetCamViewPoint"
            | "utility_api.SetSimulationQuality"
            | "utility_api.SetZoomView"
    )
}
