// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "ZingoBindingsContractTests",
    platforms: [.iOS("16.0")],
    dependencies: [
        .package(name: "ZingoBindings", path: "../../../bindings/swift"),
    ],
    targets: [
        .testTarget(
            name: "GoldenWireContractTests",
            dependencies: [.product(name: "ZingoBindings", package: "ZingoBindings")],
            path: "swift"
        ),
    ]
)
