pub struct Camera {
    pub scale_x: f32,
    pub scale_y: f32,
    pub translate_x: f32,
    pub translate_y: f32,
    /// Whether the model coordinate system has Y pointing DOWN (MOC2).
    /// When true, scale_y is negated to flip Y-up for OpenGL NDC.
    /// MOC3 (Core) typically has Y-up with centered origin → flip_y = false.
    pub flip_y: bool,
}

impl Camera {
    pub fn new() -> Self {
        Self { scale_x: 1.0, scale_y: 1.0, translate_x: 0.0, translate_y: 0.0, flip_y: false }
    }

    /// Set up camera to match Cubism Framework transform.
    ///
    /// - `canvas_w` / `canvas_h`: canvas size in pixels (or logical units).
    /// - `ppu`: pixels-per-unit (1.0 for MOC2, >1 possible for MOC3).
    ///
    /// Cubism Core vertex positions are in model-space coordinates.
    ///   - MOC2: [0, canvas_w] × [0, canvas_h], origin at top-left, Y-down.
    ///   - MOC3: centered at origin, Y-up (or Y-down with flip_y).
    ///
    /// The camera computes a uniform scale and centering translate.
    /// Our vertex shader: gl_Position = vec4(a_position.xy * uScale + uTranslate, 0, 1)
    pub fn fit_to_canvas(&mut self, canvas_w: f32, canvas_h: f32, ppu: f32, screen_w: f32, screen_h: f32) {
        let logical_h = canvas_h / ppu;
        let model_scale = 2.0 / logical_h;

        if screen_w > screen_h {
            // Landscape: model fills height, correct width
            self.scale_x = model_scale * (screen_h / screen_w);
        } else {
            // Portrait: model fills width, correct height
            self.scale_x = model_scale;
        }

        if self.flip_y {
            // MOC2: Y-down → flip Y-up; vertices in [0,w]×[0,h], center them
            self.scale_y = -self.scale_x;
            self.translate_x = -(canvas_w / 2.0) * self.scale_x;
            self.translate_y = -(canvas_h / 2.0) * self.scale_y; // = (canvas_h/2) * scale_x
        } else {
            // MOC3: already centered at origin
            self.scale_y = self.scale_x;
            self.translate_x = 0.0;
            self.translate_y = 0.0;
        }
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

    /// Reset pan (re-center the model).
    pub fn reset_pan(&mut self) {
        self.translate_x = 0.0;
        self.translate_y = 0.0;
    }

    /// Zoom in by one step (centered).
    pub fn zoom_in(&mut self) {
        self.zoom(1.0, 0.5, 0.5);
    }

    /// Zoom out by one step (centered).
    pub fn zoom_out(&mut self) {
        self.zoom(-1.0, 0.5, 0.5);
    }
}
