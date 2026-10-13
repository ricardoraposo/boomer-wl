use crate::config::Config;

const VELOCITY_THRESHOLD: f32 = 15.0;
const INITIAL_FL_DELTA_RADIUS: f32 = 250.0;
const FL_DELTA_RADIUS_DECELERATION: f32 = 10.0;
const FL_MAX_SHADOW: f32 = 0.8;
const FL_SHADOW_SPEED: f32 = 6.0;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }
}

impl std::ops::Add for Vec2 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s)
    }
}

impl std::ops::Div<f32> for Vec2 {
    type Output = Self;
    fn div(self, s: f32) -> Self {
        Self::new(self.x / s, self.y / s)
    }
}

impl std::ops::AddAssign for Vec2 {
    fn add_assign(&mut self, o: Self) {
        *self = *self + o;
    }
}

impl std::ops::SubAssign for Vec2 {
    fn sub_assign(&mut self, o: Self) {
        *self = *self - o;
    }
}

#[derive(Debug, Default)]
pub struct Mouse {
    pub curr: Vec2,
    pub prev: Vec2,
    pub drag: bool,
}

#[derive(Debug)]
pub struct Camera {
    pub position: Vec2,
    pub velocity: Vec2,
    pub scale: f32,
    pub delta_scale: f32,
    pub scale_pivot: Vec2,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: Vec2::ZERO,
            velocity: Vec2::ZERO,
            scale: 1.0,
            delta_scale: 0.0,
            scale_pivot: Vec2::ZERO,
        }
    }
}

impl Camera {
    pub fn world(&self, v: Vec2) -> Vec2 {
        v / self.scale
    }

    pub fn is_animating(&self, mouse: &Mouse) -> bool {
        self.delta_scale.abs() > 0.5 || (!mouse.drag && self.velocity.length() > VELOCITY_THRESHOLD)
    }

    pub fn update(&mut self, config: &Config, dt: f32, mouse: &Mouse, window_size: Vec2) {
        if self.delta_scale.abs() > 0.5 {
            let half_window = window_size * 0.5;
            let p0 = (self.scale_pivot - half_window) / self.scale;
            self.scale = (self.scale + self.delta_scale * dt).max(config.min_scale);
            let p1 = (self.scale_pivot - half_window) / self.scale;
            self.position += p0 - p1;
            self.delta_scale -= self.delta_scale * dt * config.scale_friction;
        }

        if !mouse.drag && self.velocity.length() > VELOCITY_THRESHOLD {
            self.position += self.velocity * dt;
            self.velocity -= self.velocity * dt * config.drag_friction;
        }
    }
}

#[derive(Debug)]
pub struct Flashlight {
    pub is_enabled: bool,
    pub shadow: f32,
    pub radius: f32,
    pub delta_radius: f32,
}

impl Default for Flashlight {
    fn default() -> Self {
        Self {
            is_enabled: false,
            shadow: 0.0,
            radius: 200.0,
            delta_radius: 0.0,
        }
    }
}

impl Flashlight {
    pub fn grow(&mut self) {
        self.delta_radius += INITIAL_FL_DELTA_RADIUS;
    }

    pub fn shrink(&mut self) {
        self.delta_radius -= INITIAL_FL_DELTA_RADIUS;
    }

    pub fn is_animating(&self) -> bool {
        let target = if self.is_enabled { FL_MAX_SHADOW } else { 0.0 };
        self.delta_radius.abs() > 1.0 || self.shadow != target
    }

    pub fn update(&mut self, dt: f32) {
        if self.delta_radius.abs() > 1.0 {
            self.radius = (self.radius + self.delta_radius * dt).max(0.0);
            self.delta_radius -= self.delta_radius * FL_DELTA_RADIUS_DECELERATION * dt;
        }

        self.shadow = if self.is_enabled {
            (self.shadow + FL_SHADOW_SPEED * dt).min(FL_MAX_SHADOW)
        } else {
            (self.shadow - FL_SHADOW_SPEED * dt).max(0.0)
        };
    }
}
