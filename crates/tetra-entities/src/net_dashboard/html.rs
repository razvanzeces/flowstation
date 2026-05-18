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
  --sans: 'ui-sans-serif','system-ui',-apple-system,'Segoe UI',sans-serif;
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
  border:1px solid var(--border);border-radius:6px;
  padding:12px 10px 10px;text-align:center;
  position:relative;overflow:hidden;
  transition:border-color 0.3s,box-shadow 0.3s;
  background:var(--bg3);
}
.ts-block.mcch{
  border-color:rgba(77,166,255,0.3);
  background:rgba(77,166,255,0.04);
}
.ts-block.active{
  border-color:rgba(0,212,168,0.6);
  box-shadow:0 0 12px rgba(0,212,168,0.15);
  background:rgba(0,212,168,0.05);
}
.ts-block.active .ts-activity-bar{animation:ts-pulse 0.4s ease-out;}
.ts-num{
  font-family:var(--mono);font-size:9px;font-weight:700;
  letter-spacing:0.12em;color:var(--text3);margin-bottom:8px;
}
.ts-led{
  width:10px;height:10px;border-radius:50%;
  background:var(--bg4);margin:0 auto 8px;
  transition:background 0.2s,box-shadow 0.2s;
}
.ts-block.mcch .ts-led{background:var(--accent2);box-shadow:0 0 6px rgba(77,166,255,0.5);}
.ts-block.active .ts-led{background:var(--accent);box-shadow:0 0 8px rgba(0,212,168,0.6);}
.ts-label{
  font-family:var(--mono);font-size:10px;font-weight:700;
  letter-spacing:0.06em;
  color:var(--text3);
  min-height:14px;
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
}
.ts-block.mcch .ts-label{color:var(--accent2);}
.ts-block.active .ts-label{color:var(--accent);}
.ts-sub{
  font-family:var(--mono);font-size:9px;color:var(--text3);
  margin-top:3px;min-height:12px;
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
}
.ts-activity-bar{
  position:absolute;bottom:0;left:0;right:0;height:2px;
  background:var(--accent);transform:scaleX(0);transform-origin:left;
  transition:transform 0.15s ease;
}
.ts-block.active .ts-activity-bar{transform:scaleX(1);}
@keyframes ts-pulse{0%{opacity:1;}50%{opacity:0.4;}100%{opacity:1;}}
.rf-grid{
  display:grid;
  grid-template-columns:minmax(360px,2fr) minmax(280px,1fr);
  gap:16px;
}
.rf-panel{
  background:var(--bg2);
  border:1px solid var(--border);
  border-radius:8px;
  padding:12px;
}
.rf-panel-title{
  font-family:var(--mono);
  font-size:11px;
  color:var(--text2);
  margin-bottom:8px;
  display:flex;
  justify-content:space-between;
  gap:12px;
}
.rf-canvas{
  width:100%;
  height:260px;
  display:block;
  background:#050607;
  border:1px solid rgba(255,255,255,0.08);
}
.rf-canvas.small{height:260px;}
.rf-metrics{
  display:grid;
  grid-template-columns:repeat(auto-fit,minmax(140px,1fr));
  gap:10px;
  margin-bottom:16px;
}
.rf-metric{
  background:var(--bg2);
  border:1px solid var(--border);
  border-radius:8px;
  padding:12px;
}
.rf-metric-label{
  font-family:var(--mono);
  font-size:10px;
  color:var(--text3);
  text-transform:uppercase;
}
.rf-metric-value{
  font-family:var(--mono);
  font-size:20px;
  color:var(--accent);
  margin-top:6px;
}
@media(max-width:900px){.rf-grid{grid-template-columns:1fr}.rf-canvas{height:220px}}
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
    <div class="nav-item" onclick="showPage('rf',this)" id="nav-rf">
      <span class="nav-icon">◫</span>
      <span class="nav-label" data-i18n="rf">RF MONITOR</span>
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
      </div>
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
            <div class="ts-label">MCCH</div>
            <div class="ts-sub">Control</div>
            <div class="ts-activity-bar"></div>
          </div>
          <div class="ts-block" id="ts-block-2">
            <div class="ts-num">TS 2</div>
            <div class="ts-led"></div>
            <div class="ts-label">—</div>
            <div class="ts-sub">Idle</div>
            <div class="ts-activity-bar"></div>
          </div>
          <div class="ts-block" id="ts-block-3">
            <div class="ts-num">TS 3</div>
            <div class="ts-led"></div>
            <div class="ts-label">—</div>
            <div class="ts-sub">Idle</div>
            <div class="ts-activity-bar"></div>
          </div>
          <div class="ts-block" id="ts-block-4">
            <div class="ts-num">TS 4</div>
            <div class="ts-led"></div>
            <div class="ts-label">—</div>
            <div class="ts-sub">Idle</div>
            <div class="ts-activity-bar"></div>
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

    <!-- ── RF MONITOR ── -->
    <div class="page" id="page-rf">
      <div class="rf-metrics">
        <div class="rf-metric">
          <div class="rf-metric-label">Center</div>
          <div class="rf-metric-value" id="rf-center">—</div>
        </div>
        <div class="rf-metric">
          <div class="rf-metric-label">RMS</div>
          <div class="rf-metric-value" id="rf-rms">—</div>
        </div>
        <div class="rf-metric">
          <div class="rf-metric-label">Peak</div>
          <div class="rf-metric-value" id="rf-peak">—</div>
        </div>
        <div class="rf-metric">
          <div class="rf-metric-label">Rate</div>
          <div class="rf-metric-value" id="rf-rate">—</div>
        </div>
      </div>
      <div class="rf-grid">
        <div class="rf-panel">
          <div class="rf-panel-title"><span>TX Spectrum</span><span id="rf-age">waiting</span></div>
          <canvas id="rf-spectrum" class="rf-canvas" width="900" height="260"></canvas>
        </div>
        <div class="rf-panel">
          <div class="rf-panel-title"><span>TX Constellation</span><span>IQ</span></div>
          <canvas id="rf-constellation" class="rf-canvas small" width="420" height="260"></canvas>
        </div>
        <div class="rf-panel" style="grid-column:1/-1">
          <div class="rf-panel-title"><span>TX Waterfall</span><span>WebSocket live</span></div>
          <canvas id="rf-waterfall" class="rf-canvas" width="1100" height="260"></canvas>
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
      </div>

      <!-- System info -->
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="sys_info">System Info</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="loadSystemInfo()">↻ Refresh</button>
          </div>
        </div>
        <div class="card-body">
          <div class="info-row"><div class="info-key" data-i18n="sys_version">FS Version</div><div class="info-val accent" id="sysVersion">—</div></div>
          <div class="info-row"><div class="info-key" data-i18n="sys_os">OS</div><div class="info-val" id="sysOs">—</div></div>
          <div class="info-row"><div class="info-key" data-i18n="sys_config">Active Config</div><div class="info-val" id="sysConfigPath">—</div></div>
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
    </div>

  </div><!-- /content -->
</div><!-- /main -->

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
    stations:'Radios',calls:'Calls',lastheard:'Last Heard',rf:'RF Monitor',log:'Log',config:'Config',
    terminals:'Radios',registered:'registered',
    active_calls:'Active Calls',circuits:'circuits in use',
    registered_terminals:'Registered Radios',
    no_terminals:'No radios registered',no_calls:'No active calls',
    live_log:'Live Log',autoscroll:'Auto-scroll',filter_all:'All',
    clear:'Clear',restart:'⟳ Restart',shutdown:'⏻ Shutdown',save:'Save',
    sds_title:'⬡ Send SDS Message',sds_dest:'Destination ISSI',
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
    sys_profiles:'Perfiles de Config',sys_activate:'Activar y Reiniciar',
    sys_active_badge:'ACTIVO',sys_no_profiles:'No se encontraron perfiles .toml en el directorio.',
    sys_activate_confirm:'¿Cambiar al perfil "{name}" y reiniciar?\nLa config actual será respaldada.',
    sys_bts:'Conexión BTS',
  },
  hu:{
    bts_ip:'BTS IP',offline:'OFFLINE',online:'ONLINE',
    brew_online:'ONLINE',brew_offline:'OFFLINE',
    stations:'Rádiók',calls:'Hívások',lastheard:'Utoljára Hallott',rf:'RF Monitor',log:'Napló',config:'Konfig',
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
};

let currentLang=localStorage.getItem('fs_lang')||'en';
function t(k,v){let s=(LANGS[currentLang]||LANGS.en)[k]||(LANGS.en[k]||k);if(v)Object.keys(v).forEach(x=>{s=s.replace('{'+x+'}',v[x]);});return s;}
function applyLang(){
  document.querySelectorAll('[data-i18n]').forEach(el=>el.textContent=t(el.getAttribute('data-i18n')));
  document.querySelectorAll('[data-i18n-tab]').forEach(el=>el.textContent=t(el.getAttribute('data-i18n-tab')));
  // Update nav labels
  ['stations','calls','lastheard','rf','log','config','system'].forEach(p=>{
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
const PAGE_TITLES={stations:'stations',calls:'calls',lastheard:'lastheard',rf:'rf',log:'log',config:'config',system:'system'};
function showPage(name,el){
  document.querySelectorAll('.page').forEach(p=>p.classList.remove('active'));
  document.querySelectorAll('.nav-item').forEach(n=>n.classList.remove('active'));
  document.getElementById('page-'+name).classList.add('active');
  if(el)el.classList.add('active');
  else{const nav=document.getElementById('nav-'+name);if(nav)nav.classList.add('active');}
  document.getElementById('topbar-title').textContent=t(name)||name;
  if(name==='config')loadConfig();
  if(name==='system'){loadSystemInfo();loadConfigProfiles();}
  if(window.innerWidth<=700)closeMobileSidebar();
}

// ── State + WS ────────────────────────────────────────────────────────────
let ws=null,state={ms:{},calls:{},lastHeard:[],brewOnline:false,brewVer:0},sdsDest=0;
const logFilter=()=>document.getElementById('log-filter').value;

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
    case 'tx_monitor':
      updateRfMonitor(msg);break;
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

// ── RF Monitor ───────────────────────────────────────────────────────────
const rfState={waterfall:[],lastTs:0,frames:0,rateTs:0};
function fmtMHz(v){return v?`${(v/1e6).toFixed(6)} MHz`:'—';}
function rfResizeCanvas(id){
  const c=document.getElementById(id);if(!c)return null;
  const r=c.getBoundingClientRect(),d=window.devicePixelRatio||1;
  const w=Math.max(1,Math.floor(r.width*d)),h=Math.max(1,Math.floor(r.height*d));
  if(c.width!==w||c.height!==h){c.width=w;c.height=h;}
  return c;
}
function updateRfMonitor(msg){
  rfState.lastTs=Date.now();rfState.frames++;
  if(!rfState.rateTs)rfState.rateTs=rfState.lastTs;
  const dt=(rfState.lastTs-rfState.rateTs)/1000;
  if(dt>=1){document.getElementById('rf-rate').textContent=`${(rfState.frames/dt).toFixed(1)} fps`;rfState.frames=0;rfState.rateTs=rfState.lastTs;}
  document.getElementById('rf-center').textContent=fmtMHz(msg.center_freq_hz);
  document.getElementById('rf-rms').textContent=`${msg.rms_dbfs.toFixed(1)} dBFS`;
  document.getElementById('rf-peak').textContent=`${msg.peak_dbfs.toFixed(1)} dBFS`;
  const spectrum=(msg.spectrum_db_tenths||[]).map(v=>v/10);
  drawRfSpectrum(spectrum,msg.sample_rate||600000);
  drawRfConstellation(msg.constellation_iq||[]);
  rfState.waterfall.push(spectrum);
  if(rfState.waterfall.length>180)rfState.waterfall.shift();
  drawRfWaterfall();
}
function drawRfGrid(ctx,w,h,yMin,yMax,fs){
  ctx.fillStyle='#050607';ctx.fillRect(0,0,w,h);
  ctx.strokeStyle='rgba(255,255,255,0.14)';ctx.lineWidth=1;
  ctx.fillStyle='rgba(255,255,255,0.75)';ctx.font='12px monospace';
  for(let i=0;i<=6;i++){
    const x=40+i*(w-50)/6;ctx.beginPath();ctx.moveTo(x,10);ctx.lineTo(x,h-24);ctx.stroke();
    const off=(-fs/2+i*fs/6)/1000;ctx.fillText(`${off>=0?'+':''}${off.toFixed(0)}k`,x-22,h-7);
  }
  for(let i=0;i<=5;i++){
    const y=10+i*(h-34)/5;ctx.beginPath();ctx.moveTo(40,y);ctx.lineTo(w-10,y);ctx.stroke();
    const db=yMax-i*(yMax-yMin)/5;ctx.fillText(db.toFixed(0),6,y+4);
  }
}
function drawRfSpectrum(spec,fs){
  const c=rfResizeCanvas('rf-spectrum');if(!c||!spec.length)return;
  const ctx=c.getContext('2d'),w=c.width,h=c.height,yMin=-120,yMax=0;
  drawRfGrid(ctx,w,h,yMin,yMax,fs);
  ctx.strokeStyle='#4dd8ff';ctx.lineWidth=2;ctx.beginPath();
  for(let i=0;i<spec.length;i++){
    const x=40+i*(w-50)/(spec.length-1);
    const y=10+(yMax-Math.max(yMin,Math.min(yMax,spec[i])))*(h-34)/(yMax-yMin);
    if(i===0)ctx.moveTo(x,y);else ctx.lineTo(x,y);
  }
  ctx.stroke();
}
function drawRfConstellation(iq){
  const c=rfResizeCanvas('rf-constellation');if(!c)return;
  const ctx=c.getContext('2d'),w=c.width,h=c.height,cx=w/2,cy=h/2,s=Math.min(w,h)*0.43;
  ctx.fillStyle='#050607';ctx.fillRect(0,0,w,h);
  ctx.strokeStyle='rgba(255,255,255,0.16)';ctx.beginPath();ctx.moveTo(cx,8);ctx.lineTo(cx,h-8);ctx.moveTo(8,cy);ctx.lineTo(w-8,cy);ctx.stroke();
  ctx.strokeStyle='rgba(0,212,168,0.25)';ctx.beginPath();ctx.arc(cx,cy,s,0,Math.PI*2);ctx.stroke();
  ctx.fillStyle='#24a2ff';
  for(let i=0;i+1<iq.length;i+=2){
    const x=cx+(iq[i]/32767)*s,y=cy-(iq[i+1]/32767)*s;
    ctx.fillRect(x-1,y-1,2,2);
  }
}
function wfColor(v){
  const t=Math.max(0,Math.min(1,(v+105)/65));
  const r=Math.floor(20+235*t),g=Math.floor(60+180*Math.sqrt(t)),b=Math.floor(210*(1-t));
  return `rgb(${r},${g},${b})`;
}
function drawRfWaterfall(){
  const c=rfResizeCanvas('rf-waterfall');if(!c)return;
  const ctx=c.getContext('2d'),w=c.width,h=c.height,rows=rfState.waterfall.length;
  ctx.fillStyle='#050607';ctx.fillRect(0,0,w,h);
  if(!rows)return;
  const rowH=Math.max(1,h/180),specLen=rfState.waterfall[0].length,colW=w/specLen;
  for(let r=0;r<rows;r++){
    const spec=rfState.waterfall[rows-1-r],y=h-(r+1)*rowH;
    for(let i=0;i<spec.length;i++){
      ctx.fillStyle=wfColor(spec[i]);
      ctx.fillRect(i*colW,y,Math.ceil(colW),Math.ceil(rowH));
    }
  }
}
setInterval(()=>{if(rfState.lastTs){document.getElementById('rf-age').textContent=`${((Date.now()-rfState.lastTs)/1000).toFixed(1)}s`;};},250);

// ── TS Visualizer ─────────────────────────────────────────────────────────
// ts_state[ts-1]: {call_id, call_type, label, sub, voice_ts} (voice_ts = Date.now() of last frame)
const tsState=[null,null,null,null]; // index 0=TS1..3=TS4
const TS_VOICE_DECAY_MS=800; // fade active indicator after 800ms without voice frame

function updateTsBlocks(){
  for(let i=0;i<4;i++){
    const ts=i+1;
    const block=document.getElementById('ts-block-'+ts);
    if(!block)continue;
    if(ts===1){
      // TS1 is always MCCH
      block.className='ts-block mcch';
      block.querySelector('.ts-label').textContent='MCCH';
      block.querySelector('.ts-sub').textContent='Control';
      continue;
    }
    const st=tsState[i];
    const label=block.querySelector('.ts-label');
    const sub=block.querySelector('.ts-sub');
    if(!st){
      block.className='ts-block';
      label.textContent='—';
      sub.textContent='Idle';
      continue;
    }
    // Check if voice activity is recent
    const voiceRecent=st.voice_ts&&(Date.now()-st.voice_ts)<TS_VOICE_DECAY_MS;
    block.className='ts-block'+(voiceRecent?' active':'');
    label.textContent=st.label||'—';
    sub.textContent=voiceRecent?'TX ▶':st.sub||'Alloc';
  }
}

function tsSetCall(ts, call_id, call_type, label, sub){
  if(ts<2||ts>4)return;
  tsState[ts-1]={call_id,call_type,label,sub,voice_ts:null};
}
function tsClearCall(call_id){
  for(let i=1;i<4;i++){if(tsState[i]&&tsState[i].call_id===call_id)tsState[i]=null;}
}
function tsVoice(ts){
  if(ts<2||ts>4)return;
  if(!tsState[ts-1])tsState[ts-1]={call_id:0,call_type:'',label:'Traffic',sub:'Alloc',voice_ts:null};
  tsState[ts-1].voice_ts=Date.now();
  updateTsBlocks();
}
setInterval(updateTsBlocks, 200); // refresh to catch voice decay

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
async function restartService(){if(!confirm(t('confirm_restart')))return;ws&&ws.send(JSON.stringify({type:'restart'}));}
async function shutdownService(){if(!confirm(t('confirm_shutdown')))return;ws&&ws.send(JSON.stringify({type:'shutdown'}));}
function kickMs(issi){if(!confirm(t('confirm_kick',{issi})))return;ws&&ws.send(JSON.stringify({type:'kick',issi}));}
function openSds(issi){sdsDest=issi;document.getElementById('sds-dest').value=issi;document.getElementById('sds-msg').value='';document.getElementById('sds-modal').classList.add('open');}
function closeSdsModal(){document.getElementById('sds-modal').classList.remove('open');}
function sendSds(){const dest=parseInt(document.getElementById('sds-dest').value),msg=document.getElementById('sds-msg').value.trim();if(!dest||!msg)return;ws&&ws.send(JSON.stringify({type:'sds',dest_issi:dest,message:msg}));closeSdsModal();}

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
async function loadSystemInfo(){
  try{
    const r=await fetch('/api/system');if(!r.ok)return;
    sysData=await r.json();
    document.getElementById('sysHostname').textContent=sysData.hostname||'—';
    document.getElementById('sysVersion').textContent=sysData.stack_version||'—';
    document.getElementById('sysOs').textContent=sysData.os||'—';
    document.getElementById('sysConfigPath').textContent=sysData.config_path||'—';
    updateSystemUptime();
  }catch{}
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
    if(r.ok){ws&&ws.send(JSON.stringify({type:'restart'}));}
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

// ── Tick ──────────────────────────────────────────────────────────────────
setInterval(()=>{
  if(document.getElementById('page-calls').classList.contains('active'))renderCalls();
  if(document.getElementById('page-stations').classList.contains('active'))renderStations();
  if(document.getElementById('page-lastheard').classList.contains('active'))renderLastHeard();
  if(document.getElementById('page-system').classList.contains('active'))updateSystemUptime();
},1000);

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
