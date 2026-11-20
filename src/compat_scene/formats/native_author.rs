//! Write the native garment records of an editable Rust scene.

mod seams;

use super::mesh::triangulate;
use super::native_pose::{encode_map, encoded_list, BYTE_TYPE, LIST_TYPE, MAP_TYPE};
use crate::compat_fit::{archive_pose_mesh, ArchivePoseMesh};
use crate::compat_scene::{array, patterns};
use serde_json::Value;
use std::borrow::Cow;

const BOOL: u8 = 1;
const INT: u8 = 4;
const UINT: u8 = 5;
const FLOAT: u8 = 6;
const VEC2: u8 = 10;
const VEC3: u8 = 11;
const VEC4: u8 = 12;
const MAT3: u8 = 13;
const MAT4: u8 = 14;
const ARRAY: u8 = 21;
const ZERO: [u8; 4] = [0; 4];
const TRUE: &[u8] = &[1];
const FALSE: &[u8] = &[0];

fn records(entries: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    encoded_list(
        &entries
            .iter()
            .map(|entry| Cow::Borrowed(entry.as_slice()))
            .collect::<Vec<_>>(),
    )
}

fn float_bytes(values: impl IntoIterator<Item = f64>) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    for number in values {
        let value = number as f32;
        if !value.is_finite() {
            return Err("coordinate or setting exceeds native float range".into());
        }
        bytes.extend(value.to_le_bytes());
    }
    Ok(bytes)
}

fn element(name: &str, id: u32) -> Result<Vec<u8>, String> {
    encode_map(&[
        (b"qsName", BYTE_TYPE, name.as_bytes()),
        (b"qsNameUTF8", BYTE_TYPE, name.as_bytes()),
        (b"uiID", UINT, &id.to_le_bytes()),
        (b"bVisible", BOOL, TRUE),
        (b"bEditable", BOOL, TRUE),
    ])
}

fn gid(panel: u32, line: u32) -> Result<Vec<u8>, String> {
    encode_map(&[
        (b"sFilePath", 8, b""),
        (b"uiPatternGID", UINT, &panel.to_le_bytes()),
        (b"uiLineGID", UINT, &line.to_le_bytes()),
        (b"bMirrorLine", BOOL, FALSE),
        (b"bMakeCLO", BOOL, TRUE),
    ])
}

struct Panel {
    record: Vec<u8>,
    edge_vertices: Vec<Vec<u32>>,
}

fn flat_mesh(pattern: &Value) -> Result<ArchivePoseMesh, String> {
    let (rest, corners) = patterns::sampled_perimeter(pattern, 512)?;
    let perimeter: Vec<_> = rest
        .iter()
        .map(|&[x, y]| patterns::Point { x, y, kind: 0 })
        .collect();
    let triangles = triangulate(&perimeter)?;
    if triangles.len() + 2 != rest.len() {
        return Err("native outline triangulation did not preserve each boundary point".into());
    }
    let sides = corners
        .iter()
        .enumerate()
        .map(|(index, &first)| {
            let last = corners.get(index + 1).copied().unwrap_or(rest.len());
            (first..=last)
                .map(|vertex| (vertex % rest.len()) as u32)
                .collect()
        })
        .collect();
    let positions = rest.iter().map(|&[x, y]| [x, y, 200.]).collect();
    Ok(ArchivePoseMesh {
        rest,
        positions,
        triangles,
        sides,
    })
}

fn panel(
    value: &Value,
    slot: u32,
    next_id: &mut u32,
    materials: &[Vec<u8>],
    state: &Value,
    saved: Option<&Value>,
) -> Result<Panel, String> {
    let name = value["name"]
        .as_str()
        .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
        .ok_or("invalid name for native editable pattern")?;
    let points = patterns::points(value)?;
    if points.len() > 512
        || (value["sampled_polyline"] == true && points.iter().any(|p| p.kind != 0))
    {
        return Err("native authoring requires editable outline points".into());
    }
    let edges = patterns::split_edges(&points);
    let fabric_index = value["fabric_index"]
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .filter(|&index| index < materials.len())
        .ok_or("invalid native panel fabric assignment")?;
    if ["half_symmetry", "hide", "freeze", "strengthen"]
        .into_iter()
        .any(|field| value[field] == true)
        || value.get("draft_offset_x").is_some()
        || !value["line_properties"]
            .as_object()
            .is_none_or(|properties| properties.is_empty())
        || !array(value, "native_children").is_empty()
    {
        return Err("native authoring cannot preserve these panel properties".into());
    }
    let arrangement = value["arrangement"].as_str().unwrap_or_default();
    if !matches!(arrangement, "" | "Arrangement Point") {
        return Err("native authoring has no placement calibration for this arrangement".into());
    }

    let mut next = || {
        *next_id += 1;
        *next_id
    };
    let entry = element(name, next())?;
    let [dx, dy] = patterns::offset(value);
    let geometry = match saved {
        Some(prior) => archive_pose_mesh(value, prior, state)?,
        None => flat_mesh(value)?,
    };
    if geometry.sides.len() != edges.len() || geometry.rest.len() > 130_000 {
        return Err("native pose boundary does not match the panel outline".into());
    }
    let [min_x, min_y, max_x, max_y] = patterns::point_bounds(patterns::outline(value)?, [dx, dy]);
    let (cx, cy) = ((min_x + max_x) / 2., (min_y + max_y) / 2.);
    let basis = float_bytes([1., 0., 0., 0., 1., 0., cx, cy, 1.])?;
    let transform = encode_map(&[
        (b"m3Matrix", MAT3, &basis),
        (b"fGrainAngleResetValue", FLOAT, &ZERO),
    ])?;
    // Moving an editable outline in 2D does not move its flat 3D placement.
    let placement = float_bytes([
        1.,
        0.,
        0.,
        0.,
        0.,
        1.,
        0.,
        0.,
        0.,
        0.,
        1.,
        0.,
        cx - dx,
        cy - dy,
        200.,
        1.,
    ])?;
    let rest = float_bytes(geometry.rest.iter().flat_map(|&[x, y]| [x + dx, y + dy]))?;
    let tex = float_bytes(
        geometry
            .rest
            .iter()
            .flat_map(|&[x, y]| [x + dx - cx, y + dy - cy]),
    )?;
    let world = float_bytes(geometry.positions.into_iter().flatten())?;
    let triangle_indices = geometry
        .triangles
        .iter()
        .flatten()
        .flat_map(|index| index.to_le_bytes())
        .collect::<Vec<_>>();
    let mut point_records = Vec::with_capacity(edges.len());
    let mut line_records = Vec::with_capacity(edges.len());
    let mut boundaries = Vec::with_capacity(edges.len());
    for (index, (edge, vertices)) in edges.iter().zip(&geometry.sides).enumerate() {
        let following = (index + 1) % edges.len();
        let [first, .., last] = vertices.as_slice() else {
            return Err("native panel boundary path has fewer than two vertices".into());
        };
        let matches = |index: u32, point: patterns::Point| {
            geometry
                .rest
                .get(index as usize)
                .is_some_and(|&[x, y]| (x - point.x).hypot(y - point.y) < 0.01)
        };
        if !matches(*first, edge[0]) || !matches(*last, edge[edge.len() - 1]) {
            return Err("native pose boundary endpoints do not match the editable outline".into());
        }
        let coords = float_bytes([edge[0].x + dx - cx, edge[0].y + dy - cy])?;
        let coords = coords.as_slice();
        let size_specific = [1u32.to_le_bytes().as_slice(), &[VEC2], coords].concat();
        point_records.push(encode_map(&[
            (b"uiID", UINT, &next().to_le_bytes()),
            (b"bVisible", BOOL, TRUE),
            (b"bEditable", BOOL, TRUE),
            (b"bGradedPoint", BOOL, TRUE),
            (b"v2Position", VEC2, coords),
            (b"arrSizeSpecificPosition", ARRAY, &size_specific),
        ])?);
        let nodes = vertices
            .iter()
            .map(|vertex| {
                encode_map(&[
                    (b"uiMeshPointIndex", UINT, &vertex.to_le_bytes()),
                    (b"bMeshEdgeList", BOOL, TRUE),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let boundary = records(&nodes)?;
        let controls = &edge[1..edge.len() - 1];
        let kind: i32 = match controls {
            [] => 0,
            _ if controls.iter().all(|p| p.kind == 3) => 3,
            _ => 2,
        };
        let control_records = controls
            .iter()
            .map(|point| {
                let coordinates = float_bytes([point.x + dx - cx, point.y + dy - cy])?;
                let size_specific = [1u32.to_le_bytes().as_slice(), &[VEC2], &coordinates].concat();
                encode_map(&[
                    (b"bLockState", BOOL, FALSE),
                    (b"bGradingAuthoritative", BOOL, FALSE),
                    (b"bGradedPoint", BOOL, FALSE),
                    (b"iGradeRuleIdentifier", INT, &ZERO),
                    (b"v2Position", VEC2, &coordinates),
                    (b"bHandleBinded", BOOL, FALSE),
                    (b"arrPointSizeSpec", ARRAY, &size_specific),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        line_records.push(encode_map(&[
            (b"uiID", UINT, &next().to_le_bytes()),
            (b"bVisible", BOOL, TRUE),
            (b"bEditable", BOOL, TRUE),
            (b"iLineType", INT, &kind.to_le_bytes()),
            (b"mapGID", MAP_TYPE, &gid(slot, index as u32)?),
            (b"uiStartPointIndex", UINT, &(index as u32).to_le_bytes()),
            (b"uiEndPointIndex", UINT, &(following as u32).to_le_bytes()),
            (
                b"uiCtrlPointCnt",
                UINT,
                &(control_records.len() as u32).to_le_bytes(),
            ),
            (b"listPoint", LIST_TYPE, &records(&control_records)?),
            (b"listMeshPoint", LIST_TYPE, &boundary),
        ])?);
        boundaries.push(boundary);
    }
    let mut edge_lists = (edges.len() as u32).to_le_bytes().to_vec();
    edge_lists.push(LIST_TYPE);
    for edge in &boundaries {
        edge_lists.extend((edge.len() as u32).to_le_bytes());
    }
    edge_lists.extend(boundaries.into_iter().flatten());
    let mesh = encode_map(&[
        (b"iSavedCurrentSizeIndex", INT, &ZERO),
        (
            b"uiVertexCount2D",
            UINT,
            &(geometry.rest.len() as u32).to_le_bytes(),
        ),
        (b"baRest", BYTE_TYPE, &rest),
        (
            b"uiTriangleCount",
            UINT,
            &(geometry.triangles.len() as u32).to_le_bytes(),
        ),
        (b"baTex", BYTE_TYPE, &tex),
        (b"baTri", BYTE_TYPE, &triangle_indices),
        (b"uiBaseTriangleCount", UINT, &ZERO),
        (b"uiBaseVertexCount", UINT, &ZERO),
        (b"listMeshPointList", LIST_TYPE, &edge_lists),
        (b"uiQuadCountMVMeshData", UINT, &ZERO),
        (b"bNeedToRemeshing", BOOL, FALSE),
    ])?;
    let grain = encode_map(&[(b"v2Translation", VEC2, &[0; 8]), (b"fAngle", FLOAT, &ZERO)])?;
    let shape = encode_map(&[
        (b"mapElement", MAP_TYPE, &entry),
        (b"iFabricIndex", UINT, &(fabric_index as u32).to_le_bytes()),
        (b"bIsNewColorway", BOOL, TRUE),
        (b"m3LtoUV", MAT3, &basis),
        (b"listGrainLineInfo", LIST_TYPE, &records(&[grain])?),
        (b"bLinkedGrainLine", BOOL, TRUE),
        (b"mapMesh2D", MAP_TYPE, &mesh),
        (b"iVersion", INT, &ZERO),
    ])?;
    let outline = encode_map(&[
        (b"mapGID", MAP_TYPE, &gid(slot + 1, 0)?),
        (b"mapTransformer2D", MAP_TYPE, &transform),
        (b"listPoint", LIST_TYPE, &records(&point_records)?),
        (b"listLine", LIST_TYPE, &records(&line_records)?),
        (b"uiCount", UINT, &ZERO),
        (b"bHalfSymmetric", BOOL, FALSE),
    ])?;
    let layer = i32::try_from(value["layer"].as_i64().unwrap_or(0))
        .map_err(|_| "native layer exceeds signed 32-bit range")?;
    let one = 1f32.to_le_bytes();
    let dynamics = encode_map(&[
        (b"fPressure", FLOAT, &ZERO),
        (b"fLu", FLOAT, &one),
        (b"fLv", FLOAT, &one),
        (b"iLayer", INT, &layer.to_le_bytes()),
        (b"bActive", BOOL, TRUE),
        (b"bFreeze", BOOL, FALSE),
        (b"bSolidified", BOOL, FALSE),
        (
            b"fAdditionalCollisionThickness",
            FLOAT,
            &2.5f32.to_le_bytes(),
        ),
        (b"fSelfAvoidance", FLOAT, &one),
    ])?;
    let arrangement = encode_map(&[
        (b"qsName", BYTE_TYPE, b"Arrangement Point"),
        (b"qsNameUTF8", BYTE_TYPE, b"Arrangement Point"),
        (b"enType", INT, &1i32.to_le_bytes()),
        (b"uiSpiralType", UINT, &1u32.to_le_bytes()),
    ])?;
    let distance = float_bytes([value["particle_distance"].as_f64().unwrap_or(20.)])?;
    let record = encode_map(&[
        (b"mapFabricShape2D", MAP_TYPE, &shape),
        (b"mapArrange", MAP_TYPE, &arrangement),
        (b"mapElement", MAP_TYPE, &entry),
        (
            b"mapOriginPatternPhysicalProperty",
            MAP_TYPE,
            &materials[fabric_index],
        ),
        (b"uiPatternType", UINT, &ZERO),
        (b"bPatternVisible3D", BOOL, TRUE),
        (b"mapPatternDynamics", MAP_TYPE, &dynamics),
        (b"fEdgeLength", FLOAT, &distance),
        (b"bDraped", BOOL, &[u8::from(saved.is_some())]),
        (b"uiPatternGID", UINT, &slot.to_le_bytes()),
        (b"listInnerShape", LIST_TYPE, &ZERO),
        (b"mapShape2D", MAP_TYPE, &outline),
        (b"baPosition3D", BYTE_TYPE, &world),
        (b"m4OffsetMatrix", MAT4, &placement),
        (b"bVisible3D", BOOL, TRUE),
    ])?;
    Ok(Panel {
        record,
        edge_vertices: geometry.sides,
    })
}

fn physical(fabric: &Value) -> Result<Vec<u8>, String> {
    let values = &fabric["info"]["physical_properties"];
    let keys = [
        (b"fSuK".as_slice(), "stretch_u"),
        (b"fSvK", "stretch_v"),
        (b"fHK", "shear"),
        (b"fLeftShearK", "left_shear"),
        (b"fBuK", "bend_u"),
        (b"fBvK", "bend_v"),
        (b"fBhK", "bend_bias"),
        (b"fDensity", "density"),
        (b"fThickness", "thickness"),
        (b"fFriction", "friction"),
    ];
    let mut fields = Vec::new();
    for (field, name) in keys {
        if let Some(value) = values[name].as_f64() {
            let data = float_bytes([value])?;
            fields.push((field, data));
        }
    }
    encode_map(
        &fields
            .iter()
            .map(|(key, bytes)| (*key, FLOAT, bytes.as_slice()))
            .collect::<Vec<_>>(),
    )
}

fn material(fabric: &Value, id: u32, physical: &[u8]) -> Result<Vec<u8>, String> {
    let name = fabric["name"]
        .as_str()
        .ok_or("missing native fabric name")?;
    if !fabric["texture_maps"]
        .as_object()
        .is_none_or(|maps| maps.is_empty())
        || fabric["native_colorway_materials"].is_object()
    {
        return Err(
            "native authoring for this material requires texture or colorway records".into(),
        );
    }
    let rgba = serde_json::from_value::<[f64; 4]>(fabric["rgba"].clone())
        .or_else(|_| {
            serde_json::from_value::<[f64; 3]>(fabric["color"].clone())
                .map(|[r, g, b]| [r, g, b, 1.])
        })
        .map_err(|_| "invalid native fabric color")?;
    let color_name = rgba[..3]
        .iter()
        .map(|value| format!("{:02X}", (value.clamp(0., 1.) * 255.).round() as u8))
        .collect::<String>();
    let color = encode_map(&[
        (b"qsName", BYTE_TYPE, color_name.as_bytes()),
        (b"qsNameUTF8", BYTE_TYPE, color_name.as_bytes()),
        (b"v4Color", VEC4, &float_bytes(rgba)?),
    ])?;
    let [r, g, b, a] = rgba;
    let ambient = encode_map(&[
        (b"qsName", BYTE_TYPE, color_name.as_bytes()),
        (b"qsNameUTF8", BYTE_TYPE, color_name.as_bytes()),
        (
            b"v4Color",
            VEC4,
            &float_bytes([r * 0.25713187, g * 0.25713187, b * 0.25713187, a])?,
        ),
    ])?;
    let face = encode_map(&[
        (b"enMaterialFaceType", INT, &ZERO),
        (b"iVersion", INT, &1i32.to_le_bytes()),
        (b"v3BaseColor", VEC3, &float_bytes([r, g, b])?),
        (b"mapBaseColor", MAP_TYPE, &color),
        (b"mapAmbientColor", MAP_TYPE, &ambient),
        (b"fBaseColorMult", FLOAT, &1f32.to_le_bytes()),
        (b"listTexture", LIST_TYPE, &ZERO),
    ])?;
    let faces = records(&[face])?;
    let front = encode_map(&[(b"listFaceMaterial", LIST_TYPE, &faces)])?;
    let legacy_row = encode_map(&[
        (
            b"listMaterial2D",
            LIST_TYPE,
            &records(std::slice::from_ref(&front))?,
        ),
        (b"listFaceMaterial", LIST_TYPE, &faces),
    ])?;
    let legacy = encode_map(&[
        (b"uiCurrentCoordinationIndex", UINT, &ZERO),
        (b"listColorwayInfo", LIST_TYPE, &records(&[legacy_row])?),
        (b"iSubSize", INT, &1i32.to_le_bytes()),
    ])?;
    let catalog = &fabric["info"]["catalog"];
    encode_map(&[
        (b"qsName", BYTE_TYPE, name.as_bytes()),
        (b"qsNameUTF8", BYTE_TYPE, name.as_bytes()),
        (b"qsFabricName", BYTE_TYPE, name.as_bytes()),
        (b"qsFabricNameUTF8", BYTE_TYPE, name.as_bytes()),
        (
            b"fabricTypeUTF8",
            BYTE_TYPE,
            catalog["Classification"]
                .as_str()
                .unwrap_or("None")
                .as_bytes(),
        ),
        (
            b"fabricContentUTF8",
            BYTE_TYPE,
            catalog["Content"].as_str().unwrap_or("None").as_bytes(),
        ),
        (
            b"qsOwnerUTF8",
            BYTE_TYPE,
            catalog["Owner"].as_str().unwrap_or("None").as_bytes(),
        ),
        (
            b"qsSupplierNameUTF8",
            BYTE_TYPE,
            catalog["SupplierName"]
                .as_str()
                .unwrap_or("None")
                .as_bytes(),
        ),
        (b"uiID", UINT, &id.to_le_bytes()),
        (b"bVisible", BOOL, TRUE),
        (b"bEditable", BOOL, TRUE),
        (b"mapMaterial2D", MAP_TYPE, &front),
        (b"mapColorwayInfo", MAP_TYPE, &legacy),
        (b"mapPhysical", MAP_TYPE, physical),
    ])
}

pub(super) fn encode(state: &Value) -> Result<Vec<u8>, String> {
    let panels = array(state, "patterns");
    let fabrics = array(state, "fabrics");
    let colorways = array(state, "colorways");
    if panels.is_empty() || panels.len() > 4096 || fabrics.is_empty() || fabrics.len() > 4096 {
        return Err("native authoring requires editable panels and materials".into());
    }
    if ["native_seam_groups", "imported_meshes"]
        .into_iter()
        .any(|name| !array(state, name).is_empty())
    {
        return Err(
            "native authoring cannot yet preserve imported geometry; save as .clorust".into(),
        );
    }
    let [colorway] = colorways else {
        return Err("native authoring requires a single colorway".into());
    };
    let colorway_name = colorway
        .as_str()
        .ok_or("invalid native colorway name")?
        .as_bytes();
    let properties = fabrics
        .iter()
        .map(physical)
        .collect::<Result<Vec<_>, _>>()?;
    let fabrics = fabrics
        .iter()
        .zip(&properties)
        .enumerate()
        .map(|(index, (fabric, phys))| material(fabric, index as u32 + 1, phys))
        .collect::<Result<Vec<_>, _>>()?;
    let mut next_id = fabrics.len() as u32 + 1;
    let meshes = array(&state["simulation"], "meshes");
    if meshes
        .iter()
        .any(|mesh| !panels.iter().any(|panel| panel["id"] == mesh["id"]))
    {
        return Err("the saved pose refers to an unknown panel".into());
    }
    let panel_records = panels
        .iter()
        .enumerate()
        .map(|(index, panel_value)| {
            let saved = meshes.iter().find(|mesh| mesh["id"] == panel_value["id"]);
            panel(
                panel_value,
                index as u32,
                &mut next_id,
                &properties,
                state,
                saved,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let count = panels.len() as u32;
    let mut symmetric = count.to_le_bytes().to_vec();
    symmetric.push(INT);
    symmetric.extend((0..count).flat_map(|_| (-1i32).to_le_bytes()));
    let empty_instance = encode_map(&[(b"arrInstanceIndex", ARRAY, &ZERO)])?;
    let instances = encoded_list(&vec![
        Cow::Borrowed(empty_instance.as_slice());
        panels.len()
    ])?;
    let recorded_patterns = encoded_list(
        &panel_records
            .iter()
            .map(|panel| Cow::Borrowed(panel.record.as_slice()))
            .collect::<Vec<_>>(),
    )?;
    let table = encode_map(&[
        (b"listSizeName", LIST_TYPE, &ZERO),
        (b"listPattern", LIST_TYPE, &recorded_patterns),
        (b"arrSymmetricIndex", ARRAY, &symmetric),
        (b"listInstance", LIST_TYPE, &instances),
    ])?;
    let sewing = seams::encode(state, &panel_records, &mut next_id)?;
    let editor = encode_map(&[
        (b"mapPatternList", MAP_TYPE, &table),
        (b"mapSeamLinePairGroup", MAP_TYPE, &sewing),
        (b"mapSeamLinePairGroupWithHalfSymmetry", MAP_TYPE, &sewing),
        (b"uiMaxGID", UINT, &count.to_le_bytes()),
    ])?;
    let palette = encode_map(&[
        (b"qsName", BYTE_TYPE, colorway_name),
        (b"qsNameUTF8", BYTE_TYPE, colorway_name),
        (b"uiID", UINT, &(fabrics.len() as u32 + 1).to_le_bytes()),
        (b"uiColorwayMainFabricUID", UINT, &1u32.to_le_bytes()),
        (b"v4MainColor", VEC4, &float_bytes([-1., -1., -1., 1.])?),
    ])?;
    let colorways = encode_map(&[
        (b"listColorway", LIST_TYPE, &records(&[palette])?),
        (b"uiCurrentCoordinationIndex", UINT, &ZERO),
        (b"bIsTheNewColorway", BOOL, FALSE),
    ])?;
    encode_map(&[
        (b"qsClothIDListUTF8", BYTE_TYPE, b"Default"),
        (b"uiVersion", UINT, &1001u32.to_le_bytes()),
        (b"uiSaveType", UINT, &ZERO),
        (b"qsClothIDList", BYTE_TYPE, b"Default"),
        (b"mapColorWay", MAP_TYPE, &colorways),
        (b"mapPatternEditor", MAP_TYPE, &editor),
        (b"listFabric", LIST_TYPE, &records(&fabrics)?),
    ])
}
