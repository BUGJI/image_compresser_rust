use std::fs;
use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, GenericImageView, ImageFormat, ImageReader};
use webp::Encoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Done,
    Exists,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailReason {
    Read,
    Decode,
    Encode,
    Write,
}

impl FailReason {
    pub fn as_str(self) -> &'static str {
        match self {
            FailReason::Read => "read",
            FailReason::Decode => "decode",
            FailReason::Encode => "encode",
            FailReason::Write => "write",
        }
    }
}

#[derive(Debug)]
pub struct FileResult {
    pub status: Status,
    pub fail_reason: Option<FailReason>,
    pub err: Option<String>,
    pub width: u32,
    pub height: u32,
    pub in_bytes: u64,
    pub out_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct ConvConfig {
    pub resize: bool,
    pub width: u32,
    pub quality: u8,
    pub overwrite: bool,
}

fn fail(reason: FailReason, err: String) -> FileResult {
    FileResult {
        status: Status::Failed,
        fail_reason: Some(reason),
        err: Some(err),
        width: 0,
        height: 0,
        in_bytes: 0,
        out_bytes: 0,
    }
}

fn empty(status: Status) -> FileResult {
    FileResult {
        status,
        fail_reason: None,
        err: None,
        width: 0,
        height: 0,
        in_bytes: 0,
        out_bytes: 0,
    }
}

pub fn image_format_of(ext: &str) -> Option<ImageFormat> {
    match ext {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "gif" => Some(ImageFormat::Gif),
        "bmp" => Some(ImageFormat::Bmp),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        "ico" => Some(ImageFormat::Ico),
        "tga" => Some(ImageFormat::Tga),
        "pnm" | "pbm" | "pgm" | "ppm" | "pam" => Some(ImageFormat::Pnm),
        "qoi" => Some(ImageFormat::Qoi),
        "hdr" => Some(ImageFormat::Hdr),
        "dds" => Some(ImageFormat::Dds),
        "webp" => Some(ImageFormat::WebP),
        _ => None,
    }
}

pub fn convert_one(src: &Path, out: &Path, cfg: &ConvConfig) -> FileResult {
    if !cfg.overwrite && out.exists() {
        return empty(Status::Exists);
    }

    let in_size = match fs::metadata(src) {
        Ok(m) => m.len(),
        Err(e) => return fail(FailReason::Read, format!("读取文件失败: {e}")),
    };

    let ext = src
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let fmt = match image_format_of(&ext) {
        Some(f) => f,
        None => return fail(FailReason::Decode, "不支持的扩展名".into()),
    };

    let img = match decode(src, fmt) {
        Ok(img) => img,
        Err(e) => return fail(FailReason::Decode, format!("解码错误: {e}")),
    };

    let img = apply_resize(img, cfg);

    let out_bytes = match encode_webp(&img, cfg.quality) {
        Some(b) => b,
        None => return fail(FailReason::Encode, "WebP 编码失败".into()),
    };

    if let Err(e) = write_file(out, &out_bytes, cfg.overwrite) {
        return fail(
            FailReason::Write,
            format!("写入 {} 失败: {}", out.display(), e),
        );
    }

    let (width, height) = img.dimensions();
    FileResult {
        status: Status::Done,
        fail_reason: None,
        err: None,
        width,
        height,
        in_bytes: in_size,
        out_bytes: out_bytes.len() as u64,
    }
}

fn decode(src: &Path, fmt: ImageFormat) -> image::ImageResult<DynamicImage> {
    let bytes = fs::read(src)?;
    decode_bytes(&bytes, Some(fmt)).or_else(|_| decode_bytes(&bytes, None))
}

fn decode_bytes(bytes: &[u8], fmt: Option<ImageFormat>) -> image::ImageResult<DynamicImage> {
    let reader = ImageReader::new(Cursor::new(bytes));
    let mut reader = match fmt {
        Some(f) => {
            let mut r = reader;
            r.set_format(f);
            r
        }
        None => reader.with_guessed_format()?,
    };
    reader.no_limits();
    reader.decode()
}

fn apply_resize(img: DynamicImage, cfg: &ConvConfig) -> DynamicImage {
    let (w, h) = img.dimensions();
    if !cfg.resize || w <= cfg.width {
        return img;
    }
    let new_w = cfg.width;
    let new_h = (((h as u64) * (new_w as u64)) / (w as u64)).max(1) as u32;
    img.resize_exact(new_w, new_h, image::imageops::FilterType::Triangle)
}

fn encode_webp(img: &DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let (w, h) = img.dimensions();
    let memory = if img.color().has_alpha() {
        let buf = img.to_rgba8();
        Encoder::from_rgba(buf.as_raw(), w, h).encode(quality as f32)
    } else {
        let buf = img.to_rgb8();
        Encoder::from_rgb(buf.as_raw(), w, h).encode(quality as f32)
    };
    let data = memory.as_ref();
    if data.is_empty() {
        return None;
    }
    Some(data.to_vec())
}

fn write_file(out: &Path, bytes: &[u8], overwrite: bool) -> std::io::Result<()> {
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = out.with_extension("webp.tmp");
    fs::write(&tmp, bytes)?;
    if overwrite && out.exists() {
        fs::remove_file(out)?;
    }
    match fs::rename(&tmp, out) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}
