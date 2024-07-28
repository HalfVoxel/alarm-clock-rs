use linux_embedded_hal::{Delay, I2CError, I2cdev};
use mpu6050::*;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub struct Accelerometer {
    mpu: Mpu6050<I2cdev>,
}

#[derive(Debug, Clone)]
pub struct AccelerometerData {
    pub acc: (f32, f32, f32),
    pub gyro: (f32, f32, f32),
    pub temp: f32,
}

impl AccelerometerData {
    pub fn mean(data: &[AccelerometerData]) -> AccelerometerData {
        let mut acc = (0.0, 0.0, 0.0);
        let mut gyro = (0.0, 0.0, 0.0);
        let mut temp = 0.0;
        for d in data {
            acc.0 += d.acc.0;
            acc.1 += d.acc.1;
            acc.2 += d.acc.2;
            gyro.0 += d.gyro.0;
            gyro.1 += d.gyro.1;
            gyro.2 += d.gyro.2;
            temp += d.temp;
        }
        let len = data.len() as f32;
        AccelerometerData {
            acc: (acc.0 / len, acc.1 / len, acc.2 / len),
            gyro: (gyro.0 / len, gyro.1 / len, gyro.2 / len),
            temp: temp / len,
        }
    }
}

impl Accelerometer {
    pub fn new() -> Result<Self, Mpu6050Error<I2CError>> {
        let i2c = I2cdev::new("/dev/i2c-1").map_err(|e| Mpu6050Error::I2c(I2CError::from(e)))?;
        let mut delay = Delay;
        let mut mpu = Mpu6050::new(i2c);
        mpu.init(&mut delay)?;
        mpu.set_clock_source(device::CLKSEL::GXAXIS)?;
        Ok(Accelerometer { mpu })
    }

    pub fn get_data(&mut self) -> Result<AccelerometerData, Mpu6050Error<I2CError>> {
        // get accelerometer data, scaled with sensitivity
        let acc = self.mpu.get_acc()?;

        // get gyro data, scaled with sensitivity
        let gyro = self.mpu.get_gyro()?;

        // get sensor temp
        let temp = self.mpu.get_temp()?;

        Ok(AccelerometerData {
            acc: (acc.x, acc.y, acc.z),
            gyro: (gyro.x, gyro.y, gyro.z),
            temp,
        })
    }
}

pub struct SleepMonitor {
    rolling_data: Vec<AccelerometerData>,
    times: Vec<Instant>,
    rolling_delta_magn: Vec<f32>,
    max_memory: Duration,
    is_user_in_bed: Arc<mqtt_sync::SyncedContainer<bool>>,
}

impl SleepMonitor {
    pub fn new(
        max_memory: Duration,
        is_user_in_bed: Arc<mqtt_sync::SyncedContainer<bool>>,
    ) -> Self {
        SleepMonitor {
            rolling_data: vec![],
            times: vec![],
            rolling_delta_magn: vec![],
            max_memory,
            is_user_in_bed,
        }
    }

    pub fn push(&mut self, data: AccelerometerData) {
        let prev = self.rolling_data.last().cloned();
        self.rolling_data.push(data.clone());
        self.times.push(Instant::now());

        if let Some(prev) = prev {
            let delta = (
                data.acc.0 - prev.acc.0,
                data.acc.1 - prev.acc.1,
                data.acc.2 - prev.acc.2,
            );
            let delta_magn = (delta.0.powi(2) + delta.1.powi(2) + delta.2.powi(2)).sqrt();
            self.rolling_delta_magn.push(delta_magn);
        }

        if self.times.first().unwrap().elapsed() > self.max_memory {
            self.rolling_data.remove(0);
            self.times.remove(0);
            if self.rolling_delta_magn.len() > self.times.len() {
                self.rolling_delta_magn.remove(0);
            }
        }

        futures::executor::block_on(async {
            self.is_user_in_bed.set(self.is_present()).await;
        });
    }

    pub fn is_significant_movement(&self) -> bool {
        const MOVEMENT_THRESHOLD: f32 = 0.02;
        const MOVEMENT_THRESHOLD_SAMPLES: i32 = 2;

        let mut cnt = 0;
        for &v in &self.rolling_delta_magn {
            if v > MOVEMENT_THRESHOLD {
                cnt += 1;
            }
        }

        cnt > MOVEMENT_THRESHOLD_SAMPLES
    }

    /// True if the user is present in bed
    pub fn is_present(&self) -> bool {
        const NOISE_THRESHOLD: f32 = 0.015;
        const NOISE_THRESHOLD_SAMPLES: i32 = 1;

        let mut cnt = 0;
        for &v in &self.rolling_delta_magn {
            if v > NOISE_THRESHOLD {
                cnt += 1;
            }
        }

        cnt > NOISE_THRESHOLD_SAMPLES
    }
}
