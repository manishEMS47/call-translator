// ===== DOM refs =====
const chat = document.getElementById('chat');
const statusEl = document.getElementById('status');
const typingEl = document.getElementById('typing');
const toastEl = document.getElementById('toast');

// ===== State =====
let stats = { stt: [], trl: [], tts: [], lat: [], count: 0 };
let muteState = { outgoing: false, incoming: false };
let pending = { direction: null, transcript: null, translation: null };
let lastRenderedDirection = null;
let lastMsgEl = null;
let lastMsgTime = 0;
let sessionStart = Date.now();
let compactMode = false;
let bookmarkFilterOn = false;
let allMessages = [];
let currentSettings = {};

// ===== API key masking (no password detection) =====
document.querySelectorAll('.sp-key').forEach(input => {
  let realValue = input.value;
  const mask = (v) => v.length > 4 ? '••••••••' + v.slice(-4) : v;

  input.addEventListener('focus', () => { input.value = realValue; });
  input.addEventListener('blur', () => { realValue = input.value; input.value = mask(realValue); });
  input.addEventListener('input', () => { realValue = input.value; });

  // Expose real value getter for readForm/populateForm
  input._getRealValue = () => realValue;
  input._setRealValue = (v) => { realValue = v; input.value = mask(v); };
});

// ===== Theme =====
function getTheme() { return localStorage.getItem('translator-theme') || 'dark'; }
function applyTheme(t) {
  document.documentElement.setAttribute('data-theme', t);
  document.getElementById('theme-btn').textContent = t === 'dark' ? '\u2600' : '\u263E';
}
function toggleTheme() {
  const t = getTheme() === 'dark' ? 'light' : 'dark';
  localStorage.setItem('translator-theme', t);
  applyTheme(t);
}
applyTheme(getTheme());

// ===== Timer =====
function updateTimer() {
  if (timerPaused) return;
  const elapsed = Date.now() - sessionStart - timerOffset;
  const s = Math.max(0, Math.floor(elapsed / 1000));
  const m = Math.floor(s / 60);
  document.getElementById('timer').textContent = m + ':' + String(s % 60).padStart(2, '0');
}
setInterval(updateTimer, 1000);

// ===== Toast =====
let toastTimeout = null;
function showToast(text) {
  toastEl.textContent = text;
  toastEl.classList.add('show');
  clearTimeout(toastTimeout);
  toastTimeout = setTimeout(() => toastEl.classList.remove('show'), 1500);
}

// ===== Copy =====
function copyBubble(text) {
  navigator.clipboard.writeText(text).then(() => showToast('Copied!'));
}

// ===== Compact =====
function toggleCompact() {
  compactMode = !compactMode;
  chat.classList.toggle('compact', compactMode);
  document.getElementById('btn-compact').classList.toggle('on', compactMode);
}

// ===== Bookmarks =====
function toggleBookmarkFilter() {
  bookmarkFilterOn = !bookmarkFilterOn;
  document.getElementById('btn-bookmarks').classList.toggle('on', bookmarkFilterOn);
  allMessages.forEach(m => {
    m.el.style.display = (bookmarkFilterOn && !m.bookmarked) ? 'none' : '';
  });
  chat.querySelectorAll('.direction-label, .time-sep').forEach(el => {
    el.style.display = bookmarkFilterOn ? 'none' : '';
  });
  scrollBottom();
}

// ===== Export =====
function exportChat() {
  const lines = [];
  allMessages.forEach(m => {
    const dir = m.direction === 'outgoing' ? 'YOU' : 'THEM';
    const bk = m.bookmarked ? ' *' : '';
    if (m.transcript) lines.push('[' + dir + '] ' + m.transcript);
    lines.push('[' + dir + '] >> ' + m.translation + bk);
    lines.push('');
  });
  const blob = new Blob([lines.join('\n')], { type: 'text/plain' });
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = 'transcript-' + new Date().toISOString().slice(0, 16).replace(':', '-') + '.txt';
  a.click();
  showToast('Exported!');
}

// ===== Helpers =====
function latencyClass(ms) { return ms < 400 ? 'fast' : ms < 800 ? 'medium' : 'slow'; }
function avg(arr) {
  if (!arr.length) return '-';
  return Math.round(arr.reduce((a, b) => a + b, 0) / arr.length) + 'ms';
}
function updateStats() {
  document.getElementById('avg-stt').textContent = avg(stats.stt);
  document.getElementById('avg-trl').textContent = avg(stats.trl);
  document.getElementById('avg-tts').textContent = avg(stats.tts);
  document.getElementById('avg-lat').textContent = avg(stats.lat);
  document.getElementById('total').textContent = stats.count;
}
function scrollBottom() { chat.scrollTop = chat.scrollHeight; }
function sleep(ms) { return new Promise(r => setTimeout(r, ms)); }

// ===== Time separators =====
function maybeAddTimeSep() {
  const now = Date.now();
  if (lastMsgTime && (now - lastMsgTime) > 60000) {
    const gap = Math.round((now - lastMsgTime) / 1000);
    const sep = document.createElement('div');
    sep.className = 'time-sep';
    sep.textContent = gap < 120 ? gap + 's pause' : Math.round(gap / 60) + ' min pause';
    chat.insertBefore(sep, typingEl);
  }
  lastMsgTime = now;
}

// ===== Typing indicator =====
function showTyping() { typingEl.classList.add('visible'); scrollBottom(); }
function hideTyping() { typingEl.classList.remove('visible'); }

// ===== Typewriter =====
function typewrite(el, text) {
  let i = 0;
  el.textContent = '';
  function tick() {
    if (i < text.length) {
      el.textContent += text[i++];
      scrollBottom();
      setTimeout(tick, 18);
    }
  }
  tick();
}

// ===== Chat messages =====
function flushPending() {
  if (!pending.direction || !pending.translation) return;
  hideTyping();
  maybeAddTimeSep();

  if (pending.direction !== lastRenderedDirection) {
    const label = document.createElement('div');
    label.className = 'direction-label ' + pending.direction;
    const myL = (currentSettings.my_language || 'RU').toUpperCase();
    const theirL = (currentSettings.their_language || 'EN').toUpperCase();
    label.textContent = pending.direction === 'outgoing'
      ? 'You (' + myL + ' \u2192 ' + theirL + ')'
      : 'Them (' + theirL + ' \u2192 ' + myL + ')';
    chat.insertBefore(label, typingEl);
    lastRenderedDirection = pending.direction;
  }

  const msg = document.createElement('div');
  msg.className = 'msg ' + pending.direction;
  const star = document.createElement('span');
  star.className = 'star';
  star.textContent = '\u2606';
  msg.appendChild(star);
  const bubble = document.createElement('div');
  bubble.className = 'bubble';
  msg.appendChild(bubble);
  const translationText = pending.translation;
  const transcriptText = pending.transcript;
  bubble.onclick = () => copyBubble(translationText);
  if (transcriptText) {
    const orig = document.createElement('div');
    orig.className = 'original';
    orig.textContent = transcriptText;
    msg.appendChild(orig);
  }
  chat.insertBefore(msg, typingEl);
  lastMsgEl = msg;

  const msgData = {
    el: msg, direction: pending.direction,
    transcript: transcriptText, translation: translationText, bookmarked: false
  };
  allMessages.push(msgData);
  star.onclick = (e) => {
    e.stopPropagation();
    msgData.bookmarked = !msgData.bookmarked;
    star.textContent = msgData.bookmarked ? '\u2605' : '\u2606';
    star.classList.toggle('on', msgData.bookmarked);
    msg.classList.toggle('bookmarked', msgData.bookmarked);
  };
  typewrite(bubble, translationText);
  stats.count++;
  updateStats();
  scrollBottom();
  pending = { direction: null, transcript: null, translation: null };
}

function processLine(line) {
  let m = line.match(/\uD83C\uDFA4 \[(outgoing|incoming)\] (.+)/);
  if (m) { flushPending(); pending.direction = m[1]; pending.transcript = m[2]; showTyping(); return; }
  m = line.match(/\uD83C\uDF10 \[(outgoing|incoming)\] (.+)/);
  if (m) { pending.direction = m[1]; pending.translation = m[2]; flushPending(); return; }
  m = line.match(/\u23F1\s+stt=(\d+)ms\s+trl=(\d+)ms\s+tts=(\d+)ms/);
  if (m) {
    const stt = parseInt(m[1]), trl = parseInt(m[2]), tts = parseInt(m[3]);
    const total = stt + trl + tts;
    stats.stt.push(stt); stats.trl.push(trl); stats.tts.push(tts); stats.lat.push(total);
    updateStats();
    if (lastMsgEl) {
      const meta = document.createElement('div');
      meta.className = 'meta';
      meta.innerHTML = '<span class="' + latencyClass(stt) + '">stt ' + stt + 'ms</span>' +
        '<span class="' + latencyClass(trl) + '">trl ' + trl + 'ms</span>' +
        '<span class="' + latencyClass(tts) + '">tts ' + tts + 'ms</span>' +
        '<span class="' + latencyClass(total) + '">= ' + total + 'ms</span>';
      lastMsgEl.appendChild(meta);
      scrollBottom();
    }
  }
}

// ===== Engine commands =====
async function sendCmd(cmd) {
  const resp = await fetch('/cmd', {
    method: 'POST', headers: {'Content-Type': 'application/json'},
    body: JSON.stringify({cmd})
  });
  return resp.json();
}

// ===== Monitor =====
let monitorEnabled = false;
let audioCtx = null;
let monitorQueue = [];
let monitorPlaying = false;

function toggleMonitor() {
  monitorEnabled = !monitorEnabled;
  document.getElementById('btn-monitor').classList.toggle('on', monitorEnabled);
  // Unlock AudioContext on user gesture
  if (monitorEnabled && !audioCtx) {
    audioCtx = new (window.AudioContext || window.webkitAudioContext)();
  }
  if (monitorEnabled && audioCtx && audioCtx.state === 'suspended') {
    audioCtx.resume();
  }
  showToast(monitorEnabled ? 'Monitor ON' : 'Monitor OFF');
}

// Poll for audio and play via AudioContext
async function pollAudio() {
  if (!monitorEnabled || !audioCtx) return;
  try {
    const r = await fetch('/api/poll-audio');
    const items = await r.json();
    for (const item of items) {
      monitorQueue.push(item);
    }
    if (items.length > 0 && !monitorPlaying) drainMonitorQueue();
  } catch(e) { console.error('[MONITOR] poll error:', e); }
}

async function drainMonitorQueue() {
  monitorPlaying = true;
  while (monitorQueue.length > 0) {
    const { sr, b64 } = monitorQueue.shift();
    try {
      const raw = atob(b64);
      const bytes = new Uint8Array(raw.length);
      for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
      const pcm16 = new Int16Array(bytes.buffer);
      const floats = new Float32Array(pcm16.length);
      for (let i = 0; i < pcm16.length; i++) {
        floats[i] = pcm16[i] / 32768.0;
      }
      const buf = audioCtx.createBuffer(1, floats.length, sr);
      buf.getChannelData(0).set(floats);
      const src = audioCtx.createBufferSource();
      src.buffer = buf;
      const gain = audioCtx.createGain();
      gain.gain.value = 0.8;
      src.connect(gain).connect(audioCtx.destination);
      src.start();
      await new Promise(r => { src.onended = r; });
    } catch(e) {
      console.warn('Monitor playback error:', e);
    }
  }
  monitorPlaying = false;
}

// Poll every 500ms when monitor is on
setInterval(pollAudio, 500);

// ===== Tab Audio Capture =====
let tabCaptureActive = false;
let tabStream = null;
let tabRecorder = null;
let tabDgSocket = null;

const DG_LANG_MAP = { pt: 'pt-BR', no: 'nb' };
function dgLang(code) { return DG_LANG_MAP[code] || code; }

async function toggleTabCapture() {
  if (tabCaptureActive) { stopTabCapture(); return; }
  const key = currentSettings.deepgram_api_key;
  if (!key) { showToast('Set Deepgram API key in Settings first'); return; }

  try {
    tabStream = await navigator.mediaDevices.getDisplayMedia({ audio: true, video: true });
    // Stop video track — we only need audio
    tabStream.getVideoTracks().forEach(t => t.stop());
    if (tabStream.getAudioTracks().length === 0) {
      showToast('No audio in selected source — pick a browser tab');
      tabStream = null;
      return;
    }
  } catch(e) {
    showToast('Tab capture cancelled');
    return;
  }

  const lang = dgLang(currentSettings.their_language || 'en');
  const url = 'wss://api.deepgram.com/v1/listen?model=nova-3&language=' + lang +
    '&interim_results=true&endpointing=' + (currentSettings.endpointing_ms || 300);
  tabDgSocket = new WebSocket(url, ['token', key]);

  tabDgSocket.onopen = () => {
    tabRecorder = new MediaRecorder(tabStream, { mimeType: 'audio/webm;codecs=opus' });
    tabRecorder.ondataavailable = (e) => {
      if (e.data.size > 0 && tabDgSocket && tabDgSocket.readyState === WebSocket.OPEN) {
        tabDgSocket.send(e.data);
      }
    };
    tabRecorder.start(250);
    showToast('Tab Capture ON');
  };

  tabDgSocket.onmessage = async (e) => {
    try {
      const msg = JSON.parse(e.data);
      if (msg.type !== 'Results') return;
      const alt = msg.channel?.alternatives?.[0];
      if (!alt || !alt.transcript) return;
      const text = alt.transcript.trim();
      if (!text) return;

      if (msg.is_final) {
        const t0 = performance.now();
        // Show transcript
        processLine('\uD83C\uDFA4 [incoming] ' + text);
        // Translate
        try {
          const resp = await fetch('/api/translate', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify({
              text,
              from: currentSettings.their_language || 'en',
              to: currentSettings.my_language || 'ru'
            })
          });
          const result = await resp.json();
          const trlMs = Math.round(performance.now() - t0);
          if (result.error) console.warn('[TAB] translate error:', result.error);
          processLine('\uD83C\uDF10 [incoming] ' + result.translation);
          processLine('\u23F1  stt=0ms trl=' + trlMs + 'ms tts=0ms');
        } catch(err) {
          console.error('[TAB] translate fetch failed:', err);
        }
      }
    } catch(err) { console.warn('Tab STT parse error:', err); }
  };

  tabDgSocket.onerror = () => showToast('Deepgram connection error');
  tabDgSocket.onclose = () => { if (tabCaptureActive) stopTabCapture(); };

  // Stop if user stops sharing the tab
  tabStream.getAudioTracks()[0].onended = () => stopTabCapture();

  tabCaptureActive = true;
  document.getElementById('btn-tab-capture').classList.add('on');
}

function stopTabCapture() {
  if (tabRecorder && tabRecorder.state !== 'inactive') tabRecorder.stop();
  if (tabDgSocket && tabDgSocket.readyState === WebSocket.OPEN) {
    tabDgSocket.send(new Uint8Array(0)); // close signal
    tabDgSocket.close();
  }
  if (tabStream) tabStream.getTracks().forEach(t => t.stop());
  tabRecorder = null;
  tabDgSocket = null;
  tabStream = null;
  tabCaptureActive = false;
  document.getElementById('btn-tab-capture').classList.remove('on');
  showToast('Tab Capture OFF');
}

// ===== Engine start/stop =====
let engineRunning = false;
let timerPaused = true;
let timerPausedAt = 0;
let timerOffset = 0;

async function toggleEngine() {
  const btn = document.getElementById('btn-engine');
  const icon = document.getElementById('engine-icon');
  const text = document.getElementById('engine-toggle-text');

  if (engineRunning) {
    await sendCmd('stop');
    await fetch('/api/calls/end', { method: 'POST' });
    engineRunning = false;
    timerPaused = true;
    timerPausedAt = Date.now();
    btn.className = 'btn btn-engine stopped';
    icon.innerHTML = '&#9654;';
    text.textContent = 'Start';
    setEnginePill('stopped', 'Stopped');
    showToast('Engine stopped');
  } else {
    btn.className = 'btn btn-engine stopped';
    text.textContent = 'Starting...';
    icon.innerHTML = '&#8987;';
    setEnginePill('restarting', 'Starting...');

    // New session: close previous call, clear log, clear chat
    await fetch('/api/calls/new-session', { method: 'POST' });
    clearAll();
    // Reconnect SSE so it doesn't replay old lines
    if (typeof evtSource !== 'undefined' && evtSource) { evtSource.close(); }
    connectSSE();

    await sendCmd('start');
    await sleep(2000);

    engineRunning = true;
    sessionStart = Date.now();
    timerOffset = 0;
    timerPaused = false;
    btn.className = 'btn btn-engine running';
    icon.innerHTML = '&#9724;';
    text.textContent = 'Stop';
    setEnginePill('running', 'Running');
    showToast('Engine started');
  }
}

async function toggleMute(direction) {
  muteState[direction] = !muteState[direction];
  const muted = muteState[direction];
  await sendCmd(muted ? 'mute_' + direction : 'unmute_' + direction);
  const btn = document.getElementById(direction === 'outgoing' ? 'btn-mic-out' : 'btn-mic-in');
  btn.className = muted ? 'btn muted' : 'btn active';
}

function clearAll() {
  chat.innerHTML = '';
  chat.appendChild(typingEl);
  stats = { stt: [], trl: [], tts: [], lat: [], count: 0 };
  lastRenderedDirection = null; lastMsgEl = null; lastMsgTime = 0;
  pending = { direction: null, transcript: null, translation: null };
  allMessages = []; bookmarkFilterOn = false;
  document.getElementById('btn-bookmarks').classList.remove('on');
  updateStats();
}

// ===== Settings Panel =====
function openSettings() {
  document.getElementById('sp-backdrop').classList.add('open');
  document.getElementById('sp').classList.add('open');
}
function closeSettings() {
  document.getElementById('sp-backdrop').classList.remove('open');
  document.getElementById('sp').classList.remove('open');
}
function toggleSection(id) {
  document.getElementById(id).classList.toggle('collapsed');
}

// Populate settings form from loaded settings
function populateForm(s) {
  const dg = document.getElementById('cfg-deepgram');
  const gr = document.getElementById('cfg-groq');
  const sdb = document.getElementById('cfg-sixtydb');
  if (dg._setRealValue) dg._setRealValue(s.deepgram_api_key || '');
  else dg.value = s.deepgram_api_key || '';
  if (gr._setRealValue) gr._setRealValue(s.groq_api_key || '');
  else gr.value = s.groq_api_key || '';
  if (sdb._setRealValue) sdb._setRealValue(s.sixtydb_api_key || '');
  else sdb.value = s.sixtydb_api_key || '';
  if (!s.deepgram_api_key && s._deepgram_from_env) dg.placeholder = 'Set via .env file';
  if (!s.groq_api_key && s._groq_from_env) gr.placeholder = 'Set via .env file';
  if (!s.sixtydb_api_key && s._sixtydb_from_env) sdb.placeholder = 'Set via .env file';
  document.getElementById('cfg-my-lang').value = s.my_language || 'en';
  document.getElementById('cfg-their-lang').value = s.their_language || 'en';
  document.getElementById('cfg-endpointing').value = s.endpointing_ms || 300;
  document.getElementById('endpointing-val').textContent = (s.endpointing_ms || 300) + 'ms';
  // Provider selectors
  document.getElementById('cfg-stt-out').value = s.stt_provider_outgoing || 'deepgram';
  document.getElementById('cfg-stt-in').value = s.stt_provider_incoming || 'deepgram';
  document.getElementById('cfg-tts-out').value = s.tts_provider_outgoing || 'piper';
  document.getElementById('cfg-tts-in').value = s.tts_provider_incoming || 'piper';
  apply60dbProviderUI();
  // 60db voice dropdowns are filled by load60dbVoices() (called from loadSettings)
  // Device dropdowns populated by loadDevices() using currentSettings
}

function readForm() {
  return {
    deepgram_api_key: (document.getElementById('cfg-deepgram')._getRealValue || (() => document.getElementById('cfg-deepgram').value))().trim(),
    sixtydb_api_key: (document.getElementById('cfg-sixtydb')._getRealValue || (() => document.getElementById('cfg-sixtydb').value))().trim(),
    groq_api_key: (document.getElementById('cfg-groq')._getRealValue || (() => document.getElementById('cfg-groq').value))().trim(),
    my_language: document.getElementById('cfg-my-lang').value,
    their_language: document.getElementById('cfg-their-lang').value,
    tts_outgoing_voice: document.getElementById('cfg-voice-out').value,
    tts_incoming_voice: document.getElementById('cfg-voice-in').value,
    mic_device: document.getElementById('cfg-mic').value || 'default',
    speaker_device: document.getElementById('cfg-speaker').value || 'default',
    endpointing_ms: parseInt(document.getElementById('cfg-endpointing').value),
    stt_provider_outgoing: document.getElementById('cfg-stt-out').value,
    stt_provider_incoming: document.getElementById('cfg-stt-in').value,
    tts_provider_outgoing: document.getElementById('cfg-tts-out').value,
    tts_provider_incoming: document.getElementById('cfg-tts-in').value,
    tts_60db_voice_outgoing: document.getElementById('cfg-60db-voice-out').value,
    tts_60db_voice_incoming: document.getElementById('cfg-60db-voice-in').value,
  };
}

// Download missing voice model with user confirmation
let downloadingLangs = new Set();

const LANGS_NO_TTS = [];

async function showDownloadPrompt(lang, hintId) {
  const hint = document.getElementById(hintId);
  if (LANGS_NO_TTS.includes(lang)) {
    hint.innerHTML = '<span style="color:var(--yellow)">No TTS voice exists for ' + langName(lang) +
      '. Translation will work but without audio output.</span>';
    return;
  }
  hint.innerHTML = '<button class="sp-download-btn" onclick="downloadDefaultVoice(\'' +
    lang + '\', \'' + hintId + '\')">Download ' + langName(lang) +
    ' default voice &amp; restart engine</button>';
  hint.style.color = '';
}

async function downloadDefaultVoice(lang, hintId) {
  if (downloadingLangs.has(lang)) return;
  downloadingLangs.add(lang);
  const hint = document.getElementById(hintId);
  hint.innerHTML = '<div class="sp-progress"><div class="sp-progress-bar" id="pb-' + lang +
    '"></div><div class="sp-progress-text" id="pt-' + lang + '">Connecting...</div></div>';

  try {
    const resp = await fetch('/api/download-voice', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ lang })
    });
    const reader = resp.body.getReader();
    const decoder = new TextDecoder();
    let buf = '';

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      const lines = buf.split('\n');
      buf = lines.pop() || '';
      for (const line of lines) {
        if (!line.startsWith('data: ')) continue;
        const data = JSON.parse(line.slice(6));
        if (data.progress !== undefined) {
          const bar = document.getElementById('pb-' + lang);
          const txt = document.getElementById('pt-' + lang);
          if (bar) bar.style.width = data.progress + '%';
          if (txt) txt.textContent = data.progress + '% \u2014 ' + data.mb_done + '/' + data.mb_total + ' MB';
        }
        if (data.done) {
          hint.innerHTML = '<span style="color:var(--green)">' + langName(lang) +
            ' voice installed!</span>';
          showToast(langName(lang) + ' voice downloaded');
          await loadVoices();
          await saveAndRestart();
        }
        if (data.error) {
          hint.innerHTML = '<span style="color:var(--red)">' + data.error + '</span>';
        }
      }
    }
  } catch(e) {
    hint.innerHTML = '<span style="color:var(--red)">Download failed: ' + e.message + '</span>';
  }
  downloadingLangs.delete(lang);
}

// Language change → update voice dropdowns
document.getElementById('cfg-my-lang').addEventListener('change', updateVoiceDropdowns);
document.getElementById('cfg-their-lang').addEventListener('change', updateVoiceDropdowns);

// Endpointing slider live update
document.getElementById('cfg-endpointing').addEventListener('input', function() {
  document.getElementById('endpointing-val').textContent = this.value + 'ms';
});

// Test API key
async function testKey(provider) {
  const ids = {
    deepgram: { input: 'cfg-deepgram', btn: 'test-deepgram' },
    groq:     { input: 'cfg-groq',     btn: 'test-groq' },
    '60db':   { input: 'cfg-sixtydb',  btn: 'test-sixtydb' },
  };
  const { input: inputId, btn: btnId } = ids[provider] || ids.deepgram;
  const el = document.getElementById(inputId);
  const key = (el._getRealValue ? el._getRealValue() : el.value).trim();
  const btn = document.getElementById(btnId);

  if (!key) { btn.textContent = 'Empty'; btn.className = 'sp-test-btn fail'; return; }

  btn.textContent = '...';
  btn.className = 'sp-test-btn testing';

  try {
    const r = await fetch('/api/test-key', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ provider, key })
    });
    const data = await r.json();
    btn.textContent = data.valid ? '\u2713 Valid' : '\u2717 Invalid';
    btn.className = 'sp-test-btn ' + (data.valid ? 'ok' : 'fail');
  } catch(e) {
    btn.textContent = 'Error';
    btn.className = 'sp-test-btn fail';
  }

  setTimeout(() => { btn.textContent = 'Test'; btn.className = 'sp-test-btn'; }, 4000);
}

// Voice preview — synthesize + play through speakers via engine
async function previewVoice(dir) {
  const btn = document.getElementById('preview-' + dir);
  const voiceSelect = document.getElementById('cfg-voice-' + dir);
  const voice = voiceSelect.value;
  if (!voice) { showToast('No voice selected'); return; }
  if (!isVoiceDownloaded(dir)) { showToast('Download the voice first'); return; }
  // Determine language from direction
  const lang = dir === 'out'
    ? document.getElementById('cfg-their-lang').value
    : document.getElementById('cfg-my-lang').value;
  btn.classList.add('loading');
  try {
    const r = await fetch('/api/tts-preview', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ lang, voice })
    });
    const data = await r.json();
    if (data.status && data.status.startsWith('ok')) {
      showToast('Playing preview...');
      // Engine plays through speakers, wait for it to finish
      await sleep(3500);
    } else {
      showToast('Preview failed: ' + (data.status || 'engine not running'));
    }
  } catch(e) {
    showToast('Preview error: engine not running');
  }
  btn.classList.remove('loading');
}

// Load voices into dropdowns
let allVoices = {};
const LANG_NAMES = {
  ar:'Arabic',ca:'Catalan',cs:'Czech',da:'Danish',de:'German',el:'Greek',
  en:'English',es:'Spanish',fa:'Persian',fi:'Finnish',fr:'French',
  hi:'Hindi',hu:'Hungarian',id:'Indonesian',it:'Italian',ja:'Japanese',
  ko:'Korean',lv:'Latvian',nl:'Dutch',no:'Norwegian',pl:'Polish',
  pt:'Portuguese',ro:'Romanian',ru:'Russian',sv:'Swedish',tr:'Turkish',
  uk:'Ukrainian',vi:'Vietnamese',zh:'Chinese'
};
function langName(code) { return LANG_NAMES[code] || code; }

// Re-apply tooltips when my-language changes
document.getElementById('cfg-my-lang')?.addEventListener('change', applyTooltips);

async function loadVoices() {
  try {
    const r = await fetch('/api/voices');
    allVoices = await r.json();
    updateVoiceDropdowns();
  } catch(e) { console.error('Failed to load voices', e); }
}

function updateVoiceDropdowns() {
  const theirLang = document.getElementById('cfg-their-lang').value;
  const myLang = document.getElementById('cfg-my-lang').value;

  fillVoiceSelect('cfg-voice-out', theirLang, currentSettings.tts_outgoing_voice);
  fillVoiceSelect('cfg-voice-in', myLang, currentSettings.tts_incoming_voice);

  document.getElementById('voice-label-in').textContent =
    langName(myLang) + ' Voice (I hear)';
  document.getElementById('voice-label-out').textContent =
    langName(theirLang) + ' Voice (they hear)';

  updateDlButton('in');
  updateDlButton('out');

  // Show download prompt if no downloaded voices for this language
  const hintOut = document.getElementById('voice-hint-out');
  const hintIn = document.getElementById('voice-hint-in');
  const voicesOut = allVoices[theirLang] || [];
  const voicesIn = allVoices[myLang] || [];
  const hasDownloadedOut = voicesOut.some(v => v.downloaded);
  const hasDownloadedIn = voicesIn.some(v => v.downloaded);

  if (!hasDownloadedOut) showDownloadPrompt(theirLang, 'voice-hint-out');
  else { hintOut.textContent = ''; hintOut.style.color = ''; }
  if (!hasDownloadedIn) showDownloadPrompt(myLang, 'voice-hint-in');
  else { hintIn.textContent = ''; hintIn.style.color = ''; }
}

function fillVoiceSelect(selId, lang, currentVal) {
  const sel = document.getElementById(selId);
  sel.innerHTML = '';
  const voices = allVoices[lang] || [];
  if (voices.length === 0) {
    const opt = document.createElement('option');
    opt.value = ''; opt.textContent = 'No voices for ' + langName(lang);
    sel.appendChild(opt);
    return;
  }
  const downloaded = voices.filter(v => v.downloaded);
  const available = voices.filter(v => !v.downloaded);

  if (downloaded.length > 0) {
    const grp = document.createElement('optgroup');
    grp.label = 'Downloaded';
    downloaded.forEach(v => {
      const opt = document.createElement('option');
      opt.value = v.name;
      opt.textContent = v.name.replace(/-/g, ' ');
      grp.appendChild(opt);
    });
    sel.appendChild(grp);
  }
  if (available.length > 0) {
    const grp = document.createElement('optgroup');
    grp.label = 'Available (' + available.length + ')';
    available.forEach(v => {
      const opt = document.createElement('option');
      opt.value = v.name;
      opt.textContent = v.name.replace(/-/g, ' ') + ' \u2014 ' + v.size_mb + ' MB';
      grp.appendChild(opt);
    });
    sel.appendChild(grp);
  }
  if (currentVal && voices.some(v => v.name === currentVal)) sel.value = currentVal;
}

function isVoiceDownloaded(dir) {
  const sel = document.getElementById('cfg-voice-' + dir);
  const lang = dir === 'out'
    ? document.getElementById('cfg-their-lang').value
    : document.getElementById('cfg-my-lang').value;
  const voices = allVoices[lang] || [];
  const voice = voices.find(v => v.name === sel.value);
  return voice ? voice.downloaded : true;
}

function updateDlButton(dir) {
  const btn = document.getElementById('dl-voice-' + dir);
  const sel = document.getElementById('cfg-voice-' + dir);
  if (!sel.value || isVoiceDownloaded(dir)) {
    btn.classList.add('hidden');
  } else {
    btn.classList.remove('hidden');
  }
}

// Update download button when voice selection changes
document.getElementById('cfg-voice-in').addEventListener('change', () => updateDlButton('in'));
document.getElementById('cfg-voice-out').addEventListener('change', () => updateDlButton('out'));

async function downloadSelectedVoice(dir) {
  const sel = document.getElementById('cfg-voice-' + dir);
  const btn = document.getElementById('dl-voice-' + dir);
  const hint = document.getElementById('voice-hint-' + dir);
  const voice = sel.value;
  const lang = dir === 'out'
    ? document.getElementById('cfg-their-lang').value
    : document.getElementById('cfg-my-lang').value;

  if (!voice) return;
  btn.classList.add('loading');
  hint.innerHTML = '<div class="sp-progress"><div class="sp-progress-bar" id="pb-dl-' + dir +
    '"></div><div class="sp-progress-text" id="pt-dl-' + dir + '">Connecting...</div></div>';

  try {
    const resp = await fetch('/api/download-voice', {
      method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({ lang, voice })
    });
    const reader = resp.body.getReader();
    const decoder = new TextDecoder();
    let buf = '';

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      const lines = buf.split('\n');
      buf = lines.pop() || '';
      for (const line of lines) {
        if (!line.startsWith('data: ')) continue;
        const data = JSON.parse(line.slice(6));
        if (data.progress !== undefined) {
          const bar = document.getElementById('pb-dl-' + dir);
          const txt = document.getElementById('pt-dl-' + dir);
          if (bar) bar.style.width = data.progress + '%';
          if (txt) txt.textContent = data.progress + '% \u2014 ' + data.mb_done + '/' + data.mb_total + ' MB';
        }
        if (data.done) {
          hint.innerHTML = '<span style="color:var(--green)">Downloaded!</span>';
          setTimeout(() => { hint.textContent = ''; }, 3000);
          await loadVoices();
          sel.value = voice;
          updateDlButton(dir);
        }
        if (data.error) {
          hint.innerHTML = '<span style="color:var(--red)">' + data.error + '</span>';
        }
      }
    }
  } catch(e) {
    hint.innerHTML = '<span style="color:var(--red)">Download failed: ' + e.message + '</span>';
  }
  btn.classList.remove('loading');
}

// Load audio devices into select dropdowns
async function loadDevices() {
  try {
    const r = await fetch('/api/devices');
    const data = await r.json();
    const inputDevs = data.input || [];
    const outputDevs = data.output || [];

    function fillSelect(id, devices, current) {
      const sel = document.getElementById(id);
      sel.innerHTML = '';
      // Add "default" option
      const def = document.createElement('option');
      def.value = 'default'; def.textContent = 'Default';
      sel.appendChild(def);
      devices.forEach(d => {
        const opt = document.createElement('option');
        opt.value = d; opt.textContent = d;
        sel.appendChild(opt);
      });
      if (current && current !== 'default') {
        // Add current value if not in list (e.g. device unplugged)
        if (!devices.includes(current)) {
          const opt = document.createElement('option');
          opt.value = current; opt.textContent = current + ' (saved)';
          sel.appendChild(opt);
        }
        sel.value = current;
      }
    }

    fillSelect('cfg-mic', inputDevs, currentSettings.mic_device);
    fillSelect('cfg-speaker', outputDevs, currentSettings.speaker_device);
  } catch(e) { console.error('Failed to load devices', e); }
}

// Load settings from server
// ===== 60db providers =====
let sixtyDbVoices = [];

async function load60dbVoices() {
  try {
    const r = await fetch('/api/60db-voices');
    const data = await r.json();
    sixtyDbVoices = data.voices || [];
  } catch(e) {
    sixtyDbVoices = [];
    console.error('Failed to load 60db voices', e);
  }
  fill60dbVoiceSelect('cfg-60db-voice-out', currentSettings.tts_60db_voice_outgoing);
  fill60dbVoiceSelect('cfg-60db-voice-in', currentSettings.tts_60db_voice_incoming);
}

function fill60dbVoiceSelect(selId, currentVal) {
  const sel = document.getElementById(selId);
  if (!sel) return;
  sel.innerHTML = '';
  if (sixtyDbVoices.length === 0) {
    const opt = document.createElement('option');
    opt.value = '';
    const hasKey = currentSettings.sixtydb_api_key || currentSettings._sixtydb_from_env;
    opt.textContent = hasKey ? 'No 60db voices found' : 'Add a 60db API key first';
    sel.appendChild(opt);
    return;
  }
  for (const v of sixtyDbVoices) {
    const opt = document.createElement('option');
    opt.value = v.voice_id;
    const meta = [v.language_name || v.language, v.gender, v.accent].filter(Boolean).join(', ');
    opt.textContent = meta ? (v.name + ' (' + meta + ')') : v.name;
    sel.appendChild(opt);
  }
  if (currentVal) sel.value = currentVal;
}

// Show/hide the 60db voice dropdowns based on the TTS provider selection.
function apply60dbProviderUI() {
  const outIs60 = document.getElementById('cfg-tts-out').value === '60db';
  const inIs60 = document.getElementById('cfg-tts-in').value === '60db';
  document.getElementById('row-60db-voice-out').classList.toggle('hidden', !outIs60);
  document.getElementById('row-60db-voice-in').classList.toggle('hidden', !inIs60);
}

document.getElementById('cfg-tts-out').addEventListener('change', apply60dbProviderUI);
document.getElementById('cfg-tts-in').addEventListener('change', apply60dbProviderUI);

async function loadSettings() {
  try {
    const r = await fetch('/api/settings');
    currentSettings = await r.json();
    populateForm(currentSettings);
    await load60dbVoices();
  } catch(e) { console.error('Failed to load settings', e); }
}

// Save settings to server
async function saveSettings() {
  const settings = readForm();
  await fetch('/api/settings', {
    method: 'POST', headers: {'Content-Type': 'application/json'},
    body: JSON.stringify(settings)
  });
  currentSettings = settings;
}

// ===== Engine Restart =====
function setEnginePill(state, text) {
  const pill = document.getElementById('engine-pill');
  pill.className = 'engine-pill' + (state === 'running' ? '' : ' ' + state);
  document.getElementById('engine-label').textContent = text;
}

async function saveAndRestart() {
  const btn = document.getElementById('restart-btn');
  const txt = document.getElementById('restart-text');
  const bar = document.getElementById('restart-progress');

  btn.classList.add('restarting');
  btn.classList.remove('success', 'error');

  try {
    // Stage 1: Save
    txt.textContent = 'Saving settings...';
    bar.style.width = '15%';
    setEnginePill('restarting', 'Saving...');
    await saveSettings();
    await sleep(300);

    // Stage 2: Restart
    txt.textContent = 'Restarting engine...';
    bar.style.width = '35%';
    setEnginePill('restarting', 'Restarting...');
    await fetch('/api/engine/restart', { method: 'POST' });
    await sleep(500);

    // Stage 3: Wait for models to load
    txt.textContent = 'Loading models...';
    bar.style.width = '60%';
    setEnginePill('restarting', 'Loading...');

    // Poll health
    let attempts = 0;
    while (attempts < 60) {
      await sleep(1000);
      attempts++;
      bar.style.width = Math.min(60 + attempts, 95) + '%';
      try {
        const r = await fetch('/health');
        if (r.ok) break;
      } catch(e) {}
    }

    // Stage 4: Starting pipelines
    txt.textContent = 'Starting pipelines...';
    bar.style.width = '95%';
    await sleep(1000);

    // Done!
    bar.style.width = '100%';
    btn.classList.remove('restarting');
    btn.classList.add('success');
    txt.innerHTML = '&#10003; Ready!';
    setEnginePill('running', 'Running');
    showToast('Engine restarted');

    await sleep(2500);
    btn.classList.remove('success');
    txt.textContent = 'Save & Restart Engine';
    bar.style.width = '0%';

  } catch(e) {
    btn.classList.remove('restarting');
    btn.classList.add('error');
    txt.textContent = 'Error: ' + (e.message || 'restart failed');
    setEnginePill('stopped', 'Error');

    await sleep(3000);
    btn.classList.remove('error');
    txt.textContent = 'Save & Restart Engine';
    bar.style.width = '0%';
  }
}

// ===== Init =====
async function waitForEngine() {
  const overlay = document.getElementById('overlay');
  const text = document.getElementById('overlay-text');
  const spinner = document.getElementById('spinner');
  while (true) {
    try {
      const r = await fetch('/health');
      if (r.ok) {
        text.className = 'ready';
        text.textContent = 'Engine ready';
        spinner.style.display = 'none';
        sessionStart = Date.now();
        setEnginePill('running', 'Running');
        await sleep(600);
        overlay.className = 'hidden';
        return;
      }
    } catch(e) {}
    await sleep(500);
  }
}

// Boot sequence
(async function boot() {
  // Load settings + voices + devices in parallel
  await Promise.all([loadSettings(), loadVoices(), loadDevices()]);
  applyTooltips();

  // Auto-open settings if no API keys configured
  if (!currentSettings.deepgram_api_key && !currentSettings.groq_api_key) {
    openSettings();
  }
})();

waitForEngine();

let evtSource = null;
function connectSSE(replay) {
  if (evtSource) evtSource.close();
  const url = replay ? '/stream?replay=1' : '/stream';
  evtSource = new EventSource(url);
  evtSource.onmessage = (e) => processLine(e.data);
  evtSource.onerror = () => { statusEl.textContent = 'Disconnected'; statusEl.className = 'disconnected'; };
  evtSource.onopen = () => { statusEl.textContent = 'Connected'; statusEl.className = ''; };
}
connectSSE();
