pub struct Camera {
    pub scale_x: f32,
    pub scale_y: f32,
    pub translate_x: f32,
    pub translate_y: f32,
}

impl Camera {
    pub fn new() -> Self {
        Self { scale_x: 1.0, scale_y: 1.0, translate_x: 0.0, translate_y: 0.0 }
    }

    /// Set up camera to match Cubism Framework transform.
    ///
    /// Cubism Core vertex positions are in model-space coordinates.
    /// The Cubism Framework's model matrix scales by `2.0 / logical_canvas_h`
    /// (uniform), then the projection multiplies by `(h/w, 1.0)` for landscape.
    ///
    /// Our vertex shader does: gl_Position = vec4(a_position.xy * uScale + uTranslate, 0, 1)
    /// So uScale = model_matrix_scale * projection_scale (with Y flip).
    pub fn fit_to_canvas(&mut self, _canvas_w: f32, canvas_h: f32, ppu: f32, screen_w: f32, screen_h: f32) {
        // Logical canvas dimensions
        let logical_h = canvas_h / ppu;

        // Model matrix: SetHeight(2.0) → uniform scale = 2.0 / logical_h
        let model_scale = 2.0 / logical_h;

        // Projection: Cubism Framework's OnUpdate for w > h (landscape)
        // projection.Scale(h/w, 1.0)
        let proj_scale_x = screen_h / screen_w;  // h/w
        let proj_scale_y = 1.0;

        // Combined: our vertex shader uses a_position * uScale + uTranslate
        // Y-negated: Cubism Y-down → OpenGL NDC Y-up
        self.scale_x = model_scale * proj_scale_x;
        self.scale_y = -(model_scale * proj_scale_y);
        self.translate_x = 0.0;
        self.translate_y = 0.0;
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        self.translate_x += dx * 2.0 / self.scale_x / 100.0;
        self.translate_y -= dy * 2.0 / self.scale_y / 100.0;
    }

    pub fn zoom(&mut self, delta: f32, cx: f32, cy: f32) {
        let factor = if delta > 0.0 { 1.1 } else { 0.9 };
        let wx = (cx * 2.0 - 1.0) / self.scale_x - self.translate_x / self.scale_x;
        let wy = (1.0 - cy * 2.0) / self.scale_y - self.translate_y / self.scale_y;
        self.scale_x *= factor;
        self.scale_y *= factor;
        self.translate_x = (cx * 2.0 - 1.0) - (wx + self.translate_x / self.scale_x) * self.scale_x;
        self.translate_y = (1.0 - cy * 2.0) - (wy + self.translate_y / self.scale_y) * self.scale_y;
    }
}
