// swift-tools-version:5.5

import PackageDescription

let package = Package(
  name: "tauri-plugin-nearby",
  platforms: [
    .iOS(.v13)
  ],
  products: [
    .library(
      name: "tauri-plugin-nearby",
      type: .static,
      targets: ["tauri-plugin-nearby"])
  ],
  dependencies: [
    // Copied in by `tauri_plugin::Builder::ios_path` at build time.
    .package(name: "Tauri", path: "../.tauri/tauri-api")
  ],
  targets: [
    .target(
      name: "tauri-plugin-nearby",
      dependencies: [
        .byName(name: "Tauri")
      ],
      path: "Sources")
  ]
)
