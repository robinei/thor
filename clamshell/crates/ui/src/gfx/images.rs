//! Box art and other images. They are decoded and scaled on a worker thread, kept in memory by game and
//! pixel size, and drawn by blending, with no allocation per frame. A recording refers to a loaded image by an
//! [`Image`].
//!
//! The view asks for what it draws. An image that isn't loaded is requested once and drawn as a
//! placeholder until it arrives. When the cache goes over its budget, the images drawn least recently on
//! either screen are dropped, but never one the latest frame of a screen drew.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use tiny_skia::{IntSize, Pixmap};

use crate::Screen;
use crate::screens::Screens;

/// Bytes of decoded images kept before the ones drawn least recently are dropped.
const BUDGET: usize = 96 << 20;

/// An image at one size: the entry's id, which of its images it is, and the box it is fitted into, in
/// pixels.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    pub id: Arc<str>,
    pub slot: Slot,
    pub w: u32,
    pub h: u32,
}

/// Which of an entry's images is meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    /// Its box art.
    Art,
    /// Its icon, such as an app's or a folder's.
    Icon,
    /// Its picture, such as box art or a screenshot, cropped to fill a list row's square.
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
    /// `None` if the file couldn't be read. It is not asked for again.
    pixmap: Option<Pixmap>,
    /// The frame it was last drawn in, counting the frames of both screens.
    used: u64,
    /// Whether each screen's latest frame drew it.
    in_latest: Screens<bool>,
    /// Which load this is, so that a recording made before an image was loaded or reloaded differs from
    /// one made after.
    generation: u32,
}

/// A loaded image as a recording refers to it: where it is kept, which load it is, and its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Image {
    index: u32,
    generation: u32,
    w: u32,
    h: u32,
}

impl Image {
    pub fn width(&self) -> u32 {
        self.w
    }

    pub fn height(&self) -> u32 {
        self.h
    }
}

pub struct Images {
    /// Where each image is kept in `entries`. A slot left by a dropped image is reused.
    index: HashMap<Key, usize>,
    entries: Vec<Option<Entry>>,
    pending: HashSet<Key>,
    /// Where requests go. `None` decodes at once, which snapshots use.
    worker: Option<Sender<Request>>,
    /// The frame being recorded, counting the frames of both screens.
    frame: u64,
    generation: u32,
}

impl Images {
    pub fn new(worker: Sender<Request>) -> Self {
        Images { worker: Some(worker), ..Images::synchronous() }
    }

    /// Decodes on request, in the calling thread.
    pub fn synchronous() -> Self {
        Images { index: HashMap::new(), entries: Vec::new(), pending: HashSet::new(), worker: None, frame: 0, generation: 0 }
    }

    /// The image of `id` in `slot` fitted into `w`x`h` pixels, if it is loaded. If not, it is asked for.
    pub fn get(&mut self, id: &Arc<str>, slot: Slot, path: &Path, w: u32, h: u32) -> Option<Image> {
        let key = Key { id: id.clone(), slot, w, h };
        if !self.index.contains_key(&key) {
            match &self.worker {
                Some(worker) => {
                    if !self.pending.contains(&key) {
                        match worker.send(Request { key: key.clone(), path: path.to_path_buf() }) {
                            Ok(()) => {
                                self.pending.insert(key);
                            }
                            Err(e) => log::trace!("image {}: {e}", path.display()),
                        }
                    }
                    return None;
                }
                None => {
                    let pixmap = load(path, w, h, slot == Slot::Thumb);
                    self.insert(key.clone(), pixmap);
                }
            }
        }
        let index = *self.index.get(&key)?;
        let entry = self.entries.get_mut(index)?.as_mut()?;
        entry.used = self.frame;
        let pixmap = entry.pixmap.as_ref()?;
        Some(Image { index: u32::try_from(index).ok()?, generation: entry.generation, w: pixmap.width(), h: pixmap.height() })
    }

    /// The pixels of `image`, unless it has been dropped since.
    pub fn pixmap(&self, image: Image) -> Option<&Pixmap> {
        let entry = self.entries.get(image.index as usize)?.as_ref()?;
        (entry.generation == image.generation).then_some(entry.pixmap.as_ref()).flatten()
    }

    fn insert(&mut self, key: Key, pixmap: Option<Pixmap>) {
        self.generation = self.generation.wrapping_add(1);
        let entry = Entry { pixmap, used: self.frame, in_latest: Screens::default(), generation: self.generation };
        match self.index.get(&key).copied().or_else(|| self.entries.iter().position(Option::is_none)) {
            Some(i) => {
                if let Some(slot) = self.entries.get_mut(i) {
                    *slot = Some(entry);
                }
                self.index.insert(key, i);
            }
            None => {
                self.index.insert(key, self.entries.len());
                self.entries.push(Some(entry));
            }
        }
    }

    /// A decoded image arrived from the worker.
    pub fn arrived(&mut self, key: Key, pixmap: Option<Pixmap>) {
        self.pending.remove(&key);
        self.insert(key, pixmap);
    }

    /// Called after a frame of `screen`. While the cache is over budget, drops the image drawn least
    /// recently, unless the latest frame of either screen drew it.
    pub fn end_frame(&mut self, screen: Screen) {
        let frame = self.frame;
        let mut bytes = 0;
        for entry in self.entries.iter_mut().flatten() {
            entry.in_latest[screen] = entry.used == frame;
            bytes += entry.pixmap.as_ref().map_or(0, |p| p.data().len());
        }
        while bytes > BUDGET {
            let oldest = self
                .entries
                .iter_mut()
                .filter(|slot| slot.as_ref().is_some_and(|e| e.pixmap.is_some() && !e.in_latest.top && !e.in_latest.bottom))
                .min_by_key(|slot| slot.as_ref().map_or(u64::MAX, |e| e.used));
            let Some(slot) = oldest else { break };
            bytes -= slot.take().and_then(|e| e.pixmap).map_or(0, |p| p.data().len());
        }
        let entries = &self.entries;
        self.index.retain(|_, i| entries.get(*i).is_some_and(Option::is_some));
        self.frame += 1;
    }
}

/// Decodes `path` into premultiplied RGBA and scales it to fit within `w`x`h`, keeping its shape. With
/// `crop`, it is scaled to fill the box instead. An SVG, such as an app icon, is rendered at that size.
pub fn load(path: &Path, w: u32, h: u32, crop: bool) -> Option<Pixmap> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) {
        return load_svg(path, w, h);
    }
    let image = image::ImageReader::open(path)
        .and_then(image::ImageReader::with_guessed_format)
        .map_err(image::ImageError::from)
        .and_then(image::ImageReader::decode)
        .inspect_err(|e| log::debug!("image {}: {e}", path.display()))
        .ok()?;
    let (iw, ih) = (image.width().max(1), image.height().max(1));
    // Fit within w by h, or with `crop`, fill it and keep the middle.
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
    let data = std::fs::read(path).inspect_err(|e| log::debug!("image {}: {e}", path.display())).ok()?;
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).inspect_err(|e| log::debug!("image {}: {e}", path.display())).ok()?;
    let size = tree.size();
    let scale = (w as f32 / size.width()).min(h as f32 / size.height());
    let (tw, th) = (((size.width() * scale).round() as u32).max(1), ((size.height() * scale).round() as u32).max(1));
    let mut pixmap = Pixmap::new(tw, th)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Some(pixmap)
}

/// Starts the decoding thread. It takes requests from `requests` and passes the decoded images to `done`.
pub fn spawn_worker(requests: Receiver<Request>, done: impl Fn(Key, Option<Pixmap>) -> bool + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new().name("images".into()).spawn(move || {
        for Request { key, path } in requests {
            let pixmap = load(&path, key.w, key.h, key.slot == Slot::Thumb);
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
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("half.png");
        // A 60x90 image in half-transparent red.
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

    #[test]
    fn eviction_drops_the_least_recently_drawn_and_keeps_both_screens_latest() {
        let mut images = Images::synchronous();
        // A pixmap of a quarter of the budget.
        let side = ((BUDGET / 4 / 4) as f64).sqrt() as u32;
        let image = |n: usize| Key { id: format!("{n}").into(), slot: Slot::Art, w: side, h: side };
        let draw = |images: &mut Images, n: usize| {
            let key = image(n);
            if !images.index.contains_key(&key) {
                images.insert(key.clone(), Pixmap::new(side, side));
            }
            let entry = images.index.get(&key).and_then(|&i| images.entries.get_mut(i)).and_then(Option::as_mut).unwrap();
            entry.used = images.frame;
        };
        let kept = |images: &Images| (0..6).filter(|&n| images.index.contains_key(&image(n))).collect::<Vec<_>>();
        // The bottom shows 0 and 1, the top 2.
        draw(&mut images, 0);
        draw(&mut images, 1);
        images.end_frame(Screen::Bottom);
        draw(&mut images, 2);
        images.end_frame(Screen::Top);
        // The top draws 3, then 4 and 5: over budget, and only 2 and 3 are no screen's latest.
        draw(&mut images, 3);
        images.end_frame(Screen::Top);
        assert_eq!(kept(&images), [0, 1, 2, 3]);
        draw(&mut images, 4);
        draw(&mut images, 5);
        images.end_frame(Screen::Top);
        assert_eq!(kept(&images), [0, 1, 4, 5], "the bottom's images stay while it shows them");
        // The bottom draws 1 and 2 again: 0 is no screen's latest any more and the oldest, so it goes.
        draw(&mut images, 1);
        draw(&mut images, 2);
        images.end_frame(Screen::Bottom);
        assert_eq!(kept(&images), [1, 2, 4, 5]);
    }
}
