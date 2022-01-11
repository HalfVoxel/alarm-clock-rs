use rodio::{Sink, Source};

use std::{ffi::OsStr, fs::File, thread, time};
use std::{io::BufReader, path::Path, path::PathBuf};

use time::{Duration, Instant};
use crate::filtered_source::dynamic_filter;
use rand::prelude::*;
use thiserror::Error;
use crate::precalculated_source::PrecalculatedSource;
use crate::AlarmState;

fn frquency_cutoff_lp(t: f32) -> f32 {
    let clamped_t = (t - 10.0).max(0.0);
    100000.0f32.min(800.0 + clamped_t.powf(2.5) * 1.0)
}

fn volume(t: f32) -> f32 {
    return 1.0f32.min(0.0 + 0.007 * t + 0.0f32.max(t - 5.0) * 0.013);
}

fn smoothstep(x: f32) -> f32 {
    3.0 * x.powi(2) - 2.0 * x.powi(3)
}

fn fadeout(t: f32, duration: f32) -> f32 {
    smoothstep((1.0 - (t / duration)).max(0.0))
}


fn play(path: &Path, alarm_state: &AlarmState) {
    // let (_stream, handle) = rodio::OutputStream::try_default().unwrap();
    let device = rodio::default_output_device().unwrap();

    // let sink = Sink::try_new(&handle).unwrap();
    let sink = Sink::new(&device);

    // Add a dummy source of the sake of the example.
    let file = File::open(path).unwrap();
    let source = rodio::Decoder::new(BufReader::new(file)).unwrap();

    let (source, controller) = dynamic_filter(source.convert_samples(), Box::new(|t| frquency_cutoff_lp(t as f32) as f64));
    let source = PrecalculatedSource::new(source, 44000*300);
    sink.append(source);

    let alarm_timeout = 120.0;

    let t0 = Instant::now();
    loop {
        let t = Instant::now().duration_since(t0).as_secs_f32();
        if t > alarm_timeout || !alarm_state.should_start_alarm() {
            break;
        }

        // let freq = frquency_cutoff_lp(t);
        // controller.set_lowpass(freq as f64);
        // controller.set_volume();
        sink.set_volume(volume(t));
        thread::sleep(Duration::from_millis(40));
    }

    let t1 = Instant::now();
    let fadeout_duration = 10.0;
    loop {
        let t = Instant::now().duration_since(t0).as_secs_f32();
        let t_fadeout = Instant::now().duration_since(t1).as_secs_f32();
        if t_fadeout >= fadeout_duration {
            break;
        }
        // let freq = frquency_cutoff_lp(t);
        sink.set_volume(volume(t) * fadeout(t_fadeout, fadeout_duration));
        // controller.set_lowpass(freq as f64);
        thread::sleep(Duration::from_millis(40));
    }

    controller.set_volume(0.0);
    sink.stop();
    alarm_state.disable();
}

#[derive(Error, Debug)]
enum AlarmSoundError {
    #[error("Could not read directory `{0}`: {1}")]
    CouldNotReadDir(PathBuf, std::io::Error),
    #[error("There were no sound files in the sound directory")]
    NoFiles,
}

fn random_alarm_sound(root_dir: &Path) -> Result<PathBuf, AlarmSoundError> {
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

pub fn start_alarm_thread(alarm_state: AlarmState) {
    println!("Starting alarm thread");
    loop {
        if alarm_state.should_start_alarm() {
            println!("Starting alarm...");
            match random_alarm_sound(Path::new("./sounds")) {
                Ok(path) => {
                    println!("Playing {}", path.to_str().unwrap());
                    play(&path, &alarm_state)
                }
                Err(e) => {
                    println!("{}", e);
                    alarm_state.disable();
                }
            }
            println!("Alarm finished...");
        }

        thread::sleep(Duration::from_millis(500));
    }
}
