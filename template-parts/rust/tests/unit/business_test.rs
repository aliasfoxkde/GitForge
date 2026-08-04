//! Unit tests for business logic.

use project_name::services::business::BusinessService;

#[test]
fn test_default_service_has_hello_message() {
    let service = BusinessService::new();
    assert_eq!(service.get_hello_message(), "Hello, World!");
}

#[test]
fn test_custom_greeting() {
    let service = BusinessService::with_greeting("Welcome");
    assert_eq!(service.get_hello_message(), "Welcome");
}

#[test]
fn test_greet_returns_personalized_message() {
    let service = BusinessService::new();
    let result = service.greet("World");
    assert_eq!(result, "Hello, World World");
}

#[test]
fn test_validate_name_accepts_valid_input() {
    let service = BusinessService::new();
    let result = service.validate_name("Alice");
    assert!(result.is_ok());
}

#[test]
fn test_validate_name_rejects_empty() {
    let service = BusinessService::new();
    let result = service.validate_name("");
    assert!(result.is_err());
}

#[test]
fn test_validate_name_rejects_overlength() {
    let service = BusinessService::new();
    let long_name = "a".repeat(101);
    let result = service.validate_name(&long_name);
    assert!(result.is_err());
}

#[test]
fn test_transform_converts_to_uppercase() {
    let service = BusinessService::new();
    let result = service.transform("hello world");
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "HELLO WORLD");
}

#[test]
fn test_transform_rejects_empty() {
    let service = BusinessService::new();
    let result = service.transform("");
    assert!(result.is_err());
}
