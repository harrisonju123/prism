View the PrisM context activity log — all mutations recorded across agents and threads.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context activity $ARGUMENTS
```

Common usage:
- `/activity` — recent activity
- `/activity --since 2h` — activity in the last 2 hours (supports m/h/d)
- `/activity --actor claude` — filter by agent name
- `/activity --limit 50` — limit results

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
