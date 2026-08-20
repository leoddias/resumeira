import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MeetingListItem } from '../ipc/meetings';
import type { ModelEntry } from '../ipc/models';
import type { KeyStatus, Settings } from '../ipc/settings';

const { listMeetings, listModels, getKeyStatus, getSettings } = vi.hoisted(() => ({
  listMeetings: vi.fn(),
  listModels: vi.fn(),
  getKeyStatus: vi.fn(),
  getSettings: vi.fn(),
}));

vi.mock('../ipc/meetings', () => ({ listMeetings }));
vi.mock('../ipc/models', () => ({ listModels }));
vi.mock('../ipc/settings', () => ({ getKeyStatus, getSettings }));

// Imported after the mocks so the component under test picks them up.
const { default: FirstRun } = await import('./FirstRun');

const settings: Settings = {
  notesFolder: null,
  transcription: { engine: 'local', provider: 'groq', localModel: 'large-v3-turbo' },
  summaryProvider: 'anthropic',
  summaryModel: null,
  audioRetention: 'keep',
  telemetryOptIn: false,
};

const meeting: MeetingListItem = {
  folder: '/a',
  title: 'Standup',
  createdAt: '2026-01-01T09:00:00',
  preview: 'Talked about X',
};

const noKeys: KeyStatus[] = [
  { account: 'groq', configured: false },
  { account: 'openai', configured: false },
  { account: 'anthropic', configured: false },
];

const configuredKey: KeyStatus[] = [
  { account: 'groq', configured: true, hint: '••••wxyz' },
  { account: 'openai', configured: false },
  { account: 'anthropic', configured: false },
];

const noModels: ModelEntry[] = [
  {
    id: 'large-v3-turbo',
    displayName: 'Large v3 Turbo',
    sizeBytes: 1_600_000_000,
    installed: false,
  },
];

beforeEach(() => {
  listMeetings.mockReset();
  listModels.mockReset();
  getKeyStatus.mockReset();
  getSettings.mockReset();
});

describe('FirstRun', () => {
  it('renders nothing while the checks are still loading', () => {
    listMeetings.mockReturnValue(new Promise(() => {}));
    listModels.mockReturnValue(new Promise(() => {}));
    getKeyStatus.mockReturnValue(new Promise(() => {}));
    getSettings.mockReturnValue(new Promise(() => {}));

    const { container } = render(<FirstRun />);
    expect(container).toBeEmptyDOMElement();
  });

  it('states plainly that summaries always call a cloud LLM', async () => {
    listMeetings.mockResolvedValue([]);
    listModels.mockResolvedValue(noModels);
    getKeyStatus.mockResolvedValue(noKeys);
    getSettings.mockResolvedValue(settings);

    render(<FirstRun />);

    await waitFor(() => expect(screen.getByRole('dialog')).toBeInTheDocument());
    expect(screen.getByText(/summaries always leave your machine/i)).toBeInTheDocument();
    expect(screen.getByText(/sends that transcript to a cloud llm/i)).toBeInTheDocument();
  });

  it('mentions the Windows microphone permission prompt', async () => {
    listMeetings.mockResolvedValue([]);
    listModels.mockResolvedValue(noModels);
    getKeyStatus.mockResolvedValue(noKeys);
    getSettings.mockResolvedValue(settings);

    render(<FirstRun />);

    await waitFor(() => expect(screen.getByRole('dialog')).toBeInTheDocument());
    expect(
      screen.getByText(/windows will ask for permission to use the microphone/i),
    ).toBeInTheDocument();
  });

  it('skip dismisses the screen', async () => {
    listMeetings.mockResolvedValue([]);
    listModels.mockResolvedValue(noModels);
    getKeyStatus.mockResolvedValue(noKeys);
    getSettings.mockResolvedValue(settings);
    const user = userEvent.setup();

    render(<FirstRun />);
    await waitFor(() => expect(screen.getByRole('dialog')).toBeInTheDocument());

    await user.click(screen.getByRole('button', { name: /skip for now/i }));

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('never renders for a returning user with meetings', async () => {
    listMeetings.mockResolvedValue([meeting]);
    listModels.mockResolvedValue(noModels);
    getKeyStatus.mockResolvedValue(noKeys);
    getSettings.mockResolvedValue(settings);

    const { container } = render(<FirstRun />);

    await waitFor(() => expect(listMeetings).toHaveBeenCalled());
    // Give the resolved promises a tick to settle into state.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(container).toBeEmptyDOMElement();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('never renders for a user who already configured an API key', async () => {
    listMeetings.mockResolvedValue([]);
    listModels.mockResolvedValue(noModels);
    getKeyStatus.mockResolvedValue(configuredKey);
    getSettings.mockResolvedValue(settings);

    render(<FirstRun />);

    await waitFor(() => expect(getKeyStatus).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
});
