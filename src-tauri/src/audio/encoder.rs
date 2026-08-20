//! Opus/Ogg `TrackWriter`.
//!
//! Encodes 16 kHz mono `f32` samples with libopus and encapsulates the
//! result as an Ogg Opus stream per RFC 7845. Pages are flushed to the
//! underlying file as they complete rather than held until `finish`, so a
//! crash mid-meeting costs at most a second of audio, never the whole
//! recording.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use opus::{Application, Bitrate, Channels, Encoder as OpusEncoder};

use crate::audio::{AudioError, TrackWriter, TARGET_SAMPLE_RATE};

/// Frame length libopus is fed on every call: 20 ms at [`TARGET_SAMPLE_RATE`].
const FRAME_SAMPLES: usize = 320;

/// Target average bitrate for the VOIP encoder profile (packet T-M1-2).
const TARGET_BITRATE_BPS: i32 = 24_000;

/// Encoded Opus frames at this bitrate/duration never come close to this;
/// it's a generous upper bound for the scratch buffer passed to libopus.
const MAX_PACKET_BYTES: usize = 4000;

/// Ogg Opus granule positions are always expressed in samples at 48 kHz,
/// regardless of the encoder's actual input rate (RFC 7845 section 4).
const GRANULE_RATE_HZ: u64 = 48_000;

/// Samples-at-48kHz represented by one encoded frame.
const GRANULE_PER_FRAME: u64 = GRANULE_RATE_HZ * FRAME_SAMPLES as u64 / TARGET_SAMPLE_RATE as u64;

/// How many encoded frames accumulate in a page before it is forced to
/// disk. One page per second (`TARGET_SAMPLE_RATE / FRAME_SAMPLES` frames)
/// bounds crash loss to ~1s while keeping the Ogg framing overhead
/// (~27 bytes header + 1 lacing byte/frame) well under 5% of the payload.
const FRAMES_PER_PAGE: usize = TARGET_SAMPLE_RATE as usize / FRAME_SAMPLES;

/// Fixed logical-stream serial number. Every writer owns its own file, so
/// stream collision inside one Ogg file cannot happen.
const STREAM_SERIAL: u32 = 0x4f50_5553; // "OPUS" as bytes, arbitrary but stable.

fn map_encode_err(err: opus::Error) -> AudioError {
    AudioError::Encode(err.to_string())
}

fn map_io_err(path: &str, err: std::io::Error) -> AudioError {
    AudioError::Io {
        path: path.to_string(),
        source: err,
    }
}

/// Builds the 19-byte `OpusHead` packet (RFC 7845 section 5.1).
fn opus_head_packet(channels: u8, pre_skip: u16, input_sample_rate: u32) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(19);
    pkt.extend_from_slice(b"OpusHead");
    pkt.push(1); // version
    pkt.push(channels);
    pkt.extend_from_slice(&pre_skip.to_le_bytes());
    pkt.extend_from_slice(&input_sample_rate.to_le_bytes());
    pkt.extend_from_slice(&0i16.to_le_bytes()); // output gain
    pkt.push(0); // channel mapping family 0: mono/stereo, no mapping table
    pkt
}

/// Builds a minimal `OpusTags` packet (RFC 7845 section 5.2): a vendor
/// string and zero user comments.
fn opus_tags_packet() -> Vec<u8> {
    let vendor = b"resumeira";
    let mut pkt = Vec::with_capacity(8 + 4 + vendor.len() + 4);
    pkt.extend_from_slice(b"OpusTags");
    pkt.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    pkt.extend_from_slice(vendor);
    pkt.extend_from_slice(&0u32.to_le_bytes()); // zero user comments
    pkt
}

/// Ogg/Opus `TrackWriter`.
///
/// Generic over the underlying writer so tests can inject a writer that
/// fails on demand; production code always goes through [`Self::create`],
/// which fixes `W` to [`File`].
pub struct OpusTrackWriter<W: Write = File> {
    encoder: OpusEncoder,
    packet_writer: PacketWriter<'static, W>,
    /// Samples buffered but not yet forming a whole [`FRAME_SAMPLES`] frame.
    pending: Vec<f32>,
    /// Granule position (48 kHz samples) of the last frame written.
    granule_pos: u64,
    /// Encoded frames written since the last forced page end.
    frames_since_flush: usize,
    /// Set once a write to the underlying writer has failed; further calls
    /// short-circuit instead of retrying a writer that is presumed broken.
    poisoned: Option<String>,
    /// Path used in error messages. Not meaningful for the test writer.
    path: String,
}

impl OpusTrackWriter<File> {
    /// Creates the file at `path` and writes the `OpusHead`/`OpusTags`
    /// headers immediately, so even a writer that never receives a sample
    /// leaves behind a well-formed (empty) Ogg Opus stream.
    pub fn create(path: &Path) -> Result<Self, AudioError> {
        let path_str = path.display().to_string();
        let file = File::create(path).map_err(|e| map_io_err(&path_str, e))?;
        Self::from_writer(file, path_str)
    }
}

impl<W: Write> OpusTrackWriter<W> {
    fn from_writer(writer: W, path: String) -> Result<Self, AudioError> {
        let mut encoder = OpusEncoder::new(TARGET_SAMPLE_RATE, Channels::Mono, Application::Voip)
            .map_err(map_encode_err)?;
        encoder
            .set_bitrate(Bitrate::Bits(TARGET_BITRATE_BPS))
            .map_err(map_encode_err)?;

        // Lookahead is reported in samples at the encoder's own rate
        // (TARGET_SAMPLE_RATE); OpusHead pre-skip is always in 48 kHz
        // samples (RFC 7845 section 4), hence the scale factor.
        let lookahead = encoder.get_lookahead().map_err(map_encode_err)?;
        let pre_skip = (lookahead as i64 * GRANULE_RATE_HZ as i64 / TARGET_SAMPLE_RATE as i64)
            .clamp(0, u16::MAX as i64) as u16;

        let mut packet_writer = PacketWriter::new(writer);
        packet_writer
            .write_packet(
                opus_head_packet(1, pre_skip, TARGET_SAMPLE_RATE),
                STREAM_SERIAL,
                PacketWriteEndInfo::EndPage,
                0,
            )
            .map_err(|e| map_io_err(&path, e))?;
        packet_writer
            .write_packet(
                opus_tags_packet(),
                STREAM_SERIAL,
                PacketWriteEndInfo::EndPage,
                0,
            )
            .map_err(|e| map_io_err(&path, e))?;

        Ok(Self {
            encoder,
            packet_writer,
            pending: Vec::with_capacity(FRAME_SAMPLES * 2),
            granule_pos: 0,
            frames_since_flush: 0,
            poisoned: None,
            path,
        })
    }

    /// Encodes exactly one [`FRAME_SAMPLES`]-sample frame and writes it as
    /// an Ogg packet, forcing a page end every [`FRAMES_PER_PAGE`] frames.
    fn write_frame(&mut self, frame: &[f32]) -> Result<(), AudioError> {
        debug_assert_eq!(frame.len(), FRAME_SAMPLES);

        let encoded = self
            .encoder
            .encode_vec_float(frame, MAX_PACKET_BYTES)
            .map_err(map_encode_err)?;

        self.granule_pos += GRANULE_PER_FRAME;
        self.frames_since_flush += 1;
        let end_info = if self.frames_since_flush >= FRAMES_PER_PAGE {
            self.frames_since_flush = 0;
            PacketWriteEndInfo::EndPage
        } else {
            PacketWriteEndInfo::NormalPacket
        };

        let path = self.path.clone();
        self.packet_writer
            .write_packet(encoded, STREAM_SERIAL, end_info, self.granule_pos)
            .map_err(|e| {
                self.poisoned = Some(e.to_string());
                map_io_err(&path, e)
            })
    }

    fn check_poisoned(&self) -> Result<(), AudioError> {
        match &self.poisoned {
            Some(reason) => Err(map_io_err(
                &self.path,
                std::io::Error::other(format!(
                    "writer failed on a previous write and will not retry: {reason}"
                )),
            )),
            None => Ok(()),
        }
    }
}

impl<W: Write + Send> TrackWriter for OpusTrackWriter<W> {
    fn write(&mut self, samples: &[f32]) -> Result<(), AudioError> {
        self.check_poisoned()?;

        self.pending.extend_from_slice(samples);
        let mut offset = 0;
        while self.pending.len() - offset >= FRAME_SAMPLES {
            // Take ownership of the frame slice before mutating `self`.
            let frame: Vec<f32> = self.pending[offset..offset + FRAME_SAMPLES].to_vec();
            self.write_frame(&frame)?;
            offset += FRAME_SAMPLES;
        }
        self.pending.drain(0..offset);
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<(), AudioError> {
        self.check_poisoned()?;

        if !self.pending.is_empty() {
            let valid = self.pending.len();
            let mut frame = std::mem::take(&mut self.pending);
            frame.resize(FRAME_SAMPLES, 0.0);

            let encoded = self
                .encoder
                .encode_vec_float(&frame, MAX_PACKET_BYTES)
                .map_err(map_encode_err)?;

            // Only the real samples count toward the granule position, so
            // the decoder trims the silence padding from the last frame.
            self.granule_pos += GRANULE_RATE_HZ * valid as u64 / TARGET_SAMPLE_RATE as u64;

            self.packet_writer
                .write_packet(
                    encoded,
                    STREAM_SERIAL,
                    PacketWriteEndInfo::EndStream,
                    self.granule_pos,
                )
                .map_err(|e| map_io_err(&self.path, e))?;
        } else {
            // No leftover samples: still need to mark the stream ended.
            // libopus never emits a zero-byte frame for real audio, so an
            // empty packet here is unambiguous as a pure end-of-stream
            // marker and carries no decodable audio.
            self.packet_writer
                .write_packet(
                    Vec::new(),
                    STREAM_SERIAL,
                    PacketWriteEndInfo::EndStream,
                    self.granule_pos,
                )
                .map_err(|e| map_io_err(&self.path, e))?;
        }

        self.packet_writer
            .inner_mut()
            .flush()
            .map_err(|e| map_io_err(&self.path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;
    use std::io::{self, Cursor};

    /// A writer that behaves like a `Vec<u8>` sink until `fail_after` bytes
    /// worth of `write_all` calls have gone through, then errors forever.
    struct FlakyWriter {
        inner: Cursor<Vec<u8>>,
        writes_left: usize,
    }

    impl Write for FlakyWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.writes_left == 0 {
                return Err(io::Error::other("simulated disk failure"));
            }
            self.writes_left -= 1;
            self.inner.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    fn sine_samples(seconds: f64, freq: f64) -> Vec<f32> {
        let n = (TARGET_SAMPLE_RATE as f64 * seconds) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / TARGET_SAMPLE_RATE as f64;
                (2.0 * PI as f64 * freq * t).sin() as f32 * 0.5
            })
            .collect()
    }

    fn writer_on_temp(
        dir: &tempfile::TempDir,
        name: &str,
    ) -> (OpusTrackWriter, std::path::PathBuf) {
        let path = dir.path().join(name);
        let writer = OpusTrackWriter::create(&path).expect("create writer");
        (writer, path)
    }

    #[test]
    fn creates_a_well_formed_opus_head() {
        let dir = tempfile::tempdir().unwrap();
        let (writer, path) = writer_on_temp(&dir, "head.opus");
        Box::new(writer).finish().unwrap();

        let bytes = std::fs::read(&path).unwrap();
        // Ogg page: "OggS" + version(1) + flags(1) + granule(8) + serial(4)
        // + seq(4) + crc(4) + segment_count(1) = 27 bytes, then the lacing
        // values, then the packet payload starting with "OpusHead".
        assert_eq!(&bytes[0..4], b"OggS");
        let segment_count = bytes[26] as usize;
        let packet_start = 27 + segment_count;
        assert_eq!(&bytes[packet_start..packet_start + 8], b"OpusHead");
        assert_eq!(bytes[packet_start + 8], 1, "version");
        assert_eq!(bytes[packet_start + 9], 1, "channel count");
        let pre_skip = u16::from_le_bytes([bytes[packet_start + 10], bytes[packet_start + 11]]);
        // Pre-skip must be a plausible algorithmic-delay value, not garbage.
        assert!(pre_skip < 5000, "pre_skip = {pre_skip}");
        let rate = u32::from_le_bytes([
            bytes[packet_start + 12],
            bytes[packet_start + 13],
            bytes[packet_start + 14],
            bytes[packet_start + 15],
        ]);
        assert_eq!(rate, TARGET_SAMPLE_RATE);
    }

    #[test]
    fn empty_file_still_has_tags_and_is_readable_as_ogg() {
        let dir = tempfile::tempdir().unwrap();
        let (writer, path) = writer_on_temp(&dir, "empty.opus");
        Box::new(writer).finish().unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.windows(8).any(|w| w == b"OpusTags"));
    }

    #[test]
    fn same_total_samples_produce_identical_bytes_regardless_of_call_boundaries() {
        let samples = sine_samples(1.0, 440.0);

        let dir = tempfile::tempdir().unwrap();
        let (mut one_call, path_a) = writer_on_temp(&dir, "one_call.opus");
        one_call.write(&samples).unwrap();
        Box::new(one_call).finish().unwrap();

        let (mut many_calls, path_b) = writer_on_temp(&dir, "many_calls.opus");
        let mut offset = 0;
        // Odd, varying chunk sizes that don't line up with the 320-sample
        // frame boundary.
        let chunk_sizes = [7usize, 500, 1, 319, 321, 1000, 13];
        let mut i = 0;
        while offset < samples.len() {
            let size = chunk_sizes[i % chunk_sizes.len()].min(samples.len() - offset);
            many_calls.write(&samples[offset..offset + size]).unwrap();
            offset += size;
            i += 1;
        }
        Box::new(many_calls).finish().unwrap();

        let bytes_a = std::fs::read(&path_a).unwrap();
        let bytes_b = std::fs::read(&path_b).unwrap();
        assert_eq!(bytes_a, bytes_b);
    }

    #[test]
    fn dropping_without_finish_keeps_pages_already_flushed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dropped.opus");
        let mut writer = OpusTrackWriter::create(&path).unwrap();
        // Enough samples for several whole frames, which are flushed as
        // part of `write` itself (not deferred to `finish`).
        writer.write(&sine_samples(0.5, 440.0)).unwrap();
        drop(writer);

        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.windows(4).any(|w| w == b"OggS"));
        assert!(bytes.len() > 27, "should contain at least the header page");
    }

    #[test]
    fn sixty_seconds_lands_within_bitrate_budget() {
        let dir = tempfile::tempdir().unwrap();
        let (mut writer, path) = writer_on_temp(&dir, "budget.opus");
        writer.write(&sine_samples(60.0, 440.0)).unwrap();
        Box::new(writer).finish().unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let expected = TARGET_BITRATE_BPS as u64 / 8 * 60;
        let lower = expected / 2;
        let upper = expected + expected / 2;
        assert!(
            (bytes.len() as u64) >= lower && (bytes.len() as u64) <= upper,
            "expected {}..={} bytes, got {}",
            lower,
            upper,
            bytes.len()
        );
    }

    #[test]
    fn write_after_io_failure_returns_io_error_not_panic() {
        let flaky = FlakyWriter {
            inner: Cursor::new(Vec::new()),
            // Each header page (OpusHead, OpusTags) issues 3 `write_all`
            // calls (page header, lacing table, payload): let exactly the
            // two header pages through, then fail every write after.
            writes_left: 6,
        };
        let mut writer = OpusTrackWriter::from_writer(flaky, "flaky.opus".to_string()).unwrap();

        // A full second (FRAMES_PER_PAGE frames) forces a page flush at the
        // end of this call, which should hit the now-exhausted writer and
        // come back as AudioError::Io instead of panicking.
        let result = writer.write(&sine_samples(1.0, 440.0));
        match result {
            Err(AudioError::Io { .. }) => {}
            other => panic!("expected AudioError::Io, got {other:?}"),
        }

        // A second call must not panic and must keep reporting the error.
        let result2 = writer.write(&sine_samples(0.1, 440.0));
        match result2 {
            Err(AudioError::Io { .. }) => {}
            other => panic!("expected AudioError::Io on retry, got {other:?}"),
        }
    }

    #[test]
    fn partial_frame_is_padded_and_flushed_on_finish() {
        let dir = tempfile::tempdir().unwrap();
        let (mut writer, path) = writer_on_temp(&dir, "partial.opus");
        // Not a multiple of FRAME_SAMPLES.
        writer.write(&sine_samples(0.0151, 440.0)).unwrap();
        Box::new(writer).finish().unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.windows(4).any(|w| w == b"OggS"));
    }
}
