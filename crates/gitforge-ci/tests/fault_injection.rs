//! Integration tests for the M10 fault-injection rollout gate.
//!
//! The parser is the single gate that decides whether a `fault:` key can
//! enter the system: these tests pin the flag-off rejection and the
//! flag-on acceptance end to end through `PipelineDefinition::parse`.

// Test-harness exemption, same discipline as the sibling suites:
// `allow-unwrap-in-tests` covers `#[test]` bodies, but the fixture helper
// below is neither, and setup failure IS the assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use gitforge_ci::PipelineDefinition;
use gitforge_common::{Fault, RolloutFlags};

fn yaml_with_fault(fault: &str) -> String {
    format!(
        "name: faulty\n\
         version: \"1.0\"\n\
         trigger_on: [push]\n\
         jobs:\n\
         \x20 - name: build\n\
         \x20   image: img:1\n\
         \x20   fault: {fault}\n\
         \x20   steps:\n\
         \x20     - name: build\n\
         \x20       run: cargo build\n"
    )
}

#[test]
fn fault_key_is_rejected_by_default_parse() {
    // The flag-off parse is the parse every trigger and API save uses.
    for fault in ["timeout", "nonzero-exit", "oom"] {
        let error = PipelineDefinition::parse(&yaml_with_fault(fault))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("fault injection is disabled"),
            "must name the rollout gate for '{fault}': {error}"
        );
        assert!(
            error.contains("GITFORGE_ENABLE_FAULT_INJECTION"),
            "must name the flag for '{fault}': {error}"
        );
    }
}

#[test]
fn fault_key_parses_when_rollout_flag_enabled() {
    let flags = RolloutFlags {
        fault_injection: true,
    };
    for (spelling, expected) in [
        ("timeout", Fault::Timeout),
        ("nonzero-exit", Fault::NonZeroExit),
        ("oom", Fault::Oom),
    ] {
        let pipeline =
            PipelineDefinition::parse_with_flags(&yaml_with_fault(spelling), &flags).unwrap();
        assert_eq!(pipeline.jobs.len(), 1);
        assert_eq!(pipeline.jobs[0].fault, Some(expected));
    }
}

#[test]
fn unknown_fault_value_is_rejected_even_when_enabled() {
    let flags = RolloutFlags {
        fault_injection: true,
    };
    let error = PipelineDefinition::parse_with_flags(&yaml_with_fault("segfault"), &flags)
        .unwrap_err()
        .to_string();
    assert!(
        error.to_lowercase().contains("segfault") || error.contains("fault"),
        "error names the bad value: {error}"
    );
}

#[test]
fn pipelines_without_fault_are_unaffected_by_the_flag() {
    let plain = "name: plain\n\
                 version: \"1.0\"\n\
                 trigger_on: [push]\n\
                 jobs:\n\
                 \x20 - name: build\n\
                 \x20   image: img:1\n\
                 \x20   steps:\n\
                 \x20     - name: build\n\
                 \x20       run: cargo build\n";
    let on = PipelineDefinition::parse_with_flags(
        plain,
        &RolloutFlags {
            fault_injection: true,
        },
    )
    .unwrap();
    let off = PipelineDefinition::parse_with_flags(plain, &RolloutFlags::default()).unwrap();
    assert!(on.jobs[0].fault.is_none() && off.jobs[0].fault.is_none());
}
