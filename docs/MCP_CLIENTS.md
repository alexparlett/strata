# Connecting MCP clients

How to point specific MCP clients at Strata's agent access server. What an agent can and cannot
do is the [README's agent access section](../README.md#agent-access).

Turn the server on in **Settings ▸ Agent access**, which is also where the port and the bearer
token live. Every client needs the same three facts:

| | |
|---|---|
| **URL** | `http://127.0.0.1:<port>/mcp` — `47821` by default |
| **Header** | `Authorization: Bearer <token>` |
| **Transport** | Streamable HTTP (some clients spell it `streamable-http`) |

The server lives *inside* the running app, so there is no command for a client to spawn —
Strata has to be open, with the project you want the agent to see open in a window. A client
that only speaks stdio needs a proxy (see Claude Desktop below) or the
[headless server](#with-strata-closed).

## Claude Code

```bash
claude mcp add --transport http strata http://127.0.0.1:47821/mcp --header "Authorization: Bearer YOUR_TOKEN"
```

`--scope user` makes it available in every project; `claude mcp list` reports `✔ Connected`
when it is working. The equivalent `.mcp.json` entry — `"type"` is required, since Claude Code
reads a typeless entry as a stdio server:

```json
{
  "mcpServers": {
    "strata": {
      "type": "http",
      "url": "http://127.0.0.1:47821/mcp",
      "headers": { "Authorization": "Bearer YOUR_TOKEN" }
    }
  }
}
```

## Claude Desktop

Desktop launches its servers itself and speaks stdio, which an in-app server cannot offer — so
it needs a stdio↔HTTP proxy. In `claude_desktop_config.json`
(Settings ▸ Developer ▸ Edit Config), with Node.js installed:

```json
{
  "mcpServers": {
    "strata": {
      "command": "npx",
      "args": ["-y", "mcp-remote", "http://127.0.0.1:47821/mcp",
               "--header", "Authorization: Bearer YOUR_TOKEN"]
    }
  }
}
```

## VS Code (Copilot agent mode)

`.vscode/mcp.json` for one project, or your profile's `mcp.json` for all of them.
`${input:…}` prompts for the token rather than committing it:

```json
{
  "servers": {
    "strata": {
      "type": "http",
      "url": "http://127.0.0.1:47821/mcp",
      "headers": { "Authorization": "Bearer ${input:strata-token}" }
    }
  }
}
```

## Cursor

`.cursor/mcp.json` for one project, `~/.cursor/mcp.json` globally:

```json
{
  "mcpServers": {
    "strata": {
      "url": "http://127.0.0.1:47821/mcp",
      "headers": { "Authorization": "Bearer ${env:STRATA_TOKEN}" }
    }
  }
}
```

## Gemini CLI

`~/.gemini/settings.json`, or `.gemini/settings.json` per project. The field is `httpUrl`, not
`url`:

```json
{
  "mcpServers": {
    "strata": {
      "httpUrl": "http://127.0.0.1:47821/mcp",
      "headers": { "Authorization": "Bearer YOUR_TOKEN" }
    }
  }
}
```

## Codex CLI

`~/.codex/config.toml`, or `.codex/config.toml` in a trusted project. Codex takes the **name of
an environment variable**, not the token itself:

```toml
[mcp_servers.strata]
url = "http://127.0.0.1:47821/mcp"
bearer_token_env_var = "STRATA_TOKEN"
```

Older Codex versions only pick up stdio servers; add `[features]` with
`experimental_use_rmcp_client = true` above it, or upgrade.

## Anything else

Point it at the URL as a Streamable HTTP server with that header. The token is checked before a
request reaches a tool, so a missing or wrong one is a plain `401`; the scheme is matched
case-insensitively, the secret is not.

## With Strata closed

The same tools without the app: `strata mcp <project folder>` serves one project over
**stdio**, which is the transport for a server the client spawns itself — so there is no port,
no token and no window, and the client owning the process is the whole of the access control.

```bash
claude mcp add strata-headless -- /Applications/Strata.app/Contents/MacOS/Strata mcp /data/sales
```

The equivalent entry for a client that reads a config file (Claude Desktop, and anything else
that speaks stdio):

```json
{
  "mcpServers": {
    "strata": {
      "command": "/Applications/Strata.app/Contents/MacOS/Strata",
      "args": ["mcp", "/data/sales"]
    }
  }
}
```

It runs happily beside the app, including on the same project — two engines, each with its own
snapshots. What it does not share is anything of yours: it never reads or writes your settings,
your window session or your query history, and a folder with no project in it is refused rather
than turned into one. It also cannot see your `datafusion.*` overrides (those live in app
settings), so it runs the engine's defaults. A table whose source is missing is served as a
`failed` catalog row with its error, exactly as the app lists it, and the rest of the project
queries normally.
