#!/bin/sh
set -eu

REPO="fluo10/archelon"
INSTALL_DIR="${HOME}/.local/bin"

case "$(uname -s)" in
  Linux*)  OS="linux" ;;
  Darwin*) OS="macos" ;;
  *) echo "error: unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64)        ARCH="x86_64" ;;
  aarch64|arm64) ARCH="aarch64" ;;
  *) echo "error: unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

VERSION=$(curl -sfI "https://github.com/${REPO}/releases/latest" \
  | awk '/^[Ll]ocation:/{print $2}' \
  | tr -d '\r' \
  | sed 's|.*/tag/||')

if [ -z "$VERSION" ]; then
  echo "error: failed to fetch latest version" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"

install_binary() {
  BINARY="$1"
  ASSET="${BINARY}-${OS}-${ARCH}"
  URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}"

  case "$BINARY" in
    archelon-cli) INSTALL_NAME="archelon" ;;
    *)            INSTALL_NAME="$BINARY" ;;
  esac

  printf "Installing %s %s (%s/%s) to %s...\n" "$INSTALL_NAME" "$VERSION" "$OS" "$ARCH" "$INSTALL_DIR"

  TMP=$(mktemp)
  curl -fsSL "$URL" -o "$TMP"
  chmod +x "$TMP"
  mv "$TMP" "${INSTALL_DIR}/${INSTALL_NAME}"

  echo "Done! ${INSTALL_DIR}/${INSTALL_NAME} installed."
}

if [ $# -eq 0 ]; then
  install_binary "archelon-cli"
  install_binary "archelon-mcp"
else
  for BINARY in "$@"; do
    install_binary "$BINARY"
  done
fi

case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    printf "\nNote: %s is not in PATH. Add to your shell profile:\n" "$INSTALL_DIR"
    printf '  export PATH="$HOME/.local/bin:$PATH"\n'
    ;;
esac
