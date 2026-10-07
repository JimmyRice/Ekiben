// swift-tools-version:5.9
// Copied next to the generated sources and the XCFramework by `cargo xtask uniffi swift-package`.
import PackageDescription

let package = Package(
    name: "Kaisatsu",
    platforms: [.iOS(.v13), .macOS(.v11)],
    products: [.library(name: "Kaisatsu", targets: ["Kaisatsu"])],
    targets: [
        .binaryTarget(name: "kaisatsu_uniffiFFI", path: "kaisatsu_uniffiFFI.xcframework"),
        .target(name: "Kaisatsu", dependencies: ["kaisatsu_uniffiFFI"]),
    ]
)
