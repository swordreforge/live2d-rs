pub mod mask_fbo;
pub mod mesh;
pub mod shader;

use glow::*;
use anyhow::Result;
use live2d_core::Model;
use live2d_core_sys as ffi;
use mesh::Mesh;

pub struct Live2dRenderer {
    program: NativeProgram,
    mask_program: NativeProgram,
    masked_program: NativeProgram,
    pub textures: Vec<NativeTexture>,
    draw_mesh: Mesh,
    pub mask_fbo: Option<mask_fbo::MaskFbo>,
}

impl Live2dRenderer {
    pub unsafe fn new(gl: &Context) -> Result<Self> {
        let program = shader::compile_program(gl, shader::VERT_SRC, shader::FRAG_SRC)?;
        let mask_program = shader::compile_program(gl, shader::MASK_VERT_SRC, shader::MASK_FRAG_SRC)?;
        let masked_program = shader::compile_program(gl, shader::VERT_SRC, shader::FRAG_MASKED_SRC)?;
        let draw_mesh = Mesh::new(gl).map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(Self {
            program,
            mask_program,
            masked_program,
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

        let sorted_indices = drawables.render_order_indices();
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
        let mask_counts = drawables.mask_counts();
        let masks = drawables.masks();
        let constant_flags = drawables.constant_flags();
        let dynamic_flags = drawables.dynamic_flags();

        let _ = model.canvas_info(); // init internal lazy state

        let has_masks = mask_counts.iter().any(|&c| c > 0);

        if has_masks {
            let mut viewport = [0i32; 4];
            gl.get_parameter_i32_slice(VIEWPORT, &mut viewport);
            let screen_w = viewport[2];
            let screen_h = viewport[3];

            match &mut self.mask_fbo {
                Some(fbo) => fbo.resize(gl, screen_w, screen_h),
                None => {
                    self.mask_fbo = Some(mask_fbo::MaskFbo::new(gl, screen_w, screen_h)
                        .map_err(|e| anyhow::anyhow!("{}", e)).unwrap());
                }
            }
        }

        let scale_loc = gl.get_uniform_location(self.program, "uScale");
        let translate_loc = gl.get_uniform_location(self.program, "uTranslate");
        let tex_loc = gl.get_uniform_location(self.program, "uTexture");
        let mul_loc = gl.get_uniform_location(self.program, "uMultiplyColor");
        let scr_loc = gl.get_uniform_location(self.program, "uScreenColor");
        let opacity_loc = gl.get_uniform_location(self.program, "uOpacity");

        let m_scale_loc = gl.get_uniform_location(self.masked_program, "uScale");
        let m_translate_loc = gl.get_uniform_location(self.masked_program, "uTranslate");
        let m_tex_loc = gl.get_uniform_location(self.masked_program, "uTexture");
        let m_mask_tex_loc = gl.get_uniform_location(self.masked_program, "uMaskTexture");
        let m_mask_size_loc = gl.get_uniform_location(self.masked_program, "uMaskSize");
        let m_mul_loc = gl.get_uniform_location(self.masked_program, "uMultiplyColor");
        let m_scr_loc = gl.get_uniform_location(self.masked_program, "uScreenColor");
        let m_opacity_loc = gl.get_uniform_location(self.masked_program, "uOpacity");

        let mk_scale_loc = gl.get_uniform_location(self.mask_program, "uScale");
        let mk_translate_loc = gl.get_uniform_location(self.mask_program, "uTranslate");
        let mk_opacity_loc = gl.get_uniform_location(self.mask_program, "uOpacity");

        for &i in &sorted_indices {
            let opacity = opacities[i];
            if opacity < 0.001 { continue; }
            if dynamic_flags[i] & ffi::csmIsVisible as u8 == 0 { continue; }

            let mc = mul_colors[i];
            let sc = scr_colors[i];
            let vc = vert_counts[i] as usize;
            let ic = idx_counts[i] as usize;
            if vc == 0 || ic == 0 { continue; }

            let pos_slice = std::slice::from_raw_parts(vert_positions[i], vc);
            let uv_slice = std::slice::from_raw_parts(vert_uvs[i], vc);
            let idx_slice = std::slice::from_raw_parts(idx_data[i], ic);

            let mut verts = Vec::with_capacity(vc * 4);
            for j in 0..vc {
                verts.push(pos_slice[j].X);
                verts.push(pos_slice[j].Y);
                verts.push(uv_slice[j].X);
                verts.push(uv_slice[j].Y);
            }

            let n_masks = mask_counts[i];
            if n_masks > 0 {
                let mask_slice = std::slice::from_raw_parts(masks[i], n_masks as usize);

                let fbo = self.mask_fbo.as_ref().expect("mask_fbo must be created before masked render");
                let fbo_w = fbo.width;
                let fbo_h = fbo.height;
                let fbo_tex = fbo.texture;

                gl.bind_framebuffer(FRAMEBUFFER, Some(fbo.fbo));
                gl.viewport(0, 0, fbo_w, fbo_h);
                gl.clear_color(0.0, 0.0, 0.0, 0.0);
                gl.clear(COLOR_BUFFER_BIT);

                gl.use_program(Some(self.mask_program));
                gl.uniform_2_f32(mk_scale_loc.as_ref(), camera.scale_x, -camera.scale_y);
                gl.uniform_2_f32(mk_translate_loc.as_ref(), camera.translate_x, -camera.translate_y);

                gl.enable(BLEND);

                for &mask_idx in mask_slice {
                    let mi = mask_idx as usize;
                    if dynamic_flags[mi] & ffi::csmIsVisible as u8 == 0 { continue; }
                    let m_vc = vert_counts[mi] as usize;
                    let m_ic = idx_counts[mi] as usize;
                    if m_vc == 0 || m_ic == 0 { continue; }

                    let is_inverted = constant_flags[mi] & ffi::csmIsInvertedMask as u8 != 0;
                    if is_inverted {
                        // Inverted mask: zeros out alpha in the shape area (clips this region)
                        gl.blend_func_separate(ZERO, ONE_MINUS_SRC_ALPHA, ZERO, ONE_MINUS_SRC_ALPHA);
                    } else {
                        // Normal mask: sets alpha=1 in the shape area (marks as visible)
                        gl.blend_func_separate(ONE, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA);
                    }

                    let m_opacity = opacities[mi];
                    gl.uniform_1_f32(mk_opacity_loc.as_ref(), m_opacity);

                    let m_pos = std::slice::from_raw_parts(vert_positions[mi], m_vc);
                    let m_uv = std::slice::from_raw_parts(vert_uvs[mi], m_vc);
                    let m_idx = std::slice::from_raw_parts(idx_data[mi], m_ic);

                    let mut m_verts = Vec::with_capacity(m_vc * 4);
                    for j in 0..m_vc {
                        m_verts.push(m_pos[j].X);
                        m_verts.push(m_pos[j].Y);
                        m_verts.push(m_uv[j].X);
                        m_verts.push(m_uv[j].Y);
                    }
                    self.draw_mesh.upload(gl, &m_verts, m_idx);
                    self.draw_mesh.draw(gl);
                }

                gl.disable(BLEND);
                gl.bind_framebuffer(FRAMEBUFFER, None);

                let mut viewport = [0i32; 4];
                gl.get_parameter_i32_slice(VIEWPORT, &mut viewport);
                gl.viewport(viewport[0], viewport[1], viewport[2], viewport[3]);

                gl.use_program(Some(self.masked_program));
                gl.uniform_2_f32(m_scale_loc.as_ref(), camera.scale_x, -camera.scale_y);
                gl.uniform_2_f32(m_translate_loc.as_ref(), camera.translate_x, -camera.translate_y);

                let tex_idx = tex_indices[i];
                let tex = if tex_idx >= 0 && (tex_idx as usize) < self.textures.len() {
                    self.textures[tex_idx as usize]
                } else {
                    continue;
                };
                gl.active_texture(TEXTURE0);
                gl.bind_texture(TEXTURE_2D, Some(tex));
                gl.uniform_1_i32(m_tex_loc.as_ref(), 0);

                gl.active_texture(TEXTURE1);
                gl.bind_texture(TEXTURE_2D, Some(fbo_tex));
                gl.uniform_1_i32(m_mask_tex_loc.as_ref(), 1);
                gl.uniform_2_f32(m_mask_size_loc.as_ref(), fbo_w as f32, fbo_h as f32);

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
                gl.enable(BLEND);

                gl.uniform_4_f32(m_mul_loc.as_ref(), mc.X, mc.Y, mc.Z, mc.W);
                gl.uniform_4_f32(m_scr_loc.as_ref(), sc.X, sc.Y, sc.Z, sc.W);
                gl.uniform_1_f32(m_opacity_loc.as_ref(), opacity);

                self.draw_mesh.upload(gl, &verts, idx_slice);
                self.draw_mesh.draw(gl);

                gl.active_texture(TEXTURE1);
                gl.bind_texture(TEXTURE_2D, None);
                gl.active_texture(TEXTURE0);
            } else {
                gl.use_program(Some(self.program));
                gl.uniform_2_f32(scale_loc.as_ref(), camera.scale_x, -camera.scale_y);
                gl.uniform_2_f32(translate_loc.as_ref(), camera.translate_x, -camera.translate_y);

                let tex_idx = tex_indices[i];
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
                gl.enable(BLEND);

                gl.uniform_4_f32(mul_loc.as_ref(), mc.X, mc.Y, mc.Z, mc.W);
                gl.uniform_4_f32(scr_loc.as_ref(), sc.X, sc.Y, sc.Z, sc.W);
                gl.uniform_1_f32(opacity_loc.as_ref(), opacity);

                self.draw_mesh.upload(gl, &verts, idx_slice);
                self.draw_mesh.draw(gl);
            }
        }

        gl.disable(BLEND);
        gl.use_program(None);
    }

    #[allow(dead_code)]
    pub unsafe fn render_masked(
        &mut self,
        gl: &Context,
        model: &mut Model,
        camera: &crate::camera::Camera,
        drawable_idx: usize,
        mask_indices: &[i32],
    ) {
        model.update();

        let drawables = model.drawables();
        let opacity = drawables.opacities()[drawable_idx];
        if opacity < 0.001 { return; }

        let vert_counts = drawables.vertex_counts();
        let vert_positions = drawables.vertex_positions();
        let vert_uvs = drawables.vertex_uvs();
        let idx_counts = drawables.index_counts();
        let idx_data = drawables.indices();
        let mul_colors = drawables.multiply_colors();
        let scr_colors = drawables.screen_colors();
        let blend_modes = drawables.blend_modes();
        let tex_indices = drawables.texture_indices();
        let constant_flags = drawables.constant_flags();
        let dynamic_flags = drawables.dynamic_flags();

        let vc = vert_counts[drawable_idx] as usize;
        let ic = idx_counts[drawable_idx] as usize;
        if vc == 0 || ic == 0 { return; }

        let mut viewport = [0i32; 4];
        gl.get_parameter_i32_slice(VIEWPORT, &mut viewport);
        let screen_w = viewport[2];
        let screen_h = viewport[3];

        match &mut self.mask_fbo {
            Some(fbo) => fbo.resize(gl, screen_w, screen_h),
            None => {
                self.mask_fbo = Some(mask_fbo::MaskFbo::new(gl, screen_w, screen_h)
                    .map_err(|e| anyhow::anyhow!("{}", e)).unwrap());
            }
        }

        let fbo = self.mask_fbo.as_ref().unwrap();
        let fbo_w = fbo.width;
        let fbo_h = fbo.height;
        let fbo_tex = fbo.texture;

        gl.bind_framebuffer(FRAMEBUFFER, Some(fbo.fbo));
        gl.viewport(0, 0, fbo_w, fbo_h);
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(COLOR_BUFFER_BIT);

        let mk_scale_loc = gl.get_uniform_location(self.mask_program, "uScale");
        let mk_translate_loc = gl.get_uniform_location(self.mask_program, "uTranslate");
        let mk_opacity_loc = gl.get_uniform_location(self.mask_program, "uOpacity");

        gl.use_program(Some(self.mask_program));
        gl.uniform_2_f32(mk_scale_loc.as_ref(), camera.scale_x, -camera.scale_y);
        gl.uniform_2_f32(mk_translate_loc.as_ref(), camera.translate_x, -camera.translate_y);

        gl.enable(BLEND);

        for &mask_idx in mask_indices {
            let mi = mask_idx as usize;
            if dynamic_flags[mi] & ffi::csmIsVisible as u8 == 0 { continue; }
            let m_vc = vert_counts[mi] as usize;
            let m_ic = idx_counts[mi] as usize;
            if m_vc == 0 || m_ic == 0 { continue; }

            let is_inverted = constant_flags[mi] & ffi::csmIsInvertedMask as u8 != 0;
            if is_inverted {
                gl.blend_func_separate(ZERO, ONE_MINUS_SRC_ALPHA, ZERO, ONE_MINUS_SRC_ALPHA);
            } else {
                gl.blend_func_separate(ONE, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA);
            }

            let m_opacity = drawables.opacities()[mi];
            gl.uniform_1_f32(mk_opacity_loc.as_ref(), m_opacity);

            let m_pos = std::slice::from_raw_parts(vert_positions[mi], m_vc);
            let m_uv = std::slice::from_raw_parts(vert_uvs[mi], m_vc);
            let m_idx = std::slice::from_raw_parts(idx_data[mi], m_ic);

            let mut m_verts = Vec::with_capacity(m_vc * 4);
            for j in 0..m_vc {
                m_verts.push(m_pos[j].X);
                m_verts.push(m_pos[j].Y);
                m_verts.push(m_uv[j].X);
                m_verts.push(m_uv[j].Y);
            }
            self.draw_mesh.upload(gl, &m_verts, m_idx);
            self.draw_mesh.draw(gl);
        }

        gl.disable(BLEND);
        gl.bind_framebuffer(FRAMEBUFFER, None);

        gl.viewport(viewport[0], viewport[1], viewport[2], viewport[3]);

        let m_scale_loc = gl.get_uniform_location(self.masked_program, "uScale");
        let m_translate_loc = gl.get_uniform_location(self.masked_program, "uTranslate");
        let m_tex_loc = gl.get_uniform_location(self.masked_program, "uTexture");
        let m_mask_tex_loc = gl.get_uniform_location(self.masked_program, "uMaskTexture");
        let m_mask_size_loc = gl.get_uniform_location(self.masked_program, "uMaskSize");
        let m_mul_loc = gl.get_uniform_location(self.masked_program, "uMultiplyColor");
        let m_scr_loc = gl.get_uniform_location(self.masked_program, "uScreenColor");
        let m_opacity_loc = gl.get_uniform_location(self.masked_program, "uOpacity");

        gl.use_program(Some(self.masked_program));
        gl.uniform_2_f32(m_scale_loc.as_ref(), camera.scale_x, -camera.scale_y);
        gl.uniform_2_f32(m_translate_loc.as_ref(), camera.translate_x, -camera.translate_y);

        let tex_idx = tex_indices[drawable_idx];
        let tex = if tex_idx >= 0 && (tex_idx as usize) < self.textures.len() {
            self.textures[tex_idx as usize]
        } else {
            return;
        };
        gl.active_texture(TEXTURE0);
        gl.bind_texture(TEXTURE_2D, Some(tex));
        gl.uniform_1_i32(m_tex_loc.as_ref(), 0);

        gl.active_texture(TEXTURE1);
        gl.bind_texture(TEXTURE_2D, Some(fbo_tex));
        gl.uniform_1_i32(m_mask_tex_loc.as_ref(), 1);
        gl.uniform_2_f32(m_mask_size_loc.as_ref(), fbo_w as f32, fbo_h as f32);

        let blend = blend_modes[drawable_idx];
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
        gl.enable(BLEND);

        let mc = mul_colors[drawable_idx];
        gl.uniform_4_f32(m_mul_loc.as_ref(), mc.X, mc.Y, mc.Z, mc.W);
        let sc = scr_colors[drawable_idx];
        gl.uniform_4_f32(m_scr_loc.as_ref(), sc.X, sc.Y, sc.Z, sc.W);
        gl.uniform_1_f32(m_opacity_loc.as_ref(), opacity);

        let pos_slice = std::slice::from_raw_parts(vert_positions[drawable_idx], vc);
        let uv_slice = std::slice::from_raw_parts(vert_uvs[drawable_idx], vc);
        let idx_slice = std::slice::from_raw_parts(idx_data[drawable_idx], ic);

        let mut verts = Vec::with_capacity(vc * 4);
        for j in 0..vc {
            verts.push(pos_slice[j].X);
            verts.push(pos_slice[j].Y);
            verts.push(uv_slice[j].X);
            verts.push(uv_slice[j].Y);
        }
        self.draw_mesh.upload(gl, &verts, idx_slice);
        self.draw_mesh.draw(gl);

        gl.active_texture(TEXTURE1);
        gl.bind_texture(TEXTURE_2D, None);
        gl.active_texture(TEXTURE0);
        gl.disable(BLEND);
        gl.use_program(None);
    }
}

/// Raw GL overlay for the floating/minimized play button.
/// Bypasses egui_glow entirely since its coordinate transform is bugged at small sizes.
pub struct FloatOverlay {
    program: Option<NativeProgram>,
    vao: Option<NativeVertexArray>,
    vbo: Option<NativeBuffer>,
}

impl FloatOverlay {
    pub fn new() -> Self {
        Self { program: None, vao: None, vbo: None }
    }

    unsafe fn ensure_resources(&mut self, gl: &Context) {
        if self.program.is_some() {
            return;
        }

        // Use pixel-coordinate shader: pass resolution uniform, map pixel coords to NDC
        let vs_src = "\
            #version 100\n
            attribute vec2 p;\n
            uniform vec2 u_resolution;\n
            void main() {\n
                vec2 ndc = 2.0 * p / u_resolution - 1.0;\n
                gl_Position = vec4(ndc, 0.0, 1.0);\n
            }";
        let fs_src = "\
            #version 100\n
            precision mediump float;\n
            uniform vec4 c;\n
            void main() {\n
                gl_FragColor = c;\n
            }";

        let vs = gl.create_shader(VERTEX_SHADER).unwrap();
        gl.shader_source(vs, vs_src);
        gl.compile_shader(vs);
        if !gl.get_shader_compile_status(vs) {
            eprintln!("[float] VS error: {}", gl.get_shader_info_log(vs));
        }

        let fs = gl.create_shader(FRAGMENT_SHADER).unwrap();
        gl.shader_source(fs, fs_src);
        gl.compile_shader(fs);
        if !gl.get_shader_compile_status(fs) {
            eprintln!("[float] FS error: {}", gl.get_shader_info_log(fs));
        }

        let prog = gl.create_program().unwrap();
        gl.attach_shader(prog, vs);
        gl.attach_shader(prog, fs);
        gl.link_program(prog);
        if !gl.get_program_link_status(prog) {
            eprintln!("[float] link error: {}", gl.get_program_info_log(prog));
        }
        gl.delete_shader(vs);
        gl.delete_shader(fs);

        let vao = gl.create_vertex_array().ok();
        let vbo = gl.create_buffer().unwrap();
        self.program = Some(prog);
        self.vao = vao;
        self.vbo = Some(vbo);
    }

    pub unsafe fn draw_play_button(&mut self, gl: &Context, w: f32, h: f32) {
        self.ensure_resources(gl);

        let prog = self.program.unwrap();
        let vbo = self.vbo.unwrap();

        // Play triangle centered in window, using physical pixel coordinates.
        // (0,0) = bottom-left of window, (w,h) = top-right.
        let margin = w.min(h) * 0.12;
        let left = margin;
        let right = w - margin;
        let top = h - margin;
        let bottom = margin;
        let mid_y = h * 0.5;
        // Triangle pointing right: left edge (bottom→top), tip at right-center
        let verts: [f32; 6] = [
            left,  bottom,
            left,  top,
            right, mid_y,
        ];

        gl.use_program(Some(prog));
        gl.disable(DEPTH_TEST);
        gl.disable(STENCIL_TEST);
        gl.disable(CULL_FACE);
        gl.enable(BLEND);
        gl.blend_func_separate(SRC_ALPHA, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA);

        // Set pixel resolution uniform
        let res_loc = gl.get_uniform_location(prog, "u_resolution");
        gl.uniform_2_f32(res_loc.as_ref(), w, h);

        let color_loc = gl.get_uniform_location(prog, "c");
        gl.uniform_4_f32(color_loc.as_ref(), 1.0, 1.0, 1.0, 1.0);

        if let Some(v) = self.vao {
            gl.bind_vertex_array(Some(v));
        }

        gl.bind_buffer(ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(ARRAY_BUFFER, std::slice::from_raw_parts(
            &verts as *const _ as *const u8,
            std::mem::size_of_val(&verts),
        ), STATIC_DRAW);

        let pos_loc = gl.get_attrib_location(prog, "p")
            .expect("float overlay attribute 'p' not found");
        gl.enable_vertex_attrib_array(pos_loc);
        gl.vertex_attrib_pointer_f32(pos_loc, 2, FLOAT, false, 0, 0);

        gl.draw_arrays(TRIANGLES, 0, 3);

        gl.disable_vertex_attrib_array(pos_loc);
        if let Some(_) = self.vao {
            gl.bind_vertex_array(None);
        }
        gl.bind_buffer(ARRAY_BUFFER, None);
        gl.use_program(None);
        gl.disable(BLEND);
    }
}
