Manage agent state in the PrisM context store (view state or reap stale sessions).

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context agent $ARGUMENTS
```

Common usage:
- `/agent state --name claude` — show current state for agent "claude"
- `/agent reap --name claude` — reap a stale/crashed agent session

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
