mod anthropometry;
mod avatar;
mod avatar_source;
mod collision;
mod compat_engine;
mod compat_fit;
mod compat_runtime;
mod compat_scene;
mod connectivity;
mod mesh;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::Path,
};

const VERSION: u16 = 1;

#[derive(Serialize, Deserialize)]
struct Piece {
    name: String,
    lines: Vec<Edge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    draft_paths: Option<Value>,
}

#[derive(Serialize, Deserialize)]
struct Edge {
    #[serde(rename = "lineIndex")]
    id: u32,
    length: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    start: Point,
    end: Point,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mid: Option<Point>,
}

#[derive(Serialize, Deserialize)]
struct Point {
    x: f64,
    y: f64,
}

#[derive(Serialize, Deserialize)]
struct Span(String, u32, f64, f64);

#[derive(Serialize, Deserialize)]
struct Sewing(Span, Span, f64, f64);

#[derive(Serialize, Deserialize)]
struct Project {
    version: u16,
    source: String,
    units: String,
    pieces: Vec<Piece>,
    seams: Vec<Sewing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    draft_settings: Option<Value>,
}

#[derive(Serialize)]
struct Audit {
    pieces: usize,
    boundaries: usize,
    declared_seams: usize,
    valid_endpoints: usize,
    segmented_seams: usize,
    reversed_intervals: usize,
    largest_length_delta_mm: f64,
    length_review: usize,
    interval_overlap: usize,
    errors: Vec<String>,
}

fn read<T: for<'de> Deserialize<'de>>(path: &str) -> Result<T, String> {
    serde_json::from_reader(fs::File::open(path).map_err(|e| format!("{path}：{e}"))?)
        .map_err(|e| format!("{path}：{e}"))
}

fn write(path: &str, project: &Project) -> Result<(), String> {
    let parent = Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temp = parent.join(format!(".pattern-native-{}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        serde_json::to_writer_pretty(&mut file, project).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() && temp.is_file() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[derive(Deserialize)]
struct IndexMap {
    name: String,
    edges: Vec<u32>,
    reverse: bool,
}

fn normalize_seam_keys(seams: &mut [Sewing], mapping: &[IndexMap]) -> Result<(), String> {
    let mut edge_map = BTreeMap::new();
    for part in mapping {
        for (line_index, &design_edge) in part.edges.iter().enumerate() {
            if edge_map
                .insert(
                    (part.name.as_str(), design_edge),
                    (line_index as u32, part.reverse),
                )
                .is_some()
            {
                return Err(format!("纸样边映射重复：{} / {}", part.name, design_edge));
            }
        }
    }
    for Sewing(a, b, _, _) in seams {
        for Span(name, id, start, end) in [a, b] {
            let (line, reverse) = edge_map
                .get(&(name.as_str(), *id))
                .ok_or_else(|| format!("缺少纸样边映射：{name} / {id}"))?;
            *id = *line;
            if *reverse {
                *start = 1.0 - *start;
                *end = 1.0 - *end;
            }
        }
    }
    Ok(())
}

fn audit(project: &Project, tolerance: f64) -> Audit {
    let mut result = Audit {
        pieces: project.pieces.len(),
        boundaries: 0,
        declared_seams: project.seams.len(),
        valid_endpoints: 0,
        segmented_seams: 0,
        reversed_intervals: 0,
        largest_length_delta_mm: 0.0,
        length_review: 0,
        interval_overlap: 0,
        errors: vec![],
    };
    if project.version != VERSION || project.units != "mm" {
        result.errors.push("不支持的工程版本或单位".into());
    }
    let mut pieces = BTreeSet::new();
    let mut edges = BTreeMap::new();
    for part in &project.pieces {
        if part.name.trim().is_empty() || !pieces.insert(part.name.as_str()) {
            result
                .errors
                .push(format!("重复或空白裁片名：{}", part.name));
        }
        for edge in &part.lines {
            if !edge.length.is_finite() || edge.length <= 0.0 {
                result
                    .errors
                    .push(format!("无效边长：{} / {}", part.name, edge.id));
            }
            if edges
                .insert((part.name.as_str(), edge.id), edge.length)
                .is_some()
            {
                result
                    .errors
                    .push(format!("重复边号：{} / {}", part.name, edge.id));
            }
        }
    }
    result.boundaries = edges.len();
    let mut occupied: BTreeMap<(&str, u32), Vec<(f64, f64)>> = BTreeMap::new();
    for (index, Sewing(a, b, la, lb)) in project.seams.iter().enumerate() {
        let index = index + 1;
        let mut segmented = false;
        for (Span(name, edge, t0, t1), length) in [(a, la), (b, lb)] {
            let Some(full) = edges.get(&(name.as_str(), *edge)) else {
                result
                    .errors
                    .push(format!("缝合 {index} 引用不存在的边：{name} / {edge}"));
                continue;
            };
            if !(0.0..=1.0).contains(t0) || !(0.0..=1.0).contains(t1) || (t0 - t1).abs() < 1e-9 {
                result.errors.push(format!("缝合 {index} 参数区间无效"));
                continue;
            }
            if !length.is_finite()
                || *length <= 0.0
                || (*length - (t0 - t1).abs() * full).abs() > 0.25
            {
                result
                    .errors
                    .push(format!("缝合 {index} 参考长度与对应区间不符"));
                continue;
            }
            result.valid_endpoints += 1;
            result.reversed_intervals += usize::from(t1 < t0);
            let lo = t0.min(*t1);
            let hi = t0.max(*t1);
            segmented |= lo > 1e-9 || hi < 1.0 - 1e-9;
            let intervals = occupied.entry((name.as_str(), *edge)).or_default();
            // Touching endpoints are valid. Reused spans are review findings.
            if intervals.iter().any(|&(a, b)| hi.min(b) - lo.max(a) > 1e-6) {
                result.interval_overlap += 1;
            }
            intervals.push((lo, hi));
        }
        result.segmented_seams += usize::from(segmented);
        let delta = (la - lb).abs();
        result.largest_length_delta_mm = result.largest_length_delta_mm.max(delta);
        result.length_review += usize::from(delta > tolerance);
    }
    result
}

#[derive(Serialize)]
struct Constraint<'a> {
    seam: usize,
    a: Sample<'a>,
    b: Sample<'a>,
}

#[derive(Serialize)]
struct Sample<'a> {
    piece: &'a str,
    edge: u32,
    offset: f64,
}

fn sample(span: &Span, t: f64) -> Sample<'_> {
    Sample {
        piece: &span.0,
        edge: span.1,
        offset: if t == 1.0 {
            span.3
        } else {
            span.2 + (span.3 - span.2) * t
        },
    }
}

fn constraints(project: &Project) -> Vec<Constraint<'_>> {
    let mut result = Vec::new();
    for (seam, Sewing(a, b, la, lb)) in project.seams.iter().enumerate() {
        let steps = (la.max(*lb) / 5.0).ceil().clamp(1.0, 10000.0) as u32;
        for step in 0..=steps {
            let t = f64::from(step) / f64::from(steps);
            result.push(Constraint {
                seam: seam + 1,
                a: sample(a, t),
                b: sample(b, t),
            });
        }
    }
    result
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["mcp-compat"] => compat_engine::run()?,
        ["--version"] => println!("pattern-native {}", env!("CARGO_PKG_VERSION")),
        ["clo-import", edges, mapping, seams, out] => {
            let mut seams: Vec<Sewing> = read(seams)?;
            normalize_seam_keys(&mut seams, &read::<Vec<IndexMap>>(mapping)?)?;
            let project=Project{version:VERSION,source:"CLO 样本".into(),units:"mm".into(),pieces:read(edges)?,seams,draft_settings:None};
            let result=audit(&project,2.0);
            if !result.errors.is_empty() { return Err(serde_json::to_string_pretty(&result).unwrap()); }
            write(out,&project)?;
            println!("{}",serde_json::to_string_pretty(&result).unwrap());
        }
        ["draft-import", input, out] => {
            let project:Project=read(input)?;
            let result=audit(&project,2.0);
            if !result.errors.is_empty() { return Err(serde_json::to_string_pretty(&result).unwrap()); }
            write(out,&project)?;
            println!("{}",serde_json::to_string_pretty(&result).unwrap());
        }
        ["audit", input] => {
            let project:Project=read(input)?;
            let result=audit(&project,2.0);
            let output=serde_json::to_string_pretty(&result).unwrap();
            if !result.errors.is_empty() { return Err(output); }
            println!("{output}");
        }
        ["connectivity", input] | ["connectivity", input, "--strict"] => {
            let live:connectivity::Live=read(input)?;
            let result=connectivity::check(&live)?;
            println!("{}",serde_json::to_string_pretty(&result).unwrap());
            if args.len() == 3 && !result.same_as_clo {
                return Err("缝合连通分组与参照结果不一致".into());
            }
        }
        ["avatar-measure", input, units, request] | ["avatar-resize", input, units, request, _] => {
            let request = read(request)?;
            let record = anthropometry::run(input, units, request, args.get(4).map(String::as_str))?;
            println!("{}", serde_json::to_string_pretty(&record).map_err(|e| e.to_string())?);
        }
        ["avatar-source", bundle, library, first, rest @ ..] => {
            let mut assets = vec![*first];
            assets.extend_from_slice(rest);
            let result = avatar_source::compare(bundle, library, &assets)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
        }
        ["avatar-import", input, mesh, units] | ["avatar-import", input, mesh, units, _] => {
            let body = avatar::load(input, mesh, units)?;
            if let Some(output) = args.get(4) {
                body.write_obj(output).map_err(|error| error.to_string())?;
            }
            println!("{}", serde_json::to_string_pretty(&body.summary()).unwrap());
        }
        ["collision-avatar", avatar_path, body_name, body_units, cloth_path, cloth_selector, cloth_units]
        | ["collision-avatar", avatar_path, body_name, body_units, cloth_path, cloth_selector, cloth_units, _] => {
            let body = avatar::load(avatar_path, body_name, body_units)?;
            let cloth_names: Vec<String> = if cloth_selector.starts_with('[') {
                serde_json::from_str(cloth_selector).map_err(|error| format!("服装网格名列表无效：{error}"))?
            } else {
                vec![(*cloth_selector).into()]
            };
            let cloth_names: Vec<_> = cloth_names.iter().map(String::as_str).collect();
            let cloth = avatar::load_selected(cloth_path, &cloth_names, cloth_units)?;
            let result = collision::inspect_surfaces(&body, &cloth, args.get(7).map(String::as_str))?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
        }
        ["collision-obj", input, body_materials, cloth_prefix] | ["collision-obj", input, body_materials, cloth_prefix, _] => {
            let result=collision::inspect_obj(input,body_materials,cloth_prefix,args.get(4).map(String::as_str))?;
            println!("{}",serde_json::to_string_pretty(&result).unwrap());
        }
        ["mesh-metrics", input] | ["mesh-metrics", input, _] => {
            let result=mesh::inspect(input,args.get(2).map(String::as_str))?;
            println!("{}",serde_json::to_string_pretty(&result).unwrap());
        }
        ["constraints", input] => {
            let project:Project=read(input)?;
            let result=audit(&project,2.0);
            if !result.errors.is_empty() { return Err(serde_json::to_string_pretty(&result).unwrap()); }
            println!("{}",serde_json::to_string(&constraints(&project)).unwrap());
        }
        _ => return Err("用法：pattern-native clo-import 边记录 边映射 缝合记录 工程 | draft-import 出样数据 工程 | audit 工程 | constraints 工程 | connectivity 缝合成员JSON [--strict] | mesh-metrics 服装GLB [调试VTK] | avatar-measure 人台 文件单位 量体配置JSON | avatar-resize 人台 文件单位 量体配置JSON 输出OBJ | avatar-source 应用资产目录 用户资产目录 相对资源路径... | avatar-import 人台GLB 网格名 单位 [姿势OBJ] | collision-avatar 人台GLB 人台网格名 人台单位 服装GLB 服装网格名或JSON列表 服装单位 [碰撞VTK] | collision-obj 人衣OBJ 人体材质列表 服装材质前缀 [碰撞VTK]".into()),
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
