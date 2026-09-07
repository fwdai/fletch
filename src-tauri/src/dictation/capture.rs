//! The local engine's half of the mic tap: where the audio goes when there is
//! no recognizer streaming results back.
//!
//! whisper.cpp wants one complete clip at 16 kHz mono, so a session accumulates
//! [`Pcm`] while the user speaks and [`transcribe`] runs the model once on stop.
//! The two conversions are deliberately split by thread:
//!
//! - **Channel averaging happens in the tap**, on Apple's real-time render
//!   thread, because it is a handful of adds per frame into a pre-sized buffer.
//! - **Resampling happens at stop**, off both the render thread and the main
//!   thread, because it is a framework call (`AVAudioConverter`) with an
//!   allocation behind it. Decimating 48 kHz by taking every third sample would
//!   be cheap enough for the tap, but folds everything above 8 kHz back into the
//!   speech band, and the model hears the aliases.
//!
//! macOS-only, like `whisper::engine` — iOS keeps the platform recognizer.

use std::cell::Cell;
use std::ptr::NonNull;
use std::sync::Arc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::AllocAnyThread;
use objc2_avf_audio::{
    AVAudioBuffer, AVAudioCommonFormat, AVAudioConverter, AVAudioConverterInputStatus,
    AVAudioConverterOutputStatus, AVAudioFormat, AVAudioPCMBuffer, AVAudioTime,
};
use parking_lot::Mutex;

use super::apple::Tap;
use super::whisper::engine;
use crate::error::{Error, Result};

/// How much audio one session may capture. A dictation is a sentence or two;
/// this is the bound that keeps a mic left open by a forgotten window from
/// growing the buffer — and the transcription that follows — without limit.
/// Audio past it is dropped, which truncates the transcript rather than failing.
const MAX_CAPTURE_SECS: f64 = 300.0;

/// A session's captured audio: mono f32 at the microphone's own sample rate.
pub(super) struct Pcm {
    /// The microphone's rate, which [`transcribe`] resamples from.
    rate: f64,
    /// Sample ceiling derived from [`MAX_CAPTURE_SECS`] and `rate`.
    limit: usize,
    samples: Mutex<Vec<f32>>,
}

/// Build the accumulator and the tap block that fills it.
pub(super) fn sink(format: &AVAudioFormat) -> Result<(Arc<Pcm>, Tap)> {
    // The input node hands out deinterleaved float32 on every OS that has
    // shipped. Reading a buffer in any other layout would be reading the wrong
    // memory, so refuse the session instead of guessing at it.
    if unsafe { format.commonFormat() } != AVAudioCommonFormat::PCMFormatFloat32
        || unsafe { format.isInterleaved() }
    {
        return Err(Error::Other(
            "This microphone's audio format isn't supported by local dictation. Switch the \
             dictation engine back to the system recognizer in Settings."
                .into(),
        ));
    }

    let rate = unsafe { format.sampleRate() };
    let limit = (rate * MAX_CAPTURE_SECS) as usize;
    let pcm = Arc::new(Pcm {
        rate,
        limit,
        // Sized to the cap up front: `append_mono` refuses to grow past `limit`,
        // so the render thread only ever writes into reserved capacity and a
        // reallocation — a copy of minutes of audio on the real-time thread —
        // can't happen. Reserving is cheap: the OS commits pages as they're
        // written, so an idle reservation of this size costs address space, not
        // memory.
        samples: Mutex::new(Vec::with_capacity(limit)),
    });

    let tap_pcm = pcm.clone();
    let tap = RcBlock::new(
        move |buffer: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
            append_mono(&tap_pcm, unsafe { buffer.as_ref() });
        },
    );
    Ok((pcm, tap))
}

/// Real-time audio thread: average the channels down and return. The only other
/// lock holder is the one-shot take in [`transcribe`], so `try_lock` all but
/// never fails — and dropping one buffer beats blocking the render thread if it
/// ever does.
fn append_mono(pcm: &Pcm, buffer: &AVAudioPCMBuffer) {
    let frames = unsafe { buffer.frameLength() } as usize;
    let channels = unsafe { buffer.format().channelCount() } as usize;
    let data = unsafe { buffer.floatChannelData() };
    if data.is_null() || frames == 0 || channels == 0 {
        return;
    }
    let Some(mut samples) = pcm.samples.try_lock() else {
        return;
    };
    if samples.len() + frames > pcm.limit {
        return;
    }
    for frame in 0..frames {
        let mut sum = 0.0;
        for channel in 0..channels {
            // Deinterleaved (checked in `sink`): one pointer per channel, each
            // to `frameLength` contiguous samples.
            sum += unsafe { *(*data.add(channel)).as_ptr().add(frame) };
        }
        samples.push(sum / channels as f32);
    }
}

/// Take the session's audio and transcribe it. Consumes the buffer: the session
/// is already over by the time this runs.
pub(super) async fn transcribe(pcm: Arc<Pcm>) -> Result<String> {
    let rate = pcm.rate;
    let samples = std::mem::take(&mut *pcm.samples.lock());
    let samples = tokio::task::spawn_blocking(move || resample(rate, samples))
        .await
        .map_err(|e| Error::Other(format!("dictation: resampling task failed: {e}")))??;
    engine::transcribe(samples).await
}

/// Extra output capacity for the converter's priming, which can emit a little
/// more than the rate ratio implies.
const RESAMPLE_SLACK_FRAMES: u32 = 4096;

/// Convert mono `samples` at `rate` to the mono 16 kHz whisper.cpp wants.
///
/// Every ObjC handle here is created, used and dropped inside this call, so
/// nothing is shared across threads and nothing is live across an `.await` —
/// the same rule the recognizer path follows.
fn resample(rate: f64, samples: Vec<f32>) -> Result<Vec<f32>> {
    let target = f64::from(engine::SAMPLE_RATE);
    if rate == target || samples.is_empty() {
        return Ok(samples);
    }

    let from = standard_mono(rate)?;
    let to = standard_mono(target)?;
    let converter =
        unsafe { AVAudioConverter::initFromFormat_toFormat(AVAudioConverter::alloc(), &from, &to) }
            .ok_or_else(|| Error::Other("dictation: couldn't build the audio converter".into()))?;

    let frames = samples.len() as u32;
    let input = pcm_buffer(&from, frames)?;
    unsafe {
        std::ptr::copy_nonoverlapping(samples.as_ptr(), channel(&input)?.as_ptr(), samples.len());
        input.setFrameLength(frames);
    }

    let capacity = (samples.len() as f64 * target / rate).ceil() as u32 + RESAMPLE_SLACK_FRAMES;
    let output = pcm_buffer(&to, capacity)?;

    // The converter pulls input through a block rather than taking a buffer,
    // because a rate conversion may need more or less than it is given. There
    // is only ever one buffer to hand over here: serve it, then say the stream
    // has ended.
    let served = Cell::new(false);
    let feed = RcBlock::new(
        move |_packets: u32, status: NonNull<AVAudioConverterInputStatus>| -> *mut AVAudioBuffer {
            if served.replace(true) {
                unsafe { *status.as_ptr() = AVAudioConverterInputStatus::EndOfStream };
                return std::ptr::null_mut();
            }
            unsafe { *status.as_ptr() = AVAudioConverterInputStatus::HaveData };
            Retained::as_ptr(&input).cast_mut().cast()
        },
    );

    let mut error = None;
    let status = unsafe {
        converter.convertToBuffer_error_withInputFromBlock(
            &output,
            Some(&mut error),
            RcBlock::as_ptr(&feed),
        )
    };
    if status == AVAudioConverterOutputStatus::Error {
        let message = error.map_or_else(
            || "unknown error".to_string(),
            |e| e.localizedDescription().to_string(),
        );
        return Err(Error::Other(format!(
            "dictation: couldn't resample the recording: {message}"
        )));
    }

    // Whatever the status, `frameLength` is what was actually converted.
    let converted = unsafe { output.frameLength() } as usize;
    let mut out = vec![0.0; converted];
    unsafe {
        std::ptr::copy_nonoverlapping(channel(&output)?.as_ptr(), out.as_mut_ptr(), converted)
    };
    Ok(out)
}

/// Deinterleaved float32, one channel, at `rate`.
fn standard_mono(rate: f64) -> Result<Retained<AVAudioFormat>> {
    unsafe {
        AVAudioFormat::initStandardFormatWithSampleRate_channels(AVAudioFormat::alloc(), rate, 1)
    }
    .ok_or_else(|| {
        Error::Other(format!(
            "dictation: {rate} Hz mono isn't a valid audio format"
        ))
    })
}

fn pcm_buffer(format: &AVAudioFormat, frames: u32) -> Result<Retained<AVAudioPCMBuffer>> {
    unsafe {
        AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(AVAudioPCMBuffer::alloc(), format, frames)
    }
    .ok_or_else(|| Error::Other("dictation: couldn't allocate an audio buffer".into()))
}

/// The one channel's samples. Null for a buffer that isn't float32, which the
/// formats above always are.
fn channel(buffer: &AVAudioPCMBuffer) -> Result<NonNull<f32>> {
    NonNull::new(unsafe { buffer.floatChannelData() })
        .map(|data| unsafe { *data.as_ptr() })
        .ok_or_else(|| Error::Other("dictation: audio buffer has no float samples".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resampler is the one part of the capture path that can be exercised
    /// without a microphone: a 440 Hz tone is well inside the 8 kHz band 16 kHz
    /// can carry, so it must come out the same length in seconds and the same
    /// loudness.
    #[test]
    fn resample_keeps_a_tone_that_fits() {
        let rate = 48_000.0;
        let input: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin())
            .collect();

        let out = resample(rate, input).unwrap();

        let expected = engine::SAMPLE_RATE as i64;
        assert!(
            (out.len() as i64 - expected).abs() < expected / 20,
            "one second in, {} samples out",
            out.len()
        );
        let rms = (out
            .iter()
            .map(|s| f64::from(*s) * f64::from(*s))
            .sum::<f64>()
            / out.len() as f64)
            .sqrt();
        assert!(rms > 0.5, "tone was lost: rms {rms}");
    }

    #[test]
    fn resample_passes_matching_rates_through() {
        let samples = vec![0.25; 100];
        assert_eq!(
            resample(f64::from(engine::SAMPLE_RATE), samples.clone()).unwrap(),
            samples
        );
    }
}
