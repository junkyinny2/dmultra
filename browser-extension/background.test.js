const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, 'background.js'), 'utf8');

test('recreates context menus cleanly on every extension install or update', () => {
  const listeners = {};
  const menus = new Map();
  let clearCount = 0;
  const runtime = {
    lastError: null,
    onMessage: { addListener() {} },
    onInstalled: {
      addListener(listener) {
        listeners.installed = listener;
      },
    },
  };
  const contextMenus = {
    onClicked: { addListener() {} },
    removeAll(callback) {
      clearCount += 1;
      menus.clear();
      callback();
    },
    create(menu, callback) {
      if (menus.has(menu.id)) {
        runtime.lastError = { message: `duplicate menu id: ${menu.id}` };
      } else {
        menus.set(menu.id, menu);
      }
      callback();
      runtime.lastError = null;
    },
  };
  const chrome = {
    runtime,
    contextMenus,
    tabs: {
      sendMessage() {},
      onRemoved: { addListener() {} },
    },
    webRequest: {
      onBeforeRequest: { addListener() {} },
    },
  };

  vm.runInNewContext(source, { chrome, console, Map, Set });
  listeners.installed();
  listeners.installed();

  assert.equal(clearCount, 2);
  assert.deepEqual([...menus.keys()], [
    'dmultra-link',
    'dmultra-grab',
    'dmultra-video',
  ]);
});

test('only the active video frame can hide the overlay', () => {
  const messageListeners = [];
  const nativeMessages = [];
  const runtime = {
    lastError: null,
    onMessage: {
      addListener(listener) {
        messageListeners.push(listener);
      },
    },
    onInstalled: { addListener() {} },
    sendNativeMessage(_host, message, callback) {
      nativeMessages.push(message);
      callback();
    },
  };
  const chrome = {
    runtime,
    contextMenus: {
      onClicked: { addListener() {} },
      removeAll(callback) { callback(); },
      create(_menu, callback) { callback(); },
    },
    tabs: {
      sendMessage() {},
      onRemoved: { addListener() {} },
    },
    webRequest: {
      onBeforeRequest: { addListener() {} },
    },
  };

  vm.runInNewContext(source, { chrome, console, Map, Set });
  messageListeners[0](
    { action: 'videoOverlay', url: 'https://video.test/main.mp4', w: 800, h: 450 },
    { tab: { id: 1 }, frameId: 2 }
  );
  messageListeners[0](
    { action: 'videoOverlayHide' },
    { tab: { id: 1 }, frameId: 0 }
  );
  messageListeners[0](
    { action: 'videoOverlayHide' },
    { tab: { id: 1 }, frameId: 2 }
  );

  assert.deepEqual(nativeMessages.map(({ action }) => action), [
    'videoOverlay',
    'videoOverlayHide',
  ]);
});
