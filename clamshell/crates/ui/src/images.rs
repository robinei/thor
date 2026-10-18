//! Box art and other images: decoded and scaled on a worker thread, kept in memory by game and pixel
//! size, drawn by blending (no allocation per frame). The view asks for what it draws; a miss is
//! requested once and drawn as a placeholder until it arrives; images not drawn lately are dropped when
//! the cache passes its budget.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use tiny_skia::{IntSize, Pixmap};

/// Bytes of decoded images kept before the ones not drawn lately are dropped.
const BUDGET: usize = 96 << 20;

/// An image at one size: the entry's id, which of its images, and the box it is fitted into, in pixels.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    pub id: Arc<str>,
    pub slot: Slot,
    pub w: u32,
    pub h: u32,
}

/// Which of an entry's images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    /// Its box art.
    Art,
    /// Its icon (an app's, a folder's).
    Icon,
    /// Its picture (box art, a screenshot) cropped to fill a list row's square.
    Thumb,
    /// A screenshot.
    Shot,
}

/// A request for the worker.
pub struct Request {
    pub key: Key,
    pub path: PathBuf,
}

struct Entry {
    /// `None`: the file couldn't be read; not asked for again.
    pixmap: Option<Pixmap>,
    used: u64,
}

pub struct Images {
    cache: HashMap<Key, Entry>,
    pending: HashSet<Key>,
    /// Where requests go; `None` decodes at once (snapshots).
    worker: Option<Sender<Request>>,
    frame: u64,
}

impl Images {
    pub fn new(worker: Sender<Request>) -> Self {
        Images { cache: HashMap::new(), pending: HashSet::new(), worker: Some(worker), frame: 0 }
    }

    /// Decodes on request, in the calling thread.
    pub fn synchronous() -> Self {
        Images { cache: HashMap::new(), pending: HashSet::new(), worker: None, frame: 0 }
    }

    /// The image of `id` in `slot` fitted into `w`x`h` pixels, if loaded; asks for it if not.
    pub fn get(&mut self, id: &Arc<str>, slot: Slot, path: &Path, w: u32, h: u32) -> Option<&Pixmap> {
        let key = Key { id: id.clone(), slot, w, h };
        if !self.cache.contains_key(&key) {
            match &self.worker {
                Some(worker) => {
                    if !self.pending.contains(&key) && worker.send(Request { key: key.clone(), path: path.to_path_buf() }).is_ok() {
                        self.pending.insert(key);
                    }
                    return None;
                }
                None => {
                    let pixmap = load(path, w, h, slot == Slot::Thumb);
                    self.cache.insert(key.clone(), Entry { pixmap, used: self.frame });
                }
            }
        }
        let entry = self.cache.get_mut(&key)?;
        entry.used = self.frame;
        entry.pixmap.as_ref()
    }

    /// A decoded image arrived from the worker.
    pub fn arrived(&mut self, key: Key, pixmap: Option<Pixmap>) {
        self.pending.remove(&key);
        self.cache.insert(key, Entry { pixmap, used: self.frame });
    }

    /// After a frame: drops images that weren't drawn in it while the cache is over budget.
    pub fn end_frame(&mut self) {
        let bytes: usize = self.cache.values().filter_map(|e| e.pixmap.as_ref()).map(|p| p.data().len()).sum();
        if bytes > BUDGET {
            let frame = self.frame;
            self.cache.retain(|_, e| e.used == frame);
        }
        self.frame += 1;
    }
}

/// Decodes `path` and scales it to fit within `w`x`h` (keeping its shape), premultiplied RGBA. SVG
/// (app icons) is rendered at that size.
pub fn load(path: &Path, w: u32, h: u32, crop: bool) -> Option<Pixmap> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) {
        return load_svg(path, w, h);
    }
    let image = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let (iw, ih) = (image.width().max(1), image.height().max(1));
    // Fitted in w by h, or (`crop`) filling it, its middle kept.
    let scaled = if crop {
        image.resize_to_fill(w.max(1), h.max(1), image::imageops::FilterType::Triangle).into_rgba8()
    } else {
        let scale = (w as f32 / iw as f32).min(h as f32 / ih as f32);
        let (tw, th) = (((iw as f32 * scale).round() as u32).max(1), ((ih as f32 * scale).round() as u32).max(1));
        image.resize_exact(tw, th, image::imageops::FilterType::Triangle).into_rgba8()
    };
    let (tw, th) = (scaled.width(), scaled.height());
    let mut data = scaled.into_raw();
    for px in data.chunks_exact_mut(4) {
        if let [r, g, b, a] = px {
            let alpha = u16::from(*a);
            *r = (u16::from(*r) * alpha / 255) as u8;
            *g = (u16::from(*g) * alpha / 255) as u8;
            *b = (u16::from(*b) * alpha / 255) as u8;
        }
    }
    Pixmap::from_vec(data, IntSize::from_wh(tw, th)?)
}

fn load_svg(path: &Path, w: u32, h: u32) -> Option<Pixmap> {
    let data = std::fs::read(path).ok()?;
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = (w as f32 / size.width()).min(h as f32 / size.height());
    let (tw, th) = (((size.width() * scale).round() as u32).max(1), ((size.height() * scale).round() as u32).max(1));
    let mut pixmap = Pixmap::new(tw, th)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Some(pixmap)
}

/// The decoding thread: requests in, decoded images out through `done`.
pub fn spawn_worker(requests: Receiver<Request>, done: impl Fn(Key, Option<Pixmap>) -> bool + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new().name("images".into()).spawn(move || {
        for Request { key, path } in requests {
            let pixmap = load(&path, key.w, key.h, key.slot == Slot::Thumb);
            if pixmap.is_none() {
                log::debug!("image {}: can't load", path.display());
            }
            if !done(key, pixmap) {
                return;
            }
        }
    })?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn scales_to_fit_and_premultiplies() {
        let dir = std::env::temp_dir().join(format!("clamshell-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("half.png");
        // 60x90, half-transparent red.
        image::RgbaImage::from_pixel(60, 90, image::Rgba([255, 0, 0, 128])).save(&path).unwrap();
        let p = load(&path, 40, 40, false).expect("decoded");
        assert_eq!((p.width(), p.height()), (27, 40));
        let px = p.pixel(0, 0).unwrap();
        assert_eq!((px.red(), px.alpha()), (128, 128));
        assert!(load(&dir.join("missing.png"), 10, 10, false).is_none());
        let svg = dir.join("icon.svg");
        std::fs::write(&svg, r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"><rect width="16" height="8" fill="#00f"/></svg>"##).unwrap();
        let p = load(&svg, 64, 64, false).expect("rendered");
        assert_eq!((p.width(), p.height()), (64, 32));
        assert_eq!(p.pixel(10, 10).map(|px| (px.blue(), px.alpha())), Some((255, 255)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_miss_is_requested_once() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut images = Images::new(tx);
        let id: Arc<str> = "game".into();
        assert!(images.get(&id, Slot::Art, Path::new("/x.png"), 10, 10).is_none());
        assert!(images.get(&id, Slot::Art, Path::new("/x.png"), 10, 10).is_none());
        assert_eq!(rx.try_iter().count(), 1);
    }
}
