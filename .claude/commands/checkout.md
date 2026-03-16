End this agent session and record a summary in the PrisM context store (session end).

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context checkout $ARGUMENTS
```

Common usage:
- `/checkout --name claude --summary "implemented JWT auth"` — basic checkout
- `/checkout --name claude --summary "done" --findings "X is broken" --files "src/auth.rs" --next-steps "add tests"` — full checkout

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
