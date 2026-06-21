//! Look controller — mouse/pointer following for head and eyes.
//!
//! Mirrors CubismFramework's CubismLook + CubismTargetPoint.
//! Smoothly tracks a drag position (NDC [-1,1]) and maps to parameters.

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
            target_x: 0.0,
            target_y: 0.0,
            x: 0.0, y: 0.0,
            vx: 0.0, vy: 0.0,
            last_time: 0.0,
            user_time: 0.0,
        }
    }

    /// Set the target position (NDC: -1..1).
    pub fn set(&mut self, x: f32, y: f32) {
        self.target_x = x.clamp(-1.0, 1.0);
        self.target_y = y.clamp(-1.0, 1.0);
    }

    /// Advance simulation. Returns current (x, y) in NDC.
    pub fn update(&mut self, delta: f32) -> (f32, f32) {
        self.user_time += delta;
        if self.last_time == 0.0 {
            self.last_time = self.user_time;
            return (self.x, self.y);
        }

        let dt = (self.user_time - self.last_time).min(0.1);
        self.last_time = self.user_time;

        const MAX_V: f32 = 40.0 / 10.0 / 30.0;    // max velocity per frame
        let dt_weight = dt * 30.0;                 // normalized to 30fps
        let max_a = dt_weight * MAX_V / (0.15 * 30.0); // max acceleration
        const EPS: f32 = 0.01;

        let dx = self.target_x - self.x;
        let dy = self.target_y - self.y;
        let d = (dx * dx + dy * dy).sqrt();

        if d <= EPS {
            return (self.x, self.y);
        }

        // Desired velocity toward target
        let want_vx = MAX_V * dx / d;
        let want_vy = MAX_V * dy / d;

        // Acceleration needed
        let mut ax = want_vx - self.vx;
        let mut ay = want_vy - self.vy;
        let a = (ax * ax + ay * ay).sqrt();

        if a > max_a {
            ax *= max_a / a;
            ay *= max_a / a;
        }

        self.vx += ax;
        self.vy += ay;

        // Braking: if we'd overshoot, slow down
        let cur_v = (self.vx * self.vx + self.vy * self.vy).sqrt();
        let max_v = 0.5 * ((max_a * max_a + 16.0 * max_a * d - 8.0 * max_a * d).sqrt() - max_a);
        if cur_v > max_v && max_v > 0.0 {
            self.vx *= max_v / cur_v;
            self.vy *= max_v / cur_v;
        }

        self.x += self.vx;
        self.y += self.vy;

        (self.x, self.y)
    }
}

/// Look parameter factor definition.
#[derive(Debug, Clone)]
pub struct LookParam {
    pub id: String,
    pub factor_x: f32,
    pub factor_y: f32,
    pub factor_xy: f32,
    prev_raw: f32,
}

/// Look controller: maps drag (x,y) to parameter values (delta-additive).
#[derive(Debug)]
pub struct Look {
    pub params: Vec<LookParam>,
    pub target: TargetPoint,
}

impl Look {
    /// Create with official default factors.
    pub fn new() -> Self {
        Self {
            params: vec![
                LookParam { id: "ParamAngleX".into(),     factor_x: 30.0,  factor_y: 0.0,  factor_xy: 0.0, prev_raw: 0.0 },
                LookParam { id: "ParamAngleY".into(),     factor_x: 0.0,   factor_y: 30.0, factor_xy: 0.0, prev_raw: 0.0 },
                LookParam { id: "ParamAngleZ".into(),     factor_x: 0.0,   factor_y: 0.0,  factor_xy: -30.0, prev_raw: 0.0 },
                LookParam { id: "ParamBodyAngleX".into(), factor_x: 10.0,  factor_y: 0.0,  factor_xy: 0.0, prev_raw: 0.0 },
                LookParam { id: "ParamEyeBallX".into(),   factor_x: 1.0,   factor_y: 0.0,  factor_xy: 0.0, prev_raw: 0.0 },
                LookParam { id: "ParamEyeBallY".into(),   factor_x: 0.0,   factor_y: 1.0,  factor_xy: 0.0, prev_raw: 0.0 },
            ],
            target: TargetPoint::new(),
        }
    }

    /// Set drag target from mouse NDC position.
    pub fn set_target(&mut self, ndc_x: f32, ndc_y: f32) {
        self.target.set(ndc_x, ndc_y);
    }

    /// Advance and apply look deltas (delta-based to prevent accumulation).
    pub fn update(&mut self, delta: f32, param_values: &mut [f32], param_names: &[String]) {
        let (dx, dy) = self.target.update(delta);
        let dxy = dx * dy;

        for p in &mut self.params {
            let raw = p.factor_x * dx + p.factor_y * dy + p.factor_xy * dxy;
            let delta = raw - p.prev_raw;
            p.prev_raw = raw;
            if delta.abs() < 1e-7 {
                continue;
            }
            if let Some(idx) = param_names.iter().position(|n| n == &p.id) {
                if idx < param_values.len() {
                    param_values[idx] = (param_values[idx] + delta).clamp(-100.0, 100.0);
                }
            }
        }
    }
}
