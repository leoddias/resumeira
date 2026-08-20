/**
 * The settings half of the IPC contract.
 *
 * Note what is missing: there is no way to read an API key back. Rust stores
 * keys in the OS keychain and only ever reports whether one exists plus a
 * masked tail (ADR-0009).
 */

import { invoke } from '@tauri-apps/api/core';

export type TranscriptionEngine = 'local' | 'api';
export type WhisperApiProvider = 'groq' | 'openAi';
export type SummaryProvider = 'anthropic' | 'openAi' | 'groq';
export type AudioRetention = 'keep' | 'deleteAfterTranscription';

export interface TranscriptionSettings {
  engine: TranscriptionEngine;
  provider: WhisperApiProvider;
  /** Identifier of the local Whisper model, e.g. `large-v3-turbo`. */
  localModel: string;
}

export interface Settings {
  /** Where meetings are written. Null means the default location. */
  notesFolder: string | null;
  transcription: TranscriptionSettings;
  summaryProvider: SummaryProvider;
  /** Null uses the provider's default model. */
  summaryModel: string | null;
  audioRetention: AudioRetention;
  /** Off unless the user turned it on (ADR-0010). */
  telemetryOptIn: boolean;
}

/** What the UI may know about a stored key: that it exists, and its tail. */
export interface KeyStatus {
  /** Provider id, which is also the keychain account name. */
  account: string;
  configured: boolean;
  /** A masked tail such as `••••cdef`. Never the key. */
  hint?: string;
}

export function getSettings(): Promise<Settings> {
  return invoke<Settings>('get_settings');
}

export function saveSettings(settings: Settings): Promise<void> {
  return invoke<void>('save_settings', { settings });
}

export function getKeyStatus(): Promise<KeyStatus[]> {
  return invoke<KeyStatus[]>('key_status');
}

/** Stores a key. It travels inward once and never comes back out. */
export function setApiKey(account: string, key: string): Promise<KeyStatus> {
  return invoke<KeyStatus>('set_api_key', { account, key });
}

export function deleteApiKey(account: string): Promise<KeyStatus> {
  return invoke<KeyStatus>('delete_api_key', { account });
}

/**
 * Whether this configuration will send audio off the machine.
 *
 * A single tested predicate so no screen can imply otherwise — the routing
 * rule it mirrors is enforced in Rust (ADR-0005).
 */
export function sendsAudioToTheCloud(settings: Settings): boolean {
  return settings.transcription.engine === 'api';
}

/**
 * Keychain account name for each provider.
 *
 * These are NOT the same strings as the serialized enum values: Rust
 * serializes `OpenAi` as `openAi` but stores its key under `openai`. Looking
 * up the wrong account would silently report "no key configured" for a key
 * the user had already saved, so the mapping is explicit and tested.
 */
const ACCOUNTS: Record<WhisperApiProvider | SummaryProvider, string> = {
  groq: 'groq',
  openAi: 'openai',
  anthropic: 'anthropic',
};

/** The keychain account for a provider. */
export function accountFor(provider: WhisperApiProvider | SummaryProvider): string {
  return ACCOUNTS[provider];
}

/** The keychain account a given transcription setup needs a key for. */
export function requiredTranscriptionAccount(settings: Settings): string | undefined {
  return settings.transcription.engine === 'api'
    ? accountFor(settings.transcription.provider)
    : undefined;
}

/** The keychain account the summary provider needs a key for. */
export function requiredSummaryAccount(settings: Settings): string {
  return accountFor(settings.summaryProvider);
}
