use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::{OsStr, OsString},
    fs,
    mem::{offset_of, size_of},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Component, Path, PathBuf, Prefix},
    ptr,
};
use windows::{
    Win32::{
        Foundation::{
            ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_NOT_SAME_DEVICE,
            GlobalFree, HANDLE, HGLOBAL, HWND,
        },
        Storage::FileSystem::{CopyFileW, MOVEFILE_WRITE_THROUGH, MoveFileExW},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW,
                SetClipboardData,
            },
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
            Ole::{CF_HDROP, DROPEFFECT_COPY},
        },
        UI::{
            Shell::{
                CFSTR_PREFERREDDROPEFFECT, DROPFILES, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC,
                SHELLEXECUTEINFOW, ShellExecuteExW,
            },
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::{HRESULT, PCWSTR, w},
};

pub(crate) struct MoveOutcome {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) fn open_files(paths: &[PathBuf]) -> Result<()> {
    let paths = existing_items(paths)?;
    let _apartment = crate::picker::Apartment::new()?;
    for path in paths {
        let wide = terminated(&path)?;
        let mut request = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: w!("open"),
            lpFile: PCWSTR(wide.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        // The UTF-16 path and request remain live until activation completes.
        unsafe { ShellExecuteExW(&mut request) }
            .with_context(|| format!("无法打开 {}", path.display()))?;
    }
    Ok(())
}

pub(crate) fn save_as(paths: &[PathBuf], directory: &Path) -> Result<Vec<PathBuf>> {
    let (paths, directory) = file_operation_items(paths, directory)?;
    let stage = tempfile::tempdir_in(&directory).context("无法创建保存暂存目录")?;
    let mut staged = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        let temporary = stage.path().join(index.to_string());
        let is_dir = metadata(path)?.is_dir();
        copy_item(path, &temporary).with_context(|| format!("无法复制 {}", path.display()))?;
        staged.push((temporary, is_dir));
    }
    let mut saved = Vec::with_capacity(paths.len());
    for (original, (temporary, is_dir)) in paths.iter().zip(staged) {
        let path =
            publish(&temporary, &directory, basename(original)?, is_dir).with_context(|| {
                format!("无法保存 {}，已保存 {} 项", original.display(), saved.len())
            })?;
        saved.push(path);
    }
    Ok(saved)
}

pub(crate) fn move_to(paths: &[PathBuf], directory: &Path) -> Result<MoveOutcome> {
    let (sources, directory) = file_operation_items(paths, directory)?;
    let destination = fs::canonicalize(&directory)?;
    let mut outcome = MoveOutcome {
        paths: paths.to_vec(),
        warnings: Vec::new(),
    };
    for (index, source) in sources.iter().enumerate() {
        let result = (|| -> Result<(PathBuf, Option<String>)> {
            let parent = source.parent().context("原文件没有上级目录")?;
            if fs::canonicalize(parent)? == destination {
                return Ok((source.clone(), None));
            }
            let is_dir = metadata(source)?.is_dir();
            match publish(source, &directory, basename(source)?, is_dir) {
                Ok(target) => Ok((target, None)),
                Err(error) if is_cross_volume(&error) => {
                    move_cross_volume(source, parent, &directory, is_dir)
                }
                Err(error) => Err(error),
            }
        })();
        match result {
            Ok((path, warning)) => {
                outcome.paths[index] = path;
                outcome.warnings.extend(warning);
            }
            Err(error) => outcome
                .warnings
                .push(format!("无法移动 {}：{error:#}", source.display())),
        }
    }
    Ok(outcome)
}

fn move_cross_volume(
    source: &Path,
    source_parent: &Path,
    destination: &Path,
    is_dir: bool,
) -> Result<(PathBuf, Option<String>)> {
    let target_stage = tempfile::tempdir_in(destination).context("无法创建目标暂存目录")?;
    let source_stage = tempfile::tempdir_in(source_parent).context("无法创建原文件暂存目录")?;
    let original = source_stage.path().join("original");
    let copy = target_stage.path().join("copy");
    let name = basename(source)?;
    rename_no_replace(source, &original).context("无法暂存原文件")?;
    let target = copy_item(&original, &copy)
        .context("无法复制到目标磁盘")
        .and_then(|()| publish(&copy, destination, name, is_dir));
    match target {
        Ok(path) => {
            let stage = source_stage.path().to_owned();
            let cleanup = source_stage.close().err().map(|error| {
                format!(
                    "已移动到 {}，但原磁盘的暂存副本未完全清理（{}）：{error}",
                    path.display(),
                    stage.display()
                )
            });
            Ok((path, cleanup))
        }
        Err(error) => match rename_no_replace(&original, source) {
            Ok(()) => Err(error),
            Err(restore_error) => {
                // Keep the intact original if another process has taken its old path.
                let _ = source_stage.keep();
                let warning = format!(
                    "移动 {} 失败，原文件保存在 {}。无法恢复原路径：{restore_error}；{error:#}",
                    source.display(),
                    original.display()
                );
                Ok((original, Some(warning)))
            }
        },
    }
}

fn file_operation_items(paths: &[PathBuf], directory: &Path) -> Result<(Vec<PathBuf>, PathBuf)> {
    let paths = existing_items(paths)?;
    let directory = std::path::absolute(directory)?;
    ensure!(
        directory.is_dir(),
        "目标文件夹不可用：{}",
        directory.display()
    );
    let canonical_directory = fs::canonicalize(&directory)?;
    for path in &paths {
        ensure!(
            !metadata(path)?.is_dir() || !canonical_directory.starts_with(fs::canonicalize(path)?),
            "不能将目录保存或移动到其自身内部：{}",
            path.display(),
        );
    }
    Ok((paths, directory))
}

fn existing_items(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    ensure!(!paths.is_empty(), "没有可用的接收文件");
    paths
        .iter()
        .map(|path| {
            let path = std::path::absolute(path)?;
            metadata(&path)?;
            basename(&path)?;
            Ok(path)
        })
        .collect()
}

fn basename(path: &Path) -> Result<&OsStr> {
    path.file_name().context("无法读取接收文件的名称")
}

fn metadata(path: &Path) -> Result<fs::Metadata> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("无法访问 {}", path.display()))?;
    ensure!(
        (metadata.is_file() || metadata.is_dir()) && !metadata.file_type().is_symlink(),
        "不支持此文件类型：{}",
        path.display()
    );
    Ok(metadata)
}

fn copy_item(source: &Path, target: &Path) -> Result<()> {
    if metadata(source)?.is_dir() {
        fs::create_dir(target)?;
        for child in fs::read_dir(source)? {
            let child = child?;
            copy_item(&child.path(), &target.join(child.file_name()))?;
        }
    } else {
        let source = file_api_path(source);
        let target = file_api_path(target);
        // Both strings are terminated and live for the synchronous copy. Ask
        // Windows to retain file attributes and streams and refuse overwriting.
        unsafe { CopyFileW(PCWSTR(source.as_ptr()), PCWSTR(target.as_ptr()), true) }?;
    }
    Ok(())
}

fn publish(source: &Path, directory: &Path, name: &OsStr, is_directory: bool) -> Result<PathBuf> {
    for number in 0..10_000 {
        let target = directory.join(available_name(name, is_directory, number));
        match rename_no_replace(source, &target) {
            Ok(()) => return Ok(target),
            Err(error)
                if error.code() == HRESULT::from_win32(ERROR_ALREADY_EXISTS.0)
                    || error.code() == HRESULT::from_win32(ERROR_FILE_EXISTS.0)
                    || (error.code() == HRESULT::from_win32(ERROR_ACCESS_DENIED.0)
                        && fs::symlink_metadata(&target).is_ok()) => {}
            Err(error) => return Err(error.into()),
        }
    }
    bail!("无法为 {} 找到不冲突的文件名", name.to_string_lossy())
}

fn available_name(name: &OsStr, is_directory: bool, number: u32) -> OsString {
    if number == 0 {
        return name.to_owned();
    }
    let path = Path::new(name);
    let extension = if is_directory { None } else { path.extension() };
    let stem = extension.and_then(|_| path.file_stem()).unwrap_or(name);
    let mut base: Vec<u16> = stem.encode_wide().collect();
    let suffix: Vec<u16> = format!(" ({number})").encode_utf16().collect();
    let mut extension: Vec<u16> = extension
        .into_iter()
        .flat_map(|part| std::iter::once(u16::from(b'.')).chain(part.encode_wide()))
        .collect();
    const MAX_COMPONENT_UNITS: usize = 255;
    if extension.len() + suffix.len() >= MAX_COMPONENT_UNITS {
        base = name.encode_wide().collect();
        extension.clear();
    }
    let keep = MAX_COMPONENT_UNITS - suffix.len() - extension.len();
    if base.len() > keep {
        let split_pair =
            (0xD800..=0xDBFF).contains(&base[keep - 1]) && (0xDC00..=0xDFFF).contains(&base[keep]);
        base.truncate(keep - usize::from(split_pair));
    }
    base.extend(suffix);
    base.extend(extension);
    OsString::from_wide(&base)
}

fn rename_no_replace(source: &Path, target: &Path) -> windows::core::Result<()> {
    let source = file_api_path(source);
    let target = file_api_path(target);
    // The buffers are terminated, and no replacement or implicit cross-volume
    // copy flag is used. Collisions and cross-volume moves are handled above.
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(target.as_ptr()),
            MOVEFILE_WRITE_THROUGH,
        )
    }
}

fn is_cross_volume(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<windows::core::Error>()
        .is_some_and(|native| native.code() == HRESULT::from_win32(ERROR_NOT_SAME_DEVICE.0))
}

fn terminated(path: &Path) -> Result<Vec<u16>> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    ensure!(!wide.contains(&0), "文件路径包含空字符");
    wide.push(0);
    Ok(wide)
}

fn file_api_path(path: &Path) -> Vec<u16> {
    let wide = path.as_os_str().encode_wide().chain(Some(0));
    // std::fs supports long paths; use the extended form for the two
    // direct Win32 filesystem calls when the ordinary name exceeds MAX_PATH.
    if wide.clone().count() > 260
        && let Some(Component::Prefix(prefix)) = path.components().next()
    {
        match prefix.kind() {
            Prefix::Disk(_) => return r"\\?\".encode_utf16().chain(wide).collect(),
            Prefix::UNC(..) => return r"\\?\UNC\".encode_utf16().chain(wide.skip(2)).collect(),
            _ => {}
        }
    }
    wide.collect()
}

pub(crate) fn copy_to_clipboard(paths: &[PathBuf], owner: HWND) -> Result<()> {
    ensure!(!owner.0.is_null(), "剪贴板操作需要有效窗口");
    let paths = existing_items(paths)?;
    let mut dropfiles = ClipboardMemory::new(&dropfiles_bytes(&paths)?)?;
    let mut effect = ClipboardMemory::new(&DROPEFFECT_COPY.0.to_le_bytes())?;
    // The owner is a live HWND supplied by the UI thread. Both payloads are
    // constructed before taking ownership of the clipboard.
    unsafe { OpenClipboard(Some(owner)) }.context("剪贴板正在使用")?;
    let _clipboard = Clipboard;
    let preferred = unsafe { RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT) };
    ensure!(preferred != 0, "无法注册文件复制剪贴板格式");
    unsafe { EmptyClipboard() }.context("无法清空剪贴板")?;
    effect.put(preferred)?;
    dropfiles.put(u32::from(CF_HDROP.0))
}

fn dropfiles_bytes(paths: &[PathBuf]) -> Result<Vec<u8>> {
    let mut bytes = vec![0; size_of::<DROPFILES>()];
    bytes[..size_of::<u32>()].copy_from_slice(&(size_of::<DROPFILES>() as u32).to_le_bytes());
    let wide = offset_of!(DROPFILES, fWide);
    bytes[wide..wide + size_of::<u32>()].copy_from_slice(&1u32.to_le_bytes());
    for path in paths {
        for unit in terminated(path)? {
            bytes.extend(unit.to_le_bytes());
        }
    }
    bytes.extend(0u16.to_le_bytes());
    Ok(bytes)
}

struct Clipboard;

impl Drop for Clipboard {
    fn drop(&mut self) {
        // Paired with the successful OpenClipboard on this UI thread.
        let _ = unsafe { CloseClipboard() };
    }
}

struct ClipboardMemory(HGLOBAL);

impl ClipboardMemory {
    fn new(bytes: &[u8]) -> Result<Self> {
        // SetClipboardData requires a movable HGLOBAL, rather than a Rust buffer.
        let memory = Self(unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }?);
        let data = unsafe { GlobalLock(memory.0) };
        ensure!(!data.is_null(), "无法准备剪贴板内存");
        // The block is locked, sized above, and disjoint from bytes.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), data.cast(), bytes.len());
            let _ = GlobalUnlock(memory.0);
        }
        Ok(memory)
    }

    fn put(&mut self, format: u32) -> Result<()> {
        // The system owns the unlocked HGLOBAL after a successful call.
        unsafe { SetClipboardData(format, Some(HANDLE(self.0.0))) }
            .context("无法将文件列表复制到剪贴板")?;
        self.0 = HGLOBAL::default();
        Ok(())
    }
}

impl Drop for ClipboardMemory {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            let _ = unsafe { GlobalFree(Some(self.0)) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_file_and_folder_without_overwriting_case_insensitive_matches() -> Result<()> {
        let case = tempfile::tempdir()?;
        let source = case.path().join("source");
        let target = case.path().join("target");
        fs::create_dir_all(source.join("相册"))?;
        fs::create_dir_all(target.join("相册"))?;
        fs::write(source.join("照片.png"), b"photo")?;
        fs::write(source.join("相册").join("note.txt"), b"note")?;
        fs::write(target.join("照片.PNG"), b"existing")?;
        let saved = save_as(&[source.join("照片.png"), source.join("相册")], &target)?;
        assert_eq!(
            saved,
            [target.join("照片 (1).png"), target.join("相册 (1)")]
        );
        assert_eq!(fs::read(target.join("照片.PNG"))?, b"existing");
        assert_eq!(fs::read(saved[1].join("note.txt"))?, b"note");
        assert_eq!(fs::read(source.join("照片.png"))?, b"photo");
        assert!(save_as(std::slice::from_ref(&source), &source.join("相册")).is_err());
        Ok(())
    }

    #[test]
    fn move_preserves_earlier_success_when_a_later_item_is_locked() -> Result<()> {
        use std::os::windows::fs::OpenOptionsExt;

        let case = tempfile::tempdir()?;
        let source = case.path().join("first.jpg");
        let locked = case.path().join("locked.jpg");
        let target = case.path().join("target");
        fs::create_dir(&target)?;
        fs::write(&source, b"first")?;
        fs::write(&locked, b"second")?;
        fs::write(target.join("first.jpg"), b"existing")?;
        let _guard = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&locked)?;
        let moved = move_to(&[source.clone(), locked.clone()], &target)?;
        assert_eq!(moved.paths, [target.join("first (1).jpg"), locked.clone()]);
        assert_eq!(moved.warnings.len(), 1);
        assert!(!source.exists());
        assert_eq!(fs::read(locked)?, b"second");
        Ok(())
    }

    #[test]
    fn staged_cross_volume_path_moves_directory_and_never_replaces_existing_tree() -> Result<()> {
        let case = tempfile::tempdir()?;
        let source = case.path().join("album");
        let target = case.path().join("target");
        fs::create_dir_all(&source)?;
        fs::create_dir_all(target.join("album"))?;
        fs::write(source.join("photo.jpg"), b"photo")?;
        let (moved, warning) = move_cross_volume(&source, case.path(), &target, true)?;
        assert_eq!(moved, target.join("album (1)"));
        assert!(warning.is_none());
        assert!(!source.exists());
        assert!(target.join("album").is_dir());
        assert_eq!(fs::read(moved.join("photo.jpg"))?, b"photo");
        Ok(())
    }

    #[test]
    fn clipboard_payload_keeps_the_entire_unicode_list() -> Result<()> {
        let first = PathBuf::from(r"C:接收照片.jpg");
        let second = PathBuf::from(r"C:接收说明.txt");
        let bytes = dropfiles_bytes(&[first.clone(), second.clone()])?;
        let offset = size_of::<DROPFILES>();
        assert_eq!(&bytes[..4], &(offset as u32).to_le_bytes());
        let wide = offset_of!(DROPFILES, fWide);
        assert_eq!(&bytes[wide..wide + 4], &1u32.to_le_bytes());
        let encoded: Vec<u16> = bytes[offset..]
            .as_chunks::<2>()
            .0
            .iter()
            .copied()
            .map(u16::from_le_bytes)
            .collect();
        let expected = first
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .chain(second.as_os_str().encode_wide())
            .chain([0, 0])
            .collect::<Vec<_>>();
        assert_eq!(encoded, expected);
        Ok(())
    }
}
