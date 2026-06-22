//! Look controller — mouse/pointer following for head and eyes.
//!
//! Mirrors CubismFramework's CubismLook + CubismTargetPoint.
//! Each frame: subtract old offset → update target → add new offset.
//! This gives correct absolute values without accumulation.

/// Smooth target point tracking (physics-based velocity/acceleration).
#[derive(Debug)]
pub struct TargetPoint {
    target_x: f32,
    target_y: f32,
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    last_time: f32,
    user_time: f32,
}

impl TargetPoint {
    pub fn new() -> Self {
        Self {
            target_x: 0.0, target_y: 0.0,
            x: 0.0, y: 0.0,
            vx: 0.0, vy: 0.0,
            last_time: 0.0, user_time: 0.0,
        }
    }

    pub fn set(&mut self, x: f32, y: f32) {
        self.target_x = x.clamp(-1.0, 1.0);
        self.target_y = y.clamp(-1.0, 1.0);
    }

    /// Returns current (x, y) in NDC [-1,1].
    pub fn update(&mut self, delta: f32) -> (f32, f32) {
        self.user_time += delta;
        if self.last_time == 0.0 {
            self.last_time = self.user_time;
            return (self.x, self.y);
        }
        let dt = (self.user_time - self.last_time).min(0.1);
        self.last_time = self.user_time;

        const MAX_V: f32 = 40.0 / 10.0 / 30.0 * 3.0;  // 3x faster response
        let dt_w = dt * 30.0;
        let max_a = dt_w * MAX_V / (0.05 * 30.0);      // 3x faster acceleration
        const EPS: f32 = 0.003;

        let dx = self.target_x - self.x;
        let dy = self.target_y - self.y;
        let d = (dx * dx + dy * dy).sqrt();
        if d <= EPS { return (self.x, self.y); }

        let want_vx = MAX_V * dx / d;
        let want_vy = MAX_V * dy / d;
        let mut ax = want_vx - self.vx;
        let mut ay = want_vy - self.vy;
        let a = (ax * ax + ay * ay).sqrt();
        if a > max_a { ax *= max_a / a; ay *= max_a / a; }

        self.vx += ax;
        self.vy += ay;

        let cur_v = (self.vx * self.vx + self.vy * self.vy).sqrt();
        let max_v = 0.5 * ((max_a * max_a + 16.0 * max_a * d - 8.0 * max_a * d).sqrt() - max_a);
        if cur_v > max_v && max_v > 0.0 { self.vx *= max_v / cur_v; self.vy *= max_v / cur_v; }

        self.x += self.vx;
        self.y += self.vy;
        (self.x, self.y)
    }
}

/// One look-mapped parameter.
#[derive(Debug, Clone)]
pub struct LookParam {
    pub id: String,
    pub factor_x: f32,
    pub factor_y: f32,
    pub factor_xy: f32,
    /// Absolute offset currently applied (added to/ subtracted from param_values)
    pub current_offset: f32,
}

/// Look controller.
#[derive(Debug)]
pub struct Look {
    pub params: Vec<LookParam>,
    pub target: TargetPoint,
}

impl Look {
    pub fn new() -> Self {
        Self {
            params: vec![
                LookParam { id: "ParamAngleX".into(),     factor_x: 60.0,  factor_y: 0.0,  factor_xy: 0.0, current_offset: 0.0 },
                LookParam { id: "ParamAngleY".into(),     factor_x: 0.0,   factor_y: 60.0, factor_xy: 0.0, current_offset: 0.0 },
                LookParam { id: "ParamAngleZ".into(),     factor_x: 0.0,   factor_y: 0.0,  factor_xy: -60.0, current_offset: 0.0 },
                LookParam { id: "ParamBodyAngleX".into(), factor_x: 20.0,  factor_y: 0.0,  factor_xy: 0.0, current_offset: 0.0 },
                LookParam { id: "ParamEyeBallX".into(),   factor_x: 3.0,   factor_y: 0.0,  factor_xy: 0.0, current_offset: 0.0 },
                LookParam { id: "ParamEyeBallY".into(),   factor_x: 0.0,   factor_y: 3.0,  factor_xy: 0.0, current_offset: 0.0 },
            ],
            target: TargetPoint::new(),
        }
    }

    pub fn set_target(&mut self, ndc_x: f32, ndc_y: f32) {
        self.target.set(ndc_x, ndc_y);
    }

    /// Step the target simulation and compute new raw offset (independent of accumulation).
    /// Caller must subtract old offsets before and add new offsets after.
    /// Returns the current raw value (factor * drag) for each param.
    pub fn compute_raw(&mut self, delta: f32) {
        let (dx, dy) = self.target.update(delta);
        let dxy = dx * dy;
        for p in &mut self.params {
            p.current_offset = p.factor_x * dx + p.factor_y * dy + p.factor_xy * dxy;
        }
    }
}
