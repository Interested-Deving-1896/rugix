//! PEM encoding and decoding utilities.

use crate::{PkiError, PkiResult};

/// Parse PEM-encoded data, returning the DER bytes.
pub fn parse(pem_data: &[u8], expected_label: &str) -> PkiResult<Vec<u8>> {
    let (label, decoded) =
        pem_rfc7468::decode_vec(pem_data).map_err(|e| PkiError::PemParse(e.to_string()))?;

    if label != expected_label {
        return Err(PkiError::PemParse(format!(
            "unexpected PEM label: {}, expected {}",
            label, expected_label
        )));
    }

    Ok(decoded)
}

/// Parse PEM-encoded data with multiple possible labels.
///
/// Returns the matched label and the decoded DER bytes.
pub fn parse_any(pem_data: &[u8], labels: &[&str]) -> PkiResult<(String, Vec<u8>)> {
    let (label, decoded) =
        pem_rfc7468::decode_vec(pem_data).map_err(|e| PkiError::PemParse(e.to_string()))?;

    if labels.contains(&label) {
        return Ok((label.to_string(), decoded));
    }

    Err(PkiError::PemParse(format!(
        "unexpected PEM label: {}, expected one of {:?}",
        label, labels
    )))
}
