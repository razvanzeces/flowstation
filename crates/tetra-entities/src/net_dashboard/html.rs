pub const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en" data-uisize="m">
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
  --bg:      #090d14;
  --bg2:     #111824;
  --bg3:     #19212f;
  --bg4:     #232e40;
  --border:  #232e40;
  --border2: #33415a;
  --accent:  #00d4a8;
  --accent2: #4da6ff;
  --warn:    #ffb224;
  --danger:  #ff4d6d;
  --text:    #eef3fb;
  --text2:   #94abc9;
  --text3:   #4c628a;
  --muted:   var(--text2);   /* help/secondary text — was referenced everywhere but never defined */
  --sidebar: #070a10;
  --sidebar-border: #161d2c;
  --card-shadow: 0 1px 3px rgba(0,0,0,0.4);
  --r: 10px;

  /* ── Design-system v3 "Instrument" tokens (single source of truth) ──
     Semantic + structural tokens consumed by the reusable component classes
     (.hero/.card/.pill/.gauge/.group-list/.field/.btn/.banner/.sheet …).
     Define them HERE so nothing references them before they exist. */
  --ok:    #2ec6a6;                         /* canonical "healthy" green — replaces every #3fb950 */
  --info:  var(--accent2);                  /* neutral / idle accent */
  --sep:   rgba(255,255,255,0.07);          /* hairline divider (inset from leading edge) */
  --hair:  inset 0 1px 0 rgba(255,255,255,0.05);   /* top inner-highlight (was defined far below first use) */
  --mat:   color-mix(in srgb, var(--bg2) 82%, transparent);   /* translucent material for sidebar/sheets/popovers */
  --elev-1: 0 1px 2px rgba(0,0,0,.18), 0 8px 24px -12px rgba(0,0,0,.28);  /* the ONE card shadow */
  --r-card: 12px;
  --r-ctrl: 8px;
  --r-pill: 999px;
  --r-chip: 6px;

  --mono: 'ui-monospace','Cascadia Code','Consolas','Liberation Mono','Menlo',monospace;
  --sans: 'ui-sans-serif', system-ui, -apple-system, 'Segoe UI', 'Microsoft YaHei', 'Noto Sans SC', 'PingFang SC', 'Hiragino Sans GB', 'WenQuanYi Micro Hei', sans-serif;
}
[data-theme="light"]{
  --bg:#eceff4;--bg2:#ffffff;--bg3:#e6eaf1;--bg4:#d6dde7;
  --border:#dde3ec;--border2:#c4cdd9;
  --accent:#00876a;--accent2:#1565c0;--warn:#9a5400;--danger:#c0203a;
  --text:#16202e;--text2:#3d4f66;--text3:#5f7188;
  --sidebar:#ffffff;--sidebar-border:#e3e8ef;
  --card-shadow:0 1px 3px rgba(20,30,50,0.06),0 4px 16px -8px rgba(20,30,50,0.10);
  --ok:#16876b;--info:var(--accent2);
  --sep:rgba(20,30,50,0.09);
  --hair: inset 0 1px 0 rgba(255,255,255,0.7);
  --mat: color-mix(in srgb, var(--bg2) 82%, transparent);
  --elev-1: 0 1px 2px rgba(20,30,50,.05), 0 10px 30px -16px rgba(20,30,50,.12);
}
[data-theme="blue"]{
  --bg:#03071e;--bg2:#060d2a;--bg3:#091235;--bg4:#0d1840;
  --border:#112060;--border2:#1a2e7a;
  --accent:#00f5d4;--accent2:#60b8ff;--warn:#ffc947;--danger:#ff5577;
  --text:#deeeff;--text2:#7ab0e0;--text3:#1a3a60;
  --sidebar:#020514;--sidebar-border:#0c1840;
  --card-shadow:0 1px 3px rgba(0,0,200,0.15);
  --ok:#00f5d4;--info:var(--accent2);
  --sep:rgba(120,180,255,0.10);
  --mat: color-mix(in srgb, var(--bg2) 82%, transparent);
  --elev-1: 0 1px 2px rgba(0,0,0,.30), 0 8px 24px -12px rgba(0,0,200,.30);
}

/* ── Readability scale (eye control) ──────────────────────────────────────────
   --ts is one text-scale multiplier consumed by the curated readability block
   (the @media min-width:701px block) via calc(). data-uisize lives on <html>,
   persisted as fs_uisize. High/Ultra also strengthen the muted text tiers —
   theme-agnostic, because we reassign the *tokens* themselves. */
:root{ --ts:1.10; --wt-quiet:600; }   /* boot default = Medium (≈16.5px base) */
html[data-uisize="s"]{ --ts:0.92; }
html[data-uisize="m"]{ --ts:1.10; }
html[data-uisize="h"]{ --ts:1.26; --text3:var(--text2); --wt-quiet:600; }
html[data-uisize="u"]{ --ts:1.46; --text3:var(--text); --text2:var(--text); --wt-quiet:700; }

/* ── Touchscreen mode (FH-FEAT-008) ──────────────────────────────────────────
   Opt-in via body.touch-mode (persisted in localStorage), OR auto-enabled on a
   coarse-pointer device unless the user opted out (body.no-touch-mode). Class-based
   so it composes with the dark/light/blue data-themes; scoped so the desktop
   (fine pointer, no class) is completely unaffected. Targets >=44px tap targets. */
body.touch-mode{font-size:18px;}
body.touch-mode .btn,
body.touch-mode .btn-sm{min-height:44px;padding:10px 16px;font-size:13px;}
body.touch-mode .nav-item{min-height:44px;padding:11px 14px;font-size:15px;}
body.touch-mode .theme-btn,
body.touch-mode .lang-btn,
body.touch-mode .touch-btn{min-height:40px;padding:8px 12px;font-size:13px;}
body.touch-mode .logout-btn{width:42px;height:42px;font-size:18px;}
body.touch-mode input[type="text"],
body.touch-mode input[type="number"],
body.touch-mode input[type="password"],
body.touch-mode input[type="range"],
body.touch-mode select,
body.touch-mode textarea{min-height:44px;font-size:15px;}
@media (pointer:coarse){
  body:not(.no-touch-mode){font-size:18px;}
  body:not(.no-touch-mode) .btn,
  body:not(.no-touch-mode) .btn-sm{min-height:44px;padding:10px 16px;}
  body:not(.no-touch-mode) .nav-item{min-height:44px;padding:11px 14px;font-size:15px;}
  body:not(.no-touch-mode) input,
  body:not(.no-touch-mode) select,
  body:not(.no-touch-mode) textarea{min-height:44px;}
}
/* Touch toggle — its OWN class (never .theme-btn) so setTheme()'s active-reset
   can't desync its highlight from the actual touch state. */
.touch-btn{
  background:var(--bg3);color:var(--text2);border:1px solid var(--border);
  border-radius:6px;padding:5px 10px;font-size:12px;font-weight:600;cursor:pointer;
}
.touch-btn:hover{color:var(--text);}
.touch-btn.active{background:var(--accent);color:var(--bg);border-color:var(--accent);}

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
  display:flex;flex-direction:column;gap:12px;
  flex-shrink:0;
}
.logo-row{display:flex;align-items:center;gap:10px;}
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

/* ── Hardware status rows — iOS-Settings status block fused to the brand header ── */
.hw-status{
  display:flex;flex-direction:column;gap:2px;
  padding:5px;border-radius:9px;
  background:color-mix(in srgb,var(--text) 3%,transparent);
  border:1px solid var(--sidebar-border);
  box-shadow:var(--hair);
  transition:opacity 0.15s,padding 0.2s,border-color 0.2s,background 0.2s;
}
/* JS sets display:flex on these wrappers when populated (else display:none). */
.hw-row{
  display:flex;align-items:center;gap:9px;
  padding:6px 7px;border-radius:7px;
  overflow:hidden;cursor:default;transition:background 0.15s;
}
.hw-row + .hw-row{box-shadow:inset 0 1px 0 var(--sidebar-border);}
.hw-row:hover{background:color-mix(in srgb,var(--text) 4%,transparent);}
.hw-row:hover + .hw-row{box-shadow:none;}
.hw-glyph{
  flex-shrink:0;width:22px;height:22px;border-radius:6px;
  display:flex;align-items:center;justify-content:center;
}
.hw-glyph svg{width:14px;height:14px;display:block;}
.hw-row--sdr .hw-glyph{color:var(--accent);background:color-mix(in srgb,var(--accent) 12%,transparent);}
.hw-row--pwr .hw-glyph{color:var(--warn);background:color-mix(in srgb,var(--warn) 14%,transparent);}
.hw-meta{
  flex:1;min-width:0;display:flex;flex-direction:column;line-height:1.2;
  overflow:hidden;transition:opacity 0.15s,width 0.15s;
}
.hw-key{
  font-family:var(--mono);font-size:8.5px;font-weight:700;letter-spacing:0.12em;
  text-transform:uppercase;color:var(--text3);
}
.hw-val{
  font-family:var(--mono);font-size:11px;font-weight:600;color:var(--text2);
  letter-spacing:0.01em;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
}
/* Live link indicator — soft radiating teal pulse ("SDR is talking to RF"). */
.hw-live{flex-shrink:0;display:flex;align-items:center;}
.hw-live-dot{
  width:6px;height:6px;border-radius:50%;background:var(--accent);
  box-shadow:0 0 0 0 color-mix(in srgb,var(--accent) 55%,transparent);
  animation:hw-pulse 2.4s ease-in-out infinite;
}
@keyframes hw-pulse{
  0%  {box-shadow:0 0 0 0 color-mix(in srgb,var(--accent) 55%,transparent);}
  70% {box-shadow:0 0 0 5px color-mix(in srgb,var(--accent) 0%,transparent);}
  100%{box-shadow:0 0 0 0 color-mix(in srgb,var(--accent) 0%,transparent);}
}

/* Collapsed rail (56px): keep the tinted glyphs, drop labels/value/dot gracefully. */
#sidebar.collapsed .sidebar-logo{padding-left:0;padding-right:0;align-items:center;}
#sidebar.collapsed .hw-status{background:transparent;border-color:transparent;box-shadow:none;padding:2px 0;gap:6px;}
#sidebar.collapsed .hw-row{justify-content:center;padding:4px 0;gap:0;}
#sidebar.collapsed .hw-meta,
#sidebar.collapsed .hw-live{opacity:0;width:0;pointer-events:none;}
#sidebar.collapsed .hw-row + .hw-row{box-shadow:none;}

/* Hide the whole block + its border when neither row is active (Chromium :has()). */
.hw-status:not(:has(.hw-row[style*="flex"])){display:none;}

/* ── Update-available badge (own block under the logo, not clipped by the logo box) ── */
.update-badge{
  display:none;
  margin:6px 12px 2px;
  padding:8px 11px;
  background:linear-gradient(135deg,var(--accent),var(--accent2));
  color:#fff;
  border-radius:8px;
  font-size:11px;font-weight:700;line-height:1.35;letter-spacing:0.01em;
  cursor:pointer;text-align:left;white-space:normal;word-break:break-word;
  box-shadow:0 2px 8px rgba(0,0,0,0.28);
  transition:filter 0.15s ease, transform 0.15s ease;
}
.update-badge:hover{filter:brightness(1.08);transform:translateY(-1px);}
#sidebar.collapsed .update-badge{display:none!important;}

/* ── Callsign (indicativ) shown next to an ISSI ── */
.callsign{
  display:inline-block;
  margin-left:6px;
  padding:1px 6px;
  border-radius:4px;
  background:var(--accent-soft,rgba(120,170,255,0.14));
  color:var(--accent2);
  font-family:var(--mono);font-size:11px;font-weight:700;letter-spacing:0.02em;
  vertical-align:middle;
}

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
  position:relative;z-index:50;   /* keep dropdown popovers above #content */
}
.topbar-title{
  font-size:15px;font-weight:700;color:var(--text);
  letter-spacing:-0.01em;
}
.topbar-sep{color:var(--border2);margin:0 2px;}
.topbar-sub{font-size:12px;color:var(--text3);font-family:var(--mono);}
.topbar-right{margin-left:auto;display:flex;align-items:center;gap:8px;}

/* (The old topbar SDR/power pill badges were relocated into the sidebar brand
   header as the .hw-status block — see the sidebar CSS above.) */

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

/* ── Readability eye button + Apple-style level popover ───────────────────── */
.eye-wrap{position:relative;display:flex;}
.eye-btn{
  width:30px;height:30px;display:flex;align-items:center;justify-content:center;
  background:transparent;border:1px solid var(--border);border-radius:6px;
  color:var(--text3);cursor:pointer;transition:all 0.15s;
}
.eye-btn svg{width:16px;height:16px;display:block;}
.eye-btn:hover{color:var(--text);border-color:var(--border2);background:var(--bg3);}
.eye-btn[aria-expanded="true"]{
  color:var(--accent);
  border-color:color-mix(in srgb,var(--accent) 45%,var(--border));
  background:color-mix(in srgb,var(--accent) 8%,transparent);
}

/* Popover: iOS-Settings list on a vibrancy surface — rounded, hairline rows, soft shadow */
.read-pop{
  position:absolute;top:calc(100% + 9px);right:0;
  width:248px;padding:6px;z-index:300;
  background:color-mix(in srgb,var(--bg2) 88%,transparent);
  -webkit-backdrop-filter:saturate(180%) blur(20px);
  backdrop-filter:saturate(180%) blur(20px);
  border:1px solid var(--border);border-radius:14px;
  box-shadow:
    0 18px 48px -16px rgba(20,30,50,0.34),
    0 4px 12px rgba(20,30,50,0.10),
    var(--hair);
  opacity:0;transform:translateY(-6px) scale(0.98);transform-origin:top right;
  pointer-events:none;
  transition:opacity 0.16s ease,transform 0.16s cubic-bezier(.2,.8,.2,1);
}
.read-pop.open{opacity:1;transform:translateY(0) scale(1);pointer-events:auto;}
.read-pop-title{
  font-family:var(--mono);font-size:9px;font-weight:700;letter-spacing:0.12em;
  text-transform:uppercase;color:var(--text3);padding:8px 10px 6px;
}
.read-opt{
  display:flex;align-items:center;gap:12px;width:100%;
  padding:9px 10px;border-radius:9px;
  background:transparent;border:none;cursor:pointer;text-align:left;color:var(--text);
  transition:background 0.12s;
}
.read-opt + .read-opt{box-shadow:inset 0 1px 0 var(--border);}     /* hairline separator */
.read-opt:hover{background:var(--bg3);}
.read-opt:hover + .read-opt{box-shadow:none;}                       /* hide line above hovered row */
/* Live "Aa" swatch — its font-size is the real base px for that level */
.read-aa{
  flex-shrink:0;width:34px;height:30px;border-radius:7px;
  background:var(--bg3);border:1px solid var(--border);
  display:flex;align-items:center;justify-content:center;
  font-family:var(--sans);font-weight:600;color:var(--text2);line-height:1;
}
.read-opt[data-size="s"] .read-aa{font-size:13px;}
.read-opt[data-size="m"] .read-aa{font-size:16px;}
.read-opt[data-size="h"] .read-aa{font-size:18px;font-weight:700;color:var(--text);}
.read-opt[data-size="u"] .read-aa{font-size:21px;font-weight:800;color:var(--text);}
.read-opt-text{flex:1;min-width:0;display:flex;flex-direction:column;}
.read-opt-name{font-family:var(--sans);font-size:13px;font-weight:600;letter-spacing:-0.01em;}
.read-opt-desc{font-size:11px;color:var(--text3);margin-top:1px;}
.read-check{
  flex-shrink:0;width:18px;height:18px;color:var(--accent);
  opacity:0;transform:scale(0.6);transition:opacity 0.12s,transform 0.12s;
}
.read-opt.active .read-check{opacity:1;transform:scale(1);}
.read-opt.active .read-opt-name{color:var(--accent);}

@media (max-width:700px){ .read-pop{width:220px;} }

/* ── Settings controls (Config / Telegram / WX tabs) — premium, consistent ──── */
/* Sub-label in a card header (e.g. WiFi saved-count). */
.card-sub{font-family:var(--mono);font-size:11px;color:var(--muted);letter-spacing:0.02em;}

/* iOS-style toggle switch. The real <input type=checkbox id=…> stays in the DOM
   (just visually replaced) so all .checked reads/writes keep working unchanged. */
.sw{position:relative;display:inline-block;width:44px;height:26px;flex-shrink:0;vertical-align:middle;}
.sw input{position:absolute;inset:0;width:100%;height:100%;opacity:0;margin:0;cursor:pointer;z-index:1;}
.sw i{
  position:absolute;inset:0;border-radius:999px;pointer-events:none;
  background:var(--bg4);border:1px solid var(--border2);
  transition:background .2s ease,border-color .2s ease;
}
.sw i::after{
  content:'';position:absolute;top:2px;left:2px;width:20px;height:20px;border-radius:50%;
  background:#fff;box-shadow:0 1px 3px rgba(20,30,50,.35);transition:transform .2s cubic-bezier(.2,.8,.2,1);
}
.sw input:checked ~ i{background:var(--accent);border-color:var(--accent);}
.sw input:checked ~ i::after{transform:translateX(18px);}
.sw input:focus-visible ~ i{box-shadow:0 0 0 3px color-mix(in srgb,var(--accent) 28%,transparent);}

/* Full settings row: label (with optional sub) on the left, switch on the right,
   hairline separators between rows. */
.sw-row{
  display:flex;align-items:center;justify-content:space-between;gap:16px;
  padding:11px 2px;cursor:pointer;user-select:none;
}
.sw-row + .sw-row{border-top:1px solid var(--border);}
.sw-text{font-size:14px;color:var(--text);font-weight:500;line-height:1.35;}
.sw-text .sw-sub{display:block;font-size:11.5px;color:var(--muted);margin-top:2px;font-weight:400;}

/* Native checkboxes that remain (e.g. inside modals) get the brand tint. */
input[type="checkbox"]:not(.sw input),
input[type="radio"]{accent-color:var(--accent);}

/* Help/intro text under a card title — used across the settings tabs. */
.help-text{color:var(--muted);font-size:13px;line-height:1.6;}

/* Recipient / ISSI chips (whitelist + telegram) — pill shape, brand-tinted. */
.id-chip{
  display:inline-flex;align-items:center;gap:7px;
  background:color-mix(in srgb,var(--accent2) 10%,transparent);
  border:1px solid color-mix(in srgb,var(--accent2) 30%,transparent);
  color:var(--text);border-radius:999px;padding:5px 6px 5px 12px;
  font-family:var(--mono);font-size:12.5px;font-weight:600;
}
.id-chip-x{
  display:inline-flex;align-items:center;justify-content:center;
  width:18px;height:18px;border-radius:50%;cursor:pointer;
  color:var(--danger);background:color-mix(in srgb,var(--danger) 12%,transparent);
  font-weight:700;line-height:1;transition:background .15s;
}
.id-chip-x:hover{background:color-mix(in srgb,var(--danger) 22%,transparent);}

/* The global .card-body is padding:0 (for table/grid cards). Settings + list tabs
   put text/controls straight in the body, so give those real breathing room —
   except the full-bleed code editor, which stays edge-to-edge. */
#page-telegram .card-body,
#page-config .card-body,
#page-dapnet .card-body,
#page-wifi .card-body{padding:16px 18px;}
#page-config .card-body:has(#config-editor){padding:0;}

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
/* Emergency call (ETSI call priority 15): solid danger fill + pulsing halo for high visibility. */
.badge-emergency{background:var(--danger);color:#fff;border-color:var(--danger);font-weight:700;letter-spacing:0.06em;animation:badge-emergency-pulse 1s ease-in-out infinite;}
@keyframes badge-emergency-pulse{0%,100%{box-shadow:0 0 0 0 rgba(255,77,109,0.55);}50%{box-shadow:0 0 0 4px rgba(255,77,109,0);}}
/* Active-calls table: tint an emergency call's row and mark it with a danger accent bar. */
tr.row-emergency td{background:rgba(255,77,109,0.07);}
tr.row-emergency td:first-child{box-shadow:inset 3px 0 0 var(--danger);}

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
.rf-canvas.tall{height:320px;}

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
/* Status colouring is driven by JS via these classes (now drives the value text;
   the meter itself is the shared .gauge with is-warn/is-danger). */
.rf-q-good .rf-qmetric-fill{background:var(--ok);}
.rf-q-warn .rf-qmetric-fill{background:var(--warn);}
.rf-q-bad  .rf-qmetric-fill{background:var(--danger);}
.rf-q-good .rf-qmetric-value{color:var(--ok);}
.rf-q-warn .rf-qmetric-value{color:var(--warn);}
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
.rf-hw-temp-state.nominal{color:var(--ok);}
.rf-hw-temp-state.warm{color:var(--warn);}
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

/* ── Empty state (legacy children; the .empty-state container itself is the
   v3 flex component defined in the design-system block below) ── */
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
  .logout-btn{width:30px;height:30px;font-size:13px;}

  #content{padding:8px;}

  /* Cards in a single column so each one is readable */
  .stat-grid{grid-template-columns:1fr;gap:10px;}

  /* TS visualizer: 2x2 instead of 1x4 so each block stays usable */
  .ts-grid{gap:10px;padding:10px 12px;}
  .ts-row{grid-template-columns:1fr 1fr;gap:8px;}
  .ts-carrier-head{flex-direction:column;align-items:flex-start;gap:4px;}

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
.ts-grid{display:flex;flex-direction:column;gap:12px;padding:16px 18px;}
.ts-carrier-group{display:flex;flex-direction:column;gap:8px;}
.ts-carrier-head{
  display:flex;align-items:baseline;justify-content:space-between;gap:10px;
  padding:0 2px;
}
.ts-carrier-title{
  font-family:var(--mono);font-size:10px;font-weight:700;
  letter-spacing:0.10em;color:var(--text2);text-transform:uppercase;
}
.ts-carrier-meta{
  font-family:var(--mono);font-size:10px;color:var(--text3);
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
}
.ts-row{display:grid;grid-template-columns:repeat(4,1fr);gap:10px;}
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
/* Emergency call (ETSI priority 15): danger ring + pulse, on top of the call/voice state. */
.ts-block.emergency{
  border-color:var(--danger);
  box-shadow:0 0 0 1px var(--danger),0 0 18px rgba(255,60,80,0.35);
  animation:ts-emergency-pulse 1.1s ease-in-out infinite;
}
.ts-block.emergency .ts-label,.ts-block.emergency .ts-num{color:var(--danger);}
@keyframes ts-emergency-pulse{0%,100%{box-shadow:0 0 0 1px var(--danger),0 0 10px rgba(255,60,80,0.2);}50%{box-shadow:0 0 0 1px var(--danger),0 0 22px rgba(255,60,80,0.5);}}

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

/* ════════════════════════════════════════════════════════════════════════
   Polish layer — additive motion + gloss on top of the base design (kept).
   Aesthetic only; layout unchanged. All motion is gated behind
   prefers-reduced-motion so it respects accessibility / low-power hosts.
   ════════════════════════════════════════════════════════════════════════ */

/* Glossy top sheen on the KPI cards — a faint specular highlight, no motion. */
.stat-card::after{
  content:'';position:absolute;inset:0;border-radius:inherit;pointer-events:none;
  background:linear-gradient(180deg, rgba(255,255,255,0.06), rgba(255,255,255,0) 34%);
  mix-blend-mode:soft-light;
}
.card{position:relative;}

/* Smooth focus ring on form inputs (Apple-style). */
.form-input{transition:border-color .15s ease, box-shadow .15s ease;}
.form-input:focus{
  outline:none;border-color:var(--accent2);
  box-shadow:0 0 0 3px color-mix(in srgb, var(--accent2) 22%, transparent);
}
/* Smooth table-row hover. */
tbody td{transition:background .12s ease;}

@media (prefers-reduced-motion: no-preference){
  /* Cards & KPI cards: gentle hover lift with a deeper, softer shadow. */
  .card,.stat-card{
    transition:transform .24s cubic-bezier(.2,.7,.3,1), box-shadow .24s ease, border-color .24s ease;
  }
  .card:hover,.stat-card:hover{
    transform:translateY(-2px);
    box-shadow:0 12px 30px -12px rgba(0,0,0,0.55), 0 2px 8px rgba(0,0,0,0.30);
    border-color:var(--border2);
  }
  /* Page enter: fade + rise. Fires only when a page becomes active (nav switch). */
  .page.active{animation:fsPageIn .34s cubic-bezier(.2,.7,.3,1) both;}
  @keyframes fsPageIn{from{opacity:0;transform:translateY(7px);}to{opacity:1;transform:none;}}
  /* Nav items: smoother hover/active transition. */
  .nav-item{transition:background .18s ease, color .18s ease, box-shadow .18s ease;}
  /* Buttons: tactile press + smoother hover. */
  .btn{transition:all .15s ease, transform .08s ease;}
  .btn:active{transform:scale(.96);}
  /* Update-available badge: gentle attention glow. */
  .update-badge{animation:fsGlow 2.4s ease-in-out infinite;}
  @keyframes fsGlow{
    0%,100%{box-shadow:0 2px 8px rgba(0,0,0,0.28);}
    50%{box-shadow:0 2px 8px rgba(0,0,0,0.28), 0 0 18px -2px var(--accent);}
  }
}

/* Refined, rounded scrollbar thumbs everywhere (no size change → no conflicts). */
::-webkit-scrollbar-thumb{border-radius:6px;}

/* ════════════════════════════════════════════════════════════════════════
   Ecosystem polish v2 — premium materials layer.
   Purely visual: depth, light, gradients & spacing refinements layered on top
   of the existing token system. NO structural/class/markup changes, so the
   shared mobile schema is untouched. Hues keep the teal/azure brand identity;
   only neutrals, elevation and "material" treatments are enriched.
   ════════════════════════════════════════════════════════════════════════ */
:root{
  --brand: linear-gradient(135deg, var(--accent) 0%, var(--accent2) 100%);
  /* --hair now lives in the v3 token block at :root (defined before first use). */
  --shadow-sm: 0 1px 2px rgba(0,0,0,0.45);
  --shadow-md: 0 10px 28px -14px rgba(0,0,0,0.65), 0 2px 6px rgba(0,0,0,0.32);
  --shadow-lg: 0 28px 64px -22px rgba(0,0,0,0.72), 0 6px 18px rgba(0,0,0,0.42);
  --glass: color-mix(in srgb, var(--bg2) 76%, transparent);
}

/* Ambient backdrop — faint brand glows bleed in from the corners behind the
   content, giving the shell a sense of depth without distracting from data. */
body{
  background:
    radial-gradient(1100px 560px at 82% -10%, color-mix(in srgb,var(--accent) 8%, transparent), transparent 60%),
    radial-gradient(1000px 680px at -6% 108%, color-mix(in srgb,var(--accent2) 8%, transparent), transparent 55%),
    var(--bg);
  background-attachment:fixed;
}
[data-theme="light"] body{
  background:
    radial-gradient(1100px 560px at 82% -10%, rgba(0,122,98,0.05), transparent 60%),
    radial-gradient(1000px 680px at -6% 108%, rgba(0,102,204,0.05), transparent 55%),
    var(--bg);
}

/* ── Sidebar: deeper, with a hairline inner highlight ── */
#sidebar{
  background:linear-gradient(180deg, color-mix(in srgb,var(--sidebar) 92%, var(--accent2)) 0%, var(--sidebar) 22%, var(--sidebar) 100%);
  box-shadow:1px 0 0 rgba(255,255,255,0.02), 8px 0 24px -16px rgba(0,0,0,0.6);
}
.sidebar-logo{padding-top:20px;padding-bottom:16px;}
.logo-text .logo-name{font-weight:800;letter-spacing:0.01em;}

/* ── Nav items: signature active treatment (left accent bar + soft wash) ── */
.nav-item{border-radius:8px;}
.nav-item.active{
  background:linear-gradient(90deg, color-mix(in srgb,var(--accent) 16%, transparent), color-mix(in srgb,var(--accent) 4%, transparent));
  border-color:color-mix(in srgb,var(--accent) 22%, transparent);
  box-shadow:inset 2px 0 0 var(--accent);
}
.nav-item.active .nav-icon{filter:drop-shadow(0 0 6px color-mix(in srgb,var(--accent) 60%, transparent));}
[data-theme="light"] .nav-item.active{box-shadow:inset 2px 0 0 var(--accent);}

/* ── Topbar: frosted glass with a hairline base highlight ── */
#topbar{
  background:var(--glass);
  -webkit-backdrop-filter:saturate(160%) blur(12px);
  backdrop-filter:saturate(160%) blur(12px);
  box-shadow:0 1px 0 rgba(255,255,255,0.03), 0 6px 18px -14px rgba(0,0,0,0.7);
}
.topbar-title{font-size:16px;font-weight:800;letter-spacing:-0.015em;}

/* ── Content rhythm ── */
#content{padding:24px;}
@media(max-width:700px){#content{padding:14px;}}

/* ── Stat cards: subtle vertical sheen, brand top-line fade, deeper lift ── */
.stat-grid{gap:16px;margin-bottom:22px;}
.stat-card{
  background:linear-gradient(180deg, var(--bg2) 0%, color-mix(in srgb,var(--bg2) 86%, #000) 100%);
  border:1px solid var(--border);
  border-radius:var(--r);
  box-shadow:var(--shadow-md), var(--hair);
  padding:17px 19px;
}
.stat-card::before{
  height:3px;
  background:linear-gradient(90deg, var(--accent-line,var(--accent)), color-mix(in srgb,var(--accent-line,var(--accent)) 0%, transparent) 92%);
  opacity:0.95;
}
.stat-value{font-size:30px;letter-spacing:-0.025em;}
.stat-icon{font-size:30px;opacity:0.06;}

/* ── Cards: refined elevation + header wash ── */
.card{
  border:1px solid var(--border);
  border-radius:var(--r);
  box-shadow:var(--shadow-md), var(--hair);
}
.card-head{
  background:linear-gradient(180deg, color-mix(in srgb,var(--bg3) 45%, transparent), transparent);
  padding-top:13px;padding-bottom:13px;
}
.card-title{color:var(--text2);}

/* ── Tables: zebra-free but with a soft sticky header and crisper hover ── */
thead th{
  background:color-mix(in srgb,var(--bg2) 92%, var(--accent2));
  border-bottom:1px solid var(--border2);
}
tbody tr{transition:background .12s ease;}
tbody tr:hover td{background:color-mix(in srgb,var(--bg3) 70%, transparent);}

/* ── Buttons: hairline highlight + brand primary ── */
.btn{border-radius:8px;box-shadow:var(--hair);}
.btn-primary{
  background:linear-gradient(180deg, color-mix(in srgb,var(--accent) 22%, transparent), color-mix(in srgb,var(--accent) 12%, transparent));
  border-color:color-mix(in srgb,var(--accent) 45%, transparent);
  color:var(--accent);
}
.btn-primary:hover{
  background:linear-gradient(180deg, color-mix(in srgb,var(--accent) 30%, transparent), color-mix(in srgb,var(--accent) 18%, transparent));
  border-color:var(--accent);
}

/* ── Badges: pill shape for a cleaner, app-like read ── */
.badge{border-radius:999px;padding:2px 9px;}

/* ── Pickers (theme/lang): unified segmented-control feel ── */
.theme-picker{box-shadow:var(--hair);}
.touch-btn,.theme-picker,.logout-btn,.sidebar-toggle{border-radius:8px;}

/* ── Footer status rows: a touch more contrast for the LEDs ── */
.conn-status-row,.brew-status-row{border-radius:8px;}

/* ── Deeper hover lift on cards (compose with existing motion layer) ── */
@media (prefers-reduced-motion: no-preference){
  .card:hover,.stat-card:hover{
    box-shadow:var(--shadow-lg), var(--hair);
  }
}

/* ── Scrollbar thumb: brand-tinted on hover ── */
::-webkit-scrollbar-thumb{background:var(--border2);}
::-webkit-scrollbar-thumb:hover{background:color-mix(in srgb,var(--accent) 40%, var(--border2));}

/* ════════════════════════════════════════════════════════════════════════
   DESIGN-SYSTEM v3 "INSTRUMENT" — reusable component library.
   Defined ONCE here so the Tabs phase can apply these classes across every tab.
   Everything maps to tokens (no hardcoded hex). This is the SINGLE source of
   truth; the Health-tab premium look is generalized into these classes.
   ════════════════════════════════════════════════════════════════════════ */

/* ── Section group label (Caption-2 above a card cluster) ── */
.section-label{
  font-size:12px;font-weight:600;letter-spacing:0.04em;text-transform:uppercase;
  color:var(--text3);margin:0 2px 10px;
}
.section-label + .section-label{margin-top:4px;}

/* ── Inline SVG icon sizing — any svg dropped into a slot reads as 1em-ish ── */
.nav-icon svg,.btn-icon svg,.pill-icon svg,.hero-ico svg,.chip svg,
.empty-ico svg,.banner-ico svg,.sheet-close svg,.section-act svg,.ico18 svg{
  display:block;width:100%;height:100%;
}
/* Generic 18px square icon holder for chrome buttons (hamburger/logout/toggle). */
.ico18{display:inline-flex;align-items:center;justify-content:center;width:18px;height:18px;color:inherit;}

/* ── Status pills — unified severity language (leading dot) ───────────────
   Variants drive from --ok/--warn/--danger/--info/--text3. Tinted fill +
   matching low-alpha border, mono tabular, 10/600. */
.pill{
  --pc:var(--text3);
  display:inline-flex;align-items:center;gap:6px;
  font-family:var(--mono);font-size:10px;font-weight:600;letter-spacing:0.02em;
  line-height:1;padding:4px 9px;border-radius:var(--r-pill);
  color:var(--pc);
  background:color-mix(in srgb,var(--pc) 13%,transparent);
  border:1px solid color-mix(in srgb,var(--pc) 32%,transparent);
  font-variant-numeric:tabular-nums;white-space:nowrap;vertical-align:middle;
}
.pill::before{
  content:"";flex-shrink:0;width:6px;height:6px;border-radius:50%;
  background:var(--pc);
}
.pill.no-dot::before{display:none;}
.pill-icon{flex-shrink:0;width:13px;height:13px;}
.pill-ok    {--pc:var(--ok);}
.pill-warn  {--pc:var(--warn);}
.pill-danger{--pc:var(--danger);}
.pill-info  {--pc:var(--accent2);}
.pill-idle  {--pc:var(--text3);}

/* ── Hero status banner (generalized from the Health hero) ── */
.hero{
  display:flex;align-items:center;gap:16px;
  padding:18px 20px;margin-bottom:22px;
  background:var(--bg2);border:1px solid var(--border);
  border-radius:var(--r-card);box-shadow:var(--elev-1);
}
.hero-dot{
  --pc:var(--text3);
  width:10px;height:10px;flex:0 0 auto;border-radius:50%;
  background:var(--pc);
  box-shadow:0 0 0 4px color-mix(in srgb,var(--pc) 16%,transparent);
}
.hero-dot.is-ok{--pc:var(--ok);}
.hero-dot.is-warn{--pc:var(--warn);}
.hero-dot.is-danger{--pc:var(--danger);}
.hero-dot.is-idle{--pc:var(--text3);}
.hero-main{flex:1;min-width:0;}
.hero-title{font-size:15px;font-weight:600;color:var(--text);letter-spacing:-0.01em;}
.hero-sub{font-size:12px;font-weight:400;color:var(--text2);margin-top:3px;}
.hero-metrics{display:flex;align-items:center;gap:22px;flex-shrink:0;}
.hero-metric{display:flex;flex-direction:column;gap:2px;text-align:right;}
.hero-metric-label{font-size:11px;font-weight:600;letter-spacing:0.06em;text-transform:uppercase;color:var(--text3);}
.hero-metric-value{font-family:var(--mono);font-size:14px;font-weight:600;color:var(--text);font-variant-numeric:tabular-nums;}

/* ── Horizontal gauge — track + fill + trailing tabular value ── */
.gauge{display:flex;align-items:center;gap:10px;min-width:0;}
.gauge-track{
  flex:1;height:4px;min-width:40px;border-radius:var(--r-pill);
  background:var(--bg4);overflow:hidden;
}
.gauge-fill{
  height:100%;width:0%;border-radius:var(--r-pill);
  background:var(--ok);
  transition:width .35s ease, background .25s ease;
}
.gauge.is-warn   .gauge-fill{background:var(--warn);}
.gauge.is-danger .gauge-fill{background:var(--danger);}
.gauge.is-info   .gauge-fill{background:var(--accent2);}
.gauge.is-idle   .gauge-fill{background:var(--text3);}
.gauge-value{
  font-family:var(--mono);font-size:12px;font-weight:600;color:var(--text2);
  font-variant-numeric:tabular-nums;flex-shrink:0;min-width:42px;text-align:right;
}

/* ── macOS inset list (.group-list) + .field rows ── */
.group-list{
  display:flex;flex-direction:column;
  background:var(--bg2);border:1px solid var(--border);
  border-radius:var(--r-card);overflow:hidden;
}
.field{
  display:flex;align-items:center;gap:14px;min-height:44px;
  padding:10px 16px;position:relative;
}
.field + .field::before{
  content:"";position:absolute;left:16px;right:0;top:0;height:1px;
  background:var(--sep);
}
.field-label{
  flex:0 0 auto;font-size:13px;font-weight:400;color:var(--text);
}
.field-control{
  margin-left:auto;display:flex;align-items:center;gap:8px;
  font-size:13px;font-weight:500;color:var(--text2);
  font-variant-numeric:tabular-nums;text-align:right;min-width:0;
}
.field-hint{
  flex-basis:100%;font-size:11px;font-weight:400;color:var(--text3);
  margin-top:2px;
}
.field-status{
  display:inline-flex;align-items:center;gap:5px;
  font-size:11px;font-weight:500;color:var(--text3);
  opacity:0;transition:opacity .2s ease;
}
.field-status.show{opacity:1;}
.field-status.ok{color:var(--ok);}
.field-status.err{color:var(--danger);}
.field-status svg{width:13px;height:13px;}

/* ── Button leading-icon slot (glyphs split out of i18n strings) ── */
.btn-icon{
  display:inline-flex;align-items:center;justify-content:center;
  width:15px;height:15px;flex-shrink:0;margin-right:7px;margin-left:-2px;
  vertical-align:-2px;
}
.btn .btn-icon{vertical-align:middle;}
/* Destructive action group, separated from benign Save by a hairline. */
.btn-group{display:inline-flex;align-items:center;gap:8px;}
.btn-group.danger-group{
  padding-left:12px;margin-left:4px;
  border-left:1px solid var(--sep);
}

/* ── Calm banners (replace inline #fallback-banner / #emergency-banner) ── */
.banner{
  display:flex;align-items:center;gap:12px;flex-shrink:0;
  padding:11px 18px;font-size:13px;font-weight:600;
  color:var(--text);
  background:color-mix(in srgb,var(--accent2) 12%,var(--bg2));
  border-bottom:1px solid color-mix(in srgb,var(--accent2) 30%,transparent);
}
.banner-ico{width:18px;height:18px;flex-shrink:0;color:var(--accent2);}
.banner-body{flex:1;min-width:0;}
.banner-sub{font-size:11px;font-weight:400;color:var(--text2);margin-top:2px;}
.banner-act{margin-left:auto;}
.banner-warn{
  background:color-mix(in srgb,var(--warn) 13%,var(--bg2));
  border-bottom-color:color-mix(in srgb,var(--warn) 32%,transparent);
}
.banner-warn .banner-ico{color:var(--warn);}
.banner-danger{
  background:color-mix(in srgb,var(--danger) 13%,var(--bg2));
  border-bottom-color:color-mix(in srgb,var(--danger) 34%,transparent);
}
.banner-danger .banner-ico{color:var(--danger);}
/* Steady danger dot for emergency — soft breathe, never a harsh flash. */
.banner-danger .banner-dot{
  width:8px;height:8px;border-radius:50%;background:var(--danger);flex-shrink:0;
  animation:fs-breathe 2.5s ease-in-out infinite;
}
@keyframes fs-breathe{0%,100%{opacity:1;}50%{opacity:.45;}}

/* ── Empty state (one component for the duplicated stubs) ──
   v3 flex layout; keeps the legacy .empty-icon/.empty-text children working
   (centered column) while the Tabs phase migrates them to .empty-ico/.empty-msg. */
.empty-state{
  display:flex;flex-direction:column;align-items:center;justify-content:center;
  gap:10px;padding:40px 24px;text-align:center;color:var(--text3);
}
.empty-ico{width:34px;height:34px;color:var(--text3);opacity:.7;}
.empty-msg{font-size:13px;font-weight:500;color:var(--text2);}
.empty-sub{font-size:12px;font-weight:400;color:var(--text3);max-width:340px;}

/* ── Unified sheet/modal (collapses .modal-overlay + .wifi-modal) ── */
.sheet-overlay{
  position:fixed;inset:0;z-index:1000;
  display:none;align-items:center;justify-content:center;padding:24px;
  background:rgba(0,0,0,.42);
  -webkit-backdrop-filter:blur(24px) saturate(1.3);
  backdrop-filter:blur(24px) saturate(1.3);
}
.sheet-overlay.open{display:flex;}
.sheet{
  width:100%;max-width:460px;max-height:88vh;overflow:auto;
  background:var(--mat);border:1px solid var(--border2);
  border-radius:var(--r-card);box-shadow:var(--shadow-lg),var(--hair);
  -webkit-backdrop-filter:blur(24px) saturate(1.3);
  backdrop-filter:blur(24px) saturate(1.3);
}
.sheet-head{
  display:flex;align-items:center;gap:12px;
  padding:16px 18px;border-bottom:1px solid var(--sep);
}
.sheet-title{font-size:15px;font-weight:600;color:var(--text);flex:1;letter-spacing:-0.01em;}
.sheet-close{
  width:28px;height:28px;flex-shrink:0;display:flex;align-items:center;justify-content:center;
  border-radius:var(--r-ctrl);border:1px solid transparent;
  background:transparent;color:var(--text3);cursor:pointer;transition:all .15s;
}
.sheet-close:hover{background:var(--bg3);color:var(--text);}
.sheet-close svg{width:16px;height:16px;}
.sheet-body{padding:18px;}

/* ── Ghost SVG stat-icon: the .stat-icon slot now hosts a faint inline SVG
   (was an emoji glyph). Auto-themes via currentColor, sits at low opacity. ── */
.stat-icon svg{display:block;width:30px;height:30px;color:var(--text);}
.stat-icon:has(svg){font-size:0;line-height:0;}
/* Text-valued stat cards (RF / Network / BREW) — smaller value, state tint
   via ONE class instead of inline font-size + JS color hacks. */
.stat-value.is-text{font-size:18px;letter-spacing:-0.01em;}
.stat-card.is-ok    .stat-value.is-text{color:var(--ok);}
.stat-card.is-ok::before    {--accent-line:var(--ok);}
.stat-card.is-info  .stat-value.is-text{color:var(--accent2);}
.stat-card.is-info::before  {--accent-line:var(--accent2);}
.stat-card.is-warn  .stat-value.is-text{color:var(--warn);}
.stat-card.is-warn::before  {--accent-line:var(--warn);}
.stat-card.is-danger .stat-value.is-text{color:var(--danger);}
.stat-card.is-danger::before{--accent-line:var(--danger);}
.stat-card.is-idle  .stat-value.is-text{color:var(--text3);}
.stat-card.is-idle::before  {--accent-line:var(--text3);}

/* Tabular numeric cell + muted placeholder for tables (instrument feel). */
.num{font-family:var(--mono);font-variant-numeric:tabular-nums;font-size:12px;color:var(--text2);}
.num.accent{color:var(--accent2);font-weight:600;}
.muted{color:var(--text3);}

/* Filled selection-triangle marker (▶ replacement) inside a TG pill. */
.tg-marker{display:inline-flex;align-items:center;width:9px;height:9px;margin-right:2px;}
.tg-marker svg{width:100%;height:100%;display:block;}

/* Soften the emergency table badge: steady fill + a calm 2.5s breathe
   (no harsh expanding ring). Matches the emergency BANNER's fs-breathe. */
.badge-emergency{animation:fs-breathe 2.5s ease-in-out infinite;}

/* ── Numbered steps list (Telegram setup howto) ── */
.steps{display:flex;flex-direction:column;gap:0;counter-reset:fs-step;}
.step{
  display:flex;align-items:flex-start;gap:13px;padding:11px 2px;position:relative;
  font-size:13px;color:var(--text);line-height:1.55;
}
.step + .step::before{
  content:"";position:absolute;left:32px;right:0;top:0;height:1px;background:var(--sep);
}
.step-num{
  counter-increment:fs-step;flex:0 0 auto;
  width:22px;height:22px;border-radius:50%;
  display:inline-flex;align-items:center;justify-content:center;
  font-family:var(--mono);font-size:11px;font-weight:700;font-variant-numeric:tabular-nums;
  color:var(--accent);
  background:color-mix(in srgb,var(--accent) 13%,transparent);
  border:1px solid color-mix(in srgb,var(--accent) 34%,transparent);
}
.step-num::before{content:counter(fs-step);}
.step-body{flex:1;min-width:0;padding-top:1px;}

/* ── Styled terminal block (SoapySDR probe dump, etc.) ── */
.terminal{
  margin:0;padding:13px 15px;
  background:var(--bg);border:1px solid var(--border);border-radius:var(--r-ctrl);
  box-shadow:var(--hair);
  font-family:var(--mono);font-size:11px;line-height:1.6;
  color:var(--text2);white-space:pre-wrap;word-break:break-all;
  max-height:340px;overflow:auto;font-variant-numeric:tabular-nums;
}

/* ── Big-Sur inset nav selection pill + SVG nav-icon slot ──────────────────
   Re-skins the existing .nav-item.active (overriding the polish v2 left-bar)
   to the System-Settings inset pill: accent-tinted fill + soft radius.
   The .nav-icon slot becomes an 18px square SVG holder (was an emoji glyph). */
.nav-icon{
  width:18px;height:18px;font-size:0;
  display:inline-flex;align-items:center;justify-content:center;
  flex-shrink:0;color:inherit;text-align:center;
}
.nav-item.active{
  background:color-mix(in srgb,var(--accent) 12%,transparent);
  border-color:transparent;
  box-shadow:none;
  color:var(--accent);
}
[data-theme="light"] .nav-item.active{
  background:color-mix(in srgb,var(--accent) 10%,transparent);
  border-color:transparent;box-shadow:none;
}
/* Keep the signature accent glow on the active icon (per nav spec). */
.nav-item.active .nav-icon{filter:drop-shadow(0 0 6px color-mix(in srgb,var(--accent) 55%,transparent));}

/* ── Header status chips (BS / Brew / Emergency) — calm .pill in the topbar ── */
.topbar-chips{display:flex;align-items:center;gap:8px;}
@media(max-width:760px){.topbar-chips{display:none;}}

/* ════ TETRA BTS Details card ════ */
.bts-grid{
  display:grid;grid-template-columns:repeat(auto-fit,minmax(140px,1fr));
  gap:10px;padding:16px 18px;
}
.bts-tile{
  background:linear-gradient(180deg, var(--bg), color-mix(in srgb,var(--bg) 82%, #000));
  border:1px solid var(--border);border-radius:9px;
  padding:11px 13px;display:flex;flex-direction:column;gap:6px;min-width:0;
  box-shadow:var(--hair);
}
.bts-tile-label{
  font-family:var(--mono);font-size:9px;font-weight:600;letter-spacing:0.09em;
  text-transform:uppercase;color:var(--text3);white-space:nowrap;
  overflow:hidden;text-overflow:ellipsis;
}
.bts-tile-value{
  font-family:var(--mono);font-size:15px;font-weight:700;color:var(--text);
  letter-spacing:-0.01em;min-width:0;overflow-wrap:anywhere;
}
.bts-tile-value.tx{color:var(--accent);}
.bts-tile-value.rx{color:var(--accent2);}
/* Header status chips (Neighbor Cell / HangTime) */
.bts-chip{
  display:inline-flex;align-items:center;gap:6px;
  font-family:var(--mono);font-size:10px;font-weight:700;letter-spacing:0.04em;
  padding:5px 11px;border-radius:999px;border:1px solid var(--border2);
  background:var(--bg3);color:var(--text2);white-space:nowrap;box-shadow:var(--hair);
}
.bts-chip svg{flex-shrink:0;}
.bts-chip.on{color:var(--accent);background:color-mix(in srgb,var(--accent) 13%,transparent);border-color:color-mix(in srgb,var(--accent) 40%,transparent);}
.bts-chip.off{color:var(--text3);background:var(--bg3);border-color:var(--border);}
.bts-chip.time{color:var(--accent2);background:color-mix(in srgb,var(--accent2) 13%,transparent);border-color:color-mix(in srgb,var(--accent2) 38%,transparent);}
.bts-access-bar{
  display:flex;align-items:center;justify-content:space-between;gap:12px;
  margin:0 18px 16px;padding:13px 16px;
  background:linear-gradient(180deg, var(--bg), color-mix(in srgb,var(--bg) 80%, #000));
  border:1px solid var(--border);border-radius:10px;box-shadow:var(--hair);
}
.bts-access-info{display:flex;align-items:center;gap:13px;min-width:0;}
.bts-access-icon{
  width:38px;height:38px;flex-shrink:0;border-radius:10px;
  display:flex;align-items:center;justify-content:center;
  background:color-mix(in srgb,var(--accent2) 12%, transparent);
  border:1px solid color-mix(in srgb,var(--accent2) 30%, transparent);
  color:var(--accent2);
}
.bts-access-title{font-size:12.5px;font-weight:700;color:var(--text);letter-spacing:0.01em;}
.bts-access-sub{font-family:var(--mono);font-size:10px;color:var(--text3);margin-top:2px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;}
.bts-access{
  font-family:var(--mono);font-size:11px;font-weight:800;letter-spacing:0.1em;
  padding:7px 16px;border-radius:999px;border:1px solid;white-space:nowrap;flex-shrink:0;
  background:var(--bg3);color:var(--text3);border-color:var(--border);
}
.bts-access.open{
  color:var(--accent);
  background:color-mix(in srgb,var(--accent) 13%, transparent);
  border-color:color-mix(in srgb,var(--accent) 42%, transparent);
}
.bts-access.restricted{
  color:var(--warn);
  background:color-mix(in srgb,var(--warn) 13%, transparent);
  border-color:color-mix(in srgb,var(--warn) 42%, transparent);
}
@media(max-width:500px){
  .bts-grid{grid-template-columns:1fr 1fr;gap:8px;padding:12px;}
  .bts-tile-value{font-size:13px;}
  .bts-access-bar{margin:0 12px 12px;}
}

/* ════ Monitor tables — consistent column alignment ════
   Headers were left-aligned while badges / status / signal sat centred in the cell,
   so nothing lined up vertically. Rule: the primary identifier column stays left;
   every other column is centred so each value sits directly under its header. */
#page-stations table th, #page-stations table td,
#page-calls table th,    #page-calls table td,
#page-lastheard table th, #page-lastheard table td{
  text-align:center; vertical-align:middle;
}
#page-stations table th:first-child, #page-stations table td:first-child,
#page-calls table th:first-child,    #page-calls table td:first-child,
#page-lastheard table th:first-child, #page-lastheard table td:first-child{
  text-align:left;
}
/* SDS Log: left-aligned, top-aligned rows; message wraps, timestamp stays on one line. */
#page-sdslog table th, #page-sdslog table td{ text-align:left; vertical-align:top; }
#page-sdslog .sds-time{ white-space:nowrap; color:var(--text2); font-variant-numeric:tabular-nums; }
#page-sdslog .sds-msg{ word-break:break-word; max-width:560px; }
.sds-empty{ color:var(--text3); font-style:italic; }
.sds-map-link{ color:var(--accent2); font-weight:700; text-decoration:none; }
.sds-map-link:hover{ text-decoration:underline; }
/* Signal cell: centre the bar+value as a unit, and keep the dBFS reading on one line
   (it was wrapping to two, which read as "toy-like"). */
#page-stations .rssi-bar{ justify-content:center; }
.rssi-val{ width:auto; min-width:62px; white-space:nowrap; }

/* ════ Timeslot visualizer — live identity + motion ════ */
/* Per-timeslot call timer, top-right corner. Colour-matched to the call state. */
.ts-timer{
  position:absolute;top:7px;right:9px;
  font-family:var(--mono);font-size:9px;font-weight:700;letter-spacing:0.04em;
  color:var(--text3);font-variant-numeric:tabular-nums;pointer-events:none;
}
.ts-block.call .ts-timer{color:var(--warn);}
.ts-block.voice .ts-timer{color:var(--danger);}
/* GSSI line reads a touch larger; ISSI/callsign line stays monospace + tabular. */
.ts-label{font-size:11px;}
.ts-sub{font-family:var(--mono);font-variant-numeric:tabular-nums;}

@media (prefers-reduced-motion: no-preference){
  /* Idle dots gently "breathe" so the panel feels alive when quiet. Active dots
     (control / call / voice) stay perfectly still so the ripple reads as concentric. */
  .ts-block:not(.mcch):not(.call):not(.voice) .ts-led{
    animation:tsBreathe 3.2s ease-in-out infinite;will-change:transform,opacity;
  }
  @keyframes tsBreathe{0%,100%{transform:scale(1);opacity:.5;}50%{transform:scale(1.25);opacity:.9;}}

  /* Active timeslots emit an expanding "radar" ripple from the LED — a calmer,
     more signal-like cue than a flat colour change. The ring is centred via
     translate(-50%,-50%) preserved across the whole keyframe, so it stays exactly
     concentric with the dot regardless of scale. currentColor matches the state. */
  .ts-led{position:relative;}
  .ts-led::after{
    content:'';position:absolute;top:50%;left:50%;width:100%;height:100%;
    box-sizing:border-box;  /* the global *{} reset doesn't reach ::after — set it here so
                               width:100% + border + translate(-50%) all use the same 10px box */
    border-radius:50%;border:1.5px solid currentColor;
    transform:translate(-50%,-50%) scale(1);transform-origin:center;
    opacity:0;pointer-events:none;
  }
  .ts-block.mcch  .ts-led{color:var(--accent2);}
  .ts-block.call  .ts-led{color:var(--warn);}
  .ts-block.voice .ts-led{color:var(--danger);}
  .ts-block.mcch  .ts-led::after{animation:tsRipple 2.6s ease-out infinite;}
  .ts-block.call  .ts-led::after{animation:tsRipple 1.6s ease-out infinite;}
  .ts-block.voice .ts-led::after{animation:tsRipple 0.9s ease-out infinite;}
  @keyframes tsRipple{
    0%{opacity:.6;transform:translate(-50%,-50%) scale(1);}
    100%{opacity:0;transform:translate(-50%,-50%) scale(3.2);}
  }
}

/* ════════════════════════════════════════════════════════════════════════
   Premium light/grey default (FH user feedback) — bigger high-contrast type,
   a theme-integrated (light) sidebar, tighter sections, and a subtle texture.
   Light overrides are scoped to [data-theme="light"]; the density/font bumps
   apply on desktop/tablet only so the phone layout keeps its tuned sizes.
   ════════════════════════════════════════════════════════════════════════ */

/* Softer elevation for light surfaces (the base shadows are tuned for dark). */
[data-theme="light"]{
  --shadow-sm:0 1px 2px rgba(30,45,70,0.07);
  --shadow-md:0 6px 18px -10px rgba(30,45,70,0.16), 0 2px 5px rgba(30,45,70,0.06);
  --shadow-lg:0 20px 46px -18px rgba(30,45,70,0.22), 0 6px 14px rgba(30,45,70,0.10);
}

/* Theme-integrated sidebar: the rail now follows the theme instead of staying
   dark navy (dark text on a dark rail was the "bad contrast" complaint). */
[data-theme="light"] #sidebar{
  background:var(--sidebar);
  box-shadow:1px 0 0 var(--sidebar-border), 6px 0 22px -18px rgba(30,45,70,0.22);
}
[data-theme="light"] .logo-text .logo-sub,
[data-theme="light"] .sidebar-copyright .cr-line{color:var(--text3);}

/* Flatten the dark-oriented (#000-mixed) gradients to clean light surfaces. */
[data-theme="light"] .stat-card{background:var(--bg2);}
[data-theme="light"] .bts-tile,
[data-theme="light"] .bts-access-bar,
[data-theme="light"] .bts-chip{background:var(--bg);}
[data-theme="light"] .card-head{background:linear-gradient(180deg,var(--bg3),transparent);}

/* Premium texture: a faint dot-grid + soft brand glows show through the gutters. */
[data-theme="light"] body{
  background:
    radial-gradient(circle at 1px 1px, rgba(30,45,70,0.05) 1px, transparent 0) 0 0/22px 22px,
    radial-gradient(1100px 560px at 84% -12%, rgba(0,135,106,0.06), transparent 60%),
    radial-gradient(1000px 680px at -8% 110%, rgba(21,101,192,0.06), transparent 55%),
    var(--bg);
}

/* Readability + density — desktop/tablet only. Type scales with --ts (eye control). */
@media (min-width:701px){
  body{font-size:calc(15px * var(--ts));}

  #content{padding:18px;}
  .stat-grid{gap:12px;margin-bottom:14px;}
  .stat-card{padding:13px 16px;}
  .stat-value{font-size:calc(26px * var(--ts));}
  .stat-label{font-size:calc(12px * var(--ts));font-weight:var(--wt-quiet);}
  .stat-sub{font-size:calc(11.5px * var(--ts));}
  .card{margin-bottom:12px;}
  .card-head{padding-top:11px;padding-bottom:11px;}
  .card-title{font-size:calc(13px * var(--ts));letter-spacing:0.07em;font-weight:var(--wt-quiet);}

  .nav-item{font-size:calc(14px * var(--ts));}
  .nav-section-label{font-size:calc(10px * var(--ts));font-weight:var(--wt-quiet);}

  thead th{font-size:calc(11px * var(--ts));font-weight:var(--wt-quiet);}
  tbody td{font-size:calc(14px * var(--ts));padding:9px 14px;}
  .badge{font-size:calc(10.5px * var(--ts));}
  .btn,.btn-sm{font-size:calc(11.5px * var(--ts));}

  .bts-grid{gap:9px;padding:13px 16px;}
  .bts-tile-label{font-size:calc(10px * var(--ts));font-weight:var(--wt-quiet);}
  .bts-tile-value{font-size:calc(17px * var(--ts));}
  .bts-access-bar{margin:0 16px 13px;padding:11px 14px;}
  .bts-access-title{font-size:calc(13px * var(--ts));}

  .ts-grid{padding:13px 16px;gap:9px;}

  .info-key{font-size:calc(12px * var(--ts));font-weight:var(--wt-quiet);}
  .info-val{font-size:calc(13px * var(--ts));}

  .rf-metric-label{font-size:calc(10px * var(--ts));font-weight:var(--wt-quiet);}
  .rf-metric-value{font-size:calc(16px * var(--ts));}
  .rf-qmetric-label{font-size:calc(10px * var(--ts));}
  .rf-qmetric-value{font-size:calc(15px * var(--ts));}

  .log-wrap{font-size:calc(12px * var(--ts));line-height:1.75;}
  .topbar-title{font-size:calc(17px * var(--ts));}

  /* sidebar hardware-status readout (Piece B) scales with the same knob */
  .hw-val{font-size:calc(11px * var(--ts));}
}
/* Clamp the scale on phones so Ultra never blows out the <=700px layout. */
@media (max-width:700px){
  html[data-uisize="h"]{ --ts:1.16; }
  html[data-uisize="u"]{ --ts:1.28; }
}

/* ── Premium health / integration components (Apple-style) ───────────────────
   Theme-aware via tokens + color-mix. Status hues: ok=--ok, warn=--warn,
   bad=--danger; blue/purple are fixed icon accents for domain variety.
   Used by the Health page, the SDR Hardware-Health card and the
   Asterisk / DAPNET / GeoAlarm pages so they all match. */
.h-wrap{max-width:1100px;}

/* Hero */
.h-hero{
  display:flex;align-items:center;gap:18px;
  background:var(--bg2);border:1px solid var(--border);border-radius:18px;
  padding:18px 22px;margin-bottom:6px;box-shadow:var(--card-shadow);
}
.h-ring{
  flex:0 0 auto;width:52px;height:52px;border-radius:50%;position:relative;
  display:flex;align-items:center;justify-content:center;
  background:color-mix(in srgb,var(--ok) 14%,transparent);
  box-shadow:inset 0 0 0 1px color-mix(in srgb,var(--ok) 55%,transparent),
             0 0 18px -2px color-mix(in srgb,var(--ok) 45%,transparent);
  color:var(--ok);transition:background .25s,box-shadow .25s,color .25s;
}
.h-ring svg{width:26px;height:26px;display:block;}
.h-ring.warn{background:color-mix(in srgb,var(--warn) 14%,transparent);color:var(--warn);
  box-shadow:inset 0 0 0 1px color-mix(in srgb,var(--warn) 55%,transparent),0 0 18px -2px color-mix(in srgb,var(--warn) 45%,transparent);}
.h-ring.bad{background:color-mix(in srgb,var(--danger) 14%,transparent);color:var(--danger);
  box-shadow:inset 0 0 0 1px color-mix(in srgb,var(--danger) 55%,transparent),0 0 18px -2px color-mix(in srgb,var(--danger) 45%,transparent);}
.h-hero-txt{flex:1;min-width:0;display:flex;flex-direction:column;justify-content:center;}
.h-hero-title{font-size:21px;font-weight:650;letter-spacing:-.01em;color:var(--text);line-height:1.2;}
.h-hero-sub{font-size:14px;color:var(--text2);margin-top:3px;line-height:1.4;}
.h-hero-meta{flex:0 0 auto;text-align:right;display:flex;flex-direction:column;justify-content:center;gap:2px;}
.h-hero-meta .hm-val{font-size:15px;font-weight:600;color:var(--text);font-variant-numeric:tabular-nums;}
.h-hero-meta .hm-sub{font-size:12px;color:var(--text3);}

/* Section label */
.h-sec{font-size:12px;font-weight:600;letter-spacing:.04em;text-transform:uppercase;color:var(--text3);margin:22px 4px 11px;}

/* Grid of cards */
.h-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(330px,1fr));gap:13px;}

/* Card */
.h-card{
  display:flex;gap:13px;align-items:flex-start;
  background:var(--bg2);border:1px solid var(--border);border-radius:16px;
  padding:15px 16px;box-shadow:var(--card-shadow);
}
.h-ico{
  flex:0 0 auto;width:36px;height:36px;border-radius:10px;
  display:flex;align-items:center;justify-content:center;
  background:color-mix(in srgb,var(--accent) 14%,transparent);color:var(--accent);
}
.h-ico svg{width:18px;height:18px;display:block;}
.h-ico.blue{background:color-mix(in srgb,#5ac8fa 16%,transparent);color:#5ac8fa;}
.h-ico.purple{background:color-mix(in srgb,#bf8cff 16%,transparent);color:#bf8cff;}
.h-ico.warn{background:color-mix(in srgb,var(--warn) 16%,transparent);color:var(--warn);}
.h-ico.ok{background:color-mix(in srgb,var(--ok) 16%,transparent);color:var(--ok);}
.h-ico.bad{background:color-mix(in srgb,var(--danger) 16%,transparent);color:var(--danger);}
.h-col{flex:1;min-width:0;display:flex;flex-direction:column;}
.h-head{display:flex;align-items:center;gap:8px;min-height:36px;}
.h-ttl{font-size:15px;font-weight:600;letter-spacing:-.01em;color:var(--text);flex:1;min-width:0;}
.h-card.compact .h-ttl{font-size:14px;}
.h-pill{
  flex:0 0 auto;font-size:11px;font-weight:700;letter-spacing:.03em;
  border-radius:7px;padding:2px 8px;text-transform:uppercase;white-space:nowrap;
}
.h-pill.ok{background:color-mix(in srgb,var(--ok) 15%,transparent);color:var(--ok);}
.h-pill.warn{background:color-mix(in srgb,var(--warn) 16%,transparent);color:var(--warn);}
.h-pill.bad{background:color-mix(in srgb,var(--danger) 16%,transparent);color:var(--danger);}
.h-det{font-size:13px;color:var(--text2);margin-top:6px;line-height:1.45;font-variant-numeric:tabular-nums;}
.h-det b{color:var(--text);font-weight:600;}
.h-det .h-status-lbl{color:var(--text3);}
.h-todo{
  border-top:1px solid var(--border);margin-top:11px;padding-top:10px;
  font-size:12.5px;color:var(--text2);line-height:1.5;
}
.h-todo .h-todo-h{font-weight:600;color:var(--text);}
.h-todo b{color:var(--warn);font-weight:600;}
.h-todo ul{margin:6px 0 0 16px;padding:0;}
.h-todo li{margin-top:3px;}

/* Hardware metric strip (gauge + value) */
.h-metricstrip{display:grid;grid-template-columns:repeat(auto-fill,minmax(220px,1fr));gap:13px;}
.h-metric{
  display:flex;align-items:center;gap:14px;
  background:var(--bg2);border:1px solid var(--border);border-radius:16px;
  padding:14px 16px;box-shadow:var(--card-shadow);
}
.h-gauge{
  flex:0 0 auto;width:48px;height:48px;border-radius:50%;position:relative;
  display:flex;align-items:center;justify-content:center;
  background:conic-gradient(var(--g-col,var(--ok)) calc(var(--g-pct,0)*1%),var(--border2) 0);
}
.h-gauge::before{
  content:"";position:absolute;width:37px;height:37px;border-radius:50%;background:var(--bg2);
}
.h-gauge .h-gauge-n{position:relative;font-size:12px;font-weight:700;color:var(--text);font-variant-numeric:tabular-nums;}
.h-mcol{display:flex;flex-direction:column;justify-content:center;min-width:0;}
.h-mcol .h-mval{font-size:19px;font-weight:650;color:var(--text);font-variant-numeric:tabular-nums;line-height:1.1;}
.h-mcol .h-mlbl{font-size:12px;color:var(--text3);margin-top:2px;}
.h-mcol .h-mval.ok{color:var(--ok);}
.h-mcol .h-mval.warn{color:var(--warn);}
.h-mcol .h-mval.bad{color:var(--danger);}

/* Legend / note row under the health page */
.h-note{margin-top:18px;font-size:12px;color:var(--text2);line-height:1.6;}
.h-note b.ok{color:var(--ok);}
.h-note b.warn{color:var(--warn);}
.h-note b.bad{color:var(--danger);}

/* Premium form layout (asterisk/dapnet/geoalarm) — replaces repeated inline styles */
.h-form{display:grid;grid-template-columns:repeat(auto-fit,minmax(190px,1fr));gap:10px;align-items:center;}
.h-form.wide{grid-template-columns:repeat(auto-fit,minmax(260px,1fr));align-items:stretch;}
.h-form-pair{display:grid;grid-template-columns:130px 1fr;gap:10px;align-items:center;}
.h-flabel{color:var(--muted);font-size:13px;}
.h-flabel.top{align-self:flex-start;padding-top:8px;}
.h-finline{display:flex;align-items:center;gap:10px;}
.h-finline .h-flabel-sm{color:var(--muted);font-size:12px;}
.h-fopts{display:flex;gap:14px;flex-wrap:wrap;}
.h-fopt{display:flex;align-items:center;gap:8px;color:var(--muted);font-size:12px;}
</style>
</head>
<body>

<!-- Mobile overlay -->
<div id="mobile-overlay" onclick="closeMobileSidebar()"></div>

<!-- ── Sidebar ── -->
<nav id="sidebar">
  <div class="sidebar-logo">
    <div class="logo-row">
      <div class="logo-icon">FS</div>
      <div class="logo-text">
        <div class="logo-name">FlowStation</div>
        <div class="logo-sub">{{STACK_VERSION}}</div>
      </div>
    </div>
    <!-- Hardware status — driven by the SAME JS as the old topbar badges (IDs preserved).
         loadSystemInfo() toggles #sdr-badge + writes #sdr-badge-label;
         handleSysHealth() toggles #pwr-badge + writes #pwr-badge-label. No JS changes. -->
    <div class="hw-status">
      <div id="sdr-badge" class="hw-row hw-row--sdr" style="display:none" title="Detected SDR hardware">
        <span class="hw-glyph" aria-hidden="true">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M5 18a9 9 0 0 1 14 0"/><path d="M8 15a5 5 0 0 1 8 0"/>
            <circle cx="12" cy="18" r="1.4" fill="currentColor" stroke="none"/>
          </svg>
        </span>
        <span class="hw-meta">
          <span class="hw-key" data-i18n="sdr">SDR</span>
          <span class="hw-val" id="sdr-badge-label">—</span>
        </span>
        <span class="hw-live" aria-hidden="true"><span class="hw-live-dot"></span></span>
      </div>
      <div id="health-badge" class="hw-row" style="display:none" title="Station health">
        <span class="hw-glyph" aria-hidden="true">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M3 12h4l2 5 4-12 2 7h2l2-3"/>
          </svg>
        </span>
        <span class="hw-meta">
          <span class="hw-key">HEALTH</span>
          <span class="hw-val" id="health-badge-label">—</span>
        </span>
      </div>
      <div id="pwr-badge" class="hw-row hw-row--pwr" style="display:none" title="Host system power draw">
        <span class="hw-glyph" aria-hidden="true">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M13 2 4 14h7l-1 8 9-12h-7l1-8Z"/>
          </svg>
        </span>
        <span class="hw-meta">
          <span class="hw-key" data-i18n="power">POWER</span>
          <span class="hw-val" id="pwr-badge-label">—</span>
        </span>
      </div>
    </div>
  </div>
  <div id="update-badge" class="update-badge"
       onclick="showPage('config',document.getElementById('nav-config'))"
       title="Click to update"></div>

  <div class="sidebar-nav">
    <!-- MONITOR — live, read-mostly surfaces (ordered by glance-frequency). -->
    <div class="nav-section-label" data-i18n-section="monitor">MONITOR</div>
    <div class="nav-item active" onclick="showPage('stations',this)" id="nav-stations">
      <span class="nav-icon" data-icon="radios"></span>
      <span class="nav-label" data-i18n="stations">RADIOS</span>
      <span class="nav-badge" id="badge-ms">0</span>
    </div>
    <div class="nav-item" onclick="showPage('calls',this)" id="nav-calls">
      <span class="nav-icon" data-icon="calls"></span>
      <span class="nav-label" data-i18n="calls">CALLS</span>
      <span class="nav-badge" id="badge-calls" style="display:none">0</span>
    </div>
    <div class="nav-item" onclick="showPage('lastheard',this)" id="nav-lastheard">
      <span class="nav-icon" data-icon="lastheard"></span>
      <span class="nav-label" data-i18n="lastheard">LAST HEARD</span>
    </div>
    <div class="nav-item" onclick="showPage('rf',this)" id="nav-rf">
      <span class="nav-icon" data-icon="rf"></span>
      <span class="nav-label" data-i18n="rf">RF</span>
    </div>
    <div class="nav-item" onclick="showPage('health',this)" id="nav-health">
      <span class="nav-icon" data-icon="health"></span>
      <span class="nav-label">HEALTH</span>
    </div>
    <div class="nav-item" onclick="showPage('log',this)" id="nav-log">
      <span class="nav-icon" data-icon="log"></span>
      <span class="nav-label" data-i18n="log">LOG</span>
    </div>
    <div class="nav-item" onclick="showPage('sdslog',this)" id="nav-sdslog">
      <span class="nav-icon" data-icon="sdslog"></span>
      <span class="nav-label" data-i18n="sdslog">SDS LOG</span>
    </div>

    <!-- INTEGRATIONS — external services (each hidden until its probe succeeds). -->
    <div class="nav-section-label" data-i18n-section="integrations">INTEGRATIONS</div>
    <div class="nav-item" onclick="showPage('asterisk',this)" id="nav-asterisk">
      <span class="nav-icon" data-icon="asterisk"></span>
      <span class="nav-label" data-i18n="asterisk">Asterisk SIP</span>
    </div>
    <div class="nav-item" onclick="showPage('dapnet',this)" id="nav-dapnet">
      <span class="nav-icon" data-icon="dapnet"></span>
      <span class="nav-label" data-i18n="dapnet">DAPNET</span>
    </div>
    <div class="nav-item" onclick="showPage('geoalarm',this)" id="nav-geoalarm">
      <span class="nav-icon" data-icon="geoalarm"></span>
      <span class="nav-label" data-i18n="geoalarm">GeoAlarm</span>
    </div>
    <div class="nav-item" onclick="showPage('meshcom',this)" id="nav-meshcom">
      <span class="nav-icon">⌁</span>
      <span class="nav-label" data-i18n="meshcom">MeshCom</span>
    </div>
    <div class="nav-item" onclick="showPage('telegram',this)" id="nav-telegram">
      <span class="nav-icon" data-icon="telegram"></span>
      <span class="nav-label" data-i18n="telegram">Telegram</span>
    </div>
    <!-- WiFi tab is hidden until we confirm NetworkManager is available on
         the host. The probe runs once at dashboard boot via /api/wifi/available
         and toggles this element's display. -->
    <div class="nav-item" onclick="showPage('wifi',this)" id="nav-wifi" style="display:none">
      <span class="nav-icon" data-icon="wifi"></span>
      <span class="nav-label" data-i18n="wifi">WIFI</span>
    </div>

    <!-- SYSTEM — configure / operate the station. -->
    <div class="nav-section-label" data-i18n-section="system_sec">SYSTEM</div>
    <div class="nav-item" onclick="showPage('config',this)" id="nav-config">
      <span class="nav-icon" data-icon="config"></span>
      <span class="nav-label" data-i18n="config">CONFIG</span>
    </div>
    <div class="nav-item" onclick="showPage('system',this)" id="nav-system">
      <span class="nav-icon" data-icon="system"></span>
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
    <button class="sidebar-toggle" onclick="toggleSidebar()" title="Toggle sidebar" aria-label="Toggle sidebar"><span class="ico18" data-icon="collapse"></span></button>
  </div>
</nav>

<!-- ── Main ── -->
<div id="main">
  <!-- Topbar -->
  <div id="topbar">
    <button id="sidebar-toggle-btn" onclick="openMobileSidebar()" aria-label="Menu"><span class="ico18" data-icon="hamburger"></span></button>
    <div class="topbar-title" id="topbar-title">Radios</div>

    <!-- Calm always-visible station-state chips (BS / Brew / Emergency-if-active). -->
    <div class="topbar-chips" aria-hidden="false">
      <span class="pill pill-idle" id="chip-bs" title="Base station link"><span data-i18n="bs_label">BS</span></span>
      <span class="pill pill-idle" id="chip-brew" title="Brew network"><span>Brew</span></span>
      <span class="pill pill-danger" id="chip-emergency" style="display:none" title="Emergency active">
        <span class="pill-icon" data-icon="emergency"></span><span data-i18n="emg_chip">EMERGENCY</span>
      </span>
    </div>

    <div class="topbar-right">
      <!-- Readability: opens an Apple-style level popover (Small/Medium/High/Ultra). -->
      <div class="eye-wrap">
        <button class="eye-btn" id="read-btn" onclick="toggleReadPop(event)"
                title="Text size &amp; contrast" aria-haspopup="true" aria-expanded="false" aria-label="Readability">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"
               stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7Z"/>
            <circle cx="12" cy="12" r="3"/>
          </svg>
        </button>
        <div class="read-pop" id="read-pop" role="menu" aria-label="Text size">
          <div class="read-pop-title" data-i18n="readability">READABILITY</div>
          <button class="read-opt" data-size="s" role="menuitemradio" onclick="setUiSize('s')">
            <span class="read-aa">Aa</span>
            <span class="read-opt-text">
              <span class="read-opt-name" data-i18n="size_small">Small</span>
              <span class="read-opt-desc" data-i18n="size_small_d">Compact · normal contrast</span>
            </span>
            <svg class="read-check" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 6 9 17l-5-5"/></svg>
          </button>
          <button class="read-opt" data-size="m" role="menuitemradio" onclick="setUiSize('m')">
            <span class="read-aa">Aa</span>
            <span class="read-opt-text">
              <span class="read-opt-name" data-i18n="size_medium">Medium</span>
              <span class="read-opt-desc" data-i18n="size_medium_d">Default · comfortable</span>
            </span>
            <svg class="read-check" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 6 9 17l-5-5"/></svg>
          </button>
          <button class="read-opt" data-size="h" role="menuitemradio" onclick="setUiSize('h')">
            <span class="read-aa">Aa</span>
            <span class="read-opt-text">
              <span class="read-opt-name" data-i18n="size_high">High</span>
              <span class="read-opt-desc" data-i18n="size_high_d">Larger · stronger contrast</span>
            </span>
            <svg class="read-check" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 6 9 17l-5-5"/></svg>
          </button>
          <button class="read-opt" data-size="u" role="menuitemradio" onclick="setUiSize('u')">
            <span class="read-aa">Aa</span>
            <span class="read-opt-text">
              <span class="read-opt-name" data-i18n="size_ultra">Ultra</span>
              <span class="read-opt-desc" data-i18n="size_ultra_d">Largest · maximum contrast</span>
            </span>
            <svg class="read-check" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 6 9 17l-5-5"/></svg>
          </button>
        </div>
      </div>
      <div class="theme-picker">
        <button class="theme-btn" data-t="dark" onclick="setTheme('dark',this)">Dark</button>
        <button class="theme-btn active" data-t="light" onclick="setTheme('light',this)">Light</button>
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
      <button class="logout-btn" id="logout-btn" onclick="doLogout()" title="Log out" aria-label="Log out" style="display:none"><span class="ico18" data-icon="shutdown"></span></button>
      <!-- Login: shown only in anonymous public-overview mode (FH-FEAT-033). -->
      <button class="logout-btn" id="login-btn" onclick="window.location='/login'" title="Log in" aria-label="Log in" style="display:none"><span class="ico18" data-icon="login"></span></button>
    </div>
  </div>

  <!-- Fallback config warning banner — hidden until JS shows it -->
  <div id="fallback-banner" class="banner banner-warn" style="display:none">
    <span class="banner-ico" data-icon="alert"></span>
    <div class="banner-body">
      <div data-i18n="fallback_title">FALLBACK CONFIG ACTIVE — Primary config failed to load</div>
      <div id="fallback-reason" class="banner-sub"></div>
    </div>
  </div>

  <!-- Emergency banner — persistent while >=1 ISSI is in active emergency; populated by JS.
       Single steady danger dot (soft breathe), never a harsh flashing ring. -->
  <div id="emergency-banner" class="banner banner-danger" style="display:none">
    <span class="banner-dot" aria-hidden="true"></span>
    <span class="banner-ico" data-icon="emergency"></span>
    <span data-i18n="emg_banner_title">EMERGENCY ACTIVE</span>
    <div id="emergency-banner-list" style="display:flex;flex-wrap:wrap;gap:8px"></div>
  </div>

  <!-- Content -->
  <div id="content">

    <!-- ── PUBLIC OVERVIEW (FH-FEAT-033) — shown only to anonymous visitors when public_overview is on ── -->
    <div class="page" id="page-public">
      <div class="stat-grid">
        <div class="stat-card green">
          <div class="stat-label">Radios</div>
          <div class="stat-value accent" id="pub-ms">—</div>
          <div class="stat-sub">registered</div>
          <div class="stat-icon" data-icon="radios"></div>
        </div>
        <div class="stat-card blue">
          <div class="stat-label">Active Calls</div>
          <div class="stat-value blue" id="pub-calls">—</div>
          <div class="stat-sub">circuits in use</div>
          <div class="stat-icon" data-icon="calls"></div>
        </div>
        <div class="stat-card" id="pub-rf-card">
          <div class="stat-label">RF</div>
          <div class="stat-value is-text" id="pub-rf">—</div>
          <div class="stat-sub" id="pub-freq">—</div>
          <div class="stat-icon" data-icon="rf"></div>
        </div>
        <div class="stat-card" id="pub-brew-card">
          <div class="stat-label">Network</div>
          <div class="stat-value is-text" id="pub-brew">—</div>
          <div class="stat-sub" id="pub-ver">—</div>
          <div class="stat-icon" data-icon="network"></div>
        </div>
      </div>
      <div class="card">
        <div class="card-head"><div class="card-title">Cell Status</div></div>
        <div class="card-body">
          <div class="empty-state">
            <span class="empty-ico" data-icon="login"></span>
            <div class="empty-msg">Read-only public overview</div>
            <div class="empty-sub">Log in for full access and controls.</div>
          </div>
        </div>
      </div>
    </div>

    <!-- ── RADIOS ── -->
    <div class="page active" id="page-stations">
      <!-- Hero summary -->
      <div class="hero">
        <span class="hero-dot is-idle" id="stations-hero-dot"></span>
        <div class="hero-main">
          <div class="hero-title" id="stations-hero-title" data-i18n="terminals">Radios</div>
          <div class="hero-sub" id="stations-hero-sub" data-i18n="registered">registered</div>
        </div>
        <div class="hero-metrics">
          <div class="hero-metric">
            <div class="hero-metric-label" data-i18n="active_calls">Active Calls</div>
            <div class="hero-metric-value" id="stations-hero-calls">0</div>
          </div>
          <div class="hero-metric">
            <div class="hero-metric-label">BREW</div>
            <div class="hero-metric-value" id="stations-hero-brew">—</div>
          </div>
        </div>
      </div>
      <!-- Stat cards -->
      <div class="stat-grid">
        <div class="stat-card green">
          <div class="stat-label" data-i18n="terminals">Radios</div>
          <div class="stat-value accent" id="stat-ms">0</div>
          <div class="stat-sub" data-i18n="registered">registered</div>
          <div class="stat-icon" data-icon="radios"></div>
        </div>
        <div class="stat-card blue">
          <div class="stat-label" data-i18n="active_calls">Active Calls</div>
          <div class="stat-value blue" id="stat-calls">0</div>
          <div class="stat-sub" data-i18n="circuits">circuits in use</div>
          <div class="stat-icon" data-icon="calls"></div>
        </div>
        <div class="stat-card is-danger" id="stat-brew-card">
          <div class="stat-label">BREW</div>
          <div class="stat-value is-text" id="stat-brew-val">OFFLINE</div>
          <div class="stat-sub" id="stat-brew-sub">—</div>
          <div class="stat-icon" data-icon="network"></div>
        </div>
      </div>
      <!-- TETRA BTS Details — static cell + RF identity from config.toml -->
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="bts_details">TETRA BTS Details</div>
          <div class="card-actions">
            <span id="bts-neighbor" class="bts-chip">—</span>
            <span id="bts-hang" class="bts-chip">—</span>
          </div>
        </div>
        <div class="bts-grid">
          <div class="bts-tile"><div class="bts-tile-label" data-i18n="bts_tx">TX Freq</div><div class="bts-tile-value tx" id="bts-tx">—</div></div>
          <div class="bts-tile"><div class="bts-tile-label" data-i18n="bts_rx">RX Freq</div><div class="bts-tile-value rx" id="bts-rx">—</div></div>
          <div class="bts-tile"><div class="bts-tile-label" data-i18n="bts_shift">Duplex Shift</div><div class="bts-tile-value" id="bts-shift">—</div></div>
          <div class="bts-tile"><div class="bts-tile-label">MCC</div><div class="bts-tile-value" id="bts-mcc">—</div></div>
          <div class="bts-tile"><div class="bts-tile-label">MNC</div><div class="bts-tile-value" id="bts-mnc">—</div></div>
          <div class="bts-tile"><div class="bts-tile-label" data-i18n="bts_carrier">Main Carrier</div><div class="bts-tile-value" id="bts-carrier">—</div></div>
        </div>
        <div class="bts-access-bar">
          <div class="bts-access-info">
            <span class="bts-access-icon">
              <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2 4 5v6c0 5 3.5 8 8 9 4.5-1 8-4 8-9V5z"/><path d="M9 12l2 2 4-4"/></svg>
            </span>
            <div>
              <div class="bts-access-title" data-i18n="bts_access">Registration Access</div>
              <div class="bts-access-sub" id="bts-access-sub">—</div>
            </div>
          </div>
          <span id="bts-access" class="bts-access">—</span>
        </div>
        <!-- Dual-Carrier ON/OFF — applied via controlled service restart -->
        <div class="bts-access-bar">
          <div class="bts-access-info">
            <span class="bts-access-icon">
              <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4.9 16.1a10 10 0 0 1 0-8.2"/><path d="M19.1 7.9a10 10 0 0 1 0 8.2"/><path d="M7.8 13.2a5 5 0 0 1 0-2.4"/><path d="M16.2 10.8a5 5 0 0 1 0 2.4"/><circle cx="12" cy="12" r="1.5"/></svg>
            </span>
            <div>
              <div class="bts-access-title" data-i18n="dual_carrier">Dual Carrier</div>
              <div class="bts-access-sub" id="dc-sub">—</div>
            </div>
          </div>
          <span class="sw"><input type="checkbox" id="dc-toggle" onchange="onDualCarrierToggle(this)"><i></i></span>
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
            <div class="ts-sub">ACTIVE</div>
            <div class="ts-flash"></div>
            <div class="ts-duration-bar"></div>
          </div>
          <div class="ts-block" id="ts-block-2">
            <div class="ts-num">TS 2</div>
            <div class="ts-timer"></div>
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
            <div class="ts-timer"></div>
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
            <div class="ts-timer"></div>
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
                <th data-i18n="th_issi_cs">ISSI / Callsign</th>
                <th data-i18n="th_groups">Groups</th>
                <th class="col-mobile-hide" data-i18n="th_ee">Energy Economy</th>
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
          <div class="card-actions">
            <button class="btn btn-sm" onclick="exportLog()"><span class="btn-icon" data-icon="export"></span><span data-i18n="export">Export</span></button>
            <button class="btn btn-sm" onclick="clearLog()"><span class="btn-icon" data-icon="delete"></span><span data-i18n="clear">Clear</span></button>
          </div>
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
        </div>
      </div>
    </div>

    <!-- ── SDS LOG ── -->
    <!-- SDS messages sent/received locally on this BS. Backed by a persisted ring
         (sds_log.json) so the history survives restarts. Populated live over the WS
         and refetched from /api/sds-log when the tab opens. -->
    <div class="page" id="page-sdslog">
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="sdslog">SDS Log</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="loadSdsLog()"><span class="btn-icon" data-icon="restart"></span><span data-i18n="sds_refresh">Refresh</span></button>
            <button class="btn btn-sm" onclick="exportSdsLog()"><span class="btn-icon" data-icon="export"></span><span data-i18n="export">Export</span></button>
            <button class="btn btn-sm btn-danger" onclick="clearSdsLog()"><span class="btn-icon" data-icon="delete"></span><span data-i18n="clear">Clear</span></button>
          </div>
        </div>
        <div class="card-body">
          <div class="table-wrap">
            <table>
              <thead><tr>
                <th data-i18n="th_time">Time</th>
                <th data-i18n="th_dir">Dir</th>
                <th data-i18n="th_from">From</th>
                <th data-i18n="th_to">To</th>
                <th data-i18n="th_message">Message</th>
              </tr></thead>
              <tbody id="sdslog-tbody"></tbody>
            </table>
          </div>
          <div class="log-controls">
            <button class="btn btn-sm" onclick="sdsLogPrevPage()">‹ Prev</button>
            <span class="sds-empty" id="sdslog-page">Page 1 / 1</span>
            <button class="btn btn-sm" onclick="sdsLogNextPage()">Next ›</button>
          </div>
        </div>
      </div>
    </div>

    <!-- ── RF ── -->
    <!-- Live TX DSP monitor — works on any SDR because the analysis is done on the
         complex baseband samples FlowStation generates internally, BEFORE they reach
         the radio. We do not rely on receive-side feedback. -->
    <div class="page" id="page-rf">

      <!-- Hero summary -->
      <div class="hero">
        <span class="hero-dot is-idle" id="rf-hero-dot"></span>
        <div class="hero-main">
          <div class="hero-title" data-i18n="rf_spectrum">TX DSP Spectrum (pre-PA)</div>
          <div class="hero-sub" id="rf-hero-sub" data-i18n="rf_waiting">waiting…</div>
        </div>
        <div class="hero-metrics">
          <div class="hero-metric">
            <div class="hero-metric-label" data-i18n="rf_freq">Center freq</div>
            <div class="hero-metric-value" id="rf-hero-freq">—</div>
          </div>
          <div class="hero-metric">
            <div class="hero-metric-label" data-i18n="rf_evm">EVM</div>
            <div class="hero-metric-value" id="rf-hero-evm">—</div>
          </div>
        </div>
      </div>

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

      <div class="section-label" data-i18n="rf_visualizers">Visualizers</div>
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
        <canvas id="rf-waterfall" class="rf-canvas tall"></canvas>
      </div>

      <div class="section-label" data-i18n="rf_quality">Signal Quality</div>
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
            <div class="gauge"><div class="gauge-track"><div class="gauge-fill" id="rf-evm-bar"></div></div></div>
          </div>
          <div class="rf-qmetric" id="rf-q-papr-wrap">
            <div class="rf-qmetric-label" data-i18n="rf_papr">PAPR</div>
            <div class="rf-qmetric-value" id="rf-papr">—</div>
            <div class="gauge"><div class="gauge-track"><div class="gauge-fill" id="rf-papr-bar"></div></div></div>
          </div>
          <div class="rf-qmetric" id="rf-q-cl-wrap">
            <div class="rf-qmetric-label" data-i18n="rf_carrier">Carrier leak</div>
            <div class="rf-qmetric-value" id="rf-carrier">—</div>
            <div class="gauge"><div class="gauge-track"><div class="gauge-fill" id="rf-carrier-bar"></div></div></div>
          </div>
          <div class="rf-qmetric" id="rf-q-obw-wrap">
            <div class="rf-qmetric-label" data-i18n="rf_obw">Occupied BW (99%)</div>
            <div class="rf-qmetric-value" id="rf-obw">—</div>
            <div class="gauge"><div class="gauge-track"><div class="gauge-fill" id="rf-obw-bar"></div></div></div>
          </div>
        </div>
      </div>

      <div class="section-label" data-i18n="rf_hw_health">Hardware Health</div>
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
            <div class="gauge" id="rf-temp-gauge"><div class="gauge-track"><div class="gauge-fill" id="rf-temp-bar"></div></div></div>
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

    <!-- ── ASTERISK SIP ── -->
    <div class="page" id="page-asterisk">
      <div class="section-label" data-i18n="integrations">Integrations</div>
      <!-- Connection hero — live REGISTER state as a calm status pill. -->
      <div class="hero">
        <span class="hero-dot is-idle" id="ast-hero-dot"></span>
        <div class="hero-main">
          <div class="hero-title" data-i18n="asterisk_title">Asterisk SIP</div>
          <div class="hero-sub" id="ast-hero-sub">—</div>
        </div>
        <div class="hero-metrics">
          <span class="pill pill-idle" id="ast-hero-pill">—</span>
        </div>
      </div>
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="asterisk_title">Asterisk SIP</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="loadAsteriskStatus()"><span class="btn-icon" data-icon="restart"></span><span data-i18n="refresh">Refresh</span></button>
          </div>
        </div>
        <div class="card-body">
          <div class="stat-grid" style="margin-bottom:14px">
            <div class="stat-card" id="ast-configured-card">
              <div class="stat-label" data-i18n="ast_configured">Configured</div>
              <div class="stat-value is-text" id="ast-configured">—</div>
              <div class="stat-sub" id="ast-enabled">—</div>
            </div>
            <div class="stat-card blue" id="ast-register-card">
              <div class="stat-label" data-i18n="ast_register">REGISTER</div>
              <div class="stat-value is-text blue" id="ast-register">—</div>
              <div class="stat-sub" id="ast-dialogs">—</div>
            </div>
          </div>
          <div class="info-grid">
            <div class="info-row"><div class="info-key" data-i18n="ast_sip_listen">SIP listen</div><div class="info-val" id="ast-sip-listen">—</div></div>
            <div class="info-row"><div class="info-key" data-i18n="ast_remote">Remote Asterisk</div><div class="info-val" id="ast-remote">—</div></div>
            <div class="info-row"><div class="info-key" data-i18n="ast_rtp">RTP ports</div><div class="info-val" id="ast-rtp">—</div></div>
            <div class="info-row"><div class="info-key" data-i18n="ast_codec">Codec</div><div class="info-val" id="ast-codec">—</div></div>
            <div class="info-row"><div class="info-key" data-i18n="ast_last_rx">Last RX</div><div class="info-val" id="ast-last-rx">—</div></div>
            <div class="info-row"><div class="info-key" data-i18n="ast_last_tx">Last TX</div><div class="info-val" id="ast-last-tx">—</div></div>
            <div class="info-row"><div class="info-key" data-i18n="ast_last_error">Last error</div><div class="info-val" id="ast-last-error">—</div></div>
          </div>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title">Snom SIP NOTIFY</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="loadSnomNotify()"><span class="btn-icon" data-icon="restart"></span><span data-i18n="refresh">Refresh</span></button>
            <button class="btn btn-primary" onclick="saveSnomNotify()"><span class="btn-icon" data-icon="save"></span><span data-i18n="save">Save</span></button>
          </div>
        </div>
        <div class="card-body">
          <label class="sw-row">
            <span class="sw-text">Enable SnomIPPhoneText notifications</span>
            <span class="sw"><input type="checkbox" id="snom-enabled"><i></i></span>
          </label>

          <div class="h-form" style="margin-top:14px;grid-template-columns:repeat(auto-fit,minmax(220px,1fr))">
            <label class="h-flabel">AMI host</label>
            <input type="text" id="snom-ami-host" class="form-input" placeholder="127.0.0.1">
            <label class="h-flabel">AMI port</label>
            <input type="number" id="snom-ami-port" class="form-input" min="1" max="65535" placeholder="5038">
            <label class="h-flabel">AMI user</label>
            <input type="text" id="snom-ami-user" class="form-input" autocomplete="off" spellcheck="false" placeholder="flowstation">
            <label class="h-flabel">AMI password</label>
            <input type="password" id="snom-ami-password" class="form-input" autocomplete="new-password" spellcheck="false" oninput="snomPasswordDirty=true">
            <label class="h-flabel top">PJSIP endpoints</label>
            <textarea id="snom-endpoints" class="form-input" rows="3" placeholder="385&#10;386"></textarea>
          </div>

          <div class="h-form wide" style="margin-top:16px">
            <div>
              <label class="sw-row"><span class="sw-text">Notify TETRA SDS</span><span class="sw"><input type="checkbox" id="snom-notify-sds"><i></i></span></label>
              <div class="h-fopts" style="margin:8px 0 10px">
                <label class="h-fopt"><input type="checkbox" id="snom-dir-rx"> RX</label>
                <label class="h-fopt"><input type="checkbox" id="snom-dir-net"> NET</label>
                <label class="h-fopt"><input type="checkbox" id="snom-dir-tx"> TX</label>
              </div>
              <label class="h-flabel">SDS ISSI whitelist</label>
              <textarea id="snom-sds-issis" class="form-input" rows="4" placeholder="2632585&#10;9999"></textarea>
              <div class="help-text">Empty = every SDS. A match on source or destination ISSI is enough.</div>
            </div>
            <div>
              <label class="sw-row"><span class="sw-text">Notify DAPNET</span><span class="sw"><input type="checkbox" id="snom-notify-dapnet"><i></i></span></label>
              <label class="h-flabel">DAPNET RIC whitelist</label>
              <textarea id="snom-dapnet-rics" class="form-input" rows="4" placeholder="0632585&#10;0000200"></textarea>
              <div class="help-text">Empty = every DAPNET message. Leading zeros are preserved in config.</div>
            </div>
            <div>
              <label class="sw-row"><span class="sw-text">Notify Telegram</span><span class="sw"><input type="checkbox" id="snom-notify-telegram"><i></i></span></label>
              <div class="h-form-pair" style="margin-top:10px">
                <label class="h-flabel">Title prefix</label>
                <input type="text" id="snom-title-prefix" class="form-input" placeholder="FlowStation">
                <label class="h-flabel">Max text chars</label>
                <input type="number" id="snom-max-text" class="form-input" min="40" max="2000" placeholder="240">
                <label class="h-flabel">Timeout (s)</label>
                <input type="number" id="snom-timeout" class="form-input" min="1" max="30" placeholder="3">
              </div>
            </div>
          </div>
          <div class="config-msg" id="snom-msg"></div>
        </div>
      </div>
    </div>

    <!-- ── DAPNET ── -->
    <div class="page" id="page-dapnet">
      <div class="section-label" data-i18n="integrations">Integrations</div>
      <!-- Connection hero — DAPNET feed state as a calm status pill. -->
      <div class="hero">
        <span class="hero-dot is-idle" id="dap-hero-dot"></span>
        <div class="hero-main">
          <div class="hero-title" data-i18n="dapnet_title">DAPNET</div>
          <div class="hero-sub" id="dap-hero-sub">—</div>
        </div>
        <div class="hero-metrics">
          <span class="pill pill-idle" id="dap-hero-pill">—</span>
        </div>
      </div>
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="dapnet_log">DAPNET Log</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="loadDapnetLog()"><span class="btn-icon" data-icon="restart"></span><span data-i18n="refresh">Refresh</span></button>
            <button class="btn btn-sm" onclick="exportDapnetLog()"><span class="btn-icon" data-icon="export"></span><span data-i18n="export">Export</span></button>
            <button class="btn btn-sm btn-danger" onclick="clearDapnetLog()"><span class="btn-icon" data-icon="delete"></span><span data-i18n="clear">Clear</span></button>
          </div>
        </div>
        <div class="card-body">
          <div class="table-wrap">
            <table>
              <thead><tr>
                <th data-i18n="th_time">Time</th>
                <th data-i18n="th_dir">Dir</th>
                <th>Callsign</th>
                <th>Recipient</th>
                <th>Paths</th>
                <th data-i18n="th_message">Message</th>
              </tr></thead>
              <tbody id="dapnetlog-tbody"></tbody>
            </table>
          </div>
          <div class="log-controls">
            <button class="btn btn-sm" onclick="dapnetLogPrevPage()">‹ Prev</button>
            <span class="sds-empty" id="dapnetlog-page">Page 1 / 1</span>
            <button class="btn btn-sm" onclick="dapnetLogNextPage()">Next ›</button>
          </div>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="dapnet_title">DAPNET</div>
          <div class="card-actions">
            <button class="btn btn-primary" onclick="saveDapnet()"><span class="btn-icon" data-icon="save"></span><span data-i18n="save">Save</span></button>
          </div>
        </div>
        <div class="card-body">
          <label class="sw-row">
            <span class="sw-text">Enable DAPNET integration</span>
            <span class="sw"><input type="checkbox" id="dap-enabled"><i></i></span>
          </label>
          <label class="sw-row">
            <span class="sw-text">Enable RWTH core receive feed</span>
            <span class="sw"><input type="checkbox" id="dap-rwth-enabled"><i></i></span>
          </label>

          <div class="h-form" style="margin-top:14px">
            <label class="h-flabel">Poll interval (s)</label>
            <input type="number" id="dap-poll" class="form-input" min="1" placeholder="30">
            <label class="h-flabel">Messages limit</label>
            <input type="number" id="dap-limit" class="form-input" min="1" placeholder="100">

            <label class="h-flabel">Hampager API URL</label>
            <input type="text" id="dap-api-url" class="form-input" placeholder="https://hampager.de/api/calls" style="grid-column:1 / -1;min-width:0">

            <label class="h-flabel">API username</label>
            <input type="text" id="dap-username" class="form-input" autocomplete="off" spellcheck="false">
            <label class="h-flabel">API password</label>
            <input type="password" id="dap-password" class="form-input" autocomplete="new-password" spellcheck="false" oninput="dapPasswordDirty=true">

            <label class="h-flabel">RWTH host</label>
            <input type="text" id="dap-rwth-host" class="form-input" placeholder="dapnet.afu.rwth-aachen.de">
            <label class="h-flabel">RWTH port</label>
            <input type="number" id="dap-rwth-port" class="form-input" min="1" max="65535" placeholder="43434">

            <label class="h-flabel">Device</label>
            <input type="text" id="dap-rwth-device" class="form-input" placeholder="FlowStation">
            <label class="h-flabel">Version</label>
            <input type="text" id="dap-rwth-version" class="form-input" placeholder="1.0">

            <label class="h-flabel">RWTH callsign</label>
            <input type="text" id="dap-rwth-callsign" class="form-input" autocomplete="off" spellcheck="false" style="text-transform:uppercase">
            <label class="h-flabel">RWTH authkey</label>
            <input type="password" id="dap-rwth-authkey" class="form-input" autocomplete="new-password" spellcheck="false" oninput="dapAuthDirty=true">
          </div>
          <div class="config-msg" id="dap-msg"></div>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="dapnet_routing">Routing</div>
          <div class="card-actions">
            <button class="btn btn-primary" onclick="saveDapnet()"><span class="btn-icon" data-icon="save"></span><span data-i18n="save">Save</span></button>
          </div>
        </div>
        <div class="card-body">
          <div class="h-form wide">
            <div>
              <label class="sw-row"><span class="sw-text">Forward to SDS</span><span class="sw"><input type="checkbox" id="dap-forward-sds"><i></i></span></label>
              <div class="h-form-pair" style="margin-top:10px">
                <label class="h-flabel">Source ISSI</label>
                <input type="number" id="dap-sds-source" class="form-input" min="1" max="16777215" placeholder="9999">
                <label class="h-flabel">Destination</label>
                <input type="number" id="dap-sds-dest" class="form-input" min="0" max="16777215" placeholder="ISSI or GSSI">
                <label class="h-flabel">Destination is group</label>
                <label class="h-finline"><span class="sw"><input type="checkbox" id="dap-sds-group"><i></i></span><span class="h-flabel-sm">GSSI</span></label>
                <label class="h-flabel top">RIC → ISSI</label>
                <textarea id="dap-ric-routes" class="form-input" rows="3" placeholder="0632585=2632585"></textarea>
                <label class="h-flabel top">RIC → GSSI</label>
                <textarea id="dap-ric-group-routes" class="form-input" rows="3" placeholder="0004520=80"></textarea>
                <label class="h-flabel top">SDS RIC filter</label>
                <textarea id="dap-sds-rics" class="form-input" rows="3" placeholder="0004520&#10;0000200"></textarea>
              </div>
            </div>

            <div>
              <label class="sw-row"><span class="sw-text">Forward to TPG2200 Call-Out</span><span class="sw"><input type="checkbox" id="dap-forward-callout"><i></i></span></label>
              <div class="h-form-pair" style="margin-top:10px">
                <label class="h-flabel">Source ISSI</label>
                <input type="number" id="dap-callout-source" class="form-input" min="1" max="16777215" placeholder="9999">
                <label class="h-flabel">Destination</label>
                <input type="number" id="dap-callout-dest" class="form-input" min="0" max="16777215" placeholder="TPG2200 ISSI">
                <label class="h-flabel">Incident base</label>
                <input type="number" id="dap-callout-incident" class="form-input" min="1" max="256" placeholder="2">
                <label class="h-flabel">Text prefix</label>
                <input type="text" id="dap-callout-prefix" class="form-input" placeholder="DAPNET">
                <label class="h-flabel top">Call-Out RIC filter</label>
                <textarea id="dap-callout-rics" class="form-input" rows="3" placeholder="0004520"></textarea>
              </div>
            </div>

            <div>
              <label class="sw-row"><span class="sw-text">Forward to Telegram</span><span class="sw"><input type="checkbox" id="dap-forward-telegram"><i></i></span></label>
              <div class="h-form-pair" style="margin-top:10px">
                <label class="h-flabel">Telegram prefix</label>
                <input type="text" id="dap-telegram-prefix" class="form-input" placeholder="DAPNET">
                <label class="h-flabel top">Telegram RIC filter</label>
                <textarea id="dap-telegram-rics" class="form-input" rows="3" placeholder="0004520"></textarea>
              </div>
              <div class="help-text" style="margin-top:10px">Uses the existing Telegram alert configuration and recipients.</div>
            </div>
          </div>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="dapnet_send">Send DAPNET Message</div>
          <div class="card-actions">
            <button class="btn btn-primary" onclick="sendDapnetMessage()">Send</button>
          </div>
        </div>
        <div class="card-body">
          <div class="h-form">
            <label class="h-flabel">Callsign recipients</label>
            <input type="text" id="dap-out-callsigns" class="form-input" placeholder="DJ2TH, DB0ABC">
            <label class="h-flabel">Transmitter groups</label>
            <input type="text" id="dap-out-groups" class="form-input" placeholder="dl-all, regional">
            <label class="h-flabel">Emergency</label>
            <label class="h-finline"><span class="sw"><input type="checkbox" id="dap-out-emergency"><i></i></span><span class="h-flabel-sm">Set emergency flag</span></label>
            <label class="h-flabel top">Message</label>
            <textarea id="dap-out-text" class="form-input" rows="3" maxlength="80" placeholder="Message text"></textarea>
          </div>
          <div class="config-msg" id="dap-send-msg"></div>
        </div>
      </div>
    </div>

    <!-- ── GEOALARM ── -->
    <div class="page" id="page-geoalarm">
      <div class="section-label" data-i18n="integrations">Integrations</div>
      <!-- Connection hero — GeoAlarm enabled state as a calm status pill. -->
      <div class="hero">
        <span class="hero-dot is-idle" id="geo-hero-dot"></span>
        <div class="hero-main">
          <div class="hero-title" data-i18n="geoalarm_title">GeoAlarm</div>
          <div class="hero-sub" id="geo-hero-sub">—</div>
        </div>
        <div class="hero-metrics">
          <span class="pill pill-idle" id="geo-hero-pill">—</span>
        </div>
      </div>
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="geoalarm_title">GeoAlarm</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="loadGeoalarm()"><span class="btn-icon" data-icon="restart"></span><span data-i18n="refresh">Refresh</span></button>
            <button class="btn btn-primary" onclick="saveGeoalarm()"><span class="btn-icon" data-icon="save"></span><span data-i18n="save">Save</span></button>
          </div>
        </div>
        <div class="card-body">
          <div class="stat-grid" style="margin-bottom:14px">
            <div class="stat-card">
              <div class="stat-label">Positions</div>
              <div class="stat-value" id="geo-seen">0</div>
              <div class="stat-sub" id="geo-center">—</div>
            </div>
            <div class="stat-card blue">
              <div class="stat-label">Alarms</div>
              <div class="stat-value blue" id="geo-alarms">0</div>
              <div class="stat-sub" id="geo-radius">—</div>
            </div>
          </div>
          <div class="info-grid" style="margin-bottom:14px">
            <div class="info-row"><div class="info-key">Last position</div><div class="info-val" id="geo-last-position">—</div></div>
            <div class="info-row"><div class="info-key">Last alarm</div><div class="info-val" id="geo-last-alarm">—</div></div>
            <div class="info-row"><div class="info-key">Last error</div><div class="info-val" id="geo-last-error">—</div></div>
          </div>

          <label class="sw-row">
            <span class="sw-text">Enable GeoAlarm</span>
            <span class="sw"><input type="checkbox" id="geo-enabled"><i></i></span>
          </label>
          <div class="h-form" style="margin-top:14px">
            <label class="h-flabel">FlowStation latitude</label>
            <input type="number" id="geo-lat" class="form-input" step="0.000001" min="-90" max="90" placeholder="50.775346">
            <label class="h-flabel">FlowStation longitude</label>
            <input type="number" id="geo-lon" class="form-input" step="0.000001" min="-180" max="180" placeholder="6.083887">
            <label class="h-flabel">Radius / cooldown</label>
            <div style="display:grid;grid-template-columns:1fr 1fr;gap:8px">
              <input type="number" id="geo-radius-m" class="form-input" min="1" step="1" placeholder="500">
              <input type="number" id="geo-cooldown" class="form-input" min="1" max="86400" placeholder="300">
            </div>
            <label class="h-flabel">Input sources</label>
            <div class="h-fopts">
              <label class="h-fopt"><span class="sw"><input type="checkbox" id="geo-trigger-tetra"><i></i></span><span class="h-flabel-sm">TETRA LIP</span></label>
              <label class="h-fopt"><span class="sw"><input type="checkbox" id="geo-trigger-meshcom"><i></i></span><span class="h-flabel-sm">MeshCom</span></label>
            </div>
          </div>
          <div class="help-text" style="margin-top:10px">GeoAlarm fires when an allowed device enters the radius, then suppresses repeated alarms for the cooldown time.</div>
          <div class="config-msg" id="geo-msg"></div>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title">GeoAlarm Routing</div>
        </div>
        <div class="card-body">
          <div class="h-form wide" style="grid-template-columns:repeat(auto-fit,minmax(280px,1fr))">
            <div>
              <label class="sw-row">
                <span class="sw-text">Alarm → TPG2200</span>
                <span class="sw"><input type="checkbox" id="geo-forward-tpg"><i></i></span>
              </label>
              <div style="display:grid;grid-template-columns:1fr 1fr;gap:8px;margin-top:10px">
                <input type="number" id="geo-tpg-source" class="form-input" min="1" max="16777215" placeholder="Source ISSI">
                <input type="number" id="geo-tpg-dest" class="form-input" min="0" max="16777215" placeholder="TPG ISSI">
              </div>
              <div style="display:grid;grid-template-columns:1fr 1fr;gap:8px;margin-top:10px">
                <input type="number" id="geo-tpg-incident" class="form-input" min="1" max="256" placeholder="Incident base">
                <input type="number" id="geo-tpg-max" class="form-input" min="8" max="160" placeholder="Max chars">
              </div>
              <input type="text" id="geo-tpg-prefix" class="form-input" placeholder="TPG text prefix" style="margin-top:10px">
            </div>
            <div>
              <label class="sw-row">
                <span class="sw-text">Alarm → SDS</span>
                <span class="sw"><input type="checkbox" id="geo-forward-sds"><i></i></span>
              </label>
              <div style="display:grid;grid-template-columns:1fr 1fr;gap:8px;margin-top:10px">
                <input type="number" id="geo-sds-source" class="form-input" min="1" max="16777215" placeholder="Source ISSI">
                <input type="number" id="geo-sds-dest" class="form-input" min="0" max="16777215" placeholder="Destination ISSI/GSSI">
              </div>
              <label class="h-finline" style="margin-top:10px"><span class="sw"><input type="checkbox" id="geo-sds-group"><i></i></span><span class="h-flabel-sm">Destination is group/GSSI</span></label>
            </div>
            <div>
              <label class="sw-row">
                <span class="sw-text">Alarm → SIP/Snom</span>
                <span class="sw"><input type="checkbox" id="geo-forward-sip"><i></i></span>
              </label>
              <input type="text" id="geo-sip-prefix" class="form-input" placeholder="Snom title prefix" style="margin-top:10px">
              <label class="sw-row" style="margin-top:14px">
                <span class="sw-text">Alarm → Telegram</span>
                <span class="sw"><input type="checkbox" id="geo-forward-telegram"><i></i></span>
              </label>
              <input type="text" id="geo-telegram-prefix" class="form-input" placeholder="Telegram prefix" style="margin-top:10px">
            </div>
          </div>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title">GeoAlarm Filters</div>
        </div>
        <div class="card-body">
          <div class="h-form wide">
            <div>
              <label class="h-flabel">TETRA ISSI whitelist</label>
              <textarea id="geo-tetra-white" class="form-input" rows="4" placeholder="empty = all TETRA ISSIs"></textarea>
            </div>
            <div>
              <label class="h-flabel">TETRA ISSI blacklist</label>
              <textarea id="geo-tetra-black" class="form-input" rows="4" placeholder="blocked ISSIs"></textarea>
            </div>
            <div>
              <label class="h-flabel">MeshCom source whitelist</label>
              <textarea id="geo-mesh-white" class="form-input" rows="4" placeholder="empty = all MeshCom sources"></textarea>
            </div>
            <div>
              <label class="h-flabel">MeshCom source blacklist</label>
              <textarea id="geo-mesh-black" class="form-input" rows="4" placeholder="blocked MeshCom sources"></textarea>
            </div>
          </div>
          <div class="help-text" style="margin-top:10px">Whitelist empty means allow all. Blacklists always win. MeshCom source matching is case-insensitive.</div>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title">GeoAlarm Events</div>
        </div>
        <div class="card-body">
          <div class="table-wrap">
            <table>
              <thead><tr>
                <th data-i18n="th_time">Time</th>
                <th>Source</th>
                <th>Device</th>
                <th>Distance</th>
                <th>Position</th>
                <th>Status</th>
                <th>Paths</th>
              </tr></thead>
              <tbody id="geo-events-tbody"></tbody>
            </table>
          </div>
          <div class="log-controls">
            <button class="btn btn-sm" onclick="geoPrevPage()">‹ Prev</button>
            <span class="sds-empty" id="geo-events-page">Page 1 / 1</span>
            <button class="btn btn-sm" onclick="geoNextPage()">Next ›</button>
          </div>
        </div>
      </div>
    </div>

    <!-- ── MESHCOM ── -->
    <div class="page" id="page-meshcom">
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="meshcom_title">MeshCom</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="loadMeshcom()" data-i18n="refresh">⟳ Refresh</button>
            <button class="btn btn-primary" onclick="saveMeshcom()" data-i18n="save">Save</button>
          </div>
        </div>
        <div class="card-body">
          <div class="stat-grid" style="margin-bottom:14px">
            <div class="stat-card">
              <div class="stat-label">UDP RX</div>
              <div class="stat-value" id="mesh-rx-count">0</div>
              <div class="stat-sub" id="mesh-bind">—</div>
            </div>
            <div class="stat-card blue">
              <div class="stat-label">UDP TX</div>
              <div class="stat-value blue" id="mesh-tx-count">0</div>
              <div class="stat-sub" id="mesh-tx">—</div>
            </div>
          </div>
          <div class="info-grid" style="margin-bottom:14px">
            <div class="info-row"><div class="info-key">Nodes</div><div class="info-val" id="mesh-node-count">—</div></div>
            <div class="info-row"><div class="info-key">Last RX</div><div class="info-val" id="mesh-last-rx">—</div></div>
            <div class="info-row"><div class="info-key">Last TX</div><div class="info-val" id="mesh-last-tx">—</div></div>
            <div class="info-row"><div class="info-key">Last error</div><div class="info-val" id="mesh-last-error">—</div></div>
          </div>

          <label class="sw-row">
            <span class="sw-text">Enable MeshCom UDP integration</span>
            <span class="sw"><input type="checkbox" id="mesh-enabled"><i></i></span>
          </label>
          <div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(190px,1fr));gap:10px;align-items:center;margin-top:14px">
            <label style="color:var(--muted);font-size:13px">Bind address</label>
            <input type="text" id="mesh-bind-addr" class="form-input" placeholder="0.0.0.0">
            <label style="color:var(--muted);font-size:13px">Bind port</label>
            <input type="number" id="mesh-bind-port" class="form-input" min="1" max="65535" placeholder="1799">
            <label style="color:var(--muted);font-size:13px">Node TX host</label>
            <input type="text" id="mesh-tx-host" class="form-input" placeholder="255.255.255.255">
            <label style="color:var(--muted);font-size:13px">Node TX port</label>
            <input type="number" id="mesh-tx-port" class="form-input" min="1" max="65535" placeholder="1799">
            <label style="color:var(--muted);font-size:13px">Allow broadcast</label>
            <label style="display:flex;align-items:center;gap:10px"><span class="sw"><input type="checkbox" id="mesh-broadcast"><i></i></span><span style="color:var(--muted);font-size:12px">required for 255.255.255.255</span></label>
            <label style="color:var(--muted);font-size:13px">History limits</label>
            <div style="display:grid;grid-template-columns:1fr 1fr;gap:8px">
              <input type="number" id="mesh-max-messages" class="form-input" min="10" max="10000" placeholder="500">
              <input type="number" id="mesh-max-nodes" class="form-input" min="10" max="65535" placeholder="1000">
            </div>
          </div>
          <div class="help-text" style="margin-top:10px">On the MeshCom node, enable extUDP and point it to the FlowStation host, for example: --extudpip &lt;flowstation-ip&gt; and --extudp on.</div>
          <div class="config-msg" id="mesh-msg"></div>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title">MeshCom Routing</div>
        </div>
        <div class="card-body">
          <div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:14px">
            <div>
              <label class="sw-row">
                <span class="sw-text">Forward MeshCom → SDS</span>
                <span class="sw"><input type="checkbox" id="mesh-forward-sds"><i></i></span>
              </label>
              <div style="display:grid;grid-template-columns:1fr 1fr;gap:8px;margin-top:10px">
                <input type="number" id="mesh-sds-source" class="form-input" min="1" max="16777215" placeholder="Source ISSI">
                <input type="number" id="mesh-sds-dest" class="form-input" min="0" max="16777215" placeholder="Destination ISSI/GSSI">
              </div>
              <label style="display:flex;align-items:center;gap:10px;margin-top:10px"><span class="sw"><input type="checkbox" id="mesh-sds-group"><i></i></span><span style="color:var(--muted);font-size:12px">Destination is group/GSSI</span></label>
              <textarea id="mesh-sds-sources" class="form-input" rows="3" placeholder="Allowed MeshCom sources, empty = all" style="margin-top:10px"></textarea>
            </div>
            <div>
              <label class="sw-row">
                <span class="sw-text">Forward MeshCom → SIP/Snom</span>
                <span class="sw"><input type="checkbox" id="mesh-forward-sip"><i></i></span>
              </label>
              <input type="text" id="mesh-sip-prefix" class="form-input" placeholder="Snom title prefix" style="margin-top:10px">
              <textarea id="mesh-sip-sources" class="form-input" rows="3" placeholder="Allowed MeshCom sources, empty = all" style="margin-top:10px"></textarea>
            </div>
            <div>
              <label class="sw-row">
                <span class="sw-text">Forward MeshCom → Telegram</span>
                <span class="sw"><input type="checkbox" id="mesh-forward-telegram"><i></i></span>
              </label>
              <input type="text" id="mesh-telegram-prefix" class="form-input" placeholder="Telegram prefix" style="margin-top:10px">
              <textarea id="mesh-telegram-sources" class="form-input" rows="3" placeholder="Allowed MeshCom sources, empty = all" style="margin-top:10px"></textarea>
            </div>
          </div>
          <div class="help-text" style="margin-top:10px">Source filters match MeshCom packet src values case-insensitively. Empty filters forward every MeshCom text message.</div>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title">Send MeshCom Message</div>
          <div class="card-actions">
            <button class="btn btn-primary" onclick="sendMeshcomMessage()">Send</button>
          </div>
        </div>
        <div class="card-body">
          <div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:10px;align-items:center">
            <label style="color:var(--muted);font-size:13px">Destination</label>
            <input type="text" id="mesh-out-dst" class="form-input" placeholder="CALLSIGN, group or *">
            <label style="color:var(--muted);font-size:13px;align-self:flex-start;padding-top:8px">Message</label>
            <textarea id="mesh-out-msg" class="form-input" rows="3" maxlength="512" placeholder="Message text"></textarea>
          </div>
          <div class="config-msg" id="mesh-send-msg"></div>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title">MeshCom Nodes</div>
          <div class="card-actions">
            <input type="text" id="mesh-node-filter" class="form-input" style="width:240px" placeholder="Search node, HW-ID or firmware" oninput="meshNodePageIndex=0;renderMeshcomNodes()">
          </div>
        </div>
        <div class="card-body">
          <div class="table-wrap">
            <table>
              <thead><tr>
                <th>Node</th>
                <th>Last seen</th>
                <th>Position</th>
                <th>Battery</th>
                <th>RF</th>
                <th>Firmware</th>
                <th>HW-ID</th>
              </tr></thead>
              <tbody id="mesh-nodes-tbody"></tbody>
            </table>
          </div>
          <div class="log-controls">
            <button class="btn btn-sm" onclick="meshNodePrevPage()">‹ Prev</button>
            <span class="sds-empty" id="mesh-nodes-page">Page 1 / 1</span>
            <button class="btn btn-sm" onclick="meshNodeNextPage()">Next ›</button>
          </div>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title">MeshCom Messages</div>
        </div>
        <div class="card-body">
          <div class="table-wrap">
            <table>
              <thead><tr>
                <th data-i18n="th_time">Time</th>
                <th data-i18n="th_dir">Dir</th>
                <th data-i18n="th_type">Type</th>
                <th>Source</th>
                <th>Destination</th>
                <th data-i18n="th_message">Message</th>
                <th>Paths</th>
                <th>Position / RF</th>
              </tr></thead>
              <tbody id="mesh-msgs-tbody"></tbody>
            </table>
          </div>
          <div class="log-controls">
            <button class="btn btn-sm" onclick="meshMsgPrevPage()">‹ Prev</button>
            <span class="sds-empty" id="mesh-msgs-page">Page 1 / 1</span>
            <button class="btn btn-sm" onclick="meshMsgNextPage()">Next ›</button>
          </div>
        </div>
      </div>
    </div>

    <!-- ── CONFIG ── -->
    <div class="page" id="page-config">
      <div class="section-label" data-i18n="cfg_sec_configuration">Configuration</div>
      <div class="card">
        <div class="card-head">
          <div class="card-title">config.toml</div>
          <div class="card-actions">
            <button class="btn btn-primary" onclick="saveConfig()"><span class="btn-icon" data-icon="save"></span><span data-i18n="save">Save</span></button>
            <span class="btn-group danger-group">
              <button class="btn btn-warn" onclick="restartService()"><span class="btn-icon" data-icon="restart"></span><span data-i18n="restart">Restart</span></button>
              <button class="btn btn-danger" onclick="shutdownService()"><span class="btn-icon" data-icon="shutdown"></span><span data-i18n="shutdown">Shutdown</span></button>
              <button class="btn" id="update-btn" onclick="startUpdate()"><span class="btn-icon" data-icon="update"></span><span data-i18n="update">Update</span></button>
            </span>
          </div>
        </div>
        <div class="card-body">
          <textarea id="config-editor" spellcheck="false" placeholder="Loading..."></textarea>
          <div class="config-msg" id="config-msg"></div>
        </div>
      </div>

      <!-- ── ISSI WHITELIST ──
           Editable access-control list. Empty list = open network (any ISSI may
           register). Changes apply immediately at runtime AND are written back to
           config.toml so they survive a restart. -->
      <div class="section-label" data-i18n="cfg_sec_access">Access Control</div>
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="whitelist_title">ISSI Whitelist</div>
          <div class="card-actions">
            <span id="whitelist-status" class="badge" style="margin-right:8px"></span>
            <button class="btn btn-primary" onclick="saveWhitelist()"><span class="btn-icon" data-icon="save"></span><span data-i18n="save">Save</span></button>
          </div>
        </div>
        <div class="card-body">
          <div style="color:var(--muted);font-size:13px;margin-bottom:12px" data-i18n="whitelist_help">
            When the list is empty, any radio may register (open network). When non-empty,
            only the listed ISSIs are accepted; all others are rejected. Changes apply
            instantly and persist across restarts.
          </div>
          <div style="display:flex;gap:8px;margin-bottom:12px;flex-wrap:wrap">
            <input type="number" id="whitelist-input" class="form-input" min="1" max="16777215"
                   placeholder="e.g. 2260571" style="flex:1;min-width:160px"
                   onkeydown="if(event.key==='Enter'){addWhitelistEntry();}">
            <button class="btn" onclick="addWhitelistEntry()"><span class="btn-icon" data-icon="add"></span><span data-i18n="whitelist_add">Add ISSI</span></button>
          </div>
          <div id="whitelist-chips" style="display:flex;gap:8px;flex-wrap:wrap;min-height:32px"></div>
          <div class="config-msg" id="whitelist-msg"></div>
        </div>
      </div>

      <!-- ── WX / METAR SERVICE ──
           Built-in weather responder. On-demand: a radio SDSes "METAR <ICAO>" to the
           service ISSI and gets a decoded reply. Periodic: auto-sends a station's METAR
           to a chosen ISSI/GSSI at an interval. Toggles + targets editable here; applies
           instantly and persists to config.toml. -->
      <div class="section-label" data-i18n="cfg_sec_wx">WX / METAR</div>
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="wx_title">WX / METAR Service</div>
          <div class="card-actions">
            <button class="btn btn-primary" onclick="saveWx()"><span class="btn-icon" data-icon="save"></span><span data-i18n="save">Save</span></button>
          </div>
        </div>
        <div class="card-body">
          <div style="color:var(--muted);font-size:13px;margin-bottom:14px" data-i18n="wx_help">
            Built-in weather service. Radios send an SDS like "METAR LROP" to the service
            ISSI to get a decoded report. Optionally auto-send a fixed station's METAR to an
            ISSI or talkgroup at a set interval. Data from aviationweather.gov.
          </div>

          <div class="group-list" style="margin-bottom:18px">
            <label class="field" style="cursor:pointer">
              <span class="field-label" data-i18n="wx_enabled">Enable on-demand METAR responder</span>
              <span class="field-control"><span class="sw"><input type="checkbox" id="wx-enabled"><i></i></span></span>
            </label>
            <div class="field">
              <span class="field-label" data-i18n="wx_service_issi">Service ISSI</span>
              <span class="field-control"><input type="number" id="wx-service-issi" class="form-input" min="1" max="16777215"
                     placeholder="9998" style="width:160px"></span>
            </div>
          </div>

          <div class="group-list">
            <label class="field" style="cursor:pointer">
              <span class="field-label" data-i18n="wx_periodic_enabled">Enable periodic auto-broadcast</span>
              <span class="field-control"><span class="sw"><input type="checkbox" id="wx-periodic-enabled"><i></i></span></span>
            </label>
            <div class="field">
              <span class="field-label" data-i18n="wx_periodic_icao">Station ICAO</span>
              <span class="field-control"><input type="text" id="wx-periodic-icao" class="form-input" maxlength="4" placeholder="LROP" style="text-transform:uppercase;width:160px"></span>
            </div>
            <div class="field">
              <span class="field-label" data-i18n="wx_periodic_dest">Destination</span>
              <span class="field-control"><input type="number" id="wx-periodic-issi" class="form-input" min="1" max="16777215" placeholder="ISSI or GSSI" style="width:160px"></span>
            </div>
            <label class="field" style="cursor:pointer">
              <span class="field-label" data-i18n="wx_periodic_isgroup">Destination is group</span>
              <span class="field-control">
                <span style="color:var(--muted);font-size:12px" data-i18n="wx_periodic_isgroup_hint">(GSSI instead of individual ISSI)</span>
                <span class="sw"><input type="checkbox" id="wx-periodic-isgroup"><i></i></span>
              </span>
            </label>
            <div class="field">
              <span class="field-label" data-i18n="wx_periodic_interval">Interval (seconds)</span>
              <span class="field-control"><input type="number" id="wx-periodic-interval" class="form-input" min="300" placeholder="1800" style="width:160px"></span>
              <span class="field-hint" data-i18n="wx_interval_hint">Minimum 300 s (5 min) to avoid hammering the weather API.</span>
            </div>
          </div>
          <div class="config-msg" id="wx-msg"></div>
        </div>
      </div>
    </div>

    <!-- ── TELEGRAM ALERTS ──
         Owner-facing push notifications via a Telegram bot. The owner pastes their
         @BotFather token, detects their chat ID with one click (getUpdates), picks
         which categories to receive, and saves. Applies instantly and persists to
         config.toml. -->
    <div class="page" id="page-telegram">
      <div class="section-label" data-i18n="integrations">Integrations</div>
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="tg_title">Telegram Alerts</div>
          <div class="card-actions">
            <button class="btn" onclick="testTelegram()"><span class="btn-icon" data-icon="telegram"></span><span data-i18n="tg_test">Send test</span></button>
            <button class="btn btn-primary" onclick="saveTelegram()"><span class="btn-icon" data-icon="save"></span><span data-i18n="save">Save</span></button>
          </div>
        </div>
        <div class="card-body">
          <div class="help-text" style="margin-bottom:6px" data-i18n="tg_help">
            Get instant Telegram messages when something happens on the station.
          </div>
          <label class="sw-row">
            <span class="sw-text" data-i18n="tg_enabled">Enable Telegram alerts</span>
            <span class="sw"><input type="checkbox" id="tg-enabled"><i></i></span>
          </label>
          <div class="config-msg" id="tg-msg"></div>
        </div>
      </div>

      <div class="card">
        <div class="card-head"><div class="card-title" data-i18n="tg_howto_title">Setup — 4 steps</div></div>
        <div class="card-body">
          <div class="steps">
            <div class="step"><span class="step-num"></span><span class="step-body" data-i18n="tg_step1">In Telegram, open @BotFather, send /newbot and copy the bot token.</span></div>
            <div class="step"><span class="step-num"></span><span class="step-body" data-i18n="tg_step2">Paste the token below and click Verify.</span></div>
            <div class="step"><span class="step-num"></span><span class="step-body" data-i18n="tg_step3">Send your bot any message, e.g. /start.</span></div>
            <div class="step"><span class="step-num"></span><span class="step-body" data-i18n="tg_step4">Click Detect Chat ID, add your chat, then Save.</span></div>
          </div>
        </div>
      </div>

      <div class="card">
        <div class="card-head"><div class="card-title" data-i18n="tg_bot_title">Bot token</div></div>
        <div class="card-body">
          <div style="color:var(--muted);font-size:13px;margin-bottom:12px" data-i18n="tg_bot_help">
            The token from @BotFather looks like 123456789:AAExampleTokenString.
          </div>
          <div style="display:flex;gap:8px;flex-wrap:wrap">
            <input type="text" id="tg-token" class="form-input" placeholder="123456789:AA…"
                   autocomplete="off" spellcheck="false" oninput="tgTokenDirty=true"
                   style="flex:1;min-width:220px">
            <button class="btn" onclick="verifyTelegram()"><span class="btn-icon" data-icon="search"></span><span data-i18n="tg_verify">Verify</span></button>
          </div>
          <div id="tg-verify-status" style="margin-top:8px;font-size:13px;min-height:18px"></div>
        </div>
      </div>

      <div class="card">
        <div class="card-head"><div class="card-title" data-i18n="tg_recipients_title">Recipients (Chat IDs)</div></div>
        <div class="card-body">
          <div style="color:var(--muted);font-size:13px;margin-bottom:12px" data-i18n="tg_recipients_help">
            Every alert is sent to each recipient.
          </div>
          <div style="display:flex;gap:8px;margin-bottom:12px;flex-wrap:wrap">
            <button class="btn" onclick="detectTelegramChats()"><span class="btn-icon" data-icon="detect"></span><span data-i18n="tg_detect">Detect Chat ID</span></button>
            <input type="number" id="tg-chat-input" class="form-input" placeholder="-1001234567890"
                   style="flex:1;min-width:180px" onkeydown="if(event.key==='Enter'){addRecipient();}">
            <button class="btn" onclick="addRecipient()"><span class="btn-icon" data-icon="add"></span><span data-i18n="tg_add">Add</span></button>
          </div>
          <div id="tg-detected" style="margin-bottom:10px"></div>
          <div id="tg-chips" style="display:flex;gap:8px;flex-wrap:wrap;min-height:32px"></div>
          <div class="config-msg" id="tg-recipients-msg"></div>
        </div>
      </div>

      <div class="card">
        <div class="card-head"><div class="card-title" data-i18n="tg_categories_title">Alert categories</div></div>
        <div class="card-body" style="padding-top:4px;padding-bottom:4px">
          <label class="sw-row"><span class="sw-text" data-i18n="tg_cat_connect">Radio connected</span><span class="sw"><input type="checkbox" id="tg-connect"><i></i></span></label>
          <label class="sw-row"><span class="sw-text" data-i18n="tg_cat_disconnect">Radio disconnected</span><span class="sw"><input type="checkbox" id="tg-disconnect"><i></i></span></label>
          <label class="sw-row"><span class="sw-text" data-i18n="tg_cat_t351">Radio dropped (no T351 response)</span><span class="sw"><input type="checkbox" id="tg-t351"><i></i></span></label>
          <label class="sw-row"><span class="sw-text" data-i18n="tg_cat_lip">LIP/APRS position beacon</span><span class="sw"><input type="checkbox" id="tg-lip"><i></i></span></label>
          <label class="sw-row"><span class="sw-text" data-i18n="tg_cat_backhaul">Brew backhaul up/down</span><span class="sw"><input type="checkbox" id="tg-backhaul"><i></i></span></label>
          <label class="sw-row"><span class="sw-text" data-i18n="tg_cat_logs">Critical log (warnings/errors)</span><span class="sw"><input type="checkbox" id="tg-logs"><i></i></span></label>
        </div>
      </div>
    </div>

    <!-- ── WIFI ──
         Three cards: current status (with disconnect / radio toggle), saved
         profiles list, and visible networks scan. The whole tab is only
         attached to a nav button when /api/wifi/available reports true so
         we never tease functionality the host can't deliver. -->
    <div class="page" id="page-wifi">
      <div class="section-label" data-i18n="integrations">Integrations</div>
      <!-- Status card: who we're connected to right now, IP, signal -->
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="wifi_status">Current connection</div>
          <div class="card-actions">
            <button class="btn btn-sm" id="wifi-radio-btn" onclick="wifiToggleRadio()" data-i18n="wifi_radio_off">Disable WiFi</button>
            <button class="btn btn-sm" onclick="wifiRefresh()"><span class="btn-icon" data-icon="restart"></span><span data-i18n="wifi_refresh">Refresh</span></button>
          </div>
        </div>
        <div class="card-body" style="padding:0">
          <!-- Connection safety warning: changing WiFi while connected through
               it can lock the operator out of the dashboard. -->
          <div class="banner banner-warn">
            <span class="banner-ico" data-icon="alert"></span>
            <div class="banner-body" data-i18n="wifi_warn_lose_access">If you're connected to the dashboard via WiFi, changing networks may temporarily disconnect you. Make sure you have a backup access path (Ethernet or known good network).</div>
          </div>
          <div class="wifi-status-grid" id="wifi-status-grid" style="padding:16px 18px">
            <div class="wifi-status-loading" data-i18n="wifi_loading">Loading…</div>
          </div>
        </div>
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
            <button class="btn btn-sm" onclick="wifiShowHiddenModal()"><span class="btn-icon" data-icon="add"></span><span data-i18n="wifi_add_hidden">Hidden network</span></button>
            <button class="btn btn-sm" onclick="wifiScan()"><span class="btn-icon" data-icon="restart"></span><span data-i18n="wifi_scan">Scan</span></button>
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
         security and when adding a hidden network manually. Unified .sheet. -->
    <div id="wifi-modal" class="sheet-overlay">
      <div class="sheet">
        <div class="sheet-head">
          <div class="sheet-title" id="wifi-modal-title">Connect</div>
          <button class="sheet-close" onclick="wifiCloseModal()"><span data-icon="close"></span></button>
        </div>
        <div class="sheet-body">
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
          <div class="wifi-modal-foot">
            <button class="btn" onclick="wifiCloseModal()" data-i18n="cancel">Cancel</button>
            <button class="btn btn-primary" id="wifi-modal-ok" onclick="wifiModalSubmit()" data-i18n="wifi_connect">Connect</button>
          </div>
        </div>
      </div>
    </div>

    <!-- ── SYSTEM ── -->
    <div class="page" id="page-health">
      <div class="h-wrap">
        <div id="health-hero" class="h-hero">
          <div id="health-hero-dot" class="h-ring">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>
          </div>
          <div class="h-hero-txt">
            <div id="health-hero-title" class="h-hero-title">Station health</div>
            <div id="health-hero-sub" class="h-hero-sub">Waiting for the first health snapshot…</div>
          </div>
          <div class="h-hero-meta">
            <div id="health-uptime" class="hm-val">—</div>
            <div id="health-action" class="hm-sub"></div>
          </div>
        </div>
        <div class="h-sec">System domains</div>
        <div id="health-grid" class="h-grid"></div>
        <div class="h-sec">Integrations</div>
        <div id="health-integrations-grid" class="h-grid">
          <div class="sds-empty" style="padding:12px 0">Loading integration health…</div>
        </div>
        <div class="h-note">
          Auto-refreshes every few seconds. Levels:
          <b class="ok">OK</b> · <b class="warn">DEGRADED</b> · <b class="bad">CRITICAL</b>.
          The software watchdog (auto-restart when the core loop stalls) is configured in the <code>[health]</code> section.
        </div>
      </div>
    </div>

    <div class="page" id="page-system">
      <!-- System hero — at-a-glance BTS / Brew / uptime / CPU temp summary. -->
      <div class="hero">
        <span class="hero-dot is-idle" id="sysHeroDot"></span>
        <div class="hero-main">
          <div class="hero-title" id="sysHeroTitle" data-i18n="sys_title">System</div>
          <div class="hero-sub" id="sysHeroSub">—</div>
        </div>
        <div class="hero-metrics">
          <div class="hero-metric">
            <div class="hero-metric-label" data-i18n="sys_uptime">Uptime</div>
            <div class="hero-metric-value" id="sysHeroUptime">—</div>
          </div>
          <div class="hero-metric">
            <div class="hero-metric-label" data-i18n="sys_temp">CPU Temp</div>
            <div class="hero-metric-value" id="sysHeroTemp">—</div>
          </div>
        </div>
      </div>

      <!-- BTS + Brew status -->
      <div class="section-label" data-i18n="sys_sec_status">Status</div>
      <div class="stat-grid" style="grid-template-columns:repeat(auto-fit,minmax(180px,1fr))">
        <div class="stat-card is-danger" id="sysBtsCard">
          <div class="stat-label" data-i18n="sys_bts">BTS Connection</div>
          <div class="stat-value is-text" id="sysBtsStatus">OFFLINE</div>
          <div class="stat-sub" id="sysBtsIp">—</div>
        </div>
        <div class="stat-card is-danger" id="sysBrewCard">
          <div class="stat-label">BREW</div>
          <div class="stat-value is-text" id="sysBrewStatus">OFFLINE</div>
          <div class="stat-sub" id="sysBrewBadge">—</div>
        </div>
        <div class="stat-card is-idle">
          <div class="stat-label" data-i18n="sys_uptime">Uptime</div>
          <div class="stat-value is-text" id="sysUptime">—</div>
          <div class="stat-sub" id="sysHostname">—</div>
        </div>
        <div class="stat-card is-warn" id="cpu-temp-card" style="display:none">
          <div class="stat-label" data-i18n="sys_temp">CPU Temp</div>
          <div class="stat-value is-text" id="sysCpuTemp">—</div>
          <div class="stat-sub" id="sysCpuTempSub">—</div>
        </div>
      </div>

      <!-- Display brightness (FH-FEAT-008) — hidden unless a backlight panel exists -->
      <div class="card" id="brightness-card" style="display:none">
        <div class="card-head">
          <div class="card-title">Display Brightness</div>
          <div class="card-actions"><span id="brightness-val" style="font-family:var(--mono);font-size:13px;color:var(--text2)">—</span></div>
        </div>
        <div class="card-body" style="padding:16px 18px">
          <input type="range" id="brightness-slider" min="0" max="255" step="1" value="128" oninput="onBrightnessInput(this.value)" style="width:100%">
        </div>
      </div>

      <!-- System info + CPU/RAM -->
      <div class="section-label" data-i18n="sys_sec_host">Host</div>
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="sys_info">System Info</div>
          <div class="card-actions" style="display:flex;align-items:center;gap:10px">
            <label style="display:flex;align-items:center;gap:5px;font-size:12px;color:var(--text2);cursor:pointer">
              <input type="checkbox" id="sys-autorefresh" onchange="toggleSysAutoRefresh(this.checked)" style="cursor:pointer">
              <span data-i18n="sys_autorefresh">Auto-refresh 5s</span>
            </label>
            <button class="btn btn-sm" onclick="loadSystemInfo()"><span class="btn-icon" data-icon="restart"></span><span data-i18n="sys_refresh">Refresh</span></button>
          </div>
        </div>
        <div class="card-body">
          <div class="info-row"><div class="info-key" data-i18n="sys_version">FS Version</div><div class="info-val accent" id="sysVersion">—</div></div>
          <div class="info-row"><div class="info-key" data-i18n="sys_os">OS</div><div class="info-val" id="sysOs">—</div></div>
          <div class="info-row"><div class="info-key" data-i18n="sys_config">Active Config</div><div class="info-val" id="sysConfigPath">—</div></div>
          <div class="info-row"><div class="info-key" data-i18n="sys_cpu">CPU</div><div class="info-val" id="sysCpu">—</div></div>
          <div class="info-row">
            <div class="info-key" data-i18n="sys_cpu_load">CPU Load</div>
            <div class="info-val" style="flex:1;max-width:220px">
              <div class="gauge" id="sysCpuGauge">
                <div class="gauge-track"><div class="gauge-fill" id="sysCpuBar"></div></div>
                <span class="gauge-value" id="sysCpuPct">—</span>
              </div>
            </div>
          </div>
          <div class="info-row">
            <div class="info-key" data-i18n="sys_ram">RAM</div>
            <div class="info-val" style="flex:1;max-width:260px">
              <div class="gauge is-info" id="sysRamGauge">
                <div class="gauge-track"><div class="gauge-fill" id="sysRamBar"></div></div>
                <span class="gauge-value" id="sysRamVal" style="min-width:118px">—</span>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- RF / SDR Hardware -->
      <div class="section-label" data-i18n="sys_sec_radio">Radio Hardware</div>
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="sys_rf">RF Hardware (SoapySDR)</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="loadSystemInfo()"><span class="btn-icon" data-icon="search"></span><span data-i18n="sys_probe">Probe</span></button>
          </div>
        </div>
        <div class="card-body">
          <pre id="sysSoapy" class="terminal">—</pre>
        </div>
      </div>

      <!-- Host hardware sensors (temps, voltages, currents, power) -->
      <!-- Populated from /sys via sys_telemetry. Layout adapts: if no sensors are
           found (non-Linux, locked-down kernel) the whole card is hidden. -->
      <div class="section-label" id="sys-sensors-label" data-i18n="sys_sec_sensors" style="display:none">Sensors</div>
      <div class="card" id="sys-sensors-card" style="display:none">
        <div class="card-head">
          <div class="card-title" data-i18n="sys_sensors">Host Hardware Sensors</div>
          <div class="card-actions">
            <span id="sys-sensors-power-total" style="font-family:var(--mono);font-size:12px;color:var(--accent2);font-weight:600"></span>
          </div>
        </div>
        <div class="card-body" style="padding:14px 18px">
          <div id="sys-sensors-empty" style="font-size:12px;color:var(--text3);font-style:italic;display:none" data-i18n="sys_sensors_empty">No sensors detected on this host.</div>
          <div id="sys-sensors-grid" style="display:grid;grid-template-columns:repeat(auto-fill, minmax(160px, 1fr));gap:8px"></div>
        </div>
      </div>

      <!-- Config profiles -->
      <div class="section-label" data-i18n="sys_sec_profiles">Profiles</div>
      <div class="card">
        <div class="card-head">
          <div class="card-title" data-i18n="sys_profiles">Config Profiles</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="loadConfigProfiles()"><span class="btn-icon" data-icon="restart"></span><span data-i18n="sys_refresh">Refresh</span></button>
          </div>
        </div>
        <div class="card-body" style="padding:14px 18px">
          <div id="profileList"></div>
        </div>
      </div>

      <!-- Live SDS Broadcast -->
      <div class="section-label" data-i18n="sys_sec_sds">SDS Broadcast</div>
      <div class="card">
        <div class="card-head">
          <div class="card-title" style="display:flex;align-items:center;gap:7px"><span class="btn-icon" data-icon="broadcast" style="margin:0;width:14px;height:14px"></span>Live SDS Broadcast</div>
          <div class="card-actions">
            <button class="btn btn-sm" onclick="loadLiveSds()"><span class="btn-icon" data-icon="restart"></span><span data-i18n="sys_refresh">Refresh</span></button>
            <button class="btn btn-sm btn-danger" onclick="clearAllLiveSds()" id="live-sds-clear-btn" style="display:none"><span class="btn-icon" data-icon="delete"></span><span data-i18n="live_sds_clear_all">Clear All</span></button>
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
            <button class="btn btn-primary" onclick="addLiveSds()"><span class="btn-icon" data-icon="broadcast"></span><span data-i18n="live_sds_send">Broadcast</span></button>
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
    <div class="modal-title" style="display:flex;align-items:center;gap:7px">
      <span class="btn-icon" data-icon="edit" style="margin:0"></span><span data-i18n="profile_edit_title">Edit Config Profile</span>:
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
    <div class="form-row">
      <label class="form-label" style="display:flex;align-items:center;gap:8px">
        <input type="checkbox" id="sds-callout" onchange="toggleSdsCallout()">
        <span data-i18n="sds_callout_enable">TPG2200 Call-Out / Alarm senden</span>
      </label>
    </div>
    <div id="sds-callout-fields" style="display:none">
      <div class="form-row">
        <label class="form-label" data-i18n="sds_callout_source">Source ISSI</label>
        <input type="number" id="sds-callout-source" class="form-input" value="9999" min="1">
      </div>
      <div class="form-row">
        <label class="form-label" data-i18n="sds_callout_incident">Vorfallnummer</label>
        <input type="number" id="sds-callout-incident" class="form-input" value="1" min="1" max="256">
      </div>
      <div class="form-row">
        <label class="form-label" data-i18n="sds_callout_text">Alarmtext</label>
        <input type="text" id="sds-callout-text" class="form-input" value="ALARM" maxlength="120">
      </div>
      <div class="form-row">
        <label class="form-label" data-i18n="sds_callout_raw">Raw Hex Payload optional</label>
        <input type="text" id="sds-callout-raw" class="form-input" placeholder="C3 00 09 0D 10 11 27 0F 02 30 8D 41 4C 41 52 4D">
      </div>
      <div class="form-row" style="font-size:12px;color:var(--muted);line-height:1.45" data-i18n="sds_callout_help">
        Vorfall 1-15 use the confirmed byte formula (N &lt;&lt; 4) | 0x01: 1=11, 2=21, 3=31, 4=41. Vorfall 16-256 use the extended one-byte selector. Raw Hex overrides automatic payload generation.
      </div>
    </div>
    <div class="modal-actions">
      <button class="btn" onclick="closeSdsModal()" data-i18n="cancel">Cancel</button>
      <button class="btn btn-primary" onclick="sendSds()" data-i18n="send">Send</button>
    </div>
  </div>
</div>

<!-- ── DGNA Modal (Dynamic Group Number Assignment) ── -->
<div class="modal-overlay" id="dgna-modal">
  <div class="modal">
    <div class="modal-title" data-i18n="dgna_modal_title">⬡ Dynamic Group Assignment</div>
    <div class="form-row">
      <label class="form-label" data-i18n="dgna_issi">Terminal ISSI</label>
      <input type="number" id="dgna-issi" class="form-input" readonly>
    </div>
    <div class="form-row">
      <label class="form-label" data-i18n="dgna_current">Current groups</label>
      <div id="dgna-current" style="display:flex;flex-wrap:wrap;gap:4px;min-height:22px;align-items:center">—</div>
    </div>
    <div class="form-row">
      <label class="form-label" data-i18n="dgna_gssi">Group (GSSI)</label>
      <input type="number" id="dgna-gssi" class="form-input" placeholder="e.g. 100" min="1">
    </div>
    <div class="modal-actions">
      <button class="btn" onclick="closeDgnaModal()" data-i18n="cancel">Cancel</button>
      <button class="btn btn-danger" onclick="sendDgna(false)" data-i18n="dgna_deassign">Deassign</button>
      <button class="btn btn-primary" onclick="sendDgna(true)" data-i18n="dgna_assign">Assign</button>
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
// ── Icon system (SF-Symbols-style, design-language v3) ────────────────────
// One cohesive family: 24×24 viewBox, fill=none, stroke=currentColor,
// stroke-width 1.8, round caps/joins — monochrome so each glyph inherits the
// adjacent text colour and auto-themes. svgIcon(name[,size]) returns an inline
// <svg> string; status is conveyed by the dot, never the icon. The Tabs phase
// reuses ICONS / svgIcon verbatim for every emoji site.
const ICONS = {
  // nav — monitor
  radios:'<path d="M5 14a9 9 0 0 1 9-9"/><path d="M5 14a5.5 5.5 0 0 1 5.5-5.5"/><circle cx="6.5" cy="12.5" r="1.6"/><path d="M7.5 13.5 13 19"/>',
  calls:'<path d="M6.5 4.5h3l1.2 3.2-1.7 1.3a11 11 0 0 0 4.7 4.7l1.3-1.7 3.2 1.2v3a1.5 1.5 0 0 1-1.6 1.5A13.5 13.5 0 0 1 5 6.1 1.5 1.5 0 0 1 6.5 4.5Z"/>',
  lastheard:'<path d="M4 12h2M8 8v8M12 5v14M16 8v8M20 12h-2"/>',
  log:'<rect x="5" y="4" width="14" height="16" rx="2.5"/><path d="M9 9h6M9 13h6M9 17h3"/>',
  sdslog:'<path d="M4.5 6.5A1.5 1.5 0 0 1 6 5h12a1.5 1.5 0 0 1 1.5 1.5v8A1.5 1.5 0 0 1 18 16H9l-4 3v-3a1.5 1.5 0 0 1-.5-1.1Z"/>',
  rf:'<circle cx="12" cy="12" r="2"/><path d="M7.8 7.8a6 6 0 0 0 0 8.4M16.2 7.8a6 6 0 0 1 0 8.4M5 5a9 9 0 0 0 0 14M19 5a9 9 0 0 1 0 14"/>',
  health:'<path d="M3 12h3l2-5 3 10 2.5-7 1.5 2h6"/>',
  // nav — integrations / system
  config:'<circle cx="12" cy="12" r="3"/><path d="M12 2.5v2.5M12 19v2.5M4.2 4.2l1.8 1.8M18 18l1.8 1.8M2.5 12H5M19 12h2.5M4.2 19.8 6 18M18 6l1.8-1.8"/>',
  telegram:'<path d="M20 4 3.5 11.2l6 2.1M20 4l-2.8 14-7-3.6M20 4 9.6 13.6M9.6 13.6V18l2.6-2.6"/>',
  wifi:'<path d="M4.5 9a11 11 0 0 1 15 0M7.5 12.5a6.5 6.5 0 0 1 9 0"/><circle cx="12" cy="16.5" r="1.2" fill="currentColor" stroke="none"/>',
  system:'<rect x="6" y="6" width="12" height="12" rx="2.5"/><rect x="9.5" y="9.5" width="5" height="5" rx="1"/><path d="M9 3.5v2.5M15 3.5v2.5M9 18v2.5M15 18v2.5M3.5 9H6M3.5 15H6M18 9h2.5M18 15h2.5"/>',
  asterisk:'<circle cx="12" cy="12" r="7.5"/><path d="M12 7.5v9M8.1 9.75l7.8 4.5M15.9 9.75l-7.8 4.5"/>',
  dapnet:'<path d="M6.5 16v-4a5.5 5.5 0 0 1 11 0v4l1.5 2h-14Z"/><path d="M10.5 18.5a1.6 1.6 0 0 0 3 0"/>',
  geoalarm:'<path d="M12 21s6.5-5.4 6.5-10.5A6.5 6.5 0 0 0 5.5 10.5C5.5 15.6 12 21 12 21Z"/><circle cx="12" cy="10.3" r="2.3"/>',
  overview:'<rect x="4" y="4" width="7" height="7" rx="1.6"/><rect x="13" y="4" width="7" height="7" rx="1.6"/><rect x="4" y="13" width="7" height="7" rx="1.6"/><rect x="13" y="13" width="7" height="7" rx="1.6"/>',
  // kpi / domain
  network:'<path d="M9.5 14.5 14.5 9.5M8 12l-1.8 1.8a3.4 3.4 0 0 0 4.8 4.8L13 16.5M16 11.5l1.8-1.8a3.4 3.4 0 0 0-4.8-4.8L11 6.5"/>',
  backhaul:'<path d="M5 12a7 7 0 0 1 7-7M5 12a4 4 0 0 1 4-4"/><circle cx="6" cy="11" r="1.4"/><path d="M16 8l4 4M15 13l-3 3 5 0Z"/>',
  congestion:'<path d="M6 19v-5M12 19V8M18 19v-9"/>',
  // actions
  save:'<path d="M5 12.5 10 17.5 19 7"/>',
  restart:'<path d="M19 12a7 7 0 1 1-2.1-5"/><path d="M17 4v3.5h-3.5"/>',
  shutdown:'<path d="M12 4v7"/><path d="M7.5 7.2a7 7 0 1 0 9 0"/>',
  update:'<path d="M12 19V6M7 11l5-5 5 5"/><path d="M6 4h12"/>',
  edit:'<path d="M14.5 5.5 18.5 9.5 8 20H4v-4Z"/><path d="M13 7 17 11"/>',
  add:'<path d="M12 5v14M5 12h14"/>',
  delete:'<path d="M5 7h14M9 7V5h6v2M6.5 7l.8 12a1.5 1.5 0 0 0 1.5 1.4h6.4a1.5 1.5 0 0 0 1.5-1.4L17.5 7"/>',
  export:'<path d="M12 4v10M8 10l4 4 4-4M5 18h14"/>',
  search:'<circle cx="11" cy="11" r="6"/><path d="m20 20-3.5-3.5"/>',
  detect:'<path d="M4 14v4.5a1.5 1.5 0 0 0 1.5 1.5h13a1.5 1.5 0 0 0 1.5-1.5V14M8 11l4 4 4-4M12 4v11"/>',
  broadcast:'<path d="M4 10v4l9 4V6Z"/><path d="M13 8a4 4 0 0 1 0 8M6 14v3.5a1.5 1.5 0 0 0 3 0V15"/>',
  // status / domain
  alert:'<path d="M12 4.5 21 19H3Z"/><path d="M12 10v4M12 16.5v.2"/>',
  emergency:'<path d="M12 3 19 6v5c0 4.5-3 7.6-7 9-4-1.4-7-4.5-7-9V6Z"/><path d="M12 8v4M12 15v.2"/>',
  power:'<path d="M13 3 5 13h6l-1 8 8-10h-6Z"/>',
  login:'<circle cx="8" cy="12" r="3.5"/><path d="M11.5 12H20M17 12v3M20 12v2.5"/>',
  // chrome
  collapse:'<path d="M14 7l-5 5 5 5"/><path d="M19 5v14"/>',
  hamburger:'<path d="M4 7h16M4 12h16M4 17h16"/>',
  close:'<path d="M6 6l12 12M18 6 6 18"/>',
};
// Glyphs that read better at a heavier weight (checkmarks, plus, close).
const ICON_BOLD = { save:1, add:1 };
function svgIcon(name, size){
  const body = ICONS[name]; if(body===undefined) return '';
  const sw = ICON_BOLD[name] ? 2 : 1.8;
  const px = size ? ' width="'+size+'" height="'+size+'"' : '';
  return '<svg viewBox="0 0 24 24"'+px+' fill="none" stroke="currentColor" stroke-width="'+sw+
         '" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">'+body+'</svg>';
}
// Filled selection marker (▶ in selected-TG rows) — own fill, no stroke.
const ICON_MARKER = '<svg viewBox="0 0 24 24" fill="currentColor" stroke="none" aria-hidden="true"><path d="M8 5l11 7-11 7Z"/></svg>';
// Paint every declarative icon slot ([data-icon="name"]) from the ICONS map.
// Keeps the nav/header markup DRY; the Tabs phase can drop more [data-icon] slots.
function paintIcons(root){
  (root||document).querySelectorAll('[data-icon]').forEach(function(el){
    if(el.dataset.iconPainted) return;
    el.innerHTML = svgIcon(el.getAttribute('data-icon'));
    el.dataset.iconPainted = '1';
  });
}

// ── i18n ─────────────────────────────────────────────────────────────────
const LANGS={
  en:{
    bts_ip:'BTS IP',offline:'OFFLINE',online:'ONLINE',
    brew_online:'ONLINE',brew_offline:'OFFLINE',
    stations:'Radios',calls:'Calls',lastheard:'Last Heard',log:'Log',rf:'RF',health:'Health',asterisk:'Asterisk SIP',dapnet:'DAPNET',echolink:'EchoLink',echolink_title:'EchoLink',meshcom:'MeshCom',meshcom_title:'MeshCom',geoalarm:'GeoAlarm',geoalarm_title:'GeoAlarm',config:'Config',
    sdslog:'SDS Log',th_dir:'Dir',th_from:'From',th_to:'To',th_message:'Message',no_sds:'No SDS messages yet',sds_refresh:'Refresh',
    rf_freq:'Center freq',rf_rate:'Sample rate',rf_rms:'RMS',rf_peak:'Peak',rf_age:'Snapshot',
    rf_waiting:'waiting…',rf_live:'live',rf_stale:'stale',
    rf_visualizers:'Visualizers',rf_spectrum:'TX DSP Spectrum (pre-PA)',rf_constellation:'TX DSP Constellation',
    rf_hint_spectrum:'live · 512-bin FFT',rf_hint_constellation:'π/4-DQPSK',
    rf_waterfall:'TX Spectrum Waterfall',rf_hint_waterfall:'rolling · viridis',
    rf_quality:'Signal Quality',rf_hint_quality:'measured pre-PA · derived from same DSP snapshot',
    rf_evm:'EVM',rf_papr:'PAPR',rf_carrier:'Carrier leak',rf_obw:'Occupied BW (99%)',
    rf_dc:'DC offset (I/Q)',rf_iqa:'IQ amplitude imbalance',rf_iqp:'IQ phase imbalance',
    rf_hw_health:'Hardware Health',rf_hint_health:'polled every 5s',
    rf_temp:'SDR Temperature',rf_tx_gain:'TX Gain Stages (actual)',rf_rx_gain:'RX Gain Stages (actual)',
    rf_temp_cold:'cold',rf_temp_nominal:'nominal',rf_temp_warm:'warm',rf_temp_hot:'hot',rf_temp_na:'no sensor',
    rf_no_gains:'unavailable',rf_just_now:'just now',

    asterisk_title:'Asterisk SIP',ast_configured:'Configured',ast_register:'REGISTER',ast_sip_listen:'SIP listen',
    ast_remote:'Remote Asterisk',ast_rtp:'RTP ports',ast_codec:'Codec',ast_last_rx:'Last RX',
    ast_last_tx:'Last TX',ast_last_error:'Last error',
    dapnet_title:'DAPNET',dapnet_log:'DAPNET Log',dapnet_routing:'Routing',dapnet_send:'Send DAPNET Message',dapnet_saved:'✓ Saved',
    terminals:'Radios',registered:'registered',
    active_calls:'Active Calls',circuits:'circuits in use',
    registered_terminals:'Registered Radios',
    bts_details:'TETRA BTS Details',bts_tx:'TX Freq',bts_rx:'RX Freq',bts_shift:'Duplex Shift',bts_rate:'Sample Rate',
    dual_carrier:'Dual Carrier',dc_on_sub:'On · secondary carrier #{c}',dc_off_sub:'Off · single carrier',
    dc_enter_carrier:'Secondary carrier number (e.g. main carrier ±1):',dc_bad_carrier:'Please enter a valid carrier number.',
    dc_confirm_on:'Enable Dual Carrier? This RESTARTS the base station and briefly drops all active calls.',
    dc_confirm_off:'Disable Dual Carrier? This RESTARTS the base station and briefly drops all active calls.',
    dc_applying:'Applying…',dc_restarting:'Restarting to apply… reconnecting shortly.',dc_failed:'Could not change Dual Carrier',
    bts_la:'Location Area',bts_cc:'Colour Code',bts_carrier:'Main Carrier',bts_band:'Band',
    bts_access:'Registration Access',bts_wl_entries:'whitelisted ISSI',bts_wl_open:'Open — all ISSI may register',
    readability:'Readability',size_small:'Small',size_small_d:'Compact · normal contrast',size_medium:'Medium',size_medium_d:'Default · comfortable',size_high:'High',size_high_d:'Larger · stronger contrast',size_ultra:'Ultra',size_ultra_d:'Largest · maximum contrast',sdr:'SDR',power:'Power',
    no_terminals:'No radios registered',no_calls:'No active calls',
    live_log:'Live Log',autoscroll:'Auto-scroll',filter_all:'All',
    clear:'Clear',export:'Export',restart:'Restart',shutdown:'Shutdown',save:'Save',
    cfg_sec_configuration:'Configuration',cfg_sec_access:'Access Control',cfg_sec_wx:'WX / METAR',whitelist_title:'ISSI Whitelist',whitelist_add:'Add ISSI',whitelist_empty:'List empty — open network (any radio may register).',
    whitelist_help:'When the list is empty, any radio may register (open network). When non-empty, only the listed ISSIs are accepted; all others are rejected. Changes apply instantly and persist across restarts.',
    whitelist_enforced:'ENFORCED',whitelist_open:'OPEN',whitelist_invalid:'Enter a valid ISSI (1–16777215).',
    wx_title:'WX / METAR Service',wx_help:'Built-in weather service. Radios send an SDS like "METAR LROP" to the service ISSI to get a decoded report. Optionally auto-send a fixed station\'s METAR to an ISSI or talkgroup at a set interval. Data from aviationweather.gov.',
    wx_enabled:'Enable on-demand METAR responder',wx_service_issi:'Service ISSI',wx_periodic_enabled:'Enable periodic auto-broadcast',
    wx_periodic_icao:'Station ICAO',wx_periodic_dest:'Destination',wx_periodic_isgroup:'Destination is group',wx_periodic_isgroup_hint:'(GSSI instead of individual ISSI)',
    wx_periodic_interval:'Interval (seconds)',wx_interval_hint:'Minimum 300 s (5 min) to avoid hammering the weather API.',wx_periodic_incomplete:'Set both station ICAO and destination for periodic mode.',
    sds_title:'⬡ Send SDS Message',sds_dest:'Destination ISSI',
    sds_callout_enable:'TPG2200 Call-Out / Send alarm',
    sds_callout_source:'Source ISSI',
    sds_callout_incident:'Incident number',
    sds_callout_text:'Alarm text',
    sds_callout_raw:'Raw Hex Payload optional',
    sds_callout_help:'Incidents 1-15 use the confirmed byte formula (N << 4) | 0x01: 1=11, 2=21, 3=31, 4=41. Incidents 16-256 use the extended one-byte selector. Raw Hex overrides automatic payload generation.',
    live_sds_desc:'Broadcast a text message to all radios on the cell, repeating at the Home Mode Display interval. Repeats until deleted or the repeat count is reached.',
    live_sds_text:'Message text (max 251 chars)',live_sds_repeat:'Repeat (0=∞)',live_sds_send:'Broadcast',
    live_sds_clear_all:'Clear All',live_sds_empty:'No active broadcasts.',
    live_sds_sent:'sent',live_sds_times:'×',live_sds_forever:'∞',live_sds_delete:'✕',
    fallback_title:'⚠ FALLBACK CONFIG ACTIVE — Primary config failed to load',
    sds_msg_label:'Message',cancel:'Cancel',send:'Send',
    th_issi:'ISSI',th_issi_cs:'ISSI / Callsign',th_groups:'Groups',th_ee:'Energy Economy',th_signal:'Signal',
    tg_selected:'Selected talkgroup (last keyed up)',
    tg_affiliated_short:'affiliated',tg_affiliated_hint:'Other talkgroups this radio is affiliated to (kept attached on the BS even when scan is off on the device)',
    th_status:'Status',th_last_seen:'Last seen',th_actions:'Actions',
    th_id:'ID',th_type:'Type',th_caller:'Caller',
    th_dest:'Destination',th_speaker:'Speaker',th_duration:'Duration',
    th_time:'Time',th_activity:'Activity',
    last_heard_title:'Last Heard',no_activity:'No activity yet',
    act_call_group:'Group Call',act_call_individual:'P2P Call',act_sds:'SDS',
    online_badge:'ONLINE',kick:'Kick',sds:'SDS',
    call_group:'GROUP',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',call_emergency:'EMERGENCY',
    emg_banner_title:'EMERGENCY ACTIVE',integrations:'Integrations',integ_enabled:'Enabled',integ_disabled:'Disabled',integ_error:'Error',system_sec:'System',emg_chip:'EMERGENCY',bs_label:'BS',emg_clear:'Clear',confirm_clear_emergency:'Clear emergency for ISSI {issi}?',
    confirm_kick:'Kick ISSI {issi}?\nTerminal will be deregistered and forced to re-attach.',
    dgna:'DGNA',dgna_title:'Dynamic group assignment',dgna_modal_title:'⬡ Dynamic Group Assignment',dgna_issi:'Terminal ISSI',dgna_current:'Current groups',dgna_gssi:'Group (GSSI)',dgna_assign:'Assign',dgna_deassign:'Deassign',
    confirm_restart:'Restart FlowStation?\nAll active calls will be dropped.',
    confirm_shutdown:'Shutdown FlowStation?\nThe service will stop and must be restarted manually.',
    confirm_logout:'Log out?',
    saved:'✓ Saved — restart to apply.',save_fail:'✗ Save failed',conn_error:'Connection error.',
    update:'Update',update_available:'Update available',update_title:'OTA Update — github.com/razvanzeces/flowstation',
    update_confirm:'Pull latest from main and rebuild?\nThe service will restart automatically.',
    update_running:'Updating… do not close this window.',
    update_done_ok:'✓ Update complete. Restarting…',
    update_done_err:'✗ Update failed. See log below.',
    update_close:'Close',
    system:'System',sys_info:'System Info',sys_hostname:'Hostname',sys_uptime:'Uptime',
    sys_version:'FS Version',sys_os:'OS',sys_config:'Active Config',
    sys_cpu:'CPU',sys_cpu_load:'CPU Load',sys_ram:'RAM',sys_temp:'CPU Temp',
    wifi:'WiFi',wifi_status:'Current connection',wifi_saved:'Saved networks',wifi_visible:'Available networks',wifi_loading:'Loading…',wifi_scanning:'Scanning…',wifi_no_device:'No WiFi device detected on this host.',wifi_radio_disabled:'WiFi radio is disabled.',wifi_not_connected:'Not connected to any network.',wifi_no_saved:'No saved networks.',wifi_no_networks:'No networks in range.',wifi_ssid:'Network',wifi_signal:'Signal',wifi_ip:'IP address',wifi_actions:'Actions',wifi_disconnect:'Disconnect',wifi_connect:'Connect',wifi_connect_to:'Connect to',wifi_connecting:'Connecting…',wifi_connected:'CONNECTED',wifi_connected_ok:'Connected.',wifi_saved_tag:'SAVED',wifi_open:'OPEN',wifi_forget:'Forget',wifi_confirm_forget:'Forget network',wifi_password:'Password',wifi_hidden:'Hidden network (SSID not broadcast)',wifi_add_hidden:'Hidden network',wifi_scan:'Scan',wifi_refresh:'Refresh',wifi_radio_off:'Disable WiFi',wifi_radio_on:'Enable WiFi',wifi_warn_lose_access:'If connected to the dashboard via WiFi, changing networks may temporarily disconnect you. Make sure you have a backup access path (Ethernet or known good network).',wifi_err_no_ssid:'SSID required',cancel:'Cancel',sys_sensors:'Host Hardware Sensors',sys_sensors_empty:'No sensors detected on this host.',sys_rf:'RF Hardware (SoapySDR)',sys_autorefresh:'Auto-refresh 5s',
    profile_edit_title:'Edit Config Profile',profile_edit_btn:'Edit',
    profile_edit_save_ok:'✓ Saved',profile_edit_save_fail:'✗ Save failed',
    sys_os:'OS',sys_version:'FS Version',sys_config:'Active Config',
    sys_profiles:'Config Profiles',sys_activate:'Activate & Restart',
    sys_active_badge:'ACTIVE',sys_no_profiles:'No .toml profiles found in config directory.',
    sys_activate_confirm:'Switch to profile "{name}" and restart?\nCurrent config will be backed up.',
    sys_title:'System',sys_sec_status:'Status',sys_sec_host:'Host',sys_sec_radio:'Radio Hardware',sys_sec_sensors:'Sensors',sys_sec_profiles:'Profiles',sys_sec_sds:'SDS Broadcast',sys_refresh:'Refresh',sys_probe:'Probe',sys_temp_hot:'HOT',sys_temp_warm:'Warm',sys_temp_ok:'OK',
    sys_bts:'BTS Connection',
    telegram:'Telegram',tg_title:'Telegram Alerts',
    tg_help:'Get instant Telegram messages when something happens on the station — a radio attaches or drops, the backhaul goes up or down, a position beacon arrives, or the stack logs a warning/error.',
    tg_enabled:'Enable Telegram alerts',
    tg_test:'Send test',tg_testing:'Sending test…',tg_test_ok:'✓ Test sent to {n} chat(s)',
    tg_howto_title:'Setup — 4 steps',
    tg_step1:'In Telegram, open @BotFather, send /newbot and follow the prompts. Copy the bot token it gives you.',
    tg_step2:'Paste the token below and click Verify — you should see your bot\'s @username.',
    tg_step3:'Open a chat with your new bot (or add it to a group) and send it any message, e.g. /start.',
    tg_step4:'Click "Detect Chat ID", add your chat to the recipients, then Save. Use "Send test" to confirm.',
    tg_bot_title:'Bot token',
    tg_bot_help:'The token from @BotFather looks like 123456789:AAExampleTokenString. It is stored masked and never shown in full again.',
    tg_verify:'Verify',tg_verifying:'Verifying…',
    tg_recipients_title:'Recipients (Chat IDs)',
    tg_recipients_help:'Every alert is sent to each recipient. A positive ID is a private chat; a negative ID is a group or channel.',
    tg_detect:'Detect Chat ID',tg_detecting:'Reading recent messages…',
    tg_detect_none:'No recent messages found. Send your bot a message first, then try again.',
    tg_detect_found:'Chats that messaged your bot — click Add:',
    tg_add:'Add',tg_no_recipients:'No recipients yet.',tg_invalid_chat:'Enter a valid Chat ID.',
    tg_categories_title:'Alert categories',
    tg_cat_connect:'Radio connected',tg_cat_disconnect:'Radio disconnected',
    tg_cat_t351:'Radio dropped (no T351 response)',tg_cat_lip:'LIP/APRS position beacon',
    tg_cat_backhaul:'Brew backhaul up/down',tg_cat_logs:'Critical log (warnings/errors)',
  },
  ro:{
    bts_ip:'IP BTS',offline:'DECONECTAT',online:'CONECTAT',
    brew_online:'ONLINE',brew_offline:'OFFLINE',
    stations:'Radiouri',calls:'Apeluri',lastheard:'Ultima Activitate',log:'Log',rf:'RF',health:'Health',echolink:'EchoLink',echolink_title:'EchoLink',config:'Config',
    sdslog:'Jurnal SDS',th_dir:'Dir',th_from:'De la',th_to:'Către',th_message:'Mesaj',no_sds:'Niciun mesaj SDS încă',sds_refresh:'Reîmprospătează',
    rf_freq:'Frecvență centru',rf_rate:'Rată eșantion',rf_rms:'RMS',rf_peak:'Vârf',rf_age:'Captură',
    rf_waiting:'în așteptare…',rf_live:'live',rf_stale:'expirat',
    rf_visualizers:'Vizualizatoare',rf_spectrum:'Spectru TX DSP (pre-PA)',rf_constellation:'Constelație TX DSP',
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
    bts_details:'Detalii BTS TETRA',bts_tx:'Frecvență TX',bts_rx:'Frecvență RX',bts_shift:'Decalaj Duplex',bts_rate:'Rată Eșantionare',
    dual_carrier:'Dual Carrier',dc_on_sub:'Pornit · carrier secundar #{c}',dc_off_sub:'Oprit · un singur carrier',
    dc_enter_carrier:'Numărul carrier-ului secundar (ex. carrier principal ±1):',dc_bad_carrier:'Introdu un număr de carrier valid.',
    dc_confirm_on:'Pornești Dual Carrier? Asta REPORNEȘTE stația de bază și pică toate apelurile active câteva secunde.',
    dc_confirm_off:'Oprești Dual Carrier? Asta REPORNEȘTE stația de bază și pică toate apelurile active câteva secunde.',
    dc_applying:'Se aplică…',dc_restarting:'Repornește pentru aplicare… reconectare în scurt timp.',dc_failed:'Nu am putut schimba Dual Carrier',
    bts_la:'Zonă (LA)',bts_cc:'Cod Culoare',bts_carrier:'Purtătoare Princ.',bts_band:'Bandă',
    bts_access:'Acces Înregistrare',bts_wl_entries:'ISSI permise',bts_wl_open:'Deschis — orice ISSI se poate înregistra',
    readability:'Lizibilitate',size_small:'Mic',size_small_d:'Compact · contrast normal',size_medium:'Mediu',size_medium_d:'Implicit · confortabil',size_high:'Mare',size_high_d:'Mai mare · contrast sporit',size_ultra:'Ultra',size_ultra_d:'Cel mai mare · contrast maxim',sdr:'SDR',power:'Consum',
    no_terminals:'Niciun radio înregistrat',no_calls:'Niciun apel activ',
    live_log:'Log Live',autoscroll:'Auto-scroll',filter_all:'Toate',
    clear:'Șterge',export:'Export',restart:'Repornire',shutdown:'Oprire',save:'Salvează',
    cfg_sec_configuration:'Configurație',cfg_sec_access:'Control acces',cfg_sec_wx:'WX / METAR',whitelist_title:'Listă albă ISSI',whitelist_add:'Adaugă ISSI',whitelist_empty:'Listă goală — rețea deschisă (orice radio se poate înregistra).',
    whitelist_help:'Când lista e goală, orice radio se poate înregistra (rețea deschisă). Când are intrări, doar ISSI-urile listate sunt acceptate; restul sunt respinse. Modificările se aplică instant și persistă după repornire.',
    whitelist_enforced:'ACTIVĂ',whitelist_open:'DESCHISĂ',whitelist_invalid:'Introdu un ISSI valid (1–16777215).',
    wx_title:'Serviciu WX / METAR',wx_help:'Serviciu meteo integrat. Radiourile trimit un SDS de forma "METAR LROP" către ISSI-ul serviciului și primesc raportul decodat. Opțional, trimite automat METAR-ul unei stații fixe către un ISSI sau grup la interval. Date de la aviationweather.gov.',
    wx_enabled:'Activează răspunsul METAR la cerere',wx_service_issi:'ISSI serviciu',wx_periodic_enabled:'Activează trimiterea periodică',
    wx_periodic_icao:'Cod ICAO stație',wx_periodic_dest:'Destinație',wx_periodic_isgroup:'Destinația e grup',wx_periodic_isgroup_hint:'(GSSI în loc de ISSI individual)',
    wx_periodic_interval:'Interval (secunde)',wx_interval_hint:'Minim 300 s (5 min) ca să nu suprasolicităm API-ul meteo.',wx_periodic_incomplete:'Setează și ICAO stație și destinație pentru modul periodic.',
    live_sds_desc:'Transmite un mesaj text către toate radiourile din celulă, repetând la intervalul Home Mode Display.',
    live_sds_text:'Text mesaj (max 251 caractere)',live_sds_repeat:'Repetări (0=∞)',live_sds_send:'Broadcast',
    live_sds_clear_all:'Șterge Tot',live_sds_empty:'Niciun broadcast activ.',
    live_sds_sent:'trimis',live_sds_times:'×',live_sds_forever:'∞',live_sds_delete:'✕',
    fallback_title:'⚠ CONFIG DE REZERVĂ ACTIV — Config principal nu a putut fi încărcat',
    sds_title:'⬡ Trimite Mesaj SDS',sds_dest:'ISSI Destinatar',
    sds_msg_label:'Mesaj',cancel:'Anulează',send:'Trimite',
    th_issi:'ISSI',th_issi_cs:'ISSI / Indicativ',th_groups:'Grupuri',th_ee:'Economie Energie',th_signal:'Semnal',
    tg_selected:'Grup selectat (ultima transmisie)',
    tg_affiliated_short:'afiliate',tg_affiliated_hint:'Alte grupuri la care radio-ul este afiliat (rămân atașate la BS chiar și când scan e oprit din statie)',
    th_status:'Status',th_last_seen:'Văzut',th_actions:'Acțiuni',
    th_id:'ID',th_type:'Tip',th_caller:'Apelant',
    th_dest:'Destinatar',th_speaker:'Vorbitor',th_duration:'Durată',
    th_time:'Oră',th_activity:'Activitate',
    last_heard_title:'Ultima Activitate',no_activity:'Nicio activitate încă',
    act_call_group:'Apel Grup',act_call_individual:'Apel P2P',act_sds:'SDS',
    online_badge:'ONLINE',kick:'Kick',sds:'SDS',
    call_group:'GRUP',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',call_emergency:'URGENȚĂ',
    emg_banner_title:'URGENȚĂ ACTIVĂ',integrations:'Integrări',integ_enabled:'Activat',integ_disabled:'Dezactivat',integ_error:'Eroare',system_sec:'Sistem',emg_chip:'URGENȚĂ',bs_label:'BS',emg_clear:'Anulează',confirm_clear_emergency:'Anulezi urgența pentru ISSI {issi}?',
    confirm_kick:'Kick ISSI {issi}?\nTerminalul va fi deînregistrat și forțat să se reconecteze.',
    dgna:'DGNA',dgna_title:'Atribuire dinamică de grup',dgna_modal_title:'⬡ Atribuire dinamică de grup',dgna_issi:'ISSI terminal',dgna_current:'Grupuri curente',dgna_gssi:'Grup (GSSI)',dgna_assign:'Atribuie',dgna_deassign:'Retrage',
    confirm_restart:'Repornire FlowStation?\nToate apelurile active vor fi întrerupte.',
    confirm_shutdown:'Oprire FlowStation?\nServiciul se va opri și trebuie repornit manual.',
    confirm_logout:'Deconectare?',
    saved:'✓ Salvat — repornire pentru aplicare.',save_fail:'✗ Salvare eșuată',conn_error:'Eroare de conexiune.',
    update:'Update',update_available:'Actualizare disponibilă',update_title:'Update OTA — github.com/razvanzeces/flowstation',
    update_confirm:'Descarcă ultima versiune din main și recompilează?\nServiciul va reporni automat.',
    update_running:'Se actualizează… nu închide fereastra.',
    update_done_ok:'✓ Update finalizat. Se repornește…',
    update_done_err:'✗ Update eșuat. Vezi logul de mai jos.',
    update_close:'Închide',
    system:'Sistem',sys_info:'Info Sistem',sys_hostname:'Hostname',sys_uptime:'Uptime',
    sys_os:'OS',sys_version:'Versiune FS',sys_config:'Config Activ',
    sys_cpu:'CPU',sys_cpu_load:'Încărcare CPU',sys_ram:'RAM',sys_temp:'Temp CPU',
    wifi:'WiFi',wifi_status:'Conexiunea curentă',wifi_saved:'Rețele salvate',wifi_visible:'Rețele disponibile',wifi_loading:'Se încarcă…',wifi_scanning:'Se scanează…',wifi_no_device:'Niciun dispozitiv WiFi detectat.',wifi_radio_disabled:'Radioul WiFi este dezactivat.',wifi_not_connected:'Neconectat la nicio rețea.',wifi_no_saved:'Nicio rețea salvată.',wifi_no_networks:'Nicio rețea în rază.',wifi_ssid:'Rețea',wifi_signal:'Semnal',wifi_ip:'Adresă IP',wifi_actions:'Acțiuni',wifi_disconnect:'Deconectează',wifi_connect:'Conectează',wifi_connect_to:'Conectează la',wifi_connecting:'Se conectează…',wifi_connected:'CONECTAT',wifi_connected_ok:'Conectat.',wifi_saved_tag:'SALVAT',wifi_open:'DESCHIS',wifi_forget:'Uită',wifi_confirm_forget:'Uită rețeaua',wifi_password:'Parolă',wifi_hidden:'Rețea ascunsă (SSID nedifuzat)',wifi_add_hidden:'Rețea ascunsă',wifi_scan:'Scanează',wifi_refresh:'Reîncarcă',wifi_radio_off:'Dezactivează WiFi',wifi_radio_on:'Activează WiFi',wifi_warn_lose_access:'Dacă ești conectat la dashboard prin WiFi, schimbarea rețelei te poate deconecta temporar. Asigură-te că ai o cale alternativă (Ethernet sau rețea de încredere).',wifi_err_no_ssid:'SSID necesar',cancel:'Anulează',sys_sensors:'Senzori Hardware Gazdă',sys_sensors_empty:'Niciun senzor detectat.',sys_rf:'Hardware RF (SoapySDR)',sys_autorefresh:'Auto-refresh 5s',
    profile_edit_title:'Editare Profil Config',profile_edit_btn:'Editează',
    profile_edit_save_ok:'✓ Salvat',profile_edit_save_fail:'✗ Salvare eșuată',
    sys_profiles:'Profile Config',sys_activate:'Activează & Repornire',
    sys_active_badge:'ACTIV',sys_no_profiles:'Niciun profil .toml găsit în directorul config.',
    sys_activate_confirm:'Comutare la profilul "{name}" și repornire?\nConfig-ul curent va fi salvat.',
    sys_title:'Sistem',sys_sec_status:'Stare',sys_sec_host:'Gazdă',sys_sec_radio:'Hardware radio',sys_sec_sensors:'Senzori',sys_sec_profiles:'Profiluri',sys_sec_sds:'Difuzare SDS',sys_refresh:'Reîncarcă',sys_probe:'Sondează',sys_temp_hot:'FIERBINTE',sys_temp_warm:'Cald',sys_temp_ok:'OK',
    sys_bts:'Conexiune BTS',
    telegram:'Telegram',tg_title:'Alerte Telegram',
    tg_help:'Primește mesaje Telegram instant când se întâmplă ceva pe stație — un radio se conectează sau cade, backhaul-ul urcă/coboară, sosește o baliză de poziție, sau stack-ul logează un avertisment/eroare.',
    tg_enabled:'Activează alertele Telegram',
    tg_test:'Trimite test',tg_testing:'Se trimite testul…',tg_test_ok:'✓ Test trimis către {n} conversație(i)',
    tg_howto_title:'Configurare — 4 pași',
    tg_step1:'În Telegram, deschide @BotFather, trimite /newbot și urmează pașii. Copiază token-ul botului.',
    tg_step2:'Lipește token-ul mai jos și apasă Verifică — ar trebui să vezi @username-ul botului tău.',
    tg_step3:'Deschide o conversație cu botul (sau adaugă-l într-un grup) și trimite-i orice mesaj, ex. /start.',
    tg_step4:'Apasă „Detectează Chat ID", adaugă conversația la destinatari, apoi Salvează. Folosește „Trimite test" pentru confirmare.',
    tg_bot_title:'Token bot',
    tg_bot_help:'Token-ul de la @BotFather arată ca 123456789:AAExempluToken. Este stocat mascat și nu mai e afișat integral.',
    tg_verify:'Verifică',tg_verifying:'Se verifică…',
    tg_recipients_title:'Destinatari (Chat ID-uri)',
    tg_recipients_help:'Fiecare alertă e trimisă către toți destinatarii. Un ID pozitiv e o conversație privată; unul negativ e un grup sau canal.',
    tg_detect:'Detectează Chat ID',tg_detecting:'Se citesc mesajele recente…',
    tg_detect_none:'Niciun mesaj recent. Trimite întâi un mesaj botului, apoi încearcă din nou.',
    tg_detect_found:'Conversații care au scris botului — apasă Adaugă:',
    tg_add:'Adaugă',tg_no_recipients:'Niciun destinatar încă.',tg_invalid_chat:'Introdu un Chat ID valid.',
    tg_categories_title:'Categorii de alerte',
    tg_cat_connect:'Radio conectat',tg_cat_disconnect:'Radio deconectat',
    tg_cat_t351:'Radio căzut (fără răspuns T351)',tg_cat_lip:'Baliză poziție LIP/APRS',
    tg_cat_backhaul:'Backhaul Brew up/down',tg_cat_logs:'Log critic (avertismente/erori)',
  },
  de:{
    bts_ip:'BTS-IP',offline:'OFFLINE',online:'ONLINE',
    brew_online:'ONLINE',brew_offline:'OFFLINE',
    stations:'Radios',calls:'Anrufe',lastheard:'Zuletzt Gehört',log:'Log',rf:'RF',health:'Health',asterisk:'Asterisk SIP',dapnet:'DAPNET',echolink:'EchoLink',echolink_title:'EchoLink',meshcom:'MeshCom',meshcom_title:'MeshCom',geoalarm:'GeoAlarm',geoalarm_title:'GeoAlarm',config:'Config',
    sdslog:'SDS-Log',th_dir:'Ri.',th_from:'Von',th_to:'An',th_message:'Nachricht',no_sds:'Noch keine SDS-Nachrichten',sds_refresh:'Aktualisieren',
    rf_freq:'Mittenfrequenz',rf_rate:'Abtastrate',rf_rms:'RMS',rf_peak:'Spitze',rf_age:'Aufnahme',
    rf_waiting:'wartet…',rf_live:'live',rf_stale:'veraltet',
    rf_visualizers:'Visualisierungen',rf_spectrum:'TX-DSP-Spektrum (vor PA)',rf_constellation:'TX-DSP-Konstellation',
    rf_hint_spectrum:'live · 512-bin FFT',rf_hint_constellation:'π/4-DQPSK',
    rf_waterfall:'TX-Spektrum-Wasserfall',rf_hint_waterfall:'rollend · viridis',
    rf_quality:'Signalqualität',rf_hint_quality:'gemessen vor PA · aus selbem DSP-Snapshot',
    rf_evm:'EVM',rf_papr:'PAPR',rf_carrier:'Trägerleckage',rf_obw:'Belegte BW (99%)',
    rf_dc:'DC-Offset (I/Q)',rf_iqa:'IQ-Amplitudenungleichgewicht',rf_iqp:'IQ-Phasenungleichgewicht',
    rf_hw_health:'Hardware-Zustand',rf_hint_health:'alle 5s abgefragt',
    rf_temp:'SDR-Temperatur',rf_tx_gain:'TX-Verstärkung (aktuell)',rf_rx_gain:'RX-Verstärkung (aktuell)',
    rf_temp_cold:'kalt',rf_temp_nominal:'nominal',rf_temp_warm:'warm',rf_temp_hot:'heiß',rf_temp_na:'kein Sensor',
    rf_no_gains:'nicht verfügbar',rf_just_now:'gerade eben',

    asterisk_title:'Asterisk SIP',ast_configured:'Konfiguriert',ast_register:'REGISTER',ast_sip_listen:'SIP hört auf',
    ast_remote:'Remote Asterisk',ast_rtp:'RTP-Ports',ast_codec:'Codec',ast_last_rx:'Letztes RX',
    ast_last_tx:'Letztes TX',ast_last_error:'Letzter Fehler',
    dapnet_title:'DAPNET',dapnet_log:'DAPNET-Log',dapnet_routing:'Routing',dapnet_send:'DAPNET-Nachricht senden',dapnet_saved:'✓ Gespeichert',
    terminals:'Radios',registered:'registriert',
    active_calls:'Aktive Anrufe',circuits:'Schaltkreise aktiv',
    registered_terminals:'Registrierte Radios',
    no_terminals:'Keine Radios registriert',no_calls:'Keine aktiven Anrufe',
    live_log:'Live-Log',autoscroll:'Auto-Scroll',filter_all:'Alle',
    clear:'Löschen',export:'Exportieren',restart:'Neustart',shutdown:'Herunterfahren',save:'Speichern',
    cfg_sec_configuration:'Konfiguration',cfg_sec_access:'Zugriffskontrolle',cfg_sec_wx:'WX / METAR',whitelist_title:'ISSI-Whitelist',whitelist_add:'ISSI hinzufügen',whitelist_empty:'Liste leer — offenes Netz (jedes Funkgerät darf sich anmelden).',
    whitelist_help:'Ist die Liste leer, darf sich jedes Funkgerät anmelden (offenes Netz). Bei Einträgen werden nur die gelisteten ISSIs akzeptiert; alle anderen werden abgewiesen. Änderungen wirken sofort und bleiben nach Neustart erhalten.',
    whitelist_enforced:'AKTIV',whitelist_open:'OFFEN',whitelist_invalid:'Gültige ISSI eingeben (1–16777215).',
    wx_title:'WX / METAR-Dienst',wx_help:'Integrierter Wetterdienst. Funkgeräte senden eine SDS wie "METAR LROP" an die Dienst-ISSI und erhalten einen dekodierten Bericht. Optional automatisches Senden des METAR einer festen Station an eine ISSI oder Gruppe in Intervallen. Daten von aviationweather.gov.',
    wx_enabled:'METAR-Antwort auf Anfrage aktivieren',wx_service_issi:'Dienst-ISSI',wx_periodic_enabled:'Periodisches Senden aktivieren',
    wx_periodic_icao:'Stations-ICAO',wx_periodic_dest:'Ziel',wx_periodic_isgroup:'Ziel ist Gruppe',wx_periodic_isgroup_hint:'(GSSI statt einzelner ISSI)',
    wx_periodic_interval:'Intervall (Sekunden)',wx_interval_hint:'Mindestens 300 s (5 Min), um die Wetter-API nicht zu überlasten.',wx_periodic_incomplete:'Stations-ICAO und Ziel für den periodischen Modus setzen.',
    live_sds_desc:'Sendet eine Textnachricht an alle Funkgeräte der Zelle, wiederholt im Home-Mode-Display-Intervall.',
    live_sds_text:'Nachrichtentext (max. 251 Zeichen)',live_sds_repeat:'Wiederh. (0=∞)',live_sds_send:'Senden',
    live_sds_clear_all:'Alle löschen',live_sds_empty:'Keine aktiven Broadcasts.',
    live_sds_sent:'gesendet',live_sds_times:'×',live_sds_forever:'∞',live_sds_delete:'✕',
    fallback_title:'⚠ FALLBACK-KONFIGURATION AKTIV — Primäre Konfiguration konnte nicht geladen werden',
    sds_title:'⬡ SDS-Nachricht senden',sds_dest:'Ziel-ISSI',
    sds_callout_enable:'TPG2200 Call-Out / Alarm senden',
    sds_callout_source:'Source ISSI',
    sds_callout_incident:'Vorfallnummer',
    sds_callout_text:'Alarmtext',
    sds_callout_raw:'Raw Hex Payload optional',
    sds_callout_help:'Vorfall 1-15 nutzen die bestätigte Byte-Formel (N << 4) | 0x01: 1=11, 2=21, 3=31, 4=41. Vorfall 16-256 nutzen den erweiterten Ein-Byte-Selector. Raw Hex überschreibt die automatische Payload.',
    sds_msg_label:'Nachricht',cancel:'Abbrechen',send:'Senden',
    th_issi:'ISSI',th_groups:'Gruppen',th_ee:'Energiesparen',th_signal:'Signal',
    th_status:'Status',th_last_seen:'Zuletzt',th_actions:'Aktionen',
    th_id:'ID',th_type:'Typ',th_caller:'Anrufer',
    th_dest:'Ziel',th_speaker:'Sprecher',th_duration:'Dauer',
    th_time:'Zeit',th_activity:'Aktivität',
    last_heard_title:'Zuletzt Gehört',no_activity:'Noch keine Aktivität',
    act_call_group:'Gruppenruf',act_call_individual:'P2P-Ruf',act_sds:'SDS',
    online_badge:'ONLINE',kick:'Entfernen',sds:'SDS',
    call_group:'GRUPPE',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',call_emergency:'NOTRUF',
    emg_banner_title:'NOTFALL AKTIV',integrations:'Integrationen',integ_enabled:'Aktiviert',integ_disabled:'Deaktiviert',integ_error:'Fehler',system_sec:'System',emg_chip:'NOTFALL',bs_label:'BS',emg_clear:'Löschen',confirm_clear_emergency:'Notfall für ISSI {issi} löschen?',
    confirm_kick:'ISSI {issi} entfernen?\nDas Terminal wird abgemeldet und zur Neuanmeldung gezwungen.',
    dgna:'DGNA',dgna_title:'Dynamische Gruppenzuweisung',dgna_modal_title:'⬡ Dynamische Gruppenzuweisung',dgna_issi:'Terminal-ISSI',dgna_current:'Aktuelle Gruppen',dgna_gssi:'Gruppe (GSSI)',dgna_assign:'Zuweisen',dgna_deassign:'Entfernen',
    confirm_restart:'FlowStation neu starten?\nAlle aktiven Anrufe werden beendet.',
    confirm_shutdown:'FlowStation herunterfahren?\nDer Dienst wird gestoppt und muss manuell neu gestartet werden.',
    confirm_logout:'Abmelden?',
    saved:'✓ Gespeichert — Neustart zum Anwenden.',save_fail:'✗ Fehler beim Speichern',conn_error:'Verbindungsfehler.',
    update:'Update',update_available:'Update verfügbar',update_title:'OTA-Update — github.com/razvanzeces/flowstation',
    update_confirm:'Neueste Version von main holen und neu bauen?\nDer Dienst startet automatisch neu.',
    update_running:'Aktualisierung läuft… Fenster nicht schließen.',
    update_done_ok:'✓ Update abgeschlossen. Neustart…',
    update_done_err:'✗ Update fehlgeschlagen. Siehe Log unten.',
    update_close:'Schließen',
    system:'System',sys_info:'Systeminfo',sys_hostname:'Hostname',sys_uptime:'Laufzeit',
    sys_os:'OS',sys_version:'FS-Version',sys_config:'Aktive Konfig',
    sys_cpu:'CPU',sys_cpu_load:'CPU-Auslastung',sys_ram:'RAM',sys_temp:'CPU-Temp',
    wifi:'WLAN',wifi_status:'Aktuelle Verbindung',wifi_saved:'Gespeicherte Netzwerke',wifi_visible:'Verfügbare Netzwerke',wifi_loading:'Wird geladen…',wifi_scanning:'Suche läuft…',wifi_no_device:'Kein WLAN-Gerät erkannt.',wifi_radio_disabled:'WLAN-Funk ist deaktiviert.',wifi_not_connected:'Mit keinem Netzwerk verbunden.',wifi_no_saved:'Keine gespeicherten Netzwerke.',wifi_no_networks:'Keine Netzwerke in Reichweite.',wifi_ssid:'Netzwerk',wifi_signal:'Signal',wifi_ip:'IP-Adresse',wifi_actions:'Aktionen',wifi_disconnect:'Trennen',wifi_connect:'Verbinden',wifi_connect_to:'Verbinden mit',wifi_connecting:'Verbinde…',wifi_connected:'VERBUNDEN',wifi_connected_ok:'Verbunden.',wifi_saved_tag:'GESPEICHERT',wifi_open:'OFFEN',wifi_forget:'Vergessen',wifi_confirm_forget:'Netzwerk vergessen',wifi_password:'Passwort',wifi_hidden:'Verstecktes Netzwerk (SSID nicht gesendet)',wifi_add_hidden:'Verstecktes Netzwerk',wifi_scan:'Suchen',wifi_refresh:'Aktualisieren',wifi_radio_off:'WLAN deaktivieren',wifi_radio_on:'WLAN aktivieren',wifi_warn_lose_access:'Wenn Sie über WLAN mit dem Dashboard verbunden sind, kann ein Netzwerkwechsel die Verbindung trennen. Stellen Sie sicher, dass Sie einen alternativen Zugang haben.',wifi_err_no_ssid:'SSID erforderlich',cancel:'Abbrechen',sys_sensors:'Host-Hardware-Sensoren',sys_sensors_empty:'Keine Sensoren erkannt.',sys_rf:'RF-Hardware (SoapySDR)',sys_autorefresh:'Auto-Aktualisierung 5s',
    profile_edit_title:'Konfigprofil bearbeiten',profile_edit_btn:'Bearbeiten',
    profile_edit_save_ok:'✓ Gespeichert',profile_edit_save_fail:'✗ Speichern fehlgeschlagen',
    sys_profiles:'Konfigprofile',sys_activate:'Aktivieren & Neustart',
    sys_active_badge:'AKTIV',sys_no_profiles:'Keine .toml-Profile im Konfigverzeichnis gefunden.',
    sys_activate_confirm:'Zum Profil "{name}" wechseln und neu starten?\nAktuelle Konfig wird gesichert.',
    sys_title:'System',sys_sec_status:'Status',sys_sec_host:'Host',sys_sec_radio:'Funk-Hardware',sys_sec_sensors:'Sensoren',sys_sec_profiles:'Profile',sys_sec_sds:'SDS-Rundsendung',sys_refresh:'Aktualisieren',sys_probe:'Prüfen',sys_temp_hot:'HEISS',sys_temp_warm:'Warm',sys_temp_ok:'OK',
    sys_bts:'BTS-Verbindung',
  },
  es:{
    bts_ip:'IP BTS',offline:'SIN CONEXIÓN',online:'EN LÍNEA',
    brew_online:'EN LÍNEA',brew_offline:'SIN CONEXIÓN',
    stations:'Radios',calls:'Llamadas',lastheard:'Última Actividad',log:'Log',rf:'RF',health:'Health',echolink:'EchoLink',echolink_title:'EchoLink',config:'Config',
    sdslog:'Registro SDS',th_dir:'Dir',th_from:'De',th_to:'Para',th_message:'Mensaje',no_sds:'Aún no hay mensajes SDS',sds_refresh:'Actualizar',
    rf_freq:'Frecuencia central',rf_rate:'Tasa de muestreo',rf_rms:'RMS',rf_peak:'Pico',rf_age:'Captura',
    rf_waiting:'esperando…',rf_live:'en vivo',rf_stale:'obsoleto',
    rf_visualizers:'Visualizadores',rf_spectrum:'Espectro TX DSP (pre-PA)',rf_constellation:'Constelación TX DSP',
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
    clear:'Limpiar',export:'Exportar',restart:'Reiniciar',shutdown:'Apagar',save:'Guardar',
    cfg_sec_configuration:'Configuración',cfg_sec_access:'Control de acceso',cfg_sec_wx:'WX / METAR',whitelist_title:'Lista blanca ISSI',whitelist_add:'Añadir ISSI',whitelist_empty:'Lista vacía — red abierta (cualquier radio puede registrarse).',
    whitelist_help:'Cuando la lista está vacía, cualquier radio puede registrarse (red abierta). Con entradas, solo se aceptan los ISSI listados; el resto se rechazan. Los cambios se aplican al instante y persisten tras reiniciar.',
    whitelist_enforced:'ACTIVA',whitelist_open:'ABIERTA',whitelist_invalid:'Introduce un ISSI válido (1–16777215).',
    wx_title:'Servicio WX / METAR',wx_help:'Servicio meteorológico integrado. Las radios envían un SDS como "METAR LROP" al ISSI del servicio y reciben un informe decodificado. Opcionalmente envía automáticamente el METAR de una estación fija a un ISSI o grupo a intervalos. Datos de aviationweather.gov.',
    wx_enabled:'Activar respuesta METAR a petición',wx_service_issi:'ISSI del servicio',wx_periodic_enabled:'Activar envío periódico',
    wx_periodic_icao:'ICAO de estación',wx_periodic_dest:'Destino',wx_periodic_isgroup:'El destino es grupo',wx_periodic_isgroup_hint:'(GSSI en vez de ISSI individual)',
    wx_periodic_interval:'Intervalo (segundos)',wx_interval_hint:'Mínimo 300 s (5 min) para no saturar la API meteorológica.',wx_periodic_incomplete:'Indica ICAO de estación y destino para el modo periódico.',
    live_sds_desc:'Transmite un mensaje de texto a todos los radios de la celda, repitiéndose al intervalo de Home Mode Display.',
    live_sds_text:'Texto del mensaje (máx. 251 caracteres)',live_sds_repeat:'Repetir (0=∞)',live_sds_send:'Difundir',
    live_sds_clear_all:'Borrar Todo',live_sds_empty:'No hay difusiones activas.',
    live_sds_sent:'enviado',live_sds_times:'×',live_sds_forever:'∞',live_sds_delete:'✕',
    fallback_title:'⚠ CONFIGURACIÓN DE RESERVA ACTIVA — No se pudo cargar la configuración principal',
    sds_title:'⬡ Enviar Mensaje SDS',sds_dest:'ISSI Destino',
    sds_msg_label:'Mensaje',cancel:'Cancelar',send:'Enviar',
    th_issi:'ISSI',th_groups:'Grupos',th_ee:'Ahorro Energía',th_signal:'Señal',
    th_status:'Estado',th_last_seen:'Visto',th_actions:'Acciones',
    th_id:'ID',th_type:'Tipo',th_caller:'Llamante',
    th_dest:'Destino',th_speaker:'Hablante',th_duration:'Duración',
    th_time:'Hora',th_activity:'Actividad',
    last_heard_title:'Última Actividad',no_activity:'Sin actividad aún',
    act_call_group:'Llamada Grupo',act_call_individual:'Llamada P2P',act_sds:'SDS',
    online_badge:'EN LÍNEA',kick:'Expulsar',sds:'SDS',
    call_group:'GRUPO',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',call_emergency:'EMERGENCIA',
    emg_banner_title:'EMERGENCIA ACTIVA',integrations:'Integraciones',integ_enabled:'Activado',integ_disabled:'Desactivado',integ_error:'Error',system_sec:'Sistema',emg_chip:'EMERGENCIA',bs_label:'BS',emg_clear:'Borrar',confirm_clear_emergency:'¿Borrar emergencia para ISSI {issi}?',
    confirm_kick:'¿Expulsar ISSI {issi}?\nEl terminal será desregistrado y forzado a reconectarse.',
    dgna:'DGNA',dgna_title:'Asignación dinámica de grupo',dgna_modal_title:'⬡ Asignación dinámica de grupo',dgna_issi:'ISSI del terminal',dgna_current:'Grupos actuales',dgna_gssi:'Grupo (GSSI)',dgna_assign:'Asignar',dgna_deassign:'Quitar',
    confirm_restart:'¿Reiniciar FlowStation?\nTodas las llamadas activas se interrumpirán.',
    confirm_shutdown:'¿Apagar FlowStation?\nEl servicio se detendrá y deberá reiniciarse manualmente.',
    confirm_logout:'¿Cerrar sesión?',
    saved:'✓ Guardado — reinicia para aplicar.',save_fail:'✗ Error al guardar',conn_error:'Error de conexión.',
    update:'Update',update_available:'Actualización disponible',update_title:'Actualización OTA — github.com/razvanzeces/flowstation',
    update_confirm:'¿Obtener la última versión de main y recompilar?\nEl servicio se reiniciará automáticamente.',
    update_running:'Actualizando… no cierres esta ventana.',
    update_done_ok:'✓ Actualización completa. Reiniciando…',
    update_done_err:'✗ Actualización fallida. Ver log abajo.',
    update_close:'Cerrar',
    system:'Sistema',sys_info:'Info del Sistema',sys_hostname:'Hostname',sys_uptime:'Tiempo activo',
    sys_os:'OS',sys_version:'Versión FS',sys_config:'Config Activa',
    sys_cpu:'CPU',sys_cpu_load:'Carga CPU',sys_ram:'RAM',sys_temp:'Temp CPU',
    wifi:'WiFi',wifi_status:'Conexión actual',wifi_saved:'Redes guardadas',wifi_visible:'Redes disponibles',wifi_loading:'Cargando…',wifi_scanning:'Escaneando…',wifi_no_device:'No se detectó dispositivo WiFi.',wifi_radio_disabled:'Radio WiFi desactivada.',wifi_not_connected:'No conectado a ninguna red.',wifi_no_saved:'Sin redes guardadas.',wifi_no_networks:'Sin redes en rango.',wifi_ssid:'Red',wifi_signal:'Señal',wifi_ip:'Dirección IP',wifi_actions:'Acciones',wifi_disconnect:'Desconectar',wifi_connect:'Conectar',wifi_connect_to:'Conectar a',wifi_connecting:'Conectando…',wifi_connected:'CONECTADO',wifi_connected_ok:'Conectado.',wifi_saved_tag:'GUARDADO',wifi_open:'ABIERTO',wifi_forget:'Olvidar',wifi_confirm_forget:'Olvidar red',wifi_password:'Contraseña',wifi_hidden:'Red oculta (SSID no difundido)',wifi_add_hidden:'Red oculta',wifi_scan:'Escanear',wifi_refresh:'Actualizar',wifi_radio_off:'Desactivar WiFi',wifi_radio_on:'Activar WiFi',wifi_warn_lose_access:'Si estás conectado al dashboard vía WiFi, cambiar de red puede desconectarte temporalmente. Asegúrate de tener una vía de acceso alternativa.',wifi_err_no_ssid:'SSID requerido',cancel:'Cancelar',sys_sensors:'Sensores del Sistema',sys_sensors_empty:'No se detectaron sensores.',sys_rf:'Hardware RF (SoapySDR)',sys_autorefresh:'Auto-actualización 5s',
    profile_edit_title:'Editar Perfil Config',profile_edit_btn:'Editar',
    profile_edit_save_ok:'✓ Guardado',profile_edit_save_fail:'✗ Error al guardar',
    sys_profiles:'Perfiles de Config',sys_activate:'Activar y Reiniciar',
    sys_active_badge:'ACTIVO',sys_no_profiles:'No se encontraron perfiles .toml en el directorio.',
    sys_activate_confirm:'¿Cambiar al perfil "{name}" y reiniciar?\nLa config actual será respaldada.',
    sys_title:'Sistema',sys_sec_status:'Estado',sys_sec_host:'Host',sys_sec_radio:'Hardware de radio',sys_sec_sensors:'Sensores',sys_sec_profiles:'Perfiles',sys_sec_sds:'Difusión SDS',sys_refresh:'Actualizar',sys_probe:'Sondear',sys_temp_hot:'CALIENTE',sys_temp_warm:'Templado',sys_temp_ok:'OK',
    sys_bts:'Conexión BTS',
  },
  hu:{
    bts_ip:'BTS IP',offline:'OFFLINE',online:'ONLINE',
    brew_online:'ONLINE',brew_offline:'OFFLINE',
    stations:'Rádiók',calls:'Hívások',lastheard:'Utoljára Hallott',log:'Napló',rf:'RF',health:'Health',echolink:'EchoLink',echolink_title:'EchoLink',config:'Konfig',
    sdslog:'SDS Napló',th_dir:'Irány',th_from:'Feladó',th_to:'Címzett',th_message:'Üzenet',no_sds:'Még nincs SDS üzenet',sds_refresh:'Frissítés',
    rf_freq:'Központi frekvencia',rf_rate:'Mintavételezési ráta',rf_rms:'RMS',rf_peak:'Csúcs',rf_age:'Pillanatkép',
    rf_waiting:'várakozás…',rf_live:'élő',rf_stale:'elavult',
    rf_visualizers:'Vizualizációk',rf_spectrum:'TX DSP spektrum (PA előtt)',rf_constellation:'TX DSP konstelláció',
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
    clear:'Törlés',export:'Exportálás',restart:'Újraindítás',shutdown:'Leállítás',save:'Mentés',
    cfg_sec_configuration:'Konfiguráció',cfg_sec_access:'Hozzáférés-vezérlés',cfg_sec_wx:'WX / METAR',whitelist_title:'ISSI engedélyezőlista',whitelist_add:'ISSI hozzáadása',whitelist_empty:'Üres lista — nyílt hálózat (bármely rádió regisztrálhat).',
    whitelist_help:'Ha a lista üres, bármely rádió regisztrálhat (nyílt hálózat). Ha vannak elemek, csak a listázott ISSI-k engedélyezettek; a többit elutasítja. A módosítások azonnal érvénybe lépnek és újraindítás után is megmaradnak.',
    whitelist_enforced:'AKTÍV',whitelist_open:'NYÍLT',whitelist_invalid:'Adjon meg érvényes ISSI-t (1–16777215).',
    wx_title:'WX / METAR szolgáltatás',wx_help:'Beépített időjárás-szolgáltatás. A rádiók "METAR LROP" formájú SDS-t küldenek a szolgáltatás ISSI-jére, és dekódolt jelentést kapnak. Opcionálisan automatikusan elküldi egy rögzített állomás METAR-ját egy ISSI-re vagy csoportra adott időközönként. Adatok: aviationweather.gov.',
    wx_enabled:'METAR válasz kérésre engedélyezése',wx_service_issi:'Szolgáltatás ISSI',wx_periodic_enabled:'Időszakos küldés engedélyezése',
    wx_periodic_icao:'Állomás ICAO',wx_periodic_dest:'Cél',wx_periodic_isgroup:'A cél csoport',wx_periodic_isgroup_hint:'(GSSI egyedi ISSI helyett)',
    wx_periodic_interval:'Időköz (másodperc)',wx_interval_hint:'Legalább 300 mp (5 perc), hogy ne terhelje túl az időjárás API-t.',wx_periodic_incomplete:'Add meg az állomás ICAO-t és a célt az időszakos módhoz.',
    sds_title:'⬡ SDS üzenet küldése',sds_dest:'Cél ISSI',
    sds_msg_label:'Üzenet',cancel:'Mégse',send:'Küldés',
    th_issi:'ISSI',th_groups:'Csoportok',th_ee:'Energiatakarékos',th_signal:'Jelerősség',
    th_status:'Állapot',th_last_seen:'Utoljára látva',th_actions:'Műveletek',
    th_id:'ID',th_type:'Típus',th_caller:'Hívó',
    th_dest:'Cél',th_speaker:'Beszélő',th_duration:'Időtartam',
    th_time:'Idő',th_activity:'Tevékenység',
    last_heard_title:'Utoljára hallott',no_activity:'Még nincs tevékenység',
    act_call_group:'Csoportos hívás',act_call_individual:'P2P hívás',act_sds:'SDS',
    online_badge:'ONLINE',kick:'Kizárás',sds:'SDS',
    call_group:'CSOPORT',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',call_emergency:'VÉSZHÍVÁS',
    emg_banner_title:'VÉSZHELYZET AKTÍV',integrations:'Integrációk',integ_enabled:'Engedélyezve',integ_disabled:'Letiltva',integ_error:'Hiba',system_sec:'Rendszer',emg_chip:'VÉSZHELYZET',bs_label:'BS',emg_clear:'Törlés',confirm_clear_emergency:'Vészhelyzet törlése ISSI {issi}?',
    confirm_kick:'ISSI {issi} kizárása?\nA terminál törlésre kerül és újra kell csatlakoznia.',
    dgna:'DGNA',dgna_title:'Dinamikus csoport-hozzárendelés',dgna_modal_title:'⬡ Dinamikus csoport-hozzárendelés',dgna_issi:'Terminál ISSI',dgna_current:'Jelenlegi csoportok',dgna_gssi:'Csoport (GSSI)',dgna_assign:'Hozzárendel',dgna_deassign:'Eltávolít',
    confirm_restart:'Újraindítja a FlowStation-t?\nAz összes aktív hívás megszakad.',
    confirm_shutdown:'Leállítja a FlowStation-t?\nA szolgáltatást kézzel kell újraindítani.',
    confirm_logout:'Kijelentkezik?',
    saved:'✓ Mentve — újraindítás szükséges az alkalmazáshoz.',save_fail:'✗ Mentési hiba',conn_error:'Kapcsolódási hiba.',
    update:'Frissítés',update_available:'Elérhető frissítés',update_title:'OTA frissítés — github.com/razvanzeces/flowstation',
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
    sys_title:'Rendszer',sys_sec_status:'Állapot',sys_sec_host:'Gazda',sys_sec_radio:'Rádió hardver',sys_sec_sensors:'Szenzorok',sys_sec_profiles:'Profilok',sys_sec_sds:'SDS sugárzás',sys_refresh:'Frissítés',sys_probe:'Vizsgálat',sys_temp_hot:'FORRÓ',sys_temp_warm:'Meleg',sys_temp_ok:'OK',
    sys_bts:'BTS kapcsolat',
    wifi:'WiFi',wifi_status:'Jelenlegi kapcsolat',wifi_saved:'Mentett hálózatok',wifi_visible:'Elérhető hálózatok',wifi_loading:'Betöltés…',wifi_scanning:'Keresés…',wifi_no_device:'Nem észlelhető WiFi eszköz.',wifi_radio_disabled:'WiFi rádió letiltva.',wifi_not_connected:'Nincs kapcsolat hálózathoz.',wifi_no_saved:'Nincs mentett hálózat.',wifi_no_networks:'Nincs hálózat hatótávolságon belül.',wifi_ssid:'Hálózat',wifi_signal:'Jelerősség',wifi_ip:'IP-cím',wifi_actions:'Műveletek',wifi_disconnect:'Bontás',wifi_connect:'Csatlakozás',wifi_connect_to:'Csatlakozás:',wifi_connecting:'Csatlakozás…',wifi_connected:'KAPCSOLÓDVA',wifi_connected_ok:'Csatlakoztatva.',wifi_saved_tag:'MENTETT',wifi_open:'NYITOTT',wifi_forget:'Elfelejtés',wifi_confirm_forget:'Hálózat elfelejtése',wifi_password:'Jelszó',wifi_hidden:'Rejtett hálózat (SSID nem sugárzott)',wifi_add_hidden:'Rejtett hálózat',wifi_scan:'Keresés',wifi_refresh:'Frissítés',wifi_radio_off:'WiFi letiltása',wifi_radio_on:'WiFi engedélyezése',wifi_warn_lose_access:'Ha WiFi-n keresztül csatlakozol a vezérlőpulthoz, a hálózat módosítása lecsatlakoztathat. Biztosíts alternatív hozzáférést.',wifi_err_no_ssid:'SSID szükséges',cancel:'Mégse',sys_sensors:'Gazdagép szenzorok',sys_sensors_empty:'Nem észlelhetők szenzorok.',
  },
  zh:{
    bts_ip:'BTS IP',offline:'离线',online:'在线',
    brew_online:'在线',brew_offline:'离线',
    stations:'终端',calls:'通话',lastheard:'最近通话',log:'日志',rf:'RF',health:'Health',echolink:'EchoLink',echolink_title:'EchoLink',config:'配置',
    sdslog:'SDS日志',th_dir:'方向',th_from:'发件',th_to:'收件',th_message:'消息',no_sds:'暂无SDS消息',sds_refresh:'刷新',
    rf_freq:'中心频率',rf_rate:'采样率',rf_rms:'RMS',rf_peak:'峰值',rf_age:'快照',
    rf_waiting:'等待中…',rf_live:'实时',rf_stale:'已过期',
    rf_visualizers:'可视化',rf_spectrum:'TX DSP 频谱（功放前）',rf_constellation:'TX DSP 星座图',
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
    clear:'清除',export:'导出',restart:'重启',shutdown:'关机',save:'保存',
    cfg_sec_configuration:'配置',cfg_sec_access:'访问控制',cfg_sec_wx:'WX / METAR',whitelist_title:'ISSI 白名单',whitelist_add:'添加 ISSI',whitelist_empty:'列表为空 — 开放网络（任何电台均可注册）。',
    whitelist_help:'列表为空时，任何电台均可注册（开放网络）。有条目时，仅接受列出的 ISSI，其余一律拒绝。更改即时生效并在重启后保留。',
    whitelist_enforced:'已启用',whitelist_open:'开放',whitelist_invalid:'请输入有效的 ISSI（1–16777215）。',
    wx_title:'WX / METAR 服务',wx_help:'内置气象服务。电台向服务 ISSI 发送如 "METAR LROP" 的 SDS 即可获得解码报告。可选择按间隔自动向 ISSI 或群组发送固定台站的 METAR。数据来自 aviationweather.gov。',
    wx_enabled:'启用按需 METAR 响应',wx_service_issi:'服务 ISSI',wx_periodic_enabled:'启用定时广播',
    wx_periodic_icao:'台站 ICAO',wx_periodic_dest:'目标',wx_periodic_isgroup:'目标为群组',wx_periodic_isgroup_hint:'（GSSI 而非单个 ISSI）',
    wx_periodic_interval:'间隔（秒）',wx_interval_hint:'最少 300 秒（5 分钟），以免频繁请求气象 API。',wx_periodic_incomplete:'定时模式需同时设置台站 ICAO 和目标。',
    sds_title:'⬡ 发送 SDS 短消息',sds_dest:'目标 ISSI',
    live_sds_desc:'向本小区所有终端广播文本消息，按 Home Mode Display 间隔重复发送。直到删除或达到重复次数为止。',
    live_sds_text:'消息内容（最多 251 字符）',live_sds_repeat:'重复次数 (0=无限)',live_sds_send:'广播',
    live_sds_clear_all:'清除全部',live_sds_empty:'暂无广播任务。',
    live_sds_sent:'已发送',live_sds_times:'次',live_sds_forever:'∞',live_sds_delete:'删除',
    fallback_title:'⚠ 正在使用后备配置 — 主配置加载失败',
    sds_msg_label:'消息内容',cancel:'取消',send:'发送',
    th_issi:'ISSI',th_groups:'群组',th_ee:'节能',th_signal:'信号',
    th_status:'状态',th_last_seen:'最后在线',th_actions:'操作',
    th_id:'ID',th_type:'类型',th_caller:'主叫',
    th_dest:'被叫',th_speaker:'讲话者',th_duration:'时长',
    th_time:'时间',th_activity:'活动',
    last_heard_title:'最近通话记录',no_activity:'暂无活动记录',
    act_call_group:'组呼',act_call_individual:'点对点',act_sds:'SDS',
    online_badge:'在线',kick:'踢下线',sds:'SDS',
    call_group:'组呼',call_p2p_s:'P2P-S',call_p2p_d:'P2P-D',call_emergency:'紧急呼叫',
    emg_banner_title:'紧急状态激活',integrations:'集成',integ_enabled:'已启用',integ_disabled:'已禁用',integ_error:'错误',system_sec:'系统',emg_chip:'紧急',bs_label:'BS',emg_clear:'清除',confirm_clear_emergency:'清除 ISSI {issi} 的紧急状态？',
    confirm_kick:'确定踢下 ISSI {issi}？\n终端将被注销并强制重新注册。',
    dgna:'DGNA',dgna_title:'动态组分配',dgna_modal_title:'⬡ 动态组分配',dgna_issi:'终端 ISSI',dgna_current:'当前组',dgna_gssi:'组 (GSSI)',dgna_assign:'分配',dgna_deassign:'移除',
    confirm_restart:'确定重启 FlowStation？\n所有正在进行的通话将被中断。',
    confirm_shutdown:'确定关闭 FlowStation？\n服务将停止，需要手动重启。',
    confirm_logout:'确定注销吗？',
    saved:'✓ 已保存 — 重启后生效',save_fail:'✗ 保存失败',conn_error:'连接错误',
    update:'更新',update_available:'有可用更新',update_title:'OTA 在线更新 — github.com/razvanzeces/flowstation',
    update_confirm:'是否从 main 分支拉取最新代码并重新构建？\n服务将自动重启。',
    update_running:'正在更新… 请不要关闭此窗口',
    update_done_ok:'✓ 更新完成，正在重启…',
    update_done_err:'✗ 更新失败，请查看下方日志',
    update_close:'关闭',
    system:'系统',sys_info:'系统信息',sys_hostname:'主机名',sys_uptime:'运行时间',
    sys_version:'FS 版本',sys_os:'操作系统',sys_config:'当前配置',
    sys_cpu:'CPU',sys_cpu_load:'CPU 负载',sys_ram:'内存',sys_temp:'CPU 温度',
    wifi:'WiFi',wifi_status:'当前连接',wifi_saved:'已保存的网络',wifi_visible:'可用网络',wifi_loading:'加载中…',wifi_scanning:'扫描中…',wifi_no_device:'未检测到 WiFi 设备。',wifi_radio_disabled:'WiFi 已禁用。',wifi_not_connected:'未连接任何网络。',wifi_no_saved:'无已保存的网络。',wifi_no_networks:'范围内无可用网络。',wifi_ssid:'网络',wifi_signal:'信号',wifi_ip:'IP 地址',wifi_actions:'操作',wifi_disconnect:'断开',wifi_connect:'连接',wifi_connect_to:'连接到',wifi_connecting:'连接中…',wifi_connected:'已连接',wifi_connected_ok:'已连接。',wifi_saved_tag:'已保存',wifi_open:'开放',wifi_forget:'忘记',wifi_confirm_forget:'忘记网络',wifi_password:'密码',wifi_hidden:'隐藏网络 (SSID 不广播)',wifi_add_hidden:'隐藏网络',wifi_scan:'扫描',wifi_refresh:'刷新',wifi_radio_off:'禁用 WiFi',wifi_radio_on:'启用 WiFi',wifi_warn_lose_access:'如果您通过 WiFi 连接到仪表板,更换网络可能会暂时断开您的连接。请确保有备用访问方式。',wifi_err_no_ssid:'需要 SSID',cancel:'取消',sys_sensors:'主机硬件传感器',sys_sensors_empty:'未检测到传感器。',sys_rf:'RF 硬件 (SoapySDR)',sys_autorefresh:'自动刷新 5秒',
    profile_edit_title:'编辑配置文件',profile_edit_btn:'编辑',
    profile_edit_save_ok:'✓ 已保存',profile_edit_save_fail:'✗ 保存失败',
    sys_profiles:'配置文件',sys_activate:'激活并重启',
    sys_active_badge:'当前使用',sys_no_profiles:'配置目录中未找到 .toml 配置文件。',
    sys_activate_confirm:'切换到配置文件 "{name}" 并重启？\n当前配置将被备份。',
    sys_title:'系统',sys_sec_status:'状态',sys_sec_host:'主机',sys_sec_radio:'射频硬件',sys_sec_sensors:'传感器',sys_sec_profiles:'配置档案',sys_sec_sds:'SDS 广播',sys_refresh:'刷新',sys_probe:'探测',sys_temp_hot:'过热',sys_temp_warm:'温热',sys_temp_ok:'正常',
    sys_bts:'BTS 连接',
  },
};

let currentLang=localStorage.getItem('fs_lang')||'en';
function t(k,v){let s=(LANGS[currentLang]||LANGS.en)[k]||(LANGS.en[k]||k);if(v)Object.keys(v).forEach(x=>{s=s.replace('{'+x+'}',v[x]);});return s;}
function applyLang(){
  document.querySelectorAll('[data-i18n]').forEach(el=>el.textContent=t(el.getAttribute('data-i18n')));
  document.querySelectorAll('[data-i18n-tab]').forEach(el=>el.textContent=t(el.getAttribute('data-i18n-tab')));
  // Update nav labels
  ['stations','calls','lastheard','log','config','telegram','system'].forEach(p=>{
    const el=document.querySelector(`#nav-${p} .nav-label`);
    if(el)el.textContent=t(p);
  });
  renderStations();renderCalls();renderLastHeard();renderEmergencyBanner();
}
function setLang(l,btn){
  currentLang=l;localStorage.setItem('fs_lang',l);
  document.querySelectorAll('.lang-btn').forEach(b=>b.classList.remove('active'));
  if(btn)btn.classList.add('active');
  else document.querySelectorAll('.lang-btn').forEach(b=>{if(b.textContent.toLowerCase()===l)b.classList.add('active');});
  applyLang();
}

let currentTheme=localStorage.getItem('fs_theme')||'light';
function setTheme(theme,btn){
  currentTheme=theme;localStorage.setItem('fs_theme',theme);
  document.documentElement.setAttribute('data-theme',theme==='dark'?'':theme);
  document.querySelectorAll('.theme-btn').forEach(d=>d.classList.remove('active'));
  if(btn)btn.classList.add('active');
  else document.querySelectorAll('.theme-btn').forEach(d=>{if(d.dataset.t===theme)d.classList.add('active');});
}

// ── Readability (text size + contrast) ───────────────────────────────────────
// One multiplier --ts on <html data-uisize>, consumed by the curated readability
// block via calc(). Default = Medium (bigger out of the box). Persisted: fs_uisize.
let currentUiSize=localStorage.getItem('fs_uisize')||'m';
function applyUiSize(){
  document.documentElement.setAttribute('data-uisize',currentUiSize);
  document.querySelectorAll('.read-opt').forEach(o=>
    o.classList.toggle('active',o.dataset.size===currentUiSize));
}
function setUiSize(s){
  currentUiSize=s;localStorage.setItem('fs_uisize',s);
  applyUiSize();closeReadPop();
}
function toggleReadPop(e){
  if(e)e.stopPropagation();
  const pop=document.getElementById('read-pop'),btn=document.getElementById('read-btn');
  const open=pop.classList.toggle('open');
  if(btn)btn.setAttribute('aria-expanded',open?'true':'false');
}
function closeReadPop(){
  const pop=document.getElementById('read-pop'),btn=document.getElementById('read-btn');
  if(pop)pop.classList.remove('open');
  if(btn)btn.setAttribute('aria-expanded','false');
}
// Outside-click + Esc dismissal (matches native popover behavior)
document.addEventListener('click',e=>{
  const pop=document.getElementById('read-pop');
  if(pop&&pop.classList.contains('open')&&!e.target.closest('.eye-wrap'))closeReadPop();
});
document.addEventListener('keydown',e=>{if(e.key==='Escape')closeReadPop();});

// ── Touch mode (FH-FEAT-008) ─────────────────────────────────────────────────
// '1' = forced on, '0' = forced off, null = auto (on for coarse pointers).
let touchMode=localStorage.getItem('fs_touch');
function applyTouchMode(){
  const coarse=window.matchMedia&&window.matchMedia('(pointer:coarse)').matches;
  const on=touchMode==='1'||(touchMode===null&&coarse);
  document.body.classList.toggle('touch-mode',on);
  document.body.classList.toggle('no-touch-mode',touchMode==='0');
  const b=document.getElementById('touch-toggle');if(b)b.classList.toggle('active',on);
}
function toggleTouchMode(){
  const coarse=window.matchMedia&&window.matchMedia('(pointer:coarse)').matches;
  const currentlyOn=touchMode==='1'||(touchMode===null&&coarse);
  touchMode=currentlyOn?'0':'1';
  localStorage.setItem('fs_touch',touchMode);
  applyTouchMode();
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
const PAGE_TITLES={stations:'stations',calls:'calls',lastheard:'lastheard',log:'log',sdslog:'sdslog',rf:'rf',health:'health',asterisk:'asterisk',dapnet:'dapnet',echolink:'echolink',meshcom:'meshcom',geoalarm:'geoalarm',config:'config',system:'system'};
function showPage(name,el){
  document.querySelectorAll('.page').forEach(p=>p.classList.remove('active'));
  document.querySelectorAll('.nav-item').forEach(n=>n.classList.remove('active'));
  document.getElementById('page-'+name).classList.add('active');
  if(el)el.classList.add('active');
  else{const nav=document.getElementById('nav-'+name);if(nav)nav.classList.add('active');}
  document.getElementById('topbar-title').textContent=t(name)||name;
  if(name==='stations'){loadBtsInfoLegacy();loadDualCarrier();}
  if(name==='sdslog'){loadSdsLog();}
  if(name==='health'){loadHealthIntegrations();}
  if(name==='asterisk'){loadAsteriskStatus();loadSnomNotify();}
  if(name==='dapnet'){loadDapnet();loadDapnetLog();}
  if(name==='geoalarm'){loadGeoalarm();}
  if(name==='meshcom'){loadMeshcom();}
  if(name==='config'){loadConfig();loadWhitelist();loadWx();}
  if(name==='telegram'){loadTelegram();}
  if(name==='system'){loadSystemInfo();loadConfigProfiles();loadLiveSds();loadBrightness();}
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
  document.getElementById('wifi-modal').classList.add('open'); paintIcons(document.getElementById('wifi-modal'));
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
  document.getElementById('wifi-modal').classList.add('open'); paintIcons(document.getElementById('wifi-modal'));
  setTimeout(()=>ssidInput.focus(), 50);
}

function wifiCloseModal(){
  document.getElementById('wifi-modal').classList.remove('open');
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
let ws=null,state={ms:{},calls:{},emergencies:{},lastHeard:[],sdsLog:[],dapnetLog:[],geoalarmEvents:[],meshcomNodes:[],meshcomMessages:[],brewOnline:false,brewVer:0},sdsDest=0;

// ── RadioID callsigns (indicativ) ──────────────────────────────────────────────
// issi -> {cs:"CALLSIGN", fl:"🇷🇴"} (found; fl is the country flag emoji from the prefix, or "")
//       | "" (looked up, none). A missing key means unresolved.
let callsigns={};
let _csInflight=false;
// Render an ISSI with its RadioID callsign (and country flag, when known) appended.
function idCell(issi){const c=callsigns[issi];if(!c||!c.cs)return `<code>${issi}</code>`;const fl=c.fl?c.fl+' ':'';return `<code>${issi}</code> <span class="callsign">${fl}${escHtml(c.cs)}</span>`;}
// Resolve callsigns for every ISSI currently on screen we have not looked up yet. On-demand: the
// server fetches unknowns from RadioID in the background and caches them locally; pending IDs are
// omitted from the response and retried on the next tick. Found/absent results are cached here.
function refreshCallsigns(){
  if(_csInflight)return;
  const ids=new Set();
  Object.values(state.ms).forEach(m=>ids.add(m.issi));
  Object.values(state.calls).forEach(c=>{if(c.caller_issi)ids.add(c.caller_issi);if(c.called_issi&&c.call_type!=='group')ids.add(c.called_issi);if(c.active_speaker)ids.add(c.active_speaker);});
  state.lastHeard.forEach(e=>{if(e.issi)ids.add(e.issi);});
  (state.sdsLog||[]).forEach(e=>{if(e.source_issi)ids.add(e.source_issi);if(e.dest_issi&&!e.is_group)ids.add(e.dest_issi);});
  Object.values(state.emergencies||{}).forEach(e=>{if(e.issi)ids.add(e.issi);});
  const unknown=[...ids].filter(id=>id&&callsigns[id]===undefined).slice(0,256);
  if(!unknown.length)return;
  _csInflight=true;
  fetch('/api/callsigns?ids='+unknown.join(','))
    .then(r=>r.ok?r.json():{})
    .then(d=>{let changed=false;for(const k in d){if(callsigns[k]!==d[k]){callsigns[k]=d[k];changed=true;}}if(changed){renderStations();renderCalls();renderLastHeard();renderSdsLog();renderEmergencyBanner();}})
    .catch(()=>{})
    .finally(()=>{_csInflight=false;});
}
setInterval(refreshCallsigns,4000);
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

// Persistent emergency banner — shown while >=1 ISSI is in active emergency. Each active ISSI
// gets a chip with a Clear button (operator clear). Driven by state.emergencies.
function renderEmergencyBanner(){
  const b=document.getElementById('emergency-banner'),list=document.getElementById('emergency-banner-list');
  if(!b||!list)return;
  const titleEl=b.querySelector('[data-i18n="emg_banner_title"]');
  if(titleEl)titleEl.textContent=t('emg_banner_title');
  const arr=Object.values(state.emergencies||{});
  syncTopbarChips();
  if(!arr.length){b.style.display='none';list.innerHTML='';return;}
  b.style.display='flex';
  list.innerHTML=arr.sort((a,b)=>a.issi-b.issi).map(e=>{
    // callsigns[issi] is an object {cs, fl} (see idCell/tsIssiText), not a string.
    const c=callsigns[e.issi];
    const fl=(c&&c.fl)?c.fl+' ':'';
    const who=(c&&c.cs)?(e.issi+' · '+fl+c.cs):(''+e.issi);
    return `<span style="display:inline-flex;align-items:center;gap:6px;background:rgba(255,255,255,0.18);border-radius:4px;padding:2px 8px"><code style="color:#fff">${who}</code><button onclick="clearEmergency(${e.issi})" style="padding:1px 7px;background:#fff;color:var(--danger);border:none;border-radius:3px;font-weight:600;cursor:pointer;font-size:11px">${t('emg_clear')}</button></span>`;
  }).join('');
}
function clearEmergency(issi){if(!confirm(t('confirm_clear_emergency',{issi})))return;wsSend({type:'emergency_clear',issi});}

// ── Topbar status chips (BS / Brew / Emergency) — calm always-visible state.
// Mirrors the footer LEDs + emergency state onto the .pill chips in the header.
function syncTopbarChips(){
  const led=document.getElementById('connLed');
  const bsOn=!!(led&&led.classList.contains('on'));
  const bs=document.getElementById('chip-bs');
  if(bs){
    bs.className='pill '+(bsOn?'pill-ok':'pill-idle');
    const lbl=bs.querySelector('[data-i18n="bs_label"]');
    if(lbl)lbl.textContent='BS '+(bsOn?t('online'):t('offline'));
  }
  const brew=document.getElementById('chip-brew');
  if(brew){
    brew.className='pill '+(state.brewOnline?'pill-info':'pill-idle');
    const span=brew.querySelector('span');
    if(span)span.textContent=state.brewOnline?('Brew v'+(state.brewVer||0)):'Brew';
  }
  const emg=document.getElementById('chip-emergency');
  if(emg)emg.style.display=Object.keys(state.emergencies||{}).length?'inline-flex':'none';
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
  // Update stat card — state via ONE class (kills inline color split).
  const bv=document.getElementById('stat-brew-val');
  const bs=document.getElementById('stat-brew-sub');
  const bcard=document.getElementById('stat-brew-card');
  if(bv){bv.textContent=online?t('brew_online'):t('brew_offline');}
  if(bcard){bcard.classList.remove('is-info','is-danger');bcard.classList.add(online?'is-info':'is-danger');}
  if(bs)bs.textContent=online?`Brew v${version||0}`:'—';
  const hb=document.getElementById('stations-hero-brew');
  if(hb)hb.textContent=online?`v${version||0}`:t('brew_offline');
  // System panel
  updateSysBtsPanel(document.getElementById('connLed').classList.contains('on'),online,version||0);
  syncTopbarChips();
}

function connect(){
  const proto=location.protocol==='https:'?'wss:':'ws:';
  ws=new WebSocket(`${proto}//${location.host}/ws`);
  ws.onopen=()=>{
    document.getElementById('connLed').classList.add('on');
    const ct=document.getElementById('connText');ct.textContent=t('online');ct.style.color='var(--accent)';
    updateSysBtsPanel(true,state.brewOnline,state.brewVer);
    syncTopbarChips();
    ws.send(JSON.stringify({type:'subscribe'}));
  };
  ws.onclose=()=>{
    document.getElementById('connLed').classList.remove('on');
    const ct=document.getElementById('connText');ct.textContent=t('offline');ct.style.color='var(--danger)';
    setBrewStatus(false,0);
    updateSysBtsPanel(false,false,0);
    syncTopbarChips();
    setTimeout(connect,3000);
  };
  ws.onmessage=(e)=>{try{handleMsg(JSON.parse(e.data));}catch{}};
}

function handleMsg(msg){
  switch(msg.type){
    case 'snapshot':
      state.ms={};state.calls={};state.emergencies={};state.lastHeard=msg.last_heard||[];
      (msg.emergencies||[]).forEach(e=>{state.emergencies[e.issi]={...e};});
      (msg.ms||[]).forEach(m=>{state.ms[m.issi]={...m,_last_seen_ts:Date.now()-(m.last_seen_secs_ago||0)*1000,energy_saving_mode:m.energy_saving_mode||0};});
      (msg.calls||[]).forEach(c=>{
        state.calls[c.call_id]={...c,started_at:Date.now()-(c.started_secs_ago||0)*1000};
        if(c.carrier_num!=null)tsEnsureCarrierInfo(c.carrier_num);
        if(c.peer_carrier_num!=null)tsEnsureCarrierInfo(c.peer_carrier_num);
        if(tsCanRenderAssignedCarrier(c.carrier_num,c.ts)){
          const sub=c.call_type==='group'?t('call_group'):(c.simplex?t('call_p2p_s'):t('call_p2p_d'));
          tsSetCallCarrier(c.carrier_num,c.ts,{...c,sub});
          const peerCarrier=c.peer_carrier_num!=null?c.peer_carrier_num:c.carrier_num;
          if(tsCanRenderAssignedCarrier(peerCarrier,c.peer_ts))tsSetCallCarrier(peerCarrier,c.peer_ts,{...c,sub});
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
      if(msg.health){handleHealth(msg.health);}
      renderAll();renderEmergencyBanner();refreshCallsigns();break;
    case 'brew_status':
      setBrewStatus(!!msg.connected,msg.brew_version||0);break;
    case 'ms_registered':
      // Defaults include selected_group:null so a re-register event doesn't strip the
      // property off an existing entry (Object.assign with a defaults object that omits the
      // key would otherwise just leave whatever was there — that part is fine — but freshly
      // registered entries must have a defined-but-null selected_group so the equality
      // comparison `g === sel` in renderStations behaves consistently with the server-side
      // None initialiser in server.rs.
      state.ms[msg.issi]=Object.assign({issi:msg.issi,groups:[],selected_group:null,rssi_dbfs:null,energy_saving_mode:0},state.ms[msg.issi]||{},{issi:msg.issi,_last_seen_ts:Date.now()});
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
      if(state.ms[msg.issi]){
        const rem=new Set(msg.groups||[]);
        state.ms[msg.issi].groups=(state.ms[msg.issi].groups||[]).filter(g=>!rem.has(g));
        // Drop a stale selected_group pointer if the detach removed the actively-selected TG.
        if(state.ms[msg.issi].selected_group!=null&&rem.has(state.ms[msg.issi].selected_group))state.ms[msg.issi].selected_group=null;
      }
      renderStations();break;
    case 'ms_groups_all':
      if(state.ms[msg.issi]){
        state.ms[msg.issi].groups=msg.groups||[];
        // Drop selected_group if it's no longer in the affiliated list (e.g. scan list rebuild,
        // or all detached). Keeps the data model and the visible state consistent.
        const sg=state.ms[msg.issi].selected_group;
        if(sg!=null&&!(state.ms[msg.issi].groups||[]).includes(sg))state.ms[msg.issi].selected_group=null;
      }
      renderStations();break;
    case 'call_started':
      state.calls[msg.call_id]={...msg,started_at:Date.now()};
      if(msg.carrier_num!=null)tsEnsureCarrierInfo(msg.carrier_num);
      if(msg.peer_carrier_num!=null)tsEnsureCarrierInfo(msg.peer_carrier_num);
      // The caller keyed up on this GSSI → it's their actively-selected TG.
      if(msg.call_type==='group'&&msg.gssi!=null&&state.ms[msg.caller_issi]){state.ms[msg.caller_issi].selected_group=msg.gssi;renderStations();}
      if(msg.last_heard)pushLastHeard(msg.last_heard);
      if(tsCanRenderAssignedCarrier(msg.carrier_num,msg.ts)){
        const sub=msg.call_type==='group'?t('call_group'):(msg.simplex?t('call_p2p_s'):t('call_p2p_d'));
        tsSetCallCarrier(msg.carrier_num,msg.ts,{...msg,sub});
        const peerCarrier=msg.peer_carrier_num!=null?msg.peer_carrier_num:msg.carrier_num;
        if(tsCanRenderAssignedCarrier(peerCarrier,msg.peer_ts))tsSetCallCarrier(peerCarrier,msg.peer_ts,{...msg,sub});
        updateTsBlocksCarrier();
      }
      renderCalls();renderLastHeard();break;
    case 'call_ended':
      tsClearCallCarrier(msg.call_id);updateTsBlocksCarrier();
      delete state.calls[msg.call_id];renderCalls();break;
    case 'ts_voice':
      if(msg.carrier_num!=null)tsVoiceCarrier(msg.carrier_num,msg.ts,msg.speaker_issi);break;
    case 'speaker_changed':
      if(state.calls[msg.call_id])state.calls[msg.call_id].active_speaker=msg.speaker_issi;
      if(msg.carrier_num!=null)tsEnsureCarrierInfo(msg.carrier_num);
      tsSetSpeakerCarrier(msg.call_id,msg.carrier_num,msg.ts,msg.speaker_issi);updateTsBlocksCarrier();
      // The new speaker has this call's GSSI selected (looked up from the active call).
      {const activeCall=state.calls[msg.call_id];
       const sg=activeCall&&activeCall.call_type==='group'?activeCall.gssi:null;
       if(sg!=null&&state.ms[msg.speaker_issi]){state.ms[msg.speaker_issi].selected_group=sg;renderStations();}}
      if(msg.last_heard){pushLastHeard(msg.last_heard);renderLastHeard();}
      renderCalls();break;
    case 'ms_energy_saving':
      if(state.ms[msg.issi])state.ms[msg.issi].energy_saving_mode=msg.mode;
      renderStations();break;
    case 'last_heard':
      pushLastHeard({issi:msg.issi,activity:msg.activity,dest:msg.dest,ts:new Date().toTimeString().slice(0,8)});
      renderLastHeard();break;
    case 'log':appendLog(msg);break;
    case 'sds_log':
      if(!state.sdsLog)state.sdsLog=[];
      state.sdsLog.unshift({ts:nowStamp(),direction:msg.direction,source_issi:msg.source_issi,dest_issi:msg.dest_issi,is_group:msg.is_group,protocol_id:msg.protocol_id,text:msg.text});
      if(state.sdsLog.length>500)state.sdsLog.pop();
      renderSdsLog();refreshCallsigns();break;
    case 'dapnet_log':
      if(!state.dapnetLog)state.dapnetLog=[];
      state.dapnetLog.unshift({ts:nowStamp(),direction:msg.direction,id:msg.id,callsign:msg.callsign,recipient:msg.recipient,text:msg.text,priority:msg.priority,paths:msg.paths||[]});
      if(state.dapnetLog.length>500)state.dapnetLog.pop();
      renderDapnetLog();break;
    case 'tx_visual':handleTxVisual(msg);break;
    case 'tx_quality':handleTxQuality(msg);break;
    case 'sdr_health':handleSdrHealth(msg);break;
    case 'sys_health':handleSysHealth(msg);break;
    case 'emergency_added':
      state.emergencies[msg.issi]={issi:msg.issi,dest_ssi:msg.dest_ssi,started_secs_ago:0};
      renderEmergencyBanner();renderStations();break;
    case 'emergency_removed':
      delete state.emergencies[msg.issi];
      renderEmergencyBanner();renderStations();break;
    case 'health':handleHealth(msg);break;
  }
}

// ── Render helpers ────────────────────────────────────────────────────────
// Small battery-with-bolt glyph — conveys "Energy Economy" (power-saving) at a glance.
const EE_ICON='<svg viewBox="0 0 24 24" width="9" height="9" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" style="vertical-align:-1px;margin-right:3px;flex-shrink:0"><rect x="2" y="7" width="16" height="10" rx="2"/><path d="M22 10v4" stroke-linecap="round"/><path d="M10.5 9.5 8 13h3l-2.5 3.5" fill="none" stroke-linecap="round"/></svg>';
function eeLabel(mode){
  if(!mode||mode===0)return '<span class="muted" style="font-size:10px">—</span>';
  const labels=['','EG1','EG2','EG3','EG4','EG5','EG6','EG7'];
  // Severity tier → .pill variant (no inline color literals).
  const variants=['','pill-ok','pill-ok','pill-info','pill-info','pill-warn','pill-danger','pill-danger'];
  const tips=['','~1s','~2s','~3s','~4s','~5s','~6s','~7s'];
  const v=variants[mode]||'pill-idle';
  return `<span class="pill ${v} no-dot" title="Energy Economy Mode ${mode} — wake ${tips[mode]}"><span class="pill-icon">${EE_ICON}</span>${labels[mode]}</span>`;
}
function lastSeenLabel(secs){
  if(secs==null)return'<span class="muted num">—</span>';
  if(secs<5)return'<span class="num" style="color:var(--ok)">now</span>';
  if(secs<60)return`<span class="num accent">${secs}s</span>`;
  if(secs<3600)return`<span class="num">${Math.floor(secs/60)}m${secs%60}s</span>`;
  return`<span class="num" style="color:var(--warn)">${Math.floor(secs/3600)}h${Math.floor((secs%3600)/60)}m</span>`;
}
function pushLastHeard(entry){
  const now=new Date().toTimeString().slice(0,8);
  state.lastHeard.unshift({ts:entry.ts||now,issi:entry.issi,activity:entry.activity,dest:entry.dest||0});
  if(state.lastHeard.length>50)state.lastHeard.length=50;
}
function activityBadge(activity){
  if(activity==='call_group')return`<span class="pill pill-info">${t('act_call_group')}</span>`;
  if(activity==='call_individual')return`<span class="pill pill-warn">${t('act_call_individual')}</span>`;
  if(activity==='sds')return`<span class="pill pill-info">${t('act_sds')}</span>`;
  return`<span class="pill pill-idle">${activity}</span>`;
}
function rssiColor(v){if(v==null)return'var(--text3)';if(v>-20)return'var(--accent)';if(v>-30)return'var(--accent2)';if(v>-40)return'var(--warn)';return'var(--danger)';}
function rssiPct(v){if(v==null)return 0;return Math.max(0,Math.min(100,(v+60)/50*100));}
// Map RSSI to a .gauge threshold class (no JS color literals): strong=ok,
// usable=info, marginal=warn, weak/none=danger/idle.
function rssiGaugeClass(v){if(v==null)return'is-idle';if(v>-20)return'';if(v>-30)return'is-info';if(v>-40)return'is-warn';return'is-danger';}
function escHtml(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');}
function renderAll(){renderStations();renderCalls();renderLastHeard();updateTsBlocksCarrier();}

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
      sub.textContent='ACTIVE';
      // subtle MCCH wave animation
      if(!tsWaveHeights[0].length)tsRandWave(1);
      tsApplyWave(1,true);
      if(dur)dur.style.width='0%';
      continue;
    }

    const st=tsState[i];
    const timer=block.querySelector('.ts-timer');
    if(!st){
      block.className='ts-block';
      label.textContent='—';
      sub.textContent='Idle';
      tsApplyWave(ts,false);
      if(timer)timer.textContent='';
      if(dur)dur.style.width='0%';
      continue;
    }

    const voiceRecent=st.voice_ts&&(now-st.voice_ts)<TS_VOICE_DECAY_MS;
    // Top line = GSSI (talkgroup) for group calls / called ISSI for individual;
    // bottom line = the ISSI currently keyed up, with its RadioID callsign when known.
    const lines=tsLines(st);
    label.textContent=lines.top;

    if(voiceRecent){
      block.className='ts-block voice';
      sub.textContent=lines.bottom?('▶ '+lines.bottom):'▶ TX';
    } else {
      block.className='ts-block call';
      sub.textContent=lines.bottom||(st.sub||'Alloc');
    }
    // Emergency call (ETSI priority 15): overlay the danger ring on the call/voice state.
    if((st.priority||0)>=15)block.classList.add('emergency');
    if(timer){
      const elapsed=Math.floor((now-(st.started_at||now))/1000);
      timer.textContent=elapsed>0?formatDur(elapsed):'';
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

// Render an ISSI + its RadioID callsign (indicativ) compactly for the TS sub-line.
function tsIssiText(issi){
  if(!issi)return '';
  const c=callsigns[issi];
  if(!c||!c.cs)return ''+issi;
  const fl=c.fl?c.fl+' ':'';
  return issi+' · '+fl+c.cs;
}
// Compute the two text lines for an active timeslot from its call state:
//   top    → GSSI (talkgroup number) for group calls, else the called ISSI / P2P
//   bottom → the ISSI currently transmitting, with callsign when resolved
function tsLines(st){
  const speaker=st.speaker_issi||st.caller_issi;
  if(st.call_type==='group'){
    // Group calls (the normal traffic-channel case): GSSI on top, speaking ISSI below.
    return {top: st.gssi!=null?('GSSI '+st.gssi):'GROUP', bottom: tsIssiText(speaker)};
  }
  // Individual / point-to-point calls have no talkgroup — label the top line clearly
  // so it never shows a bare "ISSI" that reads like a misplaced GSSI.
  return {top:'PRIVATE', bottom: tsIssiText(speaker)};
}
function tsSetCall(ts, call){
  if(ts<2||ts>4)return;
  tsState[ts-1]={
    call_id:call.call_id, call_type:call.call_type,
    gssi:call.gssi, called_issi:call.called_issi, caller_issi:call.caller_issi,
    speaker_issi:call.active_speaker||call.speaker_issi||call.caller_issi,
    simplex:call.simplex, sub:call.sub, priority:call.priority||0,
    voice_ts:null, started_at:Date.now()
  };
}
// Point a timeslot at the ISSI now transmitting (group-call speaker hand-offs).
function tsSetSpeaker(call_id, speaker_issi){
  for(let i=1;i<4;i++){if(tsState[i]&&tsState[i].call_id===call_id)tsState[i].speaker_issi=speaker_issi;}
}
function tsClearCall(call_id){
  for(let i=1;i<4;i++){if(tsState[i]&&tsState[i].call_id===call_id)tsState[i]=null;}
}
function tsVoice(ts){
  if(ts<2||ts>4)return;
  if(!tsState[ts-1])tsState[ts-1]={call_id:0,call_type:'',gssi:null,voice_ts:null,started_at:Date.now()};
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

// Carrier-aware RF visualizer. The original strip above assumes a single carrier;
// this overlay keeps the same look but keys everything by carrier+timeslot so a
// secondary RF carrier gets its own labelled 4-slot row.
const tsStateCarrier={};
const tsWaveHeightsCarrier={};
const tsCarrierInfo={};

function fmtMhz(hz,dp){return(hz!=null&&isFinite(hz))?(hz/1e6).toFixed(dp==null?4:dp)+' MHz':'-';}
function tsCarrierKey(carrierNum,ts){return String(carrierNum)+':'+String(ts);}
function tsCanRenderAssignedCarrier(carrierNum,ts){
  if(carrierNum==null||!isFinite(carrierNum)||ts==null||!isFinite(ts))return false;
  if(ts<1||ts>4)return false;
  if(state.mainCarrierNum!=null&&carrierNum===state.mainCarrierNum)return ts>=2&&ts<=4;
  return true;
}
function tsCarrierNumbers(){
  return Object.keys(tsCarrierInfo).map(Number).filter(Number.isFinite).sort((a,b)=>a-b);
}
function tsEnsureCarrierInfo(carrierNum,txFreqHz,rxFreqHz){
  if(carrierNum==null||!isFinite(carrierNum))return;
  const key=String(carrierNum);
  const info=tsCarrierInfo[key]||{carrier_num:carrierNum,tx_freq_hz:null,rx_freq_hz:null};
  if(txFreqHz!=null&&isFinite(txFreqHz))info.tx_freq_hz=txFreqHz;
  if(rxFreqHz!=null&&isFinite(rxFreqHz))info.rx_freq_hz=rxFreqHz;
  tsCarrierInfo[key]=info;
}
function tsCarrierMeta(info){
  const parts=[];
  if(info&&info.tx_freq_hz!=null)parts.push('DL '+fmtMhz(info.tx_freq_hz));
  if(info&&info.rx_freq_hz!=null)parts.push('UL '+fmtMhz(info.rx_freq_hz));
  return parts.join(' | ')||'Waiting for RF info';
}
function tsCarrierBlockHtml(carrierNum,ts){
  const idleHeights=(carrierNum===state.mainCarrierNum&&ts===1)?[8,14,10,16,8,12,6]:[3,3,3,3,3,3,3];
  return `<div class="ts-block${carrierNum===state.mainCarrierNum&&ts===1?' mcch':''}" id="ts-block-${carrierNum}-${ts}">
    <div class="ts-num">TS ${ts}</div>
    ${ts===1?'':'<div class="ts-timer"></div>'}
    <div class="ts-led"></div>
    <div class="ts-wave">${idleHeights.map(h=>`<div class="ts-wave-bar" style="height:${h}px"></div>`).join('')}</div>
    <div class="ts-label">${carrierNum===state.mainCarrierNum&&ts===1?'MCCH':(ts===1?'BCCH':'-')}</div>
    <div class="ts-sub">${carrierNum===state.mainCarrierNum&&ts===1?'ACTIVE':(ts===1?'SECONDARY':'Idle')}</div>
    <div class="ts-flash"></div>
    <div class="ts-duration-bar"></div>
  </div>`;
}
function renderTsGridCarrier(){
  const grid=document.getElementById('ts-grid');
  if(!grid)return;
  let carriers=tsCarrierNumbers();
  if(!carriers.length&&state.mainCarrierNum!=null)carriers=[state.mainCarrierNum];
  if(!carriers.length)return;
  grid.innerHTML=carriers.map(carrierNum=>{
    const info=tsCarrierInfo[String(carrierNum)]||{carrier_num:carrierNum};
    return `<div class="ts-carrier-group" data-carrier="${carrierNum}">
      <div class="ts-carrier-head">
        <div class="ts-carrier-title">Carrier #${carrierNum}${carrierNum===state.mainCarrierNum?' | Main':''}</div>
        <div class="ts-carrier-meta">${tsCarrierMeta(info)}</div>
      </div>
      <div class="ts-row">${[1,2,3,4].map(ts=>tsCarrierBlockHtml(carrierNum,ts)).join('')}</div>
    </div>`;
  }).join('');
  updateTsBlocksCarrier();
}
function tsRandWaveCarrier(carrierNum,ts){
  tsWaveHeightsCarrier[tsCarrierKey(carrierNum,ts)]=Array.from({length:7},()=>Math.floor(Math.random()*14)+4);
}
function tsApplyWaveCarrier(carrierNum,ts,active){
  const block=document.getElementById(`ts-block-${carrierNum}-${ts}`);
  if(!block)return;
  const bars=block.querySelectorAll('.ts-wave-bar');
  if(active){
    const heights=tsWaveHeightsCarrier[tsCarrierKey(carrierNum,ts)]||[];
    heights.forEach((h,i)=>{if(bars[i])bars[i].style.height=h+'px';});
  }else{
    bars.forEach(b=>b.style.height='3px');
  }
}
function formatDurCarrier(s){
  if(s<60)return s+'s';
  return Math.floor(s/60)+'m'+String(s%60).padStart(2,'0')+'s';
}
function tsIssiTextCarrier(issi){
  if(!issi)return '';
  const c=callsigns[issi];
  if(!c||!c.cs)return ''+issi;
  const fl=c.fl?c.fl+' ':'';
  return issi+' | '+fl+c.cs;
}
function privatePartyRole(call,issi){
  if(!call||issi==null)return '';
  if(issi===call.caller_issi)return 'CALLER';
  if(issi===call.called_issi)return 'CALLED';
  return 'TALKER';
}
function privateSlotRef(carrierNum,ts){
  return carrierNum!=null&&ts!=null?('C'+carrierNum+'/TS'+ts):'-';
}
function privateAllocText(call){
  if(!call||call.call_type!=='individual')return '';
  const main=privateSlotRef(call.carrier_num,call.ts);
  const peerCarrier=call.peer_carrier_num!=null?call.peer_carrier_num:call.carrier_num;
  const peerTs=call.peer_ts!=null?call.peer_ts:call.ts;
  const hasPeer=call.peer_carrier_num!=null||call.peer_ts!=null;
  if(call.simplex||!hasPeer)return 'Shared '+main+' UL/DL';
  return 'Caller '+main+' UL/DL | Called '+privateSlotRef(peerCarrier,peerTs)+' UL/DL';
}
function tsLinesCarrier(st){
  const speaker=st.speaker_issi;
  if(st.call_type==='group')return {top:st.gssi!=null?('GSSI '+st.gssi):'GROUP',bottom:tsIssiTextCarrier(speaker||st.caller_issi)};
  const slotRole=st.private_slot_role||'shared';
  const top=`${st.caller_issi||'?'} <-> ${st.called_issi||'?'}`;
  const shownIssi=speaker||(slotRole==='called'?st.called_issi:st.caller_issi);
  const shownRole=privatePartyRole(st,shownIssi);
  const who=tsIssiTextCarrier(shownIssi);
  const slotText=slotRole==='caller'?'CALLER SLOT':(slotRole==='called'?'CALLED SLOT':'SHARED SLOT');
  const talkerText=who?(`TX ${shownRole} ${who}`):'';
  return {top,bottom:[slotText,talkerText].filter(Boolean).join(' | ')};
}
function updateTsBlocksCarrier(){
  const now=Date.now();
  const carriers=tsCarrierNumbers().length?tsCarrierNumbers():(state.mainCarrierNum!=null?[state.mainCarrierNum]:[]);
  for(const carrierNum of carriers){
    for(let ts=1;ts<=4;ts++){
      const block=document.getElementById(`ts-block-${carrierNum}-${ts}`);
      if(!block)continue;
      const label=block.querySelector('.ts-label');
      const sub=block.querySelector('.ts-sub');
      const dur=block.querySelector('.ts-duration-bar');
      const timer=block.querySelector('.ts-timer');
      const st=tsStateCarrier[tsCarrierKey(carrierNum,ts)];
      if(ts===1&&carrierNum===state.mainCarrierNum){
        block.className='ts-block mcch';
        label.textContent='MCCH';
        sub.textContent='ACTIVE';
        if(!tsWaveHeightsCarrier[tsCarrierKey(carrierNum,ts)])tsRandWaveCarrier(carrierNum,ts);
        tsApplyWaveCarrier(carrierNum,ts,true);
        if(dur)dur.style.width='0%';
        continue;
      }
      if(!st){
        block.className='ts-block';
        label.textContent=ts===1?'BCCH':'-';
        sub.textContent=ts===1?'SECONDARY':'Idle';
        tsApplyWaveCarrier(carrierNum,ts,false);
        if(timer)timer.textContent='';
        if(dur)dur.style.width='0%';
        continue;
      }
      const voiceRecent=st.voice_ts&&(now-st.voice_ts)<TS_VOICE_DECAY_MS;
      const lines=tsLinesCarrier(st);
      label.textContent=lines.top;
      if(voiceRecent){
        block.className='ts-block voice';
        sub.textContent=lines.bottom?('â–¶ '+lines.bottom):'â–¶ TX';
      }else{
        block.className='ts-block call';
        sub.textContent=lines.bottom||(st.sub||'Alloc');
      }
      if((st.priority||0)>=15)block.classList.add('emergency');
      if(timer){
        const elapsed=Math.floor((now-(st.started_at||now))/1000);
        timer.textContent=elapsed>0?formatDurCarrier(elapsed):'';
      }
      tsApplyWaveCarrier(carrierNum,ts,voiceRecent);
      if(dur&&st.started_at){
        const pct=Math.min(100,((now-st.started_at)/120000)*100);
        dur.style.width=pct+'%';
      }
    }
  }
}
function tsSetCallCarrier(carrierNum,ts,call){
  if(!tsCanRenderAssignedCarrier(carrierNum,ts))return;
  tsEnsureCarrierInfo(carrierNum);
  if(!document.getElementById(`ts-block-${carrierNum}-${ts}`))renderTsGridCarrier();
  const peerCarrier=call.peer_carrier_num!=null?call.peer_carrier_num:call.carrier_num;
  const peerTs=call.peer_ts!=null?call.peer_ts:call.ts;
  const hasPeer=call.peer_carrier_num!=null||call.peer_ts!=null;
  const privateSlotRole=call.call_type!=='individual'
    ? null
    : ((call.simplex||!hasPeer)
      ? 'shared'
      : ((carrierNum===call.carrier_num&&ts===call.ts)?'caller':((carrierNum===peerCarrier&&ts===peerTs)?'called':'shared')));
  tsStateCarrier[tsCarrierKey(carrierNum,ts)]={
    call_id:call.call_id,call_type:call.call_type,
    gssi:call.gssi,called_issi:call.called_issi,caller_issi:call.caller_issi,
    speaker_issi:call.call_type==='individual'?(call.active_speaker||call.speaker_issi||null):(call.active_speaker||call.speaker_issi||call.caller_issi),
    simplex:call.simplex,sub:call.sub,priority:call.priority||0,
    voice_ts:null,started_at:Date.now(),carrier_num:carrierNum,ts:ts,private_slot_role:privateSlotRole
  };
}
function tsSetSpeakerCarrier(callId,speakerIssi){
  Object.keys(tsStateCarrier).forEach(key=>{if(tsStateCarrier[key]&&tsStateCarrier[key].call_id===callId)tsStateCarrier[key].speaker_issi=speakerIssi;});
}
function tsClearCallCarrier(callId){
  Object.keys(tsStateCarrier).forEach(key=>{if(tsStateCarrier[key]&&tsStateCarrier[key].call_id===callId)delete tsStateCarrier[key];});
}
function tsVoiceCarrier(carrierNum,ts,speakerIssi){
  if(!tsCanRenderAssignedCarrier(carrierNum,ts))return;
  tsEnsureCarrierInfo(carrierNum);
  if(!document.getElementById(`ts-block-${carrierNum}-${ts}`))renderTsGridCarrier();
  const key=tsCarrierKey(carrierNum,ts);
  // Voice bursts should animate an existing allocation, not create a synthetic
  // "PRIVATE" slot that outlives the call if a late frame arrives after call_ended.
  if(!tsStateCarrier[key])return;
  tsStateCarrier[key].voice_ts=Date.now();
  if(speakerIssi)tsStateCarrier[key].speaker_issi=speakerIssi;
  tsRandWaveCarrier(carrierNum,ts);
  const block=document.getElementById(`ts-block-${carrierNum}-${ts}`);
  if(block){
    const flash=block.querySelector('.ts-flash');
    if(flash){flash.style.animation='none';void flash.offsetWidth;flash.style.animation='ts-flash-in 0.08s ease-out forwards';}
  }
  updateTsBlocksCarrier();
}
setInterval(updateTsBlocksCarrier, 150);

function tsCarrierBlockHtml(carrierNum,ts){
  const idleHeights=(carrierNum===state.mainCarrierNum&&ts===1)?[8,14,10,16,8,12,6]:[3,3,3,3,3,3,3];
  const label=(carrierNum===state.mainCarrierNum&&ts===1)?'MCCH':(ts===1?'BCCH':'-');
  const sub=(carrierNum===state.mainCarrierNum&&ts===1)?'ACTIVE':(ts===1?'SECONDARY':'Idle');
  return `<div class="ts-block${carrierNum===state.mainCarrierNum&&ts===1?' mcch':''}" id="ts-block-${carrierNum}-${ts}">
    <div class="ts-num">TS ${ts}</div>
    ${ts===1?'':'<div class="ts-timer"></div>'}
    <div class="ts-led"></div>
    <div class="ts-wave">${idleHeights.map(h=>`<div class="ts-wave-bar" style="height:${h}px"></div>`).join('')}</div>
    <div class="ts-label">${label}</div>
    <div class="ts-sub">${sub}</div>
    <div class="ts-flash"></div>
    <div class="ts-duration-bar"></div>
  </div>`;
}

function updateTsBlocksCarrier(){
  const now=Date.now();
  const carriers=tsCarrierNumbers().length?tsCarrierNumbers():(state.mainCarrierNum!=null?[state.mainCarrierNum]:[]);
  for(const carrierNum of carriers){
    for(let ts=1;ts<=4;ts++){
      const block=document.getElementById(`ts-block-${carrierNum}-${ts}`);
      if(!block)continue;
      const label=block.querySelector('.ts-label');
      const sub=block.querySelector('.ts-sub');
      const dur=block.querySelector('.ts-duration-bar');
      const timer=block.querySelector('.ts-timer');
      const st=tsStateCarrier[tsCarrierKey(carrierNum,ts)];

      if(ts===1&&carrierNum===state.mainCarrierNum){
        block.className='ts-block mcch';
        label.textContent='MCCH';
        sub.textContent='ACTIVE';
        if(!tsWaveHeightsCarrier[tsCarrierKey(carrierNum,ts)])tsRandWaveCarrier(carrierNum,ts);
        tsApplyWaveCarrier(carrierNum,ts,true);
        if(dur)dur.style.width='0%';
        continue;
      }

      if(!st){
        block.className='ts-block';
        label.textContent=ts===1?'BCCH':'-';
        sub.textContent=ts===1?'SECONDARY':'Idle';
        tsApplyWaveCarrier(carrierNum,ts,false);
        if(timer)timer.textContent='';
        if(dur)dur.style.width='0%';
        continue;
      }

      const voiceRecent=st.voice_ts&&(now-st.voice_ts)<TS_VOICE_DECAY_MS;
      const lines=tsLinesCarrier(st);
      label.textContent=lines.top;
      if(voiceRecent){
        block.className='ts-block voice';
        sub.textContent=lines.bottom?('TX '+lines.bottom):'TX';
      }else{
        block.className='ts-block call';
        sub.textContent=lines.bottom||(st.sub||'Alloc');
      }
      if((st.priority||0)>=15)block.classList.add('emergency');
      if(timer){
        const elapsed=Math.floor((now-(st.started_at||now))/1000);
        timer.textContent=elapsed>0?formatDurCarrier(elapsed):'';
      }
      tsApplyWaveCarrier(carrierNum,ts,voiceRecent);
      if(dur&&st.started_at){
        const pct=Math.min(100,((now-st.started_at)/120000)*100);
        dur.style.width=pct+'%';
      }
    }
  }
}

function tsSetSpeakerCarrier(callId,carrierNum,ts,speakerIssi){
  if(carrierNum!=null&&ts!=null){
    const key=tsCarrierKey(carrierNum,ts);
    if(tsStateCarrier[key]&&tsStateCarrier[key].call_id===callId){
      tsStateCarrier[key].speaker_issi=speakerIssi;
      return;
    }
  }
  Object.keys(tsStateCarrier).forEach(key=>{if(tsStateCarrier[key]&&tsStateCarrier[key].call_id===callId)tsStateCarrier[key].speaker_issi=speakerIssi;});
}

function renderStations(){
  const ms=Object.values(state.ms);
  const msCount=ms.length,callCount=Object.keys(state.calls).length;
  document.getElementById('stat-ms').textContent=msCount;
  document.getElementById('stat-calls').textContent=callCount;
  document.getElementById('badge-ms').textContent=msCount;
  const bc=document.getElementById('badge-calls');
  if(bc){bc.textContent=callCount;bc.style.display=callCount?'flex':'none';}
  // Hero summary
  const hd=document.getElementById('stations-hero-dot');
  const ht=document.getElementById('stations-hero-title');
  const hs=document.getElementById('stations-hero-sub');
  const hc=document.getElementById('stations-hero-calls');
  if(hd){hd.className='hero-dot '+(msCount?'is-ok':'is-idle');}
  if(ht)ht.textContent=msCount+' '+t('terminals');
  if(hs)hs.textContent=msCount?t('registered'):t('no_terminals');
  if(hc)hc.textContent=callCount;
  const tb=document.getElementById('ms-tbody');
  if(!ms.length){tb.innerHTML=`<tr><td colspan="7"><div class="empty-state"><span class="empty-ico">${svgIcon('radios')}</span><div class="empty-msg">${t('no_terminals')}</div></div></td></tr>`;return;}
  tb.innerHTML=ms.sort((a,b)=>a.issi-b.issi).map(m=>{
    const r=m.rssi_dbfs,rL=r!=null?`${r.toFixed(1)} dBFS`:'—',pct=rssiPct(r),gcls=rssiGaugeClass(r);
    let grps;
    const gl=m.groups||[],sel=m.selected_group;
    // The selected/active TG (the one the MS last keyed up on) is rendered as a solid blue
    // badge with a ▶ marker; the merely scanned/affiliated TGs are dim. Until the MS is heard
    // on a call sel is null — so right after a restart all groups show dim (scanned), without
    // implying the station is actively on any of them.
    const gBadge=g=>g===sel
      ?`<span class="badge badge-blue" style="font-weight:700;font-size:9px" title="${t('tg_selected')}"><span class="tg-marker">${ICON_MARKER}</span>${g}</span>`
      :`<span class="badge badge-dim" style="font-size:9px">${g}</span>`;
    if(gl.length>1){
      const gList=gl.slice().sort((a,b)=>(b===sel)-(a===sel)||a-b).map(gBadge).join(' ');
      // Always show a neutral "+N affiliated" badge — never "⚡ SCAN" (FH-BUG-032). On the BS
      // side we have NO signal that the radio is actively scanning; we only have the static set
      // of affiliated groups, which the radio keeps re-attaching with lifetime=0 even after scan
      // is turned off on the device (intentional — see FH-BUG-022). "⚡ SCAN" was read by
      // operators as a live "this radio is scanning" claim, which we cannot back up. "+N
      // affiliated" is honest: these N groups are affiliated alongside the selected one (if any).
      // With a selected TG, the selected one is marked ▶ and N excludes it; with none selected
      // yet (e.g. before the first PTT), N counts them all.
      const others=sel!=null?gl.filter(g=>g!==sel).length:gl.length;
      const extraBadge=`<span class="badge badge-dim" style="font-size:9px;margin-right:4px" title="${t('tg_affiliated_hint')}">+${others} ${t('tg_affiliated_short')}</span>`;
      grps=`${extraBadge}${gList}`;
    } else if(gl.length===1){
      grps=`<span class="badge badge-blue">${gl[0]}</span>`;
    } else {
      grps='<span class="badge badge-dim">—</span>';
    }
    const ls=m._last_seen_ts?Math.floor((Date.now()-m._last_seen_ts)/1000):m.last_seen_secs_ago;
    const emg=!!state.emergencies[m.issi];
    return`<tr${emg?' class="row-emergency"':''}>
      <td>${emg?'<span class="badge badge-emergency">'+t('call_emergency')+'</span> ':''}${idCell(m.issi)}</td><td>${grps}</td>
      <td class="col-mobile-hide">${eeLabel(m.energy_saving_mode||0)}</td>
      <td><div class="gauge ${gcls}"><div class="gauge-track"><div class="gauge-fill" style="width:${pct}%"></div></div><span class="gauge-value">${rL}</span></div></td>
      <td><span class="pill pill-ok">${t('online_badge')}</span></td>
      <td class="col-mobile-hide">${lastSeenLabel(ls)}</td>
      <td><button class="btn btn-sm" onclick="openSds(${m.issi})">${t('sds')}</button> <button class="btn btn-sm" onclick="openDgna(${m.issi})" title="${t('dgna_title')}">${t('dgna')}</button> <button class="btn btn-sm btn-danger" onclick="kickMs(${m.issi})">${t('kick')}</button>${emg?` <button class="btn btn-sm btn-danger" onclick="clearEmergency(${m.issi})">${t('emg_clear')}</button>`:''}</td>
    </tr>`;
  }).join('');
}

function renderCalls(){
  document.getElementById('stat-calls').textContent=Object.keys(state.calls).length;
  const tb=document.getElementById('calls-tbody'),calls=Object.values(state.calls);
  if(!calls.length){tb.innerHTML=`<tr><td colspan="6"><div class="empty-state"><span class="empty-ico">${svgIcon('calls')}</span><div class="empty-msg">${t('no_calls')}</div></div></td></tr>`;return;}
  tb.innerHTML=calls.map(c=>{
    const dur=Math.floor((Date.now()-(c.started_at||Date.now()))/1000);
    const mm=String(Math.floor(dur/60)).padStart(2,'0'),ss=String(dur%60).padStart(2,'0');
    const pillv=c.call_type==='group'?'pill-info':'pill-warn';
    const label=c.call_type==='group'?t('call_group'):(c.simplex?t('call_p2p_s'):t('call_p2p_d'));
    const allocMeta=c.call_type==='individual'
      ? `<div style="margin-top:4px;font-family:var(--mono);font-size:10px;color:var(--text2)">${escHtml(privateAllocText(c))}</div>`
      : '';
    const to=c.call_type==='group'?`GSSI ${c.gssi}`:`${idCell(c.called_issi)}${allocMeta}`;
    const spk=c.active_speaker
      ? `${idCell(c.active_speaker)}${c.call_type==='individual'?` <span class="badge badge-dim" style="font-size:9px">${privatePartyRole(c,c.active_speaker)}</span>`:''}`
      : '<span style="color:var(--text3)">—</span>';
    // Emergency call = ETSI call priority 15 (terminal emergency button). Flag it prominently.
    const emg=(c.priority||0)>=15;
    const emgBadge=emg?`<span class="pill pill-danger"><span class="pill-icon">${svgIcon('emergency')}</span>${t('call_emergency')}</span> `:'';
    return`<tr${emg?' class="row-emergency"':''}><td class="col-mobile-hide"><code>${c.call_id}</code></td><td>${emgBadge}<span class="pill ${pillv}">${label}</span></td><td>${c.caller_issi?idCell(c.caller_issi):'<span class="muted">—</span>'}</td><td>${to}</td><td>${spk}</td><td><span class="num accent">${mm}:${ss}</span></td></tr>`;
  }).join('');
}

function renderLastHeard(){
  const tb=document.getElementById('lastheard-tbody');
  if(!tb)return;
  if(!state.lastHeard.length){tb.innerHTML=`<tr><td colspan="4"><div class="empty-state"><span class="empty-ico">${svgIcon('lastheard')}</span><div class="empty-msg">${t('no_activity')}</div></div></td></tr>`;return;}
  tb.innerHTML=state.lastHeard.map(e=>{
    const destStr=e.dest?`<code>${e.dest}</code>`:'<span class="muted">—</span>';
    const isOnline=!!state.ms[e.issi];
    const issiHtml=`${idCell(e.issi)}${isOnline?` <span class="pill pill-ok">${t('online_badge')}</span>`:''}`;
    return`<tr>
      <td><span class="num">${e.ts}</span></td>
      <td>${issiHtml}</td><td>${activityBadge(e.activity)}</td><td>${destStr}</td>
    </tr>`;
  }).join('');
}
function clearLastHeard(){state.lastHeard=[];renderLastHeard();}

// ── SDS Log ───────────────────────────────────────────────────────────────
function _p2(n){return String(n).padStart(2,'0');}
// Local "YYYY-MM-DD HH:MM:SS" stamp matching the server's persisted format. Used only for
// live rows arriving over the WS; rows fetched from /api/sds-log already carry a server stamp.
function nowStamp(){const d=new Date();return `${d.getFullYear()}-${_p2(d.getMonth()+1)}-${_p2(d.getDate())} ${_p2(d.getHours())}:${_p2(d.getMinutes())}:${_p2(d.getSeconds())}`;}
const LOG_PAGE_SIZE=50;
let sdsLogPageIndex=0,dapnetLogPageIndex=0,geoalarmPageIndex=0,meshNodePageIndex=0,meshMsgPageIndex=0;
function setLogPager(id,page,total){
  const el=document.getElementById(id);if(!el)return;
  if(!total){el.textContent='Page 0 / 0 · 0';return;}
  const pages=Math.max(1,Math.ceil(total/LOG_PAGE_SIZE));
  el.textContent=`Page ${page+1} / ${pages} · ${total}`;
}
function clampLogPage(page,total){
  const pages=Math.max(1,Math.ceil(total/LOG_PAGE_SIZE));
  return Math.max(0,Math.min(page,pages-1));
}
function logExportStamp(){
  const d=new Date();
  return `${d.getFullYear()}${_p2(d.getMonth()+1)}${_p2(d.getDate())}-${_p2(d.getHours())}${_p2(d.getMinutes())}${_p2(d.getSeconds())}`;
}
function logExportCell(v){
  return String(v??'').replace(/\r?\n/g,' ').replace(/\t/g,' ').trim();
}
function downloadTextFile(filename,text){
  const blob=new Blob([text],{type:'text/plain;charset=utf-8'});
  const a=document.createElement('a');
  a.href=URL.createObjectURL(blob);
  a.download=filename;
  document.body.appendChild(a);a.click();
  setTimeout(()=>{URL.revokeObjectURL(a.href);a.remove();},0);
}
// Human label for known SDS protocol-identifier bytes so binary payloads (no decoded text)
// still read meaningfully. 0x02/0x09/0x82/0x89 = text; 0x0A = LIP position; 0xDC = Home Mode Display.
function pidLabel(pid){const m={2:'text',9:'text',10:'LIP position',12:'concat',128:'text',130:'text',137:'text',218:'status',220:'home-display'};return m[pid]||('PID '+pid);}
const SDS_DIR={rx:['pill-ok','RX'],net:['pill-info','NET'],tx:['pill-warn','TX']};
function dirBadge(dir){const x=SDS_DIR[dir]||['pill-idle',(dir||'?').toUpperCase()];return `<span class="pill ${x[0]}">${x[1]}</span>`;}
function lipPositionFromText(text){
  const m=String(text||'').match(/^LIP position:\s*(-?\d+(?:\.\d+)?),\s*(-?\d+(?:\.\d+)?)/);
  if(!m)return null;
  const lat=Number(m[1]),lon=Number(m[2]);
  if(!Number.isFinite(lat)||!Number.isFinite(lon)||lat<-90||lat>90||lon<-180||lon>180)return null;
  return {lat,lon};
}
function sdsMessageBody(e){
  if(e.text&&e.text.length){
    const lip=lipPositionFromText(e.text);
    if(lip){
      const label=`LIP position: ${lip.lat.toFixed(6)}, ${lip.lon.toFixed(6)}`;
      const url=`https://www.google.com/maps/search/?api=1&query=${encodeURIComponent(`${lip.lat.toFixed(6)},${lip.lon.toFixed(6)}`)}`;
      return `<a class="sds-map-link" href="${url}" target="_blank" rel="noopener noreferrer">${escHtml(label)}</a>`;
    }
    return escHtml(e.text);
  }
  return `<span class="sds-empty">[${escHtml(pidLabel(e.protocol_id))}]</span>`;
}
function sdsRow(e){
  const to=e.is_group?`<code>${e.dest_issi}</code> <span class="sds-empty">grp</span>`:idCell(e.dest_issi);
  const body=sdsMessageBody(e);
  return `<tr><td class="sds-time num">${escHtml(e.ts||'')}</td><td>${dirBadge(e.direction)}</td><td>${idCell(e.source_issi)}</td><td>${to}</td><td class="sds-msg">${body}</td></tr>`;
}
function renderSdsLog(){
  const tb=document.getElementById('sdslog-tbody');if(!tb)return;
  const rows=state.sdsLog||[];
  sdsLogPageIndex=clampLogPage(sdsLogPageIndex,rows.length);
  setLogPager('sdslog-page',sdsLogPageIndex,rows.length);
  if(!rows.length){tb.innerHTML=`<tr><td colspan="5" class="sds-empty" style="text-align:center;padding:24px">${t('no_sds')}</td></tr>`;return;}
  const start=sdsLogPageIndex*LOG_PAGE_SIZE;
  tb.innerHTML=rows.slice(start,start+LOG_PAGE_SIZE).map(sdsRow).join('');
}
async function loadSdsLog(){
  try{const r=await fetch('/api/sds-log');if(!r.ok)return;state.sdsLog=await r.json();sdsLogPageIndex=0;renderSdsLog();refreshCallsigns();}catch{}
}
function sdsLogPrevPage(){sdsLogPageIndex--;renderSdsLog();}
function sdsLogNextPage(){sdsLogPageIndex++;renderSdsLog();}
async function clearSdsLog(){
  if(!confirm('Clear SDS Log?'))return;
  try{const r=await fetch('/api/sds-log',{method:'DELETE'});if(!r.ok)return;state.sdsLog=[];sdsLogPageIndex=0;renderSdsLog();}catch{}
}
function exportSdsLog(){
  const rows=state.sdsLog||[];
  if(!rows.length)return;
  const lines=['TIME\tDIR\tFROM\tTO\tGROUP\tPID\tMESSAGE'];
  for(const e of rows){
    lines.push([
      e.ts||'',
      (e.direction||'').toUpperCase(),
      e.source_issi||'',
      e.dest_issi||'',
      e.is_group?'yes':'no',
      e.protocol_id??'',
      logExportCell(e.text||pidLabel(e.protocol_id))
    ].map(logExportCell).join('\t'));
  }
  downloadTextFile(`flowstation-sds-log-${logExportStamp()}.txt`,lines.join('\n')+'\n');
}

// ── DAPNET ────────────────────────────────────────────────────────────────
let dapPasswordDirty=false,dapAuthDirty=false;
function dapSet(id,v){
  const el=document.getElementById(id);if(!el)return;
  const value=(v===null||v===undefined)?'':v;
  if('value' in el)el.value=value;
  else el.textContent=value;
}
function dapCheck(id,v){const el=document.getElementById(id);if(el)el.checked=!!v;}
function dapVal(id){const el=document.getElementById(id);return el?(el.value||'').trim():'';}
function dapNum(id,def,min,max){
  const n=parseInt(dapVal(id),10);
  if(!Number.isFinite(n))return def;
  return Math.max(min,Math.min(max,n));
}
function dapList(id){return dapVal(id).split(/[\s,]+/).map(s=>s.trim()).filter(Boolean);}
function dapRicRoutesText(routes){
  return Object.keys(routes||{}).sort().map(k=>`${k}=${routes[k]}`).join('\n');
}
function dapRicRoutesBody(id,label){
  const raw=dapVal(id);
  const out={};
  if(!raw)return out;
  for(const lineRaw of raw.split(/\n+/)){
    const line=lineRaw.trim();
    if(!line||line.startsWith('#'))continue;
    const m=line.match(/^([0-9A-Fa-fxX]+)\s*=\s*([0-9]+)$/);
    if(!m){setDapMsg(`Invalid ${label} route: ${line}`,false);return null;}
    const issi=parseInt(m[2],10);
    if(!Number.isFinite(issi)||issi<1||issi>16777215){setDapMsg(`Invalid SSI in ${label} route: ${line}`,false);return null;}
    out[m[1]]=issi;
  }
  return out;
}
function dapRicListText(rics){
  if(!rics)return '';
  if(Array.isArray(rics))return rics.join('\n');
  return Object.keys(rics).sort().join('\n');
}
function dapRicListBody(id,label){
  const raw=dapVal(id);
  const out=[];
  if(!raw)return out;
  const seen=new Set();
  for(const lineRaw of raw.split(/\n+/)){
    const line=lineRaw.split('#')[0].trim();
    if(!line)continue;
    for(const partRaw of line.split(/[\s,]+/)){
      const part=partRaw.trim();
      if(!part)continue;
      if(!/^(?:0x[0-9a-f]+|[0-9]+)$/i.test(part)){setDapMsg(`Invalid ${label} RIC: ${part}`,false);return null;}
      if(!seen.has(part)){seen.add(part);out.push(part);}
    }
  }
  return out;
}
function dapPaths(paths){
  const p=paths||[];
  if(!p.length)return '<span class="sds-empty">—</span>';
  return p.map(x=>`<span class="badge badge-blue" style="font-size:10px">${escHtml(x)}</span>`).join(' ');
}
function dapnetRow(e){
  return `<tr><td class="sds-time">${escHtml(e.ts||'')}</td><td>${dirBadge(e.direction)}</td><td>${escHtml(e.callsign||'')}</td><td>${escHtml(e.recipient||'')}</td><td>${dapPaths(e.paths)}</td><td class="sds-msg">${escHtml(e.text||'')}</td></tr>`;
}
function renderDapnetLog(){
  const tb=document.getElementById('dapnetlog-tbody');if(!tb)return;
  const rows=state.dapnetLog||[];
  dapnetLogPageIndex=clampLogPage(dapnetLogPageIndex,rows.length);
  setLogPager('dapnetlog-page',dapnetLogPageIndex,rows.length);
  if(!rows.length){tb.innerHTML=`<tr><td colspan="6" class="sds-empty" style="text-align:center;padding:24px">No DAPNET messages yet</td></tr>`;return;}
  const start=dapnetLogPageIndex*LOG_PAGE_SIZE;
  tb.innerHTML=rows.slice(start,start+LOG_PAGE_SIZE).map(dapnetRow).join('');
}
async function loadDapnetLog(){
  try{const r=await fetch('/api/dapnet-log');if(!r.ok)return;state.dapnetLog=await r.json();dapnetLogPageIndex=0;renderDapnetLog();}catch{}
}
function dapnetLogPrevPage(){dapnetLogPageIndex--;renderDapnetLog();}
function dapnetLogNextPage(){dapnetLogPageIndex++;renderDapnetLog();}
async function clearDapnetLog(){
  if(!confirm('Clear DAPNET Log?'))return;
  try{const r=await fetch('/api/dapnet-log',{method:'DELETE'});if(!r.ok)return;state.dapnetLog=[];dapnetLogPageIndex=0;renderDapnetLog();}catch{}
}
function exportDapnetLog(){
  const rows=state.dapnetLog||[];
  if(!rows.length)return;
  const lines=['TIME\tDIR\tCALLSIGN\tRECIPIENT\tPATHS\tMESSAGE'];
  for(const e of rows){
    lines.push([
      e.ts||'',
      (e.direction||'').toUpperCase(),
      e.callsign||'',
      e.recipient||'',
      (e.paths||[]).join(','),
      e.text||''
    ].map(logExportCell).join('\t'));
  }
  downloadTextFile(`flowstation-dapnet-log-${logExportStamp()}.txt`,lines.join('\n')+'\n');
}
async function loadDapnet(){
  try{
    const r=await fetch('/api/dapnet');
    if(!r.ok){setDapMsg(t('conn_error'),false);return;}
    const d=await r.json();
    dapCheck('dap-enabled',d.enabled);
    dapCheck('dap-rwth-enabled',d.rwth_core_enabled);
    dapSet('dap-poll',d.poll_interval_secs||30);
    dapSet('dap-limit',d.rwth_messages_limit||100);
    dapSet('dap-api-url',d.api_url||'');
    dapSet('dap-username',d.username||'');
    dapSet('dap-password',d.password_set?(d.password_masked||''):'');
    dapPasswordDirty=false;
    dapSet('dap-rwth-host',d.rwth_core_host||'');
    dapSet('dap-rwth-port',d.rwth_core_port||43434);
    dapSet('dap-rwth-device',d.rwth_core_device||'FlowStation');
    dapSet('dap-rwth-version',d.rwth_core_version||'1.0');
    dapSet('dap-rwth-callsign',d.rwth_core_callsign||'');
    dapSet('dap-rwth-authkey',d.rwth_core_authkey_set?(d.rwth_core_authkey_masked||''):'');
    dapAuthDirty=false;
    dapCheck('dap-forward-sds',d.forward_sds);
    dapCheck('dap-forward-callout',d.forward_callout);
    dapCheck('dap-forward-telegram',d.forward_telegram);
    dapSet('dap-sds-source',d.sds_source_issi||9999);
    dapSet('dap-sds-dest',d.sds_dest_issi||0);
    dapCheck('dap-sds-group',d.sds_dest_is_group);
    dapSet('dap-ric-routes',dapRicRoutesText(d.ric_issi_routes));
    dapSet('dap-ric-group-routes',dapRicRoutesText(d.ric_gssi_routes));
    dapSet('dap-sds-rics',dapRicListText(d.sds_allowed_rics));
    dapSet('dap-callout-source',d.callout_source_issi||9999);
    dapSet('dap-callout-dest',d.callout_dest_issi||0);
    dapSet('dap-callout-incident',d.callout_incident_base||2);
    dapSet('dap-callout-prefix',d.callout_text_prefix||'DAPNET');
    dapSet('dap-callout-rics',dapRicListText(d.callout_allowed_rics));
    dapSet('dap-telegram-prefix',d.telegram_prefix||'DAPNET');
    dapSet('dap-telegram-rics',dapRicListText(d.telegram_allowed_rics));
    // Hero pill — DAPNET has no live link probe; reflect the enabled feed state.
    setIntegrationHero('dap', !!d.enabled, !!d.enabled,
      d.enabled?t('integ_enabled'):t('integ_disabled'),
      d.api_url||d.rwth_core_host||'');
    setDapMsg('',true);
  }catch{setDapMsg(t('conn_error'),false);setIntegrationHero('dap',false,false,t('conn_error'),'');}
}
async function saveDapnet(){
  const ricRoutes=dapRicRoutesBody('dap-ric-routes','RIC to ISSI');
  if(ricRoutes===null)return;
  const ricGroupRoutes=dapRicRoutesBody('dap-ric-group-routes','RIC to GSSI');
  if(ricGroupRoutes===null)return;
  const sdsRics=dapRicListBody('dap-sds-rics','SDS');
  if(sdsRics===null)return;
  const calloutRics=dapRicListBody('dap-callout-rics','Call-Out');
  if(calloutRics===null)return;
  const telegramRics=dapRicListBody('dap-telegram-rics','Telegram');
  if(telegramRics===null)return;
  const body={
    enabled:document.getElementById('dap-enabled').checked,
    rwth_core_enabled:document.getElementById('dap-rwth-enabled').checked,
    poll_interval_secs:dapNum('dap-poll',30,1,86400),
    rwth_messages_limit:dapNum('dap-limit',100,1,10000),
    api_url:dapVal('dap-api-url'),
    username:dapVal('dap-username'),
    rwth_core_host:dapVal('dap-rwth-host'),
    rwth_core_port:dapNum('dap-rwth-port',43434,1,65535),
    rwth_core_device:dapVal('dap-rwth-device')||'FlowStation',
    rwth_core_version:dapVal('dap-rwth-version')||'1.0',
    rwth_core_callsign:dapVal('dap-rwth-callsign').toUpperCase(),
    forward_sds:document.getElementById('dap-forward-sds').checked,
    forward_callout:document.getElementById('dap-forward-callout').checked,
    forward_telegram:document.getElementById('dap-forward-telegram').checked,
    sds_source_issi:dapNum('dap-sds-source',9999,1,16777215),
    sds_dest_issi:dapNum('dap-sds-dest',0,0,16777215),
    sds_dest_is_group:document.getElementById('dap-sds-group').checked,
    ric_issi_routes:ricRoutes,
    ric_gssi_routes:ricGroupRoutes,
    sds_allowed_rics:sdsRics,
    callout_source_issi:dapNum('dap-callout-source',9999,1,16777215),
    callout_dest_issi:dapNum('dap-callout-dest',0,0,16777215),
    callout_incident_base:dapNum('dap-callout-incident',2,1,256),
    callout_text_prefix:dapVal('dap-callout-prefix')||'DAPNET',
    callout_allowed_rics:calloutRics,
    telegram_prefix:dapVal('dap-telegram-prefix')||'DAPNET',
    telegram_allowed_rics:telegramRics
  };
  if(dapPasswordDirty)body.password=dapVal('dap-password');
  if(dapAuthDirty)body.rwth_core_authkey=dapVal('dap-rwth-authkey');
  try{
    const r=await fetch('/api/dapnet',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
    if(r.ok){setDapMsg(t('dapnet_saved'),true);loadDapnet();}
    else setDapMsg(t('save_fail')+': '+await r.text(),false);
  }catch{setDapMsg(t('conn_error'),false);}
}
async function sendDapnetMessage(){
  const body={
    callSignNames:dapList('dap-out-callsigns'),
    transmitterGroupNames:dapList('dap-out-groups'),
    emergency:document.getElementById('dap-out-emergency').checked,
    text:document.getElementById('dap-out-text').value.trim()
  };
  if(!body.text){setDapSendMsg('Message text is empty',false);return;}
  if(!body.callSignNames.length&&!body.transmitterGroupNames.length){setDapSendMsg('Set callsign or transmitter group',false);return;}
  setDapSendMsg('Sending…',true);
  try{
    const r=await fetch('/api/dapnet/send',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
    const d=await r.json();
    if(d.ok){setDapSendMsg('✓ Sent',true);document.getElementById('dap-out-text').value='';loadDapnetLog();}
    else setDapSendMsg('✗ '+(d.error||'Send failed'),false);
  }catch{setDapSendMsg(t('conn_error'),false);}
}

// ── Shared map-link + paths helpers (also used by GeoAlarm) ────────────────
function meshMapLink(lat,lon,label){
  if(lat===null||lat===undefined||lon===null||lon===undefined)return '—';
  const la=Number(lat),lo=Number(lon);
  if(!Number.isFinite(la)||!Number.isFinite(lo))return '—';
  const url=`https://maps.google.com/?q=${encodeURIComponent(la+','+lo)}`;
  return `<a class="sds-map-link" href="${url}" target="_blank" rel="noopener noreferrer">${escHtml(label||`${la.toFixed(5)}, ${lo.toFixed(5)}`)}</a>`;
}
function meshRfText(row){
  const parts=[];
  if(row.rssi!==null&&row.rssi!==undefined)parts.push(`RSSI ${row.rssi}`);
  if(row.snr!==null&&row.snr!==undefined)parts.push(`SNR ${row.snr}`);
  return parts.join(' · ')||'—';
}
function meshSourceListText(values){
  return Array.isArray(values)?values.join('\n'):'';
}
function meshSourceListBody(id){
  const raw=dapVal(id);
  if(!raw)return [];
  return raw.split(/[\s,]+/).map(v=>v.trim()).filter(Boolean);
}
function meshPaths(paths){
  if(!Array.isArray(paths)||!paths.length)return '<span class="sds-empty">—</span>';
  return paths.map(p=>`<span class="badge badge-blue" style="font-size:10px">${escHtml(p)}</span>`).join(' ');
}

// ── GeoAlarm ──────────────────────────────────────────────────────────────
function geoFloat(id,def,min,max){
  const n=parseFloat(dapVal(id));
  if(!Number.isFinite(n))return def;
  return Math.max(min,Math.min(max,n));
}
function geoIssiListText(values){
  return Array.isArray(values)?values.join('\n'):'';
}
function geoIssiListBody(id,label){
  const raw=dapVal(id);
  if(!raw)return [];
  const out=[],seen=new Set();
  for(const part of raw.split(/[\s,]+/).map(v=>v.trim()).filter(Boolean)){
    const n=Number(part);
    if(!Number.isInteger(n)||n<0||n>16777215){setGeoMsg(`Invalid ${label} ISSI: ${part}`,false);return null;}
    if(!seen.has(n)){seen.add(n);out.push(n);}
  }
  return out;
}
function geoEventRow(e){
  const status=e.alarmed
    ? '<span class="badge badge-green" style="font-size:10px">ALARM</span>'
    : (e.inside_radius?'<span class="badge badge-blue" style="font-size:10px">inside</span>':'<span class="badge" style="font-size:10px">outside</span>');
  return `<tr>
    <td class="sds-time">${escHtml(e.ts||'')}</td>
    <td>${escHtml(e.source||'—')}</td>
    <td>${escHtml(e.device||'—')}</td>
    <td class="sds-time">${Number(e.distance_m||0).toFixed(0)} m</td>
    <td>${meshMapLink(e.lat,e.lon,'map')}</td>
    <td>${status}</td>
    <td>${meshPaths(e.paths)}</td>
  </tr>`;
}
function renderGeoalarmEvents(){
  const tb=document.getElementById('geo-events-tbody');if(!tb)return;
  const rows=state.geoalarmEvents||[];
  geoalarmPageIndex=clampLogPage(geoalarmPageIndex,rows.length);
  setLogPager('geo-events-page',geoalarmPageIndex,rows.length);
  if(!rows.length){tb.innerHTML=`<tr><td colspan="7" class="sds-empty" style="text-align:center;padding:24px">No GeoAlarm events yet</td></tr>`;return;}
  const start=geoalarmPageIndex*LOG_PAGE_SIZE;
  tb.innerHTML=rows.slice(start,start+LOG_PAGE_SIZE).map(geoEventRow).join('');
}
function geoPrevPage(){geoalarmPageIndex--;renderGeoalarmEvents();}
function geoNextPage(){geoalarmPageIndex++;renderGeoalarmEvents();}
async function loadGeoalarm(){
  try{
    const r=await fetch('/api/geoalarm');
    if(!r.ok){setGeoMsg(t('conn_error'),false);return;}
    const d=await r.json(),rt=d.runtime||{};
    dapCheck('geo-enabled',d.enabled);
    dapSet('geo-lat',d.flowstation_lat??0);
    dapSet('geo-lon',d.flowstation_lon??0);
    dapSet('geo-radius-m',d.radius_m||500);
    dapSet('geo-cooldown',d.cooldown_secs||300);
    dapCheck('geo-trigger-tetra',d.trigger_tetra);
    dapCheck('geo-trigger-meshcom',d.trigger_meshcom);
    dapCheck('geo-forward-tpg',d.forward_tpg2200);
    dapCheck('geo-forward-sds',d.forward_sds);
    dapCheck('geo-forward-sip',d.forward_sip);
    dapCheck('geo-forward-telegram',d.forward_telegram);
    dapSet('geo-tetra-white',geoIssiListText(d.tetra_issi_whitelist));
    dapSet('geo-tetra-black',geoIssiListText(d.tetra_issi_blacklist));
    dapSet('geo-mesh-white',meshSourceListText(d.meshcom_source_whitelist));
    dapSet('geo-mesh-black',meshSourceListText(d.meshcom_source_blacklist));
    dapSet('geo-sds-source',d.sds_source_issi||9999);
    dapSet('geo-sds-dest',d.sds_dest_issi||0);
    dapCheck('geo-sds-group',d.sds_dest_is_group);
    dapSet('geo-tpg-source',d.tpg2200_source_issi||9999);
    dapSet('geo-tpg-dest',d.tpg2200_dest_issi||0);
    dapSet('geo-tpg-incident',d.tpg2200_incident_base||1);
    dapSet('geo-tpg-prefix',d.tpg2200_text_prefix||'GeoAlarm');
    dapSet('geo-tpg-max',d.tpg2200_max_text_chars||80);
    dapSet('geo-sip-prefix',d.sip_title_prefix||'GeoAlarm');
    dapSet('geo-telegram-prefix',d.telegram_prefix||'GeoAlarm');
    dapSet('geo-seen',rt.seen_positions??0);
    dapSet('geo-alarms',rt.alarm_count??0);
    dapSet('geo-center',rt.center||`${d.flowstation_lat??0},${d.flowstation_lon??0}`);
    dapSet('geo-radius',`${Number(rt.radius_m||d.radius_m||0).toFixed(0)} m`);
    dapSet('geo-last-position',rt.last_position||'—');
    dapSet('geo-last-alarm',rt.last_alarm||'—');
    dapSet('geo-last-error',rt.last_error||'—');
    // Hero pill — reflect the enabled state; warn when enabled but a last error is present.
    const geoErr=rt.last_error&&rt.last_error!=='—';
    setIntegrationHero('geo', !!d.enabled, !!d.enabled&&!geoErr,
      d.enabled?(geoErr?t('integ_error'):t('integ_enabled')):t('integ_disabled'),
      rt.center||`${d.flowstation_lat??0}, ${d.flowstation_lon??0}`);
    state.geoalarmEvents=d.events||[];
    geoalarmPageIndex=0;
    renderGeoalarmEvents();
    setGeoMsg('',true);
  }catch{setGeoMsg(t('conn_error'),false);setIntegrationHero('geo',false,false,t('conn_error'),'');}
}
async function saveGeoalarm(){
  const tetraWhite=geoIssiListBody('geo-tetra-white','whitelist');
  if(tetraWhite===null)return;
  const tetraBlack=geoIssiListBody('geo-tetra-black','blacklist');
  if(tetraBlack===null)return;
  const body={
    enabled:document.getElementById('geo-enabled').checked,
    flowstation_lat:geoFloat('geo-lat',0,-90,90),
    flowstation_lon:geoFloat('geo-lon',0,-180,180),
    radius_m:dapNum('geo-radius-m',500,1,1000000),
    cooldown_secs:dapNum('geo-cooldown',300,1,86400),
    trigger_tetra:document.getElementById('geo-trigger-tetra').checked,
    trigger_meshcom:document.getElementById('geo-trigger-meshcom').checked,
    forward_tpg2200:document.getElementById('geo-forward-tpg').checked,
    forward_sds:document.getElementById('geo-forward-sds').checked,
    forward_sip:document.getElementById('geo-forward-sip').checked,
    forward_telegram:document.getElementById('geo-forward-telegram').checked,
    tetra_issi_whitelist:tetraWhite,
    tetra_issi_blacklist:tetraBlack,
    meshcom_source_whitelist:meshSourceListBody('geo-mesh-white'),
    meshcom_source_blacklist:meshSourceListBody('geo-mesh-black'),
    sds_source_issi:dapNum('geo-sds-source',9999,1,16777215),
    sds_dest_issi:dapNum('geo-sds-dest',0,0,16777215),
    sds_dest_is_group:document.getElementById('geo-sds-group').checked,
    tpg2200_source_issi:dapNum('geo-tpg-source',9999,1,16777215),
    tpg2200_dest_issi:dapNum('geo-tpg-dest',0,0,16777215),
    tpg2200_incident_base:dapNum('geo-tpg-incident',1,1,256),
    tpg2200_text_prefix:dapVal('geo-tpg-prefix')||'GeoAlarm',
    tpg2200_max_text_chars:dapNum('geo-tpg-max',80,8,160),
    sip_title_prefix:dapVal('geo-sip-prefix')||'GeoAlarm',
    telegram_prefix:dapVal('geo-telegram-prefix')||'GeoAlarm'
  };
  try{
    const r=await fetch('/api/geoalarm',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
    if(r.ok){setGeoMsg('✓ Saved',true);setTimeout(loadGeoalarm,500);}
    else setGeoMsg(t('save_fail')+': '+await r.text(),false);
  }catch{setGeoMsg(t('conn_error'),false);}
}
function setGeoMsg(txt,ok){const el=document.getElementById('geo-msg');if(!el)return;el.textContent=txt;el.style.color=ok?'var(--accent)':'var(--danger)';if(txt)setTimeout(()=>{if(el.textContent===txt)el.textContent='';},5000);}

function meshMapLink(lat,lon,label){
  if(lat===null||lat===undefined||lon===null||lon===undefined)return '—';
  const la=Number(lat),lo=Number(lon);
  if(!Number.isFinite(la)||!Number.isFinite(lo))return '—';
  const url=`https://maps.google.com/?q=${encodeURIComponent(la+','+lo)}`;
  return `<a class="sds-map-link" href="${url}" target="_blank" rel="noopener noreferrer">${escHtml(label||`${la.toFixed(5)}, ${lo.toFixed(5)}`)}</a>`;
}
function meshRfText(row){
  const parts=[];
  if(row.rssi!==null&&row.rssi!==undefined)parts.push(`RSSI ${row.rssi}`);
  if(row.snr!==null&&row.snr!==undefined)parts.push(`SNR ${row.snr}`);
  return parts.join(' · ')||'—';
}
function meshBatteryText(v){
  if(v===null||v===undefined||v==='')return '—';
  const n=Number(v);
  if(!Number.isFinite(n))return escHtml(v);
  return `${n}%`;
}
function meshSourceListText(values){
  return Array.isArray(values)?values.join('\n'):'';
}
function meshSourceListBody(id){
  const raw=dapVal(id);
  if(!raw)return [];
  return raw.split(/[\s,]+/).map(v=>v.trim()).filter(Boolean);
}
function meshPaths(paths){
  if(!Array.isArray(paths)||!paths.length)return '<span class="sds-empty">—</span>';
  return paths.map(p=>`<span class="badge badge-blue" style="font-size:10px">${escHtml(p)}</span>`).join(' ');
}
function meshNodeFiltered(){
  const q=(document.getElementById('mesh-node-filter')?.value||'').trim().toUpperCase();
  const rows=(state.meshcomNodes||[]).slice().sort((a,b)=>String(b.last_seen||'').localeCompare(String(a.last_seen||'')));
  if(!q)return rows;
  return rows.filter(n=>
    String(n.src||'').toUpperCase().includes(q) ||
    String(n.hw_id||'').toUpperCase().includes(q) ||
    String(n.firmware||'').toUpperCase().includes(q) ||
    String(n.fw_sub||'').toUpperCase().includes(q)
  );
}
function meshNodeRow(n){
  const fw=[n.firmware,n.fw_sub].filter(Boolean).join(' / ')||'—';
  return `<tr>
    <td><span class="badge badge-blue" style="font-size:10px">${escHtml(n.src||'—')}</span><div class="sds-empty">${escHtml(n.last_type||'')}</div></td>
    <td class="sds-time">${escHtml(n.last_seen||'—')}</td>
    <td>${meshMapLink(n.lat,n.lon)}</td>
    <td>${meshBatteryText(n.batt)}</td>
    <td class="sds-time">${escHtml(meshRfText(n))}</td>
    <td class="sds-time">${escHtml(fw)}</td>
    <td class="sds-time">${escHtml(n.hw_id||'—')}</td>
  </tr>`;
}
function renderMeshcomNodes(){
  const tb=document.getElementById('mesh-nodes-tbody');if(!tb)return;
  const rows=meshNodeFiltered();
  meshNodePageIndex=clampLogPage(meshNodePageIndex,rows.length);
  setLogPager('mesh-nodes-page',meshNodePageIndex,rows.length);
  if(!rows.length){tb.innerHTML=`<tr><td colspan="7" class="sds-empty" style="text-align:center;padding:24px">No MeshCom nodes yet</td></tr>`;return;}
  const start=meshNodePageIndex*LOG_PAGE_SIZE;
  tb.innerHTML=rows.slice(start,start+LOG_PAGE_SIZE).map(meshNodeRow).join('');
}
function meshNodePrevPage(){meshNodePageIndex--;renderMeshcomNodes();}
function meshNodeNextPage(){meshNodePageIndex++;renderMeshcomNodes();}
function meshMsgRow(m){
  const msgText=m.msg?escHtml(m.msg):(m.lat!==null&&m.lat!==undefined&&m.lon!==null&&m.lon!==undefined?'<span class="sds-empty">[position]</span>':'');
  const posRf=[meshMapLink(m.lat,m.lon,'map'),meshRfText(m)].filter(x=>x&&x!=='—').join(' · ')||'—';
  return `<tr>
    <td class="sds-time">${escHtml(m.ts||'')}</td>
    <td>${dirBadge(m.direction)}</td>
    <td><span class="badge" style="font-size:10px">${escHtml(m.msg_type||'unknown')}</span></td>
    <td>${escHtml(m.src||'—')}<div class="sds-empty">${escHtml(m.src_type||'')}</div></td>
    <td>${escHtml(m.dst||'—')}</td>
    <td class="sds-msg">${msgText}</td>
    <td>${meshPaths(m.paths)}</td>
    <td class="sds-time">${posRf}</td>
  </tr>`;
}
function renderMeshcomMessages(){
  const tb=document.getElementById('mesh-msgs-tbody');if(!tb)return;
  const rows=state.meshcomMessages||[];
  meshMsgPageIndex=clampLogPage(meshMsgPageIndex,rows.length);
  setLogPager('mesh-msgs-page',meshMsgPageIndex,rows.length);
  if(!rows.length){tb.innerHTML=`<tr><td colspan="8" class="sds-empty" style="text-align:center;padding:24px">No MeshCom packets yet</td></tr>`;return;}
  const start=meshMsgPageIndex*LOG_PAGE_SIZE;
  tb.innerHTML=rows.slice(start,start+LOG_PAGE_SIZE).map(meshMsgRow).join('');
}
function meshMsgPrevPage(){meshMsgPageIndex--;renderMeshcomMessages();}
function meshMsgNextPage(){meshMsgPageIndex++;renderMeshcomMessages();}
async function loadMeshcom(){
  try{
    const r=await fetch('/api/meshcom');
    if(!r.ok){setMeshMsg(t('conn_error'),false);return;}
    const d=await r.json(),rt=d.runtime||{};
    dapCheck('mesh-enabled',d.enabled);
    dapSet('mesh-bind-addr',d.bind_addr||'0.0.0.0');
    dapSet('mesh-bind-port',d.bind_port||1799);
    dapSet('mesh-tx-host',d.tx_host||'255.255.255.255');
    dapSet('mesh-tx-port',d.tx_port||1799);
    dapCheck('mesh-broadcast',d.allow_broadcast);
    dapSet('mesh-max-messages',d.max_messages||500);
    dapSet('mesh-max-nodes',d.max_nodes||1000);
    dapCheck('mesh-forward-sds',d.forward_sds);
    dapCheck('mesh-forward-sip',d.forward_sip);
    dapCheck('mesh-forward-telegram',d.forward_telegram);
    dapSet('mesh-sds-source',d.sds_source_issi||9999);
    dapSet('mesh-sds-dest',d.sds_dest_issi||0);
    dapCheck('mesh-sds-group',d.sds_dest_is_group);
    dapSet('mesh-sds-sources',meshSourceListText(d.sds_allowed_sources));
    dapSet('mesh-sip-prefix',d.sip_title_prefix||'MeshCom');
    dapSet('mesh-sip-sources',meshSourceListText(d.sip_allowed_sources));
    dapSet('mesh-telegram-prefix',d.telegram_prefix||'MeshCom');
    dapSet('mesh-telegram-sources',meshSourceListText(d.telegram_allowed_sources));
    dapSet('mesh-rx-count',rt.rx_packets??0);
    dapSet('mesh-tx-count',rt.tx_packets??0);
    dapSet('mesh-bind',rt.bind||`${d.bind_addr||'0.0.0.0'}:${d.bind_port||1799}`);
    dapSet('mesh-tx',rt.tx||`${d.tx_host||'255.255.255.255'}:${d.tx_port||1799}`);
    dapSet('mesh-node-count',rt.node_count??(d.nodes||[]).length);
    dapSet('mesh-last-rx',rt.last_rx||'—');
    dapSet('mesh-last-tx',rt.last_tx||'—');
    dapSet('mesh-last-error',rt.last_error||'—');
    state.meshcomNodes=d.nodes||[];
    state.meshcomMessages=d.messages||[];
    meshNodePageIndex=0;meshMsgPageIndex=0;
    renderMeshcomNodes();
    renderMeshcomMessages();
    setMeshMsg('',true);
  }catch{setMeshMsg(t('conn_error'),false);}
}
async function saveMeshcom(){
  const body={
    enabled:document.getElementById('mesh-enabled').checked,
    bind_addr:dapVal('mesh-bind-addr')||'0.0.0.0',
    bind_port:dapNum('mesh-bind-port',1799,1,65535),
    tx_host:dapVal('mesh-tx-host')||'255.255.255.255',
    tx_port:dapNum('mesh-tx-port',1799,1,65535),
    allow_broadcast:document.getElementById('mesh-broadcast').checked,
    max_messages:dapNum('mesh-max-messages',500,10,10000),
    max_nodes:dapNum('mesh-max-nodes',1000,10,65535),
    forward_sds:document.getElementById('mesh-forward-sds').checked,
    forward_sip:document.getElementById('mesh-forward-sip').checked,
    forward_telegram:document.getElementById('mesh-forward-telegram').checked,
    sds_source_issi:dapNum('mesh-sds-source',9999,1,16777215),
    sds_dest_issi:dapNum('mesh-sds-dest',0,0,16777215),
    sds_dest_is_group:document.getElementById('mesh-sds-group').checked,
    sds_allowed_sources:meshSourceListBody('mesh-sds-sources'),
    sip_title_prefix:dapVal('mesh-sip-prefix')||'MeshCom',
    sip_allowed_sources:meshSourceListBody('mesh-sip-sources'),
    telegram_prefix:dapVal('mesh-telegram-prefix')||'MeshCom',
    telegram_allowed_sources:meshSourceListBody('mesh-telegram-sources')
  };
  try{
    const r=await fetch('/api/meshcom',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
    if(r.ok){setMeshMsg('✓ Saved',true);setTimeout(loadMeshcom,500);}
    else setMeshMsg(t('save_fail')+': '+await r.text(),false);
  }catch{setMeshMsg(t('conn_error'),false);}
}
async function sendMeshcomMessage(){
  const body={dst:dapVal('mesh-out-dst'),msg:dapVal('mesh-out-msg')};
  if(!body.dst){setMeshSendMsg('Destination is empty',false);return;}
  if(!body.msg){setMeshSendMsg('Message text is empty',false);return;}
  setMeshSendMsg('Sending…',true);
  try{
    const r=await fetch('/api/meshcom/send',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
    const d=await r.json();
    if(d.ok){setMeshSendMsg('✓ Sent',true);document.getElementById('mesh-out-msg').value='';setTimeout(loadMeshcom,300);}
    else setMeshSendMsg('✗ '+(d.error||'Send failed'),false);
  }catch{setMeshSendMsg(t('conn_error'),false);}
}
function setMeshMsg(txt,ok){const el=document.getElementById('mesh-msg');if(!el)return;el.textContent=txt;el.style.color=ok?'var(--accent)':'var(--danger)';if(txt)setTimeout(()=>{if(el.textContent===txt)el.textContent='';},5000);}
function setMeshSendMsg(txt,ok){const el=document.getElementById('mesh-send-msg');if(!el)return;el.textContent=txt;el.style.color=ok?'var(--accent)':'var(--danger)';if(txt)setTimeout(()=>{if(el.textContent===txt)el.textContent='';},5000);}
function setDapMsg(txt,ok){const el=document.getElementById('dap-msg');if(!el)return;el.textContent=txt;el.style.color=ok?'var(--accent)':'var(--danger)';if(txt)setTimeout(()=>{if(el.textContent===txt)el.textContent='';},5000);}
function setDapSendMsg(txt,ok){const el=document.getElementById('dap-send-msg');if(!el)return;el.textContent=txt;el.style.color=ok?'var(--accent)':'var(--danger)';if(txt)setTimeout(()=>{if(el.textContent===txt)el.textContent='';},5000);}

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

// Export the live log buffer to a local .log file — no SSH required. Saves what is
// currently held in the dashboard (up to the most recent ~600 lines that passed the
// active level filter), as plain "TS  LEVEL  message" text.
function exportLog(){
  const lines=[...document.querySelectorAll('#log-container .log-line')].map(l=>{
    const ts=l.querySelector('.log-ts')?.textContent||'';
    const lv=l.querySelector('.log-level')?.textContent||'';
    const msg=(l.textContent||'').slice(ts.length+lv.length);
    return ts+'  '+lv.padEnd(5)+'  '+msg;
  });
  if(!lines.length){return;}
  const pad=n=>String(n).padStart(2,'0');
  const d=new Date();
  const stamp=`${d.getFullYear()}${pad(d.getMonth()+1)}${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`;
  const blob=new Blob([lines.join('\n')+'\n'],{type:'text/plain;charset=utf-8'});
  const a=document.createElement('a');
  a.href=URL.createObjectURL(blob);
  a.download=`flowstation-log-${stamp}.log`;
  document.body.appendChild(a);a.click();
  setTimeout(()=>{URL.revokeObjectURL(a.href);a.remove();},0);
}

// ── Asterisk SIP ───────────────────────────────────────────────────────────
async function loadAsteriskStatus(){
  const set=(id,v)=>{const el=document.getElementById(id);if(el)el.textContent=(v===null||v===undefined||v==='')?'—':v;};
  try{
    const r=await fetch('/api/asterisk/status');
    if(!r.ok)throw new Error('http '+r.status);
    const d=await r.json();
    const c=d.config||{}, rt=d.runtime||{};
    set('ast-configured', (c.configured||rt.configured)?'YES':'NO');
    set('ast-enabled', (c.enabled||rt.enabled)?'enabled':'disabled');
    set('ast-register', rt.register_status||'—');
    set('ast-dialogs', (rt.active_dialogs??0)+' active dialogs');
    set('ast-sip-listen', rt.sip_listen||c.sip_listen);
    set('ast-remote', rt.remote||c.remote);
    set('ast-rtp', rt.rtp_port_range||c.rtp_port_range);
    set('ast-codec', rt.codec||c.codec);
    set('ast-last-rx', rt.last_rx);
    set('ast-last-tx', rt.last_tx);
    set('ast-last-error', rt.last_error);
    // Hero connection pill — driven by the live REGISTER state.
    const enabled=!!(c.enabled||rt.enabled);
    const reg=(rt.register_status||'').toLowerCase();
    const registered=/regist|ok|online|200/.test(reg)&&!/fail|error|unreach|timeout/.test(reg);
    setIntegrationHero('ast', enabled, registered, rt.register_status||(enabled?t('offline'):'disabled'),
      (c.configured||rt.configured)?(rt.sip_listen||c.sip_listen||''):'');
    const cc=document.getElementById('ast-configured-card');
    if(cc){cc.classList.remove('is-ok','is-danger','is-idle');cc.classList.add((c.configured||rt.configured)?'is-ok':'is-idle');}
    const rc=document.getElementById('ast-register-card');
    if(rc){rc.classList.remove('is-ok','is-warn','is-danger','is-idle');rc.classList.add(registered?'is-ok':enabled?'is-warn':'is-idle');}
  }catch(e){
    set('ast-configured','—');set('ast-enabled','status unavailable');set('ast-register','—');
    set('ast-last-error',t('conn_error'));
    setIntegrationHero('ast', false, false, t('conn_error'), '');
  }
}
// Shared helper: drive an integration tab's hero dot + connection pill from
// (enabled, connected) state. Calm severity language: connected=ok, enabled-but-down=warn,
// disabled=idle. No color literals — all via .hero-dot/.pill variants.
function setIntegrationHero(prefix, enabled, connected, pillText, subText){
  const dot=document.getElementById(prefix+'-hero-dot');
  const pill=document.getElementById(prefix+'-hero-pill');
  const sub=document.getElementById(prefix+'-hero-sub');
  const lvl=!enabled?'idle':connected?'ok':'warn';
  if(dot) dot.className='hero-dot is-'+lvl;
  if(pill){pill.className='pill pill-'+lvl;pill.textContent=pillText||'—';}
  if(sub&&subText!=null) sub.textContent=subText||'—';
}

let snomPasswordDirty=false;
function setSnomMsg(txt,ok){
  const el=document.getElementById('snom-msg');
  if(!el)return;
  el.textContent=txt||'';
  el.style.color=ok?'var(--accent)':'var(--danger)';
}
function snomListText(values){return (values||[]).join('\n');}
function snomListBody(id){
  return (document.getElementById(id)?.value||'')
    .split(/[\s,]+/)
    .map(v=>v.trim())
    .filter(Boolean);
}
function snomRicListBody(id,label){
  const out=[],seen=new Set();
  for(const rawLine of (document.getElementById(id)?.value||'').split(/\r?\n/)){
    const line=rawLine.split('#')[0].trim();
    if(!line)continue;
    for(const raw of line.split(/[\s,]+/)){
      const part=raw.trim();
      if(!part)continue;
      if(!/^(?:0x[0-9a-f]+|[0-9]+)$/i.test(part)){setSnomMsg(`Invalid ${label} RIC: ${part}`,false);return null;}
      if(!seen.has(part)){seen.add(part);out.push(part);}
    }
  }
  return out;
}
function snomIssiListBody(id,label){
  const out=[],seen=new Set();
  for(const raw of snomListBody(id)){
    const n=Number(raw);
    if(!Number.isInteger(n)||n<0||n>16777215){setSnomMsg(`Invalid ${label} ISSI: ${raw}`,false);return null;}
    if(!seen.has(n)){seen.add(n);out.push(n);}
  }
  return out;
}
function snomSetDirections(values){
  const dirs=new Set((values&&values.length?values:['rx','net','tx']).map(v=>String(v).toLowerCase()));
  dapCheck('snom-dir-rx',dirs.has('rx'));
  dapCheck('snom-dir-net',dirs.has('net'));
  dapCheck('snom-dir-tx',dirs.has('tx'));
}
function snomDirectionsBody(){
  const dirs=[];
  if(document.getElementById('snom-dir-rx')?.checked)dirs.push('rx');
  if(document.getElementById('snom-dir-net')?.checked)dirs.push('net');
  if(document.getElementById('snom-dir-tx')?.checked)dirs.push('tx');
  return dirs;
}
async function loadSnomNotify(){
  try{
    const r=await fetch('/api/snom-notify');
    if(!r.ok){setSnomMsg(t('conn_error'),false);return;}
    const d=await r.json();
    dapCheck('snom-enabled',d.enabled);
    dapSet('snom-ami-host',d.ami_host||'127.0.0.1');
    dapSet('snom-ami-port',d.ami_port||5038);
    dapSet('snom-ami-user',d.ami_username||'');
    dapSet('snom-ami-password',d.ami_password_set?(d.ami_password_masked||''):'');
    snomPasswordDirty=false;
    dapSet('snom-endpoints',snomListText(d.endpoints));
    dapCheck('snom-notify-sds',d.notify_sds);
    dapCheck('snom-notify-dapnet',d.notify_dapnet);
    dapCheck('snom-notify-telegram',d.notify_telegram);
    snomSetDirections(d.sds_directions);
    dapSet('snom-dapnet-rics',dapRicListText(d.dapnet_allowed_rics));
    dapSet('snom-sds-issis',snomListText(d.sds_allowed_issis));
    dapSet('snom-title-prefix',d.title_prefix||'FlowStation');
    dapSet('snom-max-text',d.max_text_chars||240);
    dapSet('snom-timeout',d.connect_timeout_secs||3);
    setSnomMsg('',true);
  }catch{setSnomMsg(t('conn_error'),false);}
}
async function saveSnomNotify(){
  const dapnetRics=snomRicListBody('snom-dapnet-rics','DAPNET');
  if(dapnetRics===null)return;
  const sdsIssis=snomIssiListBody('snom-sds-issis','SDS');
  if(sdsIssis===null)return;
  const body={
    enabled:document.getElementById('snom-enabled').checked,
    ami_host:dapVal('snom-ami-host')||'127.0.0.1',
    ami_port:dapNum('snom-ami-port',5038,1,65535),
    ami_username:dapVal('snom-ami-user'),
    endpoints:snomListBody('snom-endpoints'),
    notify_sds:document.getElementById('snom-notify-sds').checked,
    notify_dapnet:document.getElementById('snom-notify-dapnet').checked,
    notify_telegram:document.getElementById('snom-notify-telegram').checked,
    sds_directions:snomDirectionsBody(),
    dapnet_allowed_rics:dapnetRics,
    sds_allowed_issis:sdsIssis,
    title_prefix:dapVal('snom-title-prefix')||'FlowStation',
    max_text_chars:dapNum('snom-max-text',240,40,2000),
    connect_timeout_secs:dapNum('snom-timeout',3,1,30)
  };
  if(snomPasswordDirty)body.ami_password=dapVal('snom-ami-password');
  try{
    const r=await fetch('/api/snom-notify',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
    if(r.ok){setSnomMsg('✓ Saved',true);loadSnomNotify();}
    else setSnomMsg(t('save_fail')+': '+await r.text(),false);
  }catch{setSnomMsg(t('conn_error'),false);}
}

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

// ── ISSI Whitelist ─────────────────────────────────────────────────────────
let whitelistEntries=[];
async function loadWhitelist(){
  try{
    const r=await fetch('/api/whitelist');
    if(!r.ok){setWhitelistMsg(t('conn_error'),false);return;}
    const d=await r.json();
    whitelistEntries=(d.issi_whitelist||[]).slice().sort((a,b)=>a-b);
    renderWhitelist();
    const badge=document.getElementById('whitelist-status');
    if(d.enabled){badge.textContent=t('whitelist_enforced');badge.style.color='var(--accent)';}
    else{badge.textContent=t('whitelist_open');badge.style.color='var(--muted)';}
  }catch{setWhitelistMsg(t('conn_error'),false);}
}
function renderWhitelist(){
  const box=document.getElementById('whitelist-chips');
  if(!whitelistEntries.length){
    box.innerHTML='<span style="color:var(--muted);font-size:13px" data-i18n="whitelist_empty">'+t('whitelist_empty')+'</span>';
    return;
  }
  box.innerHTML=whitelistEntries.map(issi=>
    '<span class="id-chip">'+issi+
    '<span class="id-chip-x" onclick="removeWhitelistEntry('+issi+')">×</span></span>'
  ).join('');
}
function addWhitelistEntry(){
  const inp=document.getElementById('whitelist-input');
  const v=parseInt(inp.value);
  if(!v||v<1||v>16777215){setWhitelistMsg(t('whitelist_invalid'),false);inp.focus();return;}
  if(whitelistEntries.includes(v)){inp.value='';return;}
  whitelistEntries.push(v);
  whitelistEntries.sort((a,b)=>a-b);
  renderWhitelist();
  inp.value='';
  inp.focus();
}
function removeWhitelistEntry(issi){
  whitelistEntries=whitelistEntries.filter(x=>x!==issi);
  renderWhitelist();
}
async function saveWhitelist(){
  try{
    const r=await fetch('/api/whitelist',{method:'POST',headers:{'Content-Type':'application/json'},
      body:JSON.stringify({issi_whitelist:whitelistEntries})});
    if(r.ok){setWhitelistMsg(t('saved'),true);loadWhitelist();}
    else setWhitelistMsg(t('save_fail')+': '+await r.text(),false);
  }catch{setWhitelistMsg(t('conn_error'),false);}
}
function setWhitelistMsg(txt,ok){const el=document.getElementById('whitelist-msg');el.textContent=txt;el.style.color=ok?'var(--accent)':'var(--danger)';setTimeout(()=>{if(el.textContent===txt)el.textContent='';},4000);}

// ── WX / METAR service ──────────────────────────────────────────────────────
async function loadWx(){
  try{
    const r=await fetch('/api/wx');
    if(!r.ok){setWxMsg(t('conn_error'),false);return;}
    const d=await r.json();
    document.getElementById('wx-enabled').checked=!!d.enabled;
    document.getElementById('wx-service-issi').value=d.service_issi||'';
    document.getElementById('wx-periodic-enabled').checked=!!d.periodic_enabled;
    document.getElementById('wx-periodic-icao').value=d.periodic_icao||'';
    document.getElementById('wx-periodic-issi').value=d.periodic_issi||'';
    document.getElementById('wx-periodic-isgroup').checked=!!d.periodic_is_group;
    document.getElementById('wx-periodic-interval').value=d.periodic_interval_secs||1800;
  }catch{setWxMsg(t('conn_error'),false);}
}
async function saveWx(){
  const body={
    enabled:document.getElementById('wx-enabled').checked,
    service_issi:parseInt(document.getElementById('wx-service-issi').value)||9998,
    periodic_enabled:document.getElementById('wx-periodic-enabled').checked,
    periodic_issi:parseInt(document.getElementById('wx-periodic-issi').value)||0,
    periodic_is_group:document.getElementById('wx-periodic-isgroup').checked,
    periodic_icao:(document.getElementById('wx-periodic-icao').value||'').trim().toUpperCase(),
    periodic_interval_secs:Math.max(300,parseInt(document.getElementById('wx-periodic-interval').value)||1800)
  };
  if(body.periodic_enabled&&(!body.periodic_issi||!body.periodic_icao)){setWxMsg(t('wx_periodic_incomplete'),false);return;}
  try{
    const r=await fetch('/api/wx',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
    if(r.ok){setWxMsg(t('saved'),true);loadWx();}
    else setWxMsg(t('save_fail')+': '+await r.text(),false);
  }catch{setWxMsg(t('conn_error'),false);}
}
function setWxMsg(txt,ok){const el=document.getElementById('wx-msg');el.textContent=txt;el.style.color=ok?'var(--accent)':'var(--danger)';setTimeout(()=>{if(el.textContent===txt)el.textContent='';},4000);}

// ── Telegram alerts ─────────────────────────────────────────────────────────
let tgChats=[];            // recipient chat IDs (numbers)
let tgChatNames={};        // id -> best-effort friendly name (display only)
let tgDetected=[];         // last "detect" result, for the Add buttons
let tgTokenDirty=false;    // true once the user edits the token field (so we send it)
function tgEsc(s){return (s||'').toString().replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');}
// The token to send: a freshly-typed value (never the masked placeholder), else '' = keep saved.
function tgTokenField(){const v=(document.getElementById('tg-token').value||'').trim();return (tgTokenDirty&&v&&!v.includes('…'))?v:'';}
async function loadTelegram(){
  try{
    const r=await fetch('/api/telegram');
    if(!r.ok){setTgMsg(t('conn_error'),false);return;}
    const d=await r.json();
    document.getElementById('tg-enabled').checked=!!d.enabled;
    const tok=document.getElementById('tg-token');
    tok.value=d.token_set?(d.bot_token_masked||''):'';
    tgTokenDirty=false;
    tgChats=(d.chat_ids||[]).slice();
    renderTgChips();
    document.getElementById('tg-connect').checked=!!d.alert_connect;
    document.getElementById('tg-disconnect').checked=!!d.alert_disconnect;
    document.getElementById('tg-t351').checked=!!d.alert_t351;
    document.getElementById('tg-lip').checked=!!d.alert_lip;
    document.getElementById('tg-backhaul').checked=!!d.alert_backhaul;
    document.getElementById('tg-logs').checked=!!d.alert_critical_logs;
    document.getElementById('tg-verify-status').textContent='';
    document.getElementById('tg-detected').innerHTML='';
  }catch{setTgMsg(t('conn_error'),false);}
}
function renderTgChips(){
  const box=document.getElementById('tg-chips');
  if(!tgChats.length){box.innerHTML='<span style="color:var(--muted);font-size:13px">'+t('tg_no_recipients')+'</span>';return;}
  box.innerHTML=tgChats.map(id=>{
    const nm=tgChatNames[id]?(' · '+tgEsc(tgChatNames[id])):'';
    return '<span class="id-chip">'+id+nm+
      '<span class="id-chip-x" onclick="removeRecipient('+id+')">×</span></span>';
  }).join('');
}
function addRecipient(){
  const inp=document.getElementById('tg-chat-input');
  const v=parseInt(inp.value,10);
  if(!Number.isInteger(v)||v===0){setTgRecipMsg(t('tg_invalid_chat'),false);inp.focus();return;}
  if(!tgChats.includes(v))tgChats.push(v);
  renderTgChips();inp.value='';inp.focus();
}
function removeRecipient(id){tgChats=tgChats.filter(x=>x!==id);renderTgChips();}
function addDetected(i){const c=tgDetected[i];if(!c)return;if(!tgChats.includes(c.id)){tgChats.push(c.id);tgChatNames[c.id]=c.name;renderTgChips();}}
async function verifyTelegram(){
  const st=document.getElementById('tg-verify-status');
  st.textContent=t('tg_verifying');st.style.color='var(--muted)';
  try{
    const r=await fetch('/api/telegram/verify',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({bot_token:tgTokenField()})});
    const d=await r.json();
    if(d.ok){st.textContent='✓ @'+(d.username||'bot');st.style.color='var(--accent)';}
    else{st.textContent='✗ '+tgEsc(d.error||'error');st.style.color='var(--danger)';}
  }catch{st.textContent=t('conn_error');st.style.color='var(--danger)';}
}
async function detectTelegramChats(){
  const box=document.getElementById('tg-detected');
  box.innerHTML='<span style="color:var(--muted);font-size:13px">'+t('tg_detecting')+'</span>';
  try{
    const r=await fetch('/api/telegram/detect',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({bot_token:tgTokenField()})});
    const d=await r.json();
    if(!d.ok){box.innerHTML='<span style="color:var(--danger);font-size:13px">✗ '+tgEsc(d.error||'error')+'</span>';return;}
    tgDetected=d.chats||[];
    if(!tgDetected.length){box.innerHTML='<span style="color:var(--muted);font-size:13px">'+t('tg_detect_none')+'</span>';return;}
    box.innerHTML='<div style="color:var(--muted);font-size:13px;margin-bottom:6px">'+t('tg_detect_found')+'</div>'+
      tgDetected.map((c,i)=>
        '<div style="display:flex;align-items:center;justify-content:space-between;gap:10px;padding:6px 0">'+
        '<span style="font-size:13px">'+tgEsc(c.name)+' <span style="color:var(--muted)">('+c.id+' · '+tgEsc(c.kind)+')</span></span>'+
        '<button class="btn" onclick="addDetected('+i+')">+ '+t('tg_add')+'</button></div>'
      ).join('');
  }catch{box.innerHTML='<span style="color:var(--danger);font-size:13px">'+t('conn_error')+'</span>';}
}
async function saveTelegram(){
  const body={
    enabled:document.getElementById('tg-enabled').checked,
    chat_ids:tgChats,
    alert_connect:document.getElementById('tg-connect').checked,
    alert_disconnect:document.getElementById('tg-disconnect').checked,
    alert_t351:document.getElementById('tg-t351').checked,
    alert_lip:document.getElementById('tg-lip').checked,
    alert_backhaul:document.getElementById('tg-backhaul').checked,
    alert_critical_logs:document.getElementById('tg-logs').checked
  };
  const tok=tgTokenField();if(tok)body.bot_token=tok;
  try{
    const r=await fetch('/api/telegram',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
    if(r.ok){setTgMsg(t('saved'),true);loadTelegram();}
    else setTgMsg(t('save_fail')+': '+await r.text(),false);
  }catch{setTgMsg(t('conn_error'),false);}
}
async function testTelegram(){
  setTgMsg(t('tg_testing'),true);
  const body={chat_ids:tgChats};const tok=tgTokenField();if(tok)body.bot_token=tok;
  try{
    const r=await fetch('/api/telegram/test',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
    const d=await r.json();
    if(d.ok)setTgMsg(t('tg_test_ok',{n:d.sent}),true);
    else setTgMsg('✗ '+tgEsc(d.error||'error'),false);
  }catch{setTgMsg(t('conn_error'),false);}
}
function setTgMsg(txt,ok){const el=document.getElementById('tg-msg');el.textContent=txt;el.style.color=ok?'var(--accent)':'var(--danger)';setTimeout(()=>{if(el.textContent===txt)el.textContent='';},5000);}
function setTgRecipMsg(txt,ok){const el=document.getElementById('tg-recipients-msg');el.textContent=txt;el.style.color=ok?'var(--accent)':'var(--danger)';setTimeout(()=>{if(el.textContent===txt)el.textContent='';},4000);}


function wsSend(msg){if(ws&&ws.readyState===WebSocket.OPEN){ws.send(JSON.stringify(msg));return true;}return false;}
async function restartService(){if(!confirm(t('confirm_restart')))return;wsSend({type:'restart'});}
async function shutdownService(){if(!confirm(t('confirm_shutdown')))return;wsSend({type:'shutdown'});}
function kickMs(issi){if(!confirm(t('confirm_kick',{issi})))return;wsSend({type:'kick',issi});}
function toggleSdsCallout(){const on=document.getElementById('sds-callout').checked;document.getElementById('sds-callout-fields').style.display=on?'block':'none';}
function resetSdsCallout(){document.getElementById('sds-callout').checked=false;document.getElementById('sds-callout-source').value='9999';document.getElementById('sds-callout-incident').value='1';document.getElementById('sds-callout-text').value='ALARM';document.getElementById('sds-callout-raw').value='';toggleSdsCallout();}
function openSds(issi){sdsDest=issi;document.getElementById('sds-dest').value=issi;document.getElementById('sds-msg').value='';resetSdsCallout();document.getElementById('sds-modal').classList.add('open');}
function closeSdsModal(){document.getElementById('sds-modal').classList.remove('open');}
function sendSds(){const dest=parseInt(document.getElementById('sds-dest').value);if(!dest)return;if(document.getElementById('sds-callout').checked){const source=parseInt(document.getElementById('sds-callout-source').value)||9999;const incident=Math.max(1,Math.min(256,parseInt(document.getElementById('sds-callout-incident').value)||1));const alarmText=document.getElementById('sds-callout-text').value.trim()||'ALARM';const rawhex=document.getElementById('sds-callout-raw').value.trim();wsSend({type:'sds_callout',dest_issi:dest,source_issi:source,incident,message:alarmText,raw_hex:rawhex});closeSdsModal();return;}const msg=document.getElementById('sds-msg').value.trim();if(!msg)return;wsSend({type:'sds',dest_issi:dest,message:msg});closeSdsModal();}
function openDgna(issi){document.getElementById('dgna-issi').value=issi;document.getElementById('dgna-gssi').value='';const cur=document.getElementById('dgna-current');const gl=(state.ms[issi]&&state.ms[issi].groups)||[];cur.innerHTML=gl.length?gl.slice().sort((a,b)=>a-b).map(g=>`<span class="badge badge-blue" style="font-size:10px">${g}</span>`).join(''):'<span class="badge badge-dim">—</span>';document.getElementById('dgna-modal').classList.add('open');}
function closeDgnaModal(){document.getElementById('dgna-modal').classList.remove('open');}
function sendDgna(attach){const issi=parseInt(document.getElementById('dgna-issi').value),gssi=parseInt(document.getElementById('dgna-gssi').value);if(!issi||!gssi)return;wsSend({type:'dgna',issi,gssi,attach});closeDgnaModal();}

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

// ── Display brightness (FH-FEAT-008) ─────────────────────────────────────────
// Debounced POST so dragging the slider doesn't flood the endpoint; status probe
// on page open reveals the card only when the backend reports a panel present.
let _brTimer=null;
function onBrightnessInput(v){
  const lbl=document.getElementById('brightness-val');if(lbl)lbl.textContent=v;
  clearTimeout(_brTimer);
  _brTimer=setTimeout(()=>{
    fetch('/api/system/brightness',{method:'POST',headers:{'Content-Type':'application/json'},credentials:'same-origin',body:JSON.stringify({value:parseInt(v,10)})}).catch(()=>{});
  },150);
}
function loadBrightness(){
  fetch('/api/system/brightness',{credentials:'same-origin'}).then(r=>r.json()).then(d=>{
    if(!d||!d.present)return;
    const card=document.getElementById('brightness-card');if(card)card.style.display='';
    const sl=document.getElementById('brightness-slider');
    if(sl){
      sl.max=d.max_brightness||255;
      if(typeof d.brightness==='number'){sl.value=d.brightness;const lbl=document.getElementById('brightness-val');if(lbl)lbl.textContent=d.brightness;}
    }
  }).catch(()=>{});
}

// Inline glyphs for the BTS header chips (no extra requests).
const BTS_TOWER_ICON='<svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 9v13"/><path d="M8.5 22h7"/><path d="M7 8a6 6 0 0 1 10 0"/><path d="M4.5 6a9 9 0 0 1 15 0"/></svg>';
const BTS_CLOCK_ICON='<svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>';
// TETRA BTS Details card — static cell + RF identity pulled from config (one fetch).
// ── Dual-Carrier ON/OFF (first-page toggle; applied via controlled restart) ──
let dcState={enabled:false,secondary_carrier:null,active:false,main_carrier:null};
function setDcSub(txt){const e=document.getElementById('dc-sub');if(e)e.textContent=txt;}
async function loadDualCarrier(){
  try{
    const r=await fetch('/api/dualcarrier',{credentials:'same-origin'});
    if(!r.ok)return;
    const d=await r.json();
    dcState=d;
    const tg=document.getElementById('dc-toggle');
    // Don't fight the user mid-toggle (while focused or a request is in flight).
    if(tg&&!tg.disabled&&document.activeElement!==tg){tg.checked=!!d.active;}
    setDcSub(d.active?t('dc_on_sub',{c:d.secondary_carrier}):t('dc_off_sub'));
  }catch{}
}
async function onDualCarrierToggle(el){
  const want=el.checked;
  let secondary=dcState.secondary_carrier;
  if(want&&!secondary){
    const def=dcState.main_carrier?(dcState.main_carrier+1):'';
    const v=prompt(t('dc_enter_carrier'),def);
    if(v===null){el.checked=false;return;}
    secondary=parseInt(v,10);
    if(!Number.isInteger(secondary)||secondary<=0){el.checked=false;alert(t('dc_bad_carrier'));return;}
  }
  if(!confirm(want?t('dc_confirm_on'):t('dc_confirm_off'))){el.checked=!want;return;}
  el.disabled=true;setDcSub(t('dc_applying'));
  try{
    const body=want?{enabled:true,secondary_carrier:secondary}:{enabled:false};
    const r=await fetch('/api/dualcarrier',{method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
    if(r.ok){setDcSub(t('dc_restarting'));}
    else{const err=await r.text();alert(t('dc_failed')+': '+err);el.checked=!want;el.disabled=false;loadDualCarrier();}
  }catch(e){alert(t('conn_error'));el.checked=!want;el.disabled=false;}
}
async function loadBtsInfo(){
  try{
    const r=await fetch('/api/btsinfo',{credentials:'same-origin'});
    if(!r.ok)return;
    const d=await r.json();
    const set=(id,v)=>setText(id,(v==null||v==='')?'—':v);
    const mhz=(hz,dp)=>(hz!=null&&isFinite(hz))?(hz/1e6).toFixed(dp==null?4:dp)+' MHz':'—';
    set('bts-tx', mhz(d.tx_freq_hz));
    set('bts-rx', mhz(d.rx_freq_hz));
    set('bts-shift', (d.shift_hz!=null&&isFinite(d.shift_hz))?((d.shift_hz>=0?'+':'')+(d.shift_hz/1e6).toFixed(3)+' MHz'):'—');
    set('bts-mcc', d.mcc);
    set('bts-mnc', d.mnc);
    const carrierValue=document.getElementById('bts-carrier');
    const carrierLabel=carrierValue?.previousElementSibling;
    if(carrierValue){
      const carriers=(Array.isArray(d.carriers)&&d.carriers.length)?d.carriers:[{
        carrier_num:d.main_carrier,
        tx_freq_hz:d.tx_freq_hz,
        rx_freq_hz:d.rx_freq_hz,
      }];
      if(carrierLabel)carrierLabel.textContent=(t('bts_carrier')||'Carrier')+(carriers.length>1?'s':'');
      carrierValue.classList.toggle('bts-carrier-listing', carriers.length>1);
      carrierValue.innerHTML=carriers.map(c=>{
        const carrierNum=(c.carrier_num??'â€”');
        const dl=mhz(c.tx_freq_hz);
        const ul=mhz(c.rx_freq_hz);
        return `<span class="bts-carrier-line">#${carrierNum} · DL ${dl} · UL ${ul}</span>`;
      }).join('');
      carrierValue.innerHTML=carrierValue.innerHTML
        .replace(/\u00c2\u00b7/g,' | ')
        .replace(/\u00c3\u00a2\u00e2\u201a\u00ac\u00e2\u20ac\u009d/g,'-');
    }
    state.mainCarrierNum=d.main_carrier!=null?d.main_carrier:state.mainCarrierNum;
    set('bts-carrier', d.main_carrier!=null?('#'+d.main_carrier):'â€”');
    ((Array.isArray(d.carriers)&&d.carriers.length)?d.carriers:[{
      carrier_num:d.main_carrier,
      tx_freq_hz:d.tx_freq_hz,
      rx_freq_hz:d.rx_freq_hz,
    }]).forEach(c=>tsEnsureCarrierInfo(c.carrier_num,c.tx_freq_hz,c.rx_freq_hz));
    renderTsGridCarrier();
    // Neighbor-cell + hangtime chips in the card header
    const nb=document.getElementById('bts-neighbor');
    if(nb){
      const n=d.neighbor_count||0;
      nb.innerHTML=BTS_TOWER_ICON+'Neighbor Cell · '+(n>0?('ON ('+n+' '+(n===1?'neighbor':'neighbors')+')'):'OFF');
      nb.className='bts-chip '+(n>0?'on':'off');
    }
    const hg=document.getElementById('bts-hang');
    if(hg){
      hg.innerHTML=BTS_CLOCK_ICON+'HangTime · '+(d.hangtime_secs!=null?d.hangtime_secs:'—')+' sec';
      hg.className='bts-chip time';
    }
    const acc=document.getElementById('bts-access');
    if(acc){
      const restricted=!!d.whitelist_restricted;
      acc.textContent=restricted?'RESTRICTED':'OPEN';
      acc.className='bts-access '+(restricted?'restricted':'open');
    }
    const sub=document.getElementById('bts-access-sub');
    if(sub){
      sub.textContent=d.whitelist_restricted
        ? ((d.whitelist_count||0)+' '+t('bts_wl_entries'))
        : t('bts_wl_open');
    }
  }catch(e){/* config endpoint unavailable — leave placeholders */}
}

async function loadBtsInfoLegacy(){
  try{
    const r=await fetch('/api/btsinfo',{credentials:'same-origin'});
    if(!r.ok)return;
    const d=await r.json();
    const set=(id,v)=>setText(id,(v==null||v==='')?'-':v);
    const mhz=(hz,dp)=>(hz!=null&&isFinite(hz))?(hz/1e6).toFixed(dp==null?4:dp)+' MHz':'-';
    const carriers=(Array.isArray(d.carriers)&&d.carriers.length)?d.carriers:[{
      carrier_num:d.main_carrier,
      tx_freq_hz:d.tx_freq_hz,
      rx_freq_hz:d.rx_freq_hz,
    }];

    set('bts-tx', mhz(d.tx_freq_hz));
    set('bts-rx', mhz(d.rx_freq_hz));
    set('bts-shift', (d.shift_hz!=null&&isFinite(d.shift_hz))?((d.shift_hz>=0?'+':'')+(d.shift_hz/1e6).toFixed(3)+' MHz'):'-');
    set('bts-mcc', d.mcc);
    set('bts-mnc', d.mnc);
    set('bts-carrier', d.main_carrier!=null?('#'+d.main_carrier):'-');

    state.mainCarrierNum=d.main_carrier!=null?d.main_carrier:state.mainCarrierNum;
    Object.keys(tsCarrierInfo).forEach(key=>delete tsCarrierInfo[key]);
    carriers.forEach(c=>tsEnsureCarrierInfo(c.carrier_num,c.tx_freq_hz,c.rx_freq_hz));
    renderTsGridCarrier();

    const nb=document.getElementById('bts-neighbor');
    if(nb){
      const n=d.neighbor_count||0;
      nb.innerHTML=BTS_TOWER_ICON+'Neighbor Cell | '+(n>0?('ON ('+n+' '+(n===1?'neighbor':'neighbors')+')'):'OFF');
      nb.className='bts-chip '+(n>0?'on':'off');
    }
    const hg=document.getElementById('bts-hang');
    if(hg){
      hg.innerHTML=BTS_CLOCK_ICON+'HangTime | '+(d.hangtime_secs!=null?d.hangtime_secs:'-')+' sec';
      hg.className='bts-chip time';
    }
    const acc=document.getElementById('bts-access');
    if(acc){
      const restricted=!!d.whitelist_restricted;
      acc.textContent=restricted?'RESTRICTED':'OPEN';
      acc.className='bts-access '+(restricted?'restricted':'open');
    }
    const sub=document.getElementById('bts-access-sub');
    if(sub){
      sub.textContent=d.whitelist_restricted
        ? ((d.whitelist_count||0)+' '+t('bts_wl_entries'))
        : t('bts_wl_open');
    }
  }catch(e){/* config endpoint unavailable - leave placeholders */}
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

    // CPU — gauge fill width + threshold state class on the .gauge wrapper.
    const cpuEl=document.getElementById('sysCpu');
    if(cpuEl) cpuEl.textContent=(sysData.cpu_model||'—')+(sysData.cpu_cores?` (${sysData.cpu_cores} cores)`:'');
    const cpuPct=sysData.cpu_pct||0;
    const cpuBarEl=document.getElementById('sysCpuBar');
    const cpuPctEl=document.getElementById('sysCpuPct');
    const cpuGauge=document.getElementById('sysCpuGauge');
    if(cpuBarEl) cpuBarEl.style.width=cpuPct+'%';
    if(cpuGauge) cpuGauge.className='gauge'+(cpuPct>80?' is-danger':cpuPct>60?' is-warn':'');
    if(cpuPctEl) cpuPctEl.textContent=cpuPct+'%';

    // RAM
    const ramTotal=sysData.ram_total_mb||0;
    const ramUsed=sysData.ram_used_mb||0;
    const ramPct=ramTotal>0?Math.round(ramUsed/ramTotal*100):0;
    const ramBarEl=document.getElementById('sysRamBar');
    const ramValEl=document.getElementById('sysRamVal');
    const ramGauge=document.getElementById('sysRamGauge');
    if(ramBarEl) ramBarEl.style.width=ramPct+'%';
    if(ramGauge) ramGauge.className='gauge'+(ramPct>85?' is-danger':ramPct>70?' is-warn':' is-info');
    if(ramValEl) ramValEl.textContent=`${ramUsed} / ${ramTotal} MB (${ramPct}%)`;

    // Temperature — state via stat-card class, hot label without emoji.
    const tempCard=document.getElementById('cpu-temp-card');
    const tempEl=document.getElementById('sysCpuTemp');
    const tempSub=document.getElementById('sysCpuTempSub');
    if(sysData.cpu_temp_c!=null){
      const tv=sysData.cpu_temp_c.toFixed(1);
      const hot=sysData.cpu_temp_c>75, warm=sysData.cpu_temp_c>60;
      if(tempCard){ tempCard.style.display=''; tempCard.className='stat-card '+(hot?'is-danger':warm?'is-warn':'is-ok'); }
      if(tempEl){ tempEl.textContent=tv+'°C'; }
      if(tempSub) tempSub.textContent=hot?t('sys_temp_hot'):warm?t('sys_temp_warm'):t('sys_temp_ok');
    } else {
      if(tempCard) tempCard.style.display='none';
    }

    // RF / SoapySDR
    const soapyEl=document.getElementById('sysSoapy');
    if(soapyEl) soapyEl.textContent=sysData.soapy_info||'—';

    updateSystemUptime();
    updateSysHero();
  }catch(e){console.error('loadSystemInfo',e);}
}
function updateSystemUptime(){
  if(!sysData||!sysData.uptime_secs)return;
  const u=sysData.uptime_secs;
  const d=Math.floor(u/86400),h=Math.floor((u%86400)/3600),m=Math.floor((u%3600)/60),s=u%60;
  let str='';if(d>0)str+=d+'d ';if(h>0||d>0)str+=h+'h ';if(m>0||h>0||d>0)str+=m+'m ';str+=s+'s';
  document.getElementById('sysUptime').textContent=str;
  const hu=document.getElementById('sysHeroUptime');if(hu)hu.textContent=str;
}
// Mirror the System tab's key state into its hero banner.
function updateSysHero(){
  const dot=document.getElementById('sysHeroDot');
  const sub=document.getElementById('sysHeroSub');
  const tempV=document.getElementById('sysHeroTemp');
  const btsCard=document.getElementById('sysBtsCard');
  const btsOnline=btsCard&&btsCard.classList.contains('is-ok');
  const brewCard=document.getElementById('sysBrewCard');
  const brewOnline=brewCard&&brewCard.classList.contains('is-info');
  if(dot) dot.className='hero-dot '+(btsOnline?'is-ok':'is-danger');
  if(sub){
    const host=(sysData&&sysData.hostname)||document.getElementById('sysHostname').textContent||'—';
    sub.textContent=(btsOnline?t('online'):t('offline'))+' · '+(brewOnline?t('brew_online'):t('brew_offline'))+' · '+host;
  }
  if(tempV){
    const tc=document.getElementById('sysCpuTemp');
    const card=document.getElementById('cpu-temp-card');
    tempV.textContent=(card&&card.style.display!=='none'&&tc)?tc.textContent:'—';
  }
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
  const btsCard=document.getElementById('sysBtsCard');
  const brewCard=document.getElementById('sysBrewCard');
  if(ipEl)ipEl.textContent=online?location.hostname:'—';
  if(stEl)stEl.textContent=online?t('online'):t('offline');
  if(btsCard)btsCard.className='stat-card '+(online?'is-ok':'is-danger');
  if(bsEl)bsEl.textContent=brewOnline?t('brew_online'):t('brew_offline');
  if(brewCard)brewCard.className='stat-card '+(brewOnline?'is-info':'is-danger');
  if(bdEl){bdEl.textContent=brewOnline?`Brew v${brewVer||0}`:'—';}
  updateSysHero();
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
paintIcons();
setLang(currentLang);
setTheme(currentTheme);
applyUiSize();
applyTouchMode();

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
  // Hero summary
  setText('rf-hero-freq', isFinite(freqMHz) && freqMHz>0 ? freqMHz.toFixed(3)+' MHz' : '—');
  setText('rf-hero-sub',  t('rf_live')||'live');
  const rhd=document.getElementById('rf-hero-dot');
  if(rhd) rhd.className='hero-dot is-ok';

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

  // Show only the operationally-relevant TX metrics. DC offset + IQ amplitude/phase
  // imbalance are modulator-calibration diagnostics and were trimmed from the UI.
  paintQuality('rf-evm',     'rf-q-evm-wrap',  fmtPct(evm, 2),       evalEvm(evm));
  setText('rf-hero-evm', fmtPct(evm, 2));
  paintQuality('rf-papr',    'rf-q-papr-wrap', fmtDb(papr, 1),       evalPapr(papr));
  paintQuality('rf-carrier', 'rf-q-cl-wrap',   fmtDb(cl, 1, true),   evalCarrierLeakage(cl));
  paintQuality('rf-obw',     'rf-q-obw-wrap',  fmtKhz(obw),          evalObw(obw));
}

function handleSdrHealth(msg){
  rfState.lastHwTs = Date.now();
  setText('rf-hw-age', t('rf_just_now')||'just now');

  // Temperature with named state. Thresholds chosen so a typical LimeSDR running
  // at room temp (~45-55°C) reads "nominal", >65 is "warm", >80 is "hot".
  const tempEl = document.getElementById('rf-temp');
  const stateEl = document.getElementById('rf-temp-state');
  const tempGauge = document.getElementById('rf-temp-gauge');
  const tempBar = document.getElementById('rf-temp-bar');
  if(tempEl && stateEl){
    if(msg.temperature_c == null){
      tempEl.textContent = '—';
      stateEl.textContent = t('rf_temp_na')||'no sensor';
      stateEl.className = 'rf-hw-temp-state';
      if(tempGauge){ tempGauge.classList.remove('is-warn','is-danger','is-info'); tempGauge.classList.add('is-idle'); }
      if(tempBar) tempBar.style.width = '0%';
    } else {
      const tc = msg.temperature_c;
      tempEl.textContent = tc.toFixed(1) + ' °C';
      let cls = 'nominal', label = t('rf_temp_nominal')||'nominal', gcls='';
      if(tc < 20){ cls='cold'; label = t('rf_temp_cold')||'cold'; gcls='is-info'; }
      else if(tc > 80){ cls='hot'; label = t('rf_temp_hot')||'hot'; gcls='is-danger'; }
      else if(tc > 65){ cls='warm'; label = t('rf_temp_warm')||'warm'; gcls='is-warn'; }
      stateEl.textContent = label;
      stateEl.className = 'rf-hw-temp-state ' + cls;
      if(tempGauge){
        tempGauge.classList.remove('is-warn','is-danger','is-info','is-idle');
        if(gcls) tempGauge.classList.add(gcls);
      }
      // Map 0-100°C onto the track (clamped).
      if(tempBar) tempBar.style.width = Math.max(0,Math.min(100,tc)).toFixed(0) + '%';
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

// Plain-English diagnosis + remediation per (domain, level) — the "Looking Glass" advice.
const HEALTH_ADVICE = {
  service: {
    ok: { why: 'The TETRA core loop is processing TDMA frames in real time.', do: [] },
    degraded: { why: 'Time between TDMA ticks is higher than expected — the SDR/USB link or the CPU is lagging behind real time. Calls still work but timing is tight.',
      do: ['Check CPU load & temperature on the System tab (or `top`).',
           'Look for "Too late to produce TX block" / SDR underrun lines in the Log.',
           'Make sure no other heavy process is starving the BTS (it runs at FIFO priority).'] },
    critical: { why: 'The stack stopped processing TDMA frames. Calls and SDS will fail and radios may drop. This is the most serious state.',
      do: ['Check the Log for a panic or repeated SDR errors.',
           'Restart the service: `systemctl restart <unit>`.',
           'Enable the software watchdog so this auto-recovers: `[health] restart_on_core_stall = true`.'] },
  },
  backhaul: {
    ok: { why: 'The Brew/TetraPack interconnect is up — calls/SDS route to other cells & BrandMeister.', do: [] },
    degraded: { why: 'The Brew/TetraPack backhaul is DOWN. The cell still works locally, but calls and SDS to/from other cells or BrandMeister will not route.',
      do: ['Check network/internet connectivity from the Pi to the Brew server.',
           'Verify the [brew] host / port / credentials on the Config tab.',
           'Confirm the Brew server is reachable. The station auto-reconnects when it comes back.'] },
  },
  radios: {
    ok: { why: 'Attached radios are being heard on the air.', do: [] },
    degraded: { why: 'Registered radios have not transmitted for a while. They may have left coverage without de-registering, or RX has degraded.',
      do: ['Check the antenna / feedline and the RX gain.',
           'Confirm the radios are actually in range and powered on.',
           'Truly-gone radios are pruned automatically at the T351 interval.'] },
  },
  congestion: {
    ok: { why: 'Downlink (MCCH) and SDS queues are draining normally.', do: [] },
    degraded: { why: 'The downlink or SDS queue is filling faster than it drains — too much signalling/SDS, a flapping radio, or the SDR dropping TX blocks.',
      do: ['Check the SDS Log for a radio spamming retransmits or a flood of broadcasts.',
           'Reduce Home-Mode-Display / broadcast-SDS rate if it is heavy.',
           'Check SDR TX health on the RF tab for dropped blocks.'] },
    critical: { why: 'The downlink/SDS backlog is severe — grants, signalling and messages will be delayed or dropped.',
      do: ['Act urgently: identify and kick a misbehaving radio from the Radios tab.',
           'Check the Log for "Too late to produce TX block" (the SDR can\'t keep up).',
           'Reduce broadcast/SDS load until the queues drain.'] },
  },
};
function healthColor(lvl){ return lvl==='critical' ? 'var(--danger)' : (lvl==='degraded' ? 'var(--warn)' : 'var(--ok)'); }
// Map a health level to the premium status class suffix used by .h-pill / .h-ring / .h-ico.
function healthLevelClass(lvl){ return lvl==='critical' ? 'bad' : (lvl==='degraded' ? 'warn' : 'ok'); }
function healthDomainLabel(d){ return ({service:'Core loop',backhaul:'Backhaul (Brew)',radios:'Radios',congestion:'Congestion'})[d] || d; }
// Clean inline SVGs replace the old emoji domain icons. {svg, accent} where accent
// drives the tinted .h-ico colour (default accent / blue / purple for domain variety).
const HEALTH_SVG = {
  service:{svg:'<path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3v6h-6"/>',accent:''},
  backhaul:{svg:'<path d="M4.93 4.93a14 14 0 0 0 0 14.14M19.07 4.93a14 14 0 0 1 0 14.14M8.46 8.46a7 7 0 0 0 0 7.08M15.54 8.46a7 7 0 0 1 0 7.08"/><circle cx="12" cy="12" r="1.5"/>',accent:'blue'},
  radios:{svg:'<rect x="3" y="9" width="13" height="11" rx="1.5"/><path d="M16 4 9 9"/><circle cx="7.5" cy="14.5" r="2.5"/><path d="M19 10v9"/>',accent:'purple'},
  congestion:{svg:'<path d="M3 3v18h18"/><rect x="7" y="11" width="3" height="6"/><rect x="13" y="7" width="3" height="10"/>',accent:''},
};
function healthDomainSvg(d){
  const m = HEALTH_SVG[d] || {svg:'<circle cx="12" cy="12" r="3"/>',accent:''};
  return '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">'+m.svg+'</svg>';
}
function healthDomainAccent(d){ return (HEALTH_SVG[d]||{}).accent || ''; }
// Inline SVGs for the integration cards (replace ☎ 📟 ◎).
const INTEGRATION_SVG = {
  asterisk:'<path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6 19.79 19.79 0 0 1-3.07-8.67A2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72c.13.96.36 1.9.7 2.81a2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.11-.45c.9.34 1.85.57 2.81.7A2 2 0 0 1 22 16.92z"/>',
  dapnet:'<rect x="5" y="2" width="14" height="20" rx="2"/><path d="M9 6h6M9 10h6M9 14h3"/>',
  geoalarm:'<path d="M12 21s-7-5.5-7-11a7 7 0 0 1 14 0c0 5.5-7 11-7 11z"/><circle cx="12" cy="10" r="2.5"/>',
};
function integrationSvg(key){
  const p = INTEGRATION_SVG[key] || '<circle cx="12" cy="12" r="3"/>';
  return '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">'+p+'</svg>';
}
function healthDur(s){ s=Math.max(0,Math.floor(s||0)); const d=Math.floor(s/86400),h=Math.floor((s%86400)/3600),m=Math.floor((s%3600)/60);
  return d>0 ? (d+'d '+h+'h') : (h>0 ? (h+'h '+m+'m') : (m+'m')); }

function renderHealthTab(h){
  const grid = document.getElementById('health-grid');
  if(!grid) return;
  const overall = h.overall || 'ok';
  const dot   = document.getElementById('health-hero-dot');
  const title = document.getElementById('health-hero-title');
  const sub   = document.getElementById('health-hero-sub');
  const up    = document.getElementById('health-uptime');
  const act   = document.getElementById('health-action');
  // Hero ring: status-tinted via class (was a bg colour) — keeps the SVG check inside.
  if(dot)   dot.className = 'h-ring ' + healthLevelClass(overall);
  if(title) title.textContent = 'Station health: ' + overall.toUpperCase();
  const bad = (h.domains||[]).filter(function(d){return d.level!=='ok';});
  if(sub) sub.textContent = bad.length
      ? (bad.length+' domain(s) need attention: '+bad.map(function(d){return healthDomainLabel(d.domain);}).join(', '))
      : 'All systems nominal.';
  if(up)  up.textContent  = (typeof h.uptime_secs==='number') ? ('Uptime '+healthDur(h.uptime_secs)) : '';
  if(act) act.textContent = h.last_action ? ('Last action: '+h.last_action) : '';

  grid.innerHTML = '';
  (h.domains||[]).forEach(function(d){
    const branch = HEALTH_ADVICE[d.domain] || {};
    const adv = branch[d.level] || branch.degraded || { why:'', do:[] };
    const lvlCls = healthLevelClass(d.level);
    const accent = d.level==='ok' ? healthDomainAccent(d.domain) : lvlCls;
    const card = document.createElement('div');
    card.className = 'h-card';
    let todoHtml = '';
    if(d.level!=='ok' && adv.do && adv.do.length){
      todoHtml = '<div class="h-todo"><span class="h-todo-h">What to do</span><ul>'
             + adv.do.map(function(x){return '<li>'+escHtml(x)+'</li>';}).join('')
             + '</ul></div>';
    }
    card.innerHTML =
      '<div class="h-ico '+accent+'">'+healthDomainSvg(d.domain)+'</div>'
      + '<div class="h-col">'
        + '<div class="h-head">'
          + '<span class="h-ttl">'+escHtml(healthDomainLabel(d.domain))+'</span>'
          + '<span class="h-pill '+lvlCls+'">'+(d.level||'').toUpperCase()+'</span>'
        + '</div>'
        + '<div class="h-det"><span class="h-status-lbl">Status:</span> '+escHtml(d.detail||'')+'</div>'
        + (adv.why ? '<div class="h-det">'+escHtml(adv.why)+'</div>' : '')
        + todoHtml
      + '</div>';
    grid.appendChild(card);
  });
}

let healthIntegrationState={asterisk:null,dapnet:null,geoalarm:null,meshcom:null,lastLoad:0};
// title, iconKey (asterisk|dapnet|geoalarm), accent (blue|purple|''), level, detail, extra.
function integrationHealthCard(title,iconKey,accent,level,detail,extra){
  const lvlCls = healthLevelClass(level);
  const icoCls = level==='ok' ? accent : lvlCls;
  const card=document.createElement('div');
  card.className='h-card compact';
  card.innerHTML=
    '<div class="h-ico '+icoCls+'">'+integrationSvg(iconKey)+'</div>'
    + '<div class="h-col">'
      + '<div class="h-head">'
        + '<span class="h-ttl">'+escHtml(title)+'</span>'
        + '<span class="h-pill '+lvlCls+'">'+level.toUpperCase()+'</span>'
      + '</div>'
      + '<div class="h-det"><span class="h-status-lbl">Status:</span> '+escHtml(detail||'')+'</div>'
      + (extra?'<div class="h-det">'+escHtml(extra)+'</div>':'')
    + '</div>';
  return card;
}
function classifyAsteriskHealth(data){
  const c=(data&&data.config)||{},rt=(data&&data.runtime)||{};
  const enabled=!!(c.enabled||rt.enabled);
  if(!enabled)return {level:'ok',detail:'disabled',extra:'SIP bridge is configured but not active.'};
  const reg=String(rt.register_status||'').toLowerCase();
  const dialogs=rt.active_dialogs??0;
  const err=rt.last_error||'';
  let level='ok';
  if(err)level='degraded';
  if(c.register && reg && !/(registered|reachable|ok|disabled)/.test(reg))level='degraded';
  const detail=(rt.register_status||'enabled')+' · '+dialogs+' active dialog(s)';
  const extra=err?('Last error: '+err):('Remote '+(rt.remote||c.remote||'—')+' · codec '+(rt.codec||c.codec||'—'));
  return {level,detail,extra};
}
function classifyDapnetHealth(data){
  if(!data||!data.enabled)return {level:'ok',detail:'disabled',extra:'DAPNET worker is not active.'};
  const rt=data.runtime||{};
  const paths=[];
  if(data.forward_sds||rt.forward_sds)paths.push('SDS');
  if(data.forward_callout||rt.forward_callout)paths.push('TPG2200');
  if(data.forward_telegram||rt.forward_telegram)paths.push('Telegram');
  let level='ok';
  const notes=[];
  const rwthStatus=String(rt.rwth_core_status||'').toLowerCase();
  const lastError=rt.last_error||'';
  if(data.rwth_core_enabled){
    if(!data.rwth_core_callsign)notes.push('RWTH callsign missing');
    if(!data.rwth_core_authkey_set)notes.push('RWTH authkey missing');
    if(lastError)notes.push('Last error: '+lastError);
    if(rwthStatus && !/(logged in|connected)/.test(rwthStatus))notes.push('RWTH status '+rt.rwth_core_status);
  } else {
    notes.push('RWTH receive feed disabled');
  }
  if(!paths.length)notes.push('no forwarding path enabled');
  if(notes.length)level='degraded';
  const status=rt.rwth_core_status||(data.rwth_core_enabled?'enabled':'disabled');
  const detail='RWTH '+status+' · '+(paths.length?paths.join(', '):'no forwarding');
  const extra=notes.length?notes.join(' · '):('Host '+(rt.endpoint||((data.rwth_core_host||'—')+':'+(data.rwth_core_port||'—')))+' · seen '+(rt.seen_messages??0)+(rt.last_rx?' · last RX '+rt.last_rx:''));
  return {level,detail,extra};
}
function classifyGeoalarmHealth(data){
  if(!data||!data.enabled)return {level:'ok',detail:'disabled',extra:'GeoAlarm is not active.'};
  const rt=data.runtime||{};
  const err=rt.last_error||'';
  const paths=[];
  if(data.forward_tpg2200||rt.forward_tpg2200)paths.push('TPG2200');
  if(data.forward_sds||rt.forward_sds)paths.push('SDS');
  if(data.forward_sip||rt.forward_sip)paths.push('SIP');
  if(data.forward_telegram||rt.forward_telegram)paths.push('Telegram');
  const notes=[];
  if(!paths.length)notes.push('no forwarding path enabled');
  if(!data.trigger_tetra&&!data.trigger_meshcom)notes.push('no input source enabled');
  if(err)notes.push('Last error: '+err);
  const level=notes.length?'degraded':'ok';
  const detail=(rt.seen_positions??0)+' position(s) · '+(rt.alarm_count??0)+' alarm(s)';
  const extra=notes.length?notes.join(' · '):('Center '+(rt.center||'—')+' · radius '+Number(rt.radius_m||data.radius_m||0).toFixed(0)+' m · routes '+paths.join(', '));
  return {level,detail,extra};
}
function classifyMeshcomHealth(data){
  if(!data||!data.enabled)return {level:'ok',detail:'disabled',extra:'MeshCom UDP bridge is not active.'};
  const rt=data.runtime||{};
  const err=rt.last_error||'';
  const level=err?'degraded':'ok';
  const paths=[];
  if(data.forward_sds||rt.forward_sds)paths.push('SDS');
  if(data.forward_sip||rt.forward_sip)paths.push('SIP');
  if(data.forward_telegram||rt.forward_telegram)paths.push('Telegram');
  const detail=(rt.rx_packets??0)+' RX · '+(rt.tx_packets??0)+' TX · '+(rt.node_count??0)+' node(s)';
  const extra=err?('Last error: '+err):('Bind '+(rt.bind||((data.bind_addr||'—')+':'+(data.bind_port||'—')))+' · TX '+(rt.tx||((data.tx_host||'—')+':'+(data.tx_port||'—')))+' · routes '+(paths.join(', ')||'none')+(rt.last_rx?' · last RX '+rt.last_rx:''));
  return {level,detail,extra};
}
function renderHealthIntegrations(){
  const grid=document.getElementById('health-integrations-grid');
  if(!grid)return;
  grid.innerHTML='';
  if(healthIntegrationState.asterisk){
    const a=classifyAsteriskHealth(healthIntegrationState.asterisk);
    grid.appendChild(integrationHealthCard('Asterisk SIP','asterisk','',a.level,a.detail,a.extra));
  } else {
    grid.appendChild(integrationHealthCard('Asterisk SIP','asterisk','','degraded','status unavailable','Open the Asterisk SIP page or wait for the next refresh.'));
  }
  if(healthIntegrationState.dapnet){
    const d=classifyDapnetHealth(healthIntegrationState.dapnet);
    grid.appendChild(integrationHealthCard('DAPNET','dapnet','blue',d.level,d.detail,d.extra));
  } else {
    grid.appendChild(integrationHealthCard('DAPNET','dapnet','blue','degraded','status unavailable','Open the DAPNET page or wait for the next refresh.'));
  }
  if(healthIntegrationState.geoalarm){
    const g=classifyGeoalarmHealth(healthIntegrationState.geoalarm);
    grid.appendChild(integrationHealthCard('GeoAlarm','geoalarm','purple',g.level,g.detail,g.extra));
  } else {
    grid.appendChild(integrationHealthCard('GeoAlarm','geoalarm','purple','degraded','status unavailable','Open the GeoAlarm page or wait for the next refresh.'));
  }
  if(healthIntegrationState.meshcom){
    const m=classifyMeshcomHealth(healthIntegrationState.meshcom);
    grid.appendChild(integrationHealthCard('MeshCom','dapnet','blue',m.level,m.detail,m.extra));
  } else {
    grid.appendChild(integrationHealthCard('MeshCom','dapnet','blue','degraded','status unavailable','Open the MeshCom page or wait for the next refresh.'));
  }
}
async function loadHealthIntegrations(){
  healthIntegrationState.lastLoad=Date.now();
  try{
    const [ast,dap,geo,mesh]=await Promise.all([
      fetch('/api/asterisk/status').then(r=>r.ok?r.json():null).catch(()=>null),
      fetch('/api/dapnet').then(r=>r.ok?r.json():null).catch(()=>null),
      fetch('/api/geoalarm').then(r=>r.ok?r.json():null).catch(()=>null),
      fetch('/api/meshcom').then(r=>r.ok?r.json():null).catch(()=>null)
    ]);
    healthIntegrationState.asterisk=ast;
    healthIntegrationState.dapnet=dap;
    healthIntegrationState.geoalarm=geo;
    healthIntegrationState.meshcom=mesh;
  }catch{}
  renderHealthIntegrations();
}

function handleHealth(h){
  // Topbar station-health badge: colour + label by overall level, details in the tooltip.
  const badge = document.getElementById('health-badge');
  const lbl   = document.getElementById('health-badge-label');
  if(!badge || !lbl) return;
  if(!h || !h.overall){ badge.style.display='none'; return; }
  const lvl = h.overall; // "ok" | "degraded" | "critical"
  const color = lvl==='critical' ? 'var(--danger)' : (lvl==='degraded' ? 'var(--warn)' : '#3fb950');
  lbl.textContent = lvl.toUpperCase();
  lbl.style.color = color;
  badge.style.display = 'flex';
  const bad = (h.domains||[]).filter(function(d){return d.level!=='ok';})
                             .map(function(d){return '• '+d.domain+': '+d.level+' ('+d.detail+')';});
  let tip = 'Station health: '+lvl.toUpperCase();
  tip += bad.length ? '\n'+bad.join('\n') : '\nAll domains nominal';
  if(h.last_action) tip += '\nAction: '+h.last_action;
  if(typeof h.uptime_secs==='number') tip += '\nUptime: '+h.uptime_secs+'s';
  badge.title = tip;
  // Also refresh the full Health "Looking Glass" tab.
  renderHealthTab(h);
  if(document.getElementById('page-health')?.classList.contains('active') && Date.now()-healthIntegrationState.lastLoad>10000){
    loadHealthIntegrations();
  }
}

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

  const sensLabel = document.getElementById('sys-sensors-label');
  const sensors = (msg && msg.sensors) || [];
  if(sensors.length === 0){
    // Nothing detected — leave the card hidden so we don't clutter the System tab.
    card.style.display = 'none';
    if(sensLabel) sensLabel.style.display = 'none';
    return;
  }
  card.style.display = '';
  if(sensLabel) sensLabel.style.display = '';

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
      totEl.innerHTML = '<span class="btn-icon" style="margin:0 4px 0 0;width:13px;height:13px;vertical-align:-2px">'+svgIcon('power')+'</span>' + msg.total_power_w.toFixed(2) + ' W total';
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
    if(v >= 65) return 'var(--warn)';
    if(v >= 50) return 'var(--ok)';
    return 'var(--accent2)';
  }
  if(kind === 'power') return 'var(--accent2)';
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
  // Keep rf-q-* on the wrap (drives the value-text color), and mirror the
  // threshold onto the shared .gauge as is-warn/is-danger (good = default --ok).
  wrap.classList.remove('rf-q-good','rf-q-warn','rf-q-bad');
  wrap.classList.add('rf-q-' + evalResult.status);
  const gauge = wrap.querySelector('.gauge');
  if(gauge){
    gauge.classList.remove('is-warn','is-danger');
    if(evalResult.status==='warn') gauge.classList.add('is-warn');
    else if(evalResult.status==='bad') gauge.classList.add('is-danger');
  }
  const bar = wrap.querySelector('.gauge-fill');
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
  const bgRgb = parseHexRgb(col.bg) || [9, 13, 20];

  const rows = rfState.waterfall.length;
  const bins = rfState.waterfall[0].length;
  if(rows <= 0 || bins <= 0) return;

  // Noise-floor threshold in [0..1]. pushWaterfall normalises -100..0 dBFS into 0..1.
  const NOISE_FLOOR = 0.16;

  // Render the heatmap at its native resolution (bins × rows) onto an offscreen
  // canvas, then scale it to fill the panel with drawImage(). drawImage honours the
  // HiDPI transform set by rfResizeCanvas — the old putImageData() path did NOT,
  // which is what left the column shifted to the left and only partly filled the
  // height. Scaling also makes the limited history fill top-to-bottom and keeps the
  // (fft-shifted) carrier dead-centre.
  let buf = rfState._wfBuf;
  if(!buf){ buf = rfState._wfBuf = document.createElement('canvas'); }
  if(buf.width !== bins || buf.height !== rows){ buf.width = bins; buf.height = rows; }
  const bctx = buf.getContext('2d');
  const img = bctx.createImageData(bins, rows);
  for(let row = 0; row < rows; row++){
    const spec = rfState.waterfall[row];
    for(let x = 0; x < bins; x++){
      const v = spec[x];
      const rgb = v < NOISE_FLOOR ? bgRgb : viridisColor(v);
      const p = (row * bins + x) * 4;
      img.data[p]   = rgb[0];
      img.data[p+1] = rgb[1];
      img.data[p+2] = rgb[2];
      img.data[p+3] = 255;
    }
  }
  bctx.putImageData(img, 0, 0);

  ctx.fillStyle = col.bg;
  ctx.fillRect(0, 0, w, h);

  // Zoom to the central frequency window so the narrow-band TETRA carrier fills the
  // view (instead of a thin strip lost in a wide span), centred on DC.
  const leftPad = 38;
  const VIEW = 0.5;                       // show the central 50% of the FFT span
  const srcX = bins * (1 - VIEW) / 2, srcW = bins * VIEW;
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = 'high';
  ctx.drawImage(buf, srcX, 0, srcW, rows, leftPad, 0, w - leftPad, h);

  // Time axis on the left. History now fills the full height, so map labels across h.
  ctx.font = '9px ui-monospace, Cascadia Code, Consolas, monospace';
  ctx.fillStyle = col.text3;
  ctx.textAlign = 'right';
  ctx.textBaseline = 'middle';
  const step = rows <= 45 ? 10 : (rows <= 120 ? 30 : 60);
  ctx.fillText('0s', leftPad - 4, 7);
  for(let s = step; s < rows - step*0.4; s += step){
    const y = (s / rows) * h;
    ctx.fillText('-'+s+'s', leftPad - 4, y);
    ctx.strokeStyle = col.grid;
    ctx.beginPath();
    ctx.moveTo(leftPad - 2, y); ctx.lineTo(leftPad, y);
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

// ── GitHub update-check ─────────────────────────────────────────────────────
// Best-effort: query GitHub for the latest release once at boot (and when the
// config page is opened). If a newer version exists, reveal the header badge and
// highlight the Update button. Failures are silent.
async function checkUpdate(){
  try{
    const r=await fetch('/api/update/check');
    if(!r.ok)return;
    const d=await r.json();
    const badge=document.getElementById('update-badge');
    const btn=document.getElementById('update-btn');
    if(d&&d.update_available&&d.latest){
      if(badge){badge.style.display='block';badge.textContent='⬆ '+t('update_available')+' '+d.latest;}
      if(btn){btn.classList.add('btn-primary');btn.textContent='⬆ '+t('update')+' → '+d.latest;}
    }else{
      if(badge)badge.style.display='none';
      if(btn){btn.classList.remove('btn-primary');btn.textContent='⬆ '+t('update');}
    }
  }catch{/* silent */}
}

// ── Boot gating (FH-FEAT-033) ───────────────────────────────────────────────
// When the dashboard has auth enabled AND public_overview is on, an anonymous
// visitor is served the SPA shell but must NOT open the WS or hit privileged
// endpoints. Probe one privileged endpoint: 401 => anonymous (public mode);
// 200 => either a no-auth deployment or an authenticated admin — behave as before.
async function boot(){
  const hasAuthMarker = document.cookie.split(';').some(c=>c.trim().startsWith('fs_auth='));
  let anonymous = false;
  if(!hasAuthMarker){
    try{ const r = await fetch('/api/system', {credentials:'same-origin'}); anonymous = (r.status===401); }
    catch{ anonymous = false; }
  }
  if(anonymous){ enterPublicMode(); return; }
  connect();
  // Populate the topbar SDR badge (and prime system data) immediately on load,
  // instead of waiting for the user to open the System tab.
  loadSystemInfo();
  loadBtsInfoLegacy();  // TETRA BTS Details card on the default (Radios) page
  loadDualCarrier();    // Dual-Carrier ON/OFF toggle state
  wifiProbeAvailable(); // toggles the WiFi nav item
  checkUpdate();
}
function enterPublicMode(){
  // Anonymous read-only mode: hide every admin nav item + logout, reveal Login, show only the
  // public overview page, and poll the narrow public snapshot. No WS, no privileged fetches.
  document.querySelectorAll('.nav-item').forEach(n=>{ n.style.display='none'; });
  const lb=document.getElementById('login-btn'); if(lb) lb.style.display='inline-flex';
  const lo=document.getElementById('logout-btn'); if(lo) lo.style.display='none';
  document.querySelectorAll('.page').forEach(p=>p.classList.remove('active'));
  const pp=document.getElementById('page-public'); if(pp) pp.classList.add('active');
  pollPublic();
  setInterval(pollPublic, 3000);
}
async function pollPublic(){
  try{
    const r=await fetch('/api/public', {credentials:'same-origin'});
    if(!r.ok) return;
    const d=await r.json();
    const setT=(id,v)=>{ const e=document.getElementById(id); if(e) e.textContent=v; };
    setT('pub-ms', d.registered_ms ?? '—');
    setT('pub-calls', (d.active_calls ?? 0) + (d.active_calls ? ' ('+d.group_calls+'G / '+d.individual_calls+'I)' : ''));
    setT('pub-freq', d.center_freq_hz ? (d.center_freq_hz/1e6).toFixed(4)+' MHz' : '—');
    setT('pub-rf', d.rf_active ? 'Active' : 'Idle');
    setT('pub-brew', d.brew_online ? 'Online' : 'Offline');
    setT('pub-ver', d.stack_version || '—');
    const STAT_STATES=['is-ok','is-idle','is-info','is-warn','is-danger'];
    const rfc=document.getElementById('pub-rf-card');
    if(rfc){ rfc.classList.remove(...STAT_STATES); rfc.classList.add(d.rf_active?'is-ok':'is-idle'); }
    const pbc=document.getElementById('pub-brew-card');
    if(pbc){ pbc.classList.remove(...STAT_STATES); pbc.classList.add(d.brew_online?'is-info':'is-danger'); }
  }catch{/* silent */}
}
boot();
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
<meta name="theme-color" content="#eceff4">
<title>FlowStation — Login</title>
<style>
:root{
  --bg:#eceff4;--bg2:#ffffff;--bg3:#e6eaf1;--bg4:#d6dde7;
  --border:#dde3ec;--border2:#c4cdd9;
  --text:#16202e;--text2:#3d4f66;--text3:#5f7188;
  --accent:#00876a;--accent2:#1565c0;--danger:#c0203a;
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
  /* Premium light backdrop: faint dot-grid texture + soft brand glows */
  background:
    radial-gradient(circle at 1px 1px, rgba(30,45,70,0.05) 1px, transparent 0) 0 0/22px 22px,
    radial-gradient(900px 520px at 18% 6%, rgba(21,101,192,0.07), transparent 55%),
    radial-gradient(900px 560px at 84% 96%, rgba(0,135,106,0.07), transparent 55%),
    var(--bg);
  -webkit-tap-highlight-color:transparent;
}

.login-card{
  width:100%;max-width:380px;
  background:linear-gradient(180deg, #ffffff 0%, #f7f9fc 100%);
  border:1px solid var(--border);
  border-radius:16px;
  box-shadow:
    0 22px 54px -22px rgba(30,45,70,0.28),
    0 6px 16px rgba(30,45,70,0.10),
    inset 0 1px 0 rgba(255,255,255,0.8);
  padding:38px 32px 30px;
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
  background:linear-gradient(135deg, rgba(0,135,106,0.12) 0%, rgba(21,101,192,0.12) 100%);
  border:1px solid rgba(0,135,106,0.30);
  display:flex;align-items:center;justify-content:center;
  box-shadow:0 6px 18px -6px rgba(0,135,106,0.30);
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
  background:linear-gradient(180deg, #00a07e 0%, var(--accent) 100%);
  color:#ffffff;font-weight:700;letter-spacing:0.04em;
  border:none;border-radius:8px;
  padding:13px 16px;font-family:var(--sans);font-size:14px;
  cursor:pointer;
  margin-top:6px;
  transition:transform 0.05s, box-shadow 0.15s, filter 0.15s;
  box-shadow:0 6px 16px -4px rgba(0,135,106,0.45);
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
