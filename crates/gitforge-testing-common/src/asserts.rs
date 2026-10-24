//! Assertion helpers with readable failure messages.
//!
//! Each helper panics with a message that shows both sides of the failed
//! comparison, so the test output names the mismatch instead of the line
//! number.

/// Assert `haystack` contains `needle`, printing both on failure.
pub fn assert_contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected text to contain {needle:?}; got:\n{haystack}"
    );
}

/// Assert `haystack` does NOT contain `needle`.
pub fn assert_not_contains(haystack: &str, needle: &str) {
    assert!(
        !haystack.contains(needle),
        "expected text NOT to contain {needle:?}; got:\n{haystack}"
    );
}

/// Assert two sets are equal ignoring order and duplicates, printing the
/// symmetric difference on failure.
pub fn assert_set_eq<T>(left: &[T], right: &[T])
where
    T: std::fmt::Debug + Ord + Clone + PartialEq,
{
    let mut a: Vec<&T> = left.iter().collect();
    let mut b: Vec<&T> = right.iter().collect();
    a.sort();
    b.sort();
    a.dedup();
    b.dedup();
    assert!(
        a == b,
        "sets differ:\n  left only:  {:?}\n  right only: {:?}\n  left:  {left:?}\n  right: {right:?}",
        a.iter().filter(|x| !b.contains(x)).collect::<Vec<_>>(),
        b.iter().filter(|x| !a.contains(x)).collect::<Vec<_>>(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "expected text to contain")]
    fn contains_failure_names_the_needle() {
        assert_contains("hello world", "goodbye");
    }

    #[test]
    #[should_panic(expected = "expected text NOT to contain")]
    fn not_contains_failure_names_the_needle() {
        assert_not_contains("hello world", "world");
    }

    #[test]
    #[should_panic(expected = "sets differ")]
    fn set_eq_failure_shows_diff() {
        assert_set_eq(&[1, 2, 3], &[1, 2, 4]);
    }

    #[test]
    fn set_eq_ignores_order_and_duplicates() {
        assert_set_eq(&[3, 1, 2, 1], &[1, 2, 3, 3]);
    }
}
