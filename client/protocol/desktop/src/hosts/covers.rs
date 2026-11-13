//! Box art: the workers that load covers, the size each is kept at, and dropping the ones out of sight.

use super::*;

/// The largest a cover is drawn, in points: the game page's. Every card, Home's hero and a PC's
/// screen are smaller.
pub const COVER_POINTS: [f32; 2] = [420.0, 630.0];

/// The pixels of a cover kept for each pixel of the largest place it is drawn in. Kept at
/// exactly that place's size, a cover shows soft wherever it is drawn a little smaller, as the
/// game page does in a narrow window: the renderer then reads between its pixels. With half as
/// many again it shows as sharp there as a store's 600 by 900 cover always has, which stays as
/// it is on a plain screen.
const COVER_DETAIL: f32 = 1.5;

/// A cover is made smaller only when that takes it under this share of its side: one already
/// near what is kept would lose a little sharpness for a little memory.
const COVER_SHRINK_BELOW: f32 = 0.9;

/// How much the cover textures may hold, in bytes of pixels. Past it the ones longest off screen
/// are given up and load again from their files when they come back; the ones on screen stay
/// whatever they add up to. A few hundred covers fit, so a long list scrolls back and forth
/// without reloading, and a library of thousands no longer keeps every cover it ever showed.
const COVER_BUDGET: usize = 256 << 20;

/// The covers drawn or asked for since [`Hosts::covers_shown`] last took them, by texture. A set,
/// so it holds no more than there are covers however many frames pass before it is taken.
#[derive(Clone, Default)]
struct CoversUsed(std::collections::HashSet<egui::TextureId>);

/// Notes that the cover `texture` is on screen (or needed off it) in this frame, so it is not one
/// given up for room.
pub fn cover_used(ctx: &egui::Context, texture: &egui::TextureHandle) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<CoversUsed>(egui::Id::NULL).0.insert(texture.id()));
}

/// The covers noted as in use since they were last taken, for a test of the pages that note them.
#[cfg(test)]
pub(crate) fn covers_noted(ctx: &egui::Context) -> std::collections::HashSet<egui::TextureId> {
    ctx.data_mut(|d| std::mem::take(&mut d.get_temp_mut_or_default::<CoversUsed>(egui::Id::NULL).0))
}

impl Hosts {
    /// As [`Self::spawn`], for a cover: on one of the few threads covers load on.
    fn load_art(&self, work: impl FnOnce() -> Option<Event> + Send + 'static) {
        if cfg!(feature = "demo-library") {
            return;
        }
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        self.art_pool.run(move || {
            if let Some(event) = work() {
                let _ = sender.send(event);
                waker();
            }
        });
    }

    /// Fetches box art once per app, from the disk cache when present. `version` is the cover's
    /// version ([`Host::cover_version`], empty when the PC gives none); a new one fetches again,
    /// and none keeps the cover that is known. At most [`ART_WORKERS`] covers load at once.
    pub fn request_art(&mut self, id: u64, app: i32, version: &str, store: &Store) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get_mut(id) else {
            return;
        };
        let Some(version) = host.begin_art(app, version) else {
            return;
        };
        // Only a cover that was given up for room is asked for while its PC is away, for its file.
        let away = host.reach != Reach::Online;
        let record = host.connect_record();
        let cache = store.art_path(&record.unique_id, app, &version);
        let files = self.art_files.clone();
        let scale = self.art_scale;
        self.load_art(move || {
            let file = std::fs::read(&cache).ok();
            if file.is_none() && away {
                return Some(Event::ArtAway { id, app, version });
            }
            let bytes = file.or_else(|| {
                let bytes = match client_for(&identity, &record, &record.address).and_then(|c| {
                    c.box_art(app).map_err(|e| e.to_string())
                }) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        // Hosts answer 404 for apps without art.
                        if !e.starts_with("not found") {
                            eprintln!("nightjar: box art for app {app}: {e}");
                        }
                        return None;
                    }
                };
                if let Some(dir) = cache.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if std::fs::write(&cache, &bytes).is_ok() {
                    files.lock().unwrap_or_else(std::sync::PoisonError::into_inner).saved(&cache, app);
                }
                Some(bytes)
            });
            let (image, ambient, sized) = art_and_ambient(bytes, scale);
            Some(Event::Art { id, app, version, image, ambient, sized })
        });
    }

    /// What the frame just drawn means for the covers. Call it after each launcher frame.
    ///
    /// The window's pixels per point are what covers are sized for from here on, and when they
    /// grew (a larger text size, a window moved to a denser screen) the covers made smaller for
    /// fewer are loaded again, each showing as it is until the sharper one lands. Then, when
    /// the cover textures hold more than their budget, the ones longest off screen are given up.
    pub fn covers_shown(&mut self, ctx: &egui::Context, store: &Store) {
        self.art_frame += 1;
        let used = ctx.data_mut(|d| std::mem::take(&mut d.get_temp_mut_or_default::<CoversUsed>(egui::Id::NULL).0));
        for texture in used {
            self.art_used.insert(texture, self.art_frame);
        }
        let scale = ctx.pixels_per_point();
        let grew = scale > self.art_scale;
        self.art_scale = scale;
        if grew || self.art_landed {
            self.sharpen_covers(store);
        }
        if std::mem::take(&mut self.art_landed) {
            self.drop_covers(COVER_BUDGET);
        }
    }

    /// Loads again, from their files, the covers that were made smaller for fewer pixels per
    /// point than the window has now.
    fn sharpen_covers(&mut self, store: &Store) {
        let scale = self.art_scale;
        let mut jobs = Vec::new();
        for host in &mut self.list {
            for (&app, sized) in &mut host.art_sized {
                if *sized >= scale || !matches!(host.art.get(&app), Some(Art::Ready(_))) {
                    continue;
                }
                let Some(version) = host.art_versions.get(&app) else { continue };
                // Taken as sized for the new scale from here, so it is asked for once.
                *sized = scale;
                jobs.push((host.id, app, version.clone(), store.art_path(&host.record.unique_id, app, version)));
            }
        }
        for (id, app, version, cache) in jobs {
            self.load_art(move || {
                // Only from its file: the cover that shows is good enough to keep if that is gone.
                let image = art_image(std::fs::read(&cache).ok())?;
                let (image, sized) = cover_sized(image, scale);
                Some(Event::ArtSharper { id, app, version, image, sized })
            });
        }
    }

    /// Gives up cover textures until they hold no more than `budget` bytes: the ones longest off
    /// screen first, and never one that was drawn or asked for in the frame just shown, so a
    /// cover on screen cannot be dropped and loaded in turns. The ambient stays, so a page's
    /// backdrop does not change with it.
    pub(super) fn drop_covers(&mut self, budget: usize) {
        // The demo's covers are drawn into textures and have no file to come back from.
        if cfg!(feature = "demo-library") {
            return;
        }
        let mut covers = Vec::new();
        let mut held = 0;
        for (at, host) in self.list.iter().enumerate() {
            for (&app, art) in &host.art {
                if let Art::Ready(texture) = art {
                    held += texture.byte_size();
                    covers.push((self.art_used.get(&texture.id()).copied().unwrap_or(0), at, app, texture.id(), texture.byte_size()));
                }
            }
        }
        // The textures that are gone (dropped here, replaced, their PC removed) need no stamp.
        let live: std::collections::HashSet<egui::TextureId> = covers.iter().map(|cover| cover.3).collect();
        self.art_used.retain(|texture, _| live.contains(texture));
        if held <= budget {
            return;
        }
        covers.sort_unstable_by_key(|&(used, at, app, ..)| (used, at, app));
        for (used, at, app, texture, bytes) in covers {
            if held <= budget || used >= self.art_frame {
                break;
            }
            let host = &mut self.list[at];
            host.art.insert(app, Art::Dropped);
            host.art_sized.remove(&app);
            self.art_used.remove(&texture);
            held -= bytes;
        }
    }
}

/// How many covers load at once. A long list asks for its covers together, and each one is a
/// thread and a TLS handshake with the PC; the rest wait their turn.
pub(super) const ART_WORKERS: usize = 4;

pub(super) type ArtJob = Box<dyn FnOnce() + Send>;

/// The covers waiting to load and the threads loading them, at most [`ART_WORKERS`].
#[derive(Default)]
pub(super) struct ArtQueue {
    pub(super) waiting: std::collections::VecDeque<ArtJob>,
    pub(super) workers: usize,
}

/// Loads covers in the order they were asked for. A worker lives while covers wait and ends with
/// the last one, so an idle launcher keeps no thread.
#[derive(Clone, Default)]
pub(super) struct ArtPool(Arc<Mutex<ArtQueue>>);

impl ArtPool {
    pub(super) fn queue(&self) -> std::sync::MutexGuard<'_, ArtQueue> {
        // Held only to move a job: a cover that panics does so outside it.
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn run(&self, job: impl FnOnce() + Send + 'static) {
        let mut queue = self.queue();
        queue.waiting.push_back(Box::new(job));
        if queue.workers == ART_WORKERS {
            return;
        }
        queue.workers += 1;
        drop(queue);
        let pool = self.clone();
        if std::thread::Builder::new().name("boxart".into()).spawn(move || pool.work()).is_err() {
            // The covers stay queued for the workers there are, or the next one that starts.
            self.queue().workers -= 1;
        }
    }

    pub(super) fn work(&self) {
        loop {
            // The seat is given up under the lock that found nothing waiting, so a cover queued
            // right after starts a worker of its own.
            let job = {
                let mut queue = self.queue();
                match queue.waiting.pop_front() {
                    Some(job) => job,
                    None => {
                        queue.workers -= 1;
                        return;
                    }
                }
            };
            // A cover that panics takes neither the worker nor the covers behind it.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
        }
    }
}

/// An app's cover from its box art bytes. A host's grey placeholder is no cover, so the card shows the title.
pub(super) fn art_image(bytes: Option<Vec<u8>>) -> Option<egui::ColorImage> {
    bytes.filter(|b| !nightjar_core::http::is_placeholder_box_art(b)).and_then(|b| decode_image(&b))
}

/// A cover and its ambient, both off the UI thread: the first draw only uploads two finished images.
/// The ambient is baked from the cover as its file has it; the cover is then made no larger than a
/// window of `scale` pixels per point needs ([`cover_sized`]), and the last value says for which.
pub(super) fn art_and_ambient(bytes: Option<Vec<u8>>, scale: f32) -> (Option<egui::ColorImage>, Option<egui::ColorImage>, Option<f32>) {
    let Some(image) = art_image(bytes) else { return (None, None, None) };
    let ambient = crate::ambient::bake(&image);
    let (image, sized) = cover_sized(image, scale);
    (Some(image), Some(ambient), sized)
}

/// The size a cover of `size` pixels is kept at for a window of `scale` pixels per point, or None
/// to keep it as it is. A cover fills its place and is cut to it (`cards::cover_uv`), so it is
/// the side that needs the most that decides, and what shows keeps [`COVER_DETAIL`] of its
/// pixels for each of the screen's.
pub(super) fn cover_size(size: [usize; 2], scale: f32) -> Option<[usize; 2]> {
    let [w, h] = size;
    if w == 0 || h == 0 {
        return None;
    }
    let detail = scale * COVER_DETAIL;
    let share = (COVER_POINTS[0] * detail / w as f32).max(COVER_POINTS[1] * detail / h as f32);
    // Up to the next pixel, less what the arithmetic is off by (900 by 0.7 is not to become 631).
    let side = |from: usize| ((from as f32 * share - 0.01).ceil() as usize).clamp(1, from);
    (share < COVER_SHRINK_BELOW).then(|| [side(w), side(h)])
}

/// A cover no larger than a window of `scale` pixels per point needs, and that scale when it
/// was made smaller. Its pixels are already multiplied by their alpha, which is what resampling
/// wants: a clear pixel's colour does not bleed into the ones beside it.
pub(super) fn cover_sized(image: egui::ColorImage, scale: f32) -> (egui::ColorImage, Option<f32>) {
    let Some([w, h]) = cover_size(image.size, scale) else { return (image, None) };
    let [from_w, from_h] = image.size;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|pixel| pixel.to_array()).collect();
    let Some(full) = image::RgbaImage::from_raw(from_w as u32, from_h as u32, bytes) else {
        return (image, None);
    };
    let small = image::imageops::resize(&full, w as u32, h as u32, image::imageops::FilterType::Lanczos3);
    (egui::ColorImage::from_rgba_premultiplied([w, h], small.as_raw()), Some(scale))
}

pub(super) fn decode_image(bytes: &[u8]) -> Option<egui::ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}
