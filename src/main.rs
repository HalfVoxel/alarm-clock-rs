#![feature(proc_macro_hygiene, decl_macro)]
use rocket::State;
use rocket_contrib::json::Json;

use std::{sync::Arc, sync::Mutex, thread, time};

use time::{Duration};

use chrono::{DateTime, Utc};
use chrono::NaiveDateTime;

#[cfg(feature="audio")]
mod filtered_source;

#[cfg(feature="audio")]
mod precalculated_source;
#[cfg(feature="audio")]
mod alarm;

#[macro_use]
extern crate rocket;


#[derive(Clone)]
pub struct AlarmState {
    inner: Arc<Mutex<InnerAlarmState>>,
}

impl AlarmState {
    #[allow(dead_code)]
    fn should_start_alarm(&self) -> bool {
        let state = self.inner.lock().unwrap();
        state.enabled && Utc::now() >= state.next_alarm
    }

    #[allow(dead_code)]
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

#[get("/get")]
fn get_info2(state: State<AlarmState>) -> Json<AlarmInfo> {
    get_info(state)
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
        store_inner(&info, &state);
    }

    get_info(state)
}

fn store_inner(info: &AlarmInfo, state: &AlarmState) {
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

fn start_server(alarm_state: AlarmState) {
    rocket::ignite()
        .manage(alarm_state)
        .mount("/", routes![get_info, get_info2, store])
        .launch();
}

fn sync(alarm_state: &AlarmState, url: &'static str, port: u32) -> Result<(), String> {
    let url = format!("{}:{}", url, port);
    let client = reqwest::blocking::Client::new();
    let res = client.post(url).send().and_then(|x| x.text()).map_err(|e| format!("{:?}", e))?;
    let info: AlarmInfo = serde_json::from_str(&res).unwrap();

    store_inner(&info, alarm_state);
    Ok(())
}

fn start_sync_thread(alarm_state: AlarmState, url: &'static str, port: u32) {
    loop {
        if let Err(err) = sync(&alarm_state, url, port) {
            println!("Sync failed {:?}", err);
            thread::sleep(Duration::from_secs(60));
        }
        thread::sleep(Duration::from_millis(2000));
    }
}

fn main() {
    let remote_server = if std::env::args().any(|x| x == "--remote-sync") {
        Some(("http://home.arongranberg.com/get", 8030))
    } else {
        None
    };

    let alarm_state = AlarmState {
        inner: Arc::new(Mutex::new(InnerAlarmState {
            next_alarm: Utc::now(),
            enabled: false,
        })),
    };

    if let Some((url, port)) = remote_server {
        let audio_alarm_state2 = alarm_state.clone();
        thread::spawn(move || start_sync_thread(audio_alarm_state2, url, port));

        #[cfg(feature="audio")]
        {
            let audio_alarm_state = alarm_state.clone();
            thread::spawn(move || alarm::start_alarm_thread(audio_alarm_state));
        }
    }

    start_server(alarm_state);
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
