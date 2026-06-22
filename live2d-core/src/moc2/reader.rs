//! MOC2 domain-type byte-stream readers (Phase 1).
//!
//! Each `read_*` function reads one domain type from the MOC binary
//! stream, stores its field values as a `Blob::Opaque` entry in the
//! registry, and returns the entry's index.

use moc_parser::{BinaryReader, Blob, MocResult, ObjIndex, Registry};

use super::read_moc_object;

// ── Opaque binary layout helpers ───────────────────────────────────

fn pack_opaque(tag: u8, fields: &[u32]) -> Blob {
    let mut data = Vec::with_capacity(1 + fields.len() * 4);
    data.push(tag);
    for &f in fields {
        data.extend_from_slice(&f.to_le_bytes());
    }
    Blob::Opaque(data.into_boxed_slice())
}

// ── Known-type tag dispatch ────────────────────────────────────────

/// Dispatch function passed to `moc_parser::parse_moc`.
pub(crate) fn read_known_type(
    reader: &mut BinaryReader,
    registry: &mut Registry,
    tag: u8,
) -> MocResult<ObjIndex> {
    match tag {
        65 => read_warp_deformer(reader, registry),
        66 => read_pivot_manager(reader, registry),
        67 => read_param_pivots(reader, registry),
        68 => read_rotation_deformer(reader, registry),
        69 => read_affine_ent(reader, registry),
        70 => read_mesh(reader, registry),
        131 => read_param_def_float(reader, registry),
        133 => read_parts_data(reader, registry),
        136 => read_model_impl(reader, registry),
        137 => read_param_def_set(reader, registry),
        142 => read_avatar(reader, registry),
        50 | 51 | 60 | 134 => read_id(reader, registry),
        _ => Err(moc_parser::MocError::UnknownTag {
            offset: reader.offset(),
            tag,
        }),
    }
}

// ── Per-type readers ───────────────────────────────────────────────

fn read_id(reader: &mut BinaryReader, registry: &mut Registry) -> MocResult<ObjIndex> {
    let s = reader.read_string()?;
    Ok(registry.push(Blob::String(s.into())))
}

fn read_warp_deformer(reader: &mut BinaryReader, registry: &mut Registry) -> MocResult<ObjIndex> {
    let id = read_moc_object(reader, registry, read_known_type)?;
    let target = read_moc_object(reader, registry, read_known_type)?;
    let col = reader.read_i32()?;
    let row = reader.read_i32()?;
    let pmgr = read_moc_object(reader, registry, read_known_type)?;
    let ppts = read_moc_object(reader, registry, read_known_type)?;
    // Opacity: raw Float32Array (no type tag) in format v10+
    // Python: Deformer.readOpacity → br.readFloat32Array()
    let popac = if reader.version() >= 10 && reader.remaining() > 0 {
        let count = reader.read_vlq()? as usize;
        let mut vals = Vec::with_capacity(count);
        for _ in 0..count {
            vals.push(reader.read_f32()?);
        }
        registry.push(Blob::F32Array(vals.into()))
    } else {
        u32::MAX
    };
    Ok(registry.push(pack_opaque(65, &[id, target, col as u32, row as u32, pmgr, ppts, popac])))
}

fn read_rotation_deformer(reader: &mut BinaryReader, registry: &mut Registry) -> MocResult<ObjIndex> {
    let id = read_moc_object(reader, registry, read_known_type)?;
    let target = read_moc_object(reader, registry, read_known_type)?;
    let pmgr = read_moc_object(reader, registry, read_known_type)?;
    let affines = read_moc_object(reader, registry, read_known_type)?;
    // Opacity: raw Float32Array (no type tag) in format v10+
    let popac = if reader.version() >= 10 && reader.remaining() > 0 {
        let count = reader.read_vlq()? as usize;
        let mut vals = Vec::with_capacity(count);
        for _ in 0..count {
            vals.push(reader.read_f32()?);
        }
        registry.push(Blob::F32Array(vals.into()))
    } else {
        u32::MAX
    };
    Ok(registry.push(pack_opaque(68, &[id, target, pmgr, affines, popac])))
}

fn read_pivot_manager(reader: &mut BinaryReader, registry: &mut Registry) -> MocResult<ObjIndex> {
    let pivots = read_moc_object(reader, registry, read_known_type)?;
    Ok(registry.push(pack_opaque(66, &[pivots])))
}

fn read_param_pivots(reader: &mut BinaryReader, registry: &mut Registry) -> MocResult<ObjIndex> {
    let pid = read_moc_object(reader, registry, read_known_type)?;
    // pivotCount is a raw int32 (4 bytes BE), NOT a VLQ
    // Python: ParamPivots.read → self.pivotCount = br.readInt32()
    let count = reader.read_i32()?;
    let pvals = read_moc_object(reader, registry, read_known_type)?;
    Ok(registry.push(pack_opaque(67, &[pid, count as u32, pvals])))
}

fn read_affine_ent(reader: &mut BinaryReader, registry: &mut Registry) -> MocResult<ObjIndex> {
    let ox = reader.read_f32()?;
    let oy = reader.read_f32()?;
    let sx = reader.read_f32()?;
    let sy = reader.read_f32()?;
    let rot = reader.read_f32()?;
    let reflect_x = if reader.version() >= 10 { reader.read_bit()? } else { false };
    let reflect_y = if reader.version() >= 10 { reader.read_bit()? } else { false };
    reader.align_to_byte();

    let mut data = vec![69u8];
    data.extend_from_slice(&ox.to_bits().to_le_bytes());
    data.extend_from_slice(&oy.to_bits().to_le_bytes());
    data.extend_from_slice(&sx.to_bits().to_le_bytes());
    data.extend_from_slice(&sy.to_bits().to_le_bytes());
    data.extend_from_slice(&rot.to_bits().to_le_bytes());
    data.push(reflect_x as u8);
    data.push(reflect_y as u8);
    Ok(registry.push(Blob::Opaque(data.into_boxed_slice())))
}

fn read_mesh(reader: &mut BinaryReader, registry: &mut Registry) -> MocResult<ObjIndex> {
    let id = read_moc_object(reader, registry, read_known_type)?;
    let target = read_moc_object(reader, registry, read_known_type)?;
    let pmgr = read_moc_object(reader, registry, read_known_type)?;
    let avg_order = reader.read_i32()?;
    // pivotDrawOrders: raw Int32Array (no type tag)
    // Python: IDrawData.read → self.pivotDrawOrders = aH.readInt32Array()
    let porders_count = reader.read_vlq()? as usize;
    let mut porders_vals = Vec::with_capacity(porders_count);
    for _ in 0..porders_count {
        porders_vals.push(reader.read_i32()?);
    }
    let porders = registry.push(Blob::I32Array(porders_vals.into()));
    // pivotOpacities: raw Float32Array (no type tag)
    // Python: IDrawData.read → self.pivotOpacities = aH.readFloat32Array()
    let popac_count = reader.read_vlq()? as usize;
    let mut popac_vals = Vec::with_capacity(popac_count);
    for _ in 0..popac_count {
        popac_vals.push(reader.read_f32()?);
    }
    let popac = registry.push(Blob::F32Array(popac_vals.into()));
    // clipID: typed object (HAS a type tag) in version >= 11
    let clip = {
        if reader.version() >= 11 && reader.remaining() > 0 {
            read_moc_object(reader, registry, read_known_type)?
        } else {
            u32::MAX
        }
    };
    let tex_no = reader.read_i32()?;
    let _vcnt = reader.read_i32()?;
    let _pcnt = reader.read_i32()?;
    let idx_arr = read_moc_object(reader, registry, read_known_type)?;
    let ppts = read_moc_object(reader, registry, read_known_type)?;
    let uvs_arr = read_moc_object(reader, registry, read_known_type)?;

    let opt_flag = if reader.version() >= 8 { reader.read_i32()? } else { 0 };
    let color_comp = if opt_flag != 0 {
        match (opt_flag & 0x1E) >> 1 {
            1 => 1u8,
            2 => 2u8,
            _ => 0u8,
        }
    } else {
        0u8
    };
    let culling = if opt_flag != 0 { (opt_flag & 32) == 0 } else { true };

    let fields = &[
        id, target, pmgr, avg_order as u32, porders, popac, clip,
        tex_no as u32, idx_arr, ppts, uvs_arr, opt_flag as u32,
    ];
    let mut data = vec![70u8];
    for &f in fields {
        data.extend_from_slice(&f.to_le_bytes());
    }
    data.push(color_comp);
    data.push(culling as u8);
    Ok(registry.push(Blob::Opaque(data.into_boxed_slice())))
}

fn read_param_def_float(reader: &mut BinaryReader, registry: &mut Registry) -> MocResult<ObjIndex> {
    let min = reader.read_f32()?;
    let max = reader.read_f32()?;
    let def = reader.read_f32()?;
    let pid = read_moc_object(reader, registry, read_known_type)?;

    let mut data = vec![131u8];
    data.extend_from_slice(&min.to_bits().to_le_bytes());
    data.extend_from_slice(&max.to_bits().to_le_bytes());
    data.extend_from_slice(&def.to_bits().to_le_bytes());
    data.extend_from_slice(&pid.to_le_bytes());
    Ok(registry.push(Blob::Opaque(data.into_boxed_slice())))
}

fn read_param_def_set(reader: &mut BinaryReader, registry: &mut Registry) -> MocResult<ObjIndex> {
    let params = read_moc_object(reader, registry, read_known_type)?;
    Ok(registry.push(pack_opaque(137, &[params])))
}

fn read_parts_data(reader: &mut BinaryReader, registry: &mut Registry) -> MocResult<ObjIndex> {
    let locked = reader.read_bit()?;
    let visible = reader.read_bit()?;
    reader.align_to_byte();
    let id = read_moc_object(reader, registry, read_known_type)?;
    let def_list = read_moc_object(reader, registry, read_known_type)?;
    let draw_list = read_moc_object(reader, registry, read_known_type)?;

    let mut data = vec![133u8, locked as u8, visible as u8];
    data.extend_from_slice(&id.to_le_bytes());
    data.extend_from_slice(&def_list.to_le_bytes());
    data.extend_from_slice(&draw_list.to_le_bytes());
    Ok(registry.push(Blob::Opaque(data.into_boxed_slice())))
}

fn read_model_impl(reader: &mut BinaryReader, registry: &mut Registry) -> MocResult<ObjIndex> {
    let pdef_set = read_moc_object(reader, registry, read_known_type)?;
    let parts_list = read_moc_object(reader, registry, read_known_type)?;
    let cw = reader.read_i32()?;
    let ch = reader.read_i32()?;
    Ok(registry.push(pack_opaque(136, &[pdef_set, parts_list, cw as u32, ch as u32])))
}

fn read_avatar(reader: &mut BinaryReader, registry: &mut Registry) -> MocResult<ObjIndex> {
    let id = read_moc_object(reader, registry, read_known_type)?;
    // Binary stores def_list BEFORE draw_list
    let def_list = read_moc_object(reader, registry, read_known_type)?;
    let draw_list = read_moc_object(reader, registry, read_known_type)?;
    Ok(registry.push(pack_opaque(142, &[id, def_list, draw_list])))
}
