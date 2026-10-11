// Mirrors of the Rust types the commands return (serde field names).

export type Provider = 'gmail' | 'microsoft';

export interface Account {
  id: number;
  provider: Provider;
  email: string;
  display_name: string;
  account_type: string;
  daily_limit: number;
  status: 'connected' | 'needs_attention';
  status_message: string | null;
  paused: boolean;
  limit_paused_until: string | null;
  next_send_at: string | null;
  signature: string;
  last_polled_at: string | null;
  connected_at: string;
  created_at: string;
  sent_24h: number;
}

export interface AppInfo {
  version: string;
  data_dir: string;
  outbox_dir: string;
  dry_run: boolean;
  dev_build: boolean;
  microsoft_configured: boolean;
  welcome_done: boolean;
  volume_warning: string;
  volume_warning_per_day: number;
  /** [id, label, provider limit, default limit] */
  account_types: [string, string, number, number][];
}

export interface SendWindow {
  /** Monday first */
  days: boolean[];
  start_min: number;
  end_min: number;
}

export interface Settings {
  dry_run: boolean;
  window: SendWindow;
  gap_min_secs: number;
  gap_max_secs: number;
  footer: string;
  poll_minutes: number;
  welcome_done: boolean;
}

export interface Contact {
  id: number;
  email: string;
  first_name: string;
  last_name: string;
  fields: Record<string, string>;
  tags: string[];
  lists: number[];
  do_not_contact: boolean;
  created_at: string;
}

export interface ContactPage {
  total: number;
  rows: Contact[];
}

export interface ContactList {
  id: number;
  name: string;
  member_count: number;
}

export type Target =
  | { kind: 'email' }
  | { kind: 'first_name' }
  | { kind: 'last_name' }
  | { kind: 'custom'; name: string }
  | { kind: 'ignore' };

export interface ImportPreview {
  headers: string[];
  sample: string[][];
  total_rows: number;
  mapping: Target[];
}

export interface ImportSummary {
  added: number;
  updated: number;
  duplicates: number;
  invalid: number;
  invalid_rows: [number, string][];
}

export interface DncEntry {
  email: string;
  reason: string;
  added_at: string;
}

export interface StepAttachment {
  id: number;
  file_name: string;
  size_bytes: number;
}

export interface Step {
  id: number;
  step_number: number;
  delay_days: number;
  subject: string;
  body: string;
  same_thread: boolean;
  attachments: StepAttachment[];
}

export type CampaignStatus = 'draft' | 'running' | 'paused' | 'stopped' | 'finished';

export interface Campaign {
  id: number;
  name: string;
  account_id: number | null;
  list_id: number | null;
  status: CampaignStatus;
  window: SendWindow;
  gap_min_secs: number;
  gap_max_secs: number;
  footer: string;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
  steps: Step[];
}

export interface StepInput {
  id: number | null;
  delay_days: number;
  subject: string;
  body: string;
  same_thread: boolean;
}

export interface CampaignInput {
  id: number | null;
  name: string;
  account_id: number | null;
  list_id: number | null;
  window: SendWindow;
  gap_min_secs: number;
  gap_max_secs: number;
  footer: string;
  steps: StepInput[];
}

export interface Stats {
  contacts: number;
  emails_sent: number;
  contacted: number;
  replied: number;
  reply_rate: number;
  bounced: number;
  unsubscribed: number;
  auto_replies: number;
  waiting: number;
  finished_no_reply: number;
  failed: number;
  skipped: number;
  dry_run_sent: number;
}

export interface CampaignSummary {
  id: number;
  name: string;
  status: CampaignStatus;
  account_email: string | null;
  list_name: string | null;
  steps: number;
  stats: Stats;
  created_at: string;
}

export interface RenderedEmail {
  subject: string;
  body: string;
  missing: string[];
}

export interface Preview {
  contact_id: number;
  contact_email: string;
  contact_name: string;
  index: number;
  total: number;
  step_number: number;
  to: string;
  from: string;
  email: RenderedEmail;
  do_not_contact: boolean;
}

export interface Checklist {
  campaign_id: number;
  account_email: string | null;
  contacts_total: number;
  will_send: number;
  skipped_dnc: number;
  skipped_invalid: number;
  skipped_missing: { contact_id: number; email: string; fields: string[] }[];
  active_elsewhere: number;
  steps: number;
  per_day: number;
  est_sending_days: number;
  est_total_days: number;
  preview: Preview | null;
  volume_warning: string | null;
  has_attachments: boolean;
  problems: string[];
}

export interface EnrolmentRow {
  id: number;
  contact_id: number;
  email: string;
  name: string;
  status: string;
  current_step: number;
  next_due_at: string | null;
  last_sent_at: string | null;
  stop_reason: string | null;
}

export interface QueueOverview {
  accounts: {
    account_id: number;
    email: string;
    due_now: number;
    sent_24h: number;
    daily_limit: number;
    next_send_at: string | null;
    blocked_reason: string | null;
  }[];
  upcoming: { campaign: string; email: string; step_number: number; due_at: string | null }[];
}

export interface InboundRow {
  id: number;
  account_email: string | null;
  from_email: string;
  from_name: string;
  subject: string;
  received_at: string;
  classification: 'reply' | 'auto_reply' | 'bounce' | 'unsubscribe';
  contact_id: number | null;
  campaign_id: number | null;
  campaign_name: string | null;
  snippet: string;
  is_read: boolean;
}

export interface InboundDetail {
  row: InboundRow;
  body_text: string;
  web_link: string | null;
}

export interface TimelineItem {
  direction: 'sent' | 'received';
  at: string;
  subject: string;
  campaign_name: string | null;
  step_number: number | null;
  status: string;
  detail: string;
}

export interface TickReport {
  sent: number;
  skipped: number;
  failed: number;
}
