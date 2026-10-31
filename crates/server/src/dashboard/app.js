const $ = (s)=>document.querySelector(s);
const key = ()=>$('#apiKey').value.trim();
function esc(v){ if(v===null||v===undefined) return ''; return String(v).replace(/[&<>"']/g, function(c){ return {'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]; }); }
function authHeaders(){ const h={'Content-Type':'application/json'}; const k=key(); if(k) h['x-api-key']=k; return h; }
function pnlClass(v){ return v>0?'pos':(v<0?'neg':'muted'); }
function fmt(v,d){ if(v===null||v===undefined||isNaN(v))return '—'; return Number(v).toFixed(d===undefined?4:d); }

async function jget(path){ const r=await fetch(path); if(!r.ok) throw new Error(path+' '+r.status); return r.json(); }
async function jpost(path,body){ const r=await fetch(path,{method:'POST',headers:authHeaders(),body:body?JSON.stringify(body):undefined});
  if(!r.ok){ const t=await r.text(); throw new Error(t||r.status);} return r.status===204?null:r.json().catch(()=>null); }

async function refresh(){
  try{
    const s = await jget('/api/status');
    $('#mode').innerHTML='mode: <b>'+esc(s.execution_mode)+'</b>';
    $('#kill').innerHTML='kill: <b>'+(s.kill_switch?'🛑 ON':'off')+'</b>';
    $('#cluster').innerHTML='cluster: <b>'+esc(s.cluster||'—')+'</b>';
    $('#kReal').textContent=fmt(s.realized_pnl); $('#kReal').className='v '+pnlClass(s.realized_pnl);
    $('#kUnreal').textContent=fmt(s.unrealized_pnl); $('#kUnreal').className='v '+pnlClass(s.unrealized_pnl);
    $('#kOpen').textContent=s.open_positions;
    $('#kSol').textContent=fmt(s.balances? s.balances.sol:0);
    $('#kUsdc').textContent=fmt(s.balances? s.balances.usdc_polygon:0,2);

    const mt=$('#modules').querySelector('tbody'); mt.innerHTML='';
    (s.modules||[]).forEach(m=>{
      const st = !m.running? 'stopped' : (m.healthy? (m.connected?'online':'no-feed') : 'degraded');
      const dot = m.healthy&&m.connected?'ok':(m.running?'warn':'bad');
      const tr=document.createElement('tr');
      tr.innerHTML =
        '<td>'+esc(m.module)+'</td>'+
        '<td><span class="dot '+dot+'"></span>'+st+(m.enabled?'':' <span class="muted">(off)</span>')+'</td>'+
        '<td class="num">'+esc(m.signals_generated)+'</td>'+
        '<td class="num">'+esc(m.orders_sent)+'</td>'+
        '<td class="num">'+esc(m.orders_filled)+'</td>'+
        '<td class="num">'+esc(m.orders_failed)+'</td>'+
        '<td class="num">'+esc(m.orders_rejected_by_risk)+'</td>'+
        '<td class="num '+pnlClass(m.realized_pnl)+'">'+fmt(m.realized_pnl)+'</td>'+
        '<td><button data-mod="'+esc(m.module)+'" data-on="'+(!m.enabled)+'" class="tgl">'+(m.enabled?'disable':'enable')+'</button></td>';
      mt.appendChild(tr);
    });
    mt.querySelectorAll('.tgl').forEach(b=>b.onclick=async()=>{
      const on=b.dataset.on==='true';
      try{ await jpost('/api/modules/'+b.dataset.mod+'/'+(on?'enable':'disable')); refresh(); }
      catch(e){ alert('failed: '+e.message); }
    });

    const pos=await jget('/api/positions'); const pt=$('#positions').querySelector('tbody'); pt.innerHTML='';
    if(!pos.length){ pt.innerHTML='<tr><td colspan="7" class="empty">no open positions</td></tr>'; }
    pos.forEach(p=>{ const pc=(p.total_pnl_pct||0)*100; const tr=document.createElement('tr');
      tr.innerHTML='<td>'+esc(p.source||'')+'</td><td>'+esc(p.symbol_display||p.symbol)+'</td>'+
        '<td class="num">'+fmt(p.qty)+'</td><td class="num">'+fmt(p.avg_entry,6)+'</td>'+
        '<td class="num">'+fmt(p.last_mark,6)+'</td>'+
        '<td class="num '+pnlClass(p.unrealised)+'">'+fmt(p.unrealised)+'</td>'+
        '<td class="num '+pnlClass(pc)+'">'+fmt(pc,1)+'%</td>'; pt.appendChild(tr); });

    const tr2=await jget('/api/trades?limit=12'); const tt=$('#trades').querySelector('tbody'); tt.innerHTML='';
    if(!tr2.length){ tt.innerHTML='<tr><td colspan="7" class="empty">no fills yet</td></tr>'; }
    tr2.forEach(t=>{ const d=new Date(t.ts); const tr=document.createElement('tr');
      tr.innerHTML='<td>'+d.toLocaleTimeString()+'</td><td>'+esc(t.source||'')+'</td>'+
        '<td>'+esc(t.side||'')+'</td><td>'+esc(t.symbol_display||t.symbol)+'</td>'+
        '<td class="num">'+fmt(t.amount_in)+'</td><td class="num">'+fmt(t.amount_out)+'</td>'+
        '<td class="num">'+fmt(t.price,6)+'</td>'; tt.appendChild(tr); });
  }catch(e){ console.error(e); }
}

function pushFeed(ev){
  const f=$('#feed'); const d=document.createElement('div');
  const t=new Date(ev.ts||Date.now()).toLocaleTimeString();
  const kind=ev.kind||'info';
  let summary=kind;
  if(kind==='fill'&&ev.trade) summary='FILL '+ev.trade.side+' '+ev.trade.symbol_display+' '+fmt(ev.trade.amount_out)+' @ '+fmt(ev.trade.price,6);
  else if(kind==='risk_rejected') summary='RISK '+ev.module+' '+ev.symbol+': '+ev.reason;
  else if(kind==='signal') summary='SIGNAL '+ev.module+' '+ev.symbol+' '+ev.side;
  else if(kind==='launch') summary='LAUNCH '+(ev.launch?ev.launch.symbol:'')+(ev.accepted?' ✓':' ✗ '+(ev.reason||''));
  else if(kind==='position_closed') summary='CLOSE '+(ev.position?ev.position.symbol_display:'')+' pnl '+fmt(ev.pnl);
  else if(kind==='error') summary='ERROR '+ev.message;
  else if(kind==='wallet_trade'&&ev.trade) summary='WHALE '+ev.trade.wallet.slice(0,6)+' '+ev.trade.side+' '+ev.trade.mint.slice(0,6);
  d.innerHTML='<span class="t">'+esc(t)+'</span><span class="k-'+esc(kind)+'">'+esc(summary)+'</span>';
  f.prepend(d); while(f.children.length>200) f.removeChild(f.lastChild);
}

let ws;
function connectWs(){
  const proto=location.protocol==='https:'?'wss':'ws';
  const k=key();
  const url=proto+'://'+location.host+'/api/saas/events';
  if(ws){ try{ ws.onclose=null; ws.close(); }catch(e){} }
  ws=new WebSocket(url);
  ws.onopen=()=>{
    $('#conn').innerHTML='<span class="dot ok"></span>live';
    if(k) ws.send(JSON.stringify({type:'auth',token:k}));
  };
  ws.onclose=()=>{ $('#conn').innerHTML='<span class="dot bad"></span>disconnected'; setTimeout(connectWs,2000); };
  ws.onerror=()=>{ try{ws.close()}catch(e){} };
  ws.onmessage=(m)=>{ try{ pushFeed(JSON.parse(m.data)); }catch(e){} };
}

$('#btnKill').onclick=async()=>{ if(!confirm('Engage kill switch?'))return; try{await jpost('/api/kill');refresh();}catch(e){alert(e.message);} };
$('#btnResume').onclick=async()=>{ try{await jpost('/api/resume');refresh();}catch(e){alert(e.message);} };
$('#btnMode').onclick=async()=>{ const m=$('#modeSel').value; if(!m)return; try{await jpost('/api/mode',{mode:m});refresh();}catch(e){alert(e.message);} };
$('#btnRefresh').onclick=refresh;
$('#btnClearFeed').onclick=()=>{$('#feed').innerHTML='';};
$('#apiKey').addEventListener('change', ()=>{ connectWs(); refresh(); });

refresh(); setInterval(refresh,4000); connectWs();
