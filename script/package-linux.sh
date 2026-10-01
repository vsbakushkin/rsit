#!/bin/sh
# Packs a release build into dist/rsit-VERSION-x86_64-linux.tar.gz.
# Usage: script/package-linux.sh VERSION [BINARY]
set -eu
version=$1
binary=${2:-target/release/rsit}
name="rsit-$version-x86_64-linux"
rm -rf "dist/$name"
mkdir -p "dist/$name"
cp "$binary" "dist/$name/rsit"
cp packaging/rsit.desktop packaging/install.sh NOTICE "dist/$name/"
chmod +x "dist/$name/rsit" "dist/$name/install.sh"
tar -C dist -czf "dist/$name.tar.gz" "$name"
echo "dist/$name.tar.gz"
