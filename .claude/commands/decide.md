Record a decision with rationale in the PrisM context store.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context decide $ARGUMENTS
```

Common usage:
- `/decide "Use JWT" --content "Chose JWT over sessions because stateless and scales better"` — global decision
- `/decide "Use JWT" --content "Rationale here" --thread auth-refactor --tags auth,security` — scoped to thread

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
