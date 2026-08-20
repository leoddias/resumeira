/**
 * The meetings half of the IPC contract.
 *
 * The Rust side owns the notes folder and the search index; the UI asks for
 * lists and note contents and never touches the filesystem itself.
 */

import { invoke } from '@tauri-apps/api/core';
import type { Track } from './types';

/** A meeting as it appears in the list. */
export interface MeetingListItem {
  /** Folder path, which is also the stable id of a meeting. */
  folder: string;
  title: string;
  /** ISO-8601 local timestamp of when the recording started. */
  createdAt: string;
  /** First line or two of the summary, for the list row. */
  preview: string;
}

/** One line of the transcript. */
export interface TranscriptLine {
  /** Seconds from the start of the meeting. */
  start: number;
  text: string;
  /** Which side spoke, when the engine could tell. */
  track?: Track;
}

/** A meeting opened for reading. */
export interface MeetingNote {
  folder: string;
  title: string;
  createdAt: string;
  /** The summary section, as Markdown. */
  summary: string;
  transcript: TranscriptLine[];
  /** Which engine produced the transcript, so the note can say so. */
  engine: 'local' | 'api';
  /** Model that wrote the summary. */
  model: string;
  /** Detected language of the meeting, when known. */
  language?: string;
  /** Whether the audio files are still on disk. */
  audioKept: boolean;
}

/** All meetings, newest first. */
export function listMeetings(): Promise<MeetingListItem[]> {
  return invoke<MeetingListItem[]>('list_meetings');
}

/** Meetings matching a query, newest first. A blank query returns nothing. */
export function searchMeetings(query: string): Promise<MeetingListItem[]> {
  return invoke<MeetingListItem[]>('search_meetings', { query });
}

/** Opens one meeting for reading. */
export function readMeeting(folder: string): Promise<MeetingNote> {
  return invoke<MeetingNote>('read_meeting', { folder });
}

/** Reveals a meeting's folder in the system file manager. */
export function openMeetingFolder(folder: string): Promise<void> {
  return invoke<void>('open_meeting_folder', { folder });
}

/** Rebuilds the search index from the notes on disk. */
export function rebuildIndex(): Promise<number> {
  return invoke<number>('rebuild_index');
}
