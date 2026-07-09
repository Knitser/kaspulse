import json, urllib.request, ssl, time
from concurrent.futures import ThreadPoolExecutor
CTX = ssl._create_unverified_context()
RPC = "https://evmrpc.kasplex.org"
WKAS = "0x2c2ae87ba178f48637acae54b87c3924f544a83e".lower()
def call(to, data):
    req = urllib.request.Request(RPC, json.dumps({"jsonrpc":"2.0","method":"eth_call","params":[{"to":to,"data":data},"latest"],"id":1}).encode(), {"content-type":"application/json"})
    try: return json.load(urllib.request.urlopen(req, timeout=10, context=CTX)).get("result","0x")
    except Exception: return "0x"
def addr(h): return "0x"+h[-40:]
def u(h,i=0):
    h=h[2:]; return int(h[i*64:(i+1)*64] or "0",16)
def dstr(h):
    h=h[2:]
    if len(h)<128: return None
    ln=int(h[64:128],16)
    try: return bytes.fromhex(h[128:128+ln*2]).decode("utf8","replace").strip("\x00")
    except: return None
POOL0="0xb905105452e5bedb1e6bd2d8c57e2b70f5a7349a"
factory=addr(call(POOL0,"0xc45a0155")); n=u(call(factory,"0x574f2ba3"))
print(f"factory {factory} · {n} pairs · discovering in parallel…")
ex=ThreadPoolExecutor(max_workers=24)
pairs=list(ex.map(lambda i: addr(call(factory,"0x1e3dd18b"+format(i,'064x'))), range(n)))
def info(pair):
    t0=addr(call(pair,"0x0dfe1681")).lower(); t1=addr(call(pair,"0xd21220a7")).lower()
    if WKAS not in (t0,t1): return None
    wkas0=t0==WKAS; tok=t1 if wkas0 else t0
    res=call(pair,"0x0902f1ac"); r0=u(res,0); r1=u(res,1)
    rw=(r0 if wkas0 else r1)/1e18; rt=(r1 if wkas0 else r0)
    sym=dstr(call(tok,"0x95d89b41")); dec=int(call(tok,"0x313ce567") or "0x12",16)
    if not sym or rt==0 or rw<50: return None  # skip empty/dead pools (<50 WKAS liquidity)
    px=rw/(rt/10**dec)
    return {"symbol":sym,"pair":pair,"wkas_is_token0":wkas0,"dec":dec,"wkas_liq":round(rw,1),"px_wkas":px}
rows=[r for r in ex.map(info, pairs) if r]
rows.sort(key=lambda r:-r["wkas_liq"])
print(f"\n{len(rows)} live WKAS-paired KRC-20 tokens (by liquidity):")
for r in rows[:14]: print(f"  {r['symbol']:12} liq={r['wkas_liq']:>10} WKAS  ~${r['px_wkas']*0.0292:.9f}  pool {r['pair']}")
json.dump(rows[:10], open("/Users/michielhamblok/Documents/code/Kaspa/kaspulse/pools.json","w"), indent=1)
print(f"\nsaved top {min(10,len(rows))} to pools.json")
