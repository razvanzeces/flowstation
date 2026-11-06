# Changelog — Bost FlowStation → PTBS

Notas para operadores. El dashboard OTA muestra las secciones posteriores a tu versión actual.

## v0.5.5 — Wizard µCell (SoapyMuCell)

- **Wizard µCell.** El asistente y la página Setup pueden instalar el driver `mucell` (placa µCell BB, distinto de SXceiver): clona/compila SoapyMuCell y el overlay `mu-cell-bb_raspberrypi`, escribe `device = "driver=mucell"`, y si el helper imprime `NEED_REBOOT` ofrece reiniciar la Pi (`POST /api/system/reboot`) para cargar el device-tree. Familias de ganancia/antenas alineadas con SXceiver; validación `SoapyDriverFamily::MuCell`.

## Pendiente de publicar — Autenticación, TEA2/TEA3, OTAR, E2EE de SDS, WAP y USRP B210

- **Traspaso sin interrupción de llamadas individuales (privadas/dúplex) — provisional, aún sin validar al aire.** Una llamada privada o dúplex ya no tiene por qué cortarse cuando la radio cambia de celda a mitad de llamada: el conmutador del sitio pre-provisiona la pata restaurada en la celda destino (con canal de tráfico asignado y abierto) y el CMCE la empareja por par de ISSI, respondiendo al `U-CALL RESTORE` con `D-CALL RESTORE` y asignación de canal para llevar a la radio a la nueva portadora. **Pendiente:** reencaminado de la voz en el conmutador y limpieza del traspaso abortado; sin validar todavía en hardware multi-celda.
- **Ticks de entrega de SDS en la consola.** Los informes de entrega SDS-TL que una radio envía al ISSI del despachador (o a 9999) ya no se descartan como «no registrado localmente»: se asocian al mensaje enviado por su referencia de mensaje y el registro SDS y la nueva vista «Enviados» de la consola muestran ✓ enviado, ✓✓ entregado o ✗ rechazado. Las entradas del registro guardan `message_ref` y `delivered`; nuevo evento de telemetría `SdsReport`.
- **Nombres de radio.** Botón «Nombre» en la tabla de radios y en la lista de la consola de despacho para asignar un nombre a un ISSI (guardado en `radio_names.json` junto a la configuración, API `GET/POST /api/radio-names`). El nombre se muestra donde aparezca ese ISSI, por delante del indicativo de RadioID, y el chip del grupo en recepción indica quién ha pulsado el PTT («▶ Dave (2358245)»); el estado de despacho incluye `rx_issi`.
- **Lista de escaneo con nombres e importación desde archivo (consola de despacho).** Cada TG puede llevar un nombre y se muestra como «World Wide (91)» en los chips y en la franja de llamada. Botón «Importar archivo» (CSV, TXT o JSON: `World Wide (91)`, `World Wide,91`, `91,World Wide` o `{"name":"World Wide","gssi":91}`) y «Exportar» a CSV; campo de nombre opcional al añadir un TG a mano.
- **Corrección: numeración de ranuras del enlace ascendente tras «Stack is N slot(s) behind the SDR clock».** Al adelantar el reloj de la pila para alcanzar al SDR, los demoduladores de subida no se movían con él, por lo que desde ese momento cada ráfaga de las radios se etiquetaba una ranura tarde: la voz de las radios caía en una ranura sin circuito (temporizador de inactividad a los 3 s de cada llamada, nadie oía a nadie en el grupo), las llamadas privadas no llegaban a establecerse y los SDS entre radios nunca se reensamblaban hasta reiniciar. Ahora la pila y los demoduladores avanzan juntos: las ranuras que el demodulador salta por pérdida de muestras se replican en la pila sin esperar a RX, y cualquier otro adelanto de la pila salta también una ranura de aire en los demoduladores de subida.
- **Autenticación de radios (TAA1, EN 300 392-7 cláusula 4).** La estación puede desafiar a cada radio con su clave K de 128 bits al registrarse (`[security] authentication = off | optional | required`, con autenticación mutua). Las claves se guardan por ISSI en `[[security.subscribers]]`. Nuevo crate `tetra-security` con HURDLE-II, TA11/TA12/TA21/TA22/TA41/TA51/TA52/TA61 y TB4/TB5, comprobado contra vectores de prueba públicos. Las radios que pasan muestran la insignia **AUTH** en la tabla de radios.
- **TEA2 y TEA3** además de TEA1 para el cifrado de clase 2 (`ksg = 2 | 3`). TEA1 se mantiene para investigación e interoperabilidad y se marca como débil (solo conserva 32 bits de clave).
- **OTAR de SCK (cláusula 4.5.2).** Entrega de la clave estática a una radio por el aire, sellada con la K de esa radio (TA41 + TA51); la respuesta de la radio (U-OTAR SCK RESULT) se muestra en el panel. Con `class = 1` y una clave configurada, la clave queda preparada solo para OTAR y la celda sigue en claro, de modo que la flota puede recibir la clave antes de activar el cifrado.
- **Página Seguridad** en el panel: estado en ejecución frente a guardado con reinicio en un clic, modo de autenticación, tabla de claves de abonado con generador de claves, cifrado (algoritmo, SCK, número, versión, grupos en claro), «Enviar SCK» por radio y a todas las radios conectadas; columna **Seguridad** en la tabla de radios y fila SEGURIDAD en la barra lateral.
- **USRP B210 (UHD):** opción «Instalar USRP (UHD)» en el asistente y en el instalador, valores por defecto correctos para full-duplex (TX en TX/RX, RX en RX2, PGA 40 dB), imagen FPGA abierta para los clones Kintex-7 (`fx3 is in state 5`), dependencias del paquete .deb y documentación.
- **Cifrado extremo a extremo (E2EE) de SDS.** Desactivado por defecto (`[security.e2ee]`): sella el contenido de los mensajes de estado y texto entre radios con AES‑128/256 en modo CTR y MAC AES‑CMAC (cifra‑luego‑autentica), independiente del cifrado de aire TEA. Pensado para sistemas privados; en la red amateur se incluye como función aunque no se use.
- **Página WAP con estado en vivo.** La página del punto de acceso muestra actividad real —SDS vistos, nivel de señal general, radios registradas y antigüedad de la última actividad de radio— con documentación de configuración para operadores. Se añade una **tabla por radio** que separa la actividad de grupo de la individual/privada.
- **Corrección: canal de datos por paquetes (PDCH) del WAP — provisional, aún sin validar al aire.** Al conceder el canal de datos, la estación movía la radio a una ranura que seguía anunciándose como no asignada y enviaba la respuesta WAP por el canal de control principal, que la radio ya no escuchaba, por lo que la página no llegaba. Ahora la ranura se anuncia como canal de control asignado mientras haya una radio en ella, y todo lo dirigido a esa radio (respuesta WAP, acuses de enlace, llamadas individuales y SDS entrantes) se envía por el canal de datos. El PDCH pasa a la ranura 4 para no chocar con la voz, que se asigna desde la ranura 2 y siempre tiene prioridad. Se responde a SN-RECONNECT, de modo que una sesión de datos continúa tras cambiar de celda.
- **Corrección: desactivar el WAP ya no impide arrancar la estación.** `[cell_info.wap_ip].enabled` pasa a ser el único interruptor maestro del WAP; con el WAP desactivado la BTS arranca con normalidad (antes fallaba al iniciar).
- **Mapa de posición (LIP) sin conexión.** La biblioteca del mapa (Leaflet) se sirve desde la propia estación, de modo que el mapa de ubicaciones funciona sin acceso a Internet.
- **Un error de configuración ya no detiene una estación en marcha.** Al cargar las celdas adicionales, una celda con configuración inválida se omite con un aviso en lugar de abortar el arranque de toda la estación.

## v0.5.4 — Escucha de ambiente (SS-AL) y petición de posición

- **Escucha de ambiente (SS-AL, ETSI EN 300 392-12-21).** Nueva función de despacho: la estación acepta el comando `AmbienceListen` del canal de control (brew-server) y reconoce el byte de servicio de escucha de ambiente que envía la consola. Monta una llamada individual directa y símplex hacia la radio destino con el indicador de notificación «AL operation» (valor 3, EN 300 392-9), de modo que una radio compatible conecta y abre micrófono por sí sola, sin acción del usuario. La radio **señaliza la llamada** como cualquier otra (no es encubierta). El audio de la radio llega a la consola y, al ser una llamada solo de recepción, ya no se corta a los 30 s. `enable: false` la libera. Validado al aire con MTH800.
- **Petición de posición (LIP).** El despacho puede pedir a una radio su posición (una vez o de forma periódica) y el informe LIP llega a la consola que lo solicitó. Antes, con el reenvío LIP activo, el informe se enviaba solo al ISSI de reenvío y la consola que lo había pedido no lo recibía.

## v0.5.2 — Multi-celda: RF de cada celda y versión en la telemetría

- **Página RF por celda.** Cada celda adicional envía ahora su espectro, constelación, cascada, calidad de señal (EVM, PAPR, fuga de portadora, ancho de banda) y salud del SDR (temperatura, ganancias). Pestañas compactas arriba de la página RF eligen la celda; cada celda conserva su propio historial de cascada.
- **Versión de la estación en la telemetría.** Nuevo evento `StationVersion` (versión, build y versión base) al conectar con el servidor de telemetría; brew-server la muestra junto a la IP de la BTS.
- **Menos tráfico de telemetría.** El espectro y la constelación (~5 KB, cinco veces por segundo y celda) ya no se envían al servidor de telemetría; solo los usa la página RF local. EVM, PAPR y salud del SDR se siguen enviando, por celda (`CellRf`).

## v0.5.1 — Multi-celda: SDS de red y telemetría de celdas

- **SDS de la red a radios de celdas adicionales.** Un SDS llegado por Brew (incluido el SMS Center de brew-server) para una radio registrada en una celda adicional se descartaba sin respuesta; ahora se entrega en su celda y Brew recibe el `SDS_REPORT`.
- **SDS entre celdas registrado una sola vez.** La celda que lo recibe de otra celda ya no lo vuelve a anotar como SDS de red en el registro de SDS ni en la telemetría.
- **Telemetría de celdas.** Nuevo evento `CellsSnapshot` cada 10 s (también en estaciones de una celda): portadoras y frecuencias, código de color, área de localización, vecinas, SDR, estado RF y las radios registradas en cada celda. brew-server lo muestra en su panel de telemetría y resincroniza con él su lista de registros tras una reconexión.

## v0.5.0 — Multi-celda: una estación, varios SDR

Primera versión multi-celda. Una estación, varios SDR: cada SDR es una celda TETRA más (p. ej. una Pi con dos Pluto+). Completo en código y tests, **aún sin validar en el aire**; las configs de una sola celda funcionan igual que en v0.4.4.

**Configuración y arranque**
- Nuevas entradas `[[cells]]` (id 1-7): heredan `[cell_info]`, llevan su propio `[cells.soapysdr]`. Portadoras únicas, mismo `freq_band` y `custom_duplex_spacing`, y `device` obligatorio en cada celda (también la principal). Las configs de una sola celda no cambian.
- Cada celda corre su propio stack de radio en su propio hilo y SDR.
- Dashboard: tarjeta **Celdas** en Inicio (estado RF, portadoras, SDR, radios) con escaneo de SDR, alta y baja de celdas (reinicio).

**Celdas enlazadas (con Brew o LST activos)**
- Llamadas de grupo, de la red o de cualquier radio, en todas las celdas con miembros; un solo hablante por grupo en todo el sitio; una llamada de emergencia se impone al hablante de otra celda.
- Llamadas individuales y SDS entre celdas; el SDS de grupo llega a los miembros de todas las celdas. Solo sale a Brew lo que no es para nadie del sitio; los grupos de `local_ssi_ranges` enlazan celdas sin salir a Brew.
- Asterisk (SIP) y WX/METAR desde todas las celdas; los cambios de WX en el dashboard se aplican a todas.
- La voz entre celdas pasa por un jitter buffer y sale al ritmo TDMA de la celda que la recibe.

**Movilidad**
- Cada celda anuncia a las demás como vecinas (reselección anunciada y no anunciada), con extensión de portadora y potencia máxima cuando difieren.
- Al registrarse en otra celda, la radio se da de baja en silencio en la anterior y restaura su llamada de grupo en la nueva.
- Traspaso anunciado: U-PREPARE → D-NEW-CELL (o D-PREPARE-FAIL); la celda destino se une antes a las llamadas de grupo de la radio; el registro reenviado en U-PREPARE (tipo 1) se procesa en la celda destino y su respuesta va dentro de D-NEW-CELL; U-RESTORE → D-RESTORE-ACK / D-RESTORE-FAIL.

**Dashboard y telemetría**
- La tabla de radios registradas muestra todas las celdas (insignia C0/C1…) y la página de llamadas incluye las de todas las celdas.
- Telemetría: nuevo evento `MsCell` (celda de cada registro); las llamadas de celdas adicionales usan identificadores propios de la estación (desde 0x4000).

**Cambios para todas las estaciones (también de una celda)**
- La estación ignoraba todas las PDU MLE de subida; ahora responde a U-PREPARE (D-PREPARE-FAIL si no hay vecinas) y a U-RESTORE.
- D-NWRK-BROADCAST con vecinas lleva umbrales/histéresis de reselección configurables (`[cell_info.cell_reselect]`, por defecto 20/10/10/6 dB); antes siempre 0.
- Tests de `tetra-entities` reparados (import `CallOrigin`, config Brew en el test de preempción) y `Cargo.lock` sincronizado.

**Pendiente de comprobar en el aire:** el orden de bits de los parámetros de reselección y de la extensión de portadora; llamadas individuales que no sobreviven a un cambio de celda; CPU/ancho de banda con varios SDR en una Pi.

## v0.4.4 — Puertos del dashboard: presets y binds estables

Elección de puertos sin pelear con nginx/apache ni spamear ERROR en el log.

- Instalador (SSH/TTY): pregunta **Estándar (80→443)** o **Puerto alto (solo HTTPS 8443)**; `BOST_DASH_PORTS=standard|high` sin TTY. Configs existentes no se tocan.
- Sistema → Acceso al panel: selector de preset + Apply & Restart (nueva URL tras reinicio).
- `port = 0` desactiva el redirect HTTP. Migración OTA solo si `port = 8080` sin `https_port`.
- Listeners legacy 8080/8443 solo en layout :443, fail-soft (sin reintentos infinitos). Bind canónico: conflicto → hint + retry 60s.
- Validado en campo. Promovido a canal **stable** (`main` / `bost`); `beta` al mismo tip.

## v0.4.3 — LST SDS: ocultar ACK de entrega en el log

- Los SDS-TL SHORT REPORT (confirmación de entrega del MS) ya no se registran en el log/inbox SDS.
- Evita la fila fantasma `[text]` justo después de un SDS enviado desde el despacho.
- Validado en campo (MS↔despacho privado/grupo, origen `operator_issi`). Promovido a canal **stable** (`main` / `bost`); `beta` al mismo tip.

## v0.4.2 — LST SDS: identidad despachador y ACK

SDS de despacho LST deja de fingir entrega a 9999 y usa la ISSI del operador.

- Con LST activo, SDS del roster/panel salen con `source_issi = operator_issi` (no 9999).
- SDS entrantes al `operator_issi` se absorben con SDS-TL SHORT REPORT (el walkie deja de marcar error de envío).
- Se elimina la absorción ciega de SDS-DATA a 9999: ruta estándar (local / Brew / undeliverable). WX y U-STATUS a 9999 sin cambios.
- Inbox LST: solo privados destinados al despachador (sin filtro dual 9999).

## v0.4.1 — Brew WiFi: leave mid-QSO como Ethernet

Misma lógica Ethernet/WiFi; con latencia WiFi el MS podía seguir como owner Local mientras Brew hablaba, y rojo/cambio de TG tumbaba el circuito (media huérfana + U-SETUP encima).

- Al preempt de Brew, ownership Local→Network.
- U-DISCONNECT del owner solo soft-leave si Brew tiene el suelo (D-RELEASE personal; grupo vivo).
- Sin listeners + Brew activo → Hold / LATE ENTRY (también si origin aún Local).
- Release Local con brew_uuid → NetworkCallEnd antes de cerrar circuito (anti-zombie DL).
- Owner con suelo local: teardown ETSI sin cambios.
- Validado en campo (WiFi Hold → LATE ENTRY). Promovido a canal **stable** (`main` / `bost`); `beta` al mismo tip.

## v0.4.0 — Red host, Dual Carrier GUI y canal estable

Salto menor de serie (aún sin rebrand a PTBS): nuevas capacidades de red en la GUI y consolidación de lo validado en beta.

### Página Red (antes WiFi)

- Menú **Red** con icono híbrido Ethernet+WiFi (superpuestos, estilo LST).
- Sección **Enlaces**: interfaces ethernet/wifi con IP(s) y badge de **ruta por defecto**.
- Gestión de perfiles **Ethernet** (conectar / desconectar) vía NetworkManager.
- WiFi (conexión actual, redes guardadas, disponibles) en **una sola tarjeta** con separadores.
- Estados NM y perfil «Wired connection» traducidos al idioma de la GUI.
- API `/api/network/*` (overview + ethernet); `/api/wifi/*` sin cambios.

### U-STATUS (walkie → ISSI 9999)

- `ip` / `info` listan **todas** las IPs de host (`eth0=…*`, `wlan0=…`; `*` = ruta por defecto).
- Respuesta **multilínea** (CR/LF admitidos en SDS de texto).
- Enumeración rápida con `getifaddrs` (sin `nmcli` en el hilo de radio — evita caída del stack).

### Dual Carrier (TMO Cell)

- Configuración Dual Carrier en GUI (Config → Advanced RF), límite al passband de Fs.
- Home / BTS Details: estado Activo/Apagado, mini-tiles secondary, orden MCCH/BCCH.

### Repo / OTA

- Eliminado el workflow de sync con upstream FlowStation (force-push a `main`).
- Promovido a canal **stable** (`main` / `bost`); `beta` al mismo tip.

## v0.3.47 — U-STATUS IP multilínea

- El SDS de texto admite CR/LF (antes se filtraban).
- Status IP: `Host IP` y cada interfaz en su línea (`eth0=…*`, `wlan0=…`).
- Página Red + fixes U-STATUS IP (v0.3.44–0.3.47) promovidos a canal **stable** (`main` / `bost`).

## v0.3.46 — Fix: U-STATUS IP no bloquea el stack

- El Status/info de IP ya no llama a `nmcli` en el hilo de radio (provocaba «Too late to produce TX block» y caída del stack).
- Lista IPs con `getifaddrs` + `primary_ip()` (rápido, como el Status de temperatura).

## v0.3.45 — Red: icono, i18n NM y WiFi unificado

- Icono de menú Red: jack Ethernet y arcos WiFi superpuestos (estilo LST), arcos más anchos.
- Sección de página «Red» (antes «Integraciones»); WiFi en una sola tarjeta con separadores.
- Estados NM (`connected`, …) y perfil «Wired connection» traducidos al idioma de la GUI.

## v0.3.44 — Página Red: Ethernet + WiFi

- La pestaña **WiFi** pasa a llamarse **Red** (icono híbrido Ethernet+WiFi).
- Nueva sección **Enlaces**: todas las interfaces ethernet/wifi con IP y badge de **ruta por defecto**.
- Gestión de perfiles **Ethernet** (conectar / desconectar) vía NetworkManager.
- El bloque WiFi (conexión, guardadas, escaneo) se mantiene debajo.
- U-STATUS `ip` / `info` listan las IPs por interfaz (`eth0=…* wlan0=…`; `*` = ruta por defecto).

## v0.3.43 — Dual Carrier: bordes de mini-tiles + promoción estable

- Marcos de Carrier / TX / RX / Duplex dentro de Dual Carrier un poco más oscuros (mejor contraste).
- Dual Carrier GUI (v0.3.38–0.3.43) promovido a canal **stable** (`main`).

## v0.3.42 — Dual Carrier: mismo fondo que el resto

- La tarjeta Dual Carrier usa el mismo fondo plano (tema light) que Registration Access / tiles BTS.

## v0.3.41 — Dual Carrier: una sola tarjeta

- Carrier / TX / RX / shift del secondary van **dentro** de la tarjeta Dual Carrier (sin caja aparte).
- Estado: **Activo** (verde negrita) / **Apagado** (naranja negrita); se quita el texto «carrier secundario #…».

## v0.3.40 — Dual Carrier UI: orden TS + mini-tiles

- En BTS Details, la fila **MCCH (main)** va arriba y el **BCCH secondary** debajo.
- Las mini-tiles del secondary (carrier, TX, RX, shift) aparecen **bajo Dual Carrier** cuando está activo (Home ya las rellena).

## v0.3.39 — Fix OTA: encoding en server.rs

- Corrige literales UTF-8 corruptos en el detector de mojibake del dashboard que impedían compilar `tetra-entities` en OTA (v0.3.38).

## v0.3.38 — Dual Carrier en TMO Cell (GUI)

- Dual Carrier se configura en **Config → Advanced RF** (checkbox bajo Main carrier; el secondary solo aparece si está ON).
- El secondary se **limita al passband** de la Fs real del SDR (o 600 kHz por defecto); al activar se guardan Fs + centros midway en TOML y perfil Cell.
- En **TETRA BTS Details**: botón «Configurar…» (sin switch) y **mini-tiles** del secondary (nº, TX, RX, shift) cuando está activo.

## v0.3.37 — Overflow mid-report → D-ATTACH inmediato

- Si el U-ATTACH trae *group report not complete* y >12 GSSI (MXP600), tras el ACK de 12 se afilia el resto con **D-ATTACH SwMI** en el acto (no se pide otro group report).
- Si el amendment siguiente llega truncado (`BufferEnded` al parsear), se usa el resto guardado del PDU anterior para el mismo D-ATTACH.

## v0.3.36 — Fallback SwMI attach si el MS no multipasa

- Tras group report (§16.8.3), si el MXP600 vuelve a mandar >12 GSSI en un solo U-ATTACH (no hace amendment multipaso), la BTS afilia el resto y envía **D-ATTACH amend** SwMI (§16.8.1) con esos GSSI. IOP puede ignorarlo; si sigue en 12, limitar scan a ≤12.

## v0.3.35 — Afiliación multipaso ETSI (§16.8.3)

- Si un U-ATTACH trae más de **12** GSSI (p. ej. scan list MXP600), tras el ACK de los 12 la BTS pide **group report** SwMI (EN 300 392-2 §16.8.3) para que el MS re-afilie en varios mensajes (detach-all + amendments).
- Log corregido: ya no dice que el MS reintentará solo. Validar en aire: scan >12 → más de 12 en «Grupos afiliados». Si el MS no multipasa tras el report, limitar scan a ≤12.

## v0.3.34 — Restart recovery en Config

- Interruptor **Restart recovery (proactive)** en Advanced network/timers (config en vivo + perfil TMO Cell), con ayuda «?». Default **off**. Tras Aplicar y reiniciar con el check activo, la BTS re-registra ISSIs cacheados sin tocar el walkie. La recuperación reactiva (al PTT/TG) sigue ON en el motor.

## v0.3.33 — Pending-tail quiet 400 ms

- Ajuste fino: quiet post-drenado Brew **400 ms** (antes 550) tras `GROUP_IDLE`.

## v0.3.32 — Brew: no cortar la última sílaba al soltar PTT remoto

- Tras `GROUP_IDLE` se **aplaza siempre** el `NetworkCallEnd` (aunque el jitter Brew esté vacío) para que UMAC termine de radiar los últimos TCH.
- Quiet post-drenado ~**400 ms** (ajustado en 0.3.33; era 550 / antes 150); evita que el hangtime “se trague” la cola y la suelte al abrir el siguiente PTT.

## v0.3.31 — Brew late entry: audio DL + Hold al salir del TG

- **Audio Brew:** las llamadas de red nuevas abren el circuito en **SwMI** (antes LocalLoopback, pensado para LST; el audio remoto no salía al aire).
- **Cambio de TG mid-QSO:** si el último walkie deja el GSSI, la sesión Brew se **retiene** (Hold) en lugar de End; al volver a afiliar se remonta + D-SETUP.

## v0.3.30 — Fix OTA compile (late entry)

- Visibilidad de `push_control` entre módulos CMCE + match exhaustivo en UMAC para los nuevos SAP de late entry (build release fallaba en 0.3.29).

## v0.3.29 — Late entry usable (QSO a medias)

- **Brew:** si llega GROUP_TX sin walkies afiliados, la llamada se **retiene** (pending) en lugar de tirarse; al primer Affiliate se monta circuito + D-SETUP + audio.
- **Affiliate / cambio de TG:** D-SETUP inmediato si ya hay QSO en ese GSSI (no esperar ~5 s).
- **Despacho LST:** al afiliar un TG con QSO activo, la consola engancha RX (`rx_gssi` verde + audio) sin textos nuevos.
- **`late_entry_supported`** por defecto **true** en `[cell_info]`; checkbox en Config (celda Advanced) junto a System-wide services.

## v0.3.28 — Site trunking suave (estilo DIMETRA / TIP)

- Tras un blip de Brew **ya no** se expulsa a todos los walkies con `D-LOCATION-UPDATE-COMMAND`.
- Al reconectar: resync de suscriptores al core (REGISTER/AFFILIATE); COMMAND solo **bajo demanda** a un ISSI si falla un setup vía Brew en la ventana de soft-recovery.
- Histéresis de backhaul (default **3 s**): blips cortos de 4G/5G no cambian el menú “solo área local”. Las llamadas Brew se liberan al instante; los grupos locales en la celda siguen.
- Nuevo `[brew] backhaul_hysteresis_secs` (0..=60).

## v0.3.27 — TetraPack: no re-registro en la primera GROUP_TX

- Si el core no anuncia versión en el handshake (TetraPack), la primera llamada con mnemonic ya no dispara `BrewReconnected` / `D-LOCATION-UPDATE-COMMAND`.
- Ese barrido solo ocurre tras un disconnect→reconnect real del backhaul (sigue cubriendo PTT denegado tras blip).

## v0.3.26 — OTA: RF OFF al empezar

- Al iniciar una actualización OTA se apaga el SDR de inmediato (antes de compilar), para que las radios pierdan la celda limpiamente. El reinicio final vuelve a abrir RF desde config.

## v0.3.25 — Modal Ubicación LIP (móvil)

- En cada radio solo queda el botón **Centrar** (se quita el texto “Centrar todos” duplicado en la tarjeta).
- El botón superior **Centrar todos** va centrado, más grande y en negrita.
- Título del modal alineado en vertical con el botón de cerrar (móvil y PC).

## v0.3.24 — Fix OTA: préstamo en lip_forward_issi

- Corrige E0716 en `brew_routable` (temporary dropped while borrowed) que bloqueaba el build OTA de 0.3.23.

## v0.3.23 — Reenvío LIP → Brew + dashboard estable

- **Brew:** en Advanced (perfil y ajustes en vivo), **Reenvío de LIP** + **ISSI de destino** justo bajo RSSI export. Cada LIP UL (PID 10) se reenvía a ese ISSI por Brew, digan lo que digan las radios.
- **Dashboard:** el WebSocket ya no ocupa un slot del tope de 32 conexiones HTTP; evita que, pasado un rato, API/WS fallen con timeout y haya que reiniciar.

## v0.3.22 — Ubicación LIP: Centrar todos solo donde toca

- Se quita **Centrar todos** del encabezado del modal.
- En escritorio sigue en la cabecera de la columna de acciones; en móvil, entre el mapa y las tarjetas.
- El **Centrar** por radio no cambia.

## v0.3.21 — Audio LST adaptativo + Geo sin fugas

- **PCM DL:** solo con despacho tomado. En llamada/RX activo vuelve a **80 ms** (latencia); en idle baja a **500 ms**.
- **Ubicación LIP:** al cerrar el modal se destruye el mapa Leaflet (deja de pedir tiles OSM) y no se vuelve a consultar `/api/lst/positions` hasta reabrir.
- Modal Geo: se elimina la barra redundante (Centrar todos + estado ISSI); **Centrar** / **Centrar todos** centrados en la columna de acciones (en móvil, Centrar todos pasa al encabezado).

## v0.3.20 — Dashboard más ligero (Pi)

- Tope de **32** conexiones HTTP(S) concurrentes: evita que el poll agresivo / reintentos tumben el proceso (ERR_CONNECTION_RESET).
- Menos re-renders por RSSI (debounce 250 ms); timers de timeslots 150→250 ms.
- Callsigns / LST status / Geo / service: no martillean la API si la pestaña está en segundo plano o el enlace está caído.

## v0.3.19 — Modal Ubicación LIP

- Título **Ubicación LIP** (antes Geo LIP).
- **Centrar todos** pasa al encabezado de la columna de acciones; en móvil se muestra encima de la tabla (el thead se apila).
- Se elimina Refresh (el modal ya refresca cada 5 s).
- Cierre (×) ya no se superpone con el separador del título en móvil.

## v0.3.18 — Filtro por tipo en Registro SDS

- Selector Todos / LIP / Texto / Estado / Concat / Home / Otros junto a Exportar.
- Se elimina Actualizar: el log se carga al abrir la pestaña y llega en vivo por WebSocket.

## v0.3.17 — Ubicación en Inicio (sin LST)

- Botón **Ubicación** en la tarjeta Radios registrados (Inicio): mismo modal Geo LIP que en LST.
- El almacén de posiciones LIP vive en el dashboard (no depende del perfil LST Dispatch).

## v0.3.16 — Botón Ubicación en roster LST

- El botón **Ubicación** pasa a la tarjeta Radios online (sustituye el Refresh manual, redundante con el poll automático).

## v0.3.15 — Marcador Geo LIP

- Pin de mapa propio (CSS, color accent del dashboard); ya no depende de las PNG rotas de Leaflet/CDN.
- Indicativo RadioID correcto en la tabla Geo.

## v0.3.14 — Geo LIP en despacho LST

- Las posiciones LIP decodificadas (SDS PID 10, UL) alimentan el almacén LST (`note_position`).
- Botón **Geo** en la consola LST: modal con tabla + mapa OpenStreetMap (Leaflet lazy, solo al abrir).
- Se ignoran coords 0,0 (handshake de inicialización). Requiere perfil LST Dispatch activo.

## v0.3.13 — LIP decode + Miura restante

- **LIP:** se decodifican informes cortos (SDS PID 10) a `LIP position: lat, lon` (ETSI TS 100 392-18-1). GeoAlarm/Telegram y el log SDS dejan de ver el payload vacío.
- **Miura (resto):** `mon_pattern` / MPN 1 en channel allocation (PTT largo Sepura); D-RELEASE también por FACCH en el timeslot de tráfico.

## v0.3.12 — Puente OTA hacia PTBS

**Haz OTA una vez** (canal Estable o Beta). Esta versión prepara la migración al producto **PTBS** (*Personal Tetra Base Station*).

- Canal **Estable** pasa a seguir la rama git **`main`** (antes `bost`). La rama `bost` sigue recibiendo este puente para que las instalaciones actuales puedan actualizar.
- OTA reconoce checkout `/opt/ptbs` (preferente) y `/opt/bost-flowstation` (legacy).
- Al instalar el binario se refresca también `/usr/local/bin/ptbs` junto a `bluestation-bs`.
- Anuncio de rebrand en README y mensajes OTA. La marca en UI sigue siendo Bost FlowStation hasta el corte **0.4.0**.
- Ventana de migración: mantén el equipo actualizado; en unos días el repo/ramas/binario completarán el rename a PTBS.

## v0.3.1

- **Grupos afiliados:** el panel del chevron (›) ya no desaparece al instante. Se queda abierto hasta cerrarlo (×, clic fuera, Escape o de nuevo el chevron). Antes lo cerraban el refresco del roster, el scroll y un timer de 3,5 s; el `title` nativo del botón también confundía en móvil.

## v0.3.0

Lanzamiento estable (canal OTA **Estable** / rama `bost`). Consolida el trabajo de la línea 0.2.4–0.2.41: despacho LST en producción, preempt de PTT, dashboard HTTPS canónico y correcciones de campo.

### Actualización desde 0.2.x (estable)

- OTA a **Estable** / `bost` o reinstalar con `install-bost.sh` (no sobrescribe `config.toml` existente).
- Al arrancar, si el dashboard seguía en puertos antiguos (`port = 8080`), se migra a `port = 80` + `https_port = 443`. Abre **`https://<IP>/`**.
- Redirecciones silenciosas en `:8080` / `:8443` se mantienen solo por compatibilidad de marcadores viejos; **instalación e interfaz ya no anuncian esos puertos**.
- Codec de voz LST: si falta, el OTA ofrece rebuild con libtetra-codec (sin SSH).

### Despacho LST (consola local)

- Consola bajo Integraciones: claim de sesión, ISSI despachador, lista de escaneo multi-TG, PTT (ratón/táctil/espacio), SDS, roster, actividad e inbox SDS.
- Llamadas privadas simplex/dúplex (salientes y entrantes); modal/franja de llamada.
- Audio ACELP vía codec OTA; dashboard canónico en HTTPS `:443` (HTTP `:80` redirige).
- **Preempt / interrupción:** PTT LST puede quitar el suelo a un MS local (deny → oferta → Ready al UL quiet); sin teardown de circuito; sin flicker de display Motorola tras Ready.
- Tras PTT de grupo, el dial privado SX/DX ya no queda bloqueado como “Establecida” con el GSSI.
- Modal de llamada entrante solo en el navegador que tiene el despacho tomado (no molesta a otros agentes del dashboard).

### Dashboard / Config / red

- Dashboard HTTPS `:443` + redirect HTTP `:80` (instalador y docs alineados).
- Ayuda «?» en Config (timers TETRA/Brew); whitelist ISSI en perfil Cell; fix TOML con 2+ ISSIs (evita arranque en fallback).
- Wi‑Fi resiliencia (NM drop-in, autoconnect, watchdog) desde 0.2.2–0.2.3.
- Pulidos UX LST/móvil, iconos de navegación, perfiles Cell × Brew.

### Limpieza en 0.3.0

- Eliminado el botón “Abrir consola segura (HTTPS)” del despacho (la UI ya sirve en HTTPS).
- Mensajes de instalador / README / example_config sin publicar `:8080` / `:8443`.

## v0.2.7

- **LST Dispatch:** admit group `NetworkCallStart` without Brew (inbound gate). Join solo selecciona GSSI; PTT abre/cierra la llamada. SDS usa el ISSI del despacho (`source_issi` / `dest_is_group`). Privadas: media ready, duplex UL, errores visibles.

## v0.2.6

- Fix LST PTT borrow-checker errors so `tetra-entities` compiles on OTA.

## v0.2.5

- Fix OTA build: import `CfgLstDispatch` / `apply_lst_dispatch_patch` in tetra-config.

## v0.2.4

- **Despacho LST / LST Dispatch:** consola local bajo Integraciones (mic/altavoz del navegador). Perfil Brew inmutable “Despacho LST”, 1 sesión, XOR con Brew real. Grupo + privadas simplex/dúplex + SDS + roster; codec de voz si el build incluye `asterisk`.

## v0.2.3

- OTA / arranque aplican solos el drop-in NetworkManager `bost-wifi.conf` (powersave off): **no hace falta SSH**.

## v0.2.2

- **WiFi resiliencia:** Disconnect usa `connection down` (ya no inhibe autoconnect). Al conectar se fuerza autoconnect + powersave off en el perfil. Watchdog ligero re-sube un perfil guardado si el enlace cae con la radio WiFi encendida.
- Instalador: drop-in NetworkManager `bost-wifi.conf` (`wifi.powersave=2`). Docs de comprobación en install-and-setup.

## v0.2.1

- Config U-STATUS: en PC los comandos vuelven a una sola fila (código | acción | Quitar); el layout móvil compacto no cambia.

## v0.2.0

Lanzamiento estable con cambios de producto (no solo parches). Canal OTA **Estable** (`bost`).

- **Dashboard móvil:** shell, System/OTA/Setup, tablas, RF/Health, Config, DGNA/Geoalarm/Wi‑Fi e integraciones usables en teléfono (PC sin cambios de layout salvo lo acordado).
- **OTA más robusto:** modal al instante, checks coalescidos y en caché, purge de `.git` vacío, mensajes claros, espera post-reinicio que exige “caída → subida” y lleva a login cuando hay auth (evita SPA “fuera de línea”).
- **Config / perfiles:** timers de Advanced network con reset a default (vacío = default del motor); Live y perfiles Cell comparten la misma semántica de persistencia.
- **Home:** perfiles rápidos Cell × Brew; pulidos de UI (Save al pie, mensajes vacíos, U-STATUS compacto, etc.).

## v0.1.57

- Instalador alineado con OTA: fetch con refspec + reintentos, `reset --hard`, y `ota_channel` según `BOST_BRANCH`.
- README / docs de instalación actualizados (comando curl sigue siendo siempre desde rama `bost`).

## v0.1.56

- OTA: reintentos de `git fetch` ante cortes TLS/red (p. ej. GnuTLS en Pi).

## v0.1.55

- Paso Progreso OTA: estado superior corto, tip distinto bajo la barra y tiempo transcurrido en negrita.

## v0.1.54

- Novedades humanas en la pantalla de actualización (CHANGELOG / Releases).
- El banner de “actualización disponible” abre directamente el resumen de cambios.
- Modal OTA con indicador de pasos más claro (canal → novedades → progreso).

## v0.1.53

- Corrección de compilación en el helper de permisos OTA (`append` / `&str`).

## v0.1.52

- Tras sincronizar el código como root, se ajusta la propiedad de todo el árbol de fuentes
  (no solo `target/`) para que `cargo` como usuario `bts` no falle en `Cargo.lock`.

## v0.1.51

- Al cambiar de canal (p. ej. a Beta), el fetch crea correctamente `origin/<rama>`
  para que la actualización no falle con “unknown revision”.

## v0.1.50

- Diálogo OTA en tres pasos: elegir canal, ver novedades y confirmar, luego progreso.
- El selector de canal se guarda y sigue alimentando el badge / banner automático.

## v0.1.49

- Corrección de un error de compilación en la configuración del canal OTA.

## v0.1.48

- Canales OTA **Estable** (`bost`) y **Beta** (`beta`).
- Sincronización segura con `git reset --hard` (recupera force-push) manteniendo `target/`
  para builds incrementales.
- Si el binario ya coincide con HEAD, no se recompila ni se reinicia en falso.
