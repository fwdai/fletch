// swift-tools-version:5.5

import PackageDescription

let package = Package(
  name: "tauri-plugin-push",
  platforms: [
    .iOS(.v13)
  ],
  products: [
    .library(
      name: "tauri-plugin-push",
      type: .static,
      targets: ["tauri-plugin-push"])
  ],
  dependencies: [
    // Copied in by `tauri_plugin::Builder::ios_path` at build time.
    .package(name: "Tauri", path: "../.tauri/tauri-api")
  ],
  targets: [
    .target(
      name: "tauri-plugin-push",
      dependencies: [
        .byName(name: "Tauri")
      ],
      path: "Sources")
  ]
)
