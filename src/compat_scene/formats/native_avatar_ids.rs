//! Keep the native identities of co-located AVT instances distinct.

use super::native_pose::{
    encode_map, encoded_list, map_list, Fields, BYTE_TYPE, LIST_TYPE, MAP_TYPE,
};
use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

const UINT: u8 = 5;
const ARRAY: u8 = 21;
const MAX_DEPTH: usize = 64;
const MAX_RECORDS: usize = 65_536;

fn integer(data: &[u8]) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        data.try_into()
            .map_err(|_| "invalid native avatar ID width")?,
    ))
}

fn visit(record: &Fields<'_>, depth: usize, ids: &mut BTreeSet<u32>) -> Result<(), String> {
    if depth == 0 {
        return Err("native avatar scene graph is too deeply nested".into());
    }
    for (key, kind, data) in record.fields() {
        match kind {
            UINT if key == b"uiID" => {
                let id = integer(data)?;
                if id != 0 && id != u32::MAX {
                    ids.insert(id);
                }
            }
            MAP_TYPE => visit(&Fields::read(data)?, depth - 1, ids)?,
            LIST_TYPE if data.get(4) == Some(&MAP_TYPE) => {
                for child in map_list(data, MAX_RECORDS)? {
                    visit(&child, depth - 1, ids)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn uuid(value: &[u8], salt: usize) -> Vec<u8> {
    let hash = |half| {
        let mut hasher = DefaultHasher::new();
        (value, salt, half).hash(&mut hasher);
        hasher.finish()
    };
    let value = (u128::from(hash(false)) << 64) | u128::from(hash(true));
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        value >> 96,
        (value >> 80) & 0xffff,
        (value >> 64) & 0xfff | 0x4000,
        (value >> 48) & 0x3fff | 0x8000,
        value & 0xffffffffffff,
    )
    .into_bytes()
}

fn remap_array(data: &[u8], ids: &HashMap<u32, u32>) -> Result<Vec<u8>, String> {
    if data == [0; 4] {
        return Ok(data.to_vec());
    }
    let count: usize = data
        .get(..4)
        .ok_or("missing native avatar reference count")
        .and_then(|b| {
            u32::from_le_bytes(b.try_into().map_err(|_| "invalid native reference count")?)
                .try_into()
                .map_err(|_| "native reference count is too large")
        })?;
    if !matches!(data.get(4), Some(4 | 5)) || data.len() != 5 + count * 4 {
        return Err("invalid native avatar joint ID array".into());
    }
    let mut result = data[..5].to_vec();
    for value in data[5..].chunks_exact(4) {
        let id = integer(value)?;
        result.extend_from_slice(&ids.get(&id).copied().unwrap_or(id).to_le_bytes());
    }
    Ok(result)
}

fn rewrite(
    record: &Fields<'_>,
    ids: &HashMap<u32, u32>,
    salt: usize,
    depth: usize,
) -> Result<Vec<u8>, String> {
    if depth == 0 {
        return Err("native avatar scene graph is too deeply nested".into());
    }
    let fields = record
        .fields()
        .map(|(key, kind, data)| {
            let bytes = match kind {
                MAP_TYPE => Cow::Owned(rewrite(&Fields::read(data)?, ids, salt, depth - 1)?),
                LIST_TYPE if data.get(4) == Some(&MAP_TYPE) => {
                    let children = map_list(data, MAX_RECORDS)?
                        .iter()
                        .map(|child| rewrite(child, ids, salt, depth - 1).map(Cow::Owned))
                        .collect::<Result<Vec<_>, _>>()?;
                    Cow::Owned(encoded_list(&children)?)
                }
                UINT if key.starts_with(b"ui") && key.ends_with(b"ID") => {
                    let id = integer(data)?;
                    match ids.get(&id) {
                        Some(new) => Cow::Owned(new.to_le_bytes().to_vec()),
                        None => Cow::Borrowed(data),
                    }
                }
                ARRAY if matches!(key, b"arrJointIndexList" | b"arrRootJointIndexList") => {
                    Cow::Owned(remap_array(data, ids)?)
                }
                BYTE_TYPE if matches!(key, b"qsUuid" | b"qsUuidUTF8") && data.len() == 36 => {
                    Cow::Owned(uuid(data, salt))
                }
                _ => Cow::Borrowed(data),
            };
            Ok((key, kind, bytes))
        })
        .collect::<Result<Vec<_>, String>>()?;
    encode_map(
        &fields
            .iter()
            .map(|(key, kind, value)| (*key, *kind, value.as_ref()))
            .collect::<Vec<_>>(),
    )
}

#[derive(Default)]
pub(super) struct AvatarIdentities {
    ids: BTreeSet<u32>,
    uuids: HashSet<Vec<u8>>,
}

impl AvatarIdentities {
    pub(super) fn separate(&mut self, dan: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let root = Fields::read(dan)?;
        let mut ids = BTreeSet::new();
        visit(&root, MAX_DEPTH, &mut ids)?;
        let original_uuid = root.get(b"qsUuidUTF8", BYTE_TYPE).unwrap_or_default();
        let uuid_collision = !original_uuid.is_empty() && self.uuids.contains(original_uuid);
        if self.ids.is_disjoint(&ids) && !uuid_collision {
            self.ids.extend(ids);
            self.uuids.insert(original_uuid.to_vec());
            return Ok(None);
        }
        let first = self.ids.last().copied().unwrap_or(0);
        let ids = ids
            .into_iter()
            .enumerate()
            .map(|(slot, old)| {
                let new = first
                    .checked_add(slot as u32 + 1)
                    .filter(|&id| id != u32::MAX)
                    .ok_or("native avatar ID space exhausted")?;
                Ok((old, new))
            })
            .collect::<Result<HashMap<_, _>, String>>()?;
        let mut salt = self.uuids.len();
        while self.uuids.contains(&uuid(original_uuid, salt)) {
            salt += 1;
        }
        let updated = rewrite(&root, &ids, salt, MAX_DEPTH)?;
        self.ids.extend(ids.into_values());
        self.uuids.insert(uuid(original_uuid, salt));
        Ok(Some(updated))
    }
}
