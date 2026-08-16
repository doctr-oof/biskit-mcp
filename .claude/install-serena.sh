#!/usr/bin/env bash
set -euo pipefail

# ─── Helpers ─────────────────────────────────────────────────────────────────

detect_os() {
    local uname_out
    uname_out="$(uname -s)"
    case "$uname_out" in
        Darwin*)              echo "mac"     ;;
        Linux*)               echo "linux"   ;;
        MINGW*|MSYS*|CYGWIN*) echo "windows" ;;
        *)                    echo "unknown" ;;
    esac
}

prompt_yn() {
    local question="$1"
    local default="${2:-n}"
    local prompt_hint
    [[ "$default" == "y" ]] && prompt_hint="Y/n" || prompt_hint="y/N"
    local answer
    while true; do
        read -rp "$question [$prompt_hint]: " answer </dev/tty
        answer="${answer:-$default}"
        case "$answer" in
            y|Y|yes|Yes|YES) echo "y"; return ;;
            n|N|no|No|NO)    echo "n"; return ;;
            *) echo "Please enter y or n." >&2 ;;
        esac
    done
}

find_python() {
    for cmd in python3 python; do
        if command -v "$cmd" >/dev/null 2>&1; then
            echo "$cmd"
            return
        fi
    done
    echo ""
}

run_python() {
    local tmpfile
    tmpfile="$(mktemp)"
    cat > "$tmpfile"
    "$PY" "$tmpfile" "$@"
    local status=$?
    rm -f "$tmpfile"
    return $status
}

ensure_gitignore_entry() {
    local file="$1"
    local entry="$2"

    if [[ ! -f "$file" ]]; then
        printf '%s\n' "$entry" > "$file"
        echo "    Created .gitignore with '$entry'."
        return
    fi

    if grep -qxF "$entry" "$file" 2>/dev/null; then
        echo "    '$entry' already in .gitignore, skipping."
        return
    fi

    run_python "$file" "$entry" << 'PYEOF'
import sys
path, entry = sys.argv[1], sys.argv[2]
with open(path, "rb") as f:
    content = f.read()
if content and content[-1:] != b"\n":
    content += b"\n"
content += (entry + "\n").encode()
with open(path, "wb") as f:
    f.write(content)
PYEOF
    echo "    Added '$entry' to .gitignore."
}

# ─── Step 0: CWD check ───────────────────────────────────────────────────────

DIRNAME="$(basename "$PWD")"
if [[ "$DIRNAME" != ".claude" ]]; then
    echo "WARNING: Expected to run from inside a '.claude' folder."
    echo "         Current directory: $PWD"
    answer="$(prompt_yn "Continue anyway?" n)"
    if [[ "$answer" != "y" ]]; then
        echo "Aborted."
        exit 1
    fi
fi

PROJECT_DIR="$(cd .. && pwd)"

# ─── Find Python ─────────────────────────────────────────────────────────────

PY="$(find_python)"
if [[ -z "$PY" ]]; then
    echo "ERROR: No Python interpreter found (tried python3, python). Install Python 3 and re-run."
    exit 1
fi

OS="$(detect_os)"
echo ""
echo "OS:             $OS"
echo "Project folder: $PROJECT_DIR"
echo ""

# ─── Step 1: Install uv ──────────────────────────────────────────────────────

echo "==> Step 1: Install uv"
if command -v uv >/dev/null 2>&1; then
    echo "    uv already installed, skipping."
else
    if [[ "$OS" == "windows" ]]; then
        powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"
        export PATH="$USERPROFILE/.local/bin:$PATH"
    else
        curl -LsSf https://astral.sh/uv/install.sh | sh
        export PATH="$HOME/.local/bin:$PATH"
    fi
fi

# ─── Step 2: Install Serena fork ─────────────────────────────────────────────

echo ""
echo "==> Step 2: Install Serena MCP fork"
if command -v uv >/dev/null 2>&1; then
    uv tool install git+https://github.com/Sawhorse-Interactive/serena-carpenter-luau-lsp --force
else
    echo "    NOTE: 'uv' is not yet on PATH in this shell session."
    echo "    Open a new terminal and run:"
    echo "      uv tool install git+https://github.com/Sawhorse-Interactive/serena-carpenter-luau-lsp"
    echo "    Then re-run this script if any steps below fail."
fi

# ─── Step 3: .mcp.json ───────────────────────────────────────────────────────

echo ""
echo "==> Step 3: Configure .mcp.json"
MCP_JSON="$PROJECT_DIR/.mcp.json"

if [[ ! -f "$MCP_JSON" ]]; then
    echo "    Creating $MCP_JSON..."
    cat > "$MCP_JSON" << 'MCPEOF'
{
    "mcpServers": {
        "serena": {
            "type": "stdio",
            "command": "serena",
            "args": [
                "start-mcp-server",
                "--context",
                "claude-code",
                "--project-from-cwd"
            ],
            "env": {}
        }
    }
}
MCPEOF
else
    echo "    Merging serena into existing $MCP_JSON..."
    run_python "$MCP_JSON" << 'PYEOF'
import sys, json

path = sys.argv[1]
with open(path) as f:
    data = json.load(f)

servers = data.setdefault("mcpServers", {})
if "serena" not in servers:
    servers["serena"] = {
        "type": "stdio",
        "command": "serena",
        "args": ["start-mcp-server", "--context", "claude-code", "--project-from-cwd"],
        "env": {}
    }
    print("    Added 'serena' to mcpServers.")
else:
    print("    'serena' already present, leaving untouched.")

with open(path, "w") as f:
    json.dump(data, f, indent=4)
    f.write("\n")
PYEOF
fi

# ─── Step 4: settings.local.json ─────────────────────────────────────────────

echo ""
echo "==> Step 4: Configure settings.local.json"
SETTINGS_LOCAL="$PWD/settings.local.json"

if [[ ! -f "$SETTINGS_LOCAL" ]]; then
    echo "    Creating $SETTINGS_LOCAL..."
    cat > "$SETTINGS_LOCAL" << 'SLEOF'
{
    "enabledMcpjsonServers": [
        "serena"
    ]
}
SLEOF
else
    echo "    Merging 'serena' into $SETTINGS_LOCAL..."
    run_python "$SETTINGS_LOCAL" << 'PYEOF'
import sys, json

path = sys.argv[1]
with open(path) as f:
    data = json.load(f)

servers = data.get("enabledMcpjsonServers", [])
if not isinstance(servers, list):
    servers = []

if "serena" not in servers:
    servers.append("serena")
    print("    Added 'serena' to enabledMcpjsonServers.")
else:
    print("    'serena' already present, skipping.")

data["enabledMcpjsonServers"] = servers

with open(path, "w") as f:
    json.dump(data, f, indent=4)
    f.write("\n")
PYEOF
fi

# ─── Step 5: Optional hooks ───────────────────────────────────────────────────

echo ""
answer="$(prompt_yn "==> Step 5: Add serena-hooks to settings.local.json?" y)"

if [[ "$answer" == "y" ]]; then
    run_python "$SETTINGS_LOCAL" << 'PYEOF'
import sys, json

EVENTS = {
    "PreToolUse": {
        "matcher": "",
        "hooks": [{"type": "command", "command": "serena-hooks remind --client=claude-code"}]
    },
    "SessionStart": {
        "matcher": "",
        "hooks": [{"type": "command", "command": "serena-hooks activate --client=claude-code"}]
    },
    "Stop": {
        "matcher": "",
        "hooks": [{"type": "command", "command": "serena-hooks cleanup --client=claude-code"}]
    }
}

COMMANDS = {
    "PreToolUse":   "serena-hooks remind --client=claude-code",
    "SessionStart": "serena-hooks activate --client=claude-code",
    "Stop":         "serena-hooks cleanup --client=claude-code",
}

path = sys.argv[1]
with open(path) as f:
    data = json.load(f)

hooks = data.setdefault("hooks", {})

for event, entry in EVENTS.items():
    target_cmd = COMMANDS[event]
    if event not in hooks:
        hooks[event] = [entry]
        print("    Added " + event + " hook.")
    else:
        already = any(
            h.get("command") == target_cmd
            for block in hooks[event]
            for h in block.get("hooks", [])
        )
        if not already:
            hooks[event].append(entry)
            print("    Added serena entry to existing " + event + " hooks.")
        else:
            print("    " + event + " hook already present, skipping.")

with open(path, "w") as f:
    json.dump(data, f, indent=4)
    f.write("\n")
PYEOF
fi

# ─── Step 6: .gitignore ──────────────────────────────────────────────────────

echo ""
echo "==> Step 6: Update .gitignore"
GITIGNORE="$PROJECT_DIR/.gitignore"

ensure_gitignore_entry "$GITIGNORE" "settings.local.json"
ensure_gitignore_entry "$GITIGNORE" ".mcp.json"

echo ""
echo "Done! Serena MCP setup complete."
echo ""
echo "Next steps:"
echo "  1. Reload Claude Code (or start a new session) to activate the Serena MCP server."
echo "  2. Verify 'serena' appears in your MCP server list."
