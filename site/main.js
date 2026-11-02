// Moth — site behaviour. No dependencies. Everything here adds to a page that already reads
// without it: the held scenes only pin when this script runs and motion is allowed.

const reduced = matchMedia('(prefers-reduced-motion: reduce)');
const clamp = (v, a = 0, b = 1) => Math.min(b, Math.max(a, v));

/* ---------- hero: the real window crossfades through four states ---------- */
(() => {
  const shots = [...document.querySelectorAll('[data-crossfade] img')];
  const caption = document.querySelector('.hero__caption');
  if (shots.length < 2 || reduced.matches) return;
  let i = 0;
  let timer;
  const show = (n) => {
    shots[i].classList.remove('is-on');
    i = n % shots.length;
    const img = shots[i];
    const go = () => {
      img.classList.add('is-on');
      caption.textContent = img.dataset.caption;
    };
    img.loading = 'eager';
    img.complete ? go() : img.addEventListener('load', go, { once: true });
  };
  const start = () => { stop(); timer = setInterval(() => show(i + 1), 5600); };
  const stop = () => clearInterval(timer);
  document.addEventListener('visibilitychange', () => (document.hidden ? stop() : start()));
  setTimeout(() => shots.slice(1).forEach((img) => { img.loading = 'eager'; }), 1500);
  start();
})();

/* ---------- reveals: per part, staggered, once ---------- */
(() => {
  const groups = [
    ['.band', '.bands__list'],
    ['.tile', '.keys__grid'],
    ['.platform', '.platforms'],
  ];
  for (const [item, parent] of groups) {
    document.querySelectorAll(parent).forEach((p) => {
      [...p.querySelectorAll(item)].forEach((el, n) => el.style.setProperty('--i', n % 4));
    });
  }
  const targets = [...document.querySelectorAll('[data-reveal], .band, .tile, .platform')];
  if (!('IntersectionObserver' in window)) { targets.forEach((el) => el.classList.add('is-in')); return; }
  // a shutter starts fully clipped, which the observer reads as no area: its section triggers it instead
  const owner = new Map();
  const io = new IntersectionObserver((entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      (owner.get(e.target) || [e.target]).forEach((el) => el.classList.add('is-in'));
      io.unobserve(e.target);
    }
  }, { rootMargin: '0px 0px -10% 0px', threshold: 0 });
  for (const el of targets) {
    const trigger = el.classList.contains('shutter') ? el.closest('section') : el;
    if (trigger !== el) owner.set(trigger, [...(owner.get(trigger) || []), el]);
    io.observe(trigger);
  }
})();

/* ---------- headings whose lines rise as they arrive ---------- */
(() => {
  const els = [...document.querySelectorAll('[data-lines]')];
  if (!els.length || !('IntersectionObserver' in window) || reduced.matches) return;
  const restore = (el) => { el.textContent = el.dataset.text; el.classList.remove('is-split', 'is-in'); };
  const split = (el) => {
    const text = el.textContent.trim().replace(/\s+/g, ' ');
    el.dataset.text = text;
    el.textContent = '';
    const words = text.split(' ').map((word) => {
      const s = document.createElement('span');
      s.style.display = 'inline-block';
      s.textContent = word;
      el.append(s, ' ');
      return s;
    });
    const lines = [];
    for (const w of words) {
      const last = lines.at(-1);
      if (last && Math.abs(last.top - w.offsetTop) < 4) last.words.push(w.textContent);
      else lines.push({ top: w.offsetTop, words: [w.textContent] });
    }
    el.textContent = '';
    lines.forEach((line, i) => {
      const mask = document.createElement('span');
      const inner = document.createElement('span');
      mask.className = 'line';
      inner.style.setProperty('--i', i);
      inner.textContent = line.words.join(' ');
      mask.append(inner);
      el.append(mask, ' ');
    });
    el.classList.add('is-split');
  };
  const io = new IntersectionObserver((entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      io.unobserve(e.target);
      e.target.classList.add('is-in');
      e.target.querySelector('.line:last-of-type > span')
        ?.addEventListener('transitionend', () => restore(e.target), { once: true });
    }
  }, { rootMargin: '0px 0px -12% 0px' });
  document.fonts.ready.then(() => els.forEach((el) => { split(el); io.observe(el); }));
})();

/* ---------- scroll scenes: one loop sets each scene's progress ---------- */
const scenes = [];
const progressOf = (el) => {
  const r = el.getBoundingClientRect();
  const run = r.height - innerHeight;
  return run > 0 ? clamp(-r.top / run) : clamp(1 - r.bottom / (r.height + innerHeight));
};
let ticking = false;
const frame = () => {
  ticking = false;
  for (const s of scenes) s.update(progressOf(s.el));
};
const request = () => { if (!ticking) { ticking = true; requestAnimationFrame(frame); } };
addEventListener('scroll', request, { passive: true });
addEventListener('resize', request);

/* hero: as the page leaves the hero, the wings of the mark open */
(() => {
  const el = document.querySelector('[data-scene="hero"]');
  const mark = el?.querySelector('.hero__mark');
  if (!mark || reduced.matches) return;
  scenes.push({
    el,
    update() {
      const r = el.getBoundingClientRect();
      mark.style.setProperty('--p', clamp(-r.top / (r.height * 0.8)).toFixed(3));
    },
  });
})();

/* how it works: four real states, driven by the scroll or by the slider */
(() => {
  const el = document.querySelector('[data-scene="stages"]');
  if (!el) return;
  const copy = [...el.querySelector('#stage-copy').content.children];
  const title = el.querySelector('.stages__clause');
  const text = el.querySelector('.stages__text');
  const imgs = [...el.querySelectorAll('.stages__frame img')];
  const names = [...el.querySelectorAll('.stages__names li')];
  const range = el.querySelector('#stage-range');
  const fill = el.querySelector('.range-fill');
  let current = -1;
  const set = (n) => {
    if (n === current) return;
    current = n;
    title.textContent = copy[n].dataset.title;
    text.textContent = copy[n].textContent;
    imgs.forEach((img, k) => img.classList.toggle('is-on', k === n));
    names.forEach((li, k) => li.classList.toggle('is-on', k === n));
    range.value = n;
    range.setAttribute('aria-valuetext', copy[n].dataset.name);
    if (fill) fill.style.width = `${(n / 3) * 100}%`;
  };
  imgs.forEach((img) => { img.loading = 'eager'; });
  set(0);
  const held = () => el.classList.contains('is-held');
  range.addEventListener('input', () => {
    const n = Number(range.value);
    set(n);
    if (held()) {
      const top = el.getBoundingClientRect().top + scrollY;
      const run = el.offsetHeight - innerHeight;
      scrollTo({ top: top + run * ((n + 0.5) / 4), behavior: 'instant' });
    }
  });
  if (reduced.matches) return;
  el.classList.add('is-held');
  scenes.push({ el, update: (p) => set(Math.min(3, Math.floor(p * 4))) });
})();

/* panes: the picture opens from a window to the whole screen */
(() => {
  const el = document.querySelector('[data-scene="opening"]');
  const runway = el?.querySelector('.panes__runway');
  if (!runway || reduced.matches) return;
  el.classList.add('is-held');
  const stage = el.querySelector('.panes__stage');
  scenes.push({ el: runway, update: (p) => stage.style.setProperty('--open', (1 - clamp(p / 0.75)).toFixed(4)) });
})();

/* the name: the logbook page zooms into relay 70 */
(() => {
  const el = document.querySelector('[data-scene="logbook"]');
  if (!el) return;
  const lines = [...el.querySelectorAll('.logbook__lines li')];
  const img = el.querySelector('.logbook__picture img');
  const clock = el.querySelector('[data-clock]');
  const timeOf = (li) => (li.dataset.time === 'now' ? new Date().toTimeString().slice(0, 5) : li.dataset.time);
  img.loading = 'eager';
  if (reduced.matches) { clock.textContent = timeOf(lines.at(-1)); return; }
  el.classList.add('is-held');
  let last = -1;
  scenes.push({
    el,
    update(p) {
      const zoom = clamp((p - 0.12) / 0.6);
      const eased = zoom < 0.5 ? 2 * zoom * zoom : 1 - Math.pow(-2 * zoom + 2, 2) / 2;
      img.style.setProperty('--zoom', eased.toFixed(4));
      const n = p < 0.22 ? 0 : p < 0.5 ? 1 : p < 0.78 ? 2 : 3;
      if (n !== last) { last = n; lines.forEach((li, k) => li.classList.toggle('is-on', k === n)); clock.textContent = timeOf(lines[n]); }
    },
  });
})();

/* ---------- the live prompt, with the rules of src/highlight.rs ---------- */
(() => {
  const root = document.querySelector('#try');
  const input = root?.querySelector('#prompt-input');
  if (!input) return;
  const paint = root.querySelector('[data-paint]');
  const scrollback = root.querySelector('[data-scrollback]');
  const live = root.querySelector('.term__live');
  const chev = '<svg class="term__chev" viewBox="0 0 830 660" aria-hidden="true"><use href="#chev"/></svg>';

  const SUBCOMMAND_TOOLS = 'git cargo npm pnpm yarn bun docker kubectl brew go rustup gh pip pip3 uv poetry systemctl launchctl terraform helm deno swift xcrun npx make just'.split(' ');
  const PRECOMMANDS = 'sudo time env exec nohup command builtin noglob nice xargs watch'.split(' ');
  const KEYWORDS = 'if then else elif fi for in do done while until case esac function select repeat { } [[ ]] !'.split(' ');
  const BUILTINS = 'cd echo export source . alias unalias unset exit history pwd pushd popd type which eval exec set local read printf test [ true false kill jobs fg bg wait ulimit umask setopt unsetopt autoload bindkey zle print whence rehash hash trap return shift getopts let typeset declare functions emulate disown builtin command time sudo dirs logout zmodload compdef'.split(' ');
  // What `$PATH` holds on the machine the screenshots were taken on, more or less.
  const ON_PATH = new Set([...SUBCOMMAND_TOOLS, ...PRECOMMANDS, ...'ls cat grep rg fd find ssh scp curl wget vim nvim hx code zed less man top htop tar unzip mkdir rm mv cp touch chmod open ps tail head sed awk jq node python3 python claude moth tmux ln du df wc sort uniq diff'.split(' ')]);
  // Files that exist in ~/code/moth, for the faint underline Moth gives real paths.
  const FILES = new Set(['src', 'src/', 'src/main.rs', 'src/blocks.rs', 'src/links.rs', 'src/highlight.rs', 'src/terminal_view.rs', 'src/shell.rs', 'src/ssh.rs', 'assets', 'assets/', 'assets/themes', 'assets/themes/graphite.json', 'Cargo.toml', 'Cargo.lock', 'README.md', 'LICENSE', '~/.ssh/config', '~/code', '~/code/moth', './install.sh', '.', '..', '~']);
  const history = ['cargo build --release', 'git log --oneline -n 5', 'cargo clippy --all-targets -- -D warnings', 'ssh staging-web', 'git switch -c find-highlight', 'cargo test blocks', 'grep -rn OSC src/shell.rs', 'npm run dev', 'git push -u origin find-highlight', 'cargo run'];

  const isOp = (c) => '|&;<>()'.includes(c);
  const isAssignment = (w) => { const i = w.indexOf('='); return i > 0 && /^[A-Za-z_][A-Za-z0-9_]*$/.test(w.slice(0, i)); };
  const isCommand = (w) => BUILTINS.includes(w) || ON_PATH.has(w) || (w.includes('/') && FILES.has(w));
  const classifyArg = (w) => {
    if (/^\d+$/.test(w)) return 'num';
    const pathy = w.includes('/') || w.startsWith('~') || w.startsWith('.');
    if (FILES.has(w)) return 'path real';
    return pathy ? 'path' : '';
  };

  const lex = (line, cursor) => {
    const out = [];
    let expect = 'command';
    let i = 0;
    while (i < line.length) {
      const c = line[i];
      if (/\s/.test(c)) { i++; continue; }
      if (c === '#' && (i === 0 || /\s/.test(line[i - 1]))) { out.push([i, line.length, 'comment']); break; }
      if (isOp(c)) {
        let j = i; while (j < line.length && isOp(line[j])) j++;
        const op = line.slice(i, j);
        expect = op.endsWith('>') || op.endsWith('<') ? 'target' : 'command';
        out.push([i, j, 'op']); i = j; continue;
      }
      if (c === "'" || c === '"') {
        let j = i + 1;
        while (j < line.length) {
          if (line[j] === '\\' && c === '"') { j += 2; continue; }
          if (line[j] === c) { j++; break; }
          j++;
        }
        j = Math.min(j, line.length);
        out.push([i, j, 'str']);
        if (expect !== 'command') expect = 'argument';
        i = j; continue;
      }
      if (c === '$') {
        if (line[i + 1] === '(') { out.push([i, i + 2, 'op']); expect = 'command'; i += 2; continue; }
        let j = i + 1;
        if (line[j] === '{') { while (j < line.length && line[j] !== '}') j++; j = Math.min(j + 1, line.length); }
        else while (j < line.length && /[\p{L}\p{N}_?@#]/u.test(line[j])) j++;
        out.push([i, j, 'var']);
        if (expect !== 'command') expect = 'argument';
        i = j; continue;
      }
      let j = i;
      while (j < line.length) {
        const ch = line[j];
        if (ch === '\\') { j += 2; continue; }
        if (/\s/.test(ch) || isOp(ch) || ch === "'" || ch === '"') break;
        j++;
      }
      j = Math.min(j, line.length);
      const word = line.slice(i, j);
      const typing = j === cursor;
      let kind;
      if (expect === 'command' && isAssignment(word)) kind = 'var';
      else if (expect === 'command' && KEYWORDS.includes(word)) kind = 'sub';
      else if (expect === 'command') {
        if (isCommand(word)) {
          expect = SUBCOMMAND_TOOLS.includes(word) ? 'subcommand' : PRECOMMANDS.includes(word) ? 'command' : 'argument';
          kind = 'cmd';
        } else {
          expect = 'argument';
          kind = typing ? '' : 'err';
        }
      } else if (word.startsWith('-')) kind = 'flag';
      else if (expect === 'subcommand' && /^[\p{L}\p{N}_-]+$/u.test(word)) { expect = 'argument'; kind = 'sub'; }
      else { expect = 'argument'; kind = classifyArg(word); }
      out.push([i, j, kind]);
      i = j;
    }
    return out;
  };

  const esc = (s) => s.replace(/[&<>]/g, (ch) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[ch]));
  const suggestion = (line) => {
    if (!line.trim()) return '';
    for (let k = history.length - 1; k >= 0; k--) {
      const h = history[k];
      if (h.length > line.length && h.startsWith(line)) return h.slice(line.length);
    }
    return '';
  };

  const colour = (line, cursor) => {
    let html = '';
    let at = 0;
    for (const [a, b, kind] of lex(line, cursor)) {
      html += esc(line.slice(at, a));
      const cls = kind.split(' ').filter(Boolean).map((k) => (k === 'real' ? 'is-real' : `t-${k}`)).join(' ');
      html += cls ? `<span class="${cls}">${esc(line.slice(a, b))}</span>` : esc(line.slice(a, b));
      at = b;
    }
    return html + esc(line.slice(at));
  };

  let ghost = '';
  const render = () => {
    const line = input.value;
    const atEnd = input.selectionStart === line.length && input.selectionEnd === line.length;
    let html = colour(line, input.selectionStart);
    ghost = atEnd ? suggestion(line) : '';
    if (ghost) html += `<span class="t-ghost">${esc(ghost)}</span>`;
    paint.innerHTML = html || '&#8203;';
    paint.parentElement.style.setProperty('--scroll', input.scrollLeft);
  };

  const accept = () => {
    if (!ghost) return false;
    input.value += ghost;
    input.setSelectionRange(input.value.length, input.value.length);
    render();
    return true;
  };

  root.querySelector('.term__body').addEventListener('click', () => { if (!getSelection().toString()) input.focus(); });
  input.addEventListener('input', render);
  input.addEventListener('scroll', render);
  input.addEventListener('keyup', render);
  input.addEventListener('click', render);
  input.addEventListener('keydown', (e) => {
    if ((e.key === 'ArrowRight' || e.key === 'End') && ghost && input.selectionStart === input.value.length) {
      if (accept()) e.preventDefault();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const line = input.value.trim();
      if (!line) return;
      const k = history.indexOf(line);
      if (k >= 0) history.splice(k, 1);
      history.push(line);
      const block = document.createElement('div');
      block.className = 'term__block term__new';
      block.setAttribute('aria-hidden', 'true');
      const time = new Date().toTimeString().slice(0, 5);
      block.innerHTML = `<code class="term__meta"><span class="m-path">~/code/moth</span><span class="t-sub">main</span><span class="m-dim">${time}</span></code>`
        + `<code class="term__cmd">${chev}<span>${colour(line, -1)}</span></code>`
        + '<code class="term__out term__note">This is a web page, so nothing ran. In Moth this is now a block: ⌘↑ selects it, ⌘⇧C copies its output.</code>';
      scrollback.insertBefore(block, live);
      while (scrollback.querySelectorAll('.term__block').length > 4) scrollback.querySelector('.term__block').remove();
      scrollback.scrollTop = scrollback.scrollHeight;
      input.value = '';
      render();
    }
  });
  requestAnimationFrame(() => { input.setSelectionRange(input.value.length, input.value.length); render(); });

  // Graphite / Graphite Light
  const word = root.querySelector('[data-theme-word]');
  root.querySelectorAll('[data-set-theme]').forEach((btn, n, all) => {
    btn.addEventListener('click', () => {
      const theme = btn.dataset.setTheme;
      root.dataset.theme = theme;
      word.textContent = theme === 'light' ? 'Graphite Light' : 'Graphite';
      all.forEach((b) => b.setAttribute('aria-checked', String(b === btn)));
    });
    btn.addEventListener('keydown', (e) => {
      if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
      const next = all[(n + (e.key === 'ArrowRight' ? 1 : all.length - 1)) % all.length];
      next.focus(); next.click();
    });
  });
})();

/* ---------- settings: each switch writes its key into config.json ---------- */
(() => {
  const bands = [...document.querySelectorAll('.band')];
  const out = document.querySelector('[data-config]');
  if (!bands.length || !out) return;
  const valueOf = (band, on) => (band.dataset.on ? (on ? band.dataset.on : band.dataset.off) : on);
  const write = () => {
    const lines = [];
    for (const band of bands) {
      const on = band.classList.contains('is-on');
      if (String(on) === band.dataset.default) continue;
      const v = valueOf(band, on);
      lines.push(`  <span class="c-k">"${band.dataset.key}"</span>: <span class="${typeof v === 'string' ? 'c-s' : 'c-n'}">${typeof v === 'string' ? `"${v}"` : v}</span>`);
    }
    out.innerHTML = lines.length ? `{\n${lines.join(',\n')}\n}` : '{}  <span class="c-comment">// every key at its default</span>';
    out.classList.remove('c-flash'); void out.offsetWidth; out.classList.add('c-flash');
  };
  for (const band of bands) {
    const sw = band.querySelector('.switch');
    const on = sw.getAttribute('aria-checked') === 'true';
    band.classList.toggle('is-on', on);
    sw.addEventListener('click', () => {
      const next = sw.getAttribute('aria-checked') !== 'true';
      sw.setAttribute('aria-checked', String(next));
      band.classList.toggle('is-on', next);
      write();
    });
  }
  write();
  out.classList.remove('c-flash');
})();

/* ---------- keys: macOS or Linux and Windows ---------- */
(() => {
  const root = document.querySelector('#keys');
  if (!root) return;
  const bigs = [...root.querySelectorAll('[data-mac]')];
  const measure = (el) => el.setAttribute('data-len', String([...el.textContent.replace(/\s/g, '')].length));
  bigs.forEach(measure);
  const buttons = [...root.querySelectorAll('[data-set-os]')];
  const setOs = (os) => {
    buttons.forEach((b) => b.setAttribute('aria-checked', String(b.dataset.setOs === os)));
    bigs.forEach((el, n) => {
      const swap = () => { el.textContent = el.dataset[os]; measure(el); el.classList.remove('is-swapping'); };
      if (reduced.matches) return swap();
      el.classList.add('is-swapping');
      setTimeout(swap, 160 + (n % 4) * 30);
    });
  };
  buttons.forEach((b, n) => {
    b.addEventListener('click', () => setOs(b.dataset.setOs));
    b.addEventListener('keydown', (e) => {
      if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
      const next = buttons[(n + 1) % buttons.length];
      next.focus(); next.click();
    });
  });
  if (!/Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent)) setOs('pc');
})();

/* ---------- the clip plays only while it is on screen ---------- */
(() => {
  const video = document.querySelector('video[data-autoplay]');
  if (!video || reduced.matches || !('IntersectionObserver' in window)) return;
  new IntersectionObserver(([e]) => {
    if (e.isIntersecting) { video.preload = 'auto'; video.play().catch(() => {}); } else video.pause();
  }, { threshold: 0.35 }).observe(video);
})();

/* ---------- download: the visitor's platform first ---------- */
(() => {
  const ua = navigator.userAgent;
  const mine = /Windows/.test(ua) ? 'win' : /Mac/.test(ua) ? 'mac' : /Linux|X11/.test(ua) ? 'linux' : null;
  const card = mine && document.querySelector(`.platform[data-platform="${mine}"]`);
  if (card) { card.classList.add('is-yours'); card.parentElement.prepend(card); }
})();

request();
