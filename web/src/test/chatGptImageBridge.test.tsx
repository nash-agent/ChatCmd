import { act, render, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import type { TimelineEvent } from '../types';
import { GlobalChatGptImageBridge, imageJobFromPayload } from '../tasks/GlobalChatGptImageBridge';
import { api, ApiError } from '../api';
import { dispatchChatGptImage } from '../chatgptBridge';

const events = vi.hoisted(() => ({ listener: null as ((event: TimelineEvent) => void) | null }));
vi.mock('../realtime', () => ({ useRealtime: (callback: (event: TimelineEvent) => void) => { events.listener = callback; return 'online'; } }));
vi.mock('../api', () => {
  class ApiError extends Error { constructor(message: string, public status?: number) { super(message); } }
  return {
    ApiError,
    api: {
      pendingChatGptImages: vi.fn(async () => []),
      claimChatGptImage: vi.fn(),
      reportChatGptImageFailure: vi.fn(async () => ({ accepted: true })),
    },
  };
});
vi.mock('../chatgptBridge', () => ({ dispatchChatGptImage: vi.fn(async () => undefined) }));

const job = { jobId: 'img-1', prompt: 'Create an image of a potion', model: 'Auto', createdAtMs: 1, deadlineAtMs: 2 };
const requested = (payload: Record<string, unknown>): TimelineEvent => ({ id: 'e', type: 'image.generation_requested', occurredAt: '2026-09-30T00:00:00Z', payload });

beforeEach(() => { vi.mocked(api.claimChatGptImage).mockReset(); vi.mocked(dispatchChatGptImage).mockClear(); vi.mocked(api.reportChatGptImageFailure).mockClear(); });

it('claims a requested image job before handing it to the extension', async () => {
  vi.mocked(api.claimChatGptImage).mockResolvedValue(job);
  render(<GlobalChatGptImageBridge />);
  act(() => events.listener?.(requested(job)));
  await waitFor(() => expect(dispatchChatGptImage).toHaveBeenCalledWith({ jobId: 'img-1', prompt: job.prompt, model: 'Auto' }));
  expect(api.claimChatGptImage).toHaveBeenCalledWith('img-1');
});

it('does not dispatch when another window already claimed the job', async () => {
  vi.mocked(api.claimChatGptImage).mockRejectedValue(new ApiError('claimed', 409));
  render(<GlobalChatGptImageBridge />);
  act(() => events.listener?.(requested(job)));
  await waitFor(() => expect(api.claimChatGptImage).toHaveBeenCalled());
  expect(dispatchChatGptImage).not.toHaveBeenCalled();
  expect(api.reportChatGptImageFailure).not.toHaveBeenCalled();
});

it('reports a failure when the extension is unavailable', async () => {
  vi.mocked(api.claimChatGptImage).mockResolvedValue(job);
  vi.mocked(dispatchChatGptImage).mockRejectedValueOnce(new Error('ChatCMD ChatGPT Bridge did not respond in time.'));
  render(<GlobalChatGptImageBridge />);
  act(() => events.listener?.(requested(job)));
  await waitFor(() => expect(api.reportChatGptImageFailure).toHaveBeenCalledWith('img-1', 'ChatCMD ChatGPT Bridge did not respond in time.'));
});

it('ignores malformed payloads', () => {
  expect(imageJobFromPayload({ jobId: '', prompt: 'x' })).toBeNull();
  expect(imageJobFromPayload({ jobId: 'img-2', prompt: '  ' })).toBeNull();
  expect(imageJobFromPayload({ jobId: 'img-2', prompt: 'p' })?.model).toBe('Auto');
});
