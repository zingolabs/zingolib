use crate::shardtree_ext::{BOUNDARY_FILE, BOUNDARY_PATH, assert_raw_boolean_calls_confined};

#[test]
fn raw_boolean_calls_route_through_the_wrapper() {
    assert_raw_boolean_calls_confined(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        Some(BOUNDARY_FILE),
        BOUNDARY_PATH,
    );
}
