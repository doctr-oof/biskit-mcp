"""Drives a Biskit MCP server over stdio and prints each response.

Usage: python scripts/smoke.py <binary> <project-dir>
"""

import json
import subprocess
import sys
import threading


def main() -> int:
    binary, project = sys.argv[1], sys.argv[2]

    process = subprocess.Popen(
        [binary, "start", "--project", project],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        bufsize=1,
    )

    def drain_stderr() -> None:
        for line in process.stderr:
            print("  [server]", line.rstrip(), file=sys.stderr)

    threading.Thread(target=drain_stderr, daemon=True).start()

    def send(message: dict) -> None:
        process.stdin.write(json.dumps(message) + "\n")
        process.stdin.flush()

    def read() -> dict:
        line = process.stdout.readline()
        if not line:
            raise SystemExit("server closed the connection")
        return json.loads(line)

    send({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "smoke", "version": "0"},
        },
    })
    initialized = read()
    print("== initialize ==")
    print("instructions:", initialized["result"].get("instructions", "")[:120], "...")

    send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    send({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
    tools = read()["result"]["tools"]
    print(f"\n== tools ({len(tools)}) ==")
    for tool in sorted(tools, key=lambda item: item["name"]):
        print(" -", tool["name"])

    calls = [
        ("create_memory", {"memory_name": "architecture/overview",
                           "content": "PlayerService owns scores. See `mem:style`."}),
        ("create_memory", {"memory_name": "style", "content": "Tabs, not spaces."}),
        ("create_memory", {"memory_name": "style", "content": "Clobbered."}),
        ("create_memory", {"memory_name": "style", "content": "Tabs, not spaces.",
                           "overwrite": True}),
        ("list_memories", {}),
        ("rename_memory", {"old_name": "style", "new_name": "conventions/style"}),
        ("read_memory", {"memory_name": "architecture/overview"}),
        ("list_dir", {"relative_path": ".", "recursive": True}),
        ("find_file", {"file_mask": "*.luau"}),
        ("search_for_pattern", {"substring_pattern": "addScore",
                                "restrict_search_to_code_files": True}),
        ("get_symbols_overview", {"relative_path": "src/PlayerService.luau"}),
        ("find_symbol", {"name_path": "addScore", "include_body": True}),
        ("find_symbol", {"name_path": "PlayerService.register"}),
        ("find_declaration", {"name_path": "PlayerService.addScore",
                              "relative_path": "src/PlayerService.luau",
                              "include_body": True}),
        ("find_referencing_symbols", {"name_path": "PlayerService/addScore",
                                      "relative_path": "src/PlayerService.luau"}),
        ("get_file_diagnostics", {"relative_path": "src/PlayerService.luau",
                                  "min_severity": 1}),
        ("get_symbol_diagnostics", {"name_path": "PlayerService.brokenOnPurpose",
                                    "relative_path": "src/PlayerService.luau",
                                    "min_severity": 1}),
        ("restart_language_server", {}),
        ("get_symbols_overview", {"relative_path": "src/Main.luau"}),
        ("delete_memory", {"memory_name": "conventions/style"}),
        ("list_memories", {}),
    ]

    next_id = 3
    for name, arguments in calls:
        send({
            "jsonrpc": "2.0",
            "id": next_id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        response = read()
        print(f"\n== {name} ==")
        if "error" in response:
            print("  ERROR:", response["error"])
        else:
            for block in response["result"]["content"]:
                body = block.get("text", "")
                print("  " + body[:1400].replace("\n", "\n  "))
        next_id += 1

    process.stdin.close()
    process.terminate()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
