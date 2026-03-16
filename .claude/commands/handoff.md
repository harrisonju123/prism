Manage handoffs between agents in the PrisM context store.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context handoff $ARGUMENTS
```

Common usage:
- `/handoff create --to claude-zed-surface --intent "Continue auth implementation"` — create a handoff
- `/handoff list` — list pending handoffs
- `/handoff accept <id>` — accept a handoff
- `/handoff complete <id>` — mark a handoff complete
- `/handoff cancel <id>` — cancel a handoff

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
