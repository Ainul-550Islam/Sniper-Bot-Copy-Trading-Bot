//! The embedded dashboard HTML and same-origin JavaScript.
//!
//! Served at `/` when `api.serve_dashboard` is on. JavaScript is a same-origin
//! asset so the script policy never requires unsafe-inline.
//! It polls the REST API for state and subscribes to the `/api/events`
//! websocket for a live feed.

/// The dashboard HTML.
pub const DASHBOARD_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>Sniper Suite — Control</title>
<style>
  :root{
    --bg:#0b0e14; --panel:#141925; --panel2:#1b2230; --line:#232c3d;
    --txt:#e6edf3; --mut:#8b98a9; --acc:#4ea1ff; --ok:#3fb950; --warn:#d29922; --bad:#f85149;
  }
  *{box-sizing:border-box}
  body{margin:0;background:var(--bg);color:var(--txt);
    font:14px/1.45 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif}
  header{display:flex;align-items:center;gap:16px;padding:14px 20px;
    background:var(--panel);border-bottom:1px solid var(--line);position:sticky;top:0;z-index:5}
  header h1{font-size:16px;margin:0;font-weight:600;letter-spacing:.3px}
  .pill{padding:3px 10px;border-radius:999px;font-size:12px;border:1px solid var(--line);background:var(--panel2)}
  .pill b{font-weight:600}
  .dot{width:8px;height:8px;border-radius:50%;display:inline-block;margin-right:6px;vertical-align:middle}
  .dot.ok{background:var(--ok)} .dot.bad{background:var(--bad)} .dot.warn{background:var(--warn)}
  .grow{flex:1}
  main{padding:18px 20px;display:grid;gap:16px;grid-template-columns:1fr}
  @media(min-width:1000px){main{grid-template-columns:1.4fr 1fr}}
  .card{background:var(--panel);border:1px solid var(--line);border-radius:10px;overflow:hidden}
  .card h2{font-size:13px;text-transform:uppercase;letter-spacing:.6px;color:var(--mut);
    margin:0;padding:12px 14px;border-bottom:1px solid var(--line);display:flex;align-items:center;gap:10px}
  .card .body{padding:8px 14px 14px}
  table{width:100%;border-collapse:collapse}
  th,td{text-align:left;padding:7px 8px;border-bottom:1px solid var(--line);font-size:13px;white-space:nowrap}
  th{color:var(--mut);font-weight:500;font-size:11px;text-transform:uppercase;letter-spacing:.4px}
  td.num,th.num{text-align:right;font-variant-numeric:tabular-nums}
  tr:last-child td{border-bottom:none}
  button{background:var(--panel2);color:var(--txt);border:1px solid var(--line);
    padding:6px 12px;border-radius:7px;cursor:pointer;font-size:13px}
  button:hover{border-color:var(--acc)}
  button.danger{border-color:#5a2530;color:#ffb4ae}
  button.danger:hover{background:#2a1418}
  button.ok{border-color:#20502a;color:#9fe6ad}
  select{background:var(--panel2);color:var(--txt);border:1px solid var(--line);padding:6px 8px;border-radius:7px}
  .row{display:flex;gap:8px;flex-wrap:wrap;align-items:center}
  .feed{max-height:340px;overflow:auto;font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12px}
  .feed div{padding:3px 0;border-bottom:1px solid #161d29}
  .feed .t{color:var(--mut);margin-right:8px}
  .k-fill{color:var(--ok)} .k-risk_rejected{color:var(--warn)} .k-error{color:var(--bad)}
  .k-signal{color:var(--acc)} .k-launch{color:#c792ea} .k-position_closed{color:#7ee0c3}
  .muted{color:var(--mut)} .pos{color:var(--ok)} .neg{color:var(--bad)}
  .empty{color:var(--mut);padding:14px 0;text-align:center}
  .kpi{display:flex;gap:18px;flex-wrap:wrap}
  .kpi .box{background:var(--panel2);border:1px solid var(--line);border-radius:8px;padding:10px 14px;min-width:120px}
  .kpi .box .l{color:var(--mut);font-size:11px;text-transform:uppercase;letter-spacing:.4px}
  .kpi .box .v{font-size:20px;font-weight:600;font-variant-numeric:tabular-nums;margin-top:2px}
  code{background:var(--panel2);padding:1px 5px;border-radius:4px}
</style>
</head>
<body>
<header>
  <h1>🎯 Sniper Suite</h1>
  <span class="pill" id="mode">mode: <b>—</b></span>
  <span class="pill" id="kill">kill: <b>—</b></span>
  <span class="pill" id="cluster">cluster: <b>—</b></span>
  <span class="grow"></span>
  <span class="pill" id="conn"><span class="dot bad"></span>disconnected</span>
</header>
<main>
  <section class="card">
    <h2>Controls</h2>
    <div class="body row">
      <button class="danger" id="btnKill">🛑 Kill switch</button>
      <button class="ok" id="btnResume">✅ Resume</button>
      <span class="muted">|</span>
      <select id="modeSel">
        <option value="">set mode…</option>
        <option value="paper">paper</option>
        <option value="simulate">simulate</option>
        <option value="live">live</option>
      </select>
      <button id="btnMode">Apply</button>
      <span class="grow"></span>
      <span class="muted">api key</span>
      <input id="apiKey" type="password" placeholder="x-api-key" style="background:var(--panel2);border:1px solid var(--line);color:var(--txt);padding:6px 8px;border-radius:7px"/>
    </div>
  </section>

  <section class="card">
    <h2>Overview</h2>
    <div class="body">
      <div class="kpi">
        <div class="box"><div class="l">Realized</div><div class="v" id="kReal">—</div></div>
        <div class="box"><div class="l">Unrealized</div><div class="v" id="kUnreal">—</div></div>
        <div class="box"><div class="l">Open pos</div><div class="v" id="kOpen">—</div></div>
        <div class="box"><div class="l">SOL</div><div class="v" id="kSol">—</div></div>
        <div class="box"><div class="l">USDC</div><div class="v" id="kUsdc">—</div></div>
      </div>
    </div>
  </section>

  <section class="card">
    <h2>Modules <span class="grow"></span><button id="btnRefresh">↻</button></h2>
    <div class="body"><table id="modules"><thead><tr>
      <th>module</th><th>state</th><th class="num">signals</th><th class="num">orders</th>
      <th class="num">fills</th><th class="num">fail</th><th class="num">rj</th>
      <th class="num">pnl</th><th>toggle</th>
    </tr></thead><tbody></tbody></table></div>
  </section>

  <section class="card">
    <h2>Open positions</h2>
    <div class="body"><table id="positions"><thead><tr>
      <th>module</th><th>symbol</th><th class="num">qty</th><th class="num">entry</th>
      <th class="num">mark</th><th class="num">uPnL</th><th class="num">%</th>
    </tr></thead><tbody></tbody></table></div>
  </section>

  <section class="card">
    <h2>Recent fills</h2>
    <div class="body"><table id="trades"><thead><tr>
      <th>time</th><th>src</th><th>side</th><th>symbol</th>
      <th class="num">in</th><th class="num">out</th><th class="num">price</th>
    </tr></thead><tbody></tbody></table></div>
  </section>

  <section class="card">
    <h2>Live events <span class="grow"></span><button id="btnClearFeed">clear</button></h2>
    <div class="body"><div class="feed" id="feed"></div></div>
  </section>
</main>
<script src="/dashboard/app.js"></script>
</body>
</html>"#;

/// Same-origin dashboard JavaScript.
pub const DASHBOARD_JS: &str = include_str!("dashboard/app.js");
