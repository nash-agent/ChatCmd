// fs_write_chatgpt_image page driver: submit one prompt in a ChatCMD-owned
// tab, wait until ChatGPT finishes generating, then download the new images
// with the page's signed-in session and hand them to the background worker.
(() => {
  const CONTEXT = globalThis.ChatCmdRuntime.install('chatgpt-image');
  const controller = globalThis.ChatCmdController;
  const dom = globalThis.ChatCmdConversationDom;
  const POLL_MS = 1_500;
  const SETTLE_MS = 8_000;
  const NO_IMAGE_GRACE_MS = 60_000;
  const MAX_RUN_MS = 9 * 60 * 1000;
  const MIN_NATURAL_EDGE = 256;
  const MIN_RENDERED_EDGE = 160;
  const MAX_IMAGES = 8;
  let activeJob = null;

  const current = () => globalThis.ChatCmdRuntime.current(CONTEXT);
  const delay = (ms) => globalThis.ChatCmdCaptureClock?.sleep(ms) ?? new Promise((resolve) => setTimeout(resolve, ms));
  const errorMessage = (error) => (error instanceof Error ? error.message : String(error || 'Image generation failed.'));

  chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
    if (!current() || message?.type !== 'chatcmd-image-run') return false;
    if (activeJob) {
      sendResponse({ ok: false, error: 'This generation session is already creating an image.' });
      return false;
    }
    if (!controller?.findComposer() || dom.findStopButton()) {
      sendResponse({ ok: false, error: 'The image generation session is not ready.' });
      return false;
    }
    activeJob = { id: message.jobId, startedAt: Date.now() };
    // Keeps native capture from recording this ChatCMD-owned conversation.
    document.documentElement.dataset.chatcmdImageTab = '1';
    void runJob(message).finally(() => { activeJob = null; });
    sendResponse({ ok: true });
    return false;
  });

  async function ensureChatMode() {
    const composer = controller?.findComposer?.();
    const scopes = [composer?.closest?.('form'), composer?.parentElement, document].filter(Boolean);
    let workButton = null;
    for (const scope of scopes) {
      workButton = [...scope.querySelectorAll?.('button,[role="button"]') || []].filter(dom.isVisible).find((button) => {
        const label = dom.normalize(button.getAttribute('aria-label') || button.getAttribute('title') || button.textContent);
        return /^(work|작업)$/.test(label);
      });
      if (workButton) break;
    }
    if (!workButton) return;
    workButton.click();
    await delay(150);
    const chatButton = await waitFor(() => [...document.querySelectorAll('[role="menuitem"],button,[role="option"]')].filter(dom.isVisible).find((button) => {
      const label = dom.normalize(button.getAttribute('aria-label') || button.getAttribute('title') || button.textContent);
      return /^(chat|채팅)$/.test(label);
    }), 3_000, 'Could not switch the image-generation tab from Work to Chat mode.');
    chatButton.click();
    await delay(150);
  }

  async function runJob(message) {
    const jobId = message.jobId;
    let conversationUrl;
    try {
      await ensureChatMode();
      const composer = await waitFor(() => controller.findComposer(), 20_000, 'The image generation environment is not ready.');
      await controller.selectModel(message.model);
      const baseline = new Set(collectImages().map((image) => image.key));
      controller.setComposerText(composer, message.prompt);
      await controller.submitPrompt(composer);
      conversationUrl = await waitForConversationUrl();
      await progress({ stage: 'started', jobId, conversationUrl });
      const found = await waitForImages(baseline);
      const images = [];
      for (const image of found.images) images.push(await downloadImage(image));
      await progress({
        stage: 'result', jobId, status: 'completed', images,
        conversationUrl: conversationIdentityUrl() || conversationUrl,
        assistantText: found.text || undefined,
      });
    } catch (error) {
      if (!current()) return;
      await progress({
        stage: 'result', jobId, status: 'failed',
        errorMessage: errorMessage(error),
        conversationUrl: conversationIdentityUrl() || conversationUrl,
        assistantText: latestAssistantText() || undefined,
      }).catch(() => undefined);
    }
  }

  function modeControlLabel(element) {
    return dom.normalize(element?.getAttribute?.('aria-label') || element?.textContent || '');
  }

  function findChatModeControl() {
    const controls = [...document.querySelectorAll('button,[role="tab"],[role="radio"]')].filter(dom.isVisible);
    for (const chat of controls) {
      if (!/^(?:chat|채팅)(?:\s+mode|\s*모드)?$/i.test(modeControlLabel(chat))) continue;
      let scope = chat.parentElement;
      for (let depth = 0; scope && depth < 4; depth += 1, scope = scope.parentElement) {
        const siblings = [...scope.querySelectorAll('button,[role="tab"],[role="radio"]')].filter(dom.isVisible);
        if (siblings.some((candidate) => /^(?:work|작업)(?:\s+mode|\s*모드)?$/i.test(modeControlLabel(candidate)))) return chat;
      }
    }
    return null;
  }

  async function ensureChatMode() {
    const chat = findChatModeControl();
    if (!chat) return;
    chat.click();
    await delay(350);
  }

  async function waitForImages(baseline) {
    const startedAt = Date.now();
    let idleSince = null;
    let lastKeys = '';
    let stableSince = Date.now();
    while (Date.now() - startedAt < MAX_RUN_MS) {
      await delay(POLL_MS);
      if (!current()) throw new Error('Extension context invalidated.');
      if (dom.findStopButton()) { idleSince = null; continue; }
      idleSince ??= Date.now();
      const images = collectImages().filter((image) => !baseline.has(image.key)).slice(0, MAX_IMAGES);
      const keys = images.map((image) => image.key).join('|');
      if (keys !== lastKeys) { lastKeys = keys; stableSince = Date.now(); }
      const now = Date.now();
      if (images.length && now - idleSince >= SETTLE_MS && now - stableSince >= SETTLE_MS) {
        return { images, text: latestAssistantText() };
      }
      if (!images.length) {
        const threadError = dom.findThreadError();
        if (threadError && now - idleSince >= 3_000) {
          throw new Error(`Image generation reported an error: ${(threadError.innerText || threadError.textContent || '').trim().slice(0, 500) || 'unknown error'}`);
        }
        if (now - idleSince >= NO_IMAGE_GRACE_MS) throw new Error(noImageMessage(baseline));
      }
    }
    throw new Error('Timed out waiting for image generation to complete.');
  }

  // Candidate generated images: large, visible, not blurred previews, placed
  // after the latest user message and outside the composer/navigation.
  function collectImages() {
    const user = latestUserNode();
    const byKey = new Map();
    for (const img of document.querySelectorAll('img')) {
      const candidate = imageCandidate(img, user);
      if (!candidate) continue;
      const previous = byKey.get(candidate.key);
      if (!previous || candidate.area > previous.area) byKey.set(candidate.key, candidate);
    }
    return [...byKey.values()];
  }

  function imageCandidate(img, user) {
    if (!(img instanceof HTMLImageElement)) return null;
    if (img.closest('form, nav, header, aside, [data-message-author-role="user"]')) return null;
    if (user && !(user.compareDocumentPosition(img) & Node.DOCUMENT_POSITION_FOLLOWING)) return null;
    const url = img.currentSrc || img.src || '';
    if (!allowedImageSource(url)) return null;
    const rect = img.getBoundingClientRect();
    const naturalOk = img.complete && img.naturalWidth >= MIN_NATURAL_EDGE && img.naturalHeight >= MIN_NATURAL_EDGE;
    const renderedOk = rect.width >= MIN_RENDERED_EDGE && rect.height >= MIN_RENDERED_EDGE;
    if (!naturalOk && !renderedOk) return null;
    const style = getComputedStyle(img);
    if (style.visibility === 'hidden' || style.display === 'none' || /blur\(/.test(style.filter || '')) return null;
    return { url, key: imageKey(url), area: Math.max(img.naturalWidth * img.naturalHeight, rect.width * rect.height) };
  }

  function allowedImageSource(value) {
    if (value.startsWith('blob:https://chatgpt.com/') || value.startsWith('data:image/')) return true;
    try {
      const url = new URL(value);
      return url.protocol === 'https:' && (url.hostname === 'chatgpt.com' || url.hostname.endsWith('.oaiusercontent.com'));
    } catch { return false; }
  }

  // Signed URLs change between renders; the file id identifies the image.
  function imageKey(value) {
    try {
      const url = new URL(value);
      const id = url.searchParams.get('id') || url.searchParams.get('file_id');
      if (id) return `${url.hostname}:${id}`;
      return `${url.origin}${url.pathname}`;
    } catch { return value.slice(0, 256); }
  }

  async function downloadImage(image) {
    const sourceUrl = image.url.startsWith('https://') ? image.url : undefined;
    let pageError;
    try {
      const response = await fetch(image.url, { credentials: 'include', cache: 'no-store' });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const blob = await response.blob();
      if (!blob.type.startsWith('image/')) throw new Error(`unexpected content type ${blob.type || 'unknown'}`);
      return { mimeType: blob.type, dataBase64: await blobToBase64(blob), sourceUrl };
    } catch (error) {
      pageError = error;
    }
    if (!sourceUrl) throw new Error(`Could not read the generated image: ${errorMessage(pageError)}`);
    const response = await globalThis.ChatCmdRuntime.sendMessage({ type: 'chatcmd-image-fetch', url: sourceUrl });
    if (!response?.ok) throw new Error(`Could not download the generated image (${errorMessage(pageError)}; ${response?.error || 'background fetch failed'}).`);
    return { mimeType: response.mimeType, dataBase64: response.dataBase64, sourceUrl };
  }

  function blobToBase64(blob) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result || '').replace(/^data:[^,]*,/, ''));
      reader.onerror = () => reject(reader.error || new Error('Could not read image data.'));
      reader.readAsDataURL(blob);
    });
  }

  function noImageMessage(baseline) {
    const skipped = [...document.querySelectorAll('img')]
      .map((img) => img.currentSrc || img.src || '')
      .filter((src) => src && !baseline.has(imageKey(src)) && !allowedImageSource(src))
      .map((src) => { try { return new URL(src).hostname; } catch { return src.slice(0, 20); } });
    const hosts = [...new Set(skipped)].slice(0, 5);
    return `Image generation finished without a generated image.${hosts.length ? ` Ignored image hosts: ${hosts.join(', ')}.` : ''}`;
  }

  function latestUserNode() {
    return [...document.querySelectorAll('[data-message-author-role="user"]')].filter(dom.isVisible).at(-1) || null;
  }

  function latestAssistantText() {
    const user = latestUserNode();
    const nodes = dom.assistantNodes().filter((node) => !user || (user.compareDocumentPosition(node) & Node.DOCUMENT_POSITION_FOLLOWING));
    return nodes.map((node) => (node.innerText || node.textContent || '').trim()).filter(Boolean).join('\n\n').slice(0, 20_000);
  }

  function conversationIdentityUrl() {
    return /(?:^|\/)c\/[^/?#]+/.test(window.location.pathname) ? window.location.href : undefined;
  }

  async function waitForConversationUrl() {
    const startedAt = Date.now();
    while (Date.now() - startedAt < 20_000) {
      const url = conversationIdentityUrl();
      if (url) return url;
      await delay(250);
    }
    return undefined;
  }

  async function waitFor(factory, timeoutMs, message) {
    const startedAt = Date.now();
    while (Date.now() - startedAt < timeoutMs) {
      const value = factory();
      if (value) return value;
      await delay(150);
    }
    throw new Error(message);
  }

  async function progress(payload) {
    if (!current()) throw new Error('Extension context invalidated.');
    const response = await globalThis.ChatCmdRuntime.sendMessage({ type: 'chatcmd-image-progress', ...payload });
    if (!response?.ok) throw new Error(response?.error || 'ChatCMD did not acknowledge the image job progress.');
    return response;
  }

  globalThis.ChatCmdImageJobs = Object.freeze({
    get active() { return activeJob; },
    collectImages, imageKey, allowedImageSource,
  });
})();
