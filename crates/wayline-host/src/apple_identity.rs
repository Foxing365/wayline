use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use wayline_core::AppleSenderIdentity;
use windows::{
    Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    },
    core::PCWSTR,
};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8; 8] = b"WAAPPLE1";
const HEADER_LEN: usize = MAGIC.len() + 8;
const CERTIFICATE_LIMIT: usize = 96 * 1024;
const KEY_LIMIT: usize = 64 * 1024;
const RECORD_LIMIT: usize = 256 * 1024;
const PLAINTEXT_LIMIT: usize = HEADER_LEN + CERTIFICATE_LIMIT + KEY_LIMIT + RECORD_LIMIT;
const PROTECTED_LIMIT: usize = PLAINTEXT_LIMIT + 16 * 1024;

fn identity_path() -> Result<PathBuf> {
    Ok(super::identity_dir()?.join("apple-airdrop.dpapi"))
}

pub fn has_apple_airdrop_identity() -> Result<bool> {
    Ok(identity_path()?.try_exists()?)
}

pub fn import_apple_airdrop_identity(directory: &Path) -> Result<()> {
    let certificate = read_required(&directory.join("certificate.pem"), CERTIFICATE_LIMIT)
        .context("certificate.pem")?;
    let key = read_required(&directory.join("key.pem"), KEY_LIMIT).context("key.pem")?;
    let record = read_required(&directory.join("validation_record.cms"), RECORD_LIMIT)
        .context("validation_record.cms")?;
    AppleSenderIdentity::from_files(&certificate, &key, &record)?;

    let mut plaintext = Zeroizing::new(Vec::with_capacity(
        HEADER_LEN + certificate.len() + key.len() + record.len(),
    ));
    plaintext.extend_from_slice(MAGIC);
    plaintext.extend_from_slice(&(certificate.len() as u32).to_le_bytes());
    plaintext.extend_from_slice(&(key.len() as u32).to_le_bytes());
    plaintext.extend_from_slice(&certificate);
    plaintext.extend_from_slice(&key);
    plaintext.extend_from_slice(&record);
    let protected = dpapi(&plaintext, true)?;

    let target = identity_path()?;
    let _guard = super::FileGuard::acquire(&target.with_extension("dpapi.lock"))?;
    let directory = target.parent().context("Invalid identity directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(&protected)?;
    temporary.as_file().sync_all()?;
    temporary.persist(&target)?;
    Ok(())
}

pub fn load_apple_airdrop_identity() -> Result<Option<AppleSenderIdentity>> {
    let Some(protected) = read_limited(&identity_path()?, PROTECTED_LIMIT)? else {
        return Ok(None);
    };
    let plaintext = dpapi(&protected, false)?;
    let (certificate, key, record) = credential_parts(&plaintext)?;
    Ok(Some(AppleSenderIdentity::from_files(
        certificate,
        key,
        record,
    )?))
}

pub fn remove_apple_airdrop_identity() -> Result<()> {
    let target = identity_path()?;
    let _guard = super::FileGuard::acquire(&target.with_extension("dpapi.lock"))?;
    match fs::remove_file(target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn credential_parts(data: &[u8]) -> Result<(&[u8], &[u8], &[u8])> {
    ensure!(
        data.len() >= HEADER_LEN && data.starts_with(MAGIC),
        "Invalid Apple AirDrop identity format"
    );
    let certificate_len = u32::from_le_bytes(data[8..12].try_into()?) as usize;
    let key_len = u32::from_le_bytes(data[12..16].try_into()?) as usize;
    ensure!(
        (1..=CERTIFICATE_LIMIT).contains(&certificate_len) && (1..=KEY_LIMIT).contains(&key_len),
        "Invalid Apple AirDrop credential size"
    );
    let (certificate, rest) = data[HEADER_LEN..]
        .split_at_checked(certificate_len)
        .context("Missing Apple AirDrop certificate")?;
    let (key, record) = rest
        .split_at_checked(key_len)
        .context("Missing Apple AirDrop key")?;
    ensure!(
        (1..=RECORD_LIMIT).contains(&record.len()),
        "Invalid Apple AirDrop validation record size"
    );
    Ok((certificate, key, record))
}

fn read_required(path: &Path, max: usize) -> Result<Zeroizing<Vec<u8>>> {
    read_limited(path, max)?.context("Required identity file does not exist")
}

fn read_limited(path: &Path, max: usize) -> Result<Option<Zeroizing<Vec<u8>>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= max as u64,
        "Invalid Apple AirDrop identity file size"
    );
    let mut data = Zeroizing::new(Vec::with_capacity(max + 1));
    file.take(max as u64 + 1).read_to_end(&mut data)?;
    ensure!(
        !data.is_empty() && data.len() <= max,
        "Invalid Apple AirDrop identity file size"
    );
    Ok(Some(data))
}

fn dpapi(input: &[u8], protect: bool) -> Result<Zeroizing<Vec<u8>>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: input.len().try_into()?,
        pbData: input.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: DPAPI borrows the input and writes a LocalFree-owned buffer to output.
    // Both calls are synchronous and the input slice outlives the FFI call.
    unsafe {
        if protect {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )?;
        } else {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )?;
        }
        let len = output.cbData as usize;
        let limit = if protect {
            PROTECTED_LIMIT
        } else {
            PLAINTEXT_LIMIT
        };
        if output.pbData.is_null() {
            anyhow::bail!("Windows returned no AirDrop identity data");
        }
        let bytes = std::slice::from_raw_parts_mut(output.pbData, len);
        let copied = (len != 0 && len <= limit).then(|| Zeroizing::new(bytes.to_vec()));
        if !protect {
            bytes.zeroize();
        }
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        copied.context("Windows returned invalid AirDrop identity data")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_preserves_complete_credentials_and_rejects_corruption() {
        let sample = [MAGIC.as_slice(), &[2, 0, 0, 0, 3, 0, 0, 0], b"c1k23record"].concat();
        let mut encrypted = dpapi(&sample, true).unwrap();
        assert_ne!(&*encrypted, &sample);
        let decrypted = dpapi(&encrypted, false).unwrap();
        assert_eq!(
            credential_parts(&decrypted).unwrap(),
            (&b"c1"[..], &b"k23"[..], &b"record"[..])
        );
        let mid = encrypted.len() / 2;
        encrypted[mid] ^= 1;
        assert!(dpapi(&encrypted, false).is_err());
        assert!(credential_parts(&sample[..HEADER_LEN + 4]).is_err());
    }
}
