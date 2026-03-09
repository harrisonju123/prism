#!/usr/bin/env bash
set -e

# 1. Check ANTHROPIC_API_KEY
if [ -z "$ANTHROPIC_API_KEY" ]; then
  echo "Error: ANTHROPIC_API_KEY is not set. Export it first:"
  echo "  export ANTHROPIC_API_KEY=sk-ant-..."
  exit 1
fi

# 2. Create config/prism.toml if it doesn't exist
if [ ! -f config/prism.toml ]; then
  MASTER_KEY=$(openssl rand -hex 20)
  sed "s/__MASTER_KEY__/$MASTER_KEY/" config/prism.dev.toml > config/prism.toml
  echo "Created config/prism.toml with master_key=$MASTER_KEY"
fi

# 3. Extract master key from config
MASTER_KEY=$(grep 'master_key' config/prism.toml | sed 's/.*= *"\(.*\)"/\1/')

# 4. Start postgres + clickhouse
docker compose up -d postgres clickhouse

# 5. Wait for postgres
echo "Waiting for postgres..."
until docker compose exec -T postgres pg_isready -U prism > /dev/null 2>&1; do
  sleep 1
done
echo "Postgres ready."

# 6. Build + start prism in background
echo "Starting prism (building if needed)..."
cargo run -p prism 2>&1 &
PRISM_PID=$!

# 7. Wait for health endpoint (up to 60s)
echo "Waiting for prism health..."
for i in $(seq 1 60); do
  if curl -sf http://localhost:9100/health > /dev/null 2>&1; then
    echo "Prism healthy."
    break
  fi
  sleep 1
done

# 8. Create virtual key
RESPONSE=$(curl -sf -X POST http://localhost:9100/api/v1/keys \
  -H "Content-Type: application/json" \
  -H "X-Master-Key: $MASTER_KEY" \
  -d '{"name":"dev-personal","daily_budget_usd":5.0}')

if command -v jq > /dev/null 2>&1; then
  API_KEY=$(echo "$RESPONSE" | jq -r '.key')
else
  API_KEY=$(echo "$RESPONSE" | grep -o '"key":"[^"]*"' | sed 's/"key":"\(.*\)"/\1/')
fi

# 9. Kill background prism
kill $PRISM_PID 2>/dev/null || true

# 10. Print exports
echo ""
echo "===================== SETUP COMPLETE ====================="
echo ""
echo "Add to ~/.zshrc (or ~/.env):"
echo "  export PRISM_URL=http://localhost:9100"
echo "  export PRISM_API_KEY=$API_KEY"
echo ""
echo "Zed settings → Language Models → PrisM:"
echo '  {"api_url": "http://localhost:9100/v1", "api_key": "'"$API_KEY"'"}'
echo ""
echo "Start the gateway:  make dev"
echo "==========================================================="
