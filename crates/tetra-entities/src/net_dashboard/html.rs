pub const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0, maximum-scale=1.0">
<title>TETRA FlowStation</title>
<style>
/* ── Reset ── */
*{box-sizing:border-box;margin:0;padding:0;}
html,body{height:100%;overflow:hidden;}

/* ── Themes ── */
:root{
  --bg:      #0f1117;
  --bg2:     #161b24;
  --bg3:     #1c2332;
  --bg4:     #232d3f;
  --border:  #2a3547;
  --border2: #334060;
  --accent:  #00d4a8;
  --accent2: #4da6ff;
  --warn:    #ffb224;
  --danger:  #ff4d6d;
  --text:    #eaf0fb;
  --text2:   #8ba3c4;
  --text3:   #3d5270;
  --sidebar: #0b0f16;
  --sidebar-border: #1a2236;
  --card-shadow: 0 1px 3px rgba(0,0,0,0.4);
  --r: 8px;
  --mono: 'ui-monospace','Cascadia Code','Consolas','Liberation Mono','Menlo',monospace;
  --sans: 'ui-sans-serif', system-ui, -apple-system, 'Segoe UI', 'Microsoft YaHei', 'Noto Sans SC', 'PingFang SC', 'Hiragino Sans GB', 'WenQuanYi Micro Hei', sans-serif;
}
[data-theme="light"]{
  --bg:#f0f4f9;--bg2:#ffffff;--bg3:#e8eef6;--bg4:#dce4ef;
  --border:#c8d5e8;--border2:#b0c4da;
  --accent:#007a62;--accent2:#0066cc;--warn:#b06000;--danger:#c0203a;
  --text:#0d1829;--text2:#3a5a7a;--text3:#9ab0c8;
  --sidebar:#1a2540;--sidebar-border:#253050;
  --card-shadow:0 1px 4px rgba(0,0,0,0.1);
}
[data-theme="blue"]{
  --bg:#03071e;--bg2:#060d2a;--bg3:#091235;--bg4:#0d1840;
  --border:#112060;--border2:#1a2e7a;
  --accent:#00f5d4;--accent2:#60b8ff;--warn:#ffc947;--danger:#ff5577;
  --text:#deeeff;--text2:#7ab0e0;--text3:#1a3a60;
  --sidebar:#020514;--sidebar-border:#0c1840;
  --card-shadow:0 1px 3px rgba(0,0,200,0.15);
}

/* ── Layout shell ── */
body{
  background:var(--bg);color:var(--text);
  font-family:var(--sans);font-size:14px;
  display:flex;height:100vh;overflow:hidden;
}

/* ── Sidebar ── */
#sidebar{
  width:220px;min-width:220px;
  background:var(--sidebar);
  border-right:1px solid var(--sidebar-border);
  display:flex;flex-direction:column;
  transition:width 0.2s ease,min-width 0.2s ease;
  overflow:hidden;
  z-index:100;
  flex-shrink:0;
}
#sidebar.collapsed{width:56px;min-width:56px;}

.sidebar-logo{
  padding:18px 16px 14px;
  border-bottom:1px solid var(--sidebar-border);
  display:flex;align-items:center;gap:10px;
  flex-shrink:0;
}
.logo-icon{
  width:28px;height:28px;border-radius:6px;
  background:linear-gradient(135deg,var(--accent),var(--accent2));
  display:flex;align-items:center;justify-content:center;
  font-size:14px;font-weight:900;color:#000;flex-shrink:0;
  font-family:var(--mono);letter-spacing:-1px;
}
.logo-text{
  overflow:hidden;white-space:nowrap;
  transition:opacity 0.15s;
}
.logo-text .logo-name{font-size:13px;font-weight:700;color:var(--text);letter-spacing:0.02em;}
.logo-text .logo-sub{font-size:10px;color:var(--text3);letter-spacing:0.08em;font-family:var(--mono);}
#sidebar.collapsed .logo-text{opacity:0;width:0;pointer-events:none;}

.sidebar-nav{
  flex:1;padding:8px 8px;overflow-y:auto;overflow-x:hidden;
}
.sidebar-nav::-webkit-scrollbar{width:3px;}
.sidebar-nav::-webkit-scrollbar-thumb{background:var(--border);}

.nav-section-label{
  font-size:9px;font-weight:600;letter-spacing:0.12em;text-transform:uppercase;
  color:var(--text3);padding:10px 8px 4px;
  white-space:nowrap;overflow:hidden;
  transition:opacity 0.15s;
}
#sidebar.collapsed .nav-section-label{opacity:0;}

.nav-item{
  display:flex;align-items:center;gap:10px;
  padding:8px 8px;border-radius:6px;cursor:pointer;
  color:var(--text2);font-size:13px;font-weight:500;
  transition:all 0.15s;white-space:nowrap;
  border:1px solid transparent;
  margin-bottom:2px;
  text-decoration:none;user-select:none;
}
.nav-item:hover{background:var(--bg3);color:var(--text);}
.nav-item.active{
  background:rgba(0,212,168,0.1);
  border-color:rgba(0,212,168,0.2);
  color:var(--accent);
}
[data-theme="light"] .nav-item.active{background:rgba(0,122,98,0.08);border-color:rgba(0,122,98,0.2);}
.nav-icon{font-size:16px;width:20px;text-align:center;flex-shrink:0;}
.nav-label{overflow:hidden;transition:opacity 0.15s,width 0.15s;}
#sidebar.collapsed .nav-label{opacity:0;width:0;}

.nav-badge{
  margin-left:auto;min-width:18px;height:18px;
  background:rgba(0,212,168,0.15);color:var(--accent);
  border-radius:9px;font-size:10px;font-weight:700;font-family:var(--mono);
  display:flex;align-items:center;justify-content:center;padding:0 5px;
  transition:opacity 0.15s;
}
#sidebar.collapsed .nav-badge{opacity:0;pointer-events:none;}

.sidebar-footer{
  border-top:1px solid var(--sidebar-border);
  padding:10px 8px;
  display:flex;flex-direction:column;gap:6px;
  flex-shrink:0;
}
.sidebar-copyright{
  overflow:hidden;padding:0 4px;
  transition:opacity 0.15s;
}
.sidebar-copyright .cr-line{
  font-family:var(--mono);font-size:9px;color:var(--text3);
  letter-spacing:0.04em;white-space:nowrap;line-height:1.6;
}
.sidebar-copyright .cr-line a{color:var(--text3);text-decoration:none;}
.sidebar-copyright .cr-line a:hover{color:var(--text2);}
#sidebar.collapsed .sidebar-copyright{opacity:0;pointer-events:none;}

/* Brew status in sidebar footer */
.brew-status-row{
  display:flex;align-items:center;gap:8px;
  padding:6px 8px;border-radius:6px;
  background:var(--bg3);
  border:1px solid var(--border);
  overflow:hidden;
}
.brew-led{width:7px;height:7px;border-radius:50%;background:var(--danger);flex-shrink:0;transition:all 0.4s;}
.brew-led.on{background:var(--accent2);box-shadow:0 0 6px rgba(77,166,255,0.6);}
.brew-info{overflow:hidden;flex:1;}
.brew-info-label{font-size:9px;color:var(--text3);letter-spacing:0.1em;font-family:var(--mono);white-space:nowrap;}
.brew-info-val{font-size:11px;font-weight:600;color:var(--text2);white-space:nowrap;font-family:var(--mono);}
.brew-ver-badge{
  font-size:9px;font-weight:700;font-family:var(--mono);
  padding:1px 5px;border-radius:3px;
  flex-shrink:0;display:none;
}
#sidebar.collapsed .brew-info,.brew-ver-badge-wrap{transition:opacity 0.15s;}
#sidebar.collapsed .brew-info{opacity:0;width:0;}

/* Connection dot in footer */
.conn-status-row{
  display:flex;align-items:center;gap:8px;
  padding:4px 8px;
  overflow:hidden;
}
.conn-led{width:7px;height:7px;border-radius:50%;background:var(--danger);flex-shrink:0;transition:all 0.4s;}
.conn-led.on{background:var(--accent);box-shadow:0 0 6px rgba(0,212,168,0.5);animation:pulse 2.5s ease-in-out infinite;}
@keyframes pulse{0%,100%{opacity:1;}50%{opacity:0.6;}}
.conn-info{overflow:hidden;flex:1;}
.conn-info-label{font-size:9px;color:var(--text3);letter-spacing:0.1em;font-family:var(--mono);white-space:nowrap;}
.conn-info-val{font-size:11px;font-weight:600;white-space:nowrap;font-family:var(--mono);}
#sidebar.collapsed .conn-info{opacity:0;width:0;}

/* Sidebar toggle */
.sidebar-toggle{
  display:flex;align-items:center;justify-content:center;
  width:28px;height:28px;border-radius:6px;
  background:transparent;border:1px solid var(--border);
  color:var(--text3);cursor:pointer;font-size:14px;
  transition:all 0.15s;flex-shrink:0;
}
.sidebar-toggle:hover{background:var(--bg3);color:var(--text);}

/* ── Main area ── */
#main{
  flex:1;display:flex;flex-direction:column;overflow:hidden;min-width:0;
}

/* ── Topbar ── */
#topbar{
  height:52px;
  background:var(--bg2);
  border-bottom:1px solid var(--border);
  display:flex;align-items:center;
  padding:0 20px;gap:12px;
  flex-shrink:0;
}
.topbar-title{
  font-size:15px;font-weight:700;color:var(--text);
  letter-spacing:-0.01em;
}
.topbar-sep{color:var(--border2);margin:0 2px;}
.topbar-sub{font-size:12px;color:var(--text3);font-family:var(--mono);}
.topbar-right{margin-left:auto;display:flex;align-items:center;gap:8px;}

/* Theme picker */
.theme-picker{display:flex;border:1px solid var(--border);border-radius:6px;overflow:hidden;}
.theme-btn{
  padding:4px 9px;cursor:pointer;background:transparent;border:none;
  font-family:var(--mono);font-size:10px;font-weight:600;letter-spacing:0.04em;
  color:var(--text3);transition:all 0.15s;
}
.theme-btn+.theme-btn{border-left:1px solid var(--border);}
.theme-btn:hover{color:var(--text);background:var(--bg3);}
.theme-btn.active{color:var(--accent);background:rgba(0,212,168,0.08);}

/* Lang picker */
.lang-picker{display:flex;gap:2px;}
.lang-btn{
  padding:3px 6px;border-radius:4px;cursor:pointer;
  font-family:var(--mono);font-size:10px;font-weight:600;
  color:var(--text3);background:transparent;
  border:1px solid transparent;
  transition:all 0.15s;
}
.lang-btn:hover{color:var(--text);background:var(--bg3);}
.lang-btn.active{color:var(--accent);background:rgba(0,212,168,0.08);border-color:rgba(0,212,168,0.2);}

/* ── Content area ── */
#content{
  flex:1;overflow-y:auto;overflow-x:hidden;
  padding:20px;
}
#content::-webkit-scrollbar{width:6px;}
#content::-webkit-scrollbar-thumb{background:var(--border);border-radius:3px;}

/* Page sections */
.page{display:none;}
.page.active{display:block;}

/* ── Stat cards ── */
.stat-grid{
  display:grid;
  grid-template-columns:repeat(auto-fit,minmax(160px,1fr));
  gap:14px;margin-bottom:20px;
}
.stat-card{
  background:var(--bg2);
  border:1px solid var(--border);
  border-radius:var(--r);
  padding:16px 18px;
  position:relative;
  overflow:hidden;
  box-shadow:var(--card-shadow);
}
.stat-card::before{
  content:'';position:absolute;top:0;left:0;right:0;height:2px;
  background:var(--accent-line,var(--accent));
}
.stat-card.blue::before{--accent-line:var(--accent2);}
.stat-card.warn::before{--accent-line:var(--warn);}
.stat-card.green::before{--accent-line:var(--accent);}
.stat-label{font-size:11px;font-weight:600;letter-spacing:0.08em;text-transform:uppercase;color:var(--text3);margin-bottom:8px;}
.stat-value{font-size:28px;font-weight:700;font-family:var(--mono);color:var(--text);line-height:1;}
.stat-value.accent{color:var(--accent);}
.stat-value.blue{color:var(--accent2);}
.stat-value.warn{color:var(--warn);}
.stat-sub{font-size:11px;color:var(--text3);margin-top:5px;font-family:var(--mono);}
.stat-icon{position:absolute;right:14px;top:50%;transform:translateY(-50%);font-size:28px;opacity:0.07;}

/* ── Cards ── */
.card{
  background:var(--bg2);border:1px solid var(--border);
  border-radius:var(--r);
  box-shadow:var(--card-shadow);
  margin-bottom:16px;overflow:hidden;
}
.card-head{
  display:flex;align-items:center;gap:10px;
  padding:14px 18px 0;
  border-bottom:1px solid var(--border);
  padding-bottom:12px;
}
.card-title{font-size:12px;font-weight:700;letter-spacing:0.08em;text-transform:uppercase;color:var(--text2);}
.card-actions{margin-left:auto;display:flex;gap:6px;align-items:center;flex-wrap:wrap;}
.card-body{padding:0;}

/* ── Table ── */
.table-wrap{width:100%;overflow-x:auto;-webkit-overflow-scrolling:touch;}
.table-wrap::-webkit-scrollbar{height:4px;}
.table-wrap::-webkit-scrollbar-thumb{background:var(--border);border-radius:2px;}
table{width:100%;border-collapse:collapse;}
thead th{
  text-align:left;font-family:var(--mono);font-size:10px;font-weight:600;
  text-transform:uppercase;letter-spacing:0.1em;color:var(--text3);
  padding:10px 16px;border-bottom:1px solid var(--border);
  white-space:nowrap;background:var(--bg2);position:sticky;top:0;z-index:1;
}
tbody td{
  padding:10px 16px;border-bottom:1px solid var(--border);
  color:var(--text);font-size:13px;vertical-align:middle;
}
tbody tr:last-child td{border-bottom:none;}
tbody tr:hover td{background:var(--bg3);}
td code{
  font-family:var(--mono);font-size:12px;font-weight:700;
  color:var(--accent);background:rgba(0,212,168,0.08);
  padding:2px 6px;border-radius:4px;
}
[data-theme="light"] td code{color:var(--accent);background:rgba(0,122,98,0.06);}

/* ── Badges ── */
.badge{
  display:inline-block;padding:2px 7px;border-radius:4px;
  font-family:var(--mono);font-size:10px;font-weight:600;
  letter-spacing:0.04em;border:1px solid;
}
.badge-green{background:rgba(0,212,168,0.1);color:var(--accent);border-color:rgba(0,212,168,0.3);}
.badge-blue{background:rgba(77,166,255,0.1);color:var(--accent2);border-color:rgba(77,166,255,0.3);}
.badge-yellow{background:rgba(255,178,36,0.1);color:var(--warn);border-color:rgba(255,178,36,0.3);}
.badge-dim{background:rgba(100,130,160,0.08);color:var(--text2);border-color:var(--border);}
.badge-red{background:rgba(255,77,109,0.1);color:var(--danger);border-color:rgba(255,77,109,0.3);}

/* ── Buttons ── */
.btn{
  display:inline-flex;align-items:center;gap:5px;
  background:var(--bg3);border:1px solid var(--border2);
  color:var(--text2);padding:5px 11px;border-radius:6px;
  cursor:pointer;font-family:var(--mono);font-size:11px;font-weight:600;
  letter-spacing:0.04em;transition:all 0.15s;white-space:nowrap;
}
.btn:hover{border-color:var(--accent2);color:var(--accent2);background:rgba(77,166,255,0.06);}
.btn-primary{background:rgba(0,212,168,0.1);border-color:rgba(0,212,168,0.4);color:var(--accent);}
.btn-primary:hover{background:rgba(0,212,168,0.18);border-color:var(--accent);}
.btn-danger{color:var(--text2);}
.btn-danger:hover{border-color:var(--danger);color:var(--danger);background:rgba(255,77,109,0.06);}
.btn-warn:hover{border-color:var(--warn);color:var(--warn);}
.btn-sm{padding:3px 8px;font-size:10px;}

/* ── RSSI bar ── */
.rssi-bar{display:flex;align-items:center;gap:8px;}
.rssi-track{width:60px;height:4px;background:var(--bg4);border-radius:2px;overflow:hidden;}
.rssi-fill{height:100%;border-radius:2px;transition:width 0.5s ease;}
.rssi-val{font-family:var(--mono);font-size:11px;color:var(--text2);width:65px;text-align:right;flex-shrink:0;}

/* ── Log ── */
.log-wrap{
  font-family:var(--mono);font-size:11px;line-height:1.7;
  background:var(--bg);padding:12px 16px;
  height:420px;overflow-y:auto;
}
.log-wrap::-webkit-scrollbar{width:4px;}
.log-wrap::-webkit-scrollbar-thumb{background:var(--border);}
.log-line{display:flex;gap:10px;padding:1px 0;}
.log-ts{color:var(--text3);flex-shrink:0;}
.log-level{flex-shrink:0;width:46px;font-weight:700;}
.log-line.log-DEBUG .log-level{color:var(--text3);}
.log-line.log-INFO  .log-level{color:var(--accent2);}
.log-line.log-WARN  .log-level{color:var(--warn);}
.log-line.log-ERROR .log-level{color:var(--danger);}
.log-controls{display:flex;align-items:center;gap:10px;padding:10px 16px;border-top:1px solid var(--border);}
.log-filter{
  background:var(--bg3);border:1px solid var(--border2);color:var(--text);
  padding:4px 8px;border-radius:6px;font-family:var(--mono);font-size:11px;
}
.autoscroll-label{display:flex;align-items:center;gap:5px;font-family:var(--mono);font-size:11px;color:var(--text2);cursor:pointer;}

/* ── Config editor ── */
#config-editor{
  width:100%;height:480px;resize:vertical;
  background:var(--bg);border:none;outline:none;
  font-family:var(--mono);font-size:12px;line-height:1.6;color:var(--text);
  padding:16px;tab-size:2;
}
.config-msg{padding:8px 16px;font-family:var(--mono);font-size:12px;border-top:1px solid var(--border);min-height:34px;}

/* ── Empty state ── */
.empty-state{text-align:center;padding:48px 20px;}
.empty-icon{font-size:32px;margin-bottom:10px;opacity:0.3;}
.empty-text{font-size:13px;color:var(--text3);}

/* ── System info table ── */
.info-row{display:flex;border-bottom:1px solid var(--border);padding:11px 18px;align-items:center;gap:12px;}
.info-row:last-child{border-bottom:none;}
.info-key{font-size:11px;color:var(--text3);font-family:var(--mono);letter-spacing:0.06em;min-width:140px;flex-shrink:0;}
.info-val{font-family:var(--mono);font-size:12px;font-weight:600;color:var(--text);word-break:break-all;}

/* ── Modals ── */
.modal-overlay{
  display:none;position:fixed;inset:0;
  background:rgba(0,0,0,0.7);backdrop-filter:blur(4px);
  z-index:500;align-items:center;justify-content:center;padding:16px;
}
.modal-overlay.open{display:flex;}
.modal{
  background:var(--bg2);border:1px solid var(--border2);
  border-radius:var(--r);padding:22px;
  width:min(440px,100%);
  box-shadow:0 20px 60px rgba(0,0,0,0.5);
}
.modal-title{
  font-family:var(--mono);font-size:12px;font-weight:700;
  letter-spacing:0.1em;text-transform:uppercase;color:var(--accent);
  margin-bottom:18px;padding-bottom:12px;border-bottom:1px solid var(--border);
}
.modal-actions{display:flex;gap:8px;justify-content:flex-end;margin-top:16px;}
.form-row{margin-bottom:12px;}
.form-label{font-family:var(--mono);font-size:10px;font-weight:600;letter-spacing:0.08em;text-transform:uppercase;color:var(--text3);display:block;margin-bottom:5px;}
.form-input{
  width:100%;background:var(--bg3);border:1px solid var(--border2);
  color:var(--text);padding:7px 10px;border-radius:6px;
  font-family:var(--mono);font-size:12px;outline:none;
  transition:border-color 0.15s;
}
.form-input:focus{border-color:var(--accent2);}

/* ── Update modal terminal ── */
.update-terminal{
  background:var(--bg);border:1px solid var(--border);border-radius:6px;
  padding:10px 12px;font-family:var(--mono);font-size:11px;line-height:1.6;
  color:var(--text2);height:300px;overflow-y:auto;white-space:pre-wrap;
  word-break:break-all;margin:12px 0;
}
.update-status{font-family:var(--mono);font-size:11px;font-weight:700;min-height:18px;}
.update-status.running{color:var(--warn);}
.update-status.ok{color:var(--accent);}
.update-status.err{color:var(--danger);}
#update-modal .modal{width:min(680px,100%);}

/* ── Profile list ── */
.profile-item{
  display:flex;align-items:center;gap:10px;
  padding:10px 14px;border:1px solid var(--border);border-radius:6px;
  margin-bottom:8px;background:var(--bg3);
  transition:border-color 0.15s;
}
.profile-item.active-profile{border-color:rgba(0,212,168,0.35);background:rgba(0,212,168,0.04);}
.profile-name{flex:1;font-family:var(--mono);font-size:12px;font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;}

/* ── Responsive: mobile top nav ── */
@media(max-width:700px){
  #sidebar{
    position:fixed;left:0;top:0;bottom:0;
    transform:translateX(-100%);
    transition:transform 0.25s ease,width 0.2s;
    z-index:200;
    box-shadow:4px 0 20px rgba(0,0,0,0.4);
    width:220px!important;min-width:220px!important;
  }
  #sidebar.mobile-open{transform:translateX(0);}
  #mobile-overlay{display:block;}
  #main{width:100%;}
  #topbar{padding:0 12px;}
  #content{padding:12px;}
  .stat-grid{grid-template-columns:1fr 1fr;}
  #sidebar-toggle-btn{display:flex;}
}
@media(min-width:701px){
  #mobile-overlay{display:none!important;}
  #sidebar-toggle-btn-mobile{display:none!important;}
}
#mobile-overlay{
  display:none;position:fixed;inset:0;background:rgba(0,0,0,0.5);z-index:150;
}

/* ── Topbar mobile toggle ── */
#sidebar-toggle-btn{
  display:none;
  width:32px;height:32px;align-items:center;justify-content:center;
  background:transparent;border:1px solid var(--border);border-radius:6px;
  color:var(--text2);cursor:pointer;font-size:16px;flex-shrink:0;
}

/* ── TS Visualizer ───────────────────────────────────────────────── */
.ts-grid{display:grid;grid-template-columns:repeat(4,1fr);gap:10px;padding:16px 18px;}
.ts-block{
  border:1px solid var(--border);border-radius:8px;
  padding:12px 10px 8px;text-align:center;
  position:relative;overflow:hidden;
  transition:border-color 0.15s, box-shadow 0.15s, background 0.15s;
  background:var(--bg3);
  cursor:default;
}
.ts-block.mcch{
  border-color:rgba(77,166,255,0.35);
  background:linear-gradient(160deg,rgba(77,166,255,0.07) 0%,var(--bg3) 100%);
}
.ts-block.call{
  border-color:rgba(255,180,36,0.5);
  background:linear-gradient(160deg,rgba(255,180,36,0.06) 0%,var(--bg3) 100%);
  box-shadow:0 0 14px rgba(255,180,36,0.1);
}
.ts-block.voice{
  border-color:rgba(255,60,80,0.7);
  background:linear-gradient(160deg,rgba(255,60,80,0.12) 0%,var(--bg3) 100%);
  box-shadow:0 0 18px rgba(255,60,80,0.25);
}
.ts-block.voice .ts-flash{animation:ts-flash-in 0.08s ease-out;}

/* number badge top-left */
.ts-num{
  position:absolute;top:7px;left:9px;
  font-family:var(--mono);font-size:9px;font-weight:700;
  letter-spacing:0.1em;color:var(--text3);
}
.ts-block.mcch .ts-num{color:var(--accent2);}
.ts-block.call .ts-num{color:var(--warn);}
.ts-block.voice .ts-num{color:var(--danger);}

/* LED */
.ts-led{
  width:10px;height:10px;border-radius:50%;
  background:var(--bg4);margin:4px auto 9px;
  transition:background 0.1s,box-shadow 0.1s;
  flex-shrink:0;
}
.ts-block.mcch .ts-led{background:var(--accent2);box-shadow:0 0 7px rgba(77,166,255,0.6);}
.ts-block.call .ts-led{background:var(--warn);box-shadow:0 0 7px rgba(255,180,36,0.5);}
.ts-block.voice .ts-led{background:var(--danger);box-shadow:0 0 10px rgba(255,60,80,0.8);animation:ts-led-pulse 0.3s ease-in-out infinite alternate;}

/* waveform bars */
.ts-wave{
  display:flex;align-items:flex-end;justify-content:center;
  gap:2px;height:22px;margin:0 auto 5px;width:60%;
  opacity:0.25;transition:opacity 0.15s;
}
.ts-block.voice .ts-wave{opacity:1;}
.ts-block.call .ts-wave{opacity:0.45;}
.ts-wave-bar{
  width:3px;border-radius:2px 2px 0 0;
  background:var(--text3);min-height:3px;
  transition:height 0.1s ease;
}
.ts-block.mcch .ts-wave-bar{background:var(--accent2);}
.ts-block.call .ts-wave-bar{background:var(--warn);}
.ts-block.voice .ts-wave-bar{background:var(--danger);}

/* label */
.ts-label{
  font-family:var(--mono);font-size:10px;font-weight:700;
  letter-spacing:0.05em;color:var(--text3);
  min-height:13px;
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
  transition:color 0.15s;
}
.ts-block.mcch .ts-label{color:var(--accent2);}
.ts-block.call .ts-label{color:var(--warn);}
.ts-block.voice .ts-label{color:var(--danger);}

/* sub */
.ts-sub{
  font-family:var(--mono);font-size:9px;color:var(--text3);
  margin-top:2px;min-height:11px;
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
}
.ts-block.voice .ts-sub{color:rgba(255,60,80,0.7);}

/* flash overlay on new voice frame */
.ts-flash{
  position:absolute;inset:0;
  background:rgba(255,60,80,0.18);
  pointer-events:none;opacity:0;border-radius:8px;
}

/* bottom progress bar (call duration) */
.ts-duration-bar{
  position:absolute;bottom:0;left:0;height:2px;
  background:var(--warn);transition:width 0.5s linear;width:0%;
  border-radius:0 0 8px 8px;
}
.ts-block.voice .ts-duration-bar{background:var(--danger);}

@keyframes ts-flash-in{
  0%{opacity:1;}
  100%{opacity:0;}
}
@keyframes ts-led-pulse{
  0%{box-shadow:0 0 6px rgba(255,60,80,0.6);}
  100%{box-shadow:0 0 14px rgba(255,60,80,1);}
}
</style>
</head>
<body>

<!-- Mobile overlay -->
<div id="mobile-overlay" onclick="closeMobileSidebar()"></div>

<!-- ── Sidebar ── -->
<nav id="sidebar">
  <div class="sidebar-logo">
    <div class="logo-icon">FS</div>
    <div class="logo-text">
      <div class="logo-name">FlowStation</div>
      <div class="logo-sub">{{STACK_VERSION}}</div>
    </div>
  </div>

  <div class="sidebar-nav">
    <div class="nav-section-label" data-i18n-section="monitor">MONITOR</div>
    <div class="nav-item active" onclick="showPage('stations',this)" id="nav-stations">
      <span class="nav-icon">📡</span>
      <span class="nav-label" data-i18n="stations">RADIOS</span>
      <span class="nav-badge" id="badge-ms">0</span>
    </div>
    <div class="nav-item" onclick="showPage('calls',this)" id="nav-calls">
      <span class="nav-icon">☎</span>
      <span class="nav-label" data-i18n="calls">CALLS</span>
      <span class="nav-badge" id="badge-calls" style="display:none">0</span>
    </div>
    <div class="nav-item" onclick="showPage('lastheard',this)" id="nav-lastheard">
      <span class="nav-icon">🎙</span>
      <span class="nav-label" data-i18n="lastheard">LAST HEARD</span>
    </div>
    <div class="nav-item" onclick="showPage('log',this)" id="nav-log">
      <span class="nav-icon">📋</span>
      <span class="nav-label" data-i18n="log">LOG</span>
    </div>

    <div class="nav-section-label" data-i18n-section="manage">MANAGE</div>
    <div class="nav-item" onclick="showPage('config',this)" id="nav-config">
      <span class="nav-icon">⚙</span>
      <span class="nav-label" data-i18n="config">CONFIG</span>
    </div>
    <div class="nav-item" onclick="showPage('system',this)" id="nav-system">
      <span class="nav-icon">🖥</span>
      <span class="nav-label" data-i18n="system">SYSTEM</span>
    </div>
  </div>

  <div class="sidebar-footer">
    <!-- BS connection -->
    <div class="conn-status-row">
      <div class="conn-led" id="connLed"></div>
      <div class="conn-info">
        <div class="conn-info-label">BS</div>
        <div class="conn-info-val" id="connText" style="color:var(--danger)">OFFLINE</div>
      </div>
    </div>
    <!-- Brew connection -->
    <div class="brew-status-row">
      <div class="brew-led" id="brewLed"></div>
      <div class="brew-info">
        <div class="brew-info-label">BREW</div>
        <div class="brew-info-val" id="brewText">OFFLINE</div>
      </div>
      <div id="brewVerBadge" class="brew-ver-badge" style="display:none"></div>
    </div>
    <!-- Copyright + client info -->
    <div class="sidebar-copyright">
      <div class="cr-line">© 2026 Razvan Zeces — YO6RZV</div>
      <div class="cr-line" id="cr-ua">—</div>
    </div>
    <!-- Collapse toggle -->
    <button class="sidebar-toggle" onclick="toggleSidebar()" title="Toggle sidebar">⇔</button>
  </div>
</nav>

<!-- ── Main ── -->
<div id="main">
  <!-- Topbar -->
  <div id="topbar">
    <button id="sidebar-toggle-btn" onclick="openMobileSidebar()">☰</button>
    <div class="topbar-title" id="topbar-title">Radios</div>
    <div class="topbar-right">
      <div class="theme-picker">
        <button class="theme-btn active" data-t="dark" onclick="setTheme('dark',this)">Dark</button>
        <button class="theme-btn" data-t="light" onclick="setTheme('light',this)">Light</button>
        <button class="theme-btn" data-t="blue" onclick="setTheme('blue',this)">Blue</button>
      </div>
      <div class="lang-picker">
        <button class="lang-btn active" onclick="setLang('en',this)">EN</button>
        <button class="lang-btn" onclick="setLang('ro',this)">RO</button>
        <button class="lang-btn" onclick="setLang('de',this)">DE</button>
        <button class="lang-btn" onclick="setLang('es',this)">ES</button>
        <button class="lang-btn" onclick="setLang('hu',this)">HU</button>
        <button class="lang-btn" onclick="setLang('hu',this)">CN</button>
      </div>
    </div>
  </div>

  <!-- Fallback config warning banner — hidden until JS shows it -->
  <div id="fallback-banner" style="display:none;background:var(--danger);color:#fff;padding:10px 18px;font-size:13px;font-weight:600;align-items:center;gap:10px;flex-shrink:0">
    <span style="font-size:18px">⚠️</span>
    <div>
      <div data-i18n="fallback_title">FALLBACK CONFIG ACTIVE — Primary config failed to load</div>
      <div id="fallback-reason" style="font-size:11px;font-weight:400;opacity:0.85;margin-top:2px"></div>
    </div>
  </div>

  <!-- Content -->
  <div id="content">

    <!-- ── RADIOS ── -->
    <div class="page active" id="page-stations">
      <!-- Stat cards -->
      <div class="stat-grid">
        <div class="stat-card green">
          <div class="stat-label" data-i18n="terminals">Radios</div>
          <div class="stat-value accent" id="stat-ms">0</div>
          <div class="stat-sub" data-i18n="registered">registered</div>
          <div class="stat-icon">📡</div>
        </div>
        <div class="stat-card blue">
          <div class="stat-label" data-i18n="active_calls">Active Calls</div>
          <div class="stat-value blue" id="stat-calls">0</div>
          <div class="stat-sub" data-i18n="circuits">circuits in use</div>
          <div class="stat-icon">☎</div>
        </div>
        <div class="stat-card" id="stat-brew-card">
          <div class="stat-label">BREW</div>
          <div class="stat-value" id="stat-brew-val" style="font-size:20px;color:var(--danger)">OFFLINE</div>
          <div class="stat-sub" id="stat-brew-sub">—</div>
          <div class="stat-icon">🔗</div>
        </div>
      </div>
      <!-- TS Visualizer -->
      <div class="card">
        <div class="card-head">
          <div class="card-title">RF Channel — Timeslots</div>
        </div>
        <div class="ts-grid" id="ts-grid">
          <div class="ts-block mcch" id="ts-block-1">
            <div class="ts-num">TS 1</div>
            <div class="ts-led"></div>
            <div class="ts-wave">
              <div class="ts-wave-bar" style="height:8px"></div>
              <div class="ts-wave-bar" style="height:14px"></div>
              <div class="ts-wave-bar" style="height:10px"></div>
              <div class="ts-wave-bar" style="height:16px"></div>
              <div class="ts-wave-bar" style="height:8px"></div>
              <div class="ts-wave-bar" style="height:12px"></div>
              <div class="ts-wave-bar" style="height:6px"></div>
            </div>
            <div class="ts-label">MCCH</div>
            <div class="ts-sub">Control</div>
            <div class="ts-flash"></div>
            <div class="ts-duration-bar"></div>
          </div>
          <div class="ts-block" id="ts-block-2">
            <div class="ts-num">TS 2</div>
            <div class="ts-led"></div>
            <div class="ts-wave">
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
            </div>
            <div class="ts-label">—</div>
            <div class="ts-sub">Idle</div>
            <div class="ts-flash"></div>
            <div class="ts-duration-bar"></div>
          </div>
          <div class="ts-block" id="ts-block-3">
            <div class="ts-num">TS 3</div>
            <div class="ts-led"></div>
            <div class="ts-wave">
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
            </div>
            <div class="ts-label">—</div>
            <div class="ts-sub">Idle</div>
            <div class="ts-flash"></div>
            <div class="ts-duration-bar"></div>
          </div>
          <div class="ts-block" id="ts-block-4">
            <div class="ts-num">TS 4</div>
            <div class="ts-led"></div>
            <div class="ts-wave">
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
              <div class="ts-wave-bar" style="height:3px"></div>
            </div>
            <div class="ts-label">—</div>
            <div class="ts-sub">Idle</div>
            <div class="ts-flash"></div>
            <div class="ts-duration-bar"></div>
          </div>
        </div>
      </div>

      <!-- Table -->
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="registered_terminals">Registered Radios</div>
        </div>
        <div class="card-body">
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
              <tbody id="ms-tbody"></tbody>
            </table>
          </div>
        </div>
      </div>
    </div>

    <!-- ── CALLS ── -->
    <div class="page" id="page-calls">
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="active_calls">Active Calls</div>
        </div>
        <div class="card-body">
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
              <tbody id="calls-tbody"></tbody>
            </table>
          </div>
        </div>
      </div>
    </div>

    <!-- ── LAST HEARD ── -->
    <div class="page" id="page-lastheard">
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="last_heard_title">Last Heard</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="clearLastHeard()" data-i18n="clear">Clear</button>
          </div>
        </div>
        <div class="card-body">
          <div class="table-wrap">
            <table>
              <thead><tr>
                <th data-i18n="th_time">Time</th>
                <th data-i18n="th_issi">ISSI</th>
                <th data-i18n="th_activity">Activity</th>
                <th data-i18n="th_dest">Destination</th>
              </tr></thead>
              <tbody id="lastheard-tbody"></tbody>
            </table>
          </div>
        </div>
      </div>
    </div>

    <!-- ── LOG ── -->
    <div class="page" id="page-log">
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="live_log">Live Log</div>
        </div>
        <div id="log-container" class="log-wrap"></div>
        <div class="log-controls">
          <select id="log-filter" class="log-filter">
            <option value="" data-i18n="filter_all">All</option>
            <option value="INFO">INFO+</option>
            <option value="WARN">WARN+</option>
            <option value="ERROR">ERROR</option>
          </select>
          <label class="autoscroll-label">
            <input type="checkbox" id="log-autoscroll" checked>
            <span data-i18n="autoscroll">Auto-scroll</span>
          </label>
          <div style="margin-left:auto">
            <button class="btn btn-sm" onclick="clearLog()" data-i18n="clear">Clear</button>
          </div>
        </div>
      </div>
    </div>

    <!-- ── CONFIG ── -->
    <div class="page" id="page-config">
      <div class="card">
        <div class="card-head">
          <div class="card-title">config.toml</div>
          <div class="card-actions">
            <button class="btn btn-warn" onclick="restartService()" data-i18n="restart">⟳ Restart</button>
            <button class="btn btn-danger" onclick="shutdownService()" data-i18n="shutdown">⏻ Shutdown</button>
            <button class="btn" onclick="startUpdate()" data-i18n="update">⬆ Update</button>
            <button class="btn btn-primary" onclick="saveConfig()" data-i18n="save">Save</button>
          </div>
        </div>
        <div class="card-body">
          <textarea id="config-editor" spellcheck="false" placeholder="Loading..."></textarea>
          <div class="config-msg" id="config-msg"></div>
        </div>
      </div>
    </div>

    <!-- ── SYSTEM ── -->
    <div class="page" id="page-system">
      <!-- BTS + Brew status -->
      <div class="stat-grid" style="grid-template-columns:repeat(auto-fit,minmax(180px,1fr))">
        <div class="stat-card green">
          <div class="stat-label" data-i18n="sys_bts">BTS Connection</div>
          <div class="stat-value" id="sysBtsStatus" style="font-size:18px;color:var(--danger)">OFFLINE</div>
          <div class="stat-sub" id="sysBtsIp">—</div>
        </div>
        <div class="stat-card blue">
          <div class="stat-label">BREW</div>
          <div class="stat-value" id="sysBrewStatus" style="font-size:18px;color:var(--danger)">OFFLINE</div>
          <div class="stat-sub" id="sysBrewBadge">—</div>
        </div>
        <div class="stat-card">
          <div class="stat-label" data-i18n="sys_uptime">Uptime</div>
          <div class="stat-value" id="sysUptime" style="font-size:20px;color:var(--text2)">—</div>
          <div class="stat-sub" id="sysHostname">—</div>
        </div>
        <div class="stat-card" id="cpu-temp-card" style="display:none">
          <div class="stat-label" data-i18n="sys_temp">CPU Temp</div>
          <div class="stat-value" id="sysCpuTemp" style="font-size:20px;color:var(--warn)">—</div>
          <div class="stat-sub" id="sysCpuTempSub">—</div>
        </div>
      </div>

      <!-- System info + CPU/RAM -->
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="sys_info">System Info</div>
          <div class="card-actions" style="display:flex;align-items:center;gap:10px">
            <label style="display:flex;align-items:center;gap:5px;font-size:12px;color:var(--text2);cursor:pointer">
              <input type="checkbox" id="sys-autorefresh" onchange="toggleSysAutoRefresh(this.checked)" style="cursor:pointer">
              <span data-i18n="sys_autorefresh">Auto-refresh 5s</span>
            </label>
            <button class="btn btn-sm" onclick="loadSystemInfo()">↻ Refresh</button>
          </div>
        </div>
        <div class="card-body">
          <div class="info-row"><div class="info-key" data-i18n="sys_version">FS Version</div><div class="info-val accent" id="sysVersion">—</div></div>
          <div class="info-row"><div class="info-key" data-i18n="sys_os">OS</div><div class="info-val" id="sysOs">—</div></div>
          <div class="info-row"><div class="info-key" data-i18n="sys_config">Active Config</div><div class="info-val" id="sysConfigPath">—</div></div>
          <div class="info-row"><div class="info-key" data-i18n="sys_cpu">CPU</div><div class="info-val" id="sysCpu">—</div></div>
          <div class="info-row">
            <div class="info-key" data-i18n="sys_cpu_load">CPU Load</div>
            <div class="info-val" style="display:flex;align-items:center;gap:8px">
              <div style="flex:1;height:6px;background:var(--bg4);border-radius:3px;overflow:hidden;max-width:120px">
                <div id="sysCpuBar" style="height:100%;width:0%;background:var(--accent);border-radius:3px;transition:width 0.3s"></div>
              </div>
              <span id="sysCpuPct" style="font-family:var(--mono);font-size:12px;color:var(--text2)">—</span>
            </div>
          </div>
          <div class="info-row">
            <div class="info-key" data-i18n="sys_ram">RAM</div>
            <div class="info-val" style="display:flex;align-items:center;gap:8px">
              <div style="flex:1;height:6px;background:var(--bg4);border-radius:3px;overflow:hidden;max-width:120px">
                <div id="sysRamBar" style="height:100%;width:0%;background:var(--accent2);border-radius:3px;transition:width 0.3s"></div>
              </div>
              <span id="sysRamVal" style="font-family:var(--mono);font-size:12px;color:var(--text2)">—</span>
            </div>
          </div>
        </div>
      </div>

      <!-- RF / SDR Hardware -->
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="sys_rf">RF Hardware (SoapySDR)</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="loadSystemInfo()">↻ Probe</button>
          </div>
        </div>
        <div class="card-body">
          <pre id="sysSoapy" style="font-family:var(--mono);font-size:11px;color:var(--text2);white-space:pre-wrap;word-break:break-all;margin:0;padding:0">—</pre>
        </div>
      </div>

      <!-- Config profiles -->
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="sys_profiles">Config Profiles</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="loadConfigProfiles()">↻ Refresh</button>
          </div>
        </div>
        <div class="card-body" style="padding:14px 18px">
          <div id="profileList"></div>
        </div>
      </div>

      <!-- Live SDS Broadcast -->
      <div class="card">
        <div class="card-head">
          <div class="card-title">📢 Live SDS Broadcast</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="loadLiveSds()">↻ Refresh</button>
            <button class="btn btn-sm btn-danger" onclick="clearAllLiveSds()" id="live-sds-clear-btn" style="display:none" data-i18n="live_sds_clear_all">Clear All</button>
          </div>
        </div>
        <div class="card-body" style="padding:14px 18px">
          <p style="font-size:12px;color:var(--text2);margin-bottom:12px" data-i18n="live_sds_desc">Broadcast a text message to all radios on the cell, repeating at the Home Mode Display interval. Repeats until deleted or the repeat count is reached.</p>
          <div class="form-row" style="display:flex;gap:8px;align-items:flex-end;flex-wrap:wrap">
            <div style="flex:1;min-width:180px">
              <label class="form-label" data-i18n="live_sds_text">Message text (max 251 chars)</label>
              <input type="text" id="live-sds-text" class="form-input" maxlength="251" placeholder="e.g. Repeater test 18:00-20:00">
            </div>
            <div style="width:90px">
              <label class="form-label" data-i18n="live_sds_repeat">Repeat (0=∞)</label>
              <input type="number" id="live-sds-repeat" class="form-input" value="0" min="0" max="999" style="width:100%">
            </div>
            <button class="btn btn-primary" onclick="addLiveSds()" data-i18n="live_sds_send">📢 Broadcast</button>
          </div>
          <div id="live-sds-list" style="margin-top:14px"></div>
        </div>
      </div>
    </div>

  </div><!-- /content -->
</div><!-- /main -->

<!-- ── Edit Profile Modal ── -->
<div class="modal-overlay" id="edit-profile-modal">
  <div class="modal" style="width:min(700px,95vw);max-height:90vh;display:flex;flex-direction:column">
    <div class="modal-title">
      ✏️ <span data-i18n="profile_edit_title">Edit Config Profile</span>:
      <span id="edit-profile-name" style="color:var(--accent);font-family:var(--mono);font-size:14px"></span>
    </div>
    <div style="flex:1;overflow:hidden;display:flex;flex-direction:column;gap:8px;min-height:0">
      <textarea id="edit-profile-editor"
        style="flex:1;width:100%;min-height:300px;font-family:var(--mono);font-size:12px;
               background:var(--bg3);color:var(--text);border:1px solid var(--border2);
               border-radius:6px;padding:10px;resize:vertical;line-height:1.5"
        spellcheck="false"></textarea>
      <div id="edit-profile-msg" style="font-size:12px;min-height:16px"></div>
    </div>
    <div class="modal-actions">
      <button class="btn" onclick="closeEditProfileModal()" data-i18n="cancel">Cancel</button>
      <button class="btn btn-primary" onclick="saveEditProfile()" data-i18n="save">Save</button>
    </div>
  </div>
</div>

<!-- ── SDS Modal ── -->
<div class="modal-overlay" id="sds-modal">
  <div class="modal">
    <div class="modal-title" data-i18n="sds_title">⬡ Send SDS Message</div>
    <div class="form-row">
      <label class="form-label" data-i18n="sds_dest">Destination ISSI</label>
      <input type="number" id="sds-dest" class="form-input" placeholder="e.g. 2260571">
    </div>
    <div class="form-row">
      <label class="form-label" data-i18n="sds_msg_label">Message</label>
      <input type="text" id="sds-msg" class="form-input" placeholder="..." maxlength="160">
    </div>
    <div class="modal-actions">
      <button class="btn" onclick="closeSdsModal()" data-i18n="cancel">Cancel</button>
      <button class="btn btn-primary" onclick="sendSds()" data-i18n="send">Send</button>
    </div>
  </div>
</div>

<!-- ── Update Modal ── -->
<div class="modal-overlay" id="update-modal">
  <div class="modal">
    <div class="modal-title" id="update-modal-title" data-i18n="update_title">⬆ OTA Update</div>
    <div class="update-status running" id="update-status-msg"></div>
    <div class="update-terminal" id="update-terminal"></div>
    <div class="modal-actions">
      <button class="btn" id="update-close-btn" onclick="closeUpdateModal()" data-i18n="update_close" disabled>Close</button>
    </div>
  </div>
</div>

<script>
// ── i18n ─────────────────────────────────────────────────────────────────
const LANGS={
  en:{
    bts_ip:'BTS IP',offline:'OFFLINE',online:'ONLINE',
    brew_online:'ONLINE',brew_offline:'OFFLINE',
    stations:'Radios',calls:'Calls',lastheard:'Last Heard',log:'Log',config:'Config',
    terminals:'Radios',registered:'registered',
    active_calls:'Active Calls',circuits:'circuits in use',
    registered_terminals:'Registered Radios',
    no_terminals:'No radios registered',no_calls:'No active calls',
    live_log:'Live Log',autoscroll:'Auto-scroll',filter_all:'All',
    clear:'Clear',restart:'⟳ Restart',shutdown:'⏻ Shutdown',save:'Save',
    sds_title:'⬡ Send SDS Message',sds_dest:'Destination ISSI',
    live_sds_desc:'Broadcast a text message to all radios on the cell, repeating at the Home Mode Display interval. Repeats until deleted or the repeat count is reached.',
    live_sds_text:'Message text (max 251 chars)',live_sds_repeat:'Repeat (0=∞)',live_sds_send:'📢 Broadcast',
    live_sds_clear_all:'Clear All',live_sds_empty:'No active broadcasts.',
    live_sds_sent:'sent',live_sds_times:'×',live_sds_forever:'∞',live_sds_delete:'✕',
    fallback_title:'⚠ FALLBACK CONFIG ACTIVE — Primary config failed to load',
    sds_msg_label:'Message',cancel:'Cancel',send:'Send',
    th_issi:'ISSI',th_groups:'Groups',th_ee:'EE',th_signal:'Signal',
    th_status:'Status',th_last_seen:'Last seen',th_actions:'Actions',
    th_id:'ID',th_type:'Type',th_caller:'Caller',
    th_dest:'Destination',th_speaker:'Speaker',th_duration:'Duration',
    th_time:'Time',th_activity:'Activity',
    last_heard_title:'Last Heard',no_activity:'No activity yet',
    act_call_group:'Group Call',act_call_individual:'P2P Call',act_sds:'SDS',
    online_badge:'ONLINE',kick:'Kick',sds:'SDS',
    call_group:'GROUP',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',
    confirm_kick:'Kick ISSI {issi}?\nTerminal will be deregistered and forced to re-attach.',
    confirm_restart:'Restart FlowStation?\nAll active calls will be dropped.',
    confirm_shutdown:'Shutdown FlowStation?\nThe service will stop and must be restarted manually.',
    saved:'✓ Saved — restart to apply.',save_fail:'✗ Save failed',conn_error:'Connection error.',
    update:'⬆ Update',update_title:'OTA Update — github.com/razvanzeces/flowstation',
    update_confirm:'Pull latest from main and rebuild?\nThe service will restart automatically.',
    update_running:'Updating… do not close this window.',
    update_done_ok:'✓ Update complete. Restarting…',
    update_done_err:'✗ Update failed. See log above.',
    update_close:'Close',
    system:'System',sys_info:'System Info',sys_hostname:'Hostname',sys_uptime:'Uptime',
    sys_version:'FS Version',sys_os:'OS',sys_config:'Active Config',
    sys_cpu:'CPU',sys_cpu_load:'CPU Load',sys_ram:'RAM',sys_temp:'CPU Temp',
    sys_rf:'RF Hardware (SoapySDR)',sys_autorefresh:'Auto-refresh 5s',
    profile_edit_title:'Edit Config Profile',profile_edit_btn:'Edit',
    profile_edit_save_ok:'✓ Saved',profile_edit_save_fail:'✗ Save failed',
    sys_os:'OS',sys_version:'FS Version',sys_config:'Active Config',
    sys_profiles:'Config Profiles',sys_activate:'Activate & Restart',
    sys_active_badge:'ACTIVE',sys_no_profiles:'No .toml profiles found in config directory.',
    sys_activate_confirm:'Switch to profile "{name}" and restart?\nCurrent config will be backed up.',
    sys_bts:'BTS Connection',
  },
  ro:{
    bts_ip:'IP BTS',offline:'DECONECTAT',online:'CONECTAT',
    brew_online:'ONLINE',brew_offline:'OFFLINE',
    stations:'Radiouri',calls:'Apeluri',lastheard:'Ultima Activitate',log:'Log',config:'Config',
    terminals:'Radiouri',registered:'înregistrate',
    active_calls:'Apeluri Active',circuits:'circuite active',
    registered_terminals:'Radiouri Înregistrate',
    no_terminals:'Niciun radio înregistrat',no_calls:'Niciun apel activ',
    live_log:'Log Live',autoscroll:'Auto-scroll',filter_all:'Toate',
    clear:'Șterge',restart:'⟳ Repornire',shutdown:'⏻ Oprire',save:'Salvează',
    live_sds_desc:'Transmite un mesaj text către toate radiourile din celulă, repetând la intervalul Home Mode Display.',
    live_sds_text:'Text mesaj (max 251 caractere)',live_sds_repeat:'Repetări (0=∞)',live_sds_send:'📢 Broadcast',
    live_sds_clear_all:'Șterge Tot',live_sds_empty:'Niciun broadcast activ.',
    live_sds_sent:'trimis',live_sds_times:'×',live_sds_forever:'∞',live_sds_delete:'✕',
    fallback_title:'⚠ CONFIG DE REZERVĂ ACTIV — Config principal nu a putut fi încărcat',
    sds_title:'⬡ Trimite Mesaj SDS',sds_dest:'ISSI Destinatar',
    sds_msg_label:'Mesaj',cancel:'Anulează',send:'Trimite',
    th_issi:'ISSI',th_groups:'Grupuri',th_ee:'EE',th_signal:'Semnal',
    th_status:'Status',th_last_seen:'Văzut',th_actions:'Acțiuni',
    th_id:'ID',th_type:'Tip',th_caller:'Apelant',
    th_dest:'Destinatar',th_speaker:'Vorbitor',th_duration:'Durată',
    th_time:'Oră',th_activity:'Activitate',
    last_heard_title:'Ultima Activitate',no_activity:'Nicio activitate încă',
    act_call_group:'Apel Grup',act_call_individual:'Apel P2P',act_sds:'SDS',
    online_badge:'ONLINE',kick:'Kick',sds:'SDS',
    call_group:'GRUP',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',
    confirm_kick:'Kick ISSI {issi}?\nTerminalul va fi deînregistrat și forțat să se reconecteze.',
    confirm_restart:'Repornire FlowStation?\nToate apelurile active vor fi întrerupte.',
    confirm_shutdown:'Oprire FlowStation?\nServiciul se va opri și trebuie repornit manual.',
    saved:'✓ Salvat — repornire pentru aplicare.',save_fail:'✗ Salvare eșuată',conn_error:'Eroare de conexiune.',
    update:'⬆ Update',update_title:'Update OTA — github.com/razvanzeces/flowstation',
    update_confirm:'Descarcă ultima versiune din main și recompilează?\nServiciul va reporni automat.',
    update_running:'Se actualizează… nu închide fereastra.',
    update_done_ok:'✓ Update finalizat. Se repornește…',
    update_done_err:'✗ Update eșuat. Vezi logul de mai sus.',
    update_close:'Închide',
    system:'Sistem',sys_info:'Info Sistem',sys_hostname:'Hostname',sys_uptime:'Uptime',
    sys_os:'OS',sys_version:'Versiune FS',sys_config:'Config Activ',
    sys_cpu:'CPU',sys_cpu_load:'Încărcare CPU',sys_ram:'RAM',sys_temp:'Temp CPU',
    sys_rf:'Hardware RF (SoapySDR)',sys_autorefresh:'Auto-refresh 5s',
    profile_edit_title:'Editare Profil Config',profile_edit_btn:'Editează',
    profile_edit_save_ok:'✓ Salvat',profile_edit_save_fail:'✗ Salvare eșuată',
    sys_profiles:'Profile Config',sys_activate:'Activează & Repornire',
    sys_active_badge:'ACTIV',sys_no_profiles:'Niciun profil .toml găsit în directorul config.',
    sys_activate_confirm:'Comutare la profilul "{name}" și repornire?\nConfig-ul curent va fi salvat.',
    sys_bts:'Conexiune BTS',
  },
  de:{
    bts_ip:'BTS-IP',offline:'OFFLINE',online:'ONLINE',
    brew_online:'ONLINE',brew_offline:'OFFLINE',
    stations:'Radios',calls:'Anrufe',lastheard:'Zuletzt Gehört',log:'Log',config:'Config',
    terminals:'Radios',registered:'registriert',
    active_calls:'Aktive Anrufe',circuits:'Schaltkreise aktiv',
    registered_terminals:'Registrierte Radios',
    no_terminals:'Keine Radios registriert',no_calls:'Keine aktiven Anrufe',
    live_log:'Live-Log',autoscroll:'Auto-Scroll',filter_all:'Alle',
    clear:'Löschen',restart:'⟳ Neustart',shutdown:'⏻ Herunterfahren',save:'Speichern',
    live_sds_desc:'Sendet eine Textnachricht an alle Funkgeräte der Zelle, wiederholt im Home-Mode-Display-Intervall.',
    live_sds_text:'Nachrichtentext (max. 251 Zeichen)',live_sds_repeat:'Wiederh. (0=∞)',live_sds_send:'📢 Senden',
    live_sds_clear_all:'Alle löschen',live_sds_empty:'Keine aktiven Broadcasts.',
    live_sds_sent:'gesendet',live_sds_times:'×',live_sds_forever:'∞',live_sds_delete:'✕',
    fallback_title:'⚠ FALLBACK-KONFIGURATION AKTIV — Primäre Konfiguration konnte nicht geladen werden',
    sds_title:'⬡ SDS-Nachricht senden',sds_dest:'Ziel-ISSI',
    sds_msg_label:'Nachricht',cancel:'Abbrechen',send:'Senden',
    th_issi:'ISSI',th_groups:'Gruppen',th_ee:'EE',th_signal:'Signal',
    th_status:'Status',th_last_seen:'Zuletzt',th_actions:'Aktionen',
    th_id:'ID',th_type:'Typ',th_caller:'Anrufer',
    th_dest:'Ziel',th_speaker:'Sprecher',th_duration:'Dauer',
    th_time:'Zeit',th_activity:'Aktivität',
    last_heard_title:'Zuletzt Gehört',no_activity:'Noch keine Aktivität',
    act_call_group:'Gruppenruf',act_call_individual:'P2P-Ruf',act_sds:'SDS',
    online_badge:'ONLINE',kick:'Entfernen',sds:'SDS',
    call_group:'GRUPPE',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',
    confirm_kick:'ISSI {issi} entfernen?\nDas Terminal wird abgemeldet und zur Neuanmeldung gezwungen.',
    confirm_restart:'FlowStation neu starten?\nAlle aktiven Anrufe werden beendet.',
    confirm_shutdown:'FlowStation herunterfahren?\nDer Dienst wird gestoppt und muss manuell neu gestartet werden.',
    saved:'✓ Gespeichert — Neustart zum Anwenden.',save_fail:'✗ Fehler beim Speichern',conn_error:'Verbindungsfehler.',
    update:'⬆ Update',update_title:'OTA-Update — github.com/razvanzeces/flowstation',
    update_confirm:'Neueste Version von main holen und neu bauen?\nDer Dienst startet automatisch neu.',
    update_running:'Aktualisierung läuft… Fenster nicht schließen.',
    update_done_ok:'✓ Update abgeschlossen. Neustart…',
    update_done_err:'✗ Update fehlgeschlagen. Siehe Log oben.',
    update_close:'Schließen',
    system:'System',sys_info:'Systeminfo',sys_hostname:'Hostname',sys_uptime:'Laufzeit',
    sys_os:'OS',sys_version:'FS-Version',sys_config:'Aktive Konfig',
    sys_cpu:'CPU',sys_cpu_load:'CPU-Auslastung',sys_ram:'RAM',sys_temp:'CPU-Temp',
    sys_rf:'RF-Hardware (SoapySDR)',sys_autorefresh:'Auto-Aktualisierung 5s',
    profile_edit_title:'Konfigprofil bearbeiten',profile_edit_btn:'Bearbeiten',
    profile_edit_save_ok:'✓ Gespeichert',profile_edit_save_fail:'✗ Speichern fehlgeschlagen',
    sys_profiles:'Konfigprofile',sys_activate:'Aktivieren & Neustart',
    sys_active_badge:'AKTIV',sys_no_profiles:'Keine .toml-Profile im Konfigverzeichnis gefunden.',
    sys_activate_confirm:'Zum Profil "{name}" wechseln und neu starten?\nAktuelle Konfig wird gesichert.',
    sys_bts:'BTS-Verbindung',
  },
  es:{
    bts_ip:'IP BTS',offline:'SIN CONEXIÓN',online:'EN LÍNEA',
    brew_online:'EN LÍNEA',brew_offline:'SIN CONEXIÓN',
    stations:'Radios',calls:'Llamadas',lastheard:'Última Actividad',log:'Log',config:'Config',
    terminals:'Radios',registered:'registrados',
    active_calls:'Llamadas Activas',circuits:'circuitos en uso',
    registered_terminals:'Radios Registrados',
    no_terminals:'No hay radios registrados',no_calls:'No hay llamadas activas',
    live_log:'Log en Vivo',autoscroll:'Auto-desplaz.',filter_all:'Todos',
    clear:'Limpiar',restart:'⟳ Reiniciar',shutdown:'⏻ Apagar',save:'Guardar',
    live_sds_desc:'Transmite un mensaje de texto a todos los radios de la celda, repitiéndose al intervalo de Home Mode Display.',
    live_sds_text:'Texto del mensaje (máx. 251 caracteres)',live_sds_repeat:'Repetir (0=∞)',live_sds_send:'📢 Difundir',
    live_sds_clear_all:'Borrar Todo',live_sds_empty:'No hay difusiones activas.',
    live_sds_sent:'enviado',live_sds_times:'×',live_sds_forever:'∞',live_sds_delete:'✕',
    fallback_title:'⚠ CONFIGURACIÓN DE RESERVA ACTIVA — No se pudo cargar la configuración principal',
    sds_title:'⬡ Enviar Mensaje SDS',sds_dest:'ISSI Destino',
    sds_msg_label:'Mensaje',cancel:'Cancelar',send:'Enviar',
    th_issi:'ISSI',th_groups:'Grupos',th_ee:'EE',th_signal:'Señal',
    th_status:'Estado',th_last_seen:'Visto',th_actions:'Acciones',
    th_id:'ID',th_type:'Tipo',th_caller:'Llamante',
    th_dest:'Destino',th_speaker:'Hablante',th_duration:'Duración',
    th_time:'Hora',th_activity:'Actividad',
    last_heard_title:'Última Actividad',no_activity:'Sin actividad aún',
    act_call_group:'Llamada Grupo',act_call_individual:'Llamada P2P',act_sds:'SDS',
    online_badge:'EN LÍNEA',kick:'Expulsar',sds:'SDS',
    call_group:'GRUPO',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',
    confirm_kick:'¿Expulsar ISSI {issi}?\nEl terminal será desregistrado y forzado a reconectarse.',
    confirm_restart:'¿Reiniciar FlowStation?\nTodas las llamadas activas se interrumpirán.',
    confirm_shutdown:'¿Apagar FlowStation?\nEl servicio se detendrá y deberá reiniciarse manualmente.',
    saved:'✓ Guardado — reinicia para aplicar.',save_fail:'✗ Error al guardar',conn_error:'Error de conexión.',
    update:'⬆ Update',update_title:'Actualización OTA — github.com/razvanzeces/flowstation',
    update_confirm:'¿Obtener la última versión de main y recompilar?\nEl servicio se reiniciará automáticamente.',
    update_running:'Actualizando… no cierres esta ventana.',
    update_done_ok:'✓ Actualización completa. Reiniciando…',
    update_done_err:'✗ Actualización fallida. Ver log arriba.',
    update_close:'Cerrar',
    system:'Sistema',sys_info:'Info del Sistema',sys_hostname:'Hostname',sys_uptime:'Tiempo activo',
    sys_os:'OS',sys_version:'Versión FS',sys_config:'Config Activa',
    sys_cpu:'CPU',sys_cpu_load:'Carga CPU',sys_ram:'RAM',sys_temp:'Temp CPU',
    sys_rf:'Hardware RF (SoapySDR)',sys_autorefresh:'Auto-actualización 5s',
    profile_edit_title:'Editar Perfil Config',profile_edit_btn:'Editar',
    profile_edit_save_ok:'✓ Guardado',profile_edit_save_fail:'✗ Error al guardar',
    sys_profiles:'Perfiles de Config',sys_activate:'Activar y Reiniciar',
    sys_active_badge:'ACTIVO',sys_no_profiles:'No se encontraron perfiles .toml en el directorio.',
    sys_activate_confirm:'¿Cambiar al perfil "{name}" y reiniciar?\nLa config actual será respaldada.',
    sys_bts:'Conexión BTS',
  },
  hu:{
    bts_ip:'BTS IP',offline:'OFFLINE',online:'ONLINE',
    brew_online:'ONLINE',brew_offline:'OFFLINE',
    stations:'Rádiók',calls:'Hívások',lastheard:'Utoljára Hallott',log:'Napló',config:'Konfig',
    terminals:'Rádiók',registered:'regisztrált',
    active_calls:'Aktív hívások',circuits:'aktív áramkör',
    registered_terminals:'Regisztrált rádiók',
    no_terminals:'Nincs regisztrált rádió',no_calls:'Nincs aktív hívás',
    live_log:'Élő napló',autoscroll:'Automatikus görgetés',filter_all:'Mind',
    clear:'Törlés',restart:'⟳ Újraindítás',shutdown:'⏻ Leállítás',save:'Mentés',
    sds_title:'⬡ SDS üzenet küldése',sds_dest:'Cél ISSI',
    sds_msg_label:'Üzenet',cancel:'Mégse',send:'Küldés',
    th_issi:'ISSI',th_groups:'Csoportok',th_ee:'EE',th_signal:'Jelerősség',
    th_status:'Állapot',th_last_seen:'Utoljára látva',th_actions:'Műveletek',
    th_id:'ID',th_type:'Típus',th_caller:'Hívó',
    th_dest:'Cél',th_speaker:'Beszélő',th_duration:'Időtartam',
    th_time:'Idő',th_activity:'Tevékenység',
    last_heard_title:'Utoljára hallott',no_activity:'Még nincs tevékenység',
    act_call_group:'Csoportos hívás',act_call_individual:'P2P hívás',act_sds:'SDS',
    online_badge:'ONLINE',kick:'Kizárás',sds:'SDS',
    call_group:'CSOPORT',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',
    confirm_kick:'ISSI {issi} kizárása?\nA terminál törlésre kerül és újra kell csatlakoznia.',
    confirm_restart:'Újraindítja a FlowStation-t?\nAz összes aktív hívás megszakad.',
    confirm_shutdown:'Leállítja a FlowStation-t?\nA szolgáltatást kézzel kell újraindítani.',
    saved:'✓ Mentve — újraindítás szükséges az alkalmazáshoz.',save_fail:'✗ Mentési hiba',conn_error:'Kapcsolódási hiba.',
    update:'⬆ Frissítés',update_title:'OTA frissítés — github.com/razvanzeces/flowstation',
    update_confirm:'Letölti a legújabb verziót a main ágból és újraépíti?\nA szolgáltatás automatikusan újraindul.',
    update_running:'Frissítés folyamatban… ne zárja be az ablakot.',
    update_done_ok:'✓ Frissítés kész. Újraindul…',
    update_done_err:'✗ Frissítés sikertelen. Lásd a naplót.',
    update_close:'Bezárás',
    system:'Rendszer',sys_info:'Rendszerinfó',sys_hostname:'Hostname',sys_uptime:'Üzemidő',
    sys_os:'OS',sys_version:'FS verzió',sys_config:'Aktív konfig',
    sys_profiles:'Konfig profilok',sys_activate:'Aktiválás és újraindítás',
    sys_active_badge:'AKTÍV',sys_no_profiles:'Nem található .toml profil a könyvtárban.',
    sys_activate_confirm:'Váltás a(z) "{name}" profilra és újraindítás?\nAz aktuális konfig mentésre kerül.',
    sys_bts:'BTS kapcsolat',
  },
  zh:{
    bts_ip:'BTS IP',offline:'离线',online:'在线',
    brew_online:'在线',brew_offline:'离线',
    stations:'终端',calls:'通话',lastheard:'最近通话',log:'日志',config:'配置',
    terminals:'终端',registered:'已注册',
    active_calls:'活跃通话',circuits:'占用信道',
    registered_terminals:'已注册终端',
    no_terminals:'暂无终端注册',no_calls:'无活跃通话',
    live_log:'实时日志',autoscroll:'自动滚动',filter_all:'全部',
    clear:'清除',restart:'⟳ 重启',shutdown:'⏻ 关机',save:'保存',
    sds_title:'⬡ 发送 SDS 短消息',sds_dest:'目标 ISSI',
    live_sds_desc:'向本小区所有终端广播文本消息，按 Home Mode Display 间隔重复发送。直到删除或达到重复次数为止。',
    live_sds_text:'消息内容（最多 251 字符）',live_sds_repeat:'重复次数 (0=无限)',live_sds_send:'📢 广播',
    live_sds_clear_all:'清除全部',live_sds_empty:'暂无广播任务。',
    live_sds_sent:'已发送',live_sds_times:'次',live_sds_forever:'∞',live_sds_delete:'删除',
    fallback_title:'⚠ 正在使用后备配置 — 主配置加载失败',
    sds_msg_label:'消息内容',cancel:'取消',send:'发送',
    th_issi:'ISSI',th_groups:'群组',th_ee:'EE',th_signal:'信号',
    th_status:'状态',th_last_seen:'最后在线',th_actions:'操作',
    th_id:'ID',th_type:'类型',th_caller:'主叫',
    th_dest:'被叫',th_speaker:'讲话者',th_duration:'时长',
    th_time:'时间',th_activity:'活动',
    last_heard_title:'最近通话记录',no_activity:'暂无活动记录',
    act_call_group:'组呼',act_call_individual:'点对点',act_sds:'SDS',
    online_badge:'在线',kick:'踢下线',sds:'SDS',
    call_group:'组呼',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',
    confirm_kick:'确定踢下 ISSI {issi}？\n终端将被注销并强制重新注册。',
    confirm_restart:'确定重启 FlowStation？\n所有正在进行的通话将被中断。',
    confirm_shutdown:'确定关闭 FlowStation？\n服务将停止，需要手动重启。',
    saved:'✓ 已保存 — 重启后生效',save_fail:'✗ 保存失败',conn_error:'连接错误',
    update:'⬆ 更新',update_title:'OTA 在线更新 — github.com/razvanzeces/flowstation',
    update_confirm:'是否从 main 分支拉取最新代码并重新构建？\n服务将自动重启。',
    update_running:'正在更新… 请不要关闭此窗口',
    update_done_ok:'✓ 更新完成，正在重启…',
    update_done_err:'✗ 更新失败，请查看上方日志',
    update_close:'关闭',
    system:'系统',sys_info:'系统信息',sys_hostname:'主机名',sys_uptime:'运行时间',
    sys_version:'FS 版本',sys_os:'操作系统',sys_config:'当前配置',
    sys_cpu:'CPU',sys_cpu_load:'CPU 负载',sys_ram:'内存',sys_temp:'CPU 温度',
    sys_rf:'RF 硬件 (SoapySDR)',sys_autorefresh:'自动刷新 5秒',
    profile_edit_title:'编辑配置文件',profile_edit_btn:'编辑',
    profile_edit_save_ok:'✓ 已保存',profile_edit_save_fail:'✗ 保存失败',
    sys_profiles:'配置文件',sys_activate:'激活并重启',
    sys_active_badge:'当前使用',sys_no_profiles:'配置目录中未找到 .toml 配置文件。',
    sys_activate_confirm:'切换到配置文件 "{name}" 并重启？\n当前配置将被备份。',
    sys_bts:'BTS 连接',
  },
};

let currentLang=localStorage.getItem('fs_lang')||'en';
function t(k,v){let s=(LANGS[currentLang]||LANGS.en)[k]||(LANGS.en[k]||k);if(v)Object.keys(v).forEach(x=>{s=s.replace('{'+x+'}',v[x]);});return s;}
function applyLang(){
  document.querySelectorAll('[data-i18n]').forEach(el=>el.textContent=t(el.getAttribute('data-i18n')));
  document.querySelectorAll('[data-i18n-tab]').forEach(el=>el.textContent=t(el.getAttribute('data-i18n-tab')));
  // Update nav labels
  ['stations','calls','lastheard','log','config','system'].forEach(p=>{
    const el=document.querySelector(`#nav-${p} .nav-label`);
    if(el)el.textContent=t(p);
  });
  renderStations();renderCalls();renderLastHeard();
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
  document.querySelectorAll('.theme-btn').forEach(d=>d.classList.remove('active'));
  if(btn)btn.classList.add('active');
  else document.querySelectorAll('.theme-btn').forEach(d=>{if(d.dataset.t===theme)d.classList.add('active');});
}

// ── Sidebar ───────────────────────────────────────────────────────────────
let sidebarCollapsed=localStorage.getItem('sb_collapsed')==='1';
function toggleSidebar(){
  sidebarCollapsed=!sidebarCollapsed;
  localStorage.setItem('sb_collapsed',sidebarCollapsed?'1':'0');
  document.getElementById('sidebar').classList.toggle('collapsed',sidebarCollapsed);
}
function openMobileSidebar(){
  document.getElementById('sidebar').classList.add('mobile-open');
  document.getElementById('mobile-overlay').style.display='block';
}
function closeMobileSidebar(){
  document.getElementById('sidebar').classList.remove('mobile-open');
  document.getElementById('mobile-overlay').style.display='none';
}

// ── Page navigation ───────────────────────────────────────────────────────
const PAGE_TITLES={stations:'stations',calls:'calls',lastheard:'lastheard',log:'log',config:'config',system:'system'};
function showPage(name,el){
  document.querySelectorAll('.page').forEach(p=>p.classList.remove('active'));
  document.querySelectorAll('.nav-item').forEach(n=>n.classList.remove('active'));
  document.getElementById('page-'+name).classList.add('active');
  if(el)el.classList.add('active');
  else{const nav=document.getElementById('nav-'+name);if(nav)nav.classList.add('active');}
  document.getElementById('topbar-title').textContent=t(name)||name;
  if(name==='config')loadConfig();
  if(name==='system'){loadSystemInfo();loadConfigProfiles();loadLiveSds();}
  else if(sysAutoRefreshTimer){clearInterval(sysAutoRefreshTimer);sysAutoRefreshTimer=null;const cb=document.getElementById('sys-autorefresh');if(cb)cb.checked=false;}
  if(window.innerWidth<=700)closeMobileSidebar();
}

// ── State + WS ────────────────────────────────────────────────────────────
let ws=null,state={ms:{},calls:{},lastHeard:[],brewOnline:false,brewVer:0},sdsDest=0;
const logFilter=()=>document.getElementById('log-filter').value;

function showFallbackBanner(reason){
  const banner=document.getElementById('fallback-banner');
  if(!banner)return;
  banner.style.display='flex';
  const titleEl=banner.querySelector('[data-i18n="fallback_title"]');
  if(titleEl)titleEl.textContent=t('fallback_title');
  const reasonEl=document.getElementById('fallback-reason');
  if(reasonEl)reasonEl.textContent=reason;
}

function setBrewStatus(online,version){
  state.brewOnline=online;state.brewVer=version||0;
  const led=document.getElementById('brewLed');
  const txt=document.getElementById('brewText');
  const vbadge=document.getElementById('brewVerBadge');
  if(online){
    led.classList.add('on');
    txt.textContent=t('brew_online');txt.style.color='var(--accent2)';
    if(vbadge){
      const v=version||0;
      vbadge.textContent='v'+v;vbadge.style.display='inline-block';
      if(v>=1){vbadge.style.background='rgba(0,212,168,0.15)';vbadge.style.color='var(--accent)';vbadge.style.border='1px solid rgba(0,212,168,0.4)';}
      else{vbadge.style.background='rgba(255,178,36,0.15)';vbadge.style.color='var(--warn)';vbadge.style.border='1px solid rgba(255,178,36,0.4)';}
    }
  } else {
    led.classList.remove('on');txt.textContent=t('brew_offline');txt.style.color='';
    if(vbadge)vbadge.style.display='none';
  }
  // Update stat card
  const bv=document.getElementById('stat-brew-val');
  const bs=document.getElementById('stat-brew-sub');
  if(bv){bv.textContent=online?t('brew_online'):t('brew_offline');bv.style.color=online?'var(--accent2)':'var(--danger)';}
  if(bs)bs.textContent=online?`Brew v${version||0}`:'—';
  // System panel
  updateSysBtsPanel(document.getElementById('connLed').classList.contains('on'),online,version||0);
}

function connect(){
  const proto=location.protocol==='https:'?'wss:':'ws:';
  ws=new WebSocket(`${proto}//${location.host}/ws`);
  ws.onopen=()=>{
    document.getElementById('connLed').classList.add('on');
    const ct=document.getElementById('connText');ct.textContent=t('online');ct.style.color='var(--accent)';
    updateSysBtsPanel(true,state.brewOnline,state.brewVer);
    ws.send(JSON.stringify({type:'subscribe'}));
  };
  ws.onclose=()=>{
    document.getElementById('connLed').classList.remove('on');
    const ct=document.getElementById('connText');ct.textContent=t('offline');ct.style.color='var(--danger)';
    setBrewStatus(false,0);
    updateSysBtsPanel(false,false,0);
    setTimeout(connect,3000);
  };
  ws.onmessage=(e)=>{try{handleMsg(JSON.parse(e.data));}catch{}};
}

function handleMsg(msg){
  switch(msg.type){
    case 'snapshot':
      state.ms={};state.calls={};state.lastHeard=msg.last_heard||[];
      (msg.ms||[]).forEach(m=>{state.ms[m.issi]={...m,_last_seen_ts:Date.now()-(m.last_seen_secs_ago||0)*1000,energy_saving_mode:m.energy_saving_mode||0};});
      (msg.calls||[]).forEach(c=>{
        state.calls[c.call_id]={...c,started_at:Date.now()-(c.started_secs_ago||0)*1000};
        if(c.ts&&c.ts>=2){
          const lbl=c.call_type==='group'?`GSSI ${c.gssi}`:(c.called_issi?`ISSI ${c.called_issi}`:'P2P');
          const sub=c.call_type==='group'?t('call_group'):(c.simplex?t('call_p2p_s'):t('call_p2p_d'));
          tsSetCall(c.ts,c.call_id,c.call_type,lbl,sub);
        }
      });
      if(msg.log&&msg.log.length){document.getElementById('log-container').innerHTML='';msg.log.forEach(e=>appendLog(e));}
      setBrewStatus(!!msg.brew_online,msg.brew_version||0);
      if(msg.fallback_config_active){showFallbackBanner(msg.fallback_config_reason||'');}
      renderAll();break;
    case 'brew_status':
      setBrewStatus(!!msg.connected,msg.brew_version||0);break;
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
      state.calls[msg.call_id]={...msg,started_at:Date.now()};
      if(msg.last_heard)pushLastHeard(msg.last_heard);
      if(msg.ts&&msg.ts>=2){
        const lbl=msg.call_type==='group'?`GSSI ${msg.gssi}`:(msg.called_issi?`ISSI ${msg.called_issi}`:'P2P');
        const sub=msg.call_type==='group'?t('call_group'):(msg.simplex?t('call_p2p_s'):t('call_p2p_d'));
        tsSetCall(msg.ts,msg.call_id,msg.call_type,lbl,sub);
        updateTsBlocks();
      }
      renderCalls();renderLastHeard();break;
    case 'call_ended':
      tsClearCall(msg.call_id);updateTsBlocks();
      delete state.calls[msg.call_id];renderCalls();break;
    case 'ts_voice':
      tsVoice(msg.ts);break;
    case 'speaker_changed':
      if(state.calls[msg.call_id])state.calls[msg.call_id].active_speaker=msg.speaker_issi;
      if(msg.last_heard){pushLastHeard(msg.last_heard);renderLastHeard();}
      renderCalls();break;
    case 'ms_energy_saving':
      if(state.ms[msg.issi])state.ms[msg.issi].energy_saving_mode=msg.mode;
      renderStations();break;
    case 'last_heard':
      pushLastHeard({issi:msg.issi,activity:msg.activity,dest:msg.dest,ts:new Date().toTimeString().slice(0,8)});
      renderLastHeard();break;
    case 'log':appendLog(msg);break;
  }
}

// ── Render helpers ────────────────────────────────────────────────────────
function eeLabel(mode){
  if(!mode||mode===0)return '<span style="color:var(--text3);font-size:10px">—</span>';
  const labels=['','EG1','EG2','EG3','EG4','EG5','EG6','EG7'];
  const colors=['','var(--accent)','var(--accent)','var(--accent2)','var(--accent2)','var(--warn)','var(--danger)','var(--danger)'];
  const tips=['','~1s','~2s','~3s','~4s','~5s','~6s','~7s'];
  const col=colors[mode]||'var(--text2)';
  return `<span class="badge" title="Energy Economy Mode ${mode} — wake ${tips[mode]}" style="background:color-mix(in srgb,${col} 12%,transparent);border-color:${col};color:${col};font-size:9px">${labels[mode]}</span>`;
}
function lastSeenLabel(secs){
  if(secs==null)return'—';
  if(secs<5)return'<span style="color:var(--accent)">now</span>';
  if(secs<60)return`<span style="color:var(--accent2)">${secs}s</span>`;
  if(secs<3600)return`<span style="color:var(--text2)">${Math.floor(secs/60)}m${secs%60}s</span>`;
  return`<span style="color:var(--warn)">${Math.floor(secs/3600)}h${Math.floor((secs%3600)/60)}m</span>`;
}
function pushLastHeard(entry){
  const now=new Date().toTimeString().slice(0,8);
  state.lastHeard.unshift({ts:entry.ts||now,issi:entry.issi,activity:entry.activity,dest:entry.dest||0});
  if(state.lastHeard.length>50)state.lastHeard.length=50;
}
function activityBadge(activity){
  if(activity==='call_group')return`<span class="badge badge-blue">${t('act_call_group')}</span>`;
  if(activity==='call_individual')return`<span class="badge badge-yellow">${t('act_call_individual')}</span>`;
  if(activity==='sds')return`<span class="badge" style="background:rgba(180,100,255,0.15);color:#c87aff;border-color:rgba(180,100,255,0.4)">${t('act_sds')}</span>`;
  return`<span class="badge badge-dim">${activity}</span>`;
}
function rssiColor(v){if(v==null)return'var(--text3)';if(v>-20)return'var(--accent)';if(v>-30)return'var(--accent2)';if(v>-40)return'var(--warn)';return'var(--danger)';}
function rssiPct(v){if(v==null)return 0;return Math.max(0,Math.min(100,(v+60)/50*100));}
function escHtml(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');}
function renderAll(){renderStations();renderCalls();renderLastHeard();updateTsBlocks();}

// ── TS Visualizer ─────────────────────────────────────────────────────────
// tsState[ts-1]: {call_id, call_type, label, sub, voice_ts, started_at}
const tsState=[null,null,null,null];
const TS_VOICE_DECAY_MS=800;
// Random wave heights per bar per TS — regenerated on each voice frame
const tsWaveHeights=[[],[],[],[]];

function tsRandWave(ts){
  const bars=7;
  tsWaveHeights[ts-1]=Array.from({length:bars},()=>Math.floor(Math.random()*14)+4);
}
function tsApplyWave(ts,active){
  const block=document.getElementById('ts-block-'+ts);
  if(!block)return;
  const bars=block.querySelectorAll('.ts-wave-bar');
  if(active){
    tsWaveHeights[ts-1].forEach((h,i)=>{if(bars[i])bars[i].style.height=h+'px';});
  } else {
    bars.forEach(b=>b.style.height='3px');
  }
}

function updateTsBlocks(){
  const now=Date.now();
  for(let i=0;i<4;i++){
    const ts=i+1;
    const block=document.getElementById('ts-block-'+ts);
    if(!block)continue;
    const label=block.querySelector('.ts-label');
    const sub=block.querySelector('.ts-sub');
    const dur=block.querySelector('.ts-duration-bar');

    if(ts===1){
      block.className='ts-block mcch';
      label.textContent='MCCH';
      sub.textContent='Control';
      // subtle MCCH wave animation
      if(!tsWaveHeights[0].length)tsRandWave(1);
      tsApplyWave(1,true);
      if(dur)dur.style.width='0%';
      continue;
    }

    const st=tsState[i];
    if(!st){
      block.className='ts-block';
      label.textContent='—';
      sub.textContent='Idle';
      tsApplyWave(ts,false);
      if(dur)dur.style.width='0%';
      continue;
    }

    const voiceRecent=st.voice_ts&&(now-st.voice_ts)<TS_VOICE_DECAY_MS;

    if(voiceRecent){
      block.className='ts-block voice';
      label.textContent=st.label||'—';
      sub.textContent='▶ TX';
    } else {
      block.className='ts-block call';
      label.textContent=st.label||'—';
      const elapsed=Math.floor((now-(st.started_at||now))/1000);
      sub.textContent=elapsed>0?formatDur(elapsed):(st.sub||'Alloc');
    }
    tsApplyWave(ts, voiceRecent);

    // Duration bar — fills over 120s then stays full
    if(dur&&st.started_at){
      const pct=Math.min(100,((now-st.started_at)/120000)*100);
      dur.style.width=pct+'%';
    }
  }
}

function formatDur(s){
  if(s<60)return s+'s';
  return Math.floor(s/60)+'m'+String(s%60).padStart(2,'0')+'s';
}

function tsSetCall(ts, call_id, call_type, label, sub){
  if(ts<2||ts>4)return;
  tsState[ts-1]={call_id,call_type,label,sub,voice_ts:null,started_at:Date.now()};
}
function tsClearCall(call_id){
  for(let i=1;i<4;i++){if(tsState[i]&&tsState[i].call_id===call_id)tsState[i]=null;}
}
function tsVoice(ts){
  if(ts<2||ts>4)return;
  if(!tsState[ts-1])tsState[ts-1]={call_id:0,call_type:'',label:'Traffic',sub:'',voice_ts:null,started_at:Date.now()};
  tsState[ts-1].voice_ts=Date.now();
  // Randomize waveform bars on each voice frame for live feel
  tsRandWave(ts);
  // Flash effect
  const block=document.getElementById('ts-block-'+ts);
  if(block){
    const flash=block.querySelector('.ts-flash');
    if(flash){flash.style.animation='none';void flash.offsetWidth;flash.style.animation='ts-flash-in 0.08s ease-out forwards';}
  }
  updateTsBlocks();
}
setInterval(updateTsBlocks, 150); // refresh to catch voice decay + duration tick

function renderStations(){
  const ms=Object.values(state.ms);
  const msCount=ms.length,callCount=Object.keys(state.calls).length;
  document.getElementById('stat-ms').textContent=msCount;
  document.getElementById('stat-calls').textContent=callCount;
  document.getElementById('badge-ms').textContent=msCount;
  const bc=document.getElementById('badge-calls');
  if(bc){bc.textContent=callCount;bc.style.display=callCount?'flex':'none';}
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
    return`<tr>
      <td><code>${m.issi}</code></td><td>${grps}</td>
      <td style="text-align:center">${eeLabel(m.energy_saving_mode||0)}</td>
      <td><div class="rssi-bar"><div class="rssi-track"><div class="rssi-fill" style="width:${pct}%;background:${col}"></div></div><span class="rssi-val" style="color:${col}">${rL}</span></div></td>
      <td><span class="badge badge-green">${t('online_badge')}</span></td>
      <td>${lastSeenLabel(ls)}</td>
      <td><button class="btn btn-sm" onclick="openSds(${m.issi})">${t('sds')}</button> <button class="btn btn-sm btn-danger" onclick="kickMs(${m.issi})">${t('kick')}</button></td>
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
    return`<tr><td><code>${c.call_id}</code></td><td><span class="badge ${badge}">${label}</span></td><td>${c.caller_issi?`<code>${c.caller_issi}</code>`:'—'}</td><td>${to}</td><td>${spk}</td><td style="font-family:var(--mono);font-size:12px;color:var(--accent2);font-weight:600">${mm}:${ss}</td></tr>`;
  }).join('');
}

function renderLastHeard(){
  const tb=document.getElementById('lastheard-tbody');
  if(!tb)return;
  if(!state.lastHeard.length){tb.innerHTML=`<tr><td colspan="4"><div class="empty-state"><div class="empty-icon">🎙</div><div class="empty-text">${t('no_activity')}</div></div></td></tr>`;return;}
  tb.innerHTML=state.lastHeard.map(e=>{
    const destStr=e.dest?`<code>${e.dest}</code>`:'<span style="color:var(--text3)">—</span>';
    const isOnline=!!state.ms[e.issi];
    const issiHtml=`<code>${e.issi}</code>${isOnline?` <span class="badge badge-green" style="font-size:9px">${t('online_badge')}</span>`:''}`;
    return`<tr>
      <td style="font-family:var(--mono);font-size:11px;color:var(--text2)">${e.ts}</td>
      <td>${issiHtml}</td><td>${activityBadge(e.activity)}</td><td>${destStr}</td>
    </tr>`;
  }).join('');
}
function clearLastHeard(){state.lastHeard=[];renderLastHeard();}

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

// ── Config ────────────────────────────────────────────────────────────────
async function loadConfig(){
  try{const r=await fetch('/api/config');if(r.ok)document.getElementById('config-editor').value=await r.text();else setConfigMsg(t('conn_error'),false);}
  catch{setConfigMsg(t('conn_error'),false);}
}
async function saveConfig(){
  try{const r=await fetch('/api/config',{method:'POST',body:document.getElementById('config-editor').value});if(r.ok)setConfigMsg(t('saved'),true);else setConfigMsg(t('save_fail')+': '+await r.text(),false);}
  catch(e){setConfigMsg(t('conn_error'),false);}
}
function setConfigMsg(txt,ok){const el=document.getElementById('config-msg');el.textContent=txt;el.style.color=ok?'var(--accent)':'var(--danger)';}
function wsSend(msg){if(ws&&ws.readyState===WebSocket.OPEN){ws.send(JSON.stringify(msg));return true;}return false;}
async function restartService(){if(!confirm(t('confirm_restart')))return;wsSend({type:'restart'});}
async function shutdownService(){if(!confirm(t('confirm_shutdown')))return;wsSend({type:'shutdown'});}
function kickMs(issi){if(!confirm(t('confirm_kick',{issi})))return;wsSend({type:'kick',issi});}
function openSds(issi){sdsDest=issi;document.getElementById('sds-dest').value=issi;document.getElementById('sds-msg').value='';document.getElementById('sds-modal').classList.add('open');}
function closeSdsModal(){document.getElementById('sds-modal').classList.remove('open');}
function sendSds(){const dest=parseInt(document.getElementById('sds-dest').value),msg=document.getElementById('sds-msg').value.trim();if(!dest||!msg)return;wsSend({type:'sds',dest_issi:dest,message:msg});closeSdsModal();}

// ── OTA Update ────────────────────────────────────────────────────────────
let updatePollTimer=null;
function closeUpdateModal(){document.getElementById('update-modal').classList.remove('open');if(updatePollTimer){clearInterval(updatePollTimer);updatePollTimer=null;}}
async function startUpdate(){
  if(!confirm(t('update_confirm')))return;
  document.getElementById('update-modal').classList.add('open');
  document.getElementById('update-modal-title').textContent=t('update_title');
  const termEl=document.getElementById('update-terminal');
  const msgEl=document.getElementById('update-status-msg');
  const closeBtn=document.getElementById('update-close-btn');
  termEl.textContent='';msgEl.className='update-status running';msgEl.textContent=t('update_running');closeBtn.disabled=true;
  try{
    const r=await fetch('/api/update',{method:'POST'});
    if(!r.ok&&r.status!==409){msgEl.className='update-status err';msgEl.textContent='✗ '+await r.text();closeBtn.disabled=false;return;}
  }catch(e){msgEl.className='update-status err';msgEl.textContent='✗ '+e.message;closeBtn.disabled=false;return;}
  let lastLen=0;
  updatePollTimer=setInterval(async()=>{
    try{
      const r=await fetch('/api/update/status');if(!r.ok)return;
      const j=await r.json();
      if(j.log&&j.log.length>lastLen){termEl.textContent+=j.log.slice(lastLen);lastLen=j.log.length;termEl.scrollTop=termEl.scrollHeight;}
      if(j.status==='done_ok'){clearInterval(updatePollTimer);updatePollTimer=null;msgEl.className='update-status ok';msgEl.textContent=t('update_done_ok');closeBtn.disabled=false;}
      else if(j.status==='done_err'){clearInterval(updatePollTimer);updatePollTimer=null;msgEl.className='update-status err';msgEl.textContent=t('update_done_err');closeBtn.disabled=false;}
    }catch{}
  },1000);
}

// ── System tab ────────────────────────────────────────────────────────────
let sysData=null;
let sysAutoRefreshTimer = null;
function toggleSysAutoRefresh(on) {
  if (sysAutoRefreshTimer) { clearInterval(sysAutoRefreshTimer); sysAutoRefreshTimer = null; }
  if (on) sysAutoRefreshTimer = setInterval(loadSystemInfo, 5000);
}

async function loadSystemInfo(){
  try{
    const r=await fetch('/api/system');if(!r.ok)return;
    sysData=await r.json();
    document.getElementById('sysHostname').textContent=sysData.hostname||'—';
    document.getElementById('sysVersion').textContent=sysData.stack_version||'—';
    document.getElementById('sysOs').textContent=sysData.os||'—';
    document.getElementById('sysConfigPath').textContent=sysData.config_path||'—';

    // CPU
    const cpuEl=document.getElementById('sysCpu');
    if(cpuEl) cpuEl.textContent=(sysData.cpu_model||'—')+(sysData.cpu_cores?` (${sysData.cpu_cores} cores)`:'');
    const cpuPct=sysData.cpu_pct||0;
    const cpuBarEl=document.getElementById('sysCpuBar');
    const cpuPctEl=document.getElementById('sysCpuPct');
    if(cpuBarEl) cpuBarEl.style.width=cpuPct+'%';
    if(cpuBarEl) cpuBarEl.style.background=cpuPct>80?'var(--danger)':cpuPct>60?'var(--warn)':'var(--accent)';
    if(cpuPctEl) cpuPctEl.textContent=cpuPct+'%';

    // RAM
    const ramTotal=sysData.ram_total_mb||0;
    const ramUsed=sysData.ram_used_mb||0;
    const ramPct=ramTotal>0?Math.round(ramUsed/ramTotal*100):0;
    const ramBarEl=document.getElementById('sysRamBar');
    const ramValEl=document.getElementById('sysRamVal');
    if(ramBarEl) ramBarEl.style.width=ramPct+'%';
    if(ramBarEl) ramBarEl.style.background=ramPct>85?'var(--danger)':ramPct>70?'var(--warn)':'var(--accent2)';
    if(ramValEl) ramValEl.textContent=`${ramUsed} / ${ramTotal} MB (${ramPct}%)`;

    // Temperature
    const tempCard=document.getElementById('cpu-temp-card');
    const tempEl=document.getElementById('sysCpuTemp');
    const tempSub=document.getElementById('sysCpuTempSub');
    if(sysData.cpu_temp_c!=null){
      const t=sysData.cpu_temp_c.toFixed(1);
      if(tempCard) tempCard.style.display='';
      if(tempEl){ tempEl.textContent=t+'°C'; tempEl.style.color=sysData.cpu_temp_c>75?'var(--danger)':sysData.cpu_temp_c>60?'var(--warn)':'var(--accent)';}
      if(tempSub) tempSub.textContent=sysData.cpu_temp_c>75?'⚠ HOT':sysData.cpu_temp_c>60?'Warm':'OK';
    } else {
      if(tempCard) tempCard.style.display='none';
    }

    // RF / SoapySDR
    const soapyEl=document.getElementById('sysSoapy');
    if(soapyEl) soapyEl.textContent=sysData.soapy_info||'—';

    updateSystemUptime();
  }catch(e){console.error('loadSystemInfo',e);}
}
function updateSystemUptime(){
  if(!sysData||!sysData.uptime_secs)return;
  const u=sysData.uptime_secs;
  const d=Math.floor(u/86400),h=Math.floor((u%86400)/3600),m=Math.floor((u%3600)/60),s=u%60;
  let str='';if(d>0)str+=d+'d ';if(h>0||d>0)str+=h+'h ';if(m>0||h>0||d>0)str+=m+'m ';str+=s+'s';
  document.getElementById('sysUptime').textContent=str;
}

async function loadConfigProfiles(){
  const list=document.getElementById('profileList');
  try{
    const r=await fetch('/api/configs');if(!r.ok){list.innerHTML='<div style="color:var(--danger);font-family:var(--mono);font-size:12px;">Failed to load profiles</div>';return;}
    const profiles=await r.json();
    if(!profiles||!profiles.length){list.innerHTML=`<div style="color:var(--text3);font-family:var(--mono);font-size:12px;">${t('sys_no_profiles')}</div>`;return;}
    list.innerHTML='';
    profiles.forEach(p=>{
      const row=document.createElement('div');
      row.className='profile-item'+(p.active?' active-profile':'');
      const name=document.createElement('div');name.className='profile-name';name.textContent=p.name;row.appendChild(name);
      if(p.active){
        const b=document.createElement('span');b.className='badge badge-green';b.textContent=t('sys_active_badge');row.appendChild(b);
      } else {
        const editBtn=document.createElement('button');
        editBtn.className='btn btn-sm';editBtn.textContent=t('profile_edit_btn')||'Edit';
        editBtn.onclick=()=>openEditProfile(p.name);
        row.appendChild(editBtn);
        const btn=document.createElement('button');btn.className='btn btn-primary btn-sm';btn.textContent=t('sys_activate');
        btn.onclick=()=>activateProfile(p.name);row.appendChild(btn);
      }
      list.appendChild(row);
    });
  }catch(e){list.innerHTML=`<div style="color:var(--danger);font-family:var(--mono);font-size:12px;">Error: ${e.message}</div>`;}
}

async function activateProfile(name){
  if(!confirm(t('sys_activate_confirm').replace('{name}',name)))return;
  try{
    const r=await fetch('/api/configs/activate',{method:'POST',body:name});
    if(r.ok){wsSend({type:'restart'});}
    else alert('Failed: '+await r.text());
  }catch(e){alert('Error: '+e.message);}
}

function updateSysBtsPanel(online,brewOnline,brewVer){
  const ipEl=document.getElementById('sysBtsIp');
  const stEl=document.getElementById('sysBtsStatus');
  const bsEl=document.getElementById('sysBrewStatus');
  const bdEl=document.getElementById('sysBrewBadge');
  if(ipEl)ipEl.textContent=online?location.hostname:'—';
  if(stEl){stEl.textContent=online?t('online'):t('offline');stEl.style.color=online?'var(--accent)':'var(--danger)';}
  if(bsEl){bsEl.textContent=brewOnline?t('brew_online'):t('brew_offline');bsEl.style.color=brewOnline?'var(--accent2)':'var(--danger)';}
  if(bdEl){bdEl.textContent=brewOnline?`Brew v${brewVer||0}`:'—';}
}

// ── Edit Profile (inactive config) ───────────────────────────────────────
let editProfileName = null;
async function openEditProfile(name) {
  editProfileName = name;
  document.getElementById('edit-profile-name').textContent = name;
  document.getElementById('edit-profile-msg').textContent = '';
  document.getElementById('edit-profile-editor').value = 'Loading...';
  document.getElementById('edit-profile-modal').classList.add('open');
  try {
    const r = await fetch(`/api/configs/${encodeURIComponent(name)}`);
    if (r.ok) {
      document.getElementById('edit-profile-editor').value = await r.text();
    } else {
      document.getElementById('edit-profile-editor').value = '';
      document.getElementById('edit-profile-msg').textContent = 'Failed to load: ' + await r.text();
      document.getElementById('edit-profile-msg').style.color = 'var(--danger)';
    }
  } catch(e) {
    document.getElementById('edit-profile-editor').value = '';
    document.getElementById('edit-profile-msg').textContent = 'Error: ' + e.message;
    document.getElementById('edit-profile-msg').style.color = 'var(--danger)';
  }
}

function closeEditProfileModal() {
  document.getElementById('edit-profile-modal').classList.remove('open');
  editProfileName = null;
}

async function saveEditProfile() {
  if (!editProfileName) return;
  const content = document.getElementById('edit-profile-editor').value;
  const msgEl = document.getElementById('edit-profile-msg');
  try {
    const r = await fetch(`/api/configs/${encodeURIComponent(editProfileName)}`, {
      method: 'POST',
      headers: { 'Content-Type': 'text/plain' },
      body: content,
    });
    if (r.ok) {
      msgEl.textContent = t('profile_edit_save_ok');
      msgEl.style.color = 'var(--accent)';
    } else {
      msgEl.textContent = t('profile_edit_save_fail') + ': ' + await r.text();
      msgEl.style.color = 'var(--danger)';
    }
  } catch(e) {
    msgEl.textContent = 'Error: ' + e.message;
    msgEl.style.color = 'var(--danger)';
  }
}

// ── Live SDS Broadcast ────────────────────────────────────────────────────
async function loadLiveSds() {
  const list = document.getElementById('live-sds-list');
  const clearBtn = document.getElementById('live-sds-clear-btn');
  try {
    const r = await fetch('/api/live-sds');
    if (!r.ok) { list.innerHTML = `<div style="color:var(--danger);font-size:12px">Error ${r.status}</div>`; return; }
    const items = await r.json();
    if (!items || !items.length) {
      list.innerHTML = `<div style="color:var(--text3);font-family:var(--mono);font-size:12px">${t('live_sds_empty')}</div>`;
      if (clearBtn) clearBtn.style.display = 'none';
      return;
    }
    if (clearBtn) clearBtn.style.display = '';
    list.innerHTML = '';
    items.forEach(m => {
      const row = document.createElement('div');
      row.style.cssText = 'display:flex;align-items:center;gap:10px;padding:8px 0;border-bottom:1px solid var(--border)';
      const repeatLabel = m.repeat_count === 0
        ? `<span style="color:var(--accent2);font-size:11px">${t('live_sds_forever')}</span>`
        : `<span style="font-size:11px;color:var(--text2)">${m.sent_count}/${m.repeat_count}${t('live_sds_times')}</span>`;
      row.innerHTML = `
        <div style="flex:1;min-width:0">
          <div style="font-size:13px;font-weight:600;color:var(--text);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${escHtml(m.text)}</div>
          <div style="font-size:10px;color:var(--text3);font-family:var(--mono);margin-top:2px">
            PID ${m.protocol_id} · src ${m.source_issi} · ${t('live_sds_sent')}: ${repeatLabel}
          </div>
        </div>
        <button class="btn btn-sm btn-danger" onclick="deleteLiveSds(${m.id})" title="${t('live_sds_delete')}">${t('live_sds_delete')}</button>`;
      list.appendChild(row);
    });
  } catch(e) {
    list.innerHTML = `<div style="color:var(--danger);font-size:12px">Error: ${escHtml(e.message)}</div>`;
  }
}

async function addLiveSds() {
  const text = document.getElementById('live-sds-text').value.trim();
  const repeat = parseInt(document.getElementById('live-sds-repeat').value) || 0;
  if (!text) { document.getElementById('live-sds-text').focus(); return; }
  try {
    const r = await fetch('/api/live-sds', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text, repeat_count: repeat, protocol_id: 220, source_issi: 16777215 })
    });
    if (r.ok) {
      document.getElementById('live-sds-text').value = '';
      document.getElementById('live-sds-repeat').value = '0';
      await loadLiveSds();
    } else {
      alert('Error: ' + await r.text());
    }
  } catch(e) { alert('Error: ' + e.message); }
}

async function deleteLiveSds(id) {
  try {
    const r = await fetch(`/api/live-sds/${id}`, { method: 'DELETE' });
    if (r.ok) await loadLiveSds();
  } catch(e) { alert('Error: ' + e.message); }
}

async function clearAllLiveSds() {
  if (!confirm(t('live_sds_clear_all') + '?')) return;
  try {
    const r = await fetch('/api/live-sds', { method: 'DELETE' });
    if (r.ok) await loadLiveSds();
  } catch(e) { alert('Error: ' + e.message); }
}

// ── Tick ──────────────────────────────────────────────────────────────────
setInterval(()=>{
  if(document.getElementById('page-calls').classList.contains('active'))renderCalls();
  if(document.getElementById('page-stations').classList.contains('active'))renderStations();
  if(document.getElementById('page-lastheard').classList.contains('active'))renderLastHeard();
  if(document.getElementById('page-system').classList.contains('active'))updateSystemUptime();
},1000);

// Refresh live SDS list every 10s when System tab is visible (sent_count updates in background)
setInterval(()=>{
  if(document.getElementById('page-system').classList.contains('active')){
    loadLiveSds();
  }
},10000);

// ── Init ──────────────────────────────────────────────────────────────────
(function(){
  const ua=navigator.userAgent;
  let os='—';
  if(/Windows NT ([\d.]+)/.test(ua)){const v=ua.match(/Windows NT ([\d.]+)/)[1];os={'10.0':'Win10','11.0':'Win11','6.3':'Win8.1','6.1':'Win7'}[v]||'Windows';}
  else if(/Mac OS X ([\d_]+)/.test(ua)){os='macOS '+ua.match(/Mac OS X ([\d_]+)/)[1].replace(/_/g,'.');}
  else if(/Android ([\d.]+)/.test(ua)){os='Android '+ua.match(/Android ([\d.]+)/)[1];}
  else if(/Linux/.test(ua)){os='Linux';}
  else if(/iPhone|iPad/.test(ua)){os='iOS';}
  let br='—';
  if(/Firefox\/([\d.]+)/.test(ua))br='Firefox '+ua.match(/Firefox\/([\d.]+)/)[1].split('.')[0];
  else if(/Edg\/([\d.]+)/.test(ua))br='Edge '+ua.match(/Edg\/([\d.]+)/)[1].split('.')[0];
  else if(/Chrome\/([\d.]+)/.test(ua))br='Chrome '+ua.match(/Chrome\/([\d.]+)/)[1].split('.')[0];
  else if(/Safari\/([\d.]+)/.test(ua)&&/Version\/([\d.]+)/.test(ua))br='Safari '+ua.match(/Version\/([\d.]+)/)[1].split('.')[0];
  const el=document.getElementById('cr-ua');
  if(el)el.textContent=os+' · '+br;
})();
if(sidebarCollapsed)document.getElementById('sidebar').classList.add('collapsed');
setLang(currentLang);
setTheme(currentTheme);
connect();
</script>
</body>
</html>
"#;
