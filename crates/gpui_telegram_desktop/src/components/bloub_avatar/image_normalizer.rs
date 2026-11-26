//! 1:1 OpenBot Custom Avatar Image Normalizer & Animated WebP RIFF Muxer (`OB-F04`).
//! Mirrors `packages/ui/src/avatar-image.ts` and `packages/contracts/src/avatar-images.ts`.

use std::fmt;
use std::io::Cursor;
use std::path::Path;

use base64::Engine as _;
use image::codecs::gif::GifDecoder;
use image::codecs::webp::{WebPDecoder, WebPEncoder};
use image::imageops::FilterType;
use image::{AnimationDecoder, ExtendedColorType, ImageFormat, RgbaImage};
use openbot_contracts::limits::AVATAR_IMAGE_LIMITS;
use serde::{Deserialize, Serialize};

pub const OUTPUT_SIZES: [u32; 4] = [512, 448, 384, 320];
pub const OUTPUT_QUALITIES: [f64; 4] = [0.88, 0.82, 0.76, 0.70];
pub const ACCEPTED_SOURCE_MIMES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];
pub const MAX_SOURCE_DIMENSION: u32 = 16_384;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedAvatarOutput {
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub byte_length: usize,
    pub magic: String,
    pub animated: bool,
    pub frame_count: usize,
    pub bytes: Vec<u8>,
}

impl NormalizedAvatarOutput {
    pub fn to_data_url(&self) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&self.bytes);
        format!("data:{};base64,{encoded}", self.mime)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvatarNormalizeError {
    UnsupportedType {
        mime_hint: Option<String>,
        i18n: &'static str,
    },
    SourceTooLarge {
        byte_length: usize,
        limit: usize,
        i18n: &'static str,
    },
    DecodeFailed {
        detail: String,
        i18n: &'static str,
    },
    EncodeFailed {
        detail: String,
        i18n: &'static str,
    },
    OutputTooLarge {
        limit: usize,
        i18n: &'static str,
    },
    Io {
        detail: String,
        i18n: &'static str,
    },
}

impl AvatarNormalizeError {
    pub const fn i18n_key(&self) -> &'static str {
        match self {
            Self::UnsupportedType { i18n, .. }
            | Self::SourceTooLarge { i18n, .. }
            | Self::DecodeFailed { i18n, .. }
            | Self::EncodeFailed { i18n, .. }
            | Self::OutputTooLarge { i18n, .. }
            | Self::Io { i18n, .. } => i18n,
        }
    }
}

impl fmt::Display for AvatarNormalizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedType { mime_hint, i18n } => {
                write!(
                    f,
                    "{i18n}: invalid or unsupported avatar image format/mime ({mime_hint:?})"
                )
            }
            Self::SourceTooLarge {
                byte_length,
                limit,
                i18n,
            } => write!(
                f,
                "{i18n}: avatar source size {byte_length} bytes is too large (exceeds 10 MiB limit of {limit} bytes)"
            ),
            Self::DecodeFailed { detail, i18n } => {
                write!(f, "{i18n}: failed to decode avatar image: {detail}")
            }
            Self::EncodeFailed { detail, i18n } => {
                write!(f, "{i18n}: failed to encode WebP avatar: {detail}")
            }
            Self::OutputTooLarge { limit, i18n } => {
                write!(f, "{i18n}: normalized WebP exceeds {limit} bytes")
            }
            Self::Io { detail, i18n } => write!(f, "{i18n}: I/O error: {detail}"),
        }
    }
}

impl std::error::Error for AvatarNormalizeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvatarCropRect {
    pub source_x: u32,
    pub source_y: u32,
    pub source_size: u32,
}

/// Exact square center-crop matching `avatar-image.ts:avatarCrop` (`L124-L131`).
pub fn avatar_crop(width: u32, height: u32) -> AvatarCropRect {
    let source_size = width.min(height);
    AvatarCropRect {
        source_x: (width.saturating_sub(source_size)) / 2,
        source_y: (height.saturating_sub(source_size)) / 2,
        source_size,
    }
}

/// Detects extended WebP animation flag (`RIFF....WEBPVP8X` with `bytes[20] & 0x02 != 0`).
pub fn is_animated_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 21
        && &bytes[0..4] == b"RIFF"
        && &bytes[8..16] == b"WEBPVP8X"
        && (bytes[20] & 0x02) != 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceFormat {
    Png,
    Jpeg,
    WebP,
    Gif,
}

fn detect_magic_format(bytes: &[u8]) -> Option<SourceFormat> {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(SourceFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(SourceFormat::Jpeg)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(SourceFormat::WebP)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(SourceFormat::Gif)
    } else {
        None
    }
}

fn validate_source_format(
    bytes: &[u8],
    mime_hint: Option<&str>,
) -> Result<SourceFormat, AvatarNormalizeError> {
    if let Some(raw_hint) = mime_hint {
        let trimmed = raw_hint.trim().to_ascii_lowercase();
        if !trimmed.is_empty() && !ACCEPTED_SOURCE_MIMES.contains(&trimmed.as_str()) {
            return Err(AvatarNormalizeError::UnsupportedType {
                mime_hint: Some(raw_hint.to_string()),
                i18n: "app.avatar.type",
            });
        }
    }
    if bytes.is_empty() {
        return Err(AvatarNormalizeError::DecodeFailed {
            detail: "empty 0-byte truncated image payload".to_string(),
            i18n: "app.avatar.processFailed",
        });
    }
    if bytes.len() > AVATAR_IMAGE_LIMITS.source_bytes {
        return Err(AvatarNormalizeError::SourceTooLarge {
            byte_length: bytes.len(),
            limit: AVATAR_IMAGE_LIMITS.source_bytes,
            i18n: "app.avatar.size",
        });
    }
    if bytes.starts_with(b"RIFF") && !(bytes.len() >= 12 && &bytes[8..12] == b"WEBP") {
        return Err(AvatarNormalizeError::DecodeFailed {
            detail: "corrupt or malformed RIFF WEBP magic header".to_string(),
            i18n: "app.avatar.processFailed",
        });
    }
    detect_magic_format(bytes).ok_or_else(|| AvatarNormalizeError::UnsupportedType {
        mime_hint: mime_hint.map(str::to_string),
        i18n: "app.avatar.type",
    })
}

struct DecodedAnimFrame {
    image: RgbaImage,
    duration_ms: u32,
}

enum DecodedSource {
    Static(RgbaImage),
    Animated(Vec<DecodedAnimFrame>),
}

fn delay_to_ms(delay: image::Delay) -> u32 {
    let (numer, denom) = delay.numer_denom_ms();
    if denom == 0 {
        0
    } else {
        let num = u64::from(numer);
        let den = u64::from(denom);
        ((num.saturating_mul(2).saturating_add(den)) / (2 * den)).min(0xFF_FFFF) as u32
    }
}

fn check_dimensions(width: u32, height: u32) -> Result<(), AvatarNormalizeError> {
    if width == 0 || height == 0 || width > MAX_SOURCE_DIMENSION || height > MAX_SOURCE_DIMENSION {
        return Err(AvatarNormalizeError::DecodeFailed {
            detail: format!("invalid image dimensions {width}x{height}"),
            i18n: "app.avatar.processFailed",
        });
    }
    Ok(())
}

fn lzw_decompress_tolerant(min_code_size: u8, data: &[u8], max_pixels: usize) -> Vec<u8> {
    if !(1..=8).contains(&min_code_size) || max_pixels == 0 {
        return Vec::new();
    }
    let clear_code = 1u16 << min_code_size;
    let eoi_code = clear_code + 1;
    let mut code_size = u32::from(min_code_size) + 1;
    let mut dict: Vec<Vec<u8>> = (0..clear_code).map(|i| vec![i as u8]).collect();
    dict.push(Vec::new()); // clear
    dict.push(Vec::new()); // eoi

    let mut bit_pos = 0usize;
    let total_bits = data.len() * 8;
    let read_code = |pos: &mut usize, width: u32| -> Option<u16> {
        if *pos + (width as usize) > total_bits {
            return None;
        }
        let mut val = 0u32;
        for b in 0..width {
            let idx = *pos + (b as usize);
            if (data[idx / 8] & (1 << (idx % 8))) != 0 {
                val |= 1 << b;
            }
        }
        *pos += width as usize;
        Some(val as u16)
    };

    let mut out = Vec::with_capacity(max_pixels.min(65_536));
    let mut prev: Option<Vec<u8>> = None;
    while out.len() < max_pixels {
        let Some(code) = read_code(&mut bit_pos, code_size) else {
            break;
        };
        if code == clear_code {
            dict.truncate(usize::from(eoi_code) + 1);
            code_size = u32::from(min_code_size) + 1;
            prev = None;
            continue;
        }
        if code == eoi_code {
            break;
        }
        let entry = if (usize::from(code)) < dict.len() {
            dict[usize::from(code)].clone()
        } else if usize::from(code) == dict.len()
            && let Some(ref p) = prev
            && !p.is_empty()
        {
            let mut e = p.clone();
            e.push(p[0]);
            e
        } else {
            break;
        };
        let take = entry.len().min(max_pixels - out.len());
        out.extend_from_slice(&entry[..take]);
        if let Some(p) = prev
            && !entry.is_empty()
            && dict.len() < 4096
        {
            let mut next_seq = p;
            next_seq.push(entry[0]);
            dict.push(next_seq);
            if dict.len() == (1usize << code_size) && code_size < 12 {
                code_size += 1;
            }
        }
        prev = Some(entry);
    }
    out
}

fn decode_gif_tolerant(bytes: &[u8]) -> Result<DecodedSource, AvatarNormalizeError> {
    if bytes.len() < 13 || (!bytes.starts_with(b"GIF87a") && !bytes.starts_with(b"GIF89a")) {
        return Err(AvatarNormalizeError::DecodeFailed {
            detail: "invalid GIF header".to_string(),
            i18n: "app.avatar.processFailed",
        });
    }
    let screen_w = u32::from(u16::from_le_bytes([bytes[6], bytes[7]]));
    let screen_h = u32::from(u16::from_le_bytes([bytes[8], bytes[9]]));
    check_dimensions(screen_w, screen_h)?;
    let packed = bytes[10];
    let bg_idx = usize::from(bytes[11]);
    let mut pos = 13usize;
    let gct: Vec<[u8; 3]> = if (packed & 0x80) != 0 {
        let count = 1usize << ((packed & 0x07) + 1);
        let need = count * 3;
        if pos + need > bytes.len() {
            return Err(AvatarNormalizeError::DecodeFailed {
                detail: "truncated GIF global color table".to_string(),
                i18n: "app.avatar.processFailed",
            });
        }
        let tbl = bytes[pos..pos + need]
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .collect();
        pos += need;
        tbl
    } else {
        Vec::new()
    };

    let bg_rgba = gct
        .get(bg_idx)
        .map_or(image::Rgba([0, 0, 0, 0]), |&[r, g, b]| image::Rgba([r, g, b, 255]));
    let mut canvas = RgbaImage::from_pixel(screen_w, screen_h, bg_rgba);
    let mut frames = Vec::new();
    let mut gce_delay_ms = 80u32;
    let mut gce_trans: Option<u8> = None;

    while pos < bytes.len() {
        match bytes[pos] {
            0x3B => break,
            0x21 => {
                if pos + 2 > bytes.len() {
                    break;
                }
                let ext = bytes[pos + 1];
                pos += 2;
                let mut first_sub: Option<&[u8]> = None;
                while pos < bytes.len() {
                    let blen = usize::from(bytes[pos]);
                    pos += 1;
                    if blen == 0 || pos + blen > bytes.len() {
                        break;
                    }
                    if first_sub.is_none() {
                        first_sub = Some(&bytes[pos..pos + blen]);
                    }
                    pos += blen;
                }
                if ext == 0xF9
                    && let Some(sub) = first_sub
                    && sub.len() >= 4
                {
                    let delay_cs = u16::from_le_bytes([sub[1], sub[2]]);
                    gce_delay_ms = u32::from(delay_cs) * 10;
                    gce_trans = ((sub[0] & 0x01) != 0).then_some(sub[3]);
                }
            }
            0x2C => {
                if pos + 10 > bytes.len() {
                    break;
                }
                let left = u32::from(u16::from_le_bytes([bytes[pos + 1], bytes[pos + 2]]));
                let top = u32::from(u16::from_le_bytes([bytes[pos + 3], bytes[pos + 4]]));
                let iw = u32::from(u16::from_le_bytes([bytes[pos + 5], bytes[pos + 6]]));
                let ih = u32::from(u16::from_le_bytes([bytes[pos + 7], bytes[pos + 8]]));
                let ipacked = bytes[pos + 9];
                pos += 10;
                if iw == 0 || ih == 0 {
                    break;
                }
                let lct: Option<Vec<[u8; 3]>> = if (ipacked & 0x80) != 0 {
                    let count = 1usize << ((ipacked & 0x07) + 1);
                    let need = count * 3;
                    if pos + need > bytes.len() {
                        break;
                    }
                    let tbl = bytes[pos..pos + need]
                        .chunks_exact(3)
                        .map(|c| [c[0], c[1], c[2]])
                        .collect();
                    pos += need;
                    Some(tbl)
                } else {
                    None
                };
                if pos >= bytes.len() {
                    break;
                }
                let min_code = bytes[pos];
                pos += 1;
                let mut lzw_data = Vec::new();
                while pos < bytes.len() {
                    let blen = usize::from(bytes[pos]);
                    pos += 1;
                    if blen == 0 {
                        break;
                    }
                    if pos + blen > bytes.len() {
                        lzw_data.extend_from_slice(&bytes[pos..]);
                        pos = bytes.len();
                        break;
                    }
                    lzw_data.extend_from_slice(&bytes[pos..pos + blen]);
                    pos += blen;
                }
                let total_px = (iw as usize).saturating_mul(ih as usize);
                let indices = lzw_decompress_tolerant(min_code, &lzw_data, total_px);
                let pal = lct.as_deref().unwrap_or(&gct);
                for (p_idx, &ci) in indices.iter().enumerate() {
                    if gce_trans == Some(ci) {
                        continue;
                    }
                    let x = left + ((p_idx as u32) % iw);
                    let y = top + ((p_idx as u32) / iw);
                    if x < screen_w
                        && y < screen_h
                        && let Some(&[r, g, b]) = pal.get(usize::from(ci))
                    {
                        canvas.put_pixel(x, y, image::Rgba([r, g, b, 255]));
                    }
                }
                frames.push(DecodedAnimFrame {
                    image: canvas.clone(),
                    duration_ms: gce_delay_ms,
                });
                gce_delay_ms = 80;
                gce_trans = None;
            }
            _ => break,
        }
    }

    if frames.is_empty() {
        return Err(AvatarNormalizeError::DecodeFailed {
            detail: "GIF contains zero valid image frames".to_string(),
            i18n: "app.avatar.processFailed",
        });
    }
    if frames.len() == 1 {
        return Ok(DecodedSource::Static(frames.remove(0).image));
    }
    Ok(DecodedSource::Animated(frames))
}

fn decode_source(
    bytes: &[u8],
    format: SourceFormat,
) -> Result<DecodedSource, AvatarNormalizeError> {
    match format {
        SourceFormat::Gif => {
            if let Ok(decoder) = GifDecoder::new(Cursor::new(bytes))
                && let Ok(raw_frames) = decoder.into_frames().collect_frames()
                && !raw_frames.is_empty()
            {
                if raw_frames.len() == 1 {
                    let first = raw_frames.into_iter().next().expect("checked len == 1");
                    let buf = first.into_buffer();
                    check_dimensions(buf.width(), buf.height())?;
                    return Ok(DecodedSource::Static(buf));
                }
                let mut frames = Vec::with_capacity(raw_frames.len());
                for frame in raw_frames {
                    let duration_ms = delay_to_ms(frame.delay());
                    let image = frame.into_buffer();
                    check_dimensions(image.width(), image.height())?;
                    frames.push(DecodedAnimFrame { image, duration_ms });
                }
                return Ok(DecodedSource::Animated(frames));
            }
            decode_gif_tolerant(bytes)
        }
        SourceFormat::WebP if is_animated_webp(bytes) => {
            let decoder = WebPDecoder::new(Cursor::new(bytes)).map_err(|e| {
                AvatarNormalizeError::DecodeFailed {
                    detail: e.to_string(),
                    i18n: "app.avatar.processFailed",
                }
            })?;
            let raw_frames = decoder.into_frames().collect_frames().map_err(|e| {
                AvatarNormalizeError::DecodeFailed {
                    detail: e.to_string(),
                    i18n: "app.avatar.processFailed",
                }
            })?;
            if raw_frames.is_empty() {
                return Err(AvatarNormalizeError::DecodeFailed {
                    detail: "animated WebP contains zero frames".to_string(),
                    i18n: "app.avatar.processFailed",
                });
            }
            if raw_frames.len() == 1 {
                let first = raw_frames.into_iter().next().expect("checked len == 1");
                let buf = first.into_buffer();
                check_dimensions(buf.width(), buf.height())?;
                return Ok(DecodedSource::Static(buf));
            }
            let mut frames = Vec::with_capacity(raw_frames.len());
            for frame in raw_frames {
                let duration_ms = delay_to_ms(frame.delay());
                let image = frame.into_buffer();
                check_dimensions(image.width(), image.height())?;
                frames.push(DecodedAnimFrame { image, duration_ms });
            }
            Ok(DecodedSource::Animated(frames))
        }
        SourceFormat::Png | SourceFormat::Jpeg | SourceFormat::WebP => {
            let img_fmt = match format {
                SourceFormat::Png => ImageFormat::Png,
                SourceFormat::Jpeg => ImageFormat::Jpeg,
                SourceFormat::WebP => ImageFormat::WebP,
                SourceFormat::Gif => ImageFormat::Gif,
            };
            let decoded = image::load_from_memory_with_format(bytes, img_fmt).map_err(|e| {
                AvatarNormalizeError::DecodeFailed {
                    detail: e.to_string(),
                    i18n: "app.avatar.processFailed",
                }
            })?;
            check_dimensions(decoded.width(), decoded.height())?;
            Ok(DecodedSource::Static(decoded.into_rgba8()))
        }
    }
}

fn crop_and_resize_square(source: &RgbaImage, output_size: u32) -> RgbaImage {
    let crop = avatar_crop(source.width(), source.height());
    let cropped = if crop.source_x == 0
        && crop.source_y == 0
        && crop.source_size == source.width()
        && crop.source_size == source.height()
    {
        source.clone()
    } else {
        image::imageops::crop_imm(
            source,
            crop.source_x,
            crop.source_y,
            crop.source_size,
            crop.source_size,
        )
        .to_image()
    };
    if cropped.width() == output_size && cropped.height() == output_size {
        cropped
    } else {
        image::imageops::resize(&cropped, output_size, output_size, FilterType::Lanczos3)
    }
}

fn quantize_rgba_for_quality(rgba: &RgbaImage, quality: f64) -> Vec<u8> {
    let raw = rgba.as_raw();
    let step_f = ((1.0 - quality.clamp(0.1, 1.0)) * 96.0).round();
    let q = (step_f as u16).clamp(2, 64);
    let half = q / 2;
    let mut out = Vec::with_capacity(raw.len());
    for px in raw.chunks_exact(4) {
        let r = (((u16::from(px[0]) + half) / q) * q).min(255) as u8;
        let g = (((u16::from(px[1]) + half) / q) * q).min(255) as u8;
        let b = (((u16::from(px[2]) + half) / q) * q).min(255) as u8;
        let a = if px[3] == 0 || px[3] == 255 {
            px[3]
        } else {
            (((u16::from(px[3]) + half) / q) * q).min(255) as u8
        };
        out.extend_from_slice(&[r, g, b, a]);
    }
    out
}

fn encode_still_webp_buf(raw_rgba: &[u8], size: u32) -> Result<Vec<u8>, AvatarNormalizeError> {
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .encode(raw_rgba, size, size, ExtendedColorType::Rgba8)
        .map_err(|e| AvatarNormalizeError::EncodeFailed {
            detail: e.to_string(),
            i18n: "app.avatar.processFailed",
        })?;
    Ok(out)
}

fn encode_still_frame(
    square: &RgbaImage,
    size: u32,
    quality: f64,
    try_lossless_first: bool,
) -> Result<Vec<u8>, AvatarNormalizeError> {
    if try_lossless_first {
        let lossless = encode_still_webp_buf(square.as_raw(), size)?;
        if lossless.len() <= AVATAR_IMAGE_LIMITS.stored_bytes {
            return Ok(lossless);
        }
    }
    let quantized = quantize_rgba_for_quality(square, quality);
    encode_still_webp_buf(&quantized, size)
}

fn le24(value: u32) -> [u8; 3] {
    [
        (value & 0xFF) as u8,
        ((value >> 8) & 0xFF) as u8,
        ((value >> 16) & 0xFF) as u8,
    ]
}

fn append_riff_chunk(target: &mut Vec<u8>, four_cc: &[u8; 4], payload: &[u8]) {
    target.extend_from_slice(four_cc);
    target.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    target.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        target.push(0);
    }
}

/// Extracts `ALPH`, `VP8 `, and `VP8L` bitstream subchunks (including even-byte RIFF padding)
/// from an encoded single-frame `RIFF....WEBP` buffer (`avatar-image.ts:frameData`, `L89-L101`).
pub fn extract_webp_frame_subchunks(still_webp: &[u8]) -> Result<Vec<u8>, AvatarNormalizeError> {
    if still_webp.len() < 12 || &still_webp[0..4] != b"RIFF" || &still_webp[8..12] != b"WEBP" {
        return Err(AvatarNormalizeError::EncodeFailed {
            detail: "invalid RIFF WEBP header in encoded frame".to_string(),
            i18n: "app.avatar.processFailed",
        });
    }
    let mut out = Vec::new();
    let mut offset = 12usize;
    while offset + 8 <= still_webp.len() {
        let four_cc = &still_webp[offset..offset + 4];
        let chunk_size = u32::from_le_bytes([
            still_webp[offset + 4],
            still_webp[offset + 5],
            still_webp[offset + 6],
            still_webp[offset + 7],
        ]) as usize;
        let padded = chunk_size + (chunk_size % 2);
        let end_unpadded = offset.saturating_add(8).saturating_add(chunk_size);
        if end_unpadded > still_webp.len() {
            return Err(AvatarNormalizeError::EncodeFailed {
                detail: "truncated subchunk in encoded WebP frame".to_string(),
                i18n: "app.avatar.processFailed",
            });
        }
        let next = offset.saturating_add(8).saturating_add(padded);
        if four_cc == b"ALPH" || four_cc == b"VP8 " || four_cc == b"VP8L" {
            if next <= still_webp.len() {
                out.extend_from_slice(&still_webp[offset..next]);
            } else {
                out.extend_from_slice(&still_webp[offset..end_unpadded]);
                if chunk_size % 2 == 1 {
                    out.push(0);
                }
            }
        }
        offset = next;
    }
    if out.is_empty() {
        return Err(AvatarNormalizeError::EncodeFailed {
            detail: "missing VP8/VP8L bitstream chunk in encoded WebP frame".to_string(),
            i18n: "app.avatar.processFailed",
        });
    }
    Ok(out)
}

/// Muxes extracted frame bitstreams into an extended `RIFF....WEBP` (`VP8X 0x12 + ANIM + ANMF 0x02`)
/// container matching `avatar-image.ts:animatedWebp` (`L103-L122`).
pub fn mux_animated_webp(frames: &[(Vec<u8>, u32)], size: u32) -> Vec<u8> {
    let minus_one = size.saturating_sub(1);
    let w24 = le24(minus_one);
    let h24 = le24(minus_one);
    let extent = [w24[0], w24[1], w24[2], h24[0], h24[1], h24[2]];

    let mut body = Vec::new();
    let vp8x_payload = [
        0x12, 0x00, 0x00, 0x00, extent[0], extent[1], extent[2], extent[3], extent[4], extent[5],
    ];
    append_riff_chunk(&mut body, b"VP8X", &vp8x_payload);
    append_riff_chunk(&mut body, b"ANIM", &[0u8; 6]);

    for (subchunks, duration_ms) in frames {
        let dur = le24((*duration_ms).min(0xFF_FFFF));
        let mut anmf_payload = Vec::with_capacity(16 + subchunks.len());
        anmf_payload.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        anmf_payload.extend_from_slice(&extent);
        anmf_payload.extend_from_slice(&dur);
        anmf_payload.push(0x02);
        anmf_payload.extend_from_slice(subchunks);
        append_riff_chunk(&mut body, b"ANMF", &anmf_payload);
    }

    let riff_size = (body.len() + 4) as u32;
    let mut out = Vec::with_capacity(12 + body.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    out
}

fn build_output(
    bytes: Vec<u8>,
    size: u32,
    animated: bool,
    frame_count: usize,
) -> NormalizedAvatarOutput {
    let magic = if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "RIFF....WEBP".to_string()
    } else {
        String::new()
    };
    let byte_length = bytes.len();
    NormalizedAvatarOutput {
        mime: "image/webp".to_string(),
        width: size,
        height: size,
        byte_length,
        magic,
        animated,
        frame_count,
        bytes,
    }
}

fn mime_hint_for_path(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "svg" => Some("image/svg+xml"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        "ico" => Some("image/x-icon"),
        "avif" => Some("image/avif"),
        "txt" | "md" => Some("text/plain"),
        "json" => Some("application/json"),
        "pdf" => Some("application/pdf"),
        _ => None,
    }
}

pub struct AvatarImageNormalizer;

impl AvatarImageNormalizer {
    pub fn normalize_bytes(
        bytes: &[u8],
        mime_hint: Option<&str>,
    ) -> Result<NormalizedAvatarOutput, AvatarNormalizeError> {
        let format = validate_source_format(bytes, mime_hint)?;
        let decoded = decode_source(bytes, format)?;

        for (index, output_size) in OUTPUT_SIZES.iter().copied().enumerate() {
            let quality = OUTPUT_QUALITIES.get(index).copied().unwrap_or(0.70);
            match &decoded {
                DecodedSource::Static(rgba) => {
                    let square = crop_and_resize_square(rgba, output_size);
                    let encoded = encode_still_frame(&square, output_size, quality, index == 0)?;
                    if encoded.len() <= AVATAR_IMAGE_LIMITS.stored_bytes {
                        return Ok(build_output(encoded, output_size, false, 1));
                    }
                }
                DecodedSource::Animated(frames) => {
                    let frame_count = frames.len();
                    let per_frame_lossless_cap =
                        AVATAR_IMAGE_LIMITS.stored_bytes / frame_count.max(1);
                    let mut mux_frames = Vec::with_capacity(frame_count);
                    let mut cumulative_bytes = 0usize;
                    let mut exceeded = false;

                    for frame in frames {
                        let square = crop_and_resize_square(&frame.image, output_size);
                        let still = if index == 0 {
                            let lossless = encode_still_webp_buf(square.as_raw(), output_size)?;
                            if lossless.len() <= per_frame_lossless_cap {
                                lossless
                            } else {
                                encode_still_frame(&square, output_size, quality, false)?
                            }
                        } else {
                            encode_still_frame(&square, output_size, quality, false)?
                        };
                        let subchunks = extract_webp_frame_subchunks(&still)?;
                        cumulative_bytes = cumulative_bytes.saturating_add(subchunks.len());
                        if cumulative_bytes > AVATAR_IMAGE_LIMITS.stored_bytes {
                            exceeded = true;
                            break;
                        }
                        mux_frames.push((subchunks, frame.duration_ms));
                    }

                    if !exceeded {
                        let muxed = mux_animated_webp(&mux_frames, output_size);
                        if muxed.len() <= AVATAR_IMAGE_LIMITS.stored_bytes {
                            return Ok(build_output(muxed, output_size, true, frame_count));
                        }
                    }
                }
            }
        }

        Err(AvatarNormalizeError::OutputTooLarge {
            limit: AVATAR_IMAGE_LIMITS.stored_bytes,
            i18n: "app.avatar.tooLarge",
        })
    }

    pub fn normalize_file(
        input: &Path,
        output: Option<&Path>,
    ) -> Result<NormalizedAvatarOutput, AvatarNormalizeError> {
        let hint = mime_hint_for_path(input);
        if let Some(raw_hint) = hint
            && !ACCEPTED_SOURCE_MIMES.contains(&raw_hint)
        {
            return Err(AvatarNormalizeError::UnsupportedType {
                mime_hint: Some(raw_hint.to_string()),
                i18n: "app.avatar.type",
            });
        }

        let meta = std::fs::metadata(input).map_err(|e| AvatarNormalizeError::Io {
            detail: e.to_string(),
            i18n: "app.avatar.processFailed",
        })?;
        if meta.len() > AVATAR_IMAGE_LIMITS.source_bytes as u64 {
            return Err(AvatarNormalizeError::SourceTooLarge {
                byte_length: usize::try_from(meta.len()).unwrap_or(usize::MAX),
                limit: AVATAR_IMAGE_LIMITS.source_bytes,
                i18n: "app.avatar.size",
            });
        }

        let bytes = std::fs::read(input).map_err(|e| AvatarNormalizeError::Io {
            detail: e.to_string(),
            i18n: "app.avatar.processFailed",
        })?;
        let normalized = Self::normalize_bytes(&bytes, hint)?;

        if let Some(out_path) = output {
            if let Some(parent) = out_path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent).map_err(|e| AvatarNormalizeError::Io {
                    detail: e.to_string(),
                    i18n: "app.avatar.processFailed",
                })?;
            }
            std::fs::write(out_path, &normalized.bytes).map_err(|e| AvatarNormalizeError::Io {
                detail: e.to_string(),
                i18n: "app.avatar.processFailed",
            })?;
        }

        Ok(normalized)
    }
}
