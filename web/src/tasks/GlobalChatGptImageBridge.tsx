import { useCallback, useEffect, useRef } from 'react';
import { api, ApiError, type ChatGptImageJob } from '../api';
import { dispatchChatGptImage } from '../chatgptBridge';
import { useRealtime } from '../realtime';
import type { TimelineEvent } from '../types';

/**
 * Relays fs_write_chatgpt_image jobs to the ChatGPT extension. The server
 * claim makes dispatch exclusive when several ChatCMD windows are open.
 */
export function GlobalChatGptImageBridge() {
  const inFlight = useRef(new Set<string>());

  const dispatchJob = useCallback(async (job: ChatGptImageJob) => {
    if (!job.jobId.trim() || !job.prompt.trim() || inFlight.current.has(job.jobId)) return;
    inFlight.current.add(job.jobId);
    try {
      let claimed: ChatGptImageJob;
      try {
        claimed = await api.claimChatGptImage(job.jobId);
      } catch (error) {
        // 409/404: another window owns it or it already finished.
        if (error instanceof ApiError && (error.status === 409 || error.status === 404)) return;
        throw error;
      }
      try {
        await dispatchChatGptImage({ jobId: claimed.jobId, prompt: claimed.prompt, model: claimed.model });
      } catch (error) {
        await api.reportChatGptImageFailure(claimed.jobId, error instanceof Error ? error.message : String(error)).catch(() => undefined);
      }
    } catch {
      // A later realtime reconnect retries through the pending endpoint.
    } finally {
      inFlight.current.delete(job.jobId);
    }
  }, []);

  const recoverPending = useCallback(async () => {
    try {
      const pending = await api.pendingChatGptImages();
      await Promise.all(pending.map(dispatchJob));
    } catch {
      // Recovery is retried on the next reconnect.
    }
  }, [dispatchJob]);

  const realtimeState = useRealtime(useCallback((event: TimelineEvent) => {
    if (event.type !== 'image.generation_requested') return;
    const job = imageJobFromPayload(event.payload);
    if (job) void dispatchJob(job);
  }, [dispatchJob]));

  useEffect(() => {
    if (realtimeState === 'online') void recoverPending();
  }, [realtimeState, recoverPending]);

  return null;
}

export function imageJobFromPayload(value: unknown): ChatGptImageJob | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const payload = value as Record<string, unknown>;
  const jobId = typeof payload.jobId === 'string' ? payload.jobId.trim() : '';
  const prompt = typeof payload.prompt === 'string' ? payload.prompt : '';
  if (!jobId || !prompt.trim()) return null;
  return {
    jobId,
    prompt,
    model: typeof payload.model === 'string' && payload.model.trim() ? payload.model.trim() : 'Auto',
    createdAtMs: Number(payload.createdAtMs) || 0,
    deadlineAtMs: Number(payload.deadlineAtMs) || 0,
  };
}
