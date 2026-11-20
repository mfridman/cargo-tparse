// The only test in this binary, so the result does not depend on timing.
#[test]
fn aborts() {
    std::process::abort();
}
