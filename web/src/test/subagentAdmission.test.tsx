import { act, cleanup, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { api } from '../api';
import type { SubagentFallbackRequest } from '../api';
import { dispatchSubagentFallback } from '../chatgptBridge';
import { ChatGptBridgeTimeoutError } from '../chatgpt/bridgeErrors';
import { GlobalSubagentFallbackBridge } from '../tasks/GlobalSubagentFallbackBridge';
import type { TimelineEvent } from '../types';

const events = vi.hoisted(() => ({ listener: null as ((event: TimelineEvent) => void) | null }));
vi.mock('../realtime', () => ({ useRealtime: (listener: (event: TimelineEvent) => void) => {
  events.listener = listener; return 'online';
} }));
vi.mock('../api', () => ({ api: {
  pendingSubagentFallbacks: vi.fn(async () => []), reportSubagentFallbackResult: vi.fn(async () => ({})),
  settings: vi.fn(async () => ({ subagentEffort: 'high' })),
} }));
vi.mock('../chatgptBridge', () => ({ dispatchSubagentFallback: vi.fn(), closeSubagentFallbackTab: vi.fn() }));
beforeEach(() => { vi.clearAllMocks(); vi.mocked(api.pendingSubagentFallbacks).mockResolvedValue([]); });
afterEach(() => { cleanup(); vi.useRealTimers(); vi.restoreAllMocks(); });
const event = (id = 'child'): TimelineEvent => ({ id: `event-${id}`, type: 'subagent.fallback_requested',
  occurredAt: '2026-09-13T00:00:00Z', payload: { subagentId: id, childTaskId: `task-${id}`,
    submittedContent: 'Read one file', attempt: 1 } });

it('lost admission ACK must not fail or advance an already dispatched child', async () => {
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  vi.mocked(dispatchSubagentFallback).mockRejectedValueOnce(new ChatGptBridgeTimeoutError('no ACK'));
  render(<GlobalSubagentFallbackBridge />);
  await act(async () => { events.listener?.(event()); });
  await waitFor(() => expect(dispatchSubagentFallback).toHaveBeenCalledTimes(1));
  expect(api.reportSubagentFallbackResult).not.toHaveBeenCalled();
});

it('an explicit dispatch rejection still reports the failed attempt once', async () => {
  vi.mocked(dispatchSubagentFallback).mockRejectedValueOnce(new Error('invalid dispatch'));
  render(<GlobalSubagentFallbackBridge />);
  await act(async () => { events.listener?.(event()); });
  await waitFor(() => expect(api.reportSubagentFallbackResult).toHaveBeenCalledWith('child',
    expect.objectContaining({ attempt: 1, status: 'failed', errorMessage: 'invalid dispatch' })));
});

it('concurrent children have independent admission state and duplicate events are coalesced', async () => {
  let resolve!: () => void;
  const pending = new Promise<void>((done) => { resolve = done; });
  vi.mocked(dispatchSubagentFallback).mockReturnValue(pending);
  render(<GlobalSubagentFallbackBridge />);
  await act(async () => { events.listener?.(event('one')); events.listener?.(event('one')); events.listener?.(event('two')); });
  expect(dispatchSubagentFallback).toHaveBeenCalledTimes(2);
  await act(async () => { resolve(); await pending; });
  expect(api.reportSubagentFallbackResult).not.toHaveBeenCalled();
});

it('reconnect never creates fresh chats from stale pending reservations', async () => {
  vi.mocked(api.pendingSubagentFallbacks).mockResolvedValue([
    { subagentId: 'stale', childTaskId: 'task-stale', submittedContent: 'work', attempt: 1 },
    { subagentId: 'known', childTaskId: 'task-known', submittedContent: 'work', attempt: 2,
      conversationUrl: 'https://chatgpt.com/c/existing' },
  ] as SubagentFallbackRequest[]);
  vi.mocked(dispatchSubagentFallback).mockResolvedValue();
  render(<GlobalSubagentFallbackBridge />);
  await waitFor(() => expect(dispatchSubagentFallback).toHaveBeenCalledTimes(1));
  expect(dispatchSubagentFallback).toHaveBeenCalledWith(expect.objectContaining({
    subagentId: 'known', conversationUrl: 'https://chatgpt.com/c/existing', effort: 'high',
  }));
  expect(api.reportSubagentFallbackResult).not.toHaveBeenCalled();
});

it('recovers a missed creation event only when the server authorizes the current reservation', async () => {
  vi.mocked(api.pendingSubagentFallbacks).mockResolvedValue([
    { subagentId: 'fresh', childTaskId: 'task-fresh', submittedContent: 'work', attempt: 1,
      canStartNewConversation: true },
    { subagentId: 'stale', childTaskId: 'task-stale', submittedContent: 'work', attempt: 1,
      canStartNewConversation: false },
  ] as SubagentFallbackRequest[]);
  vi.mocked(dispatchSubagentFallback).mockResolvedValue();
  render(<GlobalSubagentFallbackBridge />);
  await waitFor(() => expect(dispatchSubagentFallback).toHaveBeenCalledTimes(1));
  expect(dispatchSubagentFallback).toHaveBeenCalledWith(expect.objectContaining({ subagentId: 'fresh', attempt: 1 }));
});

it('recovers creation after extension Reload without needing a realtime reconnect', async () => {
  vi.useFakeTimers();
  vi.mocked(dispatchSubagentFallback).mockResolvedValue();
  render(<GlobalSubagentFallbackBridge />);
  await act(async () => { await Promise.resolve(); });
  expect(dispatchSubagentFallback).not.toHaveBeenCalled();
  vi.mocked(api.pendingSubagentFallbacks).mockResolvedValue([
    { subagentId: 'fresh', childTaskId: 'task-fresh', submittedContent: 'work', attempt: 1,
      canStartNewConversation: true },
  ] as SubagentFallbackRequest[]);
  await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
  expect(dispatchSubagentFallback).toHaveBeenCalledTimes(1);
  cleanup();
  await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
  expect(dispatchSubagentFallback).toHaveBeenCalledTimes(1);
});
