List persistent memories from the PrisM context store.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context memories $ARGUMENTS
```

Common usage:
- `/memories` — list all memories in the workspace
- `/memories --thread auth-refactor` — list memories for a specific thread
- `/memories --tags auth,security` — filter by tags

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
