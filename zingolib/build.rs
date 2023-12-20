// build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    build_utils::git_description();
    Ok(())
}
