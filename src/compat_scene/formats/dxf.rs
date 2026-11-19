use super::{super::patterns, write};
use patterns::Point;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const CLOSE_TOLERANCE_MM: f64 = 0.02;

#[derive(Default)]
struct Entity<'a> {
    kind: &'a str,
    pairs: Vec<(i32, &'a str)>,
}

impl<'a> Entity<'a> {
    fn attr(&self, key: i32) -> Option<&'a str> {
        self.pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    fn parse(&self, key: i32, default: f64) -> Result<f64, String> {
        self.attr(key)
            .map_or(Ok(default), |text| numeric(text, key))
    }

    fn vertex(&self, x: i32, y: i32, scale: f64) -> Result<Point, String> {
        Ok(Point {
            x: self.parse(x, 0.)? * scale,
            y: self.parse(y, 0.)? * scale,
            kind: 0,
        })
    }

    fn closed(&self) -> bool {
        self.attr(70)
            .and_then(|text| text.parse::<u32>().ok())
            .unwrap_or(0)
            & 1
            != 0
    }
}

fn numeric(text: &str, key: i32) -> Result<f64, String> {
    text.parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
        .ok_or_else(|| format!("invalid DXF group {key} numeric value {text:?}"))
}

fn units(pairs: &[(i32, &str)]) -> Result<f64, String> {
    let unit = pairs
        .windows(2)
        .find(|pair| pair[0] == (9, "$INSUNITS") && pair[1].0 == 70)
        .map_or("0", |pair| pair[1].1);
    match unit {
        "0" | "4" => Ok(1.),
        "1" => Ok(25.4),
        "2" => Ok(304.8),
        "5" => Ok(10.),
        "6" => Ok(1000.),
        _ => Err(format!("unsupported DXF length unit code {unit}")),
    }
}

fn close(a: Point, b: Point) -> bool {
    (a.x - b.x).hypot(a.y - b.y) < CLOSE_TOLERANCE_MM
}

fn arc_points(a: Point, b: Point, bulge: f64) -> Vec<Point> {
    if bulge.abs() < 1e-10 || close(a, b) {
        return Vec::new();
    }
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let factor = (1. - bulge * bulge) / (4. * bulge);
    let centre = [
        (a.x + b.x) / 2. - dy * factor,
        (a.y + b.y) / 2. + dx * factor,
    ];
    let phi = (a.y - centre[1]).atan2(a.x - centre[0]);
    let radius = (a.x - centre[0]).hypot(a.y - centre[1]);
    let angle = 4. * bulge.atan();
    let steps = (angle.abs() / 0.12).ceil().clamp(2., 64.) as u32;
    (1..steps)
        .map(|i| {
            let theta = phi + angle * f64::from(i) / f64::from(steps);
            Point {
                x: centre[0] + radius * theta.cos(),
                y: centre[1] + radius * theta.sin(),
                kind: 0,
            }
        })
        .collect()
}

fn polyline(vertices: &[(Point, f64)], closed: bool) -> Vec<Point> {
    let mut points = Vec::new();
    for (i, &(p, bulge)) in vertices.iter().enumerate() {
        points.push(p);
        if let Some(&(next, _)) = vertices.get(i + 1).or_else(|| closed.then(|| &vertices[0])) {
            points.extend(arc_points(p, next, bulge));
        }
    }
    points
}

fn lightweight(entity: &Entity, scale: f64) -> Result<Vec<(Point, f64)>, String> {
    let mut vertices = Vec::new();
    for (key, text) in &entity.pairs {
        if *key == 10 {
            vertices.push((
                Point {
                    x: numeric(text, 10)? * scale,
                    y: 0.,
                    kind: 0,
                },
                0.,
            ));
        } else if let Some((last, bulge)) = vertices.last_mut() {
            match key {
                20 => last.y = numeric(text, 20)? * scale,
                42 => *bulge = numeric(text, 42)?,
                _ => {}
            }
        }
    }
    Ok(vertices)
}

fn outline_layer(layer: &str) -> bool {
    let lower = layer.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "0" | "1" | "boundary" | "cut" | "outline" | "pattern"
    ) || lower.starts_with("cut-")
        || lower.starts_with("outline-")
}

struct Boundary {
    name: Option<String>,
    points: Vec<Point>,
}

fn add_boundary(
    boundaries: &mut Vec<Boundary>,
    mut points: Vec<Point>,
    closed: bool,
    name: Option<&str>,
) {
    if points.len() >= 4 && close(points[0], points[points.len() - 1]) {
        points.pop();
    }
    if closed && points.len() >= 3 {
        let area: f64 = points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .map(|(a, b)| a.x * b.y - a.y * b.x)
            .sum::<f64>()
            .abs()
            / 2.;
        if area > 1e-4 {
            boundaries.push(Boundary {
                name: name.map(str::to_owned),
                points,
            });
        }
    }
}

fn join_outlines(
    mut open_paths: Vec<Vec<Point>>,
    name: Option<&str>,
    boundaries: &mut Vec<Boundary>,
) {
    while let Some(mut path) = open_paths.pop() {
        loop {
            let first = path[0];
            let last = path[path.len() - 1];
            if close(first, last) {
                add_boundary(boundaries, path, true, name);
                break;
            }
            let mut matches = open_paths.iter().enumerate().filter_map(|(i, candidate)| {
                let start = close(last, candidate[0]);
                let end = close(last, candidate[candidate.len() - 1]);
                match (start, end) {
                    (true, false) => Some((i, false)),
                    (false, true) => Some((i, true)),
                    _ => None,
                }
            });
            let Some((i, reverse)) = matches.next() else {
                break;
            };
            if matches.next().is_some() {
                break;
            }
            let mut candidate = open_paths.swap_remove(i);
            if reverse {
                candidate.reverse();
            }
            path.extend_from_slice(&candidate[1..]);
        }
    }
}

pub(in crate::compat_scene) fn read_dxf(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let mut iter = text.lines();
    let mut pairs = Vec::new();
    while let Some(key) = iter.next() {
        let group = key
            .trim()
            .parse::<i32>()
            .map_err(|_| "expected ASCII DXF group codes")?;
        let value = iter.next().ok_or("missing DXF group value")?.trim();
        pairs.push((group, value));
    }
    let scale = units(&pairs)?;
    let mut entities = Vec::new();
    for (key, value) in pairs {
        if key == 0 {
            entities.push(Entity {
                kind: value,
                pairs: Vec::new(),
            });
        } else if let Some(entity) = entities.last_mut() {
            entity.pairs.push((key, value));
        }
    }
    let mut boundaries = Vec::new();
    let mut block = None;
    let mut skipped = false;
    let mut open_paths: BTreeMap<(Option<&str>, &str), Vec<Vec<Point>>> = BTreeMap::new();
    let mut i = 0;
    while i < entities.len() {
        let entity = &entities[i];
        let name = entity.attr(8).filter(|layer| !matches!(*layer, "0" | "1"));
        let layer = entity.attr(8).unwrap_or("0");
        let boundary_layer = outline_layer(layer);
        match entity.kind {
            "BLOCK" => block = entity.attr(2).filter(|name| !name.starts_with('*')),
            "ENDBLK" => block = None,
            "LWPOLYLINE" => {
                let v = lightweight(entity, scale)?;
                let closed = entity.closed()
                    || v.first()
                        .zip(v.last())
                        .is_some_and(|(a, b)| close(a.0, b.0));
                if boundary_layer {
                    let pts = polyline(&v, entity.closed());
                    if closed {
                        add_boundary(&mut boundaries, pts, true, block.or(name));
                    } else if pts.len() >= 2 {
                        open_paths
                            .entry((block.or(name), layer))
                            .or_default()
                            .push(pts);
                    }
                }
                skipped |= !boundary_layer || !closed;
            }
            "POLYLINE" => {
                let mut vertices = Vec::new();
                while i + 1 < entities.len() && entities[i + 1].kind == "VERTEX" {
                    i += 1;
                    let v = &entities[i];
                    vertices.push((v.vertex(10, 20, scale)?, v.parse(42, 0.)?));
                }
                let closed = entity.closed()
                    || vertices
                        .first()
                        .zip(vertices.last())
                        .is_some_and(|(a, b)| close(a.0, b.0));
                if boundary_layer {
                    let pts = polyline(&vertices, entity.closed());
                    if closed {
                        add_boundary(&mut boundaries, pts, true, block.or(name));
                    } else if pts.len() >= 2 {
                        open_paths
                            .entry((block.or(name), layer))
                            .or_default()
                            .push(pts);
                    }
                }
                skipped |= !boundary_layer || !closed;
            }
            "LINE" if boundary_layer => {
                let start = entity.vertex(10, 20, scale)?;
                let end = entity.vertex(11, 21, scale)?;
                if !close(start, end) {
                    open_paths
                        .entry((block.or(name), layer))
                        .or_default()
                        .push(vec![start, end]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    for ((name, _), paths) in open_paths {
        join_outlines(paths, name, &mut boundaries);
    }
    if boundaries.is_empty() {
        let scope = if skipped {
            " (open curves and annotation layers are not panel outlines)"
        } else {
            ""
        };
        return Err(format!(
            "DXF has no supported closed pattern boundaries{scope}"
        ));
    }
    let pieces: Vec<_> = boundaries.into_iter().enumerate().map(|(i, boundary)| {
        json!({"id": i + 1, "name": boundary.name.unwrap_or_else(|| format!("Pattern_{}", i + 1)),
            "points": boundary.points.into_iter().map(Point::record).collect::<Vec<_>>(), "arrangement": "", "fabric_index": 0})
    }).collect();
    Ok(json!({"patterns": pieces, "seams": []}))
}

pub(in crate::compat_scene) fn write_dxf(path: &Path, panels: &[Value]) -> Result<(), String> {
    let mut dxf = String::from("0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n");
    for (i, pattern) in panels.iter().enumerate() {
        let name = pattern["name"].as_str().unwrap_or("Pattern");
        let layer: String = name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || "_-.".contains(ch) {
                    ch
                } else {
                    '_'
                }
            })
            .take(200)
            .collect();
        let points = patterns::outline(pattern)?;
        let dx = pattern["pos_2d"][0].as_f64().unwrap_or(0.);
        let dy = pattern["pos_2d"][1].as_f64().unwrap_or(0.);
        let _ = writeln!(
            dxf,
            "0\nLWPOLYLINE\n100\nAcDbEntity\n8\nCut-{layer}-{i}\n100\nAcDbPolyline\n90\n{}\n70\n1",
            points.len()
        );
        for p in points {
            let _ = writeln!(dxf, "10\n{:.9}\n20\n{:.9}", p.x + dx, p.y + dy);
        }
    }
    dxf.push_str("0\nENDSEC\n0\nEOF\n");
    write(path, dxf.as_bytes())
}
