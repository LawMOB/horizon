name: iOS resources

on:
  workflow_dispatch:

permissions:
  contents: read

jobs:
  resources:
    runs-on: ubuntu-24.04
    timeout-minutes: 120
    env:
      CARGO_TERM_COLOR: always
      # One exporter lane at a time keeps the hosted runner's memory use bounded.
      HORIZON_PREPARE_JOBS: "1"
    defaults:
      run:
        shell: bash
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - name: Read this checkout's required versions
        id: versions
        run: python3 tools/ci/ios_resources.py versions
      - uses: actions/setup-java@d1374f8ac74474200d16ef8ecee22c7ae7a51880 # v6.3.0
        with:
          distribution: temurin
          java-version: ${{ steps.versions.outputs.java }}
          java-package: jdk
      - uses: dtolnay/rust-toolchain@b725eda525362e7cf34af9a7243cac1e3f01a7ec # stable, 2026-11
        with:
          toolchain: ${{ steps.versions.outputs.rust }}
      - uses: Swatinem/rust-cache@d1527c8f67ef9af357290475d4af4b0fab8f12e0 # v2.9.3
        with:
          key: ios-resources
      - name: Download the pinned game files from Mojang
        run: python3 tools/ci/ios_resources.py download "$RUNNER_TEMP/horizon-minecraft"
      - name: Build this checkout's resource preparer
        run: cargo build --release --locked -p horizon-prepare --bin horizon-prepare
      - name: Export a fresh, complete resource folder
        run: |
          target/release/horizon-prepare --prepare --complete \
            --root "$GITHUB_WORKSPACE" \
            --minecraft-dir "$RUNNER_TEMP/horizon-minecraft" \
            --server-jar "$RUNNER_TEMP/horizon-minecraft/server.jar" \
            --output "$RUNNER_TEMP/horizon-ios-resources/resources"
      - name: Check completeness and the fluid export
        run: |
          target/release/horizon-prepare --check --root "$GITHUB_WORKSPACE" \
            --output "$RUNNER_TEMP/horizon-ios-resources/resources"
          python3 tools/ci/ios_resources.py verify "$RUNNER_TEMP/horizon-ios-resources/resources"
      - name: Upload resources for Files on iPhone
        uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2
        with:
          name: Horizon-ios-resources-${{ steps.versions.outputs.minecraft }}
          # Upload only the finished bundle, never jars or the preparer's scratch files.
          # The downloaded ZIP contains a top-level resources/ folder and BUILD-INFO.txt.
          path: ${{ runner.temp }}/horizon-ios-resources/
          include-hidden-files: true
          if-no-files-found: error
          retention-days: 3
