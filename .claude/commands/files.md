Manage file claims in the PrisM context store (prevent concurrent agent conflicts on files).

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context files $ARGUMENTS
```

Common usage:
- `/files claim <path> --agent claude` — claim a file for exclusive editing
- `/files release <path>` — release a file claim
- `/files check <path>` — check if a file is claimed
- `/files list` — list all active file claims

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
