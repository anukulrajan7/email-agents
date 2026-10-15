# One image: Rust API + worker, serving the built React app.

FROM node:24-bookworm-slim AS web
WORKDIR /web
RUN npm install -g pnpm@12.5.1
COPY apps/web/package.json apps/web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY apps/web/ ./
RUN pnpm build

# cargo-chef: dependencies build in their own layer, keyed only on Cargo.toml/Cargo.lock, so a code-only
# change reuses the cached layer and compiles just this crate.
FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /api

FROM chef AS planner
COPY apps/api/ ./
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS api
COPY --from=planner /api/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json
COPY apps/api/ ./
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 app \
 && mkdir -p /app/data && chown app /app/data
WORKDIR /app
COPY --from=api /api/target/release/email-ai-api /usr/local/bin/email-ai-api
COPY --from=api /api/target/release/selftest /usr/local/bin/selftest
COPY --from=web /web/dist /app/web
ENV WEB_DIST_DIR=/app/web \
    DATABASE_URL=sqlite:///app/data/email_ai.db \
    RUST_LOG=info
USER app
CMD ["email-ai-api", "serve"]
