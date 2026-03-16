Send a heartbeat to keep the current agent session alive in the PrisM context store.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context heartbeat $ARGUMENTS
```

Common usage:
- `/heartbeat --name claude` — send heartbeat for agent "claude"

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
