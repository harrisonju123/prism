# --- Dashboard build stage ---
FROM node:22-alpine AS dashboard
RUN corepack enable && corepack prepare pnpm@latest --activate
WORKDIR /dashboard
COPY dashboard/package.json dashboard/pnpm-lock.yaml* ./
RUN pnpm install --frozen-lockfile || pnpm install
COPY dashboard/ .
RUN pnpm run build

# --- Rust build stage ---
FROM rust:1.94-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release -p prism

# --- Prism runtime stage ---
FROM alpine:3.21 AS prism

RUN apk add --no-cache ca-certificates

COPY --from=builder /app/target/release/prism-server /usr/local/bin/prism-server
COPY --from=dashboard /dashboard/dist /usr/share/prism/dashboard

EXPOSE 9100

CMD ["prism-server"]
