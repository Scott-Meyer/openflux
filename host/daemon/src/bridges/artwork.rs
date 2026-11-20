//! Album art for the keyboard's now-playing tile. Players hand over art in any size and format;
//! it reaches the keyboard as a small square JPEG, small enough to travel inside the live data.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

/// The side of the square image sent to the keyboard, in pixels. The now-playing tile draws its
/// art at up to 160 px.
pub const ARTWORK_PX: u32 = 160;

/// The most base64 text one image may take. The display drops a live-data line longer than
/// 64 KiB, and the line also carries the other live data.
pub const MAX_ARTWORK_BASE64: usize = 32 * 1024;

/// The largest source image accepted, encoded.
pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;

/// Album art ready for the keyboard.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaArtwork {
    /// Identifies the image: the same id is always the same image.
    pub id: String,
    /// The image: a square JPEG of [`ARTWORK_PX`] a side, base64 with padding.
    pub base64_jpeg: String,
}

/// Turns a player's image (PNG, JPEG, GIF or BMP, any size) into keyboard artwork: cropped to a
/// square around its centre and scaled to [`ARTWORK_PX`]. `None` when it can't be decoded or is
/// too large.
pub fn normalize_artwork(source: &[u8]) -> Option<MediaArtwork> {
    if source.is_empty() || source.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(source))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    for (side, quality) in [(ARTWORK_PX, 82u8), (ARTWORK_PX, 60), (120, 50)] {
        let square = decoded
            .resize_to_fill(side, side, image::imageops::FilterType::Triangle)
            .to_rgb8();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality)
            .encode_image(&square)
            .ok()?;
        let base64_jpeg = base64::engine::general_purpose::STANDARD.encode(&jpeg);
        if base64_jpeg.len() <= MAX_ARTWORK_BASE64 {
            let digest = Sha256::digest(&jpeg);
            let id = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
            return Some(MediaArtwork { id, base64_jpeg });
        }
    }
    None
}

/// How long a source that gave no usable image waits before it's tried again, and how many tries
/// one source gets. Players often publish the track before its art.
const RETRY_AFTER: Duration = Duration::from_secs(2);
const MAX_TRIES: u32 = 5;

/// Remembers the artwork of the current source, so an image is fetched and scaled once per track.
#[derive(Default)]
pub struct ArtworkCache {
    key: String,
    artwork: Option<MediaArtwork>,
    tries: u32,
    tried_at: Option<Instant>,
}

impl ArtworkCache {
    /// The artwork for the source `key` names (a URL, a content hash, the track's fields). `load`
    /// fetches the source image; it runs when the key changes, and again a few times while the
    /// key's image is missing. An empty key means no art.
    pub fn resolve(
        &mut self,
        key: &str,
        load: impl FnOnce() -> Option<Vec<u8>>,
    ) -> Option<MediaArtwork> {
        if key.is_empty() {
            self.key.clear();
            self.artwork = None;
            self.tries = 0;
            self.tried_at = None;
            return None;
        }
        if key != self.key {
            self.key = key.to_string();
            self.artwork = None;
            self.tries = 0;
            self.tried_at = None;
        }
        let retry_due = self.artwork.is_none()
            && self.tries < MAX_TRIES
            && self.tried_at.is_none_or(|t| t.elapsed() >= RETRY_AFTER);
        if retry_due {
            self.tries += 1;
            self.tried_at = Some(Instant::now());
            self.artwork = load().as_deref().and_then(normalize_artwork);
        }
        self.artwork.clone()
    }
}

#[cfg(test)]
pub(crate) fn test_png(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x * 7) as u8, (y * 5) as u8, ((x ^ y) * 3) as u8])
    });
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_player_image_becomes_a_small_square_jpeg_with_a_stable_id() {
        let wide = test_png(640, 360);
        let art = normalize_artwork(&wide).unwrap();
        assert!(art.base64_jpeg.len() <= MAX_ARTWORK_BASE64);
        let jpeg = base64::engine::general_purpose::STANDARD
            .decode(&art.base64_jpeg)
            .unwrap();
        let back = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((back.width(), back.height()), (ARTWORK_PX, ARTWORK_PX));
        assert_eq!(
            image::guess_format(&jpeg).unwrap(),
            image::ImageFormat::Jpeg
        );
        assert_eq!(normalize_artwork(&wide).unwrap().id, art.id);
        assert_ne!(normalize_artwork(&test_png(300, 300)).unwrap().id, art.id);

        assert!(normalize_artwork(b"").is_none());
        assert!(normalize_artwork(b"not an image").is_none());
    }

    #[test]
    fn the_cache_loads_once_per_source_and_retries_a_missing_image_a_few_times() {
        let png = test_png(64, 64);
        let mut cache = ArtworkCache::default();
        let mut loads = 0;
        let first = cache.resolve("track-a", || {
            loads += 1;
            Some(png.clone())
        });
        assert!(first.is_some());
        let again = cache.resolve("track-a", || {
            loads += 1;
            None
        });
        assert_eq!(again, first);
        assert_eq!(loads, 1);

        // Art that isn't there yet is tried again after a pause, not on every poll.
        let mut misses = 0;
        assert!(cache
            .resolve("track-b", || {
                misses += 1;
                None
            })
            .is_none());
        assert!(cache
            .resolve("track-b", || {
                misses += 1;
                Some(png.clone())
            })
            .is_none());
        assert_eq!(misses, 1);
        cache.tried_at = Some(Instant::now() - RETRY_AFTER);
        assert!(cache.resolve("track-b", || Some(png.clone())).is_some());

        assert!(cache.resolve("", || Some(png.clone())).is_none());
    }
}
