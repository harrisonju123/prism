Capture a point-in-time snapshot of the PrisM context store state.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context snapshot $ARGUMENTS
```

Common usage:
- `/snapshot` — capture snapshot with auto-generated label
- `/snapshot --label "before-auth-refactor"` — capture with a descriptive label

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
