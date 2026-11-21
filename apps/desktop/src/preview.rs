//! Bounded shell thumbnails/icons, extracted off the GPUI thread.
use anyhow::{Result, ensure};
use gpui::RenderImage;
use image::{Frame, ImageFormat, ImageReader, Limits, RgbaImage};
use std::{
    fs::{self, OpenOptions},
    io::{Cursor, Read},
    mem::size_of,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use windows::{
    Win32::{Foundation::SIZE, Graphics::Gdi::*, UI::Shell::*},
    core::HSTRING,
};

pub const MAX_PREVIEWS: usize = 4;
const PREVIEW_PIXELS: i32 = 96;
const SHARE_PREVIEW_PIXELS: u32 = 480;
const SHARE_ICON_BYTES: usize = 4 * 1024 * 1024;
const SHARE_FILE_BYTES: u64 = 24 * 1024 * 1024;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Image,
    Text,
    Word,
    Excel,
    PowerPoint,
    Pdf,
    Archive,
    Audio,
    Video,
    Other,
}
impl FileKind {
    pub fn for_path(path: &Path) -> Self {
        match path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase()
            .as_str()
        {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "heif"
            | "avif" | "ico" | "svg" => Self::Image,
            "txt" | "text" | "md" | "log" | "json" | "xml" | "yaml" | "yml" | "ini" | "toml"
            | "rs" | "py" | "js" | "ts" | "html" | "css" => Self::Text,
            "doc" | "docx" | "docm" | "dot" | "dotx" | "odt" | "rtf" => Self::Word,
            "xls" | "xlsx" | "xlsm" | "xlsb" | "xlt" | "xltx" | "csv" | "tsv" | "ods" => {
                Self::Excel
            }
            "ppt" | "pptx" | "pptm" | "pps" | "ppsx" | "pot" | "potx" | "odp" => Self::PowerPoint,
            "pdf" => Self::Pdf,
            "zip" | "7z" | "rar" | "tar" | "gz" => Self::Archive,
            "mp3" | "wav" | "flac" | "m4a" | "aac" | "ogg" => Self::Audio,
            "mp4" | "mov" | "mkv" | "avi" | "webm" | "wmv" => Self::Video,
            _ => Self::Other,
        }
    }
    pub fn badge(self, language: crate::i18n::Language) -> (&'static str, u32) {
        let text = language.strings();
        match self {
            Self::Image => ("IMG", 0x4186be),
            Self::Text => ("TXT", 0x65778b),
            Self::Word => ("W", 0x2564b6),
            Self::Excel => ("X", 0x217346),
            Self::PowerPoint => ("P", 0xc45131),
            Self::Pdf => ("PDF", 0xbd4145),
            Self::Archive => ("ZIP", 0x9e762c),
            Self::Audio => (text.badge_audio, 0x9959aa),
            Self::Video => (text.badge_video, 0x7462b7),
            Self::Other => ("FILE", 0x65778b),
        }
    }
}

pub fn load(path: &Path) -> Result<Arc<RenderImage>> {
    let _apartment = crate::picker::Apartment::new()?;
    unsafe {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None)?;
        let size = SIZE {
            cx: PREVIEW_PIXELS,
            cy: PREVIEW_PIXELS,
        };
        let bitmap = if FileKind::for_path(path) == FileKind::Image {
            factory
                .GetImage(size, SIIGBF_THUMBNAILONLY)
                .or_else(|_| factory.GetImage(size, SIIGBF_ICONONLY))?
        } else {
            // Documents show their associated app/type icon, not page content.
            factory.GetImage(size, SIIGBF_ICONONLY)?
        };
        bitmap_image(&Bitmap(bitmap))
    }
}

/// A small raster preview from a received image or the sender's inline icon.
/// Untrusted shares are decoded in memory without asking the shell for an icon.
pub fn share_image(icon: Option<&[u8]>, files: &[PathBuf]) -> Option<Arc<RenderImage>> {
    files
        .iter()
        .filter(|path| FileKind::for_path(path) == FileKind::Image)
        .take(MAX_PREVIEWS)
        .find_map(|path| share_file_image(path))
        .or_else(|| {
            let icon = icon.filter(|bytes| bytes.len() <= SHARE_ICON_BYTES)?;
            decode_share_image(icon, 2048, 24 * 1024 * 1024)
        })
}

fn share_file_image(path: &Path) -> Option<Arc<RenderImage>> {
    if !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || metadata.len() > SHARE_FILE_BYTES
    {
        return None;
    }
    let mut encoded = vec![0; metadata.len() as usize];
    file.read_exact(&mut encoded).ok()?;
    decode_share_image(&encoded, 8192, 64 * 1024 * 1024)
}

fn decode_share_image(
    encoded: &[u8],
    max_dimension: u32,
    max_alloc: u64,
) -> Option<Arc<RenderImage>> {
    let mut reader = ImageReader::new(Cursor::new(encoded))
        .with_guessed_format()
        .ok()?;
    if !matches!(
        reader.format(),
        Some(
            ImageFormat::Png
                | ImageFormat::Jpeg
                | ImageFormat::Tiff
                | ImageFormat::WebP
                | ImageFormat::Gif
                | ImageFormat::Bmp
        )
    ) {
        return None;
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(max_dimension);
    limits.max_image_height = Some(max_dimension);
    limits.max_alloc = Some(max_alloc);
    reader.limits(limits);
    let mut image = reader.decode().ok()?;
    if image.width() == 0 || image.height() == 0 {
        return None;
    }
    if image.width() > SHARE_PREVIEW_PIXELS || image.height() > SHARE_PREVIEW_PIXELS {
        image = image.thumbnail(SHARE_PREVIEW_PIXELS, SHARE_PREVIEW_PIXELS);
    }
    let mut buffer = image.into_rgba8();
    // GPUI expects straight-alpha BGRA bytes inside its RgbaImage storage.
    for pixel in buffer.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(render_image(buffer))
}

fn render_image(buffer: RgbaImage) -> Arc<RenderImage> {
    Arc::new(RenderImage::new(vec![Frame::new(buffer)]))
}

struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}
struct MemoryDc(HDC);
impl Drop for MemoryDc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

fn bitmap_image(bitmap: &Bitmap) -> Result<Arc<RenderImage>> {
    unsafe {
        let mut description = BITMAP::default();
        ensure!(
            GetObjectW(
                bitmap.0.into(),
                size_of::<BITMAP>() as i32,
                Some(&mut description as *mut _ as _)
            ) != 0,
            "无法读取文件预览"
        );
        let (width, height) = (description.bmWidth, description.bmHeight.unsigned_abs());
        ensure!(
            (1..=1024).contains(&width) && (1..=1024).contains(&height),
            "预览尺寸无效"
        );
        let dc = MemoryDc(CreateCompatibleDC(None));
        ensure!(!dc.0.is_invalid(), "无法创建预览画布");
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -(height as i32), // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        ensure!(
            GetDIBits(
                dc.0,
                bitmap.0,
                0,
                height,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS
            ) == height as i32,
            "无法读取预览像素"
        );
        normalize_alpha(&mut pixels, description.bmBitsPixel == 32);
        // The shell returns BGRA; GPUI RenderImage also expects BGRA, despite
        // using image::RgbaImage as its underlying buffer container.
        let buffer = RgbaImage::from_raw(width as u32, height, pixels).unwrap();
        Ok(render_image(buffer))
    }
}

fn normalize_alpha(pixels: &mut [u8], has_alpha: bool) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        if !has_alpha {
            pixel[3] = 255;
        }
        let alpha = pixel[3] as u32;
        if alpha > 0 && alpha < 255 {
            // Shell bitmaps are premultiplied; GPUI's image shader expects
            // straight alpha. Preserve fully transparent image backgrounds.
            for color in &mut pixel[..3] {
                *color = ((*color as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_preview_prefers_actual_raster_content_and_falls_back_to_the_offer() {
        let directory = tempfile::tempdir().unwrap();
        let photo = directory.path().join("screenshot.PNG");
        RgbaImage::from_pixel(3, 2, image::Rgba([230, 60, 22, 240]))
            .save(&photo)
            .unwrap();
        let mut icon = Cursor::new(Vec::new());
        RgbaImage::from_pixel(1, 1, image::Rgba([20, 100, 200, 255]))
            .write_to(&mut icon, ImageFormat::Png)
            .unwrap();
        let icon = icon.into_inner();
        assert_eq!(
            &share_image(Some(&icon), &[photo])
                .unwrap()
                .as_bytes(0)
                .unwrap()[..4],
            &[22, 60, 230, 240]
        );

        let renamed = directory.path().join("document.jpg");
        fs::write(&renamed, b"plain text").unwrap();
        assert!(share_image(None, std::slice::from_ref(&renamed)).is_none());
        assert_eq!(
            share_image(Some(&icon), &[renamed])
                .unwrap()
                .as_bytes(0)
                .unwrap(),
            &[200, 100, 20, 255]
        );
    }

    #[test]
    fn share_preview_accepts_jpeg_and_tiff_icons_and_enforces_dimension_limits() {
        for format in [ImageFormat::Jpeg, ImageFormat::Tiff] {
            let mut icon = Cursor::new(Vec::new());
            image::RgbImage::from_pixel(5, 3, image::Rgb([20, 40, 60]))
                .write_to(&mut icon, format)
                .unwrap();
            assert_eq!(
                share_image(Some(icon.get_ref()), &[])
                    .unwrap()
                    .as_bytes(0)
                    .unwrap()
                    .len(),
                5 * 3 * 4
            );
        }
        let mut too_wide = Cursor::new(Vec::new());
        RgbaImage::new(2049, 1)
            .write_to(&mut too_wide, ImageFormat::Png)
            .unwrap();
        assert!(share_image(Some(too_wide.get_ref()), &[]).is_none());
    }
    #[test]
    fn windows_shell_extracts_a_real_thumbnail_and_document_icon() {
        let directory = tempfile::tempdir().unwrap();
        let photo = directory.path().join("缩略图.PNG");
        RgbaImage::from_pixel(32, 20, image::Rgba([235, 70, 20, 255]))
            .save_with_format(&photo, image::ImageFormat::Png)
            .unwrap();
        let thumbnail = load(&photo).unwrap();
        let pixels = thumbnail.as_bytes(0).unwrap();
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[0] < 30 && (60..80).contains(&p[1]) && p[2] > 225 && p[3] == 255),
            "thumbnail must preserve the photo's colors in BGRA order"
        );
        let document = directory.path().join("notes.txt");
        std::fs::write(&document, "Wayline file icon test").unwrap();
        let icon = load(&document).unwrap();
        assert!(
            icon.as_bytes(0)
                .unwrap()
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 0)
        );
    }
    #[test]
    fn document_types_and_uppercase_image_extensions_have_distinct_fallbacks() {
        for (name, kind) in [
            ("照片.PNG", FileKind::Image),
            ("notes.TEXT", FileKind::Text),
            ("report.docx", FileKind::Word),
            ("table.XLSX", FileKind::Excel),
            ("talk.pptx", FileKind::PowerPoint),
            ("README", FileKind::Other),
        ] {
            assert_eq!(FileKind::for_path(Path::new(name)), kind);
        }
    }
    #[test]
    fn shell_alpha_is_converted_without_losing_transparency_or_channel_order() {
        let mut pixels = [10, 20, 40, 128, 0, 0, 0, 0, 20, 30, 40, 255];
        normalize_alpha(&mut pixels, true);
        assert_eq!(pixels, [20, 40, 80, 128, 0, 0, 0, 0, 20, 30, 40, 255]);
        normalize_alpha(&mut pixels, false);
        assert_eq!(pixels[7], 255);
    }
}
