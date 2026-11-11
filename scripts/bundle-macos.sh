#!/bin/sh
set -eu
if [ "$(uname -s)" != Darwin ]; then
  echo 'This bundler requires macOS. Use cargo run on other supported platforms.' >&2
  exit 1
fi
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cargo build --manifest-path "$project_dir/Cargo.toml" --release --locked
app_dir="$project_dir/target/release/bundle/macos/GPExit.app"
mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources"
install -m 755 "$project_dir/target/release/gpexit-desktop" "$app_dir/Contents/MacOS/gpexit-desktop"
cp "$project_dir/icons/icon.icns" "$app_dir/Contents/Resources/icon.icns"
cat > "$app_dir/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>GPExit</string>
<key>CFBundleDisplayName</key><string>GPExit</string>
<key>CFBundleIdentifier</key><string>it.gpexit.desktop</string>
<key>CFBundleVersion</key><string>0.1.0</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleExecutable</key><string>gpexit-desktop</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIconFile</key><string>icon.icns</string>
<key>LSApplicationCategoryType</key><string>public.app-category.photography</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSDocumentsFolderUsageDescription</key><string>GPExit uses the archive folders you select.</string>
<key>NSDownloadsFolderUsageDescription</key><string>GPExit uses the archive folders you select.</string>
<key>NSDesktopFolderUsageDescription</key><string>GPExit uses the archive folders you select.</string>
</dict></plist>
PLIST
codesign --force --sign - "$app_dir"
codesign --verify --deep --strict "$app_dir"
printf '%s\n' "$app_dir"
