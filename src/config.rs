use std::{fmt, fs, path::Path};

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub min_scale: f32,
    pub scroll_speed: f32,
    pub drag_friction: f32,
    pub scale_friction: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            min_scale: 0.01,
            scroll_speed: 1.5,
            drag_friction: 6.0,
            scale_friction: 4.0,
        }
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "min_scale = {}", self.min_scale)?;
        writeln!(f, "scroll_speed = {}", self.scroll_speed)?;
        writeln!(f, "drag_friction = {}", self.drag_friction)?;
        writeln!(f, "scale_friction = {}", self.scale_friction)
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut config = Self::default();
        for (index, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let location = format!("{}:{}", path.display(), index + 1);
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("{location}: expected `key = value`"))?;
            let value: f32 = value
                .trim()
                .parse()
                .map_err(|_| format!("{location}: `{}` is not a number", value.trim()))?;
            match key.trim() {
                "min_scale" => config.min_scale = value,
                "scroll_speed" => config.scroll_speed = value,
                "drag_friction" => config.drag_friction = value,
                "scale_friction" => config.scale_friction = value,
                other => return Err(format!("{location}: unknown config key `{other}`")),
            }
        }
        Ok(config)
    }
}
