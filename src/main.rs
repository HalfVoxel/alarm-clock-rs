use brevduva::{SyncStorage, SyncedContainer};
use rocket::serde::json::Json;
use rocket::State;
use serde::{Deserialize, Serialize};
use sync_common::TestSync;

use rocket::tokio::sync::Mutex;
use std::fmt::Debug;
use std::io::Write;
use std::{sync::Arc, thread, time};

use time::Duration;

use chrono::{DateTime, Utc};
use chrono::{Duration as DateDuration, NaiveDateTime};

#[cfg(feature = "audio")]
mod filtered_source;

#[cfg(feature = "audio")]
mod alarm;
#[cfg(feature = "audio")]
mod precalculated_source;

#[cfg(feature = "motion")]
mod sleep_monitor;

#[macro_use]
extern crate rocket;

#[derive(Clone)]
pub struct AlarmState {
    inner: Arc<SyncedContainer<InnerAlarmState>>,
    last_played: Arc<SyncedContainer<LastPlayed>>,
    #[cfg(feature = "motion")]
    sleep_monitor: Arc<Mutex<SleepMonitorState>>,
    storage: SyncStorage,
    is_playing: Arc<SyncedContainer<bool>>,
    is_user_in_bed: Arc<SyncedContainer<bool>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LastPlayed {
    last_played_time: Option<DateTime<Utc>>,
}

#[cfg(feature = "motion")]
struct SleepMonitorState {
    sleep_monitor: sleep_monitor::SleepMonitor,
    accelerometer: sleep_monitor::Accelerometer,
    alarm_is_playing: bool,
}

impl AlarmState {
    #[allow(dead_code)]
    fn should_start_alarm(&self) -> Option<DateTime<Utc>> {
        self.should_start_alarm_soon(DateDuration::zero())
    }

    #[allow(dead_code)]
    fn should_start_alarm_soon(&self, margin: DateDuration) -> Option<DateTime<Utc>> {
        let state = self.inner.get().clone().unwrap();
        let last_played = self.last_played.get().clone().unwrap();
        if state.enabled
            && Utc::now() + margin >= state.next_alarm
            && last_played
                .last_played_time
                .map(|v| state.next_alarm > v)
                .unwrap_or(true)
        {
            assert!(state.is_trigger_time(state.next_alarm, &last_played));
            Some(state.next_alarm)
        } else {
            None
        }
    }

    fn is_trigger_time(&self, time: DateTime<Utc>) -> bool {
        self.inner
            .get()
            .clone()
            .unwrap()
            .is_trigger_time(time, self.last_played.get().as_ref().unwrap())
    }

    async fn on_alarm_finished(&self, time: DateTime<Utc>) {
        self.last_played
            .update(|data| {
                data.last_played_time = Some(time);
            })
            .await;
    }
}

#[derive(PartialEq, Eq, Debug, Clone, serde::Serialize, serde::Deserialize)]
struct InnerAlarmState {
    next_alarm: DateTime<Utc>,
    enabled: bool,
}

impl InnerAlarmState {
    fn is_trigger_time(&self, time: DateTime<Utc>, last_played: &LastPlayed) -> bool {
        self.enabled
            && self.next_alarm == time
            && last_played
                .last_played_time
                .map(|v| time > v)
                .unwrap_or(true)
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
struct AlarmInfo {
    time: String,
    enabled: bool,
}

#[get("/get")]
fn get_info(state: &State<AlarmState>) -> Json<AlarmInfo> {
    let last_played = state.last_played.get().clone().unwrap();
    let state = state.inner.get().clone().unwrap();

    Json(state_to_info(&state, &last_played))
}

#[post("/get")]
fn get_info_compat(state: &State<AlarmState>) -> Json<AlarmInfo> {
    get_info(state)
}

#[get("/state")]
fn get_state(state: &State<AlarmState>) -> Json<InnerAlarmState> {
    let state = state.inner.get().clone().unwrap();
    Json(state)
}

#[put("/state", data = "<new_state>")]
async fn put_state(
    state: &State<AlarmState>,
    new_state: Json<InnerAlarmState>,
) -> Json<InnerAlarmState> {
    state.inner.set(new_state.0).await;
    Json(state.inner.get().clone().unwrap())
}

fn state_to_info(state: &InnerAlarmState, last_played: &LastPlayed) -> AlarmInfo {
    AlarmInfo {
        time: state.next_alarm.format("%Y-%m-%dT%H:%M:%S").to_string(),
        enabled: state.enabled
            && last_played
                .last_played_time
                .map(|v| state.next_alarm > v)
                .unwrap_or(true),
    }
}

#[post("/store", data = "<info>")]
async fn store_compat(info: Json<AlarmInfo>, state: &State<AlarmState>) -> Json<AlarmInfo> {
    let naive_datetime = NaiveDateTime::parse_from_str(&info.time, "%Y-%m-%dT%H:%M:%S%.f")
        .expect("Could not parse date");
    let next_alarm = DateTime::<Utc>::from_naive_utc_and_offset(naive_datetime, chrono::Utc);
    let new_state = {
        InnerAlarmState {
            next_alarm,
            enabled: info.enabled,
        }
    };

    store_inner(state, new_state).await;
    get_info(state)
}

// #[put("/state/last_played_alarm", data = "<time>")]
// fn on_alarm_finished(time: Json<DateTime<Utc>>, state: &State<AlarmState>) -> Json<AlarmInfo> {
//     println!("Alarm finished at {time:?}");
//     {
//         let mut s = state.inner.blocking_lock();
//         s.last_played_alarm = s.last_played_alarm.max(Some(*time));
//     }
//     get_info(state)
// }

async fn store_inner(state: &AlarmState, new_state: InnerAlarmState) {
    state
        .inner
        .update(|state| {
            let orig_state = state.clone();
            *state = new_state;
            let diff = *state != orig_state;

            if diff {
                if state.enabled {
                    info!(
                        "Set alarm to {} which is {} minutes into the future",
                        state.next_alarm,
                        state
                            .next_alarm
                            .signed_duration_since(Utc::now())
                            .num_minutes()
                    );
                } else {
                    info!("Disabled alarm");
                }
            }
        })
        .await;
}

#[cfg(feature = "motion")]
fn monitor_sleep(state: Arc<Mutex<SleepMonitorState>>) {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open("accelerometer.csv")
        .unwrap();

    loop {
        if !state.blocking_lock().sleep_monitor.is_present() {
            // Don't collect as much data when the user is not in bed
            thread::sleep(Duration::from_secs(1));
        }

        // Take 10 samples every 100 ms and average them
        const SAMPLES: usize = 10;
        const PERIOD_MS: u64 = 10;
        let mut samples = vec![];
        for _ in 0..SAMPLES {
            {
                let mut s = state.blocking_lock();
                let data = s.accelerometer.get_data().unwrap();
                samples.push(data);
            }
            thread::sleep(Duration::from_millis(PERIOD_MS));
        }
        let mean = sleep_monitor::AccelerometerData::mean(&samples);
        let alarm_is_playing = {
            let mut s = state.blocking_lock();
            s.sleep_monitor.push(mean.clone());
            s.alarm_is_playing
        };
        let time = Utc::now();
        let line = format!(
            "{},{},{},{},{},{},{},{},{},{}\n",
            // YYYY-MM-DD HH:MM:SS.SSS
            time.format("%Y-%m-%d %H:%M:%S%.3f"),
            SAMPLES,
            alarm_is_playing as u32,
            mean.acc.0,
            mean.acc.1,
            mean.acc.2,
            mean.gyro.0,
            mean.gyro.1,
            mean.gyro.2,
            mean.temp,
        );
        file.write_all(line.as_bytes()).unwrap();
    }
}

const MQTT_HOST: &str = "mqtt://arongranberg.com:1883";
const MQTT_CLIENT_ID: &str = "alarm";
const MQTT_USERNAME: &str = "wakeup_alarm";
const MQTT_PASSWORD: &str = "xafzz25nomehasff";

#[rocket::main]
async fn main() -> Result<(), rocket::Error> {
    env_logger::init();

    #[cfg(feature = "motion")]
    let acc = match sleep_monitor::Accelerometer::new() {
        Ok(acc) => acc,
        Err(e) => {
            panic!("Failed to initialize accelerometer: {:?}", e);
        }
    };

    // let remote_server = if std::env::args().any(|x| x == "--remote-sync") {
    //     Some("http://alarm.arongranberg.com")
    // } else {
    //     None
    // };

    let storage = SyncStorage::new(MQTT_CLIENT_ID, MQTT_HOST, MQTT_USERNAME, MQTT_PASSWORD).await;
    println!("0");
    let inner_state = storage
        .add_container(InnerAlarmState {
            next_alarm: Utc::now(),
            enabled: false,
        })
        .await;
    storage.add_container(TestSync { count: 0 }).await;

    let last_played = storage
        .add_container(LastPlayed {
            last_played_time: None,
        })
        .await;

    let is_playing = storage.add_container(false).await;
    let is_user_in_bed = storage.add_container(false).await;

    storage.wait_for_sync().await;

    let play_immediately = std::env::args().any(|x| x == "--play");
    let alarm_state = AlarmState {
        storage,
        inner: inner_state,
        last_played,
        is_playing,
        is_user_in_bed: is_user_in_bed.clone(),
        #[cfg(feature = "motion")]
        sleep_monitor: Arc::new(Mutex::new(SleepMonitorState {
            accelerometer: acc,
            sleep_monitor: sleep_monitor::SleepMonitor::new(
                Duration::from_secs(10 * 60),
                is_user_in_bed,
            ),
            alarm_is_playing: false,
        })),
        // sync_url: remote_server,
    };

    let r = rocket::build().manage(alarm_state.clone()).mount(
        "/",
        routes![
            get_info,
            get_info_compat,
            store_compat,
            // on_alarm_finished,
            get_state,
            put_state
        ],
    );
    let rocket_task = tokio::spawn(r.launch());

    #[cfg(feature = "motion")]
    {
        let sm = alarm_state.sleep_monitor.clone();
        thread::spawn(move || monitor_sleep(sm));
    }

    // if let Some(url) = remote_server {
    //     sync_down(&alarm_state, url).unwrap();
    // }

    if play_immediately {
        println!("Playing alarm immediately");
        alarm_state
            .inner
            .update(|s| {
                s.next_alarm = Utc::now();
                s.enabled = true;
            })
            .await;
    }

    // if let Some(url) = remote_server {
    //     sync_up(&alarm_state).unwrap();
    //     let audio_alarm_state2 = alarm_state.clone();
    //     thread::spawn(move || start_sync_thread(audio_alarm_state2, url));
    // }

    #[cfg(feature = "audio")]
    {
        let audio_alarm_state = alarm_state.clone();
        thread::spawn(move || alarm::start_alarm_thread(audio_alarm_state));
    }

    rocket_task.await.unwrap().unwrap();
    Ok(())
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
