pub const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0, maximum-scale=1.0">
<title>TETRA FlowStation — BS Dashboard</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600&family=IBM+Plex+Sans+Condensed:wght@400;500;600;700&display=swap" rel="stylesheet">
<style>
  /* ── Themes ── */
  :root {
    --bg:      #080c0f;
    --bg2:     #0d1318;
    --bg3:     #111820;
    --bg4:     #172030;
    --border:  #1e2d3d;
    --border2: #243545;
    --accent:  #00c896;
    --accent2: #0097ff;
    --warn:    #f0a800;
    --danger:  #e8445a;
    --text:    #e8f4ff;
    --text2:   #7ab0cc;
    --text3:   #304050;
    --mono:    'IBM Plex Mono', monospace;
    --sans:    'IBM Plex Sans Condensed', sans-serif;
    --r:       4px;
  }
  [data-theme="white"] {
    --bg:#f0f4f8;--bg2:#ffffff;--bg3:#e8eef4;--bg4:#dde5ed;
    --border:#c8d8e8;--border2:#b0c4d8;
    --accent:#00a078;--accent2:#0070cc;--warn:#c07800;--danger:#c03040;
    --text:#0a1420;--text2:#3a5870;--text3:#8aa0b4;
  }
  [data-theme="blue"] {
    --bg:#020818;--bg2:#051030;--bg3:#081840;--bg4:#0c2050;
    --border:#0e2c60;--border2:#143870;
    --accent:#00e4b4;--accent2:#40b8ff;--warn:#ffb800;--danger:#ff4060;
    --text:#e0f4ff;--text2:#80c8f0;--text3:#204860;
  }

  *{box-sizing:border-box;margin:0;padding:0;}
  html,body{height:100%;overflow-x:hidden;}
  body{
    background:var(--bg);color:var(--text);
    font-family:var(--sans);font-size:14px;
    min-height:100vh;display:flex;flex-direction:column;
    background-image:
      linear-gradient(rgba(0,200,150,0.025) 1px,transparent 1px),
      linear-gradient(90deg,rgba(0,200,150,0.025) 1px,transparent 1px);
    background-size:40px 40px;
  }

  /* ── Header ── */
  header{
    background:var(--bg2);border-bottom:1px solid var(--border);
    padding:0 16px;display:flex;align-items:center;
    gap:10px;height:52px;flex-shrink:0;
    position:relative;overflow:hidden;
  }
  header::before{
    content:'';position:absolute;inset:0;
    background:repeating-linear-gradient(0deg,transparent,transparent 2px,
      rgba(0,200,150,0.018) 2px,rgba(0,200,150,0.018) 4px);
    pointer-events:none;
  }
  .logo-block{display:flex;align-items:center;gap:9px;flex-shrink:0;}
  .logo-icon{
    width:26px;height:26px;border:2px solid var(--accent);border-radius:3px;
    display:flex;align-items:center;justify-content:center;
    font-size:11px;color:var(--accent);font-family:var(--mono);font-weight:600;
    box-shadow:0 0 10px rgba(0,200,150,0.25),inset 0 0 8px rgba(0,200,150,0.08);flex-shrink:0;
  }
  .logo-name{font-family:var(--mono);font-size:12px;font-weight:600;color:var(--accent);letter-spacing:0.04em;white-space:nowrap;}
  .logo-sub{font-family:var(--mono);font-size:8px;color:var(--text2);letter-spacing:0.1em;text-transform:uppercase;white-space:nowrap;}
  .hdiv{width:1px;height:24px;background:var(--border2);flex-shrink:0;}

  /* BTS IP block */
  .bts-ip-block{display:flex;flex-direction:column;gap:1px;flex-shrink:0;}
  .bts-ip-label{font-family:var(--mono);font-size:8px;letter-spacing:0.15em;text-transform:uppercase;color:var(--text3);}
  .bts-ip-value{font-family:var(--mono);font-size:11px;font-weight:600;color:var(--text2);letter-spacing:0.04em;}
  .bts-ip-value.online{color:var(--accent);}

  /* Status indicators */
  .status-group{display:flex;align-items:center;gap:12px;}
  .link-status{display:flex;align-items:center;gap:5px;}
  .status-led{width:7px;height:7px;border-radius:50%;background:var(--danger);transition:all 0.4s;flex-shrink:0;}
  .status-led.online{background:var(--accent);box-shadow:0 0 8px rgba(0,200,150,0.6);animation:pulse-led 2.5s ease-in-out infinite;}
  .status-led.brew-on{background:var(--accent2);box-shadow:0 0 8px rgba(0,151,255,0.6);animation:pulse-led2 2.5s ease-in-out infinite;}
  @keyframes pulse-led{0%,100%{box-shadow:0 0 6px rgba(0,200,150,0.5)}50%{box-shadow:0 0 14px rgba(0,200,150,0.9)}}
  @keyframes pulse-led2{0%,100%{box-shadow:0 0 6px rgba(0,151,255,0.5)}50%{box-shadow:0 0 14px rgba(0,151,255,0.9)}}
  .status-label{font-family:var(--mono);font-size:10px;color:var(--text2);letter-spacing:0.05em;white-space:nowrap;}
  .status-sublabel{font-family:var(--mono);font-size:8px;color:var(--text3);letter-spacing:0.08em;text-transform:uppercase;}

  /* Controls */
  .hdr-controls{display:flex;align-items:center;gap:5px;flex-shrink:0;margin-left:auto;}
  .lang-btns,.theme-btns{display:flex;border:1px solid var(--border2);border-radius:var(--r);overflow:hidden;}
  .lang-btn,.theme-btn{
    background:var(--bg3);border:none;color:var(--text2);
    padding:3px 6px;cursor:pointer;
    font-family:var(--mono);font-size:10px;font-weight:500;letter-spacing:0.05em;
    transition:all 0.15s;line-height:1.4;
  }
  .lang-btn+.lang-btn,.theme-btn+.theme-btn{border-left:1px solid var(--border2);}
  .lang-btn:hover,.theme-btn:hover{background:var(--bg4);color:var(--text);}
  .lang-btn.active,.theme-btn.active{background:rgba(0,200,150,0.15);color:var(--accent);}
  .theme-sep{width:1px;height:14px;background:var(--border2);}

  /* Nav */
  nav{display:flex;gap:1px;flex-shrink:0;}
  .tab{
    background:none;border:none;color:var(--text2);padding:6px 12px;
    cursor:pointer;font-family:var(--mono);font-size:10px;font-weight:500;
    letter-spacing:0.08em;text-transform:uppercase;transition:all 0.15s;border-bottom:2px solid transparent;
    white-space:nowrap;
  }
  .tab:hover{color:var(--text);}
  .tab.active{color:var(--accent);border-bottom-color:var(--accent);}

  /* ── Main ── */
  main{padding:14px 16px;max-width:1440px;margin:0 auto;width:100%;flex:1;min-width:0;}
  .page{display:none;}
  .page.active{display:block;}

  /* Stats */
  .stats-row{display:grid;grid-template-columns:repeat(2,1fr);gap:10px;margin-bottom:12px;}
  .stat-card{background:var(--bg2);border:1px solid var(--border);border-radius:var(--r);padding:12px 14px;position:relative;overflow:hidden;}
  .stat-card::before{content:'';position:absolute;top:0;left:0;right:0;height:2px;background:var(--accent);opacity:0.5;}
  .stat-card.accent2::before{background:var(--accent2);}
  .stat-lbl{font-family:var(--mono);font-size:8px;letter-spacing:0.15em;text-transform:uppercase;color:var(--text2);margin-bottom:4px;}
  .stat-val{font-family:var(--mono);font-size:24px;font-weight:600;color:var(--text);line-height:1;}
  .stat-val.ok{color:var(--accent);}
  .stat-val.info{color:var(--accent2);}
  .stat-unit{font-size:10px;color:var(--text2);margin-top:3px;font-family:var(--mono);}

  /* Card */
  .card{background:var(--bg2);border:1px solid var(--border);border-radius:var(--r);padding:14px 16px;}
  .card+.card{margin-top:10px;}
  .card-header{display:flex;align-items:center;margin-bottom:12px;gap:8px;}
  .card-title{font-family:var(--mono);font-size:9px;font-weight:600;text-transform:uppercase;letter-spacing:0.15em;color:var(--text2);}
  .card-header-actions{margin-left:auto;display:flex;gap:6px;align-items:center;flex-wrap:wrap;}

  /* Table — responsive wrapper */
  .table-wrap{width:100%;overflow-x:auto;-webkit-overflow-scrolling:touch;}
  .table-wrap::-webkit-scrollbar{height:4px;}
  .table-wrap::-webkit-scrollbar-thumb{background:var(--border2);border-radius:2px;}
  table{width:100%;border-collapse:collapse;min-width:520px;}
  th{text-align:left;font-family:var(--mono);font-size:8px;font-weight:600;text-transform:uppercase;letter-spacing:0.12em;color:var(--text3);padding:5px 8px 7px;border-bottom:1px solid var(--border);white-space:nowrap;}
  td{padding:7px 8px;border-bottom:1px solid var(--border);color:var(--text);font-size:13px;}
  tr:last-child td{border-bottom:none;}
  tr:hover td{background:var(--bg3);}
  td code{font-family:var(--mono);font-size:12px;font-weight:600;color:var(--accent);background:rgba(0,200,150,0.08);padding:1px 5px;border-radius:3px;}

  /* Badges */
  .badge{display:inline-block;padding:1px 6px;border-radius:2px;font-family:var(--mono);font-size:10px;font-weight:500;letter-spacing:0.04em;border:1px solid;}
  .badge-green{background:rgba(0,200,150,0.1);color:var(--accent);border-color:rgba(0,200,150,0.3);}
  .badge-blue{background:rgba(0,151,255,0.1);color:var(--accent2);border-color:rgba(0,151,255,0.3);}
  .badge-yellow{background:rgba(240,168,0,0.1);color:var(--warn);border-color:rgba(240,168,0,0.3);}
  .badge-dim{background:rgba(100,130,150,0.08);color:var(--text2);border-color:var(--border);}

  /* RSSI */
  .rssi-bar{display:flex;align-items:center;gap:6px;}
  .rssi-track{flex:1;height:3px;background:var(--bg4);border-radius:2px;overflow:hidden;min-width:30px;}
  .rssi-fill{height:100%;border-radius:2px;transition:width 0.6s ease;}
  .rssi-val{font-family:var(--mono);font-size:11px;color:var(--text2);width:60px;text-align:right;flex-shrink:0;}

  /* Buttons */
  .btn{background:var(--bg3);border:1px solid var(--border2);color:var(--text2);padding:4px 9px;border-radius:var(--r);cursor:pointer;font-family:var(--mono);font-size:10px;font-weight:500;letter-spacing:0.05em;transition:all 0.15s;white-space:nowrap;}
  .btn:hover{border-color:var(--accent2);color:var(--accent2);}
  .btn-danger:hover{border-color:var(--danger);color:var(--danger);}
  .btn-warn:hover{border-color:var(--warn);color:var(--warn);}
  .btn-primary{background:rgba(0,200,150,0.12);border-color:rgba(0,200,150,0.5);color:var(--accent);}
  .btn-primary:hover{background:rgba(0,200,150,0.2);border-color:var(--accent);}

  /* Log */
  #log-container{
    background:var(--bg);border:1px solid var(--border);border-radius:var(--r);
    height:clamp(260px,50vh,560px);
    overflow-y:auto;padding:10px 12px;font-family:var(--mono);font-size:11.5px;
    scrollbar-width:thin;scrollbar-color:var(--border2) transparent;
  }
  #log-container::-webkit-scrollbar{width:5px;}
  #log-container::-webkit-scrollbar-thumb{background:var(--border2);border-radius:3px;}
  .log-line{padding:1px 0;line-height:1.65;white-space:pre-wrap;word-break:break-all;}
  .log-INFO{color:#5a7a90;}
  .log-WARN{color:var(--warn);}
  .log-ERROR{color:var(--danger);}
  .log-DEBUG{color:var(--text3);}
  .log-ts{color:var(--text3);margin-right:8px;}
  .log-level{margin-right:8px;font-weight:600;min-width:38px;display:inline-block;}
  .log-INFO .log-level{color:var(--accent2);}
  .log-WARN .log-level{color:var(--warn);}
  .log-ERROR .log-level{color:var(--danger);}

  /* Modal */
  .modal-overlay{display:none;position:fixed;inset:0;background:rgba(0,0,0,0.75);backdrop-filter:blur(3px);z-index:200;align-items:center;justify-content:center;padding:16px;}
  .modal-overlay.open{display:flex;}
  .modal{background:var(--bg2);border:1px solid var(--border2);border-radius:var(--r);padding:20px;width:min(420px,100%);box-shadow:0 20px 60px rgba(0,0,0,0.6);}
  .modal-title{font-family:var(--mono);font-size:11px;font-weight:600;letter-spacing:0.1em;text-transform:uppercase;color:var(--accent);margin-bottom:16px;padding-bottom:10px;border-bottom:1px solid var(--border);}
  .form-row{margin-bottom:10px;}
  .form-label{display:block;font-family:var(--mono);font-size:9px;color:var(--text2);margin-bottom:4px;text-transform:uppercase;letter-spacing:0.12em;}
  input[type=text],input[type=number],textarea,select{width:100%;background:var(--bg);border:1px solid var(--border2);color:var(--text);padding:7px 9px;border-radius:var(--r);font-family:var(--mono);font-size:12px;transition:border-color 0.15s;}
  input:focus,textarea:focus{outline:none;border-color:var(--accent);}
  textarea{min-height:clamp(160px,35vh,260px);font-size:12px;resize:vertical;}
  .modal-actions{display:flex;gap:8px;justify-content:flex-end;margin-top:14px;}

  /* Empty */
  .empty-state{text-align:center;padding:32px 16px;color:var(--text3);}
  .empty-icon{font-size:24px;margin-bottom:8px;opacity:0.5;}
  .empty-text{font-family:var(--mono);font-size:11px;letter-spacing:0.08em;}

  /* Footer */
  footer{flex-shrink:0;background:var(--bg2);border-top:1px solid var(--border);padding:0 16px;height:32px;display:flex;align-items:center;gap:10px;font-family:var(--mono);font-size:10px;color:var(--text3);letter-spacing:0.05em;overflow:hidden;}
  .footer-copy span{color:var(--text2);}
  .footer-sep{color:var(--border2);}
  .footer-build{color:var(--text3);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex-shrink:1;min-width:0;}
  .footer-right{margin-left:auto;color:var(--text3);white-space:nowrap;flex-shrink:0;}
  .config-msg{margin-top:8px;font-family:var(--mono);font-size:11px;}
  .actions-cell{display:flex;gap:5px;align-items:center;flex-wrap:wrap;}

  /* ── Responsive ── */
  @media(max-width:640px){
    header{height:auto;min-height:52px;padding:6px 10px;flex-wrap:wrap;gap:6px;}
    .logo-sub,.hdiv,.bts-ip-block{display:none;}
    .hdr-controls{margin-left:0;}
    nav{order:10;width:100%;justify-content:center;border-top:1px solid var(--border);margin:4px -10px 0;padding:3px 0 2px;}
    .tab{padding:4px 9px;font-size:9px;}
    main{padding:8px 10px;}
    .stats-row{gap:8px;}
    .stat-val{font-size:20px;}
    footer{font-size:9px;padding:0 10px;}
    .footer-build{display:none;}
  }
  @media(min-width:641px) and (max-width:900px){
    .logo-sub{display:none;}
    header{padding:0 12px;gap:8px;}
    .tab{padding:6px 9px;font-size:9px;}
  }
</style>
</head>
<body>

<header>
  <div class="logo-block">
    <div class="logo-icon">FS</div>
    <div>
      <div class="logo-name">FlowStation</div>
      <div class="logo-sub">TETRA Base Station</div>
    </div>
  </div>
  <div class="hdiv"></div>
  <div class="bts-ip-block">
    <div class="bts-ip-label" data-i18n="bts_ip">BTS IP</div>
    <div class="bts-ip-value" id="btsIpValue">—</div>
  </div>
  <div class="hdiv"></div>
  <div class="status-group">
    <div class="link-status">
      <div class="status-led" id="statusDot"></div>
      <div>
        <div class="status-sublabel">BS</div>
        <div class="status-label" id="statusText" data-i18n="offline">OFFLINE</div>
      </div>
    </div>
    <div class="link-status">
      <div class="status-led" id="brewDot"></div>
      <div>
        <div class="status-sublabel">BREW</div>
        <div class="status-label" id="brewText">OFFLINE</div>
      </div>
    </div>
  </div>
  <div class="hdr-controls">
    <div class="lang-btns" id="langBtns">
      <button class="lang-btn active" onclick="setLang('en',this)">EN</button>
      <button class="lang-btn" onclick="setLang('ro',this)">RO</button>
      <button class="lang-btn" onclick="setLang('de',this)">DE</button>
      <button class="lang-btn" onclick="setLang('es',this)">ES</button>
    </div>
    <div class="theme-sep"></div>
    <div class="theme-btns" id="themeBtns">
      <button class="theme-btn active" onclick="setTheme('dark',this)" title="Dark">◼</button>
      <button class="theme-btn" onclick="setTheme('white',this)" title="Light">◻</button>
      <button class="theme-btn" onclick="setTheme('blue',this)" title="Blue">◈</button>
    </div>
    <div class="theme-sep"></div>
    <nav>
      <button class="tab active" onclick="showPage('stations',this)" data-i18n-tab="stations">STATIONS</button>
      <button class="tab" onclick="showPage('calls',this)" data-i18n-tab="calls">CALLS</button>
      <button class="tab" onclick="showPage('log',this)" data-i18n-tab="log">LOG</button>
      <button class="tab" onclick="showPage('config',this)" data-i18n-tab="config">CONFIG</button>
    </nav>
  </div>
</header>

<main>

<div class="page active" id="page-stations">
  <div class="stats-row">
    <div class="stat-card">
      <div class="stat-lbl" data-i18n="terminals">Terminals</div>
      <div class="stat-val ok" id="stat-ms">0</div>
      <div class="stat-unit" data-i18n="registered">registered</div>
    </div>
    <div class="stat-card accent2">
      <div class="stat-lbl" data-i18n="active_calls">Active Calls</div>
      <div class="stat-val info" id="stat-calls">0</div>
      <div class="stat-unit" data-i18n="circuits">circuits in use</div>
    </div>
  </div>
  <div class="card">
    <div class="card-header">
      <div class="card-title" data-i18n="registered_terminals">Registered Terminals</div>
    </div>
    <div class="table-wrap">
      <table>
        <thead><tr>
          <th data-i18n="th_issi">ISSI</th>
          <th data-i18n="th_groups">Groups</th>
          <th data-i18n="th_ee">EE</th>
          <th data-i18n="th_signal">Signal</th>
          <th data-i18n="th_status">Status</th>
          <th data-i18n="th_last_seen">Last seen</th>
          <th data-i18n="th_actions">Actions</th>
        </tr></thead>
        <tbody id="ms-tbody">
          <tr><td colspan="7"><div class="empty-state"><div class="empty-icon">📡</div><div class="empty-text" data-i18n="no_terminals">No terminals registered</div></div></td></tr>
        </tbody>
      </table>
    </div>
  </div>
</div>

<div class="page" id="page-calls">
  <div class="card">
    <div class="card-header">
      <div class="card-title" data-i18n="active_calls">Active Calls</div>
    </div>
    <div class="table-wrap">
      <table>
        <thead><tr>
          <th data-i18n="th_id">ID</th>
          <th data-i18n="th_type">Type</th>
          <th data-i18n="th_caller">Caller</th>
          <th data-i18n="th_dest">Destination</th>
          <th data-i18n="th_speaker">Speaker</th>
          <th data-i18n="th_duration">Duration</th>
        </tr></thead>
        <tbody id="calls-tbody">
          <tr><td colspan="6"><div class="empty-state"><div class="empty-icon">☎</div><div class="empty-text" data-i18n="no_calls">No active calls</div></div></td></tr>
        </tbody>
      </table>
    </div>
  </div>
</div>

<div class="page" id="page-log">
  <div class="card">
    <div class="card-header">
      <div class="card-title" data-i18n="live_log">Live Log</div>
      <div class="card-header-actions">
        <label style="display:flex;align-items:center;gap:5px;font-family:var(--mono);font-size:11px;color:var(--text2);cursor:pointer">
          <input type="checkbox" id="log-autoscroll" checked style="width:auto;accent-color:var(--accent)">
          <span data-i18n="autoscroll">Auto-scroll</span>
        </label>
        <select id="log-filter" style="background:var(--bg);border:1px solid var(--border2);color:var(--text2);padding:2px 6px;font-family:var(--mono);font-size:11px;border-radius:var(--r);width:auto">
          <option value="" data-i18n="filter_all">All</option>
          <option value="INFO">INFO+</option>
          <option value="WARN">WARN+</option>
          <option value="ERROR">ERROR</option>
        </select>
        <button class="btn" onclick="clearLog()" data-i18n="clear">Clear</button>
      </div>
    </div>
    <div id="log-container"></div>
  </div>
</div>

<div class="page" id="page-config">
  <div class="card">
    <div class="card-header">
      <div class="card-title">config.toml</div>
      <div class="card-header-actions">
        <button class="btn btn-warn" onclick="restartService()" data-i18n="restart">⟳ Restart</button>
        <button class="btn btn-primary" onclick="saveConfig()" data-i18n="save">Save</button>
      </div>
    </div>
    <textarea id="config-editor" spellcheck="false" placeholder="Loading..."></textarea>
    <div class="config-msg" id="config-msg"></div>
  </div>
</div>

</main>

<footer>
  <span class="footer-copy">© 2025 <span>Razvan Zeces — YO6RZV</span></span>
  <span class="footer-sep">|</span>
  <span class="footer-build" id="footer-build-str">—</span>
  <span class="footer-right">TETRA FlowStation v0.0.9</span>
</footer>

<div class="modal-overlay" id="sds-modal">
  <div class="modal">
    <div class="modal-title" data-i18n="sds_title">⬡ Send SDS Message</div>
    <div class="form-row">
      <label class="form-label" data-i18n="sds_dest">Destination ISSI</label>
      <input type="number" id="sds-dest" placeholder="e.g. 2260571">
    </div>
    <div class="form-row">
      <label class="form-label" data-i18n="sds_msg_label">Message</label>
      <input type="text" id="sds-msg" placeholder="..." maxlength="160">
    </div>
    <div class="modal-actions">
      <button class="btn" onclick="closeSdsModal()" data-i18n="cancel">Cancel</button>
      <button class="btn btn-primary" onclick="sendSds()" data-i18n="send">Send</button>
    </div>
  </div>
</div>

<script>
const LANGS={
  en:{
    bts_ip:'BTS IP',offline:'OFFLINE',online:'ONLINE',
    brew_online:'ONLINE',brew_offline:'OFFLINE',
    stations:'STATIONS',calls:'CALLS',log:'LOG',config:'CONFIG',
    terminals:'Terminals',registered:'registered',
    active_calls:'Active Calls',circuits:'circuits in use',
    registered_terminals:'Registered Terminals',
    no_terminals:'No terminals registered',no_calls:'No active calls',
    live_log:'Live Log',autoscroll:'Auto-scroll',filter_all:'All',
    clear:'Clear',restart:'⟳ Restart',save:'Save',
    sds_title:'⬡ Send SDS Message',sds_dest:'Destination ISSI',
    sds_msg_label:'Message',cancel:'Cancel',send:'Send',
    th_issi:'ISSI',th_groups:'Groups',th_ee:'EE',th_signal:'Signal',
    th_status:'Status',th_last_seen:'Last seen',th_actions:'Actions',
    th_id:'ID',th_type:'Type',th_caller:'Caller',
    th_dest:'Destination',th_speaker:'Speaker',th_duration:'Duration',
    online_badge:'ONLINE',kick:'Kick',sds:'SDS',
    call_group:'GROUP',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',
    confirm_kick:'Kick ISSI {issi}?\nTerminal will be deregistered and forced to re-attach.',
    confirm_restart:'Restart FlowStation?\nAll active calls will be dropped.',
    saved:'✓ Saved — restart to apply.',save_fail:'✗ Save failed',conn_error:'Connection error.',
  },
  ro:{
    bts_ip:'IP BTS',offline:'DECONECTAT',online:'CONECTAT',
    brew_online:'ONLINE',brew_offline:'OFFLINE',
    stations:'STAȚII',calls:'APELURI',log:'LOG',config:'CONFIG',
    terminals:'Terminale',registered:'înregistrate',
    active_calls:'Apeluri Active',circuits:'circuite active',
    registered_terminals:'Terminale Înregistrate',
    no_terminals:'Nicio stație înregistrată',no_calls:'Niciun apel activ',
    live_log:'Log Live',autoscroll:'Auto-scroll',filter_all:'Toate',
    clear:'Șterge',restart:'⟳ Repornire',save:'Salvează',
    sds_title:'⬡ Trimite Mesaj SDS',sds_dest:'ISSI Destinatar',
    sds_msg_label:'Mesaj',cancel:'Anulează',send:'Trimite',
    th_issi:'ISSI',th_groups:'Grupuri',th_ee:'EE',th_signal:'Semnal',
    th_status:'Status',th_last_seen:'Văzut',th_actions:'Acțiuni',
    th_id:'ID',th_type:'Tip',th_caller:'Apelant',
    th_dest:'Destinatar',th_speaker:'Vorbitor',th_duration:'Durată',
    online_badge:'ONLINE',kick:'Kick',sds:'SDS',
    call_group:'GRUP',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',
    confirm_kick:'Kick ISSI {issi}?\nTerminalul va fi deînregistrat și forțat să se reconecteze.',
    confirm_restart:'Repornire FlowStation?\nToate apelurile active vor fi întrerupte.',
    saved:'✓ Salvat — repornire pentru aplicare.',save_fail:'✗ Salvare eșuată',conn_error:'Eroare de conexiune.',
  },
  de:{
    bts_ip:'BTS-IP',offline:'OFFLINE',online:'ONLINE',
    brew_online:'ONLINE',brew_offline:'OFFLINE',
    stations:'STATIONEN',calls:'ANRUFE',log:'LOG',config:'CONFIG',
    terminals:'Terminals',registered:'registriert',
    active_calls:'Aktive Anrufe',circuits:'Schaltkreise aktiv',
    registered_terminals:'Registrierte Terminals',
    no_terminals:'Keine Terminals registriert',no_calls:'Keine aktiven Anrufe',
    live_log:'Live-Log',autoscroll:'Auto-Scroll',filter_all:'Alle',
    clear:'Löschen',restart:'⟳ Neustart',save:'Speichern',
    sds_title:'⬡ SDS-Nachricht senden',sds_dest:'Ziel-ISSI',
    sds_msg_label:'Nachricht',cancel:'Abbrechen',send:'Senden',
    th_issi:'ISSI',th_groups:'Gruppen',th_ee:'EE',th_signal:'Signal',
    th_status:'Status',th_last_seen:'Zuletzt',th_actions:'Aktionen',
    th_id:'ID',th_type:'Typ',th_caller:'Anrufer',
    th_dest:'Ziel',th_speaker:'Sprecher',th_duration:'Dauer',
    online_badge:'ONLINE',kick:'Entfernen',sds:'SDS',
    call_group:'GRUPPE',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',
    confirm_kick:'ISSI {issi} entfernen?\nDas Terminal wird abgemeldet und zur Neuanmeldung gezwungen.',
    confirm_restart:'FlowStation neu starten?\nAlle aktiven Anrufe werden beendet.',
    saved:'✓ Gespeichert — Neustart zum Anwenden.',save_fail:'✗ Fehler beim Speichern',conn_error:'Verbindungsfehler.',
  },
  es:{
    bts_ip:'IP BTS',offline:'SIN CONEXIÓN',online:'EN LÍNEA',
    brew_online:'EN LÍNEA',brew_offline:'SIN CONEXIÓN',
    stations:'ESTACIONES',calls:'LLAMADAS',log:'LOG',config:'CONFIG',
    terminals:'Terminales',registered:'registrados',
    active_calls:'Llamadas Activas',circuits:'circuitos en uso',
    registered_terminals:'Terminales Registrados',
    no_terminals:'No hay terminales registrados',no_calls:'No hay llamadas activas',
    live_log:'Log en Vivo',autoscroll:'Auto-desplaz.',filter_all:'Todos',
    clear:'Limpiar',restart:'⟳ Reiniciar',save:'Guardar',
    sds_title:'⬡ Enviar Mensaje SDS',sds_dest:'ISSI Destino',
    sds_msg_label:'Mensaje',cancel:'Cancelar',send:'Enviar',
    th_issi:'ISSI',th_groups:'Grupos',th_ee:'EE',th_signal:'Señal',
    th_status:'Estado',th_last_seen:'Visto',th_actions:'Acciones',
    th_id:'ID',th_type:'Tipo',th_caller:'Llamante',
    th_dest:'Destino',th_speaker:'Hablante',th_duration:'Duración',
    online_badge:'EN LÍNEA',kick:'Expulsar',sds:'SDS',
    call_group:'GRUPO',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',
    confirm_kick:'¿Expulsar ISSI {issi}?\nEl terminal será desregistrado y forzado a reconectarse.',
    confirm_restart:'¿Reiniciar FlowStation?\nTodas las llamadas activas se interrumpirán.',
    saved:'✓ Guardado — reinicia para aplicar.',save_fail:'✗ Error al guardar',conn_error:'Error de conexión.',
  },
};

let currentLang=localStorage.getItem('fs_lang')||'en';
function t(k,v){let s=(LANGS[currentLang]||LANGS.en)[k]||(LANGS.en[k]||k);if(v)Object.keys(v).forEach(x=>{s=s.replace('{'+x+'}',v[x]);});return s;}
function applyLang(){
  document.querySelectorAll('[data-i18n]').forEach(el=>el.textContent=t(el.getAttribute('data-i18n')));
  document.querySelectorAll('[data-i18n-tab]').forEach(el=>el.textContent=t(el.getAttribute('data-i18n-tab')));
  const st=document.getElementById('statusText');
  if(st)st.textContent=document.getElementById('statusDot').classList.contains('online')?t('online'):t('offline');
  renderStations();renderCalls();
}
function setLang(l,btn){
  currentLang=l;localStorage.setItem('fs_lang',l);
  document.querySelectorAll('.lang-btn').forEach(b=>b.classList.remove('active'));
  if(btn)btn.classList.add('active');
  else document.querySelectorAll('.lang-btn').forEach(b=>{if(b.textContent.toLowerCase()===l)b.classList.add('active');});
  applyLang();
}

let currentTheme=localStorage.getItem('fs_theme')||'dark';
function setTheme(theme,btn){
  currentTheme=theme;localStorage.setItem('fs_theme',theme);
  document.documentElement.setAttribute('data-theme',theme==='dark'?'':theme);
  document.querySelectorAll('.theme-btn').forEach(b=>b.classList.remove('active'));
  if(btn)btn.classList.add('active');
  else{const m={'dark':0,'white':1,'blue':2};const bs=document.querySelectorAll('.theme-btn');if(bs[m[theme]])bs[m[theme]].classList.add('active');}
}

// Build info
(function(){
  try{
    const ua=navigator.userAgent;
    let os='—';
    if(/Windows NT ([\d.]+)/.test(ua)){const v=ua.match(/Windows NT ([\d.]+)/)[1];os={'10.0':'Win10','11.0':'Win11','6.3':'Win8.1','6.1':'Win7'}[v]||'Windows';}
    else if(/Mac OS X ([\d_]+)/.test(ua))os='macOS '+ua.match(/Mac OS X ([\d_]+)/)[1].replace(/_/g,'.');
    else if(/Android ([\d.]+)/.test(ua))os='Android '+ua.match(/Android ([\d.]+)/)[1];
    else if(/Linux/.test(ua))os='Linux';
    let br='—';
    if(/Firefox\/([\d.]+)/.test(ua))br='Firefox/'+ua.match(/Firefox\/([\d.]+)/)[1];
    else if(/Edg\/([\d.]+)/.test(ua))br='Edge/'+ua.match(/Edg\/([\d.]+)/)[1];
    else if(/Chrome\/([\d.]+)/.test(ua))br='Chrome/'+ua.match(/Chrome\/([\d.]+)/)[1];
    else if(/Safari\/([\d.]+)/.test(ua))br='Safari/'+ua.match(/Safari\/([\d.]+)/)[1];
    document.getElementById('footer-build-str').textContent=os+' · '+br;
  }catch(e){}
})();

let ws=null,state={ms:{},calls:{},brewOnline:false},sdsDest=0;
const logFilter=()=>document.getElementById('log-filter').value;

function setBrewStatus(online){
  state.brewOnline=online;
  const dot=document.getElementById('brewDot');
  const txt=document.getElementById('brewText');
  if(online){
    dot.classList.add('brew-on');
    txt.textContent=t('brew_online');
    txt.style.color='var(--accent2)';
  } else {
    dot.classList.remove('brew-on');
    txt.textContent=t('brew_offline');
    txt.style.color='';
  }
}

function showPage(name,btn){
  document.querySelectorAll('.page').forEach(p=>p.classList.remove('active'));
  document.querySelectorAll('.tab').forEach(t=>t.classList.remove('active'));
  document.getElementById('page-'+name).classList.add('active');
  if(btn)btn.classList.add('active');
  if(name==='config')loadConfig();
}

function connect(){
  const proto=location.protocol==='https:'?'wss:':'ws:';
  ws=new WebSocket(`${proto}//${location.host}/ws`);
  ws.onopen=()=>{
    document.getElementById('statusDot').classList.add('online');
    document.getElementById('statusText').textContent=t('online');
    const ip=document.getElementById('btsIpValue');
    ip.textContent=location.hostname;ip.classList.add('online');
    ws.send(JSON.stringify({type:'subscribe'}));
  };
  ws.onclose=()=>{
    document.getElementById('statusDot').classList.remove('online');
    document.getElementById('statusText').textContent=t('offline');
    document.getElementById('btsIpValue').classList.remove('online');
    setBrewStatus(false);
    setTimeout(connect,3000);
  };
  ws.onmessage=(e)=>{try{handleMsg(JSON.parse(e.data));}catch{}};
}

function handleMsg(msg){
  switch(msg.type){
    case 'snapshot':
      state.ms={};state.calls={};
      (msg.ms||[]).forEach(m=>{state.ms[m.issi]={...m,_last_seen_ts:Date.now()-(m.last_seen_secs_ago||0)*1000,energy_saving_mode:m.energy_saving_mode||0};});
      (msg.calls||[]).forEach(c=>{state.calls[c.call_id]={...c,started_at:Date.now()-(c.started_secs_ago||0)*1000};});
      if(msg.log&&msg.log.length){document.getElementById('log-container').innerHTML='';msg.log.forEach(e=>appendLog(e));}
      setBrewStatus(!!msg.brew_online);
      renderAll();break;
    case 'brew_status':
      setBrewStatus(!!msg.connected);break;
    case 'ms_registered':
      state.ms[msg.issi]=Object.assign({issi:msg.issi,groups:[],rssi_dbfs:null,energy_saving_mode:0},state.ms[msg.issi]||{},{issi:msg.issi,_last_seen_ts:Date.now()});
      renderStations();break;
    case 'ms_deregistered':
      delete state.ms[msg.issi];renderStations();break;
    case 'ms_rssi':
      if(state.ms[msg.issi]){state.ms[msg.issi].rssi_dbfs=msg.rssi_dbfs;state.ms[msg.issi]._last_seen_ts=Date.now();}
      renderStations();break;
    case 'ms_groups':
      if(state.ms[msg.issi]){const cur=new Set(state.ms[msg.issi].groups||[]);(msg.groups||[]).forEach(g=>cur.add(g));state.ms[msg.issi].groups=[...cur];}
      renderStations();break;
    case 'ms_groups_detach':
      if(state.ms[msg.issi]){const rem=new Set(msg.groups||[]);state.ms[msg.issi].groups=(state.ms[msg.issi].groups||[]).filter(g=>!rem.has(g));}
      renderStations();break;
    case 'ms_groups_all':
      if(state.ms[msg.issi])state.ms[msg.issi].groups=msg.groups||[];
      renderStations();break;
    case 'call_started':
      state.calls[msg.call_id]={...msg,started_at:Date.now()};renderCalls();break;
    case 'call_ended':
      delete state.calls[msg.call_id];renderCalls();break;
    case 'speaker_changed':
      if(state.calls[msg.call_id])state.calls[msg.call_id].active_speaker=msg.speaker_issi;
      renderCalls();break;
    case 'ms_energy_saving':
      if(state.ms[msg.issi])state.ms[msg.issi].energy_saving_mode=msg.mode;
      renderStations();break;
    case 'log':appendLog(msg);break;
  }
}

function eeLabel(mode){
  if(!mode||mode===0)return '<span style="color:var(--text3);font-size:10px">—</span>';
  const labels=['','EG1','EG2','EG3','EG4','EG5','EG6','EG7'];
  const colors=['','var(--accent)','var(--accent)','var(--accent2)','var(--accent2)','var(--warn)','var(--danger)','var(--danger)'];
  const tips=['','~1s','~2s','~3s','~4s','~5s','~6s','~7s'];
  const col=colors[mode]||'var(--text2)';
  return `<span class="badge" title="Energy Economy Mode ${mode} — wake interval ${tips[mode]}" style="background:color-mix(in srgb,${col} 12%,transparent);border-color:${col};color:${col};font-size:9px">${labels[mode]}</span>`;
}
function lastSeenLabel(secs){
  if(secs==null)return '—';
  if(secs<5)return '<span style="color:var(--accent)">now</span>';
  if(secs<60)return `<span style="color:var(--accent2)">${secs}s</span>`;
  if(secs<3600)return `<span style="color:var(--text2)">${Math.floor(secs/60)}m${secs%60}s</span>`;
  return `<span style="color:var(--warn)">${Math.floor(secs/3600)}h${Math.floor((secs%3600)/60)}m</span>`;
}
function renderAll(){renderStations();renderCalls();}
function rssiColor(v){if(v==null)return'var(--text3)';if(v>-20)return'var(--accent)';if(v>-30)return'var(--accent2)';if(v>-40)return'var(--warn)';return'var(--danger)';}
function rssiPct(v){if(v==null)return 0;return Math.max(0,Math.min(100,(v+60)/50*100));}

function renderStations(){
  const ms=Object.values(state.ms);
  document.getElementById('stat-ms').textContent=ms.length;
  document.getElementById('stat-calls').textContent=Object.keys(state.calls).length;
  const tb=document.getElementById('ms-tbody');
  if(!ms.length){tb.innerHTML=`<tr><td colspan="7"><div class="empty-state"><div class="empty-icon">📡</div><div class="empty-text">${t('no_terminals')}</div></div></td></tr>`;return;}
  tb.innerHTML=ms.sort((a,b)=>a.issi-b.issi).map(m=>{
    const r=m.rssi_dbfs,rL=r!=null?`${r.toFixed(1)} dBFS`:'—',pct=rssiPct(r),col=rssiColor(r);
    let grps;
    if((m.groups||[]).length>1){
      const gList=(m.groups||[]).map(g=>`<span class="badge badge-dim" style="font-size:9px">${g}</span>`).join(' ');
      grps=`<span class="badge" style="background:rgba(255,165,0,0.15);color:#ffaa00;border-color:rgba(255,165,0,0.4);font-weight:700;font-size:9px;margin-right:4px">⚡ SCAN</span>${gList}`;
    } else if((m.groups||[]).length===1){
      grps=`<span class="badge badge-blue">${m.groups[0]}</span>`;
    } else {
      grps='<span class="badge badge-dim">—</span>';
    }
    const ls=m._last_seen_ts?Math.floor((Date.now()-m._last_seen_ts)/1000):m.last_seen_secs_ago;
    return `<tr>
      <td><code>${m.issi}</code></td><td>${grps}</td>
      <td style="text-align:center">${eeLabel(m.energy_saving_mode||0)}</td>
      <td><div class="rssi-bar"><div class="rssi-track"><div class="rssi-fill" style="width:${pct}%;background:${col}"></div></div><span class="rssi-val" style="color:${col}">${rL}</span></div></td>
      <td><span class="badge badge-green">${t('online_badge')}</span></td>
      <td style="font-family:var(--mono);font-size:11px">${lastSeenLabel(ls)}</td>
      <td class="actions-cell"><button class="btn" onclick="openSds(${m.issi})">${t('sds')}</button><button class="btn btn-danger" onclick="kickMs(${m.issi})">${t('kick')}</button></td>
    </tr>`;
  }).join('');
}

function renderCalls(){
  document.getElementById('stat-calls').textContent=Object.keys(state.calls).length;
  const tb=document.getElementById('calls-tbody'),calls=Object.values(state.calls);
  if(!calls.length){tb.innerHTML=`<tr><td colspan="6"><div class="empty-state"><div class="empty-icon">☎</div><div class="empty-text">${t('no_calls')}</div></div></td></tr>`;return;}
  tb.innerHTML=calls.map(c=>{
    const dur=Math.floor((Date.now()-(c.started_at||Date.now()))/1000);
    const mm=String(Math.floor(dur/60)).padStart(2,'0'),ss=String(dur%60).padStart(2,'0');
    const badge=c.call_type==='group'?'badge-blue':'badge-yellow';
    const label=c.call_type==='group'?t('call_group'):(c.simplex?t('call_p2p_s'):t('call_p2p_d'));
    const to=c.call_type==='group'?`GSSI ${c.gssi}`:`ISSI ${c.called_issi}`;
    const spk=c.active_speaker?`<code>${c.active_speaker}</code>`:'<span style="color:var(--text3)">—</span>';
    return `<tr><td><code>${c.call_id}</code></td><td><span class="badge ${badge}">${label}</span></td><td>${c.caller_issi?`<code>${c.caller_issi}</code>`:'—'}</td><td>${to}</td><td>${spk}</td><td style="font-family:var(--mono);font-size:12px;color:var(--accent2)">${mm}:${ss}</td></tr>`;
  }).join('');
}

function appendLog(msg){
  const f=logFilter(),lv={'':0,DEBUG:0,INFO:1,WARN:2,ERROR:3};
  if((lv[msg.level]??0)<(lv[f]??0))return;
  const c=document.getElementById('log-container'),l=document.createElement('div');
  l.className=`log-line log-${msg.level}`;
  l.innerHTML=`<span class="log-ts">${msg.ts}</span><span class="log-level">${msg.level}</span>${escHtml(msg.msg)}`;
  c.appendChild(l);
  if(c.children.length>600)c.removeChild(c.firstChild);
  if(document.getElementById('log-autoscroll').checked)c.scrollTop=c.scrollHeight;
}
function clearLog(){document.getElementById('log-container').innerHTML='';}
function escHtml(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');}

async function loadConfig(){
  try{const r=await fetch('/api/config');if(r.ok)document.getElementById('config-editor').value=await r.text();else setConfigMsg(t('conn_error'),false);}
  catch{setConfigMsg(t('conn_error'),false);}
}
async function saveConfig(){
  try{const r=await fetch('/api/config',{method:'POST',body:document.getElementById('config-editor').value});if(r.ok)setConfigMsg(t('saved'),true);else setConfigMsg(t('save_fail')+': '+await r.text(),false);}
  catch(e){setConfigMsg(t('conn_error'),false);}
}
function setConfigMsg(txt,ok){const el=document.getElementById('config-msg');el.textContent=txt;el.style.color=ok?'var(--accent)':'var(--danger)';}
async function restartService(){if(!confirm(t('confirm_restart')))return;ws&&ws.send(JSON.stringify({type:'restart'}));}
function kickMs(issi){if(!confirm(t('confirm_kick',{issi})))return;ws&&ws.send(JSON.stringify({type:'kick',issi}));}
function openSds(issi){sdsDest=issi;document.getElementById('sds-dest').value=issi;document.getElementById('sds-msg').value='';document.getElementById('sds-modal').classList.add('open');}
function closeSdsModal(){document.getElementById('sds-modal').classList.remove('open');}
function sendSds(){const dest=parseInt(document.getElementById('sds-dest').value),msg=document.getElementById('sds-msg').value.trim();if(!dest||!msg)return;ws&&ws.send(JSON.stringify({type:'sds',dest_issi:dest,message:msg}));closeSdsModal();}

setInterval(()=>{
  if(document.getElementById('page-calls').classList.contains('active'))renderCalls();
  if(document.getElementById('page-stations').classList.contains('active'))renderStations();
},1000);

setLang(currentLang);
setTheme(currentTheme);
connect();
</script>
</body>
</html>"#;
