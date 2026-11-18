Horizon for iOS

The iOS build workflow in the Actions tab builds Horizon.ipa for iPhone and iPad. Run the workflow manually, then download the Horizon-ios artifact. Install the IPA using Sideloadly or AltStore; these tools sign the application using your Apple ID.

Horizon does not include Minecraft files. Copy the prepared resources folder from your own Java Edition installation to Files → On My iPhone → Horizon. Horizon’s saves and settings are also stored in the app’s Documents folder.

On macOS with Xcode installed, you can also build the IPA directly from the repository directory.

rustup target add aarch64-apple-ios
IPHONEOS_DEPLOYMENT_TARGET=15.0 cargo build --locked --profile ios \
  --target aarch64-apple-ios -p horizon-game --bin horizon-client \
  --no-default-features --features audio
packaging/ios/package.sh target/aarch64-apple-ios/ios/horizon-client 0.1.0 dist

The layout editor is available under Options → Controls → Custom Controls.
