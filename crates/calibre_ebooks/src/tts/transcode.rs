//! Port of `calibre_extensions.ffmpeg`'s WAV -> M4A/AAC transcoding
//! (`transcode_single_audio_stream`, issue #648), the last real gap in
//! the #167 `tts.py` epic: `embed_tts` writes synthesized speech as a
//! WAV byte stream, then transcodes it to M4A/AAC for the final EPUB
//! media-overlay audio asset.
//!
//! # Library choice, decided by the user after a real feasibility spike
//!
//! Real upstream links calibre's own bundled FFmpeg build -- not an
//! option here (see this crate's own `tts` module docs on why this
//! whole epic avoids linking C media libraries). The user chose a
//! pure-Rust encoder over shelling out to a system `ffmpeg` binary or
//! narrowing the output format. Before committing to one, a feasibility
//! spike (matching this crate's own established practice -- see `tts`'s
//! own module docs for the #77 epic's spike) built a scratch project
//! against the two real candidates and ran their own test suites
//! directly, rather than trusting crate descriptions:
//!
//! - [`oxideav-aac`](https://crates.io/crates/oxideav-aac) (real AAC-LC
//!   encoder). Its own `lib.rs` doc comment claims encode/decode
//!   "are not wired up yet" -- **stale**, confirmed by actually running
//!   its `encoder_roundtrip` test suite (24 real signal-based tests:
//!   M/S stereo, TNS, PNS, intensity stereo, transients, every
//!   supported sample rate) -- all pass. Trusting a doc comment over
//!   running the code would have wrongly ruled this crate out.
//! - [`oxideav-mp4`](https://crates.io/crates/oxideav-mp4) (real MP4/
//!   ISO-BMFF muxer), needed alongside it since upstream's own `.m4a`
//!   output is an AAC stream muxed into an MP4 container, not a bare
//!   ADTS stream. Confirmed its own test suite includes a real
//!   `mp4a`/`esds`/AAC-OTI mux-then-demux round trip.
//!
//! Both are genuinely pure Rust (`cargo build` pulls no `links = `
//! dependency, confirmed by inspecting `Cargo.lock`) and part of the
//! same actively-maintained `oxideav` codec-framework family, which is
//! why they compose cleanly (`oxideav-mp4`'s `mp4a` sample entry writer
//! dispatches on `CodecId::new("aac")` and expects exactly the
//! `AudioSpecificConfig` bytes `oxideav-aac`'s own `asc_writer` module
//! produces).
//!
//! # Disclosed narrowing: mono/stereo-class layouts only
//!
//! [`EncoderConfig::channels`](oxideav_aac::encoder::EncoderConfig) only
//! has a defined element layout (ISO/IEC 14496-3 Table 1.19) up to 6
//! channels without an explicit PCE; 7.1 (8 channels) needs a PCE this
//! port doesn't build. Irrelevant to the one real caller in this
//! codebase -- every TTS audio stream `crate::tts::batch`/`stream`
//! produce is mono -- so [`wav_to_m4a`] rejects anything above 6
//! channels rather than silently miscoding it.
//!
//! # Disclosed narrowing: no gapless edit list
//!
//! AAC's encoder has an inherent one-frame (1024-sample) priming delay
//! (see [`oxideav_aac::encoder::StreamEncoder`]'s own docs: "decoded
//! frame `f ≥ 1` reconstructs input hop `f − 1`"). A byte-exact
//! transcoder trims this with an MP4 edit list (`elst`); this port
//! doesn't build one, so the produced file's own declared/decoded
//! duration is about one frame (≈ 40-50 ms at typical TTS sample rates)
//! longer than the source PCM. This does not affect SMIL sync accuracy
//! -- `embed_tts`'s own real `clipBegin`/`clipEnd` timestamps are
//! computed from the raw PCM duration (via
//! [`super::batch::text_to_raw_audio_data`]) *before* transcoding, not
//! from the M4A file's own duration.

use std::io::Cursor;

use anyhow::{bail, Context, Result};
use oxideav_aac::adts::AdtsHeader;
use oxideav_aac::asc_writer::aac_lc_asc;
use oxideav_aac::encoder::{EncoderConfig, StreamEncoder};
use oxideav_core::{CodecId, CodecParameters, Packet, StreamInfo, TimeBase};

/// The fixed AAC analysis hop `oxideav_aac`'s encoder uses -- every
/// frame (including the final flush) covers exactly this many samples
/// per channel.
const FRAME_LEN: i64 = 1024;

/// Port of `transcode_single_audio_stream(wav, m4a)`: reads a WAV byte
/// buffer and returns a real M4A (AAC-LC in an MP4 container) byte
/// buffer.
///
/// `bitrate` is the target AAC bitrate in bits/second (real upstream's
/// FFmpeg call has no equivalent parameter -- it presumably picks its
/// own default; this port makes it explicit rather than guessing at
/// what that default was). WAV input must be 16-bit PCM, 1-6 channels
/// (see the module docs on both narrowings).
pub fn wav_to_m4a(wav: &[u8], bitrate: u32) -> Result<Vec<u8>> {
    let mut reader = hound::WavReader::new(Cursor::new(wav)).context("parsing WAV header")?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        bail!(
            "WAV to M4A transcoding only supports 16-bit PCM (got {:?}, {} bits per sample)",
            spec.sample_format,
            spec.bits_per_sample
        );
    }
    if !(1..=6).contains(&spec.channels) {
        bail!(
            "WAV to M4A transcoding supports 1-6 channels (got {}); see this module's docs on the 7.1/PCE narrowing",
            spec.channels
        );
    }
    let channels = spec.channels as u8;
    let sample_rate = spec.sample_rate;

    let samples: Vec<i16> = reader
        .samples::<i16>()
        .collect::<std::result::Result<_, _>>()
        .context("reading WAV PCM samples")?;

    let mut encoder = StreamEncoder::new(EncoderConfig { sample_rate, channels, bitrate })
        .context("building the AAC encoder for this WAV's format")?;

    let mut frames: Vec<Vec<u8>> = Vec::new();
    let hop = FRAME_LEN as usize * channels as usize;
    for chunk in samples.chunks(hop) {
        frames.push(encoder.encode_frame(chunk).context("encoding a real AAC frame")?);
    }
    if frames.is_empty() {
        // Real upstream's own encoder always emits at least one content
        // frame even for empty input (see `StreamEncoder::encode_all`'s
        // own doc); matched here by feeding one all-zero hop.
        frames.push(encoder.encode_frame(&[]).context("encoding the empty-input placeholder frame")?);
    }
    frames.push(encoder.finish().context("flushing the AAC encoder's final frame")?);

    // Table 1.19 channel_configuration: identical to the channel count
    // for every layout this function accepts (1-6; see the module
    // docs). `EncoderConfig` computes the same mapping internally but
    // does not expose it, so this mirrors it rather than duplicating a
    // private accessor.
    let channel_configuration = channels;
    let extradata = aac_lc_asc(sample_rate, channel_configuration);

    let mut params = CodecParameters::audio(CodecId::new("aac"));
    params.sample_rate = Some(sample_rate);
    params.channels = Some(channels as u16);
    params.extradata = extradata;

    let stream = StreamInfo {
        index: 0,
        time_base: TimeBase::new(1, sample_rate as i64),
        duration: None,
        start_time: Some(0),
        params,
    };

    // `oxideav_mp4::muxer::Muxer` takes ownership of its
    // `Box<dyn WriteSeek>` with no accessor to reclaim it (needed here
    // since it must seek back to patch box sizes as it writes) -- an
    // owned in-memory `Cursor<Vec<u8>>` can't be handed back out once
    // boxed as a trait object, so this writes through a real temp file
    // instead and reads the finished bytes back.
    let mut tmp = tempfile::NamedTempFile::new().context("creating a temp file for the M4A output")?;
    {
        let file = tmp.reopen().context("reopening the temp file for the M4A muxer")?;
        let mut muxer = oxideav_mp4::muxer::open(Box::new(file), std::slice::from_ref(&stream))
            .context("opening the M4A muxer")?;
        muxer.write_header().context("writing the M4A file header")?;

        for (i, adts_frame) in frames.iter().enumerate() {
            let (header, payload_start) = AdtsHeader::parse(adts_frame).context("parsing this encoder's own ADTS frame header")?;
            // MP4 stores raw `raw_data_block()` payloads, not ADTS
            // frames -- the ADTS header (and its redundant frame-length
            // field, which `esds`/`stsz` already convey) is stripped.
            let payload = &adts_frame[payload_start..header.aac_frame_length as usize];
            let mut pkt = Packet::new(0, stream.time_base, payload.to_vec());
            pkt.pts = Some(i as i64 * FRAME_LEN);
            pkt.duration = Some(FRAME_LEN);
            pkt.flags.keyframe = true;
            muxer.write_packet(&pkt).context("writing an AAC packet to the M4A muxer")?;
        }

        muxer.write_trailer().context("writing the M4A file trailer")?;
    }

    std::fs::read(tmp.path()).context("reading the finished M4A file back")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_wav(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut writer = hound::WavWriter::new(cursor, spec).unwrap();
            for &s in samples {
                writer.write_sample(s).unwrap();
            }
            writer.finalize().unwrap();
        }
        buf
    }

    fn sine_wave(seconds: f64, sample_rate: u32) -> Vec<i16> {
        let n = (seconds * sample_rate as f64) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / sample_rate as f64;
                (8000.0 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as i16
            })
            .collect()
    }

    #[test]
    fn transcodes_a_real_mono_wav_to_a_valid_m4a() {
        let pcm = sine_wave(0.5, 22050);
        let wav = make_wav(&pcm, 22050, 1);
        let m4a = wav_to_m4a(&wav, 64_000).unwrap();

        assert!(m4a.len() > 100, "expected a real, non-trivial M4A file");
        // A well-formed ISO-BMFF file starts with a size field then an
        // `ftyp` box fourcc at bytes 4..8.
        assert_eq!(&m4a[4..8], b"ftyp");
        assert!(m4a.windows(4).any(|w| w == b"moov"), "must have a moov box");
        assert!(m4a.windows(4).any(|w| w == b"mdat"), "must have an mdat box");
    }

    #[test]
    fn the_produced_m4a_round_trips_through_a_real_demuxer() {
        let pcm = sine_wave(0.3, 22050);
        let wav = make_wav(&pcm, 22050, 1);
        let m4a = wav_to_m4a(&wav, 64_000).unwrap();

        let rs: Box<dyn oxideav_core::ReadSeek> = Box::new(Cursor::new(m4a));
        let mut dmx = oxideav_mp4::demux::open(rs, &oxideav_core::NullCodecResolver).unwrap();
        assert_eq!(dmx.streams().len(), 1);
        let params = &dmx.streams()[0].params;
        assert_eq!(params.codec_id.as_str(), "aac");
        assert_eq!(params.sample_rate, Some(22050));
        assert_eq!(params.channels, Some(1));
        assert!(!params.extradata.is_empty(), "AudioSpecificConfig must survive the mux");

        let mut packet_count = 0;
        loop {
            match dmx.next_packet() {
                Ok(_) => packet_count += 1,
                Err(oxideav_core::Error::Eof) => break,
                Err(e) => panic!("demux error: {e}"),
            }
        }
        // ~0.3s at 22050Hz / 1024-sample frames, plus the encoder's own
        // final flush frame.
        assert!(packet_count >= 6, "expected several real AAC frames, got {packet_count}");
    }

    #[test]
    fn a_stereo_wav_produces_a_stereo_m4a() {
        let mono = sine_wave(0.2, 22050);
        let stereo: Vec<i16> = mono.iter().flat_map(|&s| [s, s]).collect();
        let wav = make_wav(&stereo, 22050, 2);
        let m4a = wav_to_m4a(&wav, 96_000).unwrap();

        let rs: Box<dyn oxideav_core::ReadSeek> = Box::new(Cursor::new(m4a));
        let dmx = oxideav_mp4::demux::open(rs, &oxideav_core::NullCodecResolver).unwrap();
        assert_eq!(dmx.streams()[0].params.channels, Some(2));
    }

    #[test]
    fn rejects_non_16_bit_wav() {
        let spec = hound::WavSpec { channels: 1, sample_rate: 22050, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut writer = hound::WavWriter::new(cursor, spec).unwrap();
            writer.write_sample(0.5f32).unwrap();
            writer.finalize().unwrap();
        }
        let err = wav_to_m4a(&buf, 64_000).unwrap_err();
        assert!(err.to_string().contains("16-bit"), "{err}");
    }

    #[test]
    fn rejects_more_than_six_channels() {
        let pcm = vec![0i16; 8 * 100];
        let wav = make_wav(&pcm, 22050, 8);
        let err = wav_to_m4a(&wav, 64_000).unwrap_err();
        assert!(err.to_string().contains("1-6 channels"), "{err}");
    }

    #[test]
    fn a_real_synthesized_utterance_transcodes_end_to_end() {
        // The actual real-world chain: crate::tts::batch synthesizes
        // speech, this module WAV-wraps and transcodes it. Gated on the
        // same CALIBRE_OXIDE_TEST_PIPER_VOICE convention as #611/#612/#647.
        let Ok(onnx) = std::env::var("CALIBRE_OXIDE_TEST_PIPER_VOICE") else {
            eprintln!("skipping: set CALIBRE_OXIDE_TEST_PIPER_VOICE to a real .onnx voice to run this test");
            return;
        };
        let onnx = std::path::PathBuf::from(onnx);
        let json = onnx.with_extension("onnx.json");
        if !onnx.exists() || !json.exists() {
            eprintln!("skipping: CALIBRE_OXIDE_TEST_PIPER_VOICE paths don't exist");
            return;
        }

        let batch = super::super::batch::text_to_raw_audio_data(
            &json,
            &onnx,
            ["This sentence becomes a real M4A file."],
            0.0,
            0.0,
            std::time::Duration::from_secs(30),
        )
        .unwrap();
        let utterance = &batch.utterances[0];
        assert!(!utterance.audio.is_empty());

        let wav_samples: Vec<i16> = utterance
            .audio
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let wav = make_wav(&wav_samples, batch.sample_rate, 1);

        let m4a = wav_to_m4a(&wav, 64_000).unwrap();
        assert_eq!(&m4a[4..8], b"ftyp");

        let rs: Box<dyn oxideav_core::ReadSeek> = Box::new(Cursor::new(m4a));
        let mut dmx = oxideav_mp4::demux::open(rs, &oxideav_core::NullCodecResolver).unwrap();
        assert_eq!(dmx.streams()[0].params.sample_rate, Some(batch.sample_rate));
        let mut count = 0;
        while dmx.next_packet().is_ok() {
            count += 1;
        }
        assert!(count > 0, "the real synthesized speech must produce real AAC packets");
    }

    #[test]
    fn empty_audio_still_produces_a_valid_file() {
        let wav = make_wav(&[], 22050, 1);
        let m4a = wav_to_m4a(&wav, 64_000).unwrap();
        assert_eq!(&m4a[4..8], b"ftyp");
    }
}
