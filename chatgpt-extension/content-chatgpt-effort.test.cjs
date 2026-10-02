const test = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { JSDOM } = require('../web/node_modules/jsdom');

const source = readFileSync(join(__dirname, 'content-chatgpt-effort.js'), 'utf8');

function fixture(html) {
  const dom = new JSDOM(html, { url: 'https://chatgpt.com/', runScripts: 'outside-only', pretendToBeVisual: true });
  dom.window.ChatCmdConversationDom = { isVisible: () => true };
  dom.window.eval(source);
  return dom;
}

test('same web and configured effort performs zero clicks', async () => {
  const dom = fixture('<button id="effort" aria-haspopup="menu" aria-label="Reasoning effort Medium">Medium</button>');
  const button = dom.window.document.querySelector('#effort');
  let clicks = 0;
  button.addEventListener('click', () => { clicks += 1; });

  const result = await dom.window.ChatCmdEffort.select('medium');

  assert.equal(result.current, 'medium');
  assert.equal(result.changed, false);
  assert.equal(clicks, 0);
  dom.window.close();
});

test('inherit keeps the current ChatGPT web setting without opening any menu', async () => {
  const dom = fixture('<button id="effort" aria-haspopup="menu" aria-label="추론 노력 높음">높음</button>');
  const button = dom.window.document.querySelector('#effort');
  let clicks = 0;
  button.addEventListener('click', () => { clicks += 1; });

  const result = await dom.window.ChatCmdEffort.select('inherit');

  assert.equal(result.current, 'high');
  assert.equal(result.changed, false);
  assert.equal(clicks, 0);
  dom.window.close();
});

test('different configured effort uses the visible ChatGPT control and menu option', async () => {
  const dom = fixture(`
    <button id="effort" aria-haspopup="menu" aria-label="Reasoning effort Medium">Medium</button>
    <div role="menu">
      <div id="medium" role="menuitemradio" aria-checked="false">Medium</div>
      <div id="high" role="menuitemradio" aria-checked="false">High</div>
    </div>
  `);
  const button = dom.window.document.querySelector('#effort');
  const high = dom.window.document.querySelector('#high');
  let buttonClicks = 0;
  let optionClicks = 0;
  button.addEventListener('click', () => { buttonClicks += 1; });
  high.addEventListener('click', () => {
    optionClicks += 1;
    button.textContent = 'High';
    button.setAttribute('aria-label', 'Reasoning effort High');
    high.setAttribute('aria-checked', 'true');
  });

  const result = await dom.window.ChatCmdEffort.select('high');

  assert.equal(result.current, 'high');
  assert.equal(result.changed, true);
  assert.equal(buttonClicks, 1);
  assert.equal(optionClicks, 1);
  dom.window.close();
});

test('Korean extra-high labels normalize to the same effort value', () => {
  const dom = fixture('<button aria-haspopup="menu" aria-label="추론 매우 높음">매우 높음</button>');
  assert.equal(dom.window.ChatCmdEffort.current(), 'extraHigh');
  assert.equal(dom.window.ChatCmdEffort.normalize('extra high'), 'extraHigh');
  dom.window.close();
});

test('sub-agent background path uses effort and does not select a model', () => {
  const background = readFileSync(join(__dirname, 'background.js'), 'utf8');
  const start = background.indexOf("async function startSubagentRequestOnce(message)");
  const end = background.indexOf('const subagentClosures', start);
  const subagentBlock = background.slice(start, end);
  assert.match(subagentBlock, /effort:\s*message\.effort\s*\|\|\s*'inherit'/);
  assert.doesNotMatch(subagentBlock, /model\s*:/);
});

test('background preflight uses the current web effort to suppress unnecessary UI changes', () => {
  const backgroundIo = readFileSync(join(__dirname, 'background-io.js'), 'utf8');
  assert.match(backgroundIo, /health\.reasoningEffort\s*===\s*targetEffort/);
  assert.match(backgroundIo, /payload\s*=\s*\{\s*\.\.\.payload,\s*effort:\s*'inherit'\s*\}/);
});

test('content health exposes the current normalized reasoning effort', () => {
  const content = readFileSync(join(__dirname, 'content-chatgpt.js'), 'utf8');
  assert.match(content, /reasoningEffort:\s*globalThis\.ChatCmdEffort\?\.current\?\.\(\)\s*\|\|\s*null/);
});
