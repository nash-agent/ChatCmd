import { appLocale } from '../../i18n';

const copy = {
  checking: ['Checking saved compact progress…', 'Checking saved compact progress…'],
  loadError: ['Could not read compact progress. Retry before sending.', 'Could not read compact progress. Retry before sending.'],
  waiting: ['Progress is saved. You can reload this page without losing this job.', 'Progress is saved. You can reload this page without losing this job.'],
  extensionMissing: ['Progress is saved, but the extension has not acknowledged it. Enable or update ChatCMD ChatGPT Bridge, reload this page, then choose Resume via extension.', 'Progress is saved, but the extension has not acknowledged it. Enable or update ChatCMD ChatGPT Bridge, reload this page, then choose Resume via extension.'],
  waking: ['Contacting the extension…', 'Contacting the extension…'],
  acknowledged: ['Waiting for ChatGPT. The extension will continue from the saved checkpoint.', 'Waiting for ChatGPT. The extension will continue from the saved checkpoint.'],
  resume: ['Resume via extension', 'Resume via extension'],
  cancelJob: ['Cancel compact', 'Cancel compact'],
  cancelled: ['Cancelled', 'Cancelled'],
  completed: ['Completed', 'Completed'],
  completedDetail: ['The new chat is ready. This task, its history and queued messages are unchanged.', 'The new chat is ready. This task, its history and queued messages are unchanged.'],
  cancelledDetail: ['Compact was cancelled. Your draft and queued messages are preserved.', 'Compact was cancelled. Your draft and queued messages are preserved.'],
  preserved: ['Sending is paused. Your draft and queued messages are preserved.', 'Sending is paused. Your draft and queued messages are preserved.'],
  conflict: ['Progress changed before cancellation. Review the latest state and try again.', 'Progress changed before cancellation. Review the latest state and try again.'],
  empty: ['No compact history yet.', 'No compact history yet.'],
  reference: ['Open old conversation', 'Open old conversation'],
  newTab: ['(reference only, opens in a new tab)', '(reference only, opens in a new tab)'],
  oldId: ['Old conversation', 'Old conversation'],
  newId: ['New conversation', 'New conversation'],
  details: ['Conversation details', 'Conversation details'],
  noUrl: ['Old conversation link unavailable.', 'Old conversation link unavailable.'],
  confirm: ['Confirm compact', 'Confirm compact'],
  continueAfterCompact: ['Continue working after compact completes', 'Continue working after compact completes'],
  continueHint: ['Off by default. Only when checked will ChatCMD send the continuation message after attaching the new chat.', 'Off by default. Only when checked will ChatCMD send the continuation message after attaching the new chat.'],
  preparing: ['Preparing the existing conversation and its context.', 'Preparing the existing conversation and its context.'],
  writing_handoff: ['ChatGPT is writing the handoff. Keep the ChatGPT tab available.', 'ChatGPT is writing the handoff. Keep the ChatGPT tab available.'],
  saving_handoff: ['Saving the handoff before opening a new chat.', 'Saving the handoff before opening a new chat.'],
  opening_new_chat: ['Opening the new chat and reconnecting it to this same task.', 'Opening the new chat and reconnecting it to this same task.'],
  bridgeSync: ['Waiting for the new conversation link before resuming messages…', 'Waiting for the new conversation link before resuming messages…'],
} as const;

export function compactText(key: keyof typeof copy): string {
  return copy[key][appLocale().toLowerCase().startsWith('vi') ? 1 : 0];
}
