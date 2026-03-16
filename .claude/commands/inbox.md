Manage the supervisory inbox in the PrisM context store (read/dismiss/resolve items).

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context inbox $ARGUMENTS
```

Common usage:
- `/inbox list` — list inbox items
- `/inbox read <id>` — read a specific inbox item
- `/inbox dismiss <id>` — dismiss an item
- `/inbox resolve <id> --response "Fixed by ..."` — resolve an item with a response

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
