Run a prism context management command using the correct binary (`~/.cargo/bin/prism`).

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context $ARGUMENTS
```

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes (e.g. if `.prism/context.json` is missing, run `~/.cargo/bin/prism context init "Project Name"`).

Common commands:
- `~/.cargo/bin/prism context context` — full workspace overview (agents, threads, memories)
- `~/.cargo/bin/prism context checkin --name <agent>` — register session (auto-runs via hook)
- `~/.cargo/bin/prism context checkout --name <agent> --summary "..."` — end session
- `~/.cargo/bin/prism context agents` — show all agent statuses
- `~/.cargo/bin/prism context thread list` — list active threads
- `~/.cargo/bin/prism context recall <thread-name>` — get full thread context
- `~/.cargo/bin/prism context remember <key> <value>` — save a memory
- `~/.cargo/bin/prism context memories` — list all memories
- `~/.cargo/bin/prism context decide "<title>" --content "..."` — record a decision
