pub mod mask_fbo;
pub mod mesh;
pub mod shader;

use glow::*;
use glow::HasContext as _;
use anyhow::Result;
use live2d_core::Model;
use mesh::Mesh;

pub struct Live2dRenderer {
    program: NativeProgram,
    mask_program: NativeProgram,
    pub textures: Vec<NativeTexture>,
    draw_mesh: Mesh,
    pub mask_fbo: Option<mask_fbo::MaskFbo>,
}

impl Live2dRenderer {
    pub unsafe fn new(gl: &Context) -> Result<Self> {
        let program = shader::compile_program(gl, shader::VERT_SRC, shader::FRAG_SRC)?;
        let mask_program = shader::compile_program(gl, shader::MASK_VERT_SRC, shader::MASK_FRAG_SRC)?;
        let draw_mesh = Mesh::new(gl).map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(Self {
            program,
            mask_program,
            textures: Vec::new(),
            draw_mesh,
            mask_fbo: None,
        })
    }

    pub unsafe fn render(&mut self, gl: &Context, model: &mut Model, camera: &crate::camera::Camera) {
        model.update();

        let drawables = model.drawables();
        let n = drawables.len();
        if n == 0 { return; }

        let tex_indices = drawables.texture_indices();
        let opacities = drawables.opacities();
        let vert_counts = drawables.vertex_counts();
        let vert_positions = drawables.vertex_positions();
        let vert_uvs = drawables.vertex_uvs();
        let idx_counts = drawables.index_counts();
        let idx_data = drawables.indices();
        let mul_colors = drawables.multiply_colors();
        let scr_colors = drawables.screen_colors();
        let blend_modes = drawables.blend_modes();

        let scale_loc = gl.get_uniform_location(self.program, "uScale");
        let translate_loc = gl.get_uniform_location(self.program, "uTranslate");
        let tex_loc = gl.get_uniform_location(self.program, "uTexture");
        let mul_loc = gl.get_uniform_location(self.program, "uMultiplyColor");
        let scr_loc = gl.get_uniform_location(self.program, "uScreenColor");
        let opacity_loc = gl.get_uniform_location(self.program, "uOpacity");

        gl.use_program(Some(self.program));
        gl.uniform_2_f32(scale_loc.as_ref(), camera.scale_x, -camera.scale_y);
        gl.uniform_2_f32(translate_loc.as_ref(), camera.translate_x, camera.translate_y);

        gl.enable(BLEND);
        gl.blend_func_separate(ONE, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA);

        for i in 0..n {
            let tex_idx = tex_indices[i];
            let opacity = opacities[i];
            if opacity < 0.001 { continue; }

            let tex = if tex_idx >= 0 && (tex_idx as usize) < self.textures.len() {
                self.textures[tex_idx as usize]
            } else {
                continue;
            };
            gl.active_texture(TEXTURE0);
            gl.bind_texture(TEXTURE_2D, Some(tex));
            gl.uniform_1_i32(tex_loc.as_ref(), 0);

            let blend = blend_modes[i];
            match blend {
                0 | 1 => {
                    gl.blend_func_separate(ONE, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA);
                }
                2 => {
                    gl.blend_func_separate(DST_COLOR, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA);
                }
                3..=17 => {
                    gl.blend_func_separate(ONE, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA);
                }
                _ => {
                    gl.blend_func_separate(ONE, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA);
                }
            }

            let mc = mul_colors[i];
            gl.uniform_4_f32(mul_loc.as_ref(), mc.X, mc.Y, mc.Z, mc.W);
            let sc = scr_colors[i];
            gl.uniform_4_f32(scr_loc.as_ref(), sc.X, sc.Y, sc.Z, sc.W);
            gl.uniform_1_f32(opacity_loc.as_ref(), opacity);

            let vc = vert_counts[i] as usize;
            let ic = idx_counts[i] as usize;
            if vc == 0 || ic == 0 { continue; }

            let pos_slice = std::slice::from_raw_parts(vert_positions[i], vc);
            let uv_slice = std::slice::from_raw_parts(vert_uvs[i], vc);
            let idx_slice = std::slice::from_raw_parts(idx_data[i], ic);

            let mut verts = Vec::with_capacity(vc * 4);
            for j in 0..vc {
                verts.push(pos_slice[j].X);
                verts.push(-pos_slice[j].Y);
                verts.push(uv_slice[j].X);
                verts.push(uv_slice[j].Y);
            }
            self.draw_mesh.upload(gl, &verts, idx_slice);
            self.draw_mesh.draw(gl);
        }

        gl.disable(BLEND);
        gl.use_program(None);
    }
}
