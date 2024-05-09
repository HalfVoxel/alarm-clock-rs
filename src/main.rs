use rocket::State;
use rocket::serde::json::Json;

use std::{sync::Arc, sync::Mutex, thread, time};

use time::Duration;

use chrono::NaiveDateTime;
use chrono::{DateTime, Utc};

#[cfg(feature = "audio")]
mod filtered_source;

#[cfg(feature = "audio")]
mod alarm;
#[cfg(feature = "audio")]
mod precalculated_source;

#[macro_use]
extern crate rocket;

#[derive(Clone)]
pub struct AlarmState {
    inner: Arc<Mutex<InnerAlarmState>>,
    sync_url: Option<&'static str>,
}

impl AlarmState {
    #[allow(dead_code)]
    fn should_start_alarm(&self) -> Option<DateTime<Utc>> {
        let state = self.inner.lock().unwrap();
        if state.enabled && Utc::now() >= state.next_alarm && state.last_played_alarm.map(|v| state.next_alarm > v).unwrap_or(true) {
            Some(state.next_alarm)
        } else {
            None
        }
    }

    #[allow(dead_code)]
    fn disable(&self) {
        let mut state = self.inner.lock().unwrap();
        state.enabled = false;
    }

    fn on_alarm_finished(&self, time: DateTime<Utc>) {
        let mut state = self.inner.lock().unwrap();
        state.last_played_alarm = state.last_played_alarm.max(Some(time));
    }
}

#[derive(PartialEq, Eq, Debug, Clone, serde::Serialize, serde::Deserialize)]
struct InnerAlarmState {
    next_alarm: DateTime<Utc>,
    last_played_alarm: Option<DateTime<Utc>>,
    enabled: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AlarmInfo {
    time: String,
    enabled: bool,
}

#[get("/get")]
fn get_info(state: &State<AlarmState>) -> Json<AlarmInfo> {
    let state = state.inner.lock().unwrap();
    Json(state_to_info(&state))
}

#[get("/state")]
fn get_state(state: &State<AlarmState>) -> Json<InnerAlarmState> {
    let state = state.inner.lock().unwrap();
    Json(state.clone())
}

#[put("/state", data = "<new_state>")]
fn put_state(state: &State<AlarmState>, new_state: Json<InnerAlarmState>) -> Json<InnerAlarmState> {
    store_inner(state, (*new_state).clone());
    Json(state.inner.lock().unwrap().clone())
}

fn state_to_info(state: &InnerAlarmState) -> AlarmInfo {
    AlarmInfo {
        time: state.next_alarm.format("%Y-%m-%dT%H:%M:%S").to_string(),
        enabled: state.enabled && state.last_played_alarm.map(|v| state.next_alarm > v).unwrap_or(true),
    }
}

#[post("/get")]
fn get_info_compat(state: &State<AlarmState>) -> Json<AlarmInfo> {
    get_info(state)
}

#[post("/store", data = "<info>")]
fn store_compat(info: Json<AlarmInfo>, state: &State<AlarmState>) -> Json<AlarmInfo> {
    let naive_datetime = NaiveDateTime::parse_from_str(&info.time, "%Y-%m-%dT%H:%M:%S%.f")
        .expect("Could not parse date");
    let next_alarm = DateTime::<Utc>::from_utc(naive_datetime, chrono::Utc);
    let new_state = {
        let current_state = state.inner.lock().unwrap();
        InnerAlarmState {
            next_alarm,
            last_played_alarm: current_state.last_played_alarm,
            enabled: info.enabled,
        }
    };
    
    store_inner(state, new_state);
    get_info(state)
}

#[put("/state/last_played_alarm", data = "<time>")]
fn on_alarm_finished(time: Json<DateTime<Utc>>, state: &State<AlarmState>) -> Json<AlarmInfo> {
    println!("Alarm finished at {time:?}");
    let mut s = state.inner.lock().unwrap();
    s.last_played_alarm = s.last_played_alarm.max(Some(*time));
    get_info(state)
}

fn store_inner(state: &AlarmState, new_state: InnerAlarmState) {
    let mut state: std::sync::MutexGuard<InnerAlarmState> = state.inner.lock().unwrap();
    let orig_state = state.clone();
    *state = new_state;
    let diff = *state != orig_state;

    if diff {
        if state.enabled {
            println!(
                "Set alarm to {} which is {} minutes into the future",
                state.next_alarm,
                state
                    .next_alarm
                    .signed_duration_since(Utc::now())
                    .num_minutes()
            );
        } else {
            println!("Disabled alarm");
        }
    }
}

fn sync(alarm_state: &AlarmState, url: &'static str) -> Result<(), String> {
    let client = reqwest::blocking::Client::new();
    let res = client
        .get(format!("{}/state", url))
        .send()
        .and_then(|x| x.error_for_status())
        .and_then(|x| x.text())
        .map_err(|e| format!("{:?}", e))?;
    let new_state: InnerAlarmState = serde_json::from_str(&res).unwrap();

    store_inner(alarm_state, new_state);
    Ok(())
}

const RETRIES: u32 = 5;

pub fn disable_alarm_and_sync(alarm_state: &AlarmState, trigger_time: DateTime<Utc>) -> Result<(), String> {
    alarm_state.on_alarm_finished(trigger_time);
    if let Some(url) = alarm_state.sync_url {
        let client = reqwest::blocking::Client::new();

        // Sometimes the server is not reachable, so we retry a few times
        // DNS resolution fails or something. Maybe WIFI got obstructed?
        println!("Disabling alarm on remote...");
        let mut i = 0;
        loop {
            let json = serde_json::to_string(&trigger_time).unwrap();
            // This endpoint will disable the alarm on the remote iff the "next alarm" time
            // is the same as the one we have stored right now.
            match client
                .put(format!("{}/state/last_played_alarm", url))
                .body(json)
                .send()
                .and_then(|x| x.error_for_status())
                 {
                Ok(_) => {
                    return Ok(());
                }
                Err(e) if i < RETRIES => {
                    i += 1;
                    println!("Failed to sync: {:?}. Trying again... {i}/{RETRIES}", e);
                    thread::sleep(Duration::from_secs_f64(2.0f64.powf(i as f64)));
                }
                Err(e) => {
                    return Err(format!("{:?}", e));
                }
            }
        }
    } else {
        Ok(())
    }
}

fn start_sync_thread(alarm_state: AlarmState, url: &'static str) {
    loop {
        if let Err(err) = sync(&alarm_state, url) {
            println!("Sync failed {:?}", err);
            thread::sleep(Duration::from_secs(60));
        }

        let sleep_ms = if alarm_state.should_start_alarm().is_some() {
            400
        } else {
            5000
        };
        thread::sleep(Duration::from_millis(sleep_ms));
    }
}

#[launch]
fn launch_rocket() -> _ {
    let remote_server = if std::env::args().any(|x| x == "--remote-sync") {
        Some("http://alarm.arongranberg.com")
    } else {
        None
    };

    let play_immediately = std::env::args().any(|x| x == "--play");
    let alarm_state = AlarmState {
        inner: Arc::new(Mutex::new(InnerAlarmState {
            next_alarm: Utc::now(),
            last_played_alarm: None,
            enabled: play_immediately,
        })),
        sync_url: remote_server,
    };

    if let Some(url) = remote_server {
        let audio_alarm_state2 = alarm_state.clone();
        thread::spawn(move || start_sync_thread(audio_alarm_state2, url));
    }

    #[cfg(feature = "audio")]
    if remote_server.is_some() || play_immediately {
        let audio_alarm_state = alarm_state.clone();
        thread::spawn(move || alarm::start_alarm_thread(audio_alarm_state));
    }

    rocket::build()
        .manage(alarm_state)
        .mount("/", routes![get_info, get_info_compat, store_compat, on_alarm_finished, get_state, put_state])
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
