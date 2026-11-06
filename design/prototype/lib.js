// Shared components for the prototype. Query params: ?theme=day|oled &tier=lens|live|baked|solid &hc=1 &motion=reduce &stress=1 &lang=de &input=remote|pad
const Q = new URLSearchParams(location.search);
const TIER = Q.get('hc') === '1' ? 'solid' : (Q.get('tier') || 'lens');
document.documentElement.dataset.tier = TIER;
if (Q.get('hc') === '1') document.documentElement.dataset.hc = '1';
if (Q.get('motion') === 'reduce') document.documentElement.dataset.motion = 'reduce';
const STRESS = Q.get('stress') === '1';
// High contrast is always black. Desktop has one look, and the stream menu stays night over a game in every theme.
const NIGHT_ONLY = Q.get('hc') === '1' || /tv-stream|desktop-|banner/.test(location.pathname);
const THEME = NIGHT_ONLY ? 'night' : (Q.get('theme') || 'night');
const DAY = THEME === 'day';
document.documentElement.dataset.theme = THEME;
// What the ambient fades into.
const FADE_TO = { day: '246,247,251', oled: '0,0,0' }[THEME];
const themeFade = (f) => (FADE_TO ? f.replace(/rgba\(\d+,\d+,\d+,/g, `rgba(${FADE_TO},`).replace(/#[0-9a-f]{6}/gi, `rgb(${FADE_TO})`) : f);

const ICONS = {
  home: '<path d="M6.8 7.5h10.4a4.6 4.6 0 0 1 4.5 3.7l.8 4.2a2.7 2.7 0 0 1-4.7 2.3l-1.6-1.9H8.8l-1.6 1.9a2.7 2.7 0 0 1-4.7-2.3l.8-4.2a4.6 4.6 0 0 1 4.5-3.7Z"/><path d="M8.2 10.2v3M6.7 11.7h3"/><circle cx="15.3" cy="12.9" r=".5" fill="currentColor"/><circle cx="17.3" cy="10.9" r=".5" fill="currentColor"/>',
  pcs: '<rect x="2.8" y="4" width="18.4" height="12.6" rx="2.8"/><path d="M12 16.6V20M8 20h8"/>',
  settings: '<circle cx="12" cy="12" r="3.1"/><path d="M12 2.9v2.3M12 18.8v2.3M21.1 12h-2.3M5.2 12H2.9M18.4 5.6l-1.6 1.6M7.2 16.8l-1.6 1.6M18.4 18.4l-1.6-1.6M7.2 7.2 5.6 5.6"/>',
  search: '<circle cx="11" cy="11" r="6.6"/><path d="m16 16 4.6 4.6"/>',
  grid: '<rect x="3.6" y="3.6" width="7" height="7" rx="2"/><rect x="13.4" y="3.6" width="7" height="7" rx="2"/><rect x="3.6" y="13.4" width="7" height="7" rx="2"/><rect x="13.4" y="13.4" width="7" height="7" rx="2"/>',
  right: '<path d="m9.5 5.5 6.5 6.5-6.5 6.5"/>', down: '<path d="m5.5 9.5 6.5 6.5 6.5-6.5"/>', back: '<path d="M14.5 5.5 8 12l6.5 6.5"/>',
  play: '<path d="M8 5.4c0-1 1.1-1.6 2-1.1l10.4 6.6c.8.5.8 1.7 0 2.2L10 19.7c-.9.5-2-.1-2-1.1Z" fill="currentColor" stroke="none"/>',
  star: '<path d="m12 3.6 2.6 5.4 5.9.8-4.3 4.1 1 5.9L12 17l-5.2 2.8 1-5.9-4.3-4.1 5.9-.8Z"/>',
  x: '<path d="m6 6 12 12M18 6 6 18"/>', plus: '<path d="M12 5v14M5 12h14"/>', check: '<path d="m5 12.5 4.6 4.6L19 7.5"/>',
  wifi: '<path d="M2.8 9.2a13.4 13.4 0 0 1 18.4 0M6 12.8a8.8 8.8 0 0 1 12 0M9.2 16.2a4 4 0 0 1 5.6 0"/><circle cx="12" cy="19.2" r=".7" fill="currentColor"/>',
  keyboard: '<rect x="2.6" y="6" width="18.8" height="12" rx="2.8"/><path d="M6.5 10h.01M10 10h.01M14 10h.01M17.5 10h.01M8 14.2h8"/>',
  mouse: '<rect x="6.4" y="2.8" width="11.2" height="18.4" rx="5.6"/><path d="M12 6.5v3.4"/>',
  pad: '<path d="M6.8 7.5h10.4a4.6 4.6 0 0 1 4.5 3.7l.8 4.2a2.7 2.7 0 0 1-4.7 2.3l-1.6-1.9H8.8l-1.6 1.9a2.7 2.7 0 0 1-4.7-2.3l.8-4.2a4.6 4.6 0 0 1 4.5-3.7Z"/>',
  power: '<path d="M12 3.4v8"/><path d="M6.6 6.8a8 8 0 1 0 10.8 0"/>', bars: '<path d="M5 20v-7M12 20V5M19 20v-11"/>',
  video: '<rect x="2.8" y="5" width="13" height="14" rx="3"/><path d="m15.8 10.4 5.4-3v9.2l-5.4-3"/>',
  display: '<circle cx="12" cy="12" r="3.6"/><path d="M12 3v2M12 19v2M3 12h2M19 12h2M5.6 5.6 7 7M17 17l1.4 1.4M5.6 18.4 7 17M17 7l1.4-1.4"/>',
  speaker: '<path d="M4 9.6v4.8h3.2l5 4V5.6l-5 4Z"/><path d="M16 9a4.2 4.2 0 0 1 0 6M18.6 6.4a8 8 0 0 1 0 11.2"/>',
  stream: '<path d="M4 8.5a11 11 0 0 1 16 0"/><path d="M7.6 12.2a6 6 0 0 1 8.8 0"/><path d="M12 15.5v5"/>',
  app: '<rect x="4" y="4" width="16" height="16" rx="4.5"/><path d="M9 12h6"/>', tune: '<path d="M4 7h9M17 7h3M4 17h3M11 17h9"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/>',
  moon: '<path d="M20 14.2A8.4 8.4 0 0 1 9.8 4a8.4 8.4 0 1 0 10.2 10.2Z"/>', more: '<circle cx="5.5" cy="12" r="1.3" fill="currentColor"/><circle cx="12" cy="12" r="1.3" fill="currentColor"/><circle cx="18.5" cy="12" r="1.3" fill="currentColor"/>',
  lock: '<rect x="5" y="10.5" width="14" height="10" rx="3"/><path d="M8 10.5V8a4 4 0 0 1 8 0v2.5"/>', del: '<path d="M8.6 5.5H20a1.5 1.5 0 0 1 1.5 1.5v10a1.5 1.5 0 0 1-1.5 1.5H8.6L2.5 12Z"/><path d="m12 9.6 4.8 4.8M16.8 9.6 12 14.4"/>',
  mic: '<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11.5a6.5 6.5 0 0 0 13 0M12 18v3"/>', exit: '<path d="M10 4H6.5A2.5 2.5 0 0 0 4 6.5v11A2.5 2.5 0 0 0 6.5 20H10"/><path d="M15 8l4 4-4 4M19 12H10"/>',
};
const icon = (n, s = 24, w = 1.8) => `<svg width="${s}" height="${s}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="${w}" stroke-linecap="round" stroke-linejoin="round" style="flex:none;display:block">${ICONS[n]}</svg>`;
// The brand mark, same path as theme.rs and ic_nightjar_foreground.xml.
const BIRD = 'M 51.75,41.24 C 53.31,40.96 54.50,41.92 54.89,43.31 C 55.16,44.00 55.33,44.55 55.55,44.95 C 58.63,42.95 61.40,40.41 64.45,38.63 C 68.66,39.72 73.12,42.22 78.13,45.72 C 72.88,44.97 68.19,45.36 64.07,46.82 C 60.97,47.94 58.40,49.93 56.11,51.87 L 56.49,56.49 L 59.86,66.49 C 58.86,67.47 57.71,67.97 56.50,68.18 C 55.30,68.39 54.04,68.32 52.77,67.74 L 52.51,57.19 L 51.29,52.72 C 48.47,51.68 45.38,50.70 42.08,50.69 C 37.70,50.73 33.17,51.97 28.50,54.48 C 32.00,49.47 35.34,45.59 38.92,43.13 C 42.40,43.76 45.87,45.20 49.45,46.03 C 49.52,45.58 49.49,45.00 49.51,44.26 C 49.40,42.82 50.19,41.51 51.75,41.24 Z';
const mark = (s = 32) => `<svg width="${s}" height="${s}" viewBox="27 27 54 54" style="flex:none;display:block"><defs><linearGradient id="mk" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#FFC765"/><stop offset="1" stop-color="#EE9F2E"/></linearGradient></defs><circle cx="54" cy="54" r="27" fill="url(#mk)"/><path d="${BIRD}" fill="#14161F"/></svg>`;

// When stress=1 every cover is the theme's worst case for glass contrast: white for night, black for day.
const WHITE = 'data:image/svg+xml,' + encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="600" height="900"><rect width="600" height="900" fill="${DAY ? '#000' : '#fff'}"/></svg>`);
const coverSrc = (n) => (STRESS ? WHITE : `covers/${n}.png`);
const cover = (n, { w, cap, cls = '', style = '', badge = '' } = {}) => `<div class="${cap ? '' : ''}" style="flex:none;${w ? `width:${w}px;` : ''}"><div class="cover ${cls}" style="${style}"><img src="${coverSrc(n)}" alt=""><div class="sheen"></div><div class="lip"></div>${badge}</div>${cap ? `<div class="cap">${cap}</div>` : ''}</div>`;

// A glass element. Its own radius is inherited by the layers.
const glass = (mat, { cls = '', style = '', attrs = '' } = {}, inner = '') => `<div class="g ${mat} ${cls}" style="${style}" ${attrs}><i class="g-blur"></i><i class="g-lens"></i><i class="g-sheen"></i><i class="g-rim"></i><div class="g-c">${inner}</div></div>`;

const statusBar = () => `<div style="position:absolute;left:0;right:0;top:0;height:30px;display:flex;align-items:center;justify-content:space-between;padding:4px 24px 0;font:600 13px/16px Figtree;z-index:9;color:var(--color-label)"><span class="num">20:14</span><span style="display:flex;gap:6px;align-items:center">${icon('wifi', 15, 2.2)}<svg width="22" height="11" viewBox="0 0 22 11"><rect x=".5" y=".5" width="18" height="10" rx="3" fill="none" stroke="currentColor" opacity=".6"/><rect x="2" y="2" width="12" height="7" rx="1.6" fill="currentColor"/><rect x="20" y="3.5" width="1.6" height="4" rx=".8" fill="currentColor" opacity=".6"/></svg></span></div>`;

const tabBar = (active = 0, labels = ['Home', 'PCs', 'Settings']) => {
  const ic = ['home', 'pcs', 'settings'];
  const w = 100 / 3;
  return `<div class="tabfade"></div><div style="position:absolute;left:16px;right:16px;bottom:18px;height:66px;display:flex;gap:10px;z-index:8">
    ${glass('chrome', { style: 'flex:1;border-radius:33px;height:66px', cls: 'tabbar' }, `<div class="tabs" style="position:relative;height:66px;box-sizing:border-box">
      <div class="pill" style="left:calc(4px + (100% - 8px) * ${active / 3});width:calc((100% - 8px) / 3)"></div>
      ${labels.map((l, i) => `<button class="tab press ${i === active ? 'on' : ''}">${icon(ic[i], 25, i === active ? 2 : 1.8)}<span>${l}</span></button>`).join('')}
    </div>`)}
    ${glass('chrome', { style: 'width:66px;height:66px;border-radius:50%;flex:none' }, `<button class="press" style="width:66px;height:66px;display:grid;place-items:center;border-radius:50%">${icon('search', 25)}</button>`)}
  </div>`;
};

// The horizontal capsule nav for handheld, TV, desktop.
const capsuleNav = (active = 0, { h = 52 } = {}) => {
  const items = [['home', 'Home'], ['pcs', 'PCs'], ['settings', 'Settings']];
  return glass('chrome', { style: `border-radius:${h / 2}px;height:${h}px;flex:none` }, `<div style="display:flex;padding:4px;height:${h}px;gap:0">${items.map(([ic, l], i) => `<button class="tab tab-h press ${i === active ? 'on' : ''}" style="${i === active ? 'background:var(--tint-selected-fill);box-shadow:inset 0 1px 0 var(--tint-selected-rimTop),0 2px 10px rgba(0,0,0,.25);' : ''}">${icon(ic, 22, i === active ? 2 : 1.8)}<span>${l}</span></button>`).join('')}</div>`);
};
const pcCapsule = (name = 'Living-room PC', h = 48) => glass('chrome', { style: `border-radius:${h / 2}px;height:${h}px;flex:none` }, `<button class="press" style="height:${h}px;display:flex;align-items:center;gap:9px;padding:0 14px 0 16px;border-radius:999px;font:600 15px/20px Figtree;white-space:nowrap"><span class="dot"></span>${name}${icon('down', 17, 2.2)}</button>`);
const roundGlass = (ic, h = 48, s = 22) => glass('chrome', { style: `width:${h}px;height:${h}px;border-radius:50%;flex:none` }, `<button class="press" style="width:${h}px;height:${h}px;display:grid;place-items:center;border-radius:50%">${icon(ic, s)}</button>`);

const ambient = (n, { fade, bg } = {}) => `<div class="ambient" style="${bg ? `--bg:${bg}` : ''}"><img src="ambient/${STRESS ? (DAY ? 'black' : 'white') : n}-${innerWidth < innerHeight ? 'p' : 'l'}${DAY ? '-day' : ''}.png" alt="" data-ambient><div class="fade" style="background:${themeFade(fade || 'linear-gradient(180deg,rgba(11,13,18,.28),rgba(11,13,18,.46) 38%,rgba(20,22,31,.94) 74%,#14161f)')}"></div></div>`;

// The night backdrop for pages with no art: the banner's moon and orbit rings, lit.
const nightSky = ({ x = 360, y = 40, r = 74, w = 412, h = 915 } = {}) => {
  const rng = (() => { let s = 7; return () => (s = (s * 16807) % 2147483647) / 2147483647; })();
  let stars = ''; for (let i = 0; i < 26; i++) if (!DAY) stars += `<circle cx="${(rng() * w).toFixed(0)}" cy="${(rng() * 118).toFixed(0)}" r="${(0.4 + rng() * 0.9).toFixed(2)}" fill="#fff" opacity="${(0.12 + rng() * 0.5).toFixed(2)}"/>`;
  return `<div class="night" data-night><svg width="${w}" height="${h}" viewBox="0 0 ${w} ${h}"><defs><radialGradient id="ng" cx="${x}" cy="${y}" r="${r * 6}" gradientUnits="userSpaceOnUse"><stop offset="0" style="stop-color:var(--sky-glow)"/><stop offset=".25" style="stop-color:var(--sky-glow);stop-opacity:.36"/><stop offset="1" style="stop-color:var(--sky-glow);stop-opacity:0"/></radialGradient><linearGradient id="nm" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#FFD27A"/><stop offset="1" stop-color="#EE9F2E"/></linearGradient></defs><rect width="${w}" height="${h}" fill="url(#ng)"/>${stars}${[1.7, 2.5, 3.3, 4.1].map((k) => `<circle cx="${x}" cy="${y}" r="${r * k}" fill="none" style="stroke:var(--sky-ring)"/>`).join('')}<circle cx="${x}" cy="${y}" r="${r}" fill="url(#nm)"/></svg></div>`;
};

const row = (title, { sub = '', val = '', ic = '', right = '', chev = false, cls = '', style = '' } = {}) => `<div class="row press ${ic ? 'ico' : ''} ${cls}" style="${style}">${ic ? `<span class="tile">${icon(ic, 19)}</span>` : ''}<div class="ti"><div class="t-body" style="font-weight:600;font-size:16px">${title}</div>${sub ? `<div class="t-sub sec num">${sub}</div>` : ''}</div>${val || chev ? `<span class="val t-body num">${val}${chev ? icon('right', 16, 2.2) : ''}</span>` : ''}${right}</div>`;
const sw = (on) => `<span class="sw ${on ? 'on' : ''}"></span>`;
const slider = (p) => `<div class="sl"><b style="width:${p}%"></b><u style="left:${p}%"></u></div>`;
const seg = (opts, on = 0) => `<div class="seg">${opts.map((o, i) => `<span class="${i === on ? 'on' : ''}">${o}</span>`).join('')}</div>`;

function mount(html, { w, h, zoom = 1, tv = false } = {}) {
  const app = document.getElementById('app') || Object.assign(document.createElement('div'), { id: 'app' });
  document.body.appendChild(app);
  app.style.width = w + 'px'; app.style.height = h + 'px';
  if (zoom !== 1) app.style.zoom = zoom;
  if (tv) app.classList.add('tv');
  // Day shadows are cool and light; the fixtures write them once, for night.
  if (DAY) html = html.replace(/rgba\(0,0,0,(\.\d+)\)/g, (_, a) => `rgba(42,46,74,${(a * 0.5).toFixed(2)})`);
  app.innerHTML = html;
  if (Q.get('lang') === 'de') german(app);
  if (TIER === 'baked' || TIER === 'solid') bake(app, w, h, zoom);
  // Tab pill follows the finger (spring is the CSS approximation of motion.tabPill).
  app.querySelectorAll('.tabbar .tab').forEach((t, i, all) => t.addEventListener('click', () => {
    all.forEach((o) => o.classList.remove('on')); t.classList.add('on');
    app.querySelector('.tabbar .pill').style.left = `calc(4px + (100% - 8px) * ${i / 3})`;
  }));
}

// Baked tier: each glass layer samples the same static blurred ambient in screen space, like BackdropBlur plus UVs.
function bake(app, w, h, zoom) {
  if (TIER !== 'baked') return;
  const src = app.querySelector('[data-ambient]');
  const night = app.querySelector('[data-night]');
  const box = app.getBoundingClientRect();
  app.querySelectorAll('.g').forEach((g) => {
    const layer = g.querySelector('.g-blur'); if (!layer) return;
    const r = layer.getBoundingClientRect();
    const dx = -(r.left - box.left) / zoom; const dy = -(r.top - box.top) / zoom;
    const bri = getComputedStyle(g).getPropertyValue('--bri');
    if (!src && !night) return;
    layer.style.isolation = 'isolate';
    if (night) {
      const c = night.cloneNode(true); c.removeAttribute('data-night');
      Object.assign(c.style, { position: 'absolute', left: dx + 'px', top: dy + 'px', width: w + 'px', height: h + 'px', inset: 'auto', zIndex: -1, filter: `blur(18px) brightness(${bri})`, transform: 'none' });
      layer.appendChild(c);
    } else {
      const c = document.createElement('img'); c.src = src.src; c.className = 'bake';
      Object.assign(c.style, { left: dx - w * 0.14 + 'px', top: dy - h * 0.14 + 'px', width: w * 1.28 + 'px', height: h * 1.28 + 'px', objectFit: 'cover', filter: `blur(22px) brightness(${bri})` });
      layer.appendChild(c);
    }
    layer.style.overflow = 'hidden';
  });
}

// The tile for an app with no art, exactly what Android and egui draw: a gradient pair and a disc picked by the title's
// hash, and the title. No image, no browser, no procedural art.
const fnv = (s) => { let h = 0x811c9dc5; for (const b of new TextEncoder().encode(s.toLowerCase())) { h ^= b; h = Math.imul(h, 0x01000193) >>> 0; } return h; };
const DISC_AT = [[0.74, 0.62], [0.26, 0.7], [0.8, 0.42], [0.5, 0.78]];
const pairOf = (name) => fnv(name) % 12;
const pairGradient = (name) => `linear-gradient(165deg,var(--tile-pair${pairOf(name)}-top),var(--tile-pair${pairOf(name)}-bottom))`;
const tile = (name, { w, cls = '', style = '', badge = '' } = {}) => {
  const [dx, dy] = DISC_AT[(fnv(name) >>> 8) % 4];
  const longest = Math.max(...name.split(/\s+/).map((p) => p.length));
  const scale = longest * 0.5 * 0.16 > 0.8 ? 'var(--tile-titleMinScale)' : 'var(--tile-titleScale)';
  const d = `calc(${w}px * var(--tile-discScale))`;
  return `<div style="flex:none;width:${w}px"><div class="cover na ${cls}" style="background:${pairGradient(name)};${style}">
    <i class="disc" style="width:${d};height:${d};left:calc(${w}px * ${dx} - ${d} / 2);top:calc(${w * 1.5}px * ${dy} - ${d} / 2)"></i>
    <div class="na-t" style="left:calc(${w}px * var(--tile-insetScale));right:calc(${w}px * var(--tile-insetScale));top:calc(${w}px * var(--tile-insetScale));font-size:calc(${w}px * ${scale})">${name}</div>
    <div class="sheen"></div><div class="lip"></div>${badge}</div></div>`;
};
// A game with art or without. n is a cover file, or null for the no-art tile.
const app = (n, name, o = {}) => (n && !o.noArt ? cover(n, o) : tile(name, o));
const runningBadge = '<span class="badge"><span class="dot"></span></span>';

// A PC is a small screen, the same on every form factor: a gradient from the name's hash, or what it's doing.
const pcScreen = (name, { state = 'online', w = 72, art } = {}) => {
  const h = Math.round(w * 0.62); const r = Math.max(5, Math.round(w * 0.075));
  const face = state === 'offline' ? 'background:var(--pc-off)' : `background:${pairGradient(name)}`;
  const glyph = state === 'offline' ? icon('moon', Math.round(w * 0.3), 1.7) : state === 'unpaired' ? icon('lock', Math.round(w * 0.3), 1.8) : '';
  return `<div style="flex:none;width:${w}px"><div class="pc ${state}" style="height:${h}px;border-radius:${r}px;${face}">${art ? `<img src="covers/${art}.png" alt="">` : `<i class="disc" style="width:${w * 0.7}px;height:${w * 0.7}px;left:${w * 0.42}px;top:${h * 0.2}px"></i>`}<span style="position:relative">${glyph}</span></div><div class="pc-neck" style="width:${w * 0.1}px;height:${w * 0.07}px"></div><div class="pc-base" style="width:${w * 0.3}px;height:${Math.max(2, w * 0.028)}px"></div></div>`;
};

// Button hints name what the last-used input device really has. Touch and mouse show none.
const INPUT = Q.get('input') || 'remote';
const HINTS = { remote: { go: 'OK', alt: 'Hold OK' }, pad: { go: 'A', alt: 'Y' } };
const hint = (which, label) => { const k = HINTS[INPUT][which]; return `<span class="hint"><span class="key ${k.length > 2 ? 'wide' : ''}">${k}</span>${label}</span>`; };

// The moon travelling an orbit: discovery and connecting. One ring, one dot, one rotation; it stops when there's a result.
const orbit = (r, { cx, cy, dur = 2.4 } = {}) => `<svg style="position:absolute;left:${cx - r - 12}px;top:${cy - r - 12}px;overflow:visible" width="${2 * r + 24}" height="${2 * r + 24}"><circle cx="${r + 12}" cy="${r + 12}" r="${r}" fill="none" style="stroke:var(--sky-ring)" stroke-width="1.5"/><g class="spin" style="transform-origin:${r + 12}px ${r + 12}px;animation-duration:${dur}s"><circle cx="${r + 12}" cy="12" r="6" fill="var(--color-moon)" style="filter:drop-shadow(0 0 8px var(--color-moon))"/></g></svg>`;

const phoneHomeMarkup = () => `
${ambient('racing-game', { fade: 'linear-gradient(180deg,rgba(14,14,22,0),rgba(14,14,22,0) 42%,rgba(20,22,31,.94) 63%,#14161f 78%)' })}
${statusBar()}
<div style="position:absolute;left:16px;right:16px;top:38px;display:flex;justify-content:space-between;z-index:7">${pcCapsule()}${roundGlass('grid')}</div>
<!-- Continue carousel -->
<div style="position:absolute;left:0;right:0;top:98px;height:468px">
  ${cover('open-world', { style: 'position:absolute;left:-168px;top:30px;width:232px', cls: 'dim' })}
  ${cover('ocean-explorer', { style: 'position:absolute;right:-168px;top:30px;width:232px', cls: 'dim' })}
  <div data-hero>${cover('racing-game', { style: 'position:absolute;left:61px;top:0;width:290px;box-shadow:0 26px 60px rgba(0,0,0,.6),0 0 90px rgba(255,95,70,.3)' })}
  <div style="position:absolute;left:0;right:0;top:406px;display:flex;justify-content:center;z-index:3"><button class="play press" style="min-width:214px">${icon('play', 22)}Resume</button></div></div>
</div>
<div style="position:absolute;left:16px;right:0;top:574px">
  <div class="hdr" style="padding-right:16px"><span class="t-section">Steam</span><span class="t-body sec">10</span><span style="margin-left:auto" class="sec">${icon('right', 18, 2.2)}</span></div>
  <div class="shelf" style="margin-top:12px;width:800px">
    ${cover('open-world', { w: 176, cap: 'Open World' })}${cover('ocean-explorer', { w: 176, cap: 'Ocean Explorer' })}${cover('space-sim', { w: 176, cap: 'Space Sim' })}
  </div>
</div>
${tabBar(0)}
`;

// A German pass over the fixtures, to prove the layouts take the long language.
function german(app) {
  const de = { Home: 'Start', Settings: 'Einstellungen', Resume: 'Fortsetzen', Favourite: 'Favorit', 'Quit game': 'Spiel beenden', Wake: 'Aufwecken', Pair: 'Koppeln', 'Add PC': 'PC hinzufügen', 'Searching for PCs…': 'Suche nach PCs…', Offline: 'Offline', 'Not paired': 'Nicht gekoppelt', Running: 'Läuft',
    'Stream settings': 'Stream-Einstellungen', 'Quit game when I leave': 'Spiel beim Verlassen beenden', 'Hide this game': 'Dieses Spiel ausblenden', 'Also on Steam': 'Auch auf Steam',
    'Living-room PC': 'Wohnzimmer-PC', Resolution: 'Auflösung', 'Frame rate': 'Bildrate', 'This screen': 'Dieser Bildschirm', 'Automatic bitrate': 'Automatische Bitrate', '80 Mbps now': 'Gerade 80 Mbps', Automatic: 'Automatisch',
    Touch: 'Berührung', Direct: 'Direkt', Vibration: 'Vibration', 'On-screen controller': 'Bildschirm-Controller', Language: 'Sprache', System: 'System', 'High contrast': 'Hoher Kontrast',
    'Display and image': 'Anzeige und Bild', Controls: 'Steuerung', Audio: 'Audio', Streaming: 'Streaming', Video: 'Video' };
  const walk = document.createTreeWalker(app, NodeFilter.SHOW_TEXT);
  let n; while ((n = walk.nextNode())) { const t = n.nodeValue.trim(); if (de[t]) n.nodeValue = de[t]; }
}
