#!/bin/sh
set -eu

repository="beharefe/solplay"
version="${SOLPLAY_VERSION:-latest}"
install_dir="${SOLPLAY_INSTALL_DIR:-$HOME/.local/bin}"

os="$(uname -s)"
architecture="$(uname -m)"
case "$os-$architecture" in
  Darwin-arm64) target="aarch64-apple-darwin" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  Linux-x86_64) target="x86_64-unknown-linux-gnu" ;;
  *)
    echo "Unsupported platform: $os $architecture" >&2
    echo "Install with Cargo instead: cargo install --git https://github.com/$repository.git" >&2
    exit 1
    ;;
esac

asset="solplay-$target.tar.gz"
if [ "$version" = "latest" ]; then
  base_url="https://github.com/$repository/releases/latest/download"
else
  base_url="https://github.com/$repository/releases/download/$version"
fi

temporary_directory="$(mktemp -d)"
cleanup() {
  rm -rf "$temporary_directory"
}
trap cleanup EXIT HUP INT TERM

curl --fail --location --silent --show-error "$base_url/$asset" -o "$temporary_directory/$asset"
curl --fail --location --silent --show-error "$base_url/$asset.sha256" -o "$temporary_directory/$asset.sha256"

expected_checksum="$(cut -d ' ' -f 1 "$temporary_directory/$asset.sha256")"
if command -v shasum >/dev/null 2>&1; then
  actual_checksum="$(shasum -a 256 "$temporary_directory/$asset" | cut -d ' ' -f 1)"
else
  actual_checksum="$(sha256sum "$temporary_directory/$asset" | cut -d ' ' -f 1)"
fi
if [ "$expected_checksum" != "$actual_checksum" ]; then
  echo "Checksum verification failed for $asset" >&2
  exit 1
fi

tar -xzf "$temporary_directory/$asset" -C "$temporary_directory"
mkdir -p "$install_dir"
install "$temporary_directory/solplay" "$install_dir/solplay"

echo "Installed solplay to $install_dir/solplay"
case ":$PATH:" in
  *":$install_dir:"*) ;;
  *) echo "Add $install_dir to your PATH, then run: solplay setup" ;;
esac
