---
name: request-replay
description: Generate OpenAPI + Request Replay artifacts, then run a happy-path replay.
user-invocable: true
allowed-tools: bash, read_file, list_dir, glob_files, grep_files
---

You are running the Request Replay flow for a repo.

Follow this exact sequence:

1. Discover/generate OpenAPI:
   - Run: `prism request-replay openapi --output-dir request-replay`
   - If it fails, report the error and suggest setting `PRISM_OPENAPI_PATH` or `PRISM_OPENAPI_URL`.

2. Generate replay artifacts:
   - Run: `prism request-replay generate --output-dir request-replay`

3. Pick a request to replay:
   - List the generated requests directory and select one with a clear “happy path”.
   - Run: `prism request-replay run <request-id> --env local`

4. Summarize results:
   - Report the request id, status code, and whether it matched the expected schema.
   - If auth is missing, point to `PRISM_API_KEY`.

Do NOT edit source code. Only run commands and report results.