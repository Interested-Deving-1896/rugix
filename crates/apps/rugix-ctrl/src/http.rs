/// Install the process-wide TLS configuration.
pub fn setup() {
    if rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .is_err()
    {
        // This should never happen as there is no pre-installed provider at this
        // point, so the installation should always succeed.
        eprintln!("unable to install default crypto provider, continuing anyway");
    }
}

#[cfg(test)]
mod tests {
    /// Verify that HTTPS requests do not panic due to missing TLS configuration.
    #[test]
    fn https_request_does_not_panic() {
        super::setup();
        let response = ureq::get("https://rugix.org").call().unwrap();
        assert!(response.status().is_success());
    }
}
