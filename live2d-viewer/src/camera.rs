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

    pub fn fit_to_canvas(&mut self, canvas_w: f32, canvas_h: f32, screen_w: f32, screen_h: f32) {
        let aspect = canvas_w / canvas_h;
        let screen_aspect = screen_w / screen_h;
        if aspect > screen_aspect {
            self.scale_x = 2.0 / canvas_w;
            self.scale_y = 2.0 / canvas_w * screen_aspect / aspect;
        } else {
            self.scale_y = 2.0 / canvas_h;
            self.scale_x = 2.0 / canvas_h * aspect / screen_aspect;
        }
        self.translate_x = -(canvas_w / 2.0) * self.scale_x;
        self.translate_y = -(canvas_h / 2.0) * self.scale_y;
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
