#!/bin/sh
# Packs a release build into dist/: rsit-VERSION-x86_64-linux.tar.gz and,
# when appimagetool is available (APPIMAGETOOL or in PATH),
# rsit-VERSION-x86_64.AppImage.
# Usage: script/package-linux.sh VERSION [BINARY]
set -eu
version=$1
binary=${2:-target/release/rsit}
mkdir -p dist

name="rsit-$version-x86_64-linux"
rm -rf "dist/$name"
mkdir -p "dist/$name"
cp "$binary" "dist/$name/rsit"
cp packaging/rsit.desktop packaging/rsit.svg packaging/install.sh NOTICE "dist/$name/"
chmod +x "dist/$name/rsit" "dist/$name/install.sh"
tar -C dist -czf "dist/$name.tar.gz" "$name"
echo "dist/$name.tar.gz"

appimagetool=${APPIMAGETOOL:-$(command -v appimagetool || true)}
if [ -z "$appimagetool" ]; then
  echo "appimagetool not found, skipping AppImage"
  exit 0
fi
appdir="dist/rsit.AppDir"
rm -rf "$appdir"
install -Dm755 "$binary" "$appdir/usr/bin/rsit"
install -Dm644 packaging/rsit.desktop "$appdir/rsit.desktop"
install -Dm644 packaging/rsit.svg "$appdir/rsit.svg"
install -Dm644 packaging/rsit.svg "$appdir/usr/share/icons/hicolor/scalable/apps/rsit.svg"
install -Dm644 NOTICE "$appdir/usr/share/doc/rsit/NOTICE"
cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
exec "$(dirname "$(readlink -f "$0")")/usr/bin/rsit" "$@"
APPRUN
chmod +x "$appdir/AppRun"
# extract-and-run: CI runners have no FUSE to mount appimagetool itself
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$appimagetool" --no-appstream "$appdir" "dist/rsit-$version-x86_64.AppImage"
echo "dist/rsit-$version-x86_64.AppImage"
