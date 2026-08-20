/**
 * Whether the app can actually do what it is configured to do (ADR-0019).
 *
 * Rust resolves the configured route — transcription engine and summary
 * engine — and reports what is missing. The UI never re-derives that answer
 * from settings: the screen the user reads and the code that runs the
 * meeting have to agree, and the only way to guarantee that is one source.
 */

import { invoke } from '@tauri-apps/api/core';
import type { AgentCli } from './settings';

/** What the user has to do before a step can run. */
export type Blocker =
  | { kind: 'modelMissing'; model: string }
  | { kind: 'keyMissing'; account: string }
  | { kind: 'cliMissing'; cli: AgentCli; name: string };

/** One half of the pipeline: can it run, and what will it do. */
export interface StepReadiness {
  ready: boolean;
  /** What will run, in the user's words — "Local · large-v3-turbo". */
  detail: string;
  /** Whether this step sends meeting content off the machine. */
  leavesTheMachine: boolean;
  blocker?: Blocker;
}

export interface Readiness {
  transcription: StepReadiness;
  summary: StepReadiness;
  /** Both steps can run. Recording is refused in Rust unless this is true. */
  canRecord: boolean;
}

export function checkReadiness(): Promise<Readiness> {
  return invoke<Readiness>('check_readiness');
}
