#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 || ! $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Usage: bash package-unix.sh <x.y.z> <Rust target>" >&2
    exit 1
fi
version=$1
target=$2
case "$target" in
    x86_64-unknown-linux-gnu) platform=linux; arch=x64 ;;
    x86_64-apple-darwin) platform=macos; arch=x64; machine_arch=x86_64 ;;
    aarch64-apple-darwin) platform=macos; arch=arm64; machine_arch=arm64 ;;
    *) echo "Unsupported target: $target" >&2; exit 1 ;;
esac

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
binary="$repo_root/target/$target/release/Mi"
[[ -x "$binary" ]] || { echo "Build the release binary first: $binary" >&2; exit 1; }
dist="$repo_root/dist"
mkdir -p "$dist"
stage=$(mktemp -d "${TMPDIR:-/tmp}/mi-package.XXXXXX")
trap 'rm -rf "$stage"' EXIT
package_name="Mi-$version-$platform-$arch"
payload="$stage/$package_name"
mkdir -p "$payload"
cp "$repo_root/LICENSE" "$repo_root/README.md" "$payload/"

if [[ "$platform" == linux ]]; then
    ldd "$binary" > "$stage/dependencies.txt"
    cat "$stage/dependencies.txt"
    if grep -q 'not found' "$stage/dependencies.txt"; then
        echo 'The Linux binary has missing shared libraries' >&2
        exit 1
    fi
    cp "$binary" "$payload/Mi"
    chmod +x "$payload/Mi"
    # A launcher entry that offers Mi for audio files; see the README for installing it.
    cp "$repo_root/resources/appIcon.png" "$payload/mi.png"
    cat > "$payload/mi.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Mi
Comment=Local music player with synchronized lyrics
Exec=Mi %F
Icon=mi
Terminal=false
Categories=AudioVideo;Audio;Player;
MimeType=audio/mpeg;audio/flac;audio/x-flac;audio/wav;audio/x-wav;audio/mp4;audio/x-m4a;audio/aac;audio/ogg;audio/x-vorbis+ogg;
StartupWMClass=com.sqhh99.Mi
EOF
    asset="$package_name.tar.gz"
    tar -czf "$dist/$asset" -C "$stage" "$package_name"
    (cd "$dist" && sha256sum "$asset" > "$asset.sha256")
else
    deployment_target=${MACOSX_DEPLOYMENT_TARGET:-11.0}
    if [[ ! $deployment_target =~ ^[0-9]+\.[0-9]+(\.[0-9]+)?$ ]]; then
        echo "Invalid macOS deployment target: $deployment_target" >&2
        exit 1
    fi
    app="$payload/Mi.app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$binary" "$app/Contents/MacOS/Mi"
    chmod +x "$app/Contents/MacOS/Mi"
    lipo "$app/Contents/MacOS/Mi" -verify_arch "$machine_arch"
    otool -L "$app/Contents/MacOS/Mi" > "$stage/dependencies.txt"
    cat "$stage/dependencies.txt"
    # Releases may only link to macOS system libraries and frameworks.
    if tail -n +2 "$stage/dependencies.txt" | awk '{print $1}' | grep -Ev '^(/System/Library/|/usr/lib/)'; then
        echo 'The macOS binary links to a library outside the system directories' >&2
        exit 1
    fi

    iconset="$stage/appIcon.iconset"
    mkdir -p "$iconset"
    for size in 16 32 128 256 512; do
        sips -z "$size" "$size" "$repo_root/resources/appIcon.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
        double=$((size * 2))
        sips -z "$double" "$double" "$repo_root/resources/appIcon.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
    done
    iconutil -c icns "$iconset" -o "$app/Contents/Resources/appIcon.icns"
    cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Mi</string>
    <key>CFBundleDisplayName</key><string>Mi</string>
    <key>CFBundleExecutable</key><string>Mi</string>
    <key>CFBundleIdentifier</key><string>com.sqhh99.Mi</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleIconFile</key><string>appIcon.icns</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
    <key>LSMinimumSystemVersion</key><string>$deployment_target</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key><string>Audio file</string>
            <key>CFBundleTypeRole</key><string>Viewer</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>public.mp3</string>
                <string>org.xiph.flac</string>
                <string>com.microsoft.waveform-audio</string>
                <string>public.mpeg-4-audio</string>
                <string>public.aac-audio</string>
                <string>org.xiph.ogg-audio</string>
                <string>public.audio</string>
            </array>
        </dict>
    </array>
</dict>
</plist>
EOF
    plutil -lint "$app/Contents/Info.plist"
    codesign --force --sign - --timestamp=none "$app"
    codesign --verify --strict --verbose=2 "$app"
    asset="$package_name.app.zip"
    ditto -c -k --sequesterRsrc "$payload" "$dist/$asset"
    (cd "$dist" && shasum -a 256 "$asset" > "$asset.sha256")
fi
echo "Created $dist/$asset and its SHA-256 checksum"
