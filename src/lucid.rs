// At random times before waking up, play low volume music
// At random times before waking up, play low volume sfx

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use brevduva::SyncedContainer;
use chrono::TimeDelta;
use rand::{rngs::StdRng, Rng, SeedableRng};

use crate::{
    alarm::{fadein, fadeout, random_alarm_sound},
    AlarmState,
};

async fn monitor_sleeping_duration(
    alarm_state: AlarmState,
    sleeping_start_time: Arc<Mutex<Option<Instant>>>,
    is_user_in_bed: Arc<SyncedContainer<bool>>,
    is_significant_movement_in_bed: Arc<SyncedContainer<bool>>,
) {
    loop {
        let is_user_in_bed = is_user_in_bed.get().unwrap_or(false);
        let is_awake = is_significant_movement_in_bed.get().unwrap_or(false);

        let alarm_is_active = alarm_state
            .should_start_alarm_soon(TimeDelta::hours(12))
            .is_some();

        if alarm_is_active && is_user_in_bed && !is_awake {
            let mut data = sleeping_start_time.lock().unwrap();
            if data.is_none() {
                println!("Asleep at {}", chrono::Local::now());
                data.replace(Instant::now());
            }
        } else if !alarm_is_active {
            sleeping_start_time.lock().unwrap().take();
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

fn should_start_lucid_sounds(
    is_user_in_bed: bool,
    alarm_is_active: bool,
    user_is_waking_up_soon: bool,
    sleeping_time: Option<Duration>,
    minimum_sleeping_time: Duration,
) -> bool {
    is_user_in_bed
        && alarm_is_active
        && !user_is_waking_up_soon
        && sleeping_time
            .map(|d| d > minimum_sleeping_time)
            .unwrap_or(false)
}

#[test]
fn test_should_start_lucid_sounds() {
    assert!(!should_start_lucid_sounds(
        false,
        false,
        false,
        None,
        Duration::from_secs(0)
    ));

    assert!(should_start_lucid_sounds(
        true,
        true,
        false,
        Some(Duration::from_secs(60 * 60 * 2)),
        Duration::from_secs(60 * 60)
    ));

    assert!(!should_start_lucid_sounds(
        true,
        true,
        false,
        Some(Duration::from_secs(60)),
        Duration::from_secs(60 * 60)
    ));

    assert!(!should_start_lucid_sounds(
        true,
        true,
        true,
        Some(Duration::from_secs(60 * 60 * 2)),
        Duration::from_secs(60 * 60)
    ));
}

async fn should_start_lucid_sounds2(
    alarm_state: AlarmState,
    sleeping_start_time_data: &Arc<Mutex<Option<Instant>>>,
    is_user_in_bed: Arc<SyncedContainer<bool>>,
    is_significant_movement_in_bed: Arc<SyncedContainer<bool>>,
    minimum_sleeping_time: Duration,
    require_movement: bool,
) -> bool {
    let sleeping_start_time = *sleeping_start_time_data.lock().unwrap();

    let is_user_in_bed = is_user_in_bed.get().unwrap_or(false);
    let is_significant_movement = is_significant_movement_in_bed.get().unwrap_or(false);

    let alarm_is_active = alarm_state
        .should_start_alarm_soon(TimeDelta::hours(12))
        .is_some();
    let user_is_waking_up_soon = alarm_state
        .should_start_alarm_soon(TimeDelta::minutes(50))
        .is_some();

    println!("Evaluating lucid effects");

    let sleeping_time = sleeping_start_time.map(|t| t.elapsed());
    dbg!(
        is_user_in_bed,
        is_significant_movement,
        alarm_is_active,
        user_is_waking_up_soon,
        sleeping_time
    );

    if require_movement && !is_significant_movement {
        return false;
    }

    should_start_lucid_sounds(
        is_user_in_bed,
        alarm_is_active,
        user_is_waking_up_soon,
        sleeping_time,
        minimum_sleeping_time,
    )
}

fn play_lucid_sounds(
    rng: &mut StdRng,
    lucid_music_volume: &SyncedContainer<i32>,
    lucid_sfx_volume: &SyncedContainer<i32>,
) {
    if rng.gen_bool(0.2) {
        let duration = 150.0 * rng.gen::<f32>();
        let fadeout_duration = 10.0;
        let fadein_duration = 5.0;
        println!(
            "Starting lucid music. Duration={duration} at {}",
            chrono::Local::now(),
        );
        match random_alarm_sound(Path::new("./sounds/lucid")) {
            Ok(path) => {
                dbg!(&path);
                crate::alarm::play_audio(
                    &path,
                    |t| {
                        let volume = lucid_music_volume.get().unwrap() as f32 / 100.0;
                        let v = volume
                            * fadein(t, fadein_duration)
                            * fadeout(t - (duration - fadeout_duration), fadeout_duration);
                        if t < duration {
                            Some(v)
                        } else {
                            None
                        }
                    },
                    true,
                );
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
    } else {
        let duration = 500.0;
        println!("Starting lucid effects at {}.", chrono::Local::now());
        match random_alarm_sound(Path::new("./sounds/lucid_sfx")) {
            Ok(path) => {
                dbg!(&path);
                crate::alarm::play_audio(
                    &path,
                    |t| {
                        let volume = lucid_sfx_volume.get().unwrap() as f32 / 100.0;
                        let v = volume;
                        if t < duration {
                            Some(v)
                        } else {
                            None
                        }
                    },
                    false,
                );
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
        println!("Lucid effects ended");
    }
}

pub async fn start_lucid_effects(
    alarm_state: AlarmState,
    force_start: bool,
    lucid_music_volume: Arc<SyncedContainer<i32>>,
    lucid_sfx_volume: Arc<SyncedContainer<i32>>,
    is_user_in_bed: Arc<SyncedContainer<bool>>,
    is_significant_movement_in_bed: Arc<SyncedContainer<bool>>,
) {
    let sleeping_start_time_data = Arc::new(Mutex::new(None));
    tokio::spawn(monitor_sleeping_duration(
        alarm_state.clone(),
        sleeping_start_time_data.clone(),
        is_user_in_bed.clone(),
        is_significant_movement_in_bed.clone(),
    ));

    let minimum_sleeping_time = Duration::from_secs(60 * 90);
    let period_secs = 60.0 * 60.0;
    let mut rng = rand::rngs::StdRng::from_entropy();

    loop {
        let time = Duration::from_secs_f64(rng.gen::<f64>() * period_secs);

        if !force_start {
            tokio::time::sleep(time).await;
        }

        let tries = 20;
        for i in 0..tries {
            let should_start = should_start_lucid_sounds2(
                alarm_state.clone(),
                &sleeping_start_time_data,
                is_user_in_bed.clone(),
                is_significant_movement_in_bed.clone(),
                minimum_sleeping_time,
                i < tries - 1, // Require movement, unless it's the last try
            )
            .await;
            dbg!(should_start);

            if should_start || force_start {
                play_lucid_sounds(&mut rng, &lucid_music_volume, &lucid_sfx_volume);
                break;
            }

            tokio::time::sleep(Duration::from_secs_f64(60.0f64.min(period_secs))).await;
        }
    }
}
