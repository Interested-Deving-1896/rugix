//! PEM encoding and decoding utilities.

use base64::prelude::*;

use crate::{PkiError, PkiResult};

/// Parse PEM-encoded data, returning the DER bytes.
pub fn parse(pem_data: &[u8], expected_label: &str) -> PkiResult<Vec<u8>> {
    let pem_str = std::str::from_utf8(pem_data).map_err(|e| PkiError::PemParse(e.to_string()))?;

    let begin_marker = format!("-----BEGIN {}-----", expected_label);
    let end_marker = format!("-----END {}-----", expected_label);

    let start = pem_str
        .find(&begin_marker)
        .ok_or_else(|| PkiError::PemParse(format!("missing {} marker", begin_marker)))?;
    let end = pem_str
        .find(&end_marker)
        .ok_or_else(|| PkiError::PemParse(format!("missing {} marker", end_marker)))?;

    let base64_start = start + begin_marker.len();
    let base64_content: String = pem_str[base64_start..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();

    BASE64_STANDARD
        .decode(&base64_content)
        .map_err(|e| PkiError::PemParse(e.to_string()))
}

/// Parse PEM-encoded data with multiple possible labels.
///
/// Returns the matched label and the decoded DER bytes.
pub fn parse_any(pem_data: &[u8], labels: &[&str]) -> PkiResult<(String, Vec<u8>)> {
    let pem_str = std::str::from_utf8(pem_data).map_err(|e| PkiError::PemParse(e.to_string()))?;

    for label in labels {
        let begin_marker = format!("-----BEGIN {}-----", label);
        let end_marker = format!("-----END {}-----", label);

        if let Some(start) = pem_str.find(&begin_marker)
            && let Some(end) = pem_str.find(&end_marker)
        {
            let base64_start = start + begin_marker.len();
            let base64_content: String = pem_str[base64_start..end]
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();

            let decoded = BASE64_STANDARD
                .decode(&base64_content)
                .map_err(|e| PkiError::PemParse(e.to_string()))?;

            return Ok((label.to_string(), decoded));
        }
    }

    Err(PkiError::PemParse(format!(
        "no matching PEM block found for labels: {:?}",
        labels
    )))
}
