const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, 'content.js'), 'utf8');

function createClickHarness({ documentElement = {}, videos = [] } = {}) {
  let clickHandler;
  const sent = [];
  const fetchCalls = [];
  const intervals = [];
  const documentHandlers = new Map();
  let observedRoot;
  const fetch = async (url) => {
    fetchCalls.push(url);
    return { text: async () => '' };
  };
  const document = {
    addEventListener(type, handler) {
      if (type === 'click') clickHandler = handler;
      const handlers = documentHandlers.get(type) || [];
      handlers.push(handler);
      documentHandlers.set(type, handlers);
    },
    documentElement,
    visibilityState: 'visible',
    readyState: 'complete',
    title: 'Example video',
    hasFocus: () => true,
    querySelectorAll: (selector) => (
      selector === 'video' || selector === 'video, object, embed' ? videos : []
    ),
  };
  class MutationObserver {
    observe(root) {
      if (!root) throw new TypeError('MutationObserver root is not a Node');
      observedRoot = root;
    }
  };
  const window = {
    addEventListener() {},
    fetch,
    innerWidth: 1200,
    innerHeight: 800,
    screenX: 0,
    screenY: 0,
    outerWidth: 1200,
    outerHeight: 850,
    parent: null,
  };
  window.parent = window;
  const context = {
    document,
    location: { href: 'https://page.test/' },
    window,
    chrome: {
      runtime: {
        sendMessage: (message) => sent.push(message),
        onMessage: { addListener() {} },
      },
    },
    MutationObserver,
    URL,
    setTimeout: () => 1,
    setInterval: (callback, delay) => {
      intervals.push({ callback, delay });
      return intervals.length;
    },
    requestAnimationFrame() {},
    fetch,
    XMLHttpRequest: function () {},
    console,
  };
  Object.defineProperty(context, 'fetch', {
    configurable: true,
    get: () => window.fetch,
    set: (value) => { window.fetch = value; },
  });
  context.XMLHttpRequest.prototype.open = function () {};
  vm.runInNewContext(source, context);

  return {
    sent,
    fetchCalls,
    intervals,
    window,
    document,
    context,
    documentHandlers,
    get observedRoot() { return observedRoot; },
    click({ anchor, target = anchor, path = [anchor, document], ...modifiers }) {
      let prevented = false;
      clickHandler({
        button: 0,
        target,
        composedPath: () => path,
        altKey: false,
        ctrlKey: false,
        metaKey: false,
        shiftKey: false,
        ...modifiers,
        preventDefault() {
          prevented = true;
        },
        stopPropagation() {},
      });
      return prevented;
    },
  };
}

function anchor(href, download = '', hasDownload = false) {
  return {
    localName: 'a',
    href,
    getAttribute: (name) => (name === 'download' ? download : 'href'),
    hasAttribute: (name) => name === 'download' && hasDownload,
  };
}

test('intercepts encoded file links inside composed event paths', () => {
  const harness = createClickHarness();
  const link = anchor('https://host.test/files/archive%2Ezip?token=abc');
  const host = { nodeType: 1 };
  assert.equal(harness.click({ anchor: link, target: host, path: [link, host] }), true);
  assert.equal(harness.sent[0].url, link.href);
});

test('intercepts file links whose name is in the query string', () => {
  const harness = createClickHarness();
  const link = anchor('https://host.test/get?id=42&filename=archive.zip');
  assert.equal(harness.click({ anchor: link }), true);
  assert.equal(harness.sent[0].url, link.href);
});

test('leaves ordinary web navigation alone', () => {
  const harness = createClickHarness();
  const link = anchor('https://host.test/about');
  assert.equal(harness.click({ anchor: link }), false);
  assert.equal(harness.sent.length, 0);
});

test('does not recursively parse an HLS playlist fetched by the parser', async () => {
  const harness = createClickHarness();
  await harness.window.fetch('https://host.test/video.m3u8');
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(harness.fetchCalls, [
    'https://host.test/video.m3u8',
    'https://host.test/video.m3u8',
  ]);
});

test('waits for documentElement before observing early-injected frames', () => {
  const harness = createClickHarness({ documentElement: null });
  assert.equal(harness.observedRoot, undefined);
  harness.document.documentElement = {};
  for (const handler of harness.documentHandlers.get('DOMContentLoaded') || []) {
    handler();
  }
  assert.equal(harness.observedRoot, harness.document.documentElement);
});

test('labels the detected stream with the playing video resolution', () => {
  const video = {
    paused: false,
    ended: false,
    readyState: 2,
    videoWidth: 1920,
    videoHeight: 1080,
    currentSrc: 'https://host.test/movie.mp4',
    getBoundingClientRect: () => ({
      left: 20,
      top: 30,
      right: 820,
      bottom: 480,
    }),
  };
  const harness = createClickHarness({ videos: [video] });
  harness.context.addVideo(video.currentSrc, 'mp4', '');

  assert.equal(harness.sent[0].qualities[0].label, 'MP4 - 1920x1080');
});

test('refreshes the native overlay before its stale heartbeat timeout', () => {
  const video = {
    paused: false,
    ended: false,
    readyState: 2,
    videoWidth: 1280,
    videoHeight: 720,
    currentSrc: 'https://host.test/movie.mp4',
    getBoundingClientRect: () => ({
      left: 20,
      top: 30,
      right: 820,
      bottom: 480,
    }),
  };
  const harness = createClickHarness({ videos: [video] });
  harness.context.addVideo(video.currentSrc, 'mp4', '');
  const heartbeat = harness.intervals.find(({ delay }) => delay === 2000);

  assert.ok(heartbeat, 'overlay heartbeat interval is registered');
  heartbeat.callback();
  assert.equal(
    harness.sent.filter(({ action }) => action === 'videoOverlay').length,
    2
  );
});
