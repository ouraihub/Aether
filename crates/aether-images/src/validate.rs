//! Structural image validation shared by request inputs and provider output.

use aether_support::KitError;

pub(super) struct Dimensions {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) alpha: bool,
}

fn invalid() -> KitError {
    KitError::bad_request("images", "format")
}
fn be16(b: &[u8], at: usize) -> u32 {
    u32::from(u16::from_be_bytes([b[at], b[at + 1]]))
}
fn le16(b: &[u8], at: usize) -> u32 {
    u32::from(u16::from_le_bytes([b[at], b[at + 1]]))
}
fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn le24(b: &[u8], at: usize) -> u32 {
    u32::from(b[at]) | (u32::from(b[at + 1]) << 8) | (u32::from(b[at + 2]) << 16)
}

pub(super) fn sniff(bytes: &[u8]) -> Result<&'static str, KitError> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok("image/png")
    } else if bytes.starts_with(&[255, 216, 255]) {
        Ok("image/jpeg")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Ok("image/webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Ok("image/gif")
    } else {
        Err(KitError::bad_request("images", "mime"))
    }
}

pub(super) fn validate(bytes: &[u8], mime: &str) -> Result<Dimensions, KitError> {
    if sniff(bytes)? != mime {
        return Err(KitError::bad_request("images", "mime"));
    }
    let dimensions = match mime {
        "image/png" => png(bytes)?,
        "image/jpeg" => jpeg(bytes)?,
        "image/webp" => webp(bytes)?,
        "image/gif" => gif(bytes)?,
        _ => return Err(KitError::bad_request("images", "mime")),
    };
    if dimensions.width == 0
        || dimensions.height == 0
        || u64::from(dimensions.width) * u64::from(dimensions.height) > 40_000_000
    {
        return Err(KitError::bad_request("images", "dimensions").with_limit(40_000_000));
    }
    Ok(dimensions)
}

fn png(b: &[u8]) -> Result<Dimensions, KitError> {
    if b.len() < 57 {
        return Err(invalid());
    }
    let (mut at, mut data, mut end) = (8, false, false);
    let mut dimensions = Dimensions {
        width: 0,
        height: 0,
        alpha: false,
    };
    while at + 12 <= b.len() {
        let size = be32(b, at) as usize;
        if size > b.len() - at - 12 {
            return Err(invalid());
        }
        let kind = &b[at + 4..at + 8];
        if at == 8 {
            if kind != b"IHDR" || size != 13 {
                return Err(invalid());
            }
            dimensions.width = be32(b, at + 8);
            dimensions.height = be32(b, at + 12);
            let (depth, color) = (b[at + 16], b[at + 17]);
            let depth_ok = match color {
                0 => [1, 2, 4, 8, 16].contains(&depth),
                3 => [1, 2, 4, 8].contains(&depth),
                2 | 4 | 6 => [8, 16].contains(&depth),
                _ => false,
            };
            if !depth_ok || b[at + 18] != 0 || b[at + 19] != 0 || b[at + 20] > 1 {
                return Err(invalid());
            }
            dimensions.alpha = color == 4 || color == 6;
        } else if kind == b"IHDR" {
            return Err(invalid());
        }
        if kind == b"tRNS" {
            dimensions.alpha = true;
        }
        if kind == b"IDAT" && size > 0 {
            data = true;
        }
        at += size + 12;
        if kind == b"IEND" {
            if size != 0 || at != b.len() {
                return Err(invalid());
            }
            end = true;
            break;
        }
    }
    if !data || !end {
        return Err(invalid());
    }
    Ok(dimensions)
}

fn jpeg(b: &[u8]) -> Result<Dimensions, KitError> {
    if b.len() < 12 {
        return Err(invalid());
    }
    let (mut at, mut scan, mut end) = (2, false, false);
    let mut dimensions = Dimensions {
        width: 0,
        height: 0,
        alpha: false,
    };
    while at < b.len() {
        if b[at] != 255 {
            return Err(invalid());
        }
        at += 1;
        while b.get(at) == Some(&255) {
            at += 1;
        }
        let marker = *b.get(at).ok_or_else(invalid)?;
        at += 1;
        if marker == 217 {
            end = at == b.len();
            break;
        }
        if marker == 0 || marker == 216 || at + 2 > b.len() {
            return Err(invalid());
        }
        if (208..=215).contains(&marker) {
            continue;
        }
        let size = be16(b, at) as usize;
        if size < 2 || size > b.len() - at {
            return Err(invalid());
        }
        if [
            192, 193, 194, 195, 197, 198, 199, 201, 202, 203, 205, 206, 207,
        ]
        .contains(&marker)
        {
            if size < 8 || dimensions.width != 0 {
                return Err(invalid());
            }
            dimensions.height = be16(b, at + 3);
            dimensions.width = be16(b, at + 5);
        }
        at += size;
        if marker == 218 {
            scan = true;
            while at < b.len() {
                if b[at] != 255 {
                    at += 1;
                    continue;
                }
                if b.get(at + 1)
                    .is_some_and(|v| *v == 0 || (208..=215).contains(v))
                {
                    at += 2;
                    continue;
                }
                break;
            }
        }
    }
    if !scan || !end {
        return Err(invalid());
    }
    Ok(dimensions)
}

fn webp_pixels(b: &[u8], kind: &[u8]) -> Result<Dimensions, KitError> {
    if kind == b"VP8 " {
        if b.len() < 10 || b[0] & 1 != 0 || &b[3..6] != b"\x9d\x01\x2a" {
            return Err(invalid());
        }
        Ok(Dimensions {
            width: le16(b, 6) & 16383,
            height: le16(b, 8) & 16383,
            alpha: false,
        })
    } else {
        if b.len() < 5 || b[0] != 47 {
            return Err(invalid());
        }
        let bits = le32(b, 1);
        if bits >> 29 != 0 {
            return Err(invalid());
        }
        Ok(Dimensions {
            width: (bits & 16383) + 1,
            height: ((bits >> 14) & 16383) + 1,
            alpha: bits & (1 << 28) != 0,
        })
    }
}

fn webp(b: &[u8]) -> Result<Dimensions, KitError> {
    if b.len() < 26 || u64::from(le32(b, 4)) + 8 != b.len() as u64 {
        return Err(invalid());
    }
    let (mut at, mut image, mut extended, mut animated, mut animation_header) =
        (12, false, false, false, false);
    let mut dimensions = Dimensions {
        width: 0,
        height: 0,
        alpha: false,
    };
    while at + 8 <= b.len() {
        let size = le32(b, at + 4) as usize;
        let body = at + 8;
        if size > b.len() - body {
            return Err(invalid());
        }
        let kind = &b[at..at + 4];
        if kind == b"VP8X" {
            if at != 12 || size != 10 {
                return Err(invalid());
            }
            extended = true;
            dimensions.alpha = b[body] & 16 != 0;
            animated = b[body] & 2 != 0;
            dimensions.width = le24(b, body + 4) + 1;
            dimensions.height = le24(b, body + 7) + 1;
        } else if kind == b"ANIM" {
            if !animated || size != 6 || animation_header {
                return Err(invalid());
            }
            animation_header = true;
        } else if kind == b"ANMF" {
            if !animated || !animation_header || size < 24 {
                return Err(invalid());
            }
            let (x, y, w, h) = (
                le24(b, body) * 2,
                le24(b, body + 3) * 2,
                le24(b, body + 6) + 1,
                le24(b, body + 9) + 1,
            );
            if x + w > dimensions.width || y + h > dimensions.height || b[body + 15] & 252 != 0 {
                return Err(invalid());
            }
            let (mut frame, mut pixels) = (body + 16, false);
            while frame + 8 <= body + size {
                let length = le32(b, frame + 4) as usize;
                let data = frame + 8;
                if length > body + size - data {
                    return Err(invalid());
                }
                let kind = &b[frame..frame + 4];
                if kind == b"VP8 " || kind == b"VP8L" {
                    if pixels {
                        return Err(invalid());
                    }
                    let d = webp_pixels(&b[data..data + length], kind)?;
                    if d.width != w || d.height != h {
                        return Err(invalid());
                    }
                    dimensions.alpha |= d.alpha;
                    pixels = true;
                } else if kind != b"ALPH" {
                    return Err(invalid());
                }
                frame = data + length + (length & 1);
            }
            if !pixels || frame != body + size {
                return Err(invalid());
            }
            image = true;
        } else if kind == b"VP8 " || kind == b"VP8L" {
            if animated || image {
                return Err(invalid());
            }
            let d = webp_pixels(&b[body..body + size], kind)?;
            if extended && (d.width != dimensions.width || d.height != dimensions.height) {
                return Err(invalid());
            }
            dimensions.width = d.width;
            dimensions.height = d.height;
            dimensions.alpha |= d.alpha;
            image = true;
        }
        at = body + size + (size & 1);
    }
    if !image || at != b.len() {
        return Err(invalid());
    }
    Ok(dimensions)
}

fn subblocks(b: &[u8], at: &mut usize) -> Result<bool, KitError> {
    let mut data = false;
    loop {
        let size = usize::from(*b.get(*at).ok_or_else(invalid)?);
        *at += 1;
        if size == 0 {
            return Ok(data);
        }
        if size > b.len() - *at {
            return Err(invalid());
        }
        *at += size;
        data = true;
    }
}

fn gif(b: &[u8]) -> Result<Dimensions, KitError> {
    if b.len() < 14 {
        return Err(invalid());
    }
    let mut dimensions = Dimensions {
        width: le16(b, 6),
        height: le16(b, 8),
        alpha: false,
    };
    let mut at = 13;
    if b[10] & 128 != 0 {
        at += 3 * (1_usize << ((b[10] & 7) + 1));
    }
    let mut image = false;
    loop {
        let kind = *b.get(at).ok_or_else(invalid)?;
        at += 1;
        match kind {
            59 => {
                if !image || at != b.len() {
                    return Err(invalid());
                }
                break;
            }
            33 => {
                let label = *b.get(at).ok_or_else(invalid)?;
                at += 1;
                if label == 249 {
                    if b.get(at) != Some(&4) || at + 6 > b.len() || b[at + 5] != 0 {
                        return Err(invalid());
                    }
                    dimensions.alpha |= b[at + 1] & 1 != 0;
                    at += 6;
                } else {
                    subblocks(b, &mut at)?;
                }
            }
            44 => {
                if at + 9 > b.len() {
                    return Err(invalid());
                }
                let (x, y, w, h) = (
                    le16(b, at),
                    le16(b, at + 2),
                    le16(b, at + 4),
                    le16(b, at + 6),
                );
                if w == 0 || h == 0 || x + w > dimensions.width || y + h > dimensions.height {
                    return Err(invalid());
                }
                let packed = b[at + 8];
                at += 9;
                if packed & 128 != 0 {
                    at += 3 * (1_usize << ((packed & 7) + 1));
                }
                if !b.get(at).is_some_and(|v| (2..=8).contains(v)) {
                    return Err(invalid());
                }
                at += 1;
                if !subblocks(b, &mut at)? {
                    return Err(invalid());
                }
                image = true;
            }
            _ => return Err(invalid()),
        }
    }
    Ok(dimensions)
}
