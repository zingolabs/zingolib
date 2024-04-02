fn main() {
  uniffi_build::generate_scaffolding("src/rustlib.udl")
    .expect("A valid UDL file");
}