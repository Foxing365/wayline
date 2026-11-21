use super::{apple_crypto, install_provider, AppleSenderIdentity};
use rustls::{
    client::{
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        Resumption,
    },
    crypto::{
        verify_tls12_signature, verify_tls13_signature, CryptoProvider, WebPkiSupportedAlgorithms,
    },
    pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName, UnixTime},
    sign::{CertifiedKey, SingleCertAndKey},
    CertificateError, ClientConfig, DigitallySignedStruct, Error, SignatureScheme,
};
use sha2::{Digest, Sha256};
use std::{
    io,
    sync::Arc,
    time::{Duration, SystemTime},
};

const MAX_IDENTITY_FILE: usize = 128 * 1024;
// Stute et al., USENIX Security 2019, §3.3 and Figure 3.
const APPLE_ID_COMMON_NAME_PREFIX: &str = "com.apple.idms.appleid.prd.";

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(super) fn import(
    certificate_pem: &[u8],
    key_pem: &[u8],
    record_der: &[u8],
) -> io::Result<AppleSenderIdentity> {
    if [certificate_pem, key_pem, record_der]
        .iter()
        .any(|file| file.is_empty() || file.len() > MAX_IDENTITY_FILE)
    {
        return Err(invalid(
            "Apple AirDrop identity files must be 1–131072 bytes",
        ));
    }
    let certificates = CertificateDer::pem_slice_iter(certificate_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid("invalid PEM certificate chain"))?;
    let (leaf, intermediates) = certificates
        .split_first()
        .ok_or_else(|| invalid("Apple AirDrop identity has no PEM certificate"))?;
    let common_name = apple_crypto::sharing_certificate(leaf, intermediates)?;
    let content = apple_crypto::verify_record(record_der)?;
    let record = parse_record(&content, &common_name, SystemTime::now())?;

    install_provider();
    let provider = CryptoProvider::get_default()
        .cloned()
        .ok_or_else(|| io::Error::other("no rustls crypto provider installed"))?;
    let mismatch = |_| invalid("Apple AirDrop certificate and private key do not match");
    let key = PrivateKeyDer::from_pem_slice(key_pem)
        .map_err(|_| invalid("Apple AirDrop identity needs an unencrypted PEM private key"))?;
    let credentials = CertifiedKey::from_der(certificates, key, &provider).map_err(mismatch)?;
    credentials.keys_match().map_err(mismatch)?;

    let verifier = AppleAirDropPeer {
        algorithms: provider.signature_verification_algorithms,
        leaf_sha256: None,
    };
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(owly_airdrop::driver::tls::VERSIONS)
        .map_err(io::Error::other)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_cert_resolver(Arc::new(SingleCertAndKey::from(credentials)));
    config.resumption = Resumption::disabled();
    Ok(AppleSenderIdentity {
        config: Arc::new(config),
        record: record_der.to_vec(),
        expires: record.expires,
        contact_prefixes: record.contact_prefixes,
    })
}

pub(super) fn pin_receiver(config: &ClientConfig, leaf_sha256: [u8; 32]) -> Arc<ClientConfig> {
    let mut client = config.clone();
    let verifier = AppleAirDropPeer {
        algorithms: client.crypto_provider().signature_verification_algorithms,
        leaf_sha256: Some(leaf_sha256),
    };
    client
        .dangerous()
        .set_certificate_verifier(Arc::new(verifier));
    Arc::new(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record(now: SystemTime) -> plist::Value {
        let mut record = plist::Dictionary::new();
        record.insert("encDsID".into(), "test-account".into());
        record.insert(
            "ValidAsOf".into(),
            plist::Value::Date(plist::Date::from(now)),
        );
        record.insert("SuggestValidDuration".into(), 3600_u64.into());
        record.insert(
            "ValidatedPhoneHashes".into(),
            plist::Value::Array(vec![plist::Value::Data(vec![0xab; 32])]),
        );
        record.insert(
            "ValidatedEmailHashes".into(),
            plist::Value::Array(vec!["12".repeat(32).into(), "34".repeat(32).into()]),
        );
        plist::Value::Dictionary(record)
    }

    #[test]
    fn requires_the_same_account_and_fresh_signed_record_values_before_using_hashes() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let record = sample_record(now);
        let mut xml = Vec::new();
        record.to_writer_xml(&mut xml).unwrap();
        let mut binary = Vec::new();
        record.to_writer_binary(&mut binary).unwrap();
        let common_name = "com.apple.idms.appleid.prd.test-account";
        for record in [&xml, &binary] {
            let parsed = parse_record(record, common_name, now).unwrap();
            assert_eq!(parsed.contact_prefixes, [[0xab; 2], [0x12; 2], [0x34; 2]]);
            for unmatched_name in [
                "com.apple.idms.appleid.prd.other-account",
                "com.apple.idms.appleid.prd.test-account.extra",
                "com.apple.idms.appleid.dev.test-account",
                "test-account",
            ] {
                assert!(parse_record(record, unmatched_name, now).is_err());
            }
            assert!(parse_record(record, common_name, now + Duration::from_secs(3600)).is_err());
        }
    }

    #[test]
    fn verifier_rejects_a_changed_receiver_leaf_before_the_chain_check() {
        let identity = rcgen::generate_simple_self_signed(Vec::new()).unwrap();
        let leaf = identity.cert.der();
        let algorithms =
            rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms;
        let server_name = owly_airdrop::driver::tls::server_name();
        let verifier = AppleAirDropPeer {
            algorithms,
            leaf_sha256: Some([0; 32]),
        };
        assert!(matches!(
            verifier.verify_server_cert(leaf, &[], &server_name, &[], UnixTime::now()),
            Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure
            ))
        ));
        let verifier = AppleAirDropPeer {
            algorithms,
            leaf_sha256: Some(Sha256::digest(leaf).into()),
        };
        assert!(matches!(
            verifier.verify_server_cert(leaf, &[], &server_name, &[], UnixTime::now()),
            Err(Error::InvalidCertificate(CertificateError::UnknownIssuer))
        ));
    }

    #[test]
    fn rejects_an_unsigned_record_and_untrusted_certificate() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut plist = Vec::new();
        sample_record(now).to_writer_binary(&mut plist).unwrap();
        assert!(apple_crypto::verify_record(&plist).is_err());

        let mut params = rcgen::CertificateParams::new(Vec::new()).unwrap();
        params
            .custom_extensions
            .push(rcgen::CustomExtension::from_oid_content(
                &[1, 2, 840, 113635, 100, 4, 7],
                vec![5, 0],
            ));
        let cert = params
            .self_signed(&rcgen::KeyPair::generate().unwrap())
            .unwrap();
        assert!(apple_crypto::sharing_certificate(cert.der(), &[])
            .is_err_and(|error| error.to_string().contains("chain is untrusted")));
    }
}

struct ValidationRecord {
    expires: SystemTime,
    contact_prefixes: [[u8; 2]; 3],
}

fn parse_record(
    content: &[u8],
    common_name: &str,
    now: SystemTime,
) -> io::Result<ValidationRecord> {
    let value = plist::Value::from_reader(io::Cursor::new(content))
        .map_err(|_| invalid("Apple validation record has an invalid plist"))?;
    let record = value
        .as_dictionary()
        .ok_or_else(|| invalid("Apple validation record must be a plist dictionary"))?;
    // Only remove the production prefix. Match the complete remaining account ID.
    let account_id = common_name
        .strip_prefix(APPLE_ID_COMMON_NAME_PREFIX)
        .filter(|account_id| !account_id.is_empty())
        .ok_or_else(|| invalid("Apple AirDrop certificate has an unexpected account name"))?;
    if !["encDsID", "altDsID"]
        .iter()
        .any(|field| record.get(field).and_then(plist::Value::as_string) == Some(account_id))
    {
        return Err(invalid(
            "Apple validation record and certificate identify different accounts",
        ));
    }

    let valid_from = record
        .get("ValidAsOf")
        .and_then(plist::Value::as_date)
        .map(SystemTime::from)
        .ok_or_else(|| invalid("Apple validation record has no validity date"))?;
    let expires = record
        .get("SuggestValidDuration")
        .and_then(plist::Value::as_unsigned_integer)
        .and_then(|seconds| valid_from.checked_add(Duration::from_secs(seconds)))
        .filter(|expires| valid_from <= now && now < *expires)
        .ok_or_else(|| invalid("Apple validation record is expired or not yet valid"))?;

    let phones = hash_prefixes(record.get("ValidatedPhoneHashes"))?;
    let emails = hash_prefixes(record.get("ValidatedEmailHashes"))?;
    if phones.is_empty() && emails.is_empty() {
        return Err(invalid(
            "Apple validation record contains no verified contact identifiers",
        ));
    }
    let mut emails = emails.into_iter();
    Ok(ValidationRecord {
        expires,
        contact_prefixes: [
            phones.first().copied().unwrap_or_default(),
            emails.next().unwrap_or_default(),
            emails.next().unwrap_or_default(),
        ],
    })
}

fn hash_prefixes(value: Option<&plist::Value>) -> io::Result<Vec<[u8; 2]>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let hashes = value
        .as_array()
        .ok_or_else(|| invalid("Apple validation record contact hashes must be an array"))?;
    hashes
        .iter()
        .map(|hash| {
            let mut decoded = [0_u8; 32];
            if let Some(data) = hash.as_data().filter(|data| data.len() == decoded.len()) {
                decoded.copy_from_slice(data);
            } else if let Some(hex) = hash.as_string() {
                hex::decode_to_slice(hex, &mut decoded)
                    .map_err(|_| invalid("Apple validation record has a malformed SHA-256 hash"))?;
            } else {
                return Err(invalid(
                    "Apple validation record has a malformed SHA-256 hash",
                ));
            }
            Ok([decoded[0], decoded[1]])
        })
        .collect()
}

/// Only Apple ID sharing certificates can receive the imported signed record.
/// The AWDL endpoint is a link-local IP, without a conventional TLS DNS name.
#[derive(Debug)]
struct AppleAirDropPeer {
    algorithms: WebPkiSupportedAlgorithms,
    leaf_sha256: Option<[u8; 32]>,
}

impl ServerCertVerifier for AppleAirDropPeer {
    fn verify_server_cert(
        &self,
        leaf: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if self
            .leaf_sha256
            .is_some_and(|expected| <[u8; 32]>::from(Sha256::digest(leaf)) != expected)
        {
            return Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ));
        }
        apple_crypto::sharing_certificate(leaf, intermediates)
            .map_err(|_| Error::InvalidCertificate(CertificateError::UnknownIssuer))?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}
