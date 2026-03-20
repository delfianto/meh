# Meh — Configuration Reference

## Config File (`~/.config/meh/config.toml`)

```toml
[provider]
default = "anthropic"         # Default provider

[provider.anthropic]
api_key_env = "ANTHROPIC_API_KEY"   # Read from env var
# api_key = "sk-..."               # Or inline (not recommended)

[provider.openai]
api_key_env = "OPENAI_API_KEY"

[provider.gemini]
api_key_env = "GEMINI_API_KEY"

[provider.openrouter]
api_key_env = "OPENROUTER_API_KEY"

[mode]
default = "act"               # "plan", "act", or "plan_then_act"
strict_plan = false           # Require plan phase before act

[mode.plan]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
thinking_budget = 10000

[mode.act]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
thinking_budget = 8000

[permissions]
mode = "ask"                  # "ask", "auto", "yolo"

[permissions.auto_approve]
read_files = true
edit_files = false
execute_safe_commands = true
execute_all_commands = false
mcp_tools = false

[permissions.command_rules]
allow = ["git *", "cargo *", "ls", "cat", "grep *"]
deny = ["rm -rf *", "sudo *"]
allow_redirects = false
```

## MCP Settings (`~/.config/meh/mcp_settings.json`)

```json
{
  "servers": {
    "my-server": {
      "command": "node",
      "args": ["server.js"],
      "transport": "stdio",
      "env": { "API_KEY": "${API_KEY}" },
      "auto_approve": ["safe_tool_*"]
    }
  }
}
```

## CLI Arguments (via clap)

```
meh [OPTIONS] [INITIAL_PROMPT]

Options:
  -p, --provider <PROVIDER>    Override provider (anthropic|openai|gemini|openrouter)
  -m, --model <MODEL>          Override model ID
  --mode <MODE>                 Start in mode (plan|act)
  --yolo                        Enable YOLO mode (no approval prompts)
  -c, --config <PATH>           Config file path
  --resume <TASK_ID>            Resume a previous task
  -v, --verbose                 Verbose logging
```

## Build & Run

```bash
# Development
cargo run

# Release
cargo build --release
./target/release/meh

# With specific config
MEH_CONFIG=~/.config/meh/config.toml cargo run

# With env vars for API keys
ANTHROPIC_API_KEY=sk-... cargo run
```
