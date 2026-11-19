use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
pub struct Live {
    pieces: usize,
    seam_groups: usize,
    piece_groups: Vec<(String, Vec<usize>)>,
    garment_groups: Groups,
}

#[derive(Deserialize)]
struct Groups {
    groups: Vec<Group>,
}

#[derive(Deserialize)]
struct Group {
    #[serde(rename = "patternIndices")]
    pieces: Vec<usize>,
}

#[derive(Serialize)]
pub struct ResultSummary {
    pieces: usize,
    seam_groups: usize,
    single_piece_groups: usize,
    pair_groups: usize,
    components: Vec<Vec<usize>>,
    pub same_as_clo: bool,
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while i != parent[i] {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

pub fn check(live: &Live) -> Result<ResultSummary, String> {
    if live.piece_groups.len() != live.pieces {
        return Err("裁片数量与成员表不一致".into());
    }
    let mut names = BTreeSet::new();
    let mut memberships = vec![Vec::new(); live.seam_groups];
    for (piece, (name, groups)) in live.piece_groups.iter().enumerate() {
        if !names.insert(name.as_str()) {
            return Err(format!("裁片名重复：{name}"));
        }
        let mut seen = BTreeSet::new();
        for &group in groups {
            if group >= live.seam_groups || !seen.insert(group) {
                return Err(format!("缝合组无效或重复：{name} / {group}"));
            }
            memberships[group].push(piece);
        }
    }
    if memberships.iter().any(Vec::is_empty) {
        return Err("至少一组缝合没有对应裁片".into());
    }
    let mut parent: Vec<usize> = (0..live.pieces).collect();
    for list in &memberships {
        let (&a, rest) = list.split_first().unwrap();
        for &b in rest {
            let (x, y) = (root(&mut parent, a), root(&mut parent, b));
            parent[y] = x;
        }
    }
    let mut partitions = BTreeMap::<usize, Vec<usize>>::new();
    for i in 0..live.pieces {
        partitions.entry(root(&mut parent, i)).or_default().push(i);
    }
    let mut components: Vec<Vec<usize>> = partitions.into_values().collect();
    components.sort_unstable();
    let mut expected: Vec<Vec<usize>> = live
        .garment_groups
        .groups
        .iter()
        .map(|g| g.pieces.clone())
        .collect();
    expected.iter_mut().for_each(|set| set.sort_unstable());
    expected.sort_unstable();
    Ok(ResultSummary {
        pieces: live.pieces,
        seam_groups: live.seam_groups,
        single_piece_groups: memberships.iter().filter(|x| x.len() == 1).count(),
        pair_groups: memberships.iter().filter(|x| x.len() == 2).count(),
        same_as_clo: components == expected,
        components,
    })
}
