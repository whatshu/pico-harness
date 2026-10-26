# PicoHarness MCP for Claude and Codex

`ph-mcp` is a Rust stdio MCP server using the official [Rust SDK](https://github.com/modelcontextprotocol/rust-sdk). It translates typed tools into authenticated `phd` socket calls. Initialization and tool discovery work without a connected probe or running daemon; device failures return structured tool errors. stdout contains MCP frames only.

```sh
cargo build --release --manifest-path ph-bridge/Cargo.toml --bins
cargo build --release --manifest-path host/mcp/Cargo.toml
ph-bridge/target/release/phdcli daemon start
```

Use an absolute binary path when configuring agents. `PHD_ADDR` selects the daemon (default `127.0.0.1:44840`). `PHD_TOKEN_FILE` selects a local copy of its token; omitting it uses the platform's default user state path. `PHD_TOKEN` is also accepted. Restart the MCP process after changing its environment.

## Claude Code

```sh
claude mcp add --transport stdio picoharness -- /absolute/path/to/ph-mcp
```

For a remote daemon, supply `-e PHD_ADDR=HOST:44840 -e PHD_TOKEN_FILE=/local/path/token` before the server name. Alternatively merge [config/claude.json](config/claude.json) into the project's `.mcp.json`, with `command` replaced by an absolute path. See [Claude's MCP documentation](https://code.claude.com/docs/en/mcp).

## Codex

```sh
codex mcp add picoharness --env PHD_ADDR=127.0.0.1:44840 -- /absolute/path/to/ph-mcp
```

Alternatively merge [config/codex.toml](config/codex.toml) into `~/.codex/config.toml` and adjust `command` and `PHD_TOKEN_FILE`. The example gives initialization 10 seconds and tools 60 seconds. Flash tools return a task ID promptly; agents poll its status instead of waiting for an entire flash in one tool call. See [OpenAI's MCP documentation](https://developers.openai.com/codex/mcp/).

Generate either configuration with platform-correct absolute paths:

```sh
python3 host/mcp/configure.py claude --binary host/mcp/target/release/ph-mcp
python3 host/mcp/configure.py codex --binary host/mcp/target/release/ph-mcp \
  --address 192.168.1.10:44840 --token-file /local/path/remote-token
```

On Windows, use `ph-mcp.exe`. The generator prints configuration and leaves existing agent configuration intact. Only Claude and Codex adapters are supplied.

## Agent behavior and validation

Read [AGENT_GUIDE.md](AGENT_GUIDE.md) when operating devices, handling timeouts, consuming console history or monitoring firmware tasks. The MCP initialization instructions and tool descriptions carry the same operational requirements into agent sessions. Tools require exact serials, bounded payloads and typed arguments. Results include JSON `structuredContent` plus the same JSON text for clients that display text only.

There are 18 tools covering health, discovery, capabilities, identity, status, measurements, power, reset, Console, baud, Bulk write, firmware tasks, cancellation and SWD memory. Read-only/destructive/idempotent annotations describe each operation. Use the capability response to determine whether acquisition or readback exists.

```sh
cargo test --manifest-path host/mcp/Cargo.toml
```

The integration test uses the official MCP client, an actual stdio subprocess and a TCP daemon, checking schemas, tool calls, daemon failure and startup without a daemon. Native Claude/Codex applications require a local manual connection check after installation.
