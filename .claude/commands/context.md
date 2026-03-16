Run a prism context management command using the correct binary (`~/.cargo/bin/prism`).

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context $ARGUMENTS
```

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes (e.g. if `.prism/context.json` is missing, run `~/.cargo/bin/prism context init "Project Name"`).

**Individual slash commands are available for each subcommand** — type `/` to see them all. Common ones:

| Command | Description |
|---|---|
| `/checkin` | Agent session start |
| `/checkout` | Agent session end |
| `/agents` | List all agents |
| `/thread` | Thread management (create/list/archive) |
| `/recall` | Load prior context |
| `/remember` | Save a memory |
| `/memories` | List memories |
| `/decide` | Record a decision |
| `/decisions` | List decisions |
| `/activity` | Activity log |
| `/heartbeat` | Keep session alive |
| `/agent` | Agent state / reap |
| `/handoff` | Agent handoffs |
| `/snapshot` | Point-in-time snapshot |
| `/inbox` | Supervisory inbox |
| `/messages` | Agent-to-agent messages |
| `/send` | Send message to agent |
| `/plan-ctx` | Plan management |
| `/wp` | Work packages |
| `/files` | File claim management |

For the full workspace overview: `~/.cargo/bin/prism context context`
