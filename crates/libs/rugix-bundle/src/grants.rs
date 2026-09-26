//! Typed installation operation for detached grants.
//!
//! The reusable envelope and CMS verification live in `rugix-grants`. This module
//! defines only the installation contract. Installers must compare its target
//! against the request, authenticate the bundle hash, and enforce its sequence.

pub use crate::manifest::grants::*;

impl rugix_grants::Operation for InstallOperation {
    const TYPE: &'static str = "rugix.install.v1";
}

/// Service audience for Rugix Ctrl installation grants.
pub const VERIFIER: &str = "rugix-ctrl";
