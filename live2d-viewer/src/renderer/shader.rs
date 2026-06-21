use glow::*;
use glow::HasContext as _;
use anyhow::Result;

pub const VERT_SRC: &str = r#"#version 330 core
layout(location = 0) in vec2 aPos;
layout(location = 1) in vec2 aUV;
out vec2 vUV;
uniform vec2 uScale;
uniform vec2 uTranslate;

void main() {
    gl_Position = vec4(aPos.x * uScale.x + uTranslate.x, aPos.y * uScale.y + uTranslate.y, 0.0, 1.0);
    vUV = aUV;
}
"#;

pub const FRAG_SRC: &str = r#"#version 330 core
in vec2 vUV;
out vec4 FragColor;

uniform sampler2D uTexture;
uniform vec4 uMultiplyColor;
uniform vec4 uScreenColor;
uniform float uOpacity;

void main() {
    vec4 tex = texture(uTexture, vUV);

    vec4 mul = clamp(uMultiplyColor, 0.0, 1.0);
    vec4 scr = clamp(uScreenColor, 0.0, 1.0);

    vec4 color = tex;
    color.rgb *= mul.rgb;
    color.rgb = color.rgb + (scr.rgb - 0.5) * 2.0;
    color.a *= uOpacity;

    if (color.a < 0.001) { discard; }

    FragColor = color;
}
"#;

pub const MASK_VERT_SRC: &str = r#"#version 330 core
layout(location = 0) in vec2 aPos;
uniform vec2 uScale;
uniform vec2 uTranslate;

void main() {
    gl_Position = vec4(aPos.x * uScale.x + uTranslate.x, aPos.y * uScale.y + uTranslate.y, 0.0, 1.0);
}
"#;

pub const MASK_FRAG_SRC: &str = r#"#version 330 core
out vec4 FragColor;
uniform float uOpacity;

void main() {
    FragColor = vec4(0.0, 0.0, 0.0, uOpacity);
}
"#;

pub unsafe fn compile_program(gl: &Context, vert: &str, frag: &str) -> Result<NativeProgram> {
    let program = gl.create_program().map_err(|e| anyhow::anyhow!("create program: {:?}", e))?;

    let vs = compile_shader(gl, VERTEX_SHADER, vert)?;
    let fs = compile_shader(gl, FRAGMENT_SHADER, frag)?;
    gl.attach_shader(program, vs);
    gl.attach_shader(program, fs);
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        return Err(anyhow::anyhow!("program link: {}", log));
    }
    gl.delete_shader(vs);
    gl.delete_shader(fs);
    Ok(program)
}

unsafe fn compile_shader(gl: &Context, kind: u32, src: &str) -> Result<NativeShader> {
    let shader = gl.create_shader(kind).map_err(|e| anyhow::anyhow!("create shader: {:?}", e))?;
    gl.shader_source(shader, src);
    gl.compile_shader(shader);
    if !gl.get_shader_compile_status(shader) {
        let log = gl.get_shader_info_log(shader);
        gl.delete_shader(shader);
        anyhow::bail!("shader compile: {}", log);
    }
    Ok(shader)
}
