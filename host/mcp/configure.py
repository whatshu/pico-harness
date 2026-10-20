#!/usr/bin/env python3
"""Print a Claude project config or Codex config fragment, with absolute paths.

Output goes to stdout so existing user configuration is never overwritten.
"""
import argparse
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("agent", choices=("claude", "codex"))
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--address", default="127.0.0.1:44840")
    parser.add_argument("--token-file", type=Path)
    args = parser.parse_args()
    binary = str(args.binary.resolve(strict=True))
    environment = {"PHD_ADDR": args.address}
    if args.token_file:
        environment["PHD_TOKEN_FILE"] = str(args.token_file.resolve())
    if args.agent == "claude":
        print(json.dumps({"mcpServers": {"picoharness": {
            "type": "stdio", "command": binary, "args": [], "env": environment
        }}}, indent=2))
    else:
        # JSON basic strings are also TOML basic strings for paths and addresses.
        print("[mcp_servers.picoharness]")
        print(f"command = {json.dumps(binary, ensure_ascii=False)}")
        print("args = []\nstartup_timeout_sec = 10\ntool_timeout_sec = 60")
        print("\n[mcp_servers.picoharness.env]")
        for key, value in environment.items():
            print(f"{key} = {json.dumps(value, ensure_ascii=False)}")


if __name__ == "__main__":
    main()
