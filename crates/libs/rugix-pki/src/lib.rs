//! `rugix-pki` - CMS signature creation and verification library.
//!
//! This library provides high-level operations for creating and verifying CMS
//! (Cryptographic Message Syntax) signatures, as defined in RFC 5652.
//!
//! # Features
//!
//! - Create CMS SignedData structures with embedded content
//! - Verify CMS signatures against a trusted root certificate
//! - Certificate chain validation
//! - Support for ECDSA (P-256, P-384), RSA PKCS#1 v1.5, and RSA-PSS signatures
//!
//! # Example
//!
//! ```ignore
//! use rugix_pki::{CmsSigner, CmsVerifier};
//!
//! // Sign data
//! let signer = CmsSigner::new(cert_pem, key_pem)?;
//! let signature = signer.sign(data)?;
//!
//! // Verify signature
//! let verifier = CmsVerifier::new(root_cert_pem)?;
//! let extracted_data = verifier.verify(&signature)?;
//! ```

mod pem;
mod sign;
mod verify;

pub use sign::{CmsSigner, RsaHashAlgorithm, RsaSignatureMode, SignerBuilder};
pub use verify::{CmsVerifier, VerificationResult};

use thiserror::Error;

/// Errors that can occur during PKI operations.
#[derive(Debug, Error)]
pub enum PkiError {
    /// Failed to parse PEM-encoded data.
    #[error("failed to parse PEM data: {0}")]
    PemParse(String),

    /// Failed to parse DER-encoded data.
    #[error("failed to parse DER data: {0}")]
    DerParse(String),

    /// Failed to parse a certificate.
    #[error("failed to parse certificate: {0}")]
    CertificateParse(String),

    /// Failed to parse a private key.
    #[error("failed to parse private key: {0}")]
    PrivateKeyParse(String),

    /// The private key algorithm is not supported.
    #[error("unsupported private key algorithm: {0}")]
    UnsupportedKeyAlgorithm(String),

    /// The signature algorithm is not supported.
    #[error("unsupported signature algorithm: {0}")]
    UnsupportedSignatureAlgorithm(String),

    /// Signing operation failed.
    #[error("signing failed: {0}")]
    SigningFailed(String),

    /// The CMS structure is invalid.
    #[error("invalid CMS structure: {0}")]
    InvalidCms(String),

    /// No certificates found in the CMS structure.
    #[error("no certificates found in CMS structure")]
    NoCertificates,

    /// No signer information found.
    #[error("no signer information found")]
    NoSignerInfo,

    /// Certificate chain validation failed.
    #[error("certificate chain validation failed: {0}")]
    ChainValidation(String),

    /// Signature verification failed.
    #[error("signature verification failed: {0}")]
    SignatureVerification(String),

    /// The signed content does not match.
    #[error("content mismatch")]
    ContentMismatch,

    /// No encapsulated content in CMS structure.
    #[error("no encapsulated content in CMS structure")]
    NoEncapsulatedContent,
}

/// Result type for PKI operations.
pub type PkiResult<T> = Result<T, PkiError>;

/// Supported signature algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    /// ECDSA with P-256 and SHA-256.
    EcdsaP256Sha256,
    /// ECDSA with P-384 and SHA-384.
    EcdsaP384Sha384,
    /// RSA with PKCS#1 v1.5 padding and SHA-256.
    RsaPkcs1Sha256,
    /// RSA with PKCS#1 v1.5 padding and SHA-384.
    RsaPkcs1Sha384,
    /// RSA with PKCS#1 v1.5 padding and SHA-512.
    RsaPkcs1Sha512,
    /// RSA-PSS with SHA-256.
    RsaPssSha256,
    /// RSA-PSS with SHA-384.
    RsaPssSha384,
    /// RSA-PSS with SHA-512.
    RsaPssSha512,
    /// Ed25519 (EdDSA with Curve25519).
    Ed25519,
}
