// fs_write_chatgpt_image: open a fresh ChatGPT chat, let the content script
// submit the prompt and collect the generated images, then relay the bytes to
// the local ChatCMD API. ChatCMD (not the extension) writes the files.
const IMAGE_JOB_PREFIX = 'chatcmd-image-job:';
const IMAGE_FETCH_MAX_BYTES = 32 * 1024 * 1024;
const IMAGE_ALLOWED_MIME = ['image/png', 'image/jpeg', 'image/webp', 'image/gif'];

function imageJobKey(jobId) { return `${IMAGE_JOB_PREFIX}${jobId}`; }

async function imageJobContext(jobId) {
  const key = imageJobKey(jobId);
  const stored = await chrome.storage.session.get(key);
  return stored[key] || null;
}

async function imageJobForTab(tabId) {
  const stored = await chrome.storage.session.get(null);
  return Object.values(stored).find((value) => value && typeof value === 'object' && value.kind === 'image-job' && value.tabId === tabId) || null;
}

async function startImageJob(message) {
  const jobId = String(message.jobId || '').trim();
  const prompt = String(message.prompt || '');
  if (!/^img-[A-Za-z0-9-]{8,80}$/.test(jobId) || !prompt.trim()) throw new Error('Invalid image generation job.');
  const localBaseUrl = localOrigin(message.localBaseUrl);
  const existing = await imageJobContext(jobId);
  if (existing?.tabId && await safeTab(existing.tabId)) return;

  // Reuse the same new-conversation acquisition path as ordinary ChatCMD messages.
  // This avoids maintaining a second tab-opening flow that can drift into Work mode.
  const tab = await acquireNewConversationTab(null, CHATGPT_HOME);
  if (!tab?.id) throw new Error('Could not start image generation.');
  await chrome.storage.session.set({
    [imageJobKey(jobId)]: { kind: 'image-job', jobId, tabId: tab.id, localBaseUrl, startedAt: Date.now() },
  });
  await logExtension('info', 'image', `Started image generation job ${jobId} in tab ${tab.id}.`);
  await sendToChatGpt(tab.id, {
    type: 'chatcmd-image-run',
    jobId,
    prompt,
    model: message.model || 'Auto',
  });
}

async function finishImageJob(jobId, payload, { closeTab }) {
  const context = await imageJobContext(jobId);
  if (!context?.localBaseUrl) return { accepted: false, reason: 'job_context_missing' };
  let result;
  try {
    result = await postImageResult(context.localBaseUrl, jobId, payload);
  } finally {
    await chrome.storage.session.remove(imageJobKey(jobId));
  }
  if (closeTab && context.tabId) {
    setTimeout(() => void chrome.tabs.remove(context.tabId).catch(() => undefined), 1_500);
  }
  return result;
}

async function postImageResult(baseUrl, jobId, body) {
  // Image payloads are large; do not use postJson's 10 s timeout.
  const response = await fetch(`${baseUrl}/api/local/chatgpt/images/${encodeURIComponent(jobId)}/result`, {
    method: 'POST',
    signal: AbortSignal.timeout(120_000),
    headers: { 'Content-Type': 'application/json', 'X-ChatCmdClient': 'chatgpt-extension' },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    let detail = `ChatCMD local API returned ${response.status}.`;
    try { detail = (await response.json()).detail || detail; } catch { /* non-json */ }
    throw new Error(detail);
  }
  return response.json();
}

async function reportImageFailure(jobId, localBaseUrl, error) {
  if (!jobId || !localBaseUrl) return;
  try {
    await postImageResult(localBaseUrl, jobId, { status: 'failed', errorMessage: errorMessage(error) });
  } catch { /* ChatCMD may be closed or the job already finished */ }
  await chrome.storage.session.remove(imageJobKey(jobId)).catch(() => undefined);
}

async function handleImageProgress(message, tabId) {
  const jobId = String(message.jobId || '');
  const context = await imageJobContext(jobId);
  if (!context) throw new Error('Unknown image generation job.');
  if (!tabId || context.tabId !== tabId) throw new Error('Image progress came from a different tab.');
  if (message.stage === 'started') {
    await postJson(context.localBaseUrl, `/api/local/chatgpt/images/${encodeURIComponent(jobId)}/started`, {
      conversationUrl: message.conversationUrl,
    });
    return { stage: 'started' };
  }
  if (message.stage === 'result') {
    const completed = message.status === 'completed';
    const images = Array.isArray(message.images) ? message.images : [];
    await logExtension(completed ? 'info' : 'warn', 'image', `Image job ${jobId} ${message.status}: ${images.length} image(s).${message.errorMessage ? ` ${message.errorMessage}` : ''}`);
    // Keep failed tabs open so the user can inspect the generation response.
    const result = await finishImageJob(jobId, {
      status: completed ? 'completed' : 'failed',
      images,
      conversationUrl: message.conversationUrl,
      assistantText: message.assistantText,
      errorMessage: message.errorMessage,
    }, { closeTab: completed });
    return { stage: 'result', ...result };
  }
  throw new Error(`Unsupported image progress stage: ${message.stage || 'missing'}.`);
}

// Fallback download for image URLs the page context cannot fetch (CORS).
async function fetchImageForTab(message, sender) {
  if (!isChatGptUrl(sender.tab?.url) || !await imageJobForTab(sender.tab?.id)) {
    throw new Error('Image download is only available to ChatCMD image tabs.');
  }
  const url = new URL(String(message.url || ''));
  const allowedHost = url.hostname === 'chatgpt.com' || url.hostname.endsWith('.oaiusercontent.com');
  if (url.protocol !== 'https:' || !allowedHost) throw new Error(`Refusing to download image from ${url.hostname}.`);
  const response = await fetch(url.href, { credentials: 'include', cache: 'no-store', signal: AbortSignal.timeout(60_000) });
  if (!response.ok) throw new Error(`Image download failed with HTTP ${response.status}.`);
  const mimeType = String(response.headers.get('content-type') || '').split(';')[0].trim().toLowerCase();
  if (!IMAGE_ALLOWED_MIME.includes(mimeType)) throw new Error(`Downloaded content is not an image (${mimeType || 'unknown'}).`);
  const buffer = await response.arrayBuffer();
  if (buffer.byteLength <= 0 || buffer.byteLength > IMAGE_FETCH_MAX_BYTES) throw new Error('Downloaded image size is invalid.');
  return { mimeType, dataBase64: imageBytesToBase64(new Uint8Array(buffer)) };
}

function imageBytesToBase64(bytes) {
  let binary = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, Math.min(i + chunk, bytes.length)));
  }
  return btoa(binary);
}

function handleImageBridgeMessage(message, sender, sendResponse) {
  if (message?.type === 'chatcmd-local-command' && message.action === 'image-send') {
    let localBaseUrl;
    try { localBaseUrl = localOrigin(message.localBaseUrl); }
    catch (error) { sendResponse({ ok: false, error: errorMessage(error) }); return true; }
    void startImageJob(message).catch((error) => void reportImageFailure(message.jobId, localBaseUrl, error));
    // Acknowledge before the tab loads; startup failures go to /result.
    sendResponse({ ok: true });
    return true;
  }
  if (message?.type === 'chatcmd-image-progress') {
    void handleImageProgress(message, sender.tab?.id)
      .then((result) => sendResponse({ ok: true, ...result }))
      .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
    return true;
  }
  if (message?.type === 'chatcmd-image-fetch') {
    void fetchImageForTab(message, sender)
      .then((result) => sendResponse({ ok: true, ...result }))
      .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
    return true;
  }
  return false;
}

chrome.tabs.onRemoved.addListener((tabId) => {
  setTimeout(() => void (async () => {
    const context = await imageJobForTab(tabId);
    if (context?.jobId) await reportImageFailure(context.jobId, context.localBaseUrl, new Error('The image generation session was closed before output was delivered.'));
  })(), 400);
});
