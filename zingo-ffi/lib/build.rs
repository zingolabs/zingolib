fn main() {
  uniffi_build::generate_scaffolding("src/zingo.udl")
    .expect("A valid UDL file");
}