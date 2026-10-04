#[test]
fn checkpoint_dialect_compile_fixtures() {
    let tests = trybuild::TestCases::new();
    tests.pass("tests/ui/pass*.rs");
    tests.compile_fail("tests/ui/fail_*.rs");
}
