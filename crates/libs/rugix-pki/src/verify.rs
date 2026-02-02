//! CMS signature verification with certificate chain validation.

use aws_lc_rs::digest::{digest, SHA256, SHA384, SHA512};
use aws_lc_rs::signature::{
    UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, ECDSA_P384_SHA384_ASN1, ED25519,
    RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_2048_8192_SHA384, RSA_PKCS1_2048_8192_SHA512,
    RSA_PSS_2048_8192_SHA256, RSA_PSS_2048_8192_SHA384, RSA_PSS_2048_8192_SHA512,
};
use cms::cert::CertificateChoices;
use cms::content_info::ContentInfo;
use cms::signed_data::{SignedData, SignerIdentifier};
use const_oid::db::rfc5911::{ID_MESSAGE_DIGEST, ID_SIGNED_DATA};
use const_oid::db::rfc5912::{
    ECDSA_WITH_SHA_256, ECDSA_WITH_SHA_384, ID_EC_PUBLIC_KEY, ID_SHA_256, ID_SHA_384, ID_SHA_512,
    RSA_ENCRYPTION, SHA_256_WITH_RSA_ENCRYPTION, SHA_384_WITH_RSA_ENCRYPTION,
    SHA_512_WITH_RSA_ENCRYPTION,
};
use const_oid::ObjectIdentifier;
use der::asn1::OctetString;
use der::{Decode, Encode, Tag};
use x509_cert::Certificate;

use crate::{pem, PkiError, PkiResult};

/// OID for RSA-PSS signature algorithm (1.2.840.113549.1.1.10)
const ID_RSASSA_PSS: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.10");

/// OID for Ed25519 (1.3.101.112) - RFC 8410
const ID_ED25519: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.101.112");

/// Result of successful CMS verification.
#[derive(Debug)]
pub struct VerificationResult {
    /// The extracted encapsulated content.
    pub content: Vec<u8>,
    /// The signer certificate (DER-encoded).
    pub signer_certificate: Vec<u8>,
    /// The certificate chain from signer to root (inclusive, DER-encoded).
    pub certificate_chain: Vec<Vec<u8>>,
}

/// A CMS verifier that validates signatures against a trusted root certificate.
pub struct CmsVerifier {
    root_cert: Certificate,
}

impl CmsVerifier {
    /// Create a new CMS verifier with a PEM-encoded root certificate.
    pub fn new(root_cert_pem: &[u8]) -> PkiResult<Self> {
        let root_cert_der = pem::parse(root_cert_pem, "CERTIFICATE")?;
        let root_cert = Certificate::from_der(&root_cert_der)
            .map_err(|e| PkiError::CertificateParse(e.to_string()))?;

        Ok(Self { root_cert })
    }

    /// Create a new CMS verifier from a DER-encoded root certificate.
    pub fn from_der(root_cert_der: &[u8]) -> PkiResult<Self> {
        let root_cert = Certificate::from_der(root_cert_der)
            .map_err(|e| PkiError::CertificateParse(e.to_string()))?;

        Ok(Self { root_cert })
    }

    /// Verify a DER-encoded CMS signature.
    ///
    /// This method:
    /// 1. Parses the CMS structure
    /// 2. Extracts and validates the certificate chain
    /// 3. Verifies the signature
    /// 4. Returns the encapsulated content if verification succeeds
    pub fn verify(&self, cms_der: &[u8]) -> PkiResult<VerificationResult> {
        let content_info = ContentInfo::from_der(cms_der)
            .map_err(|e| PkiError::InvalidCms(format!("failed to parse ContentInfo: {}", e)))?;

        if content_info.content_type != ID_SIGNED_DATA {
            return Err(PkiError::InvalidCms(format!(
                "unexpected content type: {}, expected SignedData",
                content_info.content_type
            )));
        }

        let signed_data: SignedData = content_info
            .content
            .decode_as()
            .map_err(|e| PkiError::InvalidCms(format!("failed to parse SignedData: {}", e)))?;

        let econtent = signed_data
            .encap_content_info
            .econtent
            .as_ref()
            .ok_or(PkiError::NoEncapsulatedContent)?;

        let content_bytes: OctetString = econtent
            .decode_as()
            .map_err(|e| PkiError::InvalidCms(format!("failed to decode content: {}", e)))?;
        let content = content_bytes.as_bytes().to_vec();

        let signer_info = signed_data
            .signer_infos
            .0
            .iter()
            .next()
            .ok_or(PkiError::NoSignerInfo)?;

        let embedded_certs = extract_certificates(&signed_data)?;
        let signer_cert = find_signer_certificate(&embedded_certs, &signer_info.sid)?;
        let chain = build_certificate_chain(signer_cert, &embedded_certs, &self.root_cert)?;

        verify_cms_signature(&content, signer_info, signer_cert)?;

        let signer_cert_der = signer_cert
            .to_der()
            .map_err(|e| PkiError::DerParse(e.to_string()))?;
        let chain_der: Vec<Vec<u8>> = chain
            .iter()
            .map(|c| c.to_der())
            .collect::<Result<_, _>>()
            .map_err(|e| PkiError::DerParse(e.to_string()))?;

        Ok(VerificationResult {
            content,
            signer_certificate: signer_cert_der,
            certificate_chain: chain_der,
        })
    }

    /// Verify a DER-encoded CMS signature and check that the content matches expected
    /// data.
    pub fn verify_content(&self, cms_der: &[u8], expected_content: &[u8]) -> PkiResult<()> {
        let result = self.verify(cms_der)?;
        if result.content != expected_content {
            return Err(PkiError::ContentMismatch);
        }
        Ok(())
    }
}

/// Extract all certificates from a SignedData structure.
fn extract_certificates(signed_data: &SignedData) -> PkiResult<Vec<Certificate>> {
    let certs_opt = signed_data.certificates.as_ref();
    let Some(certs) = certs_opt else {
        return Err(PkiError::NoCertificates);
    };

    let mut certificates = Vec::new();
    for cert_choice in certs.0.iter() {
        match cert_choice {
            CertificateChoices::Certificate(cert) => {
                let cert_der = cert
                    .to_der()
                    .map_err(|e| PkiError::CertificateParse(e.to_string()))?;
                let parsed_cert = Certificate::from_der(&cert_der)
                    .map_err(|e| PkiError::CertificateParse(e.to_string()))?;
                certificates.push(parsed_cert);
            }
            _ => continue,
        }
    }

    if certificates.is_empty() {
        return Err(PkiError::NoCertificates);
    }

    Ok(certificates)
}

/// Find the signer certificate from the embedded certificates.
fn find_signer_certificate<'a>(
    certificates: &'a [Certificate],
    signer_id: &SignerIdentifier,
) -> PkiResult<&'a Certificate> {
    match signer_id {
        SignerIdentifier::IssuerAndSerialNumber(issuer_and_serial) => {
            for cert in certificates {
                if cert.tbs_certificate.issuer == issuer_and_serial.issuer
                    && cert.tbs_certificate.serial_number == issuer_and_serial.serial_number
                {
                    return Ok(cert);
                }
            }
            Err(PkiError::ChainValidation(
                "signer certificate not found in embedded certificates".into(),
            ))
        }
        SignerIdentifier::SubjectKeyIdentifier(_ski) => {
            // TODO: Support SKI-based lookup
            Err(PkiError::ChainValidation(
                "SubjectKeyIdentifier lookup not yet supported".into(),
            ))
        }
    }
}

/// Build the certificate chain from signer to root.
fn build_certificate_chain<'a>(
    signer_cert: &'a Certificate,
    embedded_certs: &'a [Certificate],
    root_cert: &'a Certificate,
) -> PkiResult<Vec<&'a Certificate>> {
    let mut chain = vec![signer_cert];
    let mut current_cert = signer_cert;

    const MAX_CHAIN_LENGTH: usize = 10;

    for _ in 0..MAX_CHAIN_LENGTH {
        if is_issued_by(current_cert, root_cert) {
            verify_certificate_signature(current_cert, root_cert)?;
            chain.push(root_cert);
            return Ok(chain);
        }

        if is_self_signed(current_cert) {
            return Err(PkiError::ChainValidation(
                "chain ends at self-signed certificate that is not the trusted root".into(),
            ));
        }

        let issuer = find_issuer_certificate(current_cert, embedded_certs)?;
        verify_certificate_signature(current_cert, issuer)?;

        chain.push(issuer);
        current_cert = issuer;
    }

    Err(PkiError::ChainValidation(format!(
        "certificate chain too long (max {} certificates)",
        MAX_CHAIN_LENGTH
    )))
}

/// Check if a certificate is issued by another certificate.
fn is_issued_by(cert: &Certificate, issuer: &Certificate) -> bool {
    cert.tbs_certificate.issuer == issuer.tbs_certificate.subject
}

/// Check if a certificate is self-signed.
fn is_self_signed(cert: &Certificate) -> bool {
    cert.tbs_certificate.issuer == cert.tbs_certificate.subject
}

/// Find the issuer certificate for a given certificate.
fn find_issuer_certificate<'a>(
    cert: &Certificate,
    candidates: &'a [Certificate],
) -> PkiResult<&'a Certificate> {
    for candidate in candidates {
        if is_issued_by(cert, candidate) {
            return Ok(candidate);
        }
    }
    Err(PkiError::ChainValidation(format!(
        "issuer certificate not found for subject: {:?}",
        cert.tbs_certificate.subject
    )))
}

/// Verify that a certificate's signature is valid given the issuer's public key.
fn verify_certificate_signature(cert: &Certificate, issuer: &Certificate) -> PkiResult<()> {
    let tbs_der = cert
        .tbs_certificate
        .to_der()
        .map_err(|e| PkiError::ChainValidation(format!("failed to encode TBS: {}", e)))?;

    let signature_bytes = cert
        .signature
        .as_bytes()
        .ok_or_else(|| PkiError::ChainValidation("certificate signature has unused bits".into()))?;

    let issuer_spki = &issuer.tbs_certificate.subject_public_key_info;
    let public_key_der = issuer_spki
        .subject_public_key
        .as_bytes()
        .ok_or_else(|| PkiError::ChainValidation("public key has unused bits".into()))?;

    let sig_alg = &cert.signature_algorithm.oid;

    verify_signature_with_algorithm(
        &tbs_der,
        signature_bytes,
        public_key_der,
        sig_alg,
        issuer_spki,
    )
    .map_err(|e| PkiError::ChainValidation(format!("certificate signature invalid: {}", e)))
}

/// Verify the CMS signature on the content.
///
/// This handles both cases:
/// - Without signed attributes: signature is over the content directly
/// - With signed attributes: signature is over the DER-encoded signed attributes
fn verify_cms_signature(
    content: &[u8],
    signer_info: &cms::signed_data::SignerInfo,
    signer_cert: &Certificate,
) -> PkiResult<()> {
    let signature_bytes = signer_info.signature.as_bytes();
    let sig_alg = &signer_info.signature_algorithm.oid;
    let digest_alg = &signer_info.digest_alg.oid;

    let signer_spki = &signer_cert.tbs_certificate.subject_public_key_info;
    let public_key_der = signer_spki
        .subject_public_key
        .as_bytes()
        .ok_or_else(|| PkiError::SignatureVerification("public key has unused bits".into()))?;

    let data_to_verify = if let Some(signed_attrs) = &signer_info.signed_attrs {
        // When signed attributes are present, the signature is over the DER encoding
        // of the signed attributes as a SET (not the IMPLICIT [0] used in the CMS structure)
        let content_digest =
            compute_digest(content, digest_alg).map_err(PkiError::SignatureVerification)?;

        verify_message_digest_attribute(signed_attrs, &content_digest)
            .map_err(PkiError::SignatureVerification)?;

        encode_signed_attrs_as_set(signed_attrs).map_err(PkiError::SignatureVerification)?
    } else {
        content.to_vec()
    };

    verify_signature_with_algorithm_params(
        &data_to_verify,
        signature_bytes,
        public_key_der,
        sig_alg,
        signer_spki,
        signer_info.signature_algorithm.parameters.as_ref(),
    )
    .map_err(PkiError::SignatureVerification)
}

/// Compute the digest of content using the specified algorithm.
fn compute_digest(content: &[u8], digest_alg: &ObjectIdentifier) -> Result<Vec<u8>, String> {
    if *digest_alg == ID_SHA_256 {
        Ok(digest(&SHA256, content).as_ref().to_vec())
    } else if *digest_alg == ID_SHA_384 {
        Ok(digest(&SHA384, content).as_ref().to_vec())
    } else if *digest_alg == ID_SHA_512 {
        Ok(digest(&SHA512, content).as_ref().to_vec())
    } else {
        Err(format!("unsupported digest algorithm: {}", digest_alg))
    }
}

/// Verify the message-digest attribute matches the content digest.
fn verify_message_digest_attribute(
    signed_attrs: &cms::signed_data::SignedAttributes,
    expected_digest: &[u8],
) -> Result<(), String> {
    for attr in signed_attrs.iter() {
        if attr.oid == ID_MESSAGE_DIGEST {
            for value in attr.values.iter() {
                if let Ok(digest) = value.decode_as::<OctetString>() {
                    if digest.as_bytes() == expected_digest {
                        return Ok(());
                    } else {
                        return Err("message-digest attribute does not match content".into());
                    }
                }
            }
            return Err("message-digest attribute has invalid format".into());
        }
    }
    Err("message-digest attribute not found in signed attributes".into())
}

/// Re-encode signed attributes as a SET instead of IMPLICIT [0].
///
/// CMS encodes signed attributes with IMPLICIT [0] tag (0xA0), but signatures
/// are computed over the SET encoding (0x31 tag).
fn encode_signed_attrs_as_set(
    signed_attrs: &cms::signed_data::SignedAttributes,
) -> Result<Vec<u8>, String> {
    let original_der = signed_attrs
        .to_der()
        .map_err(|e| format!("failed to encode signed attrs: {}", e))?;

    let mut result = original_der;
    if !result.is_empty() && result[0] == 0xA0 {
        result[0] = Tag::Set.into();
    }

    Ok(result)
}

/// Verify a signature using the appropriate algorithm.
fn verify_signature_with_algorithm(
    data: &[u8],
    signature: &[u8],
    public_key: &[u8],
    sig_alg_oid: &ObjectIdentifier,
    spki: &x509_cert::spki::SubjectPublicKeyInfoOwned,
) -> Result<(), String> {
    verify_signature_with_algorithm_params(data, signature, public_key, sig_alg_oid, spki, None)
}

/// Verify a signature using the appropriate algorithm, with optional signature algorithm parameters.
fn verify_signature_with_algorithm_params(
    data: &[u8],
    signature: &[u8],
    public_key: &[u8],
    sig_alg_oid: &ObjectIdentifier,
    spki: &x509_cert::spki::SubjectPublicKeyInfoOwned,
    sig_alg_params: Option<&der::Any>,
) -> Result<(), String> {
    let key_alg = &spki.algorithm.oid;

    if *key_alg == ID_EC_PUBLIC_KEY {
        let params = spki
            .algorithm
            .parameters
            .as_ref()
            .ok_or("missing EC parameters")?;

        let curve_oid = params
            .decode_as::<ObjectIdentifier>()
            .map_err(|e| format!("invalid EC parameters: {}", e))?;

        if curve_oid == const_oid::db::rfc5912::SECP_256_R_1 {
            if *sig_alg_oid != ECDSA_WITH_SHA_256 {
                return Err(format!(
                    "algorithm mismatch: P-256 key with {} signature",
                    sig_alg_oid
                ));
            }
            let key = UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, public_key);
            key.verify(data, signature)
                .map_err(|e| format!("ECDSA P-256 verification failed: {}", e))
        } else if curve_oid == const_oid::db::rfc5912::SECP_384_R_1 {
            if *sig_alg_oid != ECDSA_WITH_SHA_384 {
                return Err(format!(
                    "algorithm mismatch: P-384 key with {} signature",
                    sig_alg_oid
                ));
            }
            let key = UnparsedPublicKey::new(&ECDSA_P384_SHA384_ASN1, public_key);
            key.verify(data, signature)
                .map_err(|e| format!("ECDSA P-384 verification failed: {}", e))
        } else {
            Err(format!("unsupported EC curve: {}", curve_oid))
        }
    } else if *key_alg == RSA_ENCRYPTION {
        let algorithm: &dyn aws_lc_rs::signature::VerificationAlgorithm =
            if *sig_alg_oid == SHA_256_WITH_RSA_ENCRYPTION {
                &RSA_PKCS1_2048_8192_SHA256
            } else if *sig_alg_oid == SHA_384_WITH_RSA_ENCRYPTION {
                &RSA_PKCS1_2048_8192_SHA384
            } else if *sig_alg_oid == SHA_512_WITH_RSA_ENCRYPTION {
                &RSA_PKCS1_2048_8192_SHA512
            } else if *sig_alg_oid == ID_RSASSA_PSS {
                determine_rsa_pss_algorithm(sig_alg_params)?
            } else {
                return Err(format!(
                    "unsupported RSA signature algorithm: {}",
                    sig_alg_oid
                ));
            };

        // RSA verification requires the full SPKI DER, not just the key bytes
        let spki_der = spki
            .to_der()
            .map_err(|e| format!("failed to encode SPKI: {}", e))?;
        let key = UnparsedPublicKey::new(algorithm, &spki_der);
        key.verify(data, signature)
            .map_err(|e| format!("RSA verification failed: {}", e))
    } else if *key_alg == ID_ED25519 {
        if *sig_alg_oid != ID_ED25519 {
            return Err(format!(
                "algorithm mismatch: Ed25519 key with {} signature",
                sig_alg_oid
            ));
        }
        let key = UnparsedPublicKey::new(&ED25519, public_key);
        key.verify(data, signature)
            .map_err(|e| format!("Ed25519 verification failed: {}", e))
    } else {
        Err(format!("unsupported key algorithm: {}", key_alg))
    }
}

/// Determine the RSA-PSS verification algorithm from signature algorithm parameters.
///
/// RSA-PSS parameters (RFC 4055) encode the hash algorithm. We use a simple heuristic
/// of searching for hash algorithm OIDs in the DER-encoded parameters, falling back
/// to SHA-256 if parameters are missing or unparseable.
fn determine_rsa_pss_algorithm(
    sig_alg_params: Option<&der::Any>,
) -> Result<&'static dyn aws_lc_rs::signature::VerificationAlgorithm, String> {
    if let Some(params) = sig_alg_params
        && let Ok(params_bytes) = params.to_der()
    {
        let sha384_bytes = ID_SHA_384.as_bytes();
        let sha512_bytes = ID_SHA_512.as_bytes();

        if contains_subsequence(&params_bytes, sha512_bytes) {
            return Ok(&RSA_PSS_2048_8192_SHA512);
        } else if contains_subsequence(&params_bytes, sha384_bytes) {
            return Ok(&RSA_PSS_2048_8192_SHA384);
        }
    }

    Ok(&RSA_PSS_2048_8192_SHA256)
}

/// Check if a byte slice contains a subsequence.
fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
