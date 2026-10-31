const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, 'content.js'), 'utf8');

function createClickHarness() {
  let clickHandler;
  const sent = [];
  const fetchCalls = [];
  const fetch = async (url) => {
    fetchCalls.push(url);
    return { text: async () => '' };
  };
  const document = {
    addEventListener(type, handler) {
      if (type === 'click') clickHandler = handler;
    },
    documentElement: {},
    visibilityState: 'visible',
    hasFocus: () => true,
    querySelectorAll: () => [],
  };
  class MutationObserver {
    observe() {}
  }
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
    setInterval: () => 1,
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
    window,
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
