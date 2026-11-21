//! The extraction sandbox: a fresh staging directory that an upload is
//! written into, moved into the destination only once it's complete (SEC-1),
//! without overwriting anything there (SEC-2).
//!
//! Paths come from the extraction rules already validated, and only this
//! code creates anything in staging, so no entry in it can be a symlink or
//! lead outside it.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsString;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::proto::archive::extract::{RelPath, collision_name};

pub struct Staging {
    dir: tempfile::TempDir,
    dest: PathBuf,
    file: Option<fs::File>,
    /// An exact logical child name is relative to its parent directory node.
    entries: HashMap<(usize, String), StagedEntry>,
    next_directory: usize,
    /// Items to publish, in the order they arrived.
    top: Vec<Publication>,
}

struct Publication {
    source: PathBuf,
    name: String,
}

enum StagedEntry {
    Directory {
        id: usize,
        basename: OsString,
        declared: bool,
    },
    File,
}

impl Staging {
    /// A staging directory inside `dest`, so moving out of it is a rename.
    pub async fn new(dest: &Path) -> io::Result<Self> {
        fs::create_dir_all(dest).await?;
        let dest = dest.to_path_buf();
        let dir = tokio::task::spawn_blocking({
            let dest = dest.clone();
            move || {
                tempfile::Builder::new()
                    .prefix(".owly-staging-")
                    .tempdir_in(dest)
            }
        })
        .await
        .map_err(io::Error::other)??;
        Ok(Staging {
            dir,
            dest,
            file: None,
            entries: HashMap::new(),
            next_directory: 1,
            top: Vec::new(),
        })
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    async fn directories(
        &mut self,
        components: &[String],
        declared: bool,
    ) -> io::Result<(usize, PathBuf)> {
        let mut parent_id = 0;
        let mut parent = self.dir.path().to_path_buf();
        for (index, name) in components.iter().enumerate() {
            let key = (parent_id, name.clone());
            let explicit = declared && index + 1 == components.len();
            if let Some(entry) = self.entries.get_mut(&key) {
                let StagedEntry::Directory {
                    id,
                    basename,
                    declared,
                } = entry
                else {
                    return Err(staging_path_collision(io::ErrorKind::NotADirectory));
                };
                if explicit && std::mem::replace(declared, true) {
                    return Err(staging_path_collision(io::ErrorKind::AlreadyExists));
                }
                parent.push(basename.as_os_str());
                parent_id = *id;
                continue;
            }

            let (path, ()) = create_available(&parent, name, fs::create_dir).await?;
            if index == 0 {
                self.top.push(Publication {
                    source: path.clone(),
                    name: name.clone(),
                });
            }
            let basename = path
                .file_name()
                .expect("reserved path has a name")
                .to_owned();
            let id = self.next_directory;
            self.next_directory += 1;
            self.entries.insert(
                key,
                StagedEntry::Directory {
                    id,
                    basename,
                    declared: explicit,
                },
            );
            parent_id = id;
            parent = path;
        }
        Ok((parent_id, parent))
    }

    pub async fn create_dir(&mut self, path: &RelPath) -> io::Result<()> {
        self.directories(path.components(), true).await.map(|_| ())
    }

    pub async fn create_file(&mut self, path: &RelPath) -> io::Result<()> {
        let components = path.components();
        let (name, parents) = components.split_last().expect("validated path has a name");
        let (parent_id, parent) = self.directories(parents, false).await?;
        let key = (parent_id, name.clone());
        if self.entries.contains_key(&key) {
            return Err(staging_path_collision(io::ErrorKind::AlreadyExists));
        }
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o644);
        let (source, file) = create_available(&parent, name, |path| options.open(path)).await?;
        if parents.is_empty() {
            self.top.push(Publication {
                source,
                name: name.clone(),
            });
        }
        self.entries.insert(key, StagedEntry::File);
        self.file = Some(file);
        Ok(())
    }

    pub async fn create_wrapped_file(&mut self, name: &RelPath) -> io::Result<()> {
        // Keep the published basename separate from the staging path, so
        // MoveFileExW/renameat2 can choose one suffix for all collisions.
        let directory = self.dir.path().to_path_buf();
        let (file, source) = tokio::task::spawn_blocking(move || {
            tempfile::Builder::new()
                .prefix(".owly-wrapped-")
                .tempfile_in(directory)?
                .keep()
                .map_err(|error| error.error)
        })
        .await
        .map_err(io::Error::other)??;
        self.file = Some(fs::File::from_std(file));
        self.top.push(Publication {
            source,
            name: name.last().to_owned(),
        });
        Ok(())
    }

    pub async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.write_all(data).await,
            None => Err(io::Error::other("no file open")),
        }
    }

    pub async fn end_file(&mut self) -> io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush().await?;
        }
        Ok(())
    }

    /// Moves everything into the destination, renaming on collision, and
    /// returns where each top-level item ended up. Staging is removed.
    pub async fn finish(mut self) -> io::Result<Vec<PathBuf>> {
        self.end_file().await?;
        let dest = self.dest.clone();
        let top = std::mem::take(&mut self.top);
        tokio::task::spawn_blocking(move || {
            let mut moved = Vec::new();
            for Publication { source, name } in top {
                match move_unique(&source, &dest, &name) {
                    Ok(path) => moved.push(path),
                    Err(error) => {
                        rollback(&moved);
                        return Err(error);
                    }
                }
            }
            Ok(moved)
        })
        .await
        .map_err(io::Error::other)?
        // `self.dir` is dropped here, removing staging and anything left in it.
    }
}

fn staging_path_collision(kind: io::ErrorKind) -> io::Error {
    io::Error::new(
        kind,
        "received archive entries collide at the same staging path",
    )
}

/// Reserve a name with an exclusive filesystem operation. The filesystem also
/// detects aliases such as case variants and Windows short names.
async fn create_available<T, F: Future<Output = io::Result<T>>>(
    parent: &Path,
    name: &str,
    mut create: impl FnMut(PathBuf) -> F,
) -> io::Result<(PathBuf, T)> {
    for index in 0..10_000 {
        let path = parent.join(available_name(name, index).as_ref());
        match create(path.clone()).await {
            Ok(entry) => return Ok((path, entry)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                // Windows can report access denied for a creation attempt at
                // an existing name of another type. Only retry an occupied name.
                if error.kind() != io::ErrorKind::PermissionDenied
                    || fs::symlink_metadata(&path).await.is_err()
                {
                    return Err(error);
                }
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free staging name",
    ))
}

fn available_name(name: &str, index: u32) -> Cow<'_, str> {
    if index == 0 {
        return Cow::Borrowed(name);
    }
    let numbered = collision_name(name, index);
    // RelPath limits any component to 255 bytes. Keep generated names within
    // that same limit while retaining the extension when it fits.
    const MAX_NAME_BYTES: usize = 255;
    let excess = numbered.len().saturating_sub(MAX_NAME_BYTES);
    if excess == 0 {
        return Cow::Owned(numbered);
    }
    let extension = name
        .rfind('.')
        .filter(|&index| index > 0)
        .unwrap_or(name.len());
    let shortened = utf8_prefix(&name[..extension], extension.saturating_sub(excess));
    if !shortened.is_empty() {
        return Cow::Owned(collision_name(
            &format!("{shortened}{}", &name[extension..]),
            index,
        ));
    }
    let suffix = format!(" ({index})");
    Cow::Owned(format!(
        "{}{suffix}",
        utf8_prefix(name, MAX_NAME_BYTES - suffix.len())
    ))
}

fn utf8_prefix(value: &str, mut length: usize) -> &str {
    while !value.is_char_boundary(length) {
        length -= 1;
    }
    &value[..length]
}

/// Renames `from` to `dest/name`, or the first free `name (n)`, never
/// replacing what's there (SEC-2).
fn move_unique(from: &Path, dest: &Path, name: &str) -> io::Result<PathBuf> {
    for n in 0..10_000 {
        let to = dest.join(available_name(name, n).as_ref());
        match rename_noreplace(from, &to) {
            Ok(()) => return Ok(to),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e)
                if e.kind() == io::ErrorKind::PermissionDenied
                    && std::fs::symlink_metadata(&to).is_ok() =>
            {
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name"))
}

#[cfg(target_os = "linux")]
fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    use nix::fcntl::{AT_FDCWD, RenameFlags, renameat2};
    renameat2(AT_FDCWD, from, AT_FDCWD, to, RenameFlags::RENAME_NOREPLACE).map_err(io::Error::from)
}

/// Without an atomic no-replace rename, check first. The race is with the
/// destination's owner, not with the sender.
#[cfg(not(any(target_os = "linux", windows)))]
fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    std::fs::rename(from, to)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: both buffers are terminated and live through this synchronous call.
    // Omitting REPLACE_EXISTING makes collision handling atomic, including races.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Roll back only paths returned by this staging transaction.
pub fn rollback(paths: &[PathBuf]) {
    for path in paths.iter().rev() {
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(p: &str) -> RelPath {
        RelPath::parse(p).unwrap()
    }

    #[tokio::test]
    async fn directory_collisions_preserve_existing_contents() {
        let dest = tempfile::tempdir().unwrap();
        std::fs::create_dir(dest.path().join("same")).unwrap();
        std::fs::write(dest.path().join("same/user.txt"), b"original").unwrap();
        let mut s = Staging::new(dest.path()).await.unwrap();
        s.create_dir(&path("same")).await.unwrap();
        s.create_file(&path("same/new.txt")).await.unwrap();
        s.write(b"new").await.unwrap();
        let paths = s.finish().await.unwrap();
        assert_eq!(paths, [dest.path().join("same (1)")]);
        assert_eq!(
            std::fs::read(dest.path().join("same/user.txt")).unwrap(),
            b"original"
        );
        assert_eq!(
            std::fs::read(dest.path().join("same (1)/new.txt")).unwrap(),
            b"new"
        );
    }

    // spec: SEC-1, SEC-2
    #[tokio::test]
    async fn moves_into_place_without_overwriting() {
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(dest.path().join("a.txt"), b"mine").unwrap();
        let mut s = Staging::new(dest.path()).await.unwrap();
        let staging = s.path().to_path_buf();
        assert!(staging.starts_with(dest.path()));
        s.create_file(&path("a.txt")).await.unwrap();
        s.write(b"theirs").await.unwrap();
        s.create_dir(&path("d")).await.unwrap();
        s.create_file(&path("d/e/f")).await.unwrap();
        s.write(b"deep").await.unwrap();
        let moved = s.finish().await.unwrap();
        assert_eq!(
            moved,
            [dest.path().join("a (1).txt"), dest.path().join("d")]
        );
        assert_eq!(std::fs::read(dest.path().join("a.txt")).unwrap(), b"mine");
        assert_eq!(
            std::fs::read(dest.path().join("a (1).txt")).unwrap(),
            b"theirs"
        );
        assert_eq!(std::fs::read(dest.path().join("d/e/f")).unwrap(), b"deep");
        assert!(!staging.exists());
    }

    #[tokio::test]
    async fn dropping_discards_everything() {
        let dest = tempfile::tempdir().unwrap();
        let mut s = Staging::new(dest.path()).await.unwrap();
        s.create_file(&path("x")).await.unwrap();
        s.write(b"partial").await.unwrap();
        drop(s);
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn never_replaces_a_file_in_staging() {
        let dest = tempfile::tempdir().unwrap();
        let mut s = Staging::new(dest.path()).await.unwrap();
        s.create_file(&path("x")).await.unwrap();
        let error = s.create_file(&path("x")).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            error.to_string(),
            "received archive entries collide at the same staging path"
        );
    }

    #[tokio::test]
    async fn a_late_directory_header_reuses_its_parent_and_rejects_type_conflicts() {
        let dest = tempfile::tempdir().unwrap();
        let mut stage = Staging::new(dest.path()).await.unwrap();
        stage.create_file(&path("folder/child/data")).await.unwrap();
        stage.write(b"content").await.unwrap();
        stage.end_file().await.unwrap();
        stage.create_dir(&path("folder/child")).await.unwrap();
        stage.create_dir(&path("folder")).await.unwrap();
        assert_eq!(
            stage.create_dir(&path("folder")).await.unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            stage
                .create_file(&path("folder/child"))
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            stage
                .create_dir(&path("folder/child/data"))
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotADirectory
        );
        assert_eq!(
            stage
                .create_file(&path("folder/child/data/nested"))
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotADirectory
        );
        let published = stage.finish().await.unwrap();
        assert_eq!(published, [dest.path().join("folder")]);
        assert_eq!(
            std::fs::read(published[0].join("child/data")).unwrap(),
            b"content"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn case_variants_keep_distinct_nested_files_and_directories() {
        let dest = tempfile::tempdir().unwrap();
        let mut stage = Staging::new(dest.path()).await.unwrap();
        stage
            .create_file(&path("album/Upper/Photo.JPG"))
            .await
            .unwrap();
        stage.write(b"first image").await.unwrap();
        stage.end_file().await.unwrap();
        let aliases = stage.path().join("album/upper").is_dir();

        for (entry, contents) in [
            ("album/upper/Photo.JPG", b"different directory".as_slice()),
            ("album/Upper/photo.jpg", b"different image".as_slice()),
            ("album/Name", b"ordinary file".as_slice()),
            ("album/name/deep", b"ordinary directory".as_slice()),
        ] {
            stage.create_file(&path(entry)).await.unwrap();
            stage.write(contents).await.unwrap();
            stage.end_file().await.unwrap();
        }
        stage.create_dir(&path("album/Upper")).await.unwrap();
        stage.create_dir(&path("album/upper")).await.unwrap();
        stage.create_dir(&path("album/name")).await.unwrap();
        let published = stage.finish().await.unwrap();
        assert_eq!(published, [dest.path().join("album")]);
        let upper_variant = if aliases { "upper (1)" } else { "upper" };
        let photo_variant = if aliases {
            "photo (1).jpg"
        } else {
            "photo.jpg"
        };
        let name_variant = if aliases { "name (1)" } else { "name" };
        let album = &published[0];
        assert_eq!(
            std::fs::read(album.join("Upper/Photo.JPG")).unwrap(),
            b"first image"
        );
        assert_eq!(
            std::fs::read(album.join(upper_variant).join("Photo.JPG")).unwrap(),
            b"different directory"
        );
        assert_eq!(
            std::fs::read(album.join("Upper").join(photo_variant)).unwrap(),
            b"different image"
        );
        assert_eq!(std::fs::read(album.join("Name")).unwrap(), b"ordinary file");
        assert_eq!(
            std::fs::read(album.join(name_variant).join("deep")).unwrap(),
            b"ordinary directory"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn aliased_top_level_sources_publish_under_their_requested_names() {
        let dest = tempfile::tempdir().unwrap();
        let original = dest.path().join("ALBUM");
        std::fs::create_dir(&original).unwrap();
        std::fs::write(original.join("original"), b"untouched").unwrap();
        let aliases = dest.path().join("Album").is_dir();
        let mut stage = Staging::new(dest.path()).await.unwrap();
        for (entry, data) in [
            ("Album/one", b"first".as_slice()),
            ("album/two", b"second".as_slice()),
        ] {
            stage.create_file(&path(entry)).await.unwrap();
            stage.write(data).await.unwrap();
            stage.end_file().await.unwrap();
        }
        stage.create_wrapped_file(&path("ALBUM")).await.unwrap();
        stage.write(b"third").await.unwrap();
        let names = if aliases {
            ["Album (1)", "album (2)", "ALBUM (3)"]
        } else {
            ["Album", "album", "ALBUM (1)"]
        };
        let published = stage.finish().await.unwrap();
        assert_eq!(published, names.map(|name| dest.path().join(name)));
        assert_eq!(std::fs::read(published[0].join("one")).unwrap(), b"first");
        assert_eq!(std::fs::read(published[1].join("two")).unwrap(), b"second");
        assert_eq!(std::fs::read(&published[2]).unwrap(), b"third");
        assert_eq!(
            std::fs::read(original.join("original")).unwrap(),
            b"untouched"
        );
    }

    #[test]
    fn collision_suffix_fits_long_unicode_components_and_keeps_the_extension() {
        let name = format!("{}.jpg", "图".repeat(83));
        let candidate = available_name(&name, 123);
        assert!(candidate.ends_with(" (123).jpg"));
        assert!(RelPath::file_name(&candidate).is_ok());
    }
}
