use glow::*;


pub struct Mesh {
    vao: NativeVertexArray,
    vbo: NativeBuffer,
    ebo: NativeBuffer,
    pub vertex_count: i32,
    pub index_count: i32,
}

impl Mesh {
    pub unsafe fn new(gl: &Context) -> Result<Self, String> {
        let vao = gl.create_vertex_array().map_err(|e| format!("create vao: {:?}", e))?;
        let vbo = gl.create_buffer().map_err(|e| format!("create vbo: {:?}", e))?;
        let ebo = gl.create_buffer().map_err(|e| format!("create ebo: {:?}", e))?;
        Ok(Self { vao, vbo, ebo, vertex_count: 0, index_count: 0 })
    }

    pub unsafe fn upload(&mut self, gl: &Context, vertices: &[f32], indices: &[u16]) {
        self.vertex_count = (vertices.len() / 4) as i32;
        self.index_count = indices.len() as i32;

        gl.bind_vertex_array(Some(self.vao));

        gl.bind_buffer(ARRAY_BUFFER, Some(self.vbo));
        let vert_bytes = std::slice::from_raw_parts(
            vertices.as_ptr() as *const u8,
            vertices.len() * 4,
        );
        gl.buffer_data_u8_slice(ARRAY_BUFFER, vert_bytes, DYNAMIC_DRAW);

        gl.vertex_attrib_pointer_f32(0, 2, FLOAT, false, 16, 0);
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(1, 2, FLOAT, false, 16, 8);
        gl.enable_vertex_attrib_array(1);

        gl.bind_buffer(ELEMENT_ARRAY_BUFFER, Some(self.ebo));
        let idx_bytes = std::slice::from_raw_parts(
            indices.as_ptr() as *const u8,
            indices.len() * 2,
        );
        gl.buffer_data_u8_slice(ELEMENT_ARRAY_BUFFER, idx_bytes, DYNAMIC_DRAW);

        gl.bind_vertex_array(None);
    }

    pub unsafe fn draw(&self, gl: &Context) {
        gl.bind_vertex_array(Some(self.vao));
        gl.draw_elements(TRIANGLES, self.index_count, UNSIGNED_SHORT, 0);
        gl.bind_vertex_array(None);
    }
}

impl Drop for Mesh {
    fn drop(&mut self) {
        // NOTE: requires active GL context.  Caller must ensure.
    }
}
