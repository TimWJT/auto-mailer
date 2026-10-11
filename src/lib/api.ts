// Typed wrappers around the Tauri commands. Top-level argument names are
// camelCase here; Tauri maps them to the Rust snake_case parameters.

import { invoke } from '@tauri-apps/api/core';
import type * as T from './types';

export const api = {
  appInfo: () => invoke<T.AppInfo>('app_info'),
  getSettings: () => invoke<T.Settings>('get_settings'),
  saveSettings: (settings: T.Settings) => invoke<void>('save_settings', { settings }),

  listAccounts: () => invoke<T.Account[]>('list_accounts'),
  addGmail: (email: string, appPassword: string, displayName: string) =>
    invoke<T.Account>('add_gmail', { email, appPassword, displayName }),
  signInMicrosoft: () => invoke<T.Account>('sign_in_microsoft'),
  updateGmailPassword: (id: number, appPassword: string) =>
    invoke<T.Account>('update_gmail_password', { id, appPassword }),
  testAccount: (id: number) => invoke<T.Account>('test_account', { id }),
  updateAccount: (
    id: number,
    update: { display_name: string; signature: string; daily_limit: number; account_type: string; paused: boolean },
  ) => invoke<T.Account>('update_account', { id, update }),
  removeAccount: (id: number) => invoke<void>('remove_account', { id }),

  listContacts: (query: { search?: string; list_id?: number | null; tag?: string | null; offset?: number; limit?: number }) =>
    invoke<T.ContactPage>('list_contacts', { query }),
  getContact: (id: number) => invoke<T.Contact>('get_contact', { id }),
  saveContact: (contact: T.Contact) => invoke<T.Contact>('save_contact', { contact }),
  deleteContacts: (ids: number[]) => invoke<number>('delete_contacts', { ids }),
  fieldNames: () => invoke<string[]>('field_names'),
  listTags: () => invoke<string[]>('list_tags'),
  listLists: () => invoke<T.ContactList[]>('list_lists'),
  createList: (name: string) => invoke<number>('create_list', { name }),
  renameList: (id: number, name: string) => invoke<void>('rename_list', { id, name }),
  deleteList: (id: number) => invoke<void>('delete_list', { id }),
  addToList: (listId: number, contactIds: number[]) => invoke<void>('add_to_list', { listId, contactIds }),
  removeFromList: (listId: number, contactIds: number[]) => invoke<void>('remove_from_list', { listId, contactIds }),
  importPreview: (path: string) => invoke<T.ImportPreview>('import_preview', { path }),
  importCommit: (path: string, request: { mapping: T.Target[]; list_id: number | null; tags: string[]; to_dnc: boolean }) =>
    invoke<T.ImportSummary>('import_commit', { path, request }),
  dncList: () => invoke<T.DncEntry[]>('dnc_list'),
  dncAdd: (emails: string[]) => invoke<number>('dnc_add', { emails }),
  dncRemove: (email: string) => invoke<void>('dnc_remove', { email }),
  contactTimeline: (contactId: number) => invoke<T.TimelineItem[]>('contact_timeline', { contactId }),

  listCampaigns: () => invoke<T.CampaignSummary[]>('list_campaigns'),
  getCampaign: (id: number) => invoke<T.Campaign>('get_campaign', { id }),
  saveCampaign: (input: T.CampaignInput) => invoke<T.Campaign>('save_campaign', { input }),
  deleteCampaign: (id: number) => invoke<void>('delete_campaign', { id }),
  addAttachment: (stepId: number, path: string) => invoke<void>('add_attachment', { stepId, path }),
  removeAttachment: (id: number) => invoke<void>('remove_attachment', { id }),
  previewStep: (campaignId: number, stepNumber: number, index: number) =>
    invoke<T.Preview | null>('preview_step', { campaignId, stepNumber, index }),
  checklist: (id: number) => invoke<T.Checklist>('campaign_checklist', { id }),
  launch: (id: number) => invoke<void>('launch_campaign', { id }),
  setCampaignStatus: (id: number, status: 'running' | 'paused' | 'stopped') =>
    invoke<void>('set_campaign_status', { id, status }),
  enrolNewMembers: (id: number) => invoke<number>('enrol_new_members', { id }),
  campaignStats: (id: number) => invoke<T.Stats>('campaign_stats', { id }),
  campaignEnrolments: (id: number, status: string | null) =>
    invoke<T.EnrolmentRow[]>('campaign_enrolments', { id, status }),
  sendTest: (campaignId: number, stepNumber: number, contactId: number | null) =>
    invoke<string>('send_test', { campaignId, stepNumber, contactId }),

  queueOverview: () => invoke<T.QueueOverview>('queue_overview'),
  runQueueNow: () => invoke<T.TickReport>('run_queue_now'),
  checkRepliesNow: () => invoke<number>('check_replies_now'),
  listInbox: (filter: { classification?: string | null; campaign_id?: number | null; unread_only?: boolean }) =>
    invoke<T.InboundRow[]>('list_inbox', { filter }),
  getInbound: (id: number) => invoke<T.InboundDetail>('get_inbound', { id }),
  unreadCount: () => invoke<number>('unread_count'),

  exportCampaignCsv: (id: number, path: string) => invoke<number>('export_campaign_csv', { id, path }),
  exportContactsCsv: (path: string) => invoke<number>('export_contacts_csv', { path }),
  backup: (path: string) => invoke<void>('backup', { path }),
  restore: (path: string) => invoke<void>('restore', { path }),
};

/** Error text from a failed command (always a plain string from Rust). */
export function errorText(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return String(e);
}
