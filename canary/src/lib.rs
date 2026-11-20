//! Tests with known results. tests/canary.rs runs them and compares the
//! result with files in tests/testdata/.

/// ```
/// assert_eq!(canary::add(1, 2), 3);
/// ```
///
/// ```
/// assert_eq!(canary::add(1, 2), 4);
/// ```
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    #[test]
    fn passes() {}

    #[test]
    fn fails() {
        assert_eq!(1 + 1, 3, "math is broken");
    }

    #[test]
    #[ignore = "a reason"]
    fn ignored() {}

    #[test]
    #[should_panic(expected = "boom")]
    fn panics() {
        panic!("boom");
    }

    #[test]
    #[should_panic]
    fn does_not_panic() {}

    #[test]
    fn returns_err() -> Result<(), String> {
        Err("bad result".to_string())
    }
}
