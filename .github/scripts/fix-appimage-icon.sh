#!/usr/bin/env bash
# Rewrite an AppImage's broken symlinks and repack it.
#
# linuxdeploy bakes .DirIcon as an ABSOLUTE symlink into the build machine's
# path (e.g. /home/runner/work/rssh/rssh/.../RSSH.AppDir/RSSH.png). That path
# does not exist inside the AppImage, so tools that unpack/validate it
# (AppManager, `--appimage-extract`) fail with "Symlink target not found". The
# icon file itself sits next to the symlink inside the AppDir, so a relative
# link is all we need. (issue #91)
#
# A symlink with an absolute target is only broken when that target does not
# resolve inside the AppImage; absolute links into the image itself (e.g.
# /usr/lib/...) stay untouched.
#
# appimagetool refuses to build an AppImage from an AppDir whose root directory
# is not readable by other users (mode 0700 -> "found 1 ERROR(S) in the
# AppDir"), and the extracted tree inherits that mode. The tree therefore has to
# be chmod'ed before it is repacked. The repack is verified afterwards: the
# published AppImage must not contain a single symlink that fails to resolve
# inside it. (issue #318)
#
# Usage: fix-appimage-icon.sh <path-to.AppImage> [arch]
#   arch defaults to the host arch (uname -m); pass x86_64 / aarch64 to override.
set -euo pipefail

APPIMAGE="${1:?usage: fix-appimage-icon.sh <path-to.AppImage> [arch]}"
case "${2:-$(uname -m)}" in
    aarch64|arm64) AI_ARCH="aarch64" ;;
    *)             AI_ARCH="x86_64" ;;
esac

# appimagetool repacks the squashfs. --appimage-extract-and-run keeps it
# self-contained (bundled mksquashfs, no FUSE) so it works on bare CI runners.
APPIMAGETOOL="$HOME/.cache/tauri/appimagetool-$AI_ARCH.AppImage"
if [ ! -x "$APPIMAGETOOL" ]; then
    mkdir -p "$(dirname "$APPIMAGETOOL")"
    curl -fsSL -o "$APPIMAGETOOL" \
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$AI_ARCH.AppImage"
    chmod +x "$APPIMAGETOOL"
fi

# Absolutize paths so the cd into the bundle dir below stays valid.
APPIMAGE="$(cd "$(dirname "$APPIMAGE")" && pwd)/$(basename "$APPIMAGE")"
APPIMAGETOOL="$(cd "$(dirname "$APPIMAGETOOL")" && pwd)/$(basename "$APPIMAGETOOL")"

# Print the symlinks of the given tree that point at a path which does not
# exist inside it. Those are the broken ones.
broken_links() {
    local tree="$1" link target
    while IFS= read -r link; do
        target="$(readlink "$link")"
        case "$target" in
            /*) ;;
            *) continue ;;
        esac
        if [ -e "${tree}${target}" ]; then
            continue
        fi
        printf '%s\n' "$link"
    done < <(find "$tree" -type l -print)
}

( cd "$(dirname "$APPIMAGE")"
  base="$(basename "$APPIMAGE")"
  rm -rf squashfs-root
  "./$base" --appimage-extract >/dev/null

  # Every broken symlink has to become relative: the file it refers to sits
  # next to the symlink inside the AppDir (that is how .DirIcon refers to the
  # icon), so a plain file name is enough.
  while IFS= read -r link; do
      target="$(readlink "$link")"
      name="$(basename "$target")"
      if [ -e "$(dirname "$link")/$name" ]; then
          ln -sf "$name" "$link"
      else
          echo "ERROR: cannot relativise $link -> $target" >&2
          exit 1
      fi
  done < <(broken_links squashfs-root)

  # appimagetool rejects an AppDir that other users cannot read (mode 0700),
  # which is what the extracted tree inherits from the AppImage.
  chmod -R a+rX squashfs-root

  ARCH="$AI_ARCH" "$APPIMAGETOOL" --appimage-extract-and-run squashfs-root "$base"
  rm -rf squashfs-root

  # Post-condition: the repacked AppImage must not contain a symlink that
  # fails to resolve inside it, otherwise unpackers fail with "Symlink target
  # not found" (#91, #318).
  rm -rf squashfs-root
  "./$base" --appimage-extract >/dev/null
  broken="$(broken_links squashfs-root)"
  if [ -n "$broken" ]; then
      echo "ERROR: $base still contains unresolvable symlinks:" >&2
      while IFS= read -r link; do
          printf '  %s -> %s\n' "$link" "$(readlink "$link")" >&2
      done <<< "$broken"
      exit 1
  fi
  rm -rf squashfs-root )

echo "Fixed $(basename "$APPIMAGE")"
