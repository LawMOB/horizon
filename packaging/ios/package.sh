#!/usr/bin/env bash
# Packages the game for iOS: Horizon.app (the binary, Info.plist, the icon) in an unsigned
# Horizon.ipa. A sideloading tool (Sideloadly, AltStore) signs it with the player's own Apple ID
# when it installs it; with IOS_SIGN_IDENTITY set (an "Apple Development: …" identity) and
# IOS_PROVISIONING_PROFILE naming a .mobileprovision, it is signed here instead.
#
#   packaging/ios/package.sh BINARY VERSION OUTDIR
#
# IOS_PROFILE=jailbreak-ram embeds memory entitlements with an ad-hoc signature for
# TrollStore Lite on a jailbroken device, producing Horizon-jailbreak-ram.ipa.
# The default standard profile produces Horizon.ipa.
#
# IOS_INSTANCE (digits) makes it a separate instance: identifier dev.horizon.game.i<digits>, link
# horizon-i<digits>://, so it installs beside Horizon and other instances; IOS_NAME names it on the
# home screen.
#
# Needs macOS (sips, plutil, ditto). Horizon ships none of Mojang's files: the player copies the
# prepared resources into the app's Documents folder with the Files app (packaging/ios/README.md).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
profile=${IOS_PROFILE:-standard}
case "$profile" in
  standard) ipa_name=Horizon.ipa ;;
  jailbreak-ram)
    ipa_name=Horizon-jailbreak-ram.ipa
    if [[ -n "${IOS_SIGN_IDENTITY:-}" || -n "${IOS_PROVISIONING_PROFILE:-}" ]]; then
      echo "jailbreak-ram uses ad-hoc signing; unset IOS_SIGN_IDENTITY and IOS_PROVISIONING_PROFILE" >&2
      exit 1
    fi
    ;;
  *) echo "IOS_PROFILE must be standard or jailbreak-ram" >&2; exit 1 ;;
esac
binary=$1
version=$2
out=$(mkdir -p "$3" && cd "$3" && pwd)
[[ "$(lipo -archs "$binary")" == "arm64" ]] || { echo "$binary is not an arm64 binary" >&2; exit 1; }

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
app=$stage/Payload/Horizon.app
mkdir -p "$app"
cp "$binary" "$app/horizon-client"
chmod 755 "$app/horizon-client"
instance=${IOS_INSTANCE:-}
[[ -z "$instance" || "$instance" =~ ^[0-9]{4,12}$ ]] || { echo "IOS_INSTANCE must be 4-12 digits" >&2; exit 1; }
if [[ -n "$instance" ]]; then
  bundle=dev.horizon.game.i$instance scheme=horizon-i$instance name=${IOS_NAME:-Horizon $instance}
else
  bundle=dev.horizon.game scheme=horizon name=${IOS_NAME:-Horizon}
fi
# The home screen name, as plain text in the plist.
name=$(printf '%s' "$name" | tr -d '<>&"\\/' | cut -c1-30)
sed -e "s/@VERSION@/$version/g" -e "s/@BUNDLE@/$bundle/g" -e "s/@SCHEME@/$scheme/g" -e "s/@NAME@/$name/g" \
  "$here/Info.plist" > "$app/Info.plist"
plutil -lint "$app/Info.plist"
icon="$here/../../crates/horizon-theme/icons/horizon-64.png"
cp "$icon" "$app/BootLogo.png"
xcrun ibtool --compile "$app/LaunchScreen.storyboardc" "$here/LaunchScreen.storyboard" \
  --target-device iphone --target-device ipad --minimum-deployment-target 15.0 \
  --module Horizon
printf 'APPL????' > "$app/PkgInfo"
sips -z 120 120 "$icon" --out "$app/AppIcon60x60@2x.png" >/dev/null
sips -z 180 180 "$icon" --out "$app/AppIcon60x60@3x.png" >/dev/null

if [[ "$profile" == jailbreak-ram ]]; then
  entitlements="$here/entitlements-jailbreak-ram.plist"
  plutil -lint "$entitlements"
  codesign --force --sign - --timestamp=none --generate-entitlement-der \
    --entitlements "$entitlements" "$app"
  codesign --verify --strict "$app"
  # Check the signed executable, not just the input plist, before publishing the IPA.
  codesign --display --entitlements - --xml "$app" > "$stage/signed-entitlements.plist"
  for key in com.apple.developer.kernel.increased-memory-limit \
             com.apple.developer.kernel.extended-virtual-addressing; do
    [[ "$(/usr/libexec/PlistBuddy -c "Print :$key" "$stage/signed-entitlements.plist")" == true ]] || {
      echo "Missing signed entitlement: $key" >&2
      exit 1
    }
  done
elif [[ -n "${IOS_SIGN_IDENTITY:-}" ]]; then
  cp "$IOS_PROVISIONING_PROFILE" "$app/embedded.mobileprovision"
  codesign --force --sign "$IOS_SIGN_IDENTITY" --timestamp=none "$app"
fi

rm -f "$out/$ipa_name"
(cd "$stage" && ditto -c -k --sequesterRsrc --keepParent Payload "$out/$ipa_name")
echo "$out/$ipa_name"
