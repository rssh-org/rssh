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

# List the symlinks of the given tree that do not resolve inside it, one per
# line, prefixed with the kind of breakage:
#
#   absolute  the target is absolute and does not exist inside the image
#             (this is the build path artefact of #91)
#   relative  the target is relative and does not resolve
#   self      the link points at itself
#
# Resolution is lexical on purpose: following symlinks would resolve outside
# the image and hide the very breakage we are looking for.
broken_links() {
    local tree="$1" link linkrel target resolved linkpath
    while IFS= read -r link; do
        target="$(readlink "$link")"
        linkrel="${link#"$tree"/}"
        case "$target" in
            /*) resolved="${tree}${target}" ;;
            *)  resolved="${tree}/$(dirname "$linkrel")/${target}" ;;
        esac
        resolved="$(realpath -ms "$resolved")"
        linkpath="$(realpath -ms "$link")"
        if [ "$resolved" = "$linkpath" ]; then
            printf 'self %s\n' "$link"
            continue
        fi
        if [ -e "$resolved" ]; then
            continue
        fi
        case "$target" in
            /*) printf 'absolute %s\n' "$link" ;;
            *)  printf 'relative %s\n' "$link" ;;
        esac
    done < <(find "$tree" -type l -print)
}

( cd "$(dirname "$APPIMAGE")"
  base="$(basename "$APPIMAGE")"
  rm -rf squashfs-root
  "./$base" --appimage-extract >/dev/null

  # The build path artefacts have to become relative: the file they refer to
  # sits next to the symlink inside the AppDir (that is how .DirIcon refers to
  # the icon), so a plain file name is enough. The candidate has to be a plain
  # file: a symlink of the same name would be the link itself, and turning a
  # link into a self-reference would break it. Relative links are left alone,
  # they are not build path artefacts.
  while IFS= read -r entry; do
      case "$entry" in
          "absolute "*) ;;
          *) continue ;;
      esac
      link="${entry#absolute }"
      target="$(readlink "$link")"
      name="$(basename "$target")"
      candidate="$(dirname "$link")/$name"
      if [ "$candidate" != "$link" ] && [ -f "$candidate" ] && [ ! -L "$candidate" ]; then
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
  # not found" (#91, #318). Absolute links and self-references are fatal,
  # dangling relative links are reported but do not block the release: an
  # AppImage may carry them without breaking the installation.
  rm -rf squashfs-root
  "./$base" --appimage-extract >/dev/null
  fatal=""
  while IFS= read -r entry; do
      case "$entry" in
          "relative "*)
              link="${entry#relative }"
              echo "WARNING: $link -> $(readlink "$link") does not resolve inside the AppImage" >&2
              continue ;;
      esac
      link="${entry#* }"
      fatal="${fatal}${entry}"$'\n'
  done < <(broken_links squashfs-root)
  if [ -n "$fatal" ]; then
      echo "ERROR: $base still contains unresolvable symlinks:" >&2
      printf '%s' "$fatal" >&2
      exit 1
  fi
  rm -rf squashfs-root )

echo "Fixed $(basename "$APPIMAGE")"
