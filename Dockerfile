# kaspulse oracle — build + serve (dashboard + /v1 API + OG cards) on $PORT
FROM rust:1-slim AS build
RUN apt-get update && apt-get install -y pkg-config libssl-dev git curl ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
# OG share cards need the JetBrains Mono TTFs (debian-slim ships no fonts, and
# the repo never vendors binaries) — fetched at build time; the renderer loads
# them at runtime from assets/fonts/.
RUN ./scripts/fetch-fonts.sh
# `og` feature = resvg card renderer; ONLY the container build enables it —
# the default local build stays lean (and tokio-free).
RUN cargo build --release --bin oracle --features og
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /app/target/release/oracle /usr/local/bin/oracle
COPY --from=build /app/assets ./assets
COPY web ./web
COPY pools.json ./pools.json
ENV PORT=8080
EXPOSE 8080
CMD ["oracle"]
