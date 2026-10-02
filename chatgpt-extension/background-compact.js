// SQLite owns semantic progress. Local storage owns browser dispatch fences.
// Neither a missing tab nor a lost HTTP response is proof that Send did not happen.
const COMPACT_PREFIX = 'chatcmd-compact-job:';
const COMPACT_ALARM = 'chatcmd-compact-recovery';
const compactFlights = new Map();
const compactJobs = new Map();
let compactTimer;
let compactRecovery;
const compactPath = (id) => `/api/local/chatgpt/compact/${encodeURIComponent(id)}`;
async function compactRecord(id) { return (await chrome.storage.local.get(`${COMPACT_PREFIX}${id}`))[`${COMPACT_PREFIX}${id}`] || null; }
async function saveCompactRecord(id, value) { await chrome.storage.local.set({ [`${COMPACT_PREFIX}${id}`]: value }); return value; }
async function compactCheckpoint(record, job, patch) {
  const next = await postJson(record.localBaseUrl, `${compactPath(job.id)}/checkpoint`, { expectedRevision: job.revision, ...patch });
  compactJobs.set(job.id, next);
  return next;
}
async function compactDetail(record, job, detail) {
  if (job.detail === detail || ChatCmdCompactProtocol.terminal(job)) return job;
  return compactCheckpoint(record, job, { detail });
}
async function compactSend(tabId, action, job, kind, documentToken) {
  return sendToChatGpt(tabId, { type: `chatcmd-compact-${action}`, job, kind, documentToken }, { quiet: true });
}
// A lost dispatch response is never retried. Only a matching document's explicit
// no-click result may reopen the fence, persisted before any future attempt.
async function compactDispatch(tabId, job, record, kind, documentToken) {
  const field = kind === 'HANDOFF' ? 'sourceSend' : 'destinationSend';
  const documentField = kind === 'HANDOFF' ? 'sourceDocument' : 'destinationDocument';
  record = await saveCompactRecord(job.id, { ...record, [field]: 'dispatched-unresolved', [documentField]: documentToken });
  const result = await compactSend(tabId, 'dispatch', job, kind, documentToken);
  if (result.sent === false && result.retryable === true && result.documentToken === documentToken) {
    await saveCompactRecord(job.id, { ...record, [field]: 'not-sent' });
    await compactDetail(record, job, 'Waiting for ChatGPT\'s Send button to become ready. Send has not been clicked; the process will continue automatically.');
  }
}
async function startCompactJob(message, sender) {
  const sourceUrl = sender.tab?.url || sender.url || '';
  if (!/^http:\/\/(?:localhost|127\.0\.0\.1)(?::\d+)?\//.test(sourceUrl)) throw new Error('Compact must be started from the local ChatCMD console.');
  const localBaseUrl = localOrigin(message.localBaseUrl);
  const job = await getJson(localBaseUrl, compactPath(message.jobId));
  if (job.taskId !== message.taskId) throw new Error('Compact job belongs to another task.');
  let record = await compactRecord(job.id);
  if (record && record.localBaseUrl !== localBaseUrl) throw new Error('Compact API origin changed.');
  if (!record) {
    record = await saveCompactRecord(job.id, { localBaseUrl, sourceTabId: sender.tab?.id,
      oldConversationId: job.oldConversationId, oldConversationUrl: job.oldConversationUrl,
      sourceSend: job.phase === 'preparing' ? 'not-attempted' : 'unknown',
      destinationSend: job.phase === 'opening_new_chat' ? 'unknown' : 'not-attempted',
      initialized: true, initialOpenAllowed: job.phase === 'preparing' });
  }
  compactJobs.set(job.id, job);
  void runCompactJob(job.id);
  return { jobId: job.id, accepted: true };
}
async function runCompactJob(id) {
  if (compactFlights.has(id)) return compactFlights.get(id);
  const flight = compactTick(id).catch(async (error) => {
    const record = await compactRecord(id);
    if (!record) return;
    try {
      const job = await getJson(record.localBaseUrl, compactPath(id));
      await compactDetail(record, job, String(error?.message || error).slice(0, 900));
    } catch { /* offline state stays on disk; no destructive fallback */ }
  }).finally(() => { compactFlights.delete(id); scheduleCompactTick(); });
  compactFlights.set(id, flight);
  return flight;
}
function scheduleCompactTick() {
  clearTimeout(compactTimer);
  if ([...compactJobs.values()].some((job) => !ChatCmdCompactProtocol.terminal(job))) {
    compactTimer = setTimeout(() => { for (const id of compactJobs.keys()) void runCompactJob(id); }, 2000);
  }
}
async function compactTick(id) {
  let record = await compactRecord(id);
  if (!record) return;
  let job = await getJson(record.localBaseUrl, compactPath(id));
  compactJobs.set(id, job);
  if (ChatCmdCompactProtocol.terminal(job)) { await finishCompactBrowser(job, record); return; }
  const tabs = await chatGptTabs();
  const sourceStatusTab = tabs.find((tab) => conversationIdFromUrl(tab.url || '') === job.oldConversationId);
  if (sourceStatusTab?.id) {
    try { await chrome.tabs.sendMessage(sourceStatusTab.id, { type: 'chatcmd-compact-status', job: { ...job, handoffText: null }, kind: 'HANDOFF' }); }
    catch { /* the source may be closed or still loading; progress is durable in SQLite */ }
  }
  if (['preparing', 'writing_handoff'].includes(job.phase)) {
    let source = tabs.find((tab) => conversationIdFromUrl(tab.url || '') === job.oldConversationId);
    if (!source && record.initialOpenAllowed) {
      // Consume before creation. An unknown result cannot open a second tab on retry.
      record = await saveCompactRecord(id, { ...record, initialOpenAllowed: false });
      source = await chrome.tabs.create({ url: job.oldConversationUrl, active: false });
    }
    if (!source?.id) {
      await compactDetail(record, job, 'Waiting for the previous ChatGPT conversation to reopen. Progress is saved; do not resend the handoff automatically.');
      return;
    }
    record = await saveCompactRecord(id, { ...record, sourceChatTabId: source.id, initialOpenAllowed: false });
    const probe = await compactSend(source.id, 'probe', job, 'HANDOFF');
    if (probe.markerFound) {
      if (probe.superseded) throw new Error('A new message appeared after the handoff request. Do not capture another turn\'s response; cancel and inspect the conversation.');
      if (probe.handoffText) {
        job = await compactCheckpoint(record, job, { phase: 'saving_handoff', handoffText: probe.handoffText, detail: null });
        return;
      }
      await compactDetail(record, job, probe.threadError
        ? 'ChatGPT has not completed the handoff. Open the tab to inspect the error; the previous content and task are preserved.'
        : 'Waiting for ChatGPT to finish the handoff for this exact turn. You may close the tab and reopen it later.');
      return;
    }
    const maySend = (job.phase === 'preparing' && record.sourceSend === 'not-attempted') || record.sourceSend === 'not-sent';
    if (!maySend) {
      await compactDetail(record, job, 'Reconciling the recorded handoff send. Do not send it twice while the result is unclear; reopen the tab or cancel to inspect.');
      return;
    }
    const ready = await compactSend(source.id, 'prepare', job, 'HANDOFF', probe.documentToken);
    if (!ready.ready) { await compactDetail(record, job, 'Preparing: stopping the current response and waiting for the ChatGPT composer to become ready.'); return; }
    // The server transition is also the cross-document source dispatch permit.
    job = await compactCheckpoint(record, job, { phase: 'writing_handoff', detail: null });
    await compactDispatch(source.id, job, record, 'HANDOFF', probe.documentToken);
    return;
  }
  if (job.phase === 'saving_handoff') {
    if (!job.handoffText) throw new Error('The handoff has not been durably saved; do not open a new chat.');
    await compactCheckpoint(record, job, { phase: 'opening_new_chat', detail: null });
    return;
  }
  if (job.phase === 'opening_new_chat') await compactDestination(job, record, tabs);
}
async function recoverCompactJobs() {
  if (compactRecovery) return compactRecovery;
  compactRecovery = (async () => {
    const stored = await chrome.storage.local.get(null);
    const records = Object.entries(stored).filter(([key]) => key.startsWith(COMPACT_PREFIX));
    const origins = new Set(records.map(([, record]) => record.localBaseUrl));
    origins.add(approvalBaseUrl);
    for (const origin of origins) {
      try {
        const base = localOrigin(origin);
        const result = await getJson(base, '/api/local/chatgpt/compact/pending');
        for (const job of result.jobs || []) {
          if (!await compactRecord(job.id)) await saveCompactRecord(job.id, { localBaseUrl: base,
            oldConversationId: job.oldConversationId, oldConversationUrl: job.oldConversationUrl,
            sourceSend: job.phase === 'preparing' ? 'not-attempted' : 'unknown',
            destinationSend: job.phase === 'opening_new_chat' ? 'unknown' : 'not-attempted',
            initialized: true, initialOpenAllowed: false });
          compactJobs.set(job.id, job);
        }
      } catch { /* startup may precede the local application's listener */ }
    }
    for (const [key, record] of records) {
      if (!record.finished) void runCompactJob(key.slice(COMPACT_PREFIX.length));
    }
    for (const job of compactJobs.values()) if (!ChatCmdCompactProtocol.terminal(job)) void runCompactJob(job.id);
    const alarm = await chrome.alarms.get(COMPACT_ALARM);
    if (!alarm) await chrome.alarms.create(COMPACT_ALARM, { periodInMinutes: 1 });
  })().finally(() => { compactRecovery = null; });
  return compactRecovery;
}
async function compactOwnsTab(tabId, conversationId) {
  for (const job of compactJobs.values()) {
    if (ChatCmdCompactProtocol.terminal(job)) continue;
    if (job.oldConversationId === conversationId || job.newConversationId === conversationId) return true;
    const record = await compactRecord(job.id);
    if (record?.destinationTabId === tabId) return true;
  }
  return false;
}
chrome.runtime.onMessage.addListener((message, sender, reply) => {
  if (message?.type === 'chatcmd-local-command' && message.action === 'compact-resume') {
    void startCompactJob(message, sender).then((value) => reply({ ok: true, ...value }))
      .catch((error) => reply({ ok: false, error: errorMessage(error) }));
    return true;
  }
  if (message?.type === 'chatcmd-compact-wake' && sender.tab?.id && isChatGptUrl(sender.tab.url)) {
    void recoverCompactJobs(); reply({ ok: true }); return false;
  }
  return false;
});
chrome.alarms.onAlarm.addListener((alarm) => { if (alarm.name === COMPACT_ALARM) void recoverCompactJobs(); });
chrome.runtime.onStartup.addListener(() => void recoverCompactJobs());
chrome.runtime.onInstalled.addListener(() => void recoverCompactJobs());
chrome.tabs.onUpdated.addListener((_id, change, tab) => {
  if ((change.status === 'complete' || change.url) && isChatGptUrl(tab?.url)) void recoverCompactJobs();
});
chrome.tabs.onReplaced.addListener((addedId, removedId) => {
  void (async () => {
    const records = await chrome.storage.local.get(null);
    for (const [key, record] of Object.entries(records)) {
      if (!key.startsWith(COMPACT_PREFIX) || record.finished) continue;
      const next = { ...record };
      for (const field of ['sourceChatTabId', 'destinationTabId', 'sourceTabId']) if (next[field] === removedId) next[field] = addedId;
      await chrome.storage.local.set({ [key]: next });
    }
    await recoverCompactJobs();
  })();
});
void recoverCompactJobs();
