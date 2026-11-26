import {
  cacheAssetsInDb,
  hydrateAssetsFromDb,
  mountFromDirectoryPicker,
  mountFromDropEvent,
  mountFromFileList
} from '../vfs_loader.js';
import { WebInputController } from '../input.js';
import { WebAudioMixer } from '../audio.js';

export const STARTER_TEMPLATES = {
  vampire_shockwave: {
    id: 'vampire_shockwave',
    file: 'vampire_shockwave.py',
    name: 'Vampire Shockwave',
    code: `# Vampire Shockwave — Kill/Headshot lifesteal, VFX shockwave ring & HUD flash
from cs16 import mod, vfx, Color, HookAction, Armor

@mod.info(
    id="vampire_shockwave",
    name="Vampire Shockwave",
    version="1.0.0",
    author="cs16-rs Playground",
    description="Heal on kill (+15 HP / +35 HP headshot), crimson/gold shockwave ring, aura & screen flash",
    permissions=["entities", "messages", "vfx", "storage", "commands"],
)
def _meta():
    pass

KILL_HEAL = 15.0
HS_HEAL = 35.0
MAX_HP = 150.0

@mod.hook("player_spawn", phase="post")
def on_spawn(ev):
    p = ev.player
    p.set_max_health(MAX_HP)
    p.set_armor(100, Armor.VESTHELM)
    p.hud(
        "VAMPIRE MODE ACTIVE — Kills heal +15 HP (HS +35 HP)",
        x=-1.0,
        y=0.18,
        channel=1,
        rgba=(255, 80, 90, 255),
        hold=2.5,
    )

@mod.hook("player_killed", phase="post")
def on_kill(ev):
    k = ev.killer
    v = ev.victim
    if k is None or ev.team_kill or k.slot == v.slot:
        return
    heal = HS_HEAL if ev.headshot else KILL_HEAL
    k.set_max_health(MAX_HP)
    new_hp = min(k.health + heal, MAX_HP)
    k.set_health(new_hp)
    k.add_money(500 if ev.headshot else 250, flash=True)

    # Persistent mod.storage survives live hot-reloads!
    key = f"vamp_kills_{k.slot}"
    total = int(mod.storage.get(key, "0")) + 1
    mod.storage.set(key, str(total))

    ring_rgba = (255, 210, 40, 255) if ev.headshot else (220, 30, 50, 240)
    aura_rgb = (255, 215, 0) if ev.headshot else (220, 20, 40)
    vfx.shockwave_ring(v.origin, radius=240.0, rgba=ring_rgba, life=10, width=18)
    vfx.aura_light(k.origin, radius=24, rgb=aura_rgb, life=8, decay=16)
    k.screen_fade(duration=0.35, hold=0.06, flags=0, rgba=ring_rgba)
    tag = "HEADSHOT DRAIN" if ev.headshot else "VAMPIRE DRAIN"
    k.dhud(f"{tag} +{int(heal)} HP ({int(new_hp)} HP)", x=-1.0, y=0.28, rgb=aura_rgb, hold=1.6)
    mod.log(f"[vampire] {k.name} drained {v.name} (+{int(heal)} HP, total={total})")

@mod.chat_command("/vamp", "!vamp")
def cmd_vamp(player, args):
    total = int(mod.storage.get(f"vamp_kills_{player.slot}", "0"))
    player.chat(f"{Color.GREEN}[Vampire]{Color.DEFAULT} HP={int(player.health)}/{int(MAX_HP)} | Drains={total}")
    vfx.shockwave_ring(player.origin, radius=200.0, rgba=(220, 30, 50, 240), life=10, width=16)
    return HookAction.SUPERCEDE
`
  },
  golden_ak_damager: {
    id: 'golden_ak_damager',
    file: 'golden_ak_damager.py',
    name: 'Golden AK-47 & Damager',
    code: `# Golden AK-47 (1.35x DMG) + Floating Hit Damager HUD + Gold Laser Tracer
from cs16 import mod, vfx, Color, HookAction, WeaponId, Armor

@mod.info(
    id="golden_ak_damager",
    name="Golden AK-47 & Damager",
    version="1.0.0",
    author="cs16-rs Playground",
    description="1.35x Golden AK-47 damage boost, gold laser bullet tracers, and floating hit damager HUD",
    permissions=["entities", "messages", "vfx", "weapons", "commands"],
)
def _meta():
    pass

GOLDEN_MULT = 1.35
golden_holders = set()

@mod.hook("player_spawn", phase="post")
def on_spawn(ev):
    p = ev.player
    golden_holders.add(p.slot)
    p.give_item("weapon_ak47")
    p.set_bpammo(WeaponId.AK47, 90)
    p.set_armor(100, Armor.VESTHELM)
    p.chat(f"{Color.GREEN}[Golden AK]{Color.DEFAULT} Equipped 1.35x Golden AK-47! Type /gold to re-equip.")

@mod.hook("player_take_damage", phase="pre")
def on_damage_pre(ev):
    att = ev.attacker
    if att is None or ev.team_attack:
        return HookAction.CONTINUE
    if ev.weapon_id == WeaponId.AK47:
        ev.damage = ev.damage * GOLDEN_MULT
        return HookAction.OVERRIDE
    return HookAction.CONTINUE

@mod.hook("player_take_damage", phase="post")
def on_damage_post(ev):
    att = ev.attacker
    vic = ev.victim
    if att is None or att.slot == vic.slot or ev.team_attack:
        return
    dmg = int(round(ev.damage))
    if dmg <= 0:
        return
    is_gold = ev.weapon_id == WeaponId.AK47
    rgba = (255, 215, 60, 255) if is_gold else (0, 200, 255, 255)
    crit = " CRIT!" if ev.hitgroup == 1 else ""
    att.hud(
        f"-{dmg}{crit}",
        x=-1.0,
        y=0.55,
        channel=2,
        rgba=rgba,
        fadein=0.02,
        hold=0.85,
        fadeout=0.15,
    )
    if is_gold:
        ax, ay, az = att.origin
        vx, vy, vz = vic.origin
        vfx.laser_beam(
            (ax, ay, az + 20.0),
            (vx, vy, vz + 20.0),
            rgba=(255, 210, 40, 235),
            life=6,
            width=8,
            noise=2,
        )
        vfx.sparks((vx, vy, vz + 20.0))
        mod.log(f"[golden_ak] {att.name} hit {vic.name} for {dmg} dmg (hg={ev.hitgroup})")

@mod.chat_command("/gold", "!gold")
def cmd_gold(player, args):
    golden_holders.add(player.slot)
    player.give_item("weapon_ak47")
    player.set_bpammo(WeaponId.AK47, 90)
    vfx.aura_light(player.origin, radius=22, rgb=(255, 215, 0), life=10, decay=15)
    player.chat(f"{Color.GREEN}[Golden AK]{Color.DEFAULT} Golden AK-47 (1.35x DMG + Laser Tracer) granted!")
    return HookAction.SUPERCEDE
`
  },
  moon_gravity_parachute: {
    id: 'moon_gravity_parachute',
    file: 'moon_gravity_parachute.py',
    name: 'Moon Gravity & Parachute',
    code: `# Moon Gravity Bhop Arena + Hold-E (+use) Parachute Glide + Speedometer HUD
import math
from cs16 import mod, vfx, Button, Color, HookAction

@mod.info(
    id="moon_gravity_parachute",
    name="Moon Gravity & Parachute",
    version="1.0.0",
    author="cs16-rs Playground",
    description="0.38x low-gravity bhop boosts, hold-E (+use) parachute glide, and live u/s speedometer HUD",
    permissions=["entities", "messages", "vfx", "cvars", "commands"],
)
def _meta():
    pass

MOON_GRAVITY = 0.38
CHUTE_FALL_SPEED = -95.0
parachute_active = set()
hud_tick = {}

@mod.hook("player_spawn", phase="post")
def on_spawn(ev):
    p = ev.player
    p.set_gravity(MOON_GRAVITY)
    p.set_maxspeed(285.0)
    p.chat(f"{Color.GREEN}[Moon Arena]{Color.DEFAULT} Low gravity ({MOON_GRAVITY}x) active! Hold E in mid-air to glide.")

@mod.hook("player_jump", phase="post")
def on_jump(ev):
    p = ev.player
    vx, vy, vz = p.velocity
    p.set_gravity(MOON_GRAVITY)
    p.set_velocity((vx * 1.12, vy * 1.12, max(vz, 290.0)))
    vfx.shockwave_ring(p.origin, radius=95.0, rgba=(100, 220, 255, 190), life=4, width=8)

@mod.hook("player_pre_think", phase="pre")
def on_pre_think(ev):
    p = ev.player
    if not p.alive:
        parachute_active.discard(p.slot)
        return
    vx, vy, vz = p.velocity
    holding_use = (ev.buttons & Button.USE) != 0
    if not p.on_ground and holding_use and vz < CHUTE_FALL_SPEED:
        parachute_active.add(p.slot)
        p.set_velocity((vx, vy, CHUTE_FALL_SPEED))
        p.set_gravity(0.12)
    elif p.slot in parachute_active:
        parachute_active.discard(p.slot)
        p.set_gravity(MOON_GRAVITY)

    tick = hud_tick.get(p.slot, 0) + 1
    hud_tick[p.slot] = tick
    if tick % 10 == 0:
        speed = int(round(math.sqrt(vx * vx + vy * vy)))
        chute = " [PARACHUTE]" if p.slot in parachute_active else ""
        p.hud(
            f"SPEED: {speed} u/s{chute}",
            x=-1.0,
            y=0.78,
            channel=3,
            rgba=(130, 235, 255, 235),
            fadein=0.0,
            hold=0.25,
            fadeout=0.05,
        )

@mod.chat_command("/moon", "!moon")
def cmd_moon(player, args):
    player.set_gravity(MOON_GRAVITY)
    player.set_maxspeed(285.0)
    player.chat(f"{Color.GREEN}[Moon Arena]{Color.DEFAULT} Gravity reset to {MOON_GRAVITY}x. Hold E while falling to glide!")
    return HookAction.SUPERCEDE
`
  },
  vip_menu: {
    id: 'vip_menu',
    file: 'vip_menu.py',
    name: 'Interactive VIP Menu',
    code: `# Interactive GoldSrc \\yVIP Menu on /vip, !vip, or spawn
from cs16 import mod, vfx, Color, HookAction, Armor, Team, Menu, RenderFx, RenderMode

@mod.info(
    id="vip_menu",
    name="Interactive VIP Menu",
    version="1.0.0",
    author="cs16-rs Playground",
    description="GoldSrc \\\\yVIP Menu on /vip, !vip, or spawn: +HP/Armor, Deagle+Nades, $16000, Golden Glow",
    permissions=["entities", "messages", "menus", "vfx", "weapons", "commands"],
)
def _meta():
    pass

def on_vip_pick(player, info):
    if not player.alive:
        player.chat(f"{Color.GREEN}[VIP]{Color.DEFAULT} Wait until you respawn to claim VIP perks.")
        return
    if info == "hp_armor":
        player.set_max_health(150.0)
        player.set_health(150.0)
        player.set_armor(100, Armor.VESTHELM)
        player.chat(f"{Color.GREEN}[VIP]{Color.DEFAULT} Granted 150 HP + 100 VestHelm!")
        vfx.shockwave_ring(player.origin, radius=180.0, rgba=(80, 240, 120, 240), life=8, width=14)
    elif info == "deagle_nades":
        player.give_item("weapon_deagle")
        player.give_item("weapon_hegrenade")
        player.give_item("weapon_flashbang")
        player.give_item("weapon_smokegrenade")
        if player.team == Team.CT:
            player.set_defuser(True)
        player.chat(f"{Color.GREEN}[VIP]{Color.DEFAULT} Granted Desert Eagle + Full Grenade Pack!")
    elif info == "money_max":
        player.set_money(16000, flash=True)
        player.chat(f"{Color.GREEN}[VIP]{Color.DEFAULT} Account set to $16,000!")
    elif info == "gold_glow":
        player.set_rendering(RenderFx.GLOW_SHELL, (255, 200, 40), RenderMode.NORMAL, 24)
        vfx.aura_light(player.origin, radius=24, rgb=(255, 215, 0), life=12, decay=15)
        player.chat(f"{Color.GREEN}[VIP]{Color.DEFAULT} Golden VIP Glow Shell activated!")
    mod.log(f"[vip_menu] {player.name} selected VIP option: {info}")

def open_vip_menu(player):
    menu = Menu("\\\\yVIP Playground Menu\\\\w", callback=on_vip_pick)
    menu.add_item("\\\\w+150 HP & 100 VestHelm", "hp_armor")
    menu.add_item("\\\\wDeagle + HE / Flash / Smoke", "deagle_nades")
    menu.add_item("\\\\ySet Money to $16,000", "money_max")
    menu.add_item("\\\\rGolden Glow Shell + Aura", "gold_glow")
    menu.show(player)

@mod.hook("player_spawn", phase="post")
def on_spawn(ev):
    p = ev.player
    if p.slot == 0:
        open_vip_menu(p)
        p.chat(f"{Color.GREEN}[VIP]{Color.DEFAULT} Press 1-4 to pick a perk, or click Chat: /vip anytime!")

@mod.chat_command("/vip", "!vip", "/vipmenu")
def cmd_vip(player, args):
    open_vip_menu(player)
    return HookAction.SUPERCEDE
`
  }
};

const PY_KEYWORDS = new Set([
  'False', 'None', 'True', 'and', 'as', 'assert', 'break', 'class', 'continue',
  'def', 'del', 'elif', 'else', 'except', 'finally', 'for', 'from', 'global',
  'if', 'import', 'in', 'is', 'lambda', 'nonlocal', 'not', 'or', 'pass',
  'raise', 'return', 'try', 'while', 'with', 'yield'
]);
const PY_BUILTINS = new Set([
  'int', 'float', 'str', 'bool', 'len', 'min', 'max', 'round', 'abs', 'range',
  'set', 'list', 'dict', 'tuple', 'print', 'enumerate', 'sorted', 'sum', 'any', 'all'
]);

const canvas = document.getElementById('playground-canvas');
const canvasOverlay = document.getElementById('canvas-overlay');
const gpuPill = document.getElementById('gpu-pill');
const gameStatusEl = document.getElementById('game-status');
const overlayAssetStatusEl = document.getElementById('overlay-asset-status');
const templateSelect = document.getElementById('template-select');
const editor = document.getElementById('py-editor');
const gutterEl = document.getElementById('line-gutter');
const highlightEl = document.getElementById('code-highlight');
const reloadBadgeEl = document.getElementById('reload-badge');
const resmonSummaryEl = document.getElementById('resmon-summary');
const resmonListEl = document.getElementById('resmon-list');
const consoleEl = document.getElementById('plugin-console');
const mapSelect = document.getElementById('map-select');
const botsInput = document.getElementById('bots-count');
const quickCmdInput = document.getElementById('quick-cmd-input');

let cs16Web = null;
let audio = null;
let input = null;
let activePluginName = 'vampire_shockwave.py';
let activePluginId = 'vampire_shockwave';
let lastTelemetryMs = 0;

function escapeHtml(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

function highlightPythonLine(line) {
  let out = '';
  let i = 0;
  const n = line.length;
  while (i < n) {
    const ch = line[i];
    if (ch === '#') {
      out += `<span class="tok-com">${escapeHtml(line.slice(i))}</span>`;
      break;
    }
    if (ch === '@' && line.slice(0, i).trim() === '') {
      let j = i + 1;
      while (j < n && /[A-Za-z0-9_.]/.test(line[j])) j++;
      out += `<span class="tok-dec">${escapeHtml(line.slice(i, j))}</span>`;
      i = j;
      continue;
    }
    if (
      (ch === 'f' || ch === 'r' || ch === 'b' || ch === 'F' || ch === 'R') &&
      (line[i + 1] === '"' || line[i + 1] === "'") &&
      (i === 0 || !/[A-Za-z0-9_]/.test(line[i - 1]))
    ) {
      const q = line[i + 1];
      let j = i + 2;
      while (j < n && line[j] !== q) {
        if (line[j] === '\\' && j + 1 < n) j += 2;
        else j++;
      }
      if (j < n) j++;
      out += `<span class="tok-str">${escapeHtml(line.slice(i, j))}</span>`;
      i = j;
      continue;
    }
    if (ch === '"' || ch === "'") {
      const q = ch;
      let j = i + 1;
      while (j < n && line[j] !== q) {
        if (line[j] === '\\' && j + 1 < n) j += 2;
        else j++;
      }
      if (j < n) j++;
      out += `<span class="tok-str">${escapeHtml(line.slice(i, j))}</span>`;
      i = j;
      continue;
    }
    if (/[0-9]/.test(ch) && (i === 0 || !/[A-Za-z0-9_]/.test(line[i - 1]))) {
      let j = i + 1;
      while (j < n && /[0-9._xXa-fA-F]/.test(line[j])) j++;
      out += `<span class="tok-num">${escapeHtml(line.slice(i, j))}</span>`;
      i = j;
      continue;
    }
    if (/[A-Za-z_]/.test(ch)) {
      let j = i + 1;
      while (j < n && /[A-Za-z0-9_]/.test(line[j])) j++;
      const word = line.slice(i, j);
      if (PY_KEYWORDS.has(word)) {
        out += `<span class="tok-kw">${word}</span>`;
      } else if (PY_BUILTINS.has(word)) {
        out += `<span class="tok-bi">${word}</span>`;
      } else {
        out += escapeHtml(word);
      }
      i = j;
      continue;
    }
    out += escapeHtml(ch);
    i++;
  }
  return out;
}

function syncEditorVisuals() {
  if (!editor || !gutterEl || !highlightEl) return;
  const text = editor.value || '';
  const lines = text.split('\n');
  let nums = '';
  for (let i = 1; i <= lines.length; i++) {
    nums += i + (i < lines.length ? '\n' : '');
  }
  gutterEl.textContent = nums;
  highlightEl.innerHTML = lines.map(highlightPythonLine).join('\n') + '\n';
  syncEditorScroll();
}

function syncEditorScroll() {
  if (!editor || !gutterEl || !highlightEl) return;
  gutterEl.scrollTop = editor.scrollTop;
  highlightEl.scrollTop = editor.scrollTop;
  highlightEl.scrollLeft = editor.scrollLeft;
}

function appendConsole(level, modId, message, timeSecs = null) {
  if (!consoleEl) return;
  const div = document.createElement('div');
  div.className = `log-line log-${level}`;
  const ts =
    typeof timeSecs === 'number' && Number.isFinite(timeSecs)
      ? `${timeSecs.toFixed(1)}s`
      : new Date().toLocaleTimeString([], { hour12: false });
  const prefix = modId ? `[${modId}] ` : '';
  div.innerHTML = `<span class="log-ts">${escapeHtml(ts)}</span>${escapeHtml(prefix + message)}`;
  consoleEl.appendChild(div);
  while (consoleEl.childNodes.length > 250) {
    consoleEl.removeChild(consoleEl.firstChild);
  }
  consoleEl.scrollTop = consoleEl.scrollHeight;
}

function setReloadBadge(text, state = '') {
  if (!reloadBadgeEl) return;
  reloadBadgeEl.textContent = text;
  reloadBadgeEl.className = state ? state : '';
}

function setGameStatus(text) {
  if (gameStatusEl) gameStatusEl.textContent = text;
}

function setOverlayAssetStatus(text) {
  if (overlayAssetStatusEl) overlayAssetStatusEl.textContent = text;
}

function renderResmon(rawJson) {
  if (!resmonListEl || !resmonSummaryEl) return;
  let data;
  try {
    data = JSON.parse(rawJson || '{}');
  } catch {
    return;
  }
  const plugins = Array.isArray(data.plugins) ? data.plugins : [];
  const activeCount = plugins.filter((p) => p.enabled).length;
  resmonSummaryEl.textContent = `${activeCount} active / ${plugins.length} loaded`;
  if (plugins.length === 0) {
    resmonListEl.innerHTML =
      '<div class="resmon-row"><span>No plugin loaded</span><span class="resmon-status-queued">Idle</span><span>0 calls</span><div class="fuel-track"><div class="fuel-fill" style="width:0%"></div></div><span>0 fuel</span></div>';
    return;
  }
  const rowsHtml = plugins
    .map((p) => {
      const status = String(p.status || 'Unknown');
      const cls = status.startsWith('Faulted')
        ? 'resmon-status-err'
        : status === 'Active'
          ? 'resmon-status-ok'
          : 'resmon-status-queued';
      const fuel = Number(p.fuel_used || 0);
      const calls = Number(p.hook_calls || 0);
      const fuelPerCall = calls > 0 ? fuel / calls : fuel;
      const pct = Math.min(100, Math.max(fuel > 0 ? 4 : 0, Math.round((fuelPerCall / 200000) * 100)));
      const fillCls = pct >= 70 ? 'hot' : pct >= 25 ? 'warn' : '';
      const avgUs = Number(p.avg_us || 0);
      const hooksLabel = Array.isArray(p.hooks) && p.hooks.length > 0 ? ` title="Hooks: ${escapeHtml(p.hooks.join(', '))}"` : '';
      const cpuText = avgUs > 0 ? `${avgUs.toFixed(0)}us` : `${calls} calls`;
      return `<div class="resmon-row"${hooksLabel}>` +
        `<span>${escapeHtml(p.id)}</span>` +
        `<span class="${cls}">${escapeHtml(status)}</span>` +
        `<span>${escapeHtml(cpuText)}</span>` +
        `<div class="fuel-track"><div class="fuel-fill ${fillCls}" style="width:${pct}%"></div></div>` +
        `<span>${fuel.toLocaleString()} fuel</span>` +
        `</div>`;
    })
    .join('');
  resmonListEl.innerHTML = rowsHtml;
}

function pollModTelemetry() {
  if (!cs16Web) return;
  try {
    const resmonJson = cs16Web.modResmonJson?.();
    if (resmonJson) renderResmon(resmonJson);
  } catch {}
  try {
    const logsJson = cs16Web.drainModLogsJson?.();
    if (logsJson && logsJson !== '[]') {
      const entries = JSON.parse(logsJson);
      if (Array.isArray(entries)) {
        for (const e of entries) {
          appendConsole(e.level || 'log', e.mod_id || '', e.message || '', e.time);
        }
      }
    }
  } catch {}
}

function hotReloadEditor({ unloadPrevious = false } = {}) {
  if (!cs16Web || !editor) return false;
  const code = editor.value;
  if (unloadPrevious && activePluginId) {
    try {
      cs16Web.unloadPythonPlugin?.(activePluginId);
    } catch {}
  }
  const t0 = performance.now();
  try {
    const raw = cs16Web.reloadPythonPlugin(activePluginName, code);
    const report = JSON.parse(raw);
    const elapsedMs =
      typeof report.load_ms === 'number' && report.load_ms > 0
        ? report.load_ms
        : Math.max(0.01, performance.now() - t0);
    activePluginId = report.id || activePluginId;
    const hooksN = Number(report.hook_count || 0);
    const cmdsN = Number(report.chat_command_count || 0) + Number(report.console_command_count || 0);
    const modeTag = report.queued ? 'Queued' : 'Reloaded';
    setReloadBadge(
      `${modeTag} in ${elapsedMs.toFixed(2)} ms • ${hooksN} hooks${cmdsN ? ` • ${cmdsN} cmds` : ''}`,
      'ok'
    );
    pollModTelemetry();
    return true;
  } catch (err) {
    const msg = String(err?.message || err || 'Python reload failed');
    setReloadBadge('Syntax / Runtime Error (live match kept intact)', 'err');
    appendConsole('error', activePluginId || activePluginName, msg);
    pollModTelemetry();
    return false;
  }
}

function unloadCurrentPlugin() {
  if (!cs16Web) return;
  const target = activePluginId || activePluginName;
  const ok = cs16Web.unloadPythonPlugin?.(target);
  if (ok) {
    setReloadBadge(`Unloaded ${target}`);
  } else {
    setReloadBadge(`Plugin ${target} was not loaded`);
  }
  pollModTelemetry();
}

function loadTemplateByKey(key, { autoReload = true } = {}) {
  const tpl = STARTER_TEMPLATES[key] || STARTER_TEMPLATES.vampire_shockwave;
  const prevId = activePluginId;
  activePluginName = tpl.file;
  activePluginId = tpl.id;
  if (templateSelect && templateSelect.value !== tpl.id) {
    templateSelect.value = tpl.id;
  }
  if (editor) {
    editor.value = tpl.code;
    syncEditorVisuals();
  }
  if (autoReload && cs16Web) {
    if (prevId && prevId !== tpl.id) {
      cs16Web.unloadPythonPlugin?.(prevId);
    }
    hotReloadEditor();
  }
}

async function resolveInitialPluginCode() {
  const params = new URLSearchParams(window.location.search);
  const modParam = (params.get('mod') || params.get('template') || '').trim();

  // 1. Fast path: localStorage handoff from /mods/ Plugin Hub or /docs/ AMXX Converter
  try {
    const rawImport = localStorage.getItem('cs16_playground_import');
    if (rawImport) {
      const parsed = JSON.parse(rawImport);
      if (parsed && typeof parsed.code === 'string' && parsed.code.trim().length > 0) {
        const importId = String(parsed.id || 'imported_mod').trim();
        if (!modParam || importId === modParam) {
          localStorage.removeItem('cs16_playground_import');
          activePluginId = importId;
          activePluginName = importId.endsWith('.py') ? importId : `${importId}.py`;
          if (editor) {
            editor.value = parsed.code;
            syncEditorVisuals();
          }
          appendConsole('info', activePluginId, `Imported "${parsed.name || activePluginName}" into Playground editor.`);
          return;
        }
      }
    }
  } catch {}

  // 2. Built-in starter template match (`?mod=vip_menu`)
  if (modParam && STARTER_TEMPLATES[modParam]) {
    loadTemplateByKey(modParam, { autoReload: false });
    return;
  }

  // 3. Fetch `../mods/plugins.json` emitted by `xtask` Plugin Hub (`?mod=vip_system` etc.)
  if (modParam) {
    try {
      const resp = await fetch('../mods/plugins.json');
      if (resp.ok) {
        const catalog = await resp.json();
        const list = Array.isArray(catalog) ? catalog : catalog?.plugins || [];
        const match = list.find((p) => p && String(p.id) === modParam);
        if (match) {
          const pyFile = Array.isArray(match.source_files)
            ? match.source_files.find(
                (f) => f && typeof f.path === 'string' && f.path.toLowerCase().endsWith('.py')
              )
            : null;
          const code =
            (pyFile && typeof pyFile.content === 'string' && pyFile.content) ||
            (typeof match.playground_code === 'string' && match.playground_code) ||
            '';
          if (code.trim().length > 0) {
            activePluginId = String(match.id);
            activePluginName = `${activePluginId}.py`;
            if (editor) {
              editor.value = code;
              syncEditorVisuals();
            }
            appendConsole('info', activePluginId, `Loaded "${match.name || activePluginId}" from Plugin Hub.`);
            return;
          }
        }
      }
    } catch {}
  }

  // 4. Default starter template
  loadTemplateByKey('vampire_shockwave', { autoReload: false });
}

function refreshMaps(preferredMap = '') {
  if (!cs16Web || !mapSelect) return;
  const maps = cs16Web.listMaps?.() || [];
  if (maps.length === 0) return;
  mapSelect.innerHTML = '';
  const pick =
    (preferredMap && maps.includes(preferredMap) && preferredMap) ||
    (maps.includes('de_dust2') && 'de_dust2') ||
    (maps.includes('dm_rusthouse') && 'dm_rusthouse') ||
    maps[0];
  for (const m of maps) {
    const opt = document.createElement('option');
    opt.value = m;
    opt.textContent = m;
    if (m === pick) opt.selected = true;
    mapSelect.appendChild(opt);
  }
}

function ensureCanvasSize() {
  if (!canvas || !cs16Web) return;
  const rect = canvas.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return;
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const w = Math.max(1, Math.floor(rect.width * dpr));
  const h = Math.max(1, Math.floor(rect.height * dpr));
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
    cs16Web.resize?.(w, h);
  }
}

function startInstantDemo() {
  if (!cs16Web) return;
  audio?.unlock();
  cs16Web.unlockAudio?.();
  hotReloadEditor();
  const bots = Math.max(0, Math.min(15, Number(botsInput?.value || 9)));
  const t0 = performance.now();
  const res = cs16Web.startInstantDemo?.(bots);
  const ms = Math.max(1, Math.round(performance.now() - t0));
  if (!res || !res.ok) {
    setGameStatus(`Instant Demo failed: ${res?.error || 'unavailable'}`);
    return;
  }
  const mapName = res.map || 'dm_rusthouse';
  refreshMaps(mapName);
  canvasOverlay?.classList.add('hidden');
  setGameStatus(`Live: ${mapName} (${bots} bots, booted in ${ms} ms) — Click canvas to aim, Esc to edit`);
  setOverlayAssetStatus(`Instant Demo ready (${res.files || 0} procedural files)`);
  pollModTelemetry();
}

function startSelectedMap() {
  if (!cs16Web) return;
  const map = mapSelect?.value || 'dm_rusthouse';
  const maps = cs16Web.listMaps?.() || [];
  if (map === 'dm_rusthouse' && !maps.includes('dm_rusthouse')) {
    startInstantDemo();
    return;
  }
  audio?.unlock();
  cs16Web.unlockAudio?.();
  hotReloadEditor();
  const bots = Math.max(0, Math.min(15, Number(botsInput?.value || 9)));
  cs16Web.startLocal?.(map, bots);
  canvasOverlay?.classList.add('hidden');
  setGameStatus(`Live: ${map} (${bots} bots) — Click canvas to aim, Esc to edit`);
  pollModTelemetry();
}

async function mountSteamBatch(loaderPromise) {
  if (!cs16Web) return;
  setOverlayAssetStatus('Mounting local Steam Half-Life folder...');
  const mounted = await loaderPromise;
  cs16Web.finishMount?.();
  await cacheAssetsInDb(mounted);
  refreshMaps();
  const count = cs16Web.listMaps?.().length || 0;
  setOverlayAssetStatus(`Mounted ${mounted.length} files (${count} maps ready — select de_dust2 and click Launch!)`);
  setGameStatus(`Steam assets mounted (${count} maps available)`);
}

function bindEditorBehavior() {
  if (!editor) return;
  editor.addEventListener('input', syncEditorVisuals);
  editor.addEventListener('scroll', syncEditorScroll);
  editor.addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
      e.preventDefault();
      hotReloadEditor();
      return;
    }
    if (e.key === 'Tab') {
      e.preventDefault();
      const start = editor.selectionStart;
      const end = editor.selectionEnd;
      const val = editor.value;
      if (e.shiftKey) {
        const lineStart = val.lastIndexOf('\n', start - 1) + 1;
        if (val.slice(lineStart, lineStart + 4) === '    ') {
          editor.value = val.slice(0, lineStart) + val.slice(lineStart + 4);
          editor.selectionStart = Math.max(lineStart, start - 4);
          editor.selectionEnd = Math.max(lineStart, end - 4);
          syncEditorVisuals();
        }
      } else {
        editor.value = val.slice(0, start) + '    ' + val.slice(end);
        editor.selectionStart = editor.selectionEnd = start + 4;
        syncEditorVisuals();
      }
      return;
    }
    if (e.key === 'Enter') {
      const start = editor.selectionStart;
      const end = editor.selectionEnd;
      const val = editor.value;
      const lineStart = val.lastIndexOf('\n', start - 1) + 1;
      const currentLine = val.slice(lineStart, start);
      const indentMatch = currentLine.match(/^(\s*)/);
      let indent = indentMatch ? indentMatch[1] : '';
      if (currentLine.trimEnd().endsWith(':')) {
        indent += '    ';
      }
      e.preventDefault();
      editor.value = val.slice(0, start) + '\n' + indent + val.slice(end);
      editor.selectionStart = editor.selectionEnd = start + 1 + indent.length;
      syncEditorVisuals();
    }
  });
}

async function main() {
  bindEditorBehavior();
  await resolveInitialPluginCode();

  templateSelect?.addEventListener('change', () => {
    loadTemplateByKey(templateSelect.value, { autoReload: true });
  });

  document.getElementById('btn-reset-template')?.addEventListener('click', () => {
    loadTemplateByKey(templateSelect?.value || 'vampire_shockwave', { autoReload: true });
  });

  document.getElementById('btn-download-py')?.addEventListener('click', () => {
    const blob = new Blob([editor?.value || ''], { type: 'text/x-python;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = activePluginName || 'plugin.py';
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  });

  document.getElementById('btn-hot-reload')?.addEventListener('click', () => hotReloadEditor());
  document.getElementById('btn-unload-plugin')?.addEventListener('click', () => unloadCurrentPlugin());
  document.getElementById('btn-clear-console')?.addEventListener('click', () => {
    if (consoleEl) consoleEl.innerHTML = '';
  });

  for (const btn of document.querySelectorAll('.trigger-btn')) {
    btn.addEventListener('click', () => {
      const cmd = btn.getAttribute('data-cmd');
      if (!cmd || !cs16Web) return;
      if (!cs16Web.isSessionActive?.()) {
        startInstantDemo();
      }
      cs16Web.execCommand?.(cmd);
      appendConsole('info', 'cmd', `> ${cmd}`);
      setTimeout(pollModTelemetry, 80);
    });
  }

  const runQuickCmd = () => {
    const cmd = quickCmdInput?.value?.trim();
    if (!cmd || !cs16Web) return;
    if (!cs16Web.isSessionActive?.()) {
      startInstantDemo();
    }
    cs16Web.execCommand?.(cmd);
    appendConsole('info', 'cmd', `> ${cmd}`);
    quickCmdInput.value = '';
    setTimeout(pollModTelemetry, 80);
  };
  document.getElementById('btn-send-cmd')?.addEventListener('click', runQuickCmd);
  quickCmdInput?.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      runQuickCmd();
    }
  });

  try {
    const wasmMod = await import('../pkg/cs_web.js');
    await wasmMod.default();
  } catch (err) {
    if (gpuPill) {
      gpuPill.textContent = 'WASM missing — run cargo xtask web-build';
      gpuPill.className = 'pill err';
    }
    appendConsole('error', 'wasm', `Failed to load ../pkg/cs_web.js: ${err}`);
    return;
  }

  cs16Web = window.cs16Web;
  if (!cs16Web) {
    if (gpuPill) {
      gpuPill.textContent = 'cs16Web failed to initialize';
      gpuPill.className = 'pill err';
    }
    return;
  }

  ensureCanvasSize();
  if (typeof ResizeObserver !== 'undefined' && canvas?.parentElement) {
    new ResizeObserver(() => ensureCanvasSize()).observe(canvas.parentElement);
  }
  window.addEventListener('resize', ensureCanvasSize);

  try {
    await cs16Web.init(canvas);
    if (gpuPill) {
      gpuPill.textContent = `Ready (${cs16Web.rendererInfo?.() || 'WebGPU/WebGL2'})`;
      gpuPill.className = 'pill ok';
    }
  } catch (err) {
    if (gpuPill) {
      gpuPill.textContent = `GPU init failed: ${err}`;
      gpuPill.className = 'pill err';
    }
  }

  audio = new WebAudioMixer((path) => cs16Web.readFile?.(path));
  input = new WebInputController(canvas, cs16Web, {
    onGesture: () => {
      audio.unlock();
      cs16Web.unlockAudio?.();
    }
  });
  canvas?.addEventListener('mousedown', () => {
    if (document.activeElement && document.activeElement !== document.body) {
      document.activeElement.blur?.();
    }
  });

  // Restore cached Steam assets from IndexedDB if user previously mounted Half-Life
  try {
    const cached = await hydrateAssetsFromDb(
      (p, b) => cs16Web.mountFile?.(p, b),
      (n) => setOverlayAssetStatus(`Restoring cached Steam assets (${n})...`)
    );
    if (cached > 0) {
      cs16Web.finishMount?.();
      refreshMaps();
      const mapCount = cs16Web.listMaps?.().length || 0;
      setOverlayAssetStatus(`Restored ${cached} cached Steam assets (${mapCount} maps ready)`);
    }
  } catch {}

  // Pre-validate & queue the initial editor plugin so starting Instant Demo activates it on tick 0
  hotReloadEditor();

  document.getElementById('btn-start-demo')?.addEventListener('click', startInstantDemo);
  document.getElementById('btn-overlay-demo')?.addEventListener('click', startInstantDemo);
  document.getElementById('btn-start-map')?.addEventListener('click', startSelectedMap);
  document.getElementById('btn-fullscreen')?.addEventListener('click', () => {
    canvas?.requestFullscreen?.().catch(() => input?.toggleFullscreen());
  });

  const openSteamPicker = () => {
    if (typeof window.showDirectoryPicker === 'function') {
      mountSteamBatch(
        mountFromDirectoryPicker(
          (p, b) => cs16Web.mountFile?.(p, b),
          (n, cur) => setOverlayAssetStatus(`Reading ${n}: ${cur}`)
        )
      ).catch((err) => setOverlayAssetStatus(`Mount cancelled/failed: ${err.message}`));
    } else {
      document.getElementById('steam-dir-fallback')?.click();
    }
  };
  document.getElementById('btn-mount-steam')?.addEventListener('click', openSteamPicker);
  document.getElementById('btn-overlay-steam')?.addEventListener('click', openSteamPicker);
  document.getElementById('steam-dir-fallback')?.addEventListener('change', (e) => {
    const files = e.target.files;
    if (files && files.length > 0) {
      mountSteamBatch(
        mountFromFileList(
          files,
          (p, b) => cs16Web.mountFile?.(p, b),
          (n, cur) => setOverlayAssetStatus(`Reading ${n}: ${cur}`)
        )
      ).catch((err) => setOverlayAssetStatus(`Mount failed: ${err.message}`));
    }
  });

  window.addEventListener('dragover', (e) => e.preventDefault());
  window.addEventListener('drop', (e) => {
    e.preventDefault();
    mountSteamBatch(
      mountFromDropEvent(
        e,
        (p, b) => cs16Web.mountFile?.(p, b),
        (n, cur) => setOverlayAssetStatus(`Dropped ${n}: ${cur}`)
      )
    ).catch((err) => setOverlayAssetStatus(`Drop failed: ${err.message}`));
  });

  function frame(nowMs) {
    input.pollGamepadAndTouch();
    cs16Web.stepAndRender?.(nowMs);
    const audioEvents = cs16Web.takeAudioEvents?.();
    if (audio && audioEvents && audioEvents.length > 0) {
      audio.processEvents(audioEvents);
    }
    if (nowMs - lastTelemetryMs >= 250) {
      lastTelemetryMs = nowMs;
      pollModTelemetry();
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

main();
