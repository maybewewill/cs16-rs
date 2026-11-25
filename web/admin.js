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

  statsSummary: document.getElementById('stats-summary'),
  statsBody: document.getElementById('stats-body'),
};

let adminToken = '';
let refreshTimer = null;
const rconHistory = [];
let historyCursor = -1;

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
    const dlTd = document.createElement('td');
    const dlBtn = document.createElement('button');
    dlBtn.textContent = 'Download .dem';
    dlBtn.addEventListener('click', () => {
      downloadReplayDem(r.id).catch((err) => appendTerminal(`! download failed: ${err.message}`));
    });
    dlTd.appendChild(dlBtn);
    tr.appendChild(dlTd);
    frag.appendChild(tr);
  }
  els.replaysBody.replaceChildren(frag);
}

async function refreshAll() {
  if (!adminToken) return;
  setRefreshBadge('Syncing...');
  try {
    const [status, players, stats, replays] = await Promise.all([
      apiJson('/api/status'),
      apiJson('/api/players'),
      apiJson('/api/stats'),
      apiJson('/api/replays'),
    ]);
    setAuthState(true, 'Authenticated');
    renderStatus(status);
    renderPlayers(players);
    renderStats(stats);
    renderReplays(replays);
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
}

init();
