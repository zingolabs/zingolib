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
        .binaryTarget(
            name: "Zingolib",
            path: "\(builderOutput)/Zingolib.xcframework"
        ),
        .binaryTarget(
            name: "ZingoNymProxyFFI",
            path: "\(builderOutput)/ZingoNymProxyFFI.xcframework"
        ),
        .target(
            name: "ZingoBindings",
            dependencies: ["Zingolib", "ZingoNymProxyFFI"],
            path: "\(builderOutput)/Sources/ZingoBindings"
        ),
    ]
)
