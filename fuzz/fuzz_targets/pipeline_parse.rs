//! Fuzz the pipeline-definition parser.
//!
//! `.gitforge.yml` is untrusted input: any user who can push to any
//! repository on the platform controls its bytes, and the CI engine
//! parses it before any validation runs. The contract under fuzz is
//! narrow and absolute — `PipelineDefinition::parse` answers `Ok` or
//! `Err`, never panics, never hangs, never aborts (serde_yaml's
//! deep-nesting recursion and every custom `Deserialize` impl in the
//! definition types are in scope).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = gitforge_ci::pipeline::PipelineDefinition::parse(text);
    }
});
