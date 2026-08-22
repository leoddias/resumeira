//! Who said what (ADR-0021).
//!
//! Transcription answers *what was said* and, thanks to the two tracks
//! (ADR-0004), whether it came from this machine's microphone or from the
//! call. It cannot answer *who*: every remote participant arrives on one
//! track. This module asks the model the user already configured for
//! summaries to read the transcript and split it into speakers, naming them
//! where the conversation names them.
//!
//! Everything here is pure except the trait implementations that live in
//! `live.rs`: a prompt builder, a tolerant parser, and a function that
//! applies turns to a transcript. Nothing in this module makes a network
//! call, touches disk, or logs.
//!
//! Failure is not fatal anywhere in this path. A meeting that could not be
//! split into speakers still becomes a note — an unlabelled note is a much
//! better outcome than a lost one.

pub mod parse;
pub mod prompt;

/// A stretch of consecutive transcript lines attributed to one person.
///
/// Ranges rather than one entry per line: an hour of speech is hundreds of
/// segments, and people speak in turns anyway. `from` and `to` are inclusive
/// indices into the transcript's non-empty segments, as numbered by
/// [`prompt::build`].
///
/// `Debug` is implemented by hand and prints no label: a speaker's name is
/// somebody's, and a stray `{:?}` on a model's reply would put a meeting's
/// attendees on disk (docs/CONVENTIONS.md § Privacy). Same rule as
/// [`crate::transcribe::Segment`].
#[derive(Clone, PartialEq, Eq)]
pub struct Turn {
    pub from: usize,
    pub to: usize,
    pub speaker: String,
}

impl std::fmt::Debug for Turn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Turn")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("speaker_chars", &self.speaker.chars().count())
            .finish()
    }
}

/// Longest speaker label kept. A model that starts emitting prose instead of
/// a name must not be able to push a paragraph into every transcript line.
const MAX_SPEAKER_LEN: usize = 60;

/// Whether a label is safe to write into a note.
///
/// This is the boundary between a model's reply and a line-based file
/// format, and the reply is downstream of meeting audio, so it is treated as
/// hostile. A label carrying a newline could close the note's summary section
/// and reopen it as a transcript - the whole note would then read back
/// scrambled, with the summary rendered as speech and every line stripped of
/// its timestamp. A label carrying `": "` would mis-split on read-back and
/// shuffle the first words of the line into the name.
///
/// Neither is worth salvaging by rewriting: a name that needs repair is not a
/// name, and an unlabelled line is a correct outcome.
fn is_writable(speaker: &str) -> bool {
    !speaker.is_empty()
        && speaker.chars().count() <= MAX_SPEAKER_LEN
        && !speaker.chars().any(char::is_control)
        && !speaker.contains(": ")
}

/// Writes the turns onto the transcript's segments.
///
/// Deliberately forgiving, because the alternative to a partially labelled
/// transcript is an unlabelled one:
///
/// - a range that runs past the end is clamped, and one that starts past the
///   end is dropped;
/// - a reversed range (`to` before `from`) is read in the order given;
/// - overlaps are resolved first-writer-wins, so a later turn cannot silently
///   relabel lines an earlier one already claimed;
/// - a label that is blank, over-long, or unsafe for the note format is
///   dropped rather than written (see [`is_writable`]).
///
/// Indices address the *non-empty* segments, matching what the prompt showed
/// the model — a blank segment was never numbered, so it can never be
/// labelled.
pub fn apply(transcript: &mut crate::transcribe::Transcript, turns: &[Turn]) {
    let numbered: Vec<usize> = transcript
        .segments
        .iter()
        .enumerate()
        .filter(|(_, segment)| !segment.text.trim().is_empty())
        .map(|(index, _)| index)
        .collect();

    for turn in turns {
        let speaker = turn.speaker.trim();
        if !is_writable(speaker) {
            continue;
        }

        let (low, high) = if turn.from <= turn.to {
            (turn.from, turn.to)
        } else {
            (turn.to, turn.from)
        };
        if low >= numbered.len() {
            continue;
        }
        let high = high.min(numbered.len() - 1);

        for &index in &numbered[low..=high] {
            let segment = &mut transcript.segments[index];
            if segment.speaker.is_none() {
                segment.speaker = Some(speaker.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::{Engine, Segment, Transcript};

    fn transcript(lines: &[&str]) -> Transcript {
        Transcript {
            segments: lines
                .iter()
                .enumerate()
                .map(|(index, text)| Segment {
                    start: index as f64,
                    end: index as f64 + 1.0,
                    text: (*text).to_owned(),
                    track: None,
                    speaker: None,
                })
                .collect(),
            language: None,
            engine: Engine::Local,
        }
    }

    fn turn(from: usize, to: usize, speaker: &str) -> Turn {
        Turn {
            from,
            to,
            speaker: speaker.to_owned(),
        }
    }

    fn speakers(transcript: &Transcript) -> Vec<Option<String>> {
        transcript
            .segments
            .iter()
            .map(|segment| segment.speaker.clone())
            .collect()
    }

    #[test]
    fn turns_label_their_range_inclusively() {
        let mut t = transcript(&["a", "b", "c"]);
        apply(&mut t, &[turn(0, 1, "Ana"), turn(2, 2, "Bruno")]);
        assert_eq!(
            speakers(&t),
            vec![
                Some("Ana".to_owned()),
                Some("Ana".to_owned()),
                Some("Bruno".to_owned())
            ]
        );
    }

    #[test]
    fn a_range_past_the_end_is_clamped_not_dropped() {
        let mut t = transcript(&["a", "b"]);
        apply(&mut t, &[turn(1, 99, "Ana")]);
        assert_eq!(
            speakers(&t),
            vec![None, Some("Ana".to_owned())],
            "the part of the range that exists must still be labelled"
        );
    }

    #[test]
    fn a_range_starting_past_the_end_labels_nothing() {
        let mut t = transcript(&["a"]);
        apply(&mut t, &[turn(7, 9, "Ana")]);
        assert_eq!(speakers(&t), vec![None]);
    }

    #[test]
    fn an_overlap_cannot_relabel_lines_an_earlier_turn_claimed() {
        let mut t = transcript(&["a", "b", "c"]);
        apply(&mut t, &[turn(0, 2, "Ana"), turn(1, 2, "Bruno")]);
        assert_eq!(
            speakers(&t),
            vec![
                Some("Ana".to_owned()),
                Some("Ana".to_owned()),
                Some("Ana".to_owned())
            ]
        );
    }

    #[test]
    fn a_reversed_range_is_read_in_the_order_given() {
        let mut t = transcript(&["a", "b", "c"]);
        apply(&mut t, &[turn(2, 0, "Ana")]);
        assert_eq!(speakers(&t).iter().filter(|s| s.is_some()).count(), 3);
    }

    #[test]
    fn blank_segments_are_never_numbered_and_never_labelled() {
        let mut t = transcript(&["a", "   ", "b"]);
        // As the model saw it: line 0 is "a", line 1 is "b".
        apply(&mut t, &[turn(1, 1, "Ana")]);
        assert_eq!(
            speakers(&t),
            vec![None, None, Some("Ana".to_owned())],
            "indices address the numbered lines, skipping the blank one"
        );
    }

    #[test]
    fn a_blank_or_overlong_label_is_dropped() {
        let mut t = transcript(&["a", "b"]);
        let long = "x".repeat(MAX_SPEAKER_LEN + 1);
        apply(&mut t, &[turn(0, 0, "   "), turn(1, 1, &long)]);
        assert_eq!(speakers(&t), vec![None, None]);
    }

    #[test]
    fn a_label_that_could_break_the_note_format_is_dropped() {
        let mut t = transcript(&["a", "b", "c"]);
        apply(
            &mut t,
            &[
                // What a participant can make a model emit: 23 characters,
                // inside every length limit, and it closes the summary
                // section and reopens the file as a transcript.
                turn(0, 0, "A\n---\n\n## Transcript\n\nB"),
                turn(1, 1, "Ana\tBruno"),
                turn(2, 2, "Ana: CFO"),
            ],
        );

        assert_eq!(
            speakers(&t),
            vec![None, None, None],
            "an unlabelled line is correct; a scrambled note is not"
        );
    }

    #[test]
    fn a_turn_never_prints_the_name_it_carries() {
        let printed = format!("{:?}", turn(0, 1, "Ana"));
        assert!(!printed.contains("Ana"), "{printed}");
        assert!(printed.contains("speaker_chars: 3"), "{printed}");
    }

    #[test]
    fn a_label_is_trimmed_before_it_is_written() {
        let mut t = transcript(&["a"]);
        apply(&mut t, &[turn(0, 0, "  Ana  ")]);
        assert_eq!(speakers(&t), vec![Some("Ana".to_owned())]);
    }
}
