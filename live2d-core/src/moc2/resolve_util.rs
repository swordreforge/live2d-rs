//! Helper functions for `resolve.rs`.

use std::collections::HashMap;

use moc_parser::{Blob, ObjIndex, Registry};

use super::types::Id;

pub(crate) fn read_tag(data: &[u8], pos: &mut usize) -> u8 {
    let tag = data[*pos];
    *pos += 1;
    tag
}

pub(crate) fn read_u32(data: &[u8], pos: &mut usize) -> u32 {
    let v = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    v
}

pub(crate) fn read_i32(data: &[u8], pos: &mut usize) -> i32 {
    let v = i32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    v
}

pub(crate) fn read_f32(data: &[u8], pos: &mut usize) -> f32 {
    let v = f32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    v
}

pub(crate) fn read_u8(data: &[u8], pos: &mut usize) -> u8 {
    let v = data[*pos];
    *pos += 1;
    v
}

pub(crate) fn resolve_id(idx: ObjIndex, ids: &HashMap<ObjIndex, Id>) -> Id {
    ids.get(&idx).cloned().unwrap_or_else(|| format!("<unresolved id #{idx}>").into())
}

pub(crate) fn resolve_index(idx: ObjIndex, map: &HashMap<ObjIndex, usize>) -> Option<usize> {
    map.get(&idx).copied()
}

pub(crate) fn resolve_obj_array(
    idx: ObjIndex,
    registry: &Registry,
    map: &HashMap<ObjIndex, usize>,
) -> Vec<usize> {
    let mut result = Vec::new();
    if let Ok(Blob::ObjArray(refs)) = registry.get(idx) {
        for &r in refs.iter() {
            let target = resolve_ref(r, registry);
            if let Some(&vi) = map.get(&target) {
                result.push(vi);
            }
        }
    }
    result
}

pub(crate) fn resolve_ref(idx: ObjIndex, registry: &Registry) -> ObjIndex {
    if let Ok(Blob::UnresolvedRef(target)) = registry.get(idx) {
        resolve_ref(*target, registry)
    } else {
        idx
    }
}

pub(crate) fn resolve_f32_array(idx: ObjIndex, registry: &Registry) -> Vec<f32> {
    let resolved = resolve_ref(idx, registry);
    if let Ok(Blob::F32Array(arr)) = registry.get(resolved) {
        arr.to_vec()
    } else {
        Vec::new()
    }
}

pub(crate) fn resolve_i32_array(idx: ObjIndex, registry: &Registry) -> Vec<i32> {
    let resolved = resolve_ref(idx, registry);
    if let Ok(Blob::I32Array(arr)) = registry.get(resolved) {
        arr.to_vec()
    } else {
        Vec::new()
    }
}

pub(crate) fn resolve_u16_array(idx: ObjIndex, registry: &Registry) -> Vec<u16> {
    let resolved = resolve_ref(idx, registry);
    if let Ok(Blob::I32Array(arr)) = registry.get(resolved) {
        arr.iter().map(|&v| v as u16).collect()
    } else {
        Vec::new()
    }
}
