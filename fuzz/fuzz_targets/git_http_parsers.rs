//! Fuzz the smart-HTTP request parsers.
//!
//! `parse_service` classifies the URL path of every Git-over-HTTP
//! request and `parse_content_type` splits the Content-Type header —
//! both run on raw client-controlled strings before any authentication
//! or repo resolution. The contract: return `Some(..)` or `None`, never
//! panic, whatever bytes arrive.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = gitforge_core::git_protocol::http::parse_service(text);
        let _ = gitforge_core::git_protocol::http::parse_content_type(text);
    }
});
