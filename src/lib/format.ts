// Display helpers. All times are shown in the user's local time zone.

export function when(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  const time = d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  if (sameDay) return `Today ${time}`;
  const tomorrow = new Date(now);
  tomorrow.setDate(now.getDate() + 1);
  if (d.toDateString() === tomorrow.toDateString()) return `Tomorrow ${time}`;
  return d.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: d.getFullYear() === now.getFullYear() ? undefined : 'numeric' }) + ` ${time}`;
}

export function pct(x: number): string {
  return `${Math.round(x * 100)}%`;
}

export function minutesToTime(m: number): string {
  const h = Math.floor(m / 60) % 24;
  const mm = m % 60;
  return `${String(h).padStart(2, '0')}:${String(mm).padStart(2, '0')}`;
}

export function timeToMinutes(s: string): number {
  const [h, m] = s.split(':').map((x) => parseInt(x, 10));
  return (h || 0) * 60 + (m || 0);
}

export const DAY_NAMES = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

const ENROLMENT_LABELS: Record<string, string> = {
  queued: 'Waiting to send',
  waiting: 'Waiting for follow-up',
  sending: 'Sending',
  replied: 'Replied',
  bounced: 'Bounced',
  unsubscribed: 'Unsubscribed',
  finished: 'Finished, no reply',
  failed: 'Failed',
  skipped: 'Skipped',
};

export function enrolmentLabel(s: string): string {
  return ENROLMENT_LABELS[s] ?? s;
}

const CLASS_LABELS: Record<string, string> = {
  reply: 'Reply',
  auto_reply: 'Auto-reply',
  bounce: 'Bounce',
  unsubscribe: 'Unsubscribe',
};

export function classLabel(s: string): string {
  return CLASS_LABELS[s] ?? s;
}

export function campaignStatusLabel(s: string): string {
  return { draft: 'Draft', running: 'Running', paused: 'Paused', stopped: 'Stopped', finished: 'Finished' }[s] ?? s;
}
