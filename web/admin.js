const TOKEN_KEY = 'cs16_admin_token';

const els = {
  tokenInput: document.getElementById('token-input'),
  rememberSession: document.getElementById('remember-session'),
  btnLogin: document.getElementById('btn-login'),
  btnLogout: document.getElementById('btn-logout'),
  refreshSelect: document.getElementById('refresh-select'),
  btnRefreshNow: document.getElementById('btn-refresh-now'),
  authBadge: document.getElementById('auth-badge'),
  refreshBadge: document.getElementById('refresh-badge'),

  stHostname: document.getElementById('st-hostname'),
  stMap: document.getElementById('st-map'),
  stPlayers: document.getElementById('st-players'),
  stScore: document.getElementById('st-score'),
  stPorts: document.getElementById('st-ports'),
  stUptime: document.getElementById('st-uptime'),
  statusTick: document.getElementById('status-tick'),

  mapSelect: document.getElementById('map-select'),
  mapCustom: document.getElementById('map-custom'),
  btnChangelevel: document.getElementById('btn-changelevel'),
  botQuota: document.getElementById('bot-quota'),
  botDiff: document.getElementById('bot-diff'),
  btnApplyBots: document.getElementById('btn-apply-bots'),
  btnAddT: document.getElementById('btn-add-t'),
  btnAddCt: document.getElementById('btn-add-ct'),
  btnKickBots: document.getElementById('btn-kick-bots'),
  btnRestartRound: document.getElementById('btn-restart-round'),

  invHost: document.getElementById('inv-host'),
  invPort: document.getElementById('inv-port'),
  invRoom: document.getElementById('inv-room'),
  btnBuildInvite: document.getElementById('btn-build-invite'),
  invUrl: document.getElementById('inv-url'),
  btnCopyInvite: document.getElementById('btn-copy-invite'),
  invConnect: document.getElementById('inv-connect'),
  btnCopyConnect: document.getElementById('btn-copy-connect'),
  invFeedback: document.getElementById('inv-feedback'),

  playersCount: document.getElementById('players-count'),
  playersBody: document.getElementById('players-body'),

  rconOutput: document.getElementById('rcon-output'),
  rconForm: document.getElementById('rcon-form'),
  rconInput: document.getElementById('rcon-input'),
  btnRconClear: document.getElementById('btn-rcon-clear'),

  replaysMeta: document.getElementById('replays-meta'),
  replaysBody: document.getElementById('replays-body'),

  replay2dModal: document.getElementById('replay2d-modal'),
  replay2dTitle: document.getElementById('replay2d-title'),
  replay2dStatus: document.getElementById('replay2d-status'),
  replay2dClose: document.getElementById('replay2d-close'),
  replay2dCanvas: document.getElementById('replay2d-canvas'),
  replay2dScrubber: document.getElementById('replay2d-scrubber'),
  replay2dCtList: document.getElementById('replay2d-ct-list'),
  replay2dTList: document.getElementById('replay2d-t-list'),
  replay2dPlay: document.getElementById('replay2d-play'),
  replay2dRestart: document.getElementById('replay2d-restart'),
  replay2dTime: document.getElementById('replay2d-time'),

  statsSummary: document.getElementById('stats-summary'),
  statsBody: document.getElementById('stats-body'),

  tabBtnDashboard: document.getElementById('tab-btn-dashboard'),
  tabBtnMods: document.getElementById('tab-btn-mods'),
  tabPaneDashboard: document.getElementById('tab-pane-dashboard'),
  tabPaneMods: document.getElementById('tab-pane-mods'),

  modsSummary: document.getElementById('mods-summary'),
  modsBody: document.getElementById('mods-body'),
  btnModsReloadAll: document.getElementById('btn-mods-reload-all'),
  btnModsRefresh: document.getElementById('btn-mods-refresh'),

  faultsSummary: document.getElementById('faults-summary'),
  faultsBody: document.getElementById('faults-body'),

  studioFileSelect: document.getElementById('studio-file-select'),
  studioActiveFile: document.getElementById('studio-active-file'),
  studioEditor: document.getElementById('studio-editor'),
  studioStatus: document.getElementById('studio-status'),
  btnStudioLoad: document.getElementById('btn-studio-load'),
  btnStudioSave: document.getElementById('btn-studio-save'),
};

let adminToken = '';
let refreshTimer = null;
const rconHistory = [];
let historyCursor = -1;
let studioCurrentFile = '';
let studioFilesCache = [];

function setAuthState(ok, label) {
  if (!els.authBadge) return;
  els.authBadge.textContent = label;
  els.authBadge.className = ok ? 'badge ok' : 'badge err';
}

function setRefreshBadge(text, isErr = false) {
  if (!els.refreshBadge) return;
  els.refreshBadge.textContent = text;
  els.refreshBadge.className = isErr ? 'badge err' : 'badge';
}

function formatDuration(secs) {
  const s = Math.max(0, Math.floor(Number(secs) || 0));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const rem = s % 60;
  if (h > 0) return `${h}h ${m}m ${rem}s`;
  if (m > 0) return `${m}m ${rem}s`;
  return `${rem}s`;
}

function formatBytes(n) {
  const bytes = Math.max(0, Number(n) || 0);
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(2)} MiB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${bytes} B`;
}

function makeCell(text, className = '') {
  const td = document.createElement('td');
  td.textContent = String(text ?? '');
  if (className) td.className = className;
  return td;
}

function setEmptyRow(tbody, cols, msg) {
  if (!tbody) return;
  tbody.replaceChildren();
  const tr = document.createElement('tr');
  const td = document.createElement('td');
  td.colSpan = cols;
  td.className = 'empty-cell';
  td.textContent = msg;
  tr.appendChild(td);
  tbody.appendChild(tr);
}

async function apiFetch(path, options = {}) {
  if (!adminToken) {
    throw new Error('Not authenticated');
  }
  const headers = new Headers(options.headers || {});
  headers.set('Authorization', `Bearer ${adminToken}`);
  const resp = await fetch(path, { ...options, headers });
  if (!resp.ok) {
    let errText = `HTTP ${resp.status}`;
    try {
      const data = await resp.json();
      if (data && data.error) errText = data.error;
    } catch {
      // ignore non-JSON error body
    }
    if (resp.status === 401 || resp.status === 403) {
      setAuthState(false, 'Auth Failed');
    }
    throw new Error(errText);
  }
  return resp;
}

async function apiJson(path, options = {}) {
  const resp = await apiFetch(path, options);
  return resp.json();
}

function appendTerminal(line) {
  if (!els.rconOutput) return;
  const current = els.rconOutput.textContent || '';
  const next = current ? `${current}\n${line}` : line;
  const lines = next.split('\n');
  els.rconOutput.textContent = lines.slice(-300).join('\n');
  els.rconOutput.scrollTop = els.rconOutput.scrollHeight;
}

async function runRcon(command) {
  const trimmed = String(command || '').trim();
  if (!trimmed) return null;
  appendTerminal(`] ${trimmed}`);
  try {
    const res = await apiJson('/api/rcon', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ command: trimmed }),
    });
    if (res.output) {
      appendTerminal(String(res.output).replace(/\n+$/, ''));
    } else {
      appendTerminal('(ok)');
    }
    return res;
  } catch (err) {
    appendTerminal(`! error: ${err.message}`);
    throw err;
  }
}

function renderStatus(st) {
  if (!st) return;
  if (els.stHostname) els.stHostname.textContent = st.hostname || '-';
  if (els.stMap) els.stMap.textContent = st.map || '-';
  if (els.stPlayers) {
    els.stPlayers.textContent = `${st.players ?? 0} / ${st.max_players ?? 16} (${st.bots ?? 0} bots)`;
  }
  if (els.stScore) {
    els.stScore.replaceChildren();
    const roundSpan = document.createTextNode(`R${st.round_number ?? 0} — `);
    const ctSpan = document.createElement('span');
    ctSpan.className = 'score-ct';
    ctSpan.textContent = `CT ${st.score_ct ?? 0}`;
    const sep = document.createTextNode(' : ');
    const tSpan = document.createElement('span');
    tSpan.className = 'score-t';
    tSpan.textContent = `T ${st.score_t ?? 0}`;
    els.stScore.append(roundSpan, ctSpan, sep, tSpan);
  }
  if (els.stPorts) {
    const web = st.web_port != null ? st.web_port : 'off';
    els.stPorts.textContent = `${st.game_port ?? 27015} UDP / ${web} TCP`;
  }
  if (els.stUptime) els.stUptime.textContent = formatDuration(st.uptime_secs);
  if (els.statusTick) els.statusTick.textContent = `${st.tick_rate ?? 100} Hz`;

  if (els.botQuota && document.activeElement !== els.botQuota && st.bot_quota != null) {
    els.botQuota.value = String(st.bot_quota);
  }
  if (els.botDiff && document.activeElement !== els.botDiff && st.bot_difficulty != null) {
    els.botDiff.value = String(st.bot_difficulty);
  }
  if (els.invPort && document.activeElement !== els.invPort && st.web_port != null) {
    els.invPort.value = String(st.web_port);
  }

  if (els.mapSelect && Array.isArray(st.available_maps) && st.available_maps.length > 0) {
    const prev = els.mapSelect.value;
    els.mapSelect.replaceChildren();
    for (const m of st.available_maps) {
      const opt = document.createElement('option');
      opt.value = m;
      opt.textContent = m;
      if (m === st.map || m === prev) opt.selected = true;
      els.mapSelect.appendChild(opt);
    }
  }
}

function renderPlayers(data) {
  const list = Array.isArray(data?.players) ? data.players : [];
  if (els.playersCount) {
    els.playersCount.textContent = `${list.length} connected`;
  }
  if (!els.playersBody) return;
  if (list.length === 0) {
    setEmptyRow(els.playersBody, 10, 'No players currently connected.');
    return;
  }

  const frag = document.createDocumentFragment();
  for (const p of list) {
    const tr = document.createElement('tr');
    tr.append(
      makeCell(p.slot),
      makeCell(`#${p.userid}`),
      makeCell(p.name),
      makeCell(p.team || '-'),
      makeCell(p.frags ?? 0),
      makeCell(p.deaths ?? 0),
      makeCell(`${p.ping_ms ?? 0}ms`),
      makeCell(p.state || '-'),
      makeCell(p.is_bot ? 'BOT' : (p.addr || '-'), 'mono')
    );

    const actionTd = document.createElement('td');
    const kickBtn = document.createElement('button');
    kickBtn.textContent = 'Kick';
    kickBtn.addEventListener('click', async () => {
      await runRcon(`kick #${Number(p.userid)} "Kicked by web admin"`);
      await refreshAll();
    });

    const banBtn = document.createElement('button');
    banBtn.textContent = 'Ban';
    banBtn.className = 'danger';
    banBtn.style.marginLeft = '4px';
    banBtn.disabled = Boolean(p.is_bot);
    banBtn.addEventListener('click', async () => {
      const minsRaw = window.prompt(`Ban ${p.name} for how many minutes? (0 = permanent)`, '30');
      if (minsRaw === null) return;
      const mins = Math.max(0, Number.parseInt(minsRaw, 10) || 0);
      await runRcon(`banid ${mins} #${Number(p.userid)} kick`);
      await refreshAll();
    });

    actionTd.append(kickBtn, banBtn);
    tr.appendChild(actionTd);
    frag.appendChild(tr);
  }
  els.playersBody.replaceChildren(frag);
}

function renderStats(data) {
  const list = Array.isArray(data?.players) ? data.players : [];
  if (els.statsSummary) {
    els.statsSummary.textContent = `Map: ${data?.map || '-'} | Round ${data?.current_round ?? 0} | CT ${data?.score_ct ?? 0} : ${data?.score_t ?? 0} T`;
  }
  if (!els.statsBody) return;
  if (list.length === 0) {
    setEmptyRow(els.statsBody, 14, 'No match statistics recorded yet.');
    return;
  }

  const frag = document.createDocumentFragment();
  list.forEach((p, idx) => {
    const tr = document.createElement('tr');
    tr.append(
      makeCell(idx + 1),
      makeCell(p.name),
      makeCell(p.team || '-'),
      makeCell(p.kills ?? 0),
      makeCell(p.deaths ?? 0),
      makeCell(Number(p.kd_ratio ?? 0).toFixed(2)),
      makeCell(`${Number(p.hs_percent ?? 0).toFixed(1)}%`),
      makeCell(p.assists ?? 0),
      makeCell(p.damage ?? 0),
      makeCell(Number(p.adr ?? 0).toFixed(1)),
      makeCell(p.plants ?? 0),
      makeCell(p.defuses ?? 0),
      makeCell(p.hostage_rescues ?? 0),
      makeCell(p.mvps ?? 0)
    );
    frag.appendChild(tr);
  });
  els.statsBody.replaceChildren(frag);
}

async function downloadReplayDem(id) {
  const resp = await apiFetch(`/api/replays/${encodeURIComponent(id)}.dem`);
  const blob = await resp.blob();
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = `${id}.dem`;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

function renderReplays(data) {
  const rounds = Array.isArray(data?.rounds) ? data.rounds : [];
  if (els.replaysMeta) {
    els.replaysMeta.textContent = `${rounds.length} rounds (${formatBytes(data?.total_bytes ?? 0)})`;
  }
  if (!els.replaysBody) return;
  if (rounds.length === 0) {
    setEmptyRow(els.replaysBody, 8, 'No recorded round .dem replays yet.');
    return;
  }

  const frag = document.createDocumentFragment();
  for (const r of rounds) {
    const tr = document.createElement('tr');
    tr.append(
      makeCell(`#${r.round_number}`),
      makeCell(r.map),
      makeCell(r.winner),
      makeCell(r.reason),
      makeCell(`${r.score_ct ?? 0}:${r.score_t ?? 0}`),
      makeCell(`${Number(r.duration_secs ?? 0).toFixed(1)}s`),
      makeCell(formatBytes(r.byte_len ?? 0))
    );
    const actionTd = document.createElement('td');
    const watchBtn = document.createElement('button');
    watchBtn.textContent = 'Watch 2D Replay';
    watchBtn.className = 'primary';
    watchBtn.disabled = r.has_timeline === false;
    watchBtn.addEventListener('click', () => {
      open2dReplay(r).catch((err) => appendTerminal(`! 2D replay failed: ${err.message}`));
    });

    const dlBtn = document.createElement('button');
    dlBtn.textContent = 'Download .dem';
    dlBtn.style.marginLeft = '4px';
    dlBtn.addEventListener('click', () => {
      downloadReplayDem(r.id).catch((err) => appendTerminal(`! download failed: ${err.message}`));
    });
    actionTd.append(watchBtn, dlBtn);
    tr.appendChild(actionTd);
    frag.appendChild(tr);
  }
  els.replaysBody.replaceChildren(frag);
}

function setStudioStatus(text, kind = '') {
  if (!els.studioStatus) return;
  els.studioStatus.textContent = text;
  els.studioStatus.className = kind ? `studio-status ${kind}` : 'studio-status';
}

function switchAdminTab(tab) {
  const isMods = tab === 'mods';
  els.tabBtnDashboard?.classList.toggle('active', !isMods);
  els.tabBtnMods?.classList.toggle('active', isMods);
  els.tabBtnDashboard?.setAttribute('aria-selected', String(!isMods));
  els.tabBtnMods?.setAttribute('aria-selected', String(isMods));
  els.tabPaneDashboard?.classList.toggle('hidden', isMods);
  els.tabPaneMods?.classList.toggle('hidden', !isMods);
  if (isMods && adminToken) {
    refreshModsStudio().catch((err) => setStudioStatus(`Sync error: ${err.message}`, 'err'));
  }
}

async function toggleModAction(id, action) {
  try {
    const data = await apiJson(`/api/mods/${encodeURIComponent(id)}/toggle`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ action }),
    });
    setStudioStatus(`${id}: ${data.message || action}`, 'ok');
    await refreshModsStudio();
  } catch (err) {
    setStudioStatus(`Failed to ${action} ${id}: ${err.message}`, 'err');
  }
}

async function downloadFaultDump(id) {
  const resp = await apiFetch(`/api/mods/faults/${encodeURIComponent(id)}.json`);
  const blob = await resp.blob();
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = `${id}.json`;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

function renderModsResmon(data) {
  const mods = Array.isArray(data?.mods) ? data.mods : [];
  const editableFiles = Array.isArray(data?.editable_files) ? data.editable_files : [];
  const activeCount = mods.filter((m) => m.status === 'Active').length;
  const faultedCount = mods.filter((m) => String(m.status || '').startsWith('Faulted')).length;
  if (els.modsSummary) {
    els.modsSummary.textContent = `${mods.length} plugins (${activeCount} active, ${faultedCount} faulted)`;
  }

  if (els.studioFileSelect) {
    const key = editableFiles.join('\n');
    if (key !== studioFilesCache.join('\n')) {
      studioFilesCache = editableFiles;
      const prev = els.studioFileSelect.value || studioCurrentFile;
      els.studioFileSelect.replaceChildren();
      const placeholder = document.createElement('option');
      placeholder.value = '';
      placeholder.textContent = editableFiles.length > 0
        ? `Select a file (${editableFiles.length} editable)...`
        : 'No editable .py / configs/*.toml files found';
      els.studioFileSelect.appendChild(placeholder);
      for (const file of editableFiles) {
        const opt = document.createElement('option');
        opt.value = file;
        opt.textContent = file;
        els.studioFileSelect.appendChild(opt);
      }
      if (prev && editableFiles.includes(prev)) {
        els.studioFileSelect.value = prev;
      }
    }
  }

  if (!els.modsBody) return;
  if (mods.length === 0) {
    setEmptyRow(els.modsBody, 10, 'No plugins loaded on this server.');
    return;
  }

  const sorted = [...mods].sort((a, b) => (Number(b.avg_us_per_tick) || 0) - (Number(a.avg_us_per_tick) || 0));
  const frag = document.createDocumentFragment();
  for (const m of sorted) {
    const tr = document.createElement('tr');
    const avgUs = Number(m.avg_us_per_tick || 0);
    const p99Us = Number(m.p99_us_per_tick || 0);
    const maxUs = Number(m.max_us || 0);
    const faults = Number(m.fault_count || 0);
    const isFaulted = String(m.status || '').startsWith('Faulted');
    const isPaused = m.status === 'Paused';

    tr.appendChild(makeCell(m.id, 'mono'));

    const langTd = document.createElement('td');
    const langSpan = document.createElement('span');
    langSpan.className = 'lang-tag';
    langSpan.textContent = m.language || 'WASM';
    langTd.appendChild(langSpan);
    tr.appendChild(langTd);

    tr.appendChild(makeCell(m.version || '-', 'mono'));

    const stTd = document.createElement('td');
    const stBadge = document.createElement('span');
    stBadge.className = isFaulted ? 'badge err' : isPaused ? 'badge' : 'badge ok';
    stBadge.textContent = isFaulted ? 'Faulted' : (m.status || 'Active');
    if (isFaulted) stBadge.title = m.status;
    stTd.appendChild(stBadge);
    tr.appendChild(stTd);

    const cpuTd = document.createElement('td');
    const barWrap = document.createElement('div');
    barWrap.className = 'cpu-bar-wrap';
    const track = document.createElement('div');
    track.className = 'cpu-bar-track';
    const fill = document.createElement('div');
    const colorCls = avgUs < 50 ? 'cpu-green' : avgUs < 250 ? 'cpu-yellow' : 'cpu-red';
    fill.className = `cpu-bar-fill ${colorCls}`;
    fill.style.width = `${Math.max(4, Math.min(100, (avgUs / 300) * 100)).toFixed(0)}%`;
    track.appendChild(fill);
    const cpuLabel = document.createElement('span');
    cpuLabel.className = 'mono';
    cpuLabel.textContent = `${avgUs.toFixed(1)}us`;
    barWrap.append(track, cpuLabel);
    cpuTd.appendChild(barWrap);
    tr.appendChild(cpuTd);

    tr.appendChild(makeCell(`${p99Us} / ${maxUs}`, 'mono'));
    tr.appendChild(makeCell(formatBytes(m.memory_bytes || 0), 'mono'));
    tr.appendChild(makeCell(String(m.hook_calls || 0), 'mono'));

    const fTd = document.createElement('td');
    if (faults > 0) {
      const fb = document.createElement('span');
      fb.className = 'badge err';
      fb.textContent = String(faults);
      fTd.appendChild(fb);
    } else {
      fTd.textContent = '0';
      fTd.className = 'mono';
    }
    tr.appendChild(fTd);

    const actTd = document.createElement('td');
    const toggleBtn = document.createElement('button');
    toggleBtn.textContent = isPaused ? 'Resume' : 'Pause';
    toggleBtn.disabled = isFaulted;
    toggleBtn.addEventListener('click', () => {
      toggleModAction(m.id, isPaused ? 'unpause' : 'pause');
    });
    const reloadBtn = document.createElement('button');
    reloadBtn.textContent = 'Reload';
    reloadBtn.style.marginLeft = '4px';
    reloadBtn.disabled = m.language === 'AMXX';
    reloadBtn.addEventListener('click', () => {
      toggleModAction(m.id, 'reload');
    });
    actTd.append(toggleBtn, reloadBtn);

    if (m.source_path && editableFiles.includes(m.source_path)) {
      const editBtn = document.createElement('button');
      editBtn.textContent = 'Edit';
      editBtn.className = 'primary';
      editBtn.style.marginLeft = '4px';
      editBtn.addEventListener('click', () => {
        if (els.studioFileSelect) els.studioFileSelect.value = m.source_path;
        loadStudioFile(m.source_path);
      });
      actTd.appendChild(editBtn);
    }

    tr.appendChild(actTd);
    frag.appendChild(tr);
  }
  els.modsBody.replaceChildren(frag);
}

function renderModFaults(data) {
  const faults = Array.isArray(data?.faults) ? data.faults : [];
  if (els.faultsSummary) {
    els.faultsSummary.textContent = `${faults.length} dumps`;
  }
  if (!els.faultsBody) return;
  if (faults.length === 0) {
    setEmptyRow(els.faultsBody, 6, 'No plugin faults recorded.');
    return;
  }
  const frag = document.createDocumentFragment();
  for (const f of faults) {
    const dumpId = f.id || f.dump_id || '-';
    const tr = document.createElement('tr');
    tr.append(
      makeCell(dumpId, 'mono'),
      makeCell(f.mod_id || '-', 'mono'),
      makeCell(f.language || '-', 'mono'),
      makeCell(f.trigger || '-', 'mono'),
      makeCell(f.error || '-', 'mono')
    );
    const actTd = document.createElement('td');
    const dlBtn = document.createElement('button');
    dlBtn.textContent = 'Download .json';
    dlBtn.className = 'primary';
    dlBtn.addEventListener('click', () => {
      downloadFaultDump(dumpId).catch((err) => setStudioStatus(`Fault download failed: ${err.message}`, 'err'));
    });
    actTd.appendChild(dlBtn);
    tr.appendChild(actTd);
    frag.appendChild(tr);
  }
  els.faultsBody.replaceChildren(frag);
}

async function loadStudioFile(relPath) {
  const target = (relPath || els.studioFileSelect?.value || '').trim();
  if (!target) {
    setStudioStatus('Select a file first.', 'err');
    return;
  }
  try {
    const encodedPath = target.split('/').map(encodeURIComponent).join('/');
    const data = await apiJson(`/api/mods/source/${encodedPath}`);
    studioCurrentFile = data.path || target;
    if (els.studioActiveFile) els.studioActiveFile.textContent = studioCurrentFile;
    if (els.studioEditor) els.studioEditor.value = data.content || '';
    setStudioStatus(`Loaded ${studioCurrentFile} (${(data.content || '').length} bytes).`, 'ok');
  } catch (err) {
    setStudioStatus(`Failed to load ${target}: ${err.message}`, 'err');
  }
}

async function saveStudioFile() {
  const target = (studioCurrentFile || els.studioFileSelect?.value || '').trim();
  if (!target) {
    setStudioStatus('No file selected to save.', 'err');
    return;
  }
  const content = els.studioEditor?.value ?? '';
  const t0 = performance.now();
  try {
    const encodedPath = target.split('/').map(encodeURIComponent).join('/');
    const data = await apiJson(`/api/mods/source/${encodedPath}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ content }),
    });
    const elapsedMs = (performance.now() - t0).toFixed(1);
    setStudioStatus(`Saved & hot-reloaded ${target} in ${elapsedMs} ms — ${data.message || 'OK'}`, 'ok');
    await refreshModsStudio();
  } catch (err) {
    setStudioStatus(`Save/reload failed: ${err.message}`, 'err');
  }
}

async function refreshModsStudio() {
  if (!adminToken) return;
  const [mods, faults] = await Promise.all([
    apiJson('/api/mods'),
    apiJson('/api/mods/faults'),
  ]);
  renderModsResmon(mods);
  renderModFaults(faults);
}

async function refreshAll() {
  if (!adminToken) return;
  setRefreshBadge('Syncing...');
  try {
    const [status, players, stats, replays, mods, faults] = await Promise.all([
      apiJson('/api/status'),
      apiJson('/api/players'),
      apiJson('/api/stats'),
      apiJson('/api/replays'),
      apiJson('/api/mods'),
      apiJson('/api/mods/faults'),
    ]);
    setAuthState(true, 'Authenticated');
    renderStatus(status);
    renderPlayers(players);
    renderStats(stats);
    renderReplays(replays);
    renderModsResmon(mods);
    renderModFaults(faults);
    setRefreshBadge(`Synced ${new Date().toLocaleTimeString()}`);
  } catch (err) {
    setRefreshBadge(`Error: ${err.message}`, true);
  }
}

async function buildInvite() {
  const host = (els.invHost?.value || window.location.hostname || '127.0.0.1').trim();
  const port = Math.max(1, Math.min(65535, Number.parseInt(els.invPort?.value || '27016', 10) || 27016));
  const room = (els.invRoom?.value || 'alpha').trim() || 'alpha';
  const base = `${window.location.origin}/`;

  if (!adminToken) {
    const query = `?join=${encodeURIComponent(`${host}:${port}`)}&room=${encodeURIComponent(room)}`;
    if (els.invUrl) els.invUrl.value = `${base}${query}`;
    if (els.invConnect) els.invConnect.value = `connect ${host}:27015`;
    if (els.invFeedback) els.invFeedback.textContent = 'Built offline link (authenticate for server-verified ports).';
    return;
  }

  try {
    const params = new URLSearchParams({ host, port: String(port), room, base });
    const data = await apiJson(`/api/invite?${params.toString()}`);
    if (els.invUrl) els.invUrl.value = data.invite_url || '';
    if (els.invConnect) els.invConnect.value = data.connect_cmd || '';
    if (els.invFeedback) els.invFeedback.textContent = `Invite ready for room "${data.room}".`;
  } catch (err) {
    if (els.invFeedback) els.invFeedback.textContent = `Invite error: ${err.message}`;
  }
}

async function copyText(inputEl, label) {
  const val = inputEl?.value?.trim();
  if (!val) return;
  try {
    await navigator.clipboard.writeText(val);
    if (els.invFeedback) els.invFeedback.textContent = `Copied ${label} to clipboard.`;
  } catch {
    inputEl.select();
    if (els.invFeedback) els.invFeedback.textContent = `Selected ${label} (press Ctrl+C).`;
  }
}

function resetAutoRefresh() {
  if (refreshTimer !== null) {
    clearInterval(refreshTimer);
    refreshTimer = null;
  }
  const ms = Number.parseInt(els.refreshSelect?.value || '0', 10);
  if (adminToken && ms > 0) {
    refreshTimer = setInterval(() => {
      refreshAll();
    }, ms);
  }
}

function init() {
  if (els.invHost) {
    els.invHost.value = window.location.hostname || '127.0.0.1';
  }
  const saved = sessionStorage.getItem(TOKEN_KEY);
  if (saved) {
    adminToken = saved;
    if (els.tokenInput) els.tokenInput.value = saved;
    if (els.rememberSession) els.rememberSession.checked = true;
    refreshAll();
    resetAutoRefresh();
  }

  els.btnLogin?.addEventListener('click', async () => {
    adminToken = (els.tokenInput?.value || '').trim();
    if (!adminToken) {
      setAuthState(false, 'Token Required');
      return;
    }
    if (els.rememberSession?.checked) {
      sessionStorage.setItem(TOKEN_KEY, adminToken);
    } else {
      sessionStorage.removeItem(TOKEN_KEY);
    }
    await refreshAll();
    await buildInvite();
    resetAutoRefresh();
  });

  els.btnLogout?.addEventListener('click', () => {
    adminToken = '';
    sessionStorage.removeItem(TOKEN_KEY);
    if (els.tokenInput) els.tokenInput.value = '';
    setAuthState(false, 'Locked');
    setRefreshBadge('Idle');
    resetAutoRefresh();
  });

  els.refreshSelect?.addEventListener('change', resetAutoRefresh);
  els.btnRefreshNow?.addEventListener('click', () => refreshAll());

  els.btnChangelevel?.addEventListener('click', async () => {
    const target = (els.mapCustom?.value || '').trim() || els.mapSelect?.value || 'de_dust2';
    await runRcon(`changelevel ${target}`);
    if (els.mapCustom) els.mapCustom.value = '';
    await refreshAll();
  });

  els.btnApplyBots?.addEventListener('click', async () => {
    const quota = Math.max(0, Math.min(32, Number.parseInt(els.botQuota?.value || '0', 10) || 0));
    const diff = Math.max(0, Math.min(3, Number.parseInt(els.botDiff?.value || '1', 10) || 1));
    await runRcon(`bot_difficulty ${diff}`);
    await runRcon(`bot_quota ${quota}`);
    await refreshAll();
  });

  els.btnAddT?.addEventListener('click', async () => {
    await runRcon('bot_add_t');
    await refreshAll();
  });
  els.btnAddCt?.addEventListener('click', async () => {
    await runRcon('bot_add_ct');
    await refreshAll();
  });
  els.btnKickBots?.addEventListener('click', async () => {
    await runRcon('bot_kick all');
    await refreshAll();
  });
  els.btnRestartRound?.addEventListener('click', async () => {
    await runRcon('sv_restart 1');
    await refreshAll();
  });

  els.rconForm?.addEventListener('submit', async (e) => {
    e.preventDefault();
    const cmd = (els.rconInput?.value || '').trim();
    if (!cmd) return;
    rconHistory.unshift(cmd);
    if (rconHistory.length > 50) rconHistory.pop();
    historyCursor = -1;
    if (els.rconInput) els.rconInput.value = '';
    await runRcon(cmd);
    await refreshAll();
  });

  els.rconInput?.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowUp' && rconHistory.length > 0) {
      e.preventDefault();
      historyCursor = Math.min(rconHistory.length - 1, historyCursor + 1);
      els.rconInput.value = rconHistory[historyCursor];
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      historyCursor = Math.max(-1, historyCursor - 1);
      els.rconInput.value = historyCursor === -1 ? '' : rconHistory[historyCursor];
    }
  });

  els.btnRconClear?.addEventListener('click', () => {
    if (els.rconOutput) els.rconOutput.textContent = '';
  });

  for (const chip of document.querySelectorAll('.rcon-preset')) {
    chip.addEventListener('click', async () => {
      const cmd = chip.getAttribute('data-cmd');
      if (!cmd) return;
      await runRcon(cmd);
      await refreshAll();
    });
  }

  els.btnBuildInvite?.addEventListener('click', () => buildInvite());
  els.btnCopyInvite?.addEventListener('click', () => copyText(els.invUrl, 'invite URL'));
  els.btnCopyConnect?.addEventListener('click', () => copyText(els.invConnect, 'console command'));

  els.tabBtnDashboard?.addEventListener('click', () => switchAdminTab('dashboard'));
  els.tabBtnMods?.addEventListener('click', () => switchAdminTab('mods'));

  els.btnModsRefresh?.addEventListener('click', () => {
    refreshModsStudio().catch((err) => setStudioStatus(`Refresh failed: ${err.message}`, 'err'));
  });
  els.btnModsReloadAll?.addEventListener('click', async () => {
    await runRcon('mod_reload all');
    await refreshModsStudio();
    setStudioStatus('Reloaded all mods from disk.', 'ok');
  });
  els.btnStudioLoad?.addEventListener('click', () => loadStudioFile());
  els.studioFileSelect?.addEventListener('change', () => {
    if (els.studioFileSelect?.value) loadStudioFile(els.studioFileSelect.value);
  });
  els.btnStudioSave?.addEventListener('click', () => saveStudioFile());
  els.studioEditor?.addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
      e.preventDefault();
      saveStudioFile();
    } else if (e.key === 'Tab') {
      e.preventDefault();
      const el = els.studioEditor;
      const start = el.selectionStart;
      const end = el.selectionEnd;
      el.value = `${el.value.slice(0, start)}    ${el.value.slice(end)}`;
      el.selectionStart = el.selectionEnd = start + 4;
    }
  });

  init2dReplayControls();
}

// ---------------------------------------------------------------------------
// Interactive HLTV-Style 2D Round Replay Player
// ---------------------------------------------------------------------------

const WEAPON_NAMES = {
  0: '',
  1: 'P228',
  3: 'Scout',
  4: 'HE',
  5: 'XM1014',
  6: 'C4',
  7: 'MAC-10',
  8: 'AUG',
  9: 'Smoke',
  10: 'Elites',
  11: 'Five-SeveN',
  12: 'UMP45',
  13: 'SG550',
  14: 'Galil',
  15: 'FAMAS',
  16: 'USP',
  17: 'Glock',
  18: 'AWP',
  19: 'MP5',
  20: 'M249',
  21: 'M3',
  22: 'M4A1',
  23: 'TMP',
  24: 'G3SG1',
  25: 'Flash',
  26: 'Deagle',
  27: 'SG552',
  28: 'AK-47',
  29: 'Knife',
  30: 'P90',
  99: 'Shield',
};

const r2d = {
  open: false,
  playing: false,
  speed: 1,
  time: 0,
  duration: 0,
  lastRafMs: 0,
  rafId: null,
  draggingScrubber: false,
  timeline: null,
  overview: null,
  overviewImg: null,
  overviewBlobUrl: null,
  lastSidebarTick: -1,
};

function formatReplayTime(secs) {
  const s = Math.max(0, Number(secs) || 0);
  const mins = Math.floor(s / 60);
  const rem = s - mins * 60;
  return `${mins}:${rem < 10 ? '0' : ''}${rem.toFixed(1)}`;
}

function setReplay2dBadge(text, isErr = false) {
  if (!els.replay2dStatus) return;
  els.replay2dStatus.textContent = text;
  els.replay2dStatus.className = isErr ? 'badge err' : 'badge ok';
}

function close2dReplay() {
  r2d.open = false;
  r2d.playing = false;
  if (r2d.rafId !== null) {
    cancelAnimationFrame(r2d.rafId);
    r2d.rafId = null;
  }
  if (r2d.overviewBlobUrl) {
    URL.revokeObjectURL(r2d.overviewBlobUrl);
    r2d.overviewBlobUrl = null;
  }
  r2d.overviewImg = null;
  r2d.timeline = null;
  r2d.overview = null;
  els.replay2dModal?.classList.remove('open');
}

async function open2dReplay(roundMeta) {
  close2dReplay();
  r2d.open = true;
  r2d.time = 0;
  r2d.lastSidebarTick = -1;
  els.replay2dModal?.classList.add('open');
  if (els.replay2dTitle) {
    els.replay2dTitle.textContent = `Round #${roundMeta.round_number} — ${roundMeta.map} — ${roundMeta.winner} (${roundMeta.reason || 'End'}) | CT ${roundMeta.score_ct ?? 0} : ${roundMeta.score_t ?? 0} T`;
  }
  setReplay2dBadge('Loading...');

  const [timeline, overview] = await Promise.all([
    apiJson(`/api/replays/${encodeURIComponent(roundMeta.id)}/timeline`),
    apiJson(`/api/map-overview/${encodeURIComponent(roundMeta.map)}`).catch(() => null),
  ]);

  if (!r2d.open) return;

  r2d.timeline = timeline;
  r2d.overview = overview;
  const frames = Array.isArray(timeline?.frames) ? timeline.frames : [];
  const lastFrameT = frames.length > 0 ? Number(frames[frames.length - 1].t || 0) : 0;
  r2d.duration = Math.max(0.1, Number(timeline?.duration_secs || lastFrameT || 0.1));

  if (overview?.has_overview && overview?.image_url) {
    try {
      const resp = await apiFetch(overview.image_url);
      const blob = await resp.blob();
      if (!r2d.open) return;
      const url = URL.createObjectURL(blob);
      const img = new Image();
      img.onload = () => {
        if (!r2d.open) return;
        r2d.overviewImg = img;
        render2dFrame();
      };
      img.src = url;
      r2d.overviewBlobUrl = url;
      setReplay2dBadge(`Overview (${overview.rotated ? 'ROTATED 1' : 'ROTATED 0'})`);
    } catch {
      setReplay2dBadge('Wireframe Fallback');
    }
  } else {
    setReplay2dBadge('Wireframe Radar');
  }

  r2d.playing = frames.length > 1;
  updatePlayButtonText();
  render2dFrame();
  start2dLoop();
}

function updatePlayButtonText() {
  if (els.replay2dPlay) {
    els.replay2dPlay.textContent = r2d.playing ? 'Pause' : 'Play';
  }
}

function toggle2dPlay() {
  if (!r2d.open || !r2d.timeline) return;
  if (!r2d.playing && r2d.time >= r2d.duration) {
    r2d.time = 0;
  }
  r2d.playing = !r2d.playing;
  r2d.lastRafMs = performance.now();
  updatePlayButtonText();
  render2dFrame();
}

function start2dLoop() {
  if (r2d.rafId !== null) cancelAnimationFrame(r2d.rafId);
  r2d.lastRafMs = performance.now();
  const step = (nowMs) => {
    if (!r2d.open) return;
    const dt = Math.min(0.1, Math.max(0, (nowMs - r2d.lastRafMs) / 1000));
    r2d.lastRafMs = nowMs;
    if (r2d.playing) {
      r2d.time += dt * r2d.speed;
      if (r2d.time >= r2d.duration) {
        r2d.time = r2d.duration;
        r2d.playing = false;
        updatePlayButtonText();
      }
      render2dFrame();
    } else if (hasPulsingOverlay()) {
      render2dFrame();
    }
    r2d.rafId = requestAnimationFrame(step);
  };
  r2d.rafId = requestAnimationFrame(step);
}

function hasPulsingOverlay() {
  const tl = r2d.timeline;
  if (!tl || !Array.isArray(tl.bombs)) return false;
  return tl.bombs.some((b) => b.state === 'planted' && b.tick * 0.1 <= r2d.time);
}

function computeCanvasTransform(w, h) {
  const ov = r2d.overview;
  if (ov?.has_overview && r2d.overviewImg) {
    const imgW = r2d.overviewImg.naturalWidth || 1024;
    const imgH = r2d.overviewImg.naturalHeight || 768;
    const scale = Math.min(w / imgW, h / imgH);
    const drawW = imgW * scale;
    const drawH = imgH * scale;
    const offX = (w - drawW) * 0.5;
    const offY = (h - drawH) * 0.5;
    const zoom = Math.max(0.05, Number(ov.zoom) || 1.0);
    const ox = Number(ov.origin?.[0] || 0);
    const oy = Number(ov.origin?.[1] || 0);
    const spanW = 8192.0 / zoom;
    const spanH = 6144.0 / zoom;
    const rotated = Boolean(ov.rotated);
    return {
      mode: 'overview',
      offX,
      offY,
      drawW,
      drawH,
      pxPerUnit: drawW / spanW,
      toScreen(wx, wy) {
        const dx = wx - ox;
        const dy = wy - oy;
        const u = rotated ? 0.5 + dx / spanW : 0.5 - dy / spanW;
        const v = rotated ? 0.5 - dy / spanH : 0.5 - dx / spanH;
        return { x: offX + u * drawW, y: offY + v * drawH };
      },
      yawRad(yawDeg) {
        const yaw = (Number(yawDeg) || 0) * (Math.PI / 180);
        return rotated ? -yaw : -(yaw + Math.PI * 0.5);
      },
    };
  }

  // Wireframe radar fallback auto-fitted to BSP world mins/maxs.
  const mins = ov?.mins || [-2048, -2048, -512];
  const maxs = ov?.maxs || [2048, 2048, 512];
  const minX = Number(mins[0] ?? -2048);
  const maxX = Number(maxs[0] ?? 2048);
  const minY = Number(mins[1] ?? -2048);
  const maxY = Number(maxs[1] ?? 2048);
  const spanX = Math.max(256, maxX - minX);
  const spanY = Math.max(256, maxY - minY);
  const pad = 36;
  const scale = Math.min((w - pad * 2) / spanX, (h - pad * 2) / spanY);
  const drawW = spanX * scale;
  const drawH = spanY * scale;
  const offX = (w - drawW) * 0.5;
  const offY = (h - drawH) * 0.5;
  return {
    mode: 'wireframe',
    offX,
    offY,
    drawW,
    drawH,
    minX,
    maxX,
    minY,
    maxY,
    pxPerUnit: scale,
    toScreen(wx, wy) {
      return {
        x: offX + (wx - minX) * scale,
        y: offY + (maxY - wy) * scale,
      };
    },
    yawRad(yawDeg) {
      return -((Number(yawDeg) || 0) * (Math.PI / 180));
    },
  };
}

function wrapAngleDeg(a) {
  let d = a % 360;
  if (d > 180) d -= 360;
  if (d < -180) d += 360;
  return d;
}

function lerpAngleDeg(a, b, t) {
  const diff = wrapAngleDeg(b - a);
  return a + diff * t;
}

function samplePlayersAtTime(timeline, t) {
  const frames = Array.isArray(timeline?.frames) ? timeline.frames : [];
  if (frames.length === 0) return [];
  let idx = 0;
  for (let i = 0; i < frames.length; i++) {
    if (Number(frames[i].t) <= t) idx = i;
    else break;
  }
  const f0 = frames[idx];
  const f1 = frames[Math.min(frames.length - 1, idx + 1)];
  const t0 = Number(f0.t || 0);
  const t1 = Number(f1.t || t0);
  const alpha = f1 !== f0 && t1 > t0 ? Math.max(0, Math.min(1, (t - t0) / (t1 - t0))) : 0;

  const nextBySlot = new Map();
  for (const row of f1.p || []) {
    nextBySlot.set(row[0], row);
  }
  const out = [];
  for (const p of f0.p || []) {
    const nxt = nextBySlot.get(p[0]);
    const canInterp = nxt && p[2] === 1 && nxt[2] === 1;
    out.push({
      slot: p[0],
      team: p[1],
      alive: p[2] === 1,
      hp: p[3],
      x: canInterp ? p[4] + (nxt[4] - p[4]) * alpha : p[4],
      y: canInterp ? p[5] + (nxt[5] - p[5]) * alpha : p[5],
      z: canInterp ? p[6] + (nxt[6] - p[6]) * alpha : p[6],
      yaw: canInterp ? lerpAngleDeg(p[7], nxt[7], alpha) : p[7],
      weaponId: p[8],
      hasC4: p[9] === 1,
      flashedAlpha: p[10],
    });
  }
  return out;
}

function drawWireframeGrid(ctx, xform, w, h) {
  ctx.strokeStyle = 'rgba(136, 145, 128, 0.18)';
  ctx.lineWidth = 1;
  const step = 512;
  const startX = Math.floor(xform.minX / step) * step;
  const startY = Math.floor(xform.minY / step) * step;
  for (let gx = startX; gx <= xform.maxX; gx += step) {
    const p0 = xform.toScreen(gx, xform.minY);
    const p1 = xform.toScreen(gx, xform.maxY);
    ctx.beginPath();
    ctx.moveTo(p0.x, p0.y);
    ctx.lineTo(p1.x, p1.y);
    ctx.stroke();
  }
  for (let gy = startY; gy <= xform.maxY; gy += step) {
    const p0 = xform.toScreen(xform.minX, gy);
    const p1 = xform.toScreen(xform.maxX, gy);
    ctx.beginPath();
    ctx.moveTo(p0.x, p0.y);
    ctx.lineTo(p1.x, p1.y);
    ctx.stroke();
  }
  ctx.strokeStyle = 'rgba(196, 181, 80, 0.45)';
  ctx.lineWidth = 1.5;
  ctx.strokeRect(xform.offX, xform.offY, xform.drawW, xform.drawH);

  ctx.fillStyle = 'rgba(196, 181, 80, 0.78)';
  ctx.font = 'bold 11px Consolas, monospace';
  ctx.fillText('WIREFRAME RADAR (NO OVERVIEW BMP)', 12, h - 12);
}

function render2dFrame() {
  const canvas = els.replay2dCanvas;
  const tl = r2d.timeline;
  if (!canvas || !tl) return;
  const ctx = canvas.getContext('2d');
  if (!ctx) return;

  const w = canvas.width;
  const h = canvas.height;
  const t = r2d.time;
  const xform = computeCanvasTransform(w, h);

  if (els.replay2dTime) {
    els.replay2dTime.textContent = `${formatReplayTime(t)} / ${formatReplayTime(r2d.duration)}`;
  }

  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = '#0d110c';
  ctx.fillRect(0, 0, w, h);

  if (xform.mode === 'overview' && r2d.overviewImg) {
    ctx.drawImage(r2d.overviewImg, xform.offX, xform.offY, xform.drawW, xform.drawH);
  } else {
    drawWireframeGrid(ctx, xform, w, h);
  }

  // 1. Active 18-second smoke clouds
  for (const g of tl.grenades || []) {
    if (g.kind !== 'smoke' || g.detonate_tick == null || !g.detonate_pos) continue;
    const detT = g.detonate_tick * 0.1;
    const age = t - detT;
    if (age < 0 || age > 18.0) continue;
    const fade = age > 15.5 ? Math.max(0, (18.0 - age) / 2.5) : 1.0;
    const sc = xform.toScreen(g.detonate_pos[0], g.detonate_pos[1]);
    const radius = Math.max(18, 128 * xform.pxPerUnit);
    ctx.save();
    ctx.fillStyle = `rgba(185, 194, 198, ${(0.58 * fade).toFixed(3)})`;
    ctx.strokeStyle = `rgba(220, 228, 232, ${(0.75 * fade).toFixed(3)})`;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.arc(sc.x, sc.y, radius, 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();
    ctx.restore();
  }

  // 2. Grenade flight arcs and HE/Flash detonation bursts
  for (const g of tl.grenades || []) {
    const startT = Number(g.start_tick || 0) * 0.1;
    const detT = g.detonate_tick != null ? g.detonate_tick * 0.1 : r2d.duration;
    const traj = Array.isArray(g.trajectory) ? g.trajectory : [];
    if (t >= startT && t <= detT + 0.45 && traj.length > 0) {
      const span = Math.max(0.1, detT - startT);
      const frac = Math.max(0, Math.min(1, (t - startT) / span));
      const count = Math.max(1, Math.ceil(frac * traj.length));
      const color = g.kind === 'he' ? '#ff934f' : g.kind === 'flash' ? '#fff49c' : '#96d1c7';
      ctx.save();
      ctx.strokeStyle = color;
      ctx.lineWidth = 2;
      ctx.setLineDash([4, 3]);
      ctx.beginPath();
      for (let i = 0; i < count; i++) {
        const pt = xform.toScreen(traj[i][0], traj[i][1]);
        if (i === 0) ctx.moveTo(pt.x, pt.y);
        else ctx.lineTo(pt.x, pt.y);
      }
      ctx.stroke();
      ctx.setLineDash([]);
      const tip = xform.toScreen(traj[count - 1][0], traj[count - 1][1]);
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(tip.x, tip.y, 4, 0, Math.PI * 2);
      ctx.fill();
      ctx.restore();
    }

    if (g.detonate_tick != null && g.detonate_pos) {
      const detAt = g.detonate_tick * 0.1;
      const age = t - detAt;
      if (g.kind === 'flash' && age >= 0 && age <= 0.85) {
        const alpha = 1 - age / 0.85;
        const sc = xform.toScreen(g.detonate_pos[0], g.detonate_pos[1]);
        const r = 10 + age * 46;
        ctx.save();
        ctx.fillStyle = `rgba(255, 255, 240, ${(0.75 * alpha).toFixed(3)})`;
        ctx.strokeStyle = `rgba(255, 250, 180, ${alpha.toFixed(3)})`;
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.arc(sc.x, sc.y, r, 0, Math.PI * 2);
        ctx.fill();
        ctx.stroke();
        ctx.restore();
      } else if (g.kind === 'he' && age >= 0 && age <= 0.65) {
        const alpha = 1 - age / 0.65;
        const sc = xform.toScreen(g.detonate_pos[0], g.detonate_pos[1]);
        const r = 10 + age * 54;
        ctx.save();
        ctx.fillStyle = `rgba(255, 110, 48, ${(0.45 * alpha).toFixed(3)})`;
        ctx.strokeStyle = `rgba(255, 190, 90, ${alpha.toFixed(3)})`;
        ctx.lineWidth = 2.5;
        ctx.beginPath();
        ctx.arc(sc.x, sc.y, r, 0, Math.PI * 2);
        ctx.fill();
        ctx.stroke();
        ctx.restore();
      }
    }
  }

  // 3. Bullet tracers (150 ms fade)
  for (const s of tl.shots || []) {
    const st = Number(s.tick || 0) * 0.1;
    const age = t - st;
    if (age < 0 || age > 0.15) continue;
    const alpha = 1 - age / 0.15;
    const p0 = xform.toScreen(s.from[0], s.from[1]);
    const p1 = xform.toScreen(s.to[0], s.to[1]);
    ctx.save();
    ctx.strokeStyle = `rgba(255, 236, 130, ${(0.9 * alpha).toFixed(3)})`;
    ctx.lineWidth = 1.6;
    ctx.beginPath();
    ctx.moveTo(p0.x, p0.y);
    ctx.lineTo(p1.x, p1.y);
    ctx.stroke();
    ctx.restore();
  }

  // 4. C4 state (dropped / planted pulse / defused / exploded)
  let bombStatus = null;
  for (const b of tl.bombs || []) {
    if (Number(b.tick || 0) * 0.1 <= t) {
      bombStatus = b;
    }
  }
  if (bombStatus && bombStatus.state !== 'pickup') {
    const bPos = xform.toScreen(bombStatus.pos[0], bombStatus.pos[1]);
    const bTime = Number(bombStatus.tick || 0) * 0.1;
    if (bombStatus.state === 'planted') {
      const pulse = 0.5 + 0.5 * Math.sin(performance.now() * 0.01);
      const ringR = 11 + pulse * 14;
      ctx.save();
      ctx.strokeStyle = `rgba(255, 70, 50, ${(0.85 - pulse * 0.45).toFixed(2)})`;
      ctx.lineWidth = 2.2;
      ctx.beginPath();
      ctx.arc(bPos.x, bPos.y, ringR, 0, Math.PI * 2);
      ctx.stroke();
      ctx.fillStyle = '#ff3b2f';
      ctx.fillRect(bPos.x - 5, bPos.y - 5, 10, 10);
      ctx.fillStyle = '#fff';
      ctx.font = 'bold 9px Tahoma, sans-serif';
      ctx.textAlign = 'center';
      ctx.fillText('C4', bPos.x, bPos.y - 9);
      ctx.restore();
    } else if (bombStatus.state === 'dropped') {
      ctx.save();
      ctx.fillStyle = '#c4b550';
      ctx.fillRect(bPos.x - 4, bPos.y - 4, 8, 8);
      ctx.font = 'bold 9px Tahoma, sans-serif';
      ctx.textAlign = 'center';
      ctx.fillText('C4', bPos.x, bPos.y - 7);
      ctx.restore();
    } else if (bombStatus.state === 'defused') {
      ctx.save();
      ctx.strokeStyle = '#96d178';
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(bPos.x, bPos.y, 11, 0, Math.PI * 2);
      ctx.stroke();
      ctx.fillStyle = '#96d178';
      ctx.font = 'bold 10px Tahoma, sans-serif';
      ctx.textAlign = 'center';
      ctx.fillText('DEFUSED', bPos.x, bPos.y - 14);
      ctx.restore();
    } else if (bombStatus.state === 'exploded' && t - bTime <= 1.5) {
      const alpha = Math.max(0, 1 - (t - bTime) / 1.5);
      const r = 16 + (t - bTime) * 140;
      ctx.save();
      ctx.fillStyle = `rgba(255, 120, 40, ${(0.45 * alpha).toFixed(3)})`;
      ctx.strokeStyle = `rgba(255, 220, 110, ${alpha.toFixed(3)})`;
      ctx.lineWidth = 3;
      ctx.beginPath();
      ctx.arc(bPos.x, bPos.y, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
      ctx.restore();
    }
  }

  // 5. Players (FOV wedges, CT/T circles, HP arcs, flash halo, C4 carrier badge)
  const metaBySlot = new Map();
  for (const pm of tl.players || []) metaBySlot.set(pm.slot, pm);
  const playersNow = samplePlayersAtTime(tl, t);

  for (const p of playersNow) {
    if (p.team !== 1 && p.team !== 2) continue;
    const sc = xform.toScreen(p.x, p.y);
    const isCt = p.team === 2;
    const baseCol = isCt ? '#4da3ff' : '#ff6b4a';

    if (!p.alive) {
      ctx.save();
      ctx.strokeStyle = isCt ? 'rgba(77,163,255,0.45)' : 'rgba(255,107,74,0.45)';
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.moveTo(sc.x - 5, sc.y - 5);
      ctx.lineTo(sc.x + 5, sc.y + 5);
      ctx.moveTo(sc.x + 5, sc.y - 5);
      ctx.lineTo(sc.x - 5, sc.y + 5);
      ctx.stroke();
      ctx.restore();
      continue;
    }

    const yawRad = xform.yawRad(p.yaw);
    const coneLen = 30;
    const fovHalf = 0.45;
    ctx.save();
    ctx.fillStyle = isCt ? 'rgba(77, 163, 255, 0.22)' : 'rgba(255, 107, 74, 0.22)';
    ctx.beginPath();
    ctx.moveTo(sc.x, sc.y);
    ctx.arc(sc.x, sc.y, coneLen, yawRad - fovHalf, yawRad + fovHalf);
    ctx.closePath();
    ctx.fill();

    const r = 9.5;
    // Flash halo
    if (p.flashedAlpha > 8) {
      const fa = Math.min(1, p.flashedAlpha / 255);
      ctx.strokeStyle = `rgba(255, 255, 255, ${(0.95 * fa).toFixed(2)})`;
      ctx.lineWidth = 4;
      ctx.beginPath();
      ctx.arc(sc.x, sc.y, r + 4, 0, Math.PI * 2);
      ctx.stroke();
    }

    // Player body circle
    ctx.fillStyle = baseCol;
    ctx.strokeStyle = '#111510';
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.arc(sc.x, sc.y, r, 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();

    // HP arc around outer rim
    const hpFrac = Math.max(0, Math.min(1, p.hp / 100));
    const hpCol = p.hp > 50 ? '#96d178' : p.hp > 25 ? '#e8b948' : '#e05a47';
    ctx.strokeStyle = hpCol;
    ctx.lineWidth = 2.5;
    ctx.beginPath();
    ctx.arc(sc.x, sc.y, r + 2, -Math.PI / 2, -Math.PI / 2 + Math.PI * 2 * hpFrac);
    ctx.stroke();

    // C4 carrier dot
    if (p.hasC4) {
      ctx.fillStyle = '#f5d442';
      ctx.strokeStyle = '#111';
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.arc(sc.x + 8, sc.y - 8, 3.5, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
    }

    // Slot number
    ctx.fillStyle = '#0f120e';
    ctx.font = 'bold 10px Tahoma, sans-serif';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText(String(p.slot), sc.x, sc.y + 0.5);

    // Player label
    const meta = metaBySlot.get(p.slot);
    const name = meta?.name || `#${p.slot}`;
    const wLabel = WEAPON_NAMES[p.weaponId] || '';
    const text = wLabel ? `${name} [${wLabel}]` : name;
    ctx.font = '10px Tahoma, sans-serif';
    ctx.textBaseline = 'top';
    ctx.fillStyle = 'rgba(10, 14, 10, 0.72)';
    const tw = ctx.measureText(text).width;
    ctx.fillRect(sc.x - tw * 0.5 - 3, sc.y + 12, tw + 6, 13);
    ctx.fillStyle = '#ecefe9';
    ctx.fillText(text, sc.x, sc.y + 13);
    ctx.restore();
  }

  // 6. Killfeed overlay (top-right inside canvas)
  const visibleKills = (tl.kills || []).filter((k) => Number(k.tick || 0) * 0.1 <= t).slice(-5);
  if (visibleKills.length > 0) {
    ctx.save();
    ctx.font = '11px Tahoma, sans-serif';
    ctx.textAlign = 'right';
    ctx.textBaseline = 'top';
    visibleKills.forEach((k, idx) => {
      const killerName = k.killer != null ? (metaBySlot.get(k.killer)?.name || `#${k.killer}`) : 'World';
      const victimName = metaBySlot.get(k.victim)?.name || `#${k.victim}`;
      const line = `${killerName} [${k.weapon || 'kill'}${k.headshot ? ' HS' : ''}] ${victimName}`;
      const tw = ctx.measureText(line).width;
      const y = 10 + idx * 19;
      ctx.fillStyle = 'rgba(12, 16, 11, 0.78)';
      ctx.fillRect(w - tw - 18, y, tw + 10, 16);
      ctx.fillStyle = k.headshot ? '#ffd873' : '#ecefe9';
      ctx.fillText(line, w - 13, y + 2);
    });
    ctx.restore();
  }

  // 7. Sidebars & Scrubber
  const curTick = Math.floor(t * 10);
  if (curTick !== r2d.lastSidebarTick) {
    r2d.lastSidebarTick = curTick;
    render2dSidebars(playersNow, metaBySlot);
  }
  render2dScrubber();
}

function render2dSidebars(playersNow, metaBySlot) {
  if (!els.replay2dCtList || !els.replay2dTList) return;
  const ctFrag = document.createDocumentFragment();
  const tFrag = document.createDocumentFragment();
  const sorted = [...playersNow].sort((a, b) => a.slot - b.slot);
  for (const p of sorted) {
    if (p.team !== 1 && p.team !== 2) continue;
    const row = document.createElement('div');
    row.className = p.alive ? 'replay2d-prow' : 'replay2d-prow dead';

    const top = document.createElement('div');
    top.className = 'replay2d-prow-top';
    const nameEl = document.createElement('span');
    const name = metaBySlot.get(p.slot)?.name || `Player ${p.slot}`;
    nameEl.textContent = `${p.slot}. ${name}`;
    const hpEl = document.createElement('span');
    hpEl.textContent = p.alive ? `${p.hp} HP` : 'DEAD';
    top.append(nameEl, hpEl);

    const sub = document.createElement('div');
    sub.className = 'replay2d-prow-sub';
    const weapEl = document.createElement('span');
    weapEl.textContent = WEAPON_NAMES[p.weaponId] || '-';
    const badgesEl = document.createElement('span');
    const flags = [];
    if (p.hasC4) flags.push('C4');
    if (p.flashedAlpha > 20) flags.push('BLIND');
    badgesEl.textContent = flags.join(' ');
    sub.append(weapEl, badgesEl);

    const track = document.createElement('div');
    track.className = 'replay2d-hp-track';
    const fill = document.createElement('div');
    fill.className = 'replay2d-hp-fill';
    const pct = p.alive ? Math.max(0, Math.min(100, p.hp)) : 0;
    fill.style.width = `${pct}%`;
    fill.style.background = pct > 50 ? '#96d178' : pct > 25 ? '#e8b948' : '#d96b5b';
    track.appendChild(fill);

    row.append(top, sub, track);
    if (p.team === 2) ctFrag.appendChild(row);
    else tFrag.appendChild(row);
  }
  els.replay2dCtList.replaceChildren(ctFrag);
  els.replay2dTList.replaceChildren(tFrag);
}

function render2dScrubber() {
  const sc = els.replay2dScrubber;
  const tl = r2d.timeline;
  if (!sc || !tl) return;
  const ctx = sc.getContext('2d');
  if (!ctx) return;

  const w = sc.width;
  const h = sc.height;
  const padX = 12;
  const trackW = Math.max(1, w - padX * 2);
  const trackY = 22;

  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = '#20261c';
  ctx.fillRect(0, 0, w, h);

  // Track background & progress fill
  ctx.fillStyle = '#121610';
  ctx.fillRect(padX, trackY, trackW, 6);
  const frac = Math.max(0, Math.min(1, r2d.time / Math.max(0.1, r2d.duration)));
  ctx.fillStyle = '#c4b550';
  ctx.fillRect(padX, trackY, trackW * frac, 6);

  // Kill skull markers along the scrubber track
  ctx.font = '12px Segoe UI Emoji, Apple Color Emoji, sans-serif';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  for (const k of tl.kills || []) {
    const kt = Number(k.tick || 0) * 0.1;
    const kf = Math.max(0, Math.min(1, kt / Math.max(0.1, r2d.duration)));
    const mx = padX + kf * trackW;
    ctx.fillStyle = k.headshot ? '#ffd873' : '#e6e6e6';
    ctx.fillText('\u2620', mx, 11);
  }

  // C4 plant / defuse / explosion markers
  for (const b of tl.bombs || []) {
    if (b.state !== 'planted' && b.state !== 'defused' && b.state !== 'exploded') continue;
    const bt = Number(b.tick || 0) * 0.1;
    const bf = Math.max(0, Math.min(1, bt / Math.max(0.1, r2d.duration)));
    const mx = padX + bf * trackW;
    ctx.fillStyle = b.state === 'defused' ? '#96d178' : b.state === 'exploded' ? '#ff934f' : '#ff4c3b';
    ctx.beginPath();
    ctx.moveTo(mx, 4);
    ctx.lineTo(mx + 5, 10);
    ctx.lineTo(mx, 16);
    ctx.lineTo(mx - 5, 10);
    ctx.closePath();
    ctx.fill();
  }

  // Playhead needle
  const px = padX + frac * trackW;
  ctx.fillStyle = '#ecefe9';
  ctx.fillRect(px - 1.5, trackY - 4, 3, 14);
}

function seekScrubberFromPointer(clientX) {
  const sc = els.replay2dScrubber;
  if (!sc || !r2d.timeline) return;
  const rect = sc.getBoundingClientRect();
  const padPx = (12 / sc.width) * rect.width;
  const frac = Math.max(0, Math.min(1, (clientX - rect.left - padPx) / Math.max(1, rect.width - padPx * 2)));
  r2d.time = frac * r2d.duration;
  render2dFrame();
}

function init2dReplayControls() {
  els.replay2dClose?.addEventListener('click', () => close2dReplay());
  els.replay2dModal?.addEventListener('click', (e) => {
    if (e.target === els.replay2dModal) close2dReplay();
  });
  els.replay2dPlay?.addEventListener('click', () => toggle2dPlay());
  els.replay2dRestart?.addEventListener('click', () => {
    r2d.time = 0;
    r2d.playing = true;
    r2d.lastRafMs = performance.now();
    updatePlayButtonText();
    render2dFrame();
  });

  for (const btn of document.querySelectorAll('.speed-btn')) {
    btn.addEventListener('click', () => {
      const spd = Number.parseFloat(btn.getAttribute('data-speed') || '1') || 1;
      r2d.speed = spd;
      for (const other of document.querySelectorAll('.speed-btn')) {
        other.classList.toggle('active', other === btn);
      }
    });
  }

  els.replay2dScrubber?.addEventListener('pointerdown', (e) => {
    r2d.draggingScrubber = true;
    seekScrubberFromPointer(e.clientX);
  });
  window.addEventListener('pointermove', (e) => {
    if (r2d.draggingScrubber) seekScrubberFromPointer(e.clientX);
  });
  window.addEventListener('pointerup', () => {
    r2d.draggingScrubber = false;
  });

  window.addEventListener('keydown', (e) => {
    if (!r2d.open) return;
    const tag = document.activeElement?.tagName?.toLowerCase();
    if (tag === 'input' || tag === 'textarea' || tag === 'select') return;
    if (e.code === 'Space') {
      e.preventDefault();
      toggle2dPlay();
    } else if (e.code === 'Escape') {
      e.preventDefault();
      close2dReplay();
    } else if (e.code === 'ArrowLeft') {
      e.preventDefault();
      r2d.time = Math.max(0, r2d.time - 2);
      render2dFrame();
    } else if (e.code === 'ArrowRight') {
      e.preventDefault();
      r2d.time = Math.min(r2d.duration, r2d.time + 2);
      render2dFrame();
    }
  });
}

init();
