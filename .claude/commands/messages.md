List agent-to-agent messages in the PrisM context store.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context messages $ARGUMENTS
```

Common usage:
- `/messages` — list messages for the current agent
- `/messages --agent claude-zed-surface` — list messages for a specific agent

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
