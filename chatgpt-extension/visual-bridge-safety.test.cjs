'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const read = name => fs.readFileSync(path.join(__dirname, name), 'utf8').replace(/^\uFEFF/, '');
const backgroundSource = read('background-visual.js');
const baseConfig = {
  enabled: true, deviceId: 'computer-1', origin: 'http://127.0.0.1:8765'
};
function background(raw) {
  const requests = [];
  const context = vm.createContext({
    URL,
    chrome: { runtime: { getURL: name => 'chrome-extension://test/' + name } },
    fetch: async url => {
      requests.push(url);
      assert.equal(url, 'chrome-extension://test/visual-bridge-config.json',
        'disabled bridge must not request local images');
      return { ok: true, json: async () => raw };
    }
  });
  vm.runInContext(backgroundSource + '\nglobalThis.api={visualBridgeConfig,visualBridgeLatest};', context);
  return { api: context.api, requests };
}

test('background: only boolean true enables auto-send', async () => {
  const values = [undefined, false, null, 0, 1, 'true', 'false', [], {}];
  for (const value of values) {
    const raw = { ...baseConfig };
    if (value !== undefined) raw.autoSend = value;
    const { api, requests } = background(raw);
    const result = await api.visualBridgeLatest('');
    assert.equal(result.changed, false);
    assert.equal(result.config.autoSend, false);
    assert.equal(requests.length, 1);
  }
  const { api } = background({ ...baseConfig, autoSend: true });
  assert.equal((await api.visualBridgeConfig()).autoSend, true);
});

test('background: disabled bridge never requests an image', async () => {
  const { api, requests } = background({ ...baseConfig, enabled: false, autoSend: true });
  assert.equal((await api.visualBridgeLatest('')).changed, false);
  assert.equal(requests.length, 1);
});

for (const filename of ['content-chatgpt-native.js', 'content-chatgpt-visual.js']) {
  test(filename + ': missing or disabled consent does not touch the composer', async () => {
    const source = read(filename);
    const start = source.indexOf('async function sendImage(result) {');
    const end = source.indexOf('\nasync function poll()', start);
    assert.ok(start >= 0 && end > start, 'sendImage function must exist');
    let controllerCalls = 0;
    const context = vm.createContext({
      controller: () => { controllerCalls++; throw new Error('controller reached'); }
    });
    vm.runInContext(source.slice(start, end) + '\nglobalThis.send=sendImage;', context);
    for (const result of [undefined, {}, { config: {} },
      { config: { enabled: true } },
      { config: { enabled: true, autoSend: false } },
      { config: { enabled: true, autoSend: 'true' } },
      { config: { enabled: false, autoSend: true } }]) {
      assert.equal(await context.send(result), false);
    }
    assert.equal(controllerCalls, 0);
    await assert.rejects(context.send({ config: { enabled: true, autoSend: true } }), /controller reached/);
    assert.equal(controllerCalls, 1);
  });
}

test('shipped extension config disables auto-send', () => {
  assert.equal(JSON.parse(read('visual-bridge-config.json')).autoSend, false);
});
