//! Glass through the headless canvas: where it samples, whether the blur is real, and a fixture of
//! a capsule, a panel and a focus halo over a busy cover shelf. Test only, not in the app.

use egui::{pos2, vec2, Color32, Context, LayerId, Rect, Shape, TextureHandle, TexturesDelta};

use super::glass::{self, Material, Tier};
use crate::live_glass::{self, LIVE_BACKDROP};
use crate::shot::Canvas;

/// A context of its own at one scale, and every texture it uploaded.
struct Fixture {
    ctx: Context,
    textures: TexturesDelta,
    size: egui::Vec2,
    ppp: f32,
    time: f64,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.textures.clear();
    }
}

impl Fixture {
    fn new(size: egui::Vec2, ppp: f32, tier: Tier) -> Self {
        crate::theme::own_palette();
        let ctx = Context::default();
        live_glass::set_tier(&ctx, tier);
        Self {
            ctx,
            textures: TexturesDelta::default(),
            size,
            ppp,
            time: 0.0,
        }
    }

    /// Lays the frame out twice, so the scale has landed, and paints the last pass.
    fn paint(&mut self, frame: &dyn Fn(&Context, Rect) -> Vec<Shape>) -> Canvas {
        let mut last = Vec::new();
        for _ in 0..2 {
            self.time += 0.05;
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, self.size)),
                time: Some(self.time),
                ..Default::default()
            };
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .native_pixels_per_point = Some(self.ppp);
            let output = self.ctx.run_ui(input, |ui| {
                let painter = ui.ctx().layer_painter(LayerId::background());
                painter.extend(frame(ui.ctx(), ui.ctx().content_rect()));
            });
            self.textures.append(output.textures_delta);
            last = output.shapes;
        }
        let ppp = self.ctx.pixels_per_point();
        let primitives = self.ctx.tessellate(last, ppp);
        let mut canvas = Canvas::new(
            (self.size.x * ppp).round() as usize,
            (self.size.y * ppp).round() as usize,
        );
        canvas.update_textures(&self.textures);
        canvas.paint(&primitives, ppp);
        canvas
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(pos2(x, y), vec2(w, h))
}

/// The glass's shapes behind `content`, through the boundary as a page would.
fn with_glass(ctx: &Context, mut content: Vec<Shape>, chrome: &[(Rect, f32, Material)]) -> Vec<Shape> {
    if glass::tier(ctx) == Tier::Live {
        let screen = ctx.content_rect();
        let regions = chrome.iter().map(|(r, _, _)| *r).collect();
        content.push(Shape::Callback(egui::epaint::PaintCallback {
            rect: screen,
            callback: std::sync::Arc::new(glass::Boundary { regions }),
        }));
    }
    for (rect, r, material) in chrome {
        glass::surface(&mut content, ctx, *rect, *r, *material);
    }
    content
}

/// Black left of `edge` and white right of it, in points.
fn split(screen: Rect, edge: f32) -> Vec<Shape> {
    let black = Rect::from_min_max(screen.min, pos2(edge, screen.max.y));
    let white = Rect::from_min_max(pos2(edge, screen.min.y), screen.max);
    vec![
        Shape::rect_filled(black, 0.0, Color32::BLACK),
        Shape::rect_filled(white, 0.0, Color32::WHITE),
    ]
}

/// Where along the row `y` a dark-to-light step crosses a fraction of its height, in pixels.
fn crossing(canvas: &Canvas, y: usize, from: usize, to: usize, fraction: f32) -> f32 {
    let value = |x: usize| canvas.pixel(x, y)[1];
    let (low, high) = (value(from), value(to - 1));
    assert!(high > low + 0.1, "a step to measure: {low} to {high}");
    let level = low + (high - low) * fraction;
    for x in from..to - 1 {
        let (a, b) = (value(x), value(x + 1));
        if a <= level && b > level {
            return x as f32 + 0.5 + (level - a) / (b - a).max(1e-6);
        }
    }
    panic!("the row never crosses {level}");
}

#[test]
fn live_glass_samples_what_is_behind_it_with_no_offset_and_really_blurs() {
    for ppp in [1.0, 1.25, 1.5, 2.0] {
        let mut fixture = Fixture::new(vec2(640.0, 400.0), ppp, Tier::Live);
        let (bar, edge) = (rect(40.0, 100.0, 560.0, 120.0), 317.0);
        let canvas = fixture.paint(&|ctx, screen| {
            with_glass(ctx, split(screen, edge), &[(bar, 24.0, Material::Chrome)])
        });
        let y = (bar.center().y * ppp) as usize;
        let (from, to) = (((bar.left() + 30.0) * ppp) as usize, ((bar.right() - 30.0) * ppp) as usize);
        let middle = crossing(&canvas, y, from, to, 0.5);
        let wanted = edge * ppp;
        assert!(
            (middle - wanted).abs() <= 2.0,
            "at {ppp}: the step is at {middle}, the content's edge at {wanted}"
        );
        let wide = crossing(&canvas, y, from, to, 0.9) - crossing(&canvas, y, from, to, 0.1);
        assert!(wide > 12.0, "at {ppp}: a real blur spreads the step, here {wide} pixels");
    }
}

#[test]
fn baked_glass_samples_its_ambient_in_place_and_ignores_the_content() {
    for ppp in [1.0, 1.5] {
        let mut fixture = Fixture::new(vec2(640.0, 400.0), ppp, Tier::Baked);
        // Black left, white right, sharp: the same kind of step, in the ambient alone.
        let ambient = fixture.ctx.load_texture(
            "ambient",
            egui::ColorImage::new([2, 1], vec![Color32::BLACK, Color32::WHITE]),
            egui::TextureOptions::NEAREST,
        );
        let screen = rect(0.0, 0.0, 640.0, 400.0);
        glass::set_backdrop(
            &fixture.ctx,
            glass::Backdrop {
                base: Some(glass::Layer {
                    texture: ambient.id(),
                    alpha: 1.0,
                    area: screen,
                }),
                top: None,
            },
        );
        let bar = rect(40.0, 100.0, 560.0, 120.0);
        let paint = |fixture: &mut Fixture, content: Color32| {
            fixture.paint(&|ctx, screen| {
                with_glass(
                    ctx,
                    vec![Shape::rect_filled(screen, 0.0, content)],
                    &[(bar, 24.0, Material::Chrome)],
                )
            })
        };
        let canvas = paint(&mut fixture, Color32::from_rgb(200, 0, 0));
        let y = (bar.center().y * ppp) as usize;
        let (from, to) = (((bar.left() + 30.0) * ppp) as usize, ((bar.right() - 30.0) * ppp) as usize);
        let middle = crossing(&canvas, y, from, to, 0.5);
        assert!((middle - 320.0 * ppp).abs() <= 1.0, "at {ppp}: {middle}");
        let other = paint(&mut fixture, Color32::from_rgb(0, 0, 200));
        let at = ((bar.center().x * 0.5) * ppp) as usize;
        assert_eq!(
            canvas.pixel(at, y),
            other.pixel(at, y),
            "live content does not show through baked glass"
        );
    }
}

#[test]
fn solid_glass_has_no_boundary_and_samples_nothing() {
    crate::theme::own_palette();
    crate::theme::set_high_contrast(true);
    let ctx = Context::default();
    live_glass::set_tier(&ctx, Tier::Live);
    assert_eq!(glass::tier(&ctx), Tier::Solid);
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let painter = ui.painter();
        glass::boundary(painter, vec![rect(0.0, 0.0, 100.0, 48.0)]);
        let mut shapes = Vec::new();
        glass::surface(&mut shapes, ui.ctx(), rect(0.0, 0.0, 100.0, 48.0), 24.0, Material::Chrome);
        painter.extend(shapes);
    });
    output.textures_delta.clear();
    let primitives = ctx.tessellate(std::mem::take(&mut output.shapes), 1.0);
    assert!(live_glass::find_boundary(&primitives).is_none());
    assert!(!primitives.iter().any(|p| matches!(&p.primitive,
        egui::epaint::Primitive::Mesh(m) if m.texture_id == LIVE_BACKDROP)));
    crate::theme::set_high_contrast(false);
}

#[test]
fn the_ambient_fades_in_asking_for_frames_only_while_it_runs() {
    crate::theme::own_palette();
    let ctx = Context::default();
    let texture = |name: &str| {
        ctx.load_texture(
            name,
            egui::ColorImage::new([1, 1], vec![Color32::RED]),
            egui::TextureOptions::LINEAR,
        )
    };
    let (first, second) = (texture("first"), texture("second"));
    let area = rect(0.0, 0.0, 100.0, 100.0);
    let mut fade = glass::CrossFade::default();
    let mut pass = |time: f64, show: Option<(u64, &TextureHandle)>| {
        let input = egui::RawInput {
            time: Some(time),
            ..Default::default()
        };
        let mut backdrop = glass::Backdrop::default();
        let mut output = ctx.run_ui(input, |ui| {
            if let Some((key, texture)) = show {
                fade.show(ui.ctx(), key, texture.clone());
            }
            backdrop = fade.backdrop(ui.ctx(), area);
        });
        output.textures_delta.clear();
        let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        (backdrop, delay < std::time::Duration::from_secs(1))
    };
    let ms = f64::from(crate::ui::design_tokens::MOTION_AMBIENT_MS) / 1000.0;
    pass(1.0, Some((1, &first)));
    pass(1.0 + ms * 2.0, None);
    let (done, _) = pass(1.0 + ms * 3.0, None);
    assert_eq!(done.base, None);
    assert_eq!(done.top.map(|l| (l.texture, l.alpha)), Some((first.id(), 1.0)));

    let (halfway, asks) = pass(10.0, Some((2, &second)));
    assert!(asks, "a running fade asks for its next frame");
    assert_eq!(halfway.base.map(|l| l.texture), Some(first.id()));
    assert_eq!(halfway.top.map(|l| (l.texture, l.alpha)), Some((second.id(), 0.0)));
    let (halfway, _) = pass(10.0 + ms / 2.0, Some((2, &second)));
    let alpha = halfway.top.expect("the new ambient").alpha;
    assert!((alpha - 0.5).abs() < 0.01, "{alpha}: the same key does not restart it");
    pass(10.0 + ms * 2.0, None);
    let (done, asks) = pass(10.0 + ms * 3.0, None);
    assert_eq!(done.base, None, "the old one lets go");
    assert!(!asks, "a finished fade leaves the launcher idle");
}

fn cover(ctx: &Context, n: usize) -> (TextureHandle, egui::ColorImage) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../app/src/debug/assets/demo/art/{}.jpg", n + 1));
    let image = image::load_from_memory(&std::fs::read(&path).expect("the demo cover"))
        .expect("a JPEG")
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, &image);
    (
        ctx.load_texture(format!("cover-{n}"), image.clone(), egui::TextureOptions::LINEAR),
        image,
    )
}

/// Rows of covers filling the window, so glass over them has something to blur.
fn shelf(screen: Rect, covers: &[TextureHandle]) -> Vec<Shape> {
    let mut out = vec![Shape::rect_filled(screen, 0.0, Color32::from_rgb(0x14, 0x16, 0x1F))];
    let (w, h, gap) = (155.0, 232.0, 16.0);
    let mut n = 0;
    let mut y = 8.0;
    while y < screen.bottom() {
        let mut x = 12.0;
        while x < screen.right() {
            let mut mesh = egui::epaint::Mesh::with_texture(covers[n % covers.len()].id());
            mesh.add_rect_with_uv(
                rect(x, y, w, h),
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            out.push(Shape::mesh(mesh));
            n += 1;
            x += w + gap;
        }
        y += h + gap;
    }
    out
}

/// The fixture for the eye: `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop --bin nightjar
/// glass_fixture_screenshots --release -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn glass_fixture_screenshots() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    for (tier, name) in [
        (Tier::Live, "live"),
        (Tier::Baked, "baked"),
        (Tier::Solid, "solid"),
    ] {
        for (ppp, scale) in [(1.0, "100"), (1.5, "150")] {
            let mut fixture = Fixture::new(vec2(1280.0, 800.0), ppp, Tier::Live);
            if tier == Tier::Solid {
                crate::theme::set_high_contrast(true);
            } else {
                live_glass::set_tier(&fixture.ctx, tier);
            }
            let ctx = fixture.ctx.clone();
            let loaded: Vec<_> = (0..12).map(|n| cover(&ctx, n)).collect();
            let covers: Vec<TextureHandle> = loaded.iter().map(|(t, _)| t.clone()).collect();
            let ambient = ctx.load_texture(
                "ambient",
                crate::ambient::bake(&loaded[0].1),
                egui::TextureOptions::LINEAR,
            );
            let screen = rect(0.0, 0.0, 1280.0, 800.0);
            glass::set_backdrop(
                &ctx,
                glass::Backdrop {
                    base: Some(glass::Layer {
                        texture: ambient.id(),
                        alpha: 1.0,
                        area: screen.expand2(screen.size() * 0.14),
                    }),
                    top: None,
                },
            );
            let canvas = fixture.paint(&|ctx, screen| {
                let content = shelf(screen, &covers);
                let capsule = rect(40.0, 20.0, 1200.0, 48.0);
                let panel = rect(760.0, 120.0, 460.0, 420.0);
                let chip = rect(60.0, 610.0, 150.0, 40.0);
                let mut out = with_glass(
                    ctx,
                    content,
                    &[
                        (capsule, 24.0, Material::Chrome),
                        (panel, 24.0, Material::Panel),
                        (chip, 20.0, Material::Chip),
                    ],
                );
                // A focused control on the capsule and a focused cover, each over its own halo.
                let control = rect(100.0, 28.0, 120.0, 32.0);
                glass::focus(&mut out, ctx, control, 16.0, false);
                glass::control(&mut out, ctx, control, 16.0);
                let focused = rect(183.0, 256.0, 155.0, 232.0);
                glass::focus(&mut out, ctx, focused, 12.0, false);
                let mut mesh = egui::epaint::Mesh::with_texture(covers[9].id());
                mesh.add_rect_with_uv(
                    focused,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                out.push(Shape::mesh(mesh));
                out
            });
            canvas.save(&dir.join(format!("glass-{name}-{scale}.png")));
            crate::theme::set_high_contrast(false);
        }
    }
}
