# kaspulse on the Windows VPS (the ACTUAL live deploy)

This is how kaspulse actually runs in production, on the same Windows Server
2022 VPS (`157.90.7.39`) that serves `ironwood.live`. Mirrors that box's
conventions: **nssm** service wrapper (`C:\tools\nssm.exe`), **Caddy**
service (`C:\tools\caddy.exe run --config C:\caddy\Caddyfile`) shared across
sites, one app per `C:\<name>\` folder.

Public URL: **https://pulse.kascov.io** → A record → the VPS → Caddy →
`127.0.0.1:8093` (the oracle, loopback-only).

## Layout on the box

| thing | value |
|---|---|
| app dir | `C:\kaspulse` (git clone; `AppDirectory`, so `web/`, `pools.json`, `*.key` resolve) |
| binary | `C:\kaspulse\target\release\oracle.exe` (`cargo build --release --features og`) |
| service | nssm `kaspulse`, env `PORT=8093 KASPULSE_BIND=127.0.0.1 KASPULSE_REQUIRE_KEYS=1 BASE_URL=https://pulse.kascov.io` |
| committee keys | `C:\kaspulse\kaspulse-node-{0..4}.key` (copied by `scp`, NEVER git) |
| OG fonts | `C:\kaspulse\assets\fonts\JetBrainsMono-{Regular,Bold}.ttf` (+ `OFL.txt`) |
| logs | `C:\kaspulse\logs\{out,err}.log` |
| Caddy block | `pulse.kascov.io { reverse_proxy 127.0.0.1:8093 }` appended to `C:\caddy\Caddyfile` |

Port 8093 (ironwood is 8095). The box also runs `kaspad-mainnet` and
`kaspad-tn10` node services — the future first-party on-chain-publish path.

## First deploy (done 2026-07-20)

1. `git clone --depth 1 https://github.com/Knitser/kaspulse C:\kaspulse`
2. Build via a **scheduled task** (an SSH-spawned process is killed when the
   session ends; a task survives it):
   `schtasks /create /tn kaspulse-build /tr C:\kaspulse\logs\build.cmd /sc once /st 00:00 /ru SYSTEM /f && schtasks /run /tn kaspulse-build`
   where `build.cmd` sets `CARGO_HOME`/`RUSTUP_HOME` to Administrator's and
   runs `cargo build --release --features og`. Poll `logs\build.log` for
   `Finished`. (`build.cmd` is kept in `logs\` on the box.)
3. `scp kaspulse-node-{0..4}.key Administrator@157.90.7.39:C:/kaspulse/`
4. Fonts: `Invoke-WebRequest` the two JetBrains Mono TTFs + `OFL.txt` (tag
   v2.304) into `assets\fonts\` (the bash `scripts/fetch-fonts.sh` doesn't
   run on Windows).
5. nssm: `install kaspulse …\oracle.exe`, `set AppDirectory C:\kaspulse`,
   `set AppEnvironmentExtra …`, `set AppStdout/AppStderr`, `start kaspulse`.
6. Verify local: `curl http://127.0.0.1:8093/health` → `ok:true`;
   `/og/KAS-USD.png` → valid PNG; `/share/KAS-USD` → 200.
7. DNS: Squarespace A record `pulse` → `157.90.7.39`.
8. Caddy (ONLY after DNS resolves, else Let's Encrypt rate-limits failed
   challenges): append the block, `caddy validate`, then `caddy reload`
   (never restart — keeps ironwood.live up). Cert auto-provisions.

## Update after a `git push`

```
ssh … "cd C:\kaspulse && git pull"        # (via git.exe)
# rebuild through the scheduled task (same as step 2), then:
C:\tools\nssm.exe restart kaspulse
```

The build must be detached (scheduled task), not run inline over SSH.
