#!/bin/sh
# Installs Biskit MCP.
#
# Downloads a release, verifies its SHA-256 digest against the published
# SHA256SUMS file, and installs it into ~/.local/bin.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/doctr-oof/biskit-mcp/main/install.sh | sh
#   BISKIT_VERSION=v0.1.0 BISKIT_INSTALL_DIR=/usr/local/bin sh install.sh
#
# After installing, the script offers to register Biskit in a project. Every write is
# confirmed first, and the offer is skipped entirely when no terminal is attached.
#
# To drive that step without prompts, for example from a provisioning script:
#   BISKIT_SETUP_CLIENTS="claude,cursor" BISKIT_SETUP_HOOKS=1 sh install.sh
#
#   BISKIT_SETUP_CLIENTS       claude, cursor, vscode; comma or space separated
#   BISKIT_SETUP_PROJECT       project directory, defaults to the working directory
#   BISKIT_SETUP_HOOKS         1 to add the Claude Code SessionStart hook
#   BISKIT_SETUP_HOOKS_TARGET  local (default) or shared
#   BISKIT_SETUP_FROM_CWD      1 (default) pins the registration with --project-from-cwd
#   BISKIT_NO_SETUP            1 skips the whole step

set -eu

REPOSITORY="doctr-oof/biskit-mcp"
VERSION="${BISKIT_VERSION:-latest}"
INSTALL_DIR="${BISKIT_INSTALL_DIR:-$HOME/.local/bin}"

fail() {
	echo "error: $1" >&2
	exit 1
}

need() {
	command -v "$1" >/dev/null 2>&1 || fail "$1 is required but was not found on PATH."
}

need curl
need tar

detect_asset() {
	os="$(uname -s)"
	arch="$(uname -m)"

	case "$os" in
	Darwin) platform="macos" ;;
	Linux) platform="linux" ;;
	*) fail "unsupported operating system: $os" ;;
	esac

	case "$arch" in
	x86_64 | amd64) cpu="x86_64" ;;
	arm64 | aarch64) cpu="aarch64" ;;
	*) fail "unsupported architecture: $arch" ;;
	esac

	echo "biskit-mcp-${platform}-${cpu}.tar.gz"
}

resolve_tag() {
	if [ "$VERSION" != "latest" ]; then
		echo "$VERSION"
		return
	fi
	curl -fsSL "https://api.github.com/repos/${REPOSITORY}/releases/latest" |
		sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
		head -n 1
}

checksum_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d' ' -f1
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 "$1" | cut -d' ' -f1
	else
		fail "neither sha256sum nor shasum is available; cannot verify the download."
	fi
}

ASSET="$(detect_asset)"
TAG="$(resolve_tag)"
[ -n "$TAG" ] || fail "could not determine the release tag to install."
BASE_URL="https://github.com/${REPOSITORY}/releases/download/${TAG}"

WORKSPACE="$(mktemp -d)"
trap 'rm -rf "$WORKSPACE"' EXIT INT TERM

echo "Downloading ${ASSET} (${TAG})..."
curl -fsSL "${BASE_URL}/${ASSET}" -o "${WORKSPACE}/${ASSET}"
curl -fsSL "${BASE_URL}/SHA256SUMS" -o "${WORKSPACE}/SHA256SUMS"

EXPECTED="$(grep " ${ASSET}\$" "${WORKSPACE}/SHA256SUMS" | cut -d' ' -f1 | head -n 1)"
[ -n "$EXPECTED" ] || fail "SHA256SUMS does not list ${ASSET}. Refusing to install."

ACTUAL="$(checksum_of "${WORKSPACE}/${ASSET}")"
if [ "$EXPECTED" != "$ACTUAL" ]; then
	fail "checksum mismatch for ${ASSET}: expected ${EXPECTED}, got ${ACTUAL}. The download was discarded."
fi
echo "Checksum verified."

mkdir -p "$INSTALL_DIR"
tar -xzf "${WORKSPACE}/${ASSET}" -C "$INSTALL_DIR" biskit-mcp
chmod 0755 "${INSTALL_DIR}/biskit-mcp"

echo ""
echo "Biskit installed to ${INSTALL_DIR}/biskit-mcp"

case ":${PATH}:" in
*":${INSTALL_DIR}:"*) ;;
*) echo "Note: ${INSTALL_DIR} is not on your PATH. Add it to your shell profile." ;;
esac

BIN="${INSTALL_DIR}/biskit-mcp"

manual_steps() {
	echo ""
	echo "Register it with your agent, for example:"
	echo "  claude mcp add biskit -- ${BIN} start"
	echo ""
	echo "Or register it inside a project at any time:"
	echo "  cd /path/to/project && ${BIN} setup --client claude --hooks"
}

# stdin carries the script itself under `curl | sh`, so prompts must read the terminal directly.
can_prompt() {
	# A failed redirection on a special built-in ends the shell, so probe in a subshell.
	(: </dev/tty) 2>/dev/null || return 1
	(: >/dev/tty) 2>/dev/null || return 1
	return 0
}

prompt_yn() {
	if [ "$2" = "y" ]; then
		hint="Y/n"
	else
		hint="y/N"
	fi
	while :; do
		printf '%s [%s]: ' "$1" "$hint" >/dev/tty
		reply=""
		read -r reply </dev/tty || reply=""
		[ -n "$reply" ] || reply="$2"
		case "$reply" in
		y | Y | yes | Yes | YES) return 0 ;;
		n | N | no | No | NO) return 1 ;;
		*) echo "Please enter y or n." >/dev/tty ;;
		esac
	done
}

prompt_path() {
	while :; do
		printf '%s [%s]: ' "$1" "$2" >/dev/tty
		reply=""
		read -r reply </dev/tty || reply=""
		[ -n "$reply" ] || reply="$2"
		if [ -d "$reply" ]; then
			printf '%s' "$reply"
			return 0
		fi
		echo "Not a directory: $reply" >/dev/tty
	done
}

if [ "${BISKIT_NO_SETUP:-0}" = "1" ]; then
	manual_steps
	exit 0
fi

if [ -n "${BISKIT_SETUP_CLIENTS:-}" ] || [ "${BISKIT_SETUP_HOOKS:-0}" = "1" ]; then
	set -- setup --project "${BISKIT_SETUP_PROJECT:-$PWD}"
	for client in $(printf '%s' "${BISKIT_SETUP_CLIENTS:-}" | tr ',' ' '); do
		set -- "$@" --client "$client"
	done
	if [ "${BISKIT_SETUP_HOOKS:-0}" = "1" ]; then
		set -- "$@" --hooks --hooks-target "${BISKIT_SETUP_HOOKS_TARGET:-local}"
	fi
	if [ "${BISKIT_SETUP_FROM_CWD:-1}" = "1" ]; then
		set -- "$@" --project-from-cwd
	fi
	echo ""
	"$BIN" "$@"
	exit 0
fi

if ! can_prompt; then
	manual_steps
	exit 0
fi

echo ""
if ! prompt_yn "Register Biskit in a project now?" n; then
	manual_steps
	exit 0
fi

SETUP_PROJECT="$(prompt_path "Project directory" "$PWD")"
set -- setup --project "$SETUP_PROJECT"
SELECTED=0

if prompt_yn "  Write .mcp.json (Claude Code)?" y; then
	set -- "$@" --client claude
	SELECTED=1
fi
if prompt_yn "  Write .cursor/mcp.json (Cursor)?" n; then
	set -- "$@" --client cursor
	SELECTED=1
fi
if prompt_yn "  Write .vscode/mcp.json (VS Code)?" n; then
	set -- "$@" --client vscode
	SELECTED=1
fi
if prompt_yn "  Add the Claude Code SessionStart hook to .claude/settings.local.json?" n; then
	set -- "$@" --hooks
	SELECTED=1
fi

if [ "$SELECTED" = "0" ]; then
	echo ""
	echo "Nothing selected, so nothing was written."
	manual_steps
	exit 0
fi

if prompt_yn "  Pin the registration to this project (--project-from-cwd)?" y; then
	set -- "$@" --project-from-cwd
fi

echo ""
"$BIN" "$@"
