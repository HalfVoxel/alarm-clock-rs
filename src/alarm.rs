use chrono::{DateTime, TimeDelta, Utc};
use log::info;
use rodio::{Sink, Source};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use std::{ffi::OsStr, thread, time};
use std::{path::Path, path::PathBuf};

use crate::filtered_source::dynamic_filter;
use crate::AlarmState;
use rand::prelude::*;
use symphonia::core::audio::SampleBuffer;
use thiserror::Error;
use time::{Duration, Instant};

fn frequency_cutoff_lowpass(t: f32) -> f32 {
    let clamped_t = (t - 10.0).max(0.0);
    100000.0f32.min(800.0 + clamped_t.powf(2.5) * 1.0)
}

pub fn fadein_slow(t: f32) -> f32 {
    1.0f32.min(0.007 * t + 0.0f32.max(t - 5.0) * 0.013)
}

pub fn smoothstep(x: f32) -> f32 {
    3.0 * x.powi(2) - 2.0 * x.powi(3)
}

pub fn fadein(t: f32, duration: f32) -> f32 {
    smoothstep((t.max(0.0) / duration).min(1.0))
}

pub fn fadeout(t: f32, duration: f32) -> f32 {
    smoothstep((1.0 - (t.max(0.0) / duration)).max(0.0))
}

/// Decode Mp3 using symphonia.
///
/// rodio's built-in mp3 decodeer (minimp3) seems to trigger out of range asserts in debug mode, and possibly does pretty unsafe things in release mode.
/// It's also just a c++ blob. Which is also not very nice.
///
/// Hopefully symphonia is more robust.
fn decode_mp3(path: &Path) -> rodio::buffer::SamplesBuffer<f32> {
    // Open the media source.
    let src = std::fs::File::open(path).expect("failed to open media");

    // Create the media source stream.
    let mss = MediaSourceStream::new(Box::new(src), Default::default());

    // Create a probe hint using the file's extension. [Optional]
    let mut hint = symphonia::core::probe::Hint::new();
    hint.with_extension("mp3");

    // Use the default options for metadata and format readers.
    let meta_opts: MetadataOptions = Default::default();
    let fmt_opts: FormatOptions = Default::default();

    // Probe the media source.
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &fmt_opts, &meta_opts)
        .expect("unsupported format");

    // Get the instantiated format reader.
    let mut format = probed.format;

    // Find the first audio track with a known (decodeable) codec.
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .expect("no supported audio tracks");

    // Use the default options for the decoder.
    let dec_opts: DecoderOptions = Default::default();

    // Create a decoder for the track.
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &dec_opts)
        .expect("unsupported codec");

    // Store the track identifier, it will be used to filter packets.
    let track_id = track.id;
    let mut all_samples: Vec<f32> = vec![];
    let sample_rate = track.codec_params.sample_rate.unwrap();

    // The decode loop.
    loop {
        // Get the next packet from the media format.
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::ResetRequired) => {
                // The track list has been changed. Re-examine it and create a new set of decoders,
                // then restart the decode loop. This is an advanced feature and it is not
                // unreasonable to consider this "the end." As of v0.5.0, the only usage of this is
                // for chained OGG physical streams.
                unimplemented!();
            }
            Err(symphonia::core::errors::Error::IoError(er))
                if er.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                // End of file
                break;
            }
            Err(err) => {
                // A unrecoverable error occurred, halt decoding.
                panic!("{}", err);
            }
        };

        // Consume any new metadata that has been read since the last packet.
        while !format.metadata().is_latest() {
            // Pop the old head of the metadata queue.
            format.metadata().pop();

            // Consume the new metadata at the head of the metadata queue.
        }

        // If the packet does not belong to the selected track, skip over it.
        if packet.track_id() != track_id {
            continue;
        }

        // Decode the packet into audio samples.
        match decoder.decode(&packet) {
            Ok(decoded) => {
                // Consume the decoded audio samples (see below).
                let mut sample_buf =
                    SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
                sample_buf.copy_interleaved_ref(decoded);
                // let buf = decoded.make_equivalent::<f32>();
                // all_samples.extend(buf.chan(0).iter().cloned());
                all_samples.extend(sample_buf.samples());
            }
            Err(symphonia::core::errors::Error::IoError(er))
                if er.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                // End of file
                break;
            }
            Err(symphonia::core::errors::Error::IoError(err)) => {
                // The packet failed to decode due to an IO error, skip the packet.
                panic!("{:#?}", err);
            }
            Err(symphonia::core::errors::Error::DecodeError(err)) => {
                // The packet failed to decode due to invalid data, skip the packet.
                panic!("{:#?}", err);
            }
            Err(err) => {
                // An unrecoverable error occurred, halt decoding.
                panic!("{:#?}", err);
            }
        }
    }

    println!("Decoded {} samples", all_samples.len());

    rodio::buffer::SamplesBuffer::new(2, sample_rate, all_samples)
}

pub fn play_audio(path: &Path, mut vol: impl FnMut(f32) -> Option<f32>, lowpass: bool) {
    let device = rodio::default_output_device().unwrap();

    let sink = Sink::new(&device);

    // Add a dummy source of the sake of the example.
    let source_samples = decode_mp3(path);
    let total_duration = source_samples.total_duration();

    let (source, controller) = dynamic_filter(
        source_samples,
        Box::new(move |t| {
            if lowpass {
                frequency_cutoff_lowpass(t as f32) as f64
            } else {
                100_000.0
            }
        }),
    );

    let mut sources: Vec<Box<dyn rodio::source::Source<Item = f32> + Send>> = vec![];

    let speaker_has_standby_mode = false;
    if speaker_has_standby_mode {
        let sine = rodio::source::SineWave::new(30).amplify(0.7);
        sources.push(Box::new(
            // Play sine wave for a few seconds to make the speakers wake up
            sine.take_duration(Duration::from_millis(5000))
                // Fade in sine wave over one second to avoid speaker pop
                .fade_in(Duration::from_millis(1000)),
        ))
    }

    sources.push(Box::new(source));

    let source = rodio::source::from_iter(sources);

    sink.append(source);

    let t0 = Instant::now();
    loop {
        let t = Instant::now().duration_since(t0).as_secs_f32();
        if let Some(total_duration) = total_duration {
            if t > total_duration.as_secs_f32() {
                break;
            }
        }

        if let Some(v) = vol(t) {
            controller.set_volume(v);
            thread::sleep(Duration::from_millis(40));
        } else {
            break;
        }
    }

    controller.set_volume(0.0);
    sink.stop();
}

#[cfg(feature = "motion")]
async fn snooze(alarm_state: AlarmState, trigger_time: DateTime<Utc>) {
    // In a few minutes, check if the user is still in bed, and if so, re-enable the alarm

    use chrono::TimeDelta;
    tokio::time::sleep(time::Duration::from_secs(15 * 60)).await;
    let prev_state = alarm_state.inner.get().clone().unwrap();
    if prev_state.enabled
        && prev_state.next_alarm == trigger_time
        && alarm_state
            .sleep_monitor
            .lock()
            .await
            .sleep_monitor
            .is_present()
    {
        alarm_state
            .last_played
            .update(|s| {
                // We must do a minimum here, because if the alarm started early because of motion, then this could otherwise fail to play the alarm again, because we
                // set the 'last played time' to trigger time, which might be after the alarm retry time (now).
                s.last_played_time = s
                    .last_played_time
                    .min(Some(Utc::now() - TimeDelta::seconds(1)));
            })
            .await;
        alarm_state
            .inner
            .update(|s| {
                s.enabled = true;
                s.next_alarm = Utc::now();
            })
            .await;
    }
}

fn play_alarm(path: &Path, trigger_time: DateTime<Utc>, alarm_state: &AlarmState) {
    let alarm_timeout = 5.0 * 60.0;
    let mut fadeout_start = None;
    let fadeout_duration = 5.0;

    play_audio(
        path,
        |t| {
            let v = fadein_slow(t);
            if let Some(fadeout_start) = fadeout_start {
                let t_fadeout = t - fadeout_start;
                if t_fadeout > fadeout_duration {
                    return None;
                }
                Some(v * fadeout(t_fadeout, fadeout_duration))
            } else {
                if t > alarm_timeout || !alarm_state.is_trigger_time(trigger_time) {
                    fadeout_start = Some(t);
                }
                Some(v)
            }
        },
        true,
    );

    let manually_cancelled = !alarm_state.is_trigger_time(trigger_time);

    futures::executor::block_on(alarm_state.on_alarm_finished(trigger_time));

    #[cfg(feature = "motion")]
    {
        if !manually_cancelled {
            let alarm_state = alarm_state.clone();
            tokio::spawn(snooze(alarm_state, trigger_time));
        }
    }
}

#[derive(Error, Debug)]
pub enum AlarmSoundError {
    #[error("Could not read directory `{0}`: {1}")]
    CouldNotReadDir(PathBuf, std::io::Error),
    #[error("There were no sound files in the sound directory")]
    NoFiles,
}

pub fn random_alarm_sound(root_dir: &Path) -> Result<PathBuf, AlarmSoundError> {
    let valid_extensions = ["mp3", "ogg", "flac", "wav"];
    match root_dir.read_dir() {
        Ok(iter) => iter
            .filter_map(|x| x.ok().map(|x| x.path()))
            .filter(|path| {
                path.extension()
                    .and_then(OsStr::to_str)
                    .map(|x| valid_extensions.contains(&x))
                    .unwrap_or_default()
            })
            .choose(&mut rand::thread_rng())
            .ok_or(AlarmSoundError::NoFiles),
        Err(e) => Err(AlarmSoundError::CouldNotReadDir(root_dir.to_path_buf(), e)),
    }
}

pub async fn start_alarm_thread(alarm_state: AlarmState) {
    info!("Starting alarm thread");
    loop {
        #[allow(unused_mut)]
        let mut trigger_time = alarm_state.should_start_alarm();

        // If the alarm should start soon, and there is significant movement, start the alarm.
        // Movement may indicate REM sleep, and it is desirable to wake up the user during REM sleep.
        #[cfg(feature = "motion")]
        if let Some(t) = alarm_state.should_start_alarm_soon(TimeDelta::minutes(30)) {
            if alarm_state
                .sleep_monitor
                .lock()
                .await
                .sleep_monitor
                .is_significant_movement()
            {
                trigger_time = Some(t);
            }
        };

        if let Some(trigger_time) = trigger_time {
            info!("Starting alarm...");
            match random_alarm_sound(Path::new("./sounds")) {
                Ok(path) => {
                    info!("Playing {}", path.to_str().unwrap());
                    #[cfg(feature = "motion")]
                    {
                        alarm_state.sleep_monitor.lock().await.alarm_is_playing = true;
                    }
                    alarm_state.is_playing.set(true).await;
                    {
                        let alarm_state = alarm_state.clone();
                        tokio::task::spawn_blocking(move || {
                            // TODO: Make into async function
                            play_alarm(&path, trigger_time, &alarm_state);
                        })
                        .await
                        .unwrap();
                    }
                    #[cfg(feature = "motion")]
                    {
                        alarm_state.sleep_monitor.lock().await.alarm_is_playing = false;
                    }
                    alarm_state.is_playing.set(false).await;
                }
                Err(e) => {
                    error!("{}", e);
                    alarm_state.on_alarm_finished(trigger_time).await;
                }
            }
            info!("Alarm finished...");
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
