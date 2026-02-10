// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "WalletSDK",
    platforms: [.iOS(.v13)],
    products: [
        .library(name: "WalletSDK", targets: ["WalletSDK"])
    ],
    targets: [
        .binaryTarget(
            name: "WalletFFI",
            path: "Binaries/WalletFFI.xcframework"
        ),

        .systemLibrary(
            name: "ffiFFI",
            path: "Sources/ffiFFI"
        ),

        .target(
            name: "WalletSDK",
            dependencies: ["WalletFFI", "ffiFFI"],
            path: "Sources/WalletSDK",
            sources: [
                "WalletSDK.swift",
                "Generated/ffi.swift",
            ]
        ),

        .testTarget(
            name: "WalletSDKTests",
            dependencies: ["WalletSDK"],
            path: "Tests/WalletSDKTests"
        ),
    ]
)
