# kaspulse oracle — build + serve (dashboard + /api/feed) on $PORT
FROM rust:1-slim AS build
RUN apt-get update && apt-get install -y pkg-config libssl-dev git && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN cargo build --release --bin oracle
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /app/target/release/oracle /usr/local/bin/oracle
COPY web ./web
COPY pools.json ./pools.json
ENV PORT=8080
EXPOSE 8080
CMD ["oracle"]
