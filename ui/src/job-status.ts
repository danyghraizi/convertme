import type { JobStatus } from './types';

export function isActiveStatus(status: JobStatus): boolean {
  return status === 'queued' || status === 'running';
}

export function isRetryableStatus(status: JobStatus): boolean {
  return status === 'failed' || status === 'cancelled' || status === 'interrupted';
}
