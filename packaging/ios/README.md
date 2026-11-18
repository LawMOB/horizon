# Horizon for iOS

## Installation

Prebuilt packages are generated automatically through GitHub Actions:

1. Go to the **Actions** tab and manually trigger the **iOS build** workflow.
2. Download the `Horizon-ios` build artifact to obtain `Horizon.ipa`.
3. Install `Horizon.ipa` on your iPhone or iPad using **Sideloadly** or **AltStore** (both tools sign the package using your Apple ID).

---

## Game Assets & Storage

> [!IMPORTANT]
> Horizon does not bundle any Minecraft game files. You must supply assets from your own Java Edition installation.

- **Assets:** Copy your prepared `resources` folder from Minecraft: Java Edition to **Files → On My iPhone → Horizon**.
- **Saves & Configuration:** World saves and Horizon settings are stored inside the app's `Documents` directory at the same location.

---

## Controls

To customize the touch layout, open **Options → Controls → Custom Controls** in-game.

---

## Building from Source

On **macOS** with **Xcode** installed, you can compile and package `Horizon.ipa` directly from the repository root:

```sh
rustup target add aarch64-apple-ios
IPHONEOS_DEPLOYMENT_TARGET=15.0 cargo build --locked --profile ios \
  --target aarch64-apple-ios -p horizon-game --bin horizon-client \
  --no-default-features --features audio
packaging/ios/package.sh target/aarch64-apple-ios/ios/horizon-client 0.1.0 dist
