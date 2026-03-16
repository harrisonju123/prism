Recall context from the PrisM context store — the primary agent interface for loading prior work.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context recall $ARGUMENTS
```

Common usage:
- `/recall auth-refactor` — full thread context (memories, decisions, sessions)
- `/recall --tags auth,security` — memories + decisions by tag
- `/recall --since 2h` — everything from the last 2 hours (supports m/h/d)
- `/recall --since 1d` — everything from the last day

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
