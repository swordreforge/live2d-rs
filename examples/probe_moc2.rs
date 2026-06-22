fn main() {
    let path = std::env::args().nth(1).expect("usage: probe <moc-file>");
    let data = std::fs::read(&path).expect("read file");
    println!("File size: {} bytes", data.len());
    println!("Magic: {:02x} {:02x} {:02x}", data[0], data[1], data[2]);
    println!("Version: {}", data[3]);

    match live2d_core::moc2::parse_moc2(&data) {
        Ok(moc2) => {
            println!("Parse OK!");
            println!("  canvas: {}x{}", moc2.canvas_width, moc2.canvas_height);
            println!("  params: {} parts: {} drawables: {}",
                moc2.param_defs.len(), moc2.parts.len(), moc2.drawables.len());
            println!("  deformers: {} pivot_managers: {} param_pivots: {}",
                moc2.deformers.len(), moc2.pivot_managers.len(), moc2.param_pivots.len());
            for (i, d) in moc2.drawables.iter().enumerate() {
                println!("  drawable[{}]: id={:?} verts={} indices={} tex={}",
                    i, d.id, d.vertex_count, d.index_array.len(), d.texture_no);
                if d.vertex_count > 0 {
                    let n = std::cmp::min(6, d.pivot_points.len());
                    println!("    first pivot_points[{}]: {:?}", n, &d.pivot_points[..n]);
                }
            }
        }
        Err(e) => println!("Parse FAILED: {:?}", e),
    }
}
