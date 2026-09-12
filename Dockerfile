# ===== Stage 1: React-UI bauen =====
FROM node:20-alpine AS ui-builder
WORKDIR /app/dashboard-ui
COPY dashboard-ui/package.json dashboard-ui/package-lock.json* ./
RUN npm ci
COPY dashboard-ui/ ./
RUN npm run build

# ===== Stage 2: Rust bauen =====
# rust:1-slim ist derzeit trixie-basiert (glibc >= 2.38), passend zum trixie-Runtime
FROM rust:1-slim AS rust-builder
WORKDIR /app
# deps zuerst cachen
COPY Cargo.toml Cargo.lock* ./
COPY crates/ crates/
COPY migrations/ migrations/
# gebautes UI ins binary-layout einbetten
COPY --from=ui-builder /app/dashboard-ui/dist dashboard-ui/dist
RUN cargo build --release --bin yalr

# ===== Stage 3: Runtime (minimal) =====
FROM debian:trixie-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=rust-builder /app/target/release/yalr /usr/local/bin/yalr
ENV HOST=0.0.0.0 PORT=8080
EXPOSE 8080
ENTRYPOINT ["yalr"]
