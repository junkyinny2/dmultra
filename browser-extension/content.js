// DM Ultra content script: link interception + video detection.
// Videos are detected by 3 layers, then the Win32 overlay (in the DM Ultra
// GUI process) draws the floating button — NO DOM button is created.
const EXT_RE = /\.(3gp|7z|aac|ace|aif|apk|arj|asf|avi|bin|bz2|doc|docx|exe|flv|gz|gzip|img|iso|lzh|m4a|m4v|mkv|mov|mp3|mp4|mpa|mpe|mpeg|mpg|msi|msu|ogg|ogv|pdf|plj|pps|ppt|qt|ra|rar|rm|rmvb|sea|sit|sitx|tar|tif|tiff|wav|webm|wma|wmv|z|zip|r0\d?|r1\d?)(\?|#|$)/i;
const MEDIA_RE = /\.(m3u8|mpd|mp4|webm|mkv|flv|mp3|m4a|ts|mpg|mpeg|wmv|wav|ogg|aac|opus|mov|avi)(\?|#|$)/i;

function forward(url, filename) {
  try { chrome.runtime.sendMessage({ url, referer: location.href, filename: filename || '' }); } catch(e) {}
}
function forwardBatch(urls) {
  try { chrome.runtime.sendMessage({ urls, referer: location.href }); } catch(e) {}
}

// Click interception for downloadable links.
document.addEventListener('click', (e) => {
  if (e.defaultPrevented || e.button !== 0) return;
  if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
  const a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
  if (!a) return;
  const url = a.href || '';
  if (!/^https?:/i.test(url)) return;
  const name = (a.download || '').trim();
  if (a.hasAttribute('download') || EXT_RE.test(url) || EXT_RE.test(name)) {
    e.preventDefault(); e.stopPropagation();
    forward(url, name);
  }
}, true);

// Background requests (grabLinks / grabVideos).
chrome.runtime.onMessage.addListener((msg) => {
  if (!msg || !msg.action) return;
  const urls = new Set();
  if (msg.action === 'grabLinks') {
    document.querySelectorAll('a[href]').forEach((a) => {
      const u = a.href || '';
      if (/^https?:/i.test(u) && (EXT_RE.test(u) || MEDIA_RE.test(u) || a.hasAttribute('download'))) urls.add(u.split('#')[0]);
    });
  } else if (msg.action === 'grabVideos') {
    document.querySelectorAll('video, audio, source, iframe').forEach((el) => {
      const u = el.currentSrc || el.src || '';
      if (/^https?:/i.test(u) && MEDIA_RE.test(u)) urls.add(u.split('#')[0]);
    });
    document.querySelectorAll('a[href]').forEach((a) => {
      const u = a.href || '';
      if (/^https?:/i.test(u) && /\.(m3u8|mpd)(\?|#|$)/i.test(u)) urls.add(u.split('#')[0]);
    });
  }
  if (urls.size > 0) forwardBatch(Array.from(urls).slice(0, 200));
});

// ---------------------------------------------------------------------------
// Video detection — Win32 overlay (no DOM button; the DM Ultra GUI process
// draws a native floating button over the video, like IDMan.exe does).
// The extension sends the video URL + format + screen coordinates to the
// native messaging host, which passes them to the overlay module.
// ---------------------------------------------------------------------------

let detectedVideos = [];
let currentBestUrl = '';
let currentBestFormat = '';
let overlayShown = false;

// Performance: throttle MutationObserver and periodic scans.
let scanScheduled = false;
let lastScanTime = 0;
const MIN_SCAN_INTERVAL = 3000; // ms between scans

// Tell the desktop overlay to go away (video paused/ended, tab hidden,
// page unloading). The desktop also has a heartbeat timeout, so a missed
// hide can never leave the button stuck on screen.
function sendOverlayHide() {
  if (!overlayShown) return;
  overlayShown = false;
  try { chrome.runtime.sendMessage({ action: 'videoOverlayHide' }); } catch (e) {}
}

function fmtFromUrl(url) {
  const m = url.match(MEDIA_RE);
  return m ? m[1].toLowerCase() : '';
}

function addVideo(url, format, quality) {
  if (!url || !/^https?:/i.test(url)) return;
  if (detectedVideos.find((v) => v.url === url)) return;
  const fmt = format || fmtFromUrl(url) || 'video';
  const label = quality ? `${fmt.toUpperCase()} - ${quality}` : `${fmt.toUpperCase()}`;
  detectedVideos.push({ url, format: fmt, quality: quality || '', label });
  // Don't send update immediately; let the throttle batch it.
  scheduleOverlayUpdate();
}

// Parse m3u8 master playlist for variant resolutions.
async function parseHLS(url) {
  try {
    const resp = await fetch(url, { credentials: 'include' });
    const text = await resp.text();
    const lines = text.split('\n');
    const variants = [];
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i].trim();
      if (line.startsWith('#EXT-X-STREAM-INF')) {
        const res = (line.match(/RESOLUTION=(\d+x\d+)/) || [])[1];
        const bw = (line.match(/BANDWIDTH=(\d+)/) || [])[1];
        const next = lines[i + 1] && lines[i + 1].trim();
        if (next && !next.startsWith('#')) {
          const abs = new URL(next, url).href;
          const quality = res || (bw ? `${Math.round(parseInt(bw) / 1000)} kbps` : 'auto');
          variants.push({ url: abs, format: 'm3u8', quality });
        }
      }
    }
    if (variants.length > 0) {
      detectedVideos = detectedVideos.filter((v) => v.url !== url);
      for (const v of variants) addVideo(v.url, v.format, v.quality);
    } else {
      addVideo(url, 'm3u8', '');
    }
  } catch (e) {
    addVideo(url, 'm3u8', '');
  }
}

// Find the largest visible, *playing* video element and get its screen
// coordinates. Paused/ended videos (and plain <object>/<iframe> boxes)
// never get the overlay, so the button cannot appear on a dead player.
function getVideoRect() {
  let best = null, bestArea = 0;
  document.querySelectorAll('video').forEach((el) => {
    if (el.paused || el.ended || el.readyState < 2) return;
    const r = el.getBoundingClientRect();
    // Only the part inside the viewport counts as "on screen".
    const left = Math.max(r.left, 0);
    const top = Math.max(r.top, 0);
    const right = Math.min(r.right, window.innerWidth);
    const bottom = Math.min(r.bottom, window.innerHeight);
    const w = Math.max(0, right - left);
    const h = Math.max(0, bottom - top);
    if (w * h > bestArea && w >= 200 && h >= 100) {
      best = { left, top, width: w, height: h };
      bestArea = w * h;
    }
  });
  if (!best) return null;
  // Convert to screen coords. In a same-origin iframe the video rect is
  // relative to the frame viewport, so add the cumulative frame offsets.
  let ox = 0, oy = 0;
  let win = window;
  try {
    while (win !== win.parent) {
      const fe = win.frameElement;
      if (!fe) return null;
      const fr = fe.getBoundingClientRect();
      ox += fr.left;
      oy += fr.top;
      win = win.parent;
    }
  } catch (e) {
    return null; // cross-origin ancestor: screen coords are unknowable here
  }
  const sx = Math.round(ox + best.left + window.screenX + (window.outerWidth - window.innerWidth) / 2);
  const sy = Math.round(oy + best.top + window.screenY + (window.outerHeight - window.innerHeight));
  return { x: sx, y: sy, w: Math.round(best.width), h: Math.round(best.height) };
}

// Throttled overlay update: only send if enough time passed since last send.
function scheduleOverlayUpdate() {
  const now = Date.now();
  if (now - lastScanTime >= MIN_SCAN_INTERVAL) {
    sendOverlayUpdateNow();
  } else if (!scanScheduled) {
    scanScheduled = true;
    setTimeout(() => {
      scanScheduled = false;
      sendOverlayUpdateNow();
    }, MIN_SCAN_INTERVAL - (now - lastScanTime));
  }
}

function sendOverlayUpdateNow() {
  lastScanTime = Date.now();
  // A background tab or an unfocused browser window must never drive the
  // overlay (two windows playing would otherwise fight over the position).
  if (document.visibilityState !== 'visible' || !document.hasFocus()) {
    sendOverlayHide();
    return;
  }
  const rect = getVideoRect();
  if (!rect || detectedVideos.length === 0) { sendOverlayHide(); return; }
  // Best format for the button label.
  const best = detectedVideos.find((v) => v.format === 'mp4' || v.format === 'm3u8')
    || detectedVideos[0];
  currentBestUrl = best.url;
  currentBestFormat = best.format;
  overlayShown = true;
  try {
    chrome.runtime.sendMessage({
      action: 'videoOverlay',
      url: best.url,
      format: best.format,
      x: rect.x,
      y: rect.y,
      w: rect.w,
      h: rect.h,
      qualities: detectedVideos.slice(0, 10).map((v) => ({
        label: v.label,
        url: v.url,
      })),
    });
  } catch (e) {}
}

// Layer 1: Background webRequest detection.
chrome.runtime.onMessage.addListener((msg) => {
  if (msg && msg.action === 'videoUrlFound' && msg.url) {
    const fmt = msg.format || '';
    if (fmt === 'm3u8') parseHLS(msg.url);
    else addVideo(msg.url, fmt, '');
  }
});

// Layer 2: DOM scan — throttled.
function scanForVideo() {
  document.querySelectorAll('video, object, embed').forEach((el) => {
    const src = el.currentSrc || el.src || el.getAttribute('src') || '';
    if (/^https?:/i.test(src) && MEDIA_RE.test(src)) {
      if (fmtFromUrl(src) === 'm3u8') parseHLS(src);
      else addVideo(src, fmtFromUrl(src), '');
    }
    el.querySelectorAll('source').forEach((s) => {
      const u = s.src || s.getAttribute('src') || '';
      if (/^https?:/i.test(u) && MEDIA_RE.test(u)) {
        if (fmtFromUrl(u) === 'm3u8') parseHLS(u);
        else addVideo(u, fmtFromUrl(u), '');
      }
    });
  });
  if (detectedVideos.length === 0) {
    try { chrome.runtime.sendMessage({ action: 'queryVideos' }); } catch(e) {}
  }
  scheduleOverlayUpdate();
}

// Layer 3: Hook fetch/XHR for JS-loaded media.
try {
  const origFetch = window.fetch;
  window.fetch = function(...args) {
    const result = origFetch.apply(this, args);
    const url = typeof args[0] === 'string' ? args[0] : (args[0] && args[0].url) || '';
    if (url && MEDIA_RE.test(url)) {
      if (fmtFromUrl(url) === 'm3u8') parseHLS(url);
      else addVideo(url, fmtFromUrl(url), '');
    }
    return result;
  };
} catch(e) {}
try {
  const origOpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function(method, url, ...rest) {
    if (url && MEDIA_RE.test(String(url))) {
      if (fmtFromUrl(String(url)) === 'm3u8') parseHLS(String(url));
      else addVideo(String(url), fmtFromUrl(String(url)), '');
    }
    return origOpen.apply(this, [method, url, ...rest]);
  };
} catch(e) {}

// Observers + periodic rescan — heavily throttled to avoid main-thread jank.
// MutationObserver: debounce to max once per 3s.
let mutationDebounce = 0;
new MutationObserver(() => {
  if (mutationDebounce) return;
  mutationDebounce = 1;
  setTimeout(() => {
    mutationDebounce = 0;
    scanForVideo();
  }, 3000);
}).observe(document.documentElement, { childList: true, subtree: true });

// Scroll/resize: only update position if we already have videos (cheap).
let scrollResizeScheduled = false;
function scheduleScrollResizeUpdate() {
  if (scrollResizeScheduled || detectedVideos.length === 0) return;
  scrollResizeScheduled = true;
  requestAnimationFrame(() => {
    scrollResizeScheduled = false;
    sendOverlayUpdateNow();
  });
}
window.addEventListener('scroll', scheduleScrollResizeUpdate, { passive: true });
window.addEventListener('resize', scheduleScrollResizeUpdate, { passive: true });

// Video element events: only scan when media actually loads/starts.
document.addEventListener('loadeddata', () => { setTimeout(scanForVideo, 200); }, true);
document.addEventListener('canplay', () => { setTimeout(scanForVideo, 200); }, true);
document.addEventListener('play', () => { setTimeout(scanForVideo, 200); }, true);

// The button belongs to a *playing* video only. Pausing, ending, hiding
// the tab or leaving the page removes it immediately.
const hideOnStop = () => setTimeout(sendOverlayHide, 0);
document.addEventListener('pause', hideOnStop, true);
document.addEventListener('ended', hideOnStop, true);
document.addEventListener('emptied', hideOnStop, true);
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible') scanForVideo();
  else sendOverlayHide();
}, true);
window.addEventListener('pagehide', sendOverlayHide);
window.addEventListener('beforeunload', sendOverlayHide);

// Initial scan.
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', () => setTimeout(scanForVideo, 300));
} else {
  setTimeout(scanForVideo, 300);
}
// Periodic full rescan — very slow (30s) just to catch late-loading players.
setInterval(scanForVideo, 30000);