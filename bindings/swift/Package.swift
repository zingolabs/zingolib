// swift-tools-version:5.9
import PackageDescription

let builderOutput = "build"

let package = Package(
    name: "ZingoBindings",
    platforms: [.iOS("16.0")],
    products: [
        .library(name: "ZingoBindings", targets: ["ZingoBindings"]),
    ],
    targets: [
        // One archive carries the wallet crate and the proxy crate, so an app
        // links one Rust std and one `rust_eh_personality`: compact unwind
        // encodes three personality routines at most, and the app brings the
        // C++ and Objective-C ones.
        .binaryTarget(
            name: "Zingolib",
            path: "\(builderOutput)/Zingolib.xcframework"
        ),
        .target(
            name: "ZingoBindings",
            dependencies: ["Zingolib"],
            path: "\(builderOutput)/Sources/ZingoBindings",
            linkerSettings: [.linkedFramework("SystemConfiguration")]
        ),
    ]
)
