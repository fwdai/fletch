//! Telling an icon's bytes apart, with no image crate: the format from its
//! magic bytes, a PNG's width from its header, and an ICO's largest embedded
//! PNG. Nothing is decoded or resized — the webview scales what it draws, and
//! an SVG is only ever drawn through `<img>`, where its scripts do not run.

use std::io::Read;
use std::path::Path;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The MIME type of an image the webview can draw, from its first bytes.
pub fn mime_of(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(PNG_MAGIC) {
        Some("image/png")
    } else if bytes.starts_with(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if std::str::from_utf8(bytes).is_ok_and(|text| text.contains("<svg")) {
        Some("image/svg+xml")
    } else {
        None
    }
}

/// `bytes` as the icon to store: an image as it is, an ICO as its largest
/// embedded PNG. `None` when it is neither, or an ICO of bitmaps only.
pub fn normalize(bytes: Vec<u8>) -> Option<(Vec<u8>, &'static str)> {
    if bytes.starts_with(b"\0\0\x01\0") {
        let png = largest_ico_png(&bytes)?;
        return Some((png, "image/png"));
    }
    let mime = mime_of(&bytes)?;
    Some((bytes, mime))
}

/// A PNG's width, from the `IHDR` chunk that always comes first.
fn png_width(bytes: &[u8]) -> Option<u32> {
    if !bytes.starts_with(PNG_MAGIC) || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    Some(u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?))
}

/// The width of the PNG at `path`, reading only its header.
pub(super) fn png_width_of(path: &Path) -> Option<u32> {
    let mut head = [0u8; 20];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    png_width(&head)
}

/// The widest PNG an ICO directory points at. Entries are 16 bytes after a
/// 6-byte header; each names its data's length and offset.
fn largest_ico_png(ico: &[u8]) -> Option<Vec<u8>> {
    let count = u16::from_le_bytes(ico.get(4..6)?.try_into().ok()?) as usize;
    (0..count)
        .filter_map(|i| {
            let entry = ico.get(6 + i * 16..6 + (i + 1) * 16)?;
            let len = u32::from_le_bytes(entry[8..12].try_into().ok()?) as usize;
            let at = u32::from_le_bytes(entry[12..16].try_into().ok()?) as usize;
            let data = ico.get(at..at.checked_add(len)?)?;
            Some((png_width(data)?, data))
        })
        .max_by_key(|(width, _)| *width)
        .map(|(_, data)| data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32) -> Vec<u8> {
        let mut b = PNG_MAGIC.to_vec();
        b.extend([0, 0, 0, 13]);
        b.extend(b"IHDR");
        b.extend(width.to_be_bytes());
        b.extend(width.to_be_bytes());
        b
    }

    fn ico(images: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0, 0, 1, 0];
        out.extend((images.len() as u16).to_le_bytes());
        let mut at = 6 + 16 * images.len();
        for image in images {
            out.extend([0u8; 8]);
            out.extend((image.len() as u32).to_le_bytes());
            out.extend((at as u32).to_le_bytes());
            at += image.len();
        }
        for image in images {
            out.extend(image);
        }
        out
    }

    #[test]
    fn formats_are_told_from_their_bytes() {
        assert_eq!(mime_of(&png(16)), Some("image/png"));
        assert_eq!(
            mime_of(b"<?xml version=\"1.0\"?><svg/>"),
            Some("image/svg+xml")
        );
        assert_eq!(mime_of(b"\xFF\xD8\xFF\xE0"), Some("image/jpeg"));
        assert_eq!(mime_of(b"-----BEGIN OPENSSH PRIVATE KEY-----"), None);
        assert_eq!(png_width(&png(512)), Some(512));
    }

    #[test]
    fn an_ico_keeps_its_widest_png_and_a_bitmap_ico_nothing() {
        let (bytes, mime) = normalize(ico(&[png(16), png(256), b"BMPDATA".to_vec()])).unwrap();
        assert_eq!((png_width(&bytes), mime), (Some(256), "image/png"));
        assert_eq!(normalize(ico(&[b"BMPDATA".to_vec()])), None);
        assert_eq!(normalize(b"not an image".to_vec()), None);
    }
}
