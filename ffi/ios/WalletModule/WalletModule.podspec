Pod::Spec.new do |s|
  s.name         = "WalletModule"
  s.version      = "0.1.0"
  s.summary      = "Rust wallet engine RN bridge"
  s.platforms    = { :ios => "13.0" }
  s.source       = { :path => "." }
  s.source_files = "**/*.{h,m,mm,swift}"
  s.swift_version = "5.0"

  # UniFFI generated Swift:
  s.source_files += "Generated/**/*.{swift,h}"

  # Rust static lib(s) you built for iOS:
  s.vendored_libraries = "RustLibs/*.a"

  s.dependency "React-Core"
end
