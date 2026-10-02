const VISUAL_CONFIG_RESOURCE = 'visual-bridge-config.json';
let visualConfigCache = null;
let visualConfigLoadedAt = 0;

function visualBridgeLoopbackOrigin(origin) {
  const url = new URL(String(origin || ''));
  if (url.protocol !== 'http:') throw new Error('Visual Bridge origin must use http on loopback.');
  if (!['127.0.0.1', 'localhost', '::1'].includes(url.hostname)) {
    throw new Error('Visual Bridge origin must be loopback-only.');
  }
  return url.origin;
}

async function visualBridgeConfig(force = false) {
  const now = Date.now();
  if (!force && visualConfigCache && now - visualConfigLoadedAt < 2_000) return visualConfigCache;

  const response = await fetch(chrome.runtime.getURL(VISUAL_CONFIG_RESOURCE), {
    cache: 'no-store',
    credentials: 'omit',
  });
  if (!response.ok) throw new Error('Visual Bridge config failed: ' + response.status);

  const raw = await response.json();
  const enabled = raw?.enabled === true;
  const deviceId = String(raw?.deviceId || '').trim();
  if (enabled && !/^[A-Za-z0-9_-]{1,64}$/.test(deviceId)) {
    throw new Error('Visual Bridge config has invalid deviceId.');
  }

  const config = {
    enabled,
    deviceId,
    origin: enabled ? visualBridgeLoopbackOrigin(raw?.origin) : null,
    autoSend: raw?.autoSend === true,
    pollMs: Math.max(250, Math.min(Number(raw?.pollMs) || 1200, 10_000)),
    maxAgeMs: Math.max(5_000, Math.min(Number(raw?.maxAgeMs) || 90_000, 600_000)),
    initialFreshMs: Math.max(0, Number(raw?.initialFreshMs) || 10_000),
  };
  config.initialFreshMs = Math.min(config.initialFreshMs, config.maxAgeMs);

  visualConfigCache = config;
  visualConfigLoadedAt = now;
  return config;
}

function visualBridgeBytesToBase64(bytes) {
  let binary = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, Math.min(i + chunk, bytes.length)));
  }
  return btoa(binary);
}

async function visualBridgeLatest(lastImageId) {
  const config = await visualBridgeConfig();
  if (!config.enabled || !config.autoSend) {
    return { ok: true, changed: false, config };
  }

  const latestResponse = await fetch(config.origin + '/latest', {
    cache: 'no-store',
    credentials: 'omit',
  });
  if (latestResponse.status === 404) return { ok: true, changed: false, config };
  if (!latestResponse.ok) throw new Error('Visual Bridge latest failed: ' + latestResponse.status);

  const meta = await latestResponse.json();
  if (!meta?.image_id) return { ok: true, changed: false, config };
  if (String(meta.device_id || '') !== config.deviceId) {
    throw new Error('Visual Bridge refused image from a different device.');
  }
  if (meta.image_id === lastImageId) return { ok: true, changed: false, meta, config };

  const imageUrl = new URL(meta.url);
  if (imageUrl.origin !== config.origin || !imageUrl.pathname.startsWith('/v/')) {
    throw new Error('Visual Bridge refused image URL outside configured loopback origin.');
  }

  const imageResponse = await fetch(imageUrl.href, {
    cache: 'no-store',
    credentials: 'omit',
  });
  if (!imageResponse.ok) throw new Error('Visual Bridge image fetch failed: ' + imageResponse.status);

  const mime = String(imageResponse.headers.get('content-type') || meta.mime || '').toLowerCase();
  if (!['image/png', 'image/jpeg'].includes(mime)) {
    throw new Error('Visual Bridge returned unsupported MIME type.');
  }

  const responseDevice = String(imageResponse.headers.get('x-visual-device') || '');
  if (responseDevice && responseDevice !== config.deviceId) {
    throw new Error('Visual Bridge device header mismatch.');
  }

  const buffer = await imageResponse.arrayBuffer();
  if (buffer.byteLength <= 0 || buffer.byteLength > 16 * 1024 * 1024) {
    throw new Error('Visual Bridge image size is invalid.');
  }

  const digest = await crypto.subtle.digest('SHA-256', buffer);
  const sha256 = [...new Uint8Array(digest)]
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join('');
  if (meta.sha256 && sha256 !== String(meta.sha256).toLowerCase()) {
    throw new Error('Visual Bridge SHA-256 mismatch.');
  }

  return {
    ok: true,
    changed: true,
    meta,
    config,
    mime,
    dataBase64: visualBridgeBytesToBase64(new Uint8Array(buffer)),
  };
}

function handleVisualBridgeMessage(message, sendResponse) {
  if (message?.type !== 'chatcmd-visual-bridge-latest') return false;
  void visualBridgeLatest(String(message.lastImageId || ''))
    .then((result) => sendResponse(result))
    .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
  return true;
}
