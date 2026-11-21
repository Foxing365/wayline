//! Platform identity conversion and the upstream TLS configuration.
use std::{io, sync::Arc, time::SystemTime};

#[cfg(windows)]
mod apple;
#[cfg(windows)]
mod apple_crypto;
pub type TlsError = io::Error;
pub(crate) fn install_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}
pub(crate) fn upstream_identity(identity: &crate::Identity) -> owly_airdrop::driver::tls::Identity {
    owly_airdrop::driver::tls::Identity::from_der(
        identity.certificate_der.clone(),
        identity.private_key_der.clone(),
    )
}
pub fn server_config(identity: &crate::Identity) -> io::Result<Arc<rustls::ServerConfig>> {
    install_provider();
    owly_airdrop::driver::tls::server_config(&upstream_identity(identity))
}

/// A user-supplied Apple AirDrop identity. Its record, key and contact hashes
/// are private and intentionally unavailable through Debug or serialization.
pub struct AppleSenderIdentity {
    config: Arc<rustls::ClientConfig>,
    record: Vec<u8>,
    expires: SystemTime,
    #[cfg(windows)]
    contact_prefixes: [[u8; 2]; 3],
}

impl AppleSenderIdentity {
    pub fn from_files(
        certificate_pem: &[u8],
        key_pem: &[u8],
        record_der: &[u8],
    ) -> io::Result<Self> {
        #[cfg(windows)]
        {
            apple::import(certificate_pem, key_pem, record_der)
        }
        #[cfg(not(windows))]
        {
            let _ = (certificate_pem, key_pem, record_der);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Apple AirDrop identities require Windows",
            ))
        }
    }

    pub(crate) fn client_config(&self) -> io::Result<Arc<rustls::ClientConfig>> {
        if SystemTime::now() >= self.expires {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the imported Apple validation record has expired",
            ));
        }
        Ok(self.config.clone())
    }

    pub(crate) fn record(&self) -> &[u8] {
        &self.record
    }

    pub(crate) fn pinned_client_config(
        &self,
        receiver_certificate_sha256: [u8; 32],
    ) -> io::Result<Arc<rustls::ClientConfig>> {
        #[cfg(windows)]
        {
            let config = self.client_config()?;
            Ok(apple::pin_receiver(&config, receiver_certificate_sha256))
        }
        #[cfg(not(windows))]
        {
            let _ = receiver_certificate_sha256;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Apple AirDrop identities require Windows",
            ))
        }
    }

    #[cfg(windows)]
    pub(crate) fn contact_prefixes(&self) -> [[u8; 2]; 3] {
        self.contact_prefixes
    }
}

/// Selects an Apple identity for one outbound session. Without it the sender
/// uses anonymous Apple BLE fields and the ordinary Everyone-mode connection.
#[derive(Clone, Copy)]
pub struct SenderIdentity<'a> {
    pub local: &'a crate::Identity,
    pub apple: Option<&'a AppleSenderIdentity>,
}

impl<'a> From<&'a crate::Identity> for SenderIdentity<'a> {
    fn from(local: &'a crate::Identity) -> Self {
        Self { local, apple: None }
    }
}
