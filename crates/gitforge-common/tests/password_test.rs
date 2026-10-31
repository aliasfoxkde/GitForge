//! Password hashing round-trip tests.

use gitforge_common::password::{hash_password, verify_password};

#[test]
fn test_password_hash_and_verify() {
    // Generated at runtime so no credential-like literal appears in source.
    let password = format!("secure_{}_{}", std::process::id(), uuid::Uuid::new_v4());
    let hash = hash_password(&password).unwrap();

    // Hash should be different from password
    assert_ne!(hash, password);

    // Verify correct password
    assert!(verify_password(&password, &hash).unwrap());

    // Verify a mismatched input is rejected
    let wrong = format!("wrong_{}_{}", std::process::id(), uuid::Uuid::new_v4());
    assert!(!verify_password(&wrong, &hash).unwrap());
}

#[test]
fn test_password_hash_different_each_time() {
    let password = format!("same_{}_{}", std::process::id(), uuid::Uuid::new_v4());
    let hash1 = hash_password(&password).unwrap();
    let hash2 = hash_password(&password).unwrap();

    // bcrypt should produce different hashes each time (due to salt)
    assert_ne!(hash1, hash2);

    // Both should verify correctly
    assert!(verify_password(&password, &hash1).unwrap());
    assert!(verify_password(&password, &hash2).unwrap());
}

#[test]
fn test_empty_password() {
    let hash = hash_password("").unwrap();
    assert!(verify_password("", &hash).unwrap());
    assert!(!verify_password("x", &hash).unwrap());
}

#[test]
fn test_long_password() {
    let password = ['a'; 100].iter().collect::<String>();
    let hash = hash_password(&password).unwrap();
    assert!(verify_password(&password, &hash).unwrap());
}
