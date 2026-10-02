const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = (name) => fs.readFileSync(path.join(__dirname, name), 'utf8');

const JOB = 'img-0f8e2a4c-1111-2222-3333-444455556666';

function backgroundHarness() {
  const stored = {};
  const posted = [];
  const created = [];
  const removed = [];
  const sent = [];
  const removedListeners = [];
  const fetches = [];
  const context = vm.createContext({
    URL, Date, Promise, AbortSignal, JSON, String, Array, Object, Uint8Array, btoa,
    CHATGPT_HOME: 'https://chatgpt.com/',
    setTimeout: (fn) => { fn(); return 0; },
    chrome: {
      storage: { session: {
        get: async (key) => (key === null ? { ...stored } : { [key]: stored[key] }),
        set: async (values) => Object.assign(stored, values),
        remove: async (key) => { for (const item of [].concat(key)) delete stored[item]; },
      } },
      tabs: {
        create: async (options) => { created.push(options); return { id: 41, url: options.url }; },
        remove: async (id) => { removed.push(id); },
        onRemoved: { addListener: (fn) => removedListeners.push(fn) },
      },
    },
    fetch: async (url, init) => {
      fetches.push({ url, init });
      if (url.includes('/api/local/chatgpt/images/')) {
        posted.push({ url, body: JSON.parse(init.body) });
        return { ok: true, status: 200, json: async () => ({ accepted: true }) };
      }
      return {
        ok: true, status: 200,
        headers: { get: () => 'image/png' },
        arrayBuffer: async () => new Uint8Array([0x89, 0x50, 0x4e, 0x47]).buffer,
      };
    },
    safeTab: async (id) => (id === 41 ? { id: 41, url: 'https://chatgpt.com/' } : null),
    acquireNewConversationTab: async () => {
      const options = { url: 'https://chatgpt.com/', active: true };
      created.push(options);
      return { id: 41, url: options.url };
    },
    sendToChatGpt: async (tabId, payload) => { sent.push({ tabId, payload }); return { ok: true }; },
    postJson: async (base, url, body) => { posted.push({ url: `${base}${url}`, body }); return { accepted: true }; },
    logExtension: async () => undefined,
    errorMessage: (error) => (error instanceof Error ? error.message : String(error)),
    isChatGptUrl: (url) => { try { return new URL(url).origin === 'https://chatgpt.com'; } catch { return false; } },
    localOrigin: (value) => {
      const url = new URL(value);
      if (url.protocol !== 'http:' || !['localhost', '127.0.0.1'].includes(url.hostname)) throw new Error('local only');
      return url.origin;
    },
  });
  vm.runInContext(source('background-image.js'), context);
  const call = (message, sender = {}) => new Promise((resolve) => {
    const handled = context.handleImageBridgeMessage(message, sender, resolve);
    if (!handled) resolve({ unhandled: true });
  });
  const settle = () => new Promise((resolve) => setImmediate(resolve));
  return { context, stored, posted, created, removed, sent, removedListeners, fetches, call, settle };
}

test('image-send opens a background ChatGPT tab and forwards the prompt', async () => {
  const h = backgroundHarness();
  const response = await h.call({ type: 'chatcmd-local-command', action: 'image-send', jobId: JOB, prompt: 'Create an image of a red potion', model: 'Auto', localBaseUrl: 'http://127.0.0.1:8080' });
  assert.equal(response.ok, true);
  await h.settle();
  assert.equal(h.created.length, 1);
  assert.equal(h.created[0].url, 'https://chatgpt.com/');
  assert.equal(h.created[0].active, true);
  assert.equal(h.sent[0].tabId, 41);
  assert.equal(h.sent[0].payload.type, 'chatcmd-image-run');
  assert.equal(h.sent[0].payload.prompt, 'Create an image of a red potion');
  assert.equal(h.stored[`chatcmd-image-job:${JOB}`].tabId, 41);
});

test('image-send refuses non-local callback origins', async () => {
  const h = backgroundHarness();
  const response = await h.call({ type: 'chatcmd-local-command', action: 'image-send', jobId: JOB, prompt: 'x', localBaseUrl: 'https://evil.example' });
  assert.equal(response.ok, false);
  assert.equal(h.created.length, 0);
});

test('progress is accepted only from the tab that owns the job', async () => {
  const h = backgroundHarness();
  h.stored[`chatcmd-image-job:${JOB}`] = { kind: 'image-job', jobId: JOB, tabId: 41, localBaseUrl: 'http://127.0.0.1:8080' };
  const foreign = await h.call({ type: 'chatcmd-image-progress', stage: 'started', jobId: JOB }, { tab: { id: 99 } });
  assert.equal(foreign.ok, false);
  const own = await h.call({ type: 'chatcmd-image-progress', stage: 'started', jobId: JOB, conversationUrl: 'https://chatgpt.com/c/abc' }, { tab: { id: 41 } });
  assert.equal(own.ok, true);
  assert.match(h.posted[0].url, /\/api\/local\/chatgpt\/images\/img-.*\/started$/);
});

test('completed result is posted to ChatCMD and closes the tab; failed keeps it open', async () => {
  const h = backgroundHarness();
  h.stored[`chatcmd-image-job:${JOB}`] = { kind: 'image-job', jobId: JOB, tabId: 41, localBaseUrl: 'http://127.0.0.1:8080' };
  const images = [{ mimeType: 'image/png', dataBase64: 'iVBORw0KGgo=' }];
  const done = await h.call({ type: 'chatcmd-image-progress', stage: 'result', status: 'completed', jobId: JOB, images }, { tab: { id: 41 } });
  assert.equal(done.ok, true);
  assert.equal(h.posted.at(-1).body.status, 'completed');
  assert.equal(h.posted.at(-1).body.images.length, 1);
  assert.deepEqual(h.removed, [41]);
  assert.equal(h.stored[`chatcmd-image-job:${JOB}`], undefined);

  const second = backgroundHarness();
  second.stored[`chatcmd-image-job:${JOB}`] = { kind: 'image-job', jobId: JOB, tabId: 41, localBaseUrl: 'http://127.0.0.1:8080' };
  await second.call({ type: 'chatcmd-image-progress', stage: 'result', status: 'failed', jobId: JOB, errorMessage: 'no image' }, { tab: { id: 41 } });
  assert.equal(second.posted.at(-1).body.status, 'failed');
  assert.deepEqual(second.removed, []);
});

test('background image fetch is limited to ChatCMD image tabs and OpenAI image hosts', async () => {
  const h = backgroundHarness();
  const sender = { tab: { id: 41, url: 'https://chatgpt.com/c/abc' } };
  const notOwned = await h.call({ type: 'chatcmd-image-fetch', url: 'https://files.oaiusercontent.com/a.png' }, sender);
  assert.equal(notOwned.ok, false);

  h.stored[`chatcmd-image-job:${JOB}`] = { kind: 'image-job', jobId: JOB, tabId: 41, localBaseUrl: 'http://127.0.0.1:8080' };
  const evil = await h.call({ type: 'chatcmd-image-fetch', url: 'https://evil.example/a.png' }, sender);
  assert.equal(evil.ok, false);
  const ok = await h.call({ type: 'chatcmd-image-fetch', url: 'https://files.oaiusercontent.com/a.png' }, sender);
  assert.equal(ok.ok, true);
  assert.equal(ok.mimeType, 'image/png');
  assert.equal(ok.dataBase64, 'iVBORw==');
});

test('closing an unfinished image tab reports a failure to ChatCMD', async () => {
  const h = backgroundHarness();
  h.stored[`chatcmd-image-job:${JOB}`] = { kind: 'image-job', jobId: JOB, tabId: 41, localBaseUrl: 'http://127.0.0.1:8080' };
  h.removedListeners[0](41);
  await h.settle(); await h.settle();
  assert.equal(h.posted.at(-1).body.status, 'failed');
  assert.match(h.posted.at(-1).body.errorMessage, /closed/);
});

test('content image helpers accept only ChatGPT image sources and dedupe signed URLs', () => {
  const listeners = [];
  const context = vm.createContext({
    URL, Node: { DOCUMENT_POSITION_FOLLOWING: 4 }, Map, Set, Promise, setTimeout,
    document: { documentElement: { dataset: {}, setAttribute() {}, getAttribute() { return null; } }, querySelectorAll: () => [] },
    chrome: { runtime: { onMessage: { addListener: (fn) => listeners.push(fn) } } },
    ChatCmdRuntime: { install: () => ({}), current: () => true, sendMessage: async () => ({ ok: true }) },
    ChatCmdController: { findComposer: () => null },
    ChatCmdConversationDom: { findStopButton: () => null, isVisible: () => true, assistantNodes: () => [] },
  });
  context.globalThis = context;
  vm.runInContext(source('content-chatgpt-image.js'), context);
  const helpers = context.ChatCmdImageJobs;
  assert.equal(helpers.allowedImageSource('https://chatgpt.com/backend-api/estuary/content?id=file_1&sig=a'), true);
  assert.equal(helpers.allowedImageSource('https://files.oaiusercontent.com/file-1?se=1'), true);
  assert.equal(helpers.allowedImageSource('blob:https://chatgpt.com/0b1c'), true);
  assert.equal(helpers.allowedImageSource('https://cdn.oaistatic.com/avatar.png'), false);
  assert.equal(helpers.allowedImageSource('https://evil.example/x.png'), false);
  assert.equal(
    helpers.imageKey('https://chatgpt.com/backend-api/estuary/content?id=file_1&sig=a'),
    helpers.imageKey('https://chatgpt.com/backend-api/estuary/content?id=file_1&sig=b'),
  );
  assert.equal(listeners.length, 1);
});
