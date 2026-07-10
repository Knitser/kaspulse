# Deploying kaspulse

The `oracle` binary serves BOTH the dashboard and the JSON API on `$PORT`
(default 8080, binds `0.0.0.0`), so one container is the whole public service.

## Cloud Run (same pattern as kascov)
```sh
gcloud run deploy kaspulse \
  --source . --region us-central1 \
  --allow-unauthenticated \
  --min-instances 1 \        # keep the WS streams + node keys warm (no scale-to-zero)
  --cpu 1 --memory 512Mi \
  --timeout 3600 --port 8080
```
`--min-instances 1` matters: the oracle holds live WebSocket connections and the
5 node keys in memory; scaling to zero would drop them.

## Local / VPS (systemd)
```sh
cargo build --release --bin oracle
PORT=8080 ./target/release/oracle
```
Point a domain at it, put it behind TLS, done — the dashboard is at `/`, the API
at `/api/feed`.

## Own-node config (removes the last third party)
Set the RPC envs to your own Kasplex/Igra nodes (+ a public one to cross-check):
```sh
KASPLEX_RPCS="https://your-kasplex-node,https://evmrpc.kasplex.org" \
IGRA_RPCS="https://your-igra-node,https://rpc.igralabs.com:8545" \
PORT=8080 ./target/release/oracle
```
With ≥2 RPCs the cross-check activates: any single RPC that lies gets its read
dropped.

## Operators (decentralization)
Each independent operator runs `signer <their-key> <port>` on their own box and
exposes `/attest`; an aggregator polls them for the k-of-n. See `src/signer.rs`.
