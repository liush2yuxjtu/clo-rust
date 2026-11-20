use super::{array, vec_mut};
use serde_json::{json, Value};
use std::collections::HashSet;

fn from_segments(group: &Value) -> impl Iterator<Item = &Value> {
    array(group, "segments")
        .iter()
        .flat_map(|pair| pair.as_array().into_iter().flatten())
}

fn members(group: &Value) -> Vec<&Value> {
    from_segments(group)
        .map(|endpoint| &endpoint["pattern_id"])
        .collect()
}

fn user_seams(state: &Value) -> impl Iterator<Item = &Value> {
    array(state, "seams")
        .iter()
        .filter(|seam| seam.get("native_group").is_none())
}

pub(crate) fn seam_group_names(state: &Value) -> impl Iterator<Item = &Value> {
    array(state, "native_seam_groups")
        .iter()
        .chain(user_seams(state))
        .map(|group| &group["name"])
}

pub(super) fn count(state: &Value) -> usize {
    array(state, "native_seam_groups").len() + user_seams(state).count()
}

pub(super) fn indices(state: &Value, id: &Value) -> Vec<usize> {
    let native = array(state, "native_seam_groups");
    let first = native
        .iter()
        .enumerate()
        .filter_map(|(i, group)| members(group).contains(&id).then_some(i));
    let direct = user_seams(state).enumerate().filter_map(|(i, seam)| {
        (&seam["pattern_a"] == id || &seam["pattern_b"] == id).then_some(native.len() + i)
    });
    first.chain(direct).collect()
}

pub(super) fn list(state: &Value) -> Value {
    let names = |ids: &[&Value]| -> Vec<&Value> {
        array(state, "patterns")
            .iter()
            .filter(|pattern| ids.contains(&&pattern["id"]))
            .map(|p| &p["name"])
            .collect()
    };
    let native = array(state, "native_seam_groups");
    let descriptions = native.iter().enumerate().map(|(i, group)| {
        json!({"index": i, "name": group["name"], "patterns": names(&members(group)),
            "native_segments": group["segments"], "restored": group["restored"], "active": group["active"]})
    });
    let direct = user_seams(state).enumerate().map(|(i, seam)| {
        json!({"index": native.len() + i, "name": seam["name"], "patterns": names(&[&seam["pattern_a"], &seam["pattern_b"]])})
    });
    let groups: Vec<_> = descriptions.chain(direct).collect();
    json!({"count": groups.len(), "groups": groups})
}

pub(super) fn remove_metadata_for(state: &mut Value, invalid: &[Value]) {
    let removed: Vec<_> = array(state, "native_seam_groups")
        .iter()
        .filter_map(|group| {
            members(group)
                .into_iter()
                .any(|id| invalid.contains(id))
                .then_some(group["id"].clone())
        })
        .collect();
    if removed.is_empty() {
        return;
    }
    vec_mut(state, "native_seam_groups").retain(|group| !removed.contains(&group["id"]));
    for seam in vec_mut(state, "seams") {
        if removed.contains(&seam["native_group"]) {
            if let Some(record) = seam.as_object_mut() {
                record.remove("native_group");
            }
        }
    }
}

pub(super) fn remap_outline(state: &mut Value, id: &Value, lines: &[Option<usize>]) {
    let mut removed = HashSet::new();
    vec_mut(state, "native_seam_groups").retain_mut(|group| {
        let intact = group["segments"].as_array_mut().unwrap().iter_mut()
            .flat_map(|pair| pair.as_array_mut().into_iter().flatten())
            .all(|endpoint| {
                if endpoint["pattern_id"] != *id || endpoint["shape_depth"] != 1 {
                    return true;
                }
                remap_span(endpoint, lines).is_some()
            });
        if !intact {
            removed.insert(group["id"].clone());
        }
        intact
    });
    vec_mut(state, "seams").retain(|seam| !removed.contains(&seam["native_group"]));
}

fn remap_span(endpoint: &mut Value, lines: &[Option<usize>]) -> Option<()> {
    let start = usize::try_from(endpoint["start_line"].as_u64()?).ok()?;
    let end = usize::try_from(endpoint["end_line"].as_u64()?).ok()?;
    let forward = endpoint["direction"].as_bool()?;
    let start_new = lines.get(start).copied().flatten()?;
    let end_new = lines.get(end).copied().flatten()?;
    // A spanning seam also uses every intermediate edge, including across
    // the original outline's last-to-first junction.
    let mut line = start;
    while line != end {
        line = if forward {
            (line + 1) % lines.len()
        } else {
            line.checked_sub(1).unwrap_or(lines.len() - 1)
        };
        lines[line]?;
    }
    endpoint["start_line"] = json!(start_new);
    endpoint["end_line"] = json!(end_new);
    Some(())
}

pub(super) fn validate(content: &Value, patterns: &[Value], seams: &[Value]) -> Result<(), String> {
    let Some(native) = content.get("native_seam_groups") else {
        if seams.iter().any(|seam| seam.get("native_group").is_some()) {
            return Err("editable seam is missing its native group metadata".into());
        }
        return Ok(());
    };
    let groups = native
        .as_array()
        .ok_or("native_seam_groups must be an array")?;
    let mut ids = HashSet::new();
    let expected: HashSet<_> = groups
        .iter()
        .filter(|group| group["restored"] == true)
        .filter_map(|group| group["id"].as_u64())
        .collect();
    for group in groups {
        let id = group["id"]
            .as_u64()
            .ok_or("native seam group ID must be an unsigned integer")?;
        if !ids.insert(id)
            || group["name"].as_str().is_none()
            || group["restored"].as_bool().is_none()
        {
            return Err("invalid native seam group metadata".into());
        }
        let segments = group["segments"]
            .as_array()
            .ok_or("native seam segments must be an array")?;
        if segments.is_empty() {
            return Err("native seam group has no segments".into());
        }
        for segment in segments {
            let endpoints = segment
                .as_array()
                .filter(|record| record.len() == 2)
                .ok_or("native seam segment must have two endpoints")?;
            for endpoint in endpoints {
                if !patterns
                    .iter()
                    .any(|pattern| pattern["id"] == endpoint["pattern_id"])
                {
                    return Err("native seam endpoint refers to an unknown pattern".into());
                }
            }
        }
    }
    let mut actual = HashSet::new();
    for seam in seams
        .iter()
        .filter(|seam| seam.get("native_group").is_some())
    {
        let id = seam["native_group"]
            .as_u64()
            .ok_or("invalid editable native seam group ID")?;
        if !expected.contains(&id) || !actual.insert(id) {
            return Err("editable seam does not match native group metadata".into());
        }
    }
    if expected != actual {
        return Err("native group is missing its editable seam".into());
    }
    Ok(())
}

pub(super) fn remap<'a, F>(
    state: &Value,
    source: impl Iterator<Item = &'a Value>,
    map_id: F,
) -> Result<(Vec<Value>, std::collections::HashMap<u64, u64>), String>
where
    F: Fn(&Value) -> Result<Value, String>,
{
    let last = array(state, "native_seam_groups")
        .iter()
        .filter_map(|group| group["id"].as_u64())
        .max()
        .unwrap_or(0);
    let mut converted = Vec::new();
    let mut mapping = std::collections::HashMap::new();
    for (i, mut group) in source.cloned().enumerate() {
        let id = last
            .checked_add(i as u64 + 1)
            .ok_or("native sewing group ID limit exceeded")?;
        let old = group["id"].as_u64().ok_or("invalid native seam group ID")?;
        group["id"] = json!(id);
        let segments = group["segments"]
            .as_array_mut()
            .ok_or("invalid native seam segments")?;
        for pair in segments {
            for endpoint in pair.as_array_mut().ok_or("invalid native seam segment")? {
                endpoint["pattern_id"] = map_id(&endpoint["pattern_id"])?;
            }
        }
        mapping.insert(old, id);
        converted.push(group);
    }
    Ok((converted, mapping))
}

pub(super) fn duplicate_sewing(state: &mut Value, old: &Value, new: &Value) -> Result<(), String> {
    let map_panel = |id: &Value| Ok(if id == old { new.clone() } else { id.clone() });
    let groups = array(state, "native_seam_groups").iter()
        .filter(|group| members(group).contains(&old));
    let (groups, mapping) = remap(state, groups, map_panel)?;
    let joins = array(state, "seams").iter()
        .filter(|seam| seam["pattern_a"] == *old || seam["pattern_b"] == *old)
        .cloned()
        .map(|mut seam| {
            for key in ["pattern_a", "pattern_b"] {
                seam[key] = map_panel(&seam[key])?;
            }
            if let Some(id) = seam["native_group"].as_u64() {
                seam["native_group"] = json!(mapping.get(&id).ok_or("missing mirrored native seam group")?);
            }
            Ok(seam)
        })
        .collect::<Result<Vec<_>, String>>()?;
    vec_mut(state, "native_seam_groups").extend(groups);
    vec_mut(state, "seams").extend(joins);
    Ok(())
}
