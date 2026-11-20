//! Estimate CLO MCP drafting measurements using the saved avatar surface and its body regions.

use serde_json::{json, Value};
use std::collections::HashMap;

type Point = [f64; 3];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Region {
    Body,
    Arm,
    Leg,
    Head,
}

pub(super) type RegionFace = ([usize; 3], Region);

impl Region {
    pub(super) fn from_material(material: &str) -> Option<Self> {
        let suffix = material.rsplit(':').next()?.to_ascii_lowercase();
        if suffix.contains("arm") || suffix.contains("hand") {
            Some(Self::Arm)
        } else if suffix.contains("leg") || suffix.contains("foot") || suffix.contains("feet") {
            Some(Self::Leg)
        } else if suffix.contains("face") || suffix.contains("head") {
            Some(Self::Head)
        } else if suffix.contains("body") {
            Some(Self::Body)
        } else {
            None
        }
    }
}

fn distance(a: Point, b: Point) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}

fn perimeter(points: &[Point]) -> f64 {
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(&a, &b)| distance(a, b))
        .sum()
}

fn center(contour: &[Point], axis: usize) -> f64 {
    contour.iter().map(|p| p[axis]).sum::<f64>() / contour.len() as f64
}

fn tape(contour: &[Point]) -> Option<f64> {
    super::assets::tape_perimeter(contour.iter().map(|p| [p[0], p[2]]).collect())
}

fn join_segments(segments: Vec<[Point; 2]>) -> Vec<Vec<Point>> {
    let key = |point: Point| point.map(|n| (n / 0.05).round_ties_even() as i64);
    let mut incident = HashMap::<_, Vec<_>>::new();
    for (index, &[a, b]) in segments.iter().enumerate() {
        incident.entry(key(a)).or_default().push((index, b));
        incident.entry(key(b)).or_default().push((index, a));
    }
    let mut used = vec![false; segments.len()];
    let mut contours = Vec::new();
    for (index, &[start, end]) in segments.iter().enumerate() {
        if used[index] {
            continue;
        }
        let mut current = end;
        let mut contour = vec![start, end];
        used[index] = true;
        while let Some(&(next, other)) = incident[&key(current)].iter().find(|&&(i, _)| !used[i]) {
            used[next] = true;
            if key(other) == key(start) {
                break;
            }
            contour.push(other);
            current = other;
        }
        contours.push(contour);
    }
    contours
}

struct Surface<'a> {
    points: &'a [Point],
    faces: &'a [RegionFace],
}

impl Surface<'_> {
    fn slice(
        &self,
        signed_offset: impl Fn(Point) -> f64,
        accept: impl Fn(Region) -> bool,
        include_vertex: bool,
    ) -> Vec<Vec<Point>> {
        let mut segments = Vec::new();
        for &(indices, region) in self.faces {
            if !accept(region) {
                continue;
            }
            let triangle = indices.map(|i| self.points[i]);
            let offsets = triangle.map(&signed_offset);
            let mut hits = [None; 2];
            let mut count = 0;
            for i in 0..3 {
                let j = (i + 1) % 3;
                let (a, b) = (offsets[i], offsets[j]);
                let point = if a * b < 0.0 {
                    let weight = a / (a - b);
                    Some(std::array::from_fn(|axis| {
                        triangle[i][axis] + weight * (triangle[j][axis] - triangle[i][axis])
                    }))
                } else {
                    (include_vertex && a == 0.0).then_some(triangle[i])
                };
                if point.is_some() {
                    hits[count] = point;
                    count += 1;
                    if count == 2 {
                        break;
                    }
                }
            }
            if let [Some(a), Some(b)] = hits {
                segments.push([a, b]);
            }
        }
        join_segments(segments)
    }

    fn horizontal(&self, y: f64, accept: impl Fn(Region) -> bool) -> Vec<Vec<Point>> {
        self.slice(|point| point[1] - y, accept, true)
    }

    fn torso(&self, y: f64, accept: impl Fn(Region) -> bool) -> Option<(f64, Vec<Point>)> {
        self.horizontal(y, accept)
            .into_iter()
            .filter(|contour| contour.len() >= 8 && center(contour, 0).abs() <= 160.0)
            .filter_map(|contour| Some((tape(&contour)?, contour)))
            .min_by(|a, b| b.0.total_cmp(&a.0))
    }

    fn scan(
        &self,
        from: f64,
        to: f64,
        step: f64,
        score: impl Fn(f64, &[Point]) -> f64,
        accept: impl Fn(Region) -> bool,
    ) -> Option<(f64, f64)> {
        heights(from, to, step)
            .filter_map(|y| {
                let (girth, contour) = self.torso(y, &accept)?;
                Some((score(girth, &contour), girth, y))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, girth, y)| (girth, y))
    }

    fn arm_circumference(&self, start: Point, hand: Point, fraction: f64) -> f64 {
        let origin = std::array::from_fn::<_, 3, _>(|axis| {
            start[axis] + (hand[axis] - start[axis]) * fraction
        });
        self.slice(
            |point| {
                (0..3)
                    .map(|axis| (point[axis] - origin[axis]) * (hand[axis] - start[axis]))
                    .sum()
            },
            |region| region == Region::Arm,
            false,
        )
        .iter()
        .filter(|contour| contour.len() > 6 && contour[0][0] < 0.0)
        .map(|contour| perimeter(contour))
        .fold(0.0, f64::max)
    }
}

fn heights(from: f64, to: f64, step: f64) -> impl Iterator<Item = f64> {
    std::iter::successors(Some(from), move |&y| Some(y + step)).take_while(move |&y| y <= to)
}

fn x_extent(contour: &[Point]) -> f64 {
    let (min, max) = contour
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), p| {
            (min.min(p[0]), max.max(p[0]))
        });
    max - min
}

pub(super) fn measure(
    points: &[Point],
    faces: &[RegionFace],
    floor: f64,
    height: f64,
) -> Option<Value> {
    let surface = Surface { points, faces };
    let all = |_| true;
    let leg_body = |r| matches!(r, Region::Leg | Region::Body);
    let min_girth = |girth: f64, _: &[Point]| girth;
    let max_girth = |girth: f64, _: &[Point]| -girth;
    let biggest_left_leg = |y| {
        surface
            .horizontal(y, leg_body)
            .into_iter()
            .filter(|c| c.len() > 6 && center(c, 0) < 0.0)
            .min_by(|a, b| perimeter(b).total_cmp(&perimeter(a)))
    };
    let front = match (
        biggest_left_leg(floor + 25.0),
        biggest_left_leg(floor + 160.0),
    ) {
        (Some(foot), Some(shin)) if center(&foot, 2) <= center(&shin, 2) => -1.0,
        _ => 1.0,
    };
    let crotch = heights(floor + 0.30 * height, floor + 0.60 * height, 8.0)
        .find(|&y| {
            surface
                .horizontal(y, all)
                .iter()
                .filter(|c| c.len() > 8 && center(c, 0).abs() < 160.0)
                .count()
                == 1
        })
        .unwrap_or(floor + 0.45 * height);

    let arm_points: Vec<_> = faces
        .iter()
        .filter(|(_, r)| *r == Region::Arm)
        .flat_map(|(indices, _)| indices.map(|i| points[i]))
        .collect();
    let shoulder = *arm_points.iter().min_by(|a, b| b[1].total_cmp(&a[1]))?;
    let mut shoulder_half = shoulder[0].abs();
    for offset in [5.0, 12.0, 20.0, 30.0] {
        if let Some(contour) = surface
            .horizontal(shoulder[1] - offset, all)
            .into_iter()
            .filter(|c| c.len() > 8)
            .min_by(|a, b| perimeter(b).total_cmp(&perimeter(a)))
        {
            shoulder_half = shoulder_half.max(x_extent(&contour) * 0.5);
        }
    }
    let armpit = arm_points
        .iter()
        .filter(|p| p[0].abs() < shoulder_half + 30.0 && p[1] > shoulder[1] - 350.0)
        .map(|p| p[1])
        .reduce(f64::min)
        .unwrap_or(shoulder[1] - 150.0);

    let (hip, hip_y) = surface.scan(crotch + 40.0, crotch + 300.0, 8.0, max_girth, all)?;
    let (waist, waist_y) = surface.scan(
        hip_y + 80.0,
        (hip_y + 340.0).min(armpit - 150.0),
        6.0,
        min_girth,
        all,
    )?;
    let bust = surface
        .scan(
            waist_y + 60.0,
            armpit - 15.0,
            5.0,
            |_, contour| {
                contour
                    .iter()
                    .map(|p| -front * p[2])
                    .fold(f64::INFINITY, f64::min)
            },
            all,
        )
        .or_else(|| {
            let y = waist_y + 180.0;
            surface.torso(y, all).map(|(girth, _)| (girth, y))
        });
    let neck = surface.scan(
        shoulder[1] + 5.0,
        shoulder[1] + 150.0,
        5.0,
        min_girth,
        |r| r != Region::Leg,
    );
    let knee = heights(floor + 0.20 * height, crotch - 80.0, 8.0)
        .filter_map(|y| {
            let contour = biggest_left_leg(y)?;
            (contour.len() > 8).then(|| Some((tape(&contour)?, y)))?
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, y)| y)
        .unwrap_or(floor + 0.28 * height);

    // Keep the standard drafting values for fields that the geometry fallback
    // does not measure.
    let mut body = super::assets::default_body();
    let mut set =
        |key: &str, value: f64| body[key] = json!((value * 10.0).round_ties_even() / 10.0);
    set("height", height);
    set("hip", hip);
    set("h_hip", hip_y - floor);
    set("waist", waist);
    set("h_waist", waist_y - floor);
    set("h_crotch", crotch - floor);
    set("h_knee", knee - floor);
    set("across_shoulder", shoulder_half * 2.0);
    set("h_shoulder", shoulder[1] - floor);
    let (neck, neck_y) = neck.unwrap_or((360.0, shoulder[1] + 60.0));
    set("neck", neck);
    set("h_hps", neck_y - floor);
    if let Some((bust, y)) = bust {
        set("bust", bust);
        set("h_apex", y - floor);
        if let Some((underbust, y)) = surface.scan(
            (y - 120.0).max(waist_y + 30.0),
            y - 45.0,
            5.0,
            min_girth,
            all,
        ) {
            set("underbust", underbust);
            set("h_underbust", y - floor);
        }
    }
    let start = [-shoulder[0].abs(), shoulder[1], shoulder[2]];
    if let Some(&hand) = arm_points
        .iter()
        .filter(|p| p[0] < 0.0)
        .min_by(|&&a, &&b| distance(b, start).total_cmp(&distance(a, start)))
    {
        let arm = |fraction| surface.arm_circumference(start, hand, fraction);
        let bicep = [0.14, 0.18, 0.22, 0.26, 0.30]
            .into_iter()
            .map(arm)
            .fold(0.0, f64::max);
        if bicep > 0.0 {
            set("bicep", bicep);
        }
        let wrist = [0.62, 0.66, 0.70, 0.74, 0.78, 0.82, 0.86]
            .into_iter()
            .map(|fraction| (arm(fraction), fraction))
            .filter(|&(girth, _)| girth > 0.0)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((girth, _)) = wrist {
            set("wrist", girth);
        }
        set(
            "arm_length",
            distance(hand, start) * wrist.map_or(0.8, |(_, t)| t),
        );
    }
    // The body subtype follows the CLO MCP mesh fallback. The tool envelope
    // reports that this implementation read a saved Rust avatar surface.
    body["source"] = json!("obj");
    Some(body)
}
