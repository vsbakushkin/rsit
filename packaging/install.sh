#!/bin/sh
# Installs rsit for the current user: the binary into ~/.local/bin and the
# desktop entry into ~/.local/share/applications. PREFIX=/usr/local installs
# system-wide (run with sudo).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}
install -Dm755 "$here/rsit" "$prefix/bin/rsit"
install -Dm644 "$here/rsit.desktop" "$prefix/share/applications/rsit.desktop"
echo "Installed $prefix/bin/rsit"
case ":$PATH:" in
  *":$prefix/bin:"*) ;;
  *) echo "Note: $prefix/bin is not in PATH" ;;
esac
