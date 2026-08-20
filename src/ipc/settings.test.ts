import { describe, expect, it } from 'vitest';
import {
  accountFor,
  requiredSummaryAccount,
  requiredTranscriptionAccount,
  sendsAudioToTheCloud,
  type Settings,
} from './settings';

const local: Settings = {
  notesFolder: null,
  transcription: { engine: 'local', provider: 'groq', localModel: 'large-v3-turbo' },
  summaryEngine: 'api',
  summaryProvider: 'anthropic',
  summaryCli: 'claude',
  summaryModel: null,
  audioRetention: 'keep',
  telemetryOptIn: false,
};

describe('settings contract', () => {
  it('reports cloud transcription only when the API engine is selected', () => {
    expect(sendsAudioToTheCloud(local)).toBe(false);
    expect(
      sendsAudioToTheCloud({
        ...local,
        transcription: { ...local.transcription, engine: 'api' },
      }),
    ).toBe(true);
  });

  it('maps providers to their keychain account names', () => {
    // Rust serializes the enum as `openAi` but stores the key under `openai`.
    // Getting this wrong reports "no key" for a key the user already saved.
    expect(accountFor('openAi')).toBe('openai');
    expect(accountFor('groq')).toBe('groq');
    expect(accountFor('anthropic')).toBe('anthropic');
  });

  it('needs no transcription key while transcribing locally', () => {
    expect(requiredTranscriptionAccount(local)).toBeUndefined();
  });

  it('names the account the selected transcription provider needs', () => {
    expect(
      requiredTranscriptionAccount({
        ...local,
        transcription: { engine: 'api', provider: 'openAi', localModel: 'x' },
      }),
    ).toBe('openai');
  });

  it('always needs a summary key, whatever the transcription engine', () => {
    expect(requiredSummaryAccount(local)).toBe('anthropic');
    expect(requiredSummaryAccount({ ...local, summaryProvider: 'openAi' })).toBe('openai');
  });

  it('needs no summary key at all when an agent CLI writes the notes', () => {
    // The point of the CLI engine: a user with no API key is not stuck
    // (ADR-0020). Reporting a required account here would keep the setup
    // screen demanding a key they do not need.
    expect(requiredSummaryAccount({ ...local, summaryEngine: 'cli' })).toBeUndefined();
  });
});
