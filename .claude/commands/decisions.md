List recorded decisions from the PrisM context store.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context decisions $ARGUMENTS
```

Common usage:
- `/decisions` — list all decisions in the workspace
- `/decisions --thread auth-refactor` — decisions for a specific thread
- `/decisions --tags auth,security` — filter by tags

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
