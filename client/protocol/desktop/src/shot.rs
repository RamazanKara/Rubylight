//! Test only: paints egui's output on the CPU, so page screenshots need no window, GPU or PC.
//!
//! It follows what the renderers do: premultiplied sRGB vertex colours times the bilinear texture
//! sample, blended in gamma space, clipped to each primitive's rectangle. Edges shared by two
//! triangles are filled once (a top-left style rule), so translucent fills show no seams.

use std::collections::HashMap;
use std::path::Path;

use egui::epaint::{ClippedPrimitive, ImageData, Mesh, TextureId};
use egui::{Color32, Pos2, Rect, TexturesDelta};

use crate::live_glass::{level_size, Glass, Stage, LEVELS, LIVE_BACKDROP};

struct Texture {
    size: [usize; 2],
    pixels: Vec<Color32>,
}

impl Texture {
    fn texel(&self, x: isize, y: isize) -> [f32; 4] {
        let x = x.clamp(0, self.size[0] as isize - 1) as usize;
        let y = y.clamp(0, self.size[1] as isize - 1) as usize;
        let c = self.pixels[y * self.size[0] + x];
        [c.r(), c.g(), c.b(), c.a()].map(|v| f32::from(v) / 255.0)
    }

    fn sample(&self, uv: Pos2) -> [f32; 4] {
        let x = uv.x * self.size[0] as f32 - 0.5;
        let y = uv.y * self.size[1] as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let [a, b, c, d] = [self.texel(x0, y0), self.texel(x0 + 1, y0), self.texel(x0, y0 + 1), self.texel(x0 + 1, y0 + 1)];
        std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
    }

    /// This texture drawn over a whole target of `size`.
    fn resampled(&self, size: [usize; 2]) -> Self {
        let mut pixels = Vec::with_capacity(size[0] * size[1]);
        for y in 0..size[1] {
            for x in 0..size[0] {
                let uv = Pos2::new((x as f32 + 0.5) / size[0] as f32, (y as f32 + 0.5) / size[1] as f32);
                let [r, g, b, a] = self.sample(uv).map(|v| (v * 255.0).round() as u8);
                pixels.push(Color32::from_rgba_premultiplied(r, g, b, a));
            }
        }
        Self { size, pixels }
    }
}

pub struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<[f32; 4]>,
    textures: HashMap<TextureId, Texture>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, pixels: vec![[0.0, 0.0, 0.0, 1.0]; width * height], textures: HashMap::new() }
    }

    /// Takes the texture changes of a frame; call it for every frame from the first.
    pub fn update_textures(&mut self, delta: &TexturesDelta) {
        for (id, change) in delta.set.iter().flat_map(|(id, changes)| changes.iter().map(move |c| (id, c))) {
            let ImageData::Color(image) = &change.image;
            match change.pos {
                None => {
                    self.textures.insert(*id, Texture { size: image.size, pixels: image.pixels.clone() });
                }
                Some([px, py]) => {
                    let texture = self.textures.get_mut(id).expect("a patch to a known texture");
                    for y in 0..image.size[1] {
                        for x in 0..image.size[0] {
                            texture.pixels[(py + y) * texture.size[0] + px + x] = image.pixels[y * image.size[0] + x];
                        }
                    }
                }
            }
        }
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    /// Walks the frame as the app does: with a boundary, the content goes through the same chain
    /// and the chrome samples its output.
    pub fn paint(&mut self, primitives: &[ClippedPrimitive], pixels_per_point: f32) {
        let mut glass = Glass::live();
        glass.paint(&mut Page { canvas: self, ppp: pixels_per_point }, primitives, true);
    }

    fn draw(&mut self, clip: Rect, mesh: &Mesh, ppp: f32) {
        let Some(texture) = self.textures.get(&mesh.texture_id) else { return };
        let clip = [
            (clip.min.x * ppp).round().max(0.0) as usize,
            (clip.min.y * ppp).round().max(0.0) as usize,
            ((clip.max.x * ppp).round().max(0.0) as usize).min(self.width),
            ((clip.max.y * ppp).round().max(0.0) as usize).min(self.height),
        ];
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v = [0, 1, 2].map(|k| mesh.vertices[tri[k] as usize]);
            fill_triangle(&mut self.pixels, self.width, clip, texture, v, ppp);
        }
    }

    /// The chain's passes on the CPU: each level samples the one before it bilinearly over its
    /// whole area, as the GPU's full-target quads do.
    fn blur(&self) -> Texture {
        let scene = Texture {
            size: [self.width, self.height],
            pixels: self.pixels.iter().map(|p| {
                let [r, g, b, a] = p.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
                Color32::from_rgba_premultiplied(r, g, b, a)
            }).collect(),
        };
        LEVELS.iter().fold(scene, |from, &div| {
            let (w, h) = level_size((self.width as i32, self.height as i32), div);
            from.resampled([w as usize, h as usize])
        })
    }

    /// One pixel, for tests that read what was painted.
    pub fn pixel(&self, x: usize, y: usize) -> [f32; 4] {
        self.pixels[y * self.width + x]
    }

    pub fn save(&self, path: &Path) {
        let bytes: Vec<u8> = self
            .pixels
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
            .collect();
        image::RgbImage::from_raw(self.width as u32, self.height as u32, bytes)
            .expect("the buffer fits the size")
            .save(path)
            .expect("the screenshot is written");
    }
}

/// The canvas as glass's stage. Content lands on the page itself, which is also what the scene's
/// copy to the screen ends in; the chain reads it from there.
struct Page<'a> {
    canvas: &'a mut Canvas,
    ppp: f32,
}

impl Stage for Page<'_> {
    fn size(&self) -> (i32, i32) {
        (self.canvas.width as i32, self.canvas.height as i32)
    }

    fn mesh(&mut self, clip: Rect, mesh: &Mesh) {
        self.canvas.draw(clip, mesh, self.ppp);
    }

    fn targets_work(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn begin_scene(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn end_scene(&mut self, _timed: bool) -> Result<Option<f32>, String> {
        let output = self.canvas.blur();
        self.canvas.textures.insert(LIVE_BACKDROP, output);
        Ok(None)
    }

    fn release(&mut self) {
        self.canvas.textures.remove(&LIVE_BACKDROP);
    }
}

fn fill_triangle(
    pixels: &mut [[f32; 4]],
    width: usize,
    clip: [usize; 4],
    texture: &Texture,
    mut v: [egui::epaint::Vertex; 3],
    ppp: f32,
) {
    let mut p = v.map(|v| (v.pos.x * ppp, v.pos.y * ppp));
    let edge = |a: (f32, f32), b: (f32, f32), x: f32, y: f32| (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0);
    let area = edge(p[0], p[1], p[2].0, p[2].1);
    if area == 0.0 {
        return;
    }
    if area < 0.0 {
        v.swap(1, 2);
        p.swap(1, 2);
    }
    let area = area.abs();
    // Of the two triangles on an edge, only the one that owns the edge's direction fills it.
    let owns = |a: (f32, f32), b: (f32, f32)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        dy > 0.0 || (dy == 0.0 && dx > 0.0)
    };
    let edges = [(p[1], p[2]), (p[2], p[0]), (p[0], p[1])];
    let own = edges.map(|(a, b)| owns(a, b));
    let min_x = (p.iter().map(|p| p.0).fold(f32::MAX, f32::min).floor().max(0.0) as usize).max(clip[0]);
    let max_x = (p.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(clip[2]);
    let min_y = (p.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor().max(0.0) as usize).max(clip[1]);
    let max_y = (p.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(clip[3]);
    let colors = v.map(|v| [v.color.r(), v.color.g(), v.color.b(), v.color.a()].map(|c| f32::from(c) / 255.0));
    for y in min_y..max_y {
        for x in min_x..max_x {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let w = edges.map(|(a, b)| edge(a, b, cx, cy));
            if (0..3).any(|i| w[i] < 0.0 || (w[i] == 0.0 && !own[i])) {
                continue;
            }
            let w = w.map(|w| w / area);
            let uv = Pos2::new(
                w[0] * v[0].uv.x + w[1] * v[1].uv.x + w[2] * v[2].uv.x,
                w[0] * v[0].uv.y + w[1] * v[1].uv.y + w[2] * v[2].uv.y,
            );
            let texel = texture.sample(uv);
            let src: [f32; 4] = std::array::from_fn(|i| (w[0] * colors[0][i] + w[1] * colors[1][i] + w[2] * colors[2][i]) * texel[i]);
            let dst = &mut pixels[y * width + x];
            for i in 0..4 {
                dst[i] = src[i] + dst[i] * (1.0 - src[3]);
            }
        }
    }
}
