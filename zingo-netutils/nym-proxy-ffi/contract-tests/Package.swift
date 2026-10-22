// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "ZingoBindingsContractTests",
    platforms: [.iOS("16.0")],
    dependencies: [
        .package(name: "ZingoBindings", path: "../../../bindings/swift"),
    ],
    targets: [
        // A C++ throw and an Objective-C throw, so the test binary carries the
        // personality routines an app brings beside the archive's Rust one.
        // Compact unwind encodes three at most, so a bundle with two Rust
        // archives fails this link as it fails the app's.
        .target(
            name: "AppPersonalities",
            path: "app-personalities"
        ),
        .testTarget(
            name: "GoldenWireContractTests",
            dependencies: [.product(name: "ZingoBindings", package: "ZingoBindings"), "AppPersonalities"],
            path: "swift"
        ),
    ]
)
