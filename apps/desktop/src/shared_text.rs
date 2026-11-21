//! Text from a received document or an AirDrop share item.
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use wayline_core::airdrop::web_links;

const MAX_TEXT_FILE_BYTES: u64 = 64 * 1024;
const MAX_PREVIEW_CHARS: usize = 200;
const AUTO_OPEN_CHAR_LIMIT: usize = 120;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
const FILE_SHARE_READ: u32 = 0x0000_0001;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReceivedText {
    pub(crate) original: String,
    pub(crate) preview: String,
    pub(crate) character_count: usize,
    pub(crate) urls: Vec<String>,
}

impl ReceivedText {
    /// A short share with one distinct web link can be opened with the caller's permission.
    pub(crate) fn eligible_for_automatic_open(&self) -> bool {
        self.character_count < AUTO_OPEN_CHAR_LIMIT && self.urls.len() == 1
    }
}

pub(crate) fn classify_text(original: String) -> Option<ReceivedText> {
    if original.trim().is_empty() || original.contains('\0') {
        return None;
    }

    let character_count = original.chars().count();
    let preview = if character_count > MAX_PREVIEW_CHARS {
        let mut preview: String = original.chars().take(MAX_PREVIEW_CHARS).collect();
        preview.push('…');
        preview
    } else {
        original.clone()
    };
    let urls = web_links(std::slice::from_ref(&original));
    Some(ReceivedText {
        original,
        preview,
        character_count,
        urls,
    })
}

pub(crate) fn read_single_text(files: &[PathBuf]) -> Option<ReceivedText> {
    let [path] = files else {
        return None;
    };
    if !is_text_path(path) || !is_bounded_regular_file(&fs::symlink_metadata(path).ok()?) {
        return None;
    }

    // Verify the opened handle to catch a replacement after the path check.
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !is_bounded_regular_file(&metadata) {
        return None;
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_TEXT_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > MAX_TEXT_FILE_BYTES {
        return None;
    }
    classify_text(decode_text(&bytes)?)
}

pub(crate) fn is_text_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("txt"))
}

fn is_bounded_regular_file(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
        && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
        && metadata.len() <= MAX_TEXT_FILE_BYTES
}

fn decode_text(bytes: &[u8]) -> Option<String> {
    match bytes {
        [0xff, 0xfe, rest @ ..] => decode_utf16(rest, u16::from_le_bytes),
        [0xfe, 0xff, rest @ ..] => decode_utf16(rest, u16::from_be_bytes),
        [0xef, 0xbb, 0xbf, rest @ ..] => str::from_utf8(rest).ok().map(str::to_owned),
        _ => str::from_utf8(bytes).ok().map(str::to_owned),
    }
}

fn decode_utf16(bytes: &[u8], code_unit: impl Fn([u8; 2]) -> u16) -> Option<String> {
    let (units, remainder) = bytes.as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    char::decode_utf16(units.iter().copied().map(code_unit))
        .collect::<Result<String, _>>()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_limits_and_automatic_open_use_the_original_character_count() {
        for count in [119, 120, 200, 201] {
            let link = "https://b23.tv/test";
            let prefix = "字".repeat(count - link.chars().count() - 1);
            let source = format!("{prefix} {link}");
            let share = classify_text(source.clone()).unwrap();
            assert_eq!(share.original, source);
            assert_eq!(share.character_count, count);
            assert_eq!(share.urls, [link]);
            assert_eq!(share.eligible_for_automatic_open(), count < 120);
            if count > 200 {
                assert_eq!(
                    share.preview,
                    format!("{}…", source.chars().take(200).collect::<String>())
                );
            } else {
                assert_eq!(share.preview, source);
            }
        }
    }

    #[test]
    fn multiple_distinct_links_stay_text() {
        let share =
            classify_text("示例 https://v.douyin.com/test/ https://b23.tv/test".into()).unwrap();
        assert_eq!(share.urls.len(), 2);
        assert!(!share.eligible_for_automatic_open());

        let repeated = classify_text("https://b23.tv/test \nhttps://b23.tv/test".into()).unwrap();
        assert!(repeated.eligible_for_automatic_open());
        let app_only = classify_text("视频 bilibili://video/1 file:///C:/example".into()).unwrap();
        assert!(!app_only.eligible_for_automatic_open());
    }

    #[test]
    fn single_txt_reads_utf8_and_utf16_boms_without_changing_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Share.TXT");
        let expected = "复制打开抖音，查看〖示例作品〗https://v.douyin.com/Test/ 🐱\r\n";
        let mut utf8_bom = vec![0xef, 0xbb, 0xbf];
        utf8_bom.extend_from_slice(expected.as_bytes());
        let mut utf16_le = vec![0xff, 0xfe];
        utf16_le.extend(expected.encode_utf16().flat_map(u16::to_le_bytes));
        let mut utf16_be = vec![0xfe, 0xff];
        utf16_be.extend(expected.encode_utf16().flat_map(u16::to_be_bytes));
        for bytes in [expected.as_bytes().to_vec(), utf8_bom, utf16_le, utf16_be] {
            fs::write(&path, bytes).unwrap();
            let share = read_single_text(std::slice::from_ref(&path)).unwrap();
            assert_eq!(share.original, expected);
            assert_eq!(share.preview, expected);
            assert_eq!(share.urls, ["https://v.douyin.com/Test/"]);
        }
    }

    #[test]
    fn rejects_over_limit_invalid_encodings_and_non_single_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("share.txt");
        for content in [
            vec![b'a'; MAX_TEXT_FILE_BYTES as usize + 1],
            vec![0xff],
            vec![0xff, 0xfe, 0x01],
            vec![0xfe, 0xff, 0xd8, 0x00],
            vec![b'x', 0],
            b" \t\r\n".to_vec(),
        ] {
            fs::write(&path, content).unwrap();
            assert!(read_single_text(std::slice::from_ref(&path)).is_none());
        }
        let full = "a".repeat(MAX_TEXT_FILE_BYTES as usize);
        fs::write(&path, &full).unwrap();
        assert_eq!(
            read_single_text(std::slice::from_ref(&path))
                .unwrap()
                .original,
            full
        );
        assert!(read_single_text(&[path.clone(), path]).is_none());
        let other = dir.path().join("other.md");
        fs::write(&other, "ordinary text").unwrap();
        assert!(read_single_text(&[other]).is_none());
        let folder = dir.path().join("directory.txt");
        fs::create_dir(&folder).unwrap();
        assert!(read_single_text(&[folder]).is_none());
    }
}
