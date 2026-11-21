//! Windows certificate and CMS validation for explicitly imported Apple identities.
use rustls::pki_types::{pem::PemObject, CertificateDer};
use std::{ffi::CStr, io, mem::size_of, ptr};
use windows_sys::Win32::Security::Cryptography::{
    CertAddEncodedCertificateToStore, CertCloseStore, CertCreateCertificateChainEngine,
    CertCreateCertificateContext, CertFindExtension, CertFreeCertificateChain,
    CertFreeCertificateChainEngine, CertFreeCertificateContext, CertGetCertificateChain,
    CertGetNameStringW, CertOpenStore, CryptGetMessageSignerCount, CryptVerifyMessageSignature,
    CERT_CHAIN_CACHE_ONLY_URL_RETRIEVAL, CERT_CHAIN_DISABLE_AUTH_ROOT_AUTO_UPDATE,
    CERT_CHAIN_ENGINE_CONFIG, CERT_CHAIN_PARA, CERT_CONTEXT, CERT_NAME_ATTR_TYPE,
    CERT_STORE_ADD_ALWAYS, CERT_STORE_PROV_MEMORY, CERT_TRUST_NO_ERROR, CRYPT_VERIFY_MESSAGE_PARA,
    HCERTCHAINENGINE, HCERTSTORE, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
};

// https://www.apple.com/certificateauthority/ (root fingerprints: Apple Support 105116).
const ROOTS: &[u8] = include_bytes!("../../assets/apple-pki-roots.pem");
// Apple Application Integration Certification Practice Statement §4.12.1 and §4.12.5.
const SHARING: &CStr = c"1.2.840.113635.100.4.7";
const RECORD_SIGNING: &CStr = c"1.2.840.113635.100.6.25";

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

struct Certificate(*mut CERT_CONTEXT);

impl Certificate {
    fn from_der(der: &CertificateDer<'_>) -> io::Result<Self> {
        // SAFETY: the encoded bytes are readable for the supplied length; Windows copies them.
        let context = unsafe {
            CertCreateCertificateContext(X509_ASN_ENCODING, der.as_ptr(), der.len() as u32)
        };
        if context.is_null() {
            return Err(invalid("invalid Apple AirDrop certificate"));
        }
        Ok(Self(context))
    }

    fn has_extension(&self, oid: &CStr) -> bool {
        // SAFETY: a live context owns its CERT_INFO, extensions and encoded data.
        unsafe {
            let cert = &*(*self.0).pCertInfo;
            !CertFindExtension(oid.as_ptr().cast(), cert.cExtension, cert.rgExtension).is_null()
        }
    }

    fn common_name(&self) -> io::Result<String> {
        let oid = c"2.5.4.3";
        let mut name = [0_u16; 256];
        // SAFETY: the output buffer is valid and the OID has a NUL terminator.
        let len = unsafe {
            CertGetNameStringW(
                self.0,
                CERT_NAME_ATTR_TYPE,
                0,
                oid.as_ptr().cast(),
                name.as_mut_ptr(),
                name.len() as u32,
            )
        } as usize;
        if len <= 1 || len >= name.len() {
            return Err(invalid(
                "Apple AirDrop certificate has no usable account identifier",
            ));
        }
        String::from_utf16(&name[..len - 1])
            .map_err(|_| invalid("invalid Apple account identifier"))
    }
}

impl Drop for Certificate {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns the certificate context.
        unsafe { CertFreeCertificateContext(self.0) };
    }
}

struct Store(HCERTSTORE);

impl Store {
    fn new() -> io::Result<Self> {
        // SAFETY: the memory-store provider accepts null initialization parameters.
        let store = unsafe { CertOpenStore(CERT_STORE_PROV_MEMORY, 0, 0, 0, ptr::null()) };
        if store.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(store))
    }

    fn add(&self, certificate: &CertificateDer<'_>) -> io::Result<()> {
        // SAFETY: the store is live and Windows copies the supplied certificate bytes.
        if unsafe {
            CertAddEncodedCertificateToStore(
                self.0,
                X509_ASN_ENCODING,
                certificate.as_ptr(),
                certificate.len() as u32,
                CERT_STORE_ADD_ALWAYS,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns the open memory store.
        unsafe { CertCloseStore(self.0, 0) };
    }
}

struct ChainEngine(HCERTCHAINENGINE);

impl Drop for ChainEngine {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns the chain engine.
        unsafe { CertFreeCertificateChainEngine(self.0) };
    }
}

fn verify_chain(certificate: &Certificate, additional: HCERTSTORE) -> io::Result<()> {
    let roots = Store::new()?;
    for root in CertificateDer::pem_slice_iter(ROOTS) {
        roots.add(&root.map_err(io::Error::other)?)?;
    }
    let config = CERT_CHAIN_ENGINE_CONFIG {
        cbSize: size_of::<CERT_CHAIN_ENGINE_CONFIG>() as u32,
        hExclusiveRoot: roots.0,
        ..Default::default()
    };
    let mut engine = ptr::null_mut();
    // SAFETY: the config and root store live through the engine's lifetime.
    if unsafe { CertCreateCertificateChainEngine(&config, &mut engine) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let engine = ChainEngine(engine);
    let mut chain = ptr::null_mut();
    let params = CERT_CHAIN_PARA {
        cbSize: size_of::<CERT_CHAIN_PARA>() as u32,
        ..Default::default()
    };
    // SAFETY: the engine and certificate are live, and chain is valid output storage.
    if unsafe {
        CertGetCertificateChain(
            engine.0,
            certificate.0,
            ptr::null(),
            additional,
            &params,
            CERT_CHAIN_CACHE_ONLY_URL_RETRIEVAL | CERT_CHAIN_DISABLE_AUTH_ROOT_AUTO_UPDATE,
            ptr::null(),
            &mut chain,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call produced an owned chain, freed after reading its status.
    let error = unsafe {
        let error = (*chain).TrustStatus.dwErrorStatus;
        CertFreeCertificateChain(chain);
        error
    };
    if error != CERT_TRUST_NO_ERROR {
        return Err(invalid(
            "Apple AirDrop certificate chain is untrusted, expired or incomplete",
        ));
    }
    Ok(())
}

pub(super) fn sharing_certificate(
    leaf: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
) -> io::Result<String> {
    let certificate = Certificate::from_der(leaf)?;
    if !certificate.has_extension(SHARING) {
        return Err(invalid(
            "certificate is not an Apple ID sharing certificate",
        ));
    }
    let store = Store::new()?;
    for cert in intermediates {
        store.add(cert)?;
    }
    verify_chain(&certificate, store.0)?;
    certificate.common_name()
}

pub(super) fn verify_record(record: &[u8]) -> io::Result<Vec<u8>> {
    let encoding = X509_ASN_ENCODING | PKCS_7_ASN_ENCODING;
    let params = CRYPT_VERIFY_MESSAGE_PARA {
        cbSize: size_of::<CRYPT_VERIFY_MESSAGE_PARA>() as u32,
        dwMsgAndCertEncodingType: encoding,
        ..Default::default()
    };
    let len = record.len() as u32;
    // SAFETY: the signed input buffer is valid for its length.
    if unsafe { CryptGetMessageSignerCount(encoding, record.as_ptr(), len) } != 1 {
        return Err(invalid(
            "Apple validation record must contain one CMS signer",
        ));
    }
    let mut content = vec![0; record.len()];
    let mut content_len = len;
    let mut signer = ptr::null_mut();
    // SAFETY: all buffers and pointers remain live during the call.
    let verified = unsafe {
        CryptVerifyMessageSignature(
            &params,
            0,
            record.as_ptr(),
            len,
            content.as_mut_ptr(),
            &mut content_len,
            &mut signer,
        )
    };
    let signer = (!signer.is_null()).then(|| Certificate(signer));
    if verified == 0 {
        return Err(invalid(
            "Apple validation record has an invalid CMS signature",
        ));
    }
    let signer =
        signer.ok_or_else(|| invalid("Apple validation record has no signer certificate"))?;
    if !signer.has_extension(RECORD_SIGNING) {
        return Err(invalid(
            "CMS certificate is not an Apple validation record signer",
        ));
    }
    // SAFETY: the signer context owns its store while the chain is checked.
    verify_chain(&signer, unsafe { (*signer.0).hCertStore })?;
    content.truncate(content_len as usize);
    Ok(content)
}
