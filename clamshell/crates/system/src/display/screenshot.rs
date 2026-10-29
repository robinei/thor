//! Screenshots of both screens: the compositor's pixels, saved as PNG.

use std::path::PathBuf;

use clamshell_state::ScreenshotPixels;

/// Encode compositor pixels on the system worker, without changing either screen's render format.
pub fn save(dir: Option<&PathBuf>, pixels: ScreenshotPixels) -> Result<String, String> {
    let (width, height) = pixels.size;
    let length = usize::try_from(width).ok().and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h))).and_then(|n| n.checked_mul(4));
    if width == 0 || height == 0 || length != Some(pixels.rgba.len()) {
        return Err("invalid screenshot dimensions or pixel length".into());
    }
    let file = filename(dir)?;
    // Written beside it first, so that a half-written screenshot never shows under its name.
    let part = file.with_extension("png.part");
    let written = (|| {
        let output = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(output), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&pixels.rgba).map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())?;
        std::fs::rename(&part, &file).map_err(|e| e.to_string())
    })();
    if let Err(e) = written {
        for leftover in [&part, &file] {
            if let Err(e) = std::fs::remove_file(leftover).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }) {
                log::warn!("screenshot: removing {}: {e}", leftover.display());
            }
        }
        return Err(e);
    }
    let path = file.to_string_lossy().into_owned();
    log::info!("screenshot: {path}");
    Ok(path)
}

/// A name for a screenshot taken now, held by an empty file so that another in the same second gets the
/// next: `clamshell-<time>.png`, then `clamshell-<time>-2.png` and so on.
fn filename(dir: Option<&PathBuf>) -> Result<PathBuf, String> {
    let dir = dir.ok_or("no folder for screenshots (no HOME)")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stamp = timestamp(crate::time::clock::unix_secs());
    for n in 1u32.. {
        let file = dir.join(if n == 1 { format!("clamshell-{stamp}.png") } else { format!("clamshell-{stamp}-{n}.png") });
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&file) {
            Ok(_) => return Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{}: {e}", file.display())),
        }
    }
    Err("no free name".into())
}

/// A UTC timestamp for file names: 20261015-103205.
fn timestamp(secs: u64) -> String {
    let at = i64::try_from(secs).ok().and_then(|s| jiff::Timestamp::from_second(s).ok()).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    at.strftime("%Y%m%d-%H%M%S").to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn captured_pixels_are_saved_with_their_size_and_row_order() {
        let directory = tempfile::tempdir().unwrap();
        let pixels = ScreenshotPixels { size: (2, 2), rgba: vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255] };
        let expected = pixels.rgba.clone();
        let path = save(Some(&directory.path().to_path_buf()), pixels).unwrap();
        let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut bytes = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut bytes).unwrap();
        assert_eq!((info.width, info.height), (2, 2));
        assert_eq!(bytes, expected);
    }
    /// Two screenshots in one second are both kept.
    #[test]
    fn screenshots_in_the_same_second_both_stay() {
        let directory = tempfile::tempdir().unwrap();
        let dir = directory.path().to_path_buf();
        let pixels = ScreenshotPixels { size: (1, 1), rgba: vec![0, 0, 0, 255] };
        let a = save(Some(&dir), pixels.clone()).unwrap();
        let b = save(Some(&dir), pixels).unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "and no part left");
    }

    #[test]
    fn invalid_captures_and_unwritable_folders_return_errors() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().to_path_buf();
        for pixels in [ScreenshotPixels { size: (0, 2), rgba: vec![] }, ScreenshotPixels { size: (2, 2), rgba: vec![0; 3] }] {
            assert!(save(Some(&target), pixels).is_err());
        }
        let valid = ScreenshotPixels { size: (1, 1), rgba: vec![0, 0, 0, 255] };
        assert!(save(None, valid.clone()).is_err());
        let blocked = target.join("file");
        std::fs::write(&blocked, b"occupied").unwrap();
        assert!(save(Some(&blocked), valid).is_err());
    }

    #[test]
    fn timestamps_are_utc_dates() {
        assert_eq!(timestamp(0), "19700101-000000");
        assert_eq!(timestamp(1_792_060_325), "20261015-103205");
        assert_eq!(timestamp(951_782_400), "20000229-000000", "a leap day");
    }
}
