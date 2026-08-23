#!/bin/sh
# Builds Biskit MCP from the working tree and replaces the local install.
#
# Runs a release build and copies the resulting binary over the one the published
# installer writes, so the machine's `biskit-mcp` command reflects local changes.
# Nothing is downloaded and no shell profile is touched.
#
# Usage:
#   sh scripts/install-local.sh
#   BISKIT_INSTALL_DIR=/usr/local/bin sh scripts/install-local.sh

set -eu

ROOT="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
INSTALL_DIR="${BISKIT_INSTALL_DIR:-$HOME/.local/bin}"

fail() {
	echo "error: $1" >&2
	exit 1
}

command -v cargo >/dev/null 2>&1 || fail "cargo is required but was not found on PATH."

cd "$ROOT"
cargo build --release

SOURCE="$ROOT/target/release/biskit-mcp"
[ -f "$SOURCE" ] || fail "the build finished but $SOURCE is missing."

mkdir -p "$INSTALL_DIR"
DESTINATION="$INSTALL_DIR/biskit-mcp"

# Writing straight into a running binary fails with ETXTBSY, so stage the copy
# beside it and rename. The rename replaces the directory entry, and any process
# still running the old build keeps the inode it already opened.
STAGED="$DESTINATION.new-$$"
trap 'rm -f "$STAGED"' EXIT INT TERM

cp "$SOURCE" "$STAGED"
chmod 0755 "$STAGED"
mv -f "$STAGED" "$DESTINATION"

echo ""
echo "Installed $("$DESTINATION" --version) to $DESTINATION"

case ":${PATH}:" in
*":${INSTALL_DIR}:"*) ;;
*) echo "Note: ${INSTALL_DIR} is not on your PATH." ;;
esac

echo "Restart any agent holding Biskit open to pick up the new build."
