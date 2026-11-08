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
# be chmod'ed before it is repacked. The repack is verified afterwards:
# escaping links, self-references and unresolved absolute links are fatal;
# dangling relative links inside the image are warnings. (issue #318)
#
# Usage: fix-appimage-icon.sh <path-to.AppImage> [arch]
#   arch defaults to the host arch (uname -m); pass x86_64 / aarch64 to override.
#   APPIMAGETOOL can select an existing appimagetool executable.
set -euo pipefail

APPIMAGE="${1:?usage: fix-appimage-icon.sh <path-to.AppImage> [arch]}"
case "${2:-$(uname -m)}" in
    aarch64|arm64) AI_ARCH="aarch64" ;;
    *)             AI_ARCH="x86_64" ;;
esac

# appimagetool repacks the squashfs. --appimage-extract-and-run keeps it
# self-contained (bundled mksquashfs, no FUSE) so it works on bare CI runners.
APPIMAGETOOL="${APPIMAGETOOL:-$HOME/.cache/tauri/appimagetool-$AI_ARCH.AppImage}"
if [ ! -x "$APPIMAGETOOL" ]; then
    mkdir -p "$(dirname "$APPIMAGETOOL")"
    curl -fsSL -o "$APPIMAGETOOL" \
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$AI_ARCH.AppImage"
    chmod +x "$APPIMAGETOOL"
fi

# Absolutize paths so the cd into the bundle dir below stays valid.
APPIMAGE="$(cd "$(dirname "$APPIMAGE")" && pwd)/$(basename "$APPIMAGE")"
APPIMAGETOOL="$(cd "$(dirname "$APPIMAGETOOL")" && pwd)/$(basename "$APPIMAGETOOL")"

# List problematic symlinks as NUL-delimited records prefixed with their kind:
#
#   absolute  the target is absolute and does not exist inside the image
#             (this is the build path artefact of #91)
#   relative  the target is relative and does not resolve
#   self      the link points at itself
#   outside   the target escapes the image, whether or not it exists
#
# Check both the lexical path and resolution through directory symlinks.
# The second argument is a scratch file outside the AppDir. Complete find
# before reading it so a failed scan cannot look like an empty result.
broken_links() {
    local tree="$1" links="$2" link target targetpath resolved
    tree="$(realpath -m "$tree")" || return
    find "$tree" -type l -print0 > "$links" || return
    while IFS= read -r -d '' link; do
        target="$(readlink "$link")" || return
        case "$target" in
            /*) targetpath="${tree}${target}" ;;
            *)  targetpath="${link%/*}/${target}" ;;
        esac
        resolved="$(realpath -ms "$targetpath")" || return
        if [ "$resolved" = "$link" ]; then
            printf 'self %s\0' "$link"
            continue
        fi
        case "$resolved" in
            "$tree"|"$tree"/*) ;;
            *) printf 'outside %s\0' "$link"; continue ;;
        esac
        # Resolve the original path: normalizing away '..' first would hide
        # traversal through a symlink such as sub/parent -> .. .
        resolved="$(realpath -m "$targetpath")" || return
        case "$resolved" in
            "$tree"|"$tree"/*) ;;
            *) printf 'outside %s\0' "$link"; continue ;;
        esac
        if [ -e "$resolved" ]; then
            continue
        fi
        case "$target" in
            /*) printf 'absolute %s\0' "$link" ;;
            *)  printf 'relative %s\0' "$link" ;;
        esac
    done < "$links"
}

( cd "$(dirname "$APPIMAGE")"
  base="$(basename "$APPIMAGE")"
  scan_dir="$(mktemp -d)"
  trap 'rm -rf "$scan_dir"' EXIT
  rm -rf squashfs-root
  "./$base" --appimage-extract >/dev/null

  # The build path artefacts have to become relative: the file they refer to
  # sits next to the symlink inside the AppDir (that is how .DirIcon refers to
  # the icon), so a plain file name is enough. The candidate has to be a plain
  # file: a symlink of the same name would be the link itself, and turning a
  # link into a self-reference would break it. Relative links are left alone,
  # they are not build path artefacts.
  broken_links squashfs-root "$scan_dir/links" > "$scan_dir/broken"
  while IFS= read -r -d '' entry; do
      case "$entry" in
          "absolute "*) ;;
          "relative "*) continue ;;
          *) echo "ERROR: unsafe AppImage symlink: $entry" >&2; exit 1 ;;
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
  done < "$scan_dir/broken"

  # appimagetool rejects an AppDir that other users cannot read (mode 0700),
  # which is what the extracted tree inherits from the AppImage.
  chmod -R a+rX squashfs-root

  ARCH="$AI_ARCH" "$APPIMAGETOOL" --appimage-extract-and-run squashfs-root "$base"
  rm -rf squashfs-root

  # Validate the repacked image too. Run the scan as a normal command so its
  # failure stops the script, rather than disappearing in process substitution.
  "./$base" --appimage-extract >/dev/null
  broken_links squashfs-root "$scan_dir/links" > "$scan_dir/broken"
  fatal=""
  while IFS= read -r -d '' entry; do
      case "$entry" in
          "relative "*)
              link="${entry#relative }"
              echo "WARNING: $link -> $(readlink "$link") does not resolve inside the AppImage" >&2
              continue ;;
      esac
      fatal="${fatal}${entry}"$'\n'
  done < "$scan_dir/broken"
  if [ -n "$fatal" ]; then
      echo "ERROR: $base still contains unresolvable symlinks:" >&2
      printf '%s' "$fatal" >&2
      exit 1
  fi
  rm -rf squashfs-root )

echo "Fixed $(basename "$APPIMAGE")"
