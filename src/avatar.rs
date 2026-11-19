use gltf::{
    mesh::{Mode, Semantic},
    Gltf, Node,
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufWriter, Write},
};

pub type Point = [f64; 3];
type Matrix = [[f64; 4]; 4];

const IDENTITY: Matrix = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

pub struct Surface {
    pub points: Vec<Point>,
    pub faces: Vec<[usize; 3]>,
    joints: Vec<Joint>,
    skinned_vertices: usize,
}

#[derive(Serialize)]
struct Joint {
    node: usize,
    name: Option<String>,
    position_mm: Point,
}

#[derive(Serialize)]
pub struct AvatarSummary<'a> {
    vertices: usize,
    triangles: usize,
    skinned_vertices: usize,
    bounds_mm: [Point; 2],
    height_mm: f64,
    joints: &'a [Joint],
}

fn multiply(left: Matrix, right: Matrix) -> Matrix {
    std::array::from_fn(|column| {
        std::array::from_fn(|row| (0..4).map(|k| left[k][row] * right[column][k]).sum())
    })
}

fn transform(matrix: Matrix, p: Point) -> Point {
    std::array::from_fn(|row| (0..3).map(|k| matrix[k][row] * p[k]).sum::<f64>() + matrix[3][row])
}

fn world_matrices(
    node: Node<'_>,
    parent: Matrix,
    world: &mut [Option<Matrix>],
) -> Result<(), String> {
    let index = node.index();
    if world[index].is_some() {
        return Err(format!("GLB 场景节点有重复父项或循环：{index}"));
    }
    let matrix = multiply(
        parent,
        node.transform().matrix().map(|col| col.map(f64::from)),
    );
    if matrix.iter().flatten().any(|value| !value.is_finite()) {
        return Err(format!("GLB 场景节点变换含非有限值：{index}"));
    }
    world[index] = Some(matrix);
    for child in node.children() {
        world_matrices(child, matrix, world)?;
    }
    Ok(())
}

pub fn millimetres_per_unit(units: &str) -> Result<f64, String> {
    match units {
        "mm" => Ok(1.0),
        "cm" => Ok(10.0),
        "m" => Ok(1000.0),
        _ => Err("请指定源场景坐标单位 mm、cm 或 m".into()),
    }
}

pub fn load(path: &str, name: &str, units: &str) -> Result<Surface, String> {
    load_selected(path, &[name], units)
}

pub fn load_selected(path: &str, names: &[&str], units: &str) -> Result<Surface, String> {
    let names: BTreeSet<_> = names.iter().copied().collect();
    if names.is_empty() || names.contains("") {
        return Err("需要至少一个明确的 GLB 网格名".into());
    }
    let scale = millimetres_per_unit(units)?;
    let model = Gltf::open(path).map_err(|e| e.to_string())?;
    let bytes = model.blob.as_deref().ok_or("需要内嵌缓冲区的 GLB")?;
    let scene = model
        .default_scene()
        .or_else(|| model.scenes().next())
        .ok_or("GLB 缺少场景")?;
    let mut world = vec![None; model.nodes().count()];
    for root in scene.nodes() {
        world_matrices(root, IDENTITY, &mut world)?;
    }
    let matrix_for =
        |index: usize| world[index].ok_or_else(|| format!("节点 {index} 不在活动场景里"));
    let read_buffer = |buffer: gltf::Buffer<'_>| match buffer.source() {
        gltf::buffer::Source::Bin => Some(bytes),
        gltf::buffer::Source::Uri(_) => None,
    };
    let mut points = Vec::new();
    let mut faces = Vec::new();
    let mut joints = Vec::new();
    let mut listed = BTreeSet::new();
    let mut skinned_vertices = 0;
    let mut matched = BTreeSet::new();
    for (node, mesh) in model
        .nodes()
        .filter_map(|node| node.mesh().map(|mesh| (node, mesh)))
    {
        let selected: Vec<_> = [node.name(), mesh.name()]
            .into_iter()
            .flatten()
            .filter_map(|name| names.get(name).copied())
            .collect();
        if world[node.index()].is_none() || selected.is_empty() {
            continue;
        }
        matched.extend(selected);
        let rigid = matrix_for(node.index())?;
        let joint_matrices = if let Some(skin) = node.skin() {
            let nodes: Vec<_> = skin.joints().collect();
            let inverse: Vec<_> = match skin.reader(read_buffer).read_inverse_bind_matrices() {
                Some(read) => read.map(|m| m.map(|col| col.map(f64::from))).collect(),
                None if skin.inverse_bind_matrices().is_none() => vec![IDENTITY; nodes.len()],
                None => return Err("无法读取人台逆绑定矩阵".into()),
            };
            if nodes.len() > inverse.len() || nodes.is_empty() {
                return Err("人台关节与逆绑定矩阵数量不同".into());
            }
            nodes
                .iter()
                .zip(inverse)
                .map(|(joint, inverse)| {
                    let current = matrix_for(joint.index())?;
                    if listed.insert(joint.index()) {
                        joints.push(Joint {
                            node: joint.index(),
                            name: joint.name().map(str::to_owned),
                            position_mm: transform(current, [0.0; 3]).map(|value| value * scale),
                        });
                    }
                    Ok(multiply(current, inverse))
                })
                .collect::<Result<Vec<_>, String>>()?
        } else {
            Vec::new()
        };
        for primitive in mesh.primitives() {
            if primitive.mode() != Mode::Triangles || primitive.morph_targets().next().is_some() {
                return Err("人台和服装输入需要三角面。请先导出已经应用变化的形状".into());
            }
            let reader = primitive.reader(read_buffer);
            let rest: Vec<_> = reader
                .read_positions()
                .ok_or("GLB 缺少三维坐标")?
                .map(|point| point.map(f64::from))
                .collect();
            let offset = points.len();
            if joint_matrices.is_empty() {
                points.extend(rest.iter().map(|&p| transform(rigid, p).map(|v| v * scale)));
            } else {
                let mut skinned = vec![[0.0; 3]; rest.len()];
                let mut totals = vec![0.0; rest.len()];
                let sets: BTreeSet<_> = primitive
                    .attributes()
                    .filter_map(|(semantic, _)| match semantic {
                        Semantic::Joints(index) | Semantic::Weights(index) => Some(index),
                        _ => None,
                    })
                    .collect();
                if sets.is_empty() {
                    return Err("人台绑定网格缺少顶点蒙皮信息".into());
                }
                for set in sets {
                    let indices = reader.read_joints(set).ok_or("缺少关节索引")?.into_u16();
                    let weights = reader.read_weights(set).ok_or("缺少蒙皮权重")?.into_f32();
                    if indices.len() != rest.len() || weights.len() != rest.len() {
                        return Err("人台顶点、关节索引和权重数量不同".into());
                    }
                    for (i, (indices, weights)) in indices.zip(weights).enumerate() {
                        for (index, weight) in indices.into_iter().zip(weights) {
                            let weight = f64::from(weight);
                            if !weight.is_finite() || weight < 0.0 {
                                return Err("蒙皮权重无效".into());
                            }
                            if weight == 0.0 {
                                continue;
                            }
                            let matrix = *joint_matrices
                                .get(usize::from(index))
                                .ok_or("关节索引越界")?;
                            let p = transform(matrix, rest[i]);
                            for (value, contribution) in skinned[i].iter_mut().zip(p) {
                                *value += contribution * weight;
                            }
                            totals[i] += weight;
                        }
                    }
                }
                for (p, total) in skinned.into_iter().zip(totals) {
                    if total <= 0.0 {
                        return Err("至少一个顶点没有有效蒙皮权重".into());
                    }
                    points.push(p.map(|value| value / total * scale));
                }
                skinned_vertices += rest.len();
            }
            if points[offset..]
                .iter()
                .flatten()
                .any(|value| !value.is_finite())
            {
                return Err("计算后的网格坐标含非有限值".into());
            }
            let indices: Vec<_> = match reader.read_indices() {
                Some(indices) => indices.into_u32().map(|value| value as usize).collect(),
                None if primitive.indices().is_none() => (0..rest.len()).collect(),
                None => return Err("无法读取 GLB 的三角面索引".into()),
            };
            if indices.len() % 3 != 0 || indices.iter().any(|&index| index >= rest.len()) {
                return Err("GLB 的三角面索引无效".into());
            }
            faces.extend(
                indices
                    .chunks_exact(3)
                    .map(|f| [offset + f[0], offset + f[1], offset + f[2]]),
            );
        }
    }
    let missing: Vec<_> = names.difference(&matched).copied().collect();
    if !missing.is_empty() {
        return Err(format!("活动场景找不到网格：{}", missing.join("、")));
    }
    if points.is_empty() || faces.is_empty() {
        return Err("所选网格没有有效三角面".into());
    }
    Ok(Surface {
        points,
        faces,
        joints,
        skinned_vertices,
    })
}

impl Surface {
    pub fn summary(&self) -> AvatarSummary<'_> {
        let mut lo = self.points[0];
        let mut hi = lo;
        for point in &self.points {
            for (axis, &value) in point.iter().enumerate() {
                lo[axis] = lo[axis].min(value);
                hi[axis] = hi[axis].max(value);
            }
        }
        AvatarSummary {
            vertices: self.points.len(),
            triangles: self.faces.len(),
            skinned_vertices: self.skinned_vertices,
            bounds_mm: [lo, hi],
            height_mm: hi[1] - lo[1],
            joints: &self.joints,
        }
    }

    pub fn write_obj(&self, path: &str) -> std::io::Result<()> {
        let mut out = BufWriter::new(File::create(path)?);
        writeln!(out, "# Pose baked by CLO_RUST. Coordinates in mm. Y up.")?;
        for [x, y, z] in &self.points {
            writeln!(out, "v {x} {y} {z}")?;
        }
        for [a, b, c] in &self.faces {
            writeln!(out, "f {} {} {}", a + 1, b + 1, c + 1)?;
        }
        out.flush()
    }
}
