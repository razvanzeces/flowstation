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

/* SDR hardware badge — shows the auto-detected SDR (LimeSDR, SXceiver, µCell, etc).
   Positioned between the page title and the right-side controls. Carries a subtle
   animated dot to convey "live link to RF". */
.sdr-badge{
  display:flex;align-items:center;gap:7px;
  padding:5px 10px;
  background:rgba(0,212,168,0.08);
  border:1px solid rgba(0,212,168,0.3);
  border-radius:6px;
  font-family:var(--mono);font-size:10px;font-weight:600;
  letter-spacing:0.05em;
  color:var(--accent);
  margin-left:14px;
  cursor:default;
  transition:background 0.15s;
}
.sdr-badge:hover{background:rgba(0,212,168,0.14);}
.sdr-badge-dot{
  width:6px;height:6px;border-radius:50%;
  background:var(--accent);
  box-shadow:0 0 6px var(--accent);
  animation:sdr-pulse 2s ease-in-out infinite;
}
@keyframes sdr-pulse{
  0%,100%{opacity:1;}
  50%{opacity:0.4;}
}
.sdr-badge-label{white-space:nowrap;}

/* Host power-draw badge: lives next to the SDR badge, uses a violet accent so
   it's visually distinct from the SDR (teal) badge. Hidden when sys_telemetry
   can't find any power-capable sensor on the host. */
.pwr-badge{
  display:flex;align-items:center;gap:6px;
  padding:5px 10px;
  background:rgba(167,114,232,0.10);
  border:1px solid rgba(167,114,232,0.35);
  border-radius:6px;
  font-family:var(--mono);font-size:10px;font-weight:600;
  letter-spacing:0.05em;
  color:#c8a4f5;
  margin-left:6px;
  cursor:default;
  transition:background 0.15s;
}
.pwr-badge:hover{background:rgba(167,114,232,0.18);}
.pwr-badge-icon{
  font-size:11px;line-height:1;
  filter:drop-shadow(0 0 4px rgba(167,114,232,0.5));
}
.pwr-badge-label{white-space:nowrap;}
[data-theme="light"] .pwr-badge{
  background:rgba(123,68,200,0.08);
  border-color:rgba(123,68,200,0.3);
  color:#6432aa;
}

/* Host hardware sensor tiles on the System tab. Compact, single-line per
   sensor, monospace numbers so columns of values line up visually. */
.sys-sensor-tile{
  background:var(--bg);border:1px solid var(--border);border-radius:6px;
  padding:8px 10px;
  display:flex;flex-direction:column;gap:3px;
  min-width:0;
}
.sys-sensor-label{
  font-family:var(--mono);font-size:9px;font-weight:600;
  letter-spacing:0.05em;text-transform:uppercase;color:var(--text3);
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
}
.sys-sensor-value{
  font-family:var(--mono);font-size:13px;font-weight:600;
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
}
.sys-sensor-unit{
  font-size:10px;font-weight:500;color:var(--text3);margin-left:2px;
}

/* ── WiFi tab ─────────────────────────────────────────────────────────────
   The WiFi tab shows three cards (status / saved profiles / scan results)
   and a modal for entering passwords. Visual language matches the rest of
   the dashboard: monospace labels, accent green for active items, hover
   row highlighting that doesn't move content. */

.wifi-status-grid{
  display:grid;grid-template-columns:repeat(auto-fit, minmax(170px, 1fr));
  gap:14px;
}
.wifi-status-loading{
  font-size:12px;color:var(--text3);font-style:italic;
}
.wifi-status-item{
  display:flex;flex-direction:column;gap:4px;
}
.wifi-status-label{
  font-family:var(--mono);font-size:9px;font-weight:600;
  letter-spacing:0.08em;text-transform:uppercase;color:var(--text3);
}
.wifi-status-value{
  font-size:14px;color:var(--text);font-weight:500;
  font-family:var(--mono);
}
.wifi-status-value.accent{color:var(--accent);font-weight:600;}
.wifi-status-value.muted{color:var(--text3);font-weight:400;}

.callout.wifi-warn{
  margin:10px 0 14px;padding:10px 14px;
  background:rgba(255,178,36,0.08);border:1px solid rgba(255,178,36,0.30);
  border-radius:6px;color:var(--text);font-size:12.5px;
}

/* Network list rows (used for both saved profiles and scan results). */
.wifi-list{display:flex;flex-direction:column;gap:4px;}
.wifi-list-empty{
  padding:18px;text-align:center;color:var(--text3);
  font-size:12.5px;font-style:italic;
}
.wifi-row{
  display:flex;align-items:center;gap:12px;
  padding:10px 14px;
  background:var(--bg);border:1px solid var(--border);border-radius:6px;
  transition:border-color 0.15s,background 0.15s;
}
.wifi-row:hover{border-color:var(--border2);background:var(--bg2);}
.wifi-row.active{
  border-color:var(--accent);
  background:rgba(0,212,168,0.06);
}
.wifi-row-signal{
  width:36px;flex-shrink:0;text-align:center;
}
.wifi-bars{
  display:inline-flex;align-items:flex-end;gap:2px;height:14px;
}
.wifi-bars span{
  display:block;width:3px;
  background:var(--text3);border-radius:1px;
  transition:background 0.15s;
}
.wifi-bars span.lit{background:var(--accent);}
.wifi-bars .b1{height:4px;}
.wifi-bars .b2{height:7px;}
.wifi-bars .b3{height:10px;}
.wifi-bars .b4{height:13px;}
.wifi-row-main{flex:1;min-width:0;}
.wifi-row-ssid{
  font-size:13.5px;font-weight:600;color:var(--text);
  display:flex;align-items:center;gap:8px;
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
}
.wifi-row-meta{
  font-family:var(--mono);font-size:10.5px;color:var(--text3);
  margin-top:2px;
  display:flex;gap:10px;
}
.wifi-row-meta .sec{color:var(--text3);}
.wifi-row-meta .sec.open{color:var(--warn);}
.wifi-tag{
  font-family:var(--mono);font-size:9px;font-weight:600;
  padding:2px 6px;border-radius:3px;
  letter-spacing:0.05em;text-transform:uppercase;
}
.wifi-tag.saved{
  background:rgba(77,166,255,0.12);color:var(--accent2);
  border:1px solid rgba(77,166,255,0.25);
}
.wifi-tag.active{
  background:rgba(0,212,168,0.15);color:var(--accent);
  border:1px solid rgba(0,212,168,0.35);
}
.wifi-row-actions{
  display:flex;gap:4px;flex-shrink:0;
}

/* Modal for password entry / hidden network. Overlay covers the page;
   the box is centered and styled like a card. */
.wifi-modal{
  position:fixed;inset:0;
  background:rgba(0,0,0,0.55);
  z-index:1000;
  display:flex;align-items:center;justify-content:center;
  padding:20px;
}
.wifi-modal-box{
  width:100%;max-width:420px;
  background:var(--bg2);border:1px solid var(--border);border-radius:10px;
  box-shadow:0 8px 32px rgba(0,0,0,0.6);
  overflow:hidden;
}
.wifi-modal-head{
  display:flex;align-items:center;justify-content:space-between;
  padding:14px 18px;border-bottom:1px solid var(--border);
}
.wifi-modal-title{
  font-size:14px;font-weight:600;color:var(--text);
}
.wifi-modal-x{
  background:none;border:none;color:var(--text3);
  font-size:20px;line-height:1;cursor:pointer;padding:0 4px;
}
.wifi-modal-x:hover{color:var(--text);}
.wifi-modal-body{padding:18px;}
.wifi-modal-row{margin-bottom:14px;}
.wifi-modal-row label{
  display:block;font-family:var(--mono);font-size:10px;font-weight:600;
  letter-spacing:0.08em;text-transform:uppercase;color:var(--text3);
  margin-bottom:6px;
}
.wifi-modal-row input[type="text"],
.wifi-modal-row input[type="password"]{
  width:100%;padding:8px 10px;
  background:var(--bg);border:1px solid var(--border);border-radius:5px;
  color:var(--text);font-family:var(--mono);font-size:13px;
}
.wifi-modal-row input:focus{
  outline:none;border-color:var(--accent);
}
.wifi-modal-check{
  display:flex;align-items:center;gap:8px;cursor:pointer;
  font-family:var(--sans);font-size:12px;font-weight:400;
  color:var(--text2);letter-spacing:normal;text-transform:none;
}
.wifi-modal-msg{
  font-size:12px;color:var(--danger);margin-top:8px;min-height:16px;
}
.wifi-modal-msg.ok{color:var(--accent);}
.wifi-modal-foot{
  display:flex;justify-content:flex-end;gap:8px;
  padding:12px 18px;border-top:1px solid var(--border);
}

/* Logout button: muted icon in topbar, becomes warning-red on hover. */
.logout-btn{
  width:30px;height:30px;
  display:flex;align-items:center;justify-content:center;
  background:transparent;border:1px solid var(--border);border-radius:6px;
  color:var(--text3);cursor:pointer;font-size:14px;
  transition:all 0.15s;
  margin-left:4px;
}
.logout-btn:hover{color:var(--danger);border-color:var(--danger);background:rgba(255,77,94,0.08);}

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

/* ── RF live monitor ─────────────────────────────────────────────────────── */
.rf-metrics{
  display:grid;
  grid-template-columns:repeat(5, 1fr);
  gap:10px;
  margin-bottom:12px;
}
.rf-metric{
  background:var(--bg2);border:1px solid var(--border);border-radius:var(--r);
  padding:10px 14px;
  display:flex;flex-direction:column;gap:4px;
  min-width:0;
}
.rf-metric-label{
  font-family:var(--mono);font-size:9px;font-weight:600;
  letter-spacing:0.08em;text-transform:uppercase;color:var(--text3);
}
.rf-metric-value{
  font-family:var(--mono);font-size:15px;font-weight:600;color:var(--text);
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
}
.rf-grid{
  display:grid;
  grid-template-columns:2fr 1fr;
  gap:12px;
}
.rf-panel{
  background:var(--bg2);border:1px solid var(--border);border-radius:var(--r);
  padding:14px;
  display:flex;flex-direction:column;gap:10px;
}
.rf-panel-title{
  display:flex;align-items:center;justify-content:space-between;
  font-family:var(--mono);font-size:10px;font-weight:700;
  letter-spacing:0.08em;text-transform:uppercase;color:var(--text2);
}
.rf-hint{font-weight:500;color:var(--text3);text-transform:none;letter-spacing:0;font-size:10px;}
.rf-canvas{
  width:100%;
  height:260px;
  background:var(--bg);border:1px solid var(--border);border-radius:6px;
  display:block;
}
.rf-canvas.small{height:260px;}

@media(max-width:900px){
  .rf-grid{grid-template-columns:1fr;}
  .rf-metrics{grid-template-columns:repeat(2, 1fr);}
}
@media(max-width:500px){
  .rf-metrics{grid-template-columns:1fr 1fr;gap:6px;}
  .rf-metric{padding:8px 10px;}
  .rf-metric-value{font-size:13px;}
  .rf-canvas{height:200px;}
  .rf-panel{padding:10px;}
}

/* ── RF signal-quality card ──────────────────────────────────────────── */
/* Each metric is a small tile: label, value, and a bar that fills horizontally
   with a colour reflecting health (green/amber/red). The bar replaces the need
   for a separate badge and gives an at-a-glance read of the whole panel. */
.rf-quality-card{
  background:var(--bg2);border:1px solid var(--border);border-radius:var(--r);
  padding:14px;margin-top:12px;
  display:flex;flex-direction:column;gap:14px;
}
.rf-quality-grid{
  display:grid;
  grid-template-columns:repeat(auto-fit, minmax(160px, 1fr));
  gap:10px;
}
.rf-qmetric{
  background:var(--bg);border:1px solid var(--border);border-radius:6px;
  padding:10px 12px;
  display:flex;flex-direction:column;gap:6px;
  min-width:0;
}
.rf-qmetric-label{
  font-family:var(--mono);font-size:9px;font-weight:600;
  letter-spacing:0.08em;text-transform:uppercase;color:var(--text3);
}
.rf-qmetric-value{
  font-family:var(--mono);font-size:14px;font-weight:600;color:var(--text);
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
}
.rf-qmetric-bar{
  height:4px;background:var(--bg3);border-radius:2px;overflow:hidden;
  margin-top:2px;
}
.rf-qmetric-fill{
  height:100%;width:0%;background:var(--accent);
  transition:width 0.4s ease, background 0.3s;
  border-radius:2px;
}
/* Status colouring is driven by JS via these classes */
.rf-q-good .rf-qmetric-fill{background:var(--accent);}
.rf-q-warn .rf-qmetric-fill{background:#f5a623;}
.rf-q-bad  .rf-qmetric-fill{background:var(--danger);}
.rf-q-good .rf-qmetric-value{color:var(--accent);}
.rf-q-warn .rf-qmetric-value{color:#f5a623;}
.rf-q-bad  .rf-qmetric-value{color:var(--danger);}

/* ── Hardware health card ────────────────────────────────────────────── */
.rf-hw-grid{
  display:grid;
  grid-template-columns:200px 1fr 1fr;
  gap:16px;
}
.rf-hw-temp{
  background:var(--bg);border:1px solid var(--border);border-radius:6px;
  padding:14px;
  display:flex;flex-direction:column;gap:6px;
}
.rf-hw-temp-value{
  font-family:var(--mono);font-size:28px;font-weight:700;color:var(--text);
  line-height:1;
}
.rf-hw-temp-state{
  font-family:var(--mono);font-size:10px;font-weight:600;
  letter-spacing:0.08em;text-transform:uppercase;
}
.rf-hw-temp-state.cold{color:var(--accent2);}
.rf-hw-temp-state.nominal{color:var(--accent);}
.rf-hw-temp-state.warm{color:#f5a623;}
.rf-hw-temp-state.hot{color:var(--danger);}
.rf-hw-gain-block{
  background:var(--bg);border:1px solid var(--border);border-radius:6px;
  padding:14px;
  display:flex;flex-direction:column;gap:6px;
  min-width:0;
}
.rf-hw-gain-list{
  display:flex;flex-direction:column;gap:4px;
  font-family:var(--mono);font-size:12px;
}
.rf-hw-gain-row{
  display:flex;justify-content:space-between;
  color:var(--text2);
}
.rf-hw-gain-row .stage{color:var(--text3);}
.rf-hw-gain-row .val{color:var(--text);font-weight:600;}

@media(max-width:900px){
  .rf-hw-grid{grid-template-columns:1fr;}
}

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

/* ── Phone portrait (~380px) — single column, larger touch targets ── */
@media(max-width:500px){
  /* Sidebar covers more of the viewport so the menu items are tappable */
  #sidebar{width:80vw!important;min-width:240px!important;max-width:280px;}

  /* Tighter topbar so the title + lang/theme don't overflow */
  #topbar{height:48px;padding:0 8px;gap:6px;}
  .topbar-title{font-size:13px;}
  .topbar-sub{display:none;}
  .topbar-sep{display:none;}
  .topbar-right{gap:4px;}
  .theme-btn{padding:3px 6px;font-size:9px;}
  .lang-btn{padding:2px 4px;font-size:9px;}
  /* SDR badge: keep the dot but shrink the text; if the badge would push out the
     right-side controls, hide the label entirely. */
  .sdr-badge{margin-left:6px;padding:4px 7px;font-size:9px;}
  .sdr-badge-label{max-width:60px;overflow:hidden;text-overflow:ellipsis;}
  .logout-btn{width:30px;height:30px;font-size:13px;}

  #content{padding:8px;}

  /* Cards in a single column so each one is readable */
  .stat-grid{grid-template-columns:1fr;gap:10px;}

  /* TS visualizer: 2x2 instead of 1x4 so each block stays usable */
  .ts-grid{grid-template-columns:1fr 1fr;gap:8px;padding:10px 12px;}

  /* System info: vertical layout per row, full-width values */
  .info-row{flex-direction:column;align-items:flex-start;gap:4px;padding:10px 14px;}
  .info-key{min-width:0!important;font-size:10px;}

  /* Tables: stacked-cards layout via data-label attributes on td (set in JS).
     For tables without labels, fall back to compact rows + horizontal scroll. */
  table{font-size:12px;}
  th,td{padding:8px 6px!important;}
  /* Hide less-important columns on phones to keep tables one-screen-wide */
  .col-mobile-hide{display:none;}

  /* Log: shorter on phone (more room for other UI) and break long lines */
  .log-wrap{height:300px!important;font-size:10px!important;padding:8px 10px!important;}
  .log-line{flex-wrap:wrap;}
  .log-ts{font-size:9px;}
  .log-level{width:38px;font-size:9px;}

  /* Modal dialogs: near full screen on phone, scrollable content */
  .modal{width:95vw!important;max-height:90vh!important;padding:14px!important;overflow-y:auto;}
  .modal-title{font-size:11px;margin-bottom:12px;padding-bottom:8px;}
  #update-modal .modal{width:95vw!important;}
  .update-terminal{height:200px!important;font-size:10px!important;}

  /* Make buttons easier to tap */
  button,.btn{min-height:36px;}

  /* Forms: stack inputs full-width */
  input[type="text"],input[type="number"],textarea,select{font-size:16px;} /* 16px prevents iOS zoom on focus */
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
    <div class="nav-item" onclick="showPage('rf',this)" id="nav-rf">
      <span class="nav-icon">⚡</span>
      <span class="nav-label" data-i18n="rf">RF</span>
    </div>

    <div class="nav-section-label" data-i18n-section="manage">MANAGE</div>
    <div class="nav-item" onclick="showPage('config',this)" id="nav-config">
      <span class="nav-icon">⚙</span>
      <span class="nav-label" data-i18n="config">CONFIG</span>
    </div>
    <!-- WiFi tab is hidden until we confirm NetworkManager is available on
         the host. The probe runs once at dashboard boot via /api/wifi/available
         and toggles this element's display. -->
    <div class="nav-item" onclick="showPage('wifi',this)" id="nav-wifi" style="display:none">
      <span class="nav-icon">📶</span>
      <span class="nav-label" data-i18n="wifi">WIFI</span>
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

    <!-- SDR hardware badge — auto-detected at stack startup. Hidden until populated. -->
    <div id="sdr-badge" class="sdr-badge" style="display:none" title="Detected SDR hardware">
      <span class="sdr-badge-dot"></span>
      <span class="sdr-badge-label" id="sdr-badge-label">—</span>
    </div>

    <!-- Host power-draw badge — populated from /sys via sys_telemetry. Stays hidden
         when no power-capable sensor is found (e.g. non-Pi, non-x86 hosts). -->
    <div id="pwr-badge" class="pwr-badge" style="display:none" title="Host system power draw">
      <span class="pwr-badge-icon">⚡</span>
      <span class="pwr-badge-label" id="pwr-badge-label">—</span>
    </div>

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
        <button class="lang-btn" onclick="setLang('zh',this)">CN</button>
      </div>
      <!-- Logout: clears session cookie and redirects to /login. Hidden when auth is off. -->
      <button class="logout-btn" id="logout-btn" onclick="doLogout()" title="Log out" style="display:none">⏻</button>
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
                <th class="col-mobile-hide" data-i18n="th_ee">EE</th>
                <th data-i18n="th_signal">Signal</th>
                <th data-i18n="th_status">Status</th>
                <th class="col-mobile-hide" data-i18n="th_last_seen">Last seen</th>
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
                <th class="col-mobile-hide" data-i18n="th_id">ID</th>
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

    <!-- ── RF ── -->
    <!-- Live TX DSP monitor — works on any SDR because the analysis is done on the
         complex baseband samples FlowStation generates internally, BEFORE they reach
         the radio. We do not rely on receive-side feedback. -->
    <div class="page" id="page-rf">

      <!-- Top stat strip: instantaneous big-number metrics -->
      <div class="rf-metrics">
        <div class="rf-metric">
          <div class="rf-metric-label" data-i18n="rf_freq">Center freq</div>
          <div class="rf-metric-value" id="rf-freq">—</div>
        </div>
        <div class="rf-metric">
          <div class="rf-metric-label" data-i18n="rf_rate">Sample rate</div>
          <div class="rf-metric-value" id="rf-rate">—</div>
        </div>
        <div class="rf-metric">
          <div class="rf-metric-label" data-i18n="rf_rms">RMS</div>
          <div class="rf-metric-value" id="rf-rms">—</div>
        </div>
        <div class="rf-metric">
          <div class="rf-metric-label" data-i18n="rf_peak">Peak</div>
          <div class="rf-metric-value" id="rf-peak">—</div>
        </div>
        <div class="rf-metric">
          <div class="rf-metric-label" data-i18n="rf_age">Snapshot</div>
          <div class="rf-metric-value" id="rf-age" data-i18n="rf_waiting">waiting…</div>
        </div>
      </div>

      <!-- Visualizers grid: spectrum + constellation -->
      <div class="rf-grid">
        <div class="rf-panel">
          <div class="rf-panel-title">
            <span data-i18n="rf_spectrum">TX DSP Spectrum (pre-PA)</span>
            <span class="rf-hint" data-i18n="rf_hint_spectrum">live · 512-bin FFT</span>
          </div>
          <canvas id="rf-spectrum" class="rf-canvas" width="900" height="260"></canvas>
        </div>
        <div class="rf-panel">
          <div class="rf-panel-title">
            <span data-i18n="rf_constellation">TX DSP Constellation</span>
            <span class="rf-hint" data-i18n="rf_hint_constellation">π/4-DQPSK</span>
          </div>
          <canvas id="rf-constellation" class="rf-canvas small" width="420" height="260"></canvas>
        </div>
      </div>

      <!-- Waterfall: time-vs-frequency heatmap, scrolls downward -->
      <div class="rf-panel" style="margin-top:12px">
        <div class="rf-panel-title">
          <span data-i18n="rf_waterfall">TX Spectrum Waterfall</span>
          <span class="rf-hint" data-i18n="rf_hint_waterfall">rolling · viridis</span>
        </div>
        <canvas id="rf-waterfall" class="rf-canvas" style="height:320px"></canvas>
      </div>

      <!-- Signal Quality strip — derived metrics with health badges (good/warn/bad) -->
      <div class="rf-quality-card">
        <div class="rf-panel-title">
          <span data-i18n="rf_quality">Signal Quality</span>
          <span class="rf-hint" data-i18n="rf_hint_quality">measured pre-PA · derived from same DSP snapshot</span>
        </div>
        <div class="rf-quality-grid">
          <div class="rf-qmetric" id="rf-q-evm-wrap">
            <div class="rf-qmetric-label" data-i18n="rf_evm">EVM</div>
            <div class="rf-qmetric-value" id="rf-evm">—</div>
            <div class="rf-qmetric-bar"><div class="rf-qmetric-fill" id="rf-evm-bar"></div></div>
          </div>
          <div class="rf-qmetric" id="rf-q-papr-wrap">
            <div class="rf-qmetric-label" data-i18n="rf_papr">PAPR</div>
            <div class="rf-qmetric-value" id="rf-papr">—</div>
            <div class="rf-qmetric-bar"><div class="rf-qmetric-fill" id="rf-papr-bar"></div></div>
          </div>
          <div class="rf-qmetric" id="rf-q-cl-wrap">
            <div class="rf-qmetric-label" data-i18n="rf_carrier">Carrier leak</div>
            <div class="rf-qmetric-value" id="rf-carrier">—</div>
            <div class="rf-qmetric-bar"><div class="rf-qmetric-fill" id="rf-carrier-bar"></div></div>
          </div>
          <div class="rf-qmetric" id="rf-q-obw-wrap">
            <div class="rf-qmetric-label" data-i18n="rf_obw">Occupied BW (99%)</div>
            <div class="rf-qmetric-value" id="rf-obw">—</div>
            <div class="rf-qmetric-bar"><div class="rf-qmetric-fill" id="rf-obw-bar"></div></div>
          </div>
          <div class="rf-qmetric" id="rf-q-dc-wrap">
            <div class="rf-qmetric-label" data-i18n="rf_dc">DC offset (I/Q)</div>
            <div class="rf-qmetric-value" id="rf-dc">—</div>
            <div class="rf-qmetric-bar"><div class="rf-qmetric-fill" id="rf-dc-bar"></div></div>
          </div>
          <div class="rf-qmetric" id="rf-q-iqa-wrap">
            <div class="rf-qmetric-label" data-i18n="rf_iqa">IQ amplitude imbalance</div>
            <div class="rf-qmetric-value" id="rf-iqa">—</div>
            <div class="rf-qmetric-bar"><div class="rf-qmetric-fill" id="rf-iqa-bar"></div></div>
          </div>
          <div class="rf-qmetric" id="rf-q-iqp-wrap">
            <div class="rf-qmetric-label" data-i18n="rf_iqp">IQ phase imbalance</div>
            <div class="rf-qmetric-value" id="rf-iqp">—</div>
            <div class="rf-qmetric-bar"><div class="rf-qmetric-fill" id="rf-iqp-bar"></div></div>
          </div>
        </div>
      </div>

      <!-- Hardware Health — temperature + actual gain readback from the SDR. Updated every ~5s. -->
      <div class="rf-quality-card">
        <div class="rf-panel-title">
          <span data-i18n="rf_hw_health">Hardware Health</span>
          <span class="rf-hint"><span data-i18n="rf_hint_health">polled every 5s</span> · <span id="rf-hw-age">—</span></span>
        </div>
        <div class="rf-hw-grid">
          <div class="rf-hw-temp">
            <div class="rf-qmetric-label" data-i18n="rf_temp">SDR Temperature</div>
            <div class="rf-hw-temp-value" id="rf-temp">—</div>
            <div class="rf-hw-temp-state" id="rf-temp-state">—</div>
          </div>
          <div class="rf-hw-gain-block">
            <div class="rf-qmetric-label" data-i18n="rf_tx_gain">TX Gain Stages (actual)</div>
            <div class="rf-hw-gain-list" id="rf-tx-gains">—</div>
          </div>
          <div class="rf-hw-gain-block">
            <div class="rf-qmetric-label" data-i18n="rf_rx_gain">RX Gain Stages (actual)</div>
            <div class="rf-hw-gain-list" id="rf-rx-gains">—</div>
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

    <!-- ── WIFI ──
         Three cards: current status (with disconnect / radio toggle), saved
         profiles list, and visible networks scan. The whole tab is only
         attached to a nav button when /api/wifi/available reports true so
         we never tease functionality the host can't deliver. -->
    <div class="page" id="page-wifi">
      <!-- Status card: who we're connected to right now, IP, signal -->
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="wifi_status">Current connection</div>
          <div class="card-actions">
            <button class="btn btn-sm" id="wifi-radio-btn" onclick="wifiToggleRadio()" data-i18n="wifi_radio_off">Disable WiFi</button>
            <button class="btn btn-sm" onclick="wifiRefresh()" data-i18n="wifi_refresh">↻ Refresh</button>
          </div>
        </div>
        <div class="card-body">
          <div class="wifi-status-grid" id="wifi-status-grid">
            <div class="wifi-status-loading" data-i18n="wifi_loading">Loading…</div>
          </div>
        </div>
      </div>

      <!-- Connection safety warning: changing WiFi while connected through
           it can lock the operator out of the dashboard. We show this
           prominently above the actionable cards. -->
      <div class="callout wifi-warn" data-i18n="wifi_warn_lose_access">
        ⚠ If you're connected to the dashboard via WiFi, changing networks may temporarily disconnect you. Make sure you have a backup access path (Ethernet or known good network).
      </div>

      <!-- Saved profiles: networks NM already has credentials for. Each row
           has Connect (bring up) and Forget (delete) buttons. -->
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="wifi_saved">Saved networks</div>
          <div class="card-actions">
            <span id="wifi-saved-count" class="card-sub"></span>
          </div>
        </div>
        <div class="card-body">
          <div id="wifi-saved-list" class="wifi-list">
            <div class="wifi-list-empty" data-i18n="wifi_loading">Loading…</div>
          </div>
        </div>
      </div>

      <!-- Visible networks: live nmcli scan with --rescan yes. The bottom
           "Add hidden network" button opens the manual SSID input modal. -->
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="wifi_visible">Available networks</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="wifiShowHiddenModal()" data-i18n="wifi_add_hidden">+ Hidden network</button>
            <button class="btn btn-sm" onclick="wifiScan()" data-i18n="wifi_scan">↻ Scan</button>
          </div>
        </div>
        <div class="card-body">
          <div id="wifi-scan-list" class="wifi-list">
            <div class="wifi-list-empty" data-i18n="wifi_loading">Loading…</div>
          </div>
        </div>
      </div>
    </div>

    <!-- WiFi password modal — used both when joining a visible network with
         security and when adding a hidden network manually. -->
    <div id="wifi-modal" class="wifi-modal" style="display:none">
      <div class="wifi-modal-box">
        <div class="wifi-modal-head">
          <div class="wifi-modal-title" id="wifi-modal-title">Connect</div>
          <button class="wifi-modal-x" onclick="wifiCloseModal()">×</button>
        </div>
        <div class="wifi-modal-body">
          <div class="wifi-modal-row" id="wifi-modal-ssid-row">
            <label for="wifi-modal-ssid" data-i18n="wifi_ssid">SSID</label>
            <input id="wifi-modal-ssid" type="text" autocomplete="off" spellcheck="false">
          </div>
          <div class="wifi-modal-row" id="wifi-modal-psk-row">
            <label for="wifi-modal-psk" data-i18n="wifi_password">Password</label>
            <input id="wifi-modal-psk" type="password" autocomplete="new-password" spellcheck="false">
          </div>
          <div class="wifi-modal-row" id="wifi-modal-hidden-row" style="display:none">
            <label class="wifi-modal-check">
              <input id="wifi-modal-hidden" type="checkbox"> <span data-i18n="wifi_hidden">Hidden network (SSID not broadcast)</span>
            </label>
          </div>
          <div class="wifi-modal-msg" id="wifi-modal-msg"></div>
        </div>
        <div class="wifi-modal-foot">
          <button class="btn" onclick="wifiCloseModal()" data-i18n="cancel">Cancel</button>
          <button class="btn btn-primary" id="wifi-modal-ok" onclick="wifiModalSubmit()" data-i18n="wifi_connect">Connect</button>
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

      <!-- Host hardware sensors (temps, voltages, currents, power) -->
      <!-- Populated from /sys via sys_telemetry. Layout adapts: if no sensors are
           found (non-Linux, locked-down kernel) the whole card is hidden. -->
      <div class="card" id="sys-sensors-card" style="display:none">
        <div class="card-head">
          <div class="card-title" data-i18n="sys_sensors">Host Hardware Sensors</div>
          <div class="card-actions">
            <span id="sys-sensors-power-total" style="font-family:var(--mono);font-size:12px;color:#c8a4f5;font-weight:600"></span>
          </div>
        </div>
        <div class="card-body" style="padding:14px 18px">
          <div id="sys-sensors-empty" style="font-size:12px;color:var(--text3);font-style:italic;display:none" data-i18n="sys_sensors_empty">No sensors detected on this host.</div>
          <div id="sys-sensors-grid" style="display:grid;grid-template-columns:repeat(auto-fill, minmax(160px, 1fr));gap:8px"></div>
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
    stations:'Radios',calls:'Calls',lastheard:'Last Heard',log:'Log',rf:'RF',config:'Config',
    rf_freq:'Center freq',rf_rate:'Sample rate',rf_rms:'RMS',rf_peak:'Peak',rf_age:'Snapshot',
    rf_waiting:'waiting…',rf_live:'live',rf_stale:'stale',
    rf_spectrum:'TX DSP Spectrum (pre-PA)',rf_constellation:'TX DSP Constellation',
    rf_hint_spectrum:'live · 512-bin FFT',rf_hint_constellation:'π/4-DQPSK',
    rf_waterfall:'TX Spectrum Waterfall',rf_hint_waterfall:'rolling · viridis',
    rf_quality:'Signal Quality',rf_hint_quality:'measured pre-PA · derived from same DSP snapshot',
    rf_evm:'EVM',rf_papr:'PAPR',rf_carrier:'Carrier leak',rf_obw:'Occupied BW (99%)',
    rf_dc:'DC offset (I/Q)',rf_iqa:'IQ amplitude imbalance',rf_iqp:'IQ phase imbalance',
    rf_hw_health:'Hardware Health',rf_hint_health:'polled every 5s',
    rf_temp:'SDR Temperature',rf_tx_gain:'TX Gain Stages (actual)',rf_rx_gain:'RX Gain Stages (actual)',
    rf_temp_cold:'cold',rf_temp_nominal:'nominal',rf_temp_warm:'warm',rf_temp_hot:'hot',rf_temp_na:'no sensor',
    rf_no_gains:'unavailable',rf_just_now:'just now',

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
    confirm_logout:'Log out?',
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
    wifi:'WiFi',wifi_status:'Current connection',wifi_saved:'Saved networks',wifi_visible:'Available networks',wifi_loading:'Loading…',wifi_scanning:'Scanning…',wifi_no_device:'No WiFi device detected on this host.',wifi_radio_disabled:'WiFi radio is disabled.',wifi_not_connected:'Not connected to any network.',wifi_no_saved:'No saved networks.',wifi_no_networks:'No networks in range.',wifi_ssid:'Network',wifi_signal:'Signal',wifi_ip:'IP address',wifi_actions:'Actions',wifi_disconnect:'Disconnect',wifi_connect:'Connect',wifi_connect_to:'Connect to',wifi_connecting:'Connecting…',wifi_connected:'CONNECTED',wifi_connected_ok:'Connected.',wifi_saved_tag:'SAVED',wifi_open:'OPEN',wifi_forget:'Forget',wifi_confirm_forget:'Forget network',wifi_password:'Password',wifi_hidden:'Hidden network (SSID not broadcast)',wifi_add_hidden:'+ Hidden network',wifi_scan:'↻ Scan',wifi_refresh:'↻ Refresh',wifi_radio_off:'Disable WiFi',wifi_radio_on:'Enable WiFi',wifi_warn_lose_access:'⚠ If connected to the dashboard via WiFi, changing networks may temporarily disconnect you. Make sure you have a backup access path (Ethernet or known good network).',wifi_err_no_ssid:'SSID required',cancel:'Cancel',sys_sensors:'Host Hardware Sensors',sys_sensors_empty:'No sensors detected on this host.',sys_rf:'RF Hardware (SoapySDR)',sys_autorefresh:'Auto-refresh 5s',
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
    stations:'Radiouri',calls:'Apeluri',lastheard:'Ultima Activitate',log:'Log',rf:'RF',config:'Config',
    rf_freq:'Frecvență centru',rf_rate:'Rată eșantion',rf_rms:'RMS',rf_peak:'Vârf',rf_age:'Captură',
    rf_waiting:'în așteptare…',rf_live:'live',rf_stale:'expirat',
    rf_spectrum:'Spectru TX DSP (pre-PA)',rf_constellation:'Constelație TX DSP',
    rf_hint_spectrum:'live · FFT 512-bin',rf_hint_constellation:'π/4-DQPSK',
    rf_waterfall:'Cascadă Spectru TX',rf_hint_waterfall:'derulant · viridis',
    rf_quality:'Calitate Semnal',rf_hint_quality:'măsurat pre-PA · din același snapshot DSP',
    rf_evm:'EVM',rf_papr:'PAPR',rf_carrier:'Scurgere portantă',rf_obw:'Bandă ocupată (99%)',
    rf_dc:'Offset DC (I/Q)',rf_iqa:'Dezechilibru amplitudine IQ',rf_iqp:'Dezechilibru fază IQ',
    rf_hw_health:'Stare Hardware',rf_hint_health:'citit la 5s',
    rf_temp:'Temperatură SDR',rf_tx_gain:'Câștig TX (actual)',rf_rx_gain:'Câștig RX (actual)',
    rf_temp_cold:'rece',rf_temp_nominal:'nominal',rf_temp_warm:'cald',rf_temp_hot:'fierbinte',rf_temp_na:'fără senzor',
    rf_no_gains:'indisponibil',rf_just_now:'acum',

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
    confirm_logout:'Deconectare?',
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
    wifi:'WiFi',wifi_status:'Conexiunea curentă',wifi_saved:'Rețele salvate',wifi_visible:'Rețele disponibile',wifi_loading:'Se încarcă…',wifi_scanning:'Se scanează…',wifi_no_device:'Niciun dispozitiv WiFi detectat.',wifi_radio_disabled:'Radioul WiFi este dezactivat.',wifi_not_connected:'Neconectat la nicio rețea.',wifi_no_saved:'Nicio rețea salvată.',wifi_no_networks:'Nicio rețea în rază.',wifi_ssid:'Rețea',wifi_signal:'Semnal',wifi_ip:'Adresă IP',wifi_actions:'Acțiuni',wifi_disconnect:'Deconectează',wifi_connect:'Conectează',wifi_connect_to:'Conectează la',wifi_connecting:'Se conectează…',wifi_connected:'CONECTAT',wifi_connected_ok:'Conectat.',wifi_saved_tag:'SALVAT',wifi_open:'DESCHIS',wifi_forget:'Uită',wifi_confirm_forget:'Uită rețeaua',wifi_password:'Parolă',wifi_hidden:'Rețea ascunsă (SSID nedifuzat)',wifi_add_hidden:'+ Rețea ascunsă',wifi_scan:'↻ Scanează',wifi_refresh:'↻ Reîncarcă',wifi_radio_off:'Dezactivează WiFi',wifi_radio_on:'Activează WiFi',wifi_warn_lose_access:'⚠ Dacă ești conectat la dashboard prin WiFi, schimbarea rețelei te poate deconecta temporar. Asigură-te că ai o cale alternativă (Ethernet sau rețea de încredere).',wifi_err_no_ssid:'SSID necesar',cancel:'Anulează',sys_sensors:'Senzori Hardware Gazdă',sys_sensors_empty:'Niciun senzor detectat.',sys_rf:'Hardware RF (SoapySDR)',sys_autorefresh:'Auto-refresh 5s',
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
    stations:'Radios',calls:'Anrufe',lastheard:'Zuletzt Gehört',log:'Log',rf:'RF',config:'Config',
    rf_freq:'Mittenfrequenz',rf_rate:'Abtastrate',rf_rms:'RMS',rf_peak:'Spitze',rf_age:'Aufnahme',
    rf_waiting:'wartet…',rf_live:'live',rf_stale:'veraltet',
    rf_spectrum:'TX-DSP-Spektrum (vor PA)',rf_constellation:'TX-DSP-Konstellation',
    rf_hint_spectrum:'live · 512-bin FFT',rf_hint_constellation:'π/4-DQPSK',
    rf_waterfall:'TX-Spektrum-Wasserfall',rf_hint_waterfall:'rollend · viridis',
    rf_quality:'Signalqualität',rf_hint_quality:'gemessen vor PA · aus selbem DSP-Snapshot',
    rf_evm:'EVM',rf_papr:'PAPR',rf_carrier:'Trägerleckage',rf_obw:'Belegte BW (99%)',
    rf_dc:'DC-Offset (I/Q)',rf_iqa:'IQ-Amplitudenungleichgewicht',rf_iqp:'IQ-Phasenungleichgewicht',
    rf_hw_health:'Hardware-Zustand',rf_hint_health:'alle 5s abgefragt',
    rf_temp:'SDR-Temperatur',rf_tx_gain:'TX-Verstärkung (aktuell)',rf_rx_gain:'RX-Verstärkung (aktuell)',
    rf_temp_cold:'kalt',rf_temp_nominal:'nominal',rf_temp_warm:'warm',rf_temp_hot:'heiß',rf_temp_na:'kein Sensor',
    rf_no_gains:'nicht verfügbar',rf_just_now:'gerade eben',

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
    confirm_logout:'Abmelden?',
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
    wifi:'WLAN',wifi_status:'Aktuelle Verbindung',wifi_saved:'Gespeicherte Netzwerke',wifi_visible:'Verfügbare Netzwerke',wifi_loading:'Wird geladen…',wifi_scanning:'Suche läuft…',wifi_no_device:'Kein WLAN-Gerät erkannt.',wifi_radio_disabled:'WLAN-Funk ist deaktiviert.',wifi_not_connected:'Mit keinem Netzwerk verbunden.',wifi_no_saved:'Keine gespeicherten Netzwerke.',wifi_no_networks:'Keine Netzwerke in Reichweite.',wifi_ssid:'Netzwerk',wifi_signal:'Signal',wifi_ip:'IP-Adresse',wifi_actions:'Aktionen',wifi_disconnect:'Trennen',wifi_connect:'Verbinden',wifi_connect_to:'Verbinden mit',wifi_connecting:'Verbinde…',wifi_connected:'VERBUNDEN',wifi_connected_ok:'Verbunden.',wifi_saved_tag:'GESPEICHERT',wifi_open:'OFFEN',wifi_forget:'Vergessen',wifi_confirm_forget:'Netzwerk vergessen',wifi_password:'Passwort',wifi_hidden:'Verstecktes Netzwerk (SSID nicht gesendet)',wifi_add_hidden:'+ Verstecktes Netzwerk',wifi_scan:'↻ Suchen',wifi_refresh:'↻ Aktualisieren',wifi_radio_off:'WLAN deaktivieren',wifi_radio_on:'WLAN aktivieren',wifi_warn_lose_access:'⚠ Wenn Sie über WLAN mit dem Dashboard verbunden sind, kann ein Netzwerkwechsel die Verbindung trennen. Stellen Sie sicher, dass Sie einen alternativen Zugang haben.',wifi_err_no_ssid:'SSID erforderlich',cancel:'Abbrechen',sys_sensors:'Host-Hardware-Sensoren',sys_sensors_empty:'Keine Sensoren erkannt.',sys_rf:'RF-Hardware (SoapySDR)',sys_autorefresh:'Auto-Aktualisierung 5s',
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
    stations:'Radios',calls:'Llamadas',lastheard:'Última Actividad',log:'Log',rf:'RF',config:'Config',
    rf_freq:'Frecuencia central',rf_rate:'Tasa de muestreo',rf_rms:'RMS',rf_peak:'Pico',rf_age:'Captura',
    rf_waiting:'esperando…',rf_live:'en vivo',rf_stale:'obsoleto',
    rf_spectrum:'Espectro TX DSP (pre-PA)',rf_constellation:'Constelación TX DSP',
    rf_hint_spectrum:'en vivo · FFT 512-bin',rf_hint_constellation:'π/4-DQPSK',
    rf_waterfall:'Cascada Espectro TX',rf_hint_waterfall:'desplazándose · viridis',
    rf_quality:'Calidad de Señal',rf_hint_quality:'medido pre-PA · del mismo snapshot DSP',
    rf_evm:'EVM',rf_papr:'PAPR',rf_carrier:'Fuga portadora',rf_obw:'BW ocupada (99%)',
    rf_dc:'Offset DC (I/Q)',rf_iqa:'Desequilibrio amplitud IQ',rf_iqp:'Desequilibrio fase IQ',
    rf_hw_health:'Estado Hardware',rf_hint_health:'consultado cada 5s',
    rf_temp:'Temperatura SDR',rf_tx_gain:'Ganancia TX (real)',rf_rx_gain:'Ganancia RX (real)',
    rf_temp_cold:'frío',rf_temp_nominal:'nominal',rf_temp_warm:'caliente',rf_temp_hot:'muy caliente',rf_temp_na:'sin sensor',
    rf_no_gains:'no disponible',rf_just_now:'ahora',

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
    confirm_logout:'¿Cerrar sesión?',
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
    wifi:'WiFi',wifi_status:'Conexión actual',wifi_saved:'Redes guardadas',wifi_visible:'Redes disponibles',wifi_loading:'Cargando…',wifi_scanning:'Escaneando…',wifi_no_device:'No se detectó dispositivo WiFi.',wifi_radio_disabled:'Radio WiFi desactivada.',wifi_not_connected:'No conectado a ninguna red.',wifi_no_saved:'Sin redes guardadas.',wifi_no_networks:'Sin redes en rango.',wifi_ssid:'Red',wifi_signal:'Señal',wifi_ip:'Dirección IP',wifi_actions:'Acciones',wifi_disconnect:'Desconectar',wifi_connect:'Conectar',wifi_connect_to:'Conectar a',wifi_connecting:'Conectando…',wifi_connected:'CONECTADO',wifi_connected_ok:'Conectado.',wifi_saved_tag:'GUARDADO',wifi_open:'ABIERTO',wifi_forget:'Olvidar',wifi_confirm_forget:'Olvidar red',wifi_password:'Contraseña',wifi_hidden:'Red oculta (SSID no difundido)',wifi_add_hidden:'+ Red oculta',wifi_scan:'↻ Escanear',wifi_refresh:'↻ Actualizar',wifi_radio_off:'Desactivar WiFi',wifi_radio_on:'Activar WiFi',wifi_warn_lose_access:'⚠ Si estás conectado al dashboard vía WiFi, cambiar de red puede desconectarte temporalmente. Asegúrate de tener una vía de acceso alternativa.',wifi_err_no_ssid:'SSID requerido',cancel:'Cancelar',sys_sensors:'Sensores del Sistema',sys_sensors_empty:'No se detectaron sensores.',sys_rf:'Hardware RF (SoapySDR)',sys_autorefresh:'Auto-actualización 5s',
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
    stations:'Rádiók',calls:'Hívások',lastheard:'Utoljára Hallott',log:'Napló',rf:'RF',config:'Konfig',
    rf_freq:'Központi frekvencia',rf_rate:'Mintavételezési ráta',rf_rms:'RMS',rf_peak:'Csúcs',rf_age:'Pillanatkép',
    rf_waiting:'várakozás…',rf_live:'élő',rf_stale:'elavult',
    rf_spectrum:'TX DSP spektrum (PA előtt)',rf_constellation:'TX DSP konstelláció',
    rf_hint_spectrum:'élő · 512-bin FFT',rf_hint_constellation:'π/4-DQPSK',
    rf_waterfall:'TX Spektrum Vízesés',rf_hint_waterfall:'gördülő · viridis',
    rf_quality:'Jelminőség',rf_hint_quality:'PA előtt mérve · ugyanazon DSP pillanatképből',
    rf_evm:'EVM',rf_papr:'PAPR',rf_carrier:'Vivőszivárgás',rf_obw:'Foglalt sávszélesség (99%)',
    rf_dc:'DC eltolás (I/Q)',rf_iqa:'IQ amplitúdó egyensúlytalanság',rf_iqp:'IQ fázis egyensúlytalanság',
    rf_hw_health:'Hardver állapot',rf_hint_health:'5 másodpercenként',
    rf_temp:'SDR hőmérséklet',rf_tx_gain:'TX erősítés (aktuális)',rf_rx_gain:'RX erősítés (aktuális)',
    rf_temp_cold:'hideg',rf_temp_nominal:'normál',rf_temp_warm:'meleg',rf_temp_hot:'forró',rf_temp_na:'nincs szenzor',
    rf_no_gains:'nem elérhető',rf_just_now:'most',

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
    confirm_logout:'Kijelentkezik?',
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
    wifi:'WiFi',wifi_status:'Jelenlegi kapcsolat',wifi_saved:'Mentett hálózatok',wifi_visible:'Elérhető hálózatok',wifi_loading:'Betöltés…',wifi_scanning:'Keresés…',wifi_no_device:'Nem észlelhető WiFi eszköz.',wifi_radio_disabled:'WiFi rádió letiltva.',wifi_not_connected:'Nincs kapcsolat hálózathoz.',wifi_no_saved:'Nincs mentett hálózat.',wifi_no_networks:'Nincs hálózat hatótávolságon belül.',wifi_ssid:'Hálózat',wifi_signal:'Jelerősség',wifi_ip:'IP-cím',wifi_actions:'Műveletek',wifi_disconnect:'Bontás',wifi_connect:'Csatlakozás',wifi_connect_to:'Csatlakozás:',wifi_connecting:'Csatlakozás…',wifi_connected:'KAPCSOLÓDVA',wifi_connected_ok:'Csatlakoztatva.',wifi_saved_tag:'MENTETT',wifi_open:'NYITOTT',wifi_forget:'Elfelejtés',wifi_confirm_forget:'Hálózat elfelejtése',wifi_password:'Jelszó',wifi_hidden:'Rejtett hálózat (SSID nem sugárzott)',wifi_add_hidden:'+ Rejtett hálózat',wifi_scan:'↻ Keresés',wifi_refresh:'↻ Frissítés',wifi_radio_off:'WiFi letiltása',wifi_radio_on:'WiFi engedélyezése',wifi_warn_lose_access:'⚠ Ha WiFi-n keresztül csatlakozol a vezérlőpulthoz, a hálózat módosítása lecsatlakoztathat. Biztosíts alternatív hozzáférést.',wifi_err_no_ssid:'SSID szükséges',cancel:'Mégse',sys_sensors:'Gazdagép szenzorok',sys_sensors_empty:'Nem észlelhetők szenzorok.',
  },
  zh:{
    bts_ip:'BTS IP',offline:'离线',online:'在线',
    brew_online:'在线',brew_offline:'离线',
    stations:'终端',calls:'通话',lastheard:'最近通话',log:'日志',rf:'RF',config:'配置',
    rf_freq:'中心频率',rf_rate:'采样率',rf_rms:'RMS',rf_peak:'峰值',rf_age:'快照',
    rf_waiting:'等待中…',rf_live:'实时',rf_stale:'已过期',
    rf_spectrum:'TX DSP 频谱（功放前）',rf_constellation:'TX DSP 星座图',
    rf_hint_spectrum:'实时 · 512 点 FFT',rf_hint_constellation:'π/4-DQPSK',
    rf_waterfall:'TX 频谱瀑布图',rf_hint_waterfall:'滚动 · viridis 配色',
    rf_quality:'信号质量',rf_hint_quality:'功放前测量 · 来自同一 DSP 快照',
    rf_evm:'EVM',rf_papr:'PAPR',rf_carrier:'载波泄漏',rf_obw:'占用带宽 (99%)',
    rf_dc:'直流偏置 (I/Q)',rf_iqa:'IQ 幅度不平衡',rf_iqp:'IQ 相位不平衡',
    rf_hw_health:'硬件状态',rf_hint_health:'每 5 秒轮询',
    rf_temp:'SDR 温度',rf_tx_gain:'TX 增益（实际）',rf_rx_gain:'RX 增益（实际）',
    rf_temp_cold:'冷',rf_temp_nominal:'正常',rf_temp_warm:'温',rf_temp_hot:'热',rf_temp_na:'无传感器',
    rf_no_gains:'不可用',rf_just_now:'刚刚',

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
    confirm_logout:'确定注销吗？',
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
    wifi:'WiFi',wifi_status:'当前连接',wifi_saved:'已保存的网络',wifi_visible:'可用网络',wifi_loading:'加载中…',wifi_scanning:'扫描中…',wifi_no_device:'未检测到 WiFi 设备。',wifi_radio_disabled:'WiFi 已禁用。',wifi_not_connected:'未连接任何网络。',wifi_no_saved:'无已保存的网络。',wifi_no_networks:'范围内无可用网络。',wifi_ssid:'网络',wifi_signal:'信号',wifi_ip:'IP 地址',wifi_actions:'操作',wifi_disconnect:'断开',wifi_connect:'连接',wifi_connect_to:'连接到',wifi_connecting:'连接中…',wifi_connected:'已连接',wifi_connected_ok:'已连接。',wifi_saved_tag:'已保存',wifi_open:'开放',wifi_forget:'忘记',wifi_confirm_forget:'忘记网络',wifi_password:'密码',wifi_hidden:'隐藏网络 (SSID 不广播)',wifi_add_hidden:'+ 隐藏网络',wifi_scan:'↻ 扫描',wifi_refresh:'↻ 刷新',wifi_radio_off:'禁用 WiFi',wifi_radio_on:'启用 WiFi',wifi_warn_lose_access:'⚠ 如果您通过 WiFi 连接到仪表板,更换网络可能会暂时断开您的连接。请确保有备用访问方式。',wifi_err_no_ssid:'需要 SSID',cancel:'取消',sys_sensors:'主机硬件传感器',sys_sensors_empty:'未检测到传感器。',sys_rf:'RF 硬件 (SoapySDR)',sys_autorefresh:'自动刷新 5秒',
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
const PAGE_TITLES={stations:'stations',calls:'calls',lastheard:'lastheard',log:'log',rf:'rf',config:'config',system:'system'};
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
  if(name==='wifi')wifiRefresh();
  if(window.innerWidth<=700)closeMobileSidebar();
}

// ── WiFi management ────────────────────────────────────────────────────────
// All WiFi state mutations are last-write-wins and idempotent on the server,
// so we don't bother with optimistic UI updates — just fire the request,
// wait for completion, then refresh the displayed state. This is the only
// safe approach since nmcli can take a few seconds to actually associate
// and a brief "Connecting…" state is more honest than fake instant success.

let wifiState = { status: null, saved: [], scan: [], modalMode: null, modalSsid: null };

/// One-shot probe at boot: is nmcli installed on this host? Toggles the
/// sidebar nav item visibility. Falls back to hidden if the request fails
/// for any reason — better to not advertise than to crash on click.
async function wifiProbeAvailable(){
  try{
    const res = await fetch('/api/wifi/available');
    const j = await res.json();
    if(j && j.available){
      const nav = document.getElementById('nav-wifi');
      if(nav) nav.style.display = '';
    }
  }catch(_){ /* leave hidden */ }
}

async function wifiRefresh(){
  // Run status / saved / scan in parallel — they hit nmcli independently.
  await Promise.all([wifiLoadStatus(), wifiLoadSaved(), wifiScan()]);
}

async function wifiLoadStatus(){
  try{
    const r = await fetch('/api/wifi/status');
    const j = await r.json();
    if(!j.ok){ wifiRenderStatusError(j.error); return; }
    wifiState.status = j.status;
    wifiRenderStatus();
  }catch(e){ wifiRenderStatusError({kind:'Io', msg: String(e)}); }
}

function wifiRenderStatus(){
  const el = document.getElementById('wifi-status-grid');
  const radioBtn = document.getElementById('wifi-radio-btn');
  if(!el) return;
  const s = wifiState.status;
  if(!s){ el.innerHTML = '<div class="wifi-status-loading">'+(t('wifi_loading')||'Loading…')+'</div>'; return; }

  // The radio toggle label flips based on current state so the button reads
  // as the *action* it will perform, not the current state.
  if(radioBtn){
    radioBtn.textContent = s.radio_enabled ? (t('wifi_radio_off')||'Disable WiFi')
                                           : (t('wifi_radio_on') ||'Enable WiFi');
  }

  if(!s.device_present){
    el.innerHTML = '<div class="wifi-status-loading">'+(t('wifi_no_device')||'No WiFi device detected on this host.')+'</div>';
    return;
  }
  if(!s.radio_enabled){
    el.innerHTML = '<div class="wifi-status-loading">'+(t('wifi_radio_disabled')||'WiFi radio is disabled.')+'</div>';
    return;
  }
  if(!s.connected_ssid){
    el.innerHTML = '<div class="wifi-status-loading">'+(t('wifi_not_connected')||'Not connected to any network.')+'</div>';
    return;
  }

  el.innerHTML = `
    <div class="wifi-status-item">
      <div class="wifi-status-label">${t('wifi_ssid')||'Network'}</div>
      <div class="wifi-status-value accent">${escHtml(s.connected_ssid)}</div>
    </div>
    <div class="wifi-status-item">
      <div class="wifi-status-label">${t('wifi_signal')||'Signal'}</div>
      <div class="wifi-status-value">${s.signal != null ? s.signal+'%' : '—'}</div>
    </div>
    <div class="wifi-status-item">
      <div class="wifi-status-label">${t('wifi_ip')||'IP address'}</div>
      <div class="wifi-status-value">${s.ip_address ? escHtml(s.ip_address) : '—'}</div>
    </div>
    <div class="wifi-status-item">
      <div class="wifi-status-label">${t('wifi_actions')||'Actions'}</div>
      <div class="wifi-status-value"><button class="btn btn-sm btn-warn" onclick="wifiDisconnect()">${t('wifi_disconnect')||'Disconnect'}</button></div>
    </div>
  `;
}

function wifiRenderStatusError(err){
  const el = document.getElementById('wifi-status-grid');
  if(!el) return;
  const msg = err && err.msg ? err.msg : (typeof err === 'string' ? err : 'Error');
  el.innerHTML = `<div class="wifi-status-loading" style="color:var(--danger)">${escHtml(msg)}</div>`;
}

async function wifiLoadSaved(){
  const el = document.getElementById('wifi-saved-list');
  const cnt = document.getElementById('wifi-saved-count');
  if(!el) return;
  try{
    const r = await fetch('/api/wifi/saved');
    const j = await r.json();
    if(!j.ok){ el.innerHTML = `<div class="wifi-list-empty" style="color:var(--danger)">${escHtml(j.error&&j.error.msg||'Error')}</div>`; return; }
    wifiState.saved = j.profiles || [];
    if(cnt) cnt.textContent = wifiState.saved.length ? `${wifiState.saved.length}` : '';
    if(wifiState.saved.length === 0){
      el.innerHTML = `<div class="wifi-list-empty">${t('wifi_no_saved')||'No saved networks.'}</div>`;
      return;
    }
    el.innerHTML = wifiState.saved.map(p => `
      <div class="wifi-row ${p.active?'active':''}">
        <div class="wifi-row-main">
          <div class="wifi-row-ssid">
            ${escHtml(p.name)}
            ${p.active ? `<span class="wifi-tag active">${t('wifi_connected')||'CONNECTED'}</span>` : ''}
          </div>
        </div>
        <div class="wifi-row-actions">
          ${p.active ? '' : `<button class="btn btn-sm" onclick="wifiConnectSaved('${escAttr(p.uuid)}')">${t('wifi_connect')||'Connect'}</button>`}
          <button class="btn btn-sm btn-danger" onclick="wifiForget('${escAttr(p.uuid)}','${escAttr(p.name)}')">${t('wifi_forget')||'Forget'}</button>
        </div>
      </div>
    `).join('');
  }catch(e){
    el.innerHTML = `<div class="wifi-list-empty" style="color:var(--danger)">${escHtml(String(e))}</div>`;
  }
}

async function wifiScan(){
  const el = document.getElementById('wifi-scan-list');
  if(!el) return;
  el.innerHTML = `<div class="wifi-list-empty">${t('wifi_scanning')||'Scanning…'}</div>`;
  try{
    const r = await fetch('/api/wifi/scan');
    const j = await r.json();
    if(!j.ok){ el.innerHTML = `<div class="wifi-list-empty" style="color:var(--danger)">${escHtml(j.error&&j.error.msg||'Error')}</div>`; return; }
    wifiState.scan = j.networks || [];
    if(wifiState.scan.length === 0){
      el.innerHTML = `<div class="wifi-list-empty">${t('wifi_no_networks')||'No networks in range.'}</div>`;
      return;
    }
    el.innerHTML = wifiState.scan.map(n => {
      const bars = wifiSignalBars(n.signal);
      const isOpen = !n.security || n.security === '--';
      const secCls = isOpen ? 'sec open' : 'sec';
      const secLabel = isOpen ? (t('wifi_open')||'OPEN') : n.security;
      const tags = [];
      if(n.active) tags.push(`<span class="wifi-tag active">${t('wifi_connected')||'CONNECTED'}</span>`);
      else if(n.saved) tags.push(`<span class="wifi-tag saved">${t('wifi_saved_tag')||'SAVED'}</span>`);
      // Action button differs by state: if connected, no action; if saved,
      // quick reconnect; otherwise prompt for password.
      let actionBtn = '';
      if(!n.active){
        if(n.saved){
          actionBtn = `<button class="btn btn-sm" onclick="wifiConnectBySsid('${escAttr(n.ssid)}')">${t('wifi_connect')||'Connect'}</button>`;
        } else {
          actionBtn = `<button class="btn btn-sm btn-primary" onclick="wifiShowPasswordModal('${escAttr(n.ssid)}',${isOpen?'true':'false'})">${t('wifi_connect')||'Connect'}</button>`;
        }
      }
      return `
        <div class="wifi-row ${n.active?'active':''}">
          <div class="wifi-row-signal">${bars}</div>
          <div class="wifi-row-main">
            <div class="wifi-row-ssid">${escHtml(n.ssid)} ${tags.join(' ')}</div>
            <div class="wifi-row-meta">
              <span>${n.signal}%</span>
              <span class="${secCls}">${escHtml(secLabel)}</span>
            </div>
          </div>
          <div class="wifi-row-actions">${actionBtn}</div>
        </div>
      `;
    }).join('');
  }catch(e){
    el.innerHTML = `<div class="wifi-list-empty" style="color:var(--danger)">${escHtml(String(e))}</div>`;
  }
}

function wifiSignalBars(signal){
  // 4-bar signal indicator. Thresholds picked to roughly match what most
  // OS WiFi icons use: <25 = 1 bar, <50 = 2, <75 = 3, ≥75 = 4.
  const lit = signal >= 75 ? 4 : signal >= 50 ? 3 : signal >= 25 ? 2 : signal > 0 ? 1 : 0;
  return `<span class="wifi-bars">
    <span class="b1 ${lit>=1?'lit':''}"></span>
    <span class="b2 ${lit>=2?'lit':''}"></span>
    <span class="b3 ${lit>=3?'lit':''}"></span>
    <span class="b4 ${lit>=4?'lit':''}"></span>
  </span>`;
}

async function wifiConnectSaved(uuid){
  await wifiCall('/api/wifi/connect', { uuid });
  await wifiRefresh();
}

// "Connect by SSID" path is for networks already saved but visible in the
// scan — we have the credentials, just need to bring up the right profile.
async function wifiConnectBySsid(ssid){
  const p = wifiState.saved.find(p => p.name === ssid);
  if(p){ await wifiConnectSaved(p.uuid); return; }
  // Fallback: shouldn't happen, but if profile got deleted between scan and
  // click, prompt for password.
  wifiShowPasswordModal(ssid, false);
}

function wifiShowPasswordModal(ssid, isOpen){
  wifiState.modalMode = 'visible';
  wifiState.modalSsid = ssid;
  const ssidInput = document.getElementById('wifi-modal-ssid');
  const pskInput  = document.getElementById('wifi-modal-psk');
  const hiddenRow = document.getElementById('wifi-modal-hidden-row');
  const ssidRow   = document.getElementById('wifi-modal-ssid-row');
  const pskRow    = document.getElementById('wifi-modal-psk-row');
  const title     = document.getElementById('wifi-modal-title');
  const msg       = document.getElementById('wifi-modal-msg');
  ssidInput.value = ssid;
  pskInput.value = '';
  msg.textContent = '';
  msg.className = 'wifi-modal-msg';
  ssidRow.style.display = 'none';
  pskRow.style.display = isOpen ? 'none' : '';
  hiddenRow.style.display = 'none';
  title.textContent = `${t('wifi_connect_to')||'Connect to'}: ${ssid}`;
  document.getElementById('wifi-modal').style.display = 'flex';
  if(!isOpen) setTimeout(()=>pskInput.focus(), 50);
}

function wifiShowHiddenModal(){
  wifiState.modalMode = 'hidden';
  wifiState.modalSsid = null;
  const ssidInput = document.getElementById('wifi-modal-ssid');
  const pskInput  = document.getElementById('wifi-modal-psk');
  const hiddenRow = document.getElementById('wifi-modal-hidden-row');
  const hiddenCb  = document.getElementById('wifi-modal-hidden');
  const ssidRow   = document.getElementById('wifi-modal-ssid-row');
  const pskRow    = document.getElementById('wifi-modal-psk-row');
  const title     = document.getElementById('wifi-modal-title');
  const msg       = document.getElementById('wifi-modal-msg');
  ssidInput.value = '';
  pskInput.value = '';
  hiddenCb.checked = true; // hidden modal pre-checks the box, intuitive default
  msg.textContent = '';
  msg.className = 'wifi-modal-msg';
  ssidRow.style.display = '';
  pskRow.style.display = '';
  hiddenRow.style.display = '';
  title.textContent = t('wifi_add_hidden')||'Add hidden network';
  document.getElementById('wifi-modal').style.display = 'flex';
  setTimeout(()=>ssidInput.focus(), 50);
}

function wifiCloseModal(){
  document.getElementById('wifi-modal').style.display = 'none';
}

async function wifiModalSubmit(){
  const ssid = document.getElementById('wifi-modal-ssid').value.trim();
  const psk  = document.getElementById('wifi-modal-psk').value;
  const hidden = document.getElementById('wifi-modal-hidden').checked;
  const msg = document.getElementById('wifi-modal-msg');
  const okBtn = document.getElementById('wifi-modal-ok');
  if(!ssid){
    msg.textContent = t('wifi_err_no_ssid')||'SSID required';
    msg.className = 'wifi-modal-msg';
    return;
  }
  okBtn.disabled = true;
  msg.textContent = t('wifi_connecting')||'Connecting…';
  msg.className = 'wifi-modal-msg ok';
  const r = await wifiCall('/api/wifi/connect', { ssid, psk, hidden });
  okBtn.disabled = false;
  if(r && r.ok){
    msg.textContent = t('wifi_connected_ok')||'Connected.';
    setTimeout(()=>{ wifiCloseModal(); wifiRefresh(); }, 800);
  } else {
    const errMsg = r && r.error ? (r.error.msg || JSON.stringify(r.error)) : 'Failed';
    msg.textContent = errMsg;
    msg.className = 'wifi-modal-msg';
  }
}

async function wifiDisconnect(){
  await wifiCall('/api/wifi/disconnect', {});
  await wifiRefresh();
}

async function wifiForget(uuid, name){
  if(!confirm(`${t('wifi_confirm_forget')||'Forget network'} "${name}"?`)) return;
  await wifiCall('/api/wifi/forget', { uuid });
  await wifiRefresh();
}

async function wifiToggleRadio(){
  const s = wifiState.status;
  const newEnabled = s ? !s.radio_enabled : false;
  await wifiCall('/api/wifi/radio', { enabled: newEnabled });
  await wifiRefresh();
}

async function wifiCall(url, body){
  try{
    const r = await fetch(url, {
      method: 'POST',
      headers: {'Content-Type':'application/json'},
      body: JSON.stringify(body),
    });
    return await r.json();
  }catch(e){
    return { ok:false, error:{ kind:'Io', msg:String(e) } };
  }
}

function escAttr(s){ return String(s).replace(/&/g,'&amp;').replace(/'/g,"&#39;").replace(/"/g,'&quot;'); }

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
      // If the server already has recent RF snapshots, paint them instantly
      // so the RF page has data before the next emit cycle.
      if(msg.last_tx_visual){handleTxVisual(msg.last_tx_visual);}
      if(msg.last_tx_quality){handleTxQuality(msg.last_tx_quality);}
      if(msg.last_sdr_health){handleSdrHealth(msg.last_sdr_health);}
      if(msg.last_sys_health){handleSysHealth(msg.last_sys_health);}
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
    case 'tx_visual':handleTxVisual(msg);break;
    case 'tx_quality':handleTxQuality(msg);break;
    case 'sdr_health':handleSdrHealth(msg);break;
    case 'sys_health':handleSysHealth(msg);break;
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
      <td class="col-mobile-hide" style="text-align:center">${eeLabel(m.energy_saving_mode||0)}</td>
      <td><div class="rssi-bar"><div class="rssi-track"><div class="rssi-fill" style="width:${pct}%;background:${col}"></div></div><span class="rssi-val" style="color:${col}">${rL}</span></div></td>
      <td><span class="badge badge-green">${t('online_badge')}</span></td>
      <td class="col-mobile-hide">${lastSeenLabel(ls)}</td>
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
    return`<tr><td class="col-mobile-hide"><code>${c.call_id}</code></td><td><span class="badge ${badge}">${label}</span></td><td>${c.caller_issi?`<code>${c.caller_issi}</code>`:'—'}</td><td>${to}</td><td>${spk}</td><td style="font-family:var(--mono);font-size:12px;color:var(--accent2);font-weight:600">${mm}:${ss}</td></tr>`;
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

    // SDR badge in topbar — populated from auto-detected hardware on first /api/system fetch.
    // Hidden when the value is unknown or absent (e.g. file backend in tests).
    const sdrBadge = document.getElementById('sdr-badge');
    const sdrLabel = document.getElementById('sdr-badge-label');
    if (sdrBadge && sdrLabel) {
      const name = sysData.sdr_name;
      if (name && name !== 'unknown' && name.length > 0) {
        sdrLabel.textContent = name;
        sdrBadge.style.display = 'flex';
        sdrBadge.title = 'Detected SDR hardware: ' + name;
      } else {
        sdrBadge.style.display = 'none';
      }
    }

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

// Logout: hits /api/logout (clears the session cookie server-side) and navigates
// to /login. We surface the button only when auth is actually in effect — detected
// by whether the fs_session cookie is present.
function doLogout(){
  if(!confirm(t('confirm_logout')||'Log out?'))return;
  fetch('/api/logout',{method:'POST',credentials:'same-origin'})
    .finally(()=>{ window.location='/login'; });
}
// Heuristic: if the fs_auth marker cookie is set, auth is in effect on this server
// (the actual session token is fs_session which is HttpOnly and not readable here).
if(document.cookie.split(';').some(c=>c.trim().startsWith('fs_auth='))){
  const lb=document.getElementById('logout-btn');
  if(lb) lb.style.display='flex';
}

// ── RF live monitor rendering ──────────────────────────────────────────────
// We receive tx_visual + tx_quality messages: visual carries a 512-bin spectrum
// (i16 dB-tenths, fftshift'd) and up to 192 IQ samples for the constellation.
// Plus a richer set of derived metrics (EVM, PAPR, etc) we paint as health bars.
// All drawing is done on Canvas 2D — no external libs.

const rfState = {
  lastTs: 0,
  lastHwTs: 0,
  sampleRate: 0,
  centerFreq: 0,
  // Waterfall ring buffer — rows × FFT bins. Newest row at index 0; we shift on push.
  // Each row stores normalized [0..1] magnitudes so we can recolour on theme change.
  waterfall: [],
  waterfallMaxRows: 200,
};

function rfThemeColors(){
  // Read theme variables from CSS so colors track theme switches.
  const cs = getComputedStyle(document.documentElement);
  return {
    bg:      cs.getPropertyValue('--bg').trim()      || '#0a1118',
    grid:    cs.getPropertyValue('--border').trim()  || '#243244',
    text:    cs.getPropertyValue('--text2').trim()   || '#b5c0d0',
    text3:   cs.getPropertyValue('--text3').trim()   || '#7a8a9c',
    accent:  cs.getPropertyValue('--accent').trim()  || '#00d4a8',
    accent2: cs.getPropertyValue('--accent2').trim() || '#4da6ff',
    danger:  cs.getPropertyValue('--danger').trim()  || '#ff4d5e',
  };
}

function rfResizeCanvas(id){
  // HiDPI canvas: resize the backing store to match CSS pixels × devicePixelRatio.
  // Reset transform first or repeated calls compound the scale.
  const c = document.getElementById(id);
  if(!c) return null;
  const dpr = window.devicePixelRatio || 1;
  const rect = c.getBoundingClientRect();
  const w = Math.max(rect.width|0, 100);
  const h = Math.max(rect.height|0, 100);
  if(c.width !== w*dpr || c.height !== h*dpr){
    c.width = w*dpr;
    c.height = h*dpr;
  }
  const ctx = c.getContext('2d');
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return {canvas:c, ctx, w, h};
}

// The DSP emits TWO separate events for the RF page:
//
//   * tx_visual  — every ~200 ms.  Carries spectrum + IQ + RMS/peak.  Used for
//     the spectrum trace, constellation, waterfall and the top-row RMS/Peak
//     readout.  Fast cadence so the animation feels live.
//
//   * tx_quality — once per second.  Carries the derived metrics (EVM, PAPR,
//     carrier leak, OBW, DC offset, IQ imbalance).  Slow cadence so the
//     numeric cards don't flicker.  We additionally smooth across 3 messages
//     (≈3 s window) so they sit still.

// Rolling-average smoothing for the Signal Quality numbers + RMS/Peak.
// We average across SMOOTH_WINDOW most-recent samples so the values settle
// quickly enough to track real changes (a few seconds) without flickering.
const SMOOTH_WINDOW = 3;
const rfSmooth = {
  rms_dbfs: [], peak_dbfs: [],
  evm_pct: [], papr_db: [],
  carrier_leakage_db: [], occupied_bandwidth_hz: [],
  dc_offset_i: [], dc_offset_q: [],
  iq_amplitude_imbalance_db: [], iq_phase_imbalance_deg: [],
};
function rfPushAvg(key, v){
  if(!isFinite(v)) return v;
  const arr = rfSmooth[key];
  arr.push(v);
  if(arr.length > SMOOTH_WINDOW) arr.shift();
  let s = 0; for(const x of arr) s += x;
  return s / arr.length;
}

function handleTxVisual(msg){
  rfState.lastTs = Date.now();
  rfState.sampleRate = msg.sample_rate || 0;
  rfState.centerFreq = msg.center_freq_hz || 0;

  // RMS/Peak in the top strip — these come in at the fast cadence so we
  // smooth them before painting (otherwise the dB number jumps a couple of
  // tenths every 200 ms which reads as flicker).
  const rms  = rfPushAvg('rms_dbfs',  msg.rms_dbfs);
  const peak = rfPushAvg('peak_dbfs', msg.peak_dbfs);
  const freqMHz = (rfState.centerFreq / 1e6);
  const rateK   = (rfState.sampleRate / 1e3);
  setText('rf-freq', isFinite(freqMHz) && freqMHz>0 ? freqMHz.toFixed(3)+' MHz' : '—');
  setText('rf-rate', isFinite(rateK)   && rateK  >0 ? rateK.toFixed(1)+' kS/s'  : '—');
  setText('rf-rms',  isFinite(rms)  ? rms.toFixed(1)  +' dBFS' : '—');
  setText('rf-peak', isFinite(peak) ? peak.toFixed(1) +' dBFS' : '—');
  setText('rf-age',  t('rf_live')||'live');

  // Visual feeds redraw on every message — that's the whole point.
  const spec = (msg.spectrum_db_tenths || []).map(v => v / 10);
  drawRfSpectrum(spec, rfState.sampleRate);
  drawRfConstellation(msg.constellation_iq || []);
  pushWaterfall(spec);
  drawRfWaterfall();
}

function handleTxQuality(msg){
  // All quality metrics go through the rolling smoother before being painted.
  const evm  = rfPushAvg('evm_pct',                   msg.evm_pct);
  const papr = rfPushAvg('papr_db',                   msg.papr_db);
  const cl   = rfPushAvg('carrier_leakage_db',        msg.carrier_leakage_db);
  const obw  = rfPushAvg('occupied_bandwidth_hz',     msg.occupied_bandwidth_hz);
  const dci  = rfPushAvg('dc_offset_i',               msg.dc_offset_i);
  const dcq  = rfPushAvg('dc_offset_q',               msg.dc_offset_q);
  const iqa  = rfPushAvg('iq_amplitude_imbalance_db', msg.iq_amplitude_imbalance_db);
  const iqp  = rfPushAvg('iq_phase_imbalance_deg',    msg.iq_phase_imbalance_deg);

  paintQuality('rf-evm',     'rf-q-evm-wrap',  fmtPct(evm, 2),       evalEvm(evm));
  paintQuality('rf-papr',    'rf-q-papr-wrap', fmtDb(papr, 1),       evalPapr(papr));
  paintQuality('rf-carrier', 'rf-q-cl-wrap',   fmtDb(cl, 1, true),   evalCarrierLeakage(cl));
  paintQuality('rf-obw',     'rf-q-obw-wrap',  fmtKhz(obw),          evalObw(obw));
  paintQuality('rf-dc',      'rf-q-dc-wrap',   fmtDcPair(dci, dcq),  evalDcOffset(dci, dcq));
  paintQuality('rf-iqa',     'rf-q-iqa-wrap',  fmtDb(iqa, 2, true),  evalIqAmpImbal(iqa));
  paintQuality('rf-iqp',     'rf-q-iqp-wrap',
                isFinite(iqp) ? iqp.toFixed(2)+'°' : '—',
                evalIqPhaseImbal(iqp));
}

function handleSdrHealth(msg){
  rfState.lastHwTs = Date.now();
  setText('rf-hw-age', t('rf_just_now')||'just now');

  // Temperature with named state. Thresholds chosen so a typical LimeSDR running
  // at room temp (~45-55°C) reads "nominal", >65 is "warm", >80 is "hot".
  const tempEl = document.getElementById('rf-temp');
  const stateEl = document.getElementById('rf-temp-state');
  if(tempEl && stateEl){
    if(msg.temperature_c == null){
      tempEl.textContent = '—';
      stateEl.textContent = t('rf_temp_na')||'no sensor';
      stateEl.className = 'rf-hw-temp-state';
    } else {
      const tc = msg.temperature_c;
      tempEl.textContent = tc.toFixed(1) + ' °C';
      let cls = 'nominal', label = t('rf_temp_nominal')||'nominal';
      if(tc < 20){ cls='cold'; label = t('rf_temp_cold')||'cold'; }
      else if(tc > 80){ cls='hot'; label = t('rf_temp_hot')||'hot'; }
      else if(tc > 65){ cls='warm'; label = t('rf_temp_warm')||'warm'; }
      stateEl.textContent = label;
      stateEl.className = 'rf-hw-temp-state ' + cls;
    }
  }
  renderGainList('rf-tx-gains', msg.tx_gains || []);
  renderGainList('rf-rx-gains', msg.rx_gains || []);
}

function renderGainList(id, gains){
  const el = document.getElementById(id);
  if(!el) return;
  if(!gains.length){ el.innerHTML = '<span style="color:var(--text3)">'+(t('rf_no_gains')||'unavailable')+'</span>'; return; }
  el.innerHTML = gains.map(([name, db]) =>
    `<div class="rf-hw-gain-row"><span class="stage">${name}</span><span class="val">${db.toFixed(1)} dB</span></div>`
  ).join('');
}

// ── Host system health (temps, voltages, currents, power) ──────────────────
// Drives two UI surfaces:
//   1. The violet PWR badge in the topbar (only shown when total_power_w is known).
//   2. A sensor grid on the System tab (shown when any sensors are present).

function handleSysHealth(msg){
  // Topbar badge
  const badge = document.getElementById('pwr-badge');
  const lbl   = document.getElementById('pwr-badge-label');
  if(badge && lbl){
    if(msg && typeof msg.total_power_w === 'number' && isFinite(msg.total_power_w) && msg.total_power_w > 0){
      lbl.textContent = msg.total_power_w.toFixed(1) + ' W';
      badge.style.display = 'flex';
      badge.title = 'Host power draw — '+(msg.sensors||[]).length+' sensor(s) reporting';
    } else {
      badge.style.display = 'none';
    }
  }

  // System-tab sensor grid
  const card  = document.getElementById('sys-sensors-card');
  const grid  = document.getElementById('sys-sensors-grid');
  const empty = document.getElementById('sys-sensors-empty');
  const totEl = document.getElementById('sys-sensors-power-total');
  if(!card || !grid) return;

  const sensors = (msg && msg.sensors) || [];
  if(sensors.length === 0){
    // Nothing detected — leave the card hidden so we don't clutter the System tab.
    card.style.display = 'none';
    return;
  }
  card.style.display = '';

  if(empty) empty.style.display = 'none';

  // Sort: power first (most interesting), then temp, voltage, current. Within
  // a kind, keep server order (which itself sorts by hwmon chip discovery order).
  const kindOrder = {power:0, temperature:1, voltage:2, current:3};
  const sorted = sensors.slice().sort((a,b) => (kindOrder[a.kind]||9) - (kindOrder[b.kind]||9));

  grid.innerHTML = sorted.map(s => {
    const unit = sensorUnit(s.kind);
    const dp   = s.kind === 'temperature' ? 1
               : s.kind === 'voltage'     ? 3
               : s.kind === 'current'     ? 3
               : 2;
    const valColor = sensorColor(s.kind, s.value);
    return `<div class="sys-sensor-tile">
      <div class="sys-sensor-label" title="${escHtml(s.name)}">${escHtml(s.name)}</div>
      <div class="sys-sensor-value" style="color:${valColor}">${s.value.toFixed(dp)} <span class="sys-sensor-unit">${unit}</span></div>
    </div>`;
  }).join('');

  // Power total in card header
  if(totEl){
    if(typeof msg.total_power_w === 'number' && isFinite(msg.total_power_w) && msg.total_power_w > 0){
      totEl.textContent = '⚡ ' + msg.total_power_w.toFixed(2) + ' W total';
    } else {
      totEl.textContent = '';
    }
  }
}

function sensorUnit(kind){
  switch(kind){
    case 'temperature': return '°C';
    case 'voltage':     return 'V';
    case 'current':     return 'A';
    case 'power':       return 'W';
    default:            return '';
  }
}

// Colour the value: temperatures get warm tints, power values are violet,
// voltages/currents stay neutral (just monospace).
function sensorColor(kind, v){
  if(kind === 'temperature'){
    if(v >= 80) return 'var(--danger)';
    if(v >= 65) return '#f5a623';
    if(v >= 50) return 'var(--accent)';
    return 'var(--accent2)';
  }
  if(kind === 'power') return '#c8a4f5';
  return 'var(--text)';
}

function setText(id, txt){
  const e = document.getElementById(id);
  if(e) e.textContent = txt;
}

// ── Formatters ─────────────────────────────────────────────────────────────
function fmtPct(v, dp){ return isFinite(v) ? v.toFixed(dp||1)+' %' : '—'; }
function fmtDb(v, dp, signed){
  if(!isFinite(v)) return '—';
  return (signed && v >= 0 ? '+' : '') + v.toFixed(dp||1) + ' dB';
}
function fmtKhz(hz){ return isFinite(hz)&&hz>0 ? (hz/1000).toFixed(1)+' kHz' : '—'; }
function fmtDcPair(i, q){
  if(!isFinite(i) || !isFinite(q)) return '—';
  return i.toFixed(4)+' / '+q.toFixed(4);
}

// ── Health classifiers ─────────────────────────────────────────────────────
// Each returns {status: 'good'|'warn'|'bad', pct: 0..100} for bar fill width.
function evalEvm(v){
  if(!isFinite(v)) return {status:'good', pct:0};
  // ETSI EN 300 392-2 §6.5.4 spec is ≤10% for a TETRA subscriber.
  // For TX from an amateur SDR (LimeSDR/SXceiver/µCell etc) what actually shows up
  // is typically 5-15%. Be generous: <8% good, <15% warn, ≥15% bad.
  if(v < 8)  return {status:'good', pct: Math.min(100, v/8*40)};
  if(v < 15) return {status:'warn', pct: 40 + Math.min(60, (v-8)/7*40)};
  return {status:'bad', pct: 80 + Math.min(20, (v-15)/15*20)};
}
function evalPapr(v){
  if(!isFinite(v)) return {status:'good', pct:0};
  // TETRA π/4-DQPSK theoretical PAPR is ~3.5 dB. Real DSP output with RRC
  // pulse-shaping sits 4-7 dB. <7 good, <10 warn, ≥10 means clipping risk.
  if(v < 7)  return {status:'good', pct: Math.min(100, v/7*50)};
  if(v < 10) return {status:'warn', pct: 50 + (v-7)/3*30};
  return {status:'bad', pct: Math.min(100, 80 + (v-10)/3*20)};
}
function evalCarrierLeakage(v){
  if(!isFinite(v)) return {status:'good', pct:0};
  // Direct-conversion SDRs (SXceiver, µCell, LimeSDR) typically sit -25 to -35 dB.
  // -30 dB or better is good, -20 to -30 is warn, above -20 is bad (visible spur).
  if(v < -30) return {status:'good', pct: Math.max(10, 100 + v + 30)};
  if(v < -20) return {status:'warn', pct: 60 + (-20 - v)/10*20};
  return {status:'bad', pct: Math.min(100, 80 + (v + 20)/20*20)};
}
function evalObw(v){
  if(!isFinite(v) || v <= 0) return {status:'good', pct:0};
  // TETRA channel spacing is 25 kHz. A clean signal sits ~22-24 kHz wide.
  // <24 kHz good, <26 kHz warn (touching channel edges), ≥26 kHz bad (ACI risk).
  const k = v/1000;
  if(k < 24) return {status:'good', pct: Math.min(100, k/24*80)};
  if(k < 26) return {status:'warn', pct: 80 + (k-24)/2*15};
  return {status:'bad', pct: Math.min(100, 95 + (k-26)/10*5)};
}
function evalDcOffset(i, q){
  if(!isFinite(i) || !isFinite(q)) return {status:'good', pct:0};
  // Magnitude of DC vector. Realistic thresholds for amateur SDRs:
  // <0.03 good, <0.08 warn, ≥0.08 bad (causes visible centre spike).
  const mag = Math.hypot(i, q);
  if(mag < 0.03) return {status:'good', pct: mag/0.03*40};
  if(mag < 0.08) return {status:'warn', pct: 40 + (mag-0.03)/0.05*40};
  return {status:'bad', pct: Math.min(100, 80 + (mag-0.08)/0.08*20)};
}
function evalIqAmpImbal(v){
  if(!isFinite(v)) return {status:'good', pct:0};
  // <0.5 dB good, <1.5 dB warn, >1.5 dB bad. Amateur SDRs sit ~0.2-0.6 dB typically.
  const a = Math.abs(v);
  if(a < 0.5) return {status:'good', pct: a/0.5*40};
  if(a < 1.5) return {status:'warn', pct: 40 + (a-0.5)/1*40};
  return {status:'bad', pct: Math.min(100, 80 + (a-1.5)/2*20)};
}
function evalIqPhaseImbal(v){
  if(!isFinite(v)) return {status:'good', pct:0};
  // <2° good, <5° warn, >5° bad. Sub-1° is professional-grade.
  const a = Math.abs(v);
  if(a < 2) return {status:'good', pct: a/2*40};
  if(a < 5) return {status:'warn', pct: 40 + (a-2)/3*40};
  return {status:'bad', pct: Math.min(100, 80 + (a-5)/5*20)};
}

function paintQuality(valueId, wrapId, valueText, evalResult){
  setText(valueId, valueText);
  const wrap = document.getElementById(wrapId);
  if(!wrap) return;
  wrap.classList.remove('rf-q-good','rf-q-warn','rf-q-bad');
  wrap.classList.add('rf-q-' + evalResult.status);
  const bar = wrap.querySelector('.rf-qmetric-fill');
  if(bar) bar.style.width = evalResult.pct.toFixed(0) + '%';
}

function drawRfSpectrum(spec, sampleRate){
  const r = rfResizeCanvas('rf-spectrum');
  if(!r || !spec.length) return;
  const {ctx, w, h} = r;
  const col = rfThemeColors();

  ctx.fillStyle = col.bg;
  ctx.fillRect(0, 0, w, h);

  // Y axis: dynamic dB range. Clamp to a sensible window so noise floor wiggles
  // don't make the spectrum jump around.
  let minDb = -90, maxDb = 0;
  for(const v of spec){ if(isFinite(v)){ if(v<minDb) minDb = v; if(v>maxDb) maxDb = v; } }
  minDb = Math.max(Math.floor(minDb/10)*10 - 5, -130);
  maxDb = Math.min(Math.ceil(maxDb/10)*10 + 5, 10);
  if(maxDb - minDb < 30) maxDb = minDb + 30;

  ctx.strokeStyle = col.grid;
  ctx.lineWidth = 1;
  ctx.font = '10px ui-monospace, Cascadia Code, Consolas, monospace';
  ctx.fillStyle = col.text3;
  ctx.textAlign = 'right';
  ctx.textBaseline = 'middle';

  for(let db = Math.ceil(minDb/20)*20; db <= maxDb; db += 20){
    const y = h - (db - minDb)/(maxDb - minDb) * h;
    ctx.beginPath();
    ctx.moveTo(40, y); ctx.lineTo(w, y);
    ctx.stroke();
    ctx.fillText(db+' dB', 36, y);
  }

  const halfRateKHz = (sampleRate || 600000) / 2 / 1000;
  ctx.textAlign = 'center';
  ctx.textBaseline = 'bottom';
  const numTicks = 8;
  for(let i = 0; i <= numTicks; i++){
    const x = 40 + (w - 40) * i / numTicks;
    ctx.beginPath();
    ctx.moveTo(x, 0); ctx.lineTo(x, h - 14);
    ctx.stroke();
    const offKHz = -halfRateKHz + 2*halfRateKHz * i/numTicks;
    ctx.fillText((offKHz>=0?'+':'')+offKHz.toFixed(0), x, h - 2);
  }

  ctx.strokeStyle = col.accent;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  for(let i = 0; i < spec.length; i++){
    const x = 40 + (w - 40) * i / (spec.length - 1);
    const y = h - 14 - (spec[i] - minDb)/(maxDb - minDb) * (h - 14);
    if(i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  }
  ctx.stroke();
}

function drawRfConstellation(iqInt16){
  const r = rfResizeCanvas('rf-constellation');
  if(!r) return;
  const {ctx, w, h} = r;
  const col = rfThemeColors();

  ctx.fillStyle = col.bg;
  ctx.fillRect(0, 0, w, h);

  const size = Math.min(w, h) - 20;
  const cx = w / 2;
  const cy = h / 2;

  ctx.strokeStyle = col.grid;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(cx - size/2, cy); ctx.lineTo(cx + size/2, cy);
  ctx.moveTo(cx, cy - size/2); ctx.lineTo(cx, cy + size/2);
  ctx.stroke();

  ctx.strokeStyle = col.grid;
  ctx.beginPath();
  ctx.arc(cx, cy, size/2 * 0.66, 0, Math.PI*2);
  ctx.stroke();

  ctx.fillStyle = col.text3;
  for(let k = 0; k < 8; k++){
    const a = k * Math.PI/4;
    const x = cx + Math.cos(a) * size/2 * 0.66;
    const y = cy - Math.sin(a) * size/2 * 0.66;
    ctx.beginPath();
    ctx.arc(x, y, 2.5, 0, Math.PI*2);
    ctx.fill();
  }

  const SCALE = 1.5 / 32767;
  ctx.fillStyle = col.accent;
  for(let i = 0; i + 1 < iqInt16.length; i += 2){
    const re = iqInt16[i]   * SCALE;
    const im = iqInt16[i+1] * SCALE;
    const x = cx + re * (size/2 * 0.66);
    const y = cy - im * (size/2 * 0.66);
    ctx.beginPath();
    ctx.arc(x, y, 1.8, 0, Math.PI*2);
    ctx.fill();
  }
}

// ── Waterfall ──────────────────────────────────────────────────────────────
// Maintain a rolling buffer of recent spectra. Each new snapshot lands at the
// top of the canvas; older rows scroll down. Colours come from a viridis-style
// palette so the contrast works for daltonism (no red-green dependence).

function pushWaterfall(specDb){
  if(!specDb || !specDb.length) return;
  // Normalize to [0..1] using a fixed reference window so colours don't shift wildly.
  // We keep a moving reference of the maximum to anchor the bright end.
  const REF_MIN = -100, REF_MAX = 0;
  const normalized = new Float32Array(specDb.length);
  for(let i = 0; i < specDb.length; i++){
    let v = (specDb[i] - REF_MIN) / (REF_MAX - REF_MIN);
    if(!isFinite(v)) v = 0;
    if(v < 0) v = 0;
    if(v > 1) v = 1;
    normalized[i] = v;
  }
  rfState.waterfall.unshift(normalized);
  if(rfState.waterfall.length > rfState.waterfallMaxRows){
    rfState.waterfall.length = rfState.waterfallMaxRows;
  }
}

// Viridis approximation: 5-stop colour map mov→albastru→teal→verde-galben→galben.
// Hand-tuned RGB stops so the bottom is dark blue (low magnitude) and the top is
// bright yellow (peak). Linear interpolation between stops keeps it monotonic.
function viridisColor(t){
  const stops = [
    [0.00, 68, 1, 84],
    [0.25, 59, 82, 139],
    [0.50, 33, 145, 140],
    [0.75, 94, 201, 98],
    [1.00, 253, 231, 37],
  ];
  if(t <= 0) return [stops[0][1], stops[0][2], stops[0][3]];
  if(t >= 1) return [stops[4][1], stops[4][2], stops[4][3]];
  for(let i = 0; i < stops.length - 1; i++){
    if(t >= stops[i][0] && t <= stops[i+1][0]){
      const a = stops[i], b = stops[i+1];
      const f = (t - a[0]) / (b[0] - a[0]);
      return [
        Math.round(a[1] + (b[1]-a[1])*f),
        Math.round(a[2] + (b[2]-a[2])*f),
        Math.round(a[3] + (b[3]-a[3])*f),
      ];
    }
  }
  return [0,0,0];
}

function parseHexRgb(hex){
  if(!hex || hex[0] !== '#') return null;
  const s = hex.length === 7 ? hex.slice(1) : (hex.length === 4 ?
    hex[1]+hex[1]+hex[2]+hex[2]+hex[3]+hex[3] : null);
  if(!s) return null;
  const n = parseInt(s, 16);
  if(isNaN(n)) return null;
  return [(n>>16)&0xff, (n>>8)&0xff, n&0xff];
}

function drawRfWaterfall(){
  const r = rfResizeCanvas('rf-waterfall');
  if(!r || !rfState.waterfall.length) return;
  const {ctx, w, h} = r;
  const col = rfThemeColors();
  // Background colour as RGB for the noise-floor mask. We replace viridis(0)≈purple
  // with the page background for bins below threshold so the waterfall reads as
  // "signal vs nothing" instead of "purple everywhere".
  const bgRgb = parseHexRgb(col.bg) || [10, 17, 24];

  ctx.fillStyle = col.bg;
  ctx.fillRect(0, 0, w, h);

  const rows = Math.min(rfState.waterfall.length, h|0);
  const bins = rfState.waterfall[0].length;
  const leftPad = 40;
  const drawW = (w - leftPad)|0;
  if(drawW <= 0 || rows <= 0) return;

  // Noise-floor threshold in [0..1]. pushWaterfall normalises -100..0 dBFS into 0..1,
  // so 0.18 corresponds to ~-82 dBFS — well below any real TETRA signal.
  const NOISE_FLOOR = 0.18;

  const img = ctx.createImageData(drawW, rows);
  for(let row = 0; row < rows; row++){
    const spec = rfState.waterfall[row];
    for(let x = 0; x < drawW; x++){
      const binIdx = Math.min(bins - 1, ((x / drawW) * bins)|0);
      const v = spec[binIdx];
      const rgb = v < NOISE_FLOOR ? bgRgb : viridisColor(v);
      const p = (row * drawW + x) * 4;
      img.data[p]   = rgb[0];
      img.data[p+1] = rgb[1];
      img.data[p+2] = rgb[2];
      img.data[p+3] = 255;
    }
  }
  ctx.putImageData(img, leftPad, 0);

  // Time axis on the left: tick every 30 rows ≈ 30s (one row per snapshot, ~1Hz).
  // Only draw ticks up to the number of rows we actually have, so the axis never
  // pretends "-180s" when we only have 30s of history.
  ctx.font = '9px ui-monospace, Cascadia Code, Consolas, monospace';
  ctx.fillStyle = col.text3;
  ctx.textAlign = 'right';
  ctx.textBaseline = 'middle';
  for(let s = 0; s <= rows; s += 30){
    ctx.fillText('-'+s+'s', leftPad - 4, s + 1);
    ctx.strokeStyle = col.grid;
    ctx.beginPath();
    ctx.moveTo(leftPad - 2, s); ctx.lineTo(leftPad, s);
    ctx.stroke();
  }
}

// ── Age refresh & resize ───────────────────────────────────────────────────
setInterval(() => {
  if(rfState.lastTs){
    const age = (Date.now() - rfState.lastTs) / 1000;
    if(age > 3){
      setText('rf-age', (t('rf_stale')||'stale')+' · '+age.toFixed(0)+'s');
    }
  }
  if(rfState.lastHwTs){
    const age = (Date.now() - rfState.lastHwTs) / 1000;
    if(age < 6) setText('rf-hw-age', age.toFixed(0)+'s');
    else        setText('rf-hw-age', age.toFixed(0)+'s '+(t('rf_stale')||'stale'));
  }
}, 1000);

window.addEventListener('resize', () => {
  rfResizeCanvas('rf-spectrum');
  rfResizeCanvas('rf-constellation');
  rfResizeCanvas('rf-waterfall');
  drawRfWaterfall();
});

connect();
// Probe NetworkManager availability once at boot — toggles the WiFi nav item.
wifiProbeAvailable();
</script>
</body>
</html>
"#;

/// Standalone login page. Served at GET /login by the dashboard when auth is
/// configured. Keeps the visual language of the dashboard (same dark palette, mono
/// title type) but is self-contained: a single document, no external deps, no
/// font downloads. Form posts to POST /api/login as JSON via fetch().
pub const LOGIN_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no">
<meta name="theme-color" content="#0a1118">
<title>FlowStation — Login</title>
<style>
:root{
  --bg:#0a1118;--bg2:#0f1820;--bg3:#172230;--bg4:#1d2a3a;
  --border:#243244;--border2:#2a3a52;
  --text:#e8edf3;--text2:#b5c0d0;--text3:#7a8a9c;
  --accent:#00d4a8;--accent2:#4da6ff;--danger:#ff4d5e;
  --mono:'ui-monospace','Cascadia Code','Consolas','Liberation Mono','Menlo',monospace;
  --sans: 'ui-sans-serif', system-ui, -apple-system, 'Segoe UI', 'Microsoft YaHei', 'Noto Sans SC', 'PingFang SC', 'Hiragino Sans GB', 'WenQuanYi Micro Hei', sans-serif;
}
*{box-sizing:border-box;}
html,body{margin:0;padding:0;height:100%;}
body{
  font-family:var(--sans);background:var(--bg);color:var(--text);
  display:flex;align-items:center;justify-content:center;
  min-height:100vh;min-height:100dvh;
  padding:20px;
  /* Subtle gradient backdrop so the card pops without distracting */
  background:
    radial-gradient(circle at 20% 10%, rgba(77,166,255,0.10), transparent 50%),
    radial-gradient(circle at 80% 90%, rgba(0,212,168,0.10), transparent 50%),
    var(--bg);
  -webkit-tap-highlight-color:transparent;
}

.login-card{
  width:100%;max-width:380px;
  background:var(--bg2);border:1px solid var(--border2);
  border-radius:14px;
  box-shadow:0 20px 50px rgba(0,0,0,0.5), 0 0 0 1px rgba(255,255,255,0.02);
  padding:36px 32px 32px;
  position:relative;overflow:hidden;
}
/* Top accent bar */
.login-card::before{
  content:"";position:absolute;top:0;left:0;right:0;height:3px;
  background:linear-gradient(90deg, var(--accent) 0%, var(--accent2) 100%);
}

.logo-wrap{display:flex;flex-direction:column;align-items:center;gap:14px;margin-bottom:26px;}
/* Tower / antenna mark — SVG inlined so there's no extra request */
.logo-mark{
  width:64px;height:64px;
  border-radius:14px;
  background:linear-gradient(135deg, rgba(0,212,168,0.15) 0%, rgba(77,166,255,0.15) 100%);
  border:1px solid rgba(0,212,168,0.3);
  display:flex;align-items:center;justify-content:center;
  box-shadow:0 0 24px rgba(0,212,168,0.15);
}
.logo-mark svg{width:36px;height:36px;}

.logo-title{
  font-family:var(--mono);font-size:13px;font-weight:700;
  letter-spacing:0.18em;text-transform:uppercase;
  color:var(--text);
  display:flex;align-items:center;gap:8px;
}
.logo-title .accent{color:var(--accent);}
.logo-sub{
  font-family:var(--mono);font-size:10px;font-weight:500;
  letter-spacing:0.1em;text-transform:uppercase;
  color:var(--text3);
}

form{display:flex;flex-direction:column;gap:14px;}
.field-label{
  display:block;font-family:var(--mono);font-size:10px;font-weight:600;
  letter-spacing:0.1em;text-transform:uppercase;color:var(--text3);
  margin-bottom:6px;
}
input[type="text"],input[type="password"]{
  width:100%;
  background:var(--bg3);border:1px solid var(--border2);
  color:var(--text);
  padding:12px 14px;border-radius:8px;
  font-family:var(--mono);font-size:14px;
  outline:none;transition:border-color 0.15s, background 0.15s;
  -webkit-appearance:none;appearance:none;
}
input:focus{border-color:var(--accent2);background:var(--bg4);}
/* iOS Safari respects the 16px rule to skip the auto-zoom; we set 14px on desktop
   and bump back up on mobile via the @media block below. */

.btn-login{
  width:100%;
  background:linear-gradient(180deg, var(--accent) 0%, #00b893 100%);
  color:#06231d;font-weight:700;letter-spacing:0.04em;
  border:none;border-radius:8px;
  padding:13px 16px;font-family:var(--sans);font-size:14px;
  cursor:pointer;
  margin-top:6px;
  transition:transform 0.05s, box-shadow 0.15s, filter 0.15s;
  box-shadow:0 4px 14px rgba(0,212,168,0.3);
}
.btn-login:hover{filter:brightness(1.05);}
.btn-login:active{transform:translateY(1px);}
.btn-login:disabled{opacity:0.6;cursor:not-allowed;}

.err{
  min-height:18px;font-family:var(--mono);font-size:11px;
  color:var(--danger);text-align:center;margin-top:4px;
  letter-spacing:0.05em;
}

.footer{
  margin-top:22px;text-align:center;
  font-family:var(--mono);font-size:10px;color:var(--text3);
  letter-spacing:0.06em;
}
.footer a{color:var(--text3);text-decoration:none;}
.footer a:hover{color:var(--accent2);}

@media(max-width:500px){
  body{padding:14px;}
  .login-card{padding:28px 22px;border-radius:12px;}
  .logo-mark{width:56px;height:56px;}
  .logo-mark svg{width:30px;height:30px;}
  /* Bigger inputs on mobile: prevents iOS zoom-on-focus, easier tap target. */
  input[type="text"],input[type="password"]{font-size:16px;padding:14px 14px;}
  .btn-login{font-size:15px;padding:14px 16px;min-height:48px;}
}
</style>
</head>
<body>
<div class="login-card">
  <div class="logo-wrap">
    <div class="logo-mark">
      <!-- Stylised antenna tower with radio waves -->
      <svg viewBox="0 0 32 32" xmlns="http://www.w3.org/2000/svg" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="color:var(--accent)">
        <!-- Tower legs -->
        <path d="M14 28 L16 8 L18 28" />
        <!-- Cross braces -->
        <line x1="14.6" y1="22" x2="17.4" y2="22"/>
        <line x1="14.9" y1="17" x2="17.1" y2="17"/>
        <line x1="15.2" y1="13" x2="16.8" y2="13"/>
        <!-- Tip antenna -->
        <line x1="16" y1="8" x2="16" y2="4"/>
        <circle cx="16" cy="3" r="1" fill="currentColor"/>
        <!-- Radio waves -->
        <path d="M9 8 Q6 11 6 16" style="color:var(--accent2)" opacity="0.7"/>
        <path d="M23 8 Q26 11 26 16" style="color:var(--accent2)" opacity="0.7"/>
        <path d="M11 6 Q7 9 7 14" style="color:var(--accent2)" opacity="0.4"/>
        <path d="M21 6 Q25 9 25 14" style="color:var(--accent2)" opacity="0.4"/>
      </svg>
    </div>
    <div style="text-align:center">
      <div class="logo-title"><span>Flow</span><span class="accent">Station</span></div>
      <div class="logo-sub">TETRA Base Station</div>
    </div>
  </div>

  <form id="login-form" autocomplete="on">
    <div>
      <label class="field-label" for="username">Username</label>
      <input type="text" id="username" name="username" autocomplete="username"
             autocapitalize="none" autocorrect="off" spellcheck="false"
             required>
    </div>
    <div>
      <label class="field-label" for="password">Password</label>
      <input type="password" id="password" name="password" autocomplete="current-password"
             required>
    </div>
    <button type="submit" class="btn-login" id="submit-btn">Sign In</button>
    <div class="err" id="err"></div>
  </form>

  <div class="footer">
    github.com/razvanzeces/<a href="https://github.com/razvanzeces/flowstation" target="_blank">flowstation</a>
  </div>
</div>

<script>
const form = document.getElementById('login-form');
const errBox = document.getElementById('err');
const btn = document.getElementById('submit-btn');

form.addEventListener('submit', async (e) => {
  e.preventDefault();
  errBox.textContent = '';
  btn.disabled = true;
  btn.textContent = 'Signing in…';

  const user = document.getElementById('username').value;
  const password = document.getElementById('password').value;

  try {
    const r = await fetch('/api/login', {
      method:'POST',
      headers:{'Content-Type':'application/json'},
      body: JSON.stringify({user, password}),
      credentials: 'same-origin',
    });
    if (r.ok) {
      // Session cookie has been set by the server; navigate to dashboard.
      window.location = '/';
      return;
    }
    if (r.status === 401) {
      errBox.textContent = 'Invalid credentials';
    } else {
      errBox.textContent = 'Login failed (' + r.status + ')';
    }
  } catch (e) {
    errBox.textContent = 'Network error: ' + e.message;
  }
  btn.disabled = false;
  btn.textContent = 'Sign In';
});

// Auto-focus username on desktop; mobile keyboards open virtually so we don't on small screens.
if (window.innerWidth > 600) {
  document.getElementById('username').focus();
}
</script>
</body>
</html>
"##;
