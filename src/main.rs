#![feature(proc_macro_hygiene, decl_macro)]
use rocket::State;
use rocket_contrib::json::Json;
use rodio::{Sink, Source};

use std::{ffi::OsStr, fs::File, sync::Arc, sync::Mutex, thread, time};
use std::{io::BufReader, path::Path, path::PathBuf};

use time::{Duration, Instant};
mod filtered_source;
use chrono::{DateTime, Utc};
use filtered_source::dynamic_filter;
use rand::prelude::*;
use thiserror::Error;
use chrono::NaiveDateTime;

#[macro_use]
extern crate rocket;

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

#[derive(Clone)]
struct AlarmState {
    inner: Arc<Mutex<InnerAlarmState>>,
}

impl AlarmState {
    fn should_start_alarm(&self) -> bool {
        let state = self.inner.lock().unwrap();
        state.enabled && Utc::now() >= state.next_alarm
    }

    fn disable(&self) {
        let mut state = self.inner.lock().unwrap();
        state.enabled = false;
    }
}
struct InnerAlarmState {
    next_alarm: DateTime<Utc>,
    enabled: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AlarmInfo {
    time: String,
    enabled: bool,
}

#[post("/get")]
fn get_info(state: State<AlarmState>) -> Json<AlarmInfo> {
    let state = state.inner.lock().unwrap();
    Json(AlarmInfo {
        time: state.next_alarm.format("%Y-%m-%dT%H:%M:%S").to_string(),
        enabled: state.enabled,
    })
}

#[post("/store", data = "<info>")]
fn store(info: Json<AlarmInfo>, state: State<AlarmState>) -> Json<AlarmInfo> {
    {
        let mut state = state.inner.lock().unwrap();
        let naive_datetime = NaiveDateTime::parse_from_str(&info.time, "%Y-%m-%dT%H:%M:%S%.f").expect("Could not parse date");
        state.next_alarm = DateTime::<Utc>::from_utc(naive_datetime, chrono::Utc);
        println!(
            "Set alarm to {} which is {} minutes into the future",
            state.next_alarm,
            state
                .next_alarm
                .signed_duration_since(Utc::now())
                .num_minutes()
        );
        state.enabled = info.enabled;
    }

    return get_info(state);
}

fn start_server(alarm_state: AlarmState) {
    rocket::ignite()
        .manage(alarm_state)
        .mount("/", routes![get_info, store])
        .launch();
}

fn play(path: &PathBuf, alarm_state: &AlarmState) {
    // let (_stream, handle) = rodio::OutputStream::try_default().unwrap();
    let device = rodio::default_output_device().unwrap();

    // let sink = Sink::try_new(&handle).unwrap();
    let sink = Sink::new(&device);

    // Add a dummy source of the sake of the example.
    let file = File::open(path).unwrap();
    let source = rodio::Decoder::new(BufReader::new(file)).unwrap();

    let (source, controller) = dynamic_filter(source.convert_samples());
    sink.append(source);

    let alarm_timeout = 120.0;

    let t0 = Instant::now();
    loop {
        let t = Instant::now().duration_since(t0).as_secs_f32();
        if t > alarm_timeout || !alarm_state.should_start_alarm() {
            break;
        }

        let freq = frquency_cutoff_lp(t);
        controller.set_lowpass(freq as f64);
        controller.set_volume(volume(t) as f64);
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
        let freq = frquency_cutoff_lp(t);
        controller.set_volume(volume(t) as f64 * fadeout(t_fadeout, fadeout_duration) as f64);
        controller.set_lowpass(freq as f64);
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

fn start_alarm_thread(alarm_state: AlarmState) {
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

fn main() {
    let alarm_state = AlarmState {
        inner: Arc::new(Mutex::new(InnerAlarmState {
            next_alarm: Utc::now(),
            enabled: false,
        })),
    };

    let audio_alarm_state = alarm_state.clone();
    thread::spawn(move || start_alarm_thread(audio_alarm_state));
    start_server(alarm_state.clone());
}

// use synthrs::synthesizer::{ make_samples, quantize_samples };
// use synthrs::writer::write_wav_file;

// fn main() {
//     let file = File::open("test.wav").unwrap();
//     let source = rodio::Decoder::new(BufReader::new(file)).unwrap();
//     let sample_rate = source.sample_rate() as usize;
//     let samples = source
//     .convert_samples::<f32>().step_by(2).map(|x| x as f64).collect::<Vec<f64>>();
//     println!("Done reading");

//     let lowpass = lowpass_filter(cutoff_from_frequency(5000.0, sample_rate), 0.01);
//     // let samples = synthrs::filter::convolve(&lowpass, &samples);
//     let mut output_samples = vec![];
//     output_samples.resize(samples.len() - lowpass.len(), 0.0f64);
//     convolve_f64(&lowpass, &samples, &mut output_samples);
//     let samples = output_samples;

//     // let (samples, samples_len) =
//     //     sample::samples_from_wave_file("./test.wav").unwrap();

//     let quantized = quantize_samples::<i16>(
//         &samples
//     );
//     println!("Writing");
//     // Using a predefined generator
//     write_wav_file("out.wav", sample_rate,
//         &quantized
//     ).expect("failed to write to file");
// }
