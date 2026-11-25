#!/usr/bin/env bash
# Install the published desktop application. Compatible with macOS Bash 3.2.
set -euo pipefail
umask 022

repository='https://github.com/godstealer/http-capture'
tag='latest'
prefix=''
work=''
payload=''
launcher_tmp=''
desktop_tmp=''
backup=''
app=''

say() { printf '%s\n' "$*"; }
die() { printf 'HTTP Capture: %s\n' "$*" >&2; exit 1; }
usage() {
  cat <<'HELP'
Install HTTP Capture for Linux x64 or macOS (Apple Silicon / Intel).

Usage: bash install.sh [--tag build-<12 hex digits>] [--prefix ABSOLUTE_DIR]

  --tag TAG     Install an immutable release tag (default: latest stable).
  --prefix DIR  Linux: prefix for bin/, lib/ and share/ (default: ~/.local).
                macOS: applications directory (default: ~/Applications).
  -h, --help    Show this help.

Requires Bash, curl, tar, awk and sha256sum (Linux) or shasum (macOS).
Installs for the current user; no sudo, proxy or certificate changes.
Close the application before updating. Linux portable updates: rerun this script.
HELP
}
cleanup() {
  # Each removable directory was created by this invocation with mktemp.
  if [ -n "$backup" ] && [ ! -e "$app" ]; then
    mv "$backup/HTTP Capture.app" "$app" || true
  fi
  [ -z "$launcher_tmp" ] || rm -f -- "$launcher_tmp"
  [ -z "$desktop_tmp" ] || rm -f -- "$desktop_tmp"
  [ -z "$payload" ] || rm -rf -- "$payload"
  [ -z "$backup" ] || rmdir "$backup" 2>/dev/null || true
  [ -z "$work" ] || rm -rf -- "$work"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

while [ "$#" -gt 0 ]; do
  case "$1" in
    --tag|--prefix)
      [ "$#" -ge 2 ] && [ -n "$2" ] || die "Missing value for $1."
      case "$1" in --tag) tag=$2 ;; --prefix) prefix=$2 ;; esac
      shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) die "Unknown option: $1 (see --help)." ;;
  esac
done
if [ "$tag" != latest ]; then
  [[ "$tag" =~ ^build-[0-9a-f]{12}$ ]] || die 'Expected --tag build-<12 lowercase hex digits>.'
fi

os=$(uname -s)
arch=$(uname -m)
case "$os:$arch" in
  Linux:x86_64|Linux:amd64) label=linux-x64 ;;
  Darwin:arm64|Darwin:aarch64) label=macos-arm64 ;;
  Darwin:x86_64)
    # A terminal running under Rosetta still needs the native Apple Silicon app.
    if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then
      label=macos-arm64
    else
      label=macos-x64
    fi ;;
  *) die "No published install.sh package for $os / $arch. Windows: use the Release .exe installer." ;;
esac
for utility in curl tar awk mktemp find readlink; do
  command -v "$utility" >/dev/null 2>&1 || die "Required command not found: $utility."
done
if command -v sha256sum >/dev/null 2>&1; then
  hash_command=sha256sum
elif command -v shasum >/dev/null 2>&1; then
  hash_command=shasum
else
  die 'Install sha256sum or shasum before running this script.'
fi
[ -n "${HOME:-}" ] || die 'HOME is not set.'
if [ -z "$prefix" ]; then
  case "$label" in linux-*) prefix="$HOME/.local" ;; *) prefix="$HOME/Applications" ;; esac
fi
case "$prefix" in /*) ;; *) die '--prefix must be an absolute path.' ;; esac
# These characters cannot be represented safely in a launcher/desktop entry.
case "$prefix" in *$'\n'*|*$'\r'*|*$'\t'*) die '--prefix must not contain tabs or line breaks.' ;; esac
while [ "${prefix%/}" != "$prefix" ]; do prefix=${prefix%/}; done
[ -n "$prefix" ] || die 'Refusing to use the filesystem root as --prefix.'

if [ "$tag" = latest ]; then
  say 'Finding the latest stable release...'
  resolved=$(curl --fail --silent --show-error --location --head --retry 3 \
    --connect-timeout 15 --max-time 120 --proto '=https' --proto-redir '=https' \
    --output /dev/null --write-out '%{url_effective}' "$repository/releases/latest")
  case "$resolved" in "$repository/releases/tag/"*) tag=${resolved#"$repository/releases/tag/"} ;;
    *) die 'Latest release did not resolve to this GitHub repository.' ;; esac
  [[ "$tag" =~ ^build-[0-9a-f]{12}$ ]] || die 'Latest is not a supported stable build tag.'
fi
say "Installing $label from $tag..."
work=$(mktemp -d "${TMPDIR:-/tmp}/http-capture-install.XXXXXXXX")
download() {
  curl --fail --silent --show-error --location --retry 3 --connect-timeout 15 \
    --max-time 600 --proto '=https' --proto-redir '=https' --output "$2" "$1"
}
base="$repository/releases/download/$tag"
download "$base/SHA256SUMS-$label.txt" "$work/checksums"
# Only select a single known asset basename. No JSON parser or GitHub token needed.
selection=$(awk -v label="$label" '
  NF == 2 && length($1) == 64 && $1 !~ /[^0-9a-f]/ {
    name=$2
    if ((label == "linux-x64" && name ~ /^linux-x64-HTTP-Capture_[0-9]+\.[0-9]+\.[0-9]+_amd64-portable\.tar\.gz$/) ||
        (label != "linux-x64" && name == label "-HTTP-Capture.app.tar.gz")) {
      count++; selected=$1 " " name
    }
  }
  END { if (count != 1) exit 1; print selected }
' "$work/checksums") || die 'The release must contain exactly one matching package and SHA256 checksum.'
expected=${selection%% *}
asset=${selection#* }
say "Downloading $asset..."
download "$base/$asset" "$work/package.tar.gz"
if [ "$hash_command" = sha256sum ]; then
  actual=$(sha256sum "$work/package.tar.gz")
else
  actual=$(shasum -a 256 "$work/package.tar.gz")
fi
actual=${actual%% *}
[ "$actual" = "$expected" ] || die 'SHA256 mismatch; the existing installation was not changed.'
say 'SHA256 verified.'

case "$label" in linux-*) archive_root=HTTP-Capture ;; *) archive_root='HTTP Capture.app' ;; esac
tar -tzf "$work/package.tar.gz" > "$work/members" || die 'Cannot read the release archive.'
# Refuse absolute/traversing members and unexpected top-level paths before extraction.
awk -v root="$archive_root" '
  { if ($0 != root && index($0, root "/") != 1) exit 1
    n=split($0, parts, "/"); for(i=1;i<=n;i++) if(parts[i] == "..") exit 1
    count++ }
  END { if (!count) exit 1 }
' "$work/members" || die 'Unsafe or unexpected archive layout.'

validate_extracted_links() {
  [ -d "$payload/$archive_root" ] && [ ! -L "$payload/$archive_root" ] || die 'Archive root must be an ordinary directory.'
  physical_root=$(cd -P "$payload/$archive_root" && pwd -P)
  # Keep AppDir relative library symlinks, but do not install links outside its tree.
  find "$payload" -type l -print0 | while IFS= read -r -d '' link; do
    target=$(readlink "$link") || die 'Cannot read an archive symlink.'
    case "$target" in /*|*$'\n'*|*$'\r'*) die 'Unsafe archive symlink target.' ;; esac
    relative=${link#"$payload/"}
    relative=${relative%/*}
    awk 'BEGIN {
      n=split(ARGV[1], p, "/"); depth=0
      for(i=1;i<=n;i++) {
        if(p[i] == "" || p[i] == ".") continue
        if(p[i] == "..") { if(depth == 0) exit 1; depth-- }
        else stack[++depth]=p[i]
      }
      if(depth < 1 || stack[1] != ARGV[2]) exit 1
      exit 0
    }' "$relative/$target" "$archive_root" || die 'Archive symlink escapes its application directory.'
    # Also resolve intermediate directory links physically: nested -> . followed
    # by nested/.. escapes the root despite a safe-looking lexical path.
    if [ -d "$link" ]; then
      resolved=$(cd -P "$link" && pwd -P) || die 'Cannot resolve archive directory link.'
    else
      case "$target" in */*) parent=${target%/*} ;; *) parent=. ;; esac
      resolved=$(cd -P "${link%/*}/$parent" && pwd -P) || die 'Cannot resolve archive symlink parent.'
    fi
    case "$resolved" in "$physical_root"|"$physical_root/"*) ;;
      *) die 'Archive symlink resolves outside its application directory.' ;; esac
  done
}

if [ "$label" = linux-x64 ]; then
  releases="$prefix/lib/http-capture/releases"
  mkdir -p "$releases" "$prefix/bin" "$prefix/share/applications"
  # Keep each complete portable tree together. A previous running version is untouched.
  payload=$(mktemp -d "$releases/$tag.XXXXXXXX")
  tar -xzf "$work/package.tar.gz" -C "$payload" --no-same-owner --no-same-permissions
  validate_extracted_links
  entry="$payload/HTTP-Capture/AppRun"
  [ -f "$entry" ] && [ ! -L "$entry" ] && [ -x "$entry" ] || die 'The Linux package has no executable AppRun.'
  launcher="$prefix/bin/http-capture"
  [ ! -d "$launcher" ] || die "Launcher path is a directory: $launcher"
  launcher_tmp=$(mktemp "$prefix/bin/.http-capture.XXXXXXXX")
  # %q generates a Bash-safe path, including spaces, quotes and shell metacharacters.
  printf '#!/usr/bin/env bash\nexec %q "$@"\n' "$entry" > "$launcher_tmp"
  chmod 755 "$launcher_tmp"
  desktop="$prefix/share/applications/dev.httpcapture.desktop.desktop"
  [ ! -d "$desktop" ] || die "Desktop entry path is a directory: $desktop"
  desktop_tmp=$(mktemp "$prefix/share/applications/.http-capture.XXXXXXXX")
  # Desktop Exec values have their own escaping in addition to string escaping.
  desktop_exec=${launcher//\\/\\\\\\\\}
  desktop_exec=${desktop_exec//\"/\\\\\"}
  desktop_exec=${desktop_exec//\$/\\\\\$}
  desktop_exec=${desktop_exec//\`/\\\\\`}
  desktop_exec=${desktop_exec//%/%%}
  printf '[Desktop Entry]\nType=Application\nName=HTTP Capture\nComment=HTTP and HTTPS debugging proxy\nExec="%s"\nIcon=network-workgroup\nTerminal=false\nCategories=Development;Network;\n' "$desktop_exec" > "$desktop_tmp"
  chmod 644 "$desktop_tmp"
  mv -f "$desktop_tmp" "$desktop"; desktop_tmp=''
  mv -f "$launcher_tmp" "$launcher"; launcher_tmp=''
  payload=''
  say "Installed. Launch from the applications menu or run: $launcher"
  case ":${PATH:-}:" in *":$prefix/bin:"*) ;; *) say "To use the command by name, add $prefix/bin to your PATH." ;; esac
  say "To update this portable installation, rerun install.sh. Previous release directories remain in $releases."
else
  command -v codesign >/dev/null 2>&1 || die 'macOS codesign is required to verify the application.'
  mkdir -p "$prefix"
  payload=$(mktemp -d "$prefix/.http-capture-install.XXXXXXXX")
  tar -xzf "$work/package.tar.gz" -C "$payload" --no-same-owner --no-same-permissions
  validate_extracted_links
  incoming="$payload/HTTP Capture.app"
  [ -x "$incoming/Contents/MacOS/http-capture" ] || die 'The macOS package has no executable application.'
  codesign --verify --deep --strict "$incoming" || die 'macOS application signature verification failed; existing app was not changed.'
  app="$prefix/HTTP Capture.app"
  if [ -e "$app" ] || [ -L "$app" ]; then
    [ -d "$app" ] && [ ! -L "$app" ] || die "Refusing to replace an unexpected file or symlink: $app"
    # Retain the previous bundle; the EXIT trap restores it if moving the new one fails.
    backup=$(mktemp -d "$prefix/.http-capture-previous.XXXXXXXX")
    mv "$app" "$backup/HTTP Capture.app"
  fi
  mv "$incoming" "$app"
  if [ -n "$backup" ]; then
    say "Previous application retained at: $backup/HTTP Capture.app"
    backup=''
  fi
  say "Installed: $app"
  say 'Open HTTP Capture in Finder. macOS security and certificate trust settings are unchanged.'
fi
