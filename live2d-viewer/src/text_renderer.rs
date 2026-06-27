//! Minimal bitmap-font text renderer for the sctk layer-shell overlay.
//!
//! Uses a hand-crafted 5×7 pixel monospace font for ASCII 32–122
//! (space through 'z').  Every glyph is uploaded as a single-channel
//! (alpha) texture atlas; drawing amounts to a batch of textured quads.
//!
//! No external dependencies — raw `glow` calls only.

use glow::*;

// ---------------------------------------------------------------------------
// Font data (91 glyphs × 7 rows, 5-bit encoded, MSB = leftmost pixel)
// ---------------------------------------------------------------------------

const GLYPH_W: u32 = 5;
const GLYPH_H: u32 = 7;
const GLYPH_ROWS: usize = 7;
const CHARS_PER_ROW: u32 = 13; // 13 × 7 = 91 glyphs (32–122)
const FIRST_CHAR: u32 = 32; // space
const TOTAL_CHARS: usize = 91;

/// One row of a glyph: only the low 5 bits are used.
type GlyphRow = u8;

/// 91 glyphs; each glyph is `[GLYPH_ROWS]` rows.
const FONT_DATA: &[GlyphRow] = &[
    // 32  space
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    // 33  !
    0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04,
    // 34  "
    0x0A, 0x0A, 0x0A, 0x00, 0x00, 0x00, 0x00,
    // 35  #
    0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A,
    // 36  $
    0x04, 0x0F, 0x14, 0x0E, 0x05, 0x1E, 0x04,
    // 37  %
    0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03,
    // 38  &
    0x0C, 0x12, 0x14, 0x08, 0x15, 0x12, 0x0D,
    // 39  '
    0x04, 0x04, 0x04, 0x00, 0x00, 0x00, 0x00,
    // 40  (
    0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02,
    // 41  )
    0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08,
    // 42  *
    0x04, 0x15, 0x0E, 0x15, 0x04, 0x00, 0x00,
    // 43  +
    0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00,
    // 44  ,
    0x00, 0x00, 0x00, 0x00, 0x04, 0x04, 0x08,
    // 45  -
    0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00,
    // 46  .
    0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x04,
    // 47  /
    0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10,
    // 48  0
    0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E,
    // 49  1
    0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E,
    // 50  2
    0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F,
    // 51  3
    0x0E, 0x11, 0x01, 0x06, 0x01, 0x11, 0x0E,
    // 52  4
    0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02,
    // 53  5
    0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E,
    // 54  6
    0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E,
    // 55  7
    0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08,
    // 56  8
    0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E,
    // 57  9
    0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C,
    // 58  :
    0x00, 0x00, 0x04, 0x00, 0x00, 0x04, 0x00,
    // 59  ;
    0x00, 0x00, 0x04, 0x00, 0x00, 0x04, 0x08,
    // 60  <
    0x02, 0x04, 0x08, 0x10, 0x08, 0x04, 0x02,
    // 61  =
    0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00,
    // 62  >
    0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08,
    // 63  ?
    0x0E, 0x11, 0x01, 0x06, 0x04, 0x00, 0x04,
    // 64  @
    0x0E, 0x11, 0x17, 0x15, 0x17, 0x10, 0x0F,
    // 65  A
    0x04, 0x0A, 0x11, 0x11, 0x1F, 0x11, 0x11,
    // 66  B
    0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E,
    // 67  C
    0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E,
    // 68  D
    0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E,
    // 69  E
    0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F,
    // 70  F
    0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10,
    // 71  G
    0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F,
    // 72  H
    0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11,
    // 73  I
    0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E,
    // 74  J
    0x07, 0x02, 0x02, 0x02, 0x12, 0x12, 0x0C,
    // 75  K
    0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11,
    // 76  L
    0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F,
    // 77  M
    0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11,
    // 78  N
    0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11,
    // 79  O
    0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E,
    // 80  P
    0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10,
    // 81  Q
    0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D,
    // 82  R
    0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11,
    // 83  S
    0x0E, 0x11, 0x10, 0x0E, 0x01, 0x11, 0x0E,
    // 84  T
    0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04,
    // 85  U
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E,
    // 86  V
    0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04,
    // 87  W
    0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11,
    // 88  X
    0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11,
    // 89  Y
    0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04,
    // 90  Z
    0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F,
    // 91  [
    0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E,
    // 92  \
    0x10, 0x08, 0x08, 0x04, 0x02, 0x02, 0x01,
    // 93  ]
    0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E,
    // 94  ^
    0x04, 0x0A, 0x11, 0x00, 0x00, 0x00, 0x00,
    // 95  _
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F,
    // 96  `
    0x08, 0x04, 0x02, 0x00, 0x00, 0x00, 0x00,
    // 97  a
    0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F,
    // 98  b
    0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x1E,
    // 99  c
    0x00, 0x00, 0x0E, 0x10, 0x10, 0x11, 0x0E,
    // 100 d
    0x01, 0x01, 0x0D, 0x13, 0x11, 0x11, 0x0F,
    // 101 e
    0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E,
    // 102 f
    0x06, 0x09, 0x08, 0x1C, 0x08, 0x08, 0x08,
    // 103 g
    0x00, 0x00, 0x0F, 0x11, 0x0F, 0x01, 0x0E,
    // 104 h
    0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x11,
    // 105 i
    0x04, 0x00, 0x0C, 0x04, 0x04, 0x04, 0x0E,
    // 106 j
    0x02, 0x00, 0x06, 0x02, 0x02, 0x12, 0x0C,
    // 107 k
    0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12,
    // 108 l
    0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E,
    // 109 m
    0x00, 0x00, 0x1A, 0x15, 0x15, 0x11, 0x11,
    // 110 n
    0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11,
    // 111 o
    0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E,
    // 112 p
    0x00, 0x00, 0x1E, 0x11, 0x1E, 0x10, 0x10,
    // 113 q
    0x00, 0x00, 0x0D, 0x13, 0x0F, 0x01, 0x01,
    // 114 r
    0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10,
    // 115 s
    0x00, 0x00, 0x0E, 0x10, 0x0E, 0x01, 0x1E,
    // 116 t
    0x08, 0x08, 0x1C, 0x08, 0x08, 0x09, 0x06,
    // 117 u
    0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0D,
    // 118 v
    0x00, 0x00, 0x11, 0x11, 0x11, 0x0A, 0x04,
    // 119 w
    0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0A,
    // 120 x
    0x00, 0x00, 0x11, 0x0A, 0x04, 0x0A, 0x11,
    // 121 y
    0x00, 0x00, 0x11, 0x11, 0x0F, 0x01, 0x0E,
    // 122 z
    0x00, 0x00, 0x1F, 0x02, 0x04, 0x08, 0x1F,
];

// ---------------------------------------------------------------------------
// Shader sources
// ---------------------------------------------------------------------------

const VS_SRC: &str = r#"#version 330 core
layout(location=0) in vec2 aPos;
layout(location=1) in vec2 aTexCoord;
layout(location=2) in vec4 aColor;
out vec2 vTexCoord;
out vec4 vColor;
void main() {
    gl_Position = vec4(aPos, 0.0, 1.0);
    vTexCoord = aTexCoord;
    vColor = aColor;
}"#;

const FS_SRC: &str = r#"#version 330 core
in vec2 vTexCoord;
in vec4 vColor;
out vec4 FragColor;
uniform sampler2D uTexture;
void main() {
    float a = texture(uTexture, vTexCoord).r;
    FragColor = vec4(vColor.rgb, vColor.a * a);
}"#;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ndc(sx: f32, sy: f32, vp_w: f32, vp_h: f32) -> [f32; 2] {
    [(sx / vp_w) * 2.0 - 1.0, 1.0 - (sy / vp_h) * 2.0]
}

unsafe fn compile_program(gl: &Context) -> Result<NativeProgram, String> {
    let program = gl.create_program().map_err(|e| format!("create program: {e}"))?;

    let vs = gl
        .create_shader(VERTEX_SHADER)
        .map_err(|e| format!("create vs: {e}"))?;
    gl.shader_source(vs, VS_SRC);
    gl.compile_shader(vs);
    if !gl.get_shader_compile_status(vs) {
        return Err(format!("vs compile: {}", gl.get_shader_info_log(vs)));
    }

    let fs = gl
        .create_shader(FRAGMENT_SHADER)
        .map_err(|e| format!("create fs: {e}"))?;
    gl.shader_source(fs, FS_SRC);
    gl.compile_shader(fs);
    if !gl.get_shader_compile_status(fs) {
        return Err(format!("fs compile: {}", gl.get_shader_info_log(fs)));
    }

    gl.attach_shader(program, vs);
    gl.attach_shader(program, fs);
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        return Err(format!("link: {}", gl.get_program_info_log(program)));
    }

    gl.delete_shader(vs);
    gl.delete_shader(fs);
    Ok(program)
}

// ---------------------------------------------------------------------------
// TextRenderer
// ---------------------------------------------------------------------------

pub struct TextRenderer {
    program: NativeProgram,
    vao: NativeVertexArray,
    vbo: NativeBuffer,
    font_tex: NativeTexture,
}

impl TextRenderer {
    /// Create font atlas texture and compile shader.
    ///
    /// # Safety
    ///
    /// Requires an active GL context.
    pub unsafe fn new(gl: &Context) -> Result<Self, String> {
        // --- Font atlas texture ---
        let cell_w = GLYPH_W + 1;
        let cell_h = GLYPH_H + 1;
        let atlas_w = CHARS_PER_ROW * cell_w;
        let atlas_h =
            (TOTAL_CHARS as u32).div_ceil(CHARS_PER_ROW) * cell_h;

        let mut atlas = vec![0u8; (atlas_w * atlas_h) as usize];
        for (idx, glyph) in FONT_DATA.chunks_exact(GLYPH_ROWS).enumerate() {
            let col = idx as u32 % CHARS_PER_ROW;
            let row = idx as u32 / CHARS_PER_ROW;
            let ox = (col * cell_w) as usize;
            let oy = (row * cell_h) as usize;
            for (y, &bits) in glyph.iter().enumerate().take(GLYPH_H as usize) {
                for x in 0..GLYPH_W as usize {
                    let pixel = (bits >> (4 - x)) & 1;
                    let px = ox + x;
                    let py = oy + y;
                    atlas[py * atlas_w as usize + px] = if pixel != 0 { 255 } else { 0 };
                }
            }
        }

        let font_tex = gl.create_texture().map_err(|e| format!("create tex: {e}"))?;
        gl.bind_texture(TEXTURE_2D, Some(font_tex));
        gl.tex_parameter_i32(TEXTURE_2D, TEXTURE_MIN_FILTER, NEAREST as i32);
        gl.tex_parameter_i32(TEXTURE_2D, TEXTURE_MAG_FILTER, NEAREST as i32);
        gl.tex_parameter_i32(TEXTURE_2D, TEXTURE_WRAP_S, CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(TEXTURE_2D, TEXTURE_WRAP_T, CLAMP_TO_EDGE as i32);
        gl.tex_image_2d(
            TEXTURE_2D,
            0,
            RED as i32,
            atlas_w as i32,
            atlas_h as i32,
            0,
            RED,
            UNSIGNED_BYTE,
            Some(&atlas),
        );

        // --- Shader ---
        let program = compile_program(gl)?;

        // --- VAO / VBO ---
        let vao = gl.create_vertex_array().map_err(|e| format!("vao: {e}"))?;
        let vbo = gl.create_buffer().map_err(|e| format!("vbo: {e}"))?;

        gl.bind_vertex_array(Some(vao));
        gl.bind_buffer(ARRAY_BUFFER, Some(vbo));
        // layout: pos2 + uv2 + color4 = 8 × f32 = 32 bytes
        let stride: i32 = 32;
        gl.vertex_attrib_pointer_f32(0, 2, FLOAT, false, stride, 0);
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(1, 2, FLOAT, false, stride, 8);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(2, 4, FLOAT, false, stride, 16);
        gl.enable_vertex_attrib_array(2);
        gl.bind_vertex_array(None);
        gl.bind_texture(TEXTURE_2D, None);
        gl.use_program(None);

        Ok(Self { program, vao, vbo, font_tex })
    }

    /// Draw a string at screen coordinates `(x, y)` using the bitmap font.
    ///
    /// `vp_w` / `vp_h` are the viewport dimensions (in pixels).
    /// `total_alpha` is multiplied with the per-glyph colour alpha.
    ///
    /// # Safety
    ///
    /// Requires an active GL context and the caller should enable
    /// blending with `gl.blend_func_separate(SRC_ALPHA, ONE_MINUS_SRC_ALPHA, ...)`.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn draw_text(
        &self,
        gl: &Context,
        text: &str,
        mut x: f32,
        mut y: f32,
        color: [f32; 4],
        vp_w: u32,
        vp_h: u32,
        alpha_mult: f32,
        glyph_scale: f32,
    ) {
        let vpw = vp_w as f32;
        let vph = vp_h as f32;
        let cell_w = GLYPH_W + 1;
        let cell_h = GLYPH_H + 1;
        let atlas_w = (CHARS_PER_ROW * cell_w) as f32;
        let atlas_h =
            (((TOTAL_CHARS as u32).div_ceil(CHARS_PER_ROW)) * cell_h) as f32;

        let mut verts: Vec<f32> = Vec::new();
        let a = color[3] * alpha_mult;
        let start_x = x;
        let gw = GLYPH_W as f32 * glyph_scale;
        let gh = GLYPH_H as f32 * glyph_scale;

        for ch in text.chars() {
            if ch == '\n' {
                x = start_x;
                y += cell_h as f32 * glyph_scale;
                continue;
            }
            let code = ch as u32;
            if code < FIRST_CHAR || code >= FIRST_CHAR + TOTAL_CHARS as u32 {
                x += cell_w as f32 * glyph_scale * 0.6;
                continue;
            }
            let idx = (code - FIRST_CHAR) as u32;
            let col = idx % CHARS_PER_ROW;
            let row = idx / CHARS_PER_ROW;

            let u0 = (col * cell_w) as f32 / atlas_w;
            let v0 = (row * cell_h) as f32 / atlas_h;
            let u1 = (col * cell_w + GLYPH_W) as f32 / atlas_w;
            let v1 = (row * cell_h + GLYPH_H) as f32 / atlas_h;

            let x1 = x + gw;
            let y1 = y + gh;

            // TL=(x,y), TR=(x1,y), BL=(x,y1), BR=(x1,y1)
            // UV: TL=(u0,v0), TR=(u1,v0), BL=(u0,v1), BR=(u1,v1)
            let [tlx, tly] = ndc(x, y, vpw, vph);
            let [trx, try_] = ndc(x1, y, vpw, vph);
            let [blx, bly] = ndc(x, y1, vpw, vph);
            let [brx, bry] = ndc(x1, y1, vpw, vph);

            // Triangle 1: TL, TR, BL
            verts.extend_from_slice(&[
                tlx, tly, u0, v0, color[0], color[1], color[2], a,
                trx, try_, u1, v0, color[0], color[1], color[2], a,
                blx, bly, u0, v1, color[0], color[1], color[2], a,
            ]);
            // Triangle 2: TR, BR, BL
            verts.extend_from_slice(&[
                trx, try_, u1, v0, color[0], color[1], color[2], a,
                brx, bry, u1, v1, color[0], color[1], color[2], a,
                blx, bly, u0, v1, color[0], color[1], color[2], a,
            ]);

            x += cell_w as f32 * glyph_scale;
        }

        if verts.is_empty() { return; }

        gl.bind_vertex_array(Some(self.vao));
        gl.bind_buffer(ARRAY_BUFFER, Some(self.vbo));
        let bytes = std::slice::from_raw_parts(verts.as_ptr() as *const u8, verts.len() * 4);
        gl.buffer_data_u8_slice(ARRAY_BUFFER, bytes, STREAM_DRAW);

        gl.use_program(Some(self.program));
        gl.active_texture(0);
        gl.bind_texture(TEXTURE_2D, Some(self.font_tex));
        if let Some(loc) = gl.get_uniform_location(self.program, "uTexture") {
            gl.uniform_1_i32(Some(&loc), 0);
        }

        gl.draw_arrays(TRIANGLES, 0, verts.len() as i32 / 8);

        gl.bind_vertex_array(None);
        gl.bind_texture(TEXTURE_2D, None);
        gl.use_program(None);
    }

    /// Return the advance width of a string in screen pixels.
    pub fn text_width(text: &str) -> f32 {
        let cell_w = (GLYPH_W + 1) as f32;
        text.chars().count() as f32 * cell_w
    }

    /// Destroy GL resources.
    ///
    /// # Safety
    ///
    /// Requires an active GL context.
    pub unsafe fn destroy(self, gl: &Context) {
        gl.delete_program(self.program);
        gl.delete_vertex_array(self.vao);
        gl.delete_buffer(self.vbo);
        gl.delete_texture(self.font_tex);
    }
}
