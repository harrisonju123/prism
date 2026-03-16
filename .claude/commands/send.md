Send a message to another agent via the PrisM context store.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context send $ARGUMENTS
```

Common usage:
- `/send --to claude-zed-surface --body "Auth module is ready for review"` — send a message
- `/send --to claude --body "Blocked on DB schema, need your input" --thread auth-refactor` — thread-scoped message

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
