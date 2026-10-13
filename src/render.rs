use rayon::prelude::*;

use crate::navigation::{Camera, Flashlight, Vec2};

const BACKGROUND: u32 = 0xFF1A1A1A;

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

pub struct Scene<'a> {
    pub image: &'a Image,
    pub camera: &'a Camera,
    pub flashlight: &'a Flashlight,
    pub cursor: Vec2,
    pub mirror: bool,
}

fn darken(pixel: u32, factor: u32) -> u32 {
    let r = (((pixel >> 16) & 0xFF) * factor) >> 8;
    let g = (((pixel >> 8) & 0xFF) * factor) >> 8;
    let b = ((pixel & 0xFF) * factor) >> 8;
    0xFF000000 | (r << 16) | (g << 8) | b
}

pub fn draw(target: &mut [u32], width: u32, height: u32, scene: &Scene) {
    let image = scene.image;
    let camera = scene.camera;
    let inv_scale = 1.0 / camera.scale;
    let image_w = image.width as f32;
    let image_h = image.height as f32;
    let origin_x = image_w * 0.5 + camera.position.x - width as f32 * 0.5 * inv_scale;
    let origin_y = image_h * 0.5 + camera.position.y - height as f32 * 0.5 * inv_scale;

    let shadow = scene.flashlight.shadow;
    let lit_radius = scene.flashlight.radius * camera.scale;
    let lit_radius_sq = lit_radius * lit_radius;
    let shade_factor = ((1.0 - shadow) * 256.0) as u32;

    target
        .par_chunks_exact_mut(width as usize)
        .enumerate()
        .for_each(|(y, row)| {
            let center_y = y as f32 + 0.5;
            let image_y = (origin_y + center_y * inv_scale).floor();
            if image_y < 0.0 || image_y >= image_h {
                row.fill(BACKGROUND);
                return;
            }
            let source_row = &image.pixels[image_y as usize * image.width as usize..][..image.width as usize];
            let dy = center_y - scene.cursor.y;
            let dy_sq = dy * dy;

            for (x, out) in row.iter_mut().enumerate() {
                let center_x = x as f32 + 0.5;
                let mut image_x = origin_x + center_x * inv_scale;
                if scene.mirror {
                    image_x = image_w - image_x;
                }
                let image_x = image_x.floor();
                if image_x < 0.0 || image_x >= image_w {
                    *out = BACKGROUND;
                    continue;
                }
                let pixel = source_row[image_x as usize];
                *out = if shadow > 0.0 {
                    let dx = center_x - scene.cursor.x;
                    if dx * dx + dy_sq < lit_radius_sq {
                        pixel
                    } else {
                        darken(pixel, shade_factor)
                    }
                } else {
                    pixel
                };
            }
        });
}
