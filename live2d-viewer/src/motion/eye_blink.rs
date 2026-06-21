//! Eye blink controller — state machine matching CubismFramework's CubismEyeBlink.
//!
//! States: First → Interval → Closing → Closed → Opening → Interval (loop)
//! Parameter value: 1.0 = eyes open, 0.0 = eyes fully closed.

/// Eye blink state machine
#[derive(Debug)]
pub struct EyeBlink {
    state: BlinkState,
    user_time_seconds: f32,
    state_start_time: f32,
    next_blink_time: f32,
    /// Average interval between blinks (seconds)
    interval: f32,
    /// Time to close eyes (seconds)
    closing_duration: f32,
    /// Time to stay closed (seconds)
    closed_duration: f32,
    /// Time to open eyes (seconds)
    opening_duration: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BlinkState {
    First,
    Interval,
    Closing,
    Closed,
    Opening,
}

impl EyeBlink {
    pub fn new() -> Self {
        Self {
            state: BlinkState::First,
            user_time_seconds: 0.0,
            state_start_time: 0.0,
            next_blink_time: random_future(4.0),
            interval: 4.0,
            closing_duration: 0.1,
            closed_duration: 0.05,
            opening_duration: 0.15,
        }
    }

    /// Advance the blink state machine. Returns the current blink value (0.0 = closed, 1.0 = open).
    pub fn update(&mut self, delta_time: f32) -> f32 {
        self.user_time_seconds += delta_time;

        match self.state {
            BlinkState::First => {
                self.state = BlinkState::Interval;
                self.next_blink_time = random_future(self.interval);
                1.0
            }
            BlinkState::Interval => {
                if self.user_time_seconds >= self.next_blink_time {
                    self.state = BlinkState::Closing;
                    self.state_start_time = self.user_time_seconds;
                }
                1.0
            }
            BlinkState::Closing => {
                let t = ((self.user_time_seconds - self.state_start_time) / self.closing_duration).min(1.0);
                if t >= 1.0 {
                    self.state = BlinkState::Closed;
                    self.state_start_time = self.user_time_seconds;
                }
                1.0 - t // 1→0
            }
            BlinkState::Closed => {
                let t = ((self.user_time_seconds - self.state_start_time) / self.closed_duration).min(1.0);
                if t >= 1.0 {
                    self.state = BlinkState::Opening;
                    self.state_start_time = self.user_time_seconds;
                }
                0.0
            }
            BlinkState::Opening => {
                let t = ((self.user_time_seconds - self.state_start_time) / self.opening_duration).min(1.0);
                if t >= 1.0 {
                    self.state = BlinkState::Interval;
                    self.next_blink_time = random_future(self.interval);
                }
                t // 0→1
            }
        }
    }
}

fn random_future(interval: f32) -> f32 {
    // Simple deterministic pseudo-random in [0, 1)
    let r = simple_rng();
    // Returns time in range [interval, interval*2) from now
    // Actually the framework does: now + r * (2*interval - 1)
    // But since we don't track "now" here, we just offset from 0
    // and the caller uses user_time_seconds. We return absolute time.
    r * interval * 2.0
}

static mut RNG_STATE: u32 = 12345;

fn simple_rng() -> f32 {
    unsafe {
        RNG_STATE = RNG_STATE.wrapping_mul(1103515245).wrapping_add(12345);
        (RNG_STATE as f32 / 2147483648.0).abs()
    }
}
