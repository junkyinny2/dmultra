// Native messaging bridge to DM Ultra.
const HOST = 'com.dmultra.nm';
function send(msg) {
  chrome.runtime.sendNativeMessage(HOST, msg, () => {
    if (chrome.runtime.lastError) {
      console.warn('DM Ultra:', chrome.runtime.lastError.message);
    }
  });
}
chrome.runtime.onMessage.addListener((msg) => {
  // Forward video overlay updates directly to the native host.
  if (msg && (msg.action === 'videoOverlay' || msg.action === 'videoOverlayHide')) {
    send(msg);
    return;
  }
  if (msg && typeof msg.url === 'string' && msg.url) send(msg);
  if (msg && Array.isArray(msg.urls) && msg.urls.length > 0) {
    send({ urls: msg.urls, referer: msg.referer || '' });
  }
});
function requestFromPage(action, tab) {
  if (tab && tab.id != null) {
    chrome.tabs.sendMessage(tab.id, { action }, () => {
      void chrome.runtime.lastError;
    });
  }
}
// ---------------------------------------------------------------------------
// Network-level video detection (IDM-style): intercept media URLs from
// network requests, not just DOM attributes. This catches videos that
// use MSE, blob: URLs, or lazy-loaded players.
// ---------------------------------------------------------------------------
const NET_MEDIA_RE = /\.(m3u8|mpd|mp4|webm|mkv|flv|mp3|m4a|mpg|mpeg|wav|ogg|aac)(\?|#|$)/i;
// Track best video per tab: prefer playlists and direct files over segments.
const tabVideos = new Map(); // tabId -> {url, format, priority}
function videoPriority(fmt) {
  if (fmt === 'm3u8' || fmt === 'mp4') return 3;
  if (fmt === 'mpd' || fmt === 'webm' || fmt === 'mkv') return 2;
  return 1;
}
chrome.webRequest.onBeforeRequest.addListener(
  (details) => {
    if (details.tabId < 0) return;
    const url = details.url || '';
    if (!/^https?:/i.test(url)) return;
    const m = url.match(NET_MEDIA_RE);
    if (!m) return;
    const format = m[1].toLowerCase();
    const prio = videoPriority(format);
    if (prio < 1) return;
    const existing = tabVideos.get(details.tabId);
    if (!existing || prio > existing.priority) {
      tabVideos.set(details.tabId, { url, format, priority: prio });
      // Notify the content script in that tab.
      chrome.tabs.sendMessage(details.tabId, {
        action: 'videoUrlFound',
        url: url,
        format: format,
      }, () => { void chrome.runtime.lastError; });
    }
  },
  { urls: ['<all_urls>'] },
  []
);
// Clean up on tab close.
chrome.tabs.onRemoved.addListener((tabId) => {
  tabVideos.delete(tabId);
});
// Handle "what videos do you know about" queries from the content script.
chrome.runtime.onMessage.addListener((msg, sender) => {
  if (msg && msg.action === 'queryVideos' && sender.tab && sender.tab.id != null) {
    const v = tabVideos.get(sender.tab.id);
    if (v) {
      chrome.tabs.sendMessage(sender.tab.id, {
        action: 'videoUrlFound',
        url: v.url,
        format: v.format,
      }, () => { void chrome.runtime.lastError; });
    }
  }
});
chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: 'dmultra-link',
    title: 'Download with DM Ultra',
    contexts: ['link'],
  });
  chrome.contextMenus.create({
    id: 'dmultra-grab',
    title: 'Grab download links from this page',
    contexts: ['page'],
  });
  chrome.contextMenus.create({
    id: 'dmultra-video',
    title: 'Download videos from this page',
    contexts: ['page'],
  });
});
chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === 'dmultra-link' && info.linkUrl &&
      /^https?:/i.test(info.linkUrl)) {
    send({
      url: info.linkUrl,
      referer: (tab && tab.url) || '',
      filename: '',
    });
  }
  if (info.menuItemId === 'dmultra-grab') {
    requestFromPage('grabLinks', tab);
  }
  if (info.menuItemId === 'dmultra-video') {
    requestFromPage('grabVideos', tab);
  }
});
