//! Business logic stubs.

use crate::models::types::{AppError, AppResult};

/// Business service containing core application logic.
#[derive(Debug, Clone)]
pub struct BusinessService {
    greeting: String,
}

impl BusinessService {
    /// Create a new BusinessService with default settings.
    pub fn new() -> Self {
        Self {
            greeting: "Hello, World!".to_string(),
        }
    }

    /// Create a new BusinessService with a custom greeting.
    pub fn with_greeting(greeting: impl Into<String>) -> Self {
        Self {
            greeting: greeting.into(),
        }
    }

    /// Get the configured hello message.
    pub fn get_hello_message(&self) -> &str {
        &self.greeting
    }

    /// Process a user-provided name and return a personalized greeting.
    pub fn greet(&self, name: &str) -> String {
        format!("{} {}", self.greeting.trim_end_matches('!'), name)
    }

    /// Validate input and return an error if invalid.
    pub fn validate_name(&self, name: &str) -> AppResult<String> {
        if name.is_empty() {
            return Err(AppError::InvalidInput("name cannot be empty".to_string()));
        }
        if name.len() > 100 {
            return Err(AppError::InvalidInput("name too long".to_string()));
        }
        Ok(name.to_string())
    }

    /// Transform input by applying business rules.
    pub fn transform(&self, input: &str) -> AppResult<String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(AppError::InvalidInput("input cannot be empty".to_string()));
        }
        Ok(trimmed.to_uppercase())
    }
}

impl Default for BusinessService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_business_service_default() {
        let service = BusinessService::new();
        assert_eq!(service.get_hello_message(), "Hello, World!");
    }

    #[test]
    fn test_business_service_custom_greeting() {
        let service = BusinessService::with_greeting("Welcome");
        assert_eq!(service.get_hello_message(), "Welcome");
    }

    #[test]
    fn test_greet() {
        let service = BusinessService::new();
        let result = service.greet("Alice");
        assert_eq!(result, "Hello, World Alice");
    }

    #[test]
    fn test_validate_name_valid() {
        let service = BusinessService::new();
        let result = service.validate_name("Alice");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Alice");
    }

    #[test]
    fn test_validate_name_empty() {
        let service = BusinessService::new();
        let result = service.validate_name("");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_name_too_long() {
        let service = BusinessService::new();
        let long_name = "a".repeat(101);
        let result = service.validate_name(&long_name);
        assert!(result.is_err());
    }

    #[test]
    fn test_transform() {
        let service = BusinessService::new();
        let result = service.transform("hello");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "HELLO");
    }

    #[test]
    fn test_transform_empty() {
        let service = BusinessService::new();
        let result = service.transform("");
        assert!(result.is_err());
    }
}
