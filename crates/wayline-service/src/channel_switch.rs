//! RTL8821CU channel requests and confirmations in a service-owned per-child directory.
use std::{
    ffi::OsString,
    fs, io,
    io::Write,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
    },
    path::{Path, PathBuf},
    ptr,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{CreateDirectoryW, GetVolumeInformationW, FILE_ATTRIBUTE_REPARSE_POINT},
    System::{SystemInformation::GetSystemDirectoryW, SystemServices::FILE_PERSISTENT_ACLS},
};

pub const CONTROL_DIR_ENV: &str = "WAYLINE_BRIDGE_CHANNEL_DIR";
pub const REQUESTED: &str = "requested";
pub const READY: &str = "ready";

pub(crate) struct PrivateDirectory(PathBuf);

impl PrivateDirectory {
    pub(crate) fn create() -> io::Result<Self> {
        let mut system = [0_u16; 260];
        // SAFETY: system is writable, and the size is in UTF-16 units.
        let len = unsafe { GetSystemDirectoryW(system.as_mut_ptr(), system.len() as u32) } as usize;
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        if len >= system.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Windows system directory is too long",
            ));
        }
        // Root this in the OS's protected LocalSystem profile instead of the
        // configurable service data path, whose parent could permit deletion
        // or replacement of even an ACL-protected child (FILE_DELETE_CHILD).
        let parent = PathBuf::from(OsString::from_wide(&system[..len]))
            .join("config")
            .join("systemprofile");
        for path in parent.ancestors() {
            let attr = fs::symlink_metadata(path)?;
            if !attr.is_dir() || attr.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "RTL8821CU control directory cannot use a reparse point",
                ));
            }
        }
        let volume = parent.ancestors().last().expect("Windows path has a root");
        let wide: Vec<u16> = volume.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut flags = 0;
        // SAFETY: volume path is terminated, flags is writable; other outputs are optional.
        if unsafe {
            GetVolumeInformationW(
                wide.as_ptr(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut flags,
                ptr::null_mut(),
                0,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if flags & FILE_PERSISTENT_ACLS == 0 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "RTL8821CU channel control requires a filesystem with persistent ACLs",
            ));
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = parent.join(format!(
            "wayline-channel-{}-{now}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        create_directory(&path)?;
        Ok(Self(path))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        for name in [REQUESTED, READY] {
            let _ = fs::remove_file(self.0.join(name));
        }
        let _ = fs::remove_dir(&self.0);
    }
}

fn create_directory(path: &Path) -> io::Result<()> {
    // Protect the DACL from its parent and pass the same limited ACL to new
    // request/ready records. OW grants access to the creator/owner.
    let sddl: Vec<u16> = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;OW)"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: SDDL is terminated; descriptor is valid output storage.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: path and attributes (including descriptor) are valid during creation.
    let result = if unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    };
    // SAFETY: the descriptor was allocated by the SDDL conversion API.
    unsafe { LocalFree(descriptor) };
    // Never adopt a pre-existing directory or overwrite its security.
    result
}

pub fn read(path: &Path) -> io::Result<Option<u8>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    match bytes.as_slice() {
        [channel @ (6 | 44 | 149)] => Ok(Some(*channel)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid RTL8821CU channel record",
        )),
    }
}

pub fn publish(path: &Path, channel: u8) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "channel record needs a directory",
        )
    })?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(&[channel])?;
    temp.persist(path).map_err(|failure| failure.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Security::{
        Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
            SE_FILE_OBJECT,
        },
        DACL_SECURITY_INFORMATION,
    };

    #[test]
    fn channel_directory_gets_only_the_protected_service_acl() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("private-channel");
        create_directory(&path).unwrap();
        assert_eq!(
            create_directory(&path).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: wide is terminated; descriptor is writable output storage.
        let status = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(status, 0);
        let mut sddl = ptr::null_mut();
        let mut len = 0;
        // SAFETY: descriptor is an owned, valid pointer; outputs are writable.
        let converted = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut sddl,
                &mut len,
            )
        };
        // SAFETY: the security-info API allocated descriptor.
        unsafe { LocalFree(descriptor) };
        assert_ne!(converted, 0);
        // SAFETY: the conversion returned len UTF-16 units backed by sddl.
        let acl =
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(sddl, len as usize) });
        // SAFETY: the SDDL conversion API allocated sddl.
        unsafe { LocalFree(sddl.cast()) };
        assert!(acl.starts_with("D:P"), "{acl}");
        assert_eq!(acl.matches('(').count(), 3, "{acl}");
        for sid in [";;;SY)", ";;;BA)", ";;;OW)"] {
            assert!(acl.contains(sid), "{acl}");
        }
    }
}
